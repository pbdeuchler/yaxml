//! Independent borrowed-window, event ownership and handoff contracts.
use super::*;
use crate::Limits;
use crate::parser::ParserState;

fn drain_borrowed<'a>(
    c: &mut BorrowedDtdController<'a>,
    budget: &mut Budget,
) -> Result<Vec<(TakenDtdEvent<'a>, FragmentOrigin)>, ControllerError> {
    let mut out = Vec::new();
    while !c.is_complete() {
        let consumed = c.advance_borrowed(budget)?;
        if let Some(event) = c.take_current() {
            out.push(event)
        } else {
            assert!(consumed > 0, "unreported lack of progress")
        }
    }
    Ok(out)
}
fn drain_stream(
    c: &mut DtdController,
    bytes: &[u8],
    budget: &mut Budget,
) -> Result<Vec<(TakenDtdEvent<'static>, FragmentOrigin)>, ControllerError> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while !c.is_complete() {
        let consumed = if cursor < bytes.len() {
            c.advance(&bytes[cursor..], budget)?
        } else {
            c.finish_step(budget)?;
            0
        };
        cursor += consumed;
        if let Some(event) = c.take_current() {
            out.push(event)
        } else {
            assert!(consumed > 0)
        }
    }
    Ok(out)
}
fn unit<'view, 'input>(event: &'view TakenDtdEvent<'input>) -> Option<&'view DtdUnit<'input>> {
    match event {
        TakenDtdEvent::Begin(unit) | TakenDtdEvent::Item { unit, .. } => Some(unit),
        TakenDtdEvent::End => None,
    }
}
fn bad_handoff(c: DtdControllerCore<'_>, kind: ControllerErrorKind) {
    match c.into_body_state() {
        Err(error) => assert_eq!(error.kind, kind),
        Ok(_) => panic!("invalid handoff succeeded"),
    }
}
fn completed() -> DtdController {
    let mut state = ParserState::new(Limits::default(), 0);
    let mut c = DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
    drain_stream(
        &mut c,
        b"<!DOCTYPE r [<!ENTITY g 'v'>]>",
        state.owned_budget(),
    )
    .unwrap();
    c
}

#[test]
fn spec_physical_units_borrow_the_exact_source_window_with_zero_raw_copy_budget() {
    let source = String::from("prefix<!DOCTYPE 根 [<!--x--><?p d?><!ELEMENT 根 EMPTY>]><根/>");
    let mut state = ParserState::new(
        Limits {
            max_owned_bytes: 0,
            ..Limits::default()
        },
        0,
    );
    let limits = ControllerLimits {
        max_owned_bytes: 0,
        ..ControllerLimits::default()
    };
    let mut c = DtdController::stable_utf8(&source, 100, 106, limits, false).unwrap();
    let events = drain_borrowed(&mut c, state.owned_budget()).unwrap();
    assert_eq!(events.len(), 5);
    for (event, origin) in &events {
        if let Some(unit) = unit(event) {
            // No pointer arithmetic dereference: addresses merely establish the
            // retained unit's actual stable source slice identity.
            let at = source.find(unit.raw()).unwrap();
            assert_eq!(unit.raw().as_ptr(), source[at..].as_ptr());
            if matches!(event, TakenDtdEvent::Item { .. }) {
                assert_eq!(
                    origin.input_range(),
                    Some(100 + at..100 + at + unit.raw().len())
                );
            }
        }
    }
    assert_eq!(c.counters().owned_bytes, 0);
    assert_eq!(state.owned_budget().remaining_owned(), 0);
    let end = source.find("<根/>").unwrap() + 100;
    assert_eq!(c.physical_position(), end);
    assert_eq!(c.doctype_range(), Some(106..end));
    assert_eq!(&source[end - 100..], "<根/>");
    let TakenDtdEvent::Begin(header) = &events[0].0 else {
        panic!("header")
    };
    assert!(matches!(header.view(),DtdUnitView::Header(view) if view.root_name()=="根"));
}

