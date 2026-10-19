//! Checked Text/reference operations; inclusion remains the coordinator's job.
use super::{ReadySpan, Scan, TokenKind, Tokenizer};
use crate::dtd_reference::{ReferenceKind, ValidatedReference};
use crate::encoding::{SourceEncoding, TokenSpan};
use crate::{Error, ErrorKind, Limit};
use std::ops::Range;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContentStop {
    Exhausted,
    TokenReady,
    ReferenceStart,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ContentStep {
    pub(crate) consumed: usize,
    pub(crate) stop: ContentStop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReferenceCursorErrorKind {
    PositionOverflow,
    Limit(Limit),
    InvalidState,
}

/// Offset is physical for Input and replacement-local for Local cursors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReferenceCursorError {
    pub(crate) kind: ReferenceCursorErrorKind,
    pub(crate) offset: usize,
}

#[derive(Clone, Copy, Debug)]
enum ReferenceStart {
    Input {
        decoded: usize,
        source: usize,
        encoding: SourceEncoding,
    },
    Local(usize),
}

/// Preview source progress before handing the same bytes to reference scratch.
/// Committing the returned value after scratch success needs no allocation.
/// The coordinator must offer exactly the previewed bytes and commit only the
/// accepted prefix. Range/encoding checks do not prove spelling or invocation
/// identity; the coordinator owns both bindings.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReferenceCursor {
    start: ReferenceStart,
    decoded_end: usize,
    source_end: usize,
    raw_limit: usize,
    decoded_limit: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct ReferenceExtent {
    span: ReadySpan,
}

impl ReferenceCursor {
    pub(crate) fn decoded_position(self) -> usize {
        self.decoded_end
    }
    pub(crate) fn input_position(self) -> Option<usize> {
        match self.start {
            ReferenceStart::Input { .. } => Some(self.source_end),
            ReferenceStart::Local(_) => None,
        }
    }
    pub(crate) fn map_offset(self, raw: &[u8], offset: usize) -> usize {
        match self.start {
            ReferenceStart::Local(_)
            | ReferenceStart::Input {
                encoding: SourceEncoding::Utf8,
                ..
            } => offset,
            ReferenceStart::Input {
                decoded,
                source,
                encoding,
            } => {
                let mut relative = offset.saturating_sub(decoded).min(raw.len());
                while relative < raw.len() && relative != 0 && raw[relative] & 0xc0 == 0x80 {
                    relative -= 1;
                }
                source + encoding.source_length(&raw[..relative])
            }
        }
    }

    pub(crate) fn checked_advance(self, bytes: &[u8]) -> Result<Self, ReferenceCursorError> {
        let (decoded_start, source_start, encoding) = match self.start {
            ReferenceStart::Input {
                decoded,
                source,
                encoding,
            } => (decoded, source, encoding),
            ReferenceStart::Local(start) => (start, start, SourceEncoding::Utf8),
        };
        let fail = |kind| ReferenceCursorError {
            kind,
            offset: source_start,
        };
        let decoded_end = self
            .decoded_end
            .checked_add(bytes.len())
            .ok_or_else(|| fail(ReferenceCursorErrorKind::PositionOverflow))?;
        let mut source_end = self.source_end;
        for byte in bytes {
            source_end = source_end
                .checked_add(encoding.source_length(std::slice::from_ref(byte)))
                .ok_or_else(|| fail(ReferenceCursorErrorKind::PositionOverflow))?;
        }
        if source_end - source_start > self.raw_limit {
            return Err(fail(ReferenceCursorErrorKind::Limit(Limit::TokenBytes)));
        }
        if decoded_end - decoded_start > self.decoded_limit {
            return Err(fail(ReferenceCursorErrorKind::Limit(
                Limit::DecodedTokenBytes,
            )));
        }
        Ok(Self {
            decoded_end,
            source_end,
            ..self
        })
    }

    pub(crate) fn finish(
        self,
        reference: ValidatedReference<'_>,
    ) -> Result<ReferenceExtent, ReferenceCursorError> {
        let (start, span) = match self.start {
            ReferenceStart::Input {
                decoded,
                source,
                encoding,
            } => (
                decoded,
                ReadySpan::Input(TokenSpan {
                    decoded: decoded..self.decoded_end,
                    source: source..self.source_end,
                    encoding,
                }),
            ),
            ReferenceStart::Local(start) => (start, ReadySpan::Local(start..self.decoded_end)),
        };
        if reference.range() != (start..self.decoded_end) {
            return Err(ReferenceCursorError {
                kind: ReferenceCursorErrorKind::InvalidState,
                offset: self.anchor(),
            });
        }
        Ok(ReferenceExtent { span })
    }

    fn anchor(self) -> usize {
        match self.start {
            ReferenceStart::Input { source, .. } => source,
            ReferenceStart::Local(start) => start,
        }
    }
}

impl ReferenceExtent {
    pub(crate) fn input_span(&self) -> Option<&TokenSpan> {
        match &self.span {
            ReadySpan::Input(span) => Some(span),
            ReadySpan::Local(_) => None,
        }
    }
    pub(crate) fn local_range(&self) -> Option<Range<usize>> {
        match &self.span {
            ReadySpan::Local(range) => Some(range.clone()),
            ReadySpan::Input(_) => None,
        }
    }
}

impl Tokenizer {
    pub(crate) fn advance_content(&mut self, bytes: &[u8]) -> Result<ContentStep, Error> {
        let (consumed, reference) = self.advance_inner::<true>(bytes)?;
        Ok(ContentStep {
            consumed,
            stop: if reference {
                ContentStop::ReferenceStart
            } else if self.ready.is_some() {
                ContentStop::TokenReady
            } else {
                ContentStop::Exhausted
            },
        })
    }

    pub(crate) fn reference_cursor(&self) -> Result<ReferenceCursor, Error> {
        self.check_reference_state()?;
        Ok(ReferenceCursor {
            start: if self.local_mode() {
                ReferenceStart::Local(self.position)
            } else {
                ReferenceStart::Input {
                    decoded: self.position,
                    source: self.source_position,
                    encoding: self.encoding,
                }
            },
            decoded_end: self.position,
            source_end: if self.local_mode() {
                self.position
            } else {
                self.source_position
            },
            raw_limit: self.limit,
            decoded_limit: self.decoded_limit,
        })
    }

    pub(crate) fn flush_text_before_reference(&mut self) -> Result<bool, Error> {
        self.check_reference_state()?;
        if self.buffer.is_empty() {
            return Ok(false);
        }
        self.mark(TokenKind::Text, self.position);
        Ok(true)
    }

    pub(crate) fn append_character_reference(
        &mut self,
        reference: ValidatedReference<'_>,
        extent: &ReferenceExtent,
    ) -> Result<(), Error> {
        if matches!(reference.kind(), ReferenceKind::General) {
            return Err(Error::new(ErrorKind::InvalidState, self.diagnostic_start()));
        }
        let (decoded_end, source_end, source_start) = self.checked_extent(reference, extent)?;
        let raw_start = if self.buffer.is_empty() {
            source_start
        } else if self.local_mode() {
            self.start
        } else {
            self.source_start
        };
        let raw_end = if self.local_mode() {
            decoded_end
        } else {
            source_end
        };
        if raw_end - raw_start > self.limit {
            return Err(Error::new(
                ErrorKind::LimitExceeded(Limit::TokenBytes),
                raw_start,
            ));
        }
        if self.buffer.is_empty() {
            self.start = self.position;
            self.source_start = source_start;
        }
        self.append_slice(reference.raw().as_bytes())?;
        self.position = decoded_end;
        if !self.local_mode() {
            self.source_position = source_end;
        }
        Ok(())
    }

    pub(crate) fn skip_general_reference(
        &mut self,
        reference: ValidatedReference<'_>,
        extent: &ReferenceExtent,
    ) -> Result<(), Error> {
        if reference.kind() != ReferenceKind::General || !self.buffer.is_empty() {
            return Err(Error::new(ErrorKind::InvalidState, self.diagnostic_start()));
        }
        let (decoded_end, source_end, _) = self.checked_extent(reference, extent)?;
        self.position = decoded_end;
        self.start = decoded_end;
        if !self.local_mode() {
            self.source_position = source_end;
            self.source_start = source_end;
        }
        Ok(())
    }

    fn check_reference_state(&self) -> Result<(), Error> {
        if self.ready.is_some() || self.lookahead.is_some() || !matches!(self.scan, Scan::Text) {
            Err(Error::new(ErrorKind::InvalidState, self.diagnostic_start()))
        } else {
            Ok(())
        }
    }

    fn checked_extent(
        &self,
        reference: ValidatedReference<'_>,
        extent: &ReferenceExtent,
    ) -> Result<(usize, usize, usize), Error> {
        self.check_reference_state()?;
        let fail = || Error::new(ErrorKind::InvalidState, self.diagnostic_start());
        if reference.range().start != self.position {
            return Err(fail());
        }
        match &extent.span {
            ReadySpan::Input(span)
                if !self.local_mode()
                    && span.encoding == self.encoding
                    && span.decoded == reference.range()
                    && span.source.start == self.source_position =>
            {
                Ok((span.decoded.end, span.source.end, span.source.start))
            }
            ReadySpan::Local(range) if self.local_mode() && *range == reference.range() => {
                Ok((range.end, 0, range.start))
            }
            _ => Err(fail()),
        }
    }
}

#[cfg(test)]
#[path = "../../tests/internal/dtd_text_boundary_author.rs"]
mod tests;
