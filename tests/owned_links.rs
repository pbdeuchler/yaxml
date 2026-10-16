#![forbid(unsafe_code)]

use yaxml::{Document, EditLimits, Limits, OwnedDocument, Writer};

#[test]
fn linked_walk_matches_immutable_preorder_ranges_and_exact_size_hints() {
    for depth in 1..=6 {
        for width in 0..=8 {
            let input = format!(
                "<!--pre--><r a='v'>{}{}{}{}</r><?post data?>",
                "<n>".repeat(depth),
                "text<![CDATA[c]]><!--m--><?pi d?>",
                "</n>".repeat(depth),
                "<s/>".repeat(width),
            );
            let borrowed = Document::parse(&input).unwrap();
            let owned = OwnedDocument::parse(&input).unwrap();
            let mut nodes = owned.nodes();
            for (position, expected) in borrowed.nodes().enumerate() {
                let remaining = borrowed.nodes().len() - position;
                assert_eq!(nodes.len(), remaining);
                assert_eq!(nodes.size_hint(), (remaining, Some(remaining)));
                let actual = nodes.next().unwrap();
                assert_eq!(actual.range(), expected.range());
                assert_eq!(actual.text(), expected.text());
                assert_eq!(
                    actual.name().map(|name| name.qualified().to_owned()),
                    expected.name().map(|name| name.qualified().to_owned())
                );
                assert_eq!(owned.node(actual.id()).unwrap().id(), actual.id());
                assert_eq!(
                    actual.parent().and_then(|node| node.range()),
                    expected.parent().and_then(|node| node.range())
                );
                assert_eq!(
                    actual.first_child().and_then(|node| node.range()),
                    expected.first_child().and_then(|node| node.range())
                );
                assert_eq!(
                    actual.last_child().and_then(|node| node.range()),
                    expected.last_child().and_then(|node| node.range())
                );
                assert_eq!(
                    actual.next_sibling().and_then(|node| node.range()),
                    expected.next_sibling().and_then(|node| node.range())
                );
                assert_eq!(
                    actual
                        .children()
                        .map(|node| node.range())
                        .collect::<Vec<_>>(),
                    expected
                        .children()
                        .map(|node| node.range())
                        .collect::<Vec<_>>()
                );
                let mut descendants = actual.descendants();
                for (position, expected) in expected.descendants().enumerate() {
                    assert_eq!(descendants.len(), actual.descendants().len() - position);
                    assert_eq!(descendants.next().unwrap().range(), expected.range());
                }
                assert_eq!(descendants.size_hint(), (0, Some(0)));
                assert!(descendants.next().is_none());
                assert!(descendants.next().is_none());
            }
            assert_eq!(nodes.len(), 0);
            assert_eq!(nodes.size_hint(), (0, Some(0)));
            assert!(nodes.next().is_none());
            assert!(nodes.next().is_none());
        }
    }
}

#[test]
fn linked_walk_and_drop_are_iterative_on_deep_and_wide_trees() {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            let depth = 8192;
            let input = format!("{}{}", "<n>".repeat(depth), "</n>".repeat(depth));
            let document = OwnedDocument::with_limits(
                input.as_bytes(),
                Limits {
                    max_depth: depth,
                    ..Limits::default()
                },
            )
            .unwrap();
            {
                let mut nodes = document.nodes();
                for index in 0..=depth {
                    assert_eq!(nodes.len(), depth + 1 - index);
                    let node = nodes.next().unwrap();
                    assert_eq!(node.descendants().len(), depth + 1 - index);
                    if index < depth {
                        assert_eq!(node.children().count(), 1);
                    } else {
                        assert_eq!(node.children().count(), 0);
                    }
                }
                assert!(nodes.next().is_none());
            }
            drop(document);

            let width = 20_000;
            let input = format!("<r>{}</r>", "<c/>".repeat(width));
            let document = OwnedDocument::parse(&input).unwrap();
            assert_eq!(document.nodes().len(), width + 2);
            assert_eq!(document.root_element().children().count(), width);
            let mut nodes = document.nodes();
            for remaining in (1..=width + 2).rev() {
                assert_eq!(nodes.len(), remaining);
                assert!(nodes.next().is_some());
            }
            assert_eq!(nodes.size_hint(), (0, Some(0)));
            assert!(nodes.next().is_none());
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn value_edits_keep_linked_order_handles_and_provenance_through_serialization() {
    let mut document = OwnedDocument::parse("<!--pre--><r a='old'>a<c/>b</r><!--post-->").unwrap();
    let before: Vec<_> = document
        .nodes()
        .map(|node| (node.id(), node.range()))
        .collect();
    let root = document.root_element().id();
    let first = document.root_element().first_child().unwrap().id();
    let last = document.root_element().last_child().unwrap().id();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    editor.set_node_value(first, "A<&").unwrap();
    editor.set_node_value(last, "B\r").unwrap();
    editor
        .set_attribute_value(root, None, "a", "new\t")
        .unwrap();
    assert_eq!(
        editor
            .document()
            .nodes()
            .map(|node| (node.id(), node.range()))
            .collect::<Vec<_>>(),
        before
    );
    let mut writer = Writer::new(Vec::new());
    writer.write_owned_document(editor.document()).unwrap();
    assert_eq!(
        writer.into_inner(),
        b"<!--pre--><r a=\"new&#9;\">A&lt;&amp;<c/>B&#13;</r><!--post-->"
    );
}