#[test]
fn spec_bom_and_no_subset_header_retain_the_real_delimiter_separately() {
    let source = String::from("\u{feff}<!DOCTYPE r><r/>");
    let mut state = ParserState::new(Limits::default(), 0);
    let mut c =
        DtdController::stable_utf8(&source, 50, 53, ControllerLimits::default(), false).unwrap();
    assert_eq!(c.advance_borrowed(state.owned_budget()), Ok(12));
    let (header, begin) = c.take_current().unwrap();
    assert_eq!(unit(&header).unwrap().raw(), "<!DOCTYPE r>");
    assert_eq!(unit(&header).unwrap().raw().as_ptr(), source[3..].as_ptr());
    assert_eq!(begin.input_range(), Some(53..64));
    assert!(!c.is_complete());
    let (end, origin) = c.take_current().unwrap();
    assert!(matches!(end, TakenDtdEvent::End));
    assert_eq!(origin.input_range(), Some(64..65));
    assert!(c.is_complete());
    assert!(c.take_current().is_none());
    assert_eq!(c.physical_position(), 65);
    let body = c.into_body_state().ok().unwrap();
    assert_eq!(body.resume_position(), (65, 65));
    assert_eq!(body.doctype_range(), 53..65);
    assert_eq!(unit(&header).unwrap().raw(), "<!DOCTYPE r>"); // after controller destruction
}

#[test]
fn spec_stable_window_construction_rejects_overflow_outside_and_scalar_interior() {
    for (source, start, physical, kind) in [
        ("é<!DOCTYPE r>", 10, 9, ControllerErrorKind::InvalidState),
        ("é<!DOCTYPE r>", 10, 11, ControllerErrorKind::InvalidState),
        ("é<!DOCTYPE r>", 10, 30, ControllerErrorKind::InvalidState),
        (
            "ab",
            usize::MAX,
            usize::MAX,
            ControllerErrorKind::PositionOverflow,
        ),
    ] {
        match DtdController::stable_utf8(
            source,
            start,
            physical,
            ControllerLimits::default(),
            false,
        ) {
            Err(error) => {
                assert_eq!(error.kind, kind);
                assert_eq!(error.physical_offset, physical)
            }
            Ok(_) => panic!("invalid window accepted"),
        }
    }
    let mut state = ParserState::new(Limits::default(), 0);
    let mut empty =
        DtdController::stable_utf8("é", 10, 12, ControllerLimits::default(), false).unwrap();
    let error = empty.advance_borrowed(state.owned_budget()).unwrap_err();
    assert_eq!(
        error.kind,
        ControllerErrorKind::Scan(ScanErrorKind::UnexpectedEof)
    );
    assert_eq!(error.physical_offset, 12);
}

#[test]
fn spec_input_mode_is_sealed_and_failure_sticks_without_alternate_source_bytes() {
    let mut state = ParserState::new(Limits::default(), 0);
    let mut stable =
        DtdController::stable_utf8("<!DOCTYPE r>", 0, 0, ControllerLimits::default(), false)
            .unwrap();
    let error = stable
        .advance(b"<!DOCTYPE other>", state.owned_budget())
        .unwrap_err();
    assert_eq!(error.kind, ControllerErrorKind::InvalidState);
    assert_eq!(stable.physical_position(), 0);
    assert_eq!(stable.advance_borrowed(state.owned_budget()), Err(error));
    assert!(stable.current().is_none());
    let mut stream =
        DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
    let error = stream.advance_borrowed(state.owned_budget()).unwrap_err();
    assert_eq!(error.kind, ControllerErrorKind::InvalidState);
    assert_eq!(
        stream.advance(b"<!DOCTYPE r>", state.owned_budget()),
        Err(error)
    );
    let mut stable =
        DtdController::stable_utf8("<!DOCTYPE r>", 0, 0, ControllerLimits::default(), false)
            .unwrap();
    assert_eq!(
        stable.finish_step(state.owned_budget()).unwrap_err().kind,
        ControllerErrorKind::InvalidState
    );
}

