use std::ops::ControlFlow;

use yaxml::{Error, ErrorKind, Limit, Limits, OwnedDocument, PushParser};

fn utf16(text: &str, little_endian: bool) -> Vec<u8> {
    let mut bytes = if little_endian {
        vec![0xff, 0xfe]
    } else {
        vec![0xfe, 0xff]
    };
    for unit in text.encode_utf16() {
        bytes.extend(if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}

#[test]
fn non_ascii_invalid_markup_is_syntax_at_its_original_start() {
    for text in [
        "<!é>",
        "<!🙂>",
        "<![é>",
        "<!-🙂>",
        "<r><!é></r>",
        "<r><!🙂></r>",
    ] {
        for little_endian in [false, true] {
            let input = utf16(text, little_endian);
            let expected = Error {
                kind: ErrorKind::Syntax,
                offset: 2 + text[..text.find("<!").unwrap()].encode_utf16().count() * 2,
            };
            assert_eq!(OwnedDocument::parse_bytes(&input).unwrap_err(), expected);
            for split in 0..=input.len() {
                let mut parser = PushParser::new();
                let mut events = Vec::new();
                let mut emit = |_: &yaxml::Event<'_>, origin: yaxml::SourceOrigin| {
                    events.push(origin.input_range().expect("direct input event"));
                    ControlFlow::Continue(())
                };
                let error = parser
                    .feed(&input[..split], &mut emit)
                    .and_then(|()| parser.feed(&input[split..], &mut emit))
                    .and_then(|()| parser.finish(&mut emit))
                    .unwrap_err();
                assert_eq!(
                    error, expected,
                    "{text:?}, endian={little_endian}, split={split}"
                );
                assert_eq!(
                    parser.event_range(),
                    if text.starts_with("<r>") {
                        Some(2..8)
                    } else {
                        None
                    }
                );
                if text.starts_with("<r>") {
                    assert_eq!(events.len(), 1);
                    assert_eq!(events[0], 2..8);
                } else {
                    assert!(events.is_empty());
                }
                assert_eq!(
                    parser.finish(|_, _| panic!("terminal parser emitted an event")),
                    Err(expected)
                );
            }
        }
        // A complete UTF-8 input with the same malformed construct has the
        // same syntax diagnosis, even when recognition ends on a leading byte.
        for split in 0..=text.len() {
            let mut parser = PushParser::new();
            let expected = Error {
                kind: ErrorKind::Syntax,
                offset: text.find("<!").unwrap(),
            };
            assert_eq!(
                parser
                    .feed(&text.as_bytes()[..split], |_, _| ControlFlow::Continue(()))
                    .and_then(|()| parser
                        .feed(&text.as_bytes()[split..], |_, _| ControlFlow::Continue(())))
                    .and_then(|()| parser.finish(|_, _| ControlFlow::Continue(()))),
                Err(expected)
            );
        }
    }
}

#[test]
fn invalid_prefix_uses_the_shared_event_ceiling_before_syntax() {
    for (text, events, offset) in [("<!é>", 0, 2), ("<r><!🙂></r>", 1, 8)] {
        for little_endian in [false, true] {
            let input = utf16(text, little_endian);
            for split in 0..=input.len() {
                let mut parser = PushParser::with_limits({
                    let mut cfg = Limits::default();
                    cfg.max_events = events;
                    cfg
                });
                let expected = Error {
                    kind: ErrorKind::LimitExceeded(Limit::Events),
                    offset,
                };
                assert_eq!(
                    parser
                        .feed(&input[..split], |_, _| ControlFlow::Continue(()))
                        .and_then(
                            |()| parser.feed(&input[split..], |_, _| ControlFlow::Continue(()))
                        )
                        .and_then(|()| parser.finish(|_, _| ControlFlow::Continue(()))),
                    Err(expected)
                );
            }
        }
    }
}
