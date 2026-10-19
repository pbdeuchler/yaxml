//! Private GE content composition; physical events lend scratch, generated events own.
use super::*;
use crate::dtd_catalog::CatalogLimit;
use crate::dtd_frames::{EntityInvocationId, FrameError, FrameLimit};
use crate::dtd_tag::{DtdTagError, DtdTagErrorKind, DtdTagLocation};
use crate::parser::{DtdLeafKind, ParserState};
use crate::tokenizer::local::{LocalLimits, LocalTokenizer};
use crate::tokenizer::text::{
    TextCoordinator, TextError, TextErrorKind, TextLimits, TextLocation, TextOutcome, TextShared,
    TextSourceId, TextSourceLease, TextUsage,
};
use crate::tokenizer::{TokenKind, Tokenizer};
use crate::{Event, SourceOrigin};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContentErrorKind {
    Text(TextErrorKind),
    Frame(FrameErrorKind),
    Tag(DtdTagErrorKind),
    Parser(ErrorKind),
    Work,
    PhysicalBytes,
    Allocation,
    InvalidState,
    Callback(ErrorKind),
    Poisoned,
}

/// The physical diagnostic anchor is separate from exact source/local context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ContentError {
    pub(crate) kind: ContentErrorKind,
    pub(crate) physical_offset: usize,
    pub(crate) origin: Option<FragmentOrigin>,
    pub(crate) local_offset: Option<usize>,
    pub(crate) value_location: Option<ValueLocation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContentOutcome {
    EventReady,
    NeedPhysicalInput,
    Progress,
    Complete,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ContentStep {
    /// Newly accepted decoded input bytes; replacement-only transitions use zero.
    pub(crate) consumed: usize,
    pub(crate) outcome: ContentOutcome,
}

#[derive(Clone, Copy)]
struct Active {
    id: EntityId,
    invocation: EntityInvocationId,
    // Lexical lookahead can be ahead of the frame's committed token cursor.
    scanned: usize,
    physical_reference: ByteSpan,
}

enum LocalBuffer<'input> {
    Fresh(LocalTokenizer),
    Released(TextSourceLease<'input>),
}

/// A one-shot value outside the controller; its event borrows paused source storage.
pub(crate) struct LentContent<'event> {
    pub(crate) event: Option<Event<'event>>,
    pub(crate) origin: FragmentOrigin,
    pub(crate) budget: &'event mut Budget,
    failure: &'event mut Option<ContentError>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Delivery {
    Scanning,
    Ready,
    Lent,
}

pub(crate) struct ContentControllerCore<'input> {
    body: DtdBodyState,
    parser: ParserState<'input>,
    coordinator: Option<TextCoordinator<'input>>,
    paused_input: Option<TextSourceLease<'input>>,
    local: Option<LocalBuffer<'input>>,
    active: Vec<Active>,
    delivery: Delivery,
    finalizing: bool,
    complete: bool,
    failure: Option<ContentError>,
    text_usage: TextUsage,
}

pub(crate) type ContentController = ContentControllerCore<'static>;
pub(crate) type BorrowedContentController<'input> = ContentControllerCore<'input>;

impl<'input> ContentControllerCore<'input> {
    /// Resume the same decoded source with its original transport encoding.
    pub(crate) fn new(
        body: DtdBodyState,
        parser: ParserState<'input>,
        tokenizer: Tokenizer,
        limits: TextLimits,
        local_limits: LocalLimits,
    ) -> Result<Self, ContentError> {
        let invalid = || ContentError {
            kind: ContentErrorKind::InvalidState,
            physical_offset: body.physical_position,
            origin: None,
            local_offset: None,
            value_location: None,
        };
        let input = TextSourceLease::resume(
            tokenizer,
            body.encoding,
            body.decoded_position,
            body.physical_position,
        )
        .map_err(|error| ContentError {
            kind: ContentErrorKind::Text(error.kind),
            physical_offset: error.offset,
            ..invalid()
        })?;
        Self::with_source(body, parser, input, limits, local_limits)
    }
    pub(crate) fn stable_utf8(
        body: DtdBodyState,
        parser: ParserState<'input>,
        tokenizer: Tokenizer,
        source: &'input str,
        limits: TextLimits,
        local_limits: LocalLimits,
    ) -> Result<Self, ContentError> {
        let invalid = ContentError {
            kind: ContentErrorKind::InvalidState,
            physical_offset: body.physical_position,
            origin: None,
            local_offset: None,
            value_location: None,
        };
        if body.encoding != SourceEncoding::Utf8 {
            return Err(invalid);
        }
        let input = TextSourceLease::resume_stable_utf8(
            tokenizer,
            source,
            body.decoded_position,
            body.physical_position,
        )
        .map_err(|error| ContentError {
            kind: ContentErrorKind::Text(error.kind),
            physical_offset: error.offset,
            ..invalid
        })?;
        Self::with_source(body, parser, input, limits, local_limits)
    }
    fn with_source(
        body: DtdBodyState,
        parser: ParserState<'input>,
        input: TextSourceLease<'input>,
        limits: TextLimits,
        local_limits: LocalLimits,
    ) -> Result<Self, ContentError> {
        let invalid = || ContentError {
            kind: ContentErrorKind::InvalidState,
            physical_offset: body.physical_position,
            origin: None,
            local_offset: None,
            value_location: None,
        };
        if input.input_encoding() != Some(body.encoding)
            || input.input_position() != Some(body.resume_position())
            || body.frames.depth() != 0
        {
            return Err(invalid());
        }
        let shared = TextShared::new(
            TextLimits {
                max_work: limits.max_work.min(body.limits.max_work),
                ..limits
            },
            body.counters.work,
        )
        .map_err(|kind| ContentError {
            kind: ContentErrorKind::Text(kind),
            ..invalid()
        })?;
        let local = LocalTokenizer::new(0, local_limits);
        // Local adoption is also checked when the invocation exists. Reject an
        // impossible configured slot now, before any frame can be published.
        if local.scratch_capacity_requirement() > limits.scratch.replacement_token_bytes {
            return Err(ContentError {
                kind: ContentErrorKind::Text(TextErrorKind::Scratch),
                ..invalid()
            });
        }
        let coordinator = TextCoordinator::new(shared, input).map_err(|error| ContentError {
            kind: ContentErrorKind::Text(error.kind),
            physical_offset: error.offset,
            ..invalid()
        })?;
        let text_usage = coordinator.usage();
        Ok(Self {
            body,
            parser,
            coordinator: Some(coordinator),
            paused_input: None,
            local: Some(LocalBuffer::Fresh(local)),
            active: Vec::new(),
            delivery: Delivery::Scanning,
            finalizing: false,
            complete: false,
            failure: None,
            text_usage,
        })
    }