#[test]
fn spec_generated_units_own_raw_and_keep_replacement_eol_phase_after_controller_drop() {
    let input = String::from("<!DOCTYPE r [<!ENTITY % p '<!--a&#13;b-->'>%p;]><r/>");
    let mut state = ParserState::new(
        Limits {
            max_owned_bytes: 10,
            ..Limits::default()
        },
        0,
    );
    let (taken, counts, reference) = {
        let mut c = DtdController::stable_utf8(
            &input,
            0,
            0,
            ControllerLimits {
                max_owned_bytes: 10,
                ..ControllerLimits::default()
            },
            false,
        )
        .unwrap();
        let events = drain_borrowed(&mut c, state.owned_budget()).unwrap();
        let event = events
            .into_iter()
            .find(|(event, _)| {
                matches!(
                    event,
                    TakenDtdEvent::Item {
                        literals: LiteralSource::Replacement,
                        ..
                    }
                )
            })
            .unwrap();
        let EntityLookup::Internal { replacement, .. } = c.catalog().lookup_parameter("p") else {
            panic!("PE")
        };
        assert_eq!(replacement, "<!--a\rb-->");
        assert_ne!(unit(&event.0).unwrap().raw().as_ptr(), replacement.as_ptr());
        let reference = input.find("%p;").unwrap();
        (event, c.counters(), reference)
    };
    let TakenDtdEvent::Item { unit, literals } = taken.0 else {
        panic!("generated item")
    };
    assert_eq!(literals, LiteralSource::Replacement);
    assert_eq!(unit.raw(), "<!--a\rb-->");
    assert_eq!(counts.owned_bytes, 10);
    assert_eq!(state.owned_budget().remaining_owned(), 0);
    assert_eq!(taken.1.input_range(), None);
    assert_eq!(taken.1.replacement_range(), Some(0..10));
    assert_eq!(taken.1.physical_context(), reference..reference + 3);
    let pointer = unit.raw().as_ptr() as usize;
    assert!(!(input.as_ptr() as usize..input.as_ptr() as usize + input.len()).contains(&pointer));
}

#[test]
fn spec_owned_take_current_outlives_stream_source_and_controller() {
    let event: TakenDtdEvent<'static> = {
        let input = String::from("<!DOCTYPE r [<!--retained-->]>");
        let mut state = ParserState::new(Limits::default(), 0);
        let mut c = DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
        let mut events = drain_stream(&mut c, input.as_bytes(), state.owned_budget()).unwrap();
        events.remove(1).0
    };
    let TakenDtdEvent::Item { unit, literals } = event else {
        panic!("item")
    };
    assert_eq!(unit.raw(), "<!--retained-->");
    assert_eq!(literals, LiteralSource::Physical);
    assert!(matches!(unit.view(), DtdUnitView::Comment("retained")));
}

