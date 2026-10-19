use super::*;
use crate::Limits;
use crate::dtd_reference::ReferenceLimits;
use crate::tokenizer::Tokenizer;
use crate::tokenizer::text::TextScratchLimits;
use std::borrow::Cow;

fn make(dtd: &str) -> ContentController {
    make_limits(dtd, ControllerLimits::default())
}
fn make_limits(dtd: &str, limits: ControllerLimits) -> ContentController {
    let mut parser = ParserState::new(Limits::default(), 0);
    let mut declarations = DtdController::new(SourceEncoding::Utf8, 0, limits, false);
    let mut consumed = 0;
    while !declarations.is_complete() {
        consumed += declarations
            .advance(&dtd.as_bytes()[consumed..], parser.owned_budget())
            .unwrap();
        while declarations.take_current().is_some() {}
    }
    assert_eq!(consumed, dtd.len());
    let body = declarations.into_body_state().unwrap();
    let text_limits = TextLimits {
        max_work: limits.max_work,
        reference: ReferenceLimits {
            max_bytes: 256,
            max_scratch_bytes: 256,
            max_work: limits.max_work,
        },
        scratch: TextScratchLimits {
            input_token_bytes: 4096,
            replacement_token_bytes: 4096,
            max_bytes: 8448,
        },
    };
    let local = LocalLimits {
        max_token_bytes: 4096,
        max_scratch_bytes: 4096,
        max_work: limits.max_work,
    };
    ContentController::new(body, parser, Tokenizer::new(4096, 4096), text_limits, local).unwrap()
}
fn label(event: Event<'_>) -> String {
    match event {
        Event::Start(element) => format!("+{}", element.name.qualified()),
        Event::Empty(element) => format!(
            "={}{}",
            element.name.qualified(),
            element
                .attributes
                .iter()
                .map(|attr| format!(" {}={}", attr.name.qualified(), attr.value))
                .collect::<String>()
        ),
        Event::End(name) => format!("-{}", name.qualified()),
        Event::Text(text) => format!("T{text}"),
        Event::Comment(text) => format!("C{text}"),
        Event::Cdata(text) => format!("D{text}"),
        Event::ProcessingInstruction { target, data } => format!("P{target}:{data}"),
        Event::Declaration(_) | Event::Doctype(_) => "declaration".into(),
    }
}
fn run(
    controller: &mut ContentController,
    input: &str,
) -> Result<Vec<(String, SourceOrigin, Option<u64>)>, ContentError> {
    let mut rows = Vec::new();
    let mut consumed = 0;
    for _ in 0..1000 {
        let step = if consumed < input.len() {
            controller.advance(&input.as_bytes()[consumed..])?
        } else {
            controller.finish_step()?
        };
        consumed += step.consumed;
        match step.outcome {
            ContentOutcome::EventReady => {
                let invocation = controller
                    .active
                    .last()
                    .map(|active| active.invocation.get());
                controller.emit(&mut |event, origin, _| {
                    rows.push((label(event), origin, invocation));
                    Ok(())
                })?;
            }
            ContentOutcome::Complete => {
                assert_eq!(consumed, input.len());
                return Ok(rows);
            }
            ContentOutcome::Progress | ContentOutcome::NeedPhysicalInput => {}
        }
    }
    panic!("controller did not terminate");
}

#[test]
fn nested_repeated_empty_entities_keep_order_and_unique_occurrences() {
    let dtd = "<!DOCTYPE r [<!ENTITY % setup \"<!ENTITY leaf '<a/>tail'>\">%setup;<!ENTITY empty ''><!ENTITY outer 'before&leaf;after&empty;'>]>";
    let mut controller = make(dtd);
    let before = controller.frame_counters();
    let scanned_before = controller.counters().scanned_bytes;
    let rows = run(&mut controller, "<r>pre&outer;post&leaf;</r>").unwrap();
    assert_eq!(
        rows.iter().map(|row| row.0.as_str()).collect::<Vec<_>>(),
        [
            "+r", "Tpre", "Tbefore", "=a", "Ttail", "Tafter", "Tpost", "=a", "Ttail", "-r"
        ]
    );
    let generated = rows.iter().filter_map(|row| row.2).collect::<Vec<_>>();
    assert_eq!(generated, [2, 3, 3, 2, 5, 5]);
    assert_eq!(
        controller.frame_counters().reference_attempts,
        before.reference_attempts + 4
    );
    assert!(controller.counters().work > before.work);
    assert_eq!(controller.usage().work, controller.counters().work);
    assert_eq!(
        controller.counters().scanned_bytes - scanned_before,
        "<r>pre&outer;post&leaf;</r>".len()
            + "before&leaf;after&empty;".len()
            + 2 * "<a/>tail".len(),
        "each source byte, including a reference spelling, is scanned once"
    );
    assert_eq!(
        controller.position(),
        (
            dtd.len() + "<r>pre&outer;post&leaf;</r>".len(),
            dtd.len() + "<r>pre&outer;post&leaf;</r>".len()
        )
    );
}

