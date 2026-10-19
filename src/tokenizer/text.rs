//! One active source lease and bounded Text/reference coordination.
use super::local::LocalTokenizer;
use super::reference::{ContentStep, ContentStop, ReferenceCursor, ReferenceExtent};
use super::{TokenKind, Tokenizer};
use crate::dtd::DtdLimits;
use crate::dtd_frames::EntityInvocationId;
use crate::dtd_reference::{ReferenceKind, ReferenceScratch, ValidatedReference};
use crate::encoding::{SourceEncoding, TokenSpan};
use crate::escape::invalid_char;
use crate::{Error, ErrorKind, Limit};
use std::ops::Range;

const WORK: ErrorKind = ErrorKind::LimitExceeded(Limit::EntityExpansion);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextSourceId {
    Input,
    Replacement(EntityInvocationId),
}
/// Input offsets are physical; replacement offsets are local UTF-8 bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TextError {
    pub(crate) kind: ErrorKind,
    pub(crate) source: TextSourceId,
    pub(crate) offset: usize,
}
impl TextError {
    fn at(source: TextSourceId) -> impl Fn(Error) -> Self {
        move |error| Self {
            kind: error.kind,
            source,
            offset: error.offset,
        }
    }
}
/// The same scratch allocation and work ledger move across source handoffs.
pub(crate) struct TextShared {
    limits: DtdLimits,
    work: usize,
    reference: Option<ReferenceScratch>,
}
impl TextShared {
    /// `initial_work` is the DTD phase's ledger, which never exceeds
    /// `limits.work`.
    pub(crate) fn new(limits: DtdLimits, initial_work: usize) -> Result<Self, ErrorKind> {
        // Both source slots and the reference scratch must be addressable.
        limits
            .scratch
            .checked_add(limits.scratch)
            .and_then(|tokens| tokens.checked_add(limits.token))
            .ok_or(ErrorKind::LimitExceeded(Limit::TokenBytes))?;
        Ok(Self {
            limits,
            work: initial_work,
            reference: None,
        })
    }
    pub(crate) fn work(&self) -> usize {
        self.work
    }
    fn remaining_work(&self) -> usize {
        self.limits.work - self.work
    }
    /// Adds work that a capped sub-ledger already bounded by `remaining_work`.
    pub(crate) fn spend(&mut self, work: usize) {
        self.work += work;
    }
    pub(crate) fn charge_external_work(
        &mut self,
        delta: usize,
        source: TextSourceId,
        offset: usize,
    ) -> Result<(), TextError> {
        self.work = self
            .work
            .checked_add(delta)
            .filter(|n| *n <= self.limits.work)
            .ok_or(TextError {
                kind: WORK,
                source,
                offset,
            })?;
        Ok(())
    }
}

