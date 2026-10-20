#![forbid(unsafe_code)]

use std::cell::Cell;
use std::io::{self, BufRead, Read};
use std::rc::Rc;

use yaxml::options::{DTDATTR, NOBLANKS, NOENT};
use yaxml::{
    ErrorKind, Limit, Limits, NodeType, OwnedDocument, ParseOptions, ParserProperty, ReadError,
    ReadState, Request, Standalone, TextReader, Writer,
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
    let options = ParseOptions::default().base(Some("file:///doc.xml".into()));
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
    let limits = Limits::default().max_owned_bytes(200);
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

/// `xmlTextReaderSetParserProp`: (resolver, property, value, outcome before
/// the first read, outcome after it, the property then read before the read).
#[test]
fn parser_properties_change_flags_before_the_first_read() {
    use ParserProperty::{DefaultAttributes, LoadDtd, SubstituteEntities, Validate};
    let (no, late) = (ErrorKind::UnsupportedOption, ErrorKind::InvalidState);
    let resolver = || -> Box<dyn yaxml::Resolver> { Box::new(|_: Request<'_>| Ok(None)) };
    for (loads, property, value, before, after, reads) in [
        (false, LoadDtd, false, None, None, false),
        (false, LoadDtd, true, Some(no), Some(no), false),
        (true, LoadDtd, true, None, None, true),
        // `DTDATTR` still loads the subset, as libxml2's `loadsubset` does.
        (true, LoadDtd, false, None, Some(late), true),
        (false, DefaultAttributes, false, None, None, false),
        (false, DefaultAttributes, true, None, Some(late), true),
        (true, DefaultAttributes, true, None, None, true),
        (true, DefaultAttributes, false, None, Some(late), false),
        (false, SubstituteEntities, false, None, None, false),
        (false, SubstituteEntities, true, None, Some(late), true),
        (true, SubstituteEntities, true, None, None, true),
        (true, SubstituteEntities, false, None, Some(late), false),
        (false, Validate, false, None, None, false),
        (false, Validate, true, None, Some(late), true),
        (true, Validate, true, None, Some(late), true),
        (true, Validate, false, None, None, false),
    ] {
        let make = || {
            let input = b"<r/>".as_slice();
            match loads {
                true => {
                    let options = loading(Limits::default(), "doc.xml", resolver());
                    TextReader::with_options(input, options).unwrap()
                }
                false => TextReader::new(input),
            }
        };
        let (mut early, mut late_reader) = (make(), make());
        late_reader.read().unwrap();
        let name = format!("{property:?} {value} loads={loads}");
        let kinds = [&mut early, &mut late_reader].map(|reader| {
            reader
                .set_parser_property(property, value)
                .err()
                .map(|e| e.kind)
        });
        assert_eq!(kinds, [before, after], "{name}");
        assert_eq!(early.parser_property(property), reads, "{name}");
        // A refused or redundant change leaves the property as it was.
        assert_eq!(
            late_reader.parser_property(property),
            make().parser_property(property),
            "{name}"
        );
    }
}

#[test]
fn set_parser_property_reconfigures_the_stream() {
    let input = b"<!DOCTYPE r [<!ENTITY e 'ex'><!ATTLIST r a CDATA 'd'>]><r>&e;</r>".as_slice();
    let plain = lines(TextReader::new(input)).unwrap();
    assert_eq!(plain[2], "1 5 e 0 0");
    for (property, check) in [
        (ParserProperty::SubstituteEntities, "1 3 #text 0 1 ex"),
        (ParserProperty::DefaultAttributes, "1 5 e 0 0"),
    ] {
        let mut reader = TextReader::new(input);
        reader.set_parser_property(property, true).unwrap();
        let mut seen = Vec::new();
        while reader.read().unwrap() {
            seen.push((line(&reader), reader.get_attribute("a").map(str::to_owned)));
        }
        assert_eq!(seen[2].0, check, "{property:?}");
        let defaulted = property == ParserProperty::DefaultAttributes;
        assert_eq!(
            seen[1].1.as_deref(),
            defaulted.then_some("d"),
            "{property:?}"
        );
    }
    // The resolver survives a change: the external subset is still loaded.
    let dtd = |request: Request<'_>| {
        Ok((request.uri == "x.dtd").then(|| b"<!ATTLIST r d CDATA 'x'>".to_vec()))
    };
    let options = loading(Limits::default(), "doc.xml", Box::new(dtd));
    let mut reader =
        TextReader::with_options(b"<!DOCTYPE r SYSTEM 'x.dtd'><r/>".as_slice(), options).unwrap();
    reader
        .set_parser_property(ParserProperty::SubstituteEntities, false)
        .unwrap();
    reader.read().unwrap();
    reader.read().unwrap();
    assert_eq!(reader.get_attribute("d"), Some("x"));
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

/// `xmlTextReaderRelaxNGSetSchema` and `IsValid`: (flags, document, codes).
/// Kept entity references are validated through their content, the DTD
/// serves `ENTITY` values, and `expand` does not validate a subtree twice.
#[test]
fn relaxng_validates_while_reading() {
    use yaxml::relaxng::{Code, Limits as RngLimits, Schema};
    let rng = br#"<element name="r" xmlns="http://relaxng.org/ns/structure/1.0"
        datatypeLibrary="http://www.w3.org/2001/XMLSchema-datatypes">
      <oneOrMore><element name="a"><optional><data type="ENTITY"/></optional></element></oneOrMore>
    </element>"#;
    let none = |_: Request<'_>| Ok(None);
    let schema = Schema::parse(rng, "s.rng", Box::new(none), &RngLimits::default()).unwrap();
    let noent = yaxml::options::NOENT;
    let dtd =
        "<!DOCTYPE r [<!NOTATION n SYSTEM 'n'><!ENTITY u SYSTEM 'u' NDATA n><!ENTITY e '<a/>'>]>";
    let cases: &[(u32, &str, &[Code])] = &[
        (0, "<r><a/></r>", &[]),
        (0, "<r/>", &[Code::MissingElement]),
        (
            0,
            "<r><b/><a>x</a></r>",
            &[Code::ElementName, Code::Datatype],
        ),
        (0, &format!("{dtd}<r>&e;</r>"), &[]),
        (noent, &format!("{dtd}<r>&e;</r>"), &[]),
        (0, &format!("{dtd}<r><a>u</a></r>"), &[]),
        (0, &format!("{dtd}<r><a>e</a></r>"), &[Code::Datatype]),
    ];
    for &(flags, xml, expected) in cases {
        for expand in [false, true] {
            let options = ParseOptions::new(flags);
            let mut reader = TextReader::with_options(xml.as_bytes(), options).unwrap();
            assert!(!reader.is_valid(), "no schema yet");
            reader.relaxng_set_schema(Some(&schema)).unwrap();
            while reader.read().unwrap() {
                if expand && reader.node_type() == NodeType::Element {
                    reader.expand().unwrap();
                }
            }
            let found: Vec<_> = reader
                .relaxng_diagnostics()
                .iter()
                .map(|d| d.code)
                .collect();
            assert_eq!(found, expected, "{xml}");
            assert_eq!(reader.is_valid(), expected.is_empty(), "{xml}");
        }
    }
    // After the first read the schema is fixed; without one nothing is checked.
    let mut reader = TextReader::new(b"<r/>".as_slice());
    reader.relaxng_set_schema(Some(&schema)).unwrap();
    reader.relaxng_set_schema(None).unwrap();
    assert!(reader.read().unwrap());
    let error = reader.relaxng_set_schema(Some(&schema)).unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidState);
    while reader.read().unwrap() {}
    assert!(reader.relaxng_diagnostics().is_empty() && !reader.is_valid());
}

/// `xmlTextReaderSetSchema` and `IsValid`: the XML Schema verdicts and
/// diagnostics equal `Schema::validate_stream`'s, with kept entity references,
/// the DTD serving `ENTITY` values, `expand` and a RELAX NG schema alongside.
#[test]
fn xsd_validates_while_reading() {
    use yaxml::relaxng;
    use yaxml::xsd::{self, Code, Schema};
    let xs = br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
      <xs:element name="r"><xs:complexType><xs:sequence>
        <xs:element name="a" minOccurs="0" maxOccurs="2"><xs:complexType>
          <xs:simpleContent><xs:extension base="xs:string">
            <xs:attribute name="e" type="xs:ENTITY"/></xs:extension></xs:simpleContent>
        </xs:complexType></xs:element>
        <xs:element name="n" type="xs:int" minOccurs="0"/>
      </xs:sequence></xs:complexType></xs:element></xs:schema>"#;
    let schema = Schema::parse(xs, "s.xsd").unwrap();
    let dtd = "<!DOCTYPE r [<!NOTATION n SYSTEM 'n'><!ENTITY u SYSTEM 'u' NDATA n>\
        <!ENTITY t 'x'><!ENTITY m '<n>1</n>'>]>";
    let noent = yaxml::options::NOENT;
    let cases: &[(u32, String, &[Code])] = &[
        (0, "<r><a/><n>1</n></r>".into(), &[]),
        (0, "<r/>".into(), &[]),
        (0, "<r><n>x</n></r>".into(), &[Code::CvcDatatypeValid1_2_1]),
        (0, "<r><a/><a/><a/></r>".into(), &[Code::ElementContent]),
        (0, "<q/>".into(), &[Code::CvcElt1]),
        (0, format!("{dtd}<r><a e='u'/>&m;</r>"), &[]),
        (noent, format!("{dtd}<r><a e='u'/>&m;</r>"), &[]),
        (
            0,
            format!("{dtd}<r><a e='t'/></r>"),
            &[Code::CvcDatatypeValid1_2_1],
        ),
        (0, format!("{dtd}<r><a>&t;</a></r>"), &[]),
    ];
    let relaxng = relaxng::Schema::parse(
        br#"<element name="r" xmlns="http://relaxng.org/ns/structure/1.0"><empty/></element>"#,
        "s.rng",
        Box::new(|_: Request<'_>| Ok(None)),
        &relaxng::Limits::default(),
    )
    .unwrap();
    for (flags, xml, expected) in cases {
        let options = || ParseOptions::new(*flags);
        let expected_codes = schema
            .validate_stream(xml.as_bytes(), options())
            .unwrap()
            .into_iter()
            .map(|d| (d.code, d.offset, d.message))
            .collect::<Vec<_>>();
        for (expand, with_relaxng) in [(false, false), (true, false), (false, true)] {
            let mut reader = TextReader::with_options(xml.as_bytes(), options()).unwrap();
            assert!(!reader.is_valid(), "no schema yet");
            reader.xsd_set_schema(Some(&schema)).unwrap();
            reader
                .relaxng_set_schema(with_relaxng.then_some(&relaxng))
                .unwrap();
            while reader.read().unwrap() {
                if expand && reader.node_type() == NodeType::Element {
                    reader.expand().unwrap();
                }
            }
            let found = reader
                .xsd_diagnostics()
                .iter()
                .map(|d| (d.code, d.offset, d.message.clone()))
                .collect::<Vec<_>>();
            assert_eq!(found, expected_codes, "{xml}");
            let codes: Vec<_> = found.iter().map(|d| d.0).collect();
            assert_eq!(codes, *expected, "{xml}");
            let rng_ok = !with_relaxng || reader.relaxng_diagnostics().is_empty();
            assert_eq!(
                reader.is_valid(),
                xsd::is_valid(reader.xsd_diagnostics()) && rng_ok,
                "{xml}"
            );
        }
    }
    // Either validator failing makes the reader invalid; both passing makes it valid.
    let mut reader = TextReader::new(b"<r><a/></r>".as_slice());
    reader.xsd_set_schema(Some(&schema)).unwrap();
    reader.relaxng_set_schema(Some(&relaxng)).unwrap();
    while reader.read().unwrap() {}
    assert!(reader.xsd_diagnostics().is_empty() && !reader.relaxng_diagnostics().is_empty());
    assert!(!reader.is_valid());
    // After the first read the schema is fixed; without one nothing is checked.
    let mut reader = TextReader::new(b"<r/>".as_slice());
    reader.xsd_set_schema(Some(&schema)).unwrap();
    reader.xsd_set_schema(None).unwrap();
    assert!(reader.read().unwrap());
    let error = reader.xsd_set_schema(Some(&schema)).unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidState);
    while reader.read().unwrap() {}
    assert!(reader.xsd_diagnostics().is_empty() && !reader.is_valid());
}

/// `PushParser::dtd` mirrors `BufferedReader::dtd`: absent without a DOCTYPE,
/// present once one was fed.
#[test]
fn push_parser_exposes_its_dtd() {
    use std::ops::ControlFlow;
    let cases: &[(&[u8], Option<&str>)] = &[
        (b"<r/>", None),
        (b"<!DOCTYPE r [<!ENTITY e 'x'>]><r/>", Some("r")),
    ];
    for &(xml, root) in cases {
        let mut parser = yaxml::PushParser::new();
        assert!(parser.dtd().is_none(), "before input");
        parser.feed(xml, |_, _| ControlFlow::Continue(())).unwrap();
        assert_eq!(parser.dtd().and_then(|dtd| dtd.name()), root);
    }
}

/// `ReadAttributeValue`: (flags, document, attribute index, children as
/// type, name, value, depth). A value is text unless it keeps a reference.
#[test]
fn read_attribute_value_walks_the_children_of_an_attribute() {
    const TEXT: i32 = NodeType::Text as i32;
    const REF: i32 = NodeType::EntityReference as i32;
    let defs = "<!DOCTYPE r [<!ENTITY e 'ex'>]>";
    let text = |value: &'static str| (TEXT, "#text", Some(value), 2);
    let reference = |name: &'static str| (REF, name, None, 2);
    type Child = (i32, &'static str, Option<&'static str>, usize);
    let cases: [(u32, String, usize, Vec<Child>); 9] = [
        (
            0,
            format!("{defs}<r a='1&e;2'/>"),
            0,
            vec![text("1"), reference("e"), text("2")],
        ),
        (0, format!("{defs}<r a='&e;'/>"), 0, vec![reference("e")]),
        (
            NOENT,
            format!("{defs}<r a='1&e;2'/>"),
            0,
            vec![text("1ex2")],
        ),
        (
            0,
            format!("{defs}<r a='x&amp;&e;&lt;&#38;'/>"),
            0,
            vec![text("x&"), reference("e"), text("<&")],
        ),
        (0, "<r a='plain'/>".into(), 0, vec![text("plain")]),
        // An empty value is one empty text, as is an empty namespace URI.
        (0, "<r a=''/>".into(), 0, vec![text("")]),
        (0, "<r xmlns='' xmlns:p='urn:p'/>".into(), 0, vec![text("")]),
        (
            0,
            "<r xmlns='' xmlns:p='urn:p'/>".into(),
            1,
            vec![text("urn:p")],
        ),
        (
            0,
            "<r xmlns:p='urn:p' a='1' p:b='2'/>".into(),
            2,
            vec![text("2")],
        ),
    ];
    for (flags, xml, index, expected) in cases {
        let mut reader =
            TextReader::with_options(xml.as_bytes(), ParseOptions::new(flags)).unwrap();
        while reader.node_type() != NodeType::Element {
            reader.read().unwrap();
        }
        assert!(reader.move_to_attribute_no(index), "{xml}");
        let mut children = Vec::new();
        while reader.read_attribute_value() {
            children.push((
                reader.node_type() as i32,
                reader.name().unwrap().to_owned(),
                reader.value().map(str::to_owned),
                reader.depth(),
            ));
            assert_eq!(reader.has_value(), reader.value().is_some());
            assert_eq!(reader.local_name(), reader.name());
            assert!(
                !reader.has_attributes() && !reader.is_empty_element(),
                "{xml}"
            );
            assert_eq!((reader.attribute_count(), reader.prefix()), (0, None));
        }
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(t, n, v, d)| (t, n.to_owned(), v.map(str::to_owned), d))
            .collect();
        assert_eq!(children, expected, "{xml}");
        // At the end the cursor stays on the last child.
        assert!(!reader.read_attribute_value());
        assert_eq!(reader.node_type() as i32, expected.last().unwrap().0);
    }
}

#[test]
fn attribute_values_are_navigated_like_libxml2() {
    let xml = "<!DOCTYPE r [<!ENTITY e 'ex'>]><r a='1&e;2' b='y'/>";
    let mut reader = TextReader::new(xml.as_bytes());
    // Sad paths: nothing under the cursor but a document or an element.
    assert!(!reader.read_attribute_value());
    reader.read().unwrap();
    assert!(!reader.read_attribute_value(), "DOCTYPE");
    reader.read().unwrap();
    assert!(!reader.read_attribute_value(), "element, not an attribute");
    assert_eq!(reader.node_type(), NodeType::Element);
    // From a value: no next attribute, no lookups, and `read_string` is the text.
    assert!(reader.move_to_first_attribute());
    assert!(reader.read_attribute_value());
    assert_eq!(
        (
            reader.read_string().unwrap().as_deref(),
            reader.get_attribute_no(0)
        ),
        (Some("1"), None)
    );
    assert!(!reader.move_to_next_attribute());
    assert!(reader.read_attribute_value());
    assert_eq!(reader.read_string().unwrap(), None, "reference");
    assert!(reader.move_to_first_attribute(), "back to the attribute");
    assert_eq!(
        (reader.node_type(), reader.value()),
        (NodeType::Attribute, Some("1ex2"))
    );
    assert!(reader.read_attribute_value() && reader.move_to_element());
    assert_eq!(reader.node_type(), NodeType::Element);
    assert!(!reader.read_attribute_value());
    // Another attribute starts its own walk.
    assert!(reader.move_to_attribute("b") && reader.read_attribute_value());
    assert_eq!(reader.value(), Some("y"));
    assert!(!reader.read().unwrap());
    assert!(!reader.read_attribute_value(), "end of document");
    // Outer XML keeps the reference; the expanded copy substitutes it.
    let mut reader = TextReader::new(xml.as_bytes());
    reader.read().unwrap();
    reader.read().unwrap();
    assert_eq!(
        reader.read_outer_xml().unwrap().as_deref(),
        Some("<r a=\"1&e;2\" b=\"y\"/>")
    );
    let copy = reader.expand().unwrap().unwrap();
    assert_eq!(copy.root_element().attribute("a"), Some("1ex2"));
}

/// `xmlTextReaderStandalone` after the first read; -1 before.
#[test]
fn standalone_follows_the_xml_declaration() {
    for (xml, expected) in [
        ("<r/>", Standalone::NoDeclaration),
        ("<?xml version='1.0'?><r/>", Standalone::Unspecified),
        (
            "<?xml version='1.0' encoding='UTF-8'?><r/>",
            Standalone::Unspecified,
        ),
        ("<?xml version='1.0' standalone='no'?><r/>", Standalone::No),
        (
            "<?xml version='1.0' standalone='yes'?><r/>",
            Standalone::Yes,
        ),
    ] {
        let mut reader = TextReader::new(xml.as_bytes());
        assert_eq!(
            reader.standalone(),
            Standalone::NoDeclaration,
            "before: {xml}"
        );
        reader.read().unwrap();
        assert_eq!(reader.standalone(), expected, "{xml}");
        while reader.read().unwrap() {}
        assert_eq!(reader.standalone(), expected, "end: {xml}");
        // A new document forgets the old one's declaration.
        reader.reset(b"<s/>".as_slice());
        assert_eq!(
            reader.standalone(),
            Standalone::NoDeclaration,
            "reset: {xml}"
        );
    }
    assert_eq!(
        [-1, -2, 0, 1],
        [
            Standalone::NoDeclaration,
            Standalone::Unspecified,
            Standalone::No,
            Standalone::Yes
        ]
        .map(|s| s as i32)
    );
}

/// libxml2's `IsDefault` returns 0 and `Normalization` 1 for every node.
#[test]
fn is_default_and_normalization_are_constant() {
    let xml = "<!DOCTYPE r [<!ATTLIST r d CDATA 'x'>]><r a='1'>t</r>";
    for flags in [0, DTDATTR] {
        let mut reader =
            TextReader::with_options(xml.as_bytes(), ParseOptions::new(flags)).unwrap();
        assert!(
            !reader.is_default() && reader.normalization(),
            "before the first node"
        );
        while reader.read().unwrap() {
            assert!(!reader.is_default() && reader.normalization());
            let mut next = reader.move_to_first_attribute();
            while next {
                assert!(!reader.is_default(), "{flags:#x} {:?}", reader.name());
                next = reader.move_to_next_attribute();
            }
        }
        assert!(!reader.is_default() && reader.normalization(), "at the end");
    }
}

#[test]
fn byte_consumed_counts_the_input_the_parser_took() {
    let utf16: Vec<u8> = "\u{feff}<r>a<b/>c</r>"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    for input in [
        b"<r>a<b/>c</r>\n".to_vec(),
        "<?xml version='1.0'?><!--é--><r a='é'/>".into(),
        utf16,
    ] {
        for capacity in [1, 4, 4096] {
            let source = io::BufReader::with_capacity(capacity, input.as_slice());
            let mut reader = TextReader::new(source);
            assert_eq!(reader.byte_consumed(), 0);
            let mut last = 0;
            while reader.read().unwrap() {
                let consumed = reader.byte_consumed();
                assert!(
                    last <= consumed && consumed <= input.len(),
                    "{consumed} of {}",
                    input.len()
                );
                last = consumed;
            }
            assert_eq!(
                reader.byte_consumed(),
                input.len(),
                "end of {} bytes, capacity {capacity}",
                input.len()
            );
        }
    }
}

/// `xmlTextReaderGetRemainder`: the source after what the parser took.
#[test]
fn remainder_is_the_unparsed_rest_of_the_source() {
    let xml = format!("<r>{}</r>", "<i>x</i>".repeat(40));
    let source = || io::BufReader::with_capacity(16, io::Cursor::new(xml.clone().into_bytes()));
    let mut reader = TextReader::new(source());
    reader.read().unwrap();
    let consumed = reader.byte_consumed();
    let mut rest = Vec::new();
    reader
        .into_remainder()
        .expect("current node")
        .read_to_end(&mut rest)
        .unwrap();
    assert!(consumed < xml.len());
    assert_eq!(rest, &xml.as_bytes()[consumed..]);
    // No current node: before the first read, at the end, after an error.
    assert!(TextReader::new(source()).into_remainder().is_none());
    let mut done = TextReader::new(source());
    while done.read().unwrap() {}
    assert!(done.into_remainder().is_none());
    let mut broken = TextReader::new(io::Cursor::new(b"<r></x>".to_vec()));
    assert!(broken.read().unwrap() || broken.read().is_err());
    while broken.read().is_ok_and(|more| more) {}
    assert!(broken.into_remainder().is_none());
}

/// `NOBLANKS` as libxml2's reader: (document, nodes), from
/// `xmllint --stream --debug --noblanks`.
#[test]
fn noblanks_drops_ignorable_blanks_while_reading() {
    let cases: [(u32, &str, &[&str]); 7] = [
        (
            NOBLANKS,
            "<r>\n <a>\n  <b/>\n </a>\n <c> </c>\n</r>",
            &[
                "0 1 r 0 0",
                "1 1 a 0 0",
                "2 1 b 1 0",
                "1 15 a 0 0",
                "1 1 c 0 0",
                "2 14 #text 0 1  ",
                "1 15 c 0 0",
                "0 15 r 0 0",
            ],
        ),
        // `xml:space="preserve"` keeps blanks below it, `default` does not.
        (
            NOBLANKS,
            "<r xml:space='preserve'> <a> </a><b xml:space='default'> <c/> </b></r>",
            &[
                "0 1 r 0 0",
                "1 14 #text 0 1  ",
                "1 1 a 0 0",
                "2 14 #text 0 1  ",
                "1 15 a 0 0",
                "1 1 b 0 0",
                "2 1 c 1 0",
                "1 15 b 0 0",
                "0 15 r 0 0",
            ],
        ),
        // Element content declared by the DTD is dropped, #PCDATA is kept.
        (
            NOBLANKS,
            "<!DOCTYPE r [<!ELEMENT r (a)*><!ELEMENT a (#PCDATA)>]><r> <a> </a> </r>",
            &[
                "0 10 r 0 0",
                "0 1 r 0 0",
                "1 1 a 0 0",
                "2 14 #text 0 1  ",
                "1 15 a 0 0",
                "0 15 r 0 0",
            ],
        ),
        // Text that the document spoke makes later blanks significant.
        (
            NOBLANKS,
            "<r><a/>x<b/> <c/></r>",
            &[
                "0 1 r 0 0",
                "1 1 a 1 0",
                "1 3 #text 0 1 x",
                "1 1 b 1 0",
                "1 14 #text 0 1  ",
                "1 1 c 1 0",
                "0 15 r 0 0",
            ],
        ),
        // So does a blank before a reference.
        (
            NOBLANKS,
            "<!DOCTYPE r [<!ENTITY e 'x'>]><r><a/> &e;<a/></r>",
            &[
                "0 10 r 0 0",
                "0 1 r 0 0",
                "1 1 a 1 0",
                "1 14 #text 0 1  ",
                "1 5 e 0 0",
                "1 1 a 1 0",
                "0 15 r 0 0",
            ],
        ),
        // Without the flag nothing is dropped, and no type 13 is ever reported.
        (
            0,
            "<r xml:space='default'> <a> </a> </r>",
            &[
                "0 1 r 0 0",
                "1 14 #text 0 1  ",
                "1 1 a 0 0",
                "2 14 #text 0 1  ",
                "1 15 a 0 0",
                "1 14 #text 0 1  ",
                "0 15 r 0 0",
            ],
        ),
        (
            NOBLANKS,
            "<r> </r>",
            &["0 1 r 0 0", "1 14 #text 0 1  ", "0 15 r 0 0"],
        ),
    ];
    for (flags, xml, expected) in cases {
        let reader = TextReader::with_options(xml.as_bytes(), ParseOptions::new(flags)).unwrap();
        assert_eq!(lines(reader).unwrap(), expected, "{xml}");
    }
    // The buffered subtree is what the tree would hold.
    let xml = b"<r>\n <a>\n  <b/>\n </a>\n <c> </c><d>x <e/> y</d>\n</r>".as_slice();
    let mut reader = TextReader::with_options(xml, ParseOptions::new(NOBLANKS)).unwrap();
    assert!(reader.read().unwrap());
    assert_eq!(reader.read_string().unwrap().as_deref(), Some(" x  y"));
    assert_eq!(
        reader.read_inner_xml().unwrap().as_deref(),
        Some("<a><b/></a><c> </c><d>x <e/> y</d>")
    );
    assert_eq!(
        reader.read_outer_xml().unwrap().as_deref(),
        Some("<r><a><b/></a><c> </c><d>x <e/> y</d></r>")
    );
    let copy = reader.expand().unwrap().unwrap();
    assert_eq!(copy.root_element().children().count(), 3);
    // `next` skips the rest, and the following reads agree.
    assert!(reader.read().unwrap() && reader.next().unwrap());
    assert_eq!((reader.depth(), reader.name()), (1, Some("c")));
}

/// The integer values of `xmlReaderTypes`; the unlisted ones are never reported.
#[test]
fn node_types_carry_libxml2_values() {
    for (kind, value) in [
        (NodeType::None, 0),
        (NodeType::Element, 1),
        (NodeType::Attribute, 2),
        (NodeType::Text, 3),
        (NodeType::Cdata, 4),
        (NodeType::EntityReference, 5),
        (NodeType::Entity, 6),
        (NodeType::ProcessingInstruction, 7),
        (NodeType::Comment, 8),
        (NodeType::Document, 9),
        (NodeType::DocumentType, 10),
        (NodeType::DocumentFragment, 11),
        (NodeType::Notation, 12),
        (NodeType::Whitespace, 13),
        (NodeType::SignificantWhitespace, 14),
        (NodeType::EndElement, 15),
        (NodeType::EndEntity, 16),
        (NodeType::XmlDeclaration, 17),
    ] {
        assert_eq!(kind as i32, value, "{kind:?}");
    }
    let xml = "<?xml version='1.0'?><!DOCTYPE r [<!ENTITY e 'x'><!NOTATION n SYSTEM 'n'>]>\n<r xml:space='default'>\n<a b='&e;'/> <![CDATA[ ]]>&e;<?p?><!--c--></r>";
    for flags in [0, NOENT, NOBLANKS, NOENT | DTDATTR] {
        let mut reader =
            TextReader::with_options(xml.as_bytes(), ParseOptions::new(flags)).unwrap();
        while reader.read().unwrap() {
            let kind = reader.node_type();
            let never = [
                NodeType::Entity,
                NodeType::Document,
                NodeType::DocumentFragment,
                NodeType::Notation,
                NodeType::Whitespace,
                NodeType::EndEntity,
                NodeType::XmlDeclaration,
            ];
            assert!(!never.contains(&kind), "{flags:#x} {kind:?}");
        }
    }
}