#[test]
fn generated_defaults_and_replacement_leaf_phases_use_owned_values() {
    let dtd = "<!DOCTYPE r [<!ATTLIST a x NMTOKENS '  one  two '><!ENTITY e '<a/>&#13;<![CDATA[&#13;]]><!--&#13;--><?p x&#13;?>'>]>";
    let mut controller = make(dtd);
    let rows = run(&mut controller, "<r>&e;</r>").unwrap();
    assert_eq!(
        rows.iter().map(|row| row.0.as_str()).collect::<Vec<_>>(),
        ["+r", "=a x=one two", "T\r", "D\r", "C\r", "Pp:x\r", "-r"]
    );
    let mut controller = make("<!DOCTYPE r [<!ENTITY e 'text'>]>");
    assert_eq!(
        controller.advance(b"<r>&e;</r>").unwrap().outcome,
        ContentOutcome::EventReady
    );
    controller.emit(&mut |_, _, _| Ok(())).unwrap();
    assert_eq!(
        controller.advance(b"&e;</r>").unwrap().outcome,
        ContentOutcome::Progress
    );
    assert_eq!(
        controller.advance(b"</r>").unwrap().outcome,
        ContentOutcome::EventReady
    );
    controller
        .emit(&mut |event, origin, _| {
            assert!(origin.input_range().is_none());
            assert!(matches!(event, Event::Text(Cow::Owned(_))));
            Ok(())
        })
        .unwrap();
}

#[test]
fn prefix_cancellation_prevents_lookup_entry_and_suffix_consumption() {
    let dtd = "<!DOCTYPE r [<!ENTITY e '<a/>'>]>";
    let mut controller = make(dtd);
    let initial = controller.frame_counters();
    let first = controller.advance(b"<r>pre&missing;post</r>").unwrap();
    assert_eq!(first.consumed, 3);
    controller.emit(&mut |_, _, _| Ok(())).unwrap();
    let prefix = controller.advance(b"pre&missing;post</r>").unwrap();
    assert_eq!(prefix.consumed, "pre&missing;".len());
    let failure = controller
        .emit(&mut |event, _, _| {
            assert_eq!(label(event), "Tpre");
            Err(Error::new(ErrorKind::Aborted, dtd.len() + 6))
        })
        .unwrap_err();
    assert_eq!(failure.kind, ContentErrorKind::Callback(ErrorKind::Aborted));
    assert_eq!(failure.physical_offset, dtd.len() + 6);
    assert_eq!(controller.frame_counters(), initial);
    assert_eq!(controller.advance(b"post</r>").unwrap_err(), failure);
    assert_eq!(controller.finish_step().unwrap_err(), failure);
}

