use std::borrow::Cow;
use std::io::BufReader;
use std::ops::ControlFlow;

use yaxml::{
    Attribute, BufferedReader, Document, Element, ErrorKind, Event, Name, NodeKind, OwnedDocument,
    OwnedNodeKind, PushParser, Reader, WriteError, Writer,
};

const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
const PREFIX: &str = "p";
const LOCAL: &str = "root";
const QUALIFIED: &str = "p:root";
const NAMESPACE: &str = "urn:name-ownership";
const PI_TARGET: &str = "πtarget";
const PI_DATA: &str = "literal payload";
const SERIALIZED: &[u8] =
    b"<p:root xmlns:p=\"urn:name-ownership\"><?\xcf\x80target literal payload?></p:root>";

fn assert_name(
    name: &Name<'_>,
    qualified: &str,
    prefix: Option<&str>,
    local: &str,
    namespace: Option<&str>,
) {
    assert_eq!(name.qualified(), qualified);
    assert_eq!(name.prefix(), prefix);
    assert_eq!(name.local(), local);
    assert_eq!(name.namespace(), namespace);
}

fn root_event<'a>(name: Name<'a>, attributes: Vec<Attribute<'a>>, empty: bool) -> Event<'a> {
    let element = Element {
        name,
        attributes: attributes.into(),
    };
    if empty {
        Event::Empty(element)
    } else {
        Event::Start(element)
    }
}

fn namespace_declaration<'a>(namespace: Cow<'a, str>) -> Attribute<'a> {
    Attribute {
        name: Name::new("xmlns:p", Some(Cow::Borrowed(XMLNS))).unwrap(),
        value: namespace,
    }
}

#[test]
fn constructor_accepts_unicode_qnames_and_preserves_namespace_metadata() {
    for (qualified, prefix, local, namespace) in [
        ("root", None, "root", None),
        ("xml", None, "xml", Some("")),
        (
            "_a-0.é·\u{301}\u{203f}",
            None,
            "_a-0.é·\u{301}\u{203f}",
            Some("urn:literal"),
        ),
        ("π:名", Some("π"), "名", Some("urn:π")),
        (
            "\u{10000}:\u{200c}x",
            Some("\u{10000}"),
            "\u{200c}x",
            Some(""),
        ),
    ] {
        let name = Name::new(qualified, namespace.map(Cow::Borrowed)).unwrap();
        assert_name(&name, qualified, prefix, local, namespace);
    }
}

#[test]
fn constructor_rejects_invalid_qnames_at_utf8_byte_offsets() {
    for (qualified, offset) in [
        ("", 0),
        ("1root", 0),
        ("-root", 0),
        (".root", 0),
        ("\u{301}root", 0),
        (":root", 0),
        ("root:", 5),
        ("a:b:c", 3),
        ("π:1", 3),
        ("π:\u{301}", 3),
        ("a b", 1),
        ("a/b", 1),
        ("a\u{37e}", 1),
        ("a\u{f0000}", 1),
    ] {
        let error = Name::new(qualified, None).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidName, "{qualified:?}");
        assert_eq!(error.offset, offset, "{qualified:?}");
    }
}

#[test]
fn getters_recompose_bounded_borrowed_and_owned_qnames() {
    let parts = [
        "a",
        "_",
        "π",
        "名",
        "a0",
        "a-b",
        "a·b",
        "a\u{301}",
        "\u{10000}x",
    ];
    for prefix in [None, Some("p"), Some("π"), Some("\u{10000}")] {
        for local in parts {
            let qualified =
                prefix.map_or_else(|| local.to_owned(), |prefix| format!("{prefix}:{local}"));
            let borrowed =
                Name::new(qualified.as_str(), Some(Cow::Borrowed("urn:bounded"))).unwrap();
            let owned: Name<'static> = Name::new(
                qualified.clone(),
                Some(Cow::Owned("urn:bounded".to_owned())),
            )
            .unwrap();

            for name in [&borrowed, &owned] {
                let recomposed = name.prefix().map_or_else(
                    || name.local().to_owned(),
                    |prefix| format!("{prefix}:{}", name.local()),
                );
                assert_eq!(name.qualified(), qualified);
                assert_eq!(recomposed, qualified);
                assert_eq!(name.prefix(), prefix);
                assert_eq!(name.local(), local);
                assert_eq!(name.namespace(), Some("urn:bounded"));
            }
        }
    }
}

