use super::*;

#[test]
fn physical_origins_and_element_joins() {
    let opening = SourceOrigin::input(3..6);
    assert_eq!(opening.input_range(), Some(3..6));
    assert_eq!(opening.error_offset(), 3);
    assert_eq!(opening.cancellation_offset(), 6);
    for (end, expected) in [(6..10, 3..10), (18..22, 3..22)] {
        let closed = opening.close_element(SourceOrigin::input(end));
        assert_eq!(closed.input_range(), Some(expected));
    }
    assert_eq!(opening.input_range(), Some(3..6));
}

#[test]
fn replacement_elements_close_within_their_invocation() {
    let replacement = |local: Range<usize>| {
        SourceOrigin::from_repr(OriginRepr::Replacement {
            declaration: EntityId(0),
            invocation: EntityInvocationId(NonZeroU64::MIN),
            local: ByteSpan::new(local).unwrap(),
            physical_reference: ByteSpan::new(7..10).unwrap(),
        })
    };
    let opening = replacement(0..3);
    assert_eq!(
        opening.close_element(replacement(5..9)).replacement_range(),
        Some(0..9)
    );
    // A recovered parse stopping in physical input keeps the opening tag.
    assert_eq!(opening.close_element(SourceOrigin::input(12..12)), opening);
    let text = opening.join_text(replacement(3..5));
    assert_eq!(text.replacement_range(), Some(0..5));
    assert_eq!(text.physical_context(), 7..10);
    let mixed = opening.join_text(SourceOrigin::input(10..12));
    assert_eq!(
        (mixed.replacement_range(), mixed.physical_context()),
        (None, 7..12)
    );
}
