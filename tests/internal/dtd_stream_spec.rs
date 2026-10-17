use super::*;
use crate::dtd_catalog::LiteralSource;
use crate::dtd_reference::ReferenceLimits;
use crate::tokenizer::text::TextScratchLimits;
use crate::{Attribute, Element, Limit, Name};
use std::panic::{AssertUnwindSafe, catch_unwind};

const TOKEN: usize = 4096;
const WORK: usize = 64 * 1024 * 1024;
const F0: &str = "<r/>";
const F1: &str = "<r>π😀&amp;</r>";
const F2: &str = "<!DOCTYPE r [<!ENTITY e 'x'>]><r>a&e;b</r>";
const F3: &str = "<!DOCTYPE r><r/>";

fn session_limits() -> SessionLimits {
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
    }
}

fn new_session(limits: Limits) -> StreamingSession {
    let limits = Limits {
        max_token_bytes: TOKEN,
        ..limits
    };
    StreamingSession::new(
        limits,
        DecodeLimits {
            max_decoded_token_bytes: TOKEN,
            ..DecodeLimits::default()
        },
        session_limits(),
    )
}

#[derive(Debug, Eq, PartialEq)]
enum SeenEvent {
    XmlDecl,
    Start(String),
    Empty(String),
    End(String),
    Text(String),
    Comment(String),
    Pi(String, String),
    DtdBegin { root: String, subset: bool },
    DtdItem { raw: String, source: LiteralSource },
    DtdEnd,
}

#[derive(Debug, Eq, PartialEq)]
struct Seen {
    event: SeenEvent,
    origin: SourceOrigin,
}

fn name(name: &Name<'_>) -> String {
    name.qualified().to_owned()
}

fn element(element: &Element<'_>) -> String {
    assert!(
        element
            .attributes
            .iter()
            .all(|Attribute { name, .. }| !name.qualified().is_empty())
    );
    name(&element.name)
}

fn snapshot(event: LocatedEvent<'_>, origin: SourceOrigin) -> Seen {
    let event = match event {
        LocatedEvent::Xml(Event::Declaration(_)) => SeenEvent::XmlDecl,
        LocatedEvent::Xml(Event::Start(value)) => SeenEvent::Start(element(&value)),
        LocatedEvent::Xml(Event::Empty(value)) => SeenEvent::Empty(element(&value)),
        LocatedEvent::Xml(Event::End(value)) => SeenEvent::End(name(&value)),
        LocatedEvent::Xml(Event::Text(value)) => SeenEvent::Text(value.into_owned()),
        LocatedEvent::Xml(Event::Cdata(value)) => SeenEvent::Text(value.into_owned()),
        LocatedEvent::Xml(Event::Comment(value)) => SeenEvent::Comment(value.into_owned()),
        LocatedEvent::Xml(Event::ProcessingInstruction { target, data }) => {
            SeenEvent::Pi(target.into_owned(), data.into_owned())
        }
        LocatedEvent::Dtd(DtdEvent::Begin(header)) => SeenEvent::DtdBegin {
            root: header.root_name().to_owned(),
            subset: header.has_internal_subset(),
        },
        LocatedEvent::Dtd(DtdEvent::Item { unit, literals }) => SeenEvent::DtdItem {
            raw: unit.raw().to_owned(),
            source: literals,
        },
        LocatedEvent::Dtd(DtdEvent::End) => SeenEvent::DtdEnd,
    };
    Seen { event, origin }
}

