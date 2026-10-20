#![forbid(unsafe_code)]
//! Public-API DOCTYPE behavior: every parsing surface agrees on events,
//! errors and limits; the writer and serializer round-trip the declaration.

use std::io::{BufReader, Cursor};
use std::ops::ControlFlow;

use yaxml::{
    BufferedReader, Doctype, Document, Error, ErrorKind, Event, Limit, Limits, NodeKind,
    OwnedDocument, OwnedNode, OwnedNodeKind, ParseOptions, PushParser, ReadError, Reader, Writer,
};

/// A compact, coalesced rendering shared by every surface.
fn label(event: &Event<'_>, out: &mut Vec<String>) {
    let attributes = |element: &yaxml::Element<'_>| {
        element
            .attributes
            .iter()
            .map(|a| format!(" {}={}", a.name.qualified(), a.value))
            .collect::<String>()
    };
    let item = match event {
        Event::Declaration(_) | Event::EntityStart(_) | Event::EntityEnd => return,
        Event::Doctype(d) => doctype(d),
        Event::Start(e) => format!("<{}{}>", e.name.qualified(), attributes(e)),
        Event::Empty(e) => format!("<{}{}/>", e.name.qualified(), attributes(e)),
        Event::End(name) => format!("</{}>", name.qualified()),
        Event::Text(text) => {
            if let Some(last) = out.last_mut().filter(|last| last.starts_with('"')) {
                last.insert_str(last.len() - 1, text);
            } else if !text.is_empty() {
                out.push(format!("\"{text}\""));
            }
            return;
        }
        Event::Cdata(text) => format!("cdata:{text}"),
        Event::Comment(text) => format!("comment:{text}"),
        Event::ProcessingInstruction { target, data } => format!("pi:{target}:{data}"),
    };
    out.push(item);
}

/// These checks pin the substituting behavior: `NOENT | DTDATTR`.
fn legacy(limits: Limits) -> ParseOptions {
    ParseOptions::new(yaxml::options::LEGACY).with_limits(limits)
}

fn doctype(d: &Doctype<'_>) -> String {
    format!(
        "doctype:{}:{:?}:{:?}:{:?}",
        d.name, d.public_id, d.system_id, d.internal_subset
    )
}

fn reader(input: &[u8], limits: Limits) -> Result<Vec<String>, Error> {
    let mut reader = Reader::with_options(input, legacy(limits))?;
    let mut out = Vec::new();
    while let Some(event) = reader.read_event()? {
        label(&event, &mut out);
    }
    Ok(out)
}

fn push(input: &[u8], chunks: &[usize], limits: Limits) -> Result<Vec<String>, Error> {
    let mut parser = PushParser::with_options(legacy(limits))?;
    let mut out = Vec::new();
    let mut rest = input;
    for &size in chunks.iter().chain(std::iter::repeat(&usize::MAX)) {
        if rest.is_empty() {
            break;
        }
        let (chunk, tail) = rest.split_at(size.min(rest.len()));
        rest = tail;
        parser.feed(chunk, |event, _| {
            label(event, &mut out);
            ControlFlow::Continue(())
        })?;
    }
    parser.finish(|event, _| {
        label(event, &mut out);
        ControlFlow::Continue(())
    })?;
    Ok(out)
}

fn buffered(input: &[u8], capacity: usize, limits: Limits) -> Result<Vec<String>, Error> {
    let mut reader = BufferedReader::with_options(
        BufReader::with_capacity(capacity, Cursor::new(input)),
        legacy(limits),
    )?;
    let mut out = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Some((event, _))) => label(&event, &mut out),
            Ok(None) => return Ok(out),
            Err(ReadError::Xml(error)) => return Err(error),
            Err(error) => panic!("unexpected I/O failure: {error}"),
        }
    }
}

fn borrowed_tree(input: &[u8], limits: Limits) -> Result<Vec<String>, Error> {
    let document = Document::parse_with(input, legacy(limits))?;
    let mut out = Vec::new();
    fn walk(node: yaxml::Node<'_, '_>, out: &mut Vec<String>) {
        for child in node.children() {
            match child.kind() {
                NodeKind::Element(element) if child.first_child().is_some() => {
                    label(&Event::Start(clone(element)), out);
                    walk(child, out);
                    out.push(format!("</{}>", element.name.qualified()));
                }
                NodeKind::Element(element) => label(&Event::Empty(clone(element)), out),
                NodeKind::Doctype(d) => label(&Event::Doctype(*d), out),
                NodeKind::Text(t) => label(&Event::Text(t.as_ref().into()), out),
                NodeKind::Cdata(t) => label(&Event::Cdata(t.as_ref().into()), out),
                NodeKind::Comment(t) => label(&Event::Comment(t.as_ref().into()), out),
                NodeKind::ProcessingInstruction { target, data } => {
                    label(
                        &Event::ProcessingInstruction {
                            target: target.as_ref().into(),
                            data: data.as_ref().into(),
                        },
                        out,
                    );
                }
                // Entity references are transparent in this coalesced rendering.
                NodeKind::EntityRef(_) => walk(child, out),
                NodeKind::Document => unreachable!(),
            }
        }
    }
    walk(document.root(), &mut out);
    assert_eq!(
        document.doctype().map(doctype),
        out.iter()
            .find(|item| item.starts_with("doctype:"))
            .cloned()
    );
    Ok(out)
}

