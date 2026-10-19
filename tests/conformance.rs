#![forbid(unsafe_code)]

use std::ops::ControlFlow;

use yaxml::{Declaration, Document, Element, ErrorKind, Event, NodeKind, PushParser, Reader};

// Expectations come from the XML and Namespaces grammars, rather than another
// frontend of this crate. See docs/conformance.md for the scope and references.
fn slice_result(input: &[u8]) -> Result<(), ErrorKind> {
    let mut reader = Reader::new(input).map_err(|error| error.kind)?;
    while reader.read_event().map_err(|error| error.kind)?.is_some() {}
    Ok(())
}

fn push_result(input: &[u8], chunks: &[&[u8]]) -> Result<(), ErrorKind> {
    let mut parser = PushParser::new();
    for chunk in chunks {
        parser
            .feed(chunk, |_, _| ControlFlow::Continue(()))
            .map_err(|error| error.kind)?;
    }
    assert_eq!(parser.position(), input.len());
    parser
        .finish(|_, _| ControlFlow::Continue(()))
        .map_err(|error| error.kind)
}

fn check(input: &str, expected: Result<(), ErrorKind>) {
    let input = input.as_bytes();
    assert_eq!(slice_result(input), expected, "Reader: {input:?}");
    assert_eq!(
        Document::parse_bytes(input)
            .map(|_| ())
            .map_err(|error| error.kind),
        expected,
        "DOM: {input:?}"
    );
    for size in [1, 2, 3, 5, 8, 13, input.len().max(1)] {
        let chunks: Vec<_> = input.chunks(size).collect();
        assert_eq!(
            push_result(input, &chunks),
            expected,
            "Push size {size}: {input:?}"
        );
    }
    // All two-way byte splits include UTF-8 interior boundaries and empty feeds.
    if input.len() <= 40 {
        for split in 0..=input.len() {
            assert_eq!(
                push_result(input, &[&input[..split], &[], &input[split..]]),
                expected,
                "Push split {split}: {input:?}"
            );
        }
    }
}

#[test]
fn document_and_markup_grammar() {
    use ErrorKind::*;
    for (input, expected) in [
        ("<r a = '\"' b = \"'\"></r \t\r\n>", Ok(())),
        ("<r a='/> >' b='?>!--'/>", Ok(())),
        ("<r><![CDATA[]]><![CDATA[]]]]></r>", Ok(())),
        ("<r>]]&gt;<![CDATA[]]]]>]]&#62;</r>", Ok(())),
        ("<!-- - --><?xml-stylesheet?><r/><?xmlfoo?>", Ok(())),
        ("<?p ? >??><r/>", Ok(())),
        ("<r><!--<? & %entity; ]]>--><?p <&%entity;]]>?></r>", Ok(())),
        ("<r a='&lt;' b='&quot;' c='&apos;'>&lt;r/&gt;</r>", Ok(())),
        ("<r/>\r\n<!--tail--><?tail?>\t", Ok(())),
        ("<r>&amp;amp;&#38;lt;</r>", Ok(())),
        ("<r/>\u{a0}", Err(TextOutsideRoot)),
        ("\u{85}<r/>", Err(TextOutsideRoot)),
        ("<r/>\u{feff}", Err(TextOutsideRoot)),
        ("<r/>&#x20;", Err(TextOutsideRoot)),
        ("<r/><!--a--b-->", Err(Syntax)),
        ("<r><![cdata[x]]></r>", Err(Syntax)),
        ("<r><!CDATA[x]]></r>", Err(Syntax)),
        ("<r><!--x--!></r>", Err(UnexpectedEof)),
        ("<r><!--x---></r>", Err(Syntax)),
        ("<r><![CDATA[x]]>]]></r>", Err(Syntax)),
        ("<r><?p? ></r>", Err(UnexpectedEof)),
        ("<r><?p/data?></r>", Err(InvalidName)),
        ("<r><?:p?></r>", Err(InvalidName)),
        ("<r><?p:q?></r>", Err(InvalidName)),
        ("<r>< /r>", Err(InvalidName)),
        ("<r></r/>", Err(Syntax)),
        ("<r a='1'/ >", Err(Syntax)),
        ("<r a='1' /x>", Err(InvalidName)),
        ("<r a='1' a='2'></r>", Err(DuplicateAttribute)),
        ("<r a='&lt'></r>", Err(InvalidReference)),
        ("<r a='&#x20 '></r>", Err(InvalidReference)),
        ("<r>&#x 20;</r>", Err(InvalidReference)),
        ("<r>&#x20_;</r>", Err(InvalidReference)),
        ("<r>&#１２;</r>", Err(InvalidReference)),
        ("<r>&Amp;</r>", Err(UnknownEntity)),
        ("<r>&apos ;</r>", Err(UnknownEntity)),
        ("<r>&#xFFFF;</r>", Err(InvalidReference)),
        ("<r>&#x00000000000000000000000041;</r>", Ok(())),
        ("<r>&#00000000000000000000000065;</r>", Ok(())),
        ("<r>\u{7f}\u{85}\u{fdd0}\u{1fffe}\u{10ffff}</r>", Ok(())),
        ("<r>&#xFDD0;&#x1FFFE;&#x10FFFF;</r>", Ok(())),
        ("<r>&#x1F;</r>", Err(InvalidReference)),
        ("<r>\u{1f}</r>", Err(InvalidCharacter)),
    ] {
        check(input, expected);
    }
}

