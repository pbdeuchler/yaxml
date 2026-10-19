#![forbid(unsafe_code)]
//! `ParseOptions`: libxml2's `xmlParserOption` flags on every parsing surface,
//! and the entity references kept without `XML_PARSE_NOENT`.

use std::io::BufRead;
use std::ops::ControlFlow;

use yaxml::options::*;
use yaxml::xpath::{Context, Value, XPath};
use yaxml::{
    BufferedReader, Document, ErrorKind, Event, NodeKind, NodeType, OwnedDocument, OwnedNodeKind,
    ParseOptions, ParserProperty, PushParser, ReadError, Reader, Request, TextReader, Writer, c14n,
    save,
};

const ENTITIES: &str = "<!DOCTYPE r [<!ENTITY e '<b>x</b>y'><!ENTITY o 'o&e;'>]><r>a&e;&o;z</r>";

fn options(flags: u32) -> ParseOptions {
    ParseOptions::new(flags)
}

fn kind(result: Result<impl Sized, yaxml::Error>) -> Option<ErrorKind> {
    result.err().map(|error| error.kind)
}

/// Every surface's verdict on `flags`: reader, push, buffered, text reader,
/// borrowed DOM, owned DOM.
fn verdicts(flags: u32) -> [Option<ErrorKind>; 6] {
    let input = b"<r/>".as_slice();
    [
        kind(Reader::with_options(input, options(flags))),
        kind(PushParser::with_options(options(flags))),
        kind(BufferedReader::with_options(input, options(flags))),
        kind(TextReader::with_options(input, options(flags))),
        kind(Document::parse_with(input, options(flags))),
        kind(OwnedDocument::parse_with(input, options(flags))),
    ]
}

#[test]
fn surfaces_accept_exactly_the_flags_they_implement() {
    let unsupported = Some(ErrorKind::UnsupportedOption);
    let accepted = NOENT
        | DTDLOAD
        | DTDATTR
        | NOERROR
        | NOWARNING
        | PEDANTIC
        | SAX1
        | NONET
        | NODICT
        | NSCLEAN
        | NOCDATA
        | NOXINCNODE
        | COMPACT
        | NOBASEFIX
        | HUGE
        | IGNORE_ENC
        | BIG_LINES
        | NO_XXE
        | NO_SYS_CATALOG
        | SKIP_IDS;
    assert_eq!(verdicts(accepted), [None; 6]);
    for flag in [OLD10, OLDSAX, UNZIP, CATALOG_PI, 1 << 28, 1 << 31] {
        assert_eq!(verdicts(flag), [unsupported; 6], "{flag:#x}");
    }
    // Only trees validate after parsing, drop blanks and recover.
    for flag in [DTDVALID, NOBLANKS, RECOVER] {
        let streaming = [unsupported, unsupported, unsupported, unsupported];
        assert_eq!(verdicts(flag)[..4], streaming, "{flag:#x}");
        assert_eq!(verdicts(flag)[4..], [None, None], "{flag:#x}");
    }
    // XInclude results are owned documents.
    assert_eq!(
        verdicts(XINCLUDE),
        [
            unsupported,
            unsupported,
            unsupported,
            unsupported,
            unsupported,
            None
        ]
    );
}

#[test]
fn constructors_without_options_use_libxml2_defaults() {
    let input = ENTITIES.as_bytes();
    let reference = |document: &Document<'_>| document.root_element().first_child().unwrap().id();
    let plain = Document::parse_bytes(input).unwrap();
    let kept = Document::parse_with(input, ParseOptions::default()).unwrap();
    assert_eq!(reference(&plain), reference(&kept));
    assert!(plain.nodes().any(|node| node.entity_name() == Some("e")));
}