#[test]
fn constructor_borrows_slices_and_moves_owned_buffers() {
    let borrowed_qualified = QUALIFIED;
    let borrowed_namespace = NAMESPACE;
    let borrowed = Name::new(borrowed_qualified, Some(Cow::Borrowed(borrowed_namespace))).unwrap();
    assert_eq!(borrowed.qualified().as_ptr(), borrowed_qualified.as_ptr());
    assert_eq!(
        borrowed.namespace().unwrap().as_ptr(),
        borrowed_namespace.as_ptr()
    );

    let owned_qualified = QUALIFIED.to_owned();
    let qualified_pointer = owned_qualified.as_ptr();
    let owned_namespace = NAMESPACE.to_owned();
    let namespace_pointer = owned_namespace.as_ptr();
    let owned: Name<'static> =
        Name::new(owned_qualified, Some(Cow::Owned(owned_namespace))).unwrap();
    assert_eq!(owned.qualified().as_ptr(), qualified_pointer);
    assert_eq!(owned.namespace().unwrap().as_ptr(), namespace_pointer);
}

fn owned_events() -> Vec<Event<'static>> {
    let namespace = NAMESPACE.to_owned();
    let root = Name::new(QUALIFIED.to_owned(), Some(Cow::Owned(namespace.clone()))).unwrap();
    let end = Name::new(QUALIFIED.to_owned(), Some(Cow::Owned(namespace.clone()))).unwrap();
    vec![
        root_event(
            root,
            vec![namespace_declaration(Cow::Owned(namespace))],
            false,
        ),
        Event::ProcessingInstruction {
            target: Cow::Owned(PI_TARGET.to_owned()),
            data: Cow::Owned(PI_DATA.to_owned()),
        },
        Event::End(end),
    ]
}

fn assert_reader_values(input: &[u8]) {
    let mut reader = Reader::new(input).unwrap();
    let Event::Start(root) = reader.read_event().unwrap().unwrap() else {
        panic!("missing root start")
    };
    assert_name(&root.name, QUALIFIED, Some(PREFIX), LOCAL, Some(NAMESPACE));
    assert_eq!(root.attributes.len(), 1);
    assert_name(
        &root.attributes[0].name,
        "xmlns:p",
        Some("xmlns"),
        PREFIX,
        Some(XMLNS),
    );
    assert_eq!(root.attributes[0].value, NAMESPACE);
    let Event::ProcessingInstruction { target, data } = reader.read_event().unwrap().unwrap()
    else {
        panic!("missing processing instruction")
    };
    assert_eq!(target, PI_TARGET);
    assert_eq!(data, PI_DATA);
    let Event::End(end) = reader.read_event().unwrap().unwrap() else {
        panic!("missing root end")
    };
    assert_name(&end, QUALIFIED, Some(PREFIX), LOCAL, Some(NAMESPACE));
    assert!(reader.read_event().unwrap().is_none());
}