    #[cfg(test)]
    pub(crate) fn counters(&self) -> ControllerCounters {
        let usage = self.usage();
        ControllerCounters {
            work: usage.work,
            scanned_bytes: self.body.counters.scanned_bytes
                + (usage.source_scanned - self.text_usage.source_scanned)
                + (usage.reference_scanned - self.text_usage.reference_scanned),
            ..self.body.counters
        }
    }
    #[cfg(test)]
    pub(crate) fn frame_counters(&self) -> crate::dtd_frames::FrameCounters {
        self.body.frames.counters()
    }
    #[cfg(test)]
    pub(crate) fn usage(&self) -> TextUsage {
        self.coordinator
            .as_ref()
            .map_or(self.text_usage, TextCoordinator::usage)
    }
    pub(crate) fn position(&self) -> (usize, usize) {
        self.body.resume_position()
    }
    pub(crate) fn owned_budget(&mut self) -> &mut Budget {
        self.parser.owned_budget()
    }
    pub(crate) fn catalog(&self) -> &DtdCatalog {
        &self.body.catalog
    }

    pub(crate) fn advance(&mut self, input: &[u8]) -> Result<ContentStep, ContentError> {
        self.remember(|s| {
            if s.stable_input() {
                return Err(s.simple(ContentErrorKind::InvalidState));
            }
            if s.finalizing && !input.is_empty() {
                return Err(s.simple(ContentErrorKind::InvalidState));
            }
            s.drive(input)
        })
    }

    pub(crate) fn advance_borrowed(&mut self) -> Result<ContentStep, ContentError> {
        self.remember(|s| {
            if !s.stable_input() {
                return Err(s.simple(ContentErrorKind::InvalidState));
            }
            s.drive(&[])
        })
    }
    fn stable_input(&self) -> bool {
        self.paused_input
            .as_ref()
            .is_some_and(TextSourceLease::is_stable_input)
            || self
                .coordinator
                .as_ref()
                .is_some_and(TextCoordinator::is_stable_input)
    }
    pub(crate) fn finish_step(&mut self) -> Result<ContentStep, ContentError> {
        self.remember(|s| {
            if s.delivery == Delivery::Lent {
                return Err(s.simple(ContentErrorKind::InvalidState));
            }
            s.finalizing = true;
            s.drive(&[])
        })
    }

    /// Publish once while retaining the complete source token until acknowledgment.
    #[cfg(test)]
    pub(crate) fn lend(&mut self) -> Result<(Option<Event<'_>>, SourceOrigin), ContentError> {
        let publication = self.lend_with_budget()?;
        Ok((
            publication.event,
            SourceOrigin::from_repr(publication.origin),
        ))
    }

    pub(crate) fn is_non_event(&self) -> bool {
        self.coordinator
            .as_ref()
            .and_then(TextCoordinator::current)
            .is_some_and(|token| self.parser.is_non_event(token.kind, token.raw))
    }

    pub(crate) fn acknowledge(&mut self) -> Result<(), ContentError> {
        self.remember(Self::acknowledge_inner)
    }

    fn acknowledge_inner(&mut self) -> Result<(), ContentError> {
        if self.delivery != Delivery::Lent {
            return Err(self.simple(ContentErrorKind::InvalidState));
        }
        self.coordinator
            .as_mut()
            .expect("lent coordinator")
            .release_token()
            .map_err(|error| self.text_error(error))?;
        self.delivery = Delivery::Scanning;
        Ok(())
    }