/// The borrowed DOM as `kind:payload` lines, entity references as `&name;[`…`]`.
fn outline(document: &Document<'_>) -> String {
    fn walk(node: yaxml::Node<'_, '_>, out: &mut String) {
        for child in node.children() {
            match child.kind() {
                NodeKind::Element(element) => {
                    out.push_str(&format!("<{}>", element.name.qualified()));
                    walk(child, out);
                    out.push_str(&format!("</{}>", element.name.qualified()));
                }
                NodeKind::EntityRef(name) => {
                    out.push_str(&format!("&{name};["));
                    walk(child, out);
                    out.push(']');
                }
                NodeKind::Text(text) => out.push_str(&format!("'{text}'")),
                _ => {}
            }
        }
    }
    let mut out = String::new();
    walk(document.root_element(), &mut out);
    out
}

#[test]
fn references_are_kept_without_noent_and_substituted_with_it() {
    let input = ENTITIES.as_bytes();
    let kept = Document::parse_bytes(input).unwrap();
    assert_eq!(
        outline(&kept),
        "'a'&e;[<b>'x'</b>'y']&o;['o'&e;[<b>'x'</b>'y']]'z'"
    );
    let substituted = Document::parse_with(input, options(NOENT)).unwrap();
    assert_eq!(outline(&substituted), "'a'<b>'x'</b>'yo'<b>'x'</b>'yz'");
    // xmlNodeGetContent sees through references: string values agree.
    let text = |document: &Document<'_>| document.root().text_chunks().collect::<String>();
    assert_eq!(text(&kept), "axyoxyz");
    assert_eq!(text(&kept), text(&substituted));
    let reference = kept
        .root_element()
        .first_child()
        .unwrap()
        .next_sibling()
        .unwrap();
    assert_eq!(reference.entity_name(), Some("e"));
    assert_eq!(reference.text(), None);
    assert_eq!(reference.name(), None);
    assert_eq!(reference.range().map(|range| &ENTITIES[range]), Some("&e;"));
    assert_eq!(reference.text_chunks().collect::<String>(), "xy");
    // The owned DOM keeps the same structure.
    let owned = OwnedDocument::parse_bytes(input).unwrap();
    let kinds: Vec<_> = owned.root_element().children().map(|n| n.kind()).collect();
    assert_eq!(
        kinds,
        [
            OwnedNodeKind::Text,
            OwnedNodeKind::EntityRef,
            OwnedNodeKind::EntityRef,
            OwnedNodeKind::Text
        ]
    );
    let nested = owned.root_element().children().nth(2).unwrap();
    assert_eq!(nested.entity_name(), Some("o"));
    assert_eq!(nested.text_chunks().collect::<String>(), "oxy");
    assert_eq!(
        owned.root_element().first_child().unwrap().entity_name(),
        None
    );
}

#[test]
fn save_writes_references_back_and_the_writer_expands_them() {
    let input = ENTITIES.as_bytes();
    let tail = "<r>a&e;&o;z</r>\n";
    for saved in [
        save::to_vec(&Document::parse_bytes(input).unwrap(), &Default::default()).unwrap(),
        save::to_vec(
            &OwnedDocument::parse_bytes(input).unwrap(),
            &Default::default(),
        )
        .unwrap(),
    ] {
        assert!(String::from_utf8(saved).unwrap().ends_with(tail));
    }
    // Formatting stops at an element with reference children, as for text.
    let input = b"<!DOCTYPE r [<!ENTITY e 'x'>]><r><s>&e;</s></r>";
    let options = save::Options::new(save::FORMAT);
    let saved = save::to_vec(&Document::parse_bytes(input).unwrap(), &options).unwrap();
    assert!(
        String::from_utf8(saved)
            .unwrap()
            .ends_with("<r>\n  <s>&e;</s>\n</r>\n")
    );
    // The writer writes the replacement content.
    let mut writer = Writer::new(Vec::new());
    writer
        .write_document(&Document::parse_bytes(ENTITIES.as_bytes()).unwrap())
        .unwrap();
    let written = String::from_utf8(writer.into_inner()).unwrap();
    assert!(
        written.ends_with("<r>a<b>x</b>yo<b>x</b>yz</r>"),
        "{written}"
    );
}

