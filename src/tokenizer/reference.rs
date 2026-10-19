//! Checked Text/reference operations; inclusion remains the coordinator's job.
use super::{TokenKind, Tokenizer};
use crate::dtd_reference::ValidatedReference;
use crate::encoding::{SourceEncoding, TokenSpan};
use crate::{Error, ErrorKind, Limit};

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

/// Preview source progress before handing the same bytes to reference scratch.
/// Committing the returned value after scratch success needs no allocation.
/// The coordinator must offer exactly the previewed bytes and commit only the
/// accepted prefix; it owns the spelling and invocation bindings. Replacement
/// text is UTF-8 whose source offsets are its local offsets.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReferenceCursor {
    decoded_start: usize,
    source_start: usize,
    encoding: SourceEncoding,
    decoded_end: usize,
    source_end: usize,
    raw_limit: usize,
    decoded_limit: usize,
}

/// The span of a complete reference spelling.
#[derive(Clone, Debug)]
pub(crate) struct ReferenceExtent(pub(crate) TokenSpan);

impl ReferenceCursor {
    pub(crate) fn decoded_position(self) -> usize {
        self.decoded_end
    }
    pub(crate) fn input_position(self) -> usize {
        self.source_end
    }
    pub(crate) fn map_offset(self, raw: &[u8], offset: usize) -> usize {
        if self.encoding == SourceEncoding::Utf8 {
            return offset;
        }
        let mut relative = offset.saturating_sub(self.decoded_start).min(raw.len());
        while relative < raw.len() && relative != 0 && raw[relative] & 0xc0 == 0x80 {
            relative -= 1;
        }
        self.source_start + self.encoding.source_length(&raw[..relative])
    }

    /// Errors are at the reference start: physical for input, local otherwise.
    pub(crate) fn checked_advance(self, bytes: &[u8]) -> Result<Self, Error> {
        let fail = |kind| Error::new(kind, self.source_start);
        let decoded_end = self
            .decoded_end
            .checked_add(bytes.len())
            .ok_or_else(|| fail(ErrorKind::InvalidState))?;
        let mut source_end = self.source_end;
        for byte in bytes {
            source_end = source_end
                .checked_add(self.encoding.source_length(std::slice::from_ref(byte)))
                .ok_or_else(|| fail(ErrorKind::InvalidState))?;
        }
        if source_end - self.source_start > self.raw_limit {
            return Err(fail(ErrorKind::LimitExceeded(Limit::TokenBytes)));
        }
        if decoded_end - self.decoded_start > self.decoded_limit {
            return Err(fail(ErrorKind::LimitExceeded(Limit::DecodedTokenBytes)));
        }
        Ok(Self {
            decoded_end,
            source_end,
            ..self
        })
    }

    /// The extent of the reference collected through this cursor.
    pub(crate) fn finish(self) -> ReferenceExtent {
        ReferenceExtent(TokenSpan {
            decoded: self.decoded_start..self.decoded_end,
            source: self.source_start..self.source_end,
            encoding: self.encoding,
        })
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

    pub(crate) fn reference_cursor(&self) -> ReferenceCursor {
        ReferenceCursor {
            decoded_start: self.position,
            source_start: self.source_position,
            encoding: self.encoding,
            decoded_end: self.position,
            source_end: self.source_position,
            raw_limit: self.limit,
            decoded_limit: self.decoded_limit,
        }
    }

    pub(crate) fn flush_text_before_reference(&mut self) -> bool {
        if self.buffer.is_empty() {
            return false;
        }
        self.mark(TokenKind::Text, self.position);
        true
    }

    pub(crate) fn append_character_reference(
        &mut self,
        reference: ValidatedReference<'_>,
        ReferenceExtent(span): &ReferenceExtent,
    ) -> Result<(), Error> {
        let raw_start = match self.buffer.is_empty() {
            true => span.source.start,
            false => self.source_start,
        };
        if span.source.end - raw_start > self.limit {
            return Err(Error::new(
                ErrorKind::LimitExceeded(Limit::TokenBytes),
                raw_start,
            ));
        }
        if self.buffer.is_empty() {
            self.start = self.position;
        }
        self.source_start = raw_start;
        self.append_slice(reference.raw().as_bytes())?;
        self.position = span.decoded.end;
        self.source_position = span.source.end;
        Ok(())
    }

    pub(crate) fn skip_general_reference(&mut self, ReferenceExtent(span): &ReferenceExtent) {
        (self.position, self.start) = (span.decoded.end, span.decoded.end);
        (self.source_position, self.source_start) = (span.source.end, span.source.end);
    }
}
