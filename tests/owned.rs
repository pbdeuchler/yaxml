#![forbid(unsafe_code)]

use std::borrow::Cow;

use yaxml::{
    Attribute, Document, Element, ErrorKind, Event, Limit, Limits, Name, NodeKind, OwnedDocument,
    OwnedNode, OwnedNodeKind, Reader, Writer,
};

const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
const SOURCE: &str = "\u{feff}<?xml version='1.2' encoding='uTf-8' standalone='yes'?>\r\n<!--before--><?pre data?><p:r xmlns='urn:d' xmlns:p='urn:p' a='\r\n\t&#9;&#10;&#13;&amp;&lt;&quot;&apos;' p:a='é中🙂'>A\r\nB&#13;&amp;<![CDATA[C\r\n<&]]><!--D\rE--><?pi \t\r\nvalue\r?><c xmlns=''/><p:c/></p:r><!--after-->";

fn check_name(actual: Name<'_>, expected: (&str, Option<&str>, &str, Option<&str>)) {
    assert_eq!(
        (
            actual.qualified(),
            actual.prefix(),
            actual.local(),
            actual.namespace()
        ),
        expected
    );
}

fn check_golden(document: &OwnedDocument) {
    let declaration = document.declaration().unwrap();
    assert_eq!(
        (
            declaration.version,
            declaration.encoding,
            declaration.standalone
        ),
        ("1.2", Some("uTf-8"), Some(true))
    );
    let root = document.root();
    assert!(matches!(root.kind(), OwnedNodeKind::Document));
    assert_eq!(root.range(), Some(0..SOURCE.len()));
    assert!(root.parent().is_none());
    assert!(root.next_sibling().is_none());
    assert!(root.name().is_none());
    assert!(root.text().is_none());
    assert_eq!(root.attributes().len(), 0);
    assert!(root.processing_instruction_target().is_none());

    let top: Vec<_> = root.children().collect();
    assert_eq!(top.len(), 4);
    assert_eq!(top[0].text(), Some("before"));
    assert_eq!(top[1].processing_instruction_target(), Some("pre"));
    assert_eq!(top[1].text(), Some("data"));
    assert_eq!(top[2].id(), document.root_element().id());
    assert_eq!(top[3].text(), Some("after"));
    assert_eq!(root.first_child().unwrap().id(), top[0].id());
    assert_eq!(root.last_child().unwrap().id(), top[3].id());
    for pair in top.windows(2) {
        assert_eq!(pair[0].next_sibling().unwrap().id(), pair[1].id());
    }
    assert!(top[3].next_sibling().is_none());

    let element = document.root_element();
    assert!(matches!(element.kind(), OwnedNodeKind::Element));
    check_name(
        element.name().unwrap(),
        ("p:r", Some("p"), "r", Some("urn:p")),
    );
    assert_eq!(element.parent().unwrap().id(), root.id());
    assert!(element.text().is_none());
    assert!(element.processing_instruction_target().is_none());
    let mut attributes = element.attributes();
    assert_eq!(attributes.len(), 4);
    for (index, (expected_name, expected_value)) in [
        (("xmlns", None, "xmlns", Some(XMLNS)), "urn:d"),
        (("xmlns:p", Some("xmlns"), "p", Some(XMLNS)), "urn:p"),
        (("a", None, "a", None), "  \t\n\r&<\"'"),
        (("p:a", Some("p"), "a", Some("urn:p")), "é中🙂"),
    ]
    .into_iter()
    .enumerate()
    {
        let attribute = attributes.next().unwrap();
        check_name(attribute.name, expected_name);
        assert_eq!(attribute.value, expected_value);
        assert!(matches!(attribute.value, Cow::Borrowed(_)));
        assert_eq!(attributes.len(), 3 - index);
    }
    assert!(attributes.next().is_none());
    assert!(attributes.next().is_none());

    let children: Vec<_> = element.children().collect();
    assert_eq!(children.len(), 6);
    assert_eq!(element.first_child().unwrap().id(), children[0].id());
    assert_eq!(element.last_child().unwrap().id(), children[5].id());
    assert!(matches!(children[0].kind(), OwnedNodeKind::Text));
    assert_eq!(children[0].text(), Some("A\nB\r&"));
    assert!(matches!(children[1].kind(), OwnedNodeKind::Cdata));
    assert_eq!(children[1].text(), Some("C\n<&"));
    assert!(matches!(children[2].kind(), OwnedNodeKind::Comment));
    assert_eq!(children[2].text(), Some("D\nE"));
    assert!(matches!(
        children[3].kind(),
        OwnedNodeKind::ProcessingInstruction
    ));
    assert_eq!(children[3].processing_instruction_target(), Some("pi"));
    assert_eq!(children[3].text(), Some("value\n"));
    check_name(children[4].name().unwrap(), ("c", None, "c", None));
    let attribute = children[4].attributes().next().unwrap();
    check_name(attribute.name, ("xmlns", None, "xmlns", Some(XMLNS)));
    assert_eq!(attribute.value, "");
    check_name(
        children[5].name().unwrap(),
        ("p:c", Some("p"), "c", Some("urn:p")),
    );
    for (index, child) in children.iter().copied().enumerate() {
        assert_eq!(child.parent().unwrap().id(), element.id());
        assert_eq!(
            child.next_sibling().map(|node| node.id()),
            children.get(index + 1).map(|node| node.id())
        );
        assert!(child.first_child().is_none());
        assert!(child.last_child().is_none());
        assert_eq!(child.children().count(), 0);
        assert_eq!(child.descendants().count(), 1);
        if index < 4 {
            assert!(child.name().is_none());
            assert_eq!(child.attributes().len(), 0);
        }
        if index != 3 {
            assert!(child.processing_instruction_target().is_none());
        }
    }
    assert_eq!(element.descendants().count(), 7);
    assert_eq!(document.nodes().count(), 11);

    let raw_nodes = [
        "<!--before-->",
        "<?pre data?>",
        &SOURCE[SOURCE.find("<p:r").unwrap()..SOURCE.find("</p:r>").unwrap() + "</p:r>".len()],
        "A\r\nB&#13;&amp;",
        "<![CDATA[C\r\n<&]]>",
        "<!--D\rE-->",
        "<?pi \t\r\nvalue\r?>",
        "<c xmlns=''/>",
        "<p:c/>",
        "<!--after-->",
    ];
    for (node, raw) in document.nodes().skip(1).zip(raw_nodes) {
        let start = SOURCE.find(raw).unwrap();
        assert_eq!(node.range(), Some(start..start + raw.len()));
        assert_eq!(&SOURCE[node.range().unwrap()], raw);
        assert_eq!(document.node(node.id()).unwrap().id(), node.id());
    }
}