#[test]
fn owned_names_and_pi_targets_survive_inputs_and_round_trip_through_all_surfaces() {
    let events = owned_events();
    let Event::Start(root) = &events[0] else {
        panic!("missing owned root")
    };
    assert_name(&root.name, QUALIFIED, Some(PREFIX), LOCAL, Some(NAMESPACE));
    let Event::ProcessingInstruction { target, data } = &events[1] else {
        panic!("missing owned processing instruction")
    };
    assert_eq!(target, PI_TARGET);
    assert_eq!(data, PI_DATA);

    let mut writer = Writer::new(Vec::new());
    for event in &events {
        writer.write_event(event).unwrap();
    }
    writer.finish().unwrap();
    let output = writer.into_inner();
    assert_eq!(output, SERIALIZED);

    assert_reader_values(&output);

    let text = std::str::from_utf8(&output).unwrap();
    let document = Document::parse(text).unwrap();
    assert_name(
        document.root_element().name().unwrap(),
        QUALIFIED,
        Some(PREFIX),
        LOCAL,
        Some(NAMESPACE),
    );
    let pi = document.root_element().first_child().unwrap();
    assert_eq!(pi.text(), Some(PI_DATA));
    let NodeKind::ProcessingInstruction { target, data } = pi.kind() else {
        panic!("missing borrowing-DOM processing instruction")
    };
    assert_eq!(target, PI_TARGET);
    assert_eq!(data, PI_DATA);

    let owned = OwnedDocument::parse_bytes(&output).unwrap();
    assert_name(
        &owned.root_element().name().unwrap(),
        QUALIFIED,
        Some(PREFIX),
        LOCAL,
        Some(NAMESPACE),
    );
    let pi = owned.root_element().first_child().unwrap();
    assert_eq!(pi.kind(), OwnedNodeKind::ProcessingInstruction);
    assert_eq!(pi.processing_instruction_target(), Some(PI_TARGET));
    assert_eq!(pi.text(), Some(PI_DATA));

    let mut push_values = Vec::new();
    PushParser::new()
        .read_from(output.as_slice(), |event, _| {
            match event {
                Event::Start(element) => push_values.push((
                    element.name.qualified().to_owned(),
                    element.name.namespace().map(str::to_owned),
                )),
                Event::ProcessingInstruction { target, data } => {
                    push_values.push((target.to_string(), Some(data.to_string())))
                }
                _ => {}
            }
            ControlFlow::Continue(())
        })
        .unwrap();
    assert_eq!(
        push_values,
        [
            (QUALIFIED.to_owned(), Some(NAMESPACE.to_owned())),
            (PI_TARGET.to_owned(), Some(PI_DATA.to_owned())),
        ]
    );

    let mut buffered = BufferedReader::new(BufReader::with_capacity(1, output.as_slice()));
    let mut buffered_values = Vec::new();
    while let Some((event, _)) = buffered.read_event().unwrap() {
        match event {
            Event::Start(element) => buffered_values.push((
                element.name.qualified().to_owned(),
                element.name.namespace().map(str::to_owned),
            )),
            Event::ProcessingInstruction { target, data } => {
                buffered_values.push((target.into_owned(), Some(data.into_owned())))
            }
            _ => {}
        }
    }
    assert_eq!(buffered_values, push_values);
}

#[test]
fn name_constructor_defers_namespace_context_checks_to_writer() {
    let cases = [
        Name::new("p:r", Some(Cow::Borrowed("urn:missing"))).unwrap(),
        Name::new("r", Some(Cow::Borrowed("urn:missing"))).unwrap(),
        Name::new("r", Some(Cow::Borrowed(""))).unwrap(),
    ];
    for name in cases {
        let mut writer = Writer::new(Vec::new());
        let error = writer
            .write_event(&root_event(name, Vec::new(), true))
            .unwrap_err();
        assert!(
            matches!(error, WriteError::Xml(error) if error.kind == ErrorKind::Namespace),
            "{error}"
        );
        assert_eq!(writer.position(), 0);
    }

    let mut writer = Writer::new(Vec::new());
    let wrong_binding = root_event(
        Name::new("p:r", Some(Cow::Borrowed("urn:expected"))).unwrap(),
        vec![namespace_declaration(Cow::Borrowed("urn:actual"))],
        true,
    );
    let error = writer.write_event(&wrong_binding).unwrap_err();
    assert!(matches!(error, WriteError::Xml(error) if error.kind == ErrorKind::Namespace));
    assert_eq!(writer.position(), 0);
}

#[test]
fn declarations_keep_their_borrowed_public_contract() {
    let input = "<?xml version='1.7' encoding='UTF-8' standalone='no'?><r/>";
    let mut reader = Reader::new(input.as_bytes()).unwrap();
    let Event::Declaration(declaration) = reader.read_event().unwrap().unwrap() else {
        panic!("missing declaration")
    };
    assert_eq!(declaration.version, "1.7");
    assert_eq!(declaration.encoding, Some("UTF-8"));
    assert_eq!(declaration.standalone, Some(false));
    assert!(std::ptr::eq(
        declaration.version.as_ptr(),
        input[15..].as_ptr()
    ));
}
