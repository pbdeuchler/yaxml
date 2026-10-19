//! One active source lease and bounded Text/reference coordination.
use super::local::{LocalError, LocalErrorKind, LocalTokenizer};
use super::reference::{
    ContentStep, ContentStop, ReferenceCursor, ReferenceCursorError, ReferenceCursorErrorKind,
    ReferenceExtent,
};
use super::{TokenKind, Tokenizer};
use crate::dtd_frames::EntityInvocationId;
use crate::dtd_reference::{
    ReferenceError, ReferenceErrorKind, ReferenceKind, ReferenceLimit, ReferenceLimits,
    ReferenceScratch, ValidatedReference,
};
use crate::encoding::{SourceEncoding, TokenSpan};
use crate::escape::xml_char;
use crate::{Error, ErrorKind};
#[cfg(test)]
use std::cell::Cell;
use std::ops::Range;

#[cfg(test)]
thread_local! {
    static PANIC_ON_NEXT_RELEASE: Cell<bool> = const { Cell::new(false) };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextSourceId {
    Input,
    Replacement(EntityInvocationId),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextErrorKind {
    Xml(ErrorKind),
    Local(LocalErrorKind),
    Reference(ReferenceErrorKind),
    Cursor(ReferenceCursorErrorKind),
    Work,
    PositionOverflow,
    Scratch,
    ScratchOverflow,
    InvalidState,
    SourceMismatch,
}
/// Input offsets are physical; replacement offsets are local UTF-8 bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TextError {
    pub(crate) kind: TextErrorKind,
    pub(crate) source: TextSourceId,
    pub(crate) offset: usize,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct TextScratchLimits {
    pub(crate) input_token_bytes: usize,
    pub(crate) replacement_token_bytes: usize,
    pub(crate) max_bytes: usize,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct TextLimits {
    pub(crate) max_work: usize,
    pub(crate) reference: ReferenceLimits,
    pub(crate) scratch: TextScratchLimits,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TextUsage {
    pub(crate) work: usize,
    pub(crate) source_scanned: usize,
    pub(crate) reference_scanned: usize,
    pub(crate) reference_copied: usize,
    pub(crate) reference_classified: usize,
    pub(crate) appended_reference_copied: usize,
    pub(crate) validated: usize,
}
/// The same scratch allocation and counters move across source handoffs.
pub(crate) struct TextShared {
    limits: TextLimits,
    usage: TextUsage,
    reference: Option<ReferenceScratch>,
    failure: Option<TextError>,
}
impl TextShared {
    pub(crate) fn new(limits: TextLimits, initial_work: usize) -> Result<Self, TextErrorKind> {
        if initial_work > limits.max_work {
            return Err(TextErrorKind::Work);
        }
        let reserved = limits
            .scratch
            .input_token_bytes
            .checked_add(limits.scratch.replacement_token_bytes)
            .and_then(|tokens| {
                tokens.checked_add(
                    limits
                        .reference
                        .max_bytes
                        .min(limits.reference.max_scratch_bytes),
                )
            })
            .ok_or(TextErrorKind::ScratchOverflow)?;
        if reserved > limits.scratch.max_bytes {
            return Err(TextErrorKind::Scratch);
        }
        Ok(Self {
            limits,
            usage: TextUsage {
                work: initial_work,
                ..TextUsage::default()
            },
            reference: None,
            failure: None,
        })
    }
    pub(crate) fn usage(&self) -> TextUsage {
        self.usage
    }
    pub(crate) fn remaining_work(&self) -> Result<usize, TextError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        Ok(self
            .limits
            .max_work
            .checked_sub(self.usage.work)
            .expect("accepted Text work stays within its limit"))
    }
    pub(crate) fn charge_external_work(
        &mut self,
        delta: usize,
        source: TextSourceId,
        offset: usize,
    ) -> Result<(), TextError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = charge(self, source, offset, delta);
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }
    pub(crate) fn check_source(&self, source: &TextSourceLease<'_>) -> Result<(), TextError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let (required, slot) = match &source.kind {
            LeaseKind::Input { tokenizer, .. } => (
                tokenizer.scratch_capacity_requirement(),
                self.limits.scratch.input_token_bytes,
            ),
            LeaseKind::Local { tokenizer, .. } => (
                tokenizer.scratch_capacity_requirement(),
                self.limits.scratch.replacement_token_bytes,
            ),
        };
        if required > slot {
            Err(TextError {
                kind: TextErrorKind::Scratch,
                source: source.id(),
                offset: source.offset(),
            })
        } else {
            Ok(())
        }
    }
}

enum InputMode<'input> {
    Streaming,
    Stable {
        input: &'input str,
        decoded_start: usize,
    },
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
        native_work: usize,
    },
}
/// Ownership, rather than matching offsets, binds pending work to its tokenizer.
pub(crate) struct TextSourceLease<'input> {
    kind: LeaseKind<'input>,
    finalizing: bool,
    boundary: bool,
}
impl<'input> TextSourceLease<'input> {
    pub(crate) fn streaming(tokenizer: Tokenizer) -> Self {
        let decoded = tokenizer.position;
        let physical = tokenizer.source_position;
        Self {
            kind: LeaseKind::Input {
                tokenizer,
                mode: InputMode::Streaming,
                decoded,
                physical,
            },
            finalizing: false,
            boundary: false,
        }
    }
    pub(crate) fn resume_utf8(
        tokenizer: Tokenizer,
        decoded_position: usize,
        physical_position: usize,
    ) -> Result<Self, TextError> {
        Self::resume(
            tokenizer,
            SourceEncoding::Utf8,
            decoded_position,
            physical_position,
        )
    }
    /// Resume decoded input from the same decoder at a complete token boundary.
    /// UTF-16 coordinates count decoded UTF-8 bytes and original transport bytes
    /// independently; configuring the encoding must not replay its BOM.
    pub(crate) fn resume(
        mut tokenizer: Tokenizer,
        encoding: SourceEncoding,
        decoded_position: usize,
        physical_position: usize,
    ) -> Result<Self, TextError> {
        let coordinates = match encoding {
            SourceEncoding::Utf8 => decoded_position == physical_position,
            SourceEncoding::Utf16 { bom, .. } => {
                physical_position.is_multiple_of(2) && (!bom || physical_position >= 2)
            }
            // A byte charset begins after a UTF-8-forwarded ASCII declaration.
            SourceEncoding::Byte(_) => decoded_position >= physical_position,
        };
        let valid = coordinates
            && tokenizer.encoding == SourceEncoding::Utf8
            && tokenizer.buffer.is_empty()
            && tokenizer.position == 0
            && tokenizer.source_position == 0
            && tokenizer.start == 0
            && tokenizer.source_start == 0
            && tokenizer.reference_cursor().is_ok();
        if !valid {
            return Err(TextError {
                kind: TextErrorKind::InvalidState,
                source: TextSourceId::Input,
                offset: physical_position,
            });
        }
        tokenizer.encoding = encoding;
        tokenizer.start = decoded_position;
        tokenizer.position = decoded_position;
        tokenizer.source_start = physical_position;
        tokenizer.source_position = physical_position;
        Ok(Self::streaming(tokenizer))
    }
    pub(crate) fn resume_stable_utf8(
        tokenizer: Tokenizer,
        input: &'input str,
        decoded_position: usize,
        physical_position: usize,
    ) -> Result<Self, TextError> {
        let source = Self::resume_utf8(tokenizer, decoded_position, physical_position)?;
        let LeaseKind::Input { tokenizer, .. } = source.kind else {
            unreachable!("resumed input")
        };
        Self::stable_utf8(tokenizer, input, 0)
    }
    pub(crate) fn is_stable_input(&self) -> bool {
        matches!(
            self.kind,
            LeaseKind::Input {
                mode: InputMode::Stable { .. },
                ..
            }
        )
    }
    pub(crate) fn stable_utf8(
        tokenizer: Tokenizer,
        input: &'input str,
        decoded_start: usize,
    ) -> Result<Self, TextError> {
        let end = decoded_start.checked_add(input.len());
        let valid = tokenizer.encoding == SourceEncoding::Utf8
            && tokenizer.buffer.is_empty()
            && tokenizer.position == tokenizer.source_position
            && tokenizer.reference_cursor().is_ok()
            && end.is_some_and(|end| {
                tokenizer.position >= decoded_start && tokenizer.position <= end
            })
            && input.is_char_boundary(tokenizer.position.saturating_sub(decoded_start));
        if !valid {
            return Err(TextError {
                kind: TextErrorKind::InvalidState,
                source: TextSourceId::Input,
                offset: tokenizer.source_position,
            });
        }
        let decoded = tokenizer.position;
        let physical = tokenizer.source_position;
        Ok(Self {
            kind: LeaseKind::Input {
                tokenizer,
                mode: InputMode::Stable {
                    input,
                    decoded_start,
                },
                decoded,
                physical,
            },
            finalizing: false,
            boundary: false,
        })
    }
    pub(crate) fn local(
        tokenizer: LocalTokenizer,
        invocation: EntityInvocationId,
    ) -> Result<Self, TextError> {
        if !tokenizer.is_fresh_source() {
            return Err(TextError {
                kind: TextErrorKind::InvalidState,
                source: TextSourceId::Replacement(invocation),
                offset: tokenizer.position(),
            });
        }
        let native_work = tokenizer.work_ceiling();
        Ok(Self {
            kind: LeaseKind::Local {
                tokenizer,
                invocation,
                native_work,
            },
            finalizing: false,
            boundary: false,
        })
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
    pub(crate) fn input_encoding(&self) -> Option<SourceEncoding> {
        match &self.kind {
            LeaseKind::Input { tokenizer, .. } => Some(tokenizer.encoding),
            LeaseKind::Local { .. } => None,
        }
    }
    fn stable_remaining(&self) -> Option<&'input [u8]> {
        let LeaseKind::Input {
            mode:
                InputMode::Stable {
                    input,
                    decoded_start,
                },
            decoded,
            ..
        } = &self.kind
        else {
            return None;
        };
        input.as_bytes().get(decoded.checked_sub(*decoded_start)?..)
    }
    fn stable_current(&self) -> Option<(TokenKind, &'input str, TokenSpan)> {
        let LeaseKind::Input {
            tokenizer,
            mode:
                InputMode::Stable {
                    input,
                    decoded_start,
                },
            ..
        } = &self.kind
        else {
            return None;
        };
        let (kind, _, span) = tokenizer.current()?;
        if span.encoding != SourceEncoding::Utf8 || span.source != span.decoded {
            return None;
        }
        let start = span.decoded.start.checked_sub(*decoded_start)?;
        let end = span.decoded.end.checked_sub(*decoded_start)?;
        Some((kind, input.get(start..end)?, span.clone()))
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
                ..
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
    ) -> Result<(), TextError> {
        let id = self.id();
        let offset = self.offset();
        if !self.boundary {
            return Err(TextError {
                kind: TextErrorKind::InvalidState,
                source: id,
                offset,
            });
        }
        let LeaseKind::Local {
            tokenizer,
            invocation: current,
            ..
        } = &mut self.kind
        else {
            return Err(TextError {
                kind: TextErrorKind::InvalidState,
                source: id,
                offset,
            });
        };
        tokenizer
            .switch_source_boundary(start)
            .map_err(|e| TextError {
                kind: TextErrorKind::Local(e.kind),
                source: id,
                offset: e.offset,
            })?;
        *current = invocation;
        self.finalizing = false;
        self.boundary = false;
        Ok(())
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
pub(crate) struct TextHandoff<'input> {
    shared: TextShared,
    source: TextSourceLease<'input>,
}
impl<'input> TextHandoff<'input> {
    pub(crate) fn into_parts(self) -> (TextShared, TextSourceLease<'input>) {
        (self.shared, self.source)
    }
}
impl<'input> TextCoordinator<'input> {
    #[cfg(test)]
    pub(crate) fn panic_on_next_release_for_test() {
        PANIC_ON_NEXT_RELEASE.with(|armed| armed.set(true));
    }

