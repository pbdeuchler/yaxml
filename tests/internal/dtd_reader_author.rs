use super::*;
use crate::dtd_controller::ControllerLimits;
use crate::dtd_reference::ReferenceLimits;
use crate::tokenizer::local::LocalLimits;
use crate::tokenizer::text::{TextLimits, TextScratchLimits};

fn configured() -> SessionLimits {
    SessionLimits {
        declarations: ControllerLimits::default(),
        text: TextLimits {
            max_work: 64 * 1024 * 1024,
            reference: ReferenceLimits {
                max_bytes: 1024,
                max_scratch_bytes: 1024,
                max_work: 64 * 1024 * 1024,
            },
            scratch: TextScratchLimits {
                input_token_bytes: Limits::default().max_token_bytes,
                replacement_token_bytes: 4096,
                max_bytes: Limits::default().max_token_bytes + 4096 + 1024,
            },
        },
        local: LocalLimits {
            max_token_bytes: 4096,
            max_scratch_bytes: 4096,
            max_work: 64 * 1024 * 1024,
        },
    }
}
fn label(event: &ReaderEvent<'_>) -> String {
    match event {
        ReaderEvent::Xml(event) => match event {
            Event::Start(e) => format!("+{}", e.name.qualified()),
            Event::Empty(e) => format!("={}", e.name.qualified()),
            Event::End(n) => format!("-{}", n.qualified()),
            Event::Text(t) => format!("T{t}"),
            _ => format!("{event:?}"),
        },
        ReaderEvent::Dtd(TakenDtdEvent::Begin(_)) => "begin".into(),
        ReaderEvent::Dtd(TakenDtdEvent::Item { .. }) => "item".into(),
        ReaderEvent::Dtd(TakenDtdEvent::End) => "end".into(),
    }
}

#[test]
fn stable_values_survive_advance_and_reader_drop_with_literal_high_water_positions() {
    let input = String::from("<!DOCTYPE r [<!ENTITY e '<n/>x'><!ENTITY z ''>]><r>a&e;b&z;c</r>");
    let mut reader = DtdReader::new(input.as_bytes(), configured()).unwrap();
    let mut events = Vec::new();
    while let Some((event, origin)) = reader.read_event().unwrap() {
        assert_eq!(reader.event_origin(), Some(origin));
        assert_eq!(reader.event_range(), origin.input_range());
        events.push((event, origin, reader.position()));
    }
    assert_eq!(reader.position(), input.len());
    assert!(reader.read_event().unwrap().is_none());
    drop(reader);
    assert_eq!(
        events.iter().map(|e| label(&e.0)).collect::<Vec<_>>(),
        [
            "begin", "item", "item", "end", "+r", "Ta", "=n", "Tx", "Tb", "Tc", "-r"
        ]
    );
    assert_eq!(
        events.iter().map(|e| e.2).collect::<Vec<_>>(),
        [13, 32, 46, 48, 51, 55, 55, 55, 59, 61, 64]
    );
    let ReaderEvent::Dtd(TakenDtdEvent::Item { unit, .. }) = &events[1].0 else {
        panic!()
    };
    assert_eq!(unit.raw().as_ptr(), input[13..].as_ptr());
    let ReaderEvent::Xml(Event::Text(Cow::Borrowed(text))) = &events[5].0 else {
        panic!()
    };
    assert_eq!(text.as_ptr(), input[51..].as_ptr());
    assert!(matches!(&events[7].0, ReaderEvent::Xml(Event::Text(Cow::Owned(text))) if text == "x"));
}

#[test]
fn no_subset_end_is_returned_before_body_and_failure_is_sticky() {
    let input = b"<!DOCTYPE r><r/>";
    let mut reader = DtdReader::new(input, configured()).unwrap();
    let first = reader.read_event().unwrap().unwrap();
    assert_eq!(reader.position(), 12);
    assert_eq!(first.1.input_range(), Some(0..11));
    let second = reader.read_event().unwrap().unwrap();
    assert_eq!(reader.position(), 12);
    assert_eq!(second.1.input_range(), Some(11..12));
    let third = reader.read_event().unwrap().unwrap();
    assert_eq!(reader.position(), 16);
    assert_eq!(label(&third.0), "=r");
    assert_eq!(label(&first.0), "begin");
    assert_eq!(label(&second.0), "end");
    assert!(reader.read_event().unwrap().is_none());
    assert_eq!(reader.into_inner(), input);

    let mut reader = DtdReader::new(b"<!DOCTYPE r><r>pre&missing;</r>", configured()).unwrap();
    for expected in ["begin", "end", "+r", "Tpre"] {
        assert_eq!(label(&reader.read_event().unwrap().unwrap().0), expected);
    }
    let Phase::Content(controller) = &reader.session[0] else {
        panic!()
    };
    assert_eq!(controller.frame_counters().reference_attempts, 0);
    let origin = reader.event_origin();
    let error = reader.read_event().unwrap_err();
    assert!(matches!(error, SessionError::Content(_)));
    let Phase::Content(controller) = &reader.session[0] else {
        panic!()
    };
    assert_eq!(controller.frame_counters().reference_attempts, 1);
    let position = reader.position();
    assert_eq!(reader.read_event().unwrap_err(), error);
    assert_eq!(reader.event_origin(), origin);
    assert_eq!(reader.position(), position);
}

