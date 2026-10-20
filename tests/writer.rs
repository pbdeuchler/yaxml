#![forbid(unsafe_code)]

#[path = "support/fuzz/driver.rs"]
mod driver;

#[path = "support/capture.rs"]
mod capture;

use std::borrow::Cow;

use yaxml::{
    Attribute, Declaration, Document, Element, ErrorKind, Event, Limit, Name, Reader, WriteError,
    Writer, WriterLimits,
};

fn name<'a>(qualified: &'a str, namespace: Option<&'a str>) -> Name<'a> {
    Name::new(qualified, namespace.map(Cow::Borrowed)).unwrap()
}

fn element<'a>(
    qualified: &'a str,
    namespace: Option<&'a str>,
    attributes: Vec<Attribute<'a>>,
    empty: bool,
) -> Event<'a> {
    let element = Element {
        name: name(qualified, namespace),
        attributes: attributes.into(),
    };
    if empty {
        Event::Empty(element)
    } else {
        Event::Start(element)
    }
}

fn root(empty: bool) -> Event<'static> {
    element("r", None, vec![], empty)
}

fn attribute<'a>(qualified: &'a str, namespace: Option<&'a str>, value: &'a str) -> Attribute<'a> {
    Attribute {
        name: name(qualified, namespace),
        value: Cow::Borrowed(value),
    }
}

fn read(input: &[u8]) -> Vec<Event<'_>> {
    let mut reader = Reader::new(input).unwrap();
    let mut events = Vec::new();
    while let Some(event) = reader.read_event().unwrap() {
        events.push(event);
    }
    events
}

fn serialize(input: &[u8]) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new());
    for event in read(input) {
        writer.write_event(&event).unwrap();
    }
    writer.finish().unwrap();
    let position = writer.position();
    let output = writer.into_inner();
    assert_eq!(position, output.len());
    output
}

fn assert_round_trip(input: &[u8], output: &[u8]) {
    assert_eq!(read(output), read(input));
    let events = read(output);
    let document = Document::parse_bytes(output).unwrap();
    driver::check_projection(&document, &events);
    let expected: Vec<_> = capture::slice_events(output)
        .unwrap()
        .into_iter()
        .map(|(event, _)| event)
        .collect();
    for size in [1, 2, 3, 7, 31] {
        let schedule = vec![size; output.len().div_ceil(size)];
        let actual: Vec<_> = capture::push_events(output, &schedule, yaxml::Limits::default())
            .unwrap()
            .into_iter()
            .map(|(event, _)| event)
            .collect();
        assert_eq!(actual, expected);
    }
    assert!(driver::exercise(output).error.is_none());
}

