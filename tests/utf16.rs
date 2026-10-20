use std::io::{self, Read};
use std::ops::{ControlFlow, Range};
use std::panic::{AssertUnwindSafe, catch_unwind};

use yaxml::{
    DecodeLimits, Document, Error, ErrorKind, Event, Limit, Limits, OwnedDocument, OwnedNodeKind,
    PushParser, ReadError, Reader, WriteError, Writer,
};

#[derive(Clone, Copy, Debug)]
enum Order {
    Little,
    Big,
}

const ORDERS: [Order; 2] = [Order::Little, Order::Big];

fn encode(input: &str, order: Order, bom: bool) -> Vec<u8> {
    let units = bom
        .then_some(0xfeff)
        .into_iter()
        .chain(input.encode_utf16());
    units
        .flat_map(|unit| match order {
            Order::Little => unit.to_le_bytes(),
            Order::Big => unit.to_be_bytes(),
        })
        .collect()
}

fn raw_offset(prefix: &str) -> usize {
    2 + 2 * prefix.encode_utf16().count()
}

fn span(input: &str, token: &str) -> Range<usize> {
    let start = input.find(token).unwrap();
    raw_offset(&input[..start])..raw_offset(&input[..start + token.len()])
}

#[derive(Debug, PartialEq, Eq)]
enum Value {
    Declaration(String, Option<String>, Option<bool>),
    Doctype(String),
    Start(
        String,
        Option<String>,
        Vec<(String, Option<String>, String)>,
    ),
    Empty(
        String,
        Option<String>,
        Vec<(String, Option<String>, String)>,
    ),
    End(String, Option<String>),
    Text(String),
    Cdata(String),
    Comment(String),
    Pi(String, String),
    EntityStart(String),
    EntityEnd,
}

fn record(event: &Event<'_>) -> Value {
    let empty = matches!(event, Event::Empty(_));
    let name = |name: &yaxml::Name<'_>| {
        let (prefix, local) = name
            .qualified()
            .split_once(':')
            .map_or((None, name.qualified()), |(p, l)| (Some(p), l));
        assert_eq!(name.prefix(), prefix);
        assert_eq!(name.local(), local);
        (
            name.qualified().to_owned(),
            name.namespace().map(str::to_owned),
        )
    };
    match event {
        Event::Doctype(doctype) => Value::Doctype(doctype.name.to_owned()),
        Event::Declaration(declaration) => Value::Declaration(
            declaration.version.to_owned(),
            declaration.encoding.map(str::to_owned),
            declaration.standalone,
        ),
        Event::Start(element) | Event::Empty(element) => {
            let (qualified, namespace) = name(&element.name);
            let attributes = element
                .attributes
                .iter()
                .map(|attribute| {
                    let (qualified, namespace) = name(&attribute.name);
                    (qualified, namespace, attribute.value.to_string())
                })
                .collect();
            if empty {
                Value::Empty(qualified, namespace, attributes)
            } else {
                Value::Start(qualified, namespace, attributes)
            }
        }
        Event::End(end) => {
            let (qualified, namespace) = name(end);
            Value::End(qualified, namespace)
        }
        Event::Text(text) => Value::Text(text.to_string()),
        Event::Cdata(text) => Value::Cdata(text.to_string()),
        Event::Comment(text) => Value::Comment(text.to_string()),
        Event::ProcessingInstruction { target, data } => {
            Value::Pi(target.to_string(), data.to_string())
        }
        Event::EntityStart(name) => Value::EntityStart(name.to_string()),
        Event::EntityEnd => Value::EntityEnd,
    }
}

fn stream(input: &[u8], split: usize) -> Result<Vec<(Value, Range<usize>)>, Error> {
    let mut parser = PushParser::new();
    let mut events = Vec::new();
    let mut emit = |event: &Event<'_>, origin: yaxml::SourceOrigin| {
        let source = origin.input_range().expect("direct input event");
        events.push((record(event), source));
        ControlFlow::Continue(())
    };
    let result = (|| {
        parser.feed(&input[..split], &mut emit)?;
        parser.feed(&[], &mut emit)?;
        parser.feed(&input[split..], &mut emit)?;
        parser.finish(&mut emit)
    })();
    if let Err(error) = result {
        assert_eq!(
            parser
                .feed(&[], |_, _| panic!("callback after failure"))
                .unwrap_err(),
            error
        );
        assert_eq!(
            parser
                .finish(|_, _| panic!("callback after failure"))
                .unwrap_err(),
            error
        );
        return Err(error);
    }
    assert_eq!(parser.position(), input.len());
    parser
        .finish(|_, _| panic!("event after successful EOF"))
        .unwrap();
    Ok(events)
}

