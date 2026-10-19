use std::io::{self, BufRead, Read};
use std::ops::Range;

use yaxml::{BufferedReader, DecodeLimits, Error, ErrorKind, Event, Limit, Limits, ReadError};

#[derive(Clone, Copy, Debug)]
enum Encoding {
    Utf8,
    Utf8Bom,
    Little,
    Big,
}

const ENCODINGS: [Encoding; 4] = [
    Encoding::Utf8,
    Encoding::Utf8Bom,
    Encoding::Little,
    Encoding::Big,
];
const CAPACITIES: [usize; 7] = [1, 2, 3, 7, 64, 8192, 65536];

impl Encoding {
    fn encode(self, input: &str) -> Vec<u8> {
        match self {
            Self::Utf8 => input.as_bytes().to_vec(),
            Self::Utf8Bom => [b"\xef\xbb\xbf".as_slice(), input.as_bytes()].concat(),
            Self::Little | Self::Big => std::iter::once(0xfeff)
                .chain(input.encode_utf16())
                .flat_map(|unit| {
                    if matches!(self, Self::Little) {
                        unit.to_le_bytes()
                    } else {
                        unit.to_be_bytes()
                    }
                })
                .collect(),
        }
    }

    fn offset(self, prefix: &str) -> usize {
        match self {
            Self::Utf8 => prefix.len(),
            Self::Utf8Bom => 3 + prefix.len(),
            Self::Little | Self::Big => 2 + 2 * prefix.encode_utf16().count(),
        }
    }

    fn span(self, input: &str, token: &str) -> Range<usize> {
        let start = input.find(token).unwrap();
        self.offset(&input[..start])..self.offset(&input[..start + token.len()])
    }
}

// Changing visibility never reports temporary EOF. Every consume is checked
// against the exact slice most recently exposed by fill_buf.
struct Window<'a> {
    bytes: &'a [u8],
    offset: usize,
    capacity: usize,
    boundary: usize,
    available: usize,
    fills: usize,
}

impl<'a> Window<'a> {
    fn new(bytes: &'a [u8], capacity: usize, boundary: usize) -> Self {
        Self {
            bytes,
            offset: 0,
            capacity,
            boundary,
            available: 0,
            fills: 0,
        }
    }
}

impl Read for Window<'_> {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("BufferedReader must use fill_buf/consume");
    }
}

