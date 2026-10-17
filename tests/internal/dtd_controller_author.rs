use super::*;
use crate::Limits;
use crate::parser::ParserState;

#[test]
fn no_subset_retains_the_real_end_delimiter_and_body_suffix() {
    let mut state = ParserState::new(Limits::default(), 0);
    let mut controller =
        DtdController::new(SourceEncoding::Utf8, 10, ControllerLimits::default(), false);
    assert!(controller.current().is_none());
    assert_eq!(
        controller
            .advance(b"<!DOCTYPE r><r/>", state.owned_budget())
            .unwrap(),
        12
    );
    let (event, origin) = controller.current().unwrap();
    assert!(matches!(event,DtdEvent::Begin(header) if header.root_name()=="r"));
    assert_eq!(origin.input_range(), Some(10..21));
    controller.release();
    assert!(matches!(controller.current().unwrap(), (DtdEvent::End, _)));
    assert_eq!(controller.current().unwrap().1.input_range(), Some(21..22));
    controller.release();
    assert!(controller.is_complete());
    assert_eq!(controller.doctype_range(), Some(10..22));
    assert_eq!(controller.physical_position(), 22);
    assert!(controller.counters().work > 12);
    assert_eq!(
        controller.advance(b"<r/>", state.owned_budget()).unwrap(),
        0
    );
}

#[test]
fn parameter_declarations_are_expanded_in_order_and_mutate_catalog_safely() {
    let input = b"<!DOCTYPE r [<!ENTITY % p \"<!ENTITY g 'v'>\">%p;]><r/>";
    let mut state = ParserState::new(Limits::default(), 0);
    let mut controller =
        DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
    let mut cursor = 0;
    let mut items = Vec::new();
    while !controller.is_complete() {
        cursor += controller
            .advance(&input[cursor..], state.owned_budget())
            .unwrap();
        let (event, origin) = controller.current().expect("event");
        if let DtdEvent::Item { unit, literals } = event {
            items.push((unit.raw().to_owned(), literals, origin.input_range()));
        }
        controller.release();
    }
    assert_eq!(
        items,
        vec![
            (
                "<!ENTITY % p \"<!ENTITY g 'v'>\">".into(),
                LiteralSource::Physical,
                Some(13..44)
            ),
            ("<!ENTITY g 'v'>".into(), LiteralSource::Replacement, None)
        ]
    );
    assert_eq!(&input[cursor..], b"<r/>");
    assert!(matches!(
        controller.catalog().lookup_general("g"),
        EntityLookup::Internal {
            replacement: "v",
            ..
        }
    ));
    assert_eq!(controller.counters().reference_attempts, 1);
    controller.finish_step(state.owned_budget()).unwrap();
}

type Seen = (String, FragmentOrigin, Option<LiteralSource>);

fn record(controller: &mut DtdController, seen: &mut Vec<Seen>) -> bool {
    let Some((event, origin)) = controller.current() else {
        return false;
    };
    let (text, literals) = match event {
        DtdEvent::Begin(header) => (format!("BEGIN {}", header.root_name()), None),
        DtdEvent::Item { unit, literals } => (unit.raw().to_owned(), Some(literals)),
        DtdEvent::End => ("END".into(), None),
    };
    seen.push((text, origin, literals));
    controller.release();
    true
}

fn collect(
    input: &[u8],
    split: usize,
    encoding: SourceEncoding,
    start: usize,
    limits: ControllerLimits,
) -> (DtdController, Vec<Seen>, usize, Result<(), ControllerError>) {
    let mut state = ParserState::new(Limits::default(), 0);
    let mut controller = DtdController::new(encoding, start, limits, false);
    let mut seen = Vec::new();
    let mut cursor = 0;
    let result = (|| {
        for end in [split, input.len()] {
            while cursor < end && !controller.is_complete() {
                let consumed = controller.advance(&input[cursor..end], state.owned_budget())?;
                cursor += consumed;
                let emitted = record(&mut controller, &mut seen);
                assert!(consumed != 0 || emitted, "no progress at {cursor}");
            }
            while record(&mut controller, &mut seen) {}
        }
        while !controller.is_complete() {
            controller.finish_step(state.owned_budget())?;
            assert!(
                record(&mut controller, &mut seen),
                "EOF without event or error"
            );
        }
        Ok(())
    })();
    (controller, seen, cursor, result)
}

