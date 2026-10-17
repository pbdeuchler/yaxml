use std::mem::{align_of, size_of};

use super::*;

fn span(start: usize, end: usize) -> ByteSpan {
    ByteSpan::new(start..end).unwrap()
}

fn internal(kind: EntityKind, declaration: usize, replacement: &str) -> EntityResolution {
    EntityResolution::Internal {
        kind,
        declaration: EntityDeclarationId::from_index(declaration),
        replacement_len: replacement.len(),
    }
}

fn unlimited() -> EntityFrames {
    EntityFrames::new(FrameLimits::default())
}

#[test]
fn identifiers_contexts_and_default_state_are_concrete() {
    let declaration = EntityDeclarationId::from_index(7);
    assert_eq!(declaration.index(), 7);
    assert_eq!(
        FrameBoundary::Content { element_depth: 3 }.context(),
        ReferenceContext::Content
    );
    assert_eq!(
        FrameBoundary::AttributeValue.context(),
        ReferenceContext::AttributeValue
    );
    assert_eq!(
        FrameBoundary::EntityValue.context(),
        ReferenceContext::EntityValue
    );
    assert_eq!(
        FrameBoundary::DtdSubset.context(),
        ReferenceContext::DtdSubset
    );

    let frames = EntityFrames::new(FrameLimits::default());
    assert_eq!(frames.depth(), 0);
    assert_eq!(frames.counters(), FrameCounters::default());
}

#[test]
fn fragment_origins_join_only_exact_contiguous_sources() {
    let input = FragmentOrigin::input(2..5)
        .unwrap()
        .join(FragmentOrigin::input(5..9).unwrap());
    assert_eq!(input.input_range(), Some(2..9));
    assert_eq!(input.replacement_range(), None);
    assert_eq!(input.physical_context(), 2..9);

    let invocation = EntityInvocationId(NonZeroU64::new(4).unwrap());
    let replacement = FragmentOrigin::Replacement {
        declaration: EntityDeclarationId::from_index(1),
        invocation,
        local: span(0, 2),
        physical_reference: span(10, 13),
    }
    .join(FragmentOrigin::Replacement {
        declaration: EntityDeclarationId::from_index(1),
        invocation,
        local: span(2, 6),
        physical_reference: span(10, 13),
    });
    assert_eq!(replacement.replacement_range(), Some(0..6));
    assert_eq!(replacement.input_range(), None);
    assert_eq!(replacement.physical_context(), 10..13);

    let different_invocation = FragmentOrigin::Replacement {
        declaration: EntityDeclarationId::from_index(1),
        invocation: EntityInvocationId(NonZeroU64::new(5).unwrap()),
        local: span(0, 1),
        physical_reference: span(20, 23),
    };
    assert_eq!(
        replacement.join(different_invocation),
        FragmentOrigin::Mixed {
            physical_context: span(10, 23)
        }
    );
    assert_eq!(
        FragmentOrigin::input(0..1)
            .unwrap()
            .join(FragmentOrigin::input(2..3).unwrap()),
        FragmentOrigin::Mixed {
            physical_context: span(0, 3)
        }
    );
    assert!(ByteSpan::new(std::ops::Range { start: 2, end: 1 }).is_none());
}

#[test]
fn prototype_layouts_are_recorded() {
    eprintln!(
        "EntityFrame={}/{} FragmentOrigin={}/{} EntityFrames={}/{} SourceOrigin={}/{}",
        size_of::<EntityFrame>(),
        align_of::<EntityFrame>(),
        size_of::<FragmentOrigin>(),
        align_of::<FragmentOrigin>(),
        size_of::<EntityFrames>(),
        align_of::<EntityFrames>(),
        size_of::<crate::SourceOrigin>(),
        align_of::<crate::SourceOrigin>(),
    );
}

