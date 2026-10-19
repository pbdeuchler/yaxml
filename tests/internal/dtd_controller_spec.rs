use super::*;

use crate::Limits;
use crate::dtd::DtdUnitView;
use crate::dtd_catalog::{DependencyKind, ValueLocation};
use crate::dtd_frames::{FrameErrorKind, FrameLimit};
use crate::encoding::{DecodeProgress, Decoded, Decoder, SourceEncoding};
use crate::parser::ParserState;

#[derive(Clone, Debug, Eq, PartialEq)]
enum SeenEvent {
    Begin {
        root: String,
        subset: bool,
        origin: SeenOrigin,
    },
    Item {
        family: &'static str,
        raw: String,
        literals: LiteralSource,
        origin: SeenOrigin,
    },
    End(SeenOrigin),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SeenOrigin {
    Input(std::ops::Range<usize>),
    Replacement {
        declaration: usize,
        invocation: u64,
        local: std::ops::Range<usize>,
        physical: std::ops::Range<usize>,
    },
    Mixed(std::ops::Range<usize>),
}

#[derive(Debug)]
struct Run {
    events: Vec<SeenEvent>,
    consumed: usize,
    position: usize,
    doctype: Option<std::ops::Range<usize>>,
    counters: ControllerCounters,
}

fn seen_origin(origin: FragmentOrigin) -> SeenOrigin {
    match origin {
        FragmentOrigin::Input { span } => SeenOrigin::Input(span.range()),
        FragmentOrigin::Replacement {
            declaration,
            invocation,
            local,
            physical_reference,
        } => SeenOrigin::Replacement {
            declaration: declaration.index(),
            invocation: invocation.get(),
            local: local.range(),
            physical: physical_reference.range(),
        },
        FragmentOrigin::Mixed { physical_context } => SeenOrigin::Mixed(physical_context.range()),
    }
}

fn family(view: DtdUnitView<'_>) -> &'static str {
    match view {
        DtdUnitView::Header(_) => "header",
        DtdUnitView::Element(_) => "element",
        DtdUnitView::AttributeList(_) => "attlist",
        DtdUnitView::Entity(_) => "entity",
        DtdUnitView::Notation(_) => "notation",
        DtdUnitView::Comment(_) => "comment",
        DtdUnitView::ProcessingInstruction(_) => "pi",
    }
}

fn drain(controller: &mut DtdController, events: &mut Vec<SeenEvent>) -> bool {
    let mut drained = false;
    while let Some((event, origin)) = controller.current() {
        drained = true;
        let origin = seen_origin(origin);
        events.push(match event {
            DtdEvent::Begin(header) => SeenEvent::Begin {
                root: header.root_name().to_owned(),
                subset: header.has_internal_subset(),
                origin,
            },
            DtdEvent::Item { unit, literals } => SeenEvent::Item {
                family: family(unit.view()),
                raw: unit.raw().to_owned(),
                literals,
                origin,
            },
            DtdEvent::End => SeenEvent::End(origin),
        });
        controller.release();
    }
    drained
}

fn run_parts(
    input: &str,
    encoding: SourceEncoding,
    physical_start: usize,
    limits: ControllerLimits,
    cuts: &[usize],
) -> Result<Run, ControllerError> {
    let mut state = ParserState::new(Limits::default(), physical_start);
    let mut controller = DtdController::new(encoding, physical_start, limits, false);
    let mut events = Vec::new();
    let mut consumed = 0;

    for &end in cuts.iter().chain(std::iter::once(&input.len())) {
        assert!((consumed..=input.len()).contains(&end));
        while consumed < end && !controller.is_complete() {
            let amount =
                controller.advance(&input.as_bytes()[consumed..end], state.owned_budget())?;
            consumed += amount;
            let drained = drain(&mut controller, &mut events);
            assert!(amount != 0 || drained, "controller made no progress");
        }
        if controller.is_complete() {
            break;
        }
    }

    for _ in 0..64 {
        if controller.is_complete() {
            break;
        }
        if drain(&mut controller, &mut events) {
            continue;
        }
        if consumed < input.len() {
            let amount = controller.advance(&input.as_bytes()[consumed..], state.owned_budget())?;
            consumed += amount;
            assert!(amount != 0 || controller.current().is_some());
        } else {
            controller.finish_step(state.owned_budget())?;
        }
    }
    drain(&mut controller, &mut events);
    assert!(controller.is_complete(), "controller did not finish");
    Ok(Run {
        events,
        consumed,
        position: controller.physical_position(),
        doctype: controller.doctype_range(),
        counters: controller.counters(),
    })
}

fn run(input: &str) -> Result<Run, ControllerError> {
    run_parts(
        input,
        SourceEncoding::Utf8,
        0,
        ControllerLimits::default(),
        &[],
    )
}

#[derive(Debug)]
struct FailedRun {
    events: Vec<SeenEvent>,
    error: ControllerError,
    counters: ControllerCounters,
}

fn fail_with(input: &str, limits: ControllerLimits) -> FailedRun {
    let mut state = ParserState::new(Limits::default(), 0);
    let mut controller = DtdController::new(SourceEncoding::Utf8, 0, limits, false);
    let mut events = Vec::new();
    let mut consumed = 0;
    let error = loop {
        match controller.advance(&input.as_bytes()[consumed..], state.owned_budget()) {
            Ok(amount) => {
                consumed += amount;
                let drained = drain(&mut controller, &mut events);
                assert!(amount != 0 || drained, "controller made no progress");
            }
            Err(error) => break error,
        }
    };
    let position = controller.physical_position();
    let counters = controller.counters();
    assert_eq!(
        controller.advance(&[], state.owned_budget()),
        Err(error),
        "controller failure must be sticky"
    );
    assert_eq!(controller.physical_position(), position);
    assert_eq!(controller.counters(), counters);
    FailedRun {
        events,
        error,
        counters,
    }
}

fn input(range: std::ops::Range<usize>) -> SeenOrigin {
    SeenOrigin::Input(range)
}

fn replacement(
    declaration: usize,
    invocation: u64,
    local: std::ops::Range<usize>,
    physical: std::ops::Range<usize>,
) -> SeenOrigin {
    SeenOrigin::Replacement {
        declaration,
        invocation,
        local,
        physical,
    }
}

fn encoded(source: &str, encoding: SourceEncoding) -> Vec<u8> {
    match encoding {
        SourceEncoding::Utf8 => source.as_bytes().to_vec(),
        SourceEncoding::Utf16 {
            little_endian,
            bom: true,
        } => std::iter::once(if little_endian {
            [0xff, 0xfe]
        } else {
            [0xfe, 0xff]
        })
        .chain(source.encode_utf16().map(|unit| {
            if little_endian {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            }
        }))
        .flatten()
        .collect(),
        SourceEncoding::Utf16 { bom: false, .. } => {
            panic!("transport fixture requires an unambiguous BOM")
        }
    }
}

fn run_transport(raw: &[u8], split: usize) -> (Run, usize) {
    let mut decoder = Decoder::new(1024 * 1024);
    let mut state = ParserState::new(Limits::default(), 0);
    let mut controller = None;
    let mut events = Vec::new();
    let mut raw_consumed = 0;

    for end in [split, raw.len()] {
        while raw_consumed < end && !controller.as_ref().is_some_and(DtdController::is_complete) {
            let progress = decoder
                .advance(&raw[raw_consumed..end], |decoded| match decoded {
                    Decoded::Start { encoding, bom } => {
                        controller = Some(DtdController::new(
                            encoding,
                            bom,
                            ControllerLimits::default(),
                            false,
                        ));
                        Ok(DecodeProgress {
                            consumed: 0,
                            suspended: false,
                        })
                    }
                    Decoded::Bytes(bytes) => {
                        let controller = controller.as_mut().expect("decoder start first");
                        let mut consumed = 0;
                        for _ in 0..256 {
                            if consumed == bytes.len() || controller.is_complete() {
                                break;
                            }
                            let amount = controller
                                .advance(&bytes[consumed..], state.owned_budget())
                                .expect("valid transport fixture");
                            consumed += amount;
                            drain(controller, &mut events);
                        }
                        assert!(consumed != 0 || controller.is_complete());
                        Ok(DecodeProgress {
                            consumed,
                            suspended: controller.is_complete() || consumed < bytes.len(),
                        })
                    }
                })
                .expect("valid transport fixture");
            raw_consumed += progress.consumed;
            assert!(progress.consumed != 0 || progress.suspended);
        }
        if controller.as_ref().is_some_and(DtdController::is_complete) {
            break;
        }
    }

    let controller = controller.expect("encoding selected");
    assert!(controller.is_complete());
    (
        Run {
            events,
            consumed: 0,
            position: controller.physical_position(),
            doctype: controller.doctype_range(),
            counters: controller.counters(),
        },
        raw_consumed,
    )
}

#[test]
fn no_subset_holds_the_real_delimiter_and_preserves_the_body_suffix() {
    let expected = vec![
        SeenEvent::Begin {
            root: "r".to_owned(),
            subset: false,
            origin: input(0..11),
        },
        SeenEvent::End(input(11..12)),
    ];
    let actual = run("<!DOCTYPE r><r/>TAIL").unwrap();
    assert_eq!(actual.events, expected);
    assert_eq!(actual.consumed, 12);
    assert_eq!(actual.position, 12);
    assert_eq!(actual.doctype, Some(0..12));
}

#[test]
fn empty_subset_has_no_item_and_keeps_its_complete_end_delimiter() {
    let expected = vec![
        SeenEvent::Begin {
            root: "r".to_owned(),
            subset: true,
            origin: input(0..13),
        },
        SeenEvent::End(input(13..15)),
    ];
    let actual = run("<!DOCTYPE r []><r/>TAIL").unwrap();
    assert_eq!(actual.events, expected);
    assert_eq!(actual.consumed, 15);
    assert_eq!(actual.position, 15);
    assert_eq!(actual.doctype, Some(0..15));
}

#[test]
fn one_parameter_use_emits_each_logical_declaration_once_in_place() {
    let expected = vec![
        SeenEvent::Begin {
            root: "r".to_owned(),
            subset: true,
            origin: input(0..13),
        },
        SeenEvent::Item {
            family: "entity",
            raw: "<!ENTITY % p \"<!ELEMENT a EMPTY>\">".to_owned(),
            literals: LiteralSource::Physical,
            origin: input(13..47),
        },
        SeenEvent::Item {
            family: "element",
            raw: "<!ELEMENT a EMPTY>".to_owned(),
            literals: LiteralSource::Replacement,
            origin: replacement(0, 1, 0..18, 47..50),
        },
        SeenEvent::End(input(50..52)),
    ];
    let actual = run("<!DOCTYPE r [<!ENTITY % p \"<!ELEMENT a EMPTY>\">%p;]><r/>TAIL").unwrap();
    assert_eq!(actual.events, expected);
    assert_eq!(actual.consumed, 52);
    assert_eq!(actual.doctype, Some(0..52));
}

#[test]
fn replacement_comments_and_instructions_retain_literal_order_and_ranges() {
    let actual = run(
        "<!DOCTYPE r [<!ENTITY % p \"<!--first--><?inside data?><!ELEMENT a EMPTY><!--last-->\">%p;]><r/>",
    )
    .unwrap();
    let expected_generated = [
        SeenEvent::Item {
            family: "comment",
            raw: "<!--first-->".to_owned(),
            literals: LiteralSource::Replacement,
            origin: replacement(0, 1, 0..12, 85..88),
        },
        SeenEvent::Item {
            family: "pi",
            raw: "<?inside data?>".to_owned(),
            literals: LiteralSource::Replacement,
            origin: replacement(0, 1, 12..27, 85..88),
        },
        SeenEvent::Item {
            family: "element",
            raw: "<!ELEMENT a EMPTY>".to_owned(),
            literals: LiteralSource::Replacement,
            origin: replacement(0, 1, 27..45, 85..88),
        },
        SeenEvent::Item {
            family: "comment",
            raw: "<!--last-->".to_owned(),
            literals: LiteralSource::Replacement,
            origin: replacement(0, 1, 45..56, 85..88),
        },
    ];
    assert_eq!(&actual.events[2..6], expected_generated);
    assert_eq!(actual.events[6], SeenEvent::End(input(88..90)));
}

#[test]
fn every_direct_item_family_is_validated_and_emitted_in_source_order() {
    let actual = run(concat!(
        "<!DOCTYPE r [",
        "<!ELEMENT r EMPTY>",
        "<!ATTLIST r a CDATA #IMPLIED>",
        "<!ENTITY e 'v'>",
        "<!NOTATION n SYSTEM 's'>",
        "<!--c-->",
        "<?p d?>",
        "]><r/>"
    ))
    .unwrap();
    let observed: Vec<_> = actual
        .events
        .iter()
        .filter_map(|event| match event {
            SeenEvent::Item { family, raw, .. } => Some((*family, raw.as_str())),
            _ => None,
        })
        .collect();
    let expected = [
        ("element", "<!ELEMENT r EMPTY>"),
        ("attlist", "<!ATTLIST r a CDATA #IMPLIED>"),
        ("entity", "<!ENTITY e 'v'>"),
        ("notation", "<!NOTATION n SYSTEM 's'>"),
        ("comment", "<!--c-->"),
        ("pi", "<?p d?>"),
    ];
    assert_eq!(observed, expected);
}

#[test]
fn numeric_percent_schedules_the_actual_nested_invocation_origin() {
    let expected = vec![
        SeenEvent::Begin {
            root: "r".to_owned(),
            subset: true,
            origin: input(0..13),
        },
        SeenEvent::Item {
            family: "entity",
            raw: "<!ENTITY % q \"<!ELEMENT a EMPTY>\">".to_owned(),
            literals: LiteralSource::Physical,
            origin: input(13..47),
        },
        SeenEvent::Item {
            family: "entity",
            raw: "<!ENTITY % p \"&#37;q;\">".to_owned(),
            literals: LiteralSource::Physical,
            origin: input(47..70),
        },
        SeenEvent::Item {
            family: "element",
            raw: "<!ELEMENT a EMPTY>".to_owned(),
            literals: LiteralSource::Replacement,
            origin: replacement(0, 2, 0..18, 70..73),
        },
        SeenEvent::End(input(73..75)),
    ];
    let actual =
        run("<!DOCTYPE r [<!ENTITY % q \"<!ELEMENT a EMPTY>\"><!ENTITY % p \"&#37;q;\">%p;]><r/>")
            .unwrap();
    assert_eq!(actual.events, expected);
    assert_eq!(actual.consumed, 75);
    assert_eq!(actual.counters.reference_attempts, 2);
}

#[test]
fn repeated_uses_have_distinct_invocations_and_retain_duplicate_items() {
    let actual = run("<!DOCTYPE r [<!ENTITY % p \"<!ELEMENT a EMPTY>\">%p;%p;]><r/>").unwrap();
    assert_eq!(actual.events.len(), 5);
    assert_eq!(
        actual.events[2],
        SeenEvent::Item {
            family: "element",
            raw: "<!ELEMENT a EMPTY>".to_owned(),
            literals: LiteralSource::Replacement,
            origin: replacement(0, 1, 0..18, 47..50),
        }
    );
    assert_eq!(
        actual.events[3],
        SeenEvent::Item {
            family: "element",
            raw: "<!ELEMENT a EMPTY>".to_owned(),
            literals: LiteralSource::Replacement,
            origin: replacement(0, 2, 0..18, 50..53),
        }
    );
    assert_eq!(actual.events[4], SeenEvent::End(input(53..55)));
    assert_eq!(actual.counters.reference_attempts, 2);
}

#[test]
fn every_unicode_split_preserves_utf8_and_utf16_physical_ranges() {
    let document = "<!DOCTYPE π [<!ELEMENT π EMPTY>]><π/>Z";
    let utf8_expected = vec![
        SeenEvent::Begin {
            root: "π".to_owned(),
            subset: true,
            origin: input(0..14),
        },
        SeenEvent::Item {
            family: "element",
            raw: "<!ELEMENT π EMPTY>".to_owned(),
            literals: LiteralSource::Physical,
            origin: input(14..33),
        },
        SeenEvent::End(input(33..35)),
    ];
    let utf16_expected = vec![
        SeenEvent::Begin {
            root: "π".to_owned(),
            subset: true,
            origin: input(2..28),
        },
        SeenEvent::Item {
            family: "element",
            raw: "<!ELEMENT π EMPTY>".to_owned(),
            literals: LiteralSource::Physical,
            origin: input(28..64),
        },
        SeenEvent::End(input(64..68)),
    ];

    for split in 0..=document.len() {
        let utf8 = run_parts(
            document,
            SourceEncoding::Utf8,
            0,
            ControllerLimits::default(),
            &[split],
        )
        .unwrap();
        assert_eq!(utf8.events, utf8_expected, "UTF-8 split {split}");
        assert_eq!(utf8.consumed, 35);
        assert_eq!(utf8.position, 35);

        for little_endian in [true, false] {
            let utf16 = run_parts(
                document,
                SourceEncoding::Utf16 {
                    little_endian,
                    bom: true,
                },
                2,
                ControllerLimits::default(),
                &[split],
            )
            .unwrap();
            assert_eq!(utf16.events, utf16_expected, "UTF-16 split {split}");
            assert_eq!(utf16.consumed, 35);
            assert_eq!(utf16.position, 68);
            assert_eq!(utf16.doctype, Some(2..68));
        }
    }
}

#[test]
fn every_raw_transport_split_preserves_utf8_and_utf16_transcripts() {
    let document = "<!DOCTYPE π [<!ELEMENT π EMPTY>]><π/>Z";
    for encoding in [
        SourceEncoding::Utf8,
        SourceEncoding::Utf16 {
            little_endian: true,
            bom: true,
        },
        SourceEncoding::Utf16 {
            little_endian: false,
            bom: true,
        },
    ] {
        let raw = encoded(document, encoding);
        let expected_ranges = if encoding == SourceEncoding::Utf8 {
            [0..14, 14..33, 33..35]
        } else {
            [2..28, 28..64, 64..68]
        };
        let expected_bytes = expected_ranges[2].end;
        for split in 0..=raw.len() {
            let (actual, raw_consumed) = run_transport(&raw, split);
            assert_eq!(raw_consumed, expected_bytes, "{encoding:?} split {split}");
            assert_eq!(
                actual.position, expected_bytes,
                "{encoding:?} split {split}"
            );
            assert_eq!(
                actual.doctype,
                Some(expected_ranges[0].start..expected_bytes),
                "{encoding:?} split {split}"
            );
            assert_eq!(
                actual
                    .events
                    .iter()
                    .map(|event| match event {
                        SeenEvent::Begin { origin, .. }
                        | SeenEvent::Item { origin, .. }
                        | SeenEvent::End(origin) => origin.clone(),
                    })
                    .collect::<Vec<_>>(),
                expected_ranges
                    .iter()
                    .cloned()
                    .map(input)
                    .collect::<Vec<_>>(),
                "{encoding:?} split {split}"
            );
        }
    }
}

#[test]
fn replacement_fragment_must_finish_a_complete_unit() {
    let raw = "<!DOCTYPE r [<!ENTITY % p \"<!ELEMENT a \" >%p;]><r/>";
    let mut state = ParserState::new(Limits::default(), 0);
    let mut controller =
        DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
    let mut offset = 0;
    let error = loop {
        match controller.advance(&raw.as_bytes()[offset..], state.owned_budget()) {
            Ok(consumed) => {
                offset += consumed;
                drain(&mut controller, &mut Vec::new());
            }
            Err(error) => break error,
        }
    };
    assert_eq!(
        error.kind,
        ControllerErrorKind::Scan(ScanErrorKind::UnexpectedEof)
    );
    assert_eq!(error.physical_offset, 42);
    assert_eq!(error.local_offset, Some(12));
}

#[test]
fn nested_doctype_is_rejected_as_a_unit_family_not_emitted_as_begin() {
    let raw = "<!DOCTYPE r [<!ENTITY % p \"<!DOCTYPE x>\">%p;]><r/>";
    let mut state = ParserState::new(Limits::default(), 0);
    let mut controller =
        DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
    let mut seen = Vec::new();
    let mut offset = 0;
    let error = loop {
        match controller.advance(&raw.as_bytes()[offset..], state.owned_budget()) {
            Ok(consumed) => {
                offset += consumed;
                drain(&mut controller, &mut seen);
            }
            Err(error) => break error,
        }
    };
    assert_eq!(error.kind, ControllerErrorKind::InvalidUnitFamily);
    assert_eq!(error.physical_offset, 41);
    assert_eq!(error.local_offset, Some(0));
    assert_eq!(seen.len(), 2);
    assert!(matches!(seen[0], SeenEvent::Begin { .. }));
    assert!(matches!(
        seen[1],
        SeenEvent::Item {
            family: "entity",
            ..
        }
    ));
}

#[test]
fn empty_cycle_and_unavailable_parameter_uses_are_distinct() {
    let empty = run("<!DOCTYPE r [<!ENTITY % p \"\">%p;]><r/>").unwrap();
    assert_eq!(empty.events.len(), 3);
    assert_eq!(empty.events[2], SeenEvent::End(input(32..34)));
    assert_eq!(empty.counters.reference_attempts, 1);

    for (raw, expected_offset, expected_kind) in [
        (
            "<!DOCTYPE r [<!ENTITY % p \"&#37;p;\">%p;]><r/>",
            36,
            ControllerErrorKind::Frame(FrameErrorKind::Cycle),
        ),
        (
            "<!DOCTYPE r [<!ENTITY % p \"&#37;missing;\">%p;]><r/>",
            42,
            ControllerErrorKind::Unavailable(PeUnavailable::Undeclared),
        ),
    ] {
        let mut state = ParserState::new(Limits::default(), 0);
        let mut controller =
            DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
        let mut offset = 0;
        let error = loop {
            match controller.advance(&raw.as_bytes()[offset..], state.owned_budget()) {
                Ok(consumed) => {
                    offset += consumed;
                    drain(&mut controller, &mut Vec::new());
                }
                Err(error) => break error,
            }
        };
        assert_eq!(error.kind, expected_kind);
        assert_eq!(error.physical_offset, expected_offset);
        assert_eq!(
            controller.advance(&[], state.owned_budget()),
            Err(error),
            "failure must be sticky"
        );
    }
}

#[test]
fn nonzero_physical_start_maps_utf8_and_utf16_without_a_second_bom() {
    let document = "<!DOCTYPE π [<!ELEMENT π EMPTY>]><π/>Z";
    let utf8 = run_parts(
        document,
        SourceEncoding::Utf8,
        37,
        ControllerLimits::default(),
        &[],
    )
    .unwrap();
    assert_eq!(
        utf8.events,
        [
            SeenEvent::Begin {
                root: "π".to_owned(),
                subset: true,
                origin: input(37..51),
            },
            SeenEvent::Item {
                family: "element",
                raw: "<!ELEMENT π EMPTY>".to_owned(),
                literals: LiteralSource::Physical,
                origin: input(51..70),
            },
            SeenEvent::End(input(70..72)),
        ]
    );
    let utf16 = run_parts(
        document,
        SourceEncoding::Utf16 {
            little_endian: true,
            bom: true,
        },
        37,
        ControllerLimits::default(),
        &[],
    )
    .unwrap();
    assert_eq!(
        utf16.events,
        [
            SeenEvent::Begin {
                root: "π".to_owned(),
                subset: true,
                origin: input(37..63),
            },
            SeenEvent::Item {
                family: "element",
                raw: "<!ELEMENT π EMPTY>".to_owned(),
                literals: LiteralSource::Physical,
                origin: input(63..99),
            },
            SeenEvent::End(input(99..103)),
        ]
    );
}

#[test]
fn a_replacement_cannot_supply_the_physical_subset_delimiter() {
    let raw = r#"<!DOCTYPE r [<!ENTITY % p "]>">%p;]><r/>"#;
    let failure = fail_with(raw, ControllerLimits::default());
    assert_eq!(
        failure.error.kind,
        ControllerErrorKind::Scan(ScanErrorKind::Syntax)
    );
    assert_eq!(failure.error.physical_offset, 31);
    assert_eq!(failure.error.local_offset, Some(0));
    assert_eq!(failure.events.len(), 2);
    assert!(matches!(failure.events[0], SeenEvent::Begin { .. }));
    assert!(matches!(
        failure.events[1],
        SeenEvent::Item {
            family: "entity",
            ..
        }
    ));
}

#[test]
fn controller_rejects_an_unavailable_default_even_when_its_binding_is_duplicate() {
    let raw = concat!(
        "<!DOCTYPE r [",
        "<!ENTITY indirect '&missing;'>",
        "<!ATTLIST r x CDATA 'first'>",
        "<!ATTLIST r x CDATA '&indirect;'>",
        "]><r/>"
    );
    let expected_location = ("replacement", 0, 0);
    let failure = fail_with(raw, ControllerLimits::default());
    assert_eq!(
        failure.error.kind,
        ControllerErrorKind::ValueUnavailable(DependencyKind::MissingReadDeclaration)
    );
    let Some(ValueLocation::Replacement { entity, offset }) = failure.error.value_location else {
        panic!("expected replacement dependency location")
    };
    assert_eq!(("replacement", entity.index(), offset), expected_location);
    assert_eq!(failure.events.len(), 3);
    assert!(matches!(
        failure.events[1],
        SeenEvent::Item {
            family: "entity",
            ..
        }
    ));
    assert!(matches!(
        failure.events[2],
        SeenEvent::Item {
            family: "attlist",
            ..
        }
    ));
    assert_eq!(
        failure
            .events
            .iter()
            .filter(|event| matches!(
                event,
                SeenEvent::Item {
                    family: "attlist",
                    ..
                }
            ))
            .count(),
        1,
        "the semantically rejected duplicate must not be published"
    );
}

#[test]
fn frame_reference_and_depth_ceilings_gate_before_child_publication() {
    let one_use = "<!DOCTYPE r [<!ENTITY % p \"<!ELEMENT a EMPTY>\">%p;]><r/>";
    let mut zero_references = ControllerLimits::default();
    zero_references.frames.max_reference_attempts = 0;
    let blocked = fail_with(one_use, zero_references);
    assert_eq!(
        blocked.error.kind,
        ControllerErrorKind::Frame(FrameErrorKind::LimitExceeded(FrameLimit::ReferenceAttempts))
    );
    assert_eq!(blocked.error.physical_offset, 47);
    assert_eq!(blocked.counters.reference_attempts, 0);
    assert_eq!(blocked.events.len(), 2);

    let nested = concat!(
        "<!DOCTYPE r [",
        "<!ENTITY % q \"<!ELEMENT a EMPTY>\">",
        "<!ENTITY % p \"&#37;q;\">",
        "%p;]><r/>"
    );
    let mut depth_one = ControllerLimits::default();
    depth_one.frames.max_active_depth = 1;
    let blocked = fail_with(nested, depth_one);
    assert_eq!(
        blocked.error.kind,
        ControllerErrorKind::Frame(FrameErrorKind::LimitExceeded(FrameLimit::ActiveDepth))
    );
    assert_eq!(blocked.error.physical_offset, 70);
    assert_eq!(blocked.counters.reference_attempts, 2);
    assert_eq!(blocked.events.len(), 3);

    let mut depth_two = ControllerLimits::default();
    depth_two.frames.max_active_depth = 2;
    let accepted = run_parts(nested, SourceEncoding::Utf8, 0, depth_two, &[]).unwrap();
    assert!(matches!(
        accepted.events[3],
        SeenEvent::Item {
            family: "element",
            ..
        }
    ));
}

#[test]
fn global_work_is_cumulative_across_fragment_restart_and_chunking() {
    let raw = "<!DOCTYPE r [<!ENTITY % p \"<!ELEMENT a EMPTY>\">%p;]><r/>";
    let expected_work = 302;
    for cuts in [vec![], vec![0], vec![1, 7, 31, raw.len()]] {
        let accepted = run_parts(
            raw,
            SourceEncoding::Utf8,
            0,
            ControllerLimits::default(),
            &cuts,
        )
        .unwrap();
        assert_eq!(accepted.counters.work, expected_work);
        assert_eq!(accepted.counters.scanned_bytes, 70);
        assert_eq!(accepted.counters.owned_bytes, 65);
        assert_eq!(accepted.counters.physical_bytes, 52);
        assert_eq!(accepted.counters.reference_attempts, 1);
    }

    let exact = ControllerLimits {
        max_work: expected_work,
        ..ControllerLimits::default()
    };
    assert_eq!(
        run_parts(raw, SourceEncoding::Utf8, 0, exact, &[])
            .unwrap()
            .counters
            .work,
        expected_work
    );

    let mut one_short = exact;
    one_short.max_work = expected_work - 1;
    let blocked = fail_with(raw, one_short);
    assert_eq!(
        blocked.error.kind,
        ControllerErrorKind::Limit(ControllerLimit::Work)
    );
    assert_eq!(blocked.counters.work, expected_work - 1);
    assert!(
        blocked
            .events
            .iter()
            .all(|event| !matches!(event, SeenEvent::End(_)))
    );
}