#[test]
fn both_orders_every_split_preserve_logical_values_names_and_raw_spans() {
    const INPUT: &str = "<?xml version='1.2' encoding='uTf-16' standalone='yes'?><!--前--><é:r xmlns:é='urn:ü' a='A\tB\r\nC&#9;&#13;' é:中='🙂'>é🙂\r\n&amp;&#13;<![CDATA[中\r\n🙂]]><?空 \tdata\r?><c/></é:r>";
    let expected = vec![
        (
            Value::Declaration("1.2".into(), Some("uTf-16".into()), Some(true)),
            span(
                INPUT,
                "<?xml version='1.2' encoding='uTf-16' standalone='yes'?>",
            ),
        ),
        (Value::Comment("前".into()), span(INPUT, "<!--前-->")),
        (
            Value::Start(
                "é:r".into(),
                Some("urn:ü".into()),
                vec![
                    (
                        "xmlns:é".into(),
                        Some("http://www.w3.org/2000/xmlns/".into()),
                        "urn:ü".into(),
                    ),
                    ("a".into(), None, "A B C\t\r".into()),
                    ("é:中".into(), Some("urn:ü".into()), "🙂".into()),
                ],
            ),
            span(
                INPUT,
                "<é:r xmlns:é='urn:ü' a='A\tB\r\nC&#9;&#13;' é:中='🙂'>",
            ),
        ),
        (
            Value::Text("é🙂\n&\r".into()),
            span(INPUT, "é🙂\r\n&amp;&#13;"),
        ),
        (
            Value::Cdata("中\n🙂".into()),
            span(INPUT, "<![CDATA[中\r\n🙂]]>"),
        ),
        (
            Value::Pi("空".into(), "data\n".into()),
            span(INPUT, "<?空 \tdata\r?>"),
        ),
        (Value::Empty("c".into(), None, vec![]), span(INPUT, "<c/>")),
        (
            Value::End("é:r".into(), Some("urn:ü".into())),
            span(INPUT, "</é:r>"),
        ),
    ];
    for order in ORDERS {
        let mut bytes = encode(INPUT, order, true);
        for split in 0..=bytes.len() {
            assert_eq!(
                stream(&bytes, split).unwrap(),
                expected,
                "{order:?}, split {split}"
            );
        }
        for chunk_size in [1, 2, 3, 7, 31] {
            let mut parser = PushParser::new();
            let mut events = Vec::new();
            for chunk in bytes.chunks(chunk_size) {
                parser
                    .feed(chunk, |event, source| {
                        events.push((
                            record(event),
                            source.input_range().expect("direct input event"),
                        ));
                        ControlFlow::Continue(())
                    })
                    .unwrap();
            }
            parser
                .finish(|event, source| {
                    events.push((
                        record(event),
                        source.input_range().expect("direct input event"),
                    ));
                    ControlFlow::Continue(())
                })
                .unwrap();
            assert_eq!(events, expected, "{order:?}, chunk size {chunk_size}");
        }
        let owned = OwnedDocument::parse_bytes(&bytes).unwrap();
        bytes.fill(0);
        drop(bytes);
        let owned = std::thread::spawn(move || owned).join().unwrap();
        assert_eq!(owned.declaration().unwrap().encoding, Some("uTf-16"));
        let root = owned.root_element();
        assert_eq!(root.name().unwrap().qualified(), "é:r");
        assert_eq!(root.name().unwrap().namespace(), Some("urn:ü"));
        assert_eq!(
            root.range(),
            Some(span(INPUT, &INPUT[INPUT.find("<é:r").unwrap()..]))
        );
        let children: Vec<_> = root.children().collect();
        assert_eq!(children.len(), 4);
        for (child, (expected, range)) in children.iter().zip(&expected[3..7]) {
            assert_eq!(child.range(), Some(range.clone()));
            assert_eq!(child.parent().unwrap().id(), root.id());
            match expected {
                Value::Text(text) | Value::Cdata(text) => {
                    assert_eq!(child.text(), Some(text.as_str()))
                }
                Value::Pi(target, data) => {
                    assert_eq!(child.processing_instruction_target(), Some(target.as_str()));
                    assert_eq!(child.text(), Some(data.as_str()));
                }
                Value::Empty(name, ..) => assert_eq!(child.name().unwrap().qualified(), name),
                _ => unreachable!(),
            }
        }
        assert!(matches!(children[1].kind(), OwnedNodeKind::Cdata));
        assert_eq!(
            owned.root().range(),
            Some(0..encode(INPUT, order, true).len())
        );
        let attributes: Vec<_> = root.attributes().collect();
        assert_eq!(attributes[1].value, "A B C\t\r");
        assert_eq!(attributes[2].name.local(), "中");
        assert_eq!(attributes[2].value, "🙂");
    }
}