fn clone<'a>(element: &'a yaxml::Element<'_>) -> yaxml::Element<'a> {
    yaxml::Element {
        name: borrow(&element.name),
        attributes: element
            .attributes
            .iter()
            .map(|a| yaxml::Attribute {
                name: borrow(&a.name),
                value: a.value.as_ref().into(),
            })
            .collect(),
    }
}

fn owned_tree(input: &[u8], limits: Limits) -> Result<Vec<String>, Error> {
    let document = OwnedDocument::parse_with(input, legacy(limits))?;
    let mut out = Vec::new();
    fn walk(node: OwnedNode<'_>, out: &mut Vec<String>) {
        for child in node.children() {
            let element = || yaxml::Element {
                name: child.name().unwrap(),
                attributes: child.attributes().collect(),
            };
            match child.kind() {
                OwnedNodeKind::Element if child.first_child().is_some() => {
                    label(&Event::Start(element()), out);
                    walk(child, out);
                    out.push(format!("</{}>", child.name().unwrap().qualified()));
                }
                OwnedNodeKind::Element => label(&Event::Empty(element()), out),
                OwnedNodeKind::DocumentType => {
                    label(&Event::Doctype(child.doctype().unwrap()), out)
                }
                OwnedNodeKind::Text => label(&Event::Text(child.text().unwrap().into()), out),
                OwnedNodeKind::Cdata => label(&Event::Cdata(child.text().unwrap().into()), out),
                OwnedNodeKind::Comment => label(&Event::Comment(child.text().unwrap().into()), out),
                OwnedNodeKind::ProcessingInstruction => label(
                    &Event::ProcessingInstruction {
                        target: child.processing_instruction_target().unwrap().into(),
                        data: child.text().unwrap().into(),
                    },
                    out,
                ),
                _ => unreachable!(),
            }
        }
    }
    walk(document.root(), &mut out);
    assert_eq!(
        document.doctype().map(|d| doctype(&d)),
        out.iter()
            .find(|item| item.starts_with("doctype:"))
            .cloned()
    );
    Ok(out)
}

/// Runs every surface, including every two-way push split of short inputs,
/// and requires one shared outcome.
fn all(input: &[u8], limits: Limits) -> Result<Vec<String>, Error> {
    let expected = reader(input, limits);
    let label = String::from_utf8_lossy(input);
    assert_eq!(borrowed_tree(input, limits), expected, "Document {label}");
    assert_eq!(owned_tree(input, limits), expected, "OwnedDocument {label}");
    for capacity in [1, 7, 4096] {
        assert_eq!(
            buffered(input, capacity, limits),
            expected,
            "Buffered {capacity} {label}"
        );
    }
    assert_eq!(
        push(input, &[1; 1 << 16], limits),
        expected,
        "Push bytes {label}"
    );
    let splits = if input.len() <= 200 { input.len() } else { 0 };
    for split in 0..=splits {
        assert_eq!(
            push(input, &[split], limits),
            expected,
            "Push {split} {label}"
        );
    }
    expected
}

fn ok(input: &str) -> Vec<String> {
    all(input.as_bytes(), Limits::default()).unwrap_or_else(|e| panic!("{input}: {e}"))
}

