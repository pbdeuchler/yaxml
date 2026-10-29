#![forbid(unsafe_code)]
//! Namespace errors (libxml2's `xmlNsErr`: unbound prefixes, malformed
//! QNames, unbindable declarations, duplicate expanded names, colons in PI,
//! entity and notation names) never stop a parse: every surface keeps them
//! in `errors()`, with or without `RECOVER`, and keeps the names in no
//! namespace. Parser and namespace warnings (`ErrorKind::Warning`) are kept
//! the same way. Expectations were checked against `xmllint` 2.15.4.

use std::io::BufReader;
use std::ops::ControlFlow;

use yaxml::options::{DTDATTR, DTDVALID, NOENT, RECOVER};
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

#[test]
fn warnings_are_kept_on_every_surface_and_describe_as_libxml2() {
    use Level::{Error as E, Warning as W};
    use yaxml::Warning;
    use yaxml::diagnostic::{Domain, Level};
    // (input, warning, libxml2 domain, code, level, column, message), from
    // the xmllint 2.15.4 error helper with NOENT.
    #[rustfmt::skip]
    let cases: &[(&str, Warning, Domain, u16, Level, usize, &str)] = &[
        ("<?xml version='1.1'?><r/>", Warning::UnknownVersion, Domain::Parser, 97, W, 20,
         "Unsupported version '1.1'"),
        ("<r xml:space='x'/>", Warning::SpaceValue, Domain::Parser, 102, W, 17,
         "Invalid value \"x\" for xml:space : \"default\" or \"preserve\" expected"),
        ("<r xmlns:p='a b'/>", Warning::NamespaceUri, Domain::Namespace, 99, E, 17,
         "xmlns:p: 'a b' is not a valid URI"),
        ("<r xmlns='rel'/>", Warning::RelativeNamespaceUri, Domain::Namespace, 100, W, 15,
         "xmlns: URI rel is not absolute"),
        ("<?xml-foo x?><r/>", Warning::ReservedTarget, Domain::Parser, 64, W, 10,
         "xmlParsePITarget: invalid name prefix 'xml'"),
        ("<!DOCTYPE r SYSTEM 'x.dtd'><r>&u;</r>", Warning::UndeclaredEntity { error: true },
         Domain::Parser, 27, E, 34, "Entity 'u' not defined"),
        ("<!DOCTYPE r [<!ENTITY gt \">x\">]><r/>", Warning::PredefinedEntity, Domain::Parser,
         117, W, 29, "Invalid redeclaration of predefined entity 'gt'"),
        ("<!DOCTYPE r [<!ENTITY amp SYSTEM 'x'>]><r/>", Warning::PredefinedEntity,
         Domain::Parser, 117, W, 37, "Invalid redeclaration of predefined entity 'amp'"),
        ("<!DOCTYPE r [<!ENTITY % e \"<!ENTITY quot 'q'>\">%e;]><r/>",
         Warning::PredefinedEntity, Domain::Parser, 117, W, 50,
         "Invalid redeclaration of predefined entity 'quot'"),
        ("<r xml:id=\"1a\"/>", Warning::XmlIdValue, Domain::Parser, 539, E, 15,
         "xml:id : attribute value 1a is not an NCName"),
        ("<r xml:id=\"1\" b=\"2\"  />", Warning::XmlIdValue, Domain::Parser, 539, E, 22,
         "xml:id : attribute value 1 is not an NCName"),
        ("<r xml:id=\"&#49;\"/>", Warning::XmlIdValue, Domain::Parser, 539, E, 18,
         "xml:id : attribute value 1 is not an NCName"),
        ("<r xml:id=''/>", Warning::XmlIdValue, Domain::Parser, 539, E, 13,
         "xml:id : attribute value  is not an NCName"),
        ("<r xml:id='p:q'>x</r>", Warning::XmlIdValue, Domain::Parser, 539, E, 16,
         "xml:id : attribute value p:q is not an NCName"),
        ("<!DOCTYPE r [<!ENTITY e \"1\">]><r xml:id=\"&e;\"/>", Warning::XmlIdValue,
         Domain::Parser, 539, E, 45, "xml:id : attribute value 1 is not an NCName"),
        ("<!DOCTYPE r [<!ATTLIST r xml:id CDATA #IMPLIED>]><r/>", Warning::XmlIdType,
         Domain::Parser, 540, E, 47, "xml:id : attribute type should be ID"),
        ("<!DOCTYPE r [<!ATTLIST r b CDATA #IMPLIED xml:id NMTOKEN #IMPLIED >]><r/>",
         Warning::XmlIdType, Domain::Parser, 540, E, 67, "xml:id : attribute type should be ID"),
        ("<!DOCTYPE r [<!ENTITY % p \"<!ATTLIST r xml:id CDATA #IMPLIED>\">%p;]><r/>",
         Warning::XmlIdType, Domain::Parser, 540, E, 66, "xml:id : attribute type should be ID"),
    ];
    for &(input, warning, domain, code, level, column, message) in cases {
        let (_, reference) = document(input.as_bytes(), 0).unwrap();
        let kinds: Vec<_> = reference.iter().map(|error| error.kind).collect();
        assert_eq!(kinds, [ErrorKind::Warning(warning)], "{input}");
        for flags in [0, RECOVER] {
            for (surface, parse) in SURFACES {
                let (_, errors) = parse(input.as_bytes(), flags)
                    .unwrap_or_else(|error| panic!("{surface} {flags} rejected {input}: {error}"));
                assert_eq!(errors, reference, "{surface} flags {flags}: {input}");
            }
        }
        let found = reference[0].describe(&LineIndex::new(input.as_bytes()));
        assert_eq!(
            (
                found.domain,
                found.code,
                found.level,
                found.line,
                found.column
            ),
            (domain, code, level, 1, column),
            "{input}"
        );
        assert_eq!(found.message, message, "{input}");
    }
    // Without NOENT (or DTD loading) an undeclared entity is a warning.
    let input = b"<!DOCTYPE r SYSTEM 'x.dtd'><r>&u;</r>";
    let document = OwnedDocument::parse_bytes(input).unwrap();
    let kinds: Vec<_> = document.errors().iter().map(|error| error.kind).collect();
    let undeclared = Warning::UndeclaredEntity { error: false };
    assert_eq!(kinds, [ErrorKind::Warning(undeclared)]);
    assert_eq!(
        document.errors()[0].describe(&LineIndex::new(input)).level,
        W
    );
    // An attribute value without references reports none of the previous value's.
    let input = b"<!DOCTYPE r [%p;]><r a='&e;' b='x'/>";
    for flags in [0, RECOVER] {
        for (surface, parse) in SURFACES {
            let (_, errors) = parse(input, flags).unwrap();
            let offsets: Vec<_> = errors.iter().map(|error| error.offset).collect();
            assert_eq!(offsets, [13, 24], "{surface} flags {flags}");
        }
    }
    // Values libxml2 accepts or never checks: blanks around an NCName,
    // entity content,
    // and a character reference that spells a valid value (a lent value).
    #[rustfmt::skip]
    let silent: &[&str] = &[
        "<r xml:id=' a '/>", "<r xml:id='\u{e9}1'/>", "<r xml:id='_1\u{b7}'/>",
        "<!DOCTYPE r [<!ENTITY e \"<a xml:id='1'/>\">]><r>&e;</r>",
        "<r xml:space='&#100;efault' xml:id='&#97;'/>",
    ];
    for &input in silent {
        for flags in [0, RECOVER] {
            for (surface, parse) in SURFACES {
                let (_, errors) = parse(input.as_bytes(), flags)
                    .unwrap_or_else(|error| panic!("{surface} {flags} rejected {input}: {error}"));
                assert_eq!(errors, [], "{surface} flags {flags}: {input}");
            }
        }
    }
    // Nor is a value that keeps its references (no NOENT), or one a DOCTYPE
    // validates.
    let input = b"<!DOCTYPE r [<!ENTITY e '1'>]><r xml:id='&e;'/>";
    assert_eq!(OwnedDocument::parse_bytes(input).unwrap().errors(), []);
    let input = b"<!DOCTYPE r [<!ATTLIST r xml:id ID #IMPLIED>]><r xml:id='1'/>";
    let document = OwnedDocument::parse_with(input, options(DTDVALID)).unwrap();
    assert_eq!(document.errors(), []);
    // A default is checked at its tag, normalized as its type is.
    let input = b"<!DOCTYPE r [<!ATTLIST r xml:id ID ' 9 '>]><r/>";
    let document = OwnedDocument::parse_with(input, options(DTDATTR)).unwrap();
    let [error] = document.errors() else {
        panic!("{:?}", document.errors())
    };
    let found = error.describe(&LineIndex::new(input));
    let message = "xml:id : attribute value 9 is not an NCName";
    assert_eq!(
        (found.code, found.column, &*found.message),
        (539, 46, message)
    );
    // A strict well-formedness error still fails after a warning.
    let input = b"<?xml version='1.1'?><r></s>";
    for (surface, parse) in SURFACES {
        let strict = parse(input, 0).map(drop).map_err(|error| error.kind);
        assert_eq!(strict, Err(ErrorKind::MismatchedEndTag), "{surface}");
    }
}