#[test]
fn encoding_labels_and_declaration_grammar_are_independent_checks() {
    for order in ORDERS {
        let matching = match order {
            Order::Little => "UTF-16LE",
            Order::Big => "UTF-16BE",
        };
        let opposite = match order {
            Order::Little => "UTF-16BE",
            Order::Big => "UTF-16LE",
        };
        for (bom, label, expected) in [
            (true, None, None),
            (true, Some("uTf-16"), None),
            (true, Some(matching), None),
            (false, Some(matching), None),
            (true, Some(opposite), None),
            (false, Some(opposite), None),
            (false, Some("UTF-16"), None),
            (true, Some("UTF-8"), None),
            (true, Some("UTF16"), None),
            (true, Some("UCS-2"), None),
            (true, Some("ISO-8859-1"), None),
            (true, Some("16UTF"), Some(ErrorKind::InvalidDeclaration)),
            (true, Some("UTF 16"), Some(ErrorKind::InvalidDeclaration)),
        ] {
            let input = label.map_or_else(
                || "<r/>".into(),
                |label| format!("<?xml version='1.0' encoding='{label}'?><r/>"),
            );
            let bytes = encode(&input, order, bom);
            for split in 0..=bytes.len() {
                let result = stream(&bytes, split);
                match expected {
                    Some(kind) => assert_eq!(
                        result.unwrap_err().kind,
                        kind,
                        "{order:?} {bom} {label:?} split {split}"
                    ),
                    None => {
                        result.unwrap();
                        assert_eq!(
                            OwnedDocument::parse_bytes(&bytes)
                                .unwrap()
                                .declaration()
                                .and_then(|d| d.encoding),
                            // A label outside the allowed set records the
                            // detected encoding (libxml2 warns and keeps it).
                            label.map(|label| {
                                if ["UTF-16", "UTF16", matching]
                                    .iter()
                                    .any(|a| a.eq_ignore_ascii_case(label))
                                {
                                    label
                                } else {
                                    matching
                                }
                            })
                        );
                    }
                }
            }
            if let Some(kind) = expected {
                assert_eq!(OwnedDocument::parse_bytes(&bytes).unwrap_err().kind, kind);
                let mut parser = PushParser::new();
                let error = parser
                    .feed(&bytes, |_, _| {
                        panic!("event before invalid declaration was rejected")
                    })
                    .unwrap_err();
                assert_eq!(error.kind, kind);
                assert_eq!(
                    parser
                        .finish(|_, _| panic!("callback after declaration failure"))
                        .unwrap_err(),
                    error
                );
            }
        }
        let declaration_only = encode("<?xml version='1.0'?><r/>", order, true);
        assert!(stream(&declaration_only, 1).is_ok());
        for input in ["<?xml version='1.0'?><r/>", "<?pi?><r/>"] {
            let bytes = encode(input, order, false);
            assert!(
                stream(&bytes, 1).is_err(),
                "BOM-less UTF-16 needs matching endian declaration"
            );
            assert!(OwnedDocument::parse_bytes(&bytes).is_err());
        }
    }
}