#[test]
fn unloaded_and_undeclared_references_are_empty_nodes() {
    // An external entity is not loaded without NOENT; an undeclared one is
    // tolerated once an external subset may declare it.
    let input = b"<!DOCTYPE r SYSTEM 'r.dtd' [<!ENTITY x SYSTEM 'x.xml'>]><r>&x;&u;</r>";
    let resolver =
        |request: Request<'_>| Ok(Some(format!("<!-- {} -->", request.uri).into_bytes()));
    let kept = OwnedDocument::parse_with(
        input,
        options(DTDLOAD).with_resolver("d.xml", Box::new(resolver)),
    )
    .unwrap();
    let names: Vec<_> = kept
        .root_element()
        .children()
        .map(|node| (node.entity_name(), node.first_child().is_some()))
        .collect();
    assert_eq!(names, [(Some("x"), false), (Some("u"), false)]);
    let saved = save::to_vec(&kept, &Default::default()).unwrap();
    assert!(
        String::from_utf8(saved)
            .unwrap()
            .ends_with("<r>&x;&u;</r>\n")
    );
    // Substitution loads `x` and drops `u`, as libxml2 does.
    let substituted = OwnedDocument::parse_with(
        input,
        options(DTDLOAD | NOENT).with_resolver("d.xml", Box::new(resolver)),
    )
    .unwrap();
    let kinds: Vec<_> = substituted
        .root_element()
        .children()
        .map(|n| n.kind())
        .collect();
    assert_eq!(kinds, [OwnedNodeKind::Comment]);
}

#[test]
fn streams_report_reference_boundaries_identically() {
    let input = ENTITIES.as_bytes();
    let render = |event: &Event<'_>| match event {
        Event::EntityStart(name) => Some(format!("&{name};")),
        Event::EntityEnd => Some("end".to_owned()),
        Event::Text(text) => Some(format!("'{text}'")),
        Event::Start(element) => Some(format!("<{}>", element.name.qualified())),
        _ => None,
    };
    let mut reader = Reader::new(input).unwrap();
    let mut expected = Vec::new();
    while let Some(event) = reader.read_event().unwrap() {
        expected.extend(render(&event));
    }
    assert_eq!(
        expected.join(" "),
        "<r> 'a' &e; <b> 'x' 'y' end &o; 'o' &e; <b> 'x' 'y' end end 'z'"
    );
    for size in [1, 2, 7, input.len()] {
        let mut parser = PushParser::new();
        let mut events = Vec::new();
        for chunk in input.chunks(size) {
            parser
                .feed(chunk, |event, _| {
                    events.extend(render(&event));
                    ControlFlow::Continue(())
                })
                .unwrap();
        }
        parser
            .finish(|event, _| {
                events.extend(render(&event));
                ControlFlow::Continue(())
            })
            .unwrap();
        assert_eq!(events, expected, "push chunks of {size}");
        let mut buffered = BufferedReader::new(std::io::BufReader::with_capacity(size, input));
        let mut events = Vec::new();
        while let Some((event, _)) = buffered.read_event().unwrap() {
            events.extend(render(&event));
        }
        assert_eq!(events, expected, "buffered capacity {size}");
    }
    // NOENT streams carry no boundaries.
    let mut reader = Reader::with_options(input, options(NOENT)).unwrap();
    while let Some(event) = reader.read_event().unwrap() {
        assert!(!matches!(event, Event::EntityStart(_) | Event::EntityEnd));
    }
    // Copying reader events to a writer writes the replacement text.
    let mut reader = Reader::new(b"<!DOCTYPE r [<!ENTITY e 'x'>]><r>&e;</r>").unwrap();
    let mut writer = Writer::new(Vec::new());
    while let Some(event) = reader.read_event().unwrap() {
        writer.write_event(&event).unwrap();
    }
    writer.finish().unwrap();
    assert!(writer.into_inner().ends_with(b"<r>x</r>"));
}