#[test]
fn logical_goldens_survive_source_overwrite_drop_and_document_moves() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<OwnedDocument>();
    send_sync::<OwnedNode<'static>>();
    let mut input = SOURCE.as_bytes().to_vec();
    let document = OwnedDocument::parse_bytes(&input).unwrap();
    input.fill(0);
    drop(input);
    let mut moved = vec![document];
    let document = *Box::new(moved.pop().unwrap());
    check_golden(&document);
    let document = std::thread::spawn(move || {
        check_golden(&document);
        document
    })
    .join()
    .unwrap();
    check_golden(&document);
}

#[test]
fn namespace_views_distinguish_default_predefined_and_normalized_unicode_uris() {
    const XML: &str = "http://www.w3.org/XML/1998/namespace";
    for (input, expected_element, index, expected_attribute) in [
        (
            "<r xmlns='urn:d' a='v'/>",
            ("r", None, "r", Some("urn:d")),
            1,
            ("a", None, "a", None),
        ),
        (
            "<xml:r xml:a='v'/>",
            ("xml:r", Some("xml"), "r", Some(XML)),
            0,
            ("xml:a", Some("xml"), "a", Some(XML)),
        ),
        (
            "<é:r xmlns:é='urn:&#xFC;' é:中='v'/>",
            ("é:r", Some("é"), "r", Some("urn:ü")),
            1,
            ("é:中", Some("é"), "中", Some("urn:ü")),
        ),
    ] {
        let owned = OwnedDocument::parse(input).unwrap();
        check_name(owned.root_element().name().unwrap(), expected_element);
        let attribute = owned.root_element().attributes().nth(index).unwrap();
        check_name(attribute.name, expected_attribute);
        assert_eq!(attribute.value, "v");
    }
}

