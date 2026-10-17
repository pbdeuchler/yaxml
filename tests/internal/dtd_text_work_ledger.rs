use super::*;
use crate::tokenizer::local::{LocalLimits, LocalTokenizer};
use std::num::NonZeroU64;

fn limits(max_work: usize) -> TextLimits {
    TextLimits {
        max_work,
        reference: ReferenceLimits {
            max_bytes: 16,
            max_scratch_bytes: 7,
            max_work,
        },
        scratch: TextScratchLimits {
            input_token_bytes: 11,
            replacement_token_bytes: 13,
            max_bytes: 31,
        },
    }
}

fn invocation(value: u64) -> EntityInvocationId {
    EntityInvocationId(NonZeroU64::new(value).unwrap())
}

#[test]
fn utf8_input_resume_requires_a_fresh_tokenizer_and_equal_source_positions() {
    let lease = TextSourceLease::resume_utf8(Tokenizer::new(11, 100), 17, 17).unwrap();
    assert_eq!(lease.input_position(), Some((17, 17)));
    assert_eq!(lease.input_encoding(), Some(SourceEncoding::Utf8));
    let mut coordinator =
        TextCoordinator::new(TextShared::new(limits(100), 0).unwrap(), lease).unwrap();
    coordinator.advance(b"x<r/>").unwrap();
    assert!(matches!(
        coordinator.current().unwrap().location,
        TextLocation::Input(span)
            if span.decoded == (17..18) && span.source == (17..18)
    ));

    for (tokenizer, decoded, physical) in [
        (Tokenizer::new(11, 100), 16, 17),
        (
            {
                let mut tokenizer = Tokenizer::new(11, 100);
                tokenizer.advance(b"used").unwrap();
                tokenizer
            },
            17,
            17,
        ),
        (
            {
                let mut tokenizer = Tokenizer::new(11, 100);
                tokenizer.configure(
                    SourceEncoding::Utf16 {
                        little_endian: true,
                        bom: true,
                    },
                    2,
                );
                tokenizer
            },
            17,
            17,
        ),
    ] {
        let error = match TextSourceLease::resume_utf8(tokenizer, decoded, physical) {
            Ok(_) => panic!("invalid resume accepted"),
            Err(error) => error,
        };
        assert_eq!(error.kind, TextErrorKind::InvalidState);
        assert_eq!(error.source, TextSourceId::Input);
        assert_eq!(error.offset, physical);
    }
    let local = TextSourceLease::local(
        LocalTokenizer::new(
            0,
            LocalLimits {
                max_token_bytes: 13,
                max_scratch_bytes: 13,
                max_work: 100,
            },
        ),
        invocation(1),
    )
    .unwrap();
    assert_eq!(local.input_encoding(), None);
}

#[test]
fn standalone_ledger_accepts_exact_work_and_rejects_excess_or_overflow() {
    for (initial, amount, maximum, accepted) in [
        (4, 6, 10, true),
        (4, 7, 10, false),
        (usize::MAX - 1, 1, usize::MAX, true),
        (usize::MAX - 1, 2, usize::MAX, false),
    ] {
        let mut shared = TextShared::new(limits(maximum), initial).unwrap();
        let before = shared.usage();
        let result = shared.charge_external_work(amount, TextSourceId::Input, 37);
        if accepted {
            result.unwrap();
            assert_eq!(shared.usage().work, initial + amount);
            assert_eq!(shared.remaining_work().unwrap(), maximum - initial - amount);
        } else {
            let error = result.unwrap_err();
            assert_eq!(
                error,
                TextError {
                    kind: TextErrorKind::Work,
                    source: TextSourceId::Input,
                    offset: 37,
                }
            );
            assert_eq!(shared.usage(), before);
            assert_eq!(shared.remaining_work().unwrap_err(), error);
        }
    }
}

#[test]
fn first_external_failure_is_sticky_without_debiting_rejected_work() {
    let mut shared = TextShared::new(limits(5), 4).unwrap();
    let before = shared.usage();
    let first = shared
        .charge_external_work(2, TextSourceId::Replacement(invocation(7)), 19)
        .unwrap_err();
    assert_eq!(shared.usage(), before);
    assert_eq!(shared.remaining_work().unwrap_err(), first);
    assert_eq!(
        shared
            .charge_external_work(0, TextSourceId::Input, 99)
            .unwrap_err(),
        first
    );
    assert_eq!(shared.usage(), before);

    let source = TextSourceLease::streaming(Tokenizer::new(11, 100));
    assert_eq!(shared.check_source(&source).unwrap_err(), first);
    assert_eq!(TextCoordinator::new(shared, source).err().unwrap(), first);
}

#[test]
fn coordinator_forwarding_preserves_native_failure_precedence() {
    let mut coordinator = TextCoordinator::new(
        TextShared::new(limits(100), 0).unwrap(),
        TextSourceLease::streaming(Tokenizer::new(11, 100)),
    )
    .unwrap();
    let failure = coordinator.advance(b"&1;").unwrap_err();
    let before = coordinator.usage();
    assert_eq!(coordinator.remaining_work().unwrap_err(), failure);
    assert_eq!(
        coordinator
            .charge_external_work(1, TextSourceId::Input, 88)
            .unwrap_err(),
        failure
    );
    assert_eq!(coordinator.usage(), before);
}

#[test]
fn handoffs_keep_one_work_ledger_and_the_original_scratch_reservation() {
    let policy = limits(100);
    let mut input = TextCoordinator::new(
        TextShared::new(policy, 3).unwrap(),
        TextSourceLease::streaming(Tokenizer::new(11, 100)),
    )
    .unwrap();
    assert_eq!(input.remaining_work().unwrap(), 97);
    input.advance(b"&e;tail").unwrap();
    let (mut shared, paused_input) = input.acknowledge_general().unwrap().into_parts();
    let after_reference = shared.usage().work;
    shared
        .charge_external_work(5, TextSourceId::Input, 0)
        .unwrap();

    let local = TextSourceLease::local(
        LocalTokenizer::new(
            0,
            LocalLimits {
                max_token_bytes: 13,
                max_scratch_bytes: 13,
                max_work: 100,
            },
        ),
        invocation(1),
    )
    .unwrap();
    let mut generated = TextCoordinator::new(shared, local).unwrap();
    generated
        .charge_external_work(7, TextSourceId::Replacement(invocation(1)), 0)
        .unwrap();
    generated.advance_local(invocation(1), b"x").unwrap();
    generated.finish_step().unwrap();
    generated.release_token().unwrap();
    generated.finish_step().unwrap();
    let (shared, _) = generated.take_complete().unwrap().into_parts();
    assert_eq!(shared.limits.scratch.input_token_bytes, 11);
    assert_eq!(shared.limits.scratch.replacement_token_bytes, 13);
    assert_eq!(shared.limits.scratch.max_bytes, 31);
    assert_eq!(shared.usage().work, after_reference + 5 + 7 + 2);

    let mut input = TextCoordinator::new(shared, paused_input).unwrap();
    assert_eq!(input.remaining_work().unwrap(), 100 - input.usage().work);
    input.advance(b"tail").unwrap();
    input.finish_step().unwrap();
    assert_eq!(input.current().unwrap().raw, b"tail");
}
