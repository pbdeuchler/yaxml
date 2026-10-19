//! Independent direct-builder cases from the frozen B01--B04 oracle.
use super::*;
use crate::origin::{ByteSpan, EntityDeclarationId, EntityInvocationId, OriginRepr};
use crate::parser::ParserState;
use std::num::NonZeroU64;

fn generated(i: u64, range: Range<usize>) -> SourceOrigin {
    SourceOrigin::from_repr(OriginRepr::Replacement {
        declaration: EntityDeclarationId::from_index(1),
        invocation: EntityInvocationId(NonZeroU64::new(i).unwrap()),
        local: ByteSpan::new(range).unwrap(),
        physical_reference: ByteSpan::new(103..106).unwrap(),
    })
}
fn document() -> Document<'static> {
    Document::parse("<r/>").unwrap()
}
fn tree_snapshot(doc: &Document<'_>) -> String {
    format!(
        "{:?}|{:?}|{:?}",
        doc.nodes, doc.root_element, doc.declaration
    )
}
fn finish(doc: &mut Document<'_>) {
    let end = doc.nodes.len();
    doc.nodes[0].subtree_end = end;
    doc.nodes[1].subtree_end = end;
}
fn summary(doc: &Document<'_>) -> Vec<String> {
    doc.root_element()
        .children()
        .map(|n| match n.kind() {
            NodeKind::Text(v) => format!("T:{v}"),
            NodeKind::Cdata(v) => format!("C:{v}"),
            NodeKind::Comment(v) => format!("M:{v}"),
            NodeKind::ProcessingInstruction { target, data } => format!("P:{target}:{data}"),
            NodeKind::Element(e) => format!("E:{}", e.name.qualified()),
            NodeKind::Document | NodeKind::Doctype(_) => panic!("document child"),
        })
        .collect()
}

#[test]
fn spec_borrowed_and_owned_first_fragments_are_adopted_without_extra_charge() {
    let mut doc = document();
    let mut state = ParserState::new(Limits::default(), 0);
    let budget = state.owned_budget();
    let before = budget.remaining_owned();
    let id = doc
        .append_text(
            NodeId(1),
            Cow::Borrowed("a"),
            SourceOrigin::input(100..101),
            budget,
        )
        .unwrap()
        .unwrap();
    assert_eq!(id, NodeId(2));
    assert_eq!(budget.remaining_owned(), before);
    assert!(matches!(
        doc.nodes[2].kind,
        NodeKind::Text(Cow::Borrowed("a"))
    ));
    let mut doc = document();
    let owned = String::from("already paid");
    let pointer = owned.as_ptr();
    budget.charge_owned(owned.capacity(), 100).unwrap();
    let before = budget.remaining_owned();
    doc.append_text(
        NodeId(1),
        Cow::Owned(owned),
        SourceOrigin::input(100..112),
        budget,
    )
    .unwrap();
    assert_eq!(budget.remaining_owned(), before);
    let NodeKind::Text(Cow::Owned(value)) = &doc.nodes[2].kind else {
        panic!("owned text")
    };
    assert_eq!(value.as_ptr(), pointer);
}

#[test]
fn spec_exact_and_mixed_text_joins_keep_one_id_and_all_links() {
    let cases = [
        (
            SourceOrigin::input(10..11),
            SourceOrigin::input(11..12),
            Some(10..12),
            None,
        ),
        (
            SourceOrigin::input(10..11),
            SourceOrigin::input(14..15),
            None,
            None,
        ),
        (generated(7, 0..1), generated(7, 1..2), None, Some(0..2)),
        (generated(7, 0..1), generated(8, 1..2), None, None),
        (
            SourceOrigin::input(100..103),
            SourceOrigin::input(106..110),
            None,
            None,
        ),
    ];
    for (left, right, exact, replacement) in cases {
        let mut doc = document();
        let mut state = ParserState::new(Limits::default(), 0);
        let first = doc
            .append_text(NodeId(1), Cow::Borrowed("a"), left, state.owned_budget())
            .unwrap();
        let second = doc
            .append_text(NodeId(1), Cow::Borrowed("b"), right, state.owned_budget())
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(doc.nodes.len(), 3);
        assert_eq!(doc.nodes[1].first_child, Some(NodeId(2)));
        assert_eq!(doc.nodes[1].last_child, Some(NodeId(2)));
        assert_eq!(doc.nodes[2].parent, Some(NodeId(1)));
        assert_eq!(doc.nodes[2].next_sibling, None);
        assert_eq!(doc.nodes[2].origin.input_range(), exact);
        assert_eq!(doc.nodes[2].origin.replacement_range(), replacement);
        finish(&mut doc);
        assert_eq!(summary(&doc), ["T:ab"]);
        assert_eq!(doc.root().descendants().count(), 3);
    }
}

