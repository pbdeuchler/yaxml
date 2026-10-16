use super::*;

fn error(kind: EditErrorKind, offset: Option<usize>) -> EditError {
    EditError::new(kind, offset)
}

fn output(document: &OwnedDocument) -> Vec<u8> {
    let mut writer = crate::Writer::with_limits(
        Vec::new(),
        crate::WriterLimits {
            max_depth: 8193,
            ..crate::WriterLimits::default()
        },
    );
    writer.write_owned_document(document).unwrap();
    writer.into_inner()
}

// Independently reconstruct preorder and sizes from child/sibling links, rather
// than trusting the production descendant iterator or cached counts.
fn assert_topology(document: &OwnedDocument) {
    let mut pending = vec![NodeId(0)];
    let mut order = Vec::new();
    let mut seen = vec![false; document.nodes.len()];
    let mut sizes = vec![0; document.nodes.len()];
    while let Some(id) = pending.pop() {
        assert!(!seen[id.0], "cycle or duplicate incoming edge");
        seen[id.0] = true;
        order.push(id);
        let record = &document.nodes[id.0];
        assert_eq!(record.id, id);
        assert!(!matches!(record.content, Content::Removed));
        let mut child = record.first_child.get();
        let mut previous = None;
        let mut children = Vec::new();
        while let Some(child_id) = child {
            assert!(children.len() < document.nodes.len(), "sibling cycle");
            let record = &document.nodes[child_id.0];
            assert_eq!(record.parent.get(), Some(id));
            assert_eq!(record.previous_sibling.get(), previous);
            children.push(child_id);
            previous = Some(child_id);
            child = record.next_sibling.get();
        }
        assert_eq!(record.last_child.get(), previous);
        pending.extend(children.into_iter().rev());
    }
    for id in order.iter().rev() {
        sizes[id.0] += 1;
        if let Some(parent) = document.nodes[id.0].parent.get() {
            sizes[parent.0] += sizes[id.0];
        }
    }
    for (index, record) in document.nodes.iter().enumerate() {
        assert_eq!(record.id, NodeId(index));
        let handle = OwnedNodeId {
            document: document.identity,
            slot: NodeId(index),
        };
        assert_eq!(document.node(handle).is_some(), seen[index]);
        assert_eq!(record.subtree_size, sizes[index]);
        if !seen[index] {
            assert!(matches!(record.content, Content::Removed));
            assert_eq!(record.source, None);
            for link in [
                record.parent,
                record.first_child,
                record.last_child,
                record.previous_sibling,
                record.next_sibling,
            ] {
                assert_eq!(link, Link::NONE);
            }
        }
    }
    assert_eq!(document.root().descendants().len(), order.len());
    let mut actual = document.nodes();
    for (position, id) in order.iter().enumerate() {
        assert_eq!(actual.len(), order.len() - position);
        assert_eq!(actual.size_hint(), (actual.len(), Some(actual.len())));
        assert_eq!(actual.next().unwrap().id().slot, *id);
    }
    assert!(actual.next().is_none());
    assert!(actual.next().is_none());
}