#[test]
fn physical_and_nested_frames_preserve_identity_cursor_and_outer_context() {
    let parent_id = EntityDeclarationId::from_index(1);
    let child_id = EntityDeclarationId::from_index(2);
    let parent = "pre&b;post";
    let child = "é";
    let boundary = FrameBoundary::Content { element_depth: 4 };
    let mut frames = unlimited();

    let parent_invocation = frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 1, parent),
            20..23,
            boundary,
        )
        .unwrap();
    assert_eq!(parent_invocation.get(), 1);
    assert_eq!(frames.depth(), 1);
    assert_eq!(
        frames.current(parent_id, parent).unwrap(),
        CurrentSource {
            declaration: parent_id,
            invocation: parent_invocation,
            local_start: 0,
            remaining: parent,
            physical_reference: 20..23,
        }
    );

    let prefix = frames.advance_current(parent_id, parent, 3).unwrap();
    assert_eq!(prefix.replacement_range(), Some(0..3));
    let child_invocation = frames
        .enter_nested(
            EntityKind::General,
            internal(EntityKind::General, 2, child),
            parent_id,
            parent,
            3..6,
            boundary,
        )
        .unwrap();
    assert_eq!(child_invocation.get(), 2);
    assert_eq!(frames.depth(), 2);
    let child_origin = frames
        .advance_current(child_id, child, child.len())
        .unwrap();
    assert_eq!(child_origin.replacement_range(), Some(0..2));
    assert_eq!(child_origin.physical_context(), 20..23);
    assert_eq!(
        frames.finish_current(boundary).unwrap(),
        CompletedFrame {
            declaration: child_id,
            invocation: child_invocation,
            physical_reference: 20..23,
        }
    );
    assert_eq!(frames.current(parent_id, parent).unwrap().remaining, "post");
    let suffix = frames.advance_current(parent_id, parent, 4).unwrap();
    assert_eq!(suffix.replacement_range(), Some(6..10));
    frames.finish_current(boundary).unwrap();
    assert_eq!(frames.depth(), 0);
    assert_eq!(
        frames.counters(),
        FrameCounters {
            reference_attempts: 2,
            work: 15,
            expanded_bytes: 12,
        }
    );

    assert!(matches!(
        prefix.join(child_origin).join(suffix),
        FragmentOrigin::Mixed { .. }
    ));
}

#[test]
fn repeated_and_empty_replacements_get_fresh_occurrences_and_charge_work() {
    let empty = "";
    let boundary = FrameBoundary::AttributeValue;
    let mut frames = unlimited();
    let first = frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 8, empty),
            0..3,
            boundary,
        )
        .unwrap();
    assert_eq!(
        frames
            .current(EntityDeclarationId::from_index(8), empty)
            .unwrap()
            .remaining,
        ""
    );
    frames.finish_current(boundary).unwrap();
    let second = frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 8, empty),
            9..12,
            boundary,
        )
        .unwrap();
    frames.finish_current(boundary).unwrap();
    assert_ne!(first, second);
    assert_eq!(first.get(), 1);
    assert_eq!(second.get(), 2);
    assert_eq!(
        frames.counters(),
        FrameCounters {
            reference_attempts: 2,
            work: 2,
            expanded_bytes: 0,
        }
    );
}

#[test]
fn context_matrix_and_lookup_failures_are_typed_and_counted() {
    let contexts = [
        FrameBoundary::Content { element_depth: 0 },
        FrameBoundary::AttributeValue,
        FrameBoundary::EntityValue,
        FrameBoundary::DtdSubset,
    ];
    for kind in [EntityKind::General, EntityKind::Parameter] {
        for boundary in contexts {
            let allowed = matches!(
                (kind, boundary.context()),
                (
                    EntityKind::General,
                    ReferenceContext::Content | ReferenceContext::AttributeValue
                ) | (
                    EntityKind::Parameter,
                    ReferenceContext::EntityValue | ReferenceContext::DtdSubset
                )
            );
            let mut frames = unlimited();
            let actual = frames.enter_physical(kind, internal(kind, 1, ""), 30..33, boundary);
            if allowed {
                actual.unwrap();
                frames.finish_current(boundary).unwrap();
            } else {
                assert_eq!(
                    actual.unwrap_err(),
                    FrameError {
                        kind: FrameErrorKind::InvalidContext,
                        physical_offset: 30,
                    }
                );
            }
            assert_eq!(frames.counters().reference_attempts, 1);
            assert_eq!(frames.counters().work, 1);
        }
    }

    for failure in [
        LookupFailure::Undeclared,
        LookupFailure::ExternalUnavailable,
        LookupFailure::Unparsed,
        LookupFailure::Unread,
    ] {
        let mut frames = unlimited();
        assert_eq!(
            frames
                .enter_physical(
                    EntityKind::General,
                    EntityResolution::Failed {
                        kind: EntityKind::General,
                        failure,
                    },
                    40..43,
                    FrameBoundary::Content { element_depth: 0 },
                )
                .unwrap_err(),
            FrameError {
                kind: FrameErrorKind::Lookup(failure),
                physical_offset: 40,
            }
        );
        assert_eq!(frames.counters().reference_attempts, 1);
        assert_eq!(frames.depth(), 0);
    }

    let mut frames = unlimited();
    assert_eq!(
        frames
            .enter_physical(
                EntityKind::General,
                internal(EntityKind::Parameter, 1, ""),
                50..53,
                FrameBoundary::Content { element_depth: 0 },
            )
            .unwrap_err()
            .kind,
        FrameErrorKind::KindMismatch
    );
}

