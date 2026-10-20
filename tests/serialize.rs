#![forbid(unsafe_code)]

use yaxml::{
    Document, ErrorKind, Limit, Limits, Name, NodeKind, OwnedDocument, OwnedNodeKind, WriteError,
    Writer, WriterLimits,
};

#[derive(Debug, Eq, PartialEq)]
struct NameValue(String, Option<String>, String, Option<String>);

fn name(value: &Name<'_>) -> NameValue {
    NameValue(
        value.qualified().into(),
        value.prefix().map(str::to_owned),
        value.local().into(),
        value.namespace().map(str::to_owned),
    )
}

#[derive(Debug, Eq, PartialEq)]
struct LogicalNode {
    kind: &'static str,
    name: Option<NameValue>,
    attributes: Vec<(NameValue, String)>,
    text: Option<String>,
    target: Option<String>,
    children: usize,
}

#[derive(Debug, Eq, PartialEq)]
struct LogicalDocument {
    declaration: Option<(String, Option<String>, Option<bool>)>,
    nodes: Vec<LogicalNode>,
}

fn borrowed_values(document: &Document<'_>) -> LogicalDocument {
    LogicalDocument {
        declaration: document.declaration().map(|d| {
            (
                d.version.into(),
                d.encoding.map(str::to_owned),
                d.standalone,
            )
        }),
        nodes: document
            .nodes()
            .map(|node| LogicalNode {
                kind: match node.kind() {
                    NodeKind::Document => "document",
                    NodeKind::Doctype(_) => "doctype",
                    NodeKind::Element(_) => "element",
                    NodeKind::Text(_) => "text",
                    NodeKind::Cdata(_) => "cdata",
                    NodeKind::Comment(_) => "comment",
                    NodeKind::ProcessingInstruction { .. } => "pi",
                    NodeKind::EntityRef(_) => "entity-ref",
                },
                name: node.name().map(name),
                attributes: node
                    .attributes()
                    .iter()
                    .map(|a| (name(&a.name), a.value.to_string()))
                    .collect(),
                text: node.text().map(str::to_owned),
                target: match node.kind() {
                    NodeKind::ProcessingInstruction { target, .. } => Some(target.to_string()),
                    _ => None,
                },
                children: node.children().count(),
            })
            .collect(),
    }
}

fn owned_values(document: &OwnedDocument) -> LogicalDocument {
    LogicalDocument {
        declaration: document.declaration().map(|d| {
            (
                d.version.into(),
                d.encoding.map(str::to_owned),
                d.standalone,
            )
        }),
        nodes: document
            .nodes()
            .map(|node| LogicalNode {
                kind: match node.kind() {
                    OwnedNodeKind::Document => "document",
                    OwnedNodeKind::Element => "element",
                    OwnedNodeKind::Text => "text",
                    OwnedNodeKind::Cdata => "cdata",
                    OwnedNodeKind::Comment => "comment",
                    OwnedNodeKind::ProcessingInstruction => "pi",
                    _ => panic!("unexpected node kind"),
                },
                name: node.name().as_ref().map(name),
                attributes: node
                    .attributes()
                    .map(|a| (name(&a.name), a.value.to_string()))
                    .collect(),
                text: node.text().map(str::to_owned),
                target: node.processing_instruction_target().map(str::to_owned),
                children: node.children().count(),
            })
            .collect(),
    }
}

fn output_values(mut input: LogicalDocument) -> LogicalDocument {
    if let Some((_, Some(encoding), _)) = &mut input.declaration {
        *encoding = "UTF-8".into();
    }
    input
}

fn encoded(input: &str, mode: usize) -> Vec<u8> {
    match mode {
        0 => input.as_bytes().to_vec(),
        1 => [b"\xef\xbb\xbf".as_slice(), input.as_bytes()].concat(),
        2 | 3 => std::iter::once(0xfeff)
            .chain(input.replace("uTf-8", "UTF-16").encode_utf16())
            .flat_map(|unit| {
                if mode == 2 {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                }
            })
            .collect(),
        _ => unreachable!(),
    }
}

fn borrowed_output(document: &Document<'_>, limits: WriterLimits) -> Vec<u8> {
    let mut writer = Writer::with_limits(Vec::new(), limits);
    writer.write_document(document).unwrap();
    writer.finish().unwrap();
    let position = writer.position();
    let bytes = writer.into_inner();
    assert_eq!(position, bytes.len());
    bytes
}

fn owned_output(document: &OwnedDocument, limits: WriterLimits) -> Vec<u8> {
    let mut writer = Writer::with_limits(Vec::new(), limits);
    writer.write_owned_document(document).unwrap();
    let position = writer.position();
    writer.finish().unwrap();
    let bytes = writer.into_inner();
    assert_eq!(position, bytes.len());
    bytes
}

