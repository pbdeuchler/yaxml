//! The stack of entities being expanded, with libxml2's reference accounting.

use std::num::NonZeroU64;
use std::ops::Range;

use crate::dtd::DtdLimits;
use crate::{Error, ErrorKind, Limit};

pub(crate) use crate::origin::{
    ByteSpan, EntityId, EntityInvocationId, OriginRepr as FragmentOrigin,
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct EntityFrame {
    pub(crate) entity: EntityId,
    pub(crate) invocation: EntityInvocationId,
    /// Replacement bytes consumed by complete units and tokens.
    pub(crate) cursor: usize,
    /// Replacement bytes scanned; lexical lookahead can run ahead of `cursor`.
    pub(crate) scanned: usize,
    len: usize,
    pub(crate) physical_reference: ByteSpan,
    /// Element depth at entry, or `None` for a parameter entity.
    element_depth: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct FrameCounters {
    pub(crate) reference_attempts: usize,
    pub(crate) work: usize,
    pub(crate) expanded_bytes: usize,
}

#[derive(Debug)]
pub(crate) struct EntityFrames {
    frames: Vec<EntityFrame>,
    limits: DtdLimits,
    counters: FrameCounters,
    next_invocation: u64,
    expansion_ceiling: usize,
}

const EXPANSION: ErrorKind = ErrorKind::LimitExceeded(Limit::EntityExpansion);

impl EntityFrames {
    pub(crate) fn new(limits: DtdLimits) -> Self {
        Self {
            frames: Vec::new(),
            limits,
            counters: FrameCounters::default(),
            next_invocation: 1,
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

    /// The stack without its frames, as recovery leaves the entities it
    /// was expanding.
    pub(crate) fn unwound(self) -> Self {
        Self {
            frames: Vec::new(),
            ..self
        }
    }

    pub(crate) fn counters(&self) -> FrameCounters {
        self.counters
    }

    pub(crate) fn stack(&self) -> &[EntityFrame] {
        &self.frames
    }

    pub(crate) fn top(&self) -> Option<&EntityFrame> {
        self.frames.last()
    }

    pub(crate) fn top_mut(&mut self) -> Option<&mut EntityFrame> {
        self.frames.last_mut()
    }

    /// Enters `entity`, whose replacement text is `len` bytes, from the
    /// reference at `reference`: physical input when no entity is active,
    /// otherwise the top frame's replacement text, which resumes after it.
    pub(crate) fn enter(
        &mut self,
        entity: EntityId,
        len: usize,
        reference: Range<usize>,
        element_depth: Option<usize>,
    ) -> Result<EntityInvocationId, Error> {
        let (physical_reference, spelling) = match self.frames.last() {
            None => (ByteSpan::new(reference.clone()).expect("ordered"), 0),
            Some(parent) => (parent.physical_reference, reference.len()),
        };
        let fail = |kind| Error::new(kind, physical_reference.start);
        self.charge_attempt(spelling, physical_reference.start)?;
        if self.frames.iter().any(|frame| frame.entity == entity) {
            return Err(fail(ErrorKind::EntityLoop));
        }
        if self.frames.len() >= self.limits.depth {
            return Err(fail(ErrorKind::LimitExceeded(Limit::EntityDepth)));
        }
        self.frames
            .try_reserve(1)
            .map_err(|_| fail(ErrorKind::Allocation))?;
        if let Some(parent) = self.frames.last_mut() {
            parent.cursor = reference.end;
        }
        let invocation = EntityInvocationId(NonZeroU64::new(self.next_invocation).expect("from 1"));
        self.next_invocation += 1;
        self.frames.push(EntityFrame {
            entity,
            invocation,
            cursor: 0,
            scanned: 0,
            len,
            physical_reference,
            element_depth,
        });
        Ok(invocation)
    }

    /// Consumes `bytes` of the top frame's replacement text.
    pub(crate) fn advance(&mut self, bytes: usize) -> Result<(), Error> {
        let frame = self.frames.last().expect("active frame");
        debug_assert!(bytes != 0 && frame.cursor + bytes <= frame.len);
        let offset = frame.physical_reference.start;
        let ceiling = self.limits.expansion.min(self.expansion_ceiling);
        let work = add(self.counters.work, bytes, self.limits.work, offset)?;
        let expanded = add(self.counters.expanded_bytes, bytes, ceiling, offset)?;
        (self.counters.work, self.counters.expanded_bytes) = (work, expanded);
        self.frames.last_mut().expect("active frame").cursor += bytes;
        Ok(())
    }

    /// Leaves the top frame, which must be consumed and, for a general
    /// entity, end at the element depth where it began.
    pub(crate) fn finish(&mut self, element_depth: Option<usize>) -> Result<(), Error> {
        let frame = self.frames.last().expect("active frame");
        if frame.cursor != frame.len || frame.element_depth != element_depth {
            let offset = frame.physical_reference.start;
            return Err(Error::new(ErrorKind::UnbalancedEntity, offset));
        }
        self.frames.pop();
        Ok(())
    }

    /// An end tag inside replacement text must close an element it opened.
    pub(crate) fn check_before_end_element(&self, element_depth: usize) -> Result<(), Error> {
        match self.frames.last() {
            Some(frame)
                if frame
                    .element_depth
                    .is_none_or(|entry| element_depth <= entry) =>
            {
                let offset = frame.physical_reference.start;
                Err(Error::new(ErrorKind::UnbalancedEntity, offset))
            }
            _ => Ok(()),
        }
    }

    fn charge_attempt(&mut self, spelling: usize, offset: usize) -> Result<(), Error> {
        let attempts = add(self.counters.reference_attempts, 1, usize::MAX, offset)?;
        if attempts > self.limits.references {
            return Err(Error::new(
                ErrorKind::LimitExceeded(Limit::EntityReferences),
                offset,
            ));
        }
        let delta = add(1, self.frames.len(), usize::MAX, offset)?;
        let delta = add(delta, spelling, usize::MAX, offset)?;
        let work = add(self.counters.work, delta, self.limits.work, offset)?;
        let ceiling = self.limits.expansion.min(self.expansion_ceiling);
        let expanded_bytes = add(self.counters.expanded_bytes, spelling, ceiling, offset)?;
        self.counters = FrameCounters {
            reference_attempts: attempts,
            work,
            expanded_bytes,
        };
        Ok(())
    }
}

/// `count + delta` within `ceiling`; overflow counts as excess expansion.
fn add(count: usize, delta: usize, ceiling: usize, offset: usize) -> Result<usize, Error> {
    count
        .checked_add(delta)
        .filter(|count| *count <= ceiling)
        .ok_or(Error::new(EXPANSION, offset))
}