#[test]
fn all_splits_preserve_full_family_order_catalog_values_and_unicode_offsets() {
    let text = "<!DOCTYPE 根 [ \n<!ELEMENT 根 (子*)><!ELEMENT 子 EMPTY><!ATTLIST 根 a NMTOKENS ' a  b '><!ENTITY e 'v'><!NOTATION n SYSTEM 'urn:n'><!--é >--><?p x?> ] \t><根/>";
    let (baseline, expected, cursor, result) = collect(
        text.as_bytes(),
        0,
        SourceEncoding::Utf8,
        7,
        ControllerLimits::default(),
    );
    result.unwrap();
    assert_eq!(&text.as_bytes()[cursor..], "<根/>".as_bytes());
    assert_eq!(
        expected
            .iter()
            .map(|event| event.0.as_str())
            .collect::<Vec<_>>(),
        [
            "BEGIN 根",
            "<!ELEMENT 根 (子*)>",
            "<!ELEMENT 子 EMPTY>",
            "<!ATTLIST 根 a NMTOKENS ' a  b '>",
            "<!ENTITY e 'v'>",
            "<!NOTATION n SYSTEM 'urn:n'>",
            "<!--é >-->",
            "<?p x?>",
            "END"
        ]
    );
    assert_eq!(
        expected.last().unwrap().1.input_range(),
        Some(7 + cursor - 4..7 + cursor)
    );
    assert_eq!(
        baseline
            .catalog()
            .attributes("根")
            .next()
            .unwrap()
            .default(),
        crate::dtd_catalog::DefaultValue::Value {
            fixed: false,
            value: "a b"
        }
    );
    for split in 0..=text.len() {
        let (actual, seen, consumed, result) = collect(
            text.as_bytes(),
            split,
            SourceEncoding::Utf8,
            7,
            ControllerLimits::default(),
        );
        result.unwrap_or_else(|error| panic!("split {split}: {error:?}"));
        assert_eq!(seen, expected, "split {split}");
        assert_eq!(consumed, cursor);
        assert_eq!(actual.counters(), baseline.counters());
    }
}

#[test]
fn nested_numeric_percent_references_keep_invocations_and_outer_reference() {
    let text = "<!DOCTYPE r [<!ENTITY % leaf '<!ELEMENT r EMPTY>'><!ENTITY % outer '&#37;leaf;'><!ENTITY % empty ''>%outer;%outer;%empty;]><r/>";
    let (controller, seen, cursor, result) = collect(
        text.as_bytes(),
        0,
        SourceEncoding::Utf8,
        3,
        ControllerLimits::default(),
    );
    result.unwrap();
    assert_eq!(&text[cursor..], "<r/>");
    assert_eq!(controller.counters().reference_attempts, 5);
    let generated = seen
        .iter()
        .filter(|(_, _, phase)| *phase == Some(LiteralSource::Replacement))
        .collect::<Vec<_>>();
    assert_eq!(generated.len(), 2);
    let mut invocations = Vec::new();
    for (index, event) in generated.iter().enumerate() {
        assert_eq!(event.0, "<!ELEMENT r EMPTY>");
        let FragmentOrigin::Replacement {
            declaration,
            invocation,
            local,
            physical_reference,
        } = event.1
        else {
            panic!("replacement")
        };
        assert_eq!(declaration.index(), 0);
        assert_eq!(local.range(), 0..18);
        let offset = text.find("%outer;").unwrap() + index * 7 + 3;
        assert_eq!(physical_reference.range(), offset..offset + 7);
        invocations.push(invocation.get());
    }
    assert_ne!(invocations[0], invocations[1]);
    for split in 0..=text.len() {
        let (_, actual, _, result) = collect(
            text.as_bytes(),
            split,
            SourceEncoding::Utf8,
            3,
            ControllerLimits::default(),
        );
        result.unwrap();
        assert_eq!(actual, seen, "split {split}");
    }
}

