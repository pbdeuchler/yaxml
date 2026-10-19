#![forbid(unsafe_code)]

use std::cell::Cell;
use std::io::{self, BufRead, Read};
use std::rc::Rc;

use yaxml::{
    ErrorKind, Limit, Limits, NodeType, OwnedDocument, ParseOptions, ParserProperty, ReadError,
    ReadState, Request, TextReader, Writer,
};

/// `xmllint --stream --debug` lines: depth, type, name, empty, has value, value.
fn line<R: BufRead>(reader: &TextReader<R>) -> String {
    let mut line = format!(
        "{} {} {} {} {}",
        reader.depth(),
        reader.node_type() as i32,
        reader.name().unwrap_or("--"),
        u8::from(reader.is_empty_element()),
        u8::from(reader.has_value())
    );
    if let Some(value) = reader.value() {
        line = format!("{line} {value}");
    }
    line
}

fn lines<R: BufRead>(mut reader: TextReader<R>) -> Result<Vec<String>, ReadError> {
    let mut lines = Vec::new();
    while reader.read()? {
        lines.push(line(&reader));
    }
    Ok(lines)
}

/// Expected sequences are pinned libxml2 2.15.4 `xmllint --stream --debug --noent` output.
const SEQUENCES: &[(&str, &[&str])] = &[
    ("<r/>", &["0 1 r 1 0"]),
    ("<r></r>", &["0 1 r 0 0", "0 15 r 0 0"]),
    (
        "<?xml version='1.0'?><!--a--><?p d?><r>t</r><!--z-->",
        &[
            "0 8 #comment 0 1 a",
            "0 7 p 0 1 d",
            "0 1 r 0 0",
            "1 3 #text 0 1 t",
            "0 15 r 0 0",
            "0 8 #comment 0 1 z",
        ],
    ),
    (
        "<r><a><b/></a>x<![CDATA[y]]></r>",
        &[
            "0 1 r 0 0",
            "1 1 a 0 0",
            "2 1 b 1 0",
            "1 15 a 0 0",
            "1 3 #text 0 1 x",
            "1 4 #cdata-section 0 1 y",
            "0 15 r 0 0",
        ],
    ),
    (
        "<!DOCTYPE r [<!ENTITY e 'x<b/>y'>]><r>a&e;b</r>",
        &[
            "0 10 r 0 0",
            "0 1 r 0 0",
            "1 3 #text 0 1 a",
            "1 5 e 0 0",
            "1 3 #text 0 1 b",
            "0 15 r 0 0",
        ],
    ),
    (
        "<r> <a>&#32;</a>\n</r>",
        &[
            "0 1 r 0 0",
            "1 14 #text 0 1  ",
            "1 1 a 0 0",
            "2 14 #text 0 1  ",
            "1 15 a 0 0",
            "1 14 #text 0 1 \n",
            "0 15 r 0 0",
        ],
    ),
    (
        "<p:r xmlns:p='u' xmlns='d'><c/></p:r>",
        &["0 1 p:r 0 0", "1 1 c 1 0", "0 15 p:r 0 0"],
    ),
];

/// UTF-8 and UTF-16LE/BE with a BOM.
fn encodings(input: &str) -> Vec<Vec<u8>> {
    let utf16 = |little: bool| -> Vec<u8> {
        std::iter::once(0xfeff_u16)
            .chain(input.encode_utf16())
            .flat_map(|unit| {
                if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                }
            })
            .collect()
    };
    vec![input.as_bytes().to_vec(), utf16(true), utf16(false)]
}

#[test]
fn node_sequences_match_libxml2_for_every_source() {
    for (input, expected) in SEQUENCES {
        for bytes in encodings(input) {
            assert_eq!(
                lines(TextReader::new(bytes.as_slice())).unwrap(),
                *expected,
                "{input}"
            );
            let trickle = OneByte {
                input: &bytes,
                consumed: Rc::default(),
            };
            assert_eq!(
                lines(TextReader::new(trickle)).unwrap(),
                *expected,
                "{input}"
            );
        }
    }
}

/// Yields one byte per `fill_buf` and counts consumed bytes.
struct OneByte<'a> {
    input: &'a [u8],
    consumed: Rc<Cell<usize>>,
}