#[test]
fn flagged_warnings_are_kept_on_every_surface_and_describe_as_libxml2() {
    use yaxml::Warning;
    use yaxml::options::PEDANTIC;
    // (input, flag, warning, code, column, message), from the libxml2 2.15.4
    // error helper; without the flag every input is silent.
    #[rustfmt::skip]
    let cases: &[(&str, u32, Warning, u16, usize, &str)] = &[
        ("<r xml:lang='en-us-x'/>", PEDANTIC, Warning::LangValue, 98, 22,
         "Malformed value for xml:lang : en-us-x"),
        ("<r xml:lang=\"de-CH-1996\" a=\"1\"/>", PEDANTIC, Warning::LangValue, 98, 25,
         "Malformed value for xml:lang : de-CH-1996"),
        ("<r xmlns:p='rel'/>", PEDANTIC, Warning::RelativeNamespaceUri, 100, 17,
         "xmlns:p: URI rel is not absolute"),
        ("<!DOCTYPE r [<!ENTITY e \"a\"><!ENTITY e \"b\">]><r/>", PEDANTIC,
         Warning::EntityRedefined, 107, 41, "Entity(e) already defined in the internal subset"),
        ("<!DOCTYPE r [<!ENTITY % p \"a\"><!ENTITY % p 'b'>]><r/>", PEDANTIC,
         Warning::EntityRedefined, 107, 45, "Entity(p) already defined in the internal subset"),
        ("<!DOCTYPE r [<!ENTITY e SYSTEM \"x\"><!ENTITY e \"b\" >]><r/>", PEDANTIC,
         Warning::EntityRedefined, 107, 49, "Entity(e) already defined in the internal subset"),
        ("<!DOCTYPE r [<!ENTITY % d \"<!ENTITY e 'b'>\"><!ENTITY e \"a\">%d;]><r/>", PEDANTIC,
         Warning::EntityRedefined, 107, 61, "Entity(e) already defined in the internal subset"),
        // libxml2's column lags one per entity value earlier on the line.
        ("<!DOCTYPE r [<!ENTITY e \"x\"><!ENTITY f \"y\"><!ENTITY e \"b\">]><r/>", PEDANTIC,
         Warning::EntityRedefined, 107, 55, "Entity(e) already defined in the internal subset"),
        ("<!DOCTYPE r [<!ENTITY e \"a\"><!ENTITY % p \"\">%p;<!ENTITY e \"b\">]><r/>", PEDANTIC,
         Warning::EntityRedefined, 107, 59, "Entity(e) already defined in the internal subset"),
        #[cfg(feature = "catalog")]
        ("<?oasis-xml-catalog bogus?><r/>", yaxml::options::CATALOG_PI, Warning::CatalogPi, 93,
         28, "Catalog PI syntax error: bogus"),
        #[cfg(feature = "catalog")]
        ("<?oasis-xml-catalog catalog=\"x\" junk?><r/>", yaxml::options::CATALOG_PI,
         Warning::CatalogPi, 93, 39, "Catalog PI syntax error: catalog=\"x\" junk"),
        #[cfg(feature = "catalog")]
        ("<r><?oasis-xml-catalog catalog=x?></r>", yaxml::options::CATALOG_PI,
         Warning::CatalogPi, 93, 35, "Catalog PI syntax error: catalog=x"),
    ];
    for &(input, flag, warning, code, column, message) in cases {
        let (_, silent) = document(input.as_bytes(), 0).unwrap();
        assert_eq!(silent, [], "{input}");
        let (_, reference) = document(input.as_bytes(), flag).unwrap();
        let kinds: Vec<_> = reference.iter().map(|error| error.kind).collect();
        assert_eq!(kinds, [ErrorKind::Warning(warning)], "{input}");
        for flags in [flag, flag | RECOVER] {
            for (surface, parse) in SURFACES {
                let (_, errors) = parse(input.as_bytes(), flags)
                    .unwrap_or_else(|error| panic!("{surface} {flags} rejected {input}: {error}"));
                assert_eq!(errors, reference, "{surface} flags {flags}: {input}");
            }
        }
        let found = reference[0].describe(&LineIndex::new(input.as_bytes()));
        assert_eq!(
            (found.code, found.line, found.column),
            (code, 1, column),
            "{input}"
        );
        assert_eq!(found.message, message, "{input}");
    }
    // Tags libxml2's xmlCheckLanguageID accepts (checked with xmllint
    // --pedantic), a catalog instruction without `=`, and an entity declared
    // in each subset.
    #[rustfmt::skip]
    let silent: &[(&str, u32)] = &[
        ("<r xml:lang='en'/>", PEDANTIC), ("<r xml:lang='en-US'/>", PEDANTIC),
        ("<r xml:lang='i-klingon'/>", PEDANTIC), ("<r xml:lang='abcd'/>", PEDANTIC),
        ("<r xml:lang='zh-Hant-TW'/>", PEDANTIC), ("<r xml:lang='de-419'/>", PEDANTIC),
        ("<r xmlns:p='http://x'/>", PEDANTIC),
        #[cfg(feature = "catalog")]
        ("<?oasis-xml-catalog catalog x?><r/>", yaxml::options::CATALOG_PI),
    ];
    for &(input, flags) in silent {
        assert_eq!(document(input.as_bytes(), flags).unwrap().1, [], "{input}");
    }
    for tag in [
        "x-foo-bar",
        "abcdefghi",
        "e1",
        "en-",
        "1en",
        "sr-Latn-RS-a-b",
    ] {
        let input = format!("<r xml:lang='{tag}'/>");
        let (_, errors) = document(input.as_bytes(), PEDANTIC).unwrap();
        assert_eq!(errors.len(), 1, "{tag}");
    }
    let resolver = |_: yaxml::Request<'_>| Ok(Some(b"<!ENTITY e 'b'>".to_vec()));
    let input = b"<!DOCTYPE r SYSTEM 'x.dtd' [<!ENTITY e 'a'>]><r/>";
    let options = ParseOptions::new(PEDANTIC | yaxml::options::DTDLOAD)
        .with_resolver("doc.xml", Box::new(resolver));
    assert_eq!(
        OwnedDocument::parse_with(input, options).unwrap().errors(),
        []
    );
}