#[test]
fn malformed_cycles_unavailable_and_cross_boundary_closure_are_sticky() {
    for (value, kind) in [
        (
            "<a",
            ContentErrorKind::Text(TextErrorKind::Local(
                crate::tokenizer::local::LocalErrorKind::Xml(ErrorKind::UnexpectedEof),
            )),
        ),
        ("&e;", ContentErrorKind::Frame(FrameErrorKind::Cycle)),
        (
            "</r><r>",
            ContentErrorKind::Frame(FrameErrorKind::ElementBoundaryCrossing),
        ),
        (
            "<a>",
            ContentErrorKind::Frame(FrameErrorKind::ElementDepthMismatch),
        ),
    ] {
        let dtd = format!("<!DOCTYPE r [<!ENTITY e '{value}'>]>");
        let mut controller = make(&dtd);
        let failure = run(&mut controller, "<r>&e;</r>").unwrap_err();
        assert_eq!(failure.kind, kind, "{value}");
        assert_eq!(failure.physical_offset, dtd.len() + 3);
        assert!(failure.local_offset.is_some());
        assert_eq!(controller.advance(b"ignored").unwrap_err(), failure);
    }
    for (declaration, kind) in [
        ("", LookupFailure::Undeclared),
        (
            "<!NOTATION n SYSTEM 'urn:n'><!ENTITY e SYSTEM 'urn:e' NDATA n>",
            LookupFailure::Unparsed,
        ),
    ] {
        let mut controller = make(&format!("<!DOCTYPE r [{declaration}]>"));
        let error = run(&mut controller, "<r>&e;</r>").unwrap_err();
        assert_eq!(
            error.kind,
            ContentErrorKind::Frame(FrameErrorKind::Lookup(kind))
        );
    }
    let mut controller = make("<!DOCTYPE r [<!ENTITY e SYSTEM 'urn:denied'>]>");
    run(&mut controller, "<r>&e;</r>").unwrap();
}

#[test]
fn exact_global_work_and_native_frame_limits_precede_publication() {
    let dtd = "<!DOCTYPE r [<!ENTITY e '<a/>'>]>";
    let input = "<r>&e;</r>";
    let mut baseline = make(dtd);
    let expected = run(&mut baseline, input).unwrap();
    let work = baseline.counters().work;
    for (limit, success) in [(work, true), (work - 1, false)] {
        let mut controller = make_limits(
            dtd,
            ControllerLimits {
                max_work: limit,
                ..ControllerLimits::default()
            },
        );
        let result = run(&mut controller, input);
        if success {
            assert_eq!(result.unwrap(), expected);
        } else {
            assert_eq!(result.unwrap_err().kind, ContentErrorKind::Work);
            assert_eq!(
                controller.parser.element_depth(),
                1,
                "rejected final End must leave root scope"
            );
        }
        assert!(controller.counters().work <= limit);
    }
    for (frames, expected) in [
        (
            FrameLimits {
                max_work: 4,
                ..FrameLimits::default()
            },
            FrameLimit::Work,
        ),
        (
            FrameLimits {
                max_expanded_bytes: 3,
                ..FrameLimits::default()
            },
            FrameLimit::ExpandedBytes,
        ),
    ] {
        let mut controller = make_limits(
            dtd,
            ControllerLimits {
                frames,
                ..ControllerLimits::default()
            },
        );
        let failure = run(&mut controller, input).unwrap_err();
        assert_eq!(
            failure.kind,
            ContentErrorKind::Frame(FrameErrorKind::LimitExceeded(expected))
        );
        assert_eq!(controller.parser.element_depth(), 1);
    }
}

