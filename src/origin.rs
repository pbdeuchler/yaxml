use std::num::NonZeroU64;
use std::ops::Range;

use crate::{Error, ErrorKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EntityDeclarationId(usize);

#[cfg_attr(
    not(test),
    allow(dead_code, reason = "private DTD controller is still test-only")
)]
impl EntityDeclarationId {
    pub(crate) fn from_index(index: usize) -> Self {
        Self(index)
    }

    pub(crate) fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct EntityInvocationId(pub(crate) NonZeroU64);

#[cfg_attr(
    not(test),
    allow(dead_code, reason = "private DTD controller is still test-only")
)]
impl EntityInvocationId {
    pub(crate) fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ByteSpan {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl ByteSpan {
    pub(crate) fn new(range: Range<usize>) -> Option<Self> {
        (range.start <= range.end).then_some(Self {
            start: range.start,
            end: range.end,
        })
    }

    pub(crate) fn range(self) -> Range<usize> {
        self.start..self.end
    }

    fn cover(self, other: Self) -> Self {
        Self {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

/// Shared private representation for parser origins and entity-frame fragments.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OriginRepr {
    Input {
        span: ByteSpan,
    },
    Replacement {
        declaration: EntityDeclarationId,
        invocation: EntityInvocationId,
        local: ByteSpan,
        physical_reference: ByteSpan,
    },
    Mixed {
        physical_context: ByteSpan,
    },
}

#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "replacement origins activate with the private DTD controller"
    )
)]
impl OriginRepr {
    pub(crate) fn input(span: Range<usize>) -> Option<Self> {
        ByteSpan::new(span).map(|span| Self::Input { span })
    }

    pub(crate) fn input_range(self) -> Option<Range<usize>> {
        match self {
            Self::Input { span } => Some(span.range()),
            Self::Replacement { .. } | Self::Mixed { .. } => None,
        }
    }

    pub(crate) fn replacement_range(self) -> Option<Range<usize>> {
        match self {
            Self::Replacement { local, .. } => Some(local.range()),
            Self::Input { .. } | Self::Mixed { .. } => None,
        }
    }

    pub(crate) fn physical_context(self) -> Range<usize> {
        self.physical_span().range()
    }

    pub(crate) fn error_offset(self) -> usize {
        self.physical_span().start
    }

    pub(crate) fn cancellation_offset(self) -> usize {
        self.physical_span().end
    }

    /// Joins source-homogeneous Text. Exact origins require adjacency.
    pub(crate) fn join_text(self, next: Self) -> Self {
        match (self, next) {
            (Self::Input { span: left }, Self::Input { span: right })
                if left.end == right.start =>
            {
                Self::Input {
                    span: ByteSpan {
                        start: left.start,
                        end: right.end,
                    },
                }
            }
            (
                Self::Replacement {
                    declaration: left_declaration,
                    invocation: left_invocation,
                    local: left,
                    physical_reference: left_physical,
                },
                Self::Replacement {
                    declaration: right_declaration,
                    invocation: right_invocation,
                    local: right,
                    physical_reference: right_physical,
                },
            ) if left_declaration == right_declaration
                && left_invocation == right_invocation
                && left_physical == right_physical
                && left.end == right.start =>
            {
                Self::Replacement {
                    declaration: left_declaration,
                    invocation: left_invocation,
                    local: ByteSpan {
                        start: left.start,
                        end: right.end,
                    },
                    physical_reference: left_physical,
                }
            }
            (left, right) => Self::Mixed {
                physical_context: left.physical_span().cover(right.physical_span()),
            },
        }
    }

    /// Compatibility name for the entity-frame prototype's Text-origin model.
    pub(crate) fn join(self, next: Self) -> Self {
        self.join_text(next)
    }

    fn close_element(self, end: Self) -> Result<Self, Error> {
        let invalid = || Error::new(ErrorKind::InvalidState, end.error_offset());
        match (self, end) {
            (Self::Input { span: opening }, Self::Input { span: closing })
                if closing.start >= opening.end =>
            {
                Ok(Self::Input {
                    span: ByteSpan {
                        start: opening.start,
                        end: closing.end,
                    },
                })
            }
            (
                Self::Replacement {
                    declaration: opening_declaration,
                    invocation: opening_invocation,
                    local: opening,
                    physical_reference: opening_physical,
                },
                Self::Replacement {
                    declaration: closing_declaration,
                    invocation: closing_invocation,
                    local: closing,
                    physical_reference: closing_physical,
                },
            ) if opening_declaration == closing_declaration
                && opening_invocation == closing_invocation
                && opening_physical == closing_physical
                && closing.start >= opening.end =>
            {
                Ok(Self::Replacement {
                    declaration: opening_declaration,
                    invocation: opening_invocation,
                    local: ByteSpan {
                        start: opening.start,
                        end: closing.end,
                    },
                    physical_reference: opening_physical,
                })
            }
            _ => Err(invalid()),
        }
    }

    fn physical_span(self) -> ByteSpan {
        match self {
            Self::Input { span } => span,
            Self::Replacement {
                physical_reference, ..
            } => physical_reference,
            Self::Mixed { physical_context } => physical_context,
        }
    }
}

/// Where parsed content originated, independently of its current logical value.
///
/// The representation is private. `input_range` returns an exact contiguous
/// original-input range only; replacement and mixed origins deliberately return
/// `None`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceOrigin(OriginRepr);

impl SourceOrigin {
    /// Exact original document bytes, when the value came from one contiguous
    /// physical input range.
    ///
    /// Normalization can change the logical value without changing this span.
    /// This is provenance, not a serialization of the current logical content.
    pub fn input_range(self) -> Option<Range<usize>> {
        self.0.input_range()
    }

    pub(crate) fn input(range: Range<usize>) -> Self {
        debug_assert!(range.start <= range.end);
        Self(OriginRepr::Input {
            span: ByteSpan {
                start: range.start,
                end: range.end,
            },
        })
    }

    pub(crate) fn from_repr(origin: OriginRepr) -> Self {
        Self(origin)
    }

    /// Byte range within the replacement text of the innermost entity that
    /// produced this value; absent for values read directly from the input.
    pub fn replacement_range(self) -> Option<Range<usize>> {
        self.0.replacement_range()
    }

    /// The original input bytes responsible for this value: its own span, or
    /// the outermost entity reference whose expansion produced it.
    pub fn physical_context(self) -> Range<usize> {
        self.0.physical_context()
    }

    /// Physical input location used for a failure associated with this origin.
    pub(crate) fn error_offset(self) -> usize {
        self.0.error_offset()
    }

    /// Physical input location used when a delivered event cancels parsing.
    pub(crate) fn cancellation_offset(self) -> usize {
        self.0.cancellation_offset()
    }

    /// Closes an element within the same exact source occurrence. Element bodies
    /// may create a gap between opening and closing tag spans.
    pub(crate) fn close_element(self, end: Self) -> Result<Self, Error> {
        self.0.close_element(end.0).map(Self)
    }

    /// Joins adjacent parsed Text origins, falling back to an inexact Mixed cover.
    pub(crate) fn join_text(self, next: Self) -> Self {
        Self(self.0.join_text(next.0))
    }
}

#[cfg(test)]
#[path = "../tests/internal/origin.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/internal/dtd_provenance_author.rs"]
mod provenance_author_tests;

#[cfg(test)]
#[path = "../tests/internal/dtd_provenance_spec.rs"]
mod provenance_spec_tests;
