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
        .unwrap();
    let merged = document
        .append_text(
            NodeId(0),
            Cow::Borrowed("b"),
            SourceOrigin::input(4..5),
            state.owned_budget(),
        )
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
