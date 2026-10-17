use super::*;
use crate::encoding::{DecodeProgress, Decoded, Decoder};
use crate::tokenizer::local::LocalLimits;
use std::num::NonZeroU64;

const WORK: usize = 4_096;

fn id(value: u64) -> EntityInvocationId {
    EntityInvocationId(NonZeroU64::new(value).unwrap())
}

fn limits() -> TextLimits {
    TextLimits {
        max_work: WORK,
        reference: ReferenceLimits {
            max_bytes: 256,
            max_scratch_bytes: 256,
            max_work: WORK,
        },
        scratch: TextScratchLimits {
            input_token_bytes: 256,
            replacement_token_bytes: 256,
            max_bytes: 768,
        },
    }
}

fn shared() -> TextShared {
    TextShared::new(limits(), 0).unwrap()
}

fn input() -> TextCoordinator<'static> {
    TextCoordinator::new(
        shared(),
        TextSourceLease::streaming(Tokenizer::new(256, 256)),
    )
    .unwrap()
}

fn local(start: usize) -> LocalTokenizer {
    LocalTokenizer::new(
        start,
        LocalLimits {
            max_token_bytes: 256,
            max_scratch_bytes: 256,
            max_work: WORK,
        },
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SeenToken {
    kind: TokenKind,
    raw: Vec<u8>,
    decoded: Range<usize>,
    source: Range<usize>,
}

#[derive(Debug, Eq, PartialEq)]
struct SeenReference {
    raw: String,
    decoded: Range<usize>,
    source: Range<usize>,
}

#[derive(Default)]
struct Trace {
    tokens: Vec<SeenToken>,
    references: Vec<SeenReference>,
}

fn drain_input_ready(coordinator: &mut Option<TextCoordinator<'static>>, trace: &mut Trace) {
    loop {
        if coordinator.is_none() {
            return;
        }
        if let Some(token) = coordinator.as_ref().unwrap().current() {
            let TextLocation::Input(span) = token.location else {
                panic!("expected physical token")
            };
            trace.tokens.push(SeenToken {
                kind: token.kind,
                raw: token.raw.to_vec(),
                decoded: span.decoded.clone(),
                source: span.source.clone(),
            });
            coordinator.as_mut().unwrap().release_token().unwrap();
            continue;
        }
        let general = coordinator
            .as_ref()
            .unwrap()
            .general()
            .map(|(reference, extent, source)| {
                assert_eq!(source, TextSourceId::Input);
                let span = extent.input_span().unwrap();
                SeenReference {
                    raw: reference.raw().to_owned(),
                    decoded: span.decoded.clone(),
                    source: span.source.clone(),
                }
            });
        let Some(general) = general else {
            break;
        };
        trace.references.push(general);
        let current = coordinator.take().unwrap();
        let (shared, source) = current.acknowledge_general().unwrap().into_parts();
        *coordinator = Some(TextCoordinator::new(shared, source).unwrap());
    }
}

fn drive_utf8(chunks: &[&[u8]]) -> (Trace, TextUsage, (usize, usize)) {
    let mut coordinator = Some(input());
    let mut trace = Trace::default();
    for chunk in chunks {
        let mut cursor = 0;
        while cursor < chunk.len() {
            let step = coordinator
                .as_mut()
                .unwrap()
                .advance(&chunk[cursor..])
                .unwrap();
            cursor += step.consumed;
            drain_input_ready(&mut coordinator, &mut trace);
            assert!(step.consumed != 0 || step.outcome != TextOutcome::NeedInput);
        }
    }
    loop {
        let step = coordinator.as_mut().unwrap().finish_step().unwrap();
        drain_input_ready(&mut coordinator, &mut trace);
        if step.outcome == TextOutcome::Complete {
            break;
        }
    }
    let current = coordinator.unwrap();
    (trace, current.usage(), current.input_position().unwrap())
}

fn utf16_bytes(text: &str, little_endian: bool) -> Vec<u8> {
    let mut output = if little_endian {
        vec![0xff, 0xfe]
    } else {
        vec![0xfe, 0xff]
    };
    for unit in text.encode_utf16() {
        let bytes = if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        };
        output.extend_from_slice(&bytes);
    }
    output
}

fn drive_utf16(input: &[u8], split: usize) -> (Trace, TextUsage, (usize, usize)) {
    let mut decoder = Decoder::new(4_096);
    let mut coordinator: Option<TextCoordinator<'static>> = None;
    let mut trace = Trace::default();

    for piece in [&input[..split], &input[split..]] {
        let mut cursor = 0;
        loop {
            let progress = decoder
                .advance(&piece[cursor..], |decoded| match decoded {
                    Decoded::Start { encoding, bom } => {
                        let mut tokenizer = Tokenizer::new(256, 256);
                        tokenizer.configure(encoding, bom);
                        coordinator = Some(
                            TextCoordinator::new(shared(), TextSourceLease::streaming(tokenizer))
                                .unwrap(),
                        );
                        Ok(DecodeProgress {
                            consumed: 0,
                            suspended: false,
                        })
                    }
                    Decoded::Bytes(bytes) => {
                        let step = coordinator.as_mut().unwrap().advance(bytes).unwrap();
                        Ok(DecodeProgress {
                            consumed: step.consumed,
                            suspended: step.outcome != TextOutcome::NeedInput,
                        })
                    }
                })
                .unwrap();
            cursor += progress.consumed;
            drain_input_ready(&mut coordinator, &mut trace);
            if cursor == piece.len() && !progress.suspended {
                break;
            }
            assert!(progress.consumed != 0 || progress.suspended);
        }
    }
    while decoder
        .finish_step(|decoded| match decoded {
            Decoded::Start { encoding, bom } => {
                let mut tokenizer = Tokenizer::new(256, 256);
                tokenizer.configure(encoding, bom);
                coordinator = Some(
                    TextCoordinator::new(shared(), TextSourceLease::streaming(tokenizer)).unwrap(),
                );
                Ok(DecodeProgress {
                    consumed: 0,
                    suspended: false,
                })
            }
            Decoded::Bytes(bytes) => {
                let step = coordinator.as_mut().unwrap().advance(bytes).unwrap();
                Ok(DecodeProgress {
                    consumed: step.consumed,
                    suspended: step.outcome != TextOutcome::NeedInput,
                })
            }
        })
        .unwrap()
    {
        drain_input_ready(&mut coordinator, &mut trace);
    }
    drain_input_ready(&mut coordinator, &mut trace);
    loop {
        let step = coordinator.as_mut().unwrap().finish_step().unwrap();
        drain_input_ready(&mut coordinator, &mut trace);
        if step.outcome == TextOutcome::Complete {
            break;
        }
    }
    let current = coordinator.unwrap();
    (trace, current.usage(), current.input_position().unwrap())
}

#[test]
fn utf8_every_split_preserves_literal_tokens_reference_and_suffix() {
    let input = "pre&π;post".as_bytes();
    let expected_tokens = [
        SeenToken {
            kind: TokenKind::Text,
            raw: b"pre".to_vec(),
            decoded: 0..3,
            source: 0..3,
        },
        SeenToken {
            kind: TokenKind::Text,
            raw: b"post".to_vec(),
            decoded: 7..11,
            source: 7..11,
        },
    ];
    let expected_reference = SeenReference {
        raw: "&π;".to_owned(),
        decoded: 3..7,
        source: 3..7,
    };
    for split in 0..=input.len() {
        let (trace, _, position) = drive_utf8(&[&input[..split], &input[split..]]);
        assert_eq!(trace.tokens, expected_tokens, "split {split}");
        assert_eq!(
            trace.references.as_slice(),
            std::slice::from_ref(&expected_reference),
            "split {split}"
        );
        assert_eq!(position, (11, 11), "split {split}");
    }
}

#[test]
fn decoder_utf16_every_physical_split_preserves_supplementary_origins() {
    let expected_tokens = [
        SeenToken {
            kind: TokenKind::Text,
            raw: "😀x".as_bytes().to_vec(),
            decoded: 0..5,
            source: 2..8,
        },
        SeenToken {
            kind: TokenKind::Text,
            raw: b"z".to_vec(),
            decoded: 9..10,
            source: 14..16,
        },
    ];
    let expected_reference = SeenReference {
        raw: "&π;".to_owned(),
        decoded: 5..9,
        source: 8..14,
    };
    for little_endian in [true, false] {
        let input = utf16_bytes("😀x&π;z", little_endian);
        assert_eq!(input.len(), 16);
        for split in 0..=input.len() {
            let (trace, _, position) = drive_utf16(&input, split);
            assert_eq!(
                trace.tokens, expected_tokens,
                "le={little_endian} split={split}"
            );
            assert_eq!(
                trace.references.as_slice(),
                std::slice::from_ref(&expected_reference),
                "le={little_endian} split={split}"
            );
            assert_eq!(position, (10, 16), "le={little_endian} split={split}");
        }
    }
}

#[test]
fn numeric_and_predefined_spellings_are_appended_once_without_rescan() {
    let expected = SeenToken {
        kind: TokenKind::Text,
        raw: b"a&amp;&#x41;b".to_vec(),
        decoded: 0..13,
        source: 0..13,
    };
    for split in 0..=17 {
        let input = b"a&amp;&#x41;b<r/>";
        let (trace, usage, position) = drive_utf8(&[&input[..split], &input[split..]]);
        assert_eq!(trace.tokens[0], expected, "split {split}");
        assert_eq!(trace.tokens[1].kind, TokenKind::Start, "split {split}");
        assert_eq!(trace.tokens[1].raw, b"<r/>", "split {split}");
        assert!(trace.references.is_empty());
        assert_eq!(usage.reference_classified, 11);
        assert_eq!(usage.appended_reference_copied, 11);
        assert_eq!(position, (17, 17));
    }
}

#[test]
fn stable_window_and_terminal_states_are_sticky() {
    let lease = TextSourceLease::stable_utf8(Tokenizer::new(32, 32), "ok", 0).unwrap();
    let mut coordinator = TextCoordinator::new(shared(), lease).unwrap();
    let error = coordinator.advance(b"foreign").unwrap_err();
    assert_eq!(error.kind, TextErrorKind::InvalidState);
    assert_eq!(coordinator.advance_borrowed().unwrap_err(), error);
    assert_eq!(coordinator.finish_step().unwrap_err(), error);

    let lease = TextSourceLease::stable_utf8(Tokenizer::new(32, 32), "unread", 0).unwrap();
    let mut coordinator = TextCoordinator::new(shared(), lease).unwrap();
    let error = coordinator.finish_step().unwrap_err();
    assert_eq!(error.kind, TextErrorKind::InvalidState);
    assert_eq!(coordinator.advance_borrowed().unwrap_err(), error);

    let lease = TextSourceLease::stable_utf8(Tokenizer::new(32, 32), "done", 0).unwrap();
    let mut coordinator = TextCoordinator::new(shared(), lease).unwrap();
    assert_eq!(coordinator.advance_borrowed().unwrap().consumed, 4);
    coordinator.finish_step().unwrap();
    assert_eq!(coordinator.current().unwrap().raw, b"done");
    coordinator.release_token().unwrap();
    assert_eq!(
        coordinator.finish_step().unwrap().outcome,
        TextOutcome::Complete
    );
    assert_eq!(
        coordinator.advance_borrowed().unwrap().outcome,
        TextOutcome::Complete
    );
    let error = coordinator.release_token().unwrap_err();
    assert_eq!(error.kind, TextErrorKind::InvalidState);
    assert_eq!(coordinator.finish_step().unwrap_err(), error);

    let mut coordinator = input();
    coordinator.advance(b"x").unwrap();
    coordinator.finish_step().unwrap();
    coordinator.release_token().unwrap();
    assert_eq!(
        coordinator.finish_step().unwrap().outcome,
        TextOutcome::Complete
    );
    let error = coordinator.advance(b"late").unwrap_err();
    assert_eq!(error.kind, TextErrorKind::InvalidState);
    assert_eq!(coordinator.finish_step().unwrap_err(), error);

    let mut coordinator = input();
    coordinator.advance(b"&broken").unwrap();
    let error = coordinator.finish_step().unwrap_err();
    assert_eq!(
        error.kind,
        TextErrorKind::Reference(ReferenceErrorKind::UnexpectedEof)
    );
    assert_eq!(coordinator.advance(b";").unwrap_err(), error);
}

#[test]
fn local_invocation_switches_only_at_acknowledged_or_complete_boundaries() {
    let mut coordinator =
        TextCoordinator::new(shared(), TextSourceLease::local(local(0), id(1)).unwrap()).unwrap();
    let mismatch = coordinator.advance_local(id(2), b"x").unwrap_err();
    assert_eq!(mismatch.kind, TextErrorKind::SourceMismatch);
    assert_eq!(coordinator.finish_step().unwrap_err(), mismatch);

    let mut coordinator =
        TextCoordinator::new(shared(), TextSourceLease::local(local(0), id(1)).unwrap()).unwrap();
    assert_eq!(
        coordinator
            .advance_local(id(1), b"a&e;tail")
            .unwrap()
            .consumed,
        4
    );
    assert_eq!(coordinator.current().unwrap().raw, b"a");
    assert!(coordinator.general().is_none());
    coordinator.release_token().unwrap();
    assert_eq!(coordinator.general().unwrap().0.raw(), "&e;");
    let (shared, mut source) = coordinator.acknowledge_general().unwrap().into_parts();
    source.switch_local_at_boundary(id(2), 10).unwrap();
    let mut coordinator = TextCoordinator::new(shared, source).unwrap();
    coordinator.advance_local(id(2), b"child").unwrap();
    coordinator.finish_step().unwrap();
    let TextLocation::Local { invocation, range } = coordinator.current().unwrap().location else {
        panic!("expected local child")
    };
    assert_eq!((invocation, range), (id(2), 10..15));
    coordinator.release_token().unwrap();
    coordinator.finish_step().unwrap();
    let work_after_child = coordinator.usage().work;
    let (shared, mut source) = coordinator.take_complete().unwrap().into_parts();
    source.switch_local_at_boundary(id(1), 4).unwrap();
    let mut coordinator = TextCoordinator::new(shared, source).unwrap();
    coordinator.advance_local(id(1), b"tail").unwrap();
    coordinator.finish_step().unwrap();
    assert_eq!(coordinator.current().unwrap().raw, b"tail");
    assert!(coordinator.usage().work > work_after_child);
}

#[test]
fn global_and_native_reference_work_boundaries_remain_distinct() {
    let cases = [
        (9, 100, None),
        (8, 100, Some(TextErrorKind::Work)),
        (
            100,
            8,
            Some(TextErrorKind::Reference(ReferenceErrorKind::Limit(
                ReferenceLimit::Work,
            ))),
        ),
    ];
    for (global, native, expected) in cases {
        let mut policy = limits();
        policy.max_work = global;
        policy.reference.max_work = native;
        let mut coordinator = TextCoordinator::new(
            TextShared::new(policy, 0).unwrap(),
            TextSourceLease::streaming(Tokenizer::new(32, 32)),
        )
        .unwrap();
        match expected {
            None => {
                let step = coordinator.advance(b"&e;suffix").unwrap();
                assert_eq!(step.consumed, 3);
                assert_eq!(step.outcome, TextOutcome::GeneralReferenceReady);
                assert_eq!(coordinator.usage().work, 9);
            }
            Some(expected) => {
                let error = coordinator.advance(b"&e;suffix").unwrap_err();
                assert_eq!(error.kind, expected);
                assert_eq!(coordinator.usage().work, 6);
                assert_eq!(coordinator.input_position(), Some((3, 3)));
                assert_eq!(coordinator.finish_step().unwrap_err(), error);
            }
        }
    }
}

#[test]
fn scratch_totals_slots_overflow_and_capacity_formulas_are_literal() {
    for (input_slot, local_slot, reference, total, expected) in [
        (31, 19, 11, 61, Ok(())),
        (31, 19, 11, 60, Err(TextErrorKind::Scratch)),
        (
            usize::MAX,
            1,
            0,
            usize::MAX,
            Err(TextErrorKind::ScratchOverflow),
        ),
    ] {
        let mut policy = limits();
        policy.scratch = TextScratchLimits {
            input_token_bytes: input_slot,
            replacement_token_bytes: local_slot,
            max_bytes: total,
        };
        policy.reference.max_bytes = reference;
        policy.reference.max_scratch_bytes = reference;
        assert_eq!(TextShared::new(policy, 0).map(|_| ()), expected);
    }

    for (encoding, raw, decoded, expected) in [
        (SourceEncoding::Utf8, 5, 3, 3),
        (
            SourceEncoding::Utf16 {
                little_endian: true,
                bom: true,
            },
            5,
            99,
            6,
        ),
        (
            SourceEncoding::Utf16 {
                little_endian: false,
                bom: true,
            },
            9,
            7,
            7,
        ),
        (
            SourceEncoding::Utf16 {
                little_endian: true,
                bom: false,
            },
            1,
            99,
            0,
        ),
    ] {
        let mut tokenizer = Tokenizer::new(raw, decoded);
        tokenizer.configure(encoding, 0);
        assert_eq!(tokenizer.scratch_capacity_requirement(), expected);
    }
}

#[test]
fn scratch_slot_failure_is_before_permission_work_and_scratch_mutation() {
    let mut policy = limits();
    policy.scratch = TextScratchLimits {
        input_token_bytes: 5,
        replacement_token_bytes: 7,
        max_bytes: 15,
    };
    policy.reference.max_bytes = 3;
    policy.reference.max_scratch_bytes = 3;

    for (source, exact) in [
        (TextSourceLease::streaming(Tokenizer::new(6, 64)), false),
        (
            TextSourceLease::local(
                LocalTokenizer::new(
                    0,
                    LocalLimits {
                        max_token_bytes: 8,
                        max_scratch_bytes: 64,
                        max_work: WORK,
                    },
                ),
                id(1),
            )
            .unwrap(),
            false,
        ),
        (TextSourceLease::streaming(Tokenizer::new(5, 64)), true),
        (
            TextSourceLease::local(
                LocalTokenizer::new(
                    0,
                    LocalLimits {
                        max_token_bytes: 7,
                        max_scratch_bytes: 64,
                        max_work: WORK,
                    },
                ),
                id(1),
            )
            .unwrap(),
            true,
        ),
    ] {
        let shared = TextShared::new(policy, 9).unwrap();
        let usage = shared.usage();
        let permission = source.boundary;
        let position = source.offset();
        let checked = shared.check_source(&source);
        if exact {
            assert!(checked.is_ok());
            assert!(TextCoordinator::new(shared, source).is_ok());
        } else {
            assert_eq!(checked.unwrap_err().kind, TextErrorKind::Scratch);
            assert_eq!(shared.usage(), usage);
            assert!(shared.reference.is_none());
            assert_eq!(source.boundary, permission);
            assert_eq!(source.offset(), position);
        }
    }
}

#[test]
fn retained_utf16_capacity_cannot_hide_after_utf8_reconfiguration_or_pause() {
    let mut tokenizer = Tokenizer::new(12, 18);
    tokenizer.configure(
        SourceEncoding::Utf16 {
            little_endian: true,
            bom: false,
        },
        0,
    );
    assert_eq!(
        tokenizer.advance_content(b"<r/>").unwrap().stop,
        ContentStop::TokenReady
    );
    tokenizer.release();
    let retained = tokenizer.buffer.capacity();
    assert!(retained >= 18);
    tokenizer.configure(SourceEncoding::Utf8, 0);
    assert_eq!(tokenizer.scratch_capacity_requirement(), retained);

    let mut policy = limits();
    policy.scratch.input_token_bytes = retained - 1;
    policy.scratch.max_bytes = (retained - 1) + 256 + 256;
    let source = TextSourceLease::streaming(tokenizer);
    let shared = TextShared::new(policy, 0).unwrap();
    assert_eq!(
        shared.check_source(&source).unwrap_err().kind,
        TextErrorKind::Scratch
    );

    policy.scratch.input_token_bytes = retained;
    policy.scratch.max_bytes = retained + 256 + 256;
    let mut coordinator =
        TextCoordinator::new(TextShared::new(policy, 0).unwrap(), source).unwrap();
    coordinator.advance(b"&e;z").unwrap();
    let (shared, paused_input) = coordinator.acknowledge_general().unwrap().into_parts();
    let local_source = TextSourceLease::local(local(0), id(2)).unwrap();
    let mut local_coordinator = TextCoordinator::new(shared, local_source).unwrap();
    local_coordinator.advance_local(id(2), b"x").unwrap();
    local_coordinator.finish_step().unwrap();
    local_coordinator.release_token().unwrap();
    local_coordinator.finish_step().unwrap();
    let (shared, _) = local_coordinator.take_complete().unwrap().into_parts();
    assert_eq!(shared.limits.scratch.input_token_bytes, retained);
    let mut resumed = TextCoordinator::new(shared, paused_input).unwrap();
    resumed.advance(b"z").unwrap();
    resumed.finish_step().unwrap();
    assert_eq!(resumed.current().unwrap().raw, b"z");
}