#[test]
fn malformed_generated_units_report_local_and_real_reference_coordinates() {
    for replacement in [
        "]>",
        "<!DOCTYPE r>",
        "<![IGNORE[x]]>",
        "<!ELEMENT r",
        "<!ELEMENT 1 EMPTY>",
    ] {
        let text = format!("<!DOCTYPE r [<!ENTITY % p '{replacement}'>%p;]>");
        let (_, _, _, result) = collect(
            text.as_bytes(),
            0,
            SourceEncoding::Utf8,
            9,
            ControllerLimits::default(),
        );
        let error = result.unwrap_err();
        let origin = error.origin.expect("generated diagnostic origin");
        assert!(origin.input_range().is_none(), "{replacement}");
        assert_eq!(
            origin.physical_context(),
            9 + text.find("%p;").unwrap()..9 + text.find("%p;").unwrap() + 3
        );
        assert_eq!(error.physical_offset, origin.physical_context().start);
        assert!(error.local_offset.is_some(), "{replacement}: {error:?}");
        for split in 0..=text.len() {
            let (_, _, _, result) = collect(
                text.as_bytes(),
                split,
                SourceEncoding::Utf8,
                9,
                ControllerLimits::default(),
            );
            assert_eq!(result, Err(error), "{replacement}, split {split}");
        }
    }
}

#[test]
fn unavailable_pe_and_default_values_never_publish_a_successful_end() {
    let cases = [
        (
            "<!DOCTYPE r [%missing;]>",
            ControllerErrorKind::Unavailable(PeUnavailable::Undeclared),
        ),
        (
            "<!DOCTYPE r [<!ENTITY % e SYSTEM 'urn:x'>%e;]>",
            ControllerErrorKind::Unavailable(PeUnavailable::External),
        ),
        (
            "<!DOCTYPE r [<!ENTITY e '&later;'><!ATTLIST r a CDATA '&e;'>]>",
            ControllerErrorKind::ValueUnavailable(DependencyKind::MissingReadDeclaration),
        ),
        (
            "<!DOCTYPE r [<!ENTITY e '&later;'><!ATTLIST r a CDATA 'first'><!ATTLIST r a CDATA '&e;'>]>",
            ControllerErrorKind::ValueUnavailable(DependencyKind::MissingReadDeclaration),
        ),
    ];
    for (text, kind) in cases {
        let (mut controller, seen, _, result) = collect(
            text.as_bytes(),
            0,
            SourceEncoding::Utf8,
            0,
            ControllerLimits::default(),
        );
        let error = result.unwrap_err();
        assert_eq!(error.kind, kind, "{text}");
        assert!(!controller.is_complete());
        assert!(seen.iter().all(|event| event.0 != "END"));
        let mut state = ParserState::new(Limits::default(), 0);
        assert_eq!(controller.advance(b"]>", state.owned_budget()), Err(error));
        assert_eq!(controller.finish_step(state.owned_budget()), Err(error));
    }
}

#[test]
fn replacement_cr_is_not_physically_renormalized_and_internal_pe_rules_remain() {
    let text = "<!DOCTYPE r [<!ENTITY % p \"<!ENTITY e 'a&#13;b'>\">%p;]>";
    let (controller, seen, _, result) = collect(
        text.as_bytes(),
        0,
        SourceEncoding::Utf8,
        0,
        ControllerLimits::default(),
    );
    result.unwrap();
    assert!(seen.iter().any(
        |event| event.0 == "<!ENTITY e 'a\rb'>" && event.2 == Some(LiteralSource::Replacement)
    ));
    assert!(matches!(
        controller.catalog().lookup_general("e"),
        EntityLookup::Internal {
            replacement: "a\rb",
            ..
        }
    ));
    let text = "<!DOCTYPE r [<!ENTITY % p \"<!ENTITY e '&#37;bad;'>\">%p;]>";
    let (_, _, _, result) = collect(
        text.as_bytes(),
        0,
        SourceEncoding::Utf8,
        0,
        ControllerLimits::default(),
    );
    assert_eq!(
        result.unwrap_err().kind,
        ControllerErrorKind::Catalog(CatalogErrorKind::ParameterInInternalDeclaration)
    );
}

