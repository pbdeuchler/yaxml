use std::num::NonZeroU64;

use super::*;

fn replacement(
    declaration: usize,
    invocation: u64,
    local: std::ops::Range<usize>,
    physical: std::ops::Range<usize>,
) -> SourceOrigin {
    SourceOrigin::from_repr(OriginRepr::Replacement {
        declaration: EntityDeclarationId::from_index(declaration),
        invocation: EntityInvocationId(NonZeroU64::new(invocation).unwrap()),
        local: ByteSpan::new(local).unwrap(),
        physical_reference: ByteSpan::new(physical).unwrap(),
    })
}

#[test]
fn exact_replacement_closure_and_text_join_have_distinct_gap_rules() {
    let opening = replacement(3, 7, 1..4, 20..24);
    let adjacent = replacement(3, 7, 4..8, 20..24);
    let closing = replacement(3, 7, 12..16, 20..24);

    let text = opening.join_text(adjacent);
    assert_eq!(text.replacement_range(), Some(1..8));
    assert_eq!(text.physical_context(), 20..24);

    let closed = opening.close_element(closing).unwrap();
    assert_eq!(closed.replacement_range(), Some(1..16));
    assert_eq!(closed.physical_context(), 20..24);

    let mixed = opening.join_text(closing);
    assert_eq!(mixed.input_range(), None);
    assert_eq!(mixed.replacement_range(), None);
    assert_eq!(mixed.physical_context(), 20..24);
}

#[test]
fn replacement_occurrence_and_physical_context_are_part_of_exact_identity() {
    let opening = replacement(3, 7, 1..4, 20..24);
    for different in [
        replacement(4, 7, 4..8, 20..24),
        replacement(3, 8, 4..8, 20..24),
        replacement(3, 7, 4..8, 30..34),
    ] {
        assert_eq!(opening.join_text(different).replacement_range(), None);
        assert_eq!(
            opening.close_element(different),
            Err(Error::new(
                ErrorKind::InvalidState,
                different.error_offset()
            ))
        );
    }
}

#[test]
fn replacement_and_mixed_offsets_stay_in_physical_input() {
    let value = replacement(5, 9, 100..104, 11..17);
    assert_eq!(value.input_range(), None);
    assert_eq!(value.replacement_range(), Some(100..104));
    assert_eq!(value.error_offset(), 11);
    assert_eq!(value.cancellation_offset(), 17);

    let mixed = SourceOrigin::input(2..4).join_text(value);
    assert_eq!(mixed.input_range(), None);
    assert_eq!(mixed.physical_context(), 2..17);
    assert_eq!(mixed.error_offset(), 2);
    assert_eq!(mixed.cancellation_offset(), 17);
}
