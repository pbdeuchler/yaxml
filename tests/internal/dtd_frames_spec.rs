use std::num::NonZeroU64;

use super::*;

fn id(index: usize) -> EntityDeclarationId {
    EntityDeclarationId::from_index(index)
}

fn internal(kind: EntityKind, declaration: usize, replacement: &str) -> EntityResolution {
    EntityResolution::Internal {
        kind,
        declaration: id(declaration),
        replacement_len: replacement.len(),
    }
}

fn failed(kind: EntityKind, failure: LookupFailure) -> EntityResolution {
    EntityResolution::Failed { kind, failure }
}

fn loose() -> EntityFrames {
    EntityFrames::new(FrameLimits {
        max_active_depth: 32,
        max_reference_attempts: 1024,
        max_work: 8192,
        max_expanded_bytes: 8192,
    })
}

fn span(start: usize, end: usize) -> ByteSpan {
    ByteSpan::new(start..end).unwrap()
}

fn invocation(value: u64) -> EntityInvocationId {
    EntityInvocationId(NonZeroU64::new(value).unwrap())
}

fn replacement_origin(
    declaration: usize,
    invocation_id: u64,
    local: std::ops::Range<usize>,
    physical: std::ops::Range<usize>,
) -> FragmentOrigin {
    FragmentOrigin::Replacement {
        declaration: id(declaration),
        invocation: invocation(invocation_id),
        local: span(local.start, local.end),
        physical_reference: span(physical.start, physical.end),
    }
}

#[test]
fn repeated_declaration_uses_have_fresh_invocations_and_empty_uses_still_cost() {
    let boundary = FrameBoundary::AttributeValue;
    let mut frames = loose();
    let first = frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 7, ""),
            10..13,
            boundary,
        )
        .unwrap();
    assert_eq!(first.get(), 1);
    assert_eq!(
        frames.finish_current(boundary).unwrap(),
        CompletedFrame {
            declaration: id(7),
            invocation: first,
            physical_reference: 10..13,
        }
    );

    let second = frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 7, ""),
            20..23,
            boundary,
        )
        .unwrap();
    assert_eq!(second.get(), 2);
    frames.finish_current(boundary).unwrap();
    assert_ne!(first, second);
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
fn unicode_nested_sources_keep_byte_cursors_origins_and_exact_counters() {
    let parent = "π&子;z";
    let child = "🙂";
    let boundary = FrameBoundary::Content { element_depth: 2 };
    let mut frames = loose();
    let parent_invocation = frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 1, parent),
            100..106,
            boundary,
        )
        .unwrap();
    let prefix = frames.advance_current(id(1), parent, 2).unwrap();
    assert_eq!(prefix.replacement_range(), Some(0..2));
    let child_invocation = frames
        .enter_nested(
            EntityKind::General,
            internal(EntityKind::General, 2, child),
            id(1),
            parent,
            2..7,
            boundary,
        )
        .unwrap();
    assert_eq!(parent_invocation.get(), 1);
    assert_eq!(child_invocation.get(), 2);
    assert_eq!(
        frames.current(id(2), child).unwrap(),
        CurrentSource {
            declaration: id(2),
            invocation: child_invocation,
            local_start: 0,
            remaining: child,
            physical_reference: 100..106,
        }
    );
    let nested = frames.advance_current(id(2), child, 4).unwrap();
    assert_eq!(nested.replacement_range(), Some(0..4));
    assert_eq!(nested.physical_context(), 100..106);
    frames.finish_current(boundary).unwrap();
    assert_eq!(frames.current(id(1), parent).unwrap().local_start, 7);
    assert_eq!(frames.current(id(1), parent).unwrap().remaining, "z");
    let suffix = frames.advance_current(id(1), parent, 1).unwrap();
    assert_eq!(suffix.replacement_range(), Some(7..8));
    frames.finish_current(boundary).unwrap();
    assert_eq!(
        frames.counters(),
        FrameCounters {
            reference_attempts: 2,
            work: 15,
            expanded_bytes: 12,
        }
    );
    assert_eq!(
        prefix.join(nested).join(suffix).physical_context(),
        100..106
    );
}