fn drive_parts(
    bytes: &[u8],
    cuts: &[usize],
    limits: Limits,
) -> Result<(Vec<Seen>, StreamingSession), SessionError> {
    let mut session = new_session(limits);
    let mut seen = Vec::new();
    let mut start = 0;
    let mut boundaries = cuts.to_vec();
    boundaries.push(bytes.len());
    for end in boundaries {
        assert!((start..=bytes.len()).contains(&end));
        let mut cursor = start;
        let mut transitions = 0;
        while cursor < end {
            transitions += 1;
            assert!(transitions <= 4096);
            let before = seen.len();
            let consumed = session.advance(&bytes[cursor..end], &mut |event, origin, _| {
                seen.push(snapshot(event, origin));
                Ok(())
            })?;
            assert!(consumed <= end - cursor);
            assert!(consumed != 0 || seen.len() != before);
            cursor += consumed;
        }
        start = end;
    }
    session.finish(&mut |event, origin, _| {
        seen.push(snapshot(event, origin));
        Ok(())
    })?;
    assert_eq!(
        session.event_origin(),
        seen.last().map(|event| event.origin)
    );
    Ok((seen, session))
}

fn utf16(value: &str, little: bool) -> Vec<u8> {
    let mut bytes = if little {
        vec![0xff, 0xfe]
    } else {
        vec![0xfe, 0xff]
    };
    for unit in value.encode_utf16() {
        bytes.extend(if little {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}

fn semantic(events: &[Seen]) -> Vec<&SeenEvent> {
    events.iter().map(|seen| &seen.event).collect()
}

#[test]
fn ordinary_utf8_utf16_parity_and_literal_transport_origins() {
    let (utf8, utf8_session) = drive_parts(F1.as_bytes(), &[4, 7], Limits::default()).unwrap();
    assert_eq!(utf8_session.position(), F1.len());
    assert_eq!(
        semantic(&utf8),
        [
            &SeenEvent::Start("r".into()),
            &SeenEvent::Text("π😀&".into()),
            &SeenEvent::End("r".into()),
        ]
    );
    assert_eq!(
        utf8.iter()
            .map(|e| e.origin.input_range())
            .collect::<Vec<_>>(),
        [Some(0..3), Some(3..14), Some(14..18)]
    );

    for little in [true, false] {
        let bytes = utf16(F1, little);
        let (events, mut session) =
            drive_parts(&bytes, &[1, 3, 11, 15], Limits::default()).unwrap();
        assert_eq!(semantic(&events), semantic(&utf8));
        assert_eq!(
            events
                .iter()
                .map(|e| e.origin.input_range())
                .collect::<Vec<_>>(),
            [Some(2..8), Some(8..24), Some(24..32)]
        );
        assert_eq!(session.position(), bytes.len());
        assert_eq!(session.finish(&mut |_, _, _| Ok(())), Ok(()));
    }
}

#[test]
fn every_utf8_split_preserves_dtd_order_fields_and_origins() {
    let expected_events = [
        SeenEvent::DtdBegin {
            root: "r".into(),
            subset: true,
        },
        SeenEvent::DtdItem {
            raw: "<!ENTITY e 'x'>".into(),
            source: LiteralSource::Physical,
        },
        SeenEvent::DtdEnd,
        SeenEvent::Start("r".into()),
        SeenEvent::Text("a".into()),
        SeenEvent::Text("x".into()),
        SeenEvent::Text("b".into()),
        SeenEvent::End("r".into()),
    ];
    let expected_input = [
        Some(0..13),
        Some(13..28),
        Some(28..30),
        Some(30..33),
        Some(33..34),
        None,
        Some(37..38),
        Some(38..42),
    ];
    for cut in 0..=F2.len() {
        let (events, session) = drive_parts(F2.as_bytes(), &[cut], Limits::default()).unwrap();
        assert_eq!(
            events.iter().map(|e| &e.event).collect::<Vec<_>>(),
            expected_events.iter().collect::<Vec<_>>(),
            "cut {cut}"
        );
        assert_eq!(
            events
                .iter()
                .map(|e| e.origin.input_range())
                .collect::<Vec<_>>(),
            expected_input,
            "cut {cut}"
        );
        assert_eq!(events[5].origin.replacement_range(), Some(0..1));
        assert_eq!(events[5].origin.physical_context(), 34..37);
        let counters = session.dtd_counters().unwrap();
        assert_eq!(counters.physical_bytes, 42);
        assert_eq!(counters.reference_attempts, 1);
        assert_eq!(session.position(), F2.len());
    }
    let cuts = (1..F2.len()).collect::<Vec<_>>();
    let (events, _) = drive_parts(F2.as_bytes(), &cuts, Limits::default()).unwrap();
    assert_eq!(events.len(), 8);
}

#[test]
fn empty_and_generated_entities_make_finite_progress_without_transport_replay() {
    let empty = "<!DOCTYPE r [<!ENTITY e ''>]><r>a&e;b</r>";
    let (events, session) = drive_parts(empty.as_bytes(), &[], Limits::default()).unwrap();
    assert_eq!(session.position(), empty.len());
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.event, SeenEvent::Text(_)))
            .map(|e| &e.event)
            .collect::<Vec<_>>(),
        [&SeenEvent::Text("a".into()), &SeenEvent::Text("b".into())]
    );

    let generated = "<!DOCTYPE r [<!ENTITY e '<n/>x'>]><r>&e;tail</r>";
    let reference_end = generated.find("tail").unwrap();
    let mut prefix_session = new_session(Limits::default());
    let mut prefix_events = Vec::new();
    let consumed = prefix_session
        .advance(
            &generated.as_bytes()[..reference_end],
            &mut |event, origin, _| {
                prefix_events.push(snapshot(event, origin));
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(consumed, reference_end);
    assert_eq!(prefix_session.position(), reference_end);
    assert_eq!(
        prefix_events
            .iter()
            .filter(|event| event.origin.replacement_range().is_some())
            .count(),
        2,
        "generated Empty and Text drain before any tail byte is offered"
    );

    let (events, session) =
        drive_parts(generated.as_bytes(), &[reference_end], Limits::default()).unwrap();
    assert_eq!(session.position(), generated.len());
    let generated_events = events
        .iter()
        .filter(|e| e.origin.replacement_range().is_some())
        .collect::<Vec<_>>();
    assert_eq!(generated_events.len(), 2);
    assert_eq!(generated_events[0].event, SeenEvent::Empty("n".into()));
    assert_eq!(generated_events[1].event, SeenEvent::Text("x".into()));
    assert_eq!(
        generated_events[0].origin.physical_context(),
        generated_events[1].origin.physical_context()
    );
    assert!(
        events
            .iter()
            .any(|e| e.event == SeenEvent::Text("tail".into()))
    );
}

#[test]
fn dtd_events_share_the_parser_event_ceiling() {
    for (ceiling, callbacks, offset) in [(0, 0, 0), (1, 1, 11)] {
        let limits = Limits {
            max_events: ceiling,
            ..Limits::default()
        };
        let mut session = new_session(limits);
        let mut seen = 0;
        let error = session
            .advance(F3.as_bytes(), &mut |_, _, _| {
                seen += 1;
                Ok(())
            })
            .unwrap_err();
        assert_eq!(seen, callbacks);
        assert_eq!(
            error,
            SessionError::Xml(Error::new(ErrorKind::LimitExceeded(Limit::Events), offset))
        );
    }

    let exact = Limits {
        max_events: 8,
        ..Limits::default()
    };
    assert_eq!(drive_parts(F2.as_bytes(), &[], exact).unwrap().0.len(), 8);

    let short = Limits {
        max_events: 7,
        ..Limits::default()
    };
    let mut session = new_session(short);
    let mut seen = Vec::new();
    let error = session
        .advance(F2.as_bytes(), &mut |event, origin, _| {
            seen.push(snapshot(event, origin));
            Ok(())
        })
        .unwrap_err();
    assert_eq!(seen.len(), 7);
    assert!(
        matches!(
            error,
            SessionError::Content(ContentError {
                kind: crate::dtd_controller::content::ContentErrorKind::Parser(
                    ErrorKind::LimitExceeded(Limit::Events)
                ),
                physical_offset: 38,
                ..
            })
        ),
        "{error:?}"
    );

    let two = Limits {
        max_events: 2,
        ..Limits::default()
    };
    let mut session = new_session(two);
    let mut count = 0;
    let error = session
        .advance(F3.as_bytes(), &mut |_, _, _| {
            count += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(count, 2, "no-subset Begin and End both count");
    assert!(
        matches!(
            error,
            SessionError::Content(ContentError {
                kind: crate::dtd_controller::content::ContentErrorKind::Tag(
                    crate::dtd_tag::DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(
                        Limit::Events
                    ))
                ),
                physical_offset: 12,
                ..
            })
        ),
        "{error:?}"
    );
}

#[test]
fn callback_error_and_explicit_abort_keep_distinct_offsets_and_are_sticky() {
    let mut session = new_session(Limits::default());
    assert_eq!(session.event_origin(), None);
    let requested = Error::new(ErrorKind::InvalidState, 777);
    let error = session
        .advance(F2.as_bytes(), &mut |event, _, _| match event {
            LocatedEvent::Dtd(DtdEvent::Begin(_)) => Err(requested),
            _ => Ok(()),
        })
        .unwrap_err();
    assert_eq!(error, SessionError::Xml(requested));
    assert_eq!(session.event_origin().unwrap().input_range(), Some(0..13));
    assert_eq!(session.advance(b"", &mut |_, _, _| Ok(())), Err(error));
    assert_eq!(session.finish(&mut |_, _, _| Ok(())), Err(error));

    let mut session = new_session(Limits::default());
    let requested = Error::new(ErrorKind::Syntax, 888);
    let mut body_callbacks = 0;
    let error = session
        .advance(F3.as_bytes(), &mut |event, _, _| match event {
            LocatedEvent::Dtd(DtdEvent::End) => Err(requested),
            LocatedEvent::Xml(_) => {
                body_callbacks += 1;
                Ok(())
            }
            LocatedEvent::Dtd(_) => Ok(()),
        })
        .unwrap_err();
    assert_eq!(error, SessionError::Xml(requested));
    assert_eq!(body_callbacks, 0);
    assert_eq!(session.event_origin().unwrap().input_range(), Some(11..12));
    assert_eq!(session.advance(b"<r/>", &mut |_, _, _| Ok(())), Err(error));

    for (kind, offset) in [
        (ErrorKind::LimitExceeded(Limit::OwnedBytes), 34),
        (ErrorKind::Aborted, 37),
    ] {
        let mut session = new_session(Limits::default());
        let mut callbacks = 0;
        let error = session
            .advance(F2.as_bytes(), &mut |event, origin, _| {
                callbacks += 1;
                if matches!(event, LocatedEvent::Xml(Event::Text(ref value)) if value == "x") {
                    return Err(Error::new(kind, offset));
                }
                assert!(origin.physical_context().end <= 42);
                Ok(())
            })
            .unwrap_err();
        assert_eq!(error.physical_offset(), offset);
        assert_eq!(
            session.event_origin().unwrap().replacement_range(),
            Some(0..1)
        );
        assert_eq!(callbacks, 6);
        assert_eq!(session.advance(b"", &mut |_, _, _| Ok(())), Err(error));
    }
}

#[test]
fn callback_budget_is_the_same_across_preamble_dtd_and_generated_content() {
    let input = "<?p x?><!DOCTYPE r [<!ENTITY e 'x'>]><r>&e;</r>";
    let mut session = new_session(Limits::default());
    let mut after_preamble = None;
    let mut at_generated = None;
    let consumed = session
        .advance(input.as_bytes(), &mut |event, _, budget| {
            match event {
                LocatedEvent::Xml(Event::ProcessingInstruction { .. }) => {
                    budget.charge_owned(1_000_000, 0)?;
                    after_preamble = Some(budget.remaining_owned());
                }
                LocatedEvent::Xml(Event::Text(ref value)) if value == "x" => {
                    at_generated = Some(budget.remaining_owned());
                }
                _ => {}
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(consumed, input.len());
    session.finish(&mut |_, _, _| Ok(())).unwrap();
    assert!(at_generated.unwrap() <= after_preamble.unwrap());
    assert!(at_generated.unwrap() < Limits::default().max_owned_bytes - 999_999);

    let mut session = new_session(Limits::default());
    let error = session
        .advance(F2.as_bytes(), &mut |event, _, budget| {
            if matches!(event, LocatedEvent::Dtd(DtdEvent::End)) {
                budget.charge_owned(budget.remaining_owned() + 1, 913)?;
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(
        error,
        SessionError::Xml(Error::new(ErrorKind::LimitExceeded(Limit::OwnedBytes), 913))
    );
}

#[test]
fn callback_panic_poisoning_is_terminal() {
    let mut session = new_session(Limits::default());
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _ = session.advance(F2.as_bytes(), &mut |event, _, _| {
            if matches!(event, LocatedEvent::Dtd(DtdEvent::Item { .. })) {
                panic!("intentional callback fault");
            }
            Ok(())
        });
    }));
    assert!(panic.is_err());
    let error = session.advance(b"", &mut |_, _, _| Ok(())).unwrap_err();
    assert!(matches!(error, SessionError::Poisoned { .. }));
    assert_eq!(
        session.advance(b"suffix", &mut |_, _, _| Ok(())),
        Err(error)
    );
    assert_eq!(session.finish(&mut |_, _, _| Ok(())), Err(error));
}

#[test]
fn finish_success_incomplete_dtd_and_post_finish_feed_are_sticky() {
    let (events, mut session) = drive_parts(F0.as_bytes(), &[], Limits::default()).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        session.finish(&mut |_, _, _| panic!("no repeated callbacks")),
        Ok(())
    );
    let error = session.advance(b"", &mut |_, _, _| Ok(())).unwrap_err();
    assert_eq!(
        error,
        SessionError::Xml(Error::new(ErrorKind::InvalidState, F0.len()))
    );
    assert_eq!(session.finish(&mut |_, _, _| Ok(())), Err(error));

    for input in [
        "<!DOCTYPE r [<!ENTITY e 'x'>",
        "<!DOCTYPE r [<!ENTITY e 'x'>]><r>",
    ] {
        let mut session = new_session(Limits::default());
        let consumed = session
            .advance(input.as_bytes(), &mut |_, _, _| Ok(()))
            .unwrap();
        assert_eq!(consumed, input.len());
        let error = session.finish(&mut |_, _, _| Ok(())).unwrap_err();
        assert_eq!(error.physical_offset(), input.len());
        assert_eq!(session.finish(&mut |_, _, _| Ok(())), Err(error));
    }
}

#[test]
fn utf16_dtd_has_the_same_events_with_physical_transport_ranges() {
    let (expected, _) = drive_parts(F3.as_bytes(), &[], Limits::default()).unwrap();
    for little in [true, false] {
        let bytes = utf16(F3, little);
        let (actual, session) = drive_parts(&bytes, &[1, 19, 25], Limits::default()).unwrap();
        assert_eq!(semantic(&actual), semantic(&expected));
        assert_eq!(
            actual
                .iter()
                .map(|e| e.origin.input_range())
                .collect::<Vec<_>>(),
            [Some(2..24), Some(24..26), Some(26..34)]
        );
        assert_eq!(session.position(), bytes.len());
    }
}

#[test]
fn no_subset_end_is_distinct_and_precedes_body() {
    let (events, _) = drive_parts(F3.as_bytes(), &[], Limits::default()).unwrap();
    assert_eq!(
        events.iter().map(|e| &e.event).collect::<Vec<_>>(),
        [
            &SeenEvent::DtdBegin {
                root: "r".into(),
                subset: false
            },
            &SeenEvent::DtdEnd,
            &SeenEvent::Empty("r".into()),
        ]
    );
    assert_eq!(
        events
            .iter()
            .map(|e| e.origin.input_range())
            .collect::<Vec<_>>(),
        [Some(0..11), Some(11..12), Some(12..16)]
    );
}
