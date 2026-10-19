//! A sealed replacement-local view of the existing XML tokenizer.
use super::reference::{ContentStep, ContentStop, ReferenceCursor, ReferenceExtent};
use super::{TokenKind, Tokenizer};
use crate::dtd_reference::ValidatedReference;
use crate::{Error, ErrorKind, Limit};
use std::ops::Range;

const WORK: ErrorKind = ErrorKind::LimitExceeded(Limit::EntityExpansion);

/// Offsets are positions in the replacement text being tokenized. Replacement
/// text holds only XML characters (checked when declared or loaded), so its
/// tokens need no character validation.
pub(crate) struct LocalTokenizer {
    inner: Tokenizer,
    max_work: usize,
    work: usize,
}
impl LocalTokenizer {
    pub(crate) fn new(start: usize, max_token_bytes: usize, max_work: usize) -> Self {
        let mut tokenizer = Self {
            inner: Tokenizer::new(max_token_bytes, max_token_bytes),
            max_work,
            work: 0,
        };
        tokenizer.switch_source_boundary(start);
        tokenizer
    }
    pub(crate) fn cap_work(&mut self, ceiling: usize) {
        self.max_work = self.max_work.min(ceiling);
    }
    pub(crate) fn position(&self) -> usize {
        self.inner.position
    }
    pub(crate) fn work(&self) -> usize {
        self.work
    }
    pub(crate) fn current(&self) -> Option<(TokenKind, &[u8], Range<usize>)> {
        let (kind, raw, span) = self.inner.current()?;
        Some((kind, raw, span.decoded.clone()))
    }
    pub(crate) fn release(&mut self) {
        self.inner.release();
    }
    /// Scans within the remaining work; the coordinator calls this only
    /// without a ready token.
    pub(crate) fn advance_content(&mut self, input: &[u8]) -> Result<ContentStep, Error> {
        let length = input.len().min(self.max_work.saturating_sub(self.work));
        let before = self.position();
        let result = self.inner.advance_content(&input[..length]);
        self.work += self.position() - before;
        let step = result?;
        self.validate_ready()?;
        if length < input.len() && step.stop == ContentStop::Exhausted {
            return Err(Error::new(WORK, self.position()));
        }
        Ok(step)
    }

    pub(crate) fn reference_cursor(&self) -> ReferenceCursor {
        self.inner.reference_cursor()
    }
    pub(crate) fn flush_text_before_reference(&mut self) -> Result<bool, Error> {
        let ready = self.inner.flush_text_before_reference();
        self.validate_ready()?;
        Ok(ready)
    }
    pub(crate) fn append_character_reference(
        &mut self,
        reference: ValidatedReference<'_>,
        extent: &ReferenceExtent,
    ) -> Result<(), Error> {
        self.work = self
            .work
            .checked_add(reference.raw().len())
            .filter(|n| *n <= self.max_work)
            .ok_or(Error::new(WORK, self.position()))?;
        self.inner.append_character_reference(reference, extent)
    }
    pub(crate) fn skip_general_reference(&mut self, extent: &ReferenceExtent) {
        self.inner.skip_general_reference(extent);
    }
    pub(crate) fn finish_step(&mut self) -> Result<(), Error> {
        self.inner.finish_step()?;
        self.validate_ready()
    }
    /// Restarts at local offset `start`; the owning lease calls this only at
    /// a clean token boundary (released token, no lookahead).
    pub(crate) fn switch_source_boundary(&mut self, start: usize) {
        let inner = &mut self.inner;
        (inner.start, inner.position) = (start, start);
        (inner.source_start, inner.source_position) = (start, start);
    }
    /// Charges a newly ready token's bytes; tokenizer sentinel prefixes can
    /// never be content of an internal parsed GE.
    fn validate_ready(&mut self) -> Result<(), Error> {
        let Some((kind, raw, span)) = self.inner.current() else {
            return Ok(());
        };
        let start = span.decoded.start;
        self.work = self
            .work
            .checked_add(raw.len())
            .filter(|work| *work <= self.max_work)
            .ok_or(Error::new(WORK, start))?;
        if matches!(kind, TokenKind::Dtd | TokenKind::InvalidMarkup) {
            return Err(Error::new(ErrorKind::Syntax, start));
        }
        Ok(())
    }
}