    /// Callback and lending consumers share preparation and source release.
    pub(crate) fn emit(
        &mut self,
        emit: &mut impl FnMut(Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<(), ContentError> {
        let poisoned = self.simple(ContentErrorKind::Poisoned);
        let LentContent {
            event,
            origin,
            budget,
            failure,
        } = self.lend_with_budget()?;
        *failure = Some(poisoned);
        let result = match event {
            Some(event) => emit(event, SourceOrigin::from_repr(origin), budget),
            None => Ok(()),
        };
        let error = result.err().map(|error| callback_failure(error, origin));
        *failure = error;
        if let Some(error) = error {
            return Err(error);
        }
        self.acknowledge()
    }

    fn drive(&mut self, input: &[u8]) -> Result<ContentStep, ContentError> {
        if self.delivery == Delivery::Lent {
            return Err(self.simple(ContentErrorKind::InvalidState));
        }
        if self.delivery == Delivery::Ready {
            return Ok(ContentStep {
                consumed: 0,
                outcome: ContentOutcome::EventReady,
            });
        }
        if self.complete {
            return Ok(ContentStep {
                consumed: 0,
                outcome: ContentOutcome::Complete,
            });
        }
        let stable = self.stable_input();
        let mut consumed = 0;
        loop {
            if self
                .coordinator
                .as_ref()
                .expect("active coordinator")
                .general()
                .is_some()
            {
                self.enter_general()?;
                return Ok(ContentStep {
                    consumed,
                    outcome: ContentOutcome::Progress,
                });
            }
            let result = if let Some(active) = self.active.last().copied() {
                let replacement = self
                    .body
                    .catalog
                    .replacement(active.id)
                    .expect("active declaration");
                let current = self
                    .body
                    .frames
                    .current(
                        EntityDeclarationId::from_index(active.id.index()),
                        replacement,
                    )
                    .map_err(|error| self.frame_error(error, None))?;
                if current.invocation != active.invocation
                    || active.scanned > replacement.len()
                    || !replacement.is_char_boundary(active.scanned)
                {
                    return Err(self.simple(ContentErrorKind::InvalidState));
                }
                let coordinator = self.coordinator.as_mut().expect("active coordinator");
                if active.scanned == replacement.len() {
                    coordinator.finish_step()
                } else {
                    coordinator
                        .advance_local(active.invocation, &replacement.as_bytes()[active.scanned..])
                }
            } else {
                let remaining = self
                    .body
                    .limits
                    .max_physical_bytes
                    .saturating_sub(self.body.counters.physical_bytes);
                let available = if stable {
                    self.coordinator
                        .as_ref()
                        .expect("input coordinator")
                        .stable_remaining()
                        .ok_or_else(|| self.simple(ContentErrorKind::InvalidState))?
                } else {
                    &input[consumed..]
                };
                if stable && available.is_empty() {
                    self.finalizing = true;
                }
                let length = physical_input_prefix(self.body.encoding, available, remaining);
                if !self.finalizing && !available.is_empty() && length == 0 {
                    return Err(self.simple(ContentErrorKind::PhysicalBytes));
                }
                let coordinator = self.coordinator.as_mut().expect("active coordinator");
                if self.finalizing {
                    coordinator.finish_step()
                } else if stable {
                    coordinator.advance_borrowed_bounded(length)
                } else {
                    coordinator.advance(&available[..length])
                }
            };
            // Include accepted work even when scanning fails; never replay it.
            self.sync_usage();
            if self.active.is_empty() {
                let (decoded, physical) = self
                    .coordinator
                    .as_ref()
                    .expect("input coordinator")
                    .input_position()
                    .expect("input lease");
                self.body.counters.physical_bytes += physical - self.body.physical_position;
                self.body.decoded_position = decoded;
                self.body.physical_position = physical;
            }
            let step = result.map_err(|error| self.text_error(error))?;
            if let Some(active) = self.active.last_mut() {
                active.scanned = active
                    .scanned
                    .checked_add(step.consumed)
                    .ok_or(ContentError {
                        kind: ContentErrorKind::InvalidState,
                        physical_offset: active.physical_reference.start,
                        origin: None,
                        local_offset: Some(active.scanned),
                        value_location: None,
                    })?;
            } else {
                consumed += step.consumed;
            }
            match step.outcome {
                TextOutcome::TokenReady => {
                    self.delivery = Delivery::Ready;
                    return Ok(ContentStep {
                        consumed,
                        outcome: ContentOutcome::EventReady,
                    });
                }
                TextOutcome::GeneralReferenceReady => {
                    self.enter_general()?;
                    return Ok(ContentStep {
                        consumed,
                        outcome: ContentOutcome::Progress,
                    });
                }
                TextOutcome::NeedInput if !self.active.is_empty() => continue,
                TextOutcome::NeedInput if stable && !self.finalizing => continue,
                TextOutcome::NeedInput if consumed < input.len() && !self.finalizing => {
                    // A transport quota can split one decoder scalar's UTF-8
                    // representation. Drain its zero-width continuations before
                    // reporting that the supplied decoded window was exhausted.
                    if step.consumed == 0 {
                        return Err(self.simple(ContentErrorKind::InvalidState));
                    }
                }
                TextOutcome::NeedInput => {
                    return Ok(ContentStep {
                        consumed,
                        outcome: ContentOutcome::NeedPhysicalInput,
                    });
                }
                TextOutcome::Complete if !self.active.is_empty() => {
                    self.finish_general()?;
                    return Ok(ContentStep {
                        consumed,
                        outcome: ContentOutcome::Progress,
                    });
                }
                TextOutcome::Complete => {
                    self.parser
                        .validate_finish(self.body.physical_position)
                        .map_err(|error| ContentError {
                            kind: ContentErrorKind::Parser(error.kind),
                            physical_offset: error.offset,
                            origin: None,
                            local_offset: None,
                            value_location: None,
                        })?;
                    self.complete = true;
                    return Ok(ContentStep {
                        consumed,
                        outcome: ContentOutcome::Complete,
                    });
                }
            }
        }
    }

    fn enter_general(&mut self) -> Result<(), ContentError> {
        self.share_expansion();
        let (resolved, origin) = {
            let (reference, extent, source) = self
                .coordinator
                .as_ref()
                .expect("coordinator")
                .general()
                .expect("reference ready");
            let origin = match source {
                TextSourceId::Input => {
                    FragmentOrigin::input(extent.input_span().expect("input extent").source.clone())
                        .expect("real reference")
                }
                TextSourceId::Replacement(invocation) => {
                    let active = self.active.last().expect("active source");
                    if active.invocation != invocation {
                        return Err(self.simple(ContentErrorKind::InvalidState));
                    }
                    FragmentOrigin::Replacement {
                        declaration: EntityDeclarationId::from_index(active.id.index()),
                        invocation,
                        local: ByteSpan::new(extent.local_range().expect("local extent"))
                            .expect("reference range"),
                        physical_reference: active.physical_reference,
                    }
                }
            };
            if !self.parser.in_content() {
                return Err(self.for_origin(
                    ContentErrorKind::Parser(ErrorKind::TextOutsideRoot),
                    origin,
                    None,
                ));
            }
            let name = &reference.raw()[1..reference.raw().len() - 1];
            if let Err(error) = self.body.catalog.load_general(name) {
                let kind = ContentErrorKind::Tag(DtdTagErrorKind::Catalog(error.kind));
                return Err(self.for_origin(kind, origin, None));
            }
            let failure = |failure| ContentErrorKind::Frame(FrameErrorKind::Lookup(failure));
            let resolved = match self.body.catalog.lookup_general(name) {
                EntityLookup::Internal { id, replacement } => Some((
                    id,
                    EntityResolution::Internal {
                        kind: EntityKind::General,
                        declaration: EntityDeclarationId::from_index(id.index()),
                        replacement_len: replacement.len(),
                    },
                )),
                // libxml2 drops a reference that unread markup could declare,
                // and one to an external entity that it cannot load.
                EntityLookup::UnreadEntity | EntityLookup::ExternalUnavailable { .. } => None,
                EntityLookup::Unparsed { .. } => {
                    return Err(self.for_origin(failure(LookupFailure::Unparsed), origin, None));
                }
                EntityLookup::Undeclared => {
                    return Err(self.for_origin(failure(LookupFailure::Undeclared), origin, None));
                }
                EntityLookup::Predefined(_) => {
                    return Err(self.simple(ContentErrorKind::InvalidState));
                }
            };
            (resolved, origin)
        };
        let Some((id, resolution)) = resolved else {
            // The skipped reference is consumed from its replacement frame too.
            if let (Some(active), Some(local)) = (self.active.last(), origin.replacement_range()) {
                let replacement = self
                    .body
                    .catalog
                    .replacement(active.id)
                    .expect("active replacement");
                self.body
                    .frames
                    .advance_current(
                        EntityDeclarationId::from_index(active.id.index()),
                        replacement,
                        local.len(),
                    )
                    .map_err(|error| self.frame_error(error, Some(origin)))?;
            }
            let result = self
                .coordinator
                .as_mut()
                .expect("reference coordinator")
                .discard_general();
            return result.map_err(|error| self.text_error(error));
        };
        if self.active.len() < self.body.limits.frames.max_active_depth {
            self.active
                .try_reserve(1)
                .map_err(|_| self.for_origin(ContentErrorKind::Allocation, origin, None))?;
        }
        let before = self.body.frames.counters().work;
        let ceiling = before.saturating_add(self.remaining()?);
        self.body.frames.cap_work(ceiling);
        let boundary = FrameBoundary::Content {
            element_depth: self.parser.element_depth(),
        };
        let entered = if let Some(parent) = self.active.last() {
            self.body.frames.enter_nested(
                EntityKind::General,
                resolution,
                EntityDeclarationId::from_index(parent.id.index()),
                self.body
                    .catalog
                    .replacement(parent.id)
                    .expect("parent replacement"),
                origin.replacement_range().expect("nested range"),
                boundary,
            )
        } else {
            self.body.frames.enter_physical(
                EntityKind::General,
                resolution,
                origin.physical_context(),
                boundary,
            )
        };
        self.sync_frame_work(before, origin)?;
        let invocation = entered.map_err(|error| {
            if error.kind == FrameErrorKind::LimitExceeded(FrameLimit::Work)
                && ceiling <= self.body.limits.frames.max_work
            {
                self.for_origin(
                    ContentErrorKind::Work,
                    origin,
                    origin.replacement_range().map(|range| range.start),
                )
            } else {
                self.frame_error(error, Some(origin))
            }
        })?;
        let was_physical = self.active.is_empty();
        self.active.push(Active {
            id,
            invocation,
            scanned: 0,
            physical_reference: ByteSpan::new(origin.physical_context()).expect("real reference"),
        });
        let (shared, source) = self
            .coordinator
            .take()
            .expect("reference coordinator")
            .acknowledge_general()
            .map_err(|error| self.text_error(error))?
            .into_parts();
        let local = if was_physical {
            self.paused_input = Some(source);
            match self.local.take().expect("parked local") {
                LocalBuffer::Fresh(tokenizer) => TextSourceLease::local(tokenizer, invocation)
                    .map_err(|error| self.text_error(error))?,
                LocalBuffer::Released(mut lease) => {
                    lease
                        .switch_local_at_boundary(invocation, 0)
                        .map_err(|error| self.text_error(error))?;
                    lease
                }
            }
        } else {
            let mut source = source;
            source
                .switch_local_at_boundary(invocation, 0)
                .map_err(|error| self.text_error(error))?;
            source
        };
        self.coordinator =
            Some(TextCoordinator::new(shared, local).map_err(|error| self.text_error(error))?);
        Ok(())
    }

    fn finish_general(&mut self) -> Result<(), ContentError> {
        let active = *self.active.last().expect("active source");
        let replacement = self
            .body
            .catalog
            .replacement(active.id)
            .expect("active replacement");
        let current = self
            .body
            .frames
            .current(
                EntityDeclarationId::from_index(active.id.index()),
                replacement,
            )
            .map_err(|error| self.frame_error(error, None))?;
        if current.invocation != active.invocation {
            return Err(self.simple(ContentErrorKind::InvalidState));
        }
        self.body
            .frames
            .finish_current(FrameBoundary::Content {
                element_depth: self.parser.element_depth(),
            })
            .map_err(|error| self.frame_error(error, None))?;
        let (shared, mut source) = self
            .coordinator
            .take()
            .expect("completed local")
            .take_complete()
            .map_err(|error| self.text_error(error))?
            .into_parts();
        self.active.pop();
        let next = if let Some(parent) = self.active.last_mut() {
            let replacement = self
                .body
                .catalog
                .replacement(parent.id)
                .expect("parent replacement");
            let current = self
                .body
                .frames
                .current(
                    EntityDeclarationId::from_index(parent.id.index()),
                    replacement,
                )
                .map_err(|error| ContentError {
                    kind: ContentErrorKind::Frame(error.kind),
                    physical_offset: error.physical_offset,
                    origin: None,
                    local_offset: None,
                    value_location: None,
                })?;
            if current.invocation != parent.invocation {
                return Err(self.simple(ContentErrorKind::InvalidState));
            }
            parent.scanned = current.local_start;
            source
                .switch_local_at_boundary(parent.invocation, current.local_start)
                .map_err(|error| self.text_error(error))?;
            source
        } else {
            self.local = Some(LocalBuffer::Released(source));
            self.paused_input.take().expect("paused primary input")
        };
        self.coordinator =
            Some(TextCoordinator::new(shared, next).map_err(|error| self.text_error(error))?);
        Ok(())
    }

    fn share_expansion(&mut self) {
        let body = &mut self.body;
        share_expansion(
            &body.limits,
            body.physical_position,
            &mut body.frames,
            &mut body.catalog,
        );
    }

    fn prepare_publication(
        &mut self,
    ) -> Result<(TokenKind, FragmentOrigin, usize, usize, usize), ContentError> {
        if self.delivery != Delivery::Ready {
            return Err(self.simple(ContentErrorKind::InvalidState));
        }
        self.share_expansion();
        let (kind, length, origin, offset) = {
            let token = self
                .coordinator
                .as_ref()
                .expect("ready coordinator")
                .current()
                .expect("ready token");
            let (origin, offset) = match token.location {
                TextLocation::Input(span) => (
                    FragmentOrigin::input(span.source.clone()).expect("token range"),
                    span.decoded.start,
                ),
                TextLocation::Local { invocation, range } => {
                    let active = self.active.last().expect("active replacement");
                    if invocation != active.invocation {
                        return Err(self.simple(ContentErrorKind::InvalidState));
                    }
                    (
                        FragmentOrigin::Replacement {
                            declaration: EntityDeclarationId::from_index(active.id.index()),
                            invocation,
                            local: ByteSpan::new(range.clone()).expect("local token"),
                            physical_reference: active.physical_reference,
                        },
                        range.start,
                    )
                }
            };
            (token.kind, token.raw.len(), origin, offset)
        };
        self.charge(length, origin, offset)?;
        if let FragmentOrigin::Replacement { local, .. } = origin {
            let active = *self.active.last().expect("active replacement");
            let replacement = self
                .body
                .catalog
                .replacement(active.id)
                .expect("active text");
            let current = self
                .body
                .frames
                .current(
                    EntityDeclarationId::from_index(active.id.index()),
                    replacement,
                )
                .map_err(|error| self.frame_error(error, Some(origin)))?;
            if current.invocation != active.invocation || current.local_start != local.start {
                return Err(self.for_origin(
                    ContentErrorKind::InvalidState,
                    origin,
                    Some(local.start),
                ));
            }
            let before = self.body.frames.counters().work;
            let ceiling = before.saturating_add(self.remaining()?);
            self.body.frames.cap_work(ceiling);
            let result = self.body.frames.advance_current(
                EntityDeclarationId::from_index(active.id.index()),
                replacement,
                length,
            );
            self.sync_frame_work(before, origin)?;
            result.map_err(|error| {
                if error.kind == FrameErrorKind::LimitExceeded(FrameLimit::Work)
                    && ceiling <= self.body.limits.frames.max_work
                {
                    self.for_origin(ContentErrorKind::Work, origin, Some(local.start))
                } else {
                    self.frame_error(error, Some(origin))
                }
            })?;
            if kind == TokenKind::End {
                self.body
                    .frames
                    .check_before_end_element(self.parser.element_depth())
                    .map_err(|error| self.frame_error(error, Some(origin)))?;
            }
        }
        let catalog_before = self.body.catalog.usage().work;
        let catalog_ceiling = catalog_before.saturating_add(self.remaining()?);
        self.body.catalog.cap_work(catalog_ceiling);
        Ok((kind, origin, offset, catalog_before, catalog_ceiling))
    }

    pub(crate) fn lend_with_budget(&mut self) -> Result<LentContent<'_>, ContentError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        self.failure = Some(self.simple(ContentErrorKind::Poisoned));
        let (kind, origin, offset, catalog_before, catalog_ceiling) =
            match self.prepare_publication() {
                Ok(prepared) => prepared,
                Err(error) => {
                    self.failure = Some(error);
                    return Err(error);
                }
            };
        self.delivery = Delivery::Lent;
        let (token, work) = self
            .coordinator
            .as_mut()
            .expect("ready coordinator")
            .current_with_work()
            .expect("ready token");
        let raw = if kind == TokenKind::InvalidMarkup {
            ""
        } else {
            match std::str::from_utf8(token.raw) {
                Ok(raw) => raw,
                Err(_) => {
                    let error = local_parser_failure(ErrorKind::InvalidUtf8, offset, origin);
                    self.failure = Some(error);
                    return Err(error);
                }
            }
        };
        let event = publish_raw(
            &mut self.parser,
            &mut self.body.catalog,
            RawPublication {
                kind,
                raw,
                offset,
                location: token.location,
                origin,
                stable_physical: false,
            },
            &mut |value, offset, budget| budget.copy(value, offset),
        );
        let delta = self.body.catalog.usage().work - catalog_before;
        let (result, usage) = account_publication(
            event,
            work,
            delta,
            (origin, offset),
            (catalog_ceiling, self.body.limits.catalog.max_work),
        );
        self.body.counters.work = usage.work;
        self.body.counters.scanned_bytes += (usage.source_scanned - self.text_usage.source_scanned)
            + (usage.reference_scanned - self.text_usage.reference_scanned);
        self.text_usage = usage;
        match result {
            Ok(event) => {
                self.failure = None;
                Ok(LentContent {
                    event,
                    origin,
                    budget: self.parser.owned_budget(),
                    failure: &mut self.failure,
                })
            }
            Err(error) => {
                self.failure = Some(error);
                Err(error)
            }
        }
    }

    /// Move a stable-input event out; no field can borrow reusable controller storage.
    pub(crate) fn take_event(
        &mut self,
    ) -> Result<(Option<Event<'input>>, SourceOrigin), ContentError> {
        self.remember(|s| {
            if !s.stable_input() {
                return Err(s.simple(ContentErrorKind::InvalidState));
            }
            s.failure = Some(s.simple(ContentErrorKind::Poisoned));
            let stable = {
                let coordinator = s.coordinator.as_ref().expect("active coordinator");
                let token = coordinator
                    .current()
                    .ok_or_else(|| s.simple(ContentErrorKind::InvalidState))?;
                if matches!(token.location, TextLocation::Input(_)) {
                    Some(
                        coordinator
                            .stable_current()
                            .ok_or_else(|| s.simple(ContentErrorKind::InvalidState))?,
                    )
                } else {
                    None
                }
            };
            let (kind, origin, offset, catalog_before, catalog_ceiling) =
                s.prepare_publication()?;
            s.delivery = Delivery::Lent;
            let event = if let Some((ready_kind, raw, span)) = stable {
                if ready_kind != kind {
                    return Err(s.simple(ContentErrorKind::InvalidState));
                }
                publish_raw(
                    &mut s.parser,
                    &mut s.body.catalog,
                    RawPublication {
                        kind,
                        raw,
                        offset,
                        location: TextLocation::Input(&span),
                        origin,
                        stable_physical: true,
                    },
                    &mut |value, _, _| Ok(std::borrow::Cow::Borrowed(value)),
                )
            } else {
                let token = s
                    .coordinator
                    .as_ref()
                    .expect("ready coordinator")
                    .current()
                    .expect("ready token");
                let TextLocation::Local { range, .. } = token.location else {
                    return Err(s.simple(ContentErrorKind::InvalidState));
                };
                let raw = std::str::from_utf8(token.raw)
                    .map_err(|_| local_parser_failure(ErrorKind::InvalidUtf8, offset, origin))?;
                publish_generated(&mut s.parser, &mut s.body.catalog, kind, raw, range, origin)
                    .map(Some)
            };
            let delta = s.body.catalog.usage().work - catalog_before;
            let (_, work) = s
                .coordinator
                .as_mut()
                .expect("ready coordinator")
                .current_with_work()
                .expect("ready token");
            let (result, _) = account_publication(
                event,
                work,
                delta,
                (origin, offset),
                (catalog_ceiling, s.body.limits.catalog.max_work),
            );
            s.sync_usage();
            let event = result?;
            s.acknowledge_inner()?;
            s.failure = None;
            Ok((event, SourceOrigin::from_repr(origin)))
        })
    }

    fn remaining(&self) -> Result<usize, ContentError> {
        self.coordinator
            .as_ref()
            .expect("controller owns ledger")
            .remaining_work()
            .map_err(|error| self.text_error(error))
    }
    fn charge(
        &mut self,
        amount: usize,
        origin: FragmentOrigin,
        offset: usize,
    ) -> Result<(), ContentError> {
        let source = match origin {
            FragmentOrigin::Replacement { invocation, .. } => TextSourceId::Replacement(invocation),
            _ => TextSourceId::Input,
        };
        let offset = if matches!(source, TextSourceId::Input) {
            origin.error_offset()
        } else {
            offset
        };
        let result = self
            .coordinator
            .as_mut()
            .expect("controller owns ledger")
            .charge_external_work(amount, source, offset);
        self.sync_usage();
        result.map_err(|error| self.text_error(error))
    }
    fn sync_usage(&mut self) {
        if let Some(coordinator) = &self.coordinator {
            let usage = coordinator.usage();
            self.body.counters.work = usage.work;
            self.body.counters.scanned_bytes += (usage.source_scanned
                - self.text_usage.source_scanned)
                + (usage.reference_scanned - self.text_usage.reference_scanned);
            self.text_usage = usage;
        }
    }
    fn sync_frame_work(
        &mut self,
        before: usize,
        origin: FragmentOrigin,
    ) -> Result<(), ContentError> {
        self.body.counters.reference_attempts = self.body.frames.counters().reference_attempts;
        self.charge(
            self.body.frames.counters().work - before,
            origin,
            origin
                .replacement_range()
                .map_or(origin.error_offset(), |range| range.start),
        )
    }
    fn text_error(&self, error: TextError) -> ContentError {
        let kind = if error.kind == TextErrorKind::Work {
            ContentErrorKind::Work
        } else {
            ContentErrorKind::Text(error.kind)
        };
        match error.source {
            TextSourceId::Input => ContentError {
                kind,
                physical_offset: error.offset,
                origin: None,
                local_offset: None,
                value_location: None,
            },
            TextSourceId::Replacement(invocation) => {
                let active = self
                    .active
                    .iter()
                    .rev()
                    .find(|active| active.invocation == invocation);
                ContentError {
                    kind,
                    physical_offset: active.map_or(self.body.physical_position, |active| {
                        active.physical_reference.start
                    }),
                    origin: active.map(|active| FragmentOrigin::Replacement {
                        declaration: EntityDeclarationId::from_index(active.id.index()),
                        invocation,
                        local: ByteSpan {
                            start: error.offset,
                            end: error.offset,
                        },
                        physical_reference: active.physical_reference,
                    }),
                    local_offset: Some(error.offset),
                    value_location: None,
                }
            }
        }
    }
    fn frame_error(&self, error: FrameError, origin: Option<FragmentOrigin>) -> ContentError {
        let origin = origin.or_else(|| {
            self.active
                .last()
                .map(|active| FragmentOrigin::Replacement {
                    declaration: EntityDeclarationId::from_index(active.id.index()),
                    invocation: active.invocation,
                    local: ByteSpan {
                        start: active.scanned,
                        end: active.scanned,
                    },
                    physical_reference: active.physical_reference,
                })
        });
        ContentError {
            kind: ContentErrorKind::Frame(error.kind),
            physical_offset: error.physical_offset,
            origin,
            local_offset: origin
                .and_then(|origin| origin.replacement_range().map(|range| range.start)),
            value_location: None,
        }
    }
    fn for_origin(
        &self,
        kind: ContentErrorKind,
        origin: FragmentOrigin,
        local_offset: Option<usize>,
    ) -> ContentError {
        ContentError {
            kind,
            physical_offset: origin.error_offset(),
            origin: Some(origin),
            local_offset,
            value_location: None,
        }
    }
    fn simple(&self, kind: ContentErrorKind) -> ContentError {
        ContentError {
            kind,
            physical_offset: self.body.physical_position,
            origin: None,
            local_offset: None,
            value_location: None,
        }
    }
    fn remember<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, ContentError>,
    ) -> Result<T, ContentError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = operation(self);
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }
}