#[test]
fn kind_context_and_lookup_matrix_is_typed_and_accounted() {
    let cases = [
        (
            EntityKind::General,
            FrameBoundary::Content { element_depth: 0 },
            true,
        ),
        (EntityKind::General, FrameBoundary::AttributeValue, true),
        (EntityKind::General, FrameBoundary::EntityValue, false),
        (EntityKind::General, FrameBoundary::DtdSubset, false),
        (
            EntityKind::Parameter,
            FrameBoundary::Content { element_depth: 0 },
            false,
        ),
        (EntityKind::Parameter, FrameBoundary::AttributeValue, false),
        (EntityKind::Parameter, FrameBoundary::EntityValue, true),
        (EntityKind::Parameter, FrameBoundary::DtdSubset, true),
    ];
    for (ordinal, (kind, boundary, allowed)) in cases.into_iter().enumerate() {
        let mut frames = loose();
        let result = frames.enter_physical(kind, internal(kind, ordinal, ""), 30..33, boundary);
        if allowed {
            assert_eq!(result.unwrap().get(), 1);
            assert_eq!(
                frames.finish_current(boundary).unwrap().declaration,
                id(ordinal)
            );
        } else {
            assert_eq!(
                result.unwrap_err(),
                FrameError {
                    kind: FrameErrorKind::InvalidContext,
                    physical_offset: 30,
                }
            );
            assert_eq!(frames.depth(), 0);
        }
        assert_eq!(
            frames.counters(),
            FrameCounters {
                reference_attempts: 1,
                work: 1,
                expanded_bytes: 0,
            }
        );
    }

    for failure in [
        LookupFailure::Undeclared,
        LookupFailure::ExternalUnavailable,
        LookupFailure::Unparsed,
        LookupFailure::Unread,
    ] {
        let mut frames = loose();
        assert_eq!(
            frames
                .enter_physical(
                    EntityKind::General,
                    failed(EntityKind::General, failure),
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
        assert_eq!(frames.counters().work, 1);
    }

    let mut frames = loose();
    assert_eq!(
        frames
            .enter_physical(
                EntityKind::General,
                internal(EntityKind::Parameter, 19, ""),
                50..53,
                FrameBoundary::Content { element_depth: 0 },
            )
            .unwrap_err(),
        FrameError {
            kind: FrameErrorKind::KindMismatch,
            physical_offset: 50,
        }
    );
    assert_eq!(frames.counters().reference_attempts, 1);
}

#[test]
fn reference_attempt_limit_counts_empty_and_failed_lookups_exactly() {
    let boundary = FrameBoundary::AttributeValue;
    let mut zero = EntityFrames::new(FrameLimits {
        max_reference_attempts: 0,
        max_work: usize::MAX,
        max_expanded_bytes: usize::MAX,
        max_active_depth: usize::MAX,
    });
    assert_eq!(
        zero.enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 1, ""),
            1..4,
            boundary,
        )
        .unwrap_err()
        .kind,
        FrameErrorKind::LimitExceeded(FrameLimit::ReferenceAttempts)
    );
    assert_eq!(zero.counters(), FrameCounters::default());

    let mut one = EntityFrames::new(FrameLimits {
        max_reference_attempts: 1,
        max_work: usize::MAX,
        max_expanded_bytes: usize::MAX,
        max_active_depth: usize::MAX,
    });
    one.enter_physical(
        EntityKind::General,
        internal(EntityKind::General, 1, ""),
        1..4,
        boundary,
    )
    .unwrap();
    one.finish_current(boundary).unwrap();
    let before = one.counters();
    assert_eq!(
        one.enter_physical(
            EntityKind::General,
            failed(EntityKind::General, LookupFailure::Undeclared),
            5..8,
            boundary,
        )
        .unwrap_err()
        .kind,
        FrameErrorKind::LimitExceeded(FrameLimit::ReferenceAttempts)
    );
    assert_eq!(one.counters(), before);

    let mut two = EntityFrames::new(FrameLimits {
        max_reference_attempts: 2,
        max_work: usize::MAX,
        max_expanded_bytes: usize::MAX,
        max_active_depth: usize::MAX,
    });
    two.enter_physical(
        EntityKind::General,
        internal(EntityKind::General, 1, ""),
        1..4,
        boundary,
    )
    .unwrap();
    two.finish_current(boundary).unwrap();
    assert_eq!(
        two.enter_physical(
            EntityKind::General,
            failed(EntityKind::General, LookupFailure::Undeclared),
            5..8,
            boundary,
        )
        .unwrap_err()
        .kind,
        FrameErrorKind::Lookup(LookupFailure::Undeclared)
    );
    assert_eq!(two.counters().reference_attempts, 2);
    assert_eq!(two.counters().work, 2);
}