fn number(document: &Document<'_>, expression: &str) -> f64 {
    let value = XPath::compile(expression)
        .unwrap()
        .evaluate(document, document.root().id(), &Context::new())
        .unwrap();
    match value {
        Value::Number(number) => number,
        other => panic!("{other:?}"),
    }
}

#[test]
fn xpath_and_c14n_treat_references_as_libxml2_does() {
    let kept = Document::parse_bytes(ENTITIES.as_bytes()).unwrap();
    let substituted = Document::parse_with(ENTITIES.as_bytes(), options(NOENT)).unwrap();
    // Reference nodes and their content are invisible to node tests.
    assert_eq!(number(&kept, "count(//b)"), 0.0);
    assert_eq!(number(&kept, "count(/r/node())"), 2.0);
    assert_eq!(number(&kept, "count(//text())"), 2.0);
    assert_eq!(number(&kept, "string-length(/r)"), 7.0);
    assert_eq!(number(&substituted, "count(//b)"), 2.0);
    assert_eq!(number(&substituted, "string-length(/r)"), 7.0);
    let options = c14n::Options::new(c14n::Mode::C14n10);
    assert!(matches!(
        c14n::to_string(&kept, &options),
        Err(c14n::Error::EntityReference(name)) if name == "e"
    ));
    assert_eq!(
        c14n::to_string(&substituted, &options).unwrap(),
        "<r>a<b>x</b>yo<b>x</b>yz</r>"
    );
}

#[test]
fn dtdvalid_validates_through_references() {
    let valid =
        "<!DOCTYPE r [<!ELEMENT r (b)*><!ELEMENT b (#PCDATA)><!ENTITY e '<b>x</b>'>]><r>&e;&e;</r>";
    let invalid =
        "<!DOCTYPE r [<!ELEMENT r (b)><!ELEMENT b (#PCDATA)><!ENTITY e '<b>x</b>'>]><r>&e;&e;</r>";
    for (input, ok) in [(valid, true), (invalid, false)] {
        let document = Document::parse_with(input.as_bytes(), options(DTDVALID)).unwrap();
        assert_eq!(yaxml::valid::is_valid(document.validity()), ok, "{input}");
        assert_eq!(document.validity().len(), document.validate().len());
        let owned = OwnedDocument::parse_with(input.as_bytes(), options(DTDVALID)).unwrap();
        assert_eq!(yaxml::valid::is_valid(owned.validity()), ok, "{input}");
    }
    // Without the flag nothing is recorded.
    let document = Document::parse_bytes(invalid.as_bytes()).unwrap();
    assert!(document.validity().is_empty());
    assert!(!yaxml::valid::is_valid(&document.validate()));
}

#[test]
fn dtdattr_adds_defaults_and_namespace_defaults_always_apply() {
    let input = b"<!DOCTYPE r [<!ATTLIST r a CDATA 'd' xmlns:p CDATA 'urn:p'>]><r/>";
    let names = |document: &Document<'_>| -> Vec<String> {
        document
            .root_element()
            .attributes()
            .iter()
            .map(|attribute| format!("{}={}", attribute.name.qualified(), attribute.value))
            .collect()
    };
    let plain = Document::parse_bytes(input).unwrap();
    assert_eq!(names(&plain), ["xmlns:p=urn:p"]);
    let complete = Document::parse_with(input, options(DTDATTR)).unwrap();
    assert_eq!(names(&complete), ["a=d", "xmlns:p=urn:p"]);
}