#[test]
fn utf32_signatures_repeated_boms_and_unpaired_units_are_rejected() {
    for bytes in [
        vec![0, 0, 0xfe, 0xff],
        vec![0xff, 0xfe, 0, 0],
        vec![0, 0, 0xff, 0xfe],
        vec![0xfe, 0xff, 0, 0],
        vec![0, 0, 0, 0x3c],
        vec![0x3c, 0, 0, 0],
        vec![0, 0, 0x3c, 0],
        vec![0, 0x3c, 0, 0],
    ] {
        for split in 0..=bytes.len() {
            assert_eq!(
                stream(&bytes, split).unwrap_err().kind,
                ErrorKind::UnsupportedEncoding,
                "{bytes:?}"
            );
        }
        assert_eq!(
            OwnedDocument::parse_bytes(&bytes).unwrap_err().kind,
            ErrorKind::UnsupportedEncoding
        );
    }
    for order in ORDERS {
        for (input, kind, offset) in [
            ("", ErrorKind::MissingRoot, 2),
            ("<r>", ErrorKind::UnexpectedEof, 8),
            ("<r>\0</r>", ErrorKind::InvalidCharacter, 8),
        ] {
            let bytes = encode(input, order, true);
            for split in 0..=bytes.len() {
                let error = stream(&bytes, split).unwrap_err();
                assert_eq!((error.kind, error.offset), (kind, offset));
            }
        }
        let bytes = encode("\u{feff}<r/>", order, true);
        for split in 0..=bytes.len() {
            let error = stream(&bytes, split).unwrap_err();
            assert_eq!((error.kind, error.offset), (ErrorKind::TextOutsideRoot, 2));
        }
        let inside = encode("<r>\u{feff}</r>", order, true);
        assert_eq!(
            OwnedDocument::parse_bytes(&inside)
                .unwrap()
                .root_element()
                .first_child()
                .unwrap()
                .text(),
            Some("\u{feff}")
        );
        for units in [
            vec![0xdc00_u16],
            vec![0xd800],
            vec![0xd800, 0x61],
            vec![0xd800, 0xd800],
        ] {
            let mut bytes = encode("<r>", order, true);
            for unit in units {
                bytes.extend(match order {
                    Order::Little => unit.to_le_bytes(),
                    Order::Big => unit.to_be_bytes(),
                });
            }
            for split in 0..=bytes.len() {
                let error = stream(&bytes, split).unwrap_err();
                assert_eq!(
                    (error.kind, error.offset),
                    (ErrorKind::InvalidUtf16, 8),
                    "{order:?} split {split}"
                );
            }
            let error = OwnedDocument::parse_bytes(&bytes).unwrap_err();
            assert_eq!((error.kind, error.offset), (ErrorKind::InvalidUtf16, 8));
        }
        let mut odd = encode("<r/>", order, true);
        odd.push(0);
        for split in 0..=odd.len() {
            let error = stream(&odd, split).unwrap_err();
            assert_eq!((error.kind, error.offset), (ErrorKind::InvalidUtf16, 10));
        }
    }
}

#[test]
fn errors_after_non_ascii_use_original_unnormalized_byte_coordinates() {
    for (input, kind, prefix) in [
        ("<r>é🙂&bad;</r>", ErrorKind::UnknownEntity, "<r>é🙂"),
        ("<r>é🙂&#0;</r>", ErrorKind::InvalidReference, "<r>é🙂"),
        (
            "<r>é🙂\r\n&#xZZ;</r>",
            ErrorKind::InvalidReference,
            "<r>é🙂\r\n",
        ),
        ("<r é🙂\u{f0000}='v'/>", ErrorKind::InvalidName, "<r é🙂"),
        ("<r>é🙂</x>", ErrorKind::MismatchedEndTag, "<r>é🙂"),
        ("<r>é🙂\u{1}</r>", ErrorKind::InvalidCharacter, "<r>é🙂"),
    ] {
        for order in ORDERS {
            let bytes = encode(input, order, true);
            let expected = Error {
                kind,
                offset: raw_offset(prefix),
            };
            for split in 0..=bytes.len() {
                assert_eq!(
                    stream(&bytes, split).unwrap_err(),
                    expected,
                    "{input:?} {order:?} split {split}"
                );
            }
            assert_eq!(
                OwnedDocument::parse_bytes(&bytes).unwrap_err(),
                expected,
                "{input:?}"
            );
        }
    }
}

fn limited_stream(input: &[u8], limits: Limits, decoded: DecodeLimits) -> Result<(), Error> {
    let mut parser = PushParser::with_decode_limits(limits, decoded);
    for byte in input {
        parser.feed(std::slice::from_ref(byte), |_, _| ControlFlow::Continue(()))?;
    }
    parser.finish(|_, _| ControlFlow::Continue(()))
}