    pub(crate) fn new(
        shared: TextShared,
        mut source: TextSourceLease<'input>,
    ) -> Result<Self, TextError> {
        shared.check_source(&source)?;
        source.boundary = false;
        Ok(Self {
            shared,
            source,
            pending: Pending::Scanning,
        })
    }
    pub(crate) fn usage(&self) -> TextUsage {
        self.shared.usage
    }
    pub(crate) fn remaining_work(&self) -> Result<usize, TextError> {
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
    pub(crate) fn input_position(&self) -> Option<(usize, usize)> {
        self.source.input_position()
    }
    pub(crate) fn current(&self) -> Option<TextToken<'_>> {
        if self.shared.failure.is_some()
            || !matches!(self.pending, Pending::TokenReady | Pending::PrefixReady(_))
        {
            return None;
        }
        self.source.current()
    }
    pub(crate) fn is_stable_input(&self) -> bool {
        self.source.is_stable_input()
    }
    pub(crate) fn stable_remaining(&self) -> Option<&'input [u8]> {
        if self.shared.failure.is_some() {
            return None;
        }
        self.source.stable_remaining()
    }
    pub(crate) fn stable_current(&self) -> Option<(TokenKind, &'input str, TokenSpan)> {
        self.current()?;
        self.source.stable_current()
    }
    pub(crate) fn current_with_work(&mut self) -> Option<(TextToken<'_>, &mut TextShared)> {
        if self.shared.failure.is_some()
            || !matches!(self.pending, Pending::TokenReady | Pending::PrefixReady(_))
        {
            return None;
        }
        let token = self.source.current()?;
        Some((token, &mut self.shared))
    }
    pub(crate) fn general(
        &self,
    ) -> Option<(ValidatedReference<'_>, &ReferenceExtent, TextSourceId)> {
        if self.shared.failure.is_some() {
            return None;
        }
        let Pending::GeneralReady(extent) = &self.pending else {
            return None;
        };
        Some((
            self.shared.reference.as_ref()?.validated()?,
            extent,
            self.source.id(),
        ))
    }
    pub(crate) fn advance(&mut self, input: &[u8]) -> Result<TextStep, TextError> {
        self.remember(|s| {
            if !matches!(
                s.source.kind,
                LeaseKind::Input {
                    mode: InputMode::Streaming,
                    ..
                }
            ) {
                return Err(s.error(TextErrorKind::InvalidState));
            }
            s.drive(input)
        })
    }
    pub(crate) fn advance_local(
        &mut self,
        invocation: EntityInvocationId,
        input: &[u8],
    ) -> Result<TextStep, TextError> {
        self.remember(|s| {
            if s.source.id() != TextSourceId::Replacement(invocation) {
                return Err(s.error(TextErrorKind::SourceMismatch));
            }
            s.drive(input)
        })
    }
    pub(crate) fn advance_borrowed(&mut self) -> Result<TextStep, TextError> {
        self.advance_borrowed_bounded(usize::MAX)
    }
    pub(crate) fn advance_borrowed_bounded(
        &mut self,
        maximum: usize,
    ) -> Result<TextStep, TextError> {
        self.remember(|s| {
            let LeaseKind::Input {
                mode:
                    InputMode::Stable {
                        input,
                        decoded_start,
                    },
                decoded,
                ..
            } = &s.source.kind
            else {
                return Err(s.error(TextErrorKind::InvalidState));
            };
            let (input, start, position) = (*input, *decoded_start, *decoded);
            let relative = position
                .checked_sub(start)
                .filter(|at| *at <= input.len())
                .ok_or_else(|| s.error(TextErrorKind::InvalidState))?;
            let available = &input.as_bytes()[relative..];
            s.drive(&available[..available.len().min(maximum)])
        })
    }
    pub(crate) fn finish_step(&mut self) -> Result<TextStep, TextError> {
        self.remember(|s| {
            if let LeaseKind::Input {
                mode:
                    InputMode::Stable {
                        input,
                        decoded_start,
                    },
                decoded,
                ..
            } = &s.source.kind
                && decoded.checked_sub(*decoded_start) != Some(input.len())
            {
                return Err(s.error(TextErrorKind::InvalidState));
            }
            s.source.finalizing = true;
            s.drive(&[])
        })
    }
    pub(crate) fn release_token(&mut self) -> Result<(), TextError> {
        self.remember(|s| {
            #[cfg(test)]
            PANIC_ON_NEXT_RELEASE.with(|armed| {
                assert!(!armed.replace(false), "injected token-release panic");
            });
            let pending = s.pending.clone();
            match pending {
                Pending::TokenReady => {
                    s.source.release();
                    s.pending = Pending::Scanning;
                }
                Pending::PrefixReady(extent) => {
                    s.source.release();
                    s.skip_general(&extent)?;
                    s.pending = Pending::GeneralReady(extent);
                }
                _ => return Err(s.error(TextErrorKind::InvalidState)),
            }
            Ok(())
        })
    }
    /// The caller acknowledges successful inclusion scheduling; no lookup or
    /// frame mutation is performed here. Prefix acceptance already happened.
    pub(crate) fn acknowledge_general(mut self) -> Result<TextHandoff<'input>, TextError> {
        if let Some(error) = self.shared.failure {
            return Err(error);
        }
        if !matches!(self.pending, Pending::GeneralReady(_)) {
            return Err(self.error(TextErrorKind::InvalidState));
        }
        if let Some(reference) = &mut self.shared.reference {
            reference.release();
        }
        self.source.boundary = true;
        Ok(TextHandoff {
            shared: self.shared,
            source: self.source,
        })
    }
    pub(crate) fn take_complete(mut self) -> Result<TextHandoff<'input>, TextError> {
        if let Some(error) = self.shared.failure {
            return Err(error);
        }
        if !matches!(self.pending, Pending::Complete) {
            return Err(self.error(TextErrorKind::InvalidState));
        }
        self.source.boundary = true;
        Ok(TextHandoff {
            shared: self.shared,
            source: self.source,
        })
    }
    fn drive(&mut self, input: &[u8]) -> Result<TextStep, TextError> {
        if self.source.finalizing && !input.is_empty() {
            return Err(self.error(TextErrorKind::InvalidState));
        }
        let mut consumed = 0;
        loop {
            let outcome = match self.pending.clone() {
                Pending::TokenReady | Pending::PrefixReady(_) => Some(TextOutcome::TokenReady),
                Pending::GeneralReady(_) => Some(TextOutcome::GeneralReferenceReady),
                Pending::Complete => Some(TextOutcome::Complete),
                _ => None,
            };
            if let Some(outcome) = outcome {
                return Ok(TextStep { consumed, outcome });
            }
            match self.pending.clone() {
                Pending::Scanning => {
                    let step = self.scan(&input[consumed..])?;
                    consumed += step.consumed;
                    match step.stop {
                        ContentStop::TokenReady => {
                            self.pending = Pending::TokenReady;
                        }
                        ContentStop::ReferenceStart => {
                            let cursor = self.cursor()?;
                            if let Some(reference) = &mut self.shared.reference {
                                reference.restart(cursor.decoded_position()).map_err(|e| {
                                    TextError {
                                        kind: TextErrorKind::Reference(e.kind),
                                        source: self.source.id(),
                                        offset: e.offset,
                                    }
                                })?;
                            } else {
                                self.shared.reference = Some(ReferenceScratch::new(
                                    cursor.decoded_position(),
                                    self.shared.limits.reference,
                                ));
                            }
                            self.pending = Pending::Collecting(cursor);
                        }
                        ContentStop::Exhausted => {
                            if self.source.finalizing {
                                self.finish_source()?;
                            } else {
                                return Ok(TextStep {
                                    consumed,
                                    outcome: TextOutcome::NeedInput,
                                });
                            }
                        }
                    }
                }
                Pending::Collecting(cursor) => {
                    if consumed == input.len() {
                        if self.source.finalizing {
                            let source = self.source.id();
                            let reference = self.shared.reference.as_mut().ok_or(TextError {
                                kind: TextErrorKind::InvalidState,
                                source,
                                offset: self.source.offset(),
                            })?;
                            let global_cap = reference.usage().work.saturating_add(
                                self.shared.limits.max_work - self.shared.usage.work,
                            );
                            reference.cap_work(global_cap);
                            let before = reference.usage();
                            let result = reference.finish_step();
                            self.sync_reference(before)?;
                            result.map_err(|e| self.reference_error(e, cursor, global_cap))?;
                            return Err(self.error(TextErrorKind::InvalidState));
                        }
                        return Ok(TextStep {
                            consumed,
                            outcome: TextOutcome::NeedInput,
                        });
                    }
                    let byte = &input[consumed..consumed + 1];
                    let next = cursor
                        .checked_advance(byte)
                        .map_err(|e| self.cursor_error(e))?;
                    let source = self.source.id();
                    let reference = self.shared.reference.as_mut().ok_or(TextError {
                        kind: TextErrorKind::InvalidState,
                        source,
                        offset: self.source.offset(),
                    })?;
                    let global_cap = reference
                        .usage()
                        .work
                        .saturating_add(self.shared.limits.max_work - self.shared.usage.work);
                    reference.cap_work(global_cap);
                    let before = reference.usage();
                    let before_position = reference.position();
                    let result = reference.advance(byte);
                    let accepted = reference.position() - before_position;
                    if accepted != 0 {
                        consumed += accepted;
                        self.accept_reference_cursor(next);
                        self.pending = Pending::Collecting(next);
                    }
                    self.sync_reference(before)?;
                    result.map_err(|e| self.reference_error(e, next, global_cap))?;
                    let Some(view) = self
                        .shared
                        .reference
                        .as_ref()
                        .and_then(ReferenceScratch::validated)
                    else {
                        continue;
                    };
                    let extent = next.finish(view).map_err(|e| self.cursor_error(e))?;
                    if view.kind() == ReferenceKind::General {
                        if self.flush_prefix()? {
                            self.pending = Pending::PrefixReady(extent);
                        } else {
                            self.skip_general(&extent)?;
                            self.pending = Pending::GeneralReady(extent);
                        }
                    } else {
                        self.append_character(&extent)?;
                        if let Some(reference) = &mut self.shared.reference {
                            reference.release();
                        }
                        self.pending = Pending::Scanning;
                    }
                }
                _ => return Err(self.error(TextErrorKind::InvalidState)),
            }
        }
    }
    fn scan(&mut self, input: &[u8]) -> Result<ContentStep, TextError> {
        let source = self.source.id();
        let remaining = self.shared.limits.max_work - self.shared.usage.work;
        match &mut self.source.kind {
            LeaseKind::Input {
                tokenizer,
                decoded,
                physical,
                ..
            } => {
                let before = tokenizer.position;
                let length = input.len().min(remaining);
                let safe = checked_input_prefix(tokenizer, &input[..length]);
                let position_blocked = safe < length;
                let result = tokenizer.advance_content(&input[..safe]);
                let scanned = tokenizer.position - before;
                self.shared.usage.source_scanned += scanned;
                charge(&mut self.shared, source, *physical, scanned)?;
                *decoded = tokenizer.position;
                *physical = tokenizer.source_position;
                let step = result.map_err(|e| TextError {
                    kind: TextErrorKind::Xml(e.kind),
                    source,
                    offset: e.offset,
                })?;
                if position_blocked && step.stop == ContentStop::Exhausted {
                    return Err(TextError {
                        kind: TextErrorKind::PositionOverflow,
                        source,
                        offset: *physical,
                    });
                }
                if step.stop == ContentStop::TokenReady {
                    validate_input(&mut self.shared, tokenizer)?;
                }
                if length < input.len() && step.stop == ContentStop::Exhausted {
                    return Err(TextError {
                        kind: TextErrorKind::Work,
                        source,
                        offset: *physical,
                    });
                }
                Ok(step)
            }
            LeaseKind::Local {
                tokenizer,
                native_work,
                ..
            } => {
                let before = tokenizer.usage();
                let cap = before.work.saturating_add(remaining);
                tokenizer.cap_work(cap);
                let result = tokenizer.advance_content(input);
                let after = tokenizer.usage();
                sync_local(
                    &mut self.shared,
                    source,
                    tokenizer.position(),
                    before,
                    after,
                )?;
                result.map_err(|e| local_error(source, e, cap, *native_work))
            }
        }
    }
    fn cursor(&mut self) -> Result<ReferenceCursor, TextError> {
        let source = self.source.id();
        match &mut self.source.kind {
            LeaseKind::Input { tokenizer, .. } => {
                tokenizer.reference_cursor().map_err(|e| TextError {
                    kind: TextErrorKind::Xml(e.kind),
                    source,
                    offset: e.offset,
                })
            }
            LeaseKind::Local { tokenizer, .. } => {
                tokenizer.reference_cursor().map_err(|e| TextError {
                    kind: TextErrorKind::Local(e.kind),
                    source,
                    offset: e.offset,
                })
            }
        }
    }
    fn flush_prefix(&mut self) -> Result<bool, TextError> {
        let source = self.source.id();
        let remaining = self.shared.limits.max_work - self.shared.usage.work;
        match &mut self.source.kind {
            LeaseKind::Input { tokenizer, .. } => {
                let ready = tokenizer
                    .flush_text_before_reference()
                    .map_err(|e| TextError {
                        kind: TextErrorKind::Xml(e.kind),
                        source,
                        offset: e.offset,
                    })?;
                if ready {
                    validate_input(&mut self.shared, tokenizer)?;
                }
                Ok(ready)
            }
            LeaseKind::Local {
                tokenizer,
                native_work,
                ..
            } => {
                let before = tokenizer.usage();
                let cap = before.work.saturating_add(remaining);
                tokenizer.cap_work(cap);
                let result = tokenizer.flush_text_before_reference();
                let after = tokenizer.usage();
                sync_local(
                    &mut self.shared,
                    source,
                    tokenizer.position(),
                    before,
                    after,
                )?;
                result.map_err(|e| local_error(source, e, cap, *native_work))
            }
        }
    }
    fn append_character(&mut self, extent: &ReferenceExtent) -> Result<(), TextError> {
        let source = self.source.id();
        let offset = self.source.offset();
        let remaining = self.shared.limits.max_work - self.shared.usage.work;
        let reference = self
            .shared
            .reference
            .as_ref()
            .and_then(ReferenceScratch::validated)
            .ok_or(TextError {
                kind: TextErrorKind::InvalidState,
                source,
                offset,
            })?;
        match &mut self.source.kind {
            LeaseKind::Input { tokenizer, .. } => {
                let length = reference.raw().len();
                // Charge before the possible token-buffer reservation.
                let next = self
                    .shared
                    .usage
                    .work
                    .checked_add(length)
                    .filter(|n| *n <= self.shared.limits.max_work)
                    .ok_or(TextError {
                        kind: TextErrorKind::Work,
                        source,
                        offset,
                    })?;
                self.shared.usage.work = next;
                tokenizer
                    .append_character_reference(reference, extent)
                    .map_err(|e| TextError {
                        kind: TextErrorKind::Xml(e.kind),
                        source,
                        offset: e.offset,
                    })?;
                self.shared.usage.appended_reference_copied += length;
                Ok(())
            }
            LeaseKind::Local {
                tokenizer,
                native_work,
                ..
            } => {
                let before = tokenizer.usage();
                let cap = before.work.saturating_add(remaining);
                tokenizer.cap_work(cap);
                let result = tokenizer.append_character_reference(reference, extent);
                let after = tokenizer.usage();
                sync_local(
                    &mut self.shared,
                    source,
                    tokenizer.position(),
                    before,
                    after,
                )?;
                result.map_err(|e| local_error(source, e, cap, *native_work))
            }
        }
    }
    fn skip_general(&mut self, extent: &ReferenceExtent) -> Result<(), TextError> {
        let source = self.source.id();
        let offset = self.source.offset();
        let reference = self
            .shared
            .reference
            .as_ref()
            .and_then(ReferenceScratch::validated)
            .ok_or(TextError {
                kind: TextErrorKind::InvalidState,
                source,
                offset,
            })?;
        match &mut self.source.kind {
            LeaseKind::Input { tokenizer, .. } => tokenizer
                .skip_general_reference(reference, extent)
                .map_err(|e| TextError {
                    kind: TextErrorKind::Xml(e.kind),
                    source,
                    offset: e.offset,
                }),
            LeaseKind::Local { tokenizer, .. } => tokenizer
                .skip_general_reference(reference, extent)
                .map_err(|e| TextError {
                    kind: TextErrorKind::Local(e.kind),
                    source,
                    offset: e.offset,
                }),
        }
    }
    fn finish_source(&mut self) -> Result<(), TextError> {
        let source = self.source.id();
        let remaining = self.shared.limits.max_work - self.shared.usage.work;
        let ready = match &mut self.source.kind {
            LeaseKind::Input { tokenizer, .. } => {
                tokenizer.finish_step().map_err(|e| TextError {
                    kind: TextErrorKind::Xml(e.kind),
                    source,
                    offset: e.offset,
                })?;
                let ready = tokenizer.current().is_some();
                if ready {
                    validate_input(&mut self.shared, tokenizer)?;
                }
                ready
            }
            LeaseKind::Local {
                tokenizer,
                native_work,
                ..
            } => {
                let before = tokenizer.usage();
                let cap = before.work.saturating_add(remaining);
                tokenizer.cap_work(cap);
                let result = tokenizer.finish_step();
                let after = tokenizer.usage();
                sync_local(
                    &mut self.shared,
                    source,
                    tokenizer.position(),
                    before,
                    after,
                )?;
                result.map_err(|e| local_error(source, e, cap, *native_work))?;
                tokenizer.current().is_some()
            }
        };
        self.pending = if ready {
            Pending::TokenReady
        } else {
            Pending::Complete
        };
        Ok(())
    }
    fn accept_reference_cursor(&mut self, cursor: ReferenceCursor) {
        if let LeaseKind::Input {
            decoded, physical, ..
        } = &mut self.source.kind
        {
            *decoded = cursor.decoded_position();
            if let Some(end) = cursor.input_position() {
                *physical = end;
            }
        }
    }
    fn sync_reference(
        &mut self,
        before: crate::dtd_reference::ReferenceUsage,
    ) -> Result<(), TextError> {
        let after = self
            .shared
            .reference
            .as_ref()
            .ok_or_else(|| self.error(TextErrorKind::InvalidState))?
            .usage();
        let source = self.source.id();
        let offset = self.source.offset();
        charge(&mut self.shared, source, offset, after.work - before.work)?;
        self.shared.usage.reference_scanned += after.scanned - before.scanned;
        self.shared.usage.reference_copied += after.copied - before.copied;
        self.shared.usage.reference_classified += after.classified - before.classified;
        Ok(())
    }
    fn reference_error(
        &self,
        error: ReferenceError,
        cursor: ReferenceCursor,
        global_cap: usize,
    ) -> TextError {
        let kind = if error.kind == ReferenceErrorKind::Limit(ReferenceLimit::Work)
            && global_cap <= self.shared.limits.reference.max_work
        {
            TextErrorKind::Work
        } else {
            TextErrorKind::Reference(error.kind)
        };
        let raw = self
            .shared
            .reference
            .as_ref()
            .map(ReferenceScratch::collected_bytes)
            .unwrap_or(&[]);
        TextError {
            kind,
            source: self.source.id(),
            offset: cursor.map_offset(raw, error.offset),
        }
    }
    fn cursor_error(&self, error: ReferenceCursorError) -> TextError {
        TextError {
            kind: TextErrorKind::Cursor(error.kind),
            source: self.source.id(),
            offset: error.offset,
        }
    }
    fn error(&self, kind: TextErrorKind) -> TextError {
        TextError {
            kind,
            source: self.source.id(),
            offset: self.source.offset(),
        }
    }
    fn remember<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, TextError>,
    ) -> Result<T, TextError> {
        if let Some(error) = self.shared.failure {
            return Err(error);
        }
        let result = operation(self);
        if let Err(error) = result {
            self.shared.failure = Some(error);
        }
        result
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
fn charge(
    shared: &mut TextShared,
    source: TextSourceId,
    offset: usize,
    amount: usize,
) -> Result<(), TextError> {
    shared.usage.work = shared
        .usage
        .work
        .checked_add(amount)
        .filter(|n| *n <= shared.limits.max_work)
        .ok_or(TextError {
            kind: TextErrorKind::Work,
            source,
            offset,
        })?;
    Ok(())
}
fn sync_local(
    shared: &mut TextShared,
    source: TextSourceId,
    offset: usize,
    before: super::local::LocalUsage,
    after: super::local::LocalUsage,
) -> Result<(), TextError> {
    charge(shared, source, offset, after.work - before.work)?;
    shared.usage.source_scanned += after.scanned - before.scanned;
    shared.usage.validated += after.validated - before.validated;
    shared.usage.appended_reference_copied += after.reference_copied - before.reference_copied;
    Ok(())
}
fn local_error(
    source: TextSourceId,
    error: LocalError,
    global_cap: usize,
    native: usize,
) -> TextError {
    TextError {
        kind: if error.kind == LocalErrorKind::Work && global_cap <= native {
            TextErrorKind::Work
        } else {
            TextErrorKind::Local(error.kind)
        },
        source,
        offset: error.offset,
    }
}
fn validate_input(shared: &mut TextShared, tokenizer: &Tokenizer) -> Result<(), TextError> {
    let Some((kind, bytes, span)) = tokenizer.current() else {
        return Err(TextError {
            kind: TextErrorKind::InvalidState,
            source: TextSourceId::Input,
            offset: tokenizer.source_position,
        });
    };
    charge(shared, TextSourceId::Input, span.source.start, bytes.len())?;
    shared.usage.validated += bytes.len();
    if kind == TokenKind::InvalidMarkup {
        return Ok(());
    }
    let raw = std::str::from_utf8(bytes).map_err(|e| TextError {
        kind: TextErrorKind::Xml(ErrorKind::InvalidUtf8),
        source: TextSourceId::Input,
        offset: span.source.start + e.valid_up_to(),
    })?;
    if let Some((offset, _)) = raw.char_indices().find(|(_, c)| !xml_char(*c)) {
        let mapped = span.map_error(
            raw,
            Error::new(ErrorKind::InvalidCharacter, span.decoded.start + offset),
        );
        return Err(TextError {
            kind: TextErrorKind::Xml(mapped.kind),
            source: TextSourceId::Input,
            offset: mapped.offset,
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/internal/dtd_text_coordinator_author.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/internal/dtd_text_coordinator_spec.rs"]
mod spec_tests;

#[cfg(test)]
#[path = "../../tests/internal/dtd_text_work_ledger.rs"]
mod work_ledger_tests;
