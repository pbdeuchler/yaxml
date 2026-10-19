use super::*;
use crate::tokenizer::local::LocalLimits;
use std::num::NonZeroU64;
fn limits() -> TextLimits {
    TextLimits {
        max_work: 1000,
        reference: ReferenceLimits {
            max_bytes: 100,
            max_scratch_bytes: 100,
            max_work: 1000,
        },
        scratch: TextScratchLimits {
            input_token_bytes: 100,
            replacement_token_bytes: 100,
            max_bytes: 300,
        },
    }
}
fn shared() -> TextShared {
    TextShared::new(limits(), 0).unwrap()
}
fn input() -> TextCoordinator<'static> {
    TextCoordinator::new(
        shared(),
        TextSourceLease::streaming(Tokenizer::new(100, 100)),
    )
    .unwrap()
}
fn id(value: u64) -> EntityInvocationId {
    EntityInvocationId(NonZeroU64::new(value).unwrap())
}
fn local() -> LocalTokenizer {
    LocalTokenizer::new(
        0,
        LocalLimits {
            max_token_bytes: 100,
            max_scratch_bytes: 100,
            max_work: 1000,
        },
    )
}

#[test]
fn general_waits_for_prefix_acknowledgement_and_handoff_keeps_cursor_and_work() {
    let mut c = input();
    assert_eq!(
        c.advance(b"pre&e;post").unwrap(),
        TextStep {
            consumed: 6,
            outcome: TextOutcome::TokenReady
        }
    );
    let token = c.current().unwrap();
    assert_eq!(token.kind, TokenKind::Text);
    assert_eq!(token.raw, b"pre");
    assert!(matches!(token.location, TextLocation::Input(span) if span.source == (0..3)));
    assert_eq!(c.input_position(), Some((6, 6)));
    assert!(c.general().is_none());
    assert_eq!(c.advance(b"post").unwrap().consumed, 0);
    c.release_token().unwrap();
    let (reference, extent, source) = c.general().unwrap();
    assert_eq!(reference.raw(), "&e;");
    assert_eq!(extent.input_span().unwrap().source, 3..6);
    assert_eq!(source, TextSourceId::Input);
    assert_eq!(
        c.advance(b"post").unwrap(),
        TextStep {
            consumed: 0,
            outcome: TextOutcome::GeneralReferenceReady
        }
    );
    let (shared, source) = c.acknowledge_general().unwrap().into_parts();
    assert_eq!(shared.usage().work, 15);
    let mut c = TextCoordinator::new(shared, source).unwrap();
    assert_eq!(c.advance(b"post<r/>").unwrap().consumed, 5);
    assert_eq!(c.current().unwrap().raw, b"post");
    c.release_token().unwrap();
    assert_eq!(c.advance(b"r/>").unwrap().consumed, 3);
    assert_eq!(c.current().unwrap().raw, b"<r/>");
}

#[test]
fn numeric_and_predefined_raw_append_does_not_rescan_source_or_character_output() {
    let mut c = input();
    assert_eq!(c.advance(b"pre&amp;x<r/>").unwrap().consumed, 10);
    assert_eq!(c.current().unwrap().raw, b"pre&amp;x");
    assert_eq!(
        c.usage(),
        TextUsage {
            work: 34,
            source_scanned: 5,
            reference_scanned: 5,
            reference_copied: 5,
            reference_classified: 5,
            appended_reference_copied: 5,
            validated: 9
        }
    );
    c.release_token().unwrap();
    c.advance(b"r/>").unwrap();
    assert_eq!(c.usage().work, 41);
    assert!(c.general().is_none());
}