struct RawPublication<'raw, 'span> {
    kind: TokenKind,
    raw: &'raw str,
    offset: usize,
    location: TextLocation<'span>,
    origin: FragmentOrigin,
    stable_physical: bool,
}
fn publish_raw<'source, 'raw>(
    parser: &mut ParserState<'source>,
    catalog: &mut DtdCatalog,
    publication: RawPublication<'raw, '_>,
    retain: &mut impl FnMut(
        &'raw str,
        usize,
        &mut Budget,
    ) -> Result<std::borrow::Cow<'source, str>, Error>,
) -> Result<Option<Event<'raw>>, ContentError>
where
    'source: 'raw,
{
    let RawPublication {
        kind,
        raw,
        offset,
        location,
        origin,
        stable_physical,
    } = publication;
    match location {
        TextLocation::Input(span) => {
            if kind == TokenKind::Start {
                let result = if stable_physical {
                    parser.process_stable_dtd_start(raw, offset, catalog, retain)
                } else {
                    parser.process_dtd_start(raw, offset, catalog, LiteralSource::Physical, retain)
                };
                result
                    .map(Some)
                    .map_err(|error| tag_failure(error, origin, Some((span, raw))))
            } else {
                parser
                    .process_token(kind, raw, span.decoded.clone(), retain)
                    .map_err(|error| {
                        let error = span.map_error(raw, error);
                        ContentError {
                            kind: ContentErrorKind::Parser(error.kind),
                            physical_offset: error.offset,
                            origin: Some(origin),
                            local_offset: None,
                            value_location: None,
                        }
                    })
            }
        }
        TextLocation::Local { range, .. } => {
            publish_generated(parser, catalog, kind, raw, range, origin).map(Some)
        }
    }
}