#[test]
fn spec_literal_entity_shapes_have_text_only_merging_and_hard_barriers() {
    // Synthetic logical events isolate the parser sink; public GE scheduling is
    // intentionally still disabled at this private checkpoint.
    for shape in 0..7 {
        let mut doc = document();
        let mut state = ParserState::new(Limits::default(), 0);
        doc.append_text(
            NodeId(1),
            Cow::Borrowed("pre"),
            SourceOrigin::input(100..103),
            state.owned_budget(),
        )
        .unwrap();
        if shape == 0 {
            doc.append_text(
                NodeId(1),
                Cow::Borrowed("tail"),
                generated(7, 0..4),
                state.owned_budget(),
            )
            .unwrap();
        }
        if shape == 2 || shape == 3 {
            if shape == 3 {
                doc.append_text(
                    NodeId(1),
                    Cow::Borrowed("head"),
                    generated(7, 0..4),
                    state.owned_budget(),
                )
                .unwrap();
            }
            let element = Element {
                name: Name::new("a", None).unwrap(),
                attributes: Vec::new(),
            };
            doc.append(
                NodeId(1),
                NodeKind::Element(element),
                generated(7, if shape == 2 { 0..4 } else { 4..8 }),
            )
            .unwrap();
        }
        if shape >= 4 {
            let kind = match shape {
                4 => NodeKind::Cdata(Cow::Borrowed("c")),
                5 => NodeKind::Comment(Cow::Borrowed("c")),
                _ => NodeKind::ProcessingInstruction {
                    target: Cow::Borrowed("p"),
                    data: Cow::Borrowed("d"),
                },
            };
            doc.append(
                NodeId(1),
                kind,
                generated(
                    7,
                    0..match shape {
                        4 => 13,
                        5 => 8,
                        _ => 7,
                    },
                ),
            )
            .unwrap();
        }
        if shape == 2 || shape >= 4 {
            doc.append_text(
                NodeId(1),
                Cow::Borrowed("tail"),
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
        doc.append_text(
            NodeId(1),
            Cow::Borrowed("post"),
            SourceOrigin::input(106..110),
            state.owned_budget(),
        )
        .unwrap();
        finish(&mut doc);
        let expected = match shape {
            0 => vec!["T:pretailpost"],
            1 => vec!["T:prepost"],
            2 => vec!["T:pre", "E:a", "T:tailpost"],
            3 => vec!["T:prehead", "E:a", "T:post"],
            4 => vec!["T:pre", "C:c", "T:tailpost"],
            5 => vec!["T:pre", "M:c", "T:tailpost"],
            _ => vec!["T:pre", "P:p:d", "T:tailpost"],
        };
        assert_eq!(summary(&doc), expected, "shape {shape}");
        let children = doc.root_element().children().collect::<Vec<_>>();
        for (index, child) in children.iter().enumerate() {
            if let NodeKind::Text(value) = child.kind() {
                if value.len() > 4 {
                    assert!(child.origin().input_range().is_none());
                    assert!(child.origin().replacement_range().is_none())
                } else {
                    assert!(child.origin().input_range().is_some())
                }
            } else {
                assert_eq!(child.origin().input_range(), None);
                assert_eq!(
                    child.origin().replacement_range(),
                    Some(match shape {
                        2 => 0..4,
                        3 => 4..8,
                        4 => 0..13,
                        5 => 0..8,
                        _ => 0..7,
                    })
                );
            }
            assert_eq!(child.parent().unwrap().id(), NodeId(1));
            assert_eq!(
                child.next_sibling().map(|n| n.id()),
                children.get(index + 1).map(|n| n.id())
            );
        }
    }
}

#[test]
fn spec_empty_events_do_not_create_nodes_or_hide_gaps_and_cdata_never_merges() {
    let mut doc = document();
    let mut state = ParserState::new(Limits::default(), 0);
    assert_eq!(
        doc.append_text(
            NodeId(1),
            Cow::Borrowed(""),
            generated(7, 0..0),
            state.owned_budget()
        ),
        Ok(None)
    );
    doc.append_text(
        NodeId(1),
        Cow::Borrowed("pre"),
        SourceOrigin::input(100..103),
        state.owned_budget(),
    )
    .unwrap();
    let snapshot = tree_snapshot(&doc);
    doc.append_text(
        NodeId(1),
        Cow::Borrowed(""),
        generated(8, 0..0),
        state.owned_budget(),
    )
    .unwrap();
    assert_eq!(tree_snapshot(&doc), snapshot);
    doc.append_text(
        NodeId(1),
        Cow::Borrowed("post"),
        SourceOrigin::input(106..110),
        state.owned_budget(),
    )
    .unwrap();
    assert!(doc.nodes[2].origin.input_range().is_none());
    doc.append(
        NodeId(1),
        NodeKind::Cdata(Cow::Borrowed("a")),
        generated(7, 0..13),
    )
    .unwrap();
    doc.append(
        NodeId(1),
        NodeKind::Cdata(Cow::Borrowed("b")),
        generated(7, 13..26),
    )
    .unwrap();
    finish(&mut doc);
    assert_eq!(summary(&doc), ["T:prepost", "C:a", "C:b"]);
}

#[test]
fn spec_merge_quota_failure_preserves_cow_capacity_topology_and_origin() {
    for ceiling in [1, 2] {
        let mut doc = document();
        let mut state = ParserState::new(
            Limits {
                max_owned_bytes: ceiling,
                ..Limits::default()
            },
            0,
        );
        doc.append_text(
            NodeId(1),
            Cow::Borrowed("a"),
            SourceOrigin::input(10..11),
            state.owned_budget(),
        )
        .unwrap();
        let snapshot = tree_snapshot(&doc);
        let result = doc.append_text(
            NodeId(1),
            Cow::Borrowed("b"),
            generated(7, 0..1),
            state.owned_budget(),
        );
        if ceiling == 1 {
            assert_eq!(
                result,
                Err(Error::new(
                    ErrorKind::LimitExceeded(crate::Limit::OwnedBytes),
                    103
                ))
            );
            assert_eq!(tree_snapshot(&doc), snapshot);
            assert!(matches!(
                doc.nodes[2].kind,
                NodeKind::Text(Cow::Borrowed("a"))
            ));
            assert_eq!(state.owned_budget().remaining_owned(), 1)
        } else {
            assert_eq!(result, Ok(Some(NodeId(2))));
            assert_eq!(state.owned_budget().remaining_owned(), 0)
        }
    }
    // Failure with an already owned prefix must not alter its allocation either.
    let mut doc = document();
    let mut state = ParserState::new(
        Limits {
            max_owned_bytes: 2,
            ..Limits::default()
        },
        0,
    );
    let value = String::from("ab");
    let pointer = value.as_ptr();
    let capacity = value.capacity();
    let incoming = "c".repeat(capacity);
    doc.append_text(
        NodeId(1),
        Cow::Owned(value),
        SourceOrigin::input(10..12),
        state.owned_budget(),
    )
    .unwrap();
    let snapshot = tree_snapshot(&doc);
    assert_eq!(
        doc.append_text(
            NodeId(1),
            Cow::Borrowed(&incoming),
            generated(7, 0..incoming.len()),
            state.owned_budget()
        ),
        Err(Error::new(
            ErrorKind::LimitExceeded(crate::Limit::OwnedBytes),
            103
        ))
    );
    assert_eq!(tree_snapshot(&doc), snapshot);
    let NodeKind::Text(Cow::Owned(value)) = &doc.nodes[2].kind else {
        panic!("owned")
    };
    assert_eq!(value.as_ptr(), pointer);
    assert_eq!(value.capacity(), capacity);
}

#[test]
fn spec_many_fragment_merges_reuse_capacity_and_keep_the_first_record() {
    let mut doc = document();
    let mut state = ParserState::new(Limits::default(), 0);
    for index in 0..128 {
        let before = state.owned_budget().remaining_owned();
        let pointer = match doc.nodes.get(2).map(|n| &n.kind) {
            Some(NodeKind::Text(Cow::Owned(s))) => Some(s.as_ptr()),
            _ => None,
        };
        let capacity = match doc.nodes.get(2).map(|n| &n.kind) {
            Some(NodeKind::Text(Cow::Owned(s))) => s.capacity(),
            _ => 0,
        };
        doc.append_text(
            NodeId(1),
            Cow::Borrowed("x"),
            SourceOrigin::input(index..index + 1),
            state.owned_budget(),
        )
        .unwrap();
        assert_eq!(doc.nodes.len(), 3);
        if capacity > index {
            let NodeKind::Text(Cow::Owned(value)) = &doc.nodes[2].kind else {
                panic!("owned")
            };
            assert_eq!(Some(value.as_ptr()), pointer);
            assert_eq!(state.owned_budget().remaining_owned(), before);
        }
    }
    let NodeKind::Text(Cow::Owned(value)) = &doc.nodes[2].kind else {
        panic!("promoted")
    };
    assert_eq!(value.as_str(), "x".repeat(128));
    assert_eq!(doc.nodes[2].origin.input_range(), Some(0..128));
}

#[test]
fn spec_capacity_overflow_is_a_safe_allocation_error_with_no_value_mutation() {
    let mut value = String::from("kept");
    let before = value.clone();
    let pointer = value.as_ptr();
    let capacity = value.capacity();
    let mut state = ParserState::new(
        Limits {
            max_owned_bytes: usize::MAX,
            ..Limits::default()
        },
        0,
    );
    assert_eq!(
        reserve_text(&mut value, usize::MAX, 103, state.owned_budget()),
        Err(Error::new(ErrorKind::Allocation, 103))
    );
    assert_eq!(value, before);
    assert_eq!(value.as_ptr(), pointer);
    assert_eq!(value.capacity(), capacity);
    // Failed allocation requests remain spent under the existing Budget contract.
    assert_eq!(state.owned_budget().remaining_owned(), 0);
    let mut value = String::new();
    let mut state = ParserState::new(
        Limits {
            max_owned_bytes: usize::MAX - 1,
            ..Limits::default()
        },
        0,
    );
    assert_eq!(
        reserve_text(&mut value, usize::MAX, 103, state.owned_budget()),
        Err(Error::new(
            ErrorKind::LimitExceeded(crate::Limit::OwnedBytes),
            103
        ))
    );
    assert!(value.is_empty());
    assert_eq!(state.owned_budget().remaining_owned(), usize::MAX - 1);
}