#[test]
fn spec_raw_copy_ceilings_differ_only_by_actual_ownership_requests() {
    let text = "<!DOCTYPE r [<!--x--><?p d?><!ELEMENT r EMPTY>]>";
    let mut zero = ParserState::new(
        Limits {
            max_owned_bytes: 0,
            ..Limits::default()
        },
        0,
    );
    let mut borrowed =
        DtdController::stable_utf8(text, 0, 0, ControllerLimits::default(), false).unwrap();
    drain_borrowed(&mut borrowed, zero.owned_budget()).unwrap();
    let baseline = borrowed.counters();
    for ceiling in [45, 46] {
        let mut state = ParserState::new(
            Limits {
                max_owned_bytes: ceiling,
                ..Limits::default()
            },
            0,
        );
        let mut c = DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
        let result = drain_stream(&mut c, text.as_bytes(), state.owned_budget());
        if ceiling == 46 {
            result.unwrap();
            assert_eq!(c.counters().owned_bytes, 46);
            assert_eq!(c.counters().work, baseline.work + 46);
            assert_eq!(c.counters().scanned_bytes, baseline.scanned_bytes);
            assert_eq!(c.counters().physical_bytes, baseline.physical_bytes);
            assert_eq!(state.owned_budget().remaining_owned(), 0)
        } else {
            assert_eq!(
                result.unwrap_err().kind,
                ControllerErrorKind::Ownership(ErrorKind::LimitExceeded(crate::Limit::OwnedBytes))
            );
            assert_eq!(c.counters().owned_bytes, 28);
            assert!(c.current().is_none())
        }
    }
    let generated = "<!DOCTYPE r [<!ENTITY % p '<!--a&#13;b-->'>%p;]>";
    for shared_limit in [false, true] {
        let mut state = ParserState::new(
            Limits {
                max_owned_bytes: if shared_limit { 9 } else { 10 },
                ..Limits::default()
            },
            0,
        );
        let limits = ControllerLimits {
            max_owned_bytes: if shared_limit { 10 } else { 9 },
            ..ControllerLimits::default()
        };
        let mut c = DtdController::stable_utf8(generated, 0, 0, limits, false).unwrap();
        let error = drain_borrowed(&mut c, state.owned_budget()).unwrap_err();
        assert_eq!(
            error.kind,
            if shared_limit {
                ControllerErrorKind::Ownership(ErrorKind::LimitExceeded(crate::Limit::OwnedBytes))
            } else {
                ControllerErrorKind::Limit(ControllerLimit::OwnedBytes)
            }
        );
        assert_eq!(c.counters().owned_bytes, 0);
        assert!(c.current().is_none());
        assert_eq!(c.advance_borrowed(state.owned_budget()), Err(error));
    }
}

#[test]
fn spec_grammar_family_failures_precede_copy_limits_in_both_modes() {
    for raw in [
        "<!DOCTYPE r [<!ELEMENT 1 EMPTY>]>",
        "<!DOCTYPE r [<!DOCTYPE r>]>",
    ] {
        let limits = ControllerLimits {
            max_owned_bytes: 0,
            ..ControllerLimits::default()
        };
        let mut state = ParserState::new(
            Limits {
                max_owned_bytes: 0,
                ..Limits::default()
            },
            0,
        );
        let mut borrowed = DtdController::stable_utf8(raw, 0, 0, limits, false).unwrap();
        let error = drain_borrowed(&mut borrowed, state.owned_budget()).unwrap_err();
        assert!(matches!(
            error.kind,
            ControllerErrorKind::Grammar(_) | ControllerErrorKind::InvalidUnitFamily
        ));
        // The physical header needs ownership in streaming mode; permit exactly
        // its13 bytes, leaving zero for the malformed declaration.
        let limits = ControllerLimits {
            max_owned_bytes: 13,
            ..ControllerLimits::default()
        };
        let mut state = ParserState::new(
            Limits {
                max_owned_bytes: 13,
                ..Limits::default()
            },
            0,
        );
        let mut owned = DtdController::new(SourceEncoding::Utf8, 0, limits, false);
        let other = drain_stream(&mut owned, raw.as_bytes(), state.owned_budget()).unwrap_err();
        assert_eq!(other, error);
    }
}

#[test]
fn spec_handoff_rejects_unread_pending_failed_and_active_states() {
    let mut state = ParserState::new(Limits::default(), 0);
    bad_handoff(
        DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false),
        ControllerErrorKind::InvalidState,
    );
    let mut c = DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
    c.advance(b"<!DOCTYPE r>", state.owned_budget()).unwrap();
    bad_handoff(c, ControllerErrorKind::InvalidState);
    let mut c = DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
    c.advance(b"<!DOCTYPE r>", state.owned_budget()).unwrap();
    c.take_current().unwrap();
    bad_handoff(c, ControllerErrorKind::InvalidState);
    let mut c = DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
    let error = c.advance(b"bad", state.owned_budget()).unwrap_err();
    match c.into_body_state() {
        Err(actual) => assert_eq!(actual, error),
        Ok(_) => panic!("failed handoff"),
    }
    // Independently exercise the defensive active/frame guards after completion,
    // using valid private frame entry rather than forging a public parsed name.
    for mirror in [false, true] {
        let mut c = completed();
        let EntityLookup::Internal { id, replacement } = c.catalog().lookup_general("g") else {
            panic!("g")
        };
        let len = replacement.len();
        if mirror {
            c.active.push(id)
        } else {
            c.frames
                .enter_physical(
                    EntityKind::General,
                    EntityResolution::Internal {
                        kind: EntityKind::General,
                        declaration: EntityDeclarationId::from_index(id.index()),
                        replacement_len: len,
                    },
                    30..33,
                    FrameBoundary::Content { element_depth: 1 },
                )
                .unwrap();
        }
        bad_handoff(c, ControllerErrorKind::InvalidState);
    }
}

