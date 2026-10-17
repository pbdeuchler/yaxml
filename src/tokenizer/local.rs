//! A sealed replacement-local view of the existing XML tokenizer.
use super::{ReadySpan, Scan, TokenKind, Tokenizer};
use crate::escape::xml_char;
use crate::{ErrorKind, Limit};
use std::ops::Range;

#[derive(Clone, Copy, Debug)]
pub(crate) struct LocalLimits {
    pub(crate) max_token_bytes: usize,
    pub(crate) max_scratch_bytes: usize,
    pub(crate) max_work: usize,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct LocalUsage {
    pub(crate) scanned: usize,
    pub(crate) validated: usize,
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
    limits: LocalLimits,
    usage: LocalUsage,
    validated_ready: bool,
    finalizing: bool,
    complete: bool,
    failure: Option<LocalError>,
}
impl LocalTokenizer {
    pub(crate) fn new(start: usize, limits: LocalLimits) -> Self {
        let mut inner = Tokenizer::new(limits.max_token_bytes, limits.max_scratch_bytes);
        inner.local = true;
        inner.start = start;
        inner.position = start;
        Self {
            inner,
            limits,
            usage: LocalUsage::default(),
            validated_ready: false,
            finalizing: false,
            complete: false,
            failure: None,
        }
    }
    pub(crate) fn cap_work(&mut self, ceiling: usize) {
        self.limits.max_work = self.limits.max_work.min(ceiling);
    }
    pub(crate) fn position(&self) -> usize {
        self.inner.position
    }
    pub(crate) fn usage(&self) -> LocalUsage {
        self.usage
    }
    pub(crate) fn is_complete(&self) -> bool {
        self.complete
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
    pub(crate) fn advance(&mut self, input: &[u8]) -> Result<usize, LocalError> {
        self.remember(|s| {
            if s.finalizing && !input.is_empty() {
                return Err(s.error(LocalErrorKind::Sealed, s.position()));
            }
            if s.inner.ready.is_some() || s.finalizing {
                return Ok(0);
            }
            let remaining = s.limits.max_work.saturating_sub(s.usage.work);
            let positions = usize::MAX - s.position();
            let length = input.len().min(remaining).min(positions);
            let before = s.position();
            let result = s.inner.advance(&input[..length]);
            let consumed = s.position() - before;
            s.usage.scanned += consumed;
            s.usage.work += consumed;
            result.map_err(|error| s.error(LocalErrorKind::Xml(error.kind), error.offset))?;
            s.validate_ready()?;
            if length < input.len() && s.inner.ready.is_none() {
                return Err(s.error(
                    if positions <= remaining && positions == length {
                        LocalErrorKind::PositionOverflow
                    } else {
                        LocalErrorKind::Work
                    },
                    s.position(),
                ));
            }
            Ok(consumed)
        })
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
    pub(crate) fn restart(&mut self, start: usize) -> Result<(), LocalError> {
        self.remember(|s| {
            if !s.complete || !s.clean_boundary() || s.inner.ready.is_some() {
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
            .filter(|work| *work <= self.limits.max_work)
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
            if let Some((offset, _)) = raw.char_indices().find(|(_, c)| !xml_char(*c)) {
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

#[cfg(test)]
#[path = "../../tests/internal/dtd_content_scan_author.rs"]
mod tests;
