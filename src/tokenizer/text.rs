//! Streamed input's Text/reference coordination: tokens stop before general
//! references, whose spellings are collected across chunks.
use super::reference::{ContentStop, ReferenceCursor, ReferenceExtent};
use super::{TokenKind, Tokenizer};
use crate::dtd::DtdLimits;
use crate::dtd_reference::{ReferenceKind, ReferenceScratch, ValidatedReference};
use crate::encoding::{SourceEncoding, TokenSpan};
use crate::escape::invalid_char;
use crate::{Error, ErrorKind, Limit};

const WORK: ErrorKind = ErrorKind::LimitExceeded(Limit::EntityExpansion);

/// The content phase's work ledger, which every source charges.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Work {
    limit: usize,
    used: usize,
}

impl Work {
    /// `used` is the DTD phase's ledger, which never exceeds `limit`.
    pub(crate) fn new(limit: usize, used: usize) -> Self {
        Self { limit, used }
    }
    pub(crate) fn remaining(self) -> usize {
        self.limit - self.used
    }
    /// Adds work that a capped sub-ledger already bounded by `remaining`.
    pub(crate) fn spend(&mut self, work: usize) {
        self.used += work;
    }
    pub(crate) fn charge(&mut self, delta: usize, offset: usize) -> Result<(), Error> {
        self.used = self
            .used
            .checked_add(delta)
            .filter(|n| *n <= self.limit)
            .ok_or(Error::new(WORK, offset))?;
        Ok(())
    }
}

