use super::*;
use crate::Limits;
use crate::dtd::DtdErrorKind;
use crate::dtd_frames::{EntityResolution, FrameBoundary};
use crate::parser::ParserState;

fn state(max_owned_bytes: usize) -> ParserState<'static> {
    ParserState::new(
        Limits {
            max_owned_bytes,
            ..Limits::default()
        },
        0,
    )
}

fn borrowed_error(raw: &str, limits: ControllerLimits) -> ControllerError {
    let mut state = state(usize::MAX);
    let mut controller = DtdController::stable_utf8(raw, 0, 0, limits, false).unwrap();
    loop {
        match controller.advance_borrowed(state.owned_budget()) {
            Ok(_) => while controller.take_current().is_some() {},
            Err(error) => break error,
        }
    }
}

fn streaming_error(raw: &str, limits: ControllerLimits) -> ControllerError {
    let mut state = state(usize::MAX);
    let mut controller = DtdController::new(SourceEncoding::Utf8, 0, limits, false);
    let mut cursor = 0;
    loop {
        match controller.advance(&raw.as_bytes()[cursor..], state.owned_budget()) {
            Ok(consumed) => {
                cursor += consumed;
                while controller.take_current().is_some() {}
            }
            Err(error) => break error,
        }
    }
}

#[test]
fn stable_source_constructor_checks_range_overflow_bounds_and_utf8_boundaries() {
    let limits = ControllerLimits::default();
    assert_eq!(
        DtdController::stable_utf8("ab", usize::MAX, usize::MAX, limits, false)
            .err()
            .unwrap()
            .kind,
        ControllerErrorKind::PositionOverflow
    );
    assert_eq!(
        DtdController::stable_utf8("abc", 10, 9, limits, false)
            .err()
            .unwrap()
            .kind,
        ControllerErrorKind::InvalidState
    );
    assert_eq!(
        DtdController::stable_utf8("abc", 10, 14, limits, false)
            .err()
            .unwrap()
            .kind,
        ControllerErrorKind::InvalidState
    );
    assert_eq!(
        DtdController::stable_utf8("π<!DOCTYPE r>", 20, 21, limits, false)
            .err()
            .unwrap()
            .kind,
        ControllerErrorKind::InvalidState
    );
    assert!(DtdController::stable_utf8("π<!DOCTYPE r>", 20, 22, limits, false).is_ok());
}

#[test]
fn each_input_mode_rejects_the_other_modes_driver() {
    let mut state = state(usize::MAX);
    let mut streaming =
        DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
    assert_eq!(
        streaming
            .advance_borrowed(state.owned_budget())
            .unwrap_err()
            .kind,
        ControllerErrorKind::InvalidState
    );

    let mut borrowed =
        DtdController::stable_utf8("<!DOCTYPE r>", 0, 0, ControllerLimits::default(), false)
            .unwrap();
    assert_eq!(
        borrowed
            .advance(b"<!DOCTYPE foreign>", state.owned_budget())
            .unwrap_err()
            .kind,
        ControllerErrorKind::InvalidState
    );

    let mut borrowed =
        DtdController::stable_utf8("<!DOCTYPE r>", 0, 0, ControllerLimits::default(), false)
            .unwrap();
    assert_eq!(
        borrowed.finish_step(state.owned_budget()).unwrap_err().kind,
        ControllerErrorKind::InvalidState
    );
}

#[test]
fn borrowed_current_lends_the_stored_source_without_promotion() {
    let source = "xx<!DOCTYPE r><r/>";
    let mut state = state(0);
    let mut controller = DtdController::stable_utf8(
        source,
        40,
        42,
        ControllerLimits {
            max_owned_bytes: 0,
            ..ControllerLimits::default()
        },
        false,
    )
    .unwrap();
    assert_eq!(
        controller.advance_borrowed(state.owned_budget()).unwrap(),
        "<!DOCTYPE r>".len()
    );
    let (event, origin) = controller.current().unwrap();
    let DtdEvent::Begin(header) = event else {
        panic!("expected borrowed Begin")
    };
    assert_eq!(header.root_name(), "r");
    assert_eq!(origin.input_range(), Some(42..53));
    let expected_name = &source[source.find(" r").unwrap() + 1..source.find(" r").unwrap() + 2];
    assert_eq!(header.root_name().as_ptr(), expected_name.as_ptr());
    controller.release();
}