#[test]
fn stable_window_is_sealed_and_preserves_nonzero_utf8_positions() {
    let mut tokenizer = Tokenizer::new(100, 100);
    tokenizer.configure(SourceEncoding::Utf8, 10);
    let lease = TextSourceLease::stable_utf8(tokenizer, "é&e;z", 10).unwrap();
    let mut c = TextCoordinator::new(shared(), lease).unwrap();
    assert_eq!(c.advance_borrowed().unwrap().consumed, 5);
    assert_eq!(c.current().unwrap().raw, "é".as_bytes());
    c.release_token().unwrap();
    let (shared, source) = c.acknowledge_general().unwrap().into_parts();
    let mut c = TextCoordinator::new(shared, source).unwrap();
    assert_eq!(
        c.advance_borrowed().unwrap(),
        TextStep {
            consumed: 1,
            outcome: TextOutcome::NeedInput
        }
    );
    c.finish_step().unwrap();
    assert!(
        matches!(c.current().unwrap().location, TextLocation::Input(span) if span.source == (15..16))
    );
    c.release_token().unwrap();
    assert_eq!(c.finish_step().unwrap().outcome, TextOutcome::Complete);
    let (_, source) = c.take_complete().unwrap().into_parts();
    assert_eq!(source.input_position(), Some((16, 16)));
    let lease = TextSourceLease::stable_utf8(Tokenizer::new(100, 100), "x", 0).unwrap();
    let mut c = TextCoordinator::new(self::shared(), lease).unwrap();
    let error = c.advance(b"foreign").unwrap_err();
    assert_eq!(error.kind, TextErrorKind::InvalidState);
    assert_eq!(c.finish_step().unwrap_err(), error);
    assert!(TextSourceLease::stable_utf8(Tokenizer::new(100, 100), "x", usize::MAX).is_err());
    let mut tokenizer = Tokenizer::new(100, 100);
    tokenizer.advance(b"foreign").unwrap();
    assert!(TextSourceLease::stable_utf8(tokenizer, "different window", 0).is_err());
    let lease = TextSourceLease::stable_utf8(Tokenizer::new(100, 100), "unread", 0).unwrap();
    let mut c = TextCoordinator::new(self::shared(), lease).unwrap();
    assert_eq!(
        c.finish_step().unwrap_err().kind,
        TextErrorKind::InvalidState
    );
}

#[test]
fn local_occurrence_switch_is_a_boundary_not_fake_eof_and_preserves_counters() {
    let mut used = local();
    used.advance(b"old").unwrap();
    assert!(TextSourceLease::local(used, id(1)).is_err());
    let mut lease = TextSourceLease::local(local(), id(1)).unwrap();
    assert_eq!(
        lease.switch_local_at_boundary(id(2), 0).unwrap_err().kind,
        TextErrorKind::InvalidState
    );
    let mut c = TextCoordinator::new(shared(), lease).unwrap();
    c.advance_local(id(1), b"pre&e;tail").unwrap();
    assert!(
        matches!(c.current().unwrap().location, TextLocation::Local { invocation, range } if invocation == id(1) && range == (0..3))
    );
    assert!(c.input_position().is_none());
    c.release_token().unwrap();
    let (shared, mut source) = c.acknowledge_general().unwrap().into_parts();
    assert_eq!(shared.usage().work, 15);
    source.switch_local_at_boundary(id(2), 0).unwrap();
    let mut c = TextCoordinator::new(shared, source).unwrap();
    assert_eq!(
        c.advance_local(id(2), b"x").unwrap().outcome,
        TextOutcome::NeedInput
    );
    c.finish_step().unwrap();
    assert_eq!(c.current().unwrap().raw, b"x");
    c.release_token().unwrap();
    c.finish_step().unwrap();
    let (shared, mut source) = c.take_complete().unwrap().into_parts();
    assert_eq!(shared.usage().work, 17);
    source.switch_local_at_boundary(id(1), 6).unwrap();
    let mut c = TextCoordinator::new(shared, source).unwrap();
    c.advance_local(id(1), b"tail").unwrap();
    c.finish_step().unwrap();
    assert!(
        matches!(c.current().unwrap().location, TextLocation::Local { invocation, range } if invocation == id(1) && range == (6..10))
    );
    assert_eq!(c.usage().work, 25);
    let mut c = TextCoordinator::new(
        self::shared(),
        TextSourceLease::local(local(), id(1)).unwrap(),
    )
    .unwrap();
    let error = c.advance_local(id(2), b"x").unwrap_err();
    assert_eq!(error.kind, TextErrorKind::SourceMismatch);
    assert_eq!(c.finish_step().unwrap_err(), error);
}