#[test]
fn nocdata_and_nsclean_rewrite_tokens_on_every_surface() {
    let input = b"<r xmlns:p='u'><p:i xmlns:p='u' xmlns:q='v'>a<![CDATA[<b>]]>c</p:i></r>";
    let summarize = |document: &Document<'_>| -> (Vec<String>, usize) {
        let item = document.root_element().first_child().unwrap();
        let attributes = item
            .attributes()
            .iter()
            .map(|a| a.name.qualified().to_owned());
        (attributes.collect(), item.children().count())
    };
    let plain = Document::parse_bytes(input).unwrap();
    assert_eq!(
        summarize(&plain),
        (vec!["xmlns:p".into(), "xmlns:q".into()], 3)
    );
    let cleaned = Document::parse_with(input, options(NSCLEAN | NOCDATA)).unwrap();
    assert_eq!(summarize(&cleaned), (vec!["xmlns:q".into()], 1));
    let text = cleaned
        .root_element()
        .first_child()
        .unwrap()
        .first_child()
        .unwrap();
    assert_eq!(text.text(), Some("a<b>c"));
    // The DTD path agrees, and so do the streams.
    let dtd = [b"<!DOCTYPE r>".as_slice(), input].concat();
    let cleaned = Document::parse_with(&dtd, options(NSCLEAN | NOCDATA)).unwrap();
    assert_eq!(summarize(&cleaned), (vec!["xmlns:q".into()], 1));
    let mut push = PushParser::with_options(options(NOCDATA)).unwrap();
    let mut cdata = 0;
    push.feed(input, |event, _| {
        cdata += usize::from(matches!(event, Event::Cdata(_)));
        ControlFlow::Continue(())
    })
    .unwrap();
    assert_eq!(cdata, 0);
}

#[test]
fn loading_follows_dtdload_noent_and_no_xxe() {
    let resolver = || {
        Box::new(|request: Request<'_>| {
            Ok(Some(match request.uri {
                "r.dtd" => b"<!ATTLIST r a CDATA 'd'><!ENTITY x SYSTEM 'x.xml'>".to_vec(),
                _ => b"loaded".to_vec(),
            }))
        })
    };
    let input = b"<!DOCTYPE r SYSTEM 'r.dtd'><r>[&x;]</r>";
    let saved = |flags: u32| {
        let options = options(flags).with_resolver("doc.xml", resolver());
        let document = OwnedDocument::parse_with(input, options).unwrap();
        let saved = save::to_vec(&document, &Default::default()).unwrap();
        let saved = String::from_utf8(saved).unwrap();
        saved.lines().last().unwrap().to_owned()
    };
    // Nothing loads by default; the reference to the unread entity is kept.
    assert_eq!(saved(0), "<r>[&x;]</r>");
    assert_eq!(saved(DTDLOAD), "<r>[&x;]</r>");
    assert_eq!(saved(DTDATTR), "<r a=\"d\">[&x;]</r>");
    assert_eq!(saved(DTDLOAD | NOENT), "<r>[loaded]</r>");
    assert_eq!(saved(DTDVALID | DTDATTR), "<r a=\"d\">[&x;]</r>");
    assert_eq!(saved(DTDLOAD | NOENT | NO_XXE), "<r>[]</r>");
    // Complete DTD processing loads everything.
    let document =
        OwnedDocument::parse_with(input, loading(Default::default(), "doc.xml", resolver()))
            .unwrap();
    let text = document.root_element().text_chunks().collect::<String>();
    assert_eq!(text, "[loaded]");
}

