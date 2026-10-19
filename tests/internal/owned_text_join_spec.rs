//! Independent owned sink and construction controls from frozen D01--M02.
use super::*;
use crate::origin::{ByteSpan, EntityId, EntityInvocationId, OriginRepr};
use crate::parser::ParserState;
use std::num::NonZeroU64;

fn generated(i: u64, range: Range<usize>) -> SourceOrigin {
    SourceOrigin::from_repr(OriginRepr::Replacement {
        declaration: EntityId(1),
        invocation: EntityInvocationId(NonZeroU64::new(i).unwrap()),
        local: ByteSpan::new(range).unwrap(),
        physical_reference: ByteSpan::new(103..106).unwrap(),
    })
}
fn element(name: &'static str) -> crate::Element<'static> {
    crate::Element {
        name: Name::new(name, None).unwrap(),
        attributes: Vec::new().into(),
    }
}
fn begin(budget: &mut Budget) -> OwnedBuilder {
    let mut b = OwnedBuilder::new(200, 0, Global).unwrap();
    b.consume(
        Event::Start(element("r")),
        SourceOrigin::input(0..3),
        budget,
    )
    .unwrap();
    b
}
fn snapshot(b: &OwnedBuilder) -> String {
    format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        b.nodes, b.pool, b.attributes, b.parents, b.root_element, b.declaration
    )
}
fn end(mut b: OwnedBuilder, budget: &mut Budget) -> OwnedDocument {
    b.consume(
        Event::End(Name::new("r", None).unwrap()),
        SourceOrigin::input(150..154),
        budget,
    )
    .unwrap();
    b.finish(200, None, Default::default(), Vec::new()).unwrap()
}
fn serialized(doc: &OwnedDocument) -> Vec<u8> {
    let mut w = crate::Writer::new(Vec::new());
    w.write_owned_document(doc).unwrap();
    w.into_inner()
}
fn summary(doc: &OwnedDocument) -> Vec<String> {
    doc.root_element()
        .children()
        .map(|n| match n.kind() {
            OwnedNodeKind::Text => format!("T:{}", n.text().unwrap()),
            OwnedNodeKind::Cdata => format!("C:{}", n.text().unwrap()),
            OwnedNodeKind::Comment => format!("M:{}", n.text().unwrap()),
            OwnedNodeKind::ProcessingInstruction => format!(
                "P:{}:{}",
                n.processing_instruction_target().unwrap(),
                n.text().unwrap()
            ),
            OwnedNodeKind::Element => format!("E:{}", n.name().unwrap().qualified()),
            OwnedNodeKind::Document | OwnedNodeKind::DocumentType | OwnedNodeKind::EntityRef => {
                panic!("document child")
            }
        })
        .collect()
}

#[test]
fn spec_pool_tail_extension_keeps_one_prefix_copy_slot_and_links() {
    let mut state = ParserState::new(Limits::default(), 0);
    let mut b = begin(state.owned_budget());
    b.pool.0.try_reserve_exact(128).unwrap();
    let pool_pointer = b.pool.0.as_ptr();
    let before = state.owned_budget().remaining_owned();
    for (text, origin) in [
        ("pre", SourceOrigin::input(100..103)),
        ("tail", generated(7, 0..4)),
        ("post", SourceOrigin::input(106..110)),
    ] {
        b.consume(
            Event::Text(Cow::Borrowed(text)),
            origin,
            state.owned_budget(),
        )
        .unwrap();
        assert_eq!(b.pool.0.as_ptr(), pool_pointer);
        assert_eq!(b.nodes.len(), 3);
        assert_eq!(b.nodes[1].first_child, Link::new(NodeId(2)).unwrap());
        assert_eq!(b.nodes[1].last_child, Link::new(NodeId(2)).unwrap());
        assert_eq!(b.nodes[2].parent, Link::new(NodeId(1)).unwrap());
        assert_eq!(b.nodes[2].previous_sibling, Link::NONE);
        assert_eq!(b.nodes[2].next_sibling, Link::NONE);
    }
    assert_eq!(b.pool.0, "rpretailpost");
    assert_eq!(state.owned_budget().remaining_owned(), before);
    assert!(matches!(&b.nodes[2].content,Content::Text(range) if range==&(1..12)));
    assert!(b.nodes[2].source.unwrap().input_range().is_none());
    b.consume(
        Event::Comment(Cow::Borrowed("later")),
        SourceOrigin::input(110..122),
        state.owned_budget(),
    )
    .unwrap();
    assert_eq!(b.nodes[2].next_sibling, Link::new(NodeId(3)).unwrap());
    assert_eq!(b.nodes[3].previous_sibling, Link::new(NodeId(2)).unwrap());
    let doc = end(b, state.owned_budget());
    assert_eq!(summary(&doc), ["T:pretailpost", "M:later"]);
    assert_eq!(doc.nodes[0].subtree_size, 4);
    assert_eq!(doc.nodes[1].subtree_size, 3);
    assert_eq!(doc.nodes[2].subtree_size, 1);
    assert_eq!(serialized(&doc), b"<r>pretailpost<!--later--></r>");
}