#[test]
fn declaration_grammar_and_forward_versions() {
    use ErrorKind::*;
    for (input, expected) in [
        ("<?xml\tversion\r=\n'1.0'\t?><r/>", Ok(())),
        ("<?xml version='1.0' encoding='uTf-8'?><r/>", Ok(())),
        ("<?xml version='1.0' standalone='no' ?><r/>", Ok(())),
        ("<?xml version='1.1'?><r/>", Ok(())),
        ("<?xml version='1.1'?><r>&#x1;</r>", Err(InvalidReference)),
        ("<?xml version='1.2'?><r/>", Ok(())),
        ("<?xml version='1.00'?><r/>", Ok(())),
        ("<?xml version='1.2345678901234567890'?><r/>", Ok(())),
        ("<?xml version='1.'?><r/>", Err(InvalidDeclaration)),
        ("<?xml version='01.0'?><r/>", Err(InvalidDeclaration)),
        ("<?xml version='2.0'?><r/>", Err(InvalidDeclaration)),
        ("<?xml version='1.0 '?><r/>", Err(InvalidDeclaration)),
        (
            "<?xml version='1.0'encoding='UTF-8'?><r/>",
            Err(InvalidDeclaration),
        ),
        (
            "<?xml version='1.0' Encoding='UTF-8'?><r/>",
            Err(InvalidDeclaration),
        ),
        (
            "<?xml version='1.0' encoding=''?><r/>",
            Err(InvalidDeclaration),
        ),
        (
            "<?xml version='1.0' encoding='8UTF'?><r/>",
            Err(InvalidDeclaration),
        ),
        (
            "<?xml version='1.0' encoding='UTF 8'?><r/>",
            Err(InvalidDeclaration),
        ),
        (
            "<?xml version='1.0' encoding='UTF:8'?><r/>",
            Err(InvalidDeclaration),
        ),
        (
            "<?xml version='1.0' encoding='UTF&#45;8'?><r/>",
            Err(InvalidDeclaration),
        ),
        (
            "<?xml version='1.0' standalone='YES'?><r/>",
            Err(InvalidDeclaration),
        ),
        ("<!--x--><?xml version='1.0'?><r/>", Err(InvalidDeclaration)),
        ("<?pi?><?xml version='1.0'?><r/>", Err(InvalidDeclaration)),
        ("<r/><?xml version='1.0'?>", Err(InvalidDeclaration)),
        ("<?xmL?><r/>", Err(InvalidDeclaration)),
        (
            "<?xml version='1.0' encoding='UTF-16'?><r/>",
            Err(UnsupportedEncoding),
        ),
        (
            "<?xml version='1.0' encoding='x-private.1_2'?><r/>",
            Err(UnsupportedEncoding),
        ),
    ] {
        check(input, expected);
    }
}

