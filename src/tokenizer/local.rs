//! A sealed replacement-local view of the existing XML tokenizer.
use super::reference::{ContentStep, ContentStop, ReferenceCursor, ReferenceExtent};
use super::{ReadySpan, Scan, TokenKind, Tokenizer};
use crate::ErrorKind;
use crate::dtd_reference::ValidatedReference;
use crate::escape::invalid_char;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct LocalUsage {
    pub(crate) scanned: usize,
    pub(crate) validated: usize,
    pub(crate) reference_copied: usize,
    pub(crate) work: usize,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LocalErrorKind {
    Xml(ErrorKind),
    ForbiddenMarkup,
    Work,
    PositionOverflow,
    InvalidState,
    Sealed,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LocalError {
    pub(crate) kind: LocalErrorKind,
    pub(crate) offset: usize,
}

pub(crate) struct LocalTokenizer {
    inner: Tokenizer,
    max_work: usize,
    usage: LocalUsage,
    validated_ready: bool,
    finalizing: bool,
    complete: bool,
    failure: Option<LocalError>,
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
            usage: LocalUsage::default(),
            validated_ready: false,
            finalizing: false,
            complete: false,
            failure: None,
        }
    }
    pub(crate) fn cap_work(&mut self, ceiling: usize) {
        self.max_work = self.max_work.min(ceiling);
    }
    pub(crate) fn position(&self) -> usize {
        self.inner.position
    }
    pub(crate) fn usage(&self) -> LocalUsage {
        self.usage
    }
    pub(crate) fn work_ceiling(&self) -> usize {
        self.max_work
    }
    pub(crate) fn scratch_capacity_requirement(&self) -> usize {
        self.inner.scratch_capacity_requirement()
    }
    pub(crate) fn is_fresh_source(&self) -> bool {
        self.failure.is_none()
            && self.usage.work == 0
            && !self.finalizing
            && !self.complete
            && self.clean_boundary()
            && self.inner.ready.is_none()
    }
    pub(crate) fn current(&self) -> Option<(TokenKind, &[u8], Range<usize>)> {
        if self.failure.is_some() || !self.validated_ready {
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
    pub(crate) fn advance_content(&mut self, input: &[u8]) -> Result<ContentStep, LocalError> {
        self.advance_mode::<true>(input)
    }
    fn advance_mode<const REFERENCES: bool>(
        &mut self,
        input: &[u8],
    ) -> Result<ContentStep, LocalError> {
        self.remember(|s| {
            if s.finalizing && !input.is_empty() {
                return Err(s.error(LocalErrorKind::Sealed, s.position()));
            }
            if s.inner.ready.is_some() || s.finalizing {
                return Ok(ContentStep {
                    consumed: 0,
                    stop: if s.inner.ready.is_some() {
                        ContentStop::TokenReady
                    } else {
                        ContentStop::Exhausted
                    },
                });
            }
            let remaining = s.max_work.saturating_sub(s.usage.work);
            let positions = usize::MAX - s.position();
            let length = input.len().min(remaining).min(positions);
            let before = s.position();
            let result = s.inner.advance_inner::<REFERENCES>(&input[..length]);
            let consumed = s.position() - before;
            s.usage.scanned += consumed;
            s.usage.work += consumed;
            let (_, reference) =
                result.map_err(|error| s.error(LocalErrorKind::Xml(error.kind), error.offset))?;
            s.validate_ready()?;
            if length < input.len() && s.inner.ready.is_none() && !reference {
                return Err(s.error(
                    if positions <= remaining && positions == length {
                        LocalErrorKind::PositionOverflow
                    } else {
                        LocalErrorKind::Work
                    },
                    s.position(),
                ));
            }
            Ok(ContentStep {
                consumed,
                stop: if reference {
                    ContentStop::ReferenceStart
                } else if s.inner.ready.is_some() {
                    ContentStop::TokenReady
                } else {
                    ContentStop::Exhausted
                },
            })
        })
    }

    pub(crate) fn reference_cursor(&mut self) -> Result<ReferenceCursor, LocalError> {
        self.remember(|s| {
            s.check_reference_lifecycle()?;
            s.inner
                .reference_cursor()
                .map_err(|e| s.error(LocalErrorKind::Xml(e.kind), e.offset))
        })
    }
    pub(crate) fn flush_text_before_reference(&mut self) -> Result<bool, LocalError> {
        self.remember(|s| {
            s.check_reference_lifecycle()?;
            let ready = s
                .inner
                .flush_text_before_reference()
                .map_err(|e| s.error(LocalErrorKind::Xml(e.kind), e.offset))?;
            s.validate_ready()?;
            Ok(ready)
        })
    }
    pub(crate) fn append_character_reference(
        &mut self,
        reference: ValidatedReference<'_>,
        extent: &ReferenceExtent,
    ) -> Result<(), LocalError> {
        self.remember(|s| {
            s.check_reference_lifecycle()?;
            let length = reference.raw().len();
            let next = s
                .usage
                .work
                .checked_add(length)
                .filter(|n| *n <= s.max_work)
                .ok_or_else(|| s.error(LocalErrorKind::Work, s.position()))?;
            s.usage.work = next;
            s.inner
                .append_character_reference(reference, extent)
                .map_err(|e| s.error(LocalErrorKind::Xml(e.kind), e.offset))?;
            s.usage.reference_copied += length;
            Ok(())
        })
    }
    pub(crate) fn skip_general_reference(
        &mut self,
        reference: ValidatedReference<'_>,
        extent: &ReferenceExtent,
    ) -> Result<(), LocalError> {
        self.remember(|s| {
            s.check_reference_lifecycle()?;
            s.inner
                .skip_general_reference(reference, extent)
                .map_err(|e| s.error(LocalErrorKind::Xml(e.kind), e.offset))
        })
    }
    fn check_reference_lifecycle(&self) -> Result<(), LocalError> {
        if self.finalizing || self.complete {
            Err(self.error(LocalErrorKind::Sealed, self.position()))
        } else {
            Ok(())
        }
    }
    pub(crate) fn finish_step(&mut self) -> Result<(), LocalError> {
        self.remember(|s| {
            s.finalizing = true;
            s.inner
                .finish_step()
                .map_err(|error| s.error(LocalErrorKind::Xml(error.kind), error.offset))?;
            s.validate_ready()?;
            s.complete = s.inner.ready.is_none() && s.clean_boundary();
            Ok(())
        })
    }
    /// The owning text lease calls this only after an acknowledged General
    /// boundary or completed source. Pausing a parent is not EOF.
    pub(crate) fn switch_source_boundary(&mut self, start: usize) -> Result<(), LocalError> {
        self.remember(|s| {
            if !s.clean_boundary() || s.inner.ready.is_some() {
                return Err(s.error(LocalErrorKind::InvalidState, s.position()));
            }
            s.inner.start = start;
            s.inner.position = start;
            s.finalizing = false;
            s.complete = false;
            s.validated_ready = false;
            Ok(())
        })
    }
    fn clean_boundary(&self) -> bool {
        self.inner.buffer.is_empty()
            && self.inner.lookahead.is_none()
            && matches!(self.inner.scan, Scan::Text)
    }
    fn validate_ready(&mut self) -> Result<(), LocalError> {
        if self.validated_ready {
            return Ok(());
        }
        let Some((kind, ReadySpan::Local(range))) = &self.inner.ready else {
            return Ok(());
        };
        let length = self.inner.buffer.len();
        let start = range.start;
        let next = self
            .usage
            .work
            .checked_add(length)
            .filter(|work| *work <= self.max_work)
            .ok_or_else(|| self.error(LocalErrorKind::Work, start))?;
        self.usage.work = next;
        self.usage.validated += length;
        // These tokenizer sentinel prefixes can never be content of an internal
        // parsed GE; reject before interpreting a potentially incomplete scalar.
        if matches!(kind, TokenKind::Dtd | TokenKind::InvalidMarkup) {
            return Err(self.error(LocalErrorKind::ForbiddenMarkup, start));
        }
        {
            let raw = std::str::from_utf8(&self.inner.buffer).map_err(|e| {
                self.error(
                    LocalErrorKind::Xml(ErrorKind::InvalidUtf8),
                    start + e.valid_up_to(),
                )
            })?;
            if let Some(offset) = invalid_char(raw) {
                return Err(self.error(
                    LocalErrorKind::Xml(ErrorKind::InvalidCharacter),
                    start + offset,
                ));
            }
        }
        self.validated_ready = true;
        Ok(())
    }
    fn error(&self, kind: LocalErrorKind, offset: usize) -> LocalError {
        LocalError { kind, offset }
    }
    fn remember<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, LocalError>,
    ) -> Result<T, LocalError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = f(self);
        if let Err(error) = result {
            self.failure = Some(error)
        }
        result
    }
}