#[test]
fn appended_kinds_keep_stable_handles_provenance_and_linked_document_order() {
    let mut document =
        OwnedDocument::parse("<!--pre--><r><a>A</a><b>B</b></r><!--post-->").unwrap();
    let before: Vec<_> = document
        .nodes()
        .map(|node| (node.id(), node.range()))
        .collect();
    let root = document.root_element().id();
    let top = document.root().id();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let text = editor.append_text(root, "x\r").unwrap();
    let cdata = editor.append_cdata(root, "").unwrap();
    let comment = editor.append_comment(root, "comment").unwrap();
    let pi = editor
        .append_processing_instruction(root, "é.中", "data")
        .unwrap();
    let empty = editor.append_text(root, "").unwrap();
    let tail = editor.append_comment(top, "tail").unwrap();
    let tail_pi = editor
        .append_processing_instruction(top, "end", "")
        .unwrap();
    for (id, kind, value) in [
        (text, OwnedNodeKind::Text, "x\r"),
        (cdata, OwnedNodeKind::Cdata, ""),
        (comment, OwnedNodeKind::Comment, "comment"),
        (pi, OwnedNodeKind::ProcessingInstruction, "data"),
        (empty, OwnedNodeKind::Text, ""),
        (tail, OwnedNodeKind::Comment, "tail"),
        (tail_pi, OwnedNodeKind::ProcessingInstruction, ""),
    ] {
        let node = editor.document().node(id).unwrap();
        assert_eq!(node.kind(), kind);
        assert_eq!(node.text(), Some(value));
        assert_eq!(node.range(), None);
    }
    assert_eq!(
        editor
            .document()
            .node(pi)
            .unwrap()
            .processing_instruction_target(),
        Some("é.中")
    );
    assert_eq!(
        editor.document().node(text).unwrap().parent().unwrap().id(),
        root
    );
    assert_eq!(editor.document().root().last_child().unwrap().id(), tail_pi);
    assert_eq!(
        editor.document().root_element().last_child().unwrap().id(),
        empty
    );
    for (id, provenance) in before {
        assert_eq!(editor.document().node(id).unwrap().range(), provenance);
    }
    assert!(!editor.document().slots_are_preorder);
    assert_eq!(editor.document().edit_created_nodes, 7);
    assert_topology(editor.document());
    assert_eq!(output(editor.document()), "<!--pre--><r><a>A</a><b>B</b>x&#13;<![CDATA[]]><!--comment--><?é.中 data?></r><!--post--><!--tail--><?end?>".as_bytes());
}

#[test]
fn append_validation_precedence_is_exact_and_errors_do_not_publish() {
    let mut document = OwnedDocument::parse("<r/>").unwrap();
    let parent = document.root_element().id();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    for (kind, value, expected) in [
        (
            ValueKind::Text,
            "é\0",
            error(EditErrorKind::InvalidCharacter, Some(2)),
        ),
        (
            ValueKind::Cdata,
            "🙂]]>",
            error(EditErrorKind::InvalidValue, Some(4)),
        ),
        (
            ValueKind::Cdata,
            "]]>\0",
            error(EditErrorKind::InvalidCharacter, Some(3)),
        ),
        (
            ValueKind::Comment,
            "é-",
            error(EditErrorKind::InvalidValue, Some(2)),
        ),
        (
            ValueKind::Comment,
            "x\r--",
            error(EditErrorKind::InvalidValue, Some(1)),
        ),
        (
            ValueKind::Pi,
            "é?>",
            error(EditErrorKind::InvalidValue, Some(2)),
        ),
        (
            ValueKind::Pi,
            " \0",
            error(EditErrorKind::InvalidCharacter, Some(1)),
        ),
        (
            ValueKind::Pi,
            "\tdata",
            error(EditErrorKind::InvalidValue, Some(0)),
        ),
    ] {
        let before = format!("{:?}", editor.document());
        assert_eq!(
            editor.append_leaf(
                parent,
                kind,
                matches!(kind, ValueKind::Pi).then_some("pi"),
                value
            ),
            Err(expected)
        );
        assert_eq!(format!("{:?}", editor.document()), before);
    }
    for (target, expected) in [
        ("", error(EditErrorKind::InvalidName, Some(0))),
        ("1name", error(EditErrorKind::InvalidName, Some(0))),
        ("é:pi", error(EditErrorKind::InvalidName, Some(2))),
        ("é🙂!", error(EditErrorKind::InvalidName, Some(6))),
        ("1\0", error(EditErrorKind::InvalidCharacter, Some(1))),
        ("é\u{f0000}", error(EditErrorKind::InvalidName, Some(2))),
    ] {
        let before = format!("{:?}", editor.document());
        assert_eq!(
            editor.append_processing_instruction(parent, target, ""),
            Err(expected)
        );
        assert_eq!(format!("{:?}", editor.document()), before);
    }
    for target in ["xml", "xmL", "xMl", "xML", "Xml", "XmL", "XMl", "XML"] {
        assert_eq!(
            editor.append_processing_instruction(parent, target, ""),
            Err(error(EditErrorKind::InvalidName, Some(0)))
        );
    }
    for target in ["_", "é", "🙂", "a-0.\u{b7}", "xml-stylesheet"] {
        editor
            .append_processing_instruction(parent, target, "")
            .unwrap();
    }
    assert_topology(editor.document());
}