#[test]
fn work_limit_counts_lookup_spelling_and_active_comparisons_independently() {
    let boundary = FrameBoundary::Content { element_depth: 1 };
    let limits = |max_work| FrameLimits {
        max_active_depth: usize::MAX,
        max_reference_attempts: usize::MAX,
        max_work,
        max_expanded_bytes: usize::MAX,
    };
    let mut zero = EntityFrames::new(limits(0));
    assert_eq!(
        zero.enter_physical(
            EntityKind::General,
            failed(EntityKind::General, LookupFailure::Unread),
            10..13,
            boundary,
        )
        .unwrap_err()
        .kind,
        FrameErrorKind::LimitExceeded(FrameLimit::Work)
    );
    assert_eq!(zero.counters(), FrameCounters::default());

    let mut exact_lookup = EntityFrames::new(limits(1));
    assert_eq!(
        exact_lookup
            .enter_physical(
                EntityKind::General,
                failed(EntityKind::General, LookupFailure::Unread),
                10..13,
                boundary,
            )
            .unwrap_err()
            .kind,
        FrameErrorKind::Lookup(LookupFailure::Unread)
    );
    assert_eq!(exact_lookup.counters().work, 1);

    for (maximum, expected) in [
        (5, FrameErrorKind::LimitExceeded(FrameLimit::Work)),
        (6, FrameErrorKind::Cycle),
    ] {
        let mut frames = EntityFrames::new(limits(maximum));
        frames
            .enter_physical(
                EntityKind::General,
                internal(EntityKind::General, 1, "&x;"),
                20..23,
                boundary,
            )
            .unwrap();
        let result = frames.enter_nested(
            EntityKind::General,
            internal(EntityKind::General, 1, "&x;"),
            id(1),
            "&x;",
            0..3,
            boundary,
        );
        assert_eq!(result.unwrap_err().kind, expected);
        assert_eq!(frames.depth(), 1);
        assert_eq!(frames.current(id(1), "&x;").unwrap().local_start, 0);
        assert_eq!(
            frames.counters(),
            if maximum == 5 {
                FrameCounters {
                    reference_attempts: 1,
                    work: 1,
                    expanded_bytes: 0,
                }
            } else {
                FrameCounters {
                    reference_attempts: 2,
                    work: 6,
                    expanded_bytes: 3,
                }
            }
        );
    }
}

#[test]
fn expanded_byte_limit_counts_nested_spelling_and_utf8_bytes_atomically() {
    let boundary = FrameBoundary::AttributeValue;
    let limits = |maximum| FrameLimits {
        max_active_depth: usize::MAX,
        max_reference_attempts: usize::MAX,
        max_work: usize::MAX,
        max_expanded_bytes: maximum,
    };
    for (maximum, succeeds) in [(4, false), (5, true)] {
        let mut frames = EntityFrames::new(limits(maximum));
        frames
            .enter_physical(
                EntityKind::General,
                internal(EntityKind::General, 1, "&子;"),
                20..25,
                boundary,
            )
            .unwrap();
        let before = frames.counters();
        let result = frames.enter_nested(
            EntityKind::General,
            internal(EntityKind::General, 2, ""),
            id(1),
            "&子;",
            0..5,
            boundary,
        );
        if succeeds {
            result.unwrap();
            assert_eq!(frames.counters().expanded_bytes, 5);
            frames.finish_current(boundary).unwrap();
            assert_eq!(frames.current(id(1), "&子;").unwrap().local_start, 5);
            frames.finish_current(boundary).unwrap();
        } else {
            assert_eq!(
                result.unwrap_err().kind,
                FrameErrorKind::LimitExceeded(FrameLimit::ExpandedBytes)
            );
            assert_eq!(frames.counters(), before);
            assert_eq!(frames.current(id(1), "&子;").unwrap().local_start, 0);
        }
    }

    let replacement = "π🙂";
    let mut short = EntityFrames::new(limits(5));
    short
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 3, replacement),
            30..33,
            boundary,
        )
        .unwrap();
    short.advance_current(id(3), replacement, 2).unwrap();
    let before = short.counters();
    assert_eq!(
        short
            .advance_current(id(3), replacement, 4)
            .unwrap_err()
            .kind,
        FrameErrorKind::LimitExceeded(FrameLimit::ExpandedBytes)
    );
    assert_eq!(short.counters(), before);
    assert_eq!(short.current(id(3), replacement).unwrap().local_start, 2);

    let mut exact = EntityFrames::new(limits(6));
    exact
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 3, replacement),
            30..33,
            boundary,
        )
        .unwrap();
    exact.advance_current(id(3), replacement, 2).unwrap();
    exact.advance_current(id(3), replacement, 4).unwrap();
    exact.finish_current(boundary).unwrap();
    assert_eq!(exact.counters().expanded_bytes, 6);
}