#[test]
fn stable_physical_units_borrow_exact_source_slices_and_leave_the_body() {
    let source = "π<!DOCTYPE 根 [<!--é--><?p v?><!ELEMENT 根 EMPTY>]><根/>";
    let source_start = 100;
    let dtd_start = source_start + "π".len();
    let body = source.find("<根/>").unwrap();
    let mut state = state(29);
    let limits = ControllerLimits {
        max_owned_bytes: 0,
        ..ControllerLimits::default()
    };
    let mut controller =
        DtdController::stable_utf8(source, source_start, dtd_start, limits, false).unwrap();
    let mut raw_units = Vec::new();

    while !controller.is_complete() {
        let before = controller.physical_position();
        let consumed = controller.advance_borrowed(state.owned_budget()).unwrap();
        let Some((event, origin)) = controller.take_current() else {
            assert!(consumed != 0 || controller.physical_position() != before);
            continue;
        };
        let unit = match event {
            TakenDtdEvent::Begin(unit) | TakenDtdEvent::Item { unit, .. } => Some(unit),
            TakenDtdEvent::End => None,
        };
        if let Some(unit) = unit {
            let range = origin.input_range().expect("physical unit");
            let expected = &source[range.start - source_start..range.end - source_start];
            assert_eq!(unit.raw(), expected);
            assert_eq!(unit.raw().as_ptr(), expected.as_ptr());
            raw_units.push(unit.raw().to_owned());
        }
    }

    assert_eq!(
        raw_units,
        [
            "<!DOCTYPE 根 [",
            "<!--é-->",
            "<?p v?>",
            "<!ELEMENT 根 EMPTY>"
        ]
    );
    assert_eq!(controller.physical_position(), source_start + body);
    assert_eq!(
        controller.doctype_range(),
        Some(dtd_start..source_start + body)
    );
    assert_eq!(controller.counters().owned_bytes, 0);
    assert_eq!(state.owned_budget().remaining_owned(), 29);
}

#[test]
fn borrowed_mode_still_owns_and_charges_generated_units() {
    let source = "<!DOCTYPE r [<!ENTITY % p '<!--x-->'>%p;]><r/>";
    let generated = "<!--x-->";
    let mut state = state(generated.len());
    let limits = ControllerLimits {
        max_owned_bytes: generated.len(),
        ..ControllerLimits::default()
    };
    let mut controller = DtdController::stable_utf8(source, 0, 0, limits, false).unwrap();
    let mut saw_generated = false;

    while !controller.is_complete() {
        controller.advance_borrowed(state.owned_budget()).unwrap();
        let Some((event, origin)) = controller.take_current() else {
            continue;
        };
        if let TakenDtdEvent::Item { unit, literals } = event
            && literals == LiteralSource::Replacement
        {
            assert_eq!(unit.raw(), generated);
            assert!(origin.input_range().is_none());
            let source_start = source.as_ptr() as usize;
            let source_end = source_start + source.len();
            let raw_start = unit.raw().as_ptr() as usize;
            assert!(raw_start < source_start || raw_start >= source_end);
            saw_generated = true;
        }
    }

    assert!(saw_generated);
    assert_eq!(controller.counters().owned_bytes, generated.len());
    assert_eq!(state.owned_budget().remaining_owned(), 0);
}