#[test]
fn namespace_constraints_are_case_sensitive_and_scope_local() {
    use ErrorKind::*;
    for (input, expected) in [
        ("<xml:r xml:a='v'/>", Ok(())),
        ("<xmlns/>", Ok(())),
        ("<p:r p:a='1' xmlns:p='urn:u'/>", Ok(())),
        ("<r xmlns:p='urn:u' xmlns='urn:u' a='1' p:a='2'/>", Ok(())),
        ("<r xmlns:XML='urn:u'><XML:a/></r>", Ok(())),
        ("<r xmlns:xmlfoo='urn:u'><xmlfoo:a/></r>", Ok(())),
        ("<r xmlns:xMl='urn:u'><xMl:a/></r>", Ok(())),
        ("<r xmlns:Xmlns='urn:u' Xmlns:a='1'/>", Ok(())),
        (
            "<r xmlns:p='urn:%61' xmlns:q='urn:a' p:a='1' q:a='2'/>",
            Ok(()),
        ),
        (
            "<r xmlns:p='urn:A' xmlns:q='urn:a' p:a='1' q:a='2'/>",
            Ok(()),
        ),
        (
            "<r xmlns:p='urn:u'><p:a xmlns:p='urn:v'/><p:a/></r>",
            Ok(()),
        ),
        ("<r xmlns:p='urn:u'><a xmlns=''/><p:a/></r>", Ok(())),
        ("<r xmlns='urn:u'><a xmlns=''><b/></a><c/></r>", Ok(())),
        ("<XML:r/>", Err(Namespace)),
        ("<r XML:a='v'/>", Err(Namespace)),
        ("<xmlns:r xmlns:xmlns='urn:u'/>", Err(Namespace)),
        (
            "<r xmlns:p='urn:u' xmlns:p='urn:u'/>",
            Err(DuplicateAttribute),
        ),
        ("<r xmlns='urn:u' xmlns='urn:v'/>", Err(DuplicateAttribute)),
        (
            "<r xmlns:p='urn:u' xmlns:q='urn:&#x75;' p:a='1' q:a='2'/>",
            Err(DuplicateAttribute),
        ),
        (
            "<r xmlns:p='urn:u'><a xmlns:q='urn:u' p:a='1' q:a='2'/></r>",
            Err(DuplicateAttribute),
        ),
        (
            "<r xmlns:xml='http://www.w3.org/XML/1998/namespac&#x65;' xml:a='v'/>",
            Ok(()),
        ),
        (
            "<r xmlns:p='http://www.w3.org/XML/1998/namespac&#101;'/>",
            Err(Namespace),
        ),
        (
            "<r xmlns:xml='http://www.w3.org/XML/1998/namespace '/>",
            Err(Namespace),
        ),
        (
            "<r xmlns:p='http://www.w3.org/2000/xmlns&#47;'/>",
            Err(Namespace),
        ),
        ("<r xmlns:p=''/>", Err(Namespace)),
        ("<r xmlns:p='urn:u'><a xmlns:p=''/></r>", Err(Namespace)),
        (
            "<p:a xmlns:p='urn:u' xmlns:q='urn:u'></q:a>",
            Err(MismatchedEndTag),
        ),
        (
            "<r xmlns:p='urn:u'><a xmlns:q='urn:u'/><q:a/></r>",
            Err(Namespace),
        ),
    ] {
        check(input, expected);
    }
}

#[test]
fn declaration_values_preserve_source_spelling() {
    for (input, expected) in [
        (
            "<?xml version='1.00'?><r/>",
            Declaration {
                version: "1.00",
                encoding: None,
                standalone: None,
            },
        ),
        (
            "<?xml version='1.2' encoding='uTf-8' standalone='yes'?><r/>",
            Declaration {
                version: "1.2",
                encoding: Some("uTf-8"),
                standalone: Some(true),
            },
        ),
    ] {
        let mut reader = Reader::new(input.as_bytes()).unwrap();
        assert_eq!(
            reader.read_event().unwrap(),
            Some(Event::Declaration(expected))
        );
        assert_eq!(
            Document::parse(input).unwrap().declaration(),
            Some(&expected)
        );
        let mut parser = PushParser::new();
        let mut declarations = 0;
        for chunk in input.as_bytes().chunks(1) {
            parser
                .feed(chunk, |event, _| {
                    if let Event::Declaration(actual) = event {
                        assert_eq!(actual, expected);
                        declarations += 1;
                    }
                    ControlFlow::Continue(())
                })
                .unwrap();
        }
        parser.finish(|_, _| ControlFlow::Continue(())).unwrap();
        assert_eq!(declarations, 1);
    }
}