#[test]
fn doctype_entities_defaults_and_normalization_agree_across_surfaces() {
    let cases: &[(&str, &[&str])] = &[
        ("<!DOCTYPE r><r/>", &["doctype:r:None:None:None", "<r/>"]),
        (
            "<!DOCTYPE r PUBLIC '-//p//EN' \"s'q.dtd\" [ ]><r/>",
            &[
                "doctype:r:Some(\"-//p//EN\"):Some(\"s'q.dtd\"):Some(\" \")",
                "<r/>",
            ],
        ),
        (
            "<?xml version='1.0'?><!--c--><!DOCTYPE r SYSTEM 'x.dtd'><?p d?><r/>",
            &[
                "comment:c",
                "doctype:r:None:Some(\"x.dtd\"):None",
                "pi:p:d",
                "<r/>",
            ],
        ),
        (
            "<!DOCTYPE r [<!ENTITY e '<n a=\"&#38;amp;\"/>x'><!ENTITY z ''>]><r>a&e;b&z;c</r>",
            &[
                "doctype:r:None:None:Some(\"<!ENTITY e '<n a=\\\"&#38;amp;\\\"/>x'><!ENTITY z ''>\")",
                "<r>",
                "\"a\"",
                "<n a=&/>",
                "\"xbc\"",
                "</r>",
            ],
        ),
        (
            "<!DOCTYPE r [<!ENTITY a '&b;&b;'><!ENTITY b '[&c;]'><!ENTITY c 'é🙂'>]><r t='&a;'>&a;</r>",
            &[
                "doctype:r:None:None:Some(\"<!ENTITY a '&b;&b;'><!ENTITY b '[&c;]'><!ENTITY c 'é🙂'>\")",
                "<r t=[é🙂][é🙂]>",
                "\"[é🙂][é🙂]\"",
                "</r>",
            ],
        ),
        (
            "<!DOCTYPE r [<!ENTITY % p '<!ENTITY e \"pe\">'>%p;]><r>&e;</r>",
            &[
                "doctype:r:None:None:Some(\"<!ENTITY % p '<!ENTITY e \\\"pe\\\">'>%p;\")",
                "<r>",
                "\"pe\"",
                "</r>",
            ],
        ),
        (
            "<!DOCTYPE r [<!ATTLIST r a CDATA 'd' n NMTOKENS ' x  y ' i ID #IMPLIED f CDATA #FIXED 'f'>]><r i=' v ' f='f'/>",
            &[
                "doctype:r:None:None:Some(\"<!ATTLIST r a CDATA 'd' n NMTOKENS ' x  y ' i ID #IMPLIED f CDATA #FIXED 'f'>\")",
                "<r i=v f=f a=d n=x y/>",
            ],
        ),
        (
            "<!DOCTYPE p:r [<!ATTLIST p:r xmlns:p CDATA 'urn:p' p:a CDATA 'v'>]><p:r/>",
            &[
                "doctype:p:r:None:None:Some(\"<!ATTLIST p:r xmlns:p CDATA 'urn:p' p:a CDATA 'v'>\")",
                "<p:r xmlns:p=urn:p p:a=v/>",
            ],
        ),
        (
            "<!DOCTYPE r [<!--c--><?p d?><!ELEMENT r (#PCDATA)><!NOTATION n SYSTEM 'n'>]><r>&#60;&lt;</r>",
            &[
                "doctype:r:None:None:Some(\"<!--c--><?p d?><!ELEMENT r (#PCDATA)><!NOTATION n SYSTEM 'n'>\")",
                "<r>",
                "\"<<\"",
                "</r>",
            ],
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(ok(input), *expected, "{input}");
    }
}

#[test]
fn unread_declarations_follow_libxml2_noent_recovery() {
    // An external subset or PE reference may declare an entity that is never
    // read; libxml2 reports a recoverable error and drops the reference.
    let cases: &[(&str, &[&str])] = &[
        (
            "<!DOCTYPE r SYSTEM 'm.dtd'><r a='x&u;y'>a&u;b</r>",
            &[
                "doctype:r:None:Some(\"m.dtd\"):None",
                "<r a=xy>",
                "\"ab\"",
                "</r>",
            ],
        ),
        (
            "<!DOCTYPE r [%q;]><r>a&u;b</r>",
            &["doctype:r:None:None:Some(\"%q;\")", "<r>", "\"ab\"", "</r>"],
        ),
        (
            "<!DOCTYPE r [<!ENTITY % p SYSTEM 'x.ent'>%p;<!ENTITY e 'kept'>]><r>&e;&u;</r>",
            &[
                "doctype:r:None:None:Some(\"<!ENTITY % p SYSTEM 'x.ent'>%p;<!ENTITY e 'kept'>\")",
                "<r>",
                "\"kept\"",
                "</r>",
            ],
        ),
        (
            "<!DOCTYPE r [<!ENTITY e SYSTEM 'x.ent'>]><r>a&e;b</r>",
            &[
                "doctype:r:None:None:Some(\"<!ENTITY e SYSTEM 'x.ent'>\")",
                "<r>",
                "\"ab\"",
                "</r>",
            ],
        ),
        (
            "<!DOCTYPE r SYSTEM 'm.dtd' [<!ENTITY e '<a>x&u;</a>&u;'><!ATTLIST r d CDATA '&u;!'>]><r>&e;</r>",
            &[
                "doctype:r:None:Some(\"m.dtd\"):Some(\"<!ENTITY e '<a>x&u;</a>&u;'><!ATTLIST r d CDATA '&u;!'>\")",
                "<r d=!>",
                "<a>",
                "\"x\"",
                "</a>",
                "</r>",
            ],
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(ok(input), *expected, "{input}");
    }
}

#[test]
fn malformed_and_forbidden_declarations_fail_identically() {
    use ErrorKind::*;
    let cases: &[(&str, ErrorKind, usize)] = &[
        ("<!DOCTYPE r><r>&u;</r>", UnknownEntity, 15),
        (
            "<?xml version='1.0' standalone='yes'?><!DOCTYPE r [%q;]><r/>",
            UnknownEntity,
            51,
        ),
        ("<!DOCTYPE r [<!ENTITY e '&e;'>]><r>&e;</r>", EntityLoop, 35),
        (
            "<!DOCTYPE r [<!ENTITY e SYSTEM 'x' NDATA n>]><r>&e;</r>",
            UnparsedEntity,
            48,
        ),
        (
            "<!DOCTYPE r [<!ENTITY e SYSTEM 'x'>]><r a='&e;'/>",
            ExternalEntityInAttribute,
            43,
        ),
        (
            "<!DOCTYPE r [<!ENTITY e '<a>'>]><r>&e;</a></r>",
            UnbalancedEntity,
            35,
        ),
        (
            "<!DOCTYPE r [<!ENTITY e 'x'>]><r a='&e;&undeclared;'/>",
            UnknownEntity,
            39,
        ),
        ("<!DOCTYPE r [<!BAD>]><r/>", InvalidDtd, 15),
        ("<!DOCTYPE r [<!ENTITY e '%p;'>]><r/>", InvalidDtd, 25),
        ("<!DOCTYPE r [<!ENTITY e 'x'>", UnexpectedEof, 28),
        ("<!DOCTYPE r><!DOCTYPE r><r/>", Syntax, 12),
        ("<r/><!DOCTYPE r>", Syntax, 4),
        (
            "<!DOCTYPE r [<!ATTLIST r a CDATA '<'>]><r/>",
            InvalidDtd,
            34,
        ),
    ];
    for &(input, kind, offset) in cases {
        assert_eq!(
            all(input.as_bytes(), Limits::default()),
            Err(Error { kind, offset }),
            "{input}"
        );
    }
}

fn limited(input: &str, limits: Limits) -> ErrorKind {
    all(input.as_bytes(), limits)
        .expect_err("limit ignored")
        .kind
}

#[test]
fn entity_amplification_and_dtd_ceilings_are_enforced() {
    let mut laughs = String::from("<!DOCTYPE r [<!ENTITY l0 'lol'>");
    for level in 1..10 {
        let previous = format!("&l{};", level - 1).repeat(10);
        laughs.push_str(&format!("<!ENTITY l{level} '{previous}'>"));
    }
    laughs.push_str("]><r>&l9;</r>");
    // Default ceilings stop the classic attacks; Reader alone keeps this fast.
    let default = |input: &str| {
        reader(input.as_bytes(), Limits::default())
            .expect_err("default limit ignored")
            .kind
    };
    assert_eq!(
        default(&laughs),
        ErrorKind::LimitExceeded(Limit::EntityExpansion)
    );
    // Empty leaves expand to nothing but every reference has a fixed cost.
    let empty = laughs.replace("'lol'", "''");
    assert_eq!(
        default(&empty),
        ErrorKind::LimitExceeded(Limit::EntityExpansion)
    );
    let quadratic = format!(
        "<!DOCTYPE r [<!ENTITY a '{}'>]><r>{}</r>",
        "x".repeat(16 * 1024),
        "&a;".repeat(1024)
    );
    assert_eq!(
        default(&quadratic),
        ErrorKind::LimitExceeded(Limit::EntityExpansion)
    );
    let quadratic_attribute = quadratic.replace("<r>", "<r a='").replace("</r>", "'/>");
    assert_eq!(
        default(&quadratic_attribute),
        ErrorKind::LimitExceeded(Limit::EntityExpansion)
    );
    // Every surface enforces the same ceilings.
    let small = {
        let mut cfg = Limits::default();
        cfg.max_entity_references = 50;
        cfg.max_entity_expansion_bytes = 100;
        cfg.max_entity_amplification = 0;
        cfg
    };
    let few = {
        let mut cfg = Limits::default();
        cfg.max_entity_references = 50;
        cfg
    };
    assert_eq!(
        limited(&laughs, few),
        ErrorKind::LimitExceeded(Limit::EntityReferences)
    );
    let small_quadratic =
        "<!DOCTYPE r [<!ENTITY a 'xxxxxxxxxxxxxxxxxxxx'>]><r a='&a;&a;'>&a;&a;&a;&a;&a;&a;</r>";
    assert_eq!(
        limited(small_quadratic, small),
        ErrorKind::LimitExceeded(Limit::EntityExpansion)
    );
    let nested = "<!DOCTYPE r [<!ENTITY a '&b;'><!ENTITY b '&c;'><!ENTITY c 'x'>]><r>&a;</r>";
    let depth = {
        let mut cfg = Limits::default();
        cfg.max_entity_depth = 2;
        cfg
    };
    assert_eq!(
        limited(nested, depth),
        ErrorKind::LimitExceeded(Limit::EntityDepth)
    );
    assert!(
        all(nested.as_bytes(), {
            let mut cfg = depth;
            cfg.max_entity_depth = 3;
            cfg
        })
        .is_ok()
    );
    let dtd = {
        let mut cfg = Limits::default();
        cfg.max_dtd_bytes = 20;
        cfg
    };
    assert_eq!(
        limited("<!DOCTYPE r [<!ENTITY e 'x'>]><r/>", dtd),
        ErrorKind::LimitExceeded(Limit::DtdBytes)
    );
    assert!(all(b"<!DOCTYPE r [ ]><r/>", dtd).is_ok());
    // One use costs its three replacement bytes plus libxml2's fixed 20.
    let expansion = {
        let mut cfg = Limits::default();
        cfg.max_entity_expansion_bytes = 23;
        cfg.max_entity_amplification = 0;
        cfg
    };
    let three = "<!DOCTYPE r [<!ENTITY e 'xyz'>]><r>&e;</r>";
    assert!(all(three.as_bytes(), expansion).is_ok());
    assert_eq!(
        limited(&three.replace("&e;", "&e;&e;"), expansion),
        ErrorKind::LimitExceeded(Limit::EntityExpansion)
    );
    let one = {
        let mut cfg = Limits::default();
        cfg.max_entity_references = 1;
        cfg
    };
    assert!(all(three.as_bytes(), one).is_ok());
    assert_eq!(
        limited(&three.replace("&e;", "&e;&e;"), one),
        ErrorKind::LimitExceeded(Limit::EntityReferences)
    );
}

#[test]
fn utf16_doctypes_stream_and_own_identically() {
    let text = "<!DOCTYPE r [<!ENTITY e 'é🙂'>]><r a='&e;'>&e;</r>";
    let expected = ok(text);
    for little in [true, false] {
        let mut bytes = if little {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for unit in text.encode_utf16() {
            bytes.extend(if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        assert_eq!(owned_tree(&bytes, Limits::default()).unwrap(), expected);
        assert_eq!(push(&bytes, &[3], Limits::default()).unwrap(), expected);
        assert_eq!(buffered(&bytes, 5, Limits::default()).unwrap(), expected);
    }
}

#[test]
fn doctype_origins_cover_the_declaration_and_entity_events_name_their_reference() {
    let input = b"<!DOCTYPE r [<!ENTITY e 'x'>]><r>&e;</r>";
    let mut reader = Reader::new(input).unwrap();
    assert!(matches!(
        reader.read_event().unwrap(),
        Some(Event::Doctype(_))
    ));
    assert_eq!(reader.event_range(), Some(0..30));
    reader.read_event().unwrap();
    // Without NOENT the reference itself is an event spanning `&e;`.
    assert_eq!(
        reader.read_event().unwrap(),
        Some(Event::EntityStart("e".into()))
    );
    assert_eq!(reader.event_range(), Some(33..36));
    assert_eq!(reader.read_event().unwrap(), Some(Event::Text("x".into())));
    let origin = reader.event_origin().unwrap();
    assert_eq!(origin.input_range(), None);
    assert_eq!(origin.replacement_range(), Some(0..1));
    assert_eq!(origin.physical_context(), 33..36);
    assert_eq!(reader.read_event().unwrap(), Some(Event::EntityEnd));
    assert_eq!(reader.event_range(), Some(33..36));
    let document = Document::parse_bytes(input).unwrap();
    let reference = document.root_element().first_child().unwrap();
    assert_eq!(reference.entity_name(), Some("e"));
    assert_eq!(reference.range(), Some(33..36));
    let text = reference.first_child().unwrap();
    assert_eq!(text.range(), None);
    assert_eq!(text.origin().physical_context(), 33..36);
}

fn write(events: &[Event<'_>]) -> Result<Vec<u8>, yaxml::WriteError> {
    let mut writer = Writer::new(Vec::new());
    for event in events {
        writer.write_event(event)?;
    }
    writer.finish()?;
    Ok(writer.into_inner())
}

fn empty_root() -> Event<'static> {
    Event::Empty(yaxml::Element {
        name: yaxml::Name::new("r", None).unwrap(),
        attributes: Vec::new().into(),
    })
}

#[test]
fn writer_emits_valid_doctypes_and_rejects_invalid_ones() {
    let doctype = |name, public_id, system_id, internal_subset| {
        Event::Doctype(Doctype {
            name,
            public_id,
            system_id,
            internal_subset,
        })
    };
    for (event, output) in [
        (doctype("r", None, None, None), "<!DOCTYPE r><r/>"),
        (
            doctype("r", Some("-//p"), Some("a\"b"), Some("<!ENTITY e 'x'>")),
            "<!DOCTYPE r PUBLIC \"-//p\" 'a\"b' [<!ENTITY e 'x'>]><r/>",
        ),
        (
            doctype("r", None, Some("s"), Some("")),
            "<!DOCTYPE r SYSTEM \"s\" []><r/>",
        ),
    ] {
        let written = write(&[event, empty_root()]).unwrap();
        assert_eq!(String::from_utf8(written.clone()).unwrap(), output);
        assert!(Document::parse_bytes(&written).is_ok());
    }
    for (events, kind) in [
        (
            vec![doctype("r", Some("p"), None, None)],
            ErrorKind::InvalidDtd,
        ),
        (
            vec![doctype("a b", None, None, None)],
            ErrorKind::InvalidDtd,
        ),
        (
            vec![doctype("r", None, Some("'\""), None)],
            ErrorKind::UnexpectedEof,
        ),
        (
            vec![doctype("r", None, None, Some("<!BAD>"))],
            ErrorKind::InvalidDtd,
        ),
        (
            vec![doctype("r", None, None, Some("]><!DOCTYPE x ["))],
            ErrorKind::InvalidDtd,
        ),
        (
            vec![
                doctype("r", None, None, None),
                doctype("r", None, None, None),
            ],
            ErrorKind::Syntax,
        ),
        (
            vec![empty_root(), doctype("r", None, None, None)],
            ErrorKind::Syntax,
        ),
    ] {
        match write(&events) {
            Err(yaxml::WriteError::Xml(error)) => assert_eq!(error.kind, kind, "{events:?}"),
            other => panic!("{events:?}: {other:?}"),
        }
    }
}

#[test]
fn serializers_round_trip_doctypes_with_expanded_content() {
    let input = "<!DOCTYPE r [<!ENTITY e '<n/>&#38;amp;'><!ATTLIST r a CDATA 'd'>]><r>&e;</r>";
    let expected = "<!DOCTYPE r [<!ENTITY e '<n/>&#38;amp;'><!ATTLIST r a CDATA 'd'>]><r a=\"d\"><n/>&amp;</r>";
    let options = || legacy(Limits::default());
    let mut writer = Writer::new(Vec::new());
    writer
        .write_document(&Document::parse_with(input.as_bytes(), options()).unwrap())
        .unwrap();
    assert_eq!(String::from_utf8(writer.into_inner()).unwrap(), expected);
    let mut writer = Writer::new(Vec::new());
    let owned = OwnedDocument::parse_with(input.as_bytes(), options()).unwrap();
    writer.write_owned_document(&owned).unwrap();
    assert_eq!(String::from_utf8(writer.into_inner()).unwrap(), expected);
    // Kept references are written as their content; defaults need DTDATTR.
    let mut writer = Writer::new(Vec::new());
    writer
        .write_document(&Document::parse(input).unwrap())
        .unwrap();
    let kept = expected.replace(" a=\"d\"", "");
    assert_eq!(String::from_utf8(writer.into_inner()).unwrap(), kept);
    assert_eq!(ok(expected), ok(input));
}

/// Differential check against the pinned libxml2 2.15.4 `xmllint`, using its
/// `--noent --dtdattr` substitution and default-attribute semantics.
#[test]
#[ignore = "requires YAXML_XMLLINT pointing at the pinned libxml2 2.15.4 xmllint"]
fn oracle_xmllint_agrees_on_acceptance_and_serialized_root() {
    let xmllint = std::env::var_os("YAXML_XMLLINT").expect("YAXML_XMLLINT");
    let directory = std::env::temp_dir().join(format!("yaxml-dtd-oracle-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let mut differences = Vec::new();
    for input in ORACLE_CASES {
        let path = directory.join("case.xml");
        std::fs::write(&path, input).unwrap();
        let output = std::process::Command::new(&xmllint)
            .args(["--noent", "--dtdattr", "--nonet", "--dropdtd"])
            .arg(&path)
            .output()
            .unwrap();
        let oracle = output.status.success().then(|| {
            let text = String::from_utf8(output.stdout).unwrap();
            let body = text
                .split_once("?>\n")
                .map_or(text.as_str(), |(_, body)| body);
            body.trim_end().to_owned()
        });
        let ours = Document::parse(input).ok().map(|document| {
            let mut writer = Writer::new(Vec::new());
            write_element(&mut writer, document.root_element());
            writer.finish().unwrap();
            String::from_utf8(writer.into_inner()).unwrap()
        });
        if oracle != ours {
            differences.push(format!(
                "{input}\n  libxml2: {oracle:?}\n  yaxml:   {ours:?}"
            ));
        }
    }
    std::fs::remove_dir_all(&directory).ok();
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

/// Writes only the document element, as `xmllint --dropdtd` prints it.
fn write_element(writer: &mut Writer<Vec<u8>>, node: yaxml::Node<'_, '_>) {
    match node.kind() {
        NodeKind::EntityRef(_) => node
            .children()
            .for_each(|child| write_element(writer, child)),
        NodeKind::Element(element) if node.first_child().is_some() => {
            writer.write_event(&Event::Start(clone(element))).unwrap();
            for child in node.children() {
                write_element(writer, child);
            }
            writer
                .write_event(&Event::End(borrow(&element.name)))
                .unwrap();
        }
        NodeKind::Element(element) => writer.write_event(&Event::Empty(clone(element))).unwrap(),
        NodeKind::Text(text) => writer
            .write_event(&Event::Text(text.as_ref().into()))
            .unwrap(),
        NodeKind::Cdata(text) => writer
            .write_event(&Event::Cdata(text.as_ref().into()))
            .unwrap(),
        NodeKind::Comment(text) => writer
            .write_event(&Event::Comment(text.as_ref().into()))
            .unwrap(),
        NodeKind::ProcessingInstruction { target, data } => writer
            .write_event(&Event::ProcessingInstruction {
                target: target.as_ref().into(),
                data: data.as_ref().into(),
            })
            .unwrap(),
        NodeKind::Document | NodeKind::Doctype(_) => unreachable!(),
    }
}

const ORACLE_CASES: &[&str] = &[
    "<!DOCTYPE r><r/>",
    "<!DOCTYPE r [<!ENTITY e '<n a=\"1\"/>x'>]><r>a&e;b</r>",
    "<!DOCTYPE r [<!ENTITY a '&b;&b;'><!ENTITY b '[&c;]'><!ENTITY c 'z'>]><r t='&a;'>&a;</r>",
    "<!DOCTYPE r [<!ENTITY % p '<!ENTITY e \"pe\">'>%p;]><r>&e;</r>",
    "<!DOCTYPE r [<!ATTLIST r a CDATA 'd' n NMTOKENS ' x  y '>]><r n='  p   q '/>",
    "<!DOCTYPE r [<!ATTLIST r i ID #IMPLIED>]><r i=' v '/>",
    "<!DOCTYPE r SYSTEM 'm.dtd'><r a='x&u;y'>a&u;b</r>",
    "<!DOCTYPE r [%q;]><r>a&u;b</r>",
    "<!DOCTYPE r [<!ENTITY % p SYSTEM 'x.ent'>%p;<!ENTITY e 'kept'>]><r>&e;&u;</r>",
    "<!DOCTYPE r [<!ENTITY e SYSTEM 'x.ent'>]><r>a&e;b</r>",
    "<!DOCTYPE r SYSTEM 'm.dtd' [<!ENTITY e '<a>x&u;</a>&u;'>]><r>&e;</r>",
    "<!DOCTYPE r><r>&u;</r>",
    "<!DOCTYPE r [<!ENTITY e '&e;'>]><r>&e;</r>",
    "<!DOCTYPE r [<!ENTITY e SYSTEM 'x' NDATA n>]><r>&e;</r>",
    "<!DOCTYPE r [<!ENTITY e SYSTEM 'x'>]><r a='&e;'/>",
    "<!DOCTYPE r [<!ENTITY e '<a>'>]><r>&e;</a></r>",
    "<!DOCTYPE r [<!BAD>]><r/>",
    "<!DOCTYPE r><!DOCTYPE r><r/>",
    "<?xml version='1.0' standalone='yes'?><!DOCTYPE r [%q;]><r/>",
    "<!DOCTYPE r [<!ENTITY e 'x'>]><r a='&e;&undeclared;'/>",
    "<!DOCTYPE p:r [<!ATTLIST p:r xmlns:p CDATA 'urn:p'>]><p:r/>",
    "<!DOCTYPE r [<!ENTITY e '&#38;#60;'>]><r>&e;</r>",
];

fn borrow<'a>(name: &'a yaxml::Name<'_>) -> yaxml::Name<'a> {
    yaxml::Name::new(name.qualified(), name.namespace().map(Into::into)).unwrap()
}

#[test]
fn single_byte_doctypes_decode_before_entity_processing() {
    let mut bytes =
        b"<?xml version='1.0' encoding='ISO-8859-1'?><!DOCTYPE r [<!ENTITY e '".to_vec();
    bytes.push(0xe9);
    bytes.extend_from_slice(b"'>]><r a='&e;'>&e;\xe9</r>");
    let expected = [
        "doctype:r:None:None:Some(\"<!ENTITY e 'é'>\")",
        "<r a=é>",
        "\"éé\"",
        "</r>",
    ];
    assert_eq!(owned_tree(&bytes, Limits::default()).unwrap(), expected);
    assert_eq!(
        push(&bytes, &[1; 1 << 10], Limits::default()).unwrap(),
        expected
    );
    assert_eq!(buffered(&bytes, 3, Limits::default()).unwrap(), expected);
    let mut reader = BufferedReader::new(bytes.as_slice());
    while let Some((event, origin)) = reader.read_event().unwrap() {
        if let Event::Doctype(_) = event {
            assert_eq!(origin.input_range(), Some(43..73));
        }
    }
}

#[test]
fn declaration_token_and_default_ceilings_fail_identically() {
    use ErrorKind::{InvalidDtd, LimitExceeded};
    let d = Limits::default();
    let long_reference = format!("<!DOCTYPE r [<!ENTITY a 'x'>]><r>&{};</r>", "a".repeat(40));
    let long_value = format!("<!DOCTYPE r [<!ENTITY e '{}'>]><r/>", "x".repeat(40));
    let cases: &[(&str, Limits, ErrorKind, usize)] = &[
        (
            "<!DOCTYPE r [<!ENTITY lt SYSTEM 'x'>]><r/>",
            d,
            InvalidDtd,
            13,
        ),
        (
            "<!DOCTYPE r [<!ATTLIST r a CDATA 'x'><!ATTLIST r b CDATA 'y'>]><r/>",
            {
                let mut cfg = d;
                cfg.max_attributes = 1;
                cfg
            },
            LimitExceeded(Limit::Attributes),
            63,
        ),
        (
            "<!DOCTYPE r [<!ATTLIST r a CDATA 'x'>]><r c='z'/>",
            {
                let mut cfg = d;
                cfg.max_attributes = 1;
                cfg
            },
            LimitExceeded(Limit::Attributes),
            39,
        ),
        (
            "<!DOCTYPE r [<!ELEMENT r ((((a))))>]><r/>",
            {
                let mut cfg = d;
                cfg.max_depth = 2;
                cfg
            },
            LimitExceeded(Limit::Depth),
            27,
        ),
        (
            "<!DOCTYPE r [<!ELEMENT r (#PCDATA)>]><r/>",
            {
                let mut cfg = d;
                cfg.max_depth = 0;
                cfg
            },
            LimitExceeded(Limit::Depth),
            25,
        ),
        (
            &long_reference,
            {
                let mut cfg = d;
                cfg.max_token_bytes = 32;
                cfg
            },
            LimitExceeded(Limit::TokenBytes),
            33,
        ),
        (
            "<!DOCTYPE r [<!ENTITY a 'x'>]><r>xxxxxxxxxxxxxxxxxxxxxxxxxxxx&#x41;</r>",
            {
                let mut cfg = d;
                cfg.max_token_bytes = 32;
                cfg
            },
            LimitExceeded(Limit::TokenBytes),
            33,
        ),
        (
            "<!DOCTYPE r [<!ENTITY a 'xxxxxxxxxxxxxxxxxxxxxxxxxxxx&#x41;'>]><r>&a;</r>",
            {
                let mut cfg = d;
                cfg.max_token_bytes = 32;
                cfg
            },
            LimitExceeded(Limit::TokenBytes),
            45,
        ),
        (
            &long_value,
            {
                let mut cfg = d;
                cfg.max_token_bytes = 32;
                cfg
            },
            LimitExceeded(Limit::TokenBytes),
            45,
        ),
        (
            "<!DOCTYPE r [<!-- a long comment here -->]><r/>",
            {
                let mut cfg = d;
                cfg.max_dtd_bytes = 20;
                cfg
            },
            LimitExceeded(Limit::DtdBytes),
            20,
        ),
        (
            "<!DOCTYPE r [<!ENTITY e 'xyz'>]><r>&e;&e;</r>",
            {
                let mut cfg = d;
                cfg.max_entity_references = 1;
                cfg
            },
            LimitExceeded(Limit::EntityReferences),
            38,
        ),
        (
            "<!DOCTYPE r [<!ENTITY e 'xyz'>]><r a='&e;&e;'/>",
            {
                let mut cfg = d;
                cfg.max_entity_references = 1;
                cfg
            },
            LimitExceeded(Limit::EntityReferences),
            41,
        ),
        (
            "<!DOCTYPE r [<!ENTITY e 'a]]>b'>]><r>&e;</r>",
            d,
            ErrorKind::Syntax,
            37,
        ),
        (
            "<!DOCTYPE r [<!ENTITY % p ''>%p;%p;]><r/>",
            {
                let mut cfg = d;
                cfg.max_entity_references = 1;
                cfg
            },
            LimitExceeded(Limit::EntityReferences),
            32,
        ),
    ];
    for &(input, limits, kind, offset) in cases {
        assert_eq!(
            all(input.as_bytes(), limits),
            Err(Error { kind, offset }),
            "{input}"
        );
    }
    // Normalizing a tokenized value spends the string budget; streaming
    // surfaces copy names too, so they stop earlier.
    let input = b"<!DOCTYPE r [<!ATTLIST r a NMTOKENS #IMPLIED>]>\
        <r a='   xxxxxxxxxxxxxxxxxxxxxxxx    yyyyyyyyyyyyyyyyyyyyy   '/>";
    let owned = {
        let mut cfg = d;
        cfg.max_owned_bytes = 60;
        cfg
    };
    let error = |offset| {
        Err(Error {
            kind: LimitExceeded(Limit::OwnedBytes),
            offset,
        })
    };
    assert_eq!(reader(input, owned), error(56));
    assert_eq!(push(input, &[], owned), error(45));
}

#[test]
fn dtd_bytes_bound_only_the_doctype_and_input_bytes_the_document() {
    use ErrorKind::LimitExceeded;
    let body = "<e a='v'>text &e;</e>".repeat(20);
    let small = format!("<!DOCTYPE r [<!ENTITY e 'x'>]><r>{body}</r>");
    let large = format!("<!DOCTYPE r [<!ENTITY e '{}'>]><r>&e;</r>", "x".repeat(200));
    let dtd = |max_dtd_bytes| {
        let mut cfg = Limits::default();
        cfg.max_dtd_bytes = max_dtd_bytes;
        cfg
    };
    let input = {
        let mut cfg = Limits::default();
        cfg.max_input_bytes = small.len() - 1;
        cfg
    };
    type Case<'a> = (&'a str, Limits, Option<(ErrorKind, usize)>);
    let cases: &[Case<'_>] = &[
        // Content after a small DOCTYPE may exceed max_dtd_bytes.
        (&small, dtd(64), None),
        (&small, dtd(small.len() / 4), None),
        // The DOCTYPE itself, internal subset included, may not.
        (
            &large,
            dtd(100),
            Some((LimitExceeded(Limit::DtdBytes), 100)),
        ),
        (&large, dtd(large.find("]>").unwrap() + 2), None),
        // Input bytes bound a document with a DOCTYPE as one without.
        (&small, input, Some((LimitExceeded(Limit::InputBytes), 0))),
    ];
    for (input, limits, expected) in cases {
        let result = all(input.as_bytes(), *limits);
        match expected {
            None => assert!(result.is_ok(), "{input}: {result:?}"),
            Some((kind, offset)) => assert_eq!(
                result,
                Err(Error {
                    kind: *kind,
                    offset: *offset
                }),
                "{input}"
            ),
        }
    }
}