impl Read for OneByte<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = self.fill_buf()?.len().min(out.len());
        out[..n].copy_from_slice(&self.input[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for OneByte<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        Ok(&self.input[..self.input.len().min(1)])
    }

    fn consume(&mut self, amount: usize) {
        self.input = &self.input[amount..];
        self.consumed.set(self.consumed.get() + amount);
    }
}

#[test]
fn streaming_reads_at_most_one_node_ahead() {
    let item = "<i a='1'>text</i>";
    let count = 2000;
    let input = format!("<r>{}</r>", item.repeat(count));
    let consumed = Rc::new(Cell::new(0));
    let source = OneByte {
        input: input.as_bytes(),
        consumed: consumed.clone(),
    };
    let mut reader = TextReader::new(source);
    let mut seen = 0;
    while reader.read().unwrap() {
        if reader.node_type() == NodeType::Text {
            // The text and its end tag's first byte, never the rest of the input.
            let end = 3 + seen * item.len() + "<i a='1'>text".len();
            assert!(
                consumed.get() <= end + "</i>".len(),
                "{} > {end}",
                consumed.get()
            );
            seen += 1;
        }
    }
    assert_eq!((seen, consumed.get()), (count, input.len()));
}

#[test]
fn attribute_navigation_follows_libxml2() {
    let input = "<r xmlns:p='urn:p' a='1' p:b='2' xmlns='urn:d'><c/></r>";
    let mut reader = TextReader::new(input.as_bytes());
    assert!(
        !reader.move_to_first_attribute(),
        "no node before the first read"
    );
    assert!(reader.read().unwrap());
    assert_eq!(
        (reader.attribute_count(), reader.has_attributes()),
        (4, true)
    );
    // Declarations first, in source order, then ordinary attributes.
    let names: Vec<_> = (0..4)
        .map(|index| {
            assert!(reader.move_to_attribute_no(index));
            (
                reader.name().unwrap().to_owned(),
                reader.value().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        names,
        [
            ("xmlns:p", "urn:p"),
            ("xmlns", "urn:d"),
            ("a", "1"),
            ("p:b", "2")
        ]
        .map(|(n, v)| (n.to_owned(), v.to_owned()))
    );
    assert_eq!(
        (
            reader.node_type(),
            reader.depth(),
            reader.prefix(),
            reader.local_name(),
            reader.namespace_uri()
        ),
        (NodeType::Attribute, 1, Some("p"), Some("b"), Some("urn:p"))
    );
    assert_eq!(
        (
            reader.attribute_count(),
            reader.has_attributes(),
            reader.is_empty_element()
        ),
        (0, false, false)
    );
    assert_eq!(
        reader.get_attribute("a"),
        None,
        "libxml2 answers lookups only on the element"
    );
    assert!(!reader.move_to_next_attribute(), "past the last attribute");
    assert_eq!(reader.name(), Some("p:b"), "a failed move keeps the cursor");
    assert!(
        !reader.move_to_attribute_no(4),
        "out of range returns to the element"
    );
    assert_eq!(reader.node_type(), NodeType::Element);
    assert!(!reader.move_to_element(), "already on the element");

    for (name, found, value) in [
        ("a", true, Some("1")),
        ("p:b", true, Some("2")),
        ("xmlns", true, Some("urn:d")),
        ("xmlns:p", true, Some("urn:p")),
        ("b", false, None),
        ("q:b", false, None),
        ("xmlns:q", false, None),
    ] {
        assert_eq!(reader.get_attribute(name), value, "{name}");
        assert_eq!(reader.move_to_attribute(name), found, "{name}");
        if found {
            assert_eq!(reader.value(), value);
            assert!(reader.move_to_element());
        }
    }
    for (local, namespace, value) in [
        ("b", Some("urn:p"), Some("2")),
        ("a", None, Some("1")),
        ("p", Some("http://www.w3.org/2000/xmlns/"), Some("urn:p")),
        (
            "xmlns",
            Some("http://www.w3.org/2000/xmlns/"),
            Some("urn:d"),
        ),
        ("b", None, None),
        ("a", Some("urn:p"), None),
    ] {
        assert_eq!(reader.get_attribute_ns(local, namespace), value, "{local}");
        assert_eq!(
            namespace.is_some_and(|ns| reader.move_to_attribute_ns(local, ns)),
            namespace.is_some() && value.is_some()
        );
        reader.move_to_element();
    }
    assert_eq!(
        (reader.get_attribute_no(2), reader.get_attribute_no(9)),
        (Some("1"), None)
    );
    assert!(
        reader.move_to_next_attribute(),
        "from the element, the first attribute"
    );
    assert_eq!(reader.name(), Some("xmlns:p"));

    assert!(reader.read().unwrap(), "reading leaves the attribute");
    assert_eq!(
        (reader.name(), reader.depth(), reader.attribute_count()),
        (Some("c"), 1, 0)
    );
    assert!(!reader.move_to_first_attribute() && !reader.move_to_attribute("a"));
    assert_eq!(reader.namespace_uri(), Some("urn:d"));
    assert!(reader.read().unwrap());
    assert_eq!(
        (
            reader.node_type(),
            reader.attribute_count(),
            reader.has_attributes()
        ),
        (NodeType::EndElement, 0, true)
    );
    assert!(
        !reader.move_to_first_attribute(),
        "attributes are not kept for EndElement"
    );
}

#[test]
fn scope_reports_namespaces_language_and_base() {
    let input = "<r xmlns:p='urn:p' xml:lang='en' xml:base='http://h/a/'><s xmlns:p='urn:q' xml:base='b/'>t</s><u xml:lang='' xml:base='urn:x'/></r>";
    let options = ParseOptions {
        base: Some("file:///doc.xml".into()),
        ..ParseOptions::default()
    };
    let mut reader = TextReader::with_options(input.as_bytes(), options).unwrap();
    let mut seen = Vec::new();
    while reader.read().unwrap() {
        seen.push((
            reader.lookup_namespace(Some("p")).map(str::to_owned),
            reader.lookup_namespace(None).map(str::to_owned),
            reader.xml_lang().map(str::to_owned),
            reader.base_uri(),
        ));
    }
    let row = |p: &str, lang: &str, base: &str| {
        (
            Some(p.to_owned()),
            None,
            Some(lang.to_owned()),
            Some(base.to_owned()),
        )
    };
    assert_eq!(
        seen,
        [
            row("urn:p", "en", "http://h/a/"),
            row("urn:q", "en", "http://h/a/b/"),
            row("urn:q", "en", "http://h/a/b/"),
            row("urn:q", "en", "http://h/a/b/"),
            row("urn:p", "", "urn:x"),
            row("urn:p", "en", "http://h/a/"),
        ]
    );
    // Without xml:base the document URI applies; without either there is none.
    let mut reader = TextReader::new(b"<r xml:base='sub/'/>".as_slice());
    assert_eq!(reader.base_uri(), None, "no current node");
    reader.read().unwrap();
    assert_eq!(
        (
            reader.base_uri().as_deref(),
            reader.lookup_namespace(Some("xml"))
        ),
        (Some("sub/"), Some("http://www.w3.org/XML/1998/namespace"))
    );
    assert_eq!(
        (reader.xml_lang(), reader.lookup_namespace(Some("p"))),
        (None, None)
    );
}

#[test]
fn next_skips_subtrees() {
    let input = "<r><a><b>x</b><c/></a><d/>t</r>";
    for (skips, expected) in [
        (
            &[][..],
            &["r", "a", "b", "#text", "b", "c", "a", "d", "#text", "r"][..],
        ),
        (&["a"][..], &["r", "a", "d", "#text", "r"][..]),
        (&["r"][..], &["r"][..]),
        // Empty elements and leaves are simply read past.
        (
            &["c", "d", "#text"][..],
            &["r", "a", "b", "#text", "b", "c", "a", "d", "#text", "r"][..],
        ),
    ] {
        let mut reader = TextReader::new(input.as_bytes());
        let mut seen = Vec::new();
        let mut more = reader.read().unwrap();
        while more {
            let name = reader.name().unwrap().to_owned();
            more = if skips.contains(&name.as_str()) && reader.node_type() != NodeType::EndElement {
                reader.move_to_first_attribute();
                reader.next().unwrap()
            } else {
                reader.read().unwrap()
            };
            seen.push(name);
        }
        assert_eq!(seen, expected, "{skips:?}");
    }
}

/// Serializes an owned document for comparison.
fn written(document: &OwnedDocument) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new());
    writer.write_owned_document(document).unwrap();
    writer.into_inner()
}