#[test]
fn ncname_boundary_classes_in_all_name_positions() {
    // Representative starts, continuation-only characters, and gaps. The same
    // spelling must obey NCName in prefixes, local names, attributes, and PIs.
    for (character, start, continuation) in [
        ('_', true, true),
        ('\u{c0}', true, true),
        ('\u{2ff}', true, true),
        ('\u{370}', true, true),
        ('\u{200c}', true, true),
        ('\u{fdf0}', true, true),
        ('\u{10000}', true, true),
        ('\u{effff}', true, true),
        ('0', false, true),
        ('-', false, true),
        ('.', false, true),
        ('\u{b7}', false, true),
        ('\u{300}', false, true),
        ('\u{36f}', false, true),
        ('\u{203f}', false, true),
        ('\u{2040}', false, true),
        ('\u{37e}', false, false),
        ('\u{200b}', false, false),
        ('\u{2041}', false, false),
        ('\u{fdd0}', false, false),
        ('\u{f0000}', false, false),
    ] {
        for (name, accepted) in [
            (character.to_string(), start),
            (format!("n{character}"), continuation),
        ] {
            let expected = if accepted {
                Ok(())
            } else {
                Err(ErrorKind::InvalidName)
            };
            for input in [
                format!("<{name}/>"),
                format!("<r {name}='v'/>"),
                format!("<p:{name} xmlns:p='urn:u'/>"),
                format!("<{name}:r xmlns:{name}='urn:u'/>"),
                format!("<?{name}?><r/>"),
            ] {
                check(&input, expected);
            }
        }
    }
}

#[derive(Debug, PartialEq)]
enum Value {
    Element(
        String,
        Option<String>,
        Vec<(String, Option<String>, String)>,
    ),
    Text(String),
    Cdata(String),
    Comment(String),
    Pi(String, String),
}

fn element(value: &Element<'_>) -> Value {
    Value::Element(
        value.name.qualified().into(),
        value.name.namespace().map(Into::into),
        value
            .attributes
            .iter()
            .map(|attribute| {
                (
                    attribute.name.qualified().into(),
                    attribute.name.namespace().map(Into::into),
                    attribute.value.to_string(),
                )
            })
            .collect(),
    )
}

fn capture(event: Event<'_>) -> Option<Value> {
    match event {
        Event::Start(value) | Event::Empty(value) => Some(element(&value)),
        Event::Text(value) => Some(Value::Text(value.into_owned())),
        Event::Cdata(value) => Some(Value::Cdata(value.into_owned())),
        Event::Comment(value) => Some(Value::Comment(value.into_owned())),
        Event::ProcessingInstruction { target, data } => {
            Some(Value::Pi(target.into(), data.into_owned()))
        }
        Event::Declaration(_)
        | Event::Doctype(_)
        | Event::End(_)
        | Event::EntityStart(_)
        | Event::EntityEnd => None,
    }
}

