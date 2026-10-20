#![forbid(unsafe_code)]
//! Namespace errors (libxml2's `xmlNsErr`: unbound prefixes, malformed
//! QNames, unbindable declarations, duplicate expanded names, colons in PI,
//! entity and notation names) never stop a parse: every surface keeps them
//! in `errors()`, with or without `RECOVER`, and keeps the names in no
//! namespace. Expectations were checked against `xmllint` 2.15.4.

use std::io::BufReader;
use std::ops::ControlFlow;

use yaxml::options::{NOENT, RECOVER};
use yaxml::{
    BufferedReader, Document, EditLimits, Error, ErrorKind, Event, LineIndex, NodeKind, NodeType,
    OwnedDocument, ParseOptions, PushParser, Reader, TextReader,
};

/// Element and attribute names in document order, `qualified{namespace}`.
type Names = Vec<String>;

fn label(qualified: &str, namespace: Option<&str>, attribute: bool) -> String {
    let at = if attribute { "@" } else { "" };
    format!("{at}{qualified}{{{}}}", namespace.unwrap_or(""))
}

fn element_names(element: &yaxml::Element<'_>) -> Names {
    let name = &element.name;
    let attributes = element.attributes.iter().map(|attribute| {
        let name = &attribute.name;
        label(name.qualified(), name.namespace(), true)
    });
    std::iter::once(label(name.qualified(), name.namespace(), false))
        .chain(attributes)
        .collect()
}

fn options(flags: u32) -> ParseOptions {
    ParseOptions::new(NOENT | flags)
}

fn reader(input: &[u8], flags: u32) -> Result<(Names, Vec<Error>), Error> {
    let mut reader = Reader::with_options(input, options(flags))?;
    let mut names = Vec::new();
    while let Some(event) = reader.read_event()? {
        if let Event::Start(element) | Event::Empty(element) = &event {
            names.extend(element_names(element));
        }
    }
    Ok((names, reader.errors().to_vec()))
}

fn push(input: &[u8], flags: u32) -> Result<(Names, Vec<Error>), Error> {
    let mut parser = PushParser::with_options(options(flags))?;
    let mut names = Vec::new();
    let mut emit = |event: &Event<'_>, _| {
        if let Event::Start(element) | Event::Empty(element) = event {
            names.extend(element_names(element));
        }
        ControlFlow::Continue(())
    };
    for chunk in input.chunks(3) {
        parser.feed(chunk, &mut emit)?;
    }
    parser.finish(&mut emit)?;
    Ok((names, parser.errors().to_vec()))
}

fn buffered(input: &[u8], flags: u32) -> Result<(Names, Vec<Error>), Error> {
    let source = BufReader::with_capacity(5, input);
    let mut reader = BufferedReader::with_options(source, options(flags))?;
    let mut names = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Some((Event::Start(element) | Event::Empty(element), _))) => {
                names.extend(element_names(&element));
            }
            Ok(Some(_)) => {}
            Ok(None) => return Ok((names, reader.errors().to_vec())),
            Err(yaxml::ReadError::Xml(error)) => return Err(error),
            Err(error) => panic!("in-memory source failed: {error}"),
        }
    }
}

fn text_reader(input: &[u8], flags: u32) -> Result<(Names, Vec<Error>), Error> {
    let mut reader = TextReader::with_options(input, options(flags))?;
    let mut names = Vec::new();
    let xml = |error| match error {
        yaxml::ReadError::Xml(error) => error,
        error => panic!("in-memory source failed: {error}"),
    };
    while reader.read().map_err(xml)? {
        if reader.node_type() != NodeType::Element {
            continue;
        }
        let name = reader.name().unwrap().to_owned();
        names.push(label(&name, reader.namespace_uri(), false));
        // libxml2's reader names an unbound node without a prefix.
        if reader.namespace_uri().is_none() {
            assert_eq!(reader.prefix(), None);
            assert_eq!(reader.local_name(), Some(name.as_str()));
        }
        while reader.move_to_next_attribute() {
            let name = reader.name().unwrap().to_owned();
            names.push(label(&name, reader.namespace_uri(), true));
        }
    }
    Ok((names, reader.errors().to_vec()))
}