#[test]
fn direct_indirect_cycles_and_depth_boundaries_preserve_parent_state() {
    let boundary = FrameBoundary::Content { element_depth: 2 };
    let limits = FrameLimits {
        max_active_depth: 2,
        max_reference_attempts: usize::MAX,
        max_work: usize::MAX,
        max_expanded_bytes: usize::MAX,
    };
    let mut frames = EntityFrames::new(limits);
    frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 1, "&b;"),
            90..93,
            boundary,
        )
        .unwrap();
    frames
        .enter_nested(
            EntityKind::General,
            internal(EntityKind::General, 2, "&a;"),
            id(1),
            "&b;",
            0..3,
            boundary,
        )
        .unwrap();
    assert_eq!(
        frames.counters(),
        FrameCounters {
            reference_attempts: 2,
            work: 6,
            expanded_bytes: 3,
        }
    );
    assert_eq!(
        frames
            .enter_nested(
                EntityKind::General,
                internal(EntityKind::General, 1, "&b;"),
                id(2),
                "&a;",
                0..3,
                boundary,
            )
            .unwrap_err(),
        FrameError {
            kind: FrameErrorKind::Cycle,
            physical_offset: 90,
        }
    );
    assert_eq!(frames.depth(), 2);
    assert_eq!(frames.current(id(2), "&a;").unwrap().local_start, 0);
    assert_eq!(
        frames.counters(),
        FrameCounters {
            reference_attempts: 3,
            work: 12,
            expanded_bytes: 6,
        }
    );

    assert_eq!(
        frames
            .enter_nested(
                EntityKind::General,
                internal(EntityKind::General, 3, ""),
                id(2),
                "&a;",
                0..3,
                boundary,
            )
            .unwrap_err(),
        FrameError {
            kind: FrameErrorKind::LimitExceeded(FrameLimit::ActiveDepth),
            physical_offset: 90,
        }
    );
    assert_eq!(frames.depth(), 2);
    assert_eq!(frames.current(id(2), "&a;").unwrap().local_start, 0);
    assert_eq!(
        frames.counters(),
        FrameCounters {
            reference_attempts: 4,
            work: 18,
            expanded_bytes: 9,
        }
    );
}

