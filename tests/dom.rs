use std::borrow::Cow;

use yaxml::{Document, ErrorKind, Limit, Limits, NodeKind};

#[test]
fn document_structure_and_navigation() {
    let input = "<!--before--><?a b?><root><first/><second><leaf/></second>tail</root><!--after-->";
    let document = Document::parse(input).unwrap();
    let root = document.root();
    assert!(matches!(root.kind(), NodeKind::Document));
    assert_eq!(root.range(), Some(0..input.len()));
    assert!(root.parent().is_none());
    assert!(root.next_sibling().is_none());
    assert!(root.name().is_none());
    assert!(root.text().is_none());
    assert!(root.attributes().is_empty());
    assert!(document.declaration().is_none());

    let top: Vec<_> = root.children().collect();
    assert_eq!(top.len(), 4);
    assert_eq!(top[0].text(), Some("before"));
    assert_eq!(top[1].text(), Some("b"));
    assert!(matches!(
        top[1].kind(),
        NodeKind::ProcessingInstruction {
            target: Cow::Borrowed("a"),
            ..
        }
    ));
    assert_eq!(top[2].id(), document.root_element().id());
    assert_eq!(top[3].text(), Some("after"));
    assert_eq!(root.last_child().unwrap().id(), top[3].id());

    let element = document.root_element();
    assert_eq!(element.name().unwrap().qualified(), "root");
    assert_eq!(element.parent().unwrap().id(), root.id());
    assert!(element.text().is_none());
    assert_eq!(
        &input[element.range().expect("direct input node")],
        "<root><first/><second><leaf/></second>tail</root>"
    );
    let children: Vec<_> = element.children().collect();
    assert_eq!(children.len(), 3);
    assert_eq!(element.first_child().unwrap().id(), children[0].id());
    assert_eq!(children[0].next_sibling().unwrap().id(), children[1].id());
    assert_eq!(children[0].name().unwrap().local(), "first");
    assert!(children[0].first_child().is_none());
    assert!(children[0].last_child().is_none());
    assert!(children[0].attributes().is_empty());
    assert_eq!(children[0].descendants().len(), 1);
    assert_eq!(children[1].descendants().len(), 2);
    assert_eq!(children[2].text(), Some("tail"));
    assert!(children[2].name().is_none());
    assert!(children[2].next_sibling().is_none());
    assert!(children[2].children().next().is_none());
    assert_eq!(element.descendants().len(), 5);
    assert_eq!(document.nodes().len(), 9);
    assert_eq!(document.node(element.id()).unwrap().id(), element.id());
}

#[test]
fn declaration_namespaces_and_normalization() {
    let input = "<?xml version='1.0' encoding='UTF-8' standalone='yes'?><p:r xmlns:p='urn:one' a='x&#x9;y'>raw&amp;<![CDATA[a\r\nb]]><!--c--></p:r>";
    let document = Document::parse(input).unwrap();
    let declaration = document.declaration().unwrap();
    assert_eq!(declaration.version, "1.0");
    assert_eq!(declaration.encoding, Some("UTF-8"));
    assert_eq!(declaration.standalone, Some(true));
    let root = document.root_element();
    let name = root.name().unwrap();
    assert_eq!(name.qualified(), "p:r");
    assert_eq!(name.local(), "r");
    assert_eq!(name.prefix(), Some("p"));
    assert_eq!(name.namespace(), Some("urn:one"));
    let attributes = root.attributes();
    assert_eq!(attributes.len(), 2);
    assert_eq!(attributes[0].name.qualified(), "xmlns:p");
    assert_eq!(attributes[1].value, "x\ty");
    let children: Vec<_> = root.children().collect();
    assert_eq!(children.len(), 3);
    assert_eq!(children[0].text(), Some("raw&"));
    assert!(matches!(children[0].kind(), NodeKind::Text(Cow::Owned(_))));
    assert_eq!(children[1].text(), Some("a\nb"));
    assert!(matches!(children[1].kind(), NodeKind::Cdata(_)));
    assert_eq!(
        &input[children[1].range().expect("direct input node")],
        "<![CDATA[a\r\nb]]>"
    );
    assert_eq!(children[2].text(), Some("c"));
    assert!(matches!(
        children[2].kind(),
        NodeKind::Comment(Cow::Borrowed("c"))
    ));
}

#[test]
fn leaf_strings_borrow_unchanged_input() {
    let document = Document::parse("<r a='value'>text<![CDATA[cdata]]><?pi data?></r>").unwrap();
    let root = document.root_element();
    assert!(matches!(root.attributes()[0].value, Cow::Borrowed("value")));
    let nodes: Vec<_> = root.children().collect();
    assert!(matches!(
        nodes[0].kind(),
        NodeKind::Text(Cow::Borrowed("text"))
    ));
    assert!(matches!(
        nodes[1].kind(),
        NodeKind::Cdata(Cow::Borrowed("cdata"))
    ));
    assert!(matches!(
        nodes[2].kind(),
        NodeKind::ProcessingInstruction {
            data: Cow::Borrowed("data"),
            ..
        }
    ));
}