#[test]
fn grammar_and_family_errors_precede_conditional_copy_limits_in_both_modes() {
    let zero_copy = ControllerLimits {
        max_owned_bytes: 0,
        ..ControllerLimits::default()
    };
    for error in [
        borrowed_error("<!DOCTYPE >", zero_copy),
        streaming_error("<!DOCTYPE >", zero_copy),
    ] {
        assert_eq!(
            error.kind,
            ControllerErrorKind::Grammar(DtdErrorKind::InvalidName)
        );
        assert_eq!(error.physical_offset, 10);
    }

    let mut zero_shared = state(0);
    let mut borrowed =
        DtdController::stable_utf8("<!DOCTYPE >", 0, 0, ControllerLimits::default(), false)
            .unwrap();
    let borrowed = borrowed
        .advance_borrowed(zero_shared.owned_budget())
        .unwrap_err();
    let mut streaming =
        DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
    let streaming = streaming
        .advance(b"<!DOCTYPE >", zero_shared.owned_budget())
        .unwrap_err();
    for error in [borrowed, streaming] {
        assert_eq!(
            error.kind,
            ControllerErrorKind::Grammar(DtdErrorKind::InvalidName)
        );
    }

    let nested = "<!DOCTYPE r [<!ENTITY % p \"<!DOCTYPE x>\">%p;]><r/>";
    let borrowed = borrowed_error(nested, zero_copy);
    assert_eq!(borrowed.kind, ControllerErrorKind::InvalidUnitFamily);

    // Streaming must retain the 13-byte header and 30-byte entity declaration,
    // but has no quota left for the generated nested header.
    let streaming_limits = ControllerLimits {
        max_owned_bytes: "<!DOCTYPE r [".len() + "<!ENTITY % p \"<!DOCTYPE x>\">".len(),
        ..ControllerLimits::default()
    };
    let streaming = streaming_error(nested, streaming_limits);
    assert_eq!(streaming.kind, ControllerErrorKind::InvalidUnitFamily);
}

#[test]
fn stable_window_eof_is_authoritative_and_reports_incomplete_input() {
    let error = borrowed_error(
        "<!DOCTYPE r [<!ELEMENT r EMPTY>",
        ControllerLimits::default(),
    );
    assert_eq!(
        error.kind,
        ControllerErrorKind::Scan(ScanErrorKind::UnexpectedEof)
    );
    assert_eq!(error.physical_offset, 31);
}

fn complete_with_taken_events<'input>(
    controller: &mut BorrowedDtdController<'input>,
    state: &mut ParserState<'_>,
) {
    while !controller.is_complete() {
        controller.advance_borrowed(state.owned_budget()).unwrap();
        while controller.take_current().is_some() {}
    }
}

#[test]
fn copy_accounting_differs_only_for_raw_units_that_require_ownership() {
    let source = "<!DOCTYPE r [<!--x-->]><r/>";
    let mut streaming_state = state(100);
    let mut streaming =
        DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
    let mut cursor = 0;
    while !streaming.is_complete() {
        cursor += streaming
            .advance(&source.as_bytes()[cursor..], streaming_state.owned_budget())
            .unwrap();
        while streaming.take_current().is_some() {}
    }

    let mut borrowed_state = state(100);
    let mut borrowed =
        DtdController::stable_utf8(source, 0, 0, ControllerLimits::default(), false).unwrap();
    complete_with_taken_events(&mut borrowed, &mut borrowed_state);
    let owned = streaming.counters().owned_bytes;
    assert_eq!(owned, "<!DOCTYPE r [".len() + "<!--x-->".len());
    assert_eq!(borrowed.counters().owned_bytes, 0);
    assert_eq!(streaming.counters().work, borrowed.counters().work + owned);
    assert_eq!(
        streaming.counters().scanned_bytes,
        borrowed.counters().scanned_bytes
    );
    assert_eq!(
        streaming.counters().physical_bytes,
        borrowed.counters().physical_bytes
    );
    assert_eq!(
        streaming_state.owned_budget().remaining_owned(),
        100 - owned
    );
    assert_eq!(borrowed_state.owned_budget().remaining_owned(), 100);
}

