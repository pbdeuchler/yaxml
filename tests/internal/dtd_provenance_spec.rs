//! Literal join/closure oracle frozen before the provenance implementation.
use super::*;

fn input(start: usize, end: usize) -> SourceOrigin {
    SourceOrigin::input(start..end)
}
fn replacement(
    d: usize,
    i: u64,
    start: usize,
    end: usize,
    reference: Range<usize>,
) -> SourceOrigin {
    SourceOrigin::from_repr(OriginRepr::Replacement {
        declaration: EntityDeclarationId::from_index(d),
        invocation: EntityInvocationId(NonZeroU64::new(i).unwrap()),
        local: ByteSpan::new(start..end).unwrap(),
        physical_reference: ByteSpan::new(reference).unwrap(),
    })
}
fn mixed(start: usize, end: usize) -> SourceOrigin {
    SourceOrigin::from_repr(OriginRepr::Mixed {
        physical_context: ByteSpan::new(start..end).unwrap(),
    })
}

#[test]
fn spec_exact_adjacency_preserves_the_source_occurrence() {
    assert_eq!(input(10, 13).join_text(input(13, 17)), input(10, 17));
    assert_eq!(
        replacement(1, 7, 0, 3, 40..43).join_text(replacement(1, 7, 3, 8, 40..43)),
        replacement(1, 7, 0, 8, 40..43)
    );
    // Zero-length source fragments are well-defined algebraically, although the
    // event producer emits no empty Text for an empty replacement.
    assert_eq!(input(10, 10).join_text(input(10, 13)), input(10, 13));
}

#[test]
fn spec_gaps_overlaps_source_and_invocation_changes_are_mixed() {
    let pairs = [
        (input(10, 13), input(16, 19), mixed(10, 19)),
        (input(10, 14), input(13, 17), mixed(10, 17)),
        (
            input(10, 13),
            replacement(1, 7, 13, 17, 40..43),
            mixed(10, 43),
        ),
        (
            replacement(1, 7, 0, 3, 40..43),
            replacement(1, 8, 3, 8, 40..43),
            mixed(40, 43),
        ),
        (
            replacement(1, 7, 0, 3, 40..43),
            replacement(2, 7, 3, 8, 40..43),
            mixed(40, 43),
        ),
        (
            replacement(1, 7, 0, 3, 40..43),
            replacement(1, 7, 4, 8, 40..43),
            mixed(40, 43),
        ),
        (
            replacement(1, 7, 0, 3, 40..43),
            replacement(1, 7, 3, 8, 50..53),
            mixed(40, 53),
        ),
        (input(20, 23), input(10, 13), mixed(10, 23)),
    ];
    for (left, right, expected) in pairs {
        let actual = left.join_text(right);
        assert_eq!(actual, expected);
        assert_eq!(actual.input_range(), None);
        assert_eq!(actual.replacement_range(), None);
        assert_eq!(actual.physical_context(), expected.physical_context());
    }
    for right in [input(1, 4), replacement(1, 7, 0, 3, 40..43), mixed(80, 90)] {
        let actual = mixed(10, 30).join_text(right);
        assert!(actual.input_range().is_none());
        assert!(actual.replacement_range().is_none());
        assert_eq!(
            actual.physical_context(),
            10.min(right.physical_context().start)..30.max(right.physical_context().end)
        );
        assert_eq!(
            right.join_text(mixed(10, 30)).physical_context(),
            actual.physical_context()
        );
    }
}

#[test]
fn spec_element_closure_permits_body_gaps_but_never_crosses_occurrences() {
    assert_eq!(
        input(10, 13).close_element(input(30, 34)),
        Ok(input(10, 34))
    );
    assert_eq!(
        input(10, 13).close_element(input(13, 17)),
        Ok(input(10, 17))
    );
    assert_eq!(
        replacement(1, 7, 0, 3, 40..43).close_element(replacement(1, 7, 20, 24, 40..43)),
        Ok(replacement(1, 7, 0, 24, 40..43))
    );
    let pairs = [
        (input(10, 14), input(13, 17)),
        (input(20, 23), input(10, 13)),
        (input(10, 13), replacement(1, 7, 20, 24, 40..43)),
        (
            replacement(1, 7, 0, 3, 40..43),
            replacement(1, 8, 20, 24, 40..43),
        ),
        (
            replacement(1, 7, 0, 3, 40..43),
            replacement(2, 7, 20, 24, 40..43),
        ),
        (
            replacement(1, 7, 0, 3, 40..43),
            replacement(1, 7, 2, 6, 40..43),
        ),
        (
            replacement(1, 7, 0, 3, 40..43),
            replacement(1, 7, 20, 24, 50..53),
        ),
        (mixed(10, 13), input(30, 34)),
        (input(10, 13), mixed(30, 34)),
    ];
    for (start, end) in pairs {
        assert_eq!(
            start.close_element(end),
            Err(Error::new(ErrorKind::InvalidState, end.error_offset()))
        );
    }
}

#[test]
fn spec_empty_reference_gaps_and_diagnostic_endpoints_are_explicit() {
    let left = input(100, 103);
    let right = input(106, 110);
    let joined = left.join_text(right);
    assert_eq!(joined, mixed(100, 110));
    assert_eq!(joined.error_offset(), 100);
    assert_eq!(joined.cancellation_offset(), 110);
    let generated = replacement(3, 8, 7, 12, 203..206);
    assert_eq!(generated.input_range(), None);
    assert_eq!(generated.replacement_range(), Some(7..12));
    assert_eq!(generated.error_offset(), 203);
    assert_eq!(generated.cancellation_offset(), 206);
    assert_eq!(input(4, 9).input_range(), Some(4..9));
    assert_eq!(input(4, 9).error_offset(), 4);
    assert_eq!(input(4, 9).cancellation_offset(), 9);
}