#[test]
fn handles_and_parent_rules_precede_payloads_and_quota() {
    let mut document = OwnedDocument::parse("<r>t<![CDATA[c]]><!--m--><?pi d?></r>").unwrap();
    let root = document.root_element().id();
    let top = document.root().id();
    let leaves: Vec<_> = document
        .root_element()
        .children()
        .map(|node| node.id())
        .collect();
    let foreign = OwnedDocument::parse("<r/>").unwrap().root_element().id();
    let invalid = OwnedNodeId {
        document: document.identity,
        slot: NodeId(usize::MAX),
    };
    let mut editor = document
        .edit(EditLimits {
            max_value_bytes: 0,
            max_created_nodes: 0,
            max_owned_bytes: 0,
        })
        .unwrap();
    for parent in [foreign, invalid] {
        for kind in [
            ValueKind::Text,
            ValueKind::Cdata,
            ValueKind::Comment,
            ValueKind::Pi,
        ] {
            assert_eq!(
                editor.append_leaf(parent, kind, Some("\0"), "\0"),
                Err(error(EditErrorKind::InvalidHandle, None))
            );
        }
    }
    for parent in leaves.iter().copied() {
        for kind in [
            ValueKind::Text,
            ValueKind::Cdata,
            ValueKind::Comment,
            ValueKind::Pi,
        ] {
            assert_eq!(
                editor.append_leaf(parent, kind, Some("\0"), "\0"),
                Err(error(EditErrorKind::WrongNodeKind, None))
            );
        }
    }
    for kind in [ValueKind::Text, ValueKind::Cdata] {
        assert_eq!(
            editor.append_leaf(top, kind, None, ""),
            Err(error(EditErrorKind::WrongNodeKind, None))
        );
    }
    assert_eq!(
        editor.append_processing_instruction(root, "", "x"),
        Err(error(
            EditErrorKind::LimitExceeded(EditLimit::ValueBytes),
            None
        ))
    );
    assert_eq!(
        editor.append_processing_instruction(root, "x", ""),
        Err(error(
            EditErrorKind::LimitExceeded(EditLimit::ValueBytes),
            None
        ))
    );
    assert_eq!(
        editor.append_text(root, "x"),
        Err(error(
            EditErrorKind::LimitExceeded(EditLimit::ValueBytes),
            None
        ))
    );
    assert_eq!(
        editor.append_text(root, ""),
        Err(error(
            EditErrorKind::LimitExceeded(EditLimit::CreatedNodes),
            None
        ))
    );
    editor.remove_subtree(leaves[0]).unwrap();
    assert_eq!(
        editor.append_comment(leaves[0], ""),
        Err(error(EditErrorKind::InvalidHandle, None))
    );
    assert_topology(editor.document());
}