#[test]
fn recursion_depth_and_empty_reference_attempts_are_bounded() {
    use crate::dtd_frames::FrameLimit;
    let recursion = "<!DOCTYPE r [<!ENTITY % p '&#37;p;'>%p;]>";
    let (_, _, _, result) = collect(
        recursion.as_bytes(),
        0,
        SourceEncoding::Utf8,
        0,
        ControllerLimits::default(),
    );
    let error = result.unwrap_err();
    assert_eq!(
        error.kind,
        ControllerErrorKind::Frame(FrameErrorKind::Cycle)
    );
    assert_eq!(error.local_offset, Some(0));
    let text = "<!DOCTYPE r [<!ENTITY % p ''>%p;%p;%p;]>";
    let mut limits = ControllerLimits::default();
    limits.frames.max_reference_attempts = 2;
    let (controller, _, _, result) = collect(text.as_bytes(), 0, SourceEncoding::Utf8, 0, limits);
    assert_eq!(
        result.unwrap_err().kind,
        ControllerErrorKind::Frame(FrameErrorKind::LimitExceeded(FrameLimit::ReferenceAttempts))
    );
    assert_eq!(controller.counters().reference_attempts, 2);
    let mut limits = ControllerLimits::default();
    limits.frames.max_active_depth = 1;
    let text = "<!DOCTYPE r [<!ENTITY % a ''><!ENTITY % b '&#37;a;'>%b;]>";
    let (_, _, _, result) = collect(text.as_bytes(), 0, SourceEncoding::Utf8, 0, limits);
    let error = result.unwrap_err();
    assert_eq!(
        error.kind,
        ControllerErrorKind::Frame(FrameErrorKind::LimitExceeded(FrameLimit::ActiveDepth))
    );
    assert_eq!(error.local_offset, Some(0));
}

#[test]
fn exact_cumulative_ceilings_include_every_fragment_restart_and_copy() {
    use crate::dtd_scan::ScanLimit;
    let text = "<!DOCTYPE r [<!ENTITY % p '<!--x-->  <?p?>'>%p;%p;]>";
    let (baseline, _, _, result) = collect(
        text.as_bytes(),
        0,
        SourceEncoding::Utf8,
        0,
        ControllerLimits::default(),
    );
    result.unwrap();
    let usage = baseline.counters();
    assert!(usage.scanned_bytes > text.len());
    for dimension in 0..4 {
        for at_limit in [false, true] {
            let mut limits = ControllerLimits::default();
            let expected = match dimension {
                0 => {
                    limits.scan.max_work = usage.scanned_bytes - usize::from(!at_limit);
                    ControllerErrorKind::Scan(ScanErrorKind::Limit(ScanLimit::Work))
                }
                1 => {
                    limits.max_work = usage.work - usize::from(!at_limit);
                    ControllerErrorKind::Limit(ControllerLimit::Work)
                }
                2 => {
                    limits.max_owned_bytes = usage.owned_bytes - usize::from(!at_limit);
                    ControllerErrorKind::Limit(ControllerLimit::OwnedBytes)
                }
                _ => {
                    limits.max_physical_bytes = usage.physical_bytes - usize::from(!at_limit);
                    ControllerErrorKind::Limit(ControllerLimit::PhysicalBytes)
                }
            };
            let (actual, _, _, result) = collect(
                text.as_bytes(),
                text.len() / 2,
                SourceEncoding::Utf8,
                0,
                limits,
            );
            if at_limit {
                result.unwrap();
                assert_eq!(actual.counters(), usage)
            } else {
                assert_eq!(result.unwrap_err().kind, expected, "dimension {dimension}")
            }
            assert!(actual.counters().work <= limits.max_work);
            assert!(actual.counters().scanned_bytes <= limits.scan.max_work);
        }
    }
}

