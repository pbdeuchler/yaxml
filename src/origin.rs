use std::num::NonZeroU64;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EntityId(pub(crate) usize);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct EntityInvocationId(pub(crate) NonZeroU64);

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
        declaration: EntityId,
        invocation: EntityInvocationId,
        local: ByteSpan,
        physical_reference: ByteSpan,
    },
    Mixed {
        physical_context: ByteSpan,
    },
}

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
                    span: left.cover(right),
                }
            }
            // An invocation is unique, so it fixes the declaration and reference.
            (
                Self::Replacement {
                    declaration,
                    invocation,
                    local: left,
                    physical_reference,
                },
                Self::Replacement {
                    invocation: next,
                    local: right,
                    ..
                },
            ) if invocation == next && left.end == right.start => Self::Replacement {
                declaration,
                invocation,
                local: left.cover(right),
                physical_reference,
            },
            (left, right) => Self::Mixed {
                physical_context: left.physical_span().cover(right.physical_span()),
            },
        }
    }

    /// An element's opening and closing tags lie in one source occurrence:
    /// entity frames reject unbalanced replacement text. A recovered parse
    /// stopping inside replacement text keeps the opening span.
    fn close_element(self, end: Self) -> Self {
        match (self, end) {
            (Self::Input { span }, Self::Input { span: closing }) => Self::Input {
                span: span.cover(closing),
            },
            (
                Self::Replacement {
                    declaration,
                    invocation,
                    local,
                    physical_reference,
                },
                Self::Replacement { local: closing, .. },
            ) => Self::Replacement {
                declaration,
                invocation,
                local: local.cover(closing),
                physical_reference,
            },
            _ => self,
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

/// Where an event or node came from in the input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceOrigin(OriginRepr);

impl SourceOrigin {
    /// The input bytes the value was read from, delimiters included; `None`
    /// when entity expansion produced any of it.
    ///
    /// Normalization changes the value, not this span.
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
    pub(crate) fn close_element(self, end: Self) -> Self {
        Self(self.0.close_element(end.0))
    }

    /// Joins adjacent parsed Text origins, falling back to an inexact Mixed cover.
    pub(crate) fn join_text(self, next: Self) -> Self {
        Self(self.0.join_text(next.0))
    }
}

#[cfg(test)]
#[path = "../tests/internal/origin.rs"]
mod tests;