impl BufRead for Window<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.fills += 1;
        let boundary = if self.offset < self.boundary {
            self.boundary
        } else {
            self.bytes.len()
        };
        let end = self
            .offset
            .saturating_add(self.capacity)
            .min(boundary)
            .min(self.bytes.len());
        self.available = end - self.offset;
        Ok(&self.bytes[self.offset..end])
    }

    fn consume(&mut self, count: usize) {
        assert!(count <= self.available, "consumed beyond exposed buffer");
        self.offset += count;
        self.available -= count;
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Value {
    Declaration(String, Option<String>, Option<bool>),
    Doctype(String, Option<String>),
    Element(
        bool,
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

fn value(event: &Event<'_>) -> Value {
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
        Event::Doctype(d) => {
            Value::Doctype(d.name.to_owned(), d.internal_subset.map(str::to_owned))
        }
        Event::Declaration(d) => Value::Declaration(
            d.version.to_owned(),
            d.encoding.map(str::to_owned),
            d.standalone,
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
            Value::Element(empty, qualified, namespace, attributes)
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

fn xml(error: ReadError) -> Error {
    match error {
        ReadError::Xml(error) => error,
        other => panic!("unexpected I/O failure: {other}"),
    }
}

fn collect(reader: &mut BufferedReader<Window<'_>>) -> Result<Vec<(Value, Range<usize>)>, Error> {
    let mut events = Vec::new();
    let mut previous = reader.position();
    loop {
        match reader.read_event() {
            Ok(Some((event, range))) => events.push((
                value(&event),
                range.input_range().expect("direct input event"),
            )),
            Ok(None) => {
                let calls = reader.get_ref().fills;
                for _ in 0..3 {
                    assert!(reader.read_event().unwrap().is_none());
                }
                assert_eq!(reader.get_ref().fills, calls, "I/O after successful EOF");
                return Ok(events);
            }
            Err(error) => {
                let expected = xml(error);
                let calls = reader.get_ref().fills;
                for _ in 0..3 {
                    assert_eq!(xml(reader.read_event().unwrap_err()), expected);
                }
                assert_eq!(reader.get_ref().fills, calls, "I/O after XML failure");
                return Err(expected);
            }
        }
        assert!((previous..=reader.get_ref().bytes.len()).contains(&reader.position()));
        assert_eq!(reader.position(), reader.get_ref().offset);
        previous = reader.position();
    }
}

#[test]
fn independent_logical_goldens_hold_for_every_split_and_buffer_capacity() {
    const INPUT: &str = " \r\n<!--前--><?pre x\r?><é:r xmlns:é='urn:ü' a='A\tB\r\nC&#9;&#13;' é:中='🙂'>é🙂\r\n&amp;&#13;<![CDATA[中\r\n🙂]]><?空 \tdata\r?><c/></é:r>\r\n";
    for encoding in ENCODINGS {
        let expected = vec![
            (
                Value::Comment("前".into()),
                encoding.span(INPUT, "<!--前-->"),
            ),
            (
                Value::Pi("pre".into(), "x\n".into()),
                encoding.span(INPUT, "<?pre x\r?>"),
            ),
            (
                Value::Element(
                    false,
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
                encoding.span(
                    INPUT,
                    "<é:r xmlns:é='urn:ü' a='A\tB\r\nC&#9;&#13;' é:中='🙂'>",
                ),
            ),
            (
                Value::Text("é🙂\n&\r".into()),
                encoding.span(INPUT, "é🙂\r\n&amp;&#13;"),
            ),
            (
                Value::Cdata("中\n🙂".into()),
                encoding.span(INPUT, "<![CDATA[中\r\n🙂]]>"),
            ),
            (
                Value::Pi("空".into(), "data\n".into()),
                encoding.span(INPUT, "<?空 \tdata\r?>"),
            ),
            (
                Value::Element(true, "c".into(), None, vec![]),
                encoding.span(INPUT, "<c/>"),
            ),
            (
                Value::End("é:r".into(), Some("urn:ü".into())),
                encoding.span(INPUT, "</é:r>"),
            ),
        ];
        let bytes = encoding.encode(INPUT);
        for boundary in 0..=bytes.len() {
            let mut reader = BufferedReader::new(Window::new(&bytes, usize::MAX, boundary));
            assert_eq!(
                collect(&mut reader).unwrap(),
                expected,
                "{encoding:?}, split {boundary}"
            );
            assert_eq!(reader.position(), bytes.len());
        }
        for capacity in CAPACITIES {
            let mut reader = BufferedReader::new(Window::new(&bytes, capacity, bytes.len()));
            assert_eq!(
                collect(&mut reader).unwrap(),
                expected,
                "{encoding:?}, capacity {capacity}"
            );
            assert_eq!(reader.position(), bytes.len());
        }
    }
}

#[test]
fn ordinary_suspension_preserves_namespace_scopes_and_owned_caller_copies() {
    const INPUT: &str = "<r xmlns='urn:d' xmlns:p='urn:a'><p:n a='v'><p:n xmlns:p='urn:b'/></p:n><p:n/><n xmlns=''/></r>";
    for encoding in ENCODINGS {
        let bytes = encoding.encode(INPUT);
        let mut reader = BufferedReader::new(Window::new(&bytes, 1, bytes.len()));
        let (first, first_range) = reader.read_event().unwrap().unwrap();
        let saved = value(&first);
        assert_eq!(
            first_range.input_range().expect("direct input event"),
            encoding.span(INPUT, "<r xmlns='urn:d' xmlns:p='urn:a'>")
        );
        let rest = collect(&mut reader).unwrap();
        let names: Vec<_> = rest
            .iter()
            .filter_map(|(event, _)| match event {
                Value::Element(_, name, namespace, _) | Value::End(name, namespace) => {
                    Some((name.as_str(), namespace.as_deref()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            names,
            [
                ("p:n", Some("urn:a")),
                ("p:n", Some("urn:b")),
                ("p:n", Some("urn:a")),
                ("p:n", Some("urn:a")),
                ("n", None),
                ("r", Some("urn:d"))
            ]
        );
        assert_eq!(
            saved,
            Value::Element(
                false,
                "r".into(),
                Some("urn:d".into()),
                vec![
                    (
                        "xmlns".into(),
                        Some("http://www.w3.org/2000/xmlns/".into()),
                        "urn:d".into()
                    ),
                    (
                        "xmlns:p".into(),
                        Some("http://www.w3.org/2000/xmlns/".into()),
                        "urn:a".into()
                    ),
                ]
            )
        );
    }
}

#[test]
fn event_delivery_consumes_only_its_prefix_and_documented_text_lookahead() {
    for encoding in ENCODINGS {
        let bytes = encoding.encode("<r>text<c/></r>TAIL");
        let mut reader = BufferedReader::new(Window::new(&bytes, 65536, bytes.len()));
        let (event, range) = reader.read_event().unwrap().unwrap();
        assert!(matches!(event, Event::Start(_)));
        assert_eq!(
            range.input_range().expect("direct input event"),
            encoding.offset("")..encoding.offset("<r>")
        );
        assert_eq!(reader.position(), encoding.offset("<r>"));
        assert_eq!(reader.get_ref().offset, reader.position());
        let (event, range) = reader.read_event().unwrap().unwrap();
        assert_eq!(value(&event), Value::Text("text".into()));
        assert_eq!(
            range.input_range().expect("direct input event"),
            encoding.offset("<r>")..encoding.offset("<r>text")
        );
        let position = encoding.offset("<r>text<");
        assert_eq!(reader.position(), position);
        let input = reader.into_inner();
        assert_eq!(input.offset, position);
        assert_eq!(&input.bytes[input.offset..], &bytes[position..]);
    }
}

#[test]
fn declaration_spelling_and_bomless_endian_agreement_survive_suspension() {
    for (encoding, label, bomless) in [
        (Encoding::Utf8, "uTf-8", false),
        (Encoding::Utf8Bom, "UTF-8", false),
        (Encoding::Little, "uTf-16", false),
        (Encoding::Big, "UTF-16BE", false),
        (Encoding::Little, "uTf-16LE", true),
        (Encoding::Big, "UTF-16BE", true),
    ] {
        let input = format!("<?xml version='1.2' encoding='{label}' standalone='no'?><r/>");
        let encoded = encoding.encode(&input);
        let bytes = if bomless { &encoded[2..] } else { &encoded[..] };
        for boundary in 0..=bytes.len() {
            let mut reader = BufferedReader::new(Window::new(bytes, usize::MAX, boundary));
            let events = collect(&mut reader).unwrap();
            assert_eq!(events.len(), 2);
            assert_eq!(
                events[0].0,
                Value::Declaration("1.2".into(), Some(label.into()), Some(false))
            );
            assert_eq!(
                events[0].1.start,
                if bomless { 0 } else { encoding.offset("") }
            );
            assert_eq!(events[1].1.end, bytes.len());
        }
    }
    for (label, expected) in [("16UTF", ErrorKind::InvalidDeclaration)] {
        for encoding in [Encoding::Little, Encoding::Big] {
            let bytes = encoding.encode(&format!("<?xml version='1.0' encoding='{label}'?><r/>"));
            let mut reader = BufferedReader::new(Window::new(&bytes, 1, bytes.len()));
            assert_eq!(xml(reader.read_event().unwrap_err()).kind, expected);
        }
    }
}

#[test]
fn malformed_tokens_report_explicit_original_byte_errors() {
    for (input, kind, prefix) in [
        ("", ErrorKind::MissingRoot, ""),
        ("<r>", ErrorKind::UnexpectedEof, "<r>"),
        ("<r>x<", ErrorKind::UnexpectedEof, "<r>x"),
        ("<r>é🙂&bad;</r>", ErrorKind::UnknownEntity, "<r>é🙂"),
        (
            "<r>é🙂\r\n&#0;</r>",
            ErrorKind::InvalidReference,
            "<r>é🙂\r\n",
        ),
        ("<r é🙂\u{f0000}='v'/>", ErrorKind::InvalidName, "<r é🙂"),
        ("<r>é🙂</x>", ErrorKind::MismatchedEndTag, "<r>é🙂"),
        ("<r>é🙂\u{1}</r>", ErrorKind::InvalidCharacter, "<r>é🙂"),
        ("<r/><s/>", ErrorKind::MultipleRoots, "<r/>"),
        ("<r><!🙂</r>", ErrorKind::Syntax, "<r>"),
        ("<![CDATA[x]]><r/>", ErrorKind::TextOutsideRoot, ""),
        (
            "<!DOCTYPE r [<!BAD>]><r/>",
            ErrorKind::InvalidDtd,
            "<!DOCTYPE r [<!",
        ),
        (
            "<!DOCTYPE r><r>&u;</r>",
            ErrorKind::UnknownEntity,
            "<!DOCTYPE r><r>",
        ),
    ] {
        for encoding in ENCODINGS {
            let bytes = encoding.encode(input);
            for capacity in CAPACITIES {
                let mut reader = BufferedReader::new(Window::new(&bytes, capacity, bytes.len()));
                assert_eq!(
                    collect(&mut reader).unwrap_err(),
                    Error {
                        kind,
                        offset: encoding.offset(prefix)
                    },
                    "{encoding:?} {input:?} capacity {capacity}"
                );
                assert_eq!(reader.position(), reader.get_ref().offset);
            }
        }
    }
}

#[test]
fn malformed_encoding_carry_is_checked_at_true_eof() {
    for encoding in [Encoding::Little, Encoding::Big] {
        for units in [vec![0xdc00_u16], vec![0xd800], vec![0xd800, 0x61]] {
            let mut bytes = encoding.encode("<r>");
            for unit in units {
                bytes.extend(if matches!(encoding, Encoding::Little) {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            for boundary in 0..=bytes.len() {
                let mut reader = BufferedReader::new(Window::new(&bytes, usize::MAX, boundary));
                assert_eq!(
                    collect(&mut reader).unwrap_err(),
                    Error {
                        kind: ErrorKind::InvalidUtf16,
                        offset: 8
                    }
                );
            }
        }
        let mut bytes = encoding.encode("<r>");
        bytes.extend(if matches!(encoding, Encoding::Little) {
            0xd800_u16.to_le_bytes()
        } else {
            0xd800_u16.to_be_bytes()
        });
        bytes.push(0);
        let mut reader = BufferedReader::new(Window::new(&bytes, 1, bytes.len()));
        assert_eq!(
            collect(&mut reader).unwrap_err(),
            Error {
                kind: ErrorKind::InvalidUtf16,
                offset: 8
            }
        );
        let mut bytes = encoding.encode("<r/>");
        bytes.push(0);
        let mut reader = BufferedReader::new(Window::new(&bytes, 1, bytes.len()));
        assert_eq!(
            collect(&mut reader).unwrap_err(),
            Error {
                kind: ErrorKind::InvalidUtf16,
                offset: 10
            }
        );
    }
    let bytes = b"<r>\xff</r>";
    let mut reader = BufferedReader::new(Window::new(bytes, 1, bytes.len()));
    assert_eq!(
        collect(&mut reader).unwrap_err(),
        Error {
            kind: ErrorKind::InvalidUtf8,
            offset: 3
        }
    );
}

#[test]
fn unsupported_signatures_and_repeated_transport_boms_are_not_ignored() {
    for bytes in [
        [0, 0, 0xfe, 0xff],
        [0xff, 0xfe, 0, 0],
        [0, 0, 0xff, 0xfe],
        [0xfe, 0xff, 0, 0],
        [0, 0, 0, b'<'],
        [b'<', 0, 0, 0],
        [0, 0, b'<', 0],
        [0, b'<', 0, 0],
    ] {
        for boundary in 0..=bytes.len() {
            let mut reader = BufferedReader::new(Window::new(&bytes, 1, boundary));
            assert_eq!(
                collect(&mut reader).unwrap_err(),
                Error {
                    kind: ErrorKind::UnsupportedEncoding,
                    offset: 0,
                }
            );
        }
    }
    for encoding in [Encoding::Utf8Bom, Encoding::Little, Encoding::Big] {
        let bytes = encoding.encode("\u{feff}<r/>");
        for capacity in CAPACITIES {
            let mut reader = BufferedReader::new(Window::new(&bytes, capacity, bytes.len()));
            assert_eq!(
                collect(&mut reader).unwrap_err(),
                Error {
                    kind: ErrorKind::TextOutsideRoot,
                    offset: encoding.offset(""),
                }
            );
        }
    }
    for encoding in ENCODINGS {
        let bytes = encoding.encode("<r>\u{feff}</r>");
        let mut reader = BufferedReader::new(Window::new(&bytes, 1, bytes.len()));
        let events = collect(&mut reader).unwrap();
        assert_eq!(
            events[1],
            (
                Value::Text("\u{feff}".into()),
                encoding.span("<r>\u{feff}</r>", "\u{feff}")
            )
        );
    }
}

#[test]
fn every_ceiling_is_inclusive_and_cumulative_across_returned_events() {
    for encoding in ENCODINGS {
        for (input, limit, boundary) in [
            ("<r/>", Limit::InputBytes, encoding.offset("<r/>")),
            (
                "<r>abcd</r>",
                Limit::TokenBytes,
                encoding.offset("abcd") - encoding.offset(""),
            ),
            (
                "     <r/>",
                Limit::TokenBytes,
                encoding.offset("     ") - encoding.offset(""),
            ),
            ("<r/>", Limit::DecodedBytes, 4),
            ("<r>🙂</r>", Limit::DecodedBytes, 11),
            ("<r>中中中</r>", Limit::DecodedTokenBytes, 9),
            ("     <r/>", Limit::DecodedTokenBytes, 5),
            ("<r/>", Limit::Depth, 1),
            ("<r><c/></r>", Limit::Depth, 2),
            ("<r a='1'/>", Limit::Attributes, 1),
            ("<r>x</r>", Limit::Events, 3),
            (" \r<r/>\t\n", Limit::Events, 1),
            ("<r><s></s><s></s></r>", Limit::OwnedBytes, 3),
            ("<r>&amp;</r>", Limit::OwnedBytes, 6),
            ("<r>\r\n</r>", Limit::OwnedBytes, 3),
        ] {
            for maximum in [boundary - 1, boundary, boundary + 1] {
                for capacity in [1, 7, 65536] {
                    let mut limits = Limits::default();
                    let mut decode = DecodeLimits::default();
                    match limit {
                        Limit::InputBytes => limits.max_input_bytes = maximum,
                        Limit::TokenBytes => limits.max_token_bytes = maximum,
                        Limit::DecodedBytes => decode.max_decoded_bytes = maximum,
                        Limit::DecodedTokenBytes => decode.max_decoded_token_bytes = maximum,
                        Limit::Depth => limits.max_depth = maximum,
                        Limit::Attributes => limits.max_attributes = maximum,
                        Limit::Events => limits.max_events = maximum,
                        Limit::OwnedBytes => limits.max_owned_bytes = maximum,
                        _ => unreachable!(),
                    }
                    let bytes = encoding.encode(input);
                    let mut reader = BufferedReader::with_decode_limits(
                        Window::new(&bytes, capacity, bytes.len()),
                        limits,
                        decode,
                    );
                    let result = collect(&mut reader);
                    if maximum < boundary {
                        assert_eq!(
                            result.unwrap_err().kind,
                            ErrorKind::LimitExceeded(limit),
                            "{encoding:?} {input:?} {limit:?}={maximum}, capacity{capacity}"
                        );
                    } else {
                        assert!(
                            result.is_ok(),
                            "{encoding:?} {input:?} {limit:?}={maximum}, capacity{capacity}: {result:?}"
                        );
                        assert_eq!(reader.position(), bytes.len());
                    }
                }
            }
        }
    }
}

#[test]
fn scalar_decoded_total_is_charged_before_malformed_prefix_recognition() {
    for encoding in [Encoding::Little, Encoding::Big] {
        let bytes = encoding.encode("<!é>");
        for (maximum, expected) in [
            (
                3,
                Error {
                    kind: ErrorKind::LimitExceeded(Limit::DecodedBytes),
                    offset: 0,
                },
            ),
            (
                4,
                Error {
                    kind: ErrorKind::Syntax,
                    offset: 2,
                },
            ),
        ] {
            for capacity in CAPACITIES {
                let mut reader = BufferedReader::with_decode_limits(
                    Window::new(&bytes, capacity, bytes.len()),
                    Limits::default(),
                    DecodeLimits {
                        max_decoded_bytes: maximum,
                        ..DecodeLimits::default()
                    },
                );
                assert_eq!(collect(&mut reader).unwrap_err(), expected);
            }
        }
    }
}

#[test]
fn empty_payload_events_are_delivered_once_with_distinct_ranges() {
    const INPUT: &str = "<r><![CDATA[]]><!----><?p?></r>";
    for encoding in ENCODINGS {
        let expected = vec![
            (
                Value::Element(false, "r".into(), None, vec![]),
                encoding.span(INPUT, "<r>"),
            ),
            (
                Value::Cdata(String::new()),
                encoding.span(INPUT, "<![CDATA[]]>"),
            ),
            (
                Value::Comment(String::new()),
                encoding.span(INPUT, "<!---->"),
            ),
            (
                Value::Pi("p".into(), String::new()),
                encoding.span(INPUT, "<?p?>"),
            ),
            (Value::End("r".into(), None), encoding.span(INPUT, "</r>")),
        ];
        let bytes = encoding.encode(INPUT);
        for capacity in CAPACITIES {
            let mut reader = BufferedReader::new(Window::new(&bytes, capacity, bytes.len()));
            assert_eq!(collect(&mut reader).unwrap(), expected);
        }
    }
}

#[test]
fn prepositioned_sources_start_a_new_relative_coordinate_system() {
    for encoding in ENCODINGS {
        let encoded = encoding.encode("<r/>TAIL");
        let bytes = [b"prefix".as_slice(), &encoded].concat();
        let mut input = Window::new(&bytes, 65536, bytes.len());
        input.offset = 6;
        let mut reader = BufferedReader::new(input);
        assert_eq!(reader.position(), 0);
        let (event, range) = reader.read_event().unwrap().unwrap();
        assert!(matches!(event, Event::Empty(_)));
        assert_eq!(
            range.input_range().expect("direct input event"),
            encoding.offset("")..encoding.offset("<r/>")
        );
        assert_eq!(reader.position(), encoding.offset("<r/>"));
        let source = reader.into_inner();
        assert_eq!(source.offset, 6 + encoding.offset("<r/>"));
        assert_eq!(
            &source.bytes[source.offset..],
            &encoded[encoding.offset("<r/>")..]
        );
    }
}

#[test]
fn dtd_text_consumes_its_terminating_reference_and_stops_after_entity_errors() {
    let text = |value: &str| Some(Value::Text(value.into()));
    // (input, [(text event or other, consumed prefix)], error, error prefix, consumed prefix)
    type Case<'a> = (
        &'a str,
        &'a [(Option<Value>, &'a str)],
        ErrorKind,
        &'a str,
        &'a str,
    );
    let cases: [Case<'_>; 5] = [
        (
            "<!DOCTYPE r0S><TSaH>r>&e;",
            &[
                (None, "<!DOCTYPE r0S>"),
                (None, "<!DOCTYPE r0S><TSaH>"),
                (text("r>"), "<!DOCTYPE r0S><TSaH>r>&e;"),
            ],
            ErrorKind::UnknownEntity,
            "<!DOCTYPE r0S><TSaH>r>",
            "<!DOCTYPE r0S><TSaH>r>&e;",
        ),
        (
            "<!DOCTYPE r><r>a&#x41;&lt;&e;</r>TAIL",
            &[
                (None, "<!DOCTYPE r>"),
                (None, "<!DOCTYPE r><r>"),
                (text("aA<"), "<!DOCTYPE r><r>a&#x41;&lt;&e;"),
            ],
            ErrorKind::UnknownEntity,
            "<!DOCTYPE r><r>a&#x41;&lt;",
            "<!DOCTYPE r><r>a&#x41;&lt;&e;",
        ),
        (
            "<!DOCTYPE r [<!ENTITY e 'x'>]><r>a&e;b&u;</r>",
            &[
                (None, "<!DOCTYPE r [<!ENTITY e 'x'>]>"),
                (None, "<!DOCTYPE r [<!ENTITY e 'x'>]><r>"),
                (text("a"), "<!DOCTYPE r [<!ENTITY e 'x'>]><r>a&e;"),
                (
                    Some(Value::EntityStart("e".into())),
                    "<!DOCTYPE r [<!ENTITY e 'x'>]><r>a&e;",
                ),
                (text("x"), "<!DOCTYPE r [<!ENTITY e 'x'>]><r>a&e;"),
                (
                    Some(Value::EntityEnd),
                    "<!DOCTYPE r [<!ENTITY e 'x'>]><r>a&e;",
                ),
                (text("b"), "<!DOCTYPE r [<!ENTITY e 'x'>]><r>a&e;b&u;"),
            ],
            ErrorKind::UnknownEntity,
            "<!DOCTYPE r [<!ENTITY e 'x'>]><r>a&e;b",
            "<!DOCTYPE r [<!ENTITY e 'x'>]><r>a&e;b&u;",
        ),
        (
            "<!DOCTYPE r><r>&u;</r>TAIL",
            &[(None, "<!DOCTYPE r>"), (None, "<!DOCTYPE r><r>")],
            ErrorKind::UnknownEntity,
            "<!DOCTYPE r><r>",
            "<!DOCTYPE r><r>&u;",
        ),
        (
            "<!DOCTYPE r><r>t</x>TAIL",
            &[
                (None, "<!DOCTYPE r>"),
                (None, "<!DOCTYPE r><r>"),
                (text("t"), "<!DOCTYPE r><r>t<"),
            ],
            ErrorKind::MismatchedEndTag,
            "<!DOCTYPE r><r>t",
            "<!DOCTYPE r><r>t</x>",
        ),
    ];
    for (input, events, kind, error, consumed) in cases {
        for encoding in ENCODINGS {
            let bytes = encoding.encode(input);
            for capacity in CAPACITIES {
                let context = format!("{encoding:?} {input:?} capacity {capacity}");
                let mut reader = BufferedReader::new(Window::new(&bytes, capacity, bytes.len()));
                for (expected, prefix) in events {
                    let (event, _) = reader.read_event().unwrap().expect(&context);
                    let event = value(&event);
                    if expected.is_some() {
                        assert_eq!(Some(event), *expected, "{context}");
                    } else {
                        assert!(!matches!(event, Value::Text(_)), "{context}");
                    }
                    assert_eq!(reader.position(), encoding.offset(prefix), "{context}");
                    assert_eq!(reader.get_ref().offset, reader.position(), "{context}");
                }
                let expected = Error {
                    kind,
                    offset: encoding.offset(error),
                };
                assert_eq!(collect(&mut reader), Err(expected), "{context}");
                assert_eq!(reader.position(), encoding.offset(consumed), "{context}");
                let input = reader.into_inner();
                assert_eq!(input.offset, encoding.offset(consumed), "{context}");
            }
        }
    }
}