#[test]
fn huge_skip_ids_and_ignore_enc_change_their_limits_and_lookups() {
    let deep = format!("{}{}", "<a>".repeat(300), "</a>".repeat(300));
    assert_eq!(
        kind(Document::parse_with(
            deep.as_bytes(),
            ParseOptions::default()
        )),
        Some(ErrorKind::LimitExceeded(yaxml::Limit::Depth))
    );
    assert!(Document::parse_with(deep.as_bytes(), options(HUGE)).is_ok());
    let ids = b"<r xml:id='a'/>";
    assert!(
        Document::parse_bytes(ids)
            .unwrap()
            .element_by_id("a")
            .is_some()
    );
    let skipped = Document::parse_with(ids, options(SKIP_IDS)).unwrap();
    assert!(skipped.element_by_id("a").is_none());
    let owned = OwnedDocument::parse_with(ids, options(SKIP_IDS)).unwrap();
    assert!(owned.element_by_id("a").is_none());
    // UTF-8 bytes declared as Latin-1 decode as UTF-8 under IGNORE_ENC.
    let latin = "<?xml version='1.0' encoding='ISO-8859-1'?><r>é</r>".as_bytes();
    let decoded = OwnedDocument::parse_bytes(latin).unwrap();
    assert_eq!(
        decoded.root_element().text_chunks().collect::<String>(),
        "Ã©"
    );
    let ignored = OwnedDocument::parse_with(latin, options(IGNORE_ENC)).unwrap();
    assert_eq!(
        ignored.root_element().text_chunks().collect::<String>(),
        "é"
    );
    assert_eq!(
        kind(Document::parse_bytes(latin)),
        Some(ErrorKind::UnsupportedEncoding)
    );
    assert!(Document::parse_with(latin, options(IGNORE_ENC)).is_ok());
}

#[test]
fn noblanks_drops_ignorable_whitespace_from_both_trees() {
    let input = b"<r>\n <a> </a>\n <b xml:space='preserve'> <c/> </b>\n</r>";
    let expected = "<r><a> </a><b xml:space=\"preserve\"> <c/> </b></r>\n";
    let tail = |saved: Vec<u8>| String::from_utf8(saved).unwrap();
    let document = Document::parse_with(input, options(NOBLANKS)).unwrap();
    assert!(tail(save::to_vec(&document, &Default::default()).unwrap()).ends_with(expected));
    assert_eq!(document.nodes().count(), 8);
    let owned = OwnedDocument::parse_with(input, options(NOBLANKS)).unwrap();
    assert!(tail(save::to_vec(&owned, &Default::default()).unwrap()).ends_with(expected));
    // Before a reference, whitespace is never blank (libxml2 sees its `&`).
    let input = b"<!DOCTYPE r [<!ENTITY e 'x'>]><r>\n <a/>\n &e;</r>";
    let document = Document::parse_with(input, options(NOBLANKS)).unwrap();
    let saved = tail(save::to_vec(&document, &Default::default()).unwrap());
    assert!(saved.ends_with("<r><a/>\n &e;</r>\n"), "{saved}");
}