#[test]
fn cycles_and_source_mismatches_do_not_publish_frames_or_parent_cursors() {
    let parent_id = EntityDeclarationId::from_index(1);
    let parent = "&self;";
    let boundary = FrameBoundary::Content { element_depth: 2 };
    let mut frames = unlimited();
    frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 1, parent),
            100..106,
            boundary,
        )
        .unwrap();
    assert_eq!(
        frames
            .enter_nested(
                EntityKind::General,
                internal(EntityKind::General, 1, parent),
                parent_id,
                parent,
                0..6,
                boundary,
            )
            .unwrap_err(),
        FrameError {
            kind: FrameErrorKind::Cycle,
            physical_offset: 100,
        }
    );
    assert_eq!(frames.depth(), 1);
    assert_eq!(frames.current(parent_id, parent).unwrap().local_start, 0);

    for (declaration, replacement, reference) in [
        (EntityDeclarationId::from_index(9), parent, 0..6),
        (parent_id, parent, 1..6),
        (parent_id, parent, 0..7),
    ] {
        assert_eq!(
            frames
                .enter_nested(
                    EntityKind::General,
                    internal(EntityKind::General, 2, ""),
                    declaration,
                    replacement,
                    reference,
                    boundary,
                )
                .unwrap_err()
                .kind,
            FrameErrorKind::SourceMismatch
        );
        assert_eq!(frames.depth(), 1);
        assert_eq!(frames.current(parent_id, parent).unwrap().local_start, 0);
    }
}

#[test]
fn completion_requires_full_input_and_exact_lexical_boundary() {
    let id = EntityDeclarationId::from_index(3);
    let replacement = "x";
    let entry = FrameBoundary::Content { element_depth: 7 };
    let mut frames = unlimited();
    frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 3, replacement),
            70..73,
            entry,
        )
        .unwrap();
    assert_eq!(
        frames.finish_current(entry).unwrap_err().kind,
        FrameErrorKind::PrematureEnd
    );
    frames.advance_current(id, replacement, 1).unwrap();
    assert_eq!(
        frames
            .finish_current(FrameBoundary::AttributeValue)
            .unwrap_err()
            .kind,
        FrameErrorKind::BoundaryMismatch
    );
    assert_eq!(
        frames
            .finish_current(FrameBoundary::Content { element_depth: 8 })
            .unwrap_err()
            .kind,
        FrameErrorKind::ElementDepthMismatch
    );
    assert_eq!(frames.depth(), 1);
    frames.finish_current(entry).unwrap();
}

#[test]
fn end_tag_guard_prevents_close_then_reopen_depth_restoration() {
    let id = EntityDeclarationId::from_index(12);
    let replacement = "</outer><b>";
    let boundary = FrameBoundary::Content { element_depth: 1 };
    let mut frames = unlimited();
    assert!(frames.check_before_end_element(1).is_ok());
    frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 12, replacement),
            90..93,
            boundary,
        )
        .unwrap();
    assert_eq!(
        frames.check_before_end_element(1).unwrap_err(),
        FrameError {
            kind: FrameErrorKind::ElementBoundaryCrossing,
            physical_offset: 90,
        }
    );
    frames.check_before_end_element(2).unwrap();
    assert_eq!(frames.current(id, replacement).unwrap().local_start, 0);

    let mut attribute = unlimited();
    attribute
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 13, ""),
            100..103,
            FrameBoundary::AttributeValue,
        )
        .unwrap();
    assert_eq!(
        attribute.check_before_end_element(2).unwrap_err().kind,
        FrameErrorKind::BoundaryMismatch
    );
}