#[test]
fn spec_owned_literal_entity_topologies_have_complete_links_and_provenance() {
    for shape in 0..7 {
        let mut state = ParserState::new(Limits::default(), 0);
        let mut b = begin(state.owned_budget());
        b.consume(
            Event::Text(Cow::Borrowed("pre")),
            SourceOrigin::input(100..103),
            state.owned_budget(),
        )
        .unwrap();
        if shape == 0 {
            b.consume(
                Event::Text(Cow::Borrowed("tail")),
                generated(7, 0..4),
                state.owned_budget(),
            )
            .unwrap();
        }
        if shape == 2 || shape == 3 {
            if shape == 3 {
                b.consume(
                    Event::Text(Cow::Borrowed("head")),
                    generated(7, 0..4),
                    state.owned_budget(),
                )
                .unwrap();
            }
            b.consume(
                Event::Empty(element("a")),
                generated(7, if shape == 2 { 0..4 } else { 4..8 }),
                state.owned_budget(),
            )
            .unwrap();
        }
        if shape >= 4 {
            let event = match shape {
                4 => Event::Cdata(Cow::Borrowed("c")),
                5 => Event::Comment(Cow::Borrowed("c")),
                _ => Event::ProcessingInstruction {
                    target: Cow::Borrowed("p"),
                    data: Cow::Borrowed("d"),
                },
            };
            b.consume(
                event,
                generated(
                    7,
                    0..match shape {
                        4 => 13,
                        5 => 8,
                        _ => 7,
                    },
                ),
                state.owned_budget(),
            )
            .unwrap();
        }
        if shape == 2 || shape >= 4 {
            b.consume(
                Event::Text(Cow::Borrowed("tail")),
                generated(
                    7,
                    match shape {
                        2 => 4..8,
                        4 => 13..17,
                        5 => 8..12,
                        _ => 7..11,
                    },
                ),
                state.owned_budget(),
            )
            .unwrap();
        }
        b.consume(
            Event::Text(Cow::Borrowed("post")),
            SourceOrigin::input(106..110),
            state.owned_budget(),
        )
        .unwrap();
        let doc = end(b, state.owned_budget());
        let (expected, xml) = match shape {
            0 => (vec!["T:pretailpost"], "<r>pretailpost</r>"),
            1 => (vec!["T:prepost"], "<r>prepost</r>"),
            2 => (vec!["T:pre", "E:a", "T:tailpost"], "<r>pre<a/>tailpost</r>"),
            3 => (vec!["T:prehead", "E:a", "T:post"], "<r>prehead<a/>post</r>"),
            4 => (
                vec!["T:pre", "C:c", "T:tailpost"],
                "<r>pre<![CDATA[c]]>tailpost</r>",
            ),
            5 => (
                vec!["T:pre", "M:c", "T:tailpost"],
                "<r>pre<!--c-->tailpost</r>",
            ),
            _ => (
                vec!["T:pre", "P:p:d", "T:tailpost"],
                "<r>pre<?p d?>tailpost</r>",
            ),
        };
        assert_eq!(summary(&doc), expected, "shape {shape}");
        assert_eq!(serialized(&doc), xml.as_bytes());
        let children = doc.root_element().children().collect::<Vec<_>>();
        assert_eq!(doc.nodes.len(), children.len() + 2);
        assert_eq!(doc.root().descendants().count(), doc.nodes.len());
        assert_eq!(doc.root_element().descendants().count(), children.len() + 1);
        assert_eq!(doc.nodes[1].first_child.get(), Some(NodeId(2)));
        assert_eq!(
            doc.nodes[1].last_child.get(),
            Some(NodeId(doc.nodes.len() - 1))
        );
        for (index, node) in children.iter().enumerate() {
            let record = &doc.nodes[index + 2];
            assert_eq!(record.id, NodeId(index + 2));
            assert_eq!(record.parent.get(), Some(NodeId(1)));
            assert_eq!(record.subtree_size, 1);
            assert_eq!(
                record.previous_sibling.get(),
                if index == 0 {
                    None
                } else {
                    Some(NodeId(index + 1))
                }
            );
            assert_eq!(
                record.next_sibling.get(),
                if index + 1 == children.len() {
                    None
                } else {
                    Some(NodeId(index + 3))
                }
            );
            assert_eq!(node.parent().unwrap().id(), doc.root_element().id());
            let origin = node.origin().expect("parsed provenance");
            if node.kind() == OwnedNodeKind::Text {
                if node.text().unwrap().len() > 4 {
                    assert!(origin.input_range().is_none());
                    assert!(origin.replacement_range().is_none())
                } else {
                    assert!(origin.input_range().is_some())
                }
            } else {
                assert!(origin.input_range().is_none());
                assert_eq!(
                    origin.replacement_range(),
                    Some(match shape {
                        2 => 0..4,
                        3 => 4..8,
                        4 => 0..13,
                        5 => 0..8,
                        _ => 0..7,
                    })
                )
            }
        }
    }
}

