use super::*;
use crate::parser::ParserState;
use crate::{Element, Limits};
use std::mem::{align_of, size_of};

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
fn owned_parsed_text_extends_the_pool_tail_without_a_second_node() {
    let mut builder = OwnedBuilder::new(32, 0, Global).unwrap();
    let mut state = state(3);
    builder
        .consume(
            Event::Text(Cow::Borrowed("a")),
            SourceOrigin::input(3..4),
            state.owned_budget(),
        )
        .unwrap();
    builder
        .consume(
            Event::Text(Cow::Borrowed("b")),
            SourceOrigin::input(4..5),
            state.owned_budget(),
        )
        .unwrap();

    assert_eq!(builder.nodes.len(), 2);
    assert_eq!(builder.pool.0, "ab");
    let Content::Text(range) = &builder.nodes[1].content else {
        panic!("text node changed kind")
    };
    assert_eq!(range, &(0..2));
    assert_eq!(builder.nodes[1].source.unwrap().input_range(), Some(3..5));
    assert_eq!(builder.nodes[0].first_child.get(), Some(NodeId(1)));
    assert_eq!(builder.nodes[0].last_child.get(), Some(NodeId(1)));
}

#[test]
fn owned_empty_text_is_not_published_and_does_not_hide_a_gap() {
    let mut builder = OwnedBuilder::new(32, 0, Global).unwrap();
    let mut state = state(32);
    builder
        .consume(
            Event::Text(Cow::Borrowed("")),
            SourceOrigin::input(0..3),
            state.owned_budget(),
        )
        .unwrap();
    builder
        .consume(
            Event::Text(Cow::Borrowed("a")),
            SourceOrigin::input(3..4),
            state.owned_budget(),
        )
        .unwrap();
    builder
        .consume(
            Event::Text(Cow::Borrowed("")),
            SourceOrigin::input(4..8),
            state.owned_budget(),
        )
        .unwrap();
    builder
        .consume(
            Event::Text(Cow::Borrowed("b")),
            SourceOrigin::input(8..9),
            state.owned_budget(),
        )
        .unwrap();

    assert_eq!(builder.nodes.len(), 2);
    assert_eq!(builder.pool.0, "ab");
    let origin = builder.nodes[1].source.unwrap();
    assert_eq!(origin.input_range(), None);
    assert_eq!(origin.physical_context(), 3..9);
}

#[test]
fn owned_text_limit_failure_preserves_pool_record_and_origin() {
    let mut builder = OwnedBuilder::new(32, 0, Global).unwrap();
    builder.pool.0 = String::with_capacity(1);
    let mut initial = state(0);
    builder
        .consume(
            Event::Text(Cow::Borrowed("a")),
            SourceOrigin::input(5..6),
            initial.owned_budget(),
        )
        .unwrap();
    let capacity = builder.pool.0.capacity();
    let incoming = "b".repeat(capacity);
    let mut limited = state(capacity);
    let before_pool = builder.pool.0.clone();
    let error = builder
        .consume(
            Event::Text(Cow::Borrowed(&incoming)),
            SourceOrigin::input(20..20 + incoming.len()),
            limited.owned_budget(),
        )
        .unwrap_err();

    assert_eq!(
        error,
        Error::new(ErrorKind::LimitExceeded(Limit::OwnedBytes), 20)
    );
    assert_eq!(builder.nodes.len(), 2);
    assert_eq!(builder.pool.0, before_pool);
    assert_eq!(builder.nodes[1].source.unwrap().input_range(), Some(5..6));
    assert!(matches!(builder.nodes[1].content, Content::Text(ref range) if range == &(0..1)));
}

#[test]
fn owned_pool_tail_extension_has_exact_zero_and_one_byte_boundaries() {
    let mut rejected = Pool::default();
    let mut zero = state(0);
    assert_eq!(
        rejected.extend(0..0, "x", 7, zero.owned_budget()),
        Err(Error::new(ErrorKind::LimitExceeded(Limit::OwnedBytes), 7))
    );
    assert!(rejected.0.is_empty());

    let mut accepted = Pool::default();
    let mut one = state(1);
    assert_eq!(
        accepted.extend(0..0, "x", 7, one.owned_budget()).unwrap(),
        0..1
    );
    assert_eq!(accepted.0, "x");
}

#[test]
fn owned_non_tail_text_range_is_copied_to_the_tail() {
    let mut pool = Pool("ab".to_owned());
    let mut state = state(16);
    assert_eq!(pool.extend(0..1, "c", 9, state.owned_budget()), Ok(2..4));
    assert_eq!(pool.0, "abac");
}

#[test]
fn owned_non_text_event_keeps_text_nodes_separate() {
    let mut builder = OwnedBuilder::new(32, 0, Global).unwrap();
    let mut state = state(128);
    let events = [
        (Event::Text(Cow::Borrowed("a")), 0..1),
        (Event::Cdata(Cow::Borrowed("c")), 1..13),
        (Event::Text(Cow::Borrowed("b")), 13..14),
        (Event::Comment(Cow::Borrowed("comment")), 14..28),
        (Event::Text(Cow::Borrowed("d")), 28..29),
        (
            Event::ProcessingInstruction {
                target: Cow::Borrowed("p"),
                data: Cow::Borrowed("data"),
            },
            29..39,
        ),
        (Event::Text(Cow::Borrowed("e")), 39..40),
        (
            Event::Empty(Element {
                name: Name::new(Cow::Borrowed("child"), None).unwrap(),
                attributes: Vec::new().into(),
            }),
            40..48,
        ),
        (Event::Text(Cow::Borrowed("f")), 48..49),
    ];
    for (event, range) in events {
        builder
            .consume(event, SourceOrigin::input(range), state.owned_budget())
            .unwrap();
    }
    assert_eq!(builder.nodes.len(), 10);
    assert!(matches!(builder.nodes[1].content, Content::Text(_)));
    assert!(matches!(builder.nodes[2].content, Content::Cdata(_)));
    assert!(matches!(builder.nodes[3].content, Content::Text(_)));
    assert!(matches!(builder.nodes[4].content, Content::Comment(_)));
    assert!(matches!(builder.nodes[5].content, Content::Text(_)));
    assert!(matches!(
        builder.nodes[6].content,
        Content::ProcessingInstruction { .. }
    ));
    assert!(matches!(builder.nodes[7].content, Content::Text(_)));
    assert!(matches!(builder.nodes[8].content, Content::Element { .. }));
    assert!(matches!(builder.nodes[9].content, Content::Text(_)));
}

#[test]
fn owned_record_layout_is_recorded() {
    println!(
        "OwnedRecord={}/{} Option<SourceOrigin>={}/{}",
        size_of::<OwnedRecord>(),
        align_of::<OwnedRecord>(),
        size_of::<Option<SourceOrigin>>(),
        align_of::<Option<SourceOrigin>>()
    );
}