#[test]
fn current_source_and_advancement_reject_wrong_or_split_utf8_storage() {
    let id = EntityDeclarationId::from_index(4);
    let replacement = "éx";
    let boundary = FrameBoundary::AttributeValue;
    let mut frames = unlimited();
    frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 4, replacement),
            5..8,
            boundary,
        )
        .unwrap();
    assert_eq!(
        frames
            .current(EntityDeclarationId::from_index(5), replacement)
            .unwrap_err()
            .kind,
        FrameErrorKind::SourceMismatch
    );
    assert_eq!(
        frames.current(id, "ab").unwrap_err().kind,
        FrameErrorKind::SourceMismatch
    );
    for bytes in [0, 1, 4, usize::MAX] {
        assert_eq!(
            frames
                .advance_current(id, replacement, bytes)
                .unwrap_err()
                .kind,
            FrameErrorKind::InvalidSpan,
            "byte count {bytes}"
        );
        assert_eq!(frames.current(id, replacement).unwrap().local_start, 0);
    }
    frames.advance_current(id, replacement, 2).unwrap();
    frames.advance_current(id, replacement, 1).unwrap();
    frames.finish_current(boundary).unwrap();
    assert_eq!(
        frames.current(id, replacement).unwrap_err().kind,
        FrameErrorKind::NoActiveFrame
    );
    assert_eq!(
        frames.finish_current(boundary).unwrap_err().kind,
        FrameErrorKind::NoActiveFrame
    );
}

#[test]
fn independent_limits_have_exact_boundaries_and_atomic_failures() {
    let boundary = FrameBoundary::Content { element_depth: 0 };
    let empty = internal(EntityKind::General, 1, "");
    for (limits, expected) in [
        (
            FrameLimits {
                max_reference_attempts: 0,
                ..FrameLimits::default()
            },
            FrameLimit::ReferenceAttempts,
        ),
        (
            FrameLimits {
                max_work: 0,
                ..FrameLimits::default()
            },
            FrameLimit::Work,
        ),
        (
            FrameLimits {
                max_active_depth: 0,
                ..FrameLimits::default()
            },
            FrameLimit::ActiveDepth,
        ),
    ] {
        let mut frames = EntityFrames::new(limits);
        assert_eq!(
            frames
                .enter_physical(EntityKind::General, empty, 10..13, boundary)
                .unwrap_err(),
            FrameError {
                kind: FrameErrorKind::LimitExceeded(expected),
                physical_offset: 10,
            }
        );
        assert_eq!(frames.depth(), 0);
        if expected == FrameLimit::ActiveDepth {
            assert_eq!(frames.counters().reference_attempts, 1);
            assert_eq!(frames.counters().work, 1);
        } else {
            assert_eq!(frames.counters(), FrameCounters::default());
        }
    }

    let id = EntityDeclarationId::from_index(2);
    let mut frames = EntityFrames::new(FrameLimits {
        max_expanded_bytes: 1,
        ..FrameLimits::default()
    });
    frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 2, "ab"),
            20..23,
            boundary,
        )
        .unwrap();
    frames.advance_current(id, "ab", 1).unwrap();
    let before = frames.counters();
    assert_eq!(
        frames.advance_current(id, "ab", 1).unwrap_err().kind,
        FrameErrorKind::LimitExceeded(FrameLimit::ExpandedBytes)
    );
    assert_eq!(frames.counters(), before);
    assert_eq!(frames.current(id, "ab").unwrap().local_start, 1);

    let parent_id = EntityDeclarationId::from_index(3);
    let mut frames = EntityFrames::new(FrameLimits {
        max_expanded_bytes: 2,
        ..FrameLimits::default()
    });
    frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 3, "&x;"),
            30..33,
            boundary,
        )
        .unwrap();
    let before = frames.counters();
    assert_eq!(
        frames
            .enter_nested(
                EntityKind::General,
                internal(EntityKind::General, 4, ""),
                parent_id,
                "&x;",
                0..3,
                boundary,
            )
            .unwrap_err()
            .kind,
        FrameErrorKind::LimitExceeded(FrameLimit::ExpandedBytes)
    );
    assert_eq!(frames.counters(), before);
    assert_eq!(frames.current(parent_id, "&x;").unwrap().local_start, 0);
}