fn document(input: &[u8], flags: u32) -> Result<(Names, Vec<Error>), Error> {
    let document = Document::parse_with(input, options(flags))?;
    let names = document.nodes().filter_map(|node| match node.kind() {
        NodeKind::Element(element) => Some(element_names(element)),
        _ => None,
    });
    Ok((names.flatten().collect(), document.errors().to_vec()))
}

fn owned(input: &[u8], flags: u32) -> Result<(Names, Vec<Error>), Error> {
    let document = OwnedDocument::parse_with(input, options(flags))?;
    let mut names = Vec::new();
    for node in document.nodes() {
        let Some(name) = node.name() else { continue };
        names.push(label(name.qualified(), name.namespace(), false));
        names.extend(node.attributes().map(|attribute| {
            let name = attribute.name;
            label(name.qualified(), name.namespace(), true)
        }));
    }
    Ok((names, document.errors().to_vec()))
}

type Surface = fn(&[u8], u32) -> Result<(Names, Vec<Error>), Error>;

const SURFACES: [(&str, Surface); 6] = [
    ("Reader", reader),
    ("PushParser", push),
    ("BufferedReader", buffered),
    ("TextReader", text_reader),
    ("Document", document),
    ("OwnedDocument", owned),
];

/// Sorts each element's attributes: the text reader lists declarations first.
fn normalized(names: Names) -> Names {
    let mut groups: Vec<Names> = Vec::new();
    for name in names {
        match groups.last_mut() {
            Some(group) if name.starts_with('@') => group.push(name),
            _ => groups.push(vec![name]),
        }
    }
    groups
        .into_iter()
        .flat_map(|mut group| {
            group[1..].sort();
            group
        })
        .collect()
}