#[test]
fn pi_strings_share_one_inclusive_capacity_request_and_created_quota_persists() {
    for ceiling in [2, 3, 4] {
        let mut document = OwnedDocument::parse("<r/>").unwrap();
        assert_eq!((document.pool.0.len(), document.pool.0.capacity()), (1, 1));
        let root = document.root_element().id();
        let mut editor = document
            .edit(EditLimits {
                max_owned_bytes: ceiling,
                max_created_nodes: 1,
                ..EditLimits::default()
            })
            .unwrap();
        let before = format!("{:?}", editor.document());
        let result = editor.append_processing_instruction(root, "p", "d");
        if ceiling == 2 {
            assert_eq!(
                result,
                Err(error(
                    EditErrorKind::LimitExceeded(EditLimit::OwnedBytes),
                    None
                ))
            );
            assert_eq!(format!("{:?}", editor.document()), before);
            editor.append_text(root, "").unwrap();
        } else {
            assert_eq!(editor.document().edit_owned_bytes, 3);
            assert!(editor.document().pool.0.capacity() >= 3);
            assert_eq!(output(editor.document()), b"<r><?p d?></r>");
            let old = result.unwrap();
            editor.remove_subtree(old).unwrap();
            assert!(editor.document().node(old).is_none());
            assert_eq!(editor.document().edit_created_nodes, 1);
            assert_eq!(editor.document().edit_owned_bytes, 3);
        }
        assert_eq!(
            editor.append_text(root, ""),
            Err(error(
                EditErrorKind::LimitExceeded(EditLimit::CreatedNodes),
                None
            ))
        );
        assert_topology(editor.document());
        assert!(matches!(
            document.edit(EditLimits {
                max_created_nodes: 0,
                ..EditLimits::default()
            }),
            Err(EditError {
                kind: EditErrorKind::LimitExceeded(EditLimit::CreatedNodes),
                ..
            })
        ));
        let mut editor = document
            .edit(EditLimits {
                max_created_nodes: 2,
                ..EditLimits::default()
            })
            .unwrap();
        let id = editor.append_text(root, "").unwrap();
        assert_eq!(id.slot, NodeId(3));
        assert_topology(editor.document());
    }
}

#[test]
fn removal_stales_exact_subtree_and_updates_every_sibling_position() {
    for chosen in ["a", "b", "c", "only"] {
        let input = if chosen == "only" {
            "<r><only xmlns:p='urn:p' p:a='v'>t<x/><![CDATA[c]]><!--m--><?pi d?></only></r>"
        } else {
            "<!--pre--><r><a>A</a><b xmlns='u'><x/>B</b><c>C</c></r><!--post-->"
        };
        let mut document = OwnedDocument::parse(input).unwrap();
        let selected = document
            .root_element()
            .children()
            .find(|node| node.name().unwrap().local == chosen)
            .unwrap();
        let id = selected.id();
        let removed: Vec<_> = selected.descendants().map(|node| node.id()).collect();
        let survivors: Vec<_> = document
            .nodes()
            .filter(|node| !removed.contains(&node.id()))
            .map(|node| (node.id(), node.range()))
            .collect();
        let slots = document.nodes.len();
        let mut editor = document.edit(EditLimits::default()).unwrap();
        editor.remove_subtree(id).unwrap();
        for removed in removed {
            assert!(editor.document().node(removed).is_none());
            assert_eq!(
                editor.remove_subtree(removed),
                Err(error(EditErrorKind::InvalidHandle, None))
            );
            assert_eq!(
                editor.set_node_value(removed, ""),
                Err(error(EditErrorKind::InvalidHandle, None))
            );
        }
        for (id, range) in survivors {
            assert_eq!(editor.document().node(id).unwrap().range(), range);
        }
        assert_eq!(editor.document().nodes.len(), slots);
        assert_topology(editor.document());
        assert!(OwnedDocument::parse_bytes(&output(editor.document())).is_ok());
    }
}