#[test]
fn spec_handoff_preserves_catalog_nonce_counters_limits_and_finite_history() {
    let text = "<!DOCTYPE r [<!ENTITY % p '<!--x-->'><!ENTITY g 'v'>%p;%p;]>";
    let limits = ControllerLimits {
        max_physical_bytes: 321,
        max_work: 9876,
        max_owned_bytes: 123,
        frames: FrameLimits {
            max_reference_attempts: 3,
            max_expanded_bytes: 77,
            ..FrameLimits::default()
        },
        ..ControllerLimits::default()
    };
    let mut state = ParserState::new(Limits::default(), 0);
    let mut c = DtdController::stable_utf8(text, 40, 40, limits, false).unwrap();
    let events = drain_borrowed(&mut c, state.owned_budget()).unwrap();
    let last_invocation = events
        .iter()
        .filter_map(|(_, origin)| match origin {
            FragmentOrigin::Replacement { invocation, .. } => Some(invocation.get()),
            _ => None,
        })
        .max()
        .unwrap();
    assert_eq!(last_invocation, 2);
    let counters = c.counters();
    let usage = c.catalog().usage();
    let frame_counters = c.frames.counters();
    let mut body = c.into_body_state().ok().unwrap();
    assert_eq!(body.counters(), counters);
    assert_eq!(body.catalog().usage(), usage);
    assert_eq!(body.frames().counters(), frame_counters);
    assert_eq!(body.frames().depth(), 0);
    assert_eq!(body.resume_position(), (40 + text.len(), 40 + text.len()));
    assert_eq!(body.doctype_range(), 40..40 + text.len());
    assert_eq!(body.limits().max_physical_bytes, 321);
    assert_eq!(body.limits().max_work, 9876);
    assert_eq!(body.limits().max_owned_bytes, 123);
    assert_eq!(body.limits().frames.max_reference_attempts, 3);
    assert_eq!(body.limits().frames.max_expanded_bytes, 77);
    let EntityLookup::Internal { id, replacement } = body.catalog().lookup_general("g") else {
        panic!("g")
    };
    let len = replacement.len();
    let declaration = EntityDeclarationId::from_index(id.index());
    let invocation = body
        .frames_mut()
        .enter_physical(
            EntityKind::General,
            EntityResolution::Internal {
                kind: EntityKind::General,
                declaration,
                replacement_len: len,
            },
            200..203,
            FrameBoundary::Content { element_depth: 1 },
        )
        .unwrap();
    assert_eq!(invocation.get(), 3);
    body.frames_mut()
        .advance_current(declaration, "v", 1)
        .unwrap();
    body.frames_mut()
        .finish_current(FrameBoundary::Content { element_depth: 1 })
        .unwrap();
    assert_eq!(body.frames().depth(), 0);
    assert_eq!(body.frames().counters().reference_attempts, 3);
    assert_eq!(
        body.frames().counters().expanded_bytes,
        frame_counters.expanded_bytes + 1
    );
    assert_eq!(body.frames().counters().work, frame_counters.work + 2);
    let error = body
        .frames_mut()
        .enter_physical(
            EntityKind::General,
            EntityResolution::Internal {
                kind: EntityKind::General,
                declaration,
                replacement_len: 1,
            },
            203..206,
            FrameBoundary::Content { element_depth: 1 },
        )
        .unwrap_err();
    assert_eq!(
        error.kind,
        FrameErrorKind::LimitExceeded(crate::dtd_frames::FrameLimit::ReferenceAttempts)
    );
}