fn assert_rejected(prefix: &[Event<'_>], invalid: &Event<'_>, expected: ErrorKind) {
    let mut writer = Writer::new(Vec::new());
    for event in prefix {
        writer.write_event(event).unwrap();
    }
    let position = writer.position();
    let error = writer.write_event(invalid).unwrap_err();
    assert!(
        matches!(error, WriteError::Xml(error) if error.kind == expected),
        "{error}"
    );
    assert_eq!(writer.position(), position);
    assert!(
        matches!(writer.write_event(&root(true)), Err(WriteError::Xml(error)) if error.kind == ErrorKind::InvalidState)
    );
    assert!(
        matches!(writer.finish(), Err(WriteError::Xml(error)) if error.kind == ErrorKind::InvalidState)
    );
    assert_eq!(
        writer.into_inner().len(),
        position,
        "invalid event emitted bytes"
    );
}

#[test]
fn exact_bytes_escape_logical_values_and_preserve_every_event_kind() {
    for (input, expected) in [
        ("<r/>", "<r/>"),
        (
            "<?xml version='1.00' encoding='uTf-8' standalone='no'?><!--a--><?p d?><r a='&quot;&apos;&lt;&amp;&gt;&#9;&#10;&#13;'>é中🙂&lt;&amp;&gt;&#13;<![CDATA[<&>]]><!--c--><?q?><c/></r>",
            "<?xml version=\"1.00\" encoding=\"uTf-8\" standalone=\"no\"?><!--a--><?p d?><r a=\"&quot;'&lt;&amp;>&#9;&#10;&#13;\">é中🙂&lt;&amp;&gt;&#13;<![CDATA[<&>]]><!--c--><?q?><c/></r>",
        ),
        (
            "<r>]]&gt;&amp;lt;&lt;\"'</r>",
            "<r>]]&gt;&amp;lt;&lt;\"'</r>",
        ),
        ("<é a='🙂'><中/></é>", "<é a=\"🙂\"><中/></é>"),
        (
            "<p:r xmlns:p='urn:&lt;&amp;&quot;é&#13;'><p:c/></p:r>",
            "<p:r xmlns:p=\"urn:&lt;&amp;&quot;é&#13;\"><p:c/></p:r>",
        ),
        (
            "<!--before--><r></r><!--after-->",
            "<!--before--><r></r><!--after-->",
        ),
    ] {
        let output = serialize(input.as_bytes());
        assert_eq!(output, expected.as_bytes());
        assert_round_trip(input.as_bytes(), &output);
    }
}

#[test]
fn explicit_namespaces_shadow_unwind_and_resolve_independent_of_attribute_order() {
    let input = b"<p:r p:a='1' xmlns:p='urn:u' xmlns='urn:d' a='2'><p:c xmlns:p='urn:v'/><p:c/><c xmlns=''/><c xml:lang='en'/></p:r>";
    let output = serialize(input);
    assert_eq!(output, b"<p:r p:a=\"1\" xmlns:p=\"urn:u\" xmlns=\"urn:d\" a=\"2\"><p:c xmlns:p=\"urn:v\"/><p:c/><c xmlns=\"\"/><c xml:lang=\"en\"/></p:r>");
    assert_round_trip(input, &output);
    const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
    for (prefix, invalid, expected) in [
        (
            vec![],
            element("p:r", Some("urn:u"), vec![], true),
            ErrorKind::Namespace,
        ),
        (
            vec![],
            element("r", Some("urn:u"), vec![], true),
            ErrorKind::Namespace,
        ),
        (
            vec![],
            element("r", None, vec![attribute("p:a", Some("urn:u"), "v")], true),
            ErrorKind::Namespace,
        ),
        (
            vec![],
            element("r", None, vec![attribute("xmlns:p", Some(XMLNS), "")], true),
            ErrorKind::Namespace,
        ),
        (
            vec![],
            element(
                "r",
                None,
                vec![attribute("xmlns:xml", Some(XMLNS), "urn:wrong")],
                true,
            ),
            ErrorKind::Namespace,
        ),
        (
            vec![],
            element("xmlns:r", Some(XMLNS), vec![], true),
            ErrorKind::Namespace,
        ),
        (
            vec![],
            element(
                "r",
                None,
                vec![attribute("a", None, "1"), attribute("a", None, "2")],
                true,
            ),
            ErrorKind::DuplicateAttribute,
        ),
        (
            vec![],
            element(
                "r",
                None,
                vec![
                    attribute("xmlns:p", Some(XMLNS), "urn:u"),
                    attribute("xmlns:q", Some(XMLNS), "urn:u"),
                    attribute("p:a", Some("urn:u"), "1"),
                    attribute("q:a", Some("urn:u"), "2"),
                ],
                true,
            ),
            ErrorKind::DuplicateAttribute,
        ),
        (
            vec![
                element("r", None, vec![], false),
                element(
                    "p:c",
                    Some("urn:u"),
                    vec![attribute("xmlns:p", Some(XMLNS), "urn:u")],
                    true,
                ),
            ],
            element("p:c", Some("urn:u"), vec![], true),
            ErrorKind::Namespace,
        ),
    ] {
        assert_rejected(&prefix, &invalid, expected);
    }
}

#[test]
fn name_construction_rejects_invalid_spelling_before_writing() {
    for (qualified, offset) in [
        ("", 0),
        ("1r", 0),
        (":r", 0),
        ("p:", 2),
        ("p:q:r", 3),
        ("r a='injected'", 1),
        ("a='injected' b", 1),
        ("r ", 1),
    ] {
        let error = Name::new(qualified, None).unwrap_err();
        assert_eq!((error.kind, error.offset), (ErrorKind::InvalidName, offset));
    }
}

#[test]
fn writer_name_metadata_must_match_resolved_namespace() {
    for invalid in [
        element("r", Some(""), vec![], true),
        element("r", None, vec![attribute("a", Some("urn:u"), "v")], true),
        element("r", None, vec![attribute("xmlns:p", None, "urn:u")], true),
        element(
            "p:r",
            Some("urn:wrong"),
            vec![attribute(
                "xmlns:p",
                Some("http://www.w3.org/2000/xmlns/"),
                "urn:u",
            )],
            true,
        ),
    ] {
        assert_rejected(&[], &invalid, ErrorKind::Namespace);
    }
    assert_rejected(
        &[root(false)],
        &Event::End(name("r", Some("urn:wrong"))),
        ErrorKind::Namespace,
    );
}

#[test]
fn invalid_operations_emit_nothing_and_poison_later_calls() {
    use ErrorKind::*;
    for (prefix, invalid, expected) in [
        (vec![], Event::End(name("r", None)), MismatchedEndTag),
        (
            vec![root(false)],
            Event::End(name("s", None)),
            MismatchedEndTag,
        ),
        (vec![root(true)], root(true), MultipleRoots),
        (vec![], Event::Text(Cow::Borrowed("x")), TextOutsideRoot),
        (
            vec![root(true)],
            Event::Text(Cow::Borrowed("x")),
            TextOutsideRoot,
        ),
        (vec![], Event::Cdata(Cow::Borrowed("")), TextOutsideRoot),
        (
            vec![root(false)],
            Event::Comment(Cow::Borrowed("a--b")),
            Syntax,
        ),
        (
            vec![root(false)],
            Event::Comment(Cow::Borrowed("a-")),
            Syntax,
        ),
        (
            vec![root(false)],
            Event::Cdata(Cow::Borrowed("a]]>b")),
            Syntax,
        ),
        (
            vec![root(false)],
            Event::ProcessingInstruction {
                target: Cow::Borrowed("p"),
                data: Cow::Borrowed("a?>b"),
            },
            Syntax,
        ),
        (
            vec![],
            Event::ProcessingInstruction {
                target: Cow::Borrowed("p:q"),
                data: Cow::Borrowed(""),
            },
            InvalidName,
        ),
        (
            vec![],
            Event::ProcessingInstruction {
                target: Cow::Borrowed("XML"),
                data: Cow::Borrowed("version='1.0'"),
            },
            InvalidDeclaration,
        ),
        (
            vec![root(false)],
            Event::Text(Cow::Borrowed("\0")),
            InvalidCharacter,
        ),
        (
            vec![],
            element("r", None, vec![attribute("a", None, "\0")], true),
            InvalidCharacter,
        ),
        (
            vec![root(false)],
            Event::ProcessingInstruction {
                target: Cow::Borrowed("p"),
                data: Cow::Borrowed("\0"),
            },
            InvalidCharacter,
        ),
        (
            vec![root(false)],
            Event::Comment(Cow::Borrowed("\u{b}")),
            InvalidCharacter,
        ),
        (
            vec![root(false)],
            Event::Cdata(Cow::Borrowed("\u{fffe}")),
            InvalidCharacter,
        ),
    ] {
        assert_rejected(&prefix, &invalid, expected);
    }
    let declaration = |version, encoding| {
        Event::Declaration(Declaration {
            version,
            encoding,
            standalone: None,
        })
    };
    for (prefix, invalid, expected) in [
        (
            vec![Event::Comment(Cow::Borrowed("before"))],
            declaration("1.0", None),
            InvalidDeclaration,
        ),
        (
            vec![declaration("1.0", None)],
            declaration("1.0", None),
            InvalidDeclaration,
        ),
        (
            vec![root(false)],
            declaration("1.0", None),
            InvalidDeclaration,
        ),
        (vec![], declaration("1.x", None), InvalidDeclaration),
        (
            vec![],
            declaration("1.0", Some("UTF 8")),
            InvalidDeclaration,
        ),
        (
            vec![],
            declaration("1.0", Some("UTF-16")),
            UnsupportedEncoding,
        ),
    ] {
        assert_rejected(&prefix, &invalid, expected);
    }
}

#[test]
fn supplied_name_and_declaration_delimiter_injection_is_rejected() {
    assert_rejected(
        &[],
        &Event::ProcessingInstruction {
            target: Cow::Borrowed("p data"),
            data: Cow::Borrowed(""),
        },
        ErrorKind::InvalidName,
    );
    for declaration in [
        Declaration {
            version: "1.0\" standalone=\"yes",
            encoding: None,
            standalone: None,
        },
        Declaration {
            version: "1.0",
            encoding: Some("UTF-8\" standalone=\"yes"),
            standalone: None,
        },
    ] {
        assert_rejected(
            &[],
            &Event::Declaration(declaration),
            ErrorKind::InvalidDeclaration,
        );
    }
}

#[test]
fn adjacent_text_and_top_level_whitespace_remain_well_formed() {
    let mut writer = Writer::new(Vec::new());
    for event in [
        Event::Text(Cow::Borrowed(" \r\n")),
        root(false),
        Event::Text(Cow::Borrowed("]]")),
        Event::Text(Cow::Borrowed(">")),
        Event::End(name("r", None)),
        Event::Text(Cow::Borrowed("\t\r\n")),
    ] {
        writer.write_event(&event).unwrap();
    }
    writer.finish().unwrap();
    let output = writer.into_inner();
    assert_eq!(output, b" \r\n<r>]]&gt;</r>\t\r\n");
    assert_round_trip(b"<r>]]&gt;</r>", &output);
}

#[test]
fn raw_markup_values_follow_reader_normalization_and_empty_text_is_not_markup() {
    let events = [
        root(false),
        Event::Text(Cow::Borrowed("")),
        Event::Cdata(Cow::Borrowed("a\r\nb\rc")),
        Event::Comment(Cow::Borrowed("a\r\nb\rc")),
        Event::ProcessingInstruction {
            target: Cow::Borrowed("p"),
            data: Cow::Borrowed(" \t\r\na\rb"),
        },
        Event::End(name("r", None)),
    ];
    let mut writer = Writer::new(Vec::new());
    for event in &events {
        writer.write_event(event).unwrap();
    }
    writer.finish().unwrap();
    let output = writer.into_inner();
    assert_eq!(
        output,
        b"<r><![CDATA[a\r\nb\rc]]><!--a\r\nb\rc--><?p  \t\r\na\rb?></r>"
    );
    let expected = b"<r><![CDATA[a\nb\nc]]><!--a\nb\nc--><?p a\nb?></r>";
    assert_round_trip(expected, &output);
    let mut writer = Writer::new(Vec::new());
    writer.write_event(&Event::Text(Cow::Borrowed(""))).unwrap();
    writer.write_event(&root(true)).unwrap();
    writer.write_event(&Event::Text(Cow::Borrowed(""))).unwrap();
    writer.finish().unwrap();
    writer.finish().unwrap();
    assert_eq!(writer.position(), 4);
    assert!(
        matches!(writer.write_event(&root(true)), Err(WriteError::Xml(error)) if error.kind == ErrorKind::InvalidState)
    );
    assert_eq!(writer.into_inner(), b"<r/>");
    for prefix in [vec![], vec![root(false)]] {
        let mut writer = Writer::new(Vec::new());
        for event in prefix {
            writer.write_event(&event).unwrap();
        }
        let expected = if writer.position() == 0 {
            ErrorKind::MissingRoot
        } else {
            ErrorKind::UnexpectedEof
        };
        assert!(matches!(writer.finish(), Err(WriteError::Xml(error)) if error.kind == expected));
    }
}

#[test]
fn generated_values_round_trip_without_dependency_or_recursive_generation() {
    let values = [
        "",
        "plain",
        "<&>\"'",
        "\t\n\r",
        "é中🙂",
        "\u{7f}\u{85}\u{2028}\u{10ffff}",
        "&amp; &#13; ]]> ",
    ];
    for first in values {
        for second in values {
            let events = [
                element("r", None, vec![attribute("a", None, first)], false),
                Event::Text(Cow::Borrowed(second)),
                Event::End(name("r", None)),
            ];
            let mut writer = Writer::new(Vec::new());
            for event in &events {
                writer.write_event(event).unwrap();
            }
            writer.finish().unwrap();
            let output = writer.into_inner();
            let document = Document::parse_bytes(&output).unwrap();
            assert_eq!(document.root_element().attributes()[0].value, first);
            let text: String = document
                .root_element()
                .children()
                .filter_map(|node| node.text())
                .collect();
            assert_eq!(text, second);
            assert_round_trip(&output, &output);
        }
    }
}

#[test]
fn each_limit_is_inclusive_and_counts_encoded_growth() {
    let defaults = WriterLimits::default();
    for (events, limit, boundary) in [
        (vec![root(true)], Limit::OutputBytes, 4),
        (vec![root(true)], Limit::TokenBytes, 4),
        (vec![root(false), root(true)], Limit::Depth, 2),
        (
            vec![element("r", None, vec![attribute("a", None, "v")], true)],
            Limit::Attributes,
            1,
        ),
        (
            vec![root(false), Event::Text(Cow::Borrowed("x"))],
            Limit::Events,
            2,
        ),
        (vec![root(false)], Limit::OwnedBytes, 1),
        (
            vec![root(false), Event::Text(Cow::Borrowed("<&>\r"))],
            Limit::OutputBytes,
            b"<r>&lt;&amp;&gt;&#13;".len(),
        ),
        (
            vec![root(false), Event::Text(Cow::Borrowed("<&>\r"))],
            Limit::TokenBytes,
            b"&lt;&amp;&gt;&#13;".len(),
        ),
        (
            vec![element(
                "r",
                None,
                vec![attribute("a", None, "\t\n\r\"<&")],
                true,
            )],
            Limit::TokenBytes,
            b"<r a=\"&#9;&#10;&#13;&quot;&lt;&amp;\"/>".len(),
        ),
    ] {
        for value in [boundary - 1, boundary, boundary + 1] {
            let mut limits = defaults;
            match limit {
                Limit::OutputBytes => limits.max_output_bytes = value,
                Limit::TokenBytes => limits.max_token_bytes = value,
                Limit::Depth => limits.max_depth = value,
                Limit::Attributes => limits.max_attributes = value,
                Limit::Events => limits.max_events = value,
                Limit::OwnedBytes => limits.max_owned_bytes = value,
                _ => unreachable!(),
            }
            let mut writer = Writer::with_limits(Vec::new(), limits);
            let result = events
                .iter()
                .try_for_each(|event| writer.write_event(event));
            if value < boundary {
                assert!(
                    matches!(result, Err(WriteError::Xml(error)) if error.kind == ErrorKind::LimitExceeded(limit)),
                    "{limit:?}={value}: {result:?}"
                );
            } else {
                result.unwrap_or_else(|error| panic!("{limit:?}={value}: {error}"));
            }
        }
    }
    let mut writer = Writer::with_limits(Vec::new(), defaults.max_output_bytes(3));
    writer.write_event(&root(true)).unwrap_err();
    assert_eq!(writer.position(), 0);
    assert!(writer.into_inner().is_empty());
}
