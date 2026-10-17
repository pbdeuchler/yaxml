//! Private GE content composition; physical events lend scratch, generated events own.
use super::*;
use crate::dtd_catalog::{CatalogLimit, Dependency};
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
    Unavailable(DependencyKind),
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

enum LocalBuffer {
    Fresh(LocalTokenizer),
    Released(TextSourceLease<'static>),
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

pub(crate) struct ContentController {
    body: DtdBodyState,
    parser: ParserState<'static>,
    coordinator: Option<TextCoordinator<'static>>,
    paused_input: Option<TextSourceLease<'static>>,
    local: Option<LocalBuffer>,
    active: Vec<Active>,
    delivery: Delivery,
    finalizing: bool,
    complete: bool,
    failure: Option<ContentError>,
    text_usage: TextUsage,
}

impl ContentController {
    /// Resume the same decoded source with its original transport encoding.
    pub(crate) fn new(
        body: DtdBodyState,
        parser: ParserState<'static>,
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
    pub(crate) fn frame_counters(&self) -> crate::dtd_frames::FrameCounters {
        self.body.frames.counters()
    }
    pub(crate) fn usage(&self) -> TextUsage {
        self.coordinator
            .as_ref()
            .map_or(self.text_usage, TextCoordinator::usage)
    }
    pub(crate) fn position(&self) -> (usize, usize) {
        self.body.resume_position()
    }

    pub(crate) fn advance(&mut self, input: &[u8]) -> Result<ContentStep, ContentError> {
        self.remember(|s| {
            if s.finalizing && !input.is_empty() {
                return Err(s.simple(ContentErrorKind::InvalidState));
            }
            s.drive(input)
        })
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
        self.remember(|s| {
            if s.delivery != Delivery::Lent {
                return Err(s.simple(ContentErrorKind::InvalidState));
            }
            s.coordinator
                .as_mut()
                .expect("lent coordinator")
                .release_token()
                .map_err(|error| s.text_error(error))?;
            s.delivery = Delivery::Scanning;
            Ok(())
        })
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
                let available = &input[consumed..];
                let length = physical_input_prefix(self.body.encoding, available, remaining);
                if !self.finalizing && !available.is_empty() && length == 0 {
                    return Err(self.simple(ContentErrorKind::PhysicalBytes));
                }
                let coordinator = self.coordinator.as_mut().expect("active coordinator");
                if self.finalizing {
                    coordinator.finish_step()
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
        let (id, resolution, dependency, origin) = {
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
            match self.body.catalog.lookup_general(name) {
                EntityLookup::Internal { id, replacement } => (
                    Some(id),
                    EntityResolution::Internal {
                        kind: EntityKind::General,
                        declaration: EntityDeclarationId::from_index(id.index()),
                        replacement_len: replacement.len(),
                    },
                    None,
                    origin,
                ),
                other => {
                    let (failure, dependency) = match other {
                        EntityLookup::ExternalUnavailable { .. } => {
                            (LookupFailure::ExternalUnavailable, None)
                        }
                        EntityLookup::Unparsed { .. } => (LookupFailure::Unparsed, None),
                        EntityLookup::UnreadEntity => (LookupFailure::Unread, None),
                        EntityLookup::Unavailable(dependency) => {
                            (LookupFailure::Unread, Some(dependency))
                        }
                        EntityLookup::Undeclared => (LookupFailure::Undeclared, None),
                        EntityLookup::Predefined(_) | EntityLookup::Internal { .. } => {
                            return Err(self.simple(ContentErrorKind::InvalidState));
                        }
                    };
                    (
                        None,
                        EntityResolution::Failed {
                            kind: EntityKind::General,
                            failure,
                        },
                        dependency,
                        origin,
                    )
                }
            }
        };
        if id.is_some() && self.active.len() < self.body.limits.frames.max_active_depth {
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
            if error.kind == FrameErrorKind::Lookup(LookupFailure::Unread)
                && let Some(Dependency { kind, location }) = dependency
            {
                let mut failure =
                    self.for_origin(ContentErrorKind::Unavailable(kind), origin, None);
                failure.value_location = Some(location);
                failure
            } else if error.kind == FrameErrorKind::LimitExceeded(FrameLimit::Work)
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
            id: id.expect("successful internal lookup"),
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

    fn prepare_publication(
        &mut self,
    ) -> Result<(TokenKind, FragmentOrigin, usize, usize, usize), ContentError> {
        if self.delivery != Delivery::Ready {
            return Err(self.simple(ContentErrorKind::InvalidState));
        }
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
        let event = match token.location {
            TextLocation::Input(span) => {
                if kind == TokenKind::Start {
                    self.parser
                        .process_dtd_start(
                            raw,
                            offset,
                            &mut self.body.catalog,
                            LiteralSource::Physical,
                            &mut |value, offset, budget| budget.copy(value, offset),
                        )
                        .map(Some)
                        .map_err(|error| tag_failure(error, origin, Some((span, raw))))
                } else {
                    self.parser
                        .process_token(kind, raw, span.decoded.clone(), |value, offset, budget| {
                            budget.copy(value, offset)
                        })
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
                let result = match kind {
                    TokenKind::Start => self
                        .parser
                        .process_dtd_start_owned(raw, offset, &mut self.body.catalog)
                        .map_err(|error| tag_failure(error, origin, None)),
                    TokenKind::End => {
                        self.parser
                            .process_dtd_end_owned(raw, offset)
                            .map_err(|error| {
                                local_parser_failure(error.kind, error.local_offset, origin)
                            })
                    }
                    TokenKind::Text
                    | TokenKind::Cdata
                    | TokenKind::Comment
                    | TokenKind::Instruction => {
                        let leaf = match kind {
                            TokenKind::Text => DtdLeafKind::Text,
                            TokenKind::Cdata => DtdLeafKind::Cdata,
                            TokenKind::Comment => DtdLeafKind::Comment,
                            _ => DtdLeafKind::ProcessingInstruction,
                        };
                        self.parser
                            .process_dtd_leaf_owned(leaf, raw, range)
                            .map_err(|error| {
                                local_parser_failure(error.kind, error.local_offset, origin)
                            })
                    }
                    TokenKind::Dtd | TokenKind::InvalidMarkup => {
                        Err(local_parser_failure(ErrorKind::Syntax, offset, origin))
                    }
                };
                result.map(Some)
            }
        };
        let delta = self.body.catalog.usage().work - catalog_before;
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
        let result = match (event, charged) {
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
        let usage = work.usage();
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
            Err(mut error) => {
                if error.kind
                    == ContentErrorKind::Tag(DtdTagErrorKind::Catalog(CatalogErrorKind::Limit(
                        CatalogLimit::Work,
                    )))
                    && catalog_ceiling <= self.body.limits.catalog.max_work
                {
                    error.kind = ContentErrorKind::Work;
                }
                self.failure = Some(error);
                Err(error)
            }
        }
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

#[path = "../../tests/internal/dtd_content_author.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/internal/dtd_content_spec.rs"]
mod spec_tests;