#[test]
fn global_work_exhaustion_has_truthful_kind_in_scanner_catalog_and_frame_paths() {
    let text = "<!DOCTYPE r [<!ENTITY % p '<!--x-->  <?p?>'>%p;]>";
    let (baseline, _, _, result) = collect(
        text.as_bytes(),
        0,
        SourceEncoding::Utf8,
        0,
        ControllerLimits::default(),
    );
    result.unwrap();
    for ceiling in 0..baseline.counters().work {
        let limits = ControllerLimits {
            max_work: ceiling,
            ..ControllerLimits::default()
        };
        let (controller, _, _, result) =
            collect(text.as_bytes(), 0, SourceEncoding::Utf8, 0, limits);
        assert_eq!(
            result.unwrap_err().kind,
            ControllerErrorKind::Limit(ControllerLimit::Work),
            "ceiling {ceiling}"
        );
        assert!(controller.counters().work <= ceiling);
    }
}

#[test]
fn component_and_shared_owned_limits_fail_without_publishing_the_unit() {
    use crate::dtd_catalog::CatalogLimit;
    use crate::dtd_scan::ScanLimit;
    let text = "<!DOCTYPE r [<!ENTITY e 'abc'>]>";
    let mut limits = ControllerLimits::default();
    limits.scan.max_unit_bytes = 11;
    let (_, seen, _, result) = collect(text.as_bytes(), 0, SourceEncoding::Utf8, 0, limits);
    assert!(seen.is_empty());
    assert_eq!(
        result.unwrap_err().kind,
        ControllerErrorKind::Scan(ScanErrorKind::Limit(ScanLimit::UnitBytes))
    );
    let mut limits = ControllerLimits::default();
    limits.scan.max_scratch_bytes = 11;
    let (_, seen, _, result) = collect(text.as_bytes(), 0, SourceEncoding::Utf8, 0, limits);
    assert!(seen.is_empty());
    assert_eq!(
        result.unwrap_err().kind,
        ControllerErrorKind::Scan(ScanErrorKind::Limit(ScanLimit::ScratchBytes))
    );
    for kind in [
        CatalogLimit::Bindings,
        CatalogLimit::StorageBytes,
        CatalogLimit::Declarations,
    ] {
        let mut limits = ControllerLimits::default();
        match kind {
            CatalogLimit::Bindings => limits.catalog.max_bindings = 0,
            CatalogLimit::StorageBytes => limits.catalog.max_storage_bytes = 0,
            _ => limits.catalog.max_declarations = 1,
        }
        let (_, seen, _, result) = collect(text.as_bytes(), 0, SourceEncoding::Utf8, 0, limits);
        assert_eq!(seen.len(), 1);
        assert_eq!(
            result.unwrap_err().kind,
            ControllerErrorKind::Catalog(CatalogErrorKind::Limit(kind))
        );
    }
    let mut state = ParserState::new(
        Limits {
            max_owned_bytes: 11,
            ..Limits::default()
        },
        0,
    );
    let mut controller =
        DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
    let error = controller
        .advance(text.as_bytes(), state.owned_budget())
        .unwrap_err();
    assert_eq!(
        error.kind,
        ControllerErrorKind::Ownership(ErrorKind::LimitExceeded(crate::Limit::OwnedBytes))
    );
    assert!(controller.current().is_none());
    assert_eq!(controller.counters().owned_bytes, 0);
}

#[test]
fn frame_commit_quota_errors_retain_actual_replacement_unit_origin() {
    use crate::dtd_frames::FrameLimit;
    let text = "<!DOCTYPE r [<!ENTITY % p '<!--x-->'>%p;]>";
    let mut limits = ControllerLimits::default();
    limits.frames.max_expanded_bytes = 0;
    let (_, _, _, result) = collect(text.as_bytes(), 0, SourceEncoding::Utf8, 17, limits);
    let error = result.unwrap_err();
    assert_eq!(
        error.kind,
        ControllerErrorKind::Frame(FrameErrorKind::LimitExceeded(FrameLimit::ExpandedBytes))
    );
    assert_eq!(error.local_offset, Some(0));
    let origin = error.origin.unwrap();
    assert_eq!(origin.replacement_range(), Some(0..8));
    assert_eq!(
        origin.physical_context(),
        17 + text.find("%p;").unwrap()..17 + text.find("%p;").unwrap() + 3
    );
}

