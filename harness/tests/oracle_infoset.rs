#[path = "../examples/support/infoset.rs"]
mod infoset;

use infoset::{Projection, ProjectionError};

#[test]
fn utf16_projection_storage_exhaustion_is_an_acceptance_only_ceiling() {
    use std::ops::ControlFlow;
    use yaxml::{ErrorKind, Limit, OwnedDocument, PushParser};

    let mut xml = format!("<p:r xmlns:p='{}'", "u".repeat(327_680));
    for index in 0..80 {
        xml.push_str(&format!(" p:a{index}=''"));
    }
    xml.push_str("/>");
    let bytes: Vec<_> = std::iter::once(0xfeff)
        .chain(xml.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut parser = PushParser::new();
    parser
        .feed(&bytes, |_, _| ControlFlow::Continue(()))
        .unwrap();
    parser.finish(|_, _| ControlFlow::Continue(())).unwrap();
    assert_eq!(
        OwnedDocument::parse_bytes(&bytes).unwrap_err().kind,
        ErrorKind::LimitExceeded(Limit::OwnedBytes)
    );
    assert!(matches!(
        Projection::from_input(&bytes),
        Err(ProjectionError::Limit("projection storage bytes"))
    ));
}

fn utf16(input: &str, little_endian: bool, bom: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    if bom {
        bytes.extend_from_slice(if little_endian {
            &[0xff, 0xfe]
        } else {
            &[0xfe, 0xff]
        });
    }
    for unit in input.encode_utf16() {
        bytes.extend_from_slice(&if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}

fn fields(output: &[u8]) -> Vec<&str> {
    let mut remainder = std::str::from_utf8(output)
        .unwrap()
        .strip_suffix('\n')
        .unwrap();
    let mut fields = Vec::new();
    while !remainder.is_empty() {
        let (length, value) = remainder.split_once(':').unwrap();
        let length: usize = length.parse().unwrap();
        let end = value
            .char_indices()
            .nth(length)
            .map_or(value.len(), |(i, _)| i);
        assert_eq!(value[..end].chars().count(), length);
        fields.push(&value[..end]);
        remainder = &value[end..];
    }
    fields
}

#[test]
fn minimal_projection_has_explicit_query_and_framing() {
    let projection = Projection::from_input(b"<r/>").unwrap();
    assert_eq!(projection.expected(), b"1:11:11:11:r1:r0:0:1:0\n");
    let path = "/descendant::node()[not(self::text()[string-length(.)=0])][1]";
    let expressions = [
        "count(/descendant::node()[not(self::text()[string-length(.)=0])])".to_owned(),
        format!(
            "count({path}/self::*)+2*count({path}/self::text())+3*count({path}/self::comment())+4*count({path}/self::processing-instruction())"
        ),
        format!("count({path}/ancestor::node())"),
        format!("name({path})"),
        format!("local-name({path})"),
        format!("namespace-uri({path})"),
        "''".to_owned(),
        format!("count({path}/@*)"),
    ];
    let expected = expressions
        .iter()
        .map(|expression| format!("string-length(string({expression})),':',string({expression})"))
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(projection.query(), format!("concat({expected})"));
    assert_eq!(infoset::LIBXML2_VERSION, "2.15.4");
    assert_eq!(infoset::LIBXML2_COMPILED_VERSION, "21504");
}

#[test]
fn normalized_values_and_unicode_have_independent_goldens() {
    let projection = Projection::from_input(
        "<r a='&#xA;&#xD;&#x9;é🙂'>x\r\ny&#xD;é<![CDATA[中\r\nz]]></r>".as_bytes(),
    )
    .unwrap();
    assert_eq!(
        fields(projection.expected()),
        [
            "2",
            "1",
            "1",
            "r",
            "r",
            "",
            "",
            "1",
            "a",
            "a",
            "",
            "\n\r\té🙂",
            "2",
            "2",
            "",
            "",
            "",
            "x\ny\ré中\nz",
            "0"
        ]
    );
    // Five Unicode scalar values, despite nine UTF-8 bytes in the value.
    assert!(
        projection
            .expected()
            .windows("5:\n\r\té🙂".len())
            .any(|bytes| bytes == "5:\n\r\té🙂".as_bytes())
    );
}

#[test]
fn text_grouping_matches_nocdata_with_empty_sections_removed() {
    let cases = [
        ("<r>a<![CDATA[b]]>c</r>", "<r>abc</r>"),
        (
            "<r>a<![CDATA[]]>b<![CDATA[c]]><![CDATA[]]>d</r>",
            "<r>abcd</r>",
        ),
        ("<r><![CDATA[]]><a/><![CDATA[]]></r>", "<r><a/></r>"),
        ("<r><![CDATA[]]></r>", "<r/>"),
    ];
    for (input, equivalent) in cases {
        let actual = Projection::from_input(input.as_bytes()).unwrap();
        let expected = Projection::from_input(equivalent.as_bytes()).unwrap();
        assert_eq!(actual.expected(), expected.expected(), "{input}");
        assert_eq!(actual.query(), expected.query(), "{input}");
    }
    let projection = Projection::from_input(b"<r>a<x>b</x>c<!--d-->e</r>").unwrap();
    assert_eq!(
        fields(projection.expected()),
        [
            "7", "1", "1", "r", "r", "", "", "0", "2", "2", "", "", "", "a", "0", "1", "2", "x",
            "x", "", "", "0", "2", "3", "", "", "", "b", "0", "2", "2", "", "", "", "c", "0", "3",
            "2", "", "", "", "d", "0", "2", "2", "", "", "", "e", "0"
        ]
    );
}

#[test]
fn namespaces_attributes_and_node_kinds_are_preserved() {
    let projection = Projection::from_input(
        b"<?xml version='1.0'?><!--before--><?pi data?><r z='first' xmlns='urn:r' xmlns:p='urn:p' p:b='second' a='third'><p:q xmlns=''/></r><!--after-->",
    ).unwrap();
    assert_eq!(
        fields(projection.expected()),
        [
            "5", "3", "1", "", "", "", "before", "0", "4", "1", "pi", "pi", "", "data", "0", "1",
            "1", "r", "r", "urn:r", "", "3", "z", "z", "", "first", "p:b", "b", "urn:p", "second",
            "a", "a", "", "third", "1", "2", "p:q", "q", "urn:p", "", "0", "3", "1", "", "", "",
            "after", "0"
        ]
    );
    assert!(!projection.query().contains("urn:p"));
    assert!(!projection.query().contains("a:b"));
}

#[test]
fn utf16_views_preserve_the_utf8_query_values_and_text_grouping() {
    let body = "<!--é--><?空 data?><é:r xmlns:é='urn:ü' é:a='&#9;🙂' a='&quot;'>A\r\n<![CDATA[é]]><![CDATA[]]>🙂<n/>B</é:r><!--tail-->";
    let expected = Projection::from_input(body.as_bytes()).unwrap();
    assert_eq!(
        fields(expected.expected()),
        [
            "7",
            "3",
            "1",
            "",
            "",
            "",
            "é",
            "0",
            "4",
            "1",
            "空",
            "空",
            "",
            "data",
            "0",
            "1",
            "1",
            "é:r",
            "r",
            "urn:ü",
            "",
            "2",
            "é:a",
            "a",
            "urn:ü",
            "\t🙂",
            "a",
            "a",
            "",
            "\"",
            "2",
            "2",
            "",
            "",
            "",
            "A\né🙂",
            "0",
            "1",
            "2",
            "n",
            "n",
            "",
            "",
            "0",
            "2",
            "2",
            "",
            "",
            "",
            "B",
            "0",
            "3",
            "1",
            "",
            "",
            "",
            "tail",
            "0"
        ]
    );
    for (little_endian, bom, label) in [
        (true, true, None),
        (false, true, None),
        (true, true, Some("UTF-16")),
        (false, true, Some("uTf-16")),
        (true, true, Some("UTF-16LE")),
        (false, true, Some("UTF-16BE")),
        (true, false, Some("uTf-16Le")),
        (false, false, Some("UTF-16BE")),
    ] {
        let input = match label {
            Some(label) => format!("<?xml version='1.0' encoding='{label}'?>{body}"),
            None => body.to_owned(),
        };
        let input = utf16(&input, little_endian, bom);
        let actual = Projection::from_input(&input).unwrap();
        assert_eq!(actual.query(), expected.query());
        assert_eq!(actual.expected(), expected.expected());
    }
    for body in [
        "<r/>",
        "<r><![CDATA[]]></r>",
        "<r>a<![CDATA[b]]>c<![CDATA[d]]></r>",
        "<r>a<x>b</x>c<!--d-->e</r>",
    ] {
        let expected = Projection::from_input(body.as_bytes()).unwrap();
        for little_endian in [false, true] {
            let actual = Projection::from_input(&utf16(body, little_endian, true)).unwrap();
            assert_eq!(actual.query(), expected.query());
            assert_eq!(actual.expected(), expected.expected());
        }
    }
}

#[test]
fn malformed_utf16_and_encoding_disagreements_are_xml_errors() {
    use yaxml::ErrorKind;
    for little_endian in [false, true] {
        for units in [&[0xdc00][..], &[0xd800], &[0xd800, 0x3c]] {
            let mut input = utf16("<r>", little_endian, true);
            for unit in units {
                let unit: u16 = *unit;
                input.extend_from_slice(&if little_endian {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            let error = Projection::from_input(&input).unwrap_err();
            assert!(
                matches!(error, ProjectionError::Xml(error) if error.kind == ErrorKind::InvalidUtf16 && error.offset == 8)
            );
        }
        let mut odd = utf16("<r>", little_endian, true);
        odd.push(0);
        let error = Projection::from_input(&odd).unwrap_err();
        assert!(
            matches!(error, ProjectionError::Xml(error) if error.kind == ErrorKind::InvalidUtf16 && error.offset == 8)
        );
        for (label, bom, kind) in [
            ("UTF 16", true, ErrorKind::InvalidDeclaration),
            ("16UTF", false, ErrorKind::InvalidDeclaration),
        ] {
            let input = utf16(
                &format!("<?xml version='1.0' encoding='{label}'?><r/>"),
                little_endian,
                bom,
            );
            let error = Projection::from_input(&input).unwrap_err();
            assert!(
                matches!(error, ProjectionError::Xml(error) if error.kind == kind && error.offset <= input.len())
            );
            assert!(std::error::Error::source(&error).is_some());
        }
        let error =
            Projection::from_input(&utf16("\u{feff}<r/>", little_endian, true)).unwrap_err();
        assert!(
            matches!(error, ProjectionError::Xml(error) if error.kind == ErrorKind::TextOutsideRoot)
        );
    }
}

#[test]
fn malformed_and_unsupported_documents_are_errors() {
    for input in [
        b"".as_slice(),
        b"<r>",
        b"<!DOCTYPE r><r>&missing;</r>",
        b"<r/>\0",
    ] {
        let error = Projection::from_input(input).unwrap_err();
        assert!(
            matches!(error, ProjectionError::Xml(_)),
            "{input:?}: {error}"
        );
        assert!(std::error::Error::source(&error).is_some());
        assert!(error.to_string().starts_with("cannot project XML: "));
    }
}

#[test]
fn framing_detects_semantic_changes_without_querying_source_values() {
    let baseline =
        Projection::from_input(b"<r a='first' xmlns:p='urn:p'><p:n>text</p:n></r>").unwrap();
    let spelling = Projection::from_input(
        b"<r xmlns:p=\"urn:p\" a = \"first\"><p:n><![CDATA[te]]>xt</p:n></r>",
    )
    .unwrap();
    assert_eq!(baseline.query(), spelling.query());
    assert_eq!(baseline.expected(), spelling.expected());
    for changed in [
        "<r a='second' xmlns:p='urn:p'><p:n>text</p:n></r>",
        "<r a='first' xmlns:p='urn:q'><p:n>text</p:n></r>",
        "<r a='first' xmlns:p='urn:p'><p:n>TEXT</p:n></r>",
    ] {
        let changed = Projection::from_input(changed.as_bytes()).unwrap();
        assert_eq!(baseline.query(), changed.query());
        assert_ne!(baseline.expected(), changed.expected());
    }
    let extra = Projection::from_input(b"<r a='first' xmlns:p='urn:p'><p:n>text</p:n><extra/></r>")
        .unwrap();
    assert_eq!(fields(baseline.expected())[0], "3");
    assert_eq!(fields(extra.expected())[0], "4");
    assert_ne!(baseline.expected(), extra.expected());
    let allocation = ProjectionError::Allocation;
    assert_eq!(allocation.to_string(), "cannot allocate infoset projection");
    assert!(std::error::Error::source(&allocation).is_none());
}

#[test]
fn projection_ceilings_are_errors_instead_of_partial_comparisons() {
    let many_nodes = format!("<r>{}</r>", "<n/>".repeat(infoset::MAX_NODES));
    let mut many_attributes = String::from("<r>");
    for _ in 0..5 {
        many_attributes.push_str("<n");
        for index in 0..1024 {
            many_attributes.push_str(&format!(" a{index}=''"));
        }
        many_attributes.push_str("/>");
    }
    many_attributes.push_str("</r>");
    let mut large_query = String::from("<r");
    for index in 0..128 {
        large_query.push_str(&format!(" a{index}=''"));
    }
    large_query.push_str("/>");
    let large_output = format!(
        "<r>{}</r>",
        // Encoded tokens fit the raw ceiling; coalescing keeps the generated
        // query below its own ceiling while exceeding the output bound.
        format!("<![CDATA[{}]]>", "a".repeat(450_000)).repeat(20)
    );
    for (input, ceiling) in [
        (many_nodes, "node count"),
        (many_attributes, "attribute count"),
        (large_query, "query bytes"),
        (large_output, "expected output bytes"),
    ] {
        for input in [
            input.as_bytes().to_vec(),
            utf16(&input, false, true),
            utf16(&input, true, true),
        ] {
            let error = Projection::from_input(&input).unwrap_err();
            assert!(
                matches!(error, ProjectionError::Limit(name) if name == ceiling),
                "{error}"
            );
            assert_eq!(
                error.to_string(),
                format!("infoset projection exceeds {ceiling} ceiling")
            );
            assert!(std::error::Error::source(&error).is_none());
        }
    }
    assert_eq!(infoset::MAX_QUERY_BYTES, 64 * 1024);
    assert_eq!(infoset::MAX_ATTRIBUTES, 4096);
    assert_eq!(infoset::MAX_EXPECTED_BYTES, 8 * 1024 * 1024);
}

#[test]
fn framing_total_is_bounded_without_text_coalescing() {
    let input = format!(
        "<r>{}</r>",
        format!("<n>{}</n>", "a".repeat(900_000)).repeat(10)
    );
    assert!(matches!(
        Projection::from_input(input.as_bytes()),
        Err(ProjectionError::Limit("expected output bytes"))
    ));
}