fn publish_generated(
    parser: &mut ParserState<'_>,
    catalog: &mut DtdCatalog,
    kind: TokenKind,
    raw: &str,
    range: std::ops::Range<usize>,
    origin: FragmentOrigin,
) -> Result<Event<'static>, ContentError> {
    let offset = range.start;

    match kind {
        TokenKind::Start => parser
            .process_dtd_start_owned(raw, offset, catalog)
            .map_err(|error| tag_failure(error, origin, None)),
        TokenKind::End => parser
            .process_dtd_end_owned(raw, offset)
            .map_err(|error| local_parser_failure(error.kind, error.local_offset, origin)),
        TokenKind::Text | TokenKind::Cdata | TokenKind::Comment | TokenKind::Instruction => {
            let leaf = match kind {
                TokenKind::Text => DtdLeafKind::Text,
                TokenKind::Cdata => DtdLeafKind::Cdata,
                TokenKind::Comment => DtdLeafKind::Comment,
                _ => DtdLeafKind::ProcessingInstruction,
            };
            parser
                .process_dtd_leaf_owned(leaf, raw, range)
                .map_err(|error| local_parser_failure(error.kind, error.local_offset, origin))
        }
        TokenKind::Dtd | TokenKind::InvalidMarkup => {
            Err(local_parser_failure(ErrorKind::Syntax, offset, origin))
        }
    }
}