enum InputMode<'input> {
    Streaming,
    Stable(&'input str),
}
enum LeaseKind<'input> {
    Input {
        tokenizer: Tokenizer,
        mode: InputMode<'input>,
        decoded: usize,
        physical: usize,
    },
    Local {
        tokenizer: LocalTokenizer,
        invocation: EntityInvocationId,
    },
}
/// Ownership, rather than matching offsets, binds pending work to its tokenizer.
pub(crate) struct TextSourceLease<'input> {
    kind: LeaseKind<'input>,
    finalizing: bool,
}
impl<'input> TextSourceLease<'input> {
    /// Resume decoded input from the same decoder at a complete token boundary.
    /// UTF-16 coordinates count decoded UTF-8 bytes and original transport bytes
    /// independently; configuring the encoding must not replay its BOM.
    pub(crate) fn resume(
        tokenizer: Tokenizer,
        encoding: SourceEncoding,
        decoded: usize,
        physical: usize,
    ) -> Self {
        Self::input(tokenizer, encoding, decoded, physical, InputMode::Streaming)
    }
    /// Resume at `position` of `input`, a complete UTF-8 source.
    pub(crate) fn resume_stable_utf8(
        tokenizer: Tokenizer,
        input: &'input str,
        position: usize,
    ) -> Self {
        let mode = InputMode::Stable(input);
        Self::input(tokenizer, SourceEncoding::Utf8, position, position, mode)
    }
    fn input(
        mut tokenizer: Tokenizer,
        encoding: SourceEncoding,
        decoded: usize,
        physical: usize,
        mode: InputMode<'input>,
    ) -> Self {
        debug_assert!(tokenizer.buffer.is_empty() && tokenizer.position == 0);
        tokenizer.encoding = encoding;
        (tokenizer.start, tokenizer.position) = (decoded, decoded);
        (tokenizer.source_start, tokenizer.source_position) = (physical, physical);
        Self {
            kind: LeaseKind::Input {
                tokenizer,
                mode,
                decoded,
                physical,
            },
            finalizing: false,
        }
    }
    pub(crate) fn local(tokenizer: LocalTokenizer, invocation: EntityInvocationId) -> Self {
        Self {
            kind: LeaseKind::Local {
                tokenizer,
                invocation,
            },
            finalizing: false,
        }
    }
    pub(crate) fn is_stable_input(&self) -> bool {
        self.stable().is_some()
    }
    pub(crate) fn id(&self) -> TextSourceId {
        match &self.kind {
            LeaseKind::Input { .. } => TextSourceId::Input,
            LeaseKind::Local { invocation, .. } => TextSourceId::Replacement(*invocation),
        }
    }
    pub(crate) fn input_position(&self) -> Option<(usize, usize)> {
        match self.kind {
            LeaseKind::Input {
                decoded, physical, ..
            } => Some((decoded, physical)),
            _ => None,
        }
    }
    fn stable(&self) -> Option<(&Tokenizer, &'input str, usize)> {
        match &self.kind {
            LeaseKind::Input {
                tokenizer,
                mode: InputMode::Stable(input),
                decoded,
                ..
            } => Some((tokenizer, input, *decoded)),
            _ => None,
        }
    }
    fn stable_remaining(&self) -> Option<&'input [u8]> {
        let (_, input, decoded) = self.stable()?;
        input.as_bytes().get(decoded..)
    }
    fn stable_current(&self) -> Option<(TokenKind, &'input str, TokenSpan)> {
        let (tokenizer, input, _) = self.stable()?;
        let (kind, _, span) = tokenizer.current()?;
        // Invalid markup can end inside a scalar; only its complete prefix is read.
        let raw = match kind {
            TokenKind::InvalidMarkup => {
                crate::stream::valid_prefix(&input.as_bytes()[span.decoded.clone()])
            }
            _ => input.get(span.decoded.clone())?,
        };
        Some((kind, raw, span.clone()))
    }
    fn current(&self) -> Option<TextToken<'_>> {
        match &self.kind {
            LeaseKind::Input { tokenizer, .. } => {
                tokenizer.current().map(|(kind, raw, span)| TextToken {
                    kind,
                    raw,
                    location: TextLocation::Input(span),
                })
            }
            LeaseKind::Local {
                tokenizer,
                invocation,
            } => tokenizer.current().map(|(kind, raw, range)| TextToken {
                kind,
                raw,
                location: TextLocation::Local {
                    invocation: *invocation,
                    range,
                },
            }),
        }
    }
    pub(crate) fn switch_local_at_boundary(
        &mut self,
        invocation: EntityInvocationId,
        start: usize,
    ) {
        let LeaseKind::Local {
            tokenizer,
            invocation: current,
        } = &mut self.kind
        else {
            unreachable!("only a local source switches invocation")
        };
        tokenizer.switch_source_boundary(start);
        *current = invocation;
        self.finalizing = false;
    }
    fn offset(&self) -> usize {
        match &self.kind {
            LeaseKind::Input { physical, .. } => *physical,
            LeaseKind::Local { tokenizer, .. } => tokenizer.position(),
        }
    }
    fn release(&mut self) {
        match &mut self.kind {
            LeaseKind::Input { tokenizer, .. } => tokenizer.release(),
            LeaseKind::Local { tokenizer, .. } => tokenizer.release(),
        }
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
pub(crate) enum TextLocation<'a> {
    Input(&'a TokenSpan),
    Local {
        invocation: EntityInvocationId,
        range: Range<usize>,
    },
}
pub(crate) struct TextToken<'a> {
    pub(crate) kind: TokenKind,
    pub(crate) raw: &'a [u8],
    pub(crate) location: TextLocation<'a>,
}
pub(crate) struct TextCoordinator<'input> {
    shared: TextShared,
    source: TextSourceLease<'input>,
    pending: Pending,
}
impl<'input> TextCoordinator<'input> {
    /// Token scratch never outgrows `limits.scratch`: each tokenizer's
    /// capacity is capped by its token ceilings, which the scratch doubles.
    pub(crate) fn new(shared: TextShared, source: TextSourceLease<'input>) -> Self {
        Self {
            shared,
            source,
            pending: Pending::Scanning,
        }
    }
    pub(crate) fn work(&self) -> usize {
        self.shared.work
    }
    pub(crate) fn remaining_work(&self) -> usize {
        self.shared.remaining_work()
    }
    pub(crate) fn charge_external_work(
        &mut self,
        delta: usize,
        source: TextSourceId,
        offset: usize,
    ) -> Result<(), TextError> {
        self.shared.charge_external_work(delta, source, offset)
    }
    pub(crate) fn spend(&mut self, work: usize) {
        self.shared.spend(work);
    }
    pub(crate) fn input_position(&self) -> Option<(usize, usize)> {
        self.source.input_position()
    }
    fn token_ready(&self) -> bool {
        matches!(self.pending, Pending::TokenReady | Pending::PrefixReady(_))
    }
    pub(crate) fn current(&self) -> Option<TextToken<'_>> {
        self.token_ready().then(|| self.source.current()).flatten()
    }
    pub(crate) fn is_stable_input(&self) -> bool {
        self.source.is_stable_input()
    }
    pub(crate) fn stable_remaining(&self) -> Option<&'input [u8]> {
        self.source.stable_remaining()
    }
    pub(crate) fn stable_current(&self) -> Option<(TokenKind, &'input str, TokenSpan)> {
        self.current()?;
        self.source.stable_current()
    }
    pub(crate) fn current_with_work(&mut self) -> Option<(TextToken<'_>, &mut TextShared)> {
        if !self.token_ready() {
            return None;
        }
        let token = self.source.current()?;
        Some((token, &mut self.shared))
    }
    pub(crate) fn general(
        &self,
    ) -> Option<(ValidatedReference<'_>, &ReferenceExtent, TextSourceId)> {
        let Pending::GeneralReady(extent) = &self.pending else {
            return None;
        };
        Some((
            self.shared.reference.as_ref()?.validated()?,
            extent,
            self.source.id(),
        ))
    }
    /// Scans streamed input, or the active replacement text.
    pub(crate) fn advance(&mut self, input: &[u8]) -> Result<TextStep, TextError> {
        self.drive(input)
    }
    pub(crate) fn finish_step(&mut self) -> Result<TextStep, TextError> {
        self.source.finalizing = true;
        self.drive(&[])
    }
    /// Releases the ready token; text flushed before a general reference
    /// leaves that reference ready.
    pub(crate) fn release_token(&mut self) {
        self.source.release();
        self.pending = match std::mem::replace(&mut self.pending, Pending::Scanning) {
            Pending::PrefixReady(extent) => {
                self.skip_general(&extent);
                Pending::GeneralReady(extent)
            }
            _ => Pending::Scanning,
        };
    }
    /// Drops a ready general reference as empty replacement text and resumes
    /// scanning the same source after it.
    pub(crate) fn discard_general(&mut self) {
        self.release_general();
        self.pending = Pending::Scanning;
    }
    /// Releases a ready general reference whose entity the caller enters:
    /// it switches or swaps the source and then calls `resume`.
    pub(crate) fn release_general(&mut self) {
        debug_assert!(matches!(self.pending, Pending::GeneralReady(_)));
        if let Some(reference) = &mut self.shared.reference {
            reference.release();
        }
    }
    /// The boundary source, to switch to a new invocation or swap for another.
    pub(crate) fn source_mut(&mut self) -> &mut TextSourceLease<'input> {
        &mut self.source
    }
    /// Scans the (possibly replaced) source, as `new` would.
    pub(crate) fn resume(&mut self) {
        self.pending = Pending::Scanning;
    }
    fn drive(&mut self, input: &[u8]) -> Result<TextStep, TextError> {
        let mut consumed = 0;
        loop {
            let outcome = match &self.pending {
                Pending::TokenReady | Pending::PrefixReady(_) => TextOutcome::TokenReady,
                Pending::GeneralReady(_) => TextOutcome::GeneralReferenceReady,
                Pending::Complete => TextOutcome::Complete,
                Pending::Scanning => {
                    let step = self.scan(&input[consumed..])?;
                    consumed += step.consumed;
                    match step.stop {
                        ContentStop::TokenReady => self.pending = Pending::TokenReady,
                        ContentStop::ReferenceStart => self.start_reference(),
                        ContentStop::Exhausted if self.source.finalizing => self.finish_source()?,
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
                        self.collect(cursor, &input[consumed..], &mut consumed)?;
                        continue;
                    }
                    if !self.source.finalizing {
                        TextOutcome::NeedInput
                    } else {
                        let Err(error) = self.reference(ReferenceScratch::finish_step);
                        return Err(self.reference_error(error, cursor));
                    }
                }
            };
            return Ok(TextStep { consumed, outcome });
        }
    }
    fn start_reference(&mut self) {
        let cursor = self.cursor();
        let start = cursor.decoded_position();
        match &mut self.shared.reference {
            Some(reference) => reference.restart(start),
            None => {
                let limits = self.shared.limits;
                self.shared.reference =
                    Some(ReferenceScratch::new(start, limits.token, limits.work));
            }
        }
        self.pending = Pending::Collecting(cursor);
    }
    /// Feeds reference bytes from `rest`, then completes a validated spelling.
    fn collect(
        &mut self,
        cursor: ReferenceCursor,
        rest: &[u8],
        consumed: &mut usize,
    ) -> Result<(), TextError> {
        let source = self.source.id();
        let remaining = self.shared.remaining_work();
        let reference = self.shared.reference.as_mut().expect("collecting scratch");
        reference.cap_work(reference.work().saturating_add(remaining));
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
        let next = cursor
            .checked_advance(bytes)
            .map_err(TextError::at(source))?;
        let mut accepted = 0;
        let result = self.reference(|reference| {
            let before = reference.position();
            let result = reference.advance(bytes);
            accepted = reference.position() - before;
            result
        });
        if accepted != 0 {
            *consumed += accepted;
            if let LeaseKind::Input {
                decoded, physical, ..
            } = &mut self.source.kind
            {
                *decoded = next.decoded_position();
                *physical = next.input_position();
            }
            self.pending = Pending::Collecting(next);
        }
        result.map_err(|error| self.reference_error(error, next))?;
        let Some(view) = self
            .shared
            .reference
            .as_ref()
            .and_then(ReferenceScratch::validated)
        else {
            return Ok(());
        };
        let extent = next.finish();
        if view.kind() == ReferenceKind::General {
            if self.flush_prefix()? {
                self.pending = Pending::PrefixReady(extent);
            } else {
                self.skip_general(&extent);
                self.pending = Pending::GeneralReady(extent);
            }
        } else {
            self.append_character(&extent)?;
            if let Some(reference) = &mut self.shared.reference {
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
        operation: impl FnOnce(&mut ReferenceScratch) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let remaining = self.shared.remaining_work();
        let reference = self.shared.reference.as_mut().expect("collecting scratch");
        let before = reference.work();
        reference.cap_work(before.saturating_add(remaining));
        let result = operation(reference);
        let spent = reference.work() - before;
        self.shared.spend(spent);
        result
    }
    fn scan(&mut self, input: &[u8]) -> Result<ContentStep, TextError> {
        let source = self.source.id();
        let remaining = self.shared.remaining_work();
        let (tokenizer, decoded, physical) = match &mut self.source.kind {
            LeaseKind::Input {
                tokenizer,
                decoded,
                physical,
                ..
            } => (tokenizer, decoded, physical),
            LeaseKind::Local {
                tokenizer,
                invocation,
            } => {
                let source = TextSourceId::Replacement(*invocation);
                return local(&mut self.shared, tokenizer, source, |tokenizer| {
                    tokenizer.advance_content(input)
                });
            }
        };
        let before = tokenizer.position;
        let length = input.len().min(remaining);
        let safe = checked_input_prefix(tokenizer, &input[..length]);
        let result = tokenizer.advance_content(&input[..safe]);
        let scanned = tokenizer.position - before;
        self.shared
            .charge_external_work(scanned, source, *physical)?;
        *decoded = tokenizer.position;
        *physical = tokenizer.source_position;
        let step = result.map_err(TextError::at(source))?;
        let exhausted = step.stop == ContentStop::Exhausted;
        let error = |kind| TextError {
            kind,
            source,
            offset: *physical,
        };
        if safe < length && exhausted {
            return Err(error(ErrorKind::InvalidState));
        }
        if step.stop == ContentStop::TokenReady {
            validate_input(&mut self.shared, tokenizer)?;
        }
        if length < input.len() && exhausted {
            return Err(error(WORK));
        }
        Ok(step)
    }
    fn cursor(&self) -> ReferenceCursor {
        match &self.source.kind {
            LeaseKind::Input { tokenizer, .. } => tokenizer.reference_cursor(),
            LeaseKind::Local { tokenizer, .. } => tokenizer.reference_cursor(),
        }
    }
    fn flush_prefix(&mut self) -> Result<bool, TextError> {
        let source = self.source.id();
        match &mut self.source.kind {
            LeaseKind::Input { tokenizer, .. } => {
                let ready = tokenizer.flush_text_before_reference();
                if ready {
                    validate_input(&mut self.shared, tokenizer)?;
                }
                Ok(ready)
            }
            LeaseKind::Local { tokenizer, .. } => local(
                &mut self.shared,
                tokenizer,
                source,
                LocalTokenizer::flush_text_before_reference,
            ),
        }
    }
    fn append_character(&mut self, extent: &ReferenceExtent) -> Result<(), TextError> {
        let (source, offset) = (self.source.id(), self.source.offset());
        // The scratch steps aside so the tokenizers can borrow its spelling.
        let reference = self.shared.reference.take().expect("validated scratch");
        let view = reference.validated().expect("validated reference");
        let result = match &mut self.source.kind {
            LeaseKind::Input { tokenizer, .. } => {
                // Charge before the possible token-buffer reservation.
                let result = self
                    .shared
                    .charge_external_work(view.raw().len(), source, offset);
                result.and_then(|()| {
                    tokenizer
                        .append_character_reference(view, extent)
                        .map_err(TextError::at(source))
                })
            }
            LeaseKind::Local { tokenizer, .. } => {
                local(&mut self.shared, tokenizer, source, |tokenizer| {
                    tokenizer.append_character_reference(view, extent)
                })
            }
        };
        self.shared.reference = Some(reference);
        result
    }
    fn skip_general(&mut self, extent: &ReferenceExtent) {
        match &mut self.source.kind {
            LeaseKind::Input { tokenizer, .. } => tokenizer.skip_general_reference(extent),
            LeaseKind::Local { tokenizer, .. } => tokenizer.skip_general_reference(extent),
        }
    }
    fn finish_source(&mut self) -> Result<(), TextError> {
        let source = self.source.id();
        let ready = match &mut self.source.kind {
            LeaseKind::Input { tokenizer, .. } => {
                tokenizer.finish_step().map_err(TextError::at(source))?;
                let ready = tokenizer.current().is_some();
                if ready {
                    validate_input(&mut self.shared, tokenizer)?;
                }
                ready
            }
            LeaseKind::Local { tokenizer, .. } => {
                local(&mut self.shared, tokenizer, source, |tokenizer| {
                    tokenizer.finish_step()?;
                    Ok(tokenizer.current().is_some())
                })?
            }
        };
        self.pending = match ready {
            true => Pending::TokenReady,
            false => Pending::Complete,
        };
        Ok(())
    }
    fn reference_error(&self, error: Error, cursor: ReferenceCursor) -> TextError {
        let raw = self
            .shared
            .reference
            .as_ref()
            .map(ReferenceScratch::collected_bytes)
            .unwrap_or(&[]);
        TextError {
            kind: error.kind,
            source: self.source.id(),
            offset: cursor.map_offset(raw, error.offset),
        }
    }
}
/// Runs one local-tokenizer operation within the remaining work, then
/// charges what it spent, which the cap keeps within the shared ledger.
fn local<T>(
    shared: &mut TextShared,
    tokenizer: &mut LocalTokenizer,
    source: TextSourceId,
    operation: impl FnOnce(&mut LocalTokenizer) -> Result<T, Error>,
) -> Result<T, TextError> {
    let before = tokenizer.work();
    tokenizer.cap_work(before.saturating_add(shared.remaining_work()));
    let result = operation(tokenizer);
    shared.spend(tokenizer.work() - before);
    result.map_err(TextError::at(source))
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
fn validate_input(shared: &mut TextShared, tokenizer: &Tokenizer) -> Result<(), TextError> {
    let input = |kind, offset| TextError {
        kind,
        source: TextSourceId::Input,
        offset,
    };
    let (kind, bytes, span) = tokenizer.current().expect("ready input token");
    shared.charge_external_work(bytes.len(), TextSourceId::Input, span.source.start)?;
    if kind == TokenKind::InvalidMarkup {
        return Ok(());
    }
    let raw = std::str::from_utf8(bytes)
        .map_err(|e| input(ErrorKind::InvalidUtf8, span.source.start + e.valid_up_to()))?;
    if let Some(offset) = invalid_char(raw) {
        let mapped = span.map_error(
            raw,
            Error::new(ErrorKind::InvalidCharacter, span.decoded.start + offset),
        );
        return Err(input(mapped.kind, mapped.offset));
    }
    Ok(())
}
