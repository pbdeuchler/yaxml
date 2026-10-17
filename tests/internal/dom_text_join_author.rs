use super::*;
use crate::parser::ParserState;
use crate::{Limit, Limits};

fn empty_document() -> Document<'static> {
    Document {
        nodes: vec![Record::root(NodeKind::Document, 32)],
        root_element: NodeId(0),
        declaration: None,
    }
}

fn state(max_owned_bytes: usize) -> ParserState<'static> {
    ParserState::new(
        Limits {
            max_owned_bytes,
            ..Limits::default()
        },
        0,
    )
}

#[test]
fn parsed_text_merges_without_a_second_node_or_link_change() {
    let mut document = empty_document();
    let mut state = state(2);
    let first = document
        .append_text(
            NodeId(0),
            Cow::Borrowed("a"),
            SourceOrigin::input(3..4),
            state.owned_budget(),
        )
        .unwrap()
        .unwrap();
    let merged = document
        .append_text(
            NodeId(0),
            Cow::Borrowed("b"),
            SourceOrigin::input(4..5),
            state.owned_budget(),
        )
        .unwrap()
        .unwrap();

    assert_eq!(first, merged);
    assert_eq!(document.nodes.len(), 2);
    assert_eq!(document.nodes[0].first_child, Some(first));
    assert_eq!(document.nodes[0].last_child, Some(first));
    assert_eq!(document.nodes[first.0].next_sibling, None);
    assert_eq!(document.nodes[first.0].origin.input_range(), Some(3..5));
    let NodeKind::Text(value) = &document.nodes[first.0].kind else {
        panic!("text node changed kind")
    };
    assert_eq!(value, "ab");
}

#[test]
fn failed_borrowed_promotion_preserves_value_origin_and_topology() {
    let mut document = empty_document();
    let mut state = state(1);
    let first = document
        .append_text(
            NodeId(0),
            Cow::Borrowed("a"),
            SourceOrigin::input(8..9),
            state.owned_budget(),
        )
        .unwrap()
        .unwrap();
    let before = document.nodes.len();
    let error = document
        .append_text(
            NodeId(0),
            Cow::Borrowed("b"),
            SourceOrigin::input(12..13),
            state.owned_budget(),
        )
        .unwrap_err();

    assert_eq!(
        error,
        Error::new(ErrorKind::LimitExceeded(Limit::OwnedBytes), 12)
    );
    assert_eq!(document.nodes.len(), before);
    assert_eq!(document.nodes[0].first_child, Some(first));
    assert_eq!(document.nodes[0].last_child, Some(first));
    assert_eq!(document.nodes[first.0].origin.input_range(), Some(8..9));
    let NodeKind::Text(value) = &document.nodes[first.0].kind else {
        panic!("text node changed kind")
    };
    assert_eq!(value, "a");
}

#[test]
fn owned_text_uses_spare_capacity_without_spending_budget_or_moving() {
    let mut initial = String::with_capacity(8);
    initial.push('a');
    let initial_pointer = initial.as_ptr();
    let mut document = empty_document();
    let mut state = state(0);
    let text = document
        .append_text(
            NodeId(0),
            Cow::Owned(initial),
            SourceOrigin::input(2..3),
            state.owned_budget(),
        )
        .unwrap()
        .unwrap();
    document
        .append_text(
            NodeId(0),
            Cow::Borrowed("b"),
            SourceOrigin::input(3..4),
            state.owned_budget(),
        )
        .unwrap();

    let NodeKind::Text(Cow::Owned(value)) = &document.nodes[text.0].kind else {
        panic!("text must retain owned storage")
    };
    assert_eq!(value, "ab");
    assert_eq!(value.as_ptr(), initial_pointer);
    assert_eq!(state.owned_budget().remaining_owned(), 0);
    assert_eq!(document.nodes[text.0].origin.input_range(), Some(2..4));
}

#[test]
fn failed_owned_text_growth_preserves_value_origin_and_topology() {
    let mut initial = String::with_capacity(1);
    initial.push('a');
    let capacity = initial.capacity();
    let mut document = empty_document();
    let mut state = state(capacity);
    let text = document
        .append_text(
            NodeId(0),
            Cow::Owned(initial),
            SourceOrigin::input(5..6),
            state.owned_budget(),
        )
        .unwrap()
        .unwrap();
    let before_nodes = document.nodes.len();
    let before_first = document.nodes[0].first_child;
    let before_last = document.nodes[0].last_child;
    let incoming = "b".repeat(capacity);
    let error = document
        .append_text(
            NodeId(0),
            Cow::Borrowed(&incoming),
            SourceOrigin::input(20..20 + incoming.len()),
            state.owned_budget(),
        )
        .unwrap_err();

    assert_eq!(
        error,
        Error::new(ErrorKind::LimitExceeded(Limit::OwnedBytes), 20)
    );
    assert_eq!(state.owned_budget().remaining_owned(), capacity);
    assert_eq!(document.nodes.len(), before_nodes);
    assert_eq!(document.nodes[0].first_child, before_first);
    assert_eq!(document.nodes[0].last_child, before_last);
    assert_eq!(document.nodes[text.0].origin.input_range(), Some(5..6));
    let NodeKind::Text(Cow::Owned(value)) = &document.nodes[text.0].kind else {
        panic!("text must retain owned storage")
    };
    assert_eq!(value, "a");
}