#[test]
fn expand_equals_parsing_the_subtree() {
    for (input, target, subtree) in [
        (
            "<r><a k='v'>x<b/><!--c--><?p d?><![CDATA[<]]></a></r>",
            "a",
            "<a k='v'>x<b/><!--c--><?p d?><![CDATA[<]]></a>",
        ),
        (
            "<r xmlns='urn:d' xmlns:p='urn:p'><p:a><b p:x='1'/></p:a></r>",
            "p:a",
            "<p:a xmlns:p='urn:p' xmlns='urn:d'><b p:x='1'/></p:a>",
        ),
        (
            "<!DOCTYPE r [<!ENTITY e '<i>e</i>'><!ATTLIST i d CDATA 'z'>]><r><s>&e;</s></r>",
            "s",
            "<s><i>e</i></s>",
        ),
        ("<r><e/></r>", "e", "<e/>"),
        ("<r>&#13;&lt;</r>", "r", "<r>&#13;&lt;</r>"),
    ] {
        for bytes in encodings(input) {
            let mut reader = TextReader::new(bytes.as_slice());
            while reader.name() != Some(target) {
                assert!(reader.read().unwrap());
            }
            reader.move_to_first_attribute();
            let expanded = reader.expand().unwrap().expect("element expands");
            assert_eq!(
                written(&expanded),
                written(&OwnedDocument::parse(subtree).unwrap()),
                "{input}"
            );
            // The cursor did not move, and the subtree is still read.
            reader.move_to_element();
            assert_eq!(reader.name(), Some(target));
            let rest = lines(reader).unwrap();
            let fresh = lines(TextReader::new(bytes.as_slice())).unwrap();
            assert!(fresh.ends_with(&rest), "{input}");
        }
    }
    for (input, reads) in [
        ("<!DOCTYPE r><r>t</r>", 1),
        ("<r>t</r>", 2),
        ("<r></r>", 2),
        ("<r/>", 0),
    ] {
        let mut reader = TextReader::new(input.as_bytes());
        for _ in 0..reads {
            reader.read().unwrap();
        }
        assert!(reader.expand().unwrap().is_none(), "{input}");
    }
}