#[test]
fn xinclude_runs_through_the_same_resolver() {
    let input = b"<r xmlns:xi='http://www.w3.org/2001/XInclude'><xi:include href='a.xml'/></r>";
    let resolver = || Box::new(|_: Request<'_>| Ok(Some(b"<a/>".to_vec())));
    let options = |flags| options(flags).with_resolver("d/doc.xml", resolver());
    let document = OwnedDocument::parse_with(input, options(XINCLUDE)).unwrap();
    let child = document.root_element().first_child().unwrap();
    assert_eq!(child.name().unwrap().local(), "a");
    assert_eq!(child.attributes().count(), 0, "same directory: no xml:base");
    let document = OwnedDocument::parse_with(input, options(XINCLUDE | NOXINCNODE)).unwrap();
    assert_eq!(document.root_element().children().count(), 1);
    // A failed inclusion without fallback is an XInclude error.
    let none =
        ParseOptions::new(XINCLUDE).with_resolver("doc.xml", Box::new(|_: Request<'_>| Ok(None)));
    assert_eq!(
        kind(OwnedDocument::parse_with(input, none)),
        Some(ErrorKind::XInclude)
    );
    assert_eq!(
        kind(OwnedDocument::parse_with(
            input,
            ParseOptions::new(XINCLUDE)
        )),
        Some(ErrorKind::XInclude)
    );
}

fn lines<R: BufRead>(mut reader: TextReader<R>) -> Result<Vec<String>, ReadError> {
    let mut lines = Vec::new();
    while reader.read()? {
        lines.push(format!(
            "{} {} {}",
            reader.depth(),
            reader.node_type() as i32,
            reader.name().unwrap_or_default()
        ));
    }
    Ok(lines)
}

#[test]
fn text_reader_reports_entity_reference_nodes() {
    let input = ENTITIES.as_bytes();
    let kept = lines(TextReader::new(input)).unwrap();
    assert_eq!(
        kept[1..],
        [
            "0 1 r",
            "1 3 #text",
            "1 5 e",
            "1 5 o",
            "1 3 #text",
            "0 15 r"
        ]
    );
    let substituted = TextReader::with_options(input, options(NOENT)).unwrap();
    // DOCTYPE, r, a, b, x, /b, yo, b, x, /b, yz, /r: replacement text merges.
    assert_eq!(lines(substituted).unwrap().len(), 12);
    // Outer XML keeps the reference; expand substitutes it.
    let mut reader = TextReader::new(input);
    reader.read().unwrap();
    reader.read().unwrap();
    assert_eq!(
        reader.read_outer_xml().unwrap().as_deref(),
        Some("<r>a&e;&o;z</r>")
    );
    let expanded = reader.expand().unwrap().unwrap();
    let text = expanded.root_element().text_chunks().collect::<String>();
    assert_eq!(text, "axyoxyz");
    // Properties report the options.
    let reader = TextReader::with_options(input, options(NOENT | DTDATTR)).unwrap();
    assert!(reader.parser_property(ParserProperty::SubstituteEntities));
    assert!(reader.parser_property(ParserProperty::DefaultAttributes));
    assert!(!reader.parser_property(ParserProperty::LoadDtd));
    let reader = TextReader::new(input);
    assert!(!reader.parser_property(ParserProperty::SubstituteEntities));
    assert_eq!(NodeType::EntityReference as i32, 5);
}

#[test]
fn schema_validators_see_through_references() {
    let xml = "<!DOCTYPE r [<!ENTITY e '<i>1</i>'>]><r>&e;&e;</r>";
    let bad = "<!DOCTYPE r [<!ENTITY e '<j>1</j>'>]><r>&e;</r>";
    let rng = br#"<element name="r" xmlns="http://relaxng.org/ns/structure/1.0">
      <oneOrMore><element name="i"><text/></element></oneOrMore></element>"#;
    let none = |_: Request<'_>| Ok(None);
    let relaxng =
        yaxml::relaxng::Schema::parse(rng, "r.rng", Box::new(none), &Default::default()).unwrap();
    let xsd = yaxml::xsd::Schema::parse(
        br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
          <xs:element name="r"><xs:complexType><xs:sequence>
            <xs:element name="i" type="xs:int" maxOccurs="unbounded"/>
          </xs:sequence></xs:complexType></xs:element></xs:schema>"#,
        "r.xsd",
    )
    .unwrap();
    for (input, valid) in [(xml, true), (bad, false)] {
        let document = Document::parse(input).unwrap();
        assert!(document.nodes().any(|node| node.entity_name().is_some()));
        assert_eq!(relaxng.validate(&document).is_empty(), valid, "{input}");
        assert_eq!(
            yaxml::xsd::is_valid(&xsd.validate(&document)),
            valid,
            "{input}"
        );
    }
}

/// Complete DTD processing (`NOENT | DTDATTR | DTDLOAD`) through `resolver`.
fn loading(
    limits: yaxml::Limits,
    base: &str,
    resolver: Box<dyn yaxml::Resolver>,
) -> yaxml::ParseOptions {
    let flags = yaxml::options::LEGACY | yaxml::options::DTDLOAD;
    yaxml::ParseOptions::new(flags)
        .with_limits(limits)
        .with_resolver(base, resolver)
}
