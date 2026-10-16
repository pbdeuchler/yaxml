use std::ops::Range;

use crate::{Error, ErrorKind};

/// Where parsed content originated, independently of its current logical value.
///
/// The representation is private: an origin need not always be a contiguous
/// span in the original document. All currently supported parsed content does
/// have such a span. Entity replacement provenance is not yet implemented.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceOrigin {
    start: usize,
    end: usize,
}

impl SourceOrigin {
    /// Exact original document bytes, including lexical markup delimiters.
    ///
    /// Normalization can change the logical value without changing this span.
    /// This is provenance, not a serialization of the current logical content.
    pub fn input_range(self) -> Option<Range<usize>> {
        Some(self.start..self.end)
    }

    pub(crate) fn input(range: Range<usize>) -> Self {
        debug_assert!(range.start <= range.end);
        Self {
            start: range.start,
            end: range.end,
        }
    }

    /// Physical input location used for a failure associated with this origin.
    pub(crate) fn error_offset(self) -> usize {
        self.start
    }

    /// Physical input location used when a delivered event cancels parsing.
    pub(crate) fn cancellation_offset(self) -> usize {
        self.end
    }

    /// Close an element with a later end tag from the same physical input.
    /// Replacement occurrences require a distinct representation and join rule
    /// before generated markup can be accepted.
    pub(crate) fn close_element(self, end: Self) -> Result<Self, Error> {
        if end.start < self.end {
            return Err(Error::new(ErrorKind::InvalidState, end.error_offset()));
        }
        Ok(Self {
            start: self.start,
            end: end.end,
        })
    }
}

#[cfg(test)]
#[path = "../tests/internal/origin.rs"]
mod tests;