fn check(input: &str, expected: Option<&str>) {
    let document = Document::parse(input).unwrap();
    let original = borrowed_values(&document);
    let output = borrowed_output(&document, WriterLimits::default());
    if let Some(expected) = expected {
        assert_eq!(output, expected.as_bytes(), "source {input:?}");
    }
    assert_eq!(
        borrowed_values(&document),
        original,
        "serialization changed source DOM"
    );
    let reparsed = Document::parse_bytes(&output).unwrap();
    assert_eq!(borrowed_values(&reparsed), output_values(original));
    assert_eq!(borrowed_output(&reparsed, WriterLimits::default()), output);
    for mode in 0..4 {
        let mut bytes = encoded(input.trim_start_matches('\u{feff}'), mode);
        let owned = OwnedDocument::parse_bytes(&bytes).unwrap();
        let before = owned_values(&owned);
        bytes.fill(0);
        drop(bytes);
        assert_eq!(owned_output(&owned, WriterLimits::default()), output);
        assert_eq!(owned_values(&owned), before);
        let reparsed = OwnedDocument::parse_bytes(&output).unwrap();
        assert_eq!(owned_values(&reparsed), output_values(before));
    }
}

#[test]
fn independent_byte_goldens_preserve_every_kind_and_normalized_value() {
    for (input, expected) in [
        ("<r></r>", "<r/>"),
        ("\u{feff}<r/>", "<r/>"),
        ("<?xml version='1.0'?><r/>", "<?xml version=\"1.0\"?><r/>"),
        (
            "<?xml version='1.0' encoding='uTf-8'?><r/>",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><r/>",
        ),
        (
            "<?xml version='1.2' standalone='no'?><r/>",
            "<?xml version=\"1.2\" standalone=\"no\"?><r/>",
        ),
        (
            "<!----><?pre?><r><![CDATA[]]><!----><?p?></r><?post?><!---->",
            "<!----><?pre?><r><![CDATA[]]><!----><?p?></r><?post?><!---->",
        ),
        (
            "<r>before<c></c>after<![CDATA[a]]><![CDATA[b]]></r>",
            "<r>before<c/>after<![CDATA[a]]><![CDATA[b]]></r>",
        ),
        (
            "<r xml:space='preserve'> \t<c/> \n</r>",
            "<r xml:space=\"preserve\"> \t<c/> \n</r>",
        ),
        (
            "<r a='1' xmlns='urn:d' xmlns:p='urn:a'><p:c xmlns:p='urn:b' p:a='2'/><p:c/><c xmlns=''/></r>",
            "<r a=\"1\" xmlns=\"urn:d\" xmlns:p=\"urn:a\"><p:c xmlns:p=\"urn:b\" p:a=\"2\"/><p:c/><c xmlns=\"\"/></r>",
        ),
        (
            "\u{feff}<?xml version='1.2' encoding='uTf-8' standalone='yes'?>\r\n<!--前--><?pre \tdata\r?><é:r a='A\tB\r\n&#9;&#10;&#13;&amp;&lt;&quot;&apos;>' xmlns:é='urn:ü' é:中='🙂'>é🙂\r\n&#13;&amp;&lt;&gt;<![CDATA[中\r\n<&]]><!----><?空?><c></c></é:r><!--後--><?post x?>\r\n",
            "<?xml version=\"1.2\" encoding=\"UTF-8\" standalone=\"yes\"?><!--前--><?pre data\n?><é:r a=\"A B &#9;&#10;&#13;&amp;&lt;&quot;'>\" xmlns:é=\"urn:ü\" é:中=\"🙂\">é🙂\n&#13;&amp;&lt;&gt;<![CDATA[中\n<&]]><!----><?空?><c/></é:r><!--後--><?post x?>",
        ),
    ] {
        check(input, Some(expected));
    }
}

#[test]
fn generated_documents_round_trip_strictly_and_serialize_idempotently() {
    let text = [
        "",
        "é中🙂",
        "&amp;&lt;&gt;",
        "&#13;\r\n",
        "x<c/>y",
        "<![CDATA[]]>",
        "<!--中--><?p data?>",
    ];
    let attributes = [
        "",
        " a='&#9;&#10;&#13;'",
        " p:a='中&amp;&quot;'",
        " a='>' b='&lt;'",
    ];
    for index in 0..112 {
        let depth = index % 7;
        let count = index % 9;
        let mut input = String::from("<p:r xmlns:p='urn:p'>");
        input.push_str(&"<n>".repeat(depth));
        for sibling in 0..count {
            input.push_str(&format!(
                "<p:c{}>{}</p:c>",
                attributes[(index + sibling) % attributes.len()],
                text[(index * 3 + sibling) % text.len()]
            ));
        }
        input.push_str(&"</n>".repeat(depth));
        input.push_str("</p:r>");
        check(&input, None);
    }
}

fn xml(error: WriteError) -> yaxml::Error {
    match error {
        WriteError::Xml(error) => error,
        other => panic!("unexpected I/O error {other}"),
    }
}