#[test]
fn namespace_errors_are_kept_on_every_surface() {
    // (input, namespace errors at these offsets, names as kept)
    #[rustfmt::skip]
    let cases: &[(&str, &[usize], &[&str])] = &[
        // XML_NS_ERR_UNDEFINED_NAMESPACE: element, attribute, nested scope.
        ("<r><p:c/></r>", &[4], &["r{}", "p:c{}"]),
        ("<r p:a='1'/>", &[0], &["r{}", "@p:a{}"]),
        ("<p:r p:a='1'/>", &[0, 1], &["p:r{}", "@p:a{}"]),
        ("<r><a xmlns:p='u'/><p:c/></r>", &[20], &["r{}", "a{}", "@xmlns:p{http://www.w3.org/2000/xmlns/}", "p:c{}"]),
        ("<xmlns:r/>", &[1], &["xmlns:r{}"]),
        // XML_NS_ERR_QNAME: a Name that is no QName stays unprefixed.
        ("<a:b:c/>", &[4], &["a:b:c{}"]),
        ("<r a:='1'/>", &[5], &["r{}", "@a:{}"]),
        // XML_NS_ERR_XML_NAMESPACE and empty bindings: the declaration is dropped.
        ("<r xmlns:p=''/>", &[3], &["r{}"]),
        ("<r xmlns:p=''><p:c/></r>", &[3, 15], &["r{}", "p:c{}"]),
        ("<r xmlns:xml='urn:x'/>", &[3], &["r{}"]),
        ("<r xmlns:xmlns='urn:x'/>", &[3], &["r{}"]),
        ("<r xmlns:p='http://www.w3.org/XML/1998/namespace'/>", &[3], &["r{}"]),
        ("<r xmlns='http://www.w3.org/XML/1998/namespace'/>", &[3], &["r{}"]),
        ("<r xmlns:p='http://www.w3.org/2000/xmlns/'/>", &[3], &["r{}"]),
        // XML_NS_ERR_ATTRIBUTE_REDEFINED: both attributes are kept.
        ("<r xmlns:p='u' xmlns:q='u' p:a='1' q:a='2'/>", &[0],
         &["r{}", "@xmlns:p{http://www.w3.org/2000/xmlns/}", "@xmlns:q{http://www.w3.org/2000/xmlns/}", "@p:a{u}", "@q:a{u}"]),
        // XML_NS_ERR_COLON in PI targets and entity and notation names.
        ("<?a:b x?><r/>", &[3], &["r{}"]),
        ("<!DOCTYPE r [<!ENTITY a:b 'x'><!NOTATION n:o SYSTEM 'y'>]><r>&a:b;</r>", &[13, 30], &["r{}"]),
        // In entity content.
        ("<!DOCTYPE r [<!ENTITY e '<p:c/>'>]><r>&e;</r>", &[38], &["r{}", "p:c{}"]),
        // A defaulted declaration libxml2 binds unchecked is dropped silently.
        ("<!DOCTYPE r [<!ATTLIST r xmlns:p CDATA ''>]><r/>", &[], &["r{}"]),
    ];
    for &(input, offsets, names) in cases {
        let expected: Vec<_> = offsets
            .iter()
            .map(|&offset| Error {
                kind: ErrorKind::Namespace,
                offset,
            })
            .collect();
        let names: Names = names.iter().map(|name| name.to_string()).collect();
        for flags in [0, RECOVER] {
            for (surface, parse) in SURFACES {
                let (found, errors) = parse(input.as_bytes(), flags)
                    .unwrap_or_else(|error| panic!("{surface} {flags} rejected {input}: {error}"));
                let context = format!("{surface} flags {flags}: {input}");
                assert_eq!(errors, expected, "{context}");
                assert_eq!(normalized(found), normalized(names.clone()), "{context}");
            }
        }
    }
}

#[test]
fn other_errors_still_stop_a_strict_parse() {
    // (input, strict error, errors kept under RECOVER)
    let cases: &[(&str, ErrorKind, &[ErrorKind])] = &[
        // The same QName twice is a well-formedness error, after the
        // namespace error before it.
        (
            "<r><p:c/><s a='1' a='2'/></r>",
            ErrorKind::DuplicateAttribute,
            &[ErrorKind::Namespace, ErrorKind::DuplicateAttribute],
        ),
        (
            "<r xmlns:p='u' xmlns:p='v'/>",
            ErrorKind::DuplicateAttribute,
            &[ErrorKind::DuplicateAttribute],
        ),
        (
            "<r><p:c/><x></r>",
            ErrorKind::MismatchedEndTag,
            &[
                ErrorKind::Namespace,
                ErrorKind::MismatchedEndTag,
                ErrorKind::UnexpectedEof,
            ],
        ),
    ];
    for &(input, fatal, recovered) in cases {
        for (surface, parse) in SURFACES {
            let strict = parse(input.as_bytes(), 0).map(drop).map_err(|e| e.kind);
            assert_eq!(strict, Err(fatal), "{surface}: {input}");
            let (_, errors) = parse(input.as_bytes(), RECOVER).unwrap();
            let kinds: Vec<_> = errors.iter().map(|error| error.kind).collect();
            assert_eq!(kinds, recovered, "{surface}: {input}");
        }
    }
}

#[test]
fn namespace_errors_are_capped_like_libxml2() {
    // libxml2 stops recording after XML_MAX_ERRORS (100).
    let input = format!("<r>{}</r>", "<p:c/>".repeat(150));
    for (surface, parse) in SURFACES {
        let (names, errors) = parse(input.as_bytes(), 0).unwrap();
        assert_eq!(names.len(), 151, "{surface}");
        assert_eq!(errors.len(), 100, "{surface}");
        assert_eq!(errors[99].offset, 3 + 99 * 6 + 1, "{surface}");
    }
}