fn borrowed_name<'a>(value: &'a Name<'_>) -> Name<'a> {
    Name::new(value.qualified(), value.namespace().map(Cow::Borrowed)).unwrap()
}

fn writer_from_reader(input: &str) -> Vec<u8> {
    let mut reader = Reader::new(input.as_bytes()).unwrap();
    let mut writer = Writer::new(Vec::new());
    while let Some(event) = reader.read_event().unwrap() {
        if let Event::Empty(element) = &event {
            writer
                .write_event(&Event::Start(Element {
                    name: borrowed_name(&element.name),
                    attributes: element
                        .attributes
                        .iter()
                        .map(|attribute| Attribute {
                            name: borrowed_name(&attribute.name),
                            value: Cow::Borrowed(&attribute.value),
                        })
                        .collect(),
                }))
                .unwrap();
            writer
                .write_event(&Event::End(borrowed_name(&element.name)))
                .unwrap();
        } else {
            writer.write_event(&event).unwrap();
        }
    }
    writer.finish().unwrap();
    writer.into_inner()
}

fn writer_from_owned(document: &OwnedDocument) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new());
    if let Some(declaration) = document.declaration() {
        writer
            .write_event(&Event::Declaration(declaration))
            .unwrap();
    }
    let mut pending = vec![(document.root(), false)];
    while let Some((node, closing)) = pending.pop() {
        if closing {
            writer
                .write_event(&Event::End(node.name().unwrap()))
                .unwrap();
            continue;
        }
        let event = match node.kind() {
            OwnedNodeKind::Document => None,
            OwnedNodeKind::Element => {
                pending.push((node, true));
                Some(Event::Start(Element {
                    name: node.name().unwrap(),
                    attributes: node.attributes().collect(),
                }))
            }
            OwnedNodeKind::Text => Some(Event::Text(Cow::Borrowed(node.text().unwrap()))),
            OwnedNodeKind::Cdata => Some(Event::Cdata(Cow::Borrowed(node.text().unwrap()))),
            OwnedNodeKind::Comment => Some(Event::Comment(Cow::Borrowed(node.text().unwrap()))),
            OwnedNodeKind::ProcessingInstruction => Some(Event::ProcessingInstruction {
                target: Cow::Borrowed(node.processing_instruction_target().unwrap()),
                data: Cow::Borrowed(node.text().unwrap()),
            }),
            _ => unreachable!("unknown owned node kind"),
        };
        if let Some(event) = event {
            writer.write_event(&event).unwrap();
        }
        let children: Vec<_> = node.children().collect();
        pending.extend(children.into_iter().rev().map(|child| (child, false)));
    }
    writer.finish().unwrap();
    writer.into_inner()
}

fn compare_borrowed(input: &str, owned: &OwnedDocument) {
    let borrowed = Document::parse(input).unwrap();
    assert_eq!(borrowed.declaration().copied(), owned.declaration());
    assert_eq!(borrowed.nodes().len(), owned.nodes().count());
    let handles: std::collections::HashMap<_, _> = borrowed
        .nodes()
        .zip(owned.nodes())
        .map(|(left, right)| (left.id(), right.id()))
        .collect();
    for (left, right) in borrowed.nodes().zip(owned.nodes()) {
        assert_eq!(handles[&left.id()], right.id());
        assert_eq!(left.range(), right.range());
        assert_eq!(
            left.parent().map(|node| handles[&node.id()]),
            right.parent().map(|node| node.id())
        );
        assert_eq!(
            left.first_child().map(|node| handles[&node.id()]),
            right.first_child().map(|node| node.id())
        );
        assert_eq!(
            left.last_child().map(|node| handles[&node.id()]),
            right.last_child().map(|node| node.id())
        );
        assert_eq!(
            left.next_sibling().map(|node| handles[&node.id()]),
            right.next_sibling().map(|node| node.id())
        );
        assert_eq!(left.name(), right.name().as_ref());
        assert_eq!(left.text(), right.text());
        assert!(
            left.attributes()
                .iter()
                .eq(right.attributes().collect::<Vec<_>>().iter())
        );
        assert_eq!(
            left.children()
                .map(|node| handles[&node.id()])
                .collect::<Vec<_>>(),
            right.children().map(|node| node.id()).collect::<Vec<_>>()
        );
        assert_eq!(
            left.descendants()
                .map(|node| handles[&node.id()])
                .collect::<Vec<_>>(),
            right
                .descendants()
                .map(|node| node.id())
                .collect::<Vec<_>>()
        );
        match (left.kind(), right.kind()) {
            (NodeKind::Document, OwnedNodeKind::Document)
            | (NodeKind::Element(_), OwnedNodeKind::Element)
            | (NodeKind::Text(_), OwnedNodeKind::Text)
            | (NodeKind::Cdata(_), OwnedNodeKind::Cdata)
            | (NodeKind::Comment(_), OwnedNodeKind::Comment) => {
                assert!(right.processing_instruction_target().is_none())
            }
            (
                NodeKind::ProcessingInstruction { target, .. },
                OwnedNodeKind::ProcessingInstruction,
            ) => assert_eq!(right.processing_instruction_target(), Some(target.as_ref())),
            _ => panic!("owned node kind differs from borrowed node"),
        }
    }
    assert_eq!(writer_from_owned(owned), writer_from_reader(input));
}