#[test]
fn content_strings_cover_each_node_kind() {
    let input = "<!DOCTYPE r><r a='1'>x<b>y<!--c--><![CDATA[z]]></b><e></e></r>";
    let mut reader = TextReader::new(input.as_bytes());
    let mut seen = Vec::new();
    while reader.read().unwrap() {
        seen.push([
            reader.read_string().unwrap(),
            reader.read_inner_xml().unwrap(),
            reader.read_outer_xml().unwrap(),
        ]);
    }
    let some = |s: &str| Some(s.to_owned());
    assert_eq!(
        seen,
        [
            [None, None, None],
            [
                some("xyz"),
                some("x<b>y<!--c--><![CDATA[z]]></b><e/>"),
                some("<r a=\"1\">x<b>y<!--c--><![CDATA[z]]></b><e/></r>")
            ],
            [some("x"), some(""), some("x")],
            [
                some("yz"),
                some("y<!--c--><![CDATA[z]]>"),
                some("<b>y<!--c--><![CDATA[z]]></b>")
            ],
            [some("y"), some(""), some("y")],
            [None, some(""), some("<!--c-->")],
            [some("z"), some(""), some("<![CDATA[z]]>")],
            [None, None, None],
            [None, some(""), some("<e/>")],
            [None, None, None],
            [None, None, None],
        ]
    );
    let mut reader = TextReader::new(b"<r a='1'/>".as_slice());
    assert_eq!(
        reader.read_outer_xml().unwrap(),
        None,
        "before the first read"
    );
    reader.read().unwrap();
    reader.move_to_first_attribute();
    assert_eq!(reader.read_string().unwrap(), None, "on an attribute");
    assert_eq!(
        reader.read_outer_xml().unwrap().as_deref(),
        Some("<r a=\"1\"/>"),
        "of its element"
    );
}