#[test]
fn raw_and_decoded_byte_ceilings_are_distinct_and_inclusive() {
    for order in ORDERS {
        for (input, limit, boundary, offset) in [
            ("<r/>", Limit::InputBytes, 10, 0),
            ("<r/>", Limit::TokenBytes, 8, 2),
            ("<r/>", Limit::DecodedBytes, 4, 0),
            ("<r/>", Limit::DecodedTokenBytes, 4, 2),
            ("<r>中中中</r>", Limit::TokenBytes, 8, 14),
            ("<r>abcd</r>", Limit::TokenBytes, 8, 8),
            ("<r>中中中</r>", Limit::DecodedTokenBytes, 9, 8),
            ("<r>🙂</r>", Limit::DecodedBytes, 11, 0),
        ] {
            let bytes = encode(input, order, true);
            for ceiling in [boundary - 1, boundary, boundary + 1] {
                let mut limits = Limits::default();
                let mut decoded = DecodeLimits::default();
                match limit {
                    Limit::InputBytes => limits.max_input_bytes = ceiling,
                    Limit::TokenBytes => limits.max_token_bytes = ceiling,
                    Limit::DecodedBytes => decoded.max_decoded_bytes = ceiling,
                    Limit::DecodedTokenBytes => decoded.max_decoded_token_bytes = ceiling,
                    _ => unreachable!(),
                }
                let streamed = limited_stream(&bytes, limits, decoded);
                let owned = OwnedDocument::with_decode_limits(&bytes, limits, decoded);
                if ceiling < boundary {
                    let expected = Error {
                        kind: ErrorKind::LimitExceeded(limit),
                        offset,
                    };
                    assert_eq!(
                        streamed.unwrap_err(),
                        expected,
                        "{input:?} {limit:?}={ceiling}"
                    );
                    assert_eq!(owned.unwrap_err(), expected);
                } else {
                    streamed.unwrap();
                    owned.unwrap();
                }
            }
        }
    }
    // The UTF-8 owned fast path must still apply explicitly requested decode
    // limits, and neither transport BOM spends the decoded-content allowance.
    for bytes in [b"<r/>".as_slice(), "\u{feff}<r/>".as_bytes()] {
        for limit in [Limit::DecodedBytes, Limit::DecodedTokenBytes] {
            for ceiling in [3, 4] {
                let mut decoded = DecodeLimits::default();
                match limit {
                    Limit::DecodedBytes => decoded.max_decoded_bytes = ceiling,
                    Limit::DecodedTokenBytes => decoded.max_decoded_token_bytes = ceiling,
                    _ => unreachable!(),
                }
                let streamed = limited_stream(bytes, Limits::default(), decoded);
                let owned = OwnedDocument::with_decode_limits(bytes, Limits::default(), decoded);
                if ceiling == 3 {
                    assert_eq!(streamed.unwrap_err().kind, ErrorKind::LimitExceeded(limit));
                    assert_eq!(owned.unwrap_err().kind, ErrorKind::LimitExceeded(limit));
                } else {
                    streamed.unwrap();
                    owned.unwrap();
                }
            }
        }
    }
}

#[test]
fn raw_progress_includes_incomplete_units_surrogates_and_text_lookahead() {
    for order in ORDERS {
        let bytes = encode("<r>🙂<c/></r>", order, true);
        let mut parser = PushParser::new();
        for end in 1..=13 {
            parser
                .feed(&bytes[end - 1..end], |_, _| ControlFlow::Continue(()))
                .unwrap();
            assert_eq!(parser.position(), end, "{order:?} {end}");
        }
        // The '<' code unit (bytes 12..14) terminates the text event.
        let mut delivered = Vec::new();
        parser
            .feed(&bytes[13..14], |event, origin| {
                let range = origin.input_range().expect("direct input event");
                delivered.push((record(event), range));
                ControlFlow::Continue(())
            })
            .unwrap();
        assert_eq!(parser.position(), 14);
        assert_eq!(delivered, [(Value::Text("🙂".into()), 8..12)]);
        assert_eq!(parser.event_range().expect("direct input event"), 8..12);
        parser
            .feed(&bytes[14..], |_, _| ControlFlow::Continue(()))
            .unwrap();
        parser.finish(|_, _| ControlFlow::Continue(())).unwrap();
        assert_eq!(parser.position(), bytes.len());
    }
}