fn account_publication<'event>(
    event: Result<Option<Event<'event>>, ContentError>,
    work: &mut TextShared,
    delta: usize,
    (origin, offset): (FragmentOrigin, usize),
    (catalog_ceiling, native_catalog_ceiling): (usize, usize),
) -> (Result<Option<Event<'event>>, ContentError>, TextUsage) {
    let source = match origin {
        FragmentOrigin::Replacement { invocation, .. } => TextSourceId::Replacement(invocation),
        _ => TextSourceId::Input,
    };
    let charged = work.charge_external_work(
        delta,
        source,
        if source == TextSourceId::Input {
            origin.error_offset()
        } else {
            offset
        },
    );
    let mut result = match (event, charged) {
        (Err(error), _) => Err(error),
        (_, Err(error)) => Err(ContentError {
            kind: ContentErrorKind::Text(error.kind),
            physical_offset: origin.error_offset(),
            origin: Some(origin),
            local_offset: origin.replacement_range().map(|_| error.offset),
            value_location: None,
        }),
        (Ok(event), Ok(())) => Ok(event),
    };
    if let Err(error) = &mut result
        && error.kind
            == ContentErrorKind::Tag(DtdTagErrorKind::Catalog(CatalogErrorKind::Limit(
                CatalogLimit::Work,
            )))
        && catalog_ceiling <= native_catalog_ceiling
    {
        error.kind = ContentErrorKind::Work;
    }
    (result, work.usage())
}

