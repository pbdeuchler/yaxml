use super::*;
use crate::Limits;
use crate::parser::ParserState;

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
    let mut builder = OwnedBuilder::new(32).unwrap();
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