#[test]
fn spec_owned_exact_origin_empty_event_and_cdata_controls() {
    for (left, right, exact, local) in [
        (
            SourceOrigin::input(10..11),
            SourceOrigin::input(11..12),
            Some(10..12),
            None,
        ),
        (generated(7, 0..1), generated(7, 1..2), None, Some(0..2)),
        (generated(7, 0..1), generated(8, 1..2), None, None),
    ] {
        let mut state = ParserState::new(Limits::default(), 0);
        let mut b = begin(state.owned_budget());
        b.consume(Event::Text(Cow::Borrowed("a")), left, state.owned_budget())
            .unwrap();
        let saved = snapshot(&b);
        b.consume(
            Event::Text(Cow::Borrowed("")),
            generated(9, 0..0),
            state.owned_budget(),
        )
        .unwrap();
        assert_eq!(snapshot(&b), saved);
        b.consume(Event::Text(Cow::Borrowed("b")), right, state.owned_budget())
            .unwrap();
        assert_eq!(b.nodes.len(), 3);
        let origin = b.nodes[2].source.unwrap();
        assert_eq!(origin.input_range(), exact);
        assert_eq!(origin.replacement_range(), local);
        b.consume(
            Event::Cdata(Cow::Borrowed("c")),
            generated(7, 2..15),
            state.owned_budget(),
        )
        .unwrap();
        b.consume(
            Event::Cdata(Cow::Borrowed("d")),
            generated(7, 15..28),
            state.owned_budget(),
        )
        .unwrap();
        let doc = end(b, state.owned_budget());
        assert_eq!(summary(&doc), ["T:ab", "C:c", "C:d"]);
    }
}