#[test]
fn source_identity_lengths_utf8_boundaries_and_nested_ranges_are_checked() {
    let boundary = FrameBoundary::AttributeValue;
    let replacement = "é🙂z";
    let mut frames = loose();
    assert_eq!(
        frames.current(id(4), replacement).unwrap_err().kind,
        FrameErrorKind::NoActiveFrame
    );
    frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 4, replacement),
            70..73,
            boundary,
        )
        .unwrap();
    for (declaration, source) in [(id(5), replacement), (id(4), "short")] {
        assert_eq!(
            frames.current(declaration, source).unwrap_err(),
            FrameError {
                kind: FrameErrorKind::SourceMismatch,
                physical_offset: 70,
            }
        );
    }
    for count in [0, 1, replacement.len() + 1, usize::MAX] {
        assert_eq!(
            frames
                .advance_current(id(4), replacement, count)
                .unwrap_err()
                .kind,
            FrameErrorKind::InvalidSpan
        );
        assert_eq!(frames.current(id(4), replacement).unwrap().local_start, 0);
    }
    frames.advance_current(id(4), replacement, 2).unwrap();
    assert_eq!(
        frames.current(id(4), "aéxxxx").unwrap_err().kind,
        FrameErrorKind::SourceMismatch
    );
    for range in [0..6, 3..6, 2..5, 2..8] {
        assert_eq!(
            frames
                .enter_nested(
                    EntityKind::General,
                    internal(EntityKind::General, 6, ""),
                    id(4),
                    replacement,
                    range,
                    boundary,
                )
                .unwrap_err()
                .kind,
            FrameErrorKind::SourceMismatch
        );
        assert_eq!(frames.current(id(4), replacement).unwrap().local_start, 2);
    }
    assert_eq!(
        frames
            .enter_nested(
                EntityKind::General,
                internal(EntityKind::General, 6, ""),
                id(4),
                replacement,
                2..2,
                boundary,
            )
            .unwrap_err()
            .kind,
        FrameErrorKind::InvalidSpan
    );
    frames
        .enter_nested(
            EntityKind::General,
            internal(EntityKind::General, 6, ""),
            id(4),
            replacement,
            2..6,
            boundary,
        )
        .unwrap();
    frames.finish_current(boundary).unwrap();
    assert_eq!(frames.current(id(4), replacement).unwrap().remaining, "z");
    frames.advance_current(id(4), replacement, 1).unwrap();
    frames.finish_current(boundary).unwrap();
}

#[test]
fn completion_failures_keep_the_frame_for_exact_recovery() {
    let entry = FrameBoundary::Content { element_depth: 7 };
    let mut frames = loose();
    frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 3, "x"),
            80..83,
            entry,
        )
        .unwrap();
    assert_eq!(
        frames.finish_current(entry).unwrap_err().kind,
        FrameErrorKind::PrematureEnd
    );
    assert_eq!(frames.depth(), 1);
    frames.advance_current(id(3), "x", 1).unwrap();
    let counters = frames.counters();
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
    assert_eq!(frames.counters(), counters);
    assert_eq!(
        frames.finish_current(entry).unwrap().physical_reference,
        80..83
    );
    assert_eq!(frames.depth(), 0);
}

#[test]
fn close_then_reopen_is_rejected_before_the_outer_end_tag() {
    let replacement = "</outer><b>";
    let entry = FrameBoundary::Content { element_depth: 1 };
    let mut guarded = loose();
    guarded
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 12, replacement),
            90..93,
            entry,
        )
        .unwrap();
    assert_eq!(
        guarded.check_before_end_element(1).unwrap_err(),
        FrameError {
            kind: FrameErrorKind::ElementBoundaryCrossing,
            physical_offset: 90,
        }
    );
    guarded.check_before_end_element(2).unwrap();
    assert_eq!(guarded.current(id(12), replacement).unwrap().local_start, 0);

    // The stack only sees the final numeric depth. This deliberately proves why
    // the future controller must call the guard before publishing every end tag.
    let mut final_depth_only = loose();
    final_depth_only
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 12, replacement),
            90..93,
            entry,
        )
        .unwrap();
    final_depth_only
        .advance_current(id(12), replacement, replacement.len())
        .unwrap();
    final_depth_only.finish_current(entry).unwrap();

    let mut attribute = loose();
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
fn origin_join_table_preserves_only_exact_contiguous_sources() {
    let contiguous_input = FragmentOrigin::input(0..2)
        .unwrap()
        .join(FragmentOrigin::input(2..5).unwrap());
    assert_eq!(contiguous_input.input_range(), Some(0..5));
    assert_eq!(contiguous_input.replacement_range(), None);
    assert_eq!(contiguous_input.physical_context(), 0..5);

    for mixed in [
        FragmentOrigin::input(2..5)
            .unwrap()
            .join(FragmentOrigin::input(0..2).unwrap()),
        FragmentOrigin::input(0..2)
            .unwrap()
            .join(FragmentOrigin::input(3..5).unwrap()),
        FragmentOrigin::input(0..3)
            .unwrap()
            .join(FragmentOrigin::input(2..5).unwrap()),
    ] {
        assert_eq!(
            mixed,
            FragmentOrigin::Mixed {
                physical_context: span(0, 5),
            }
        );
    }

    let exact_replacement =
        replacement_origin(1, 7, 0..2, 10..13).join(replacement_origin(1, 7, 2..5, 10..13));
    assert_eq!(exact_replacement.replacement_range(), Some(0..5));
    assert_eq!(exact_replacement.input_range(), None);
    assert_eq!(exact_replacement.physical_context(), 10..13);

    for mixed in [
        replacement_origin(1, 7, 0..2, 10..13).join(replacement_origin(1, 8, 2..5, 10..13)),
        replacement_origin(1, 7, 0..2, 10..13).join(replacement_origin(2, 7, 2..5, 10..13)),
        replacement_origin(1, 7, 0..2, 10..13).join(replacement_origin(1, 7, 3..5, 10..13)),
    ] {
        assert_eq!(
            mixed,
            FragmentOrigin::Mixed {
                physical_context: span(10, 13),
            }
        );
    }

    assert_eq!(
        FragmentOrigin::input(0..5)
            .unwrap()
            .join(replacement_origin(1, 7, 0..2, 10..13)),
        FragmentOrigin::Mixed {
            physical_context: span(0, 13),
        }
    );
    assert_eq!(
        replacement_origin(1, 7, 0..2, 10..13).join(replacement_origin(1, 7, 2..5, 20..23)),
        FragmentOrigin::Mixed {
            physical_context: span(10, 23),
        }
    );
    let chained = FragmentOrigin::Mixed {
        physical_context: span(10, 23),
    }
    .join(FragmentOrigin::input(2..4).unwrap())
    .join(replacement_origin(9, 9, 0..1, 30..33));
    assert_eq!(
        chained,
        FragmentOrigin::Mixed {
            physical_context: span(2, 33),
        }
    );
}

