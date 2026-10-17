//! Blind publication contract tests: only the accepted callback base was read.
use super::*;
use crate::Limit;
use crate::dtd::LiteralSource;
use crate::dtd_controller::content::ContentErrorKind;
use crate::dtd_reference::ReferenceLimits;
use crate::tokenizer::text::TextScratchLimits;
use std::borrow::Cow;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};

const DOC: &str = "<!DOCTYPE r [<!ENTITY e '<n/>x'><!ENTITY z ''>]><r>a&e;b&z;c</r>";
const TOKEN: usize = 4096;
const WORK: usize = 64 * 1024 * 1024;

fn configured(mut limits: Limits, mut decode: DecodeLimits) -> StreamingSession {
    limits.max_token_bytes = limits.max_token_bytes.min(TOKEN);
    decode.max_decoded_token_bytes = decode.max_decoded_token_bytes.min(TOKEN);
    StreamingSession::new(
        limits,
        decode,
        SessionLimits {
            declarations: ControllerLimits::default(),
            text: TextLimits {
                max_work: WORK,
                reference: ReferenceLimits {
                    max_bytes: TOKEN,
                    max_scratch_bytes: TOKEN,
                    max_work: WORK,
                },
                scratch: TextScratchLimits {
                    input_token_bytes: TOKEN,
                    replacement_token_bytes: TOKEN,
                    max_bytes: TOKEN * 3,
                },
            },
            local: LocalLimits {
                max_token_bytes: TOKEN,
                max_scratch_bytes: TOKEN,
                max_work: WORK,
            },
        },
    )
}

fn session() -> StreamingSession {
    configured(Limits::default(), DecodeLimits::default())
}