#[test]
fn removal_protects_roots_and_keeps_adjacent_text_handles_without_merging() {
    let mut document =
        OwnedDocument::parse("<?pre?><!--pre--><r>a<c/>b</r><!--post--><?post?>").unwrap();
    let root = document.root_element().id();
    let top = document.root().id();
    let children: Vec<_> = document
        .root_element()
        .children()
        .map(|node| node.id())
        .collect();
    let epilog: Vec<_> = document
        .root()
        .children()
        .filter(|node| node.id() != root)
        .map(|node| node.id())
        .collect();
    let foreign = OwnedDocument::parse("<r/>").unwrap().root().id();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    assert_eq!(
        editor.remove_subtree(foreign),
        Err(error(EditErrorKind::InvalidHandle, None))
    );
    for id in [top, root] {
        let before = format!("{:?}", editor.document());
        assert_eq!(
            editor.remove_subtree(id),
            Err(error(EditErrorKind::ProtectedNode, None))
        );
        assert_eq!(format!("{:?}", editor.document()), before);
    }
    editor.remove_subtree(children[1]).unwrap();
    assert_eq!(
        editor
            .document()
            .root_element()
            .children()
            .map(|node| node.id())
            .collect::<Vec<_>>(),
        [children[0], children[2]]
    );
    let fresh = editor.append_text(root, "").unwrap();
    assert_eq!(editor.document().root_element().children().count(), 3);
    assert_ne!(fresh, children[0]);
    assert_eq!(
        output(editor.document()),
        b"<?pre?><!--pre--><r>ab</r><!--post--><?post?>"
    );
    for id in epilog {
        editor.remove_subtree(id).unwrap();
        assert_topology(editor.document());
    }
    for id in [children[0], children[2], fresh] {
        editor.remove_subtree(id).unwrap();
        assert_topology(editor.document());
    }
    assert_eq!(output(editor.document()), b"<r/>");
    let next = editor.append_comment(root, "").unwrap();
    assert!(next.slot.0 > fresh.slot.0);
    assert!(editor.document().node(fresh).is_none());
    assert_eq!(output(editor.document()), b"<r><!----></r>");
}

#[test]
fn deep_and_wide_removal_are_iterative_and_keep_only_live_topology() {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            let input = format!("<r>{}x{}</r>", "<n>".repeat(8192), "</n>".repeat(8192));
            let mut document = OwnedDocument::with_limits(
                input.as_bytes(),
                Limits {
                    max_depth: 8193,
                    ..Limits::default()
                },
            )
            .unwrap();
            let selected = document.root_element().first_child().unwrap().id();
            let mut editor = document.edit(EditLimits::default()).unwrap();
            editor.remove_subtree(selected).unwrap();
            assert_topology(editor.document());
            assert_eq!(output(editor.document()), b"<r/>");
            drop(document);

            let mut document =
                OwnedDocument::parse(&format!("<r>{}</r>", "<c/><!--m-->".repeat(10_000))).unwrap();
            let root = document.root_element().id();
            let ids: Vec<_> = document
                .root_element()
                .children()
                .map(|node| node.id())
                .collect();
            let mut editor = document.edit(EditLimits::default()).unwrap();
            for index in [0, 10_000, 19_999] {
                editor.remove_subtree(ids[index]).unwrap();
            }
            editor.append_cdata(root, "tail").unwrap();
            assert_topology(editor.document());
            assert_eq!(editor.document().nodes().len(), 20_000);
            assert!(output(editor.document()).ends_with(b"<![CDATA[tail]]></r>"));
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn private_overflow_preflights_do_not_mutate_pool_or_creation_work() {
    let pool = Pool("r".to_owned());
    assert_eq!(
        pool.edit_capacity(usize::MAX, 0, usize::MAX),
        Err(error(
            EditErrorKind::LimitExceeded(EditLimit::OwnedBytes),
            None
        ))
    );
    let mut document = OwnedDocument::parse("<r/>").unwrap();
    document.edit_created_nodes = usize::MAX;
    let parent = document.root_element().id();
    let mut editor = document
        .edit(EditLimits {
            max_created_nodes: usize::MAX,
            ..EditLimits::default()
        })
        .unwrap();
    let before = format!("{:?}", editor.document());
    assert_eq!(
        editor.append_text(parent, ""),
        Err(error(
            EditErrorKind::LimitExceeded(EditLimit::CreatedNodes),
            None
        ))
    );
    assert_eq!(format!("{:?}", editor.document()), before);
}