pub(crate) fn callback_failure(error: Error, origin: FragmentOrigin) -> ContentError {
    ContentError {
        kind: ContentErrorKind::Callback(error.kind),
        physical_offset: error.offset,
        origin: Some(origin),
        local_offset: if error.kind == ErrorKind::Aborted {
            origin.replacement_range().map(|range| range.end)
        } else {
            None
        },
        value_location: None,
    }
}

fn local_parser_failure(kind: ErrorKind, offset: usize, origin: FragmentOrigin) -> ContentError {
    ContentError {
        kind: ContentErrorKind::Parser(kind),
        physical_offset: origin.error_offset(),
        origin: Some(origin),
        local_offset: Some(offset),
        value_location: None,
    }
}
fn tag_failure(
    error: DtdTagError,
    origin: FragmentOrigin,
    physical: Option<(&TokenSpan, &str)>,
) -> ContentError {
    let (local_offset, value_location) = match error.location {
        DtdTagLocation::Input(offset) => (Some(offset), None),
        DtdTagLocation::Declaration {
            declaration,
            offset,
        } => (
            None,
            Some(ValueLocation::Declaration {
                declaration,
                offset,
            }),
        ),
        DtdTagLocation::Value(location) => (
            match location {
                ValueLocation::Input(offset) => Some(offset),
                ValueLocation::Declaration { .. } | ValueLocation::Replacement { .. } => None,
            },
            Some(location),
        ),
    };
    let physical_offset = if let (Some((span, raw)), Some(offset)) = (physical, local_offset) {
        span.map_error(raw, Error::new(ErrorKind::InvalidState, offset))
            .offset
    } else {
        origin.error_offset()
    };
    ContentError {
        kind: ContentErrorKind::Tag(error.kind),
        physical_offset,
        origin: Some(origin),
        local_offset: if physical.is_none() {
            local_offset
        } else {
            None
        },
        value_location,
    }
}

/// Bound transport charges before scanning decoded bytes. UTF-16 continuations
/// have zero charge: their lead already accounted for the complete scalar.
fn physical_input_prefix(encoding: SourceEncoding, input: &[u8], remaining: usize) -> usize {
    if encoding == SourceEncoding::Utf8 {
        return input.len().min(remaining);
    }
    // Each decoded byte costs at most four transport bytes. Limit the scan near
    // exhaustion to avoid repeatedly traversing a suffix across reference stops.
    if remaining >= 4 {
        return input.len().min(remaining / 4);
    }
    let mut spent = 0;
    for (index, byte) in input.iter().take(4).enumerate() {
        spent += encoding.source_length(std::slice::from_ref(byte));
        if spent > remaining {
            return index;
        }
    }
    input.len().min(4)
}

#[cfg(test)]
#[path = "../../tests/internal/dtd_content_author.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/internal/dtd_content_spec.rs"]
mod spec_tests;