#[test]
fn each_writer_ceiling_counts_emitted_utf8_events_and_expansion() {
    for (input, category, boundary) in [
        ("<r></r>", Limit::Events, 1),
        ("<r><![CDATA[]]></r>", Limit::Events, 3),
        ("<?xml version='1.0'?><r></r>", Limit::Events, 2),
        ("<r>&amp;&amp;</r>", Limit::OutputBytes, 17),
        ("<r>&amp;&amp;</r>", Limit::TokenBytes, 10),
        ("<r a='&#9;'/>", Limit::TokenBytes, 13),
        ("<r><c/></r>", Limit::Depth, 2),
        ("<r a='1' xmlns='u'/>", Limit::Attributes, 2),
        ("<r>&amp;</r>", Limit::OwnedBytes, 6),
        ("<r><s>x</s><s>x</s></r>", Limit::OwnedBytes, 3),
    ] {
        let borrowed = Document::parse(input).unwrap();
        for maximum in [boundary - 1, boundary, boundary + 1] {
            let mut limits = WriterLimits::default();
            match category {
                Limit::Events => limits.max_events = maximum,
                Limit::OutputBytes => limits.max_output_bytes = maximum,
                Limit::TokenBytes => limits.max_token_bytes = maximum,
                Limit::Depth => limits.max_depth = maximum,
                Limit::Attributes => limits.max_attributes = maximum,
                Limit::OwnedBytes => limits.max_owned_bytes = maximum,
                _ => unreachable!(),
            }
            for mode in 0..5 {
                let mut writer = Writer::with_limits(Vec::new(), limits);
                let result = if mode == 0 {
                    writer.write_document(&borrowed)
                } else {
                    let owned = OwnedDocument::parse_bytes(&encoded(input, mode - 1)).unwrap();
                    writer.write_owned_document(&owned)
                };
                if maximum < boundary {
                    assert_eq!(
                        xml(result.unwrap_err()).kind,
                        ErrorKind::LimitExceeded(category),
                        "{input:?} {category:?}={maximum} mode{mode}"
                    );
                    assert_eq!(
                        xml(writer.finish().unwrap_err()).kind,
                        ErrorKind::InvalidState
                    );
                } else {
                    result.unwrap();
                    writer.finish().unwrap();
                    let output = writer.into_inner();
                    Document::parse_bytes(&output).unwrap();
                }
            }
        }
    }
    for owned in [false, true] {
        let mut writer = Writer::with_limits(Vec::new(), {
            let mut cfg = WriterLimits::default();
            cfg.max_owned_bytes = 0;
            cfg
        });
        if owned {
            writer
                .write_owned_document(&OwnedDocument::parse("<r></r>").unwrap())
                .unwrap();
        } else {
            writer
                .write_document(&Document::parse("<r></r>").unwrap())
                .unwrap();
        }
        assert_eq!(writer.into_inner(), b"<r/>");
    }
}

#[test]
fn failure_offsets_use_emitted_utf8_coordinates_after_discarded_input_prefixes() {
    const INPUT: &str = " \r\n<r>é🙂<c/></r>";
    let borrowed = Document::parse(INPUT).unwrap();
    for mode in 0..5 {
        let mut writer = Writer::with_limits(Vec::new(), {
            let mut cfg = WriterLimits::default();
            cfg.max_events = 2;
            cfg
        });
        let result = if mode == 0 {
            writer.write_document(&borrowed)
        } else {
            let owned = OwnedDocument::parse_bytes(&encoded(INPUT, mode - 1)).unwrap();
            writer.write_owned_document(&owned)
        };
        assert_eq!(
            xml(result.unwrap_err()),
            yaxml::Error {
                kind: ErrorKind::LimitExceeded(Limit::Events),
                offset: 9,
            }
        );
        assert_eq!(writer.position(), 9);
        assert_eq!(writer.into_inner(), "<r>é🙂".as_bytes());
    }
}

#[test]
fn deep_and_wide_trees_serialize_without_call_stack_growth_or_lost_siblings() {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 8192;
            let input = format!("{}{}", "<n>".repeat(DEPTH), "</n>".repeat(DEPTH));
            let expected = format!(
                "{}<n/>{}",
                "<n>".repeat(DEPTH - 1),
                "</n>".repeat(DEPTH - 1)
            );
            let parse_limits = {
                let mut cfg = Limits::default();
                cfg.max_depth = DEPTH;
                cfg
            };
            let write_limits = {
                let mut cfg = WriterLimits::default();
                cfg.max_depth = DEPTH;
                cfg
            };
            let borrowed = Document::with_limits(input.as_bytes(), parse_limits).unwrap();
            assert_eq!(
                borrowed_output(&borrowed, write_limits),
                expected.as_bytes()
            );
            let owned = OwnedDocument::with_limits(input.as_bytes(), parse_limits).unwrap();
            assert_eq!(owned_output(&owned, write_limits), expected.as_bytes());
        })
        .unwrap()
        .join()
        .unwrap();
    let input = format!("<r>{}</r>", "<c a='中'></c>".repeat(10_000));
    let expected = format!("<r>{}</r>", "<c a=\"中\"/>".repeat(10_000));
    check(&input, Some(&expected));
}