fn encoded(text: &str, little: Option<bool>) -> Vec<u8> {
    let Some(little) = little else {
        return text.as_bytes().to_vec();
    };
    let mut bytes = if little {
        vec![255, 254]
    } else {
        vec![254, 255]
    };
    for unit in text.encode_utf16() {
        bytes.extend(if little {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Value {
    Declaration(String, Option<String>, Option<bool>),
    Begin(String, bool),
    Item(String, LiteralSource),
    DtdEnd,
    Start(String, Vec<(String, String)>),
    Empty(String, Vec<(String, String)>),
    End(String),
    Text(String),
    Cdata(String),
    Comment(String),
    Pi(String, String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Row {
    value: Value,
    input: Option<Range<usize>>,
    local: Option<Range<usize>>,
    physical: Range<usize>,
}

fn capture(event: LocatedEvent<'_>, origin: SourceOrigin) -> Row {
    let value = match event {
        LocatedEvent::Dtd(DtdEvent::Begin(header)) => {
            Value::Begin(header.root_name().into(), header.has_internal_subset())
        }
        LocatedEvent::Dtd(DtdEvent::Item { unit, literals }) => {
            Value::Item(unit.raw().into(), literals)
        }
        LocatedEvent::Dtd(DtdEvent::End) => Value::DtdEnd,
        LocatedEvent::Xml(event) => match event {
            Event::Declaration(d) => Value::Declaration(
                d.version.into(),
                d.encoding.map(str::to_owned),
                d.standalone,
            ),
            Event::Start(e) => Value::Start(
                e.name.qualified().into(),
                e.attributes
                    .into_iter()
                    .map(|a| (a.name.qualified().into(), a.value.into_owned()))
                    .collect(),
            ),
            Event::Empty(e) => Value::Empty(
                e.name.qualified().into(),
                e.attributes
                    .into_iter()
                    .map(|a| (a.name.qualified().into(), a.value.into_owned()))
                    .collect(),
            ),
            Event::End(n) => Value::End(n.qualified().into()),
            Event::Text(s) => Value::Text(s.into_owned()),
            Event::Cdata(s) => Value::Cdata(s.into_owned()),
            Event::Comment(s) => Value::Comment(s.into_owned()),
            Event::ProcessingInstruction { target, data } => {
                Value::Pi(target.into_owned(), data.into_owned())
            }
        },
    };
    Row {
        value,
        input: origin.input_range(),
        local: origin.replacement_range(),
        physical: origin.physical_context(),
    }
}

fn expected(little: Option<bool>) -> Vec<Row> {
    let values = [
        Value::Begin("r".into(), true),
        Value::Item("<!ENTITY e '<n/>x'>".into(), LiteralSource::Physical),
        Value::Item("<!ENTITY z ''>".into(), LiteralSource::Physical),
        Value::DtdEnd,
        Value::Start("r".into(), vec![]),
        Value::Text("a".into()),
        Value::Empty("n".into(), vec![]),
        Value::Text("x".into()),
        Value::Text("b".into()),
        Value::Text("c".into()),
        Value::End("r".into()),
    ];
    let spans = [
        0..13,
        13..32,
        32..46,
        46..48,
        48..51,
        51..52,
        52..55,
        52..55,
        55..56,
        59..60,
        60..64,
    ];
    values
        .into_iter()
        .zip(spans)
        .enumerate()
        .map(|(i, (value, span))| {
            // This fixture is ASCII: transport coordinates scale by two; entity
            // local offsets remain UTF-8 and do not include a transport BOM.
            let physical = if little.is_some() {
                span.start * 2 + 2..span.end * 2 + 2
            } else {
                span
            };
            let local = match i {
                6 => Some(0..4),
                7 => Some(4..5),
                _ => None,
            };
            Row {
                value,
                input: local.is_none().then(|| physical.clone()),
                local,
                physical,
            }
        })
        .collect()
}

/// Repeating preparation must neither reparse nor publish nor consume suffix.
fn stable_ready(stream: &mut StreamingSession, input: &[u8]) {
    let position = stream.position();
    let origin = stream.event_origin();
    let counters = stream
        .dtd_counters()
        .map(|c| (c.physical_bytes, c.reference_attempts));
    for _ in 0..3 {
        let step = stream.prepare(input).unwrap();
        assert_eq!(step.consumed, 0);
        assert_eq!(step.outcome, SessionOutcome::Ready);
        assert_eq!(stream.position(), position);
        assert_eq!(stream.event_origin(), origin);
        assert_eq!(
            stream
                .dtd_counters()
                .map(|c| (c.physical_bytes, c.reference_attempts)),
            counters
        );
    }
}

fn publish(stream: &mut StreamingSession, suffix: &[u8], rows: &mut Vec<Row>) {
    try_publish(stream, suffix, rows).unwrap();
}

fn try_publish(
    stream: &mut StreamingSession,
    suffix: &[u8],
    rows: &mut Vec<Row>,
) -> Result<(), SessionError> {
    stable_ready(stream, suffix);
    let (event, origin) = stream.lend()?;
    rows.push(capture(event, origin));
    assert_eq!(stream.event_origin(), Some(origin));
    let position = stream.position();
    stream.acknowledge()?;
    assert_eq!(
        stream.position(),
        position,
        "acknowledgement consumes no transport"
    );
    Ok(())
}

fn drain_finish(stream: &mut StreamingSession, rows: &mut Vec<Row>) -> Result<(), SessionError> {
    for _ in 0..128 {
        let step = stream.finish_step()?;
        assert_eq!(step.consumed, 0);
        match step.outcome {
            SessionOutcome::Ready => try_publish(stream, &[], rows)?,
            SessionOutcome::Complete => return Ok(()),
            SessionOutcome::NeedInput => panic!("EOF must not request more transport"),
        }
    }
    panic!("nonterminating EOF drain")
}

fn parts(
    stream: &mut StreamingSession,
    bytes: &[u8],
    cuts: &[usize],
    rows: &mut Vec<Row>,
) -> Result<(), SessionError> {
    let mut cursor = 0;
    for end in cuts.iter().copied().chain(std::iter::once(bytes.len())) {
        assert!((cursor..=bytes.len()).contains(&end));
        for _ in 0..256 {
            let step = stream.prepare(&bytes[cursor..end])?;
            assert!(step.consumed <= end - cursor);
            cursor += step.consumed;
            assert_eq!(stream.position(), cursor);
            match step.outcome {
                SessionOutcome::Ready => try_publish(stream, &bytes[cursor..end], rows)?,
                SessionOutcome::NeedInput => {
                    assert_eq!(cursor, end);
                    break;
                }
                SessionOutcome::Complete => panic!("transport input does not declare EOF"),
            }
        }
        assert_eq!(
            cursor, end,
            "bounded progress must consume the offered prefix"
        );
    }
    Ok(())
}

fn invalid(error: SessionError) {
    assert!(
        matches!(
            error,
            SessionError::Xml(Error {
                kind: ErrorKind::InvalidState,
                ..
            }) | SessionError::Content(ContentError {
                kind: ContentErrorKind::InvalidState,
                ..
            }) | SessionError::Dtd(ControllerError {
                kind: crate::dtd_controller::ControllerErrorKind::InvalidState,
                ..
            })
        ),
        "expected lifecycle InvalidState, got {error:?}"
    );
}

fn sticky(stream: &mut StreamingSession, error: SessionError) {
    let position = stream.position();
    let origin = stream.event_origin();
    assert_eq!(stream.prepare(b"ignored").unwrap_err(), error);
    assert_eq!(stream.finish_step().unwrap_err(), error);
    assert_eq!(stream.lend().unwrap_err(), error);
    assert_eq!(stream.acknowledge(), Err(error));
    assert_eq!(
        stream.advance(b"ignored", &mut |_, _, _| panic!("terminal callback")),
        Err(error)
    );
    assert_eq!(
        stream.finish(&mut |_, _, _| panic!("terminal EOF callback")),
        Err(error)
    );
    assert_eq!(stream.position(), position);
    assert_eq!(stream.event_origin(), origin);
}

#[test]
fn callback_control_matches_literal_entities_order_and_origins() {
    for little in [None, Some(true), Some(false)] {
        let bytes = encoded(DOC, little);
        let mut stream = session();
        let mut rows = Vec::new();
        let accepted = stream
            .advance(&bytes, &mut |e, o, _| {
                rows.push(capture(e, o));
                Ok(())
            })
            .unwrap();
        assert_eq!(accepted, bytes.len());
        stream
            .finish(&mut |e, o, _| {
                rows.push(capture(e, o));
                Ok(())
            })
            .unwrap();
        assert_eq!(rows, expected(little));
    }
}

#[test]
fn all_transport_splits_deliver_each_physical_and_generated_event_once() {
    for little in [None, Some(true), Some(false)] {
        let bytes = encoded(DOC, little);
        for cuts in (0..=bytes.len())
            .map(|cut| vec![cut])
            .chain(std::iter::once((1..bytes.len()).collect()))
        {
            let mut stream = session();
            let mut rows = Vec::new();
            parts(&mut stream, &bytes, &cuts, &mut rows).unwrap();
            drain_finish(&mut stream, &mut rows).unwrap();
            assert_eq!(rows, expected(little), "{little:?}, cuts {cuts:?}");
            assert_eq!(stream.dtd_counters().unwrap().reference_attempts, 2);
            assert_eq!(stream.position(), bytes.len());
            for _ in 0..3 {
                let step = stream.finish_step().unwrap();
                assert_eq!(step.consumed, 0);
                assert_eq!(step.outcome, SessionOutcome::Complete);
                stream
                    .finish(&mut |_, _, _| panic!("duplicate EOF event"))
                    .unwrap();
            }
        }
    }
}

#[test]
fn prefix_acceptance_precedes_entity_scheduling_and_generated_work_uses_no_suffix() {
    for little in [None, Some(true), Some(false)] {
        let bytes = encoded(DOC, little);
        let boundary = if little.is_some() { 112 } else { 55 };
        let mut stream = session();
        let mut rows = Vec::new();
        let mut cursor = 0;
        while rows.len() < 6 {
            let step = stream.prepare(&bytes[cursor..boundary]).unwrap();
            cursor += step.consumed;
            assert_eq!(step.outcome, SessionOutcome::Ready);
            if rows.len() == 5 {
                assert_eq!(cursor, boundary);
                assert_eq!(stream.dtd_counters().unwrap().reference_attempts, 0);
                stable_ready(&mut stream, &bytes[boundary..]);
            }
            publish(&mut stream, &[], &mut rows);
        }
        for index in [6, 7] {
            let step = stream.prepare(&[]).unwrap();
            assert_eq!(step.consumed, 0);
            assert_eq!(step.outcome, SessionOutcome::Ready);
            assert_eq!(stream.position(), boundary);
            assert_eq!(stream.dtd_counters().unwrap().reference_attempts, 1);
            publish(&mut stream, &bytes[boundary..], &mut rows);
            assert_eq!(rows[index], expected(little)[index]);
        }
        // Supply the withheld suffix only after both generated publications.
        let mut suffix_rows = Vec::new();
        let mut suffix_cursor = boundary;
        for _ in 0..128 {
            let step = stream.prepare(&bytes[suffix_cursor..]).unwrap();
            suffix_cursor += step.consumed;
            if step.outcome == SessionOutcome::Ready {
                publish(&mut stream, &[], &mut suffix_rows);
            } else {
                assert_eq!(suffix_cursor, bytes.len());
                break;
            }
        }
        assert_eq!(suffix_cursor, bytes.len());
        drain_finish(&mut stream, &mut suffix_rows).unwrap();
        rows.extend(suffix_rows);
        assert_eq!(rows, expected(little));
    }
}

#[test]
fn physical_payloads_lend_and_generated_payloads_own_without_extending_the_source_loan() {
    let raw = "<!DOCTYPE r [<!ENTITY e 'x'>]><r>p&e;z</r>";
    let mut stream = session();
    let mut cursor = 0;
    let mut text = Vec::new();
    let mut retained_generated = None;
    for _ in 0..128 {
        let step = stream.prepare(&raw.as_bytes()[cursor..]).unwrap();
        cursor += step.consumed;
        if step.outcome == SessionOutcome::NeedInput {
            break;
        }
        let (event, source) = stream.lend().unwrap();
        if let LocatedEvent::Xml(Event::Text(value)) = event {
            if source.input_range().is_some() {
                assert!(matches!(value, Cow::Borrowed(_)));
                text.push(value.to_string());
            } else {
                let Cow::Owned(value) = value else {
                    panic!("replacement text must own");
                };
                text.push(value.clone());
                // Moving an owned field out of the loan permits acknowledgement;
                // the retained String must remain usable after later parses.
                retained_generated = Some(value);
            }
        }
        stream.acknowledge().unwrap();
    }
    assert_eq!(cursor, raw.len());
    stream
        .finish(&mut |_, _, _| panic!("unexpected EOF publication"))
        .unwrap();
    assert_eq!(text, ["p", "x", "z"]);
    drop(stream);
    assert_eq!(retained_generated.as_deref(), Some("x"));
}

#[test]
fn every_lifecycle_misuse_is_sticky_without_releasing_the_publication() {
    for raw in ["<r/>", "<!DOCTYPE r><r/>"] {
        for misuse in 0..9 {
            let mut stream = session();
            let mut cursor = 0;
            if misuse >= 2 {
                let step = stream.prepare(raw.as_bytes()).unwrap();
                cursor += step.consumed;
                assert_eq!(step.outcome, SessionOutcome::Ready);
            }
            if misuse >= 3 {
                let _ = stream.lend().unwrap();
            }
            if misuse == 8 {
                stream.acknowledge().unwrap();
            }
            let failure = match misuse {
                0 => stream.lend().unwrap_err(),
                1 | 2 => stream.acknowledge().unwrap_err(),
                3 => stream.lend().unwrap_err(),
                4 => stream.prepare(&raw.as_bytes()[cursor..]).unwrap_err(),
                5 => stream.finish_step().unwrap_err(),
                6 => stream
                    .finish(&mut |_, _, _| panic!("finish must not acknowledge a loan"))
                    .unwrap_err(),
                7 => stream
                    .advance(&raw.as_bytes()[cursor..], &mut |_, _, _| {
                        panic!("feed must not acknowledge a loan")
                    })
                    .unwrap_err(),
                _ => stream.acknowledge().unwrap_err(),
            };
            invalid(failure);
            sticky(&mut stream, failure);
        }
    }
}

#[test]
fn successful_eof_is_terminal_for_publication_and_input_but_repeats_eof() {
    for misuse in 0..4 {
        let mut stream = session();
        let mut rows = Vec::new();
        parts(&mut stream, b"<r/>", &[], &mut rows).unwrap();
        drain_finish(&mut stream, &mut rows).unwrap();
        assert_eq!(rows.len(), 1);
        let failure = match misuse {
            0 => stream.prepare(&[]).unwrap_err(),
            1 => stream.lend().unwrap_err(),
            2 => stream.acknowledge().unwrap_err(),
            _ => stream
                .advance(b"<s/>", &mut |_, _, _| panic!("post-EOF publication"))
                .unwrap_err(),
        };
        invalid(failure);
        assert_eq!(failure.physical_offset(), 4);
        sticky(&mut stream, failure);
    }
}

#[test]
fn explicit_acknowledgement_can_switch_between_lending_and_callback_delivery() {
    let mut stream = session();
    let step = stream.prepare(b"<r>").unwrap();
    assert_eq!((step.consumed, step.outcome), (3, SessionOutcome::Ready));
    let (event, source) = stream.lend().unwrap();
    assert_eq!(
        capture(event, source).value,
        Value::Start("r".into(), vec![])
    );
    stream.acknowledge().unwrap();
    let mut rows = Vec::new();
    assert_eq!(
        stream
            .advance(b"&amp;<n/>", &mut |e, o, _| {
                rows.push(capture(e, o));
                Ok(())
            })
            .unwrap(),
        9
    );
    assert_eq!(
        rows.iter().map(|r| &r.value).collect::<Vec<_>>(),
        [&Value::Text("&".into()), &Value::Empty("n".into(), vec![])]
    );
    let step = stream.prepare(b"</r>").unwrap();
    assert_eq!(step.outcome, SessionOutcome::Ready);
    publish(&mut stream, &[], &mut rows);
    drain_finish(&mut stream, &mut rows).unwrap();
    assert_eq!(rows.last().unwrap().value, Value::End("r".into()));
}

#[test]
fn eof_prepares_last_text_once_before_reporting_the_missing_end_tag() {
    for little in [None, Some(true), Some(false)] {
        for prefix in ["<r>", "<!DOCTYPE r><r>"] {
            let raw = format!("{prefix}α🙂");
            let bytes = encoded(&raw, little);
            let mut stream = session();
            let mut rows = Vec::new();
            parts(
                &mut stream,
                &bytes,
                &(1..bytes.len()).collect::<Vec<_>>(),
                &mut rows,
            )
            .unwrap();
            assert!(!rows.iter().any(|r| matches!(r.value, Value::Text(_))));
            let step = stream.finish_step().unwrap();
            assert_eq!((step.consumed, step.outcome), (0, SessionOutcome::Ready));
            publish(&mut stream, &[], &mut rows);
            let last = rows.last().unwrap();
            assert_eq!(last.value, Value::Text("α🙂".into()));
            let start = if little.is_some() {
                prefix.len() * 2 + 2
            } else {
                prefix.len()
            };
            assert_eq!(last.input, Some(start..bytes.len()));
            let failure = stream.finish_step().unwrap_err();
            assert_eq!(failure.physical_offset(), bytes.len());
            assert!(matches!(
                failure,
                SessionError::Xml(Error {
                    kind: ErrorKind::UnexpectedEof,
                    ..
                }) | SessionError::Content(ContentError {
                    kind: ContentErrorKind::Parser(ErrorKind::UnexpectedEof),
                    ..
                })
            ));
            sticky(&mut stream, failure);
        }
    }
}

#[test]
fn top_level_whitespace_never_surfaces_as_a_ready_publication() {
    let raw = " \r\n<!--p--><!DOCTYPE r><r/><?after data?>\t ";
    let mut stream = session();
    let mut rows = Vec::new();
    parts(
        &mut stream,
        raw.as_bytes(),
        &(1..raw.len()).collect::<Vec<_>>(),
        &mut rows,
    )
    .unwrap();
    drain_finish(&mut stream, &mut rows).unwrap();
    assert_eq!(
        rows.into_iter().map(|r| r.value).collect::<Vec<_>>(),
        [
            Value::Comment("p".into()),
            Value::Begin("r".into(), false),
            Value::DtdEnd,
            Value::Empty("r".into(), vec![]),
            Value::Pi("after".into(), "data".into()),
        ]
    );
}

#[test]
fn ready_retries_spend_event_and_normalization_budgets_only_once() {
    for maximum in [5, 6, 7] {
        let mut stream = configured(
            Limits {
                max_owned_bytes: maximum,
                max_events: 3,
                ..Limits::default()
            },
            DecodeLimits::default(),
        );
        let mut rows = Vec::new();
        let result = parts(&mut stream, b"<r>&amp;</r>", &[], &mut rows)
            .and_then(|()| drain_finish(&mut stream, &mut rows));
        if maximum < 6 {
            let failure = result.unwrap_err();
            assert!(matches!(
                failure,
                SessionError::Xml(Error {
                    kind: ErrorKind::LimitExceeded(Limit::OwnedBytes),
                    ..
                })
            ));
            assert_eq!(rows.len(), 1);
            sticky(&mut stream, failure);
        } else {
            result.unwrap();
            assert_eq!(
                rows.into_iter().map(|r| r.value).collect::<Vec<_>>(),
                [
                    Value::Start("r".into(), vec![]),
                    Value::Text("&".into()),
                    Value::End("r".into())
                ]
            );
        }
    }
    for maximum in [10, 11, 12] {
        let mut stream = configured(
            Limits {
                max_events: maximum,
                ..Limits::default()
            },
            DecodeLimits::default(),
        );
        let mut rows = Vec::new();
        let result = parts(&mut stream, DOC.as_bytes(), &[], &mut rows)
            .and_then(|()| drain_finish(&mut stream, &mut rows));
        if maximum < 11 {
            let failure = result.unwrap_err();
            assert_eq!(rows.len(), 10);
            assert_eq!(rows, expected(None)[..10]);
            assert!(matches!(
                failure,
                SessionError::Content(ContentError {
                    kind: ContentErrorKind::Parser(ErrorKind::LimitExceeded(Limit::Events)),
                    ..
                })
            ));
            sticky(&mut stream, failure);
        } else {
            result.unwrap();
            assert_eq!(rows, expected(None));
        }
    }
}

#[test]
fn callback_rejection_of_a_prefix_never_schedules_the_following_entity() {
    for kind in [ErrorKind::Aborted, ErrorKind::Syntax] {
        let mut stream = session();
        let mut prefix = Vec::new();
        let mut cursor = 0;
        while prefix.len() < 5 {
            let step = stream
                .prepare(DOC.as_bytes().get(cursor..).unwrap())
                .unwrap();
            cursor += step.consumed;
            assert_eq!(step.outcome, SessionOutcome::Ready);
            publish(&mut stream, &[], &mut prefix);
        }
        let mut delivered = Vec::new();
        let failure = stream
            .advance(&DOC.as_bytes()[cursor..], &mut |event, source, _| {
                delivered.push(capture(event, source));
                Err(Error::new(
                    kind,
                    if kind == ErrorKind::Aborted {
                        source.cancellation_offset()
                    } else {
                        777
                    },
                ))
            })
            .unwrap_err();
        assert_eq!(delivered, expected(None)[5..6]);
        assert_eq!(stream.dtd_counters().unwrap().reference_attempts, 0);
        assert_eq!(stream.position(), 55);
        assert_eq!(
            failure.physical_offset(),
            if kind == ErrorKind::Aborted { 52 } else { 777 }
        );
        assert!(
            matches!(failure, SessionError::Content(ContentError { kind: ContentErrorKind::Callback(actual), .. }) if actual == kind)
        );
        sticky(&mut stream, failure);
    }
}

#[test]
fn callback_panics_poison_both_physical_and_generated_publications() {
    for (panic_at, position) in [(0, 13), (1, 32), (5, 55), (6, 55), (7, 55)] {
        let mut stream = session();
        let mut count = 0;
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            stream.advance(DOC.as_bytes(), &mut |_, _, _| {
                let index = count;
                count += 1;
                if index == panic_at {
                    panic!("intentional publication panic");
                }
                Ok(())
            })
        }));
        assert!(outcome.is_err());
        assert_eq!(count, panic_at + 1);
        assert_eq!(stream.position(), position);
        let failure = stream.prepare(b"ignored").unwrap_err();
        assert!(
            matches!(failure, SessionError::Poisoned { physical_offset } if physical_offset == position)
        );
        sticky(&mut stream, failure);
    }
}