#[test]
fn constructor_resumes_encoding_and_rejects_used_tokenizers_and_inadequate_scratch() {
    fn body(encoding: SourceEncoding) -> DtdBodyState {
        let mut dtd = DtdController::new(encoding, 0, ControllerLimits::default(), false);
        let mut state = ParserState::new(Limits::default(), 0);
        let mut cursor = 0;
        while !dtd.is_complete() {
            cursor += dtd
                .advance(&b"<!DOCTYPE r>"[cursor..], state.owned_budget())
                .unwrap();
            while dtd.take_current().is_some() {}
        }
        dtd.into_body_state().unwrap()
    }
    let limits = TextLimits {
        max_work: 64 * 1024 * 1024,
        reference: ReferenceLimits {
            max_bytes: 16,
            max_scratch_bytes: 16,
            max_work: 1024,
        },
        scratch: TextScratchLimits {
            input_token_bytes: 128,
            replacement_token_bytes: 128,
            max_bytes: 272,
        },
    };
    let local = LocalLimits {
        max_token_bytes: 128,
        max_scratch_bytes: 128,
        max_work: 4096,
    };
    let inherited = body(SourceEncoding::Utf8);
    let inherited_max = inherited.limits.max_work;
    let inherited_spent = inherited.counters.work;
    let controller = ContentController::new(
        inherited,
        ParserState::new(Limits::default(), 0),
        Tokenizer::new(128, 128),
        TextLimits {
            max_work: usize::MAX,
            ..limits
        },
        local,
    )
    .unwrap();
    assert_eq!(
        controller.remaining().unwrap(),
        inherited_max - inherited_spent
    );
    assert_eq!(controller.counters().work, inherited_spent);
    // Constructor-only decoded-stream control; real transport and Decoder carry
    // are exercised by the UTF-16 streaming-session fixtures.
    let utf16 = ContentController::new(
        body(SourceEncoding::Utf16 {
            little_endian: true,
            bom: true,
        }),
        ParserState::new(Limits::default(), 0),
        Tokenizer::new(128, 128),
        limits,
        local,
    )
    .unwrap();
    assert_eq!(utf16.position(), (12, 24));
    let mut used = Tokenizer::new(128, 128);
    used.advance(b"<r/>").unwrap();
    used.release();
    assert_eq!(
        ContentController::new(
            body(SourceEncoding::Utf8),
            ParserState::new(Limits::default(), 0),
            used,
            limits,
            local
        )
        .err()
        .unwrap()
        .kind,
        ContentErrorKind::Text(TextErrorKind::InvalidState)
    );
    assert_eq!(
        ContentController::new(
            body(SourceEncoding::Utf8),
            ParserState::new(Limits::default(), 0),
            Tokenizer::new(129, 129),
            limits,
            local
        )
        .err()
        .unwrap()
        .kind,
        ContentErrorKind::Text(TextErrorKind::Scratch)
    );
    assert_eq!(
        ContentController::new(
            body(SourceEncoding::Utf8),
            ParserState::new(Limits::default(), 0),
            Tokenizer::new(128, 128),
            limits,
            LocalLimits {
                max_scratch_bytes: 129,
                max_token_bytes: 129,
                ..local
            }
        )
        .err()
        .unwrap()
        .kind,
        ContentErrorKind::Text(TextErrorKind::Scratch)
    );
}

#[test]
fn callback_unwind_poison_prevents_republication_and_entity_lookup() {
    let mut controller = make("<!DOCTYPE r [<!ENTITY e 'text'>]>");
    let step = controller.advance(b"<r>prefix&e;</r>").unwrap();
    assert_eq!(step.consumed, 3);
    controller.emit(&mut |_, _, _| Ok(())).unwrap();
    controller.advance(b"prefix&e;</r>").unwrap();
    let attempts = controller.frame_counters().reference_attempts;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        controller.emit(&mut |_, _, _| panic!("callback stopped"))
    }));
    assert!(outcome.is_err());
    let failure = controller.advance(b"</r>").unwrap_err();
    assert_eq!(failure.kind, ContentErrorKind::Poisoned);
    assert_eq!(controller.finish_step().unwrap_err(), failure);
    assert_eq!(
        controller
            .emit(&mut |_, _, _| panic!("must not republish"))
            .unwrap_err(),
        failure
    );
    assert_eq!(controller.frame_counters().reference_attempts, attempts);
    assert_eq!(controller.counters().work, controller.usage().work);
}

#[test]
fn unicode_source_scan_may_pause_inside_scalar_before_work_exhaustion() {
    let dtd = "<!DOCTYPE r [<!ENTITY e 'é'>]>";
    let mut probe = make(dtd);
    assert_eq!(probe.advance(b"<r>&e;</r>").unwrap().consumed, 3);
    probe.emit(&mut |_, _, _| Ok(())).unwrap();
    assert_eq!(
        probe.advance(b"&e;</r>").unwrap().outcome,
        ContentOutcome::Progress
    );
    let max_work = probe.counters().work + 1;
    let mut controller = make_limits(
        dtd,
        ControllerLimits {
            max_work,
            ..ControllerLimits::default()
        },
    );
    let failure = run(&mut controller, "<r>&e;</r>").unwrap_err();
    assert_eq!(failure.kind, ContentErrorKind::Work);
    assert!(controller.counters().work <= max_work);
    assert_eq!(controller.parser.element_depth(), 1);
}

