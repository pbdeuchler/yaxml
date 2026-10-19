//! A sealed replacement-local view of the existing XML tokenizer.
use super::reference::{ContentStep, ContentStop, ReferenceCursor, ReferenceExtent};
use super::{ReadySpan, Scan, TokenKind, Tokenizer};
use crate::dtd_reference::ValidatedReference;
use crate::escape::invalid_char;
use crate::{Error, ErrorKind, Limit};
use std::ops::Range;

const WORK: ErrorKind = ErrorKind::LimitExceeded(Limit::EntityExpansion);

/// Offsets are positions in the replacement text being tokenized.
pub(crate) struct LocalTokenizer {
    inner: Tokenizer,
    max_work: usize,
    work: usize,
    validated_ready: bool,
    finalizing: bool,
    complete: bool,
}
impl LocalTokenizer {
    pub(crate) fn new(start: usize, max_token_bytes: usize, max_work: usize) -> Self {
        let mut inner = Tokenizer::new(max_token_bytes, max_token_bytes);
        inner.local = true;
        inner.start = start;
        inner.position = start;
        Self {
            inner,
            max_work,
            work: 0,
            validated_ready: false,
            finalizing: false,
            complete: false,
        }
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
    pub(crate) fn scratch_capacity_requirement(&self) -> usize {
        self.inner.scratch_capacity_requirement()
    }
    pub(crate) fn current(&self) -> Option<(TokenKind, &[u8], Range<usize>)> {
        if !self.validated_ready {
            return None;
        }
        match self.inner.ready.as_ref()? {
            (kind, ReadySpan::Local(range)) => Some((*kind, &self.inner.buffer, range.clone())),
            (_, ReadySpan::Input(_)) => unreachable!("sealed local tokenizer"),
        }
    }
    pub(crate) fn release(&mut self) {
        if self.current().is_some() {
            self.inner.release();
            self.validated_ready = false;
            self.complete = self.finalizing && self.clean_boundary();
        }
    }
    pub(crate) fn advance_content(&mut self, input: &[u8]) -> Result<ContentStep, Error> {
        if self.finalizing && !input.is_empty() {
            return Err(Error::new(ErrorKind::InvalidState, self.position()));
        }
        if self.inner.ready.is_some() || self.finalizing {
            return Ok(ContentStep {
                consumed: 0,
                stop: match self.inner.ready {
                    Some(_) => ContentStop::TokenReady,
                    None => ContentStop::Exhausted,
                },
            });
        }
        let remaining = self.max_work.saturating_sub(self.work);
        let positions = usize::MAX - self.position();
        let length = input.len().min(remaining).min(positions);
        let before = self.position();
        let result = self.inner.advance_inner::<true>(&input[..length]);
        self.work += self.position() - before;
        let (_, reference) = result?;
        let consumed = self.position() - before;
        self.validate_ready()?;
        if length < input.len() && self.inner.ready.is_none() && !reference {
            let kind = match positions <= remaining && positions == length {
                true => ErrorKind::InvalidState,
                false => WORK,
            };
            return Err(Error::new(kind, self.position()));
        }
        Ok(ContentStep {
            consumed,
            stop: if reference {
                ContentStop::ReferenceStart
            } else if self.inner.ready.is_some() {
                ContentStop::TokenReady
            } else {
                ContentStop::Exhausted
            },
        })
    }

    pub(crate) fn reference_cursor(&mut self) -> Result<ReferenceCursor, Error> {
        self.check_reference_lifecycle()?;
        self.inner.reference_cursor()
    }
    pub(crate) fn flush_text_before_reference(&mut self) -> Result<bool, Error> {
        self.check_reference_lifecycle()?;
        let ready = self.inner.flush_text_before_reference()?;
        self.validate_ready()?;
        Ok(ready)
    }
    pub(crate) fn append_character_reference(
        &mut self,
        reference: ValidatedReference<'_>,
        extent: &ReferenceExtent,
    ) -> Result<(), Error> {
        self.check_reference_lifecycle()?;
        self.work = self
            .work
            .checked_add(reference.raw().len())
            .filter(|n| *n <= self.max_work)
            .ok_or(Error::new(WORK, self.position()))?;
        self.inner.append_character_reference(reference, extent)
    }
    pub(crate) fn skip_general_reference(
        &mut self,
        reference: ValidatedReference<'_>,
        extent: &ReferenceExtent,
    ) -> Result<(), Error> {
        self.check_reference_lifecycle()?;
        self.inner.skip_general_reference(reference, extent)
    }
    fn check_reference_lifecycle(&self) -> Result<(), Error> {
        match self.finalizing || self.complete {
            true => Err(Error::new(ErrorKind::InvalidState, self.position())),
            false => Ok(()),
        }
    }
    pub(crate) fn finish_step(&mut self) -> Result<(), Error> {
        self.finalizing = true;
        self.inner.finish_step()?;
        self.validate_ready()?;
        self.complete = self.inner.ready.is_none() && self.clean_boundary();
        Ok(())
    }
    /// The owning text lease calls this only after an acknowledged General
    /// boundary or completed source. Pausing a parent is not EOF.
    pub(crate) fn switch_source_boundary(&mut self, start: usize) -> Result<(), Error> {
        if !self.clean_boundary() || self.inner.ready.is_some() {
            return Err(Error::new(ErrorKind::InvalidState, self.position()));
        }
        self.inner.start = start;
        self.inner.position = start;
        self.finalizing = false;
        self.complete = false;
        self.validated_ready = false;
        Ok(())
    }
    fn clean_boundary(&self) -> bool {
        self.inner.buffer.is_empty()
            && self.inner.lookahead.is_none()
            && matches!(self.inner.scan, Scan::Text)
    }
    fn validate_ready(&mut self) -> Result<(), Error> {
        if self.validated_ready {
            return Ok(());
        }
        let Some((kind, ReadySpan::Local(range))) = &self.inner.ready else {
            return Ok(());
        };
        let start = range.start;
        self.work = self
            .work
            .checked_add(self.inner.buffer.len())
            .filter(|work| *work <= self.max_work)
            .ok_or(Error::new(WORK, start))?;
        // These tokenizer sentinel prefixes can never be content of an internal
        // parsed GE; reject before interpreting a potentially incomplete scalar.
        if matches!(kind, TokenKind::Dtd | TokenKind::InvalidMarkup) {
            return Err(Error::new(ErrorKind::Syntax, start));
        }
        let raw = std::str::from_utf8(&self.inner.buffer)
            .map_err(|e| Error::new(ErrorKind::InvalidUtf8, start + e.valid_up_to()))?;
        if let Some(offset) = invalid_char(raw) {
            return Err(Error::new(ErrorKind::InvalidCharacter, start + offset));
        }
        self.validated_ready = true;
        Ok(())
    }
}