#[test]
fn global_work_is_checked_before_components_and_native_reference_limit_stays_distinct() {
    for (global, native, expected) in [
        (9, 100, None),
        (8, 100, Some(TextErrorKind::Work)),
        (
            100,
            8,
            Some(TextErrorKind::Reference(ReferenceErrorKind::Limit(
                ReferenceLimit::Work,
            ))),
        ),
    ] {
        let mut limits = limits();
        limits.max_work = global;
        limits.reference.max_work = native;
        let mut c = TextCoordinator::new(
            TextShared::new(limits, 0).unwrap(),
            TextSourceLease::streaming(Tokenizer::new(100, 100)),
        )
        .unwrap();
        let result = c.advance(b"&e;suffix");
        if let Some(expected) = expected {
            let error = result.unwrap_err();
            assert_eq!(error.kind, expected);
            assert_eq!(c.finish_step().unwrap_err(), error);
            assert!(c.usage().work <= global);
        } else {
            assert_eq!(
                result.unwrap(),
                TextStep {
                    consumed: 3,
                    outcome: TextOutcome::GeneralReferenceReady
                }
            );
            assert_eq!(c.usage().work, 9);
        }
        assert_eq!(c.input_position(), Some((3, 3)));
    }
    assert!(matches!(
        TextShared::new(
            TextLimits {
                max_work: 0,
                ..limits()
            },
            1
        ),
        Err(TextErrorKind::Work)
    ));
}

#[test]
fn eof_seals_pending_input_and_preserves_the_first_typed_failure() {
    let mut c = input();
    assert_eq!(c.advance(b"pre&e").unwrap().outcome, TextOutcome::NeedInput);
    let error = c.finish_step().unwrap_err();
    assert_eq!(
        error.kind,
        TextErrorKind::Reference(ReferenceErrorKind::UnexpectedEof)
    );
    assert_eq!(error.offset, 5);
    assert_eq!(c.advance(b";").unwrap_err(), error);
    assert!(c.current().is_none());
    let mut c = input();
    c.advance(b"a&e;").unwrap();
    assert_eq!(c.finish_step().unwrap().outcome, TextOutcome::TokenReady);
    c.release_token().unwrap();
    let (shared, source) = c.acknowledge_general().unwrap().into_parts();
    let mut c = TextCoordinator::new(shared, source).unwrap();
    assert_eq!(c.finish_step().unwrap().outcome, TextOutcome::Complete);
    let error = c.advance(b"late").unwrap_err();
    assert_eq!(error.kind, TextErrorKind::InvalidState);
    assert_eq!(c.release_token().unwrap_err(), error);
}

#[test]
fn scratch_reservation_counts_input_local_and_effective_reference_capacity() {
    // Both tokenizer slots remain reserved even while only one source is active.
    for total in [60, 61] {
        let mut policy = limits();
        policy.scratch = TextScratchLimits {
            input_token_bytes: 31,
            replacement_token_bytes: 19,
            max_bytes: total,
        };
        policy.reference.max_bytes = 17;
        policy.reference.max_scratch_bytes = 11;
        let result = TextShared::new(policy, 0);
        if total == 61 {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(TextErrorKind::Scratch)));
        }
    }
    for (input, local, reference) in [(usize::MAX, 1, 0), (usize::MAX, 0, 1)] {
        let mut policy = limits();
        policy.scratch = TextScratchLimits {
            input_token_bytes: input,
            replacement_token_bytes: local,
            max_bytes: usize::MAX,
        };
        policy.reference.max_bytes = reference;
        policy.reference.max_scratch_bytes = reference;
        assert!(matches!(
            TextShared::new(policy, 0),
            Err(TextErrorKind::ScratchOverflow)
        ));
    }
}