#[test]
fn direct_lending_keeps_the_raw_source_until_acknowledgment() {
    let mut controller = make("<!DOCTYPE r [<!ENTITY e 'x'>]>");
    let first = controller.advance(b"<r>pre&e;post</r>").unwrap();
    assert_eq!(first.outcome, ContentOutcome::EventReady);
    let (event, _) = controller.lend().unwrap();
    assert_eq!(label(event.unwrap()), "+r");
    controller.acknowledge().unwrap();
    let step = controller
        .advance(&b"<r>pre&e;post</r>"[first.consumed..])
        .unwrap();
    assert_eq!(step.outcome, ContentOutcome::EventReady);
    let raw_pointer = controller
        .coordinator
        .as_ref()
        .unwrap()
        .current()
        .unwrap()
        .raw
        .as_ptr();
    let (event, origin) = controller.lend().unwrap();
    assert!(
        matches!(event, Some(Event::Text(Cow::Borrowed(text))) if text == "pre" && text.as_ptr() == raw_pointer)
    );
    assert!(origin.input_range().is_some());
    assert_eq!(controller.frame_counters().reference_attempts, 0);
    controller.acknowledge().unwrap();
    assert_eq!(controller.frame_counters().reference_attempts, 0);
    assert_eq!(
        controller.advance(&[]).unwrap().outcome,
        ContentOutcome::Progress
    );
    assert_eq!(controller.frame_counters().reference_attempts, 1);
    assert_eq!(
        controller.advance(&[]).unwrap().outcome,
        ContentOutcome::EventReady
    );
    let (event, origin) = controller.lend().unwrap();
    assert!(matches!(event, Some(Event::Text(Cow::Owned(value))) if value == "x"));
    assert_eq!(origin.replacement_range(), Some(0..1));
    controller.acknowledge().unwrap();
}

#[test]
fn failed_or_panicking_emit_retains_the_exact_lent_prefix_storage() {
    for generated in [false, true] {
        for panics in [false, true] {
            let mut controller =
                make("<!DOCTYPE r [<!ENTITY e 'pre&later;'><!ENTITY later 'tail'>]>");
            controller.advance(b"<r>").unwrap();
            controller.emit(&mut |_, _, _| Ok(())).unwrap();
            if generated {
                assert_eq!(
                    controller.advance(b"&e;after</r>").unwrap().outcome,
                    ContentOutcome::Progress
                );
                assert_eq!(
                    controller.advance(&[]).unwrap().outcome,
                    ContentOutcome::EventReady
                );
            } else {
                assert_eq!(
                    controller.advance(b"pre&later;after</r>").unwrap().outcome,
                    ContentOutcome::EventReady
                );
            }
            let pointer = controller
                .coordinator
                .as_ref()
                .unwrap()
                .current()
                .unwrap()
                .raw
                .as_ptr();
            let mut target_origin = None;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                controller.emit(&mut |event, origin, _| {
                    assert!(matches!(event, Event::Text(ref value) if value == "pre"));
                    target_origin = Some(origin);
                    if panics {
                        panic!("prefix callback stopped");
                    }
                    Err(Error::new(ErrorKind::Aborted, origin.cancellation_offset()))
                })
            }));
            let origin = target_origin.expect("prefix delivered");
            if panics {
                assert!(result.is_err());
            } else {
                let error = result.unwrap().unwrap_err();
                assert_eq!(error.kind, ContentErrorKind::Callback(ErrorKind::Aborted));
                assert_eq!(error.physical_offset, origin.cancellation_offset());
            }
            assert!(matches!(controller.delivery, Delivery::Lent));
            let coordinator = controller.coordinator.as_ref().unwrap();
            let token = coordinator
                .current()
                .expect("unacknowledged prefix retained");
            assert_eq!(token.raw, b"pre");
            assert_eq!(token.raw.as_ptr(), pointer);
            assert!(coordinator.general().is_none());
            assert_eq!(
                controller.frame_counters().reference_attempts,
                usize::from(generated)
            );
            let error = controller.finish_step().unwrap_err();
            assert_eq!(controller.acknowledge(), Err(error));
            assert!(matches!(controller.delivery, Delivery::Lent));
        }
    }
}