#[test]
fn corpus_trees_and_writer_values_match_the_borrowed_api() {
    for input in [
        SOURCE,
        "<r/>",
        "<r></r>",
        "<r><![CDATA[]]><?p?><c a=''/></r>",
        "<r xmlns='urn:d' xmlns:p='urn:u'><p:c xmlns:p='urn:v' p:a='1'/><p:c/><c xmlns=''/><c/></r>",
        include_str!("../fuzz/corpus/minimal.xml"),
        include_str!("../fuzz/corpus/namespaces.xml"),
        include_str!("../fuzz/corpus/lexical.xml"),
        include_str!("../fuzz/corpus/references.xml"),
        include_str!("../fuzz/corpus/declaration.xml"),
        include_str!("../fuzz/corpus/infoset.xml"),
    ] {
        let owned = OwnedDocument::parse(input).unwrap();
        compare_borrowed(input, &owned);
    }
}

#[test]
fn unicode_pool_growth_preserves_prior_views_and_navigation_ranges() {
    let mut input = String::from("<é:根 xmlns:é='urn:u'><![CDATA[]]><!----><?空?><é:空 a=''/>");
    for index in 0..512 {
        input.push_str(&format!("<é:子 a='{index}&amp;🙂'>é中🙂{index}&lt;</é:子>"));
    }
    input.push_str("</é:根>");
    let owned = OwnedDocument::parse(&input).unwrap();
    compare_borrowed(&input, &owned);
    let expected_range = 0..input.len();
    input.clear();
    input.push_str(&"overwritten".repeat(4096));
    drop(input);
    assert_eq!(owned.root().range(), Some(expected_range));
    check_name(
        owned.root_element().name().unwrap(),
        ("é:根", Some("é"), "根", Some("urn:u")),
    );
    let attribute = owned.root_element().attributes().next().unwrap();
    check_name(attribute.name, ("xmlns:é", Some("xmlns"), "é", Some(XMLNS)));
    assert_eq!(attribute.value, "urn:u");
    assert_eq!(owned.nodes().count(), 1030);
    let children: Vec<_> = owned.root_element().children().collect();
    assert_eq!(children.len(), 516);
    assert!(matches!(children[0].kind(), OwnedNodeKind::Cdata));
    assert!(matches!(children[1].kind(), OwnedNodeKind::Comment));
    assert_eq!(children[0].text(), Some(""));
    assert_eq!(children[1].text(), Some(""));
    assert_eq!(children[2].processing_instruction_target(), Some("空"));
    assert_eq!(children[2].text(), Some(""));
    check_name(
        children[3].name().unwrap(),
        ("é:空", Some("é"), "空", Some("urn:u")),
    );
    assert_eq!(children[3].attributes().next().unwrap().value, "");
    for (index, node) in children[4..].iter().copied().enumerate() {
        check_name(
            node.name().unwrap(),
            ("é:子", Some("é"), "子", Some("urn:u")),
        );
        assert_eq!(
            node.attributes().next().unwrap().value,
            format!("{index}&🙂")
        );
        assert_eq!(
            node.first_child().unwrap().text(),
            Some(format!("é中🙂{index}<").as_str())
        );
        assert_eq!(
            node.last_child().unwrap().id(),
            node.first_child().unwrap().id()
        );
        assert_eq!(node.parent().unwrap().id(), owned.root_element().id());
        assert_eq!(
            node.next_sibling().map(|node| node.id()),
            children.get(index + 5).map(|node| node.id())
        );
    }
}