#[test]
fn constructors_and_ordinary_error_positions_match_the_existing_reader() {
    for input in [
        b"\xff\xfe<\0".as_slice(),
        b"\0<\0?",
        b"<\0?\0",
        b"<r/>\0",
        b"<r/>\xff",
    ] {
        let expected = Reader::new(input).err().unwrap();
        assert!(
            matches!(DtdReader::new(input, configured()), Err(SessionError::Xml(e)) if e == expected)
        );
    }
    for input in [
        "<r></s>",
        "<r>&bad;</r>",
        "<?xml?><r/>",
        "<r><!--a--b--></r>",
    ] {
        let mut expected = Reader::new(input.as_bytes()).unwrap();
        let mut reader = DtdReader::new(input.as_bytes(), configured()).unwrap();
        loop {
            match expected.read_event() {
                Ok(Some(event)) => {
                    let (ReaderEvent::Xml(actual), origin) = reader.read_event().unwrap().unwrap()
                    else {
                        panic!()
                    };
                    assert_eq!(actual, event);
                    assert_eq!(Some(origin), expected.event_origin());
                    assert_eq!(reader.position(), expected.position());
                }
                Err(error) => {
                    assert_eq!(reader.read_event().unwrap_err(), SessionError::Xml(error));
                    assert_eq!(reader.position(), expected.position());
                    break;
                }
                Ok(None) => panic!("invalid fixture accepted"),
            }
        }
    }
}

#[test]
fn physical_quota_can_stop_within_a_utf8_scalar_without_losing_failure_progress() {
    let mut options = configured();
    options.declarations.max_physical_bytes = 16;
    let mut reader = DtdReader::new("<!DOCTYPE r><r>π</r>".as_bytes(), options).unwrap();
    for expected in ["begin", "end", "+r"] {
        assert_eq!(label(&reader.read_event().unwrap().unwrap().0), expected);
    }
    let last = reader.event_origin();
    let error = reader.read_event().unwrap_err();
    assert!(matches!(
        error,
        SessionError::Content(crate::dtd_controller::content::ContentError {
            kind: crate::dtd_controller::content::ContentErrorKind::PhysicalBytes,
            physical_offset: 16,
            ..
        })
    ));
    assert_eq!(reader.position(), 16);
    assert_eq!(reader.event_origin(), last);
    assert_eq!(reader.read_event().unwrap_err(), error);
}

#[test]
fn shared_event_and_copy_counters_continue_through_taken_dtd_end() {
    let input = b"<!DOCTYPE r [<!ENTITY unused 'v'>]><r/>";
    let mut options = configured();
    options.declarations.max_owned_bytes = 0;
    let mut reader = DtdReader::with_limits(
        input,
        Limits {
            max_owned_bytes: 0,
            ..Limits::default()
        },
        options,
    )
    .unwrap();
    for expected in ["begin", "item", "end"] {
        assert_eq!(label(&reader.read_event().unwrap().unwrap().0), expected);
        let Phase::Declarations { controller, .. } = &reader.session[0] else {
            panic!()
        };
        assert_eq!(controller.counters().owned_bytes, 0);
    }
    assert_eq!(label(&reader.read_event().unwrap().unwrap().0), "=r");
    assert!(reader.read_event().unwrap().is_none());

    let mut reader = DtdReader::with_limits(
        b"<!DOCTYPE r><r/>",
        Limits {
            max_events: 2,
            ..Limits::default()
        },
        configured(),
    )
    .unwrap();
    assert_eq!(label(&reader.read_event().unwrap().unwrap().0), "begin");
    assert_eq!(label(&reader.read_event().unwrap().unwrap().0), "end");
    let origin = reader.event_origin();
    assert!(matches!(
        reader.read_event(),
        Err(SessionError::Content(
            crate::dtd_controller::content::ContentError {
                kind: crate::dtd_controller::content::ContentErrorKind::Tag(
                    crate::dtd_tag::DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(
                        crate::Limit::Events
                    ))
                ),
                physical_offset: 12,
                ..
            }
        ))
    ));
    assert_eq!(reader.position(), 16);
    assert_eq!(reader.event_origin(), origin);
}