#[test]
fn many_small_text_fragments_have_linear_requested_capacity() {
    const FRAGMENTS: usize = 96;
    const LIMIT: usize = 1024;
    let mut document = empty_document();
    let mut state = state(LIMIT);
    let text = document
        .append_text(
            NodeId(0),
            Cow::Borrowed("x"),
            SourceOrigin::input(0..1),
            state.owned_budget(),
        )
        .unwrap()
        .unwrap();
    for offset in 1..FRAGMENTS {
        assert_eq!(
            document
                .append_text(
                    NodeId(0),
                    Cow::Borrowed("x"),
                    SourceOrigin::input(offset..offset + 1),
                    state.owned_budget(),
                )
                .unwrap(),
            Some(text)
        );
    }

    let NodeKind::Text(value) = &document.nodes[text.0].kind else {
        panic!("text node changed kind")
    };
    assert_eq!(value, &"x".repeat(FRAGMENTS));
    assert_eq!(document.nodes.len(), 2);
    assert_eq!(
        document.nodes[text.0].origin.input_range(),
        Some(0..FRAGMENTS)
    );
    let spent = LIMIT - state.owned_budget().remaining_owned();
    assert!(spent <= 4 * FRAGMENTS, "requested {spent} bytes");
}

#[test]
fn empty_text_is_not_published_and_cannot_hide_a_source_gap() {
    let mut document = empty_document();
    let mut state = state(16);
    assert_eq!(
        document
            .append_text(
                NodeId(0),
                Cow::Borrowed(""),
                SourceOrigin::input(0..3),
                state.owned_budget(),
            )
            .unwrap(),
        None
    );
    let text = document
        .append_text(
            NodeId(0),
            Cow::Borrowed("a"),
            SourceOrigin::input(3..4),
            state.owned_budget(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        document
            .append_text(
                NodeId(0),
                Cow::Borrowed(""),
                SourceOrigin::input(4..8),
                state.owned_budget(),
            )
            .unwrap(),
        Some(text)
    );
    document
        .append_text(
            NodeId(0),
            Cow::Borrowed("b"),
            SourceOrigin::input(8..9),
            state.owned_budget(),
        )
        .unwrap();

    assert_eq!(document.nodes.len(), 2);
    assert_eq!(document.nodes[text.0].origin.input_range(), None);
    assert_eq!(document.nodes[text.0].origin.physical_context(), 3..9);
    let NodeKind::Text(value) = &document.nodes[text.0].kind else {
        panic!("text node changed kind")
    };
    assert_eq!(value, "ab");
}

#[test]
fn non_text_nodes_are_parse_time_coalescing_boundaries() {
    let mut document = empty_document();
    let mut state = state(16);
    document
        .append_text(
            NodeId(0),
            Cow::Borrowed("a"),
            SourceOrigin::input(0..1),
            state.owned_budget(),
        )
        .unwrap();
    document
        .append(
            NodeId(0),
            NodeKind::Cdata(Cow::Borrowed("c")),
            SourceOrigin::input(1..13),
        )
        .unwrap();
    document
        .append_text(
            NodeId(0),
            Cow::Borrowed("b"),
            SourceOrigin::input(13..14),
            state.owned_budget(),
        )
        .unwrap();
    document
        .append(
            NodeId(0),
            NodeKind::Comment(Cow::Borrowed("comment")),
            SourceOrigin::input(14..28),
        )
        .unwrap();
    document
        .append_text(
            NodeId(0),
            Cow::Borrowed("d"),
            SourceOrigin::input(28..29),
            state.owned_budget(),
        )
        .unwrap();
    document
        .append(
            NodeId(0),
            NodeKind::ProcessingInstruction {
                target: Cow::Borrowed("p"),
                data: Cow::Borrowed("data"),
            },
            SourceOrigin::input(29..39),
        )
        .unwrap();
    document
        .append_text(
            NodeId(0),
            Cow::Borrowed("e"),
            SourceOrigin::input(39..40),
            state.owned_budget(),
        )
        .unwrap();
    document
        .append(
            NodeId(0),
            NodeKind::Element(Element {
                name: Name::new(Cow::Borrowed("child"), None).unwrap(),
                attributes: Vec::new(),
            }),
            SourceOrigin::input(40..48),
        )
        .unwrap();
    document
        .append_text(
            NodeId(0),
            Cow::Borrowed("f"),
            SourceOrigin::input(48..49),
            state.owned_budget(),
        )
        .unwrap();

    assert_eq!(document.nodes.len(), 10);
    assert!(matches!(document.nodes[1].kind, NodeKind::Text(_)));
    assert!(matches!(document.nodes[2].kind, NodeKind::Cdata(_)));
    assert!(matches!(document.nodes[3].kind, NodeKind::Text(_)));
    assert!(matches!(document.nodes[4].kind, NodeKind::Comment(_)));
    assert!(matches!(document.nodes[5].kind, NodeKind::Text(_)));
    assert!(matches!(
        document.nodes[6].kind,
        NodeKind::ProcessingInstruction { .. }
    ));
    assert!(matches!(document.nodes[7].kind, NodeKind::Text(_)));
    assert!(matches!(document.nodes[8].kind, NodeKind::Element(_)));
    assert!(matches!(document.nodes[9].kind, NodeKind::Text(_)));
    assert_eq!(document.nodes[1].next_sibling, Some(NodeId(2)));
    assert_eq!(document.nodes[2].next_sibling, Some(NodeId(3)));
    assert_eq!(document.nodes[8].next_sibling, Some(NodeId(9)));
}