#[test]
fn source_slot_rejection_precedes_source_permission_work_or_scratch_mutation() {
    let mut policy = limits();
    policy.scratch = TextScratchLimits {
        input_token_bytes: 5,
        replacement_token_bytes: 7,
        max_bytes: 15,
    };
    policy.reference.max_bytes = 3;
    policy.reference.max_scratch_bytes = 3;
    for local_source in [false, true] {
        let shared = TextShared::new(policy, 17).unwrap();
        let source = if local_source {
            TextSourceLease::local(
                LocalTokenizer::new(
                    0,
                    LocalLimits {
                        max_token_bytes: 8,
                        max_scratch_bytes: 100,
                        max_work: 1000,
                    },
                ),
                id(1),
            )
            .unwrap()
        } else {
            TextSourceLease::streaming(Tokenizer::new(6, 100))
        };
        let before = shared.usage();
        let identity = source.id();
        let position = source.offset();
        let permission = source.boundary;
        let error = shared.check_source(&source).unwrap_err();
        assert_eq!(
            error,
            TextError {
                kind: TextErrorKind::Scratch,
                source: identity,
                offset: position
            }
        );
        assert_eq!(shared.usage(), before);
        assert!(shared.reference.is_none());
        assert_eq!(source.boundary, permission);
        assert_eq!(source.offset(), position);
        if let LeaseKind::Local { tokenizer, .. } = &source.kind {
            assert!(tokenizer.is_fresh_source());
            assert_eq!(tokenizer.work_ceiling(), 1000);
        }
        assert_eq!(TextCoordinator::new(shared, source).err().unwrap(), error);
    }
    let shared = TextShared::new(policy, 17).unwrap();
    let source = TextSourceLease::streaming(Tokenizer::new(5, 100));
    assert!(TextCoordinator::new(shared, source).is_ok());
    let shared = TextShared::new(policy, 17).unwrap();
    let source = TextSourceLease::local(
        LocalTokenizer::new(
            0,
            LocalLimits {
                max_token_bytes: 7,
                max_scratch_bytes: 100,
                max_work: 1000,
            },
        ),
        id(1),
    )
    .unwrap();
    assert!(TextCoordinator::new(shared, source).is_ok());
    let shared = TextShared::new(policy, 17).unwrap();
    let source = TextSourceLease::streaming(Tokenizer::new(5, 100));
    let mut c = TextCoordinator::new(shared, source).unwrap();
    let failure = c.advance(b"&1;").unwrap_err();
    let source = TextSourceLease::streaming(Tokenizer::new(6, 100));
    assert_eq!(c.shared.check_source(&source).unwrap_err(), failure);
    assert_eq!(
        TextCoordinator::new(c.shared, source).err().unwrap(),
        failure
    );
}

#[test]
fn retained_input_capacity_stays_reserved_while_local_source_uses_shared_scratch() {
    let mut tokenizer = Tokenizer::new(8, 100);
    // Simulate an allocation retained from a previous encoding/configuration.
    tokenizer.buffer.try_reserve_exact(12).unwrap();
    let retained = tokenizer.buffer.capacity();
    assert!(retained > 8);
    assert_eq!(tokenizer.scratch_capacity_requirement(), retained);
    let mut policy = limits();
    policy.scratch = TextScratchLimits {
        input_token_bytes: retained - 1,
        replacement_token_bytes: 7,
        max_bytes: retained + 9,
    };
    policy.reference.max_bytes = 3;
    policy.reference.max_scratch_bytes = 3;
    let source = TextSourceLease::streaming(tokenizer);
    let ledger = TextShared::new(policy, 0).unwrap();
    assert_eq!(
        ledger.check_source(&source).unwrap_err().kind,
        TextErrorKind::Scratch
    );
    policy.scratch.input_token_bytes = retained;
    policy.scratch.max_bytes = retained + 10;
    let mut c = TextCoordinator::new(TextShared::new(policy, 0).unwrap(), source).unwrap();
    c.advance(b"&e;z").unwrap();
    let (shared, paused_input) = c.acknowledge_general().unwrap().into_parts();
    assert_eq!(shared.limits.scratch.input_token_bytes, retained);
    assert_eq!(shared.limits.scratch.max_bytes, retained + 10);
    let local = TextSourceLease::local(
        LocalTokenizer::new(
            0,
            LocalLimits {
                max_token_bytes: 7,
                max_scratch_bytes: 100,
                max_work: 1000,
            },
        ),
        id(1),
    )
    .unwrap();
    let mut c = TextCoordinator::new(shared, local).unwrap();
    c.advance_local(id(1), b"x").unwrap();
    c.finish_step().unwrap();
    c.release_token().unwrap();
    c.finish_step().unwrap();
    let (shared, _) = c.take_complete().unwrap().into_parts();
    assert_eq!(shared.limits.scratch.input_token_bytes, retained);
    let mut c = TextCoordinator::new(shared, paused_input).unwrap();
    c.advance(b"z").unwrap();
    c.finish_step().unwrap();
    assert_eq!(c.current().unwrap().raw, b"z");
    assert_eq!(c.usage().work, 13);
}
