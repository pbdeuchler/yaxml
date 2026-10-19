use super::*;
use crate::Event;
use crate::tokenizer::local::{LocalLimits, LocalTokenizer};
use std::borrow::Cow;
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
fn input_position_overflow_is_typed_sticky_and_keeps_zero_width_utf16_bytes() {
    let mut exact = TextCoordinator::new(
        TextShared::new(limits(100), 0).unwrap(),
        TextSourceLease::resume_utf8(Tokenizer::new(11, 100), usize::MAX - 1, usize::MAX - 1)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(exact.advance(b"x").unwrap().consumed, 1);
    assert_eq!(exact.input_position(), Some((usize::MAX, usize::MAX)));
    let exact_usage = exact.usage();
    let overflow = exact.advance(b"y").unwrap_err();
    assert_eq!(overflow.kind, TextErrorKind::PositionOverflow);
    assert_eq!(overflow.source, TextSourceId::Input);
    assert_eq!(overflow.offset, usize::MAX);
    assert_eq!(exact.usage(), exact_usage);
    assert_eq!(exact.advance(b"").unwrap_err(), overflow);

    let mut partial = TextCoordinator::new(
        TextShared::new(limits(100), 0).unwrap(),
        TextSourceLease::resume_utf8(Tokenizer::new(11, 100), usize::MAX - 1, usize::MAX - 1)
            .unwrap(),
    )
    .unwrap();
    let error = partial.advance(b"xy").unwrap_err();
    assert_eq!(error.kind, TextErrorKind::PositionOverflow);
    assert_eq!(partial.input_position(), Some((usize::MAX, usize::MAX)));
    assert_eq!(partial.usage().source_scanned, 1);

    let utf16 = SourceEncoding::Utf16 {
        little_endian: true,
        bom: true,
    };
    let mut exact_tokenizer = Tokenizer::new(7, 100);
    exact_tokenizer.encoding = utf16;
    exact_tokenizer.position = 20;
    exact_tokenizer.start = 20;
    exact_tokenizer.source_position = usize::MAX - 2;
    exact_tokenizer.source_start = usize::MAX - 2;
    let mut exact = TextCoordinator::new(
        TextShared::new(limits(100), 0).unwrap(),
        TextSourceLease::streaming(exact_tokenizer),
    )
    .unwrap();
    assert_eq!(exact.advance(b"x").unwrap().consumed, 1);
    assert_eq!(exact.input_position(), Some((21, usize::MAX)));
    assert_eq!(
        exact.advance(b"y").unwrap_err().kind,
        TextErrorKind::PositionOverflow
    );

    let mut continuation = Tokenizer::new(7, 100);
    continuation.encoding = utf16;
    continuation.buffer.push(0xc2);
    continuation.start = 20;
    continuation.position = 21;
    continuation.source_start = usize::MAX - 2;
    continuation.source_position = usize::MAX;
    let mut continuation = TextCoordinator::new(
        TextShared::new(limits(100), 0).unwrap(),
        TextSourceLease::streaming(continuation),
    )
    .unwrap();
    assert_eq!(continuation.advance(&[0xa2]).unwrap().consumed, 1);
    assert_eq!(continuation.input_position(), Some((22, usize::MAX)));
    continuation.finish_step().unwrap();
    assert_eq!(continuation.current().unwrap().raw, "¢".as_bytes());
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
fn borrowed_event_can_stay_live_while_the_disjoint_work_ledger_is_charged() {
    let mut coordinator = TextCoordinator::new(
        TextShared::new(limits(100), 0).unwrap(),
        TextSourceLease::streaming(Tokenizer::new(11, 100)),
    )
    .unwrap();
    coordinator.advance(b"text<r/>").unwrap();
    let before = coordinator.usage().work;
    let (token, shared) = coordinator.current_with_work().unwrap();
    let raw = std::str::from_utf8(token.raw).unwrap();
    let event = Event::Text(Cow::Borrowed(raw));
    shared
        .charge_external_work(4, TextSourceId::Input, 0)
        .unwrap();
    assert_eq!(event, Event::Text(Cow::Borrowed("text")));
    assert_eq!(shared.usage().work, before + 4);
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