#[test]
fn subtree_buffering_is_bounded_and_recoverable() {
    let input = format!("<r><big>{}</big><after/></r>", "<i>text</i>".repeat(100));
    let limits = Limits {
        max_owned_bytes: 200,
        ..Limits::default()
    };
    let mut reader = TextReader::with_limits(input.as_bytes(), limits);
    reader.read().unwrap();
    reader.read().unwrap();
    for result in [
        reader.expand().map(drop),
        reader.read_outer_xml().map(drop),
        reader.read_string().map(drop),
    ] {
        match result {
            Err(ReadError::Xml(error)) => {
                assert_eq!(error.kind, ErrorKind::LimitExceeded(Limit::OwnedBytes))
            }
            other => panic!("{other:?}"),
        }
    }
    // The failure is not sticky: the buffered nodes are replayed in order.
    assert_eq!(reader.read_state(), ReadState::Interactive);
    let rest = lines(reader).unwrap();
    assert_eq!(rest.len(), 300 + 3);
    assert_eq!(rest[rest.len() - 2..], ["1 1 after 1 0", "0 15 r 0 0"]);
}

#[test]
fn errors_are_sticky_after_the_valid_prefix() {
    for (input, prefix, kind) in [
        ("<r><a></b></r>", 2, ErrorKind::MismatchedEndTag),
        ("<r>&undefined;</r>", 1, ErrorKind::UnknownEntity),
        ("<r>text", 2, ErrorKind::UnexpectedEof),
        ("<r/><s/>", 1, ErrorKind::MultipleRoots),
        ("<r a='1' a='2'/>", 0, ErrorKind::DuplicateAttribute),
        ("", 0, ErrorKind::MissingRoot),
    ] {
        let mut reader = TextReader::new(input.as_bytes());
        for _ in 0..prefix {
            assert!(reader.read().unwrap(), "{input}");
        }
        for _ in 0..2 {
            match reader.read() {
                Err(ReadError::Xml(error)) => assert_eq!(error.kind, kind, "{input}"),
                other => panic!("{input}: {other:?}"),
            }
            assert_eq!(
                (reader.read_state(), reader.node_type(), reader.name()),
                (ReadState::Error, NodeType::None, None)
            );
        }
    }
    // A subtree that cannot complete fails expansion and later reads alike.
    let mut reader = TextReader::new(b"<r><a>x</b></r>".as_slice());
    reader.read().unwrap();
    assert!(
        matches!(reader.expand(), Err(ReadError::Xml(e)) if e.kind == ErrorKind::MismatchedEndTag)
    );
    assert!(
        matches!(reader.read(), Err(ReadError::Xml(e)) if e.kind == ErrorKind::MismatchedEndTag)
    );
}

/// Serves `input`, then fails.
struct Failing<'a>(&'a [u8]);

impl Read for Failing<'_> {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        unreachable!("BufRead only")
    }
}

impl BufRead for Failing<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.0.is_empty() {
            return Err(io::Error::other("disk"));
        }
        Ok(self.0)
    }

    fn consume(&mut self, amount: usize) {
        self.0 = &self.0[amount..];
    }
}

#[test]
fn io_failures_surface_once_after_complete_nodes() {
    for (input, delivered) in [("<r>text<", 2), ("<r>text", 1), ("<r", 0), ("<r><a/>", 2)] {
        let mut reader = TextReader::new(Failing(input.as_bytes()));
        for _ in 0..delivered {
            assert!(reader.read().unwrap(), "{input}");
        }
        assert!(
            matches!(reader.read(), Err(ReadError::Io(e)) if e.to_string() == "disk"),
            "{input}"
        );
        assert!(
            matches!(reader.read(), Err(ReadError::Xml(e)) if e.kind == ErrorKind::InvalidState),
            "{input}"
        );
        assert_eq!(reader.read_state(), ReadState::Error);
    }
}