#[test]
fn namespace_errors_describe_as_libxml2() {
    // (input, libxml2 code, message)
    let cases: &[(&str, u16, &str)] = &[
        (
            "<r><p:c/></r>",
            201,
            "Namespace prefix p on c is not defined",
        ),
        (
            "<r p:a='1'/>",
            201,
            "Namespace prefix p for a on r is not defined",
        ),
        ("<a:b:c/>", 202, "Failed to parse QName 'a:b:c'"),
        (
            "<r xmlns:p=''/>",
            200,
            "xmlns:p: Empty XML namespace is not allowed",
        ),
        (
            "<r xmlns:p='u' xmlns:q='u' p:a='1' q:a='2'/>",
            203,
            "Namespaced Attribute a in 'u' redefined",
        ),
        (
            "<?a:b x?><r/>",
            205,
            "colons are forbidden from PI names 'a:b'",
        ),
        (
            "<p:r xmlns:xmlns='u'/>",
            200,
            "redefinition of the xmlns prefix is forbidden",
        ),
    ];
    for &(input, code, message) in cases {
        let document = OwnedDocument::parse(input).unwrap();
        let diagnostic = document.errors()[0].describe(&LineIndex::new(input.as_bytes()));
        assert_eq!(
            (diagnostic.code, &*diagnostic.message),
            (code, message),
            "{input}"
        );
    }
    // The element's own error follows its attributes', as in libxml2.
    let input = b"<p:r xmlns:xmlns='u'/>";
    let document = OwnedDocument::parse_bytes(input).unwrap();
    let messages: Vec<_> = LineIndex::new(input)
        .describe_all(document.errors())
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect();
    assert_eq!(
        messages,
        [
            "redefinition of the xmlns prefix is forbidden",
            "Namespace prefix p on r is not defined"
        ]
    );
}

#[test]
fn content_parsed_in_context_keeps_namespace_errors() {
    let mut document = OwnedDocument::parse("<r xmlns:p='urn:p'><s/></r>").unwrap();
    let r = document.root_element().id();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    // (content, errors, the first top-level node's name and namespace)
    type Case<'a> = (&'a [u8], &'a [usize], (&'a str, Option<&'a str>));
    let cases: &[Case<'_>] = &[
        (b"<p:a/>", &[], ("p:a", Some("urn:p"))),
        (b"<q:a/>", &[1], ("q:a", None)),
        (b"<a q:b='1'/>", &[0], ("a", None)),
        (b"<a:b:c/>", &[4], ("a:b:c", None)),
    ];
    for &(content, offsets, (qualified, namespace)) in cases {
        for flags in [0, RECOVER] {
            let fragment = editor
                .parse_content(r, content, ParseOptions::new(flags))
                .unwrap();
            let offsets_found: Vec<_> = fragment.errors.iter().map(|e| e.offset).collect();
            assert_eq!(offsets_found, offsets, "{content:?}");
            assert!(
                fragment
                    .errors
                    .iter()
                    .all(|e| e.kind == ErrorKind::Namespace)
            );
            let node = editor.document().node(fragment.nodes[0]).unwrap();
            let name = node.name().unwrap();
            assert_eq!((name.qualified(), name.namespace()), (qualified, namespace));
            // A node left unbound is kept as libxml2 links it: its prefix
            // is not declared.
            editor.append_child(r, fragment.nodes[0]).unwrap();
        }
    }
    let index = LineIndex::content(b"<q:a/>");
    let error = Error {
        kind: ErrorKind::Namespace,
        offset: 1,
    };
    assert_eq!(error.describe(&index).code, 201);
    let saved = yaxml::save::to_vec(&document, &Default::default()).unwrap();
    assert!(
        String::from_utf8(saved)
            .unwrap()
            .contains("<q:a/><q:a/><a q:b=\"1\"/>")
    );
}