#[test]
fn owned_pool_normalization_and_streaming_retention_share_one_budget() {
    // A nonempty root retains its one-byte open name in the streaming parser.
    // The pool separately charges its first one-byte and replacement two-byte
    // allocations. Normalization spends the decoded lexical payload length.
    for (input, boundary, text) in [
        ("<r/>", 1, None),
        ("<r>a</r>", 4, Some("a")),
        ("<r>&amp;</r>", 9, Some("&")),
        ("<r>\r</r>", 5, Some("\n")),
        ("<r>\r\n</r>", 6, Some("\n")),
        ("<r a='&#9;'/>", 10, None),
    ] {
        for order in ORDERS {
            let bytes = encode(input, order, true);
            for ceiling in [boundary - 1, boundary, boundary + 1] {
                let limits = Limits::default().max_owned_bytes(ceiling);
                let result =
                    OwnedDocument::with_decode_limits(&bytes, limits, DecodeLimits::default());
                if ceiling < boundary {
                    let error = result.unwrap_err();
                    assert_eq!(
                        error.kind,
                        ErrorKind::LimitExceeded(Limit::OwnedBytes),
                        "{input:?} budget {ceiling}"
                    );
                    assert!(error.offset <= bytes.len());
                } else {
                    let document = result.unwrap();
                    assert_eq!(
                        document
                            .root_element()
                            .first_child()
                            .and_then(|node| node.text()),
                        text
                    );
                }
            }
        }
    }
    // Each consumer fits separately in eight bytes; their combined use does not.
    for order in ORDERS {
        let bytes = encode("<r>&amp;</r>", order, true);
        let limits = Limits::default().max_owned_bytes(8);
        limited_stream(&bytes, limits, DecodeLimits::default()).unwrap();
        assert_eq!(
            OwnedDocument::with_limits(&bytes, limits).unwrap_err().kind,
            ErrorKind::LimitExceeded(Limit::OwnedBytes)
        );
    }
}

#[test]
fn cancellation_panics_and_terminal_calls_preserve_decoding_lifecycle() {
    for order in ORDERS {
        let bytes = encode("<r>🙂<c/></r>", order, true);
        let mut parser = PushParser::new();
        let mut count = 0;
        let error = parser
            .feed(&bytes, |event, origin| {
                let range = origin.input_range().expect("direct input event");
                count += 1;
                assert!(matches!(event, Event::Start(_)));
                assert_eq!(range, 2..8);
                ControlFlow::Break(())
            })
            .unwrap_err();
        assert_eq!(count, 1);
        assert_eq!(
            error,
            Error {
                kind: ErrorKind::Aborted,
                offset: 8
            }
        );
        assert_eq!(parser.position(), 8);
        assert_eq!(
            parser
                .feed(&[], |_, _| panic!("callback after cancellation"))
                .unwrap_err(),
            error
        );
        assert_eq!(
            parser
                .finish(|_, _| panic!("callback after cancellation"))
                .unwrap_err(),
            error
        );

        let mut parser = PushParser::new();
        let error = parser
            .feed(&bytes, |event, origin| {
                let range = origin.input_range().expect("direct input event");
                if matches!(event, Event::Text(_)) {
                    assert_eq!(range, 8..12);
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            })
            .unwrap_err();
        assert_eq!(
            error,
            Error {
                kind: ErrorKind::Aborted,
                offset: 12
            }
        );
        assert_eq!(parser.position(), 14);
        assert_eq!(parser.event_range().expect("direct input event"), 8..12);

        let pending_text = encode("<r>🙂", order, true);
        let mut parser = PushParser::new();
        parser
            .feed(&pending_text, |_, _| ControlFlow::Continue(()))
            .unwrap();
        assert!(
            catch_unwind(AssertUnwindSafe(
                || parser.finish(|_, _| panic!("injected EOF callback panic"))
            ))
            .is_err()
        );
        assert_eq!(
            parser
                .finish(|_, _| panic!("callback after EOF panic"))
                .unwrap_err()
                .kind,
            ErrorKind::InvalidState
        );
        let mut parser = PushParser::new();
        assert!(
            catch_unwind(AssertUnwindSafe(
                || parser.feed(&bytes, |_, _| panic!("injected callback panic"))
            ))
            .is_err()
        );
        let error = parser
            .finish(|_, _| panic!("callback after panic"))
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidState);
        assert_eq!(
            parser
                .feed(&bytes, |_, _| panic!("callback after panic"))
                .unwrap_err(),
            error
        );

        let mut parser = PushParser::new();
        parser
            .feed(&bytes, |_, _| ControlFlow::Continue(()))
            .unwrap();
        parser.finish(|_, _| ControlFlow::Continue(())).unwrap();
        parser.finish(|_, _| panic!("event after EOF")).unwrap();
        assert_eq!(
            parser
                .feed(&[], |_, _| panic!("event after EOF"))
                .unwrap_err(),
            Error {
                kind: ErrorKind::InvalidState,
                offset: bytes.len()
            }
        );
    }
}