#[derive(Clone)]
enum Pending {
    Scanning,
    Collecting(ReferenceCursor),
    TokenReady,
    PrefixReady(ReferenceExtent),
    GeneralReady(ReferenceExtent),
    Complete,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextOutcome {
    TokenReady,
    GeneralReferenceReady,
    NeedInput,
    Complete,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TextStep {
    pub(crate) consumed: usize,
    pub(crate) outcome: TextOutcome,
}

/// Streamed physical input, resumed from the same decoder at a complete
/// token boundary. Offsets in errors are physical.
pub(crate) struct TextCoordinator {
    tokenizer: Tokenizer,
    decoded: usize,
    physical: usize,
    finalizing: bool,
    pending: Pending,
    limits: DtdLimits,
    /// The reference scratch, allocated on the first reference.
    reference: Option<ReferenceScratch>,
}
impl TextCoordinator {
    /// UTF-16 coordinates count decoded UTF-8 bytes and original transport
    /// bytes independently; configuring the encoding must not replay its BOM.
    pub(crate) fn new(
        mut tokenizer: Tokenizer,
        encoding: SourceEncoding,
        (decoded, physical): (usize, usize),
        limits: DtdLimits,
    ) -> Self {
        debug_assert!(tokenizer.buffer.is_empty() && tokenizer.position == 0);
        tokenizer.encoding = encoding;
        (tokenizer.start, tokenizer.position) = (decoded, decoded);
        (tokenizer.source_start, tokenizer.source_position) = (physical, physical);
        Self {
            tokenizer,
            decoded,
            physical,
            finalizing: false,
            pending: Pending::Scanning,
            limits,
            reference: None,
        }
    }
    pub(crate) fn input_position(&self) -> (usize, usize) {
        (self.decoded, self.physical)
    }
    /// The position of UTF-8 input scanned up to a clean token boundary,
    /// with nothing buffered, pending or replayed.
    pub(crate) fn idle_utf8(&self) -> Option<usize> {
        let idle = matches!(self.pending, Pending::Scanning) && !self.finalizing;
        match self.tokenizer.idle_utf8() {
            Some((position, false)) if idle => Some(position),
            _ => None,
        }
    }
    /// Resumes an idle coordinator at `position`, past UTF-8 input read in
    /// place.
    pub(crate) fn resume_at(&mut self, position: usize) {
        let tokenizer = &mut self.tokenizer;
        (tokenizer.start, tokenizer.position) = (position, position);
        (tokenizer.source_start, tokenizer.source_position) = (position, position);
        (self.decoded, self.physical) = (position, position);
    }
    pub(crate) fn current(&self) -> Option<(TokenKind, &[u8], &TokenSpan)> {
        match self.pending {
            Pending::TokenReady | Pending::PrefixReady(_) => self.tokenizer.current(),
            _ => None,
        }
    }
    pub(crate) fn general(&self) -> Option<(ValidatedReference<'_>, &ReferenceExtent)> {
        let Pending::GeneralReady(extent) = &self.pending else {
            return None;
        };
        Some((self.reference.as_ref()?.validated()?, extent))
    }
    /// Scans streamed input; `finish_step` declares its end.
    pub(crate) fn advance(&mut self, input: &[u8], work: &mut Work) -> Result<TextStep, Error> {
        self.drive(input, work)
    }
    pub(crate) fn finish_step(&mut self, work: &mut Work) -> Result<TextStep, Error> {
        self.finalizing = true;
        self.drive(&[], work)
    }
    /// Releases the ready token; text flushed before a general reference
    /// leaves that reference ready.
    pub(crate) fn release_token(&mut self) {
        self.tokenizer.release();
        self.pending = match std::mem::replace(&mut self.pending, Pending::Scanning) {
            Pending::PrefixReady(extent) => {
                self.tokenizer.skip_general_reference(&extent);
                Pending::GeneralReady(extent)
            }
            _ => Pending::Scanning,
        };
    }
    /// Releases a ready general reference, entered or expanding to nothing,
    /// and resumes scanning after it.
    pub(crate) fn release_general(&mut self) {
        debug_assert!(matches!(self.pending, Pending::GeneralReady(_)));
        if let Some(reference) = &mut self.reference {
            reference.release();
        }
        self.pending = Pending::Scanning;
    }
    fn drive(&mut self, input: &[u8], work: &mut Work) -> Result<TextStep, Error> {
        let mut consumed = 0;
        loop {
            let outcome = match &self.pending {
                Pending::TokenReady | Pending::PrefixReady(_) => TextOutcome::TokenReady,
                Pending::GeneralReady(_) => TextOutcome::GeneralReferenceReady,
                Pending::Complete => TextOutcome::Complete,
                Pending::Scanning => {
                    let stop = self.scan(&input[consumed..], work, &mut consumed)?;
                    match stop {
                        ContentStop::TokenReady => self.pending = Pending::TokenReady,
                        ContentStop::ReferenceStart => self.start_reference(),
                        ContentStop::Exhausted if self.finalizing => self.finish_source(work)?,
                        ContentStop::Exhausted => {
                            return Ok(TextStep {
                                consumed,
                                outcome: TextOutcome::NeedInput,
                            });
                        }
                    }
                    continue;
                }
                Pending::Collecting(cursor) => {
                    let cursor = *cursor;
                    if consumed < input.len() {
                        self.collect(cursor, &input[consumed..], work, &mut consumed)?;
                        continue;
                    }
                    if !self.finalizing {
                        TextOutcome::NeedInput
                    } else {
                        let Err(error) = self.reference(work, ReferenceScratch::finish_step);
                        return Err(self.reference_error(error, cursor));
                    }
                }
            };
            return Ok(TextStep { consumed, outcome });
        }
    }
    fn start_reference(&mut self) {
        let cursor = self.tokenizer.reference_cursor();
        let start = cursor.decoded_position();
        match &mut self.reference {
            Some(reference) => reference.restart(start),
            None => {
                let limits = self.limits;
                self.reference = Some(ReferenceScratch::new(start, limits.token, limits.work));
            }
        }
        self.pending = Pending::Collecting(cursor);
    }
    /// Feeds reference bytes from `rest`, then completes a validated spelling.
    fn collect(
        &mut self,
        cursor: ReferenceCursor,
        rest: &[u8],
        work: &mut Work,
        consumed: &mut usize,
    ) -> Result<(), Error> {
        let reference = self.reference.as_mut().expect("collecting scratch");
        reference.cap_work(reference.work().saturating_add(work.remaining()));
        // A whole simple ASCII reference cannot fail before its `;`, so it is
        // fed at once; anything else goes byte by byte.
        let bytes = memchr::memchr(b';', &rest[..rest.len().min(64)])
            .map(|end| &rest[..=end])
            .filter(|window| {
                window.len() > 2
                    && window[1..window.len() - 1]
                        .iter()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-.:#".contains(b))
                    && reference.fits(window)
                    && cursor.checked_advance(window).is_ok()
            })
            .unwrap_or(&rest[..1]);
        let next = cursor.checked_advance(bytes)?;
        let mut accepted = 0;
        let result = self.reference(work, |reference| {
            let before = reference.position();
            let result = reference.advance(bytes);
            accepted = reference.position() - before;
            result
        });
        if accepted != 0 {
            *consumed += accepted;
            (self.decoded, self.physical) = (next.decoded_position(), next.input_position());
            self.pending = Pending::Collecting(next);
        }
        result.map_err(|error| self.reference_error(error, next))?;
        let Some(view) = self
            .reference
            .as_ref()
            .and_then(ReferenceScratch::validated)
        else {
            return Ok(());
        };
        let extent = next.finish();
        if view.kind() == ReferenceKind::General {
            if self.flush_prefix(work)? {
                self.pending = Pending::PrefixReady(extent);
            } else {
                self.tokenizer.skip_general_reference(&extent);
                self.pending = Pending::GeneralReady(extent);
            }
        } else {
            self.append_character(&extent, work)?;
            if let Some(reference) = &mut self.reference {
                reference.release();
            }
            self.pending = Pending::Scanning;
        }
        Ok(())
    }
    /// Runs one reference-scratch operation within the remaining work, then
    /// charges what it spent.
    fn reference<T>(
        &mut self,
        work: &mut Work,
        operation: impl FnOnce(&mut ReferenceScratch) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let reference = self.reference.as_mut().expect("collecting scratch");
        let before = reference.work();
        reference.cap_work(before.saturating_add(work.remaining()));
        let result = operation(reference);
        work.spend(reference.work() - before);
        result
    }
    fn scan(
        &mut self,
        input: &[u8],
        work: &mut Work,
        consumed: &mut usize,
    ) -> Result<ContentStop, Error> {
        let tokenizer = &mut self.tokenizer;
        let before = tokenizer.position;
        let length = input.len().min(work.remaining());
        let safe = checked_input_prefix(tokenizer, &input[..length]);
        let result = tokenizer.advance_content(&input[..safe]);
        work.charge(tokenizer.position - before, self.physical)?;
        (self.decoded, self.physical) = (tokenizer.position, tokenizer.source_position);
        let step = result?;
        *consumed += step.consumed;
        let exhausted = step.stop == ContentStop::Exhausted;
        if safe < length && exhausted {
            return Err(Error::new(ErrorKind::InvalidState, self.physical));
        }
        if step.stop == ContentStop::TokenReady {
            validate_input(work, tokenizer)?;
        }
        if length < input.len() && exhausted {
            return Err(Error::new(WORK, self.physical));
        }
        Ok(step.stop)
    }
    fn flush_prefix(&mut self, work: &mut Work) -> Result<bool, Error> {
        let ready = self.tokenizer.flush_text_before_reference();
        if ready {
            validate_input(work, &self.tokenizer)?;
        }
        Ok(ready)
    }
    fn append_character(&mut self, extent: &ReferenceExtent, work: &mut Work) -> Result<(), Error> {
        let reference = self.reference.as_ref().expect("validated scratch");
        let view = reference.validated().expect("validated reference");
        // Charge before the possible token-buffer reservation.
        work.charge(view.raw().len(), self.physical)?;
        self.tokenizer.append_character_reference(view, extent)
    }
    fn finish_source(&mut self, work: &mut Work) -> Result<(), Error> {
        self.tokenizer.finish_step()?;
        let ready = self.tokenizer.current().is_some();
        if ready {
            validate_input(work, &self.tokenizer)?;
        }
        self.pending = match ready {
            true => Pending::TokenReady,
            false => Pending::Complete,
        };
        Ok(())
    }
    fn reference_error(&self, error: Error, cursor: ReferenceCursor) -> Error {
        let raw = self
            .reference
            .as_ref()
            .map(ReferenceScratch::collected_bytes)
            .unwrap_or(&[]);
        Error::new(error.kind, cursor.map_offset(raw, error.offset))
    }
}
fn checked_input_prefix(tokenizer: &Tokenizer, input: &[u8]) -> usize {
    let decoded_headroom = usize::MAX - tokenizer.position;
    let physical_headroom = usize::MAX - tokenizer.source_position;
    if tokenizer.encoding == SourceEncoding::Utf8 {
        return input.len().min(decoded_headroom).min(physical_headroom);
    }
    if input.len() <= decoded_headroom && input.len() <= physical_headroom / 4 {
        return input.len();
    }
    let mut decoded = tokenizer.position;
    let mut physical = tokenizer.source_position;
    for (index, byte) in input.iter().copied().enumerate() {
        let Some(next_decoded) = decoded.checked_add(1) else {
            return index;
        };
        let Some(next_physical) = physical.checked_add(tokenizer.encoding.source_length(&[byte]))
        else {
            return index;
        };
        decoded = next_decoded;
        physical = next_physical;
    }
    input.len()
}
fn validate_input(work: &mut Work, tokenizer: &Tokenizer) -> Result<(), Error> {
    let (kind, bytes, span) = tokenizer.current().expect("ready input token");
    work.charge(bytes.len(), span.source.start)?;
    if kind == TokenKind::InvalidMarkup {
        return Ok(());
    }
    let raw = std::str::from_utf8(bytes)
        .map_err(|e| Error::new(ErrorKind::InvalidUtf8, span.source.start + e.valid_up_to()))?;
    if let Some(offset) = invalid_char(raw) {
        return Err(span.map_error(
            raw,
            Error::new(ErrorKind::InvalidCharacter, span.decoded.start + offset),
        ));
    }
    Ok(())
}
