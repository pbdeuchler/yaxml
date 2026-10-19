use std::mem::{align_of, size_of};
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

#[test]
fn input_text_join_requires_adjacency_but_element_closure_allows_a_body_gap() {
    let opening = SourceOrigin::input(3..6);
    assert_eq!(
        opening.join_text(SourceOrigin::input(6..9)).input_range(),
        Some(3..9)
    );
    for other in [0..2, 4..8, 7..9] {
        let joined = opening.join_text(SourceOrigin::input(other.clone()));
        assert_eq!(joined.input_range(), None);
        assert_eq!(
            joined.physical_context(),
            other.start.min(3)..other.end.max(6)
        );
    }
    assert_eq!(
        opening
            .close_element(SourceOrigin::input(20..24))
            .unwrap()
            .input_range(),
        Some(3..24)
    );
    for other in [0..2, 4..8] {
        assert_eq!(
            opening.close_element(SourceOrigin::input(other.clone())),
            Err(Error::new(ErrorKind::InvalidState, other.start))
        );
    }
}

#[test]
fn mixed_origins_remain_mixed_and_extend_only_the_physical_cover() {
    let first = SourceOrigin::input(2..5).join_text(SourceOrigin::input(8..9));
    let second = first.join_text(SourceOrigin::input(12..14));
    assert_eq!(first.input_range(), None);
    assert_eq!(first.physical_context(), 2..9);
    assert_eq!(second.input_range(), None);
    assert_eq!(second.physical_context(), 2..14);
    assert_eq!(
        first.close_element(SourceOrigin::input(12..14)),
        Err(Error::new(ErrorKind::InvalidState, 12))
    );
}

#[test]
fn provenance_layouts_are_recorded() {
    println!(
        "SourceOrigin={}/{} OriginRepr={}/{} Event={}/{} Record={}/{} Reader={}/{} PushParser={}/{}",
        size_of::<SourceOrigin>(),
        align_of::<SourceOrigin>(),
        size_of::<OriginRepr>(),
        align_of::<OriginRepr>(),
        size_of::<crate::Event<'static>>(),
        align_of::<crate::Event<'static>>(),
        size_of::<crate::dom::Record<crate::NodeKind<'static>>>(),
        align_of::<crate::dom::Record<crate::NodeKind<'static>>>(),
        size_of::<crate::Reader<'static>>(),
        align_of::<crate::Reader<'static>>(),
        size_of::<crate::PushParser>(),
        align_of::<crate::PushParser>(),
    );
}

#[test]
fn source_origin_is_copy_send_and_sync() {
    fn copy<T: Copy>() {}
    fn send<T: Send>() {}
    fn sync<T: Sync>() {}
    copy::<SourceOrigin>();
    send::<SourceOrigin>();
    sync::<SourceOrigin>();
}