#[test]
fn spec_owned_pool_quota_failure_is_atomic_and_exact_capacity_recovers() {
    for enough in [false, true] {
        let mut initial = ParserState::new(Limits::default(), 0);
        let mut b = begin(initial.owned_budget());
        b.consume(
            Event::Text(Cow::Borrowed("pre")),
            SourceOrigin::input(100..103),
            initial.owned_budget(),
        )
        .unwrap();
        let incoming = "x".repeat(b.pool.0.capacity() + 1);
        let needed = b.pool.0.len() + incoming.len();
        // The existing pool contract charges a full requested replacement capacity.
        let mut constrained = ParserState::new(
            Limits {
                max_owned_bytes: needed - usize::from(!enough),
                ..Limits::default()
            },
            0,
        );
        let saved = snapshot(&b);
        let pointer = b.pool.0.as_ptr();
        let capacity = b.pool.0.capacity();
        let result = b.consume(
            Event::Text(Cow::Borrowed(&incoming)),
            generated(7, 0..incoming.len()),
            constrained.owned_budget(),
        );
        if enough {
            result.unwrap();
            assert_eq!(b.nodes.len(), 3);
            assert_eq!(b.pool.0, format!("rpre{incoming}"));
            assert_eq!(constrained.owned_budget().remaining_owned(), 0)
        } else {
            assert_eq!(
                result,
                Err(Error::new(ErrorKind::LimitExceeded(Limit::OwnedBytes), 103))
            );
            assert_eq!(snapshot(&b), saved);
            assert_eq!(b.pool.0.as_ptr(), pointer);
            assert_eq!(b.pool.0.capacity(), capacity);
            assert_eq!(constrained.owned_budget().remaining_owned(), needed - 1)
        }
    }
}

#[test]
fn spec_bad_closure_is_atomic_and_generated_same_occurrence_body_gaps_close() {
    let mut state = ParserState::new(Limits::default(), 0);
    let mut b = OwnedBuilder::new(200, 0, Global).unwrap();
    b.consume(
        Event::Start(element("r")),
        generated(7, 0..3),
        state.owned_budget(),
    )
    .unwrap();
    b.consume(
        Event::Text(Cow::Borrowed("body")),
        SourceOrigin::input(90..94),
        state.owned_budget(),
    )
    .unwrap();
    let saved = snapshot(&b);
    assert_eq!(
        b.consume(
            Event::End(Name::new("r", None).unwrap()),
            generated(8, 20..24),
            state.owned_budget()
        ),
        Err(Error::new(ErrorKind::InvalidState, 103))
    );
    assert_eq!(snapshot(&b), saved);
    b.consume(
        Event::End(Name::new("r", None).unwrap()),
        generated(7, 20..24),
        state.owned_budget(),
    )
    .unwrap();
    assert!(b.parents.is_empty());
    assert_eq!(b.nodes[1].source.unwrap().replacement_range(), Some(0..24));
    assert_eq!(b.nodes[1].subtree_size, 2);
}