#[test]
fn logical_values_have_spec_derived_goldens() {
    const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
    let cases = [
        (
            "<r a='  A\r\n\r\t\nB  &#xD;&#xA;&#x9;&#x20;'>&amp;lt;&#38;amp;]]&gt;\r\n\r\t</r>",
            vec![
                Value::Element(
                    "r".into(),
                    None,
                    vec![("a".into(), None, "  A    B  \r\n\t ".into())],
                ),
                Value::Text("&lt;&amp;]]>\n\n\t".into()),
            ],
        ),
        (
            "<r><![CDATA[&amp;&#13;\r\n\r]]><!--&amp;&#13;\r\n\r--><?p &amp;&#13;\r\n\r?></r>",
            vec![
                Value::Element("r".into(), None, vec![]),
                Value::Cdata("&amp;&#13;\n\n".into()),
                Value::Comment("&amp;&#13;\n\n".into()),
                Value::Pi("p".into(), "&amp;&#13;\n\n".into()),
            ],
        ),
        (
            "<r xmlns='urn:u' xmlns:p='urn:&#x75;' a='1' p:a='2'><c xmlns=''/><p:c/></r>",
            vec![
                Value::Element(
                    "r".into(),
                    Some("urn:u".into()),
                    vec![
                        ("xmlns".into(), Some(XMLNS.into()), "urn:u".into()),
                        ("xmlns:p".into(), Some(XMLNS.into()), "urn:u".into()),
                        ("a".into(), None, "1".into()),
                        ("p:a".into(), Some("urn:u".into()), "2".into()),
                    ],
                ),
                Value::Element(
                    "c".into(),
                    None,
                    vec![("xmlns".into(), Some(XMLNS.into()), "".into())],
                ),
                Value::Element("p:c".into(), Some("urn:u".into()), vec![]),
            ],
        ),
        (
            "<r a='é e\u{301}' b='&#xE9; e&#x301;'>é e\u{301}&#xE9;e&#x301;\u{85}\u{2028}</r>",
            vec![
                Value::Element(
                    "r".into(),
                    None,
                    vec![
                        ("a".into(), None, "é e\u{301}".into()),
                        ("b".into(), None, "é e\u{301}".into()),
                    ],
                ),
                Value::Text("é e\u{301}ée\u{301}\u{85}\u{2028}".into()),
            ],
        ),
    ];
    for (input, expected) in cases {
        let mut reader = Reader::new(input.as_bytes()).unwrap();
        let mut values = Vec::new();
        while let Some(event) = reader.read_event().unwrap() {
            values.extend(capture(event));
        }
        assert_eq!(values, expected, "Reader: {input:?}");
        let document = Document::parse(input).unwrap();
        let values: Vec<_> = document
            .nodes()
            .filter_map(|node| match node.kind() {
                NodeKind::Element(value) => Some(element(value)),
                NodeKind::Text(value) => Some(Value::Text(value.to_string())),
                NodeKind::Cdata(value) => Some(Value::Cdata(value.to_string())),
                NodeKind::Comment(value) => Some(Value::Comment(value.to_string())),
                NodeKind::ProcessingInstruction { target, data } => {
                    Some(Value::Pi(target.to_string(), data.to_string()))
                }
                NodeKind::Document | NodeKind::Doctype(_) | NodeKind::EntityRef(_) => None,
            })
            .collect();
        assert_eq!(values, expected, "DOM: {input:?}");
        for size in [1, 2, 3, 5, 13, input.len()] {
            let mut parser = PushParser::new();
            let mut values = Vec::new();
            for chunk in input.as_bytes().chunks(size) {
                parser
                    .feed(chunk, |event, _| {
                        values.extend(capture(event));
                        ControlFlow::Continue(())
                    })
                    .unwrap();
            }
            parser
                .finish(|event, _| {
                    values.extend(capture(event));
                    ControlFlow::Continue(())
                })
                .unwrap();
            assert_eq!(values, expected, "Push size {size}: {input:?}");
        }
    }
}

#[test]
fn internal_subsets_are_processed_and_external_subsets_are_not_read() {
    for input in [
        "<!DOCTYPE r SYSTEM 'urn:external'><r/>",
        "<!DOCTYPE r [<!ELEMENT r EMPTY>]><r/>",
        "<!DOCTYPE r [<!ENTITY e 'value'>]><r>&e;</r>",
        "<!DOCTYPE r SYSTEM 'urn:external'><r>&declared-externally;</r>",
    ] {
        check(input, Ok(()));
    }
    for (input, kind) in [
        ("<!DOCTYPE r [<!ELEMENT r>]><r/>", ErrorKind::InvalidDtd),
        ("<!DOCTYPE r><r>&missing;</r>", ErrorKind::UnknownEntity),
        (
            "<!DOCTYPE r [<!ENTITY e '&e;'>]><r>&e;</r>",
            ErrorKind::EntityLoop,
        ),
        ("<r/><!DOCTYPE r>", ErrorKind::Syntax),
    ] {
        check(input, Err(kind));
    }
}