#[test]
fn handoff_requires_released_end_and_preserves_catalog_frames_nonce_and_counters() {
    let input = "<!DOCTYPE r [<!ENTITY g 'v'><!ENTITY % p '<!--x-->'>%p;]>";
    let mut premature_state = state(usize::MAX);
    let mut premature =
        DtdController::stable_utf8(input, 0, 0, ControllerLimits::default(), false).unwrap();
    premature
        .advance_borrowed(premature_state.owned_budget())
        .unwrap();
    assert_eq!(
        premature.into_body_state().err().unwrap().kind,
        ControllerErrorKind::InvalidState
    );

    let mut state = state(usize::MAX);
    let mut controller =
        DtdController::stable_utf8(input, 0, 0, ControllerLimits::default(), false).unwrap();
    complete_with_taken_events(&mut controller, &mut state);
    let counters = controller.counters();
    let limits = ControllerLimits::default();
    let mut body = controller.into_body_state().unwrap();

    assert_eq!(body.counters(), counters);
    assert_eq!(body.limits().max_work, limits.max_work);
    assert_eq!(body.physical_position(), input.len());
    assert_eq!(body.decoded_position(), input.len());
    assert_eq!(body.resume_position(), (input.len(), input.len()));
    assert_eq!(body.doctype_range(), 0..input.len());
    assert_eq!(body.frames().counters().reference_attempts, 1);
    let (id, replacement_len) = match body.catalog().lookup_general("g") {
        EntityLookup::Internal { id, replacement } => (id, replacement.len()),
        other => panic!("unexpected general binding: {other:?}"),
    };
    assert_eq!(replacement_len, 1);
    let invocation = body
        .frames_mut()
        .enter_physical(
            EntityKind::General,
            EntityResolution::Internal {
                kind: EntityKind::General,
                declaration: EntityDeclarationId::from_index(id.index()),
                replacement_len,
            },
            80..83,
            FrameBoundary::Content { element_depth: 1 },
        )
        .unwrap();
    assert_eq!(invocation.get(), 2);
}

#[test]
fn handoff_keeps_checked_decoded_and_physical_resume_positions() {
    let input = b"<!DOCTYPE r>";
    let mut state = state(usize::MAX);
    let mut controller = DtdController::new_at(
        SourceEncoding::Utf8,
        50,
        7,
        ControllerLimits::default(),
        false,
    );
    let mut cursor = 0;
    while !controller.is_complete() {
        cursor += controller
            .advance(&input[cursor..], state.owned_budget())
            .unwrap();
        while controller.take_current().is_some() {}
    }
    let body = controller.into_body_state().unwrap();
    assert_eq!(body.resume_position(), (7 + input.len(), 50 + input.len()));

    let mut overflow = DtdController::new_at(
        SourceEncoding::Utf8,
        0,
        usize::MAX,
        ControllerLimits::default(),
        false,
    );
    let mut cursor = 0;
    while !overflow.is_complete() {
        cursor += overflow
            .advance(&input[cursor..], state.owned_budget())
            .unwrap();
        while overflow.take_current().is_some() {}
    }
    assert_eq!(
        overflow.into_body_state().err().unwrap().kind,
        ControllerErrorKind::PositionOverflow
    );
}

#[test]
fn failed_controller_handoff_returns_the_original_sticky_error() {
    let raw = "<!DOCTYPE >";
    let mut state = state(usize::MAX);
    let mut controller =
        DtdController::stable_utf8(raw, 0, 0, ControllerLimits::default(), false).unwrap();
    let original = controller
        .advance_borrowed(state.owned_budget())
        .unwrap_err();
    assert_eq!(
        original.kind,
        ControllerErrorKind::Grammar(DtdErrorKind::InvalidName)
    );
    assert_eq!(controller.into_body_state().err().unwrap(), original);
}