#[test]
fn spec_utf8_and_utf16_parse_pool_and_topology_match() {
    let text = "<r>é&#x1F980;z<![CDATA[c]]>tail</r>";
    let reference = OwnedDocument::parse(text).unwrap();
    assert_eq!(summary(&reference), ["T:é🦀z", "C:c", "T:tail"]);
    for little in [false, true] {
        let mut raw = if little {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for unit in text.encode_utf16() {
            raw.extend(if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        let actual = OwnedDocument::parse_bytes(&raw).unwrap();
        assert_eq!(summary(&actual), summary(&reference));
        assert_eq!(actual.pool.0, reference.pool.0);
        assert_eq!(serialized(&actual), serialized(&reference));
        assert!(
            actual
                .root()
                .descendants()
                .all(|node| node.origin().is_some())
        );
    }
}

#[test]
fn spec_editor_keeps_adjacent_text_ids_and_parsed_mixed_provenance() {
    let mut state = ParserState::new(Limits::default(), 0);
    let mut b = begin(state.owned_budget());
    b.consume(
        Event::Text(Cow::Borrowed("pre")),
        SourceOrigin::input(100..103),
        state.owned_budget(),
    )
    .unwrap();
    b.consume(
        Event::Text(Cow::Borrowed("post")),
        SourceOrigin::input(106..110),
        state.owned_budget(),
    )
    .unwrap();
    let mut doc = end(b, state.owned_budget());
    let root = doc.root_element().id();
    let parsed = doc.root_element().first_child().unwrap().id();
    let origin = doc.node(parsed).unwrap().origin().unwrap();
    assert!(origin.input_range().is_none());
    let (first, barrier, second) = {
        let mut editor = doc.edit(crate::EditLimits::default()).unwrap();
        editor.set_node_value(parsed, "changed").unwrap();
        let first = editor.append_text(root, "a").unwrap();
        let barrier = editor.append_comment(root, "barrier").unwrap();
        let second = editor.append_text(root, "b").unwrap();
        (first, barrier, second)
    };
    assert_eq!(doc.node(parsed).unwrap().origin(), Some(origin));
    assert_eq!(doc.node(first).unwrap().origin(), None);
    assert_eq!(doc.node(second).unwrap().origin(), None);
    {
        let mut editor = doc.edit(crate::EditLimits::default()).unwrap();
        editor.remove_subtree(barrier).unwrap();
        editor.set_node_value(first, "A").unwrap();
    }
    assert_eq!(
        doc.root_element()
            .children()
            .map(|n| n.id())
            .collect::<Vec<_>>(),
        [parsed, first, second]
    );
    let third = doc
        .edit(crate::EditLimits::default())
        .unwrap()
        .append_text(root, "c")
        .unwrap();
    assert_ne!(third, first);
    assert_ne!(third, second);
    assert_ne!(third, barrier);
    assert_eq!(
        doc.root_element()
            .children()
            .map(|n| n.id())
            .collect::<Vec<_>>(),
        [parsed, first, second, third]
    );
    assert_eq!(
        doc.root_element().text_chunks().collect::<Vec<_>>(),
        ["changed", "A", "b", "c"]
    );
    assert_eq!(serialized(&doc), b"<r>changedAbc</r>");
}

#[test]
fn spec_non_tail_pool_extension_rejects_before_bytes_or_budget_change() {
    let mut pool = Pool(String::from("prefixlater"));
    let before = pool.0.clone();
    let pointer = pool.0.as_ptr();
    let capacity = pool.0.capacity();
    let mut state = ParserState::new(Limits::default(), 0);
    let remaining = state.owned_budget().remaining_owned();
    assert_eq!(
        pool.extend_tail(&(0..6), "x", 103, state.owned_budget()),
        Err(Error::new(ErrorKind::InvalidState, 103))
    );
    assert_eq!(pool.0, before);
    assert_eq!(pool.0.as_ptr(), pointer);
    assert_eq!(pool.0.capacity(), capacity);
    assert_eq!(state.owned_budget().remaining_owned(), remaining);
}

#[test]
fn spec_constructed_document_appends_are_distinct_and_have_no_parsed_origin() {
    let mut doc = OwnedDocument::create(
        crate::ElementSpec {
            name: crate::NameSpec {
                local: "r",
                namespace: None,
                prefix: crate::PrefixPolicy::Exact(None),
            },
            items: &[],
        },
        crate::EditLimits::default(),
    )
    .unwrap();
    let root = doc.root_element().id();
    assert!(doc.root_element().origin().is_none());
    let (a, b) = {
        let mut edit = doc.edit(crate::EditLimits::default()).unwrap();
        (
            edit.append_text(root, "a").unwrap(),
            edit.append_text(root, "b").unwrap(),
        )
    };
    assert_ne!(a, b);
    assert_eq!(
        doc.root_element()
            .children()
            .map(|node| node.id())
            .collect::<Vec<_>>(),
        [a, b]
    );
    assert_eq!(
        doc.root_element().text_chunks().collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert!(doc.node(a).unwrap().origin().is_none());
    assert!(doc.node(b).unwrap().origin().is_none());
    assert_eq!(serialized(&doc), b"<r>ab</r>");
}