#[test]
fn malformed_and_unsupported_inputs_return_explicit_errors() {
    for (input, expected) in [
        (b"".as_slice(), ErrorKind::MissingRoot),
        (b"<!--only-->", ErrorKind::MissingRoot),
        (b"<r>", ErrorKind::UnexpectedEof),
        (b"<r></x>", ErrorKind::MismatchedEndTag),
        (b"<r/><s/>", ErrorKind::MultipleRoots),
        (b"x<r/>", ErrorKind::TextOutsideRoot),
        (b"<r>\xff</r>", ErrorKind::InvalidUtf8),
        (b"<r>\0</r>", ErrorKind::InvalidCharacter),
        (b"<p:r/>", ErrorKind::Namespace),
        (b"<r a='1' a='2'/>", ErrorKind::DuplicateAttribute),
        (
            b"<r xmlns:p='u' xmlns:q='u' p:a='1' q:a='2'/>",
            ErrorKind::DuplicateAttribute,
        ),
        (b"<r>&unknown;</r>", ErrorKind::UnknownEntity),
        (b"<r>&#0;</r>", ErrorKind::InvalidReference),
        (b"<?xml version='1.x'?><r/>", ErrorKind::InvalidDeclaration),
        (
            b"<!DOCTYPE r [<!ELEMENT r EMPTY>]><r/>",
            ErrorKind::UnsupportedDtd,
        ),
        (
            b"<?xml version='1.0' encoding='UTF-16'?><r/>",
            ErrorKind::UnsupportedEncoding,
        ),
    ] {
        let error = OwnedDocument::parse_bytes(input).expect_err("invalid input accepted");
        assert_eq!(error.kind, expected, "{input:?}");
        assert!(error.offset <= input.len());
        if let Ok(input) = std::str::from_utf8(input) {
            assert_eq!(OwnedDocument::parse(input).unwrap_err(), error);
        }
    }
}

#[test]
fn each_resource_limit_is_inclusive_for_owned_construction() {
    for (input, limit, boundary) in [
        (b"<r/>".as_slice(), Limit::InputBytes, 4),
        (b"<r/>", Limit::TokenBytes, 4),
        (b"<r/>", Limit::Depth, 1),
        (b"<r><c/></r>", Limit::Depth, 2),
        (b"<r a='v'/>", Limit::Attributes, 1),
        (b"<r>x</r>", Limit::Events, 3),
        (b"<r/>", Limit::Events, 1),
        (b"<r/>", Limit::OwnedBytes, 1),
        (b"<r>&amp;</r>", Limit::TokenBytes, 5),
    ] {
        for ceiling in [boundary - 1, boundary, boundary + 1] {
            let mut limits = Limits::default();
            match limit {
                Limit::InputBytes => limits.max_input_bytes = ceiling,
                Limit::TokenBytes => limits.max_token_bytes = ceiling,
                Limit::Depth => limits.max_depth = ceiling,
                Limit::Attributes => limits.max_attributes = ceiling,
                Limit::Events => limits.max_events = ceiling,
                Limit::OwnedBytes => limits.max_owned_bytes = ceiling,
                _ => unreachable!("table contains parser limits"),
            }
            let result = OwnedDocument::with_limits(input, limits);
            if ceiling < boundary {
                let error = result.expect_err("resource ceiling was ignored");
                assert_eq!(error.kind, ErrorKind::LimitExceeded(limit), "{input:?}");
                assert!(error.offset <= input.len());
            } else {
                assert!(result.is_ok(), "{input:?}: {limit:?}={ceiling}");
            }
        }
    }
}