#[test]
fn final_eof_is_sticky_but_drains_ready_events_without_accepting_new_input() {
    let mut state = ParserState::new(Limits::default(), 0);
    let mut controller =
        DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
    controller
        .advance(b"<!DOCTYPE r [", state.owned_budget())
        .unwrap();
    controller.finish_step(state.owned_budget()).unwrap(); // Begin remains ready.
    assert!(matches!(
        controller.current().unwrap().0,
        DtdEvent::Begin(_)
    ));
    controller.release();
    let error = controller.advance(b"]>", state.owned_budget()).unwrap_err();
    assert_eq!(
        error.kind,
        ControllerErrorKind::Scan(ScanErrorKind::UnexpectedEof)
    );
    assert_eq!(controller.physical_position(), 13);
    assert_eq!(error.physical_offset, 13);
    assert!(error.origin.is_none());
    let mut controller = DtdController::new(
        SourceEncoding::Utf8,
        usize::MAX,
        ControllerLimits::default(),
        false,
    );
    let error = controller.advance(b"<", state.owned_budget()).unwrap_err();
    assert_eq!(error.kind, ControllerErrorKind::PositionOverflow);
    assert_eq!(controller.physical_position(), usize::MAX);
}

#[test]
fn utf16_transport_mapping_and_body_pause_hold_at_every_raw_byte_split() {
    use crate::encoding::{DecodeProgress, Decoded, Decoder};
    let text = "<!DOCTYPE 根 [<!ELEMENT 根 EMPTY><!--🦀--><!ENTITY % p '<!--é-->'>%p;]><根/>";
    for little_endian in [false, true] {
        let mut raw = if little_endian {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for unit in text.encode_utf16() {
            raw.extend(if little_endian {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        let expected_end = 2 + text[..text.rfind("<根/>").unwrap()].encode_utf16().count() * 2;
        let mut oracle = None;
        for split in 0..=raw.len() {
            let mut decoder = Decoder::new(4096);
            let mut controller = None;
            let mut state = ParserState::new(Limits::default(), 0);
            let mut cursor = 0;
            let mut seen = Vec::new();
            for end in [split, raw.len()] {
                while cursor < end && !controller.as_ref().is_some_and(DtdController::is_complete) {
                    let progress = decoder
                        .advance(&raw[cursor..end], |decoded| match decoded {
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
                                let c = controller.as_mut().unwrap();
                                let consumed = c.advance(bytes, state.owned_budget()).unwrap();
                                Ok(DecodeProgress {
                                    consumed,
                                    suspended: c.current().is_some() || c.is_complete(),
                                })
                            }
                        })
                        .unwrap();
                    cursor += progress.consumed;
                    while controller.as_mut().is_some_and(|c| record(c, &mut seen)) {}
                }
            }
            let controller = controller.unwrap();
            assert!(controller.is_complete(), "split {split}");
            assert_eq!(cursor, expected_end, "raw transport cursor, split {split}");
            assert_eq!(decoder.position(), expected_end);
            assert_eq!(controller.physical_position(), expected_end);
            assert_eq!(controller.doctype_range(), Some(2..expected_end));
            if let Some(oracle) = &oracle {
                assert_eq!(&seen, oracle)
            } else {
                oracle = Some(seen)
            }
        }
    }
}

#[test]
fn utf16_grammar_errors_map_scalar_boundaries_and_generated_errors_keep_outer_reference() {
    let text = "<!DOCTYPE 根 [<!ELEMENT 根 (\u{f0000})>]>";
    let offset = text.find('\u{f0000}').unwrap();
    for little_endian in [false, true] {
        let encoding = SourceEncoding::Utf16 {
            little_endian,
            bom: true,
        };
        let (_, _, _, result) = collect(
            text.as_bytes(),
            offset + 1,
            encoding,
            2,
            ControllerLimits::default(),
        );
        let error = result.unwrap_err();
        assert_eq!(
            error.physical_offset,
            2 + text[..offset].encode_utf16().count() * 2
        );
        let text = "<!DOCTYPE 根 [<!ENTITY % p '<!ELEMENT 1 EMPTY>'>%p;]>";
        let (_, _, _, result) =
            collect(text.as_bytes(), 0, encoding, 2, ControllerLimits::default());
        let error = result.unwrap_err();
        assert_eq!(
            error.physical_offset,
            2 + text[..text.find("%p;").unwrap()].encode_utf16().count() * 2
        );
        assert!(error.local_offset.is_some());
    }
}

#[test]
fn primary_position_pauses_until_generated_items_and_ready_events_are_released() {
    let text = b"<!DOCTYPE r [<!ENTITY % p '<!--a--><!--b-->'>%p;]><r/>";
    let mut state = ParserState::new(Limits::default(), 0);
    let mut c = DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
    let mut cursor = 0;
    for _ in 0..2 {
        cursor += c.advance(&text[cursor..], state.owned_budget()).unwrap();
        c.release();
    }
    cursor += c.advance(&text[cursor..], state.owned_budget()).unwrap();
    assert_eq!(&text[cursor..], b"]><r/>");
    let position = c.physical_position();
    let first = c.current().unwrap().1;
    assert_eq!(c.advance(b"]><r/>", state.owned_budget()), Ok(0));
    assert_eq!(c.current().unwrap().1, first);
    c.release();
    assert_eq!(c.advance(&text[cursor..], state.owned_budget()), Ok(0));
    assert_eq!(c.physical_position(), position);
    assert!(matches!(c.current().unwrap().0,DtdEvent::Item{unit,..} if unit.raw()=="<!--b-->"));
    c.release();
    assert_eq!(c.advance(&text[cursor..], state.owned_budget()), Ok(2));
    assert!(matches!(c.current().unwrap().0, DtdEvent::End));
}

#[test]
fn physical_invalid_grammar_utf8_and_partial_eof_are_sticky_at_all_splits() {
    for input in [
        &b"<!DOCTYPE r [<!ELEMENT 1 EMPTY>]>"[..],
        &b"<!DOCTYPE r [<!DOCTYPE r>]>"[..],
        &b"<!DOCTYPE r [<!ENTITY x 'unterminated"[..],
        &b"<!DOCTYPE r [<!--\xff-->]>"[..],
        &b"<!DOCTYPE r [%1"[..],
    ] {
        let (_, _, _, baseline) = collect(
            input,
            0,
            SourceEncoding::Utf8,
            11,
            ControllerLimits::default(),
        );
        let error = baseline.unwrap_err();
        assert!(error.local_offset.is_none());
        for split in 0..=input.len() {
            let (_, _, _, actual) = collect(
                input,
                split,
                SourceEncoding::Utf8,
                11,
                ControllerLimits::default(),
            );
            assert_eq!(actual, Err(error), "{input:?} split {split}");
        }
    }
}

#[test]
fn generated_declarations_bind_new_parameter_entities_without_invalidating_parent() {
    let text =
        "<!DOCTYPE r [<!ENTITY % p \"<!ENTITY &#37; q '<!--new-->'>&#37;q;<!--tail-->\">%p;]>";
    let (controller, seen, _, result) = collect(
        text.as_bytes(),
        0,
        SourceEncoding::Utf8,
        0,
        ControllerLimits::default(),
    );
    result.unwrap();
    assert_eq!(
        seen.iter()
            .map(|event| event.0.as_str())
            .collect::<Vec<_>>(),
        [
            "BEGIN r",
            "<!ENTITY % p \"<!ENTITY &#37; q '<!--new-->'>&#37;q;<!--tail-->\">",
            "<!ENTITY % q '<!--new-->'>",
            "<!--new-->",
            "<!--tail-->",
            "END"
        ]
    );
    assert_eq!(controller.counters().reference_attempts, 2);
    assert!(matches!(
        controller.catalog().lookup_parameter("q"),
        EntityLookup::Internal {
            replacement: "<!--new-->",
            ..
        }
    ));
    let tail = seen.iter().find(|event| event.0 == "<!--tail-->").unwrap();
    assert_eq!(tail.1.replacement_range(), Some(29..40));
}

#[test]
fn same_lexical_ge_pe_names_and_first_binding_remain_separate() {
    let text = "<!DOCTYPE r [<!ENTITY p 'general'><!ENTITY % p '<!--first-->'><!ENTITY % p '<!--ignored-->'>%p;<!ATTLIST r a CDATA '&p;'>]>";
    let (controller, seen, _, result) = collect(
        text.as_bytes(),
        0,
        SourceEncoding::Utf8,
        0,
        ControllerLimits::default(),
    );
    result.unwrap();
    assert!(seen.iter().any(|event| event.0 == "<!--first-->"));
    assert!(seen.iter().all(|event| event.0 != "<!--ignored-->"));
    assert!(matches!(
        controller.catalog().lookup_general("p"),
        EntityLookup::Internal {
            replacement: "general",
            ..
        }
    ));
    assert_eq!(
        controller
            .catalog()
            .attributes("r")
            .next()
            .unwrap()
            .default(),
        crate::dtd_catalog::DefaultValue::Value {
            fixed: false,
            value: "general"
        }
    );
}

#[test]
fn direct_attribute_errors_and_invalid_duplicate_grammar_still_reach_the_controller() {
    let cases = [
        (
            "<!DOCTYPE r [<!ATTLIST r a CDATA '&missing;'>]>",
            ControllerErrorKind::Catalog(CatalogErrorKind::EntityDeclared),
        ),
        (
            "<!DOCTYPE r [<!ENTITY e SYSTEM 'urn:x'><!ATTLIST r a CDATA '&e;'>]>",
            ControllerErrorKind::Catalog(CatalogErrorKind::ExternalInAttribute),
        ),
        (
            "<!DOCTYPE r [<!ENTITY e '&#60;'><!ATTLIST r a CDATA '&e;'>]>",
            ControllerErrorKind::Catalog(CatalogErrorKind::LessThanInAttribute),
        ),
    ];
    for (text, kind) in cases {
        let (_, _, _, result) = collect(
            text.as_bytes(),
            0,
            SourceEncoding::Utf8,
            0,
            ControllerLimits::default(),
        );
        assert_eq!(result.unwrap_err().kind, kind);
    }
    let text = "<!DOCTYPE r [<!ENTITY e 'first'><!ENTITY e BAD>]>";
    let (_, _, _, result) = collect(
        text.as_bytes(),
        0,
        SourceEncoding::Utf8,
        0,
        ControllerLimits::default(),
    );
    assert!(matches!(
        result.unwrap_err().kind,
        ControllerErrorKind::Grammar(_)
    ));
}

#[test]
fn a_fresh_shared_budget_each_call_cannot_reset_controller_copy_allowance() {
    let text = b"<!DOCTYPE r [<!--a--><!--b-->]>";
    let limits = ControllerLimits {
        max_owned_bytes: 21,
        ..ControllerLimits::default()
    };
    let mut c = DtdController::new(SourceEncoding::Utf8, 0, limits, false);
    let mut cursor = 0;
    loop {
        let mut state = ParserState::new(Limits::default(), 0);
        match c.advance(&text[cursor..], state.owned_budget()) {
            Ok(consumed) => {
                cursor += consumed;
                c.release();
            }
            Err(error) => {
                assert_eq!(
                    error.kind,
                    ControllerErrorKind::Limit(ControllerLimit::OwnedBytes)
                );
                break;
            }
        }
    }
    assert_eq!(c.counters().owned_bytes, 21);
    assert!(c.current().is_none());
}
