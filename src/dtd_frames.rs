use std::num::NonZeroU64;
use std::ops::Range;

use crate::dtd::DtdLimits;

pub(crate) use crate::origin::{
    ByteSpan, EntityDeclarationId, EntityInvocationId, OriginRepr as FragmentOrigin,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EntityKind {
    General,
    Parameter,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReferenceContext {
    Content,
    DtdSubset,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameBoundary {
    Content { element_depth: usize },
    DtdSubset,
}

impl FrameBoundary {
    pub(crate) fn context(self) -> ReferenceContext {
        match self {
            Self::Content { .. } => ReferenceContext::Content,
            Self::DtdSubset => ReferenceContext::DtdSubset,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct FrameCounters {
    pub(crate) reference_attempts: usize,
    pub(crate) work: usize,
    pub(crate) expanded_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameLimit {
    ActiveDepth,
    ReferenceAttempts,
    Work,
    ExpandedBytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameCounter {
    ReferenceAttempts,
    Work,
    ExpandedBytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LookupFailure {
    Undeclared,
    Unparsed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EntityResolution {
    Internal {
        kind: EntityKind,
        declaration: EntityDeclarationId,
        replacement_len: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameErrorKind {
    InvalidSpan,
    InvalidContext,
    KindMismatch,
    Lookup(LookupFailure),
    Cycle,
    LimitExceeded(FrameLimit),
    CounterOverflow(FrameCounter),
    InvocationIdsExhausted,
    Allocation,
    NoActiveFrame,
    SourceMismatch,
    PrematureEnd,
    BoundaryMismatch,
    ElementBoundaryCrossing,
    ElementDepthMismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FrameError {
    pub(crate) kind: FrameErrorKind,
    pub(crate) physical_offset: usize,
}

impl FrameError {
    fn new(kind: FrameErrorKind, physical_offset: usize) -> Self {
        Self {
            kind,
            physical_offset,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EntityFrame {
    declaration: EntityDeclarationId,
    invocation: EntityInvocationId,
    cursor: usize,
    replacement_len: usize,
    physical_reference: ByteSpan,
    boundary: FrameBoundary,
}

#[derive(Debug)]
pub(crate) struct EntityFrames {
    frames: Vec<EntityFrame>,
    limits: DtdLimits,
    counters: FrameCounters,
    next_invocation: Option<NonZeroU64>,
    expansion_ceiling: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CurrentSource<'a> {
    pub(crate) declaration: EntityDeclarationId,
    pub(crate) invocation: EntityInvocationId,
    pub(crate) local_start: usize,
    pub(crate) remaining: &'a str,
    pub(crate) physical_reference: Range<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CompletedFrame {
    pub(crate) declaration: EntityDeclarationId,
    pub(crate) invocation: EntityInvocationId,
    pub(crate) physical_reference: Range<usize>,
}

impl EntityFrames {
    pub(crate) fn new(limits: DtdLimits) -> Self {
        Self {
            frames: Vec::new(),
            limits,
            counters: FrameCounters::default(),
            next_invocation: NonZeroU64::new(1),
            expansion_ceiling: usize::MAX,
        }
    }

    pub(crate) fn cap_work(&mut self, ceiling: usize) {
        self.limits.work = self.limits.work.min(ceiling);
    }

    /// The live amplification allowance, beneath the configured ceiling.
    pub(crate) fn set_expansion_ceiling(&mut self, ceiling: usize) {
        self.expansion_ceiling = ceiling;
    }

    pub(crate) fn depth(&self) -> usize {
        self.frames.len()
    }

    pub(crate) fn counters(&self) -> FrameCounters {
        self.counters
    }

    pub(crate) fn enter_physical(
        &mut self,
        requested: EntityKind,
        resolution: EntityResolution,
        reference: Range<usize>,
        boundary: FrameBoundary,
    ) -> Result<EntityInvocationId, FrameError> {
        let offset = reference.start;
        if !self.frames.is_empty() {
            return Err(FrameError::new(FrameErrorKind::SourceMismatch, offset));
        }
        let Some(reference) = nonempty_span(reference) else {
            return Err(FrameError::new(FrameErrorKind::InvalidSpan, offset));
        };
        self.enter(requested, resolution, reference, boundary, 0, None)
    }

    pub(crate) fn enter_nested(
        &mut self,
        requested: EntityKind,
        resolution: EntityResolution,
        parent_declaration: EntityDeclarationId,
        parent_replacement: &str,
        reference: Range<usize>,
        boundary: FrameBoundary,
    ) -> Result<EntityInvocationId, FrameError> {
        let Some(parent) = self.frames.last() else {
            return Err(FrameError::new(FrameErrorKind::NoActiveFrame, 0));
        };
        let physical = parent.physical_reference;
        let offset = physical.start;
        if parent.declaration != parent_declaration
            || parent.replacement_len != parent_replacement.len()
        {
            return Err(FrameError::new(FrameErrorKind::SourceMismatch, offset));
        }
        let Some(reference) = nonempty_span(reference) else {
            return Err(FrameError::new(FrameErrorKind::InvalidSpan, offset));
        };
        if reference.start != parent.cursor
            || reference.end > parent.replacement_len
            || !parent_replacement.is_char_boundary(reference.start)
            || !parent_replacement.is_char_boundary(reference.end)
        {
            return Err(FrameError::new(FrameErrorKind::SourceMismatch, offset));
        }
        self.enter(
            requested,
            resolution,
            physical,
            boundary,
            reference.end - reference.start,
            Some(reference.end),
        )
    }

    pub(crate) fn current<'a>(
        &self,
        declaration: EntityDeclarationId,
        replacement: &'a str,
    ) -> Result<CurrentSource<'a>, FrameError> {
        let Some(frame) = self.frames.last() else {
            return Err(FrameError::new(FrameErrorKind::NoActiveFrame, 0));
        };
        let offset = frame.physical_reference.start;
        if frame.declaration != declaration
            || frame.replacement_len != replacement.len()
            || !replacement.is_char_boundary(frame.cursor)
        {
            return Err(FrameError::new(FrameErrorKind::SourceMismatch, offset));
        }
        Ok(CurrentSource {
            declaration,
            invocation: frame.invocation,
            local_start: frame.cursor,
            remaining: &replacement[frame.cursor..],
            physical_reference: frame.physical_reference.range(),
        })
    }

    pub(crate) fn advance_current(
        &mut self,
        declaration: EntityDeclarationId,
        replacement: &str,
        byte_count: usize,
    ) -> Result<FragmentOrigin, FrameError> {
        let Some(frame) = self.frames.last() else {
            return Err(FrameError::new(FrameErrorKind::NoActiveFrame, 0));
        };
        let offset = frame.physical_reference.start;
        if frame.declaration != declaration || frame.replacement_len != replacement.len() {
            return Err(FrameError::new(FrameErrorKind::SourceMismatch, offset));
        }
        let Some(end) = frame.cursor.checked_add(byte_count) else {
            return Err(FrameError::new(FrameErrorKind::InvalidSpan, offset));
        };
        if byte_count == 0
            || end > frame.replacement_len
            || !replacement.is_char_boundary(frame.cursor)
            || !replacement.is_char_boundary(end)
        {
            return Err(FrameError::new(FrameErrorKind::InvalidSpan, offset));
        }
        self.charge_bytes(byte_count, offset)?;
        let frame = self.frames.last_mut().expect("active frame was checked");
        let local = ByteSpan {
            start: frame.cursor,
            end,
        };
        frame.cursor = end;
        Ok(FragmentOrigin::Replacement {
            declaration,
            invocation: frame.invocation,
            local,
            physical_reference: frame.physical_reference,
        })
    }

    pub(crate) fn finish_current(
        &mut self,
        boundary: FrameBoundary,
    ) -> Result<CompletedFrame, FrameError> {
        let Some(frame) = self.frames.last() else {
            return Err(FrameError::new(FrameErrorKind::NoActiveFrame, 0));
        };
        let offset = frame.physical_reference.start;
        if frame.cursor != frame.replacement_len {
            return Err(FrameError::new(FrameErrorKind::PrematureEnd, offset));
        }
        match (frame.boundary, boundary) {
            (
                FrameBoundary::Content {
                    element_depth: expected,
                },
                FrameBoundary::Content {
                    element_depth: actual,
                },
            ) if expected != actual => {
                return Err(FrameError::new(
                    FrameErrorKind::ElementDepthMismatch,
                    offset,
                ));
            }
            (expected, actual) if expected.context() != actual.context() => {
                return Err(FrameError::new(FrameErrorKind::BoundaryMismatch, offset));
            }
            _ => {}
        }
        let frame = self.frames.pop().expect("active frame was checked");
        Ok(CompletedFrame {
            declaration: frame.declaration,
            invocation: frame.invocation,
            physical_reference: frame.physical_reference.range(),
        })
    }

    pub(crate) fn check_before_end_element(
        &self,
        current_element_depth: usize,
    ) -> Result<(), FrameError> {
        let Some(frame) = self.frames.last() else {
            return Ok(());
        };
        let offset = frame.physical_reference.start;
        let FrameBoundary::Content {
            element_depth: entry_depth,
        } = frame.boundary
        else {
            return Err(FrameError::new(FrameErrorKind::BoundaryMismatch, offset));
        };
        if current_element_depth <= entry_depth {
            return Err(FrameError::new(
                FrameErrorKind::ElementBoundaryCrossing,
                offset,
            ));
        }
        Ok(())
    }

    fn enter(
        &mut self,
        requested: EntityKind,
        resolution: EntityResolution,
        physical_reference: ByteSpan,
        boundary: FrameBoundary,
        spelling_bytes: usize,
        parent_cursor: Option<usize>,
    ) -> Result<EntityInvocationId, FrameError> {
        let offset = physical_reference.start;
        self.charge_attempt(spelling_bytes, offset)?;
        let (kind, declaration, replacement_len) = match resolution {
            EntityResolution::Internal {
                kind,
                declaration,
                replacement_len,
            } => (kind, declaration, replacement_len),
        };
        if kind != requested {
            return Err(FrameError::new(FrameErrorKind::KindMismatch, offset));
        }
        if !context_allows(kind, boundary.context()) {
            return Err(FrameError::new(FrameErrorKind::InvalidContext, offset));
        }
        if self
            .frames
            .iter()
            .any(|frame| frame.declaration == declaration)
        {
            return Err(FrameError::new(FrameErrorKind::Cycle, offset));
        }
        if self.frames.len() >= self.limits.depth {
            return Err(FrameError::new(
                FrameErrorKind::LimitExceeded(FrameLimit::ActiveDepth),
                offset,
            ));
        }
        let Some(invocation) = self.next_invocation.map(EntityInvocationId) else {
            return Err(FrameError::new(
                FrameErrorKind::InvocationIdsExhausted,
                offset,
            ));
        };
        self.frames
            .try_reserve(1)
            .map_err(|_| FrameError::new(FrameErrorKind::Allocation, offset))?;
        if let Some(cursor) = parent_cursor {
            self.frames
                .last_mut()
                .expect("nested entry has a parent frame")
                .cursor = cursor;
        }
        self.frames.push(EntityFrame {
            declaration,
            invocation,
            cursor: 0,
            replacement_len,
            physical_reference,
            boundary,
        });
        self.next_invocation = invocation.0.get().checked_add(1).and_then(NonZeroU64::new);
        Ok(invocation)
    }

    fn charge_attempt(
        &mut self,
        spelling_bytes: usize,
        physical_offset: usize,
    ) -> Result<(), FrameError> {
        let attempts = checked_add_counter(
            self.counters.reference_attempts,
            1,
            FrameCounter::ReferenceAttempts,
            physical_offset,
        )?;
        if attempts > self.limits.references {
            return Err(FrameError::new(
                FrameErrorKind::LimitExceeded(FrameLimit::ReferenceAttempts),
                physical_offset,
            ));
        }
        let comparison_work = self.frames.len();
        let work_delta = 1_usize
            .checked_add(comparison_work)
            .and_then(|work| work.checked_add(spelling_bytes))
            .ok_or_else(|| {
                FrameError::new(
                    FrameErrorKind::CounterOverflow(FrameCounter::Work),
                    physical_offset,
                )
            })?;
        let work = checked_add_counter(
            self.counters.work,
            work_delta,
            FrameCounter::Work,
            physical_offset,
        )?;
        if work > self.limits.work {
            return Err(FrameError::new(
                FrameErrorKind::LimitExceeded(FrameLimit::Work),
                physical_offset,
            ));
        }
        let expanded_bytes = checked_add_counter(
            self.counters.expanded_bytes,
            spelling_bytes,
            FrameCounter::ExpandedBytes,
            physical_offset,
        )?;
        if expanded_bytes > self.limits.expansion.min(self.expansion_ceiling) {
            return Err(FrameError::new(
                FrameErrorKind::LimitExceeded(FrameLimit::ExpandedBytes),
                physical_offset,
            ));
        }
        self.counters = FrameCounters {
            reference_attempts: attempts,
            work,
            expanded_bytes,
        };
        Ok(())
    }

    fn charge_bytes(
        &mut self,
        byte_count: usize,
        physical_offset: usize,
    ) -> Result<(), FrameError> {
        let work = checked_add_counter(
            self.counters.work,
            byte_count,
            FrameCounter::Work,
            physical_offset,
        )?;
        if work > self.limits.work {
            return Err(FrameError::new(
                FrameErrorKind::LimitExceeded(FrameLimit::Work),
                physical_offset,
            ));
        }
        let expanded_bytes = checked_add_counter(
            self.counters.expanded_bytes,
            byte_count,
            FrameCounter::ExpandedBytes,
            physical_offset,
        )?;
        if expanded_bytes > self.limits.expansion.min(self.expansion_ceiling) {
            return Err(FrameError::new(
                FrameErrorKind::LimitExceeded(FrameLimit::ExpandedBytes),
                physical_offset,
            ));
        }
        self.counters.work = work;
        self.counters.expanded_bytes = expanded_bytes;
        Ok(())
    }
}

fn nonempty_span(range: Range<usize>) -> Option<ByteSpan> {
    (range.start < range.end).then_some(ByteSpan {
        start: range.start,
        end: range.end,
    })
}

fn context_allows(kind: EntityKind, context: ReferenceContext) -> bool {
    matches!(
        (kind, context),
        (EntityKind::General, ReferenceContext::Content)
            | (EntityKind::Parameter, ReferenceContext::DtdSubset)
    )
}

fn checked_add_counter(
    current: usize,
    delta: usize,
    counter: FrameCounter,
    physical_offset: usize,
) -> Result<usize, FrameError> {
    current
        .checked_add(delta)
        .ok_or_else(|| FrameError::new(FrameErrorKind::CounterOverflow(counter), physical_offset))
}