#[test]
fn parser_normalization_and_pool_reservations_share_one_cumulative_budget() {
    // The qualified name costs one pool byte. A second one-byte value needs a
    // new two-byte pool allocation, charged in full despite releasing the old
    // allocation. Reference/EOL normalization charges its raw input separately.
    for (input, boundary, expected_text) in [
        ("<r/>", 1, None),
        ("<r>a</r>", 3, Some("a")),
        ("<r>&amp;</r>", 8, Some("&")),
        ("<r>&#x41;</r>", 9, Some("A")),
        ("<r>\r</r>", 4, Some("\n")),
        ("<r>\r\n</r>", 5, Some("\n")),
        ("<r a='&#9;'/>", 10, None),
    ] {
        for ceiling in [boundary - 1, boundary, boundary + 1] {
            let limits = Limits {
                max_owned_bytes: ceiling,
                ..Limits::default()
            };
            let result = OwnedDocument::with_limits(input.as_bytes(), limits);
            if ceiling < boundary {
                let error = result.expect_err("aggregate budget was not enforced");
                assert_eq!(error.kind, ErrorKind::LimitExceeded(Limit::OwnedBytes));
                assert!(error.offset <= input.len());
            } else {
                let owned = result.unwrap();
                assert_eq!(
                    owned
                        .root_element()
                        .first_child()
                        .and_then(|node| node.text()),
                    expected_text
                );
            }
        }
    }
    let parser_only = Limits {
        max_owned_bytes: 5,
        ..Limits::default()
    };
    assert!(Document::with_limits(b"<r>&amp;</r>", parser_only).is_ok());
    assert_eq!(
        OwnedDocument::with_limits(b"<r>&amp;</r>", parser_only)
            .unwrap_err()
            .kind,
        ErrorKind::LimitExceeded(Limit::OwnedBytes)
    );
    let uri = format!("urn:{}", "🙂".repeat(256));
    let input = format!("<p:r xmlns:p='{uri}' p:a='v'/>");
    let limits = Limits {
        max_owned_bytes: uri.len() - 1,
        ..Limits::default()
    };
    assert!(Document::with_limits(input.as_bytes(), limits).is_ok());
    assert_eq!(
        OwnedDocument::with_limits(input.as_bytes(), limits)
            .unwrap_err()
            .kind,
        ErrorKind::LimitExceeded(Limit::OwnedBytes)
    );
}

#[test]
fn generated_topology_and_deep_wide_destruction_are_iterative() {
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            for (depth, width) in [(1, 0), (1, 8192), (8192, 1), (32, 32)] {
                let input = format!(
                    "{}{}{}",
                    "<r>".repeat(depth),
                    "<leaf/>".repeat(width),
                    "</r>".repeat(depth)
                );
                let limits = Limits {
                    max_depth: depth + 1,
                    ..Limits::default()
                };
                let document = OwnedDocument::with_limits(input.as_bytes(), limits).unwrap();
                drop(input);
                assert_eq!(document.nodes().count(), 1 + depth + width);
                let mut parent = document.root_element();
                for _ in 1..depth {
                    let child = parent.first_child().unwrap();
                    assert_eq!(child.parent().unwrap().id(), parent.id());
                    assert_eq!(
                        child.range().unwrap().start,
                        parent.range().unwrap().start + 3
                    );
                    assert_eq!(child.range().unwrap().end, parent.range().unwrap().end - 4);
                    parent = child;
                }
                assert_eq!(parent.children().count(), width);
                for (index, child) in parent.children().enumerate() {
                    assert_eq!(child.parent().unwrap().id(), parent.id());
                    assert_eq!(child.range().unwrap().start, depth * 3 + index * 7);
                    assert_eq!(child.range().unwrap().end, depth * 3 + (index + 1) * 7);
                    assert_eq!(child.descendants().count(), 1);
                }
                assert_eq!(document.root().descendants().count(), 1 + depth + width);
                drop(document);
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn owned_ids_reject_foreign_documents_and_survive_moves() {
    let larger = OwnedDocument::parse("<r><a/><b/></r>").unwrap();
    let smaller = OwnedDocument::parse("<other/>").unwrap();
    let foreign = larger.root_element().last_child().unwrap().id();
    assert!(smaller.node(foreign).is_none());
    let same_index = larger.root_element().id();
    assert!(smaller.node(same_index).is_none());
    assert!(larger.node(smaller.root().id()).is_none());
    assert_ne!(larger.root().id(), smaller.root().id());
    let larger = vec![larger].pop().unwrap();
    let larger = std::thread::spawn(move || larger).join().unwrap();
    assert_eq!(
        larger.node(same_index).unwrap().name().unwrap().qualified(),
        "r"
    );
    for node in larger.nodes() {
        assert_eq!(larger.node(node.id()).unwrap().id(), node.id());
    }
}