#[test]
fn entry_failures_are_atomic_but_spent_attempts_remain_observable() {
    let boundary = FrameBoundary::Content { element_depth: 1 };
    let mut frames = loose();
    assert_eq!(
        frames
            .enter_physical(
                EntityKind::General,
                internal(EntityKind::General, 1, "&x;"),
                4..4,
                boundary,
            )
            .unwrap_err(),
        FrameError {
            kind: FrameErrorKind::InvalidSpan,
            physical_offset: 4,
        }
    );
    assert_eq!(frames.counters(), FrameCounters::default());
    let parent_invocation = frames
        .enter_physical(
            EntityKind::General,
            internal(EntityKind::General, 1, "&x;"),
            200..203,
            boundary,
        )
        .unwrap();
    assert_eq!(parent_invocation.get(), 1);
    assert_eq!(
        frames
            .enter_physical(
                EntityKind::General,
                internal(EntityKind::General, 2, ""),
                300..303,
                boundary,
            )
            .unwrap_err()
            .kind,
        FrameErrorKind::SourceMismatch
    );
    assert_eq!(frames.counters().reference_attempts, 1);

    assert_eq!(
        frames
            .enter_nested(
                EntityKind::General,
                failed(EntityKind::General, LookupFailure::Undeclared),
                id(1),
                "&x;",
                0..3,
                boundary,
            )
            .unwrap_err(),
        FrameError {
            kind: FrameErrorKind::Lookup(LookupFailure::Undeclared),
            physical_offset: 200,
        }
    );
    assert_eq!(frames.depth(), 1);
    assert_eq!(frames.current(id(1), "&x;").unwrap().local_start, 0);
    assert_eq!(
        frames.counters(),
        FrameCounters {
            reference_attempts: 2,
            work: 6,
            expanded_bytes: 3,
        }
    );

    let child_invocation = frames
        .enter_nested(
            EntityKind::General,
            internal(EntityKind::General, 2, ""),
            id(1),
            "&x;",
            0..3,
            boundary,
        )
        .unwrap();
    assert_eq!(child_invocation.get(), 2);
    assert_eq!(frames.depth(), 2);
    frames.finish_current(boundary).unwrap();
    assert_eq!(frames.current(id(1), "&x;").unwrap().local_start, 3);
    frames.finish_current(boundary).unwrap();
    assert_eq!(
        frames.counters(),
        FrameCounters {
            reference_attempts: 3,
            work: 11,
            expanded_bytes: 6,
        }
    );
}