#[test]
fn eof_and_initial_states() {
    let mut reader = TextReader::new(b"<r/>".as_slice());
    assert_eq!(
        (
            reader.read_state(),
            reader.node_type(),
            reader.depth(),
            reader.value()
        ),
        (ReadState::Initial, NodeType::None, 0, None)
    );
    assert!(
        !reader.has_value() && !reader.has_attributes() && reader.lookup_namespace(None).is_none()
    );
    assert!(reader.read().unwrap());
    assert_eq!(reader.read_state(), ReadState::Interactive);
    for _ in 0..2 {
        assert!(!reader.read().unwrap());
        assert!(!reader.next().unwrap());
        assert_eq!(
            (reader.read_state(), reader.node_type(), reader.xml_lang()),
            (ReadState::Eof, NodeType::None, None)
        );
    }
}

#[test]
fn parser_properties_accept_only_the_fixed_behavior() {
    let resolver = || -> Box<dyn yaxml::Resolver> { Box::new(|_: Request<'_>| Ok(None)) };
    for (loads, property, value, accepted) in [
        (false, ParserProperty::LoadDtd, false, true),
        (false, ParserProperty::LoadDtd, true, false),
        (true, ParserProperty::LoadDtd, true, true),
        (true, ParserProperty::LoadDtd, false, false),
        (false, ParserProperty::DefaultAttributes, false, true),
        (false, ParserProperty::DefaultAttributes, true, false),
        (true, ParserProperty::DefaultAttributes, true, true),
        (false, ParserProperty::SubstituteEntities, false, true),
        (false, ParserProperty::SubstituteEntities, true, false),
        (true, ParserProperty::SubstituteEntities, true, true),
        (false, ParserProperty::Validate, false, true),
        (false, ParserProperty::Validate, true, false),
    ] {
        let input = b"<r/>".as_slice();
        let mut reader = if loads {
            TextReader::with_options(input, loading(Limits::default(), "doc.xml", resolver()))
                .unwrap()
        } else {
            TextReader::new(input)
        };
        let result = reader.set_parser_property(property, value);
        assert_eq!(result.is_ok(), accepted, "{property:?} {value}");
        if let Err(error) = result {
            assert_eq!(error.kind, ErrorKind::UnsupportedOption);
        }
        assert_eq!(
            reader.parser_property(property),
            if accepted { value } else { !value }
        );
    }
}

#[test]
fn resolver_loads_external_declarations_and_sets_the_base() {
    let resolver = |request: Request<'_>| {
        Ok(match request.uri {
            "dir/r.dtd" => Some(b"<!ENTITY e SYSTEM 'e.xml'><!ATTLIST r d CDATA 'x'>".to_vec()),
            "dir/e.xml" => Some(b"<i>loaded</i>".to_vec()),
            _ => None,
        })
    };
    let input = b"<!DOCTYPE r SYSTEM 'r.dtd'><r>&e;</r>";
    let options = loading(Limits::default(), "dir/doc.xml", Box::new(resolver));
    let mut reader = TextReader::with_options(input.as_slice(), options).unwrap();
    assert!(reader.parser_property(ParserProperty::LoadDtd));
    reader.read().unwrap();
    reader.read().unwrap();
    assert_eq!(
        (reader.get_attribute("d"), reader.base_uri().as_deref()),
        (Some("x"), Some("dir/doc.xml"))
    );
    assert_eq!(reader.read_string().unwrap().as_deref(), Some("loaded"));
    // Without a resolver the subset is skipped and the undeclared entity
    // kept as an empty reference, as libxml2 does without NOENT.
    let skipped = lines(TextReader::new(input.as_slice())).unwrap();
    assert_eq!(
        skipped,
        ["0 10 r 0 0", "0 1 r 0 0", "1 5 e 0 0", "0 15 r 0 0"]
    );
}

/// Complete DTD processing (`NOENT | DTDATTR | DTDLOAD`) through `resolver`.
fn loading(limits: Limits, base: &str, resolver: Box<dyn yaxml::Resolver>) -> ParseOptions {
    let flags = yaxml::options::LEGACY | yaxml::options::DTDLOAD;
    ParseOptions::new(flags)
        .with_limits(limits)
        .with_resolver(base, resolver)
}