#[test]
fn rejects_invalid_documents_and_limit_failures() {
    for (input, expected) in [
        (b"".as_slice(), ErrorKind::MissingRoot),
        (b"<r>".as_slice(), ErrorKind::UnexpectedEof),
        (b"<r></x>".as_slice(), ErrorKind::MismatchedEndTag),
        (b"<r/><s/>".as_slice(), ErrorKind::MultipleRoots),
        (b"<p:r/>".as_slice(), ErrorKind::Namespace),
        (
            b"<r a='1' a='2'/>".as_slice(),
            ErrorKind::DuplicateAttribute,
        ),
        (b"<r>\xff</r>".as_slice(), ErrorKind::InvalidUtf8),
    ] {
        assert_eq!(
            Document::parse_bytes(input).unwrap_err().kind,
            expected,
            "{input:?}"
        );
    }
    let limits = Limits::default().max_depth(1);
    assert!(Document::with_limits(b"<r/>", limits).is_ok());
    assert_eq!(
        Document::with_limits(b"<r><s/></r>", limits)
            .unwrap_err()
            .kind,
        ErrorKind::LimitExceeded(Limit::Depth)
    );
}

#[test]
fn deep_and_wide_trees_preserve_navigation_without_recursion() {
    for (depth, width) in [(1, 1), (32, 16), (4096, 1), (2, 4096)] {
        let input = format!(
            "{}{}{}</r>",
            "<r>".repeat(depth),
            "<leaf/>".repeat(width),
            "</r>".repeat(depth - 1)
        );
        let limits = Limits::default().max_depth(depth + 1);
        let document = Document::with_limits(input.as_bytes(), limits).unwrap();
        assert_eq!(document.nodes().len(), depth + width + 1);
        let mut parent = document.root_element();
        for _ in 1..depth {
            let child = parent.first_child().unwrap();
            assert_eq!(child.parent().unwrap().id(), parent.id());
            parent = child;
        }
        assert_eq!(parent.children().count(), width);
        assert_eq!(parent.first_child().unwrap().descendants().len(), 1);
        assert!(parent.last_child().unwrap().next_sibling().is_none());
        drop(document);
    }
}

#[test]
fn foreign_out_of_range_id_is_rejected() {
    let larger = Document::parse("<r><a/><b/></r>").unwrap();
    let smaller = Document::parse("<r/>").unwrap();
    let foreign = larger.root_element().last_child().unwrap().id();
    assert!(smaller.node(foreign).is_none());
}

#[test]
fn generated_tree_preorder_and_parent_invariants() {
    for depth in 1..12 {
        for width in 0..12 {
            let input = format!(
                "{}{}{}",
                "<r>".repeat(depth),
                "<leaf/>".repeat(width),
                "</r>".repeat(depth)
            );
            let document = Document::parse(&input).unwrap();
            let nodes: Vec<_> = document.nodes().collect();
            assert_eq!(nodes.len(), 1 + depth + width);
            for (index, node) in nodes.iter().copied().enumerate().skip(1) {
                let parent = node.parent().unwrap();
                assert!(
                    nodes[..index]
                        .iter()
                        .any(|candidate| candidate.id() == parent.id())
                );
                assert_eq!(
                    parent
                        .children()
                        .filter(|child| child.id() == node.id())
                        .count(),
                    1
                );
                assert!(
                    parent
                        .descendants()
                        .any(|descendant| descendant.id() == node.id())
                );
                let range = node.range().expect("direct input node");
                assert!(range.start < range.end && range.end <= input.len());
            }
        }
    }
}

/// IDs noted while building match document order, the first duplicate wins,
/// and DTD-declared ID attributes still join `xml:id` ones.
#[test]
fn element_ids_are_found_during_construction() {
    let plain = "<r xml:id='a'><s id='x'/><t b='1' xml:id='b'/><u xml:id='a'/></r>";
    let declared = "<!DOCTYPE r [<!ATTLIST s id ID #IMPLIED>]>\
                    <r xml:id='a'><s id='x'/><t b='1' xml:id='b'/></r>";
    let utf16: Vec<u8> = [0xff, 0xfe]
        .into_iter()
        .chain(plain.encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    for (input, ids) in [
        (plain.as_bytes(), [Some("r"), Some("t"), None]),
        (declared.as_bytes(), [Some("r"), Some("t"), Some("s")]),
    ] {
        let borrowed = yaxml::Document::parse_bytes(input).unwrap();
        let owned = yaxml::OwnedDocument::parse_bytes(input).unwrap();
        for (value, expected) in ["a", "b", "x"].into_iter().zip(ids) {
            let name = |node: Option<yaxml::Node<'_, '_>>| {
                node.and_then(|n| n.name())
                    .map(|n| n.qualified().to_owned())
            };
            assert_eq!(name(borrowed.element_by_id(value)).as_deref(), expected);
            let owned_name = owned
                .element_by_id(value)
                .and_then(|node| node.name().map(|n| n.qualified().to_owned()));
            assert_eq!(owned_name.as_deref(), expected);
        }
    }
    let decoded = yaxml::OwnedDocument::parse_bytes(&utf16).unwrap();
    assert_eq!(
        decoded
            .element_by_id("b")
            .and_then(|n| n.name().map(|n| n.qualified().to_owned()))
            .as_deref(),
        Some("t")
    );
    assert!(decoded.element_by_id("x").is_none());
}