#[test]
fn checked_counter_and_invocation_overflow_are_explicit() {
    let limits = FrameLimits {
        max_active_depth: usize::MAX,
        max_reference_attempts: usize::MAX,
        max_work: usize::MAX,
        max_expanded_bytes: usize::MAX,
    };
    let boundary = FrameBoundary::AttributeValue;
    for (counters, expected) in [
        (
            FrameCounters {
                reference_attempts: usize::MAX,
                work: 0,
                expanded_bytes: 0,
            },
            FrameCounter::ReferenceAttempts,
        ),
        (
            FrameCounters {
                reference_attempts: 0,
                work: usize::MAX,
                expanded_bytes: 0,
            },
            FrameCounter::Work,
        ),
    ] {
        let mut frames = EntityFrames::new(limits);
        frames.counters = counters;
        assert_eq!(
            frames
                .enter_physical(
                    EntityKind::General,
                    internal(EntityKind::General, 1, ""),
                    60..63,
                    boundary,
                )
                .unwrap_err()
                .kind,
            FrameErrorKind::CounterOverflow(expected)
        );
        assert_eq!(frames.counters(), counters);
    }

    let id = EntityDeclarationId::from_index(1);
    let mut frames = EntityFrames::new(limits);
    frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 1, "x"),
            70..73,
            boundary,
        )
        .unwrap();
    frames.counters.expanded_bytes = usize::MAX;
    assert_eq!(
        frames.advance_current(id, "x", 1).unwrap_err().kind,
        FrameErrorKind::CounterOverflow(FrameCounter::ExpandedBytes)
    );
    assert_eq!(frames.current(id, "x").unwrap().local_start, 0);

    let mut frames = EntityFrames::new(limits);
    frames.next_invocation = None;
    assert_eq!(
        frames
            .enter_physical(
                EntityKind::General,
                internal(EntityKind::General, 1, ""),
                80..83,
                boundary,
            )
            .unwrap_err()
            .kind,
        FrameErrorKind::InvocationIdsExhausted
    );
    assert_eq!(frames.depth(), 0);
}

#[test]
fn reference_spans_and_source_modes_are_checked_before_publication() {
    let boundary = FrameBoundary::Content { element_depth: 0 };
    let mut frames = unlimited();
    for reference in [4..4, std::ops::Range { start: 9, end: 8 }] {
        assert_eq!(
            frames
                .enter_physical(
                    EntityKind::General,
                    internal(EntityKind::General, 1, ""),
                    reference,
                    boundary,
                )
                .unwrap_err()
                .kind,
            FrameErrorKind::InvalidSpan
        );
    }
    assert_eq!(frames.counters(), FrameCounters::default());
    assert_eq!(
        frames
            .enter_nested(
                EntityKind::General,
                internal(EntityKind::General, 1, ""),
                EntityDeclarationId::from_index(0),
                "",
                0..1,
                boundary,
            )
            .unwrap_err()
            .kind,
        FrameErrorKind::NoActiveFrame
    );
    frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 1, ""),
            10..13,
            boundary,
        )
        .unwrap();
    assert_eq!(
        frames
            .enter_physical(
                EntityKind::General,
                internal(EntityKind::General, 2, ""),
                20..23,
                boundary,
            )
            .unwrap_err()
            .kind,
        FrameErrorKind::SourceMismatch
    );
}

#[test]
fn a_deep_stack_releases_iteratively_in_reverse_order() {
    let boundary = FrameBoundary::Content { element_depth: 1 };
    let replacement = "&x;";
    let mut frames = unlimited();
    frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 0, replacement),
            100..103,
            boundary,
        )
        .unwrap();
    for declaration in 1..32 {
        frames
            .enter_nested(
                EntityKind::General,
                internal(EntityKind::General, declaration, replacement),
                EntityDeclarationId::from_index(declaration - 1),
                replacement,
                0..3,
                boundary,
            )
            .unwrap();
    }
    assert_eq!(frames.depth(), 32);
    frames
        .advance_current(EntityDeclarationId::from_index(31), replacement, 3)
        .unwrap();
    for declaration in (0..32).rev() {
        assert_eq!(
            frames.finish_current(boundary).unwrap().declaration,
            EntityDeclarationId::from_index(declaration)
        );
    }
    assert_eq!(frames.depth(), 0);
}