#[test]
fn reader_io_handles_one_byte_reads_interruptions_and_partial_encoding_failures() {
    struct Chunks<'a> {
        input: &'a [u8],
        interrupt: bool,
    }
    impl Read for Chunks<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.interrupt = !self.interrupt;
            if self.interrupt {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let count = self.input.len().min(1);
            buffer[..count].copy_from_slice(&self.input[..count]);
            self.input = &self.input[count..];
            Ok(count)
        }
    }
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected UTF-16 read error",
            ))
        }
    }
    for order in ORDERS {
        let bytes = encode("<r>🙂</r>", order, true);
        let mut text = String::new();
        PushParser::new()
            .read_from(
                Chunks {
                    input: &bytes,
                    interrupt: false,
                },
                |event, origin| {
                    let range = origin.input_range().expect("direct input event");
                    if let Event::Text(value) = event {
                        text.push_str(value);
                        assert_eq!(range, 8..12);
                    }
                    ControlFlow::Continue(())
                },
            )
            .unwrap();
        assert_eq!(text, "🙂");
        let mut parser = PushParser::new();
        parser
            .feed(&bytes[..1], |_, _| panic!("event from partial BOM"))
            .unwrap();
        parser
            .read_from(
                Chunks {
                    input: &bytes[1..],
                    interrupt: false,
                },
                |_, _| ControlFlow::Continue(()),
            )
            .unwrap();
        let mut parser = PushParser::new();
        parser
            .feed(&bytes[..1], |_, _| panic!("event from partial BOM"))
            .unwrap();
        let error = parser
            .read_from(Broken, |_, _| panic!("callback after I/O error"))
            .unwrap_err();
        assert!(
            matches!(error, ReadError::Io(ref e) if e.kind() == io::ErrorKind::PermissionDenied && e.to_string() == "injected UTF-16 read error")
        );
        let mut odd = bytes;
        odd.push(0);
        let error = PushParser::new()
            .read_from(
                Chunks {
                    input: &odd,
                    interrupt: false,
                },
                |_, _| ControlFlow::Continue(()),
            )
            .unwrap_err();
        assert!(
            matches!(error, ReadError::Xml(e) if e.kind == ErrorKind::InvalidUtf16 && e.offset == 20)
        );
    }
}

#[test]
fn utf8_surfaces_and_writer_keep_their_existing_encoding_contracts() {
    for order in ORDERS {
        let bytes = encode("<r/>", order, true);
        assert_eq!(
            Reader::new(&bytes).err().unwrap().kind,
            ErrorKind::UnsupportedEncoding
        );
        assert_eq!(
            Document::parse_bytes(&bytes).unwrap_err().kind,
            ErrorKind::UnsupportedEncoding
        );
        let bytes = encode("<?xml version='1.0' encoding='UTF-16'?><r/>", order, true);
        let owned = OwnedDocument::parse_bytes(&bytes).unwrap();
        let mut writer = Writer::new(Vec::new());
        let error = writer
            .write_event(&Event::Declaration(owned.declaration().unwrap()))
            .unwrap_err();
        assert!(matches!(error, WriteError::Xml(e) if e.kind == ErrorKind::UnsupportedEncoding));
        assert!(writer.into_inner().is_empty());
    }
    for input in [
        "<r/>",
        "\u{feff}<r>é中🙂&amp;</r>",
        "<?xml version='1.0' encoding='uTf-8'?><r/>",
    ] {
        let mut reader = Reader::new(input.as_bytes()).unwrap();
        let mut expected = Vec::new();
        while let Some(event) = reader.read_event().unwrap() {
            expected.push((
                record(&event),
                reader.event_range().expect("direct input event"),
            ));
        }
        for split in 0..=input.len() {
            assert_eq!(stream(input.as_bytes(), split).unwrap(), expected);
        }
        assert_eq!(
            OwnedDocument::parse(input)
                .unwrap()
                .root_element()
                .name()
                .unwrap()
                .qualified(),
            "r"
        );
    }
}