#[test]
fn spec_utf16_handoff_retains_distinct_decoded_and_physical_resume_coordinates() {
    let text = "<!DOCTYPE 根 [<!--🦀-->]>";
    for little_endian in [false, true] {
        let mut state = ParserState::new(Limits::default(), 0);
        let mut c = DtdController::new_at(
            SourceEncoding::Utf16 {
                little_endian,
                bom: true,
            },
            42,
            19,
            ControllerLimits::default(),
            false,
        );
        drain_stream(&mut c, text.as_bytes(), state.owned_budget()).unwrap();
        let body = c.into_body_state().ok().unwrap();
        assert_eq!(
            body.resume_position(),
            (19 + text.len(), 42 + text.encode_utf16().count() * 2)
        );
        assert_ne!(body.decoded_position(), body.physical_position());
    }
    let mut state = ParserState::new(Limits::default(), 0);
    let mut c = DtdController::new_at(
        SourceEncoding::Utf8,
        0,
        usize::MAX,
        ControllerLimits::default(),
        false,
    );
    drain_stream(&mut c, b"<!DOCTYPE r>", state.owned_budget()).unwrap();
    bad_handoff(c, ControllerErrorKind::PositionOverflow);
}

#[test]
fn spec_stable_range_adapter_rejects_invalid_local_boundaries_without_effects() {
    let source = "é<!DOCTYPE r>";
    let c =
        DtdController::stable_utf8(source, 100, 102, ControllerLimits::default(), false).unwrap();
    for range in [99..102, 100..101, 101..102, 100..200] {
        let origin = FragmentOrigin::input(range).unwrap();
        let error = c.stable_physical_raw(origin).unwrap_err();
        assert_eq!(error.kind, ControllerErrorKind::InvalidState);
        assert_eq!(c.physical_position(), 102);
        assert_eq!(c.counters(), ControllerCounters::default());
        assert!(c.current().is_none());
    }
    let origin = FragmentOrigin::input(102..114).unwrap();
    let raw = c.stable_physical_raw(origin).unwrap().unwrap();
    assert_eq!(raw, "<!DOCTYPE r>");
    assert_eq!(raw.as_ptr(), source[2..].as_ptr());
}

#[test]
fn spec_retained_physical_events_and_empty_take_survive_later_calls() {
    let input = String::from("<!DOCTYPE r [<!--one--><!--two-->]>");
    let mut state = ParserState::new(Limits::default(), 0);
    let mut c =
        DtdController::stable_utf8(&input, 0, 0, ControllerLimits::default(), false).unwrap();
    assert!(c.take_current().is_none());
    assert_eq!(c.physical_position(), 0);
    assert_eq!(c.counters(), ControllerCounters::default());
    c.advance_borrowed(state.owned_budget()).unwrap();
    let (header, _) = c.take_current().unwrap();
    c.advance_borrowed(state.owned_budget()).unwrap();
    let (first, _) = c.take_current().unwrap();
    let pointer = unit(&first).unwrap().raw().as_ptr();
    let rest = drain_borrowed(&mut c, state.owned_budget()).unwrap();
    assert_eq!(rest.len(), 2);
    assert_eq!(unit(&header).unwrap().raw(), "<!DOCTYPE r [");
    assert_eq!(unit(&first).unwrap().raw(), "<!--one-->");
    assert_eq!(unit(&first).unwrap().raw().as_ptr(), pointer);
    assert!(c.take_current().is_none());
    let body = c.into_body_state().ok().unwrap();
    drop(body);
    assert_eq!(
        unit(&first).unwrap().raw().as_ptr(),
        input[input.find("<!--one-->").unwrap()..].as_ptr()
    );
}
