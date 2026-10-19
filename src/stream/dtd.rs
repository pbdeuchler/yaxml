//! Streaming composition of the ordinary token loop with DTD and entity phases.
use super::*;
use crate::dtd::{DtdErrorKind, DtdLimit, DtdLimits};
use crate::dtd_catalog::{CatalogErrorKind, CatalogLimit, CatalogLimits};
use crate::dtd_controller::content::{
    ContentController, ContentError, ContentErrorKind, ContentOutcome,
};
use crate::dtd_controller::{
    ControllerError, ControllerErrorKind, ControllerLimit, ControllerLimits, DtdController,
    DtdEvent,
};
use crate::dtd_frames::{FragmentOrigin, FrameErrorKind, FrameLimit, FrameLimits, LookupFailure};
use crate::dtd_reference::{ReferenceErrorKind, ReferenceLimit, ReferenceLimits};
use crate::dtd_scan::{ScanErrorKind, ScanLimit, ScanLimits};
use crate::dtd_tag::DtdTagErrorKind;
use crate::tokenizer::local::{LocalErrorKind, LocalLimits};
use crate::tokenizer::reference::ReferenceCursorErrorKind;
use crate::tokenizer::text::{TextErrorKind, TextLimits, TextScratchLimits};
use crate::{Doctype, Limit};

#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionLimits {
    pub(crate) declarations: ControllerLimits,
    pub(crate) text: TextLimits,
    pub(crate) local: LocalLimits,
}

impl SessionLimits {
    /// Derives every private DTD ceiling from the public ones. Work ceilings
    /// bound scanning and expansion together, so they scale with both inputs.
    pub(crate) fn new(limits: Limits, decode: DecodeLimits) -> Self {
        let token = limits.max_token_bytes;
        let scratch = token.max(decode.max_decoded_token_bytes).saturating_mul(2);
        let expansion = limits.max_entity_expansion_bytes;
        let references = limits.max_entity_references;
        let depth = limits.max_entity_depth;
        let work = limits
            .max_input_bytes
            .saturating_add(expansion)
            .saturating_mul(8);
        Self {
            declarations: ControllerLimits {
                scan: ScanLimits {
                    max_unit_bytes: token,
                    max_scratch_bytes: token,
                    max_work: work,
                },
                unit: DtdLimits {
                    max_unit_bytes: token,
                    max_tokens: token,
                    max_model_depth: limits.max_depth,
                    max_attribute_definitions: limits.max_attributes,
                },
                catalog: CatalogLimits {
                    max_declarations: limits.max_dtd_bytes,
                    max_bindings: limits.max_dtd_bytes,
                    max_storage_bytes: limits.max_owned_bytes,
                    max_value_bytes: token,
                    max_expanded_bytes: expansion,
                    max_work: work,
                    max_references: references,
                    max_depth: depth,
                },
                frames: FrameLimits {
                    max_active_depth: depth,
                    max_reference_attempts: references,
                    max_work: work,
                    max_expanded_bytes: expansion,
                },
                max_physical_bytes: limits.max_dtd_bytes,
                max_work: work,
                max_owned_bytes: limits.max_owned_bytes,
            },
            text: TextLimits {
                max_work: work,
                reference: ReferenceLimits {
                    max_bytes: token,
                    max_scratch_bytes: token,
                    max_work: work,
                },
                scratch: TextScratchLimits {
                    input_token_bytes: scratch,
                    replacement_token_bytes: scratch,
                    max_bytes: usize::MAX,
                },
            },
            local: LocalLimits {
                max_token_bytes: token,
                max_scratch_bytes: token,
                max_work: work,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionError {
    Xml(Error),
    Dtd(ControllerError),
    Content(ContentError),
    Allocation { physical_offset: usize },
    Poisoned { physical_offset: usize },
}
impl SessionError {
    pub(crate) fn physical_offset(self) -> usize {
        match self {
            Self::Xml(error) => error.offset,
            Self::Dtd(error) => error.physical_offset,
            Self::Content(error) => error.physical_offset,
            Self::Allocation { physical_offset } | Self::Poisoned { physical_offset } => {
                physical_offset
            }
        }
    }
    // Decoder's callback accepts Error. The actual typed phase failure is kept
    // beside this private stop signal and returned instead of exposing it.
    fn decoder_stop(self) -> Error {
        Error::new(ErrorKind::InvalidState, self.physical_offset())
    }
}
impl From<Error> for SessionError {
    fn from(error: Error) -> Self {
        Self::Xml(error)
    }
}
impl From<ControllerError> for SessionError {
    fn from(error: ControllerError) -> Self {
        Self::Dtd(error)
    }
}
impl From<ContentError> for SessionError {
    fn from(error: ContentError) -> Self {
        Self::Content(error)
    }
}

/// Every private phase failure surfaces as one public kind at its physical offset.
impl From<SessionError> for Error {
    fn from(error: SessionError) -> Self {
        let kind = match error {
            SessionError::Xml(error) => return error,
            SessionError::Dtd(error) => controller_kind(error.kind),
            SessionError::Content(error) => content_kind(error.kind),
            SessionError::Allocation { .. } => ErrorKind::Allocation,
            SessionError::Poisoned { .. } => ErrorKind::InvalidState,
        };
        Error::new(kind, error.physical_offset())
    }
}

const fn limit(limit: Limit) -> ErrorKind {
    ErrorKind::LimitExceeded(limit)
}

fn grammar_kind(kind: DtdErrorKind) -> ErrorKind {
    match kind {
        DtdErrorKind::InvalidName => ErrorKind::InvalidName,
        DtdErrorKind::InvalidCharacter => ErrorKind::InvalidCharacter,
        DtdErrorKind::InvalidReference => ErrorKind::InvalidReference,
        DtdErrorKind::LimitExceeded(DtdLimit::UnitBytes | DtdLimit::Tokens) => {
            limit(Limit::TokenBytes)
        }
        DtdErrorKind::LimitExceeded(DtdLimit::ModelDepth) => limit(Limit::Depth),
        DtdErrorKind::LimitExceeded(DtdLimit::AttributeDefinitions) => limit(Limit::Attributes),
        DtdErrorKind::Allocation => ErrorKind::Allocation,
        DtdErrorKind::Syntax
        | DtdErrorKind::InvalidNmtoken
        | DtdErrorKind::InvalidPublicIdentifier
        | DtdErrorKind::SystemIdentifierFragment
        | DtdErrorKind::ReservedPiTarget => ErrorKind::InvalidDtd,
    }
}

fn controller_kind(kind: ControllerErrorKind) -> ErrorKind {
    match kind {
        ControllerErrorKind::Scan(kind) => match kind {
            ScanErrorKind::Syntax => ErrorKind::InvalidDtd,
            ScanErrorKind::UnexpectedEof => ErrorKind::UnexpectedEof,
            ScanErrorKind::InvalidUtf8 => ErrorKind::InvalidUtf8,
            ScanErrorKind::Limit(ScanLimit::UnitBytes | ScanLimit::ScratchBytes) => {
                limit(Limit::TokenBytes)
            }
            ScanErrorKind::Limit(ScanLimit::Work) => limit(Limit::EntityExpansion),
            ScanErrorKind::Allocation => ErrorKind::Allocation,
        },
        ControllerErrorKind::Grammar(kind) => grammar_kind(kind),
        ControllerErrorKind::Catalog(kind) => catalog_kind(kind),
        ControllerErrorKind::Frame(kind) => frame_kind(kind),
        ControllerErrorKind::Ownership(kind) => kind,
        ControllerErrorKind::Limit(ControllerLimit::PhysicalBytes) => limit(Limit::DtdBytes),
        ControllerErrorKind::Limit(ControllerLimit::Work) => limit(Limit::EntityExpansion),
        ControllerErrorKind::Limit(ControllerLimit::OwnedBytes) => limit(Limit::OwnedBytes),
        ControllerErrorKind::InvalidUnitFamily => ErrorKind::InvalidDtd,
        ControllerErrorKind::PositionOverflow | ControllerErrorKind::InvalidState => {
            ErrorKind::InvalidState
        }
        ControllerErrorKind::Allocation => ErrorKind::Allocation,
    }
}

fn catalog_kind(kind: CatalogErrorKind) -> ErrorKind {
    match kind {
        CatalogErrorKind::EntityDeclared => ErrorKind::UnknownEntity,
        CatalogErrorKind::ExternalInAttribute => ErrorKind::ExternalEntityInAttribute,
        CatalogErrorKind::UnparsedEntity => ErrorKind::UnparsedEntity,
        CatalogErrorKind::LessThanInAttribute => ErrorKind::Syntax,
        CatalogErrorKind::Recursion => ErrorKind::EntityLoop,
        CatalogErrorKind::ParameterInInternalDeclaration
        | CatalogErrorKind::InvalidPredefinedDeclaration => ErrorKind::InvalidDtd,
        CatalogErrorKind::InvalidReference => ErrorKind::InvalidReference,
        CatalogErrorKind::Namespace => ErrorKind::Namespace,
        CatalogErrorKind::InvalidCharacter => ErrorKind::InvalidCharacter,
        CatalogErrorKind::CallerValueBytes | CatalogErrorKind::Limit(CatalogLimit::ValueBytes) => {
            limit(Limit::TokenBytes)
        }
        CatalogErrorKind::Limit(CatalogLimit::Declarations | CatalogLimit::Bindings) => {
            limit(Limit::DtdBytes)
        }
        CatalogErrorKind::Limit(CatalogLimit::StorageBytes) => limit(Limit::OwnedBytes),
        CatalogErrorKind::Limit(CatalogLimit::ExpandedBytes | CatalogLimit::Work) => {
            limit(Limit::EntityExpansion)
        }
        CatalogErrorKind::Limit(CatalogLimit::References) => limit(Limit::EntityReferences),
        CatalogErrorKind::Limit(CatalogLimit::Depth) => limit(Limit::EntityDepth),
        CatalogErrorKind::Allocation => ErrorKind::Allocation,
        CatalogErrorKind::Grammar(kind) => grammar_kind(kind),
        CatalogErrorKind::Xml(kind) => kind,
        CatalogErrorKind::Load(_) => ErrorKind::InvalidState,
    }
}

fn frame_kind(kind: FrameErrorKind) -> ErrorKind {
    match kind {
        FrameErrorKind::Lookup(LookupFailure::Unparsed) => ErrorKind::UnparsedEntity,
        FrameErrorKind::Lookup(_) => ErrorKind::UnknownEntity,
        FrameErrorKind::Cycle => ErrorKind::EntityLoop,
        FrameErrorKind::LimitExceeded(FrameLimit::ActiveDepth) => limit(Limit::EntityDepth),
        FrameErrorKind::LimitExceeded(FrameLimit::ReferenceAttempts) => {
            limit(Limit::EntityReferences)
        }
        FrameErrorKind::LimitExceeded(FrameLimit::Work | FrameLimit::ExpandedBytes)
        | FrameErrorKind::CounterOverflow(_) => limit(Limit::EntityExpansion),
        FrameErrorKind::PrematureEnd
        | FrameErrorKind::BoundaryMismatch
        | FrameErrorKind::ElementBoundaryCrossing
        | FrameErrorKind::ElementDepthMismatch => ErrorKind::UnbalancedEntity,
        FrameErrorKind::Allocation => ErrorKind::Allocation,
        FrameErrorKind::InvalidSpan
        | FrameErrorKind::InvalidContext
        | FrameErrorKind::KindMismatch
        | FrameErrorKind::InvocationIdsExhausted
        | FrameErrorKind::NoActiveFrame
        | FrameErrorKind::SourceMismatch => ErrorKind::InvalidState,
    }
}

fn content_kind(kind: ContentErrorKind) -> ErrorKind {
    match kind {
        ContentErrorKind::Text(kind) => match kind {
            TextErrorKind::Xml(kind) | TextErrorKind::Local(LocalErrorKind::Xml(kind)) => kind,
            TextErrorKind::Local(LocalErrorKind::ForbiddenMarkup) => ErrorKind::Syntax,
            TextErrorKind::Local(LocalErrorKind::Work) | TextErrorKind::Work => {
                limit(Limit::EntityExpansion)
            }
            TextErrorKind::Reference(kind) => match kind {
                ReferenceErrorKind::Syntax => ErrorKind::InvalidReference,
                ReferenceErrorKind::InvalidUtf8 => ErrorKind::InvalidUtf8,
                ReferenceErrorKind::InvalidCharacter => ErrorKind::InvalidCharacter,
                ReferenceErrorKind::UnexpectedEof => ErrorKind::UnexpectedEof,
                ReferenceErrorKind::Limit(ReferenceLimit::Bytes | ReferenceLimit::ScratchBytes) => {
                    limit(Limit::TokenBytes)
                }
                ReferenceErrorKind::Limit(ReferenceLimit::Work) => limit(Limit::EntityExpansion),
                ReferenceErrorKind::Allocation => ErrorKind::Allocation,
                ReferenceErrorKind::PositionOverflow
                | ReferenceErrorKind::InvalidState
                | ReferenceErrorKind::Sealed => ErrorKind::InvalidState,
            },
            TextErrorKind::Cursor(ReferenceCursorErrorKind::Limit(kind)) => limit(kind),
            TextErrorKind::Scratch | TextErrorKind::ScratchOverflow => limit(Limit::TokenBytes),
            TextErrorKind::Local(_)
            | TextErrorKind::Cursor(_)
            | TextErrorKind::PositionOverflow
            | TextErrorKind::InvalidState
            | TextErrorKind::SourceMismatch => ErrorKind::InvalidState,
        },
        ContentErrorKind::Frame(kind) => frame_kind(kind),
        ContentErrorKind::Tag(kind) => match kind {
            DtdTagErrorKind::Parser(kind) => kind,
            DtdTagErrorKind::Catalog(kind) => catalog_kind(kind),
            DtdTagErrorKind::PositionOverflow => ErrorKind::InvalidState,
        },
        ContentErrorKind::Parser(kind) | ContentErrorKind::Callback(kind) => kind,
        ContentErrorKind::Work => limit(Limit::EntityExpansion),
        ContentErrorKind::PhysicalBytes => limit(Limit::InputBytes),
        ContentErrorKind::Allocation => ErrorKind::Allocation,
        ContentErrorKind::InvalidState | ContentErrorKind::Poisoned => ErrorKind::InvalidState,
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "the entire phase already occupies one fallibly reserved, out-of-line session slot"
)]
enum Phase {
    Declarations {
        controller: DtdController,
        parser: ParserState<'static>,
        /// The declaration's decoded UTF-8 text, published once at its end.
        raw: Vec<u8>,
    },
    Content(ContentController),
}
struct DtdSession {
    decoder: Decoder,
    phase: Option<Phase>,
    decoder_finished: bool,
    physical: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionOutcome {
    Ready,
    NeedInput,
    Complete,
}
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct SessionStep {
    /// Newly accepted transport bytes; replay and generated sources count zero.
    pub(crate) consumed: usize,
    pub(crate) outcome: SessionOutcome,
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum Delivery {
    Idle,
    Ready,
    Lent,
}
enum Drain {
    Ready,
    NeedInput,
    Complete,
}

#[derive(Clone, Copy)]
enum Failure {
    Error(SessionError),
    Poisoned,
}

/// Length-zero/one slot keeps the large DTD/body state off the ordinary stack.
/// Whole-input receipt/limits belong to the caller, as with Stream::advance.
pub(crate) struct StreamingSession {
    ordinary: Option<Stream>,
    session: Vec<DtdSession>,
    limits: Limits,
    decode: DecodeLimits,
    options: SessionLimits,
    standalone: bool,
    loader: Option<crate::resolve::Loader>,
    origin: Option<SourceOrigin>,
    physical: usize,
    finalizing: bool,
    finished: bool,
    failure: Option<Failure>,
    delivery: Delivery,
}

impl StreamingSession {
    pub(crate) fn new(limits: Limits, decode: DecodeLimits) -> Self {
        Self {
            ordinary: Some(Stream::new(limits, decode)),
            session: Vec::new(),
            limits,
            decode,
            options: SessionLimits::new(limits, decode),
            standalone: false,
            loader: None,
            origin: None,
            physical: 0,
            finalizing: false,
            finished: false,
            failure: None,
            delivery: Delivery::Idle,
        }
    }
    pub(crate) fn position(&self) -> usize {
        self.ordinary.as_ref().map_or_else(
            || {
                self.session
                    .first()
                    .map_or(self.physical, DtdSession::position)
            },
            Stream::position,
        )
    }
    pub(crate) fn event_origin(&self) -> Option<SourceOrigin> {
        self.origin
    }
    pub(crate) fn set_resolver(&mut self, base: &str, resolver: Box<dyn crate::Resolver>) {
        let unit = self.options.declarations.unit;
        self.loader = Some(crate::resolve::Loader::new(
            base,
            resolver,
            &self.limits,
            unit,
        ));
    }
    pub(crate) fn catalog(&self) -> Option<&crate::dtd_catalog::DtdCatalog> {
        match self.session.first()?.phase.as_ref()? {
            Phase::Content(controller) => Some(controller.catalog()),
            Phase::Declarations { .. } => None,
        }
    }
    /// Park one source token; refeed any unconsumed transport suffix after acknowledgment.
    pub(crate) fn prepare(&mut self, input: &[u8]) -> Result<SessionStep, SessionError> {
        self.remember(|s| {
            if s.finished || (s.finalizing && (!input.is_empty() || s.delivery != Delivery::Ready))
            {
                return Err(Error::new(ErrorKind::InvalidState, s.position()).into());
            }
            s.prepare_inner(input)
        })
    }
    /// Seal physical EOF while allowing a parked Ready token to be lent once.
    pub(crate) fn finish_step(&mut self) -> Result<SessionStep, SessionError> {
        self.remember(|s| {
            if s.delivery == Delivery::Lent {
                return Err(Error::new(ErrorKind::InvalidState, s.position()).into());
            }
            s.finalizing = true;
            s.prepare_inner(&[])
        })
    }
    /// Publish once and lend source storage until the returned borrow ends.
    pub(crate) fn lend(&mut self) -> Result<(Event<'_>, SourceOrigin), SessionError> {
        if let Some(error) = self.stored_failure() {
            return Err(error);
        }
        if self.delivery != Delivery::Ready {
            let error = Error::new(ErrorKind::InvalidState, self.position()).into();
            self.failure = Some(Failure::Error(error));
            return Err(error);
        }
        self.failure = Some(Failure::Poisoned);
        match lend_current(
            &mut self.ordinary,
            &mut self.session,
            &mut self.standalone,
            &mut self.origin,
        ) {
            Ok(publication) => {
                self.delivery = Delivery::Lent;
                self.failure = None;
                Ok((
                    publication.event,
                    SourceOrigin::from_repr(publication.origin),
                ))
            }
            Err(error) => {
                self.failure = Some(Failure::Error(error));
                Err(error)
            }
        }
    }
    /// Confirm delivery and release the paused source; no entity is entered here.
    pub(crate) fn acknowledge(&mut self) -> Result<(), SessionError> {
        self.remember(Self::acknowledge_inner)
    }
    fn acknowledge_inner(&mut self) -> Result<(), SessionError> {
        if self.delivery != Delivery::Lent {
            return Err(Error::new(ErrorKind::InvalidState, self.position()).into());
        }
        if let Some(ordinary) = &mut self.ordinary {
            ordinary.release();
        } else {
            match self.session[0].phase.as_mut().expect("lent phase") {
                Phase::Declarations { controller, .. } => controller.release(),
                Phase::Content(controller) => controller.acknowledge()?,
            }
        }
        self.delivery = Delivery::Idle;
        Ok(())
    }
    pub(crate) fn advance(
        &mut self,
        input: &[u8],
        emit: &mut impl FnMut(Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<usize, SessionError> {
        self.remember(|s| {
            if s.finalizing || s.finished {
                return Err(Error::new(ErrorKind::InvalidState, s.position()).into());
            }
            s.drive(input, emit)
        })
    }
    pub(crate) fn finish(
        &mut self,
        emit: &mut impl FnMut(Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<(), SessionError> {
        self.remember(|s| {
            if s.finished {
                return Ok(());
            }
            if s.delivery == Delivery::Lent {
                return Err(Error::new(ErrorKind::InvalidState, s.position()).into());
            }
            s.finalizing = true;
            s.drive(&[], emit).map(|_| ())
        })
    }
    fn drive(
        &mut self,
        input: &[u8],
        emit: &mut impl FnMut(Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<usize, SessionError> {
        let mut consumed = 0;
        loop {
            let step = self.prepare_inner(&input[consumed..])?;
            consumed += step.consumed;
            match step.outcome {
                SessionOutcome::Ready => {
                    let publication = lend_current(
                        &mut self.ordinary,
                        &mut self.session,
                        &mut self.standalone,
                        &mut self.origin,
                    )?;
                    self.delivery = Delivery::Lent;
                    emit(
                        publication.event,
                        SourceOrigin::from_repr(publication.origin),
                        publication.budget,
                    )?;
                    self.acknowledge_inner()?;
                }
                SessionOutcome::NeedInput | SessionOutcome::Complete => return Ok(consumed),
            }
        }
    }
    fn prepare_inner(&mut self, input: &[u8]) -> Result<SessionStep, SessionError> {
        if self.delivery == Delivery::Lent {
            return Err(Error::new(ErrorKind::InvalidState, self.position()).into());
        }
        if self.delivery == Delivery::Ready {
            return Ok(SessionStep {
                consumed: 0,
                outcome: SessionOutcome::Ready,
            });
        }
        if self.finished {
            return Ok(SessionStep {
                consumed: 0,
                outcome: SessionOutcome::Complete,
            });
        }
        let mut consumed = 0;
        loop {
            if let Some(ordinary) = &mut self.ordinary {
                if ordinary.has_token() {
                    if ordinary
                        .tokenizer
                        .current()
                        .is_some_and(|(kind, _, _)| kind == TokenKind::Dtd)
                    {
                        self.start_dtd()?;
                        continue;
                    }
                    if ordinary.is_non_event() {
                        ordinary.emit(&mut |_, _, _| unreachable!("classified non-event"))?;
                        ordinary.release();
                        continue;
                    }
                    self.delivery = Delivery::Ready;
                    return Ok(SessionStep {
                        consumed,
                        outcome: SessionOutcome::Ready,
                    });
                }
                if self.finalizing {
                    if ordinary.finish_step()? {
                        continue;
                    }
                    ordinary.validate_finish()?;
                    self.finished = true;
                    return Ok(SessionStep {
                        consumed,
                        outcome: SessionOutcome::Complete,
                    });
                }
                consumed += ordinary.advance(&input[consumed..])?;
                if ordinary.has_token() {
                    continue;
                }
                if consumed == input.len() {
                    return Ok(SessionStep {
                        consumed,
                        outcome: SessionOutcome::NeedInput,
                    });
                }
                return Err(Error::new(ErrorKind::InvalidState, ordinary.position()).into());
            } else {
                let session = self.session.first_mut().expect("one active DTD session");
                match session.drain(self.limits, self.decode, self.options)? {
                    Drain::Ready => {
                        self.delivery = Delivery::Ready;
                        return Ok(SessionStep {
                            consumed,
                            outcome: SessionOutcome::Ready,
                        });
                    }
                    Drain::Complete => {
                        self.finished = true;
                        return Ok(SessionStep {
                            consumed,
                            outcome: SessionOutcome::Complete,
                        });
                    }
                    Drain::NeedInput => {}
                }
                if self.finalizing {
                    session.finish_decoder()?;
                    continue;
                }
                let progress = session.advance_decoder(&input[consumed..])?;
                consumed += progress.consumed;
                if !progress.suspended {
                    if consumed == input.len() {
                        return Ok(SessionStep {
                            consumed,
                            outcome: SessionOutcome::NeedInput,
                        });
                    }
                    if progress.consumed == 0 {
                        return Err(Error::new(ErrorKind::InvalidState, session.position()).into());
                    }
                }
            }
        }
    }
    fn start_dtd(&mut self) -> Result<(), SessionError> {
        let ordinary = self.ordinary.as_ref().expect("ordinary phase");
        let (_, raw, span) = ordinary.tokenizer.current().expect("DTD prefix ready");
        if raw != b"<!DOCTYPE" {
            return Err(Error::new(ErrorKind::InvalidState, span.source.start).into());
        }
        ordinary.state.check_doctype_start(span.source.start)?;
        let encoding = span.encoding;
        let physical_start = span.source.start;
        let decoded_start = span.decoded.start;
        self.session
            .try_reserve_exact(1)
            .map_err(|_| SessionError::Allocation {
                physical_offset: physical_start,
            })?;
        self.physical = ordinary.position();
        let Stream {
            tokenizer,
            decoder,
            mut state,
            ..
        } = self.ordinary.take().expect("checked ordinary owner");
        let mut controller = DtdController::new_at(
            encoding,
            physical_start,
            decoded_start,
            self.options.declarations,
            self.standalone,
        );
        if let Some(loader) = self.loader.take() {
            controller.set_loader(loader);
        }
        // Existing scanner work charges the nine-byte grammar rescan. These are
        // already accepted transport bytes: never resend them through Decoder.
        let prefix = tokenizer.current().expect("owned prefix").1;
        let mut raw = Vec::new();
        let result = controller
            .advance(prefix, state.owned_budget())
            .map_err(SessionError::from)
            .and_then(|consumed| {
                capture(
                    &mut raw,
                    &prefix[..consumed],
                    state.owned_budget(),
                    physical_start,
                )
                .map_err(SessionError::from)
            });
        drop(tokenizer);
        self.session.push(DtdSession {
            decoder,
            phase: Some(Phase::Declarations {
                controller,
                parser: state,
                raw,
            }),
            decoder_finished: false,
            physical: self.physical,
        });
        result
    }
    fn stored_failure(&mut self) -> Option<SessionError> {
        let failure = self.failure?;
        let error = match failure {
            Failure::Error(error) => error,
            Failure::Poisoned => SessionError::Poisoned {
                physical_offset: self.position(),
            },
        };
        self.failure = Some(Failure::Error(error));
        Some(error)
    }
    fn remember<T>(
        &mut self,
        run: impl FnOnce(&mut Self) -> Result<T, SessionError>,
    ) -> Result<T, SessionError> {
        if let Some(error) = self.stored_failure() {
            return Err(error);
        }
        // An unwind can follow transport progress. Resolve its diagnostic site
        // only on re-entry, then retain that first resolved error unchanged.
        self.failure = Some(Failure::Poisoned);
        let result = run(self);
        self.failure = result.as_ref().err().copied().map(Failure::Error);
        result
    }
}

/// Retains accepted declaration bytes, charged to the shared string budget.
fn capture(
    raw: &mut Vec<u8>,
    bytes: &[u8],
    budget: &mut Budget,
    offset: usize,
) -> Result<(), Error> {
    budget.charge_owned(bytes.len(), offset)?;
    raw.try_reserve(bytes.len())
        .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
    raw.extend_from_slice(bytes);
    Ok(())
}

impl DtdSession {
    fn position(&self) -> usize {
        self.decoder.position().max(match &self.phase {
            Some(Phase::Declarations { controller, .. }) => controller.physical_position(),
            Some(Phase::Content(controller)) => controller.position().1,
            None => self.physical,
        })
    }
    fn drain(
        &mut self,
        limits: Limits,
        decode: DecodeLimits,
        options: SessionLimits,
    ) -> Result<Drain, SessionError> {
        loop {
            match self.phase.as_mut().expect("owned semantic phase") {
                Phase::Declarations {
                    controller, parser, ..
                } => {
                    // Only the declaration's end is published; its parts are internal.
                    match controller.current() {
                        Some((DtdEvent::End, _)) => return Ok(Drain::Ready),
                        Some(_) => {
                            controller.release();
                            continue;
                        }
                        None => {}
                    }
                    if controller.is_complete() {
                        self.physical = self.position();
                        let Phase::Declarations {
                            controller, parser, ..
                        } = self.phase.take().expect("declaration owner")
                        else {
                            unreachable!("checked declaration phase")
                        };
                        let body = controller.into_body_state()?;
                        let content = ContentController::new(
                            body,
                            parser,
                            Tokenizer::new(limits.max_token_bytes, decode.max_decoded_token_bytes),
                            options.text,
                            options.local,
                        )?;
                        self.phase = Some(Phase::Content(content));
                        continue;
                    }
                    if self.decoder_finished {
                        controller.finish_step(parser.owned_budget())?;
                    } else {
                        controller.advance(&[], parser.owned_budget())?;
                    }
                    if controller.current().is_none() && !controller.is_complete() {
                        if self.decoder_finished {
                            return Err(Error::new(
                                ErrorKind::InvalidState,
                                controller.physical_position(),
                            )
                            .into());
                        }
                        return Ok(Drain::NeedInput);
                    }
                }
                Phase::Content(controller) => {
                    let step = if self.decoder_finished {
                        controller.finish_step()?
                    } else {
                        controller.advance(&[])?
                    };
                    match step.outcome {
                        ContentOutcome::EventReady => {
                            if controller.is_non_event() {
                                controller
                                    .emit(&mut |_, _, _| unreachable!("classified non-event"))?;
                                continue;
                            }
                            return Ok(Drain::Ready);
                        }
                        ContentOutcome::Progress => {}
                        ContentOutcome::NeedPhysicalInput => {
                            if self.decoder_finished {
                                return Err(Error::new(
                                    ErrorKind::InvalidState,
                                    controller.position().1,
                                )
                                .into());
                            }
                            return Ok(Drain::NeedInput);
                        }
                        ContentOutcome::Complete => return Ok(Drain::Complete),
                    }
                }
            }
        }
    }
    fn advance_decoder(&mut self, input: &[u8]) -> Result<DecodeProgress, SessionError> {
        let mut failure = None;
        let phase = self.phase.as_mut().expect("semantic phase");
        let result = self
            .decoder
            .advance(input, |decoded| phase_decode(phase, decoded, &mut failure));
        if let Some(error) = failure {
            return Err(error);
        }
        result.map_err(SessionError::Xml)
    }
    fn finish_decoder(&mut self) -> Result<(), SessionError> {
        if self.decoder_finished {
            return Ok(());
        }
        let mut failure = None;
        let phase = self.phase.as_mut().expect("semantic phase");
        let result = self
            .decoder
            .finish_step(|decoded| phase_decode(phase, decoded, &mut failure));
        if let Some(error) = failure {
            return Err(error);
        }
        if !result? {
            self.decoder_finished = true;
        }
        Ok(())
    }
}
struct LentSession<'event> {
    event: Event<'event>,
    origin: FragmentOrigin,
    budget: &'event mut Budget,
}
fn lend_current<'event>(
    ordinary: &'event mut Option<Stream>,
    session: &'event mut [DtdSession],
    standalone: &mut bool,
    last_origin: &mut Option<SourceOrigin>,
) -> Result<LentSession<'event>, SessionError> {
    let (event, origin, budget) = if let Some(ordinary) = ordinary {
        let at = ordinary.position();
        let (event, origin) = super::parse(&ordinary.tokenizer, &mut ordinary.state)?
            .ok_or_else(|| Error::new(ErrorKind::InvalidState, at))?;
        if let Event::Declaration(declaration) = &event {
            *standalone = declaration.standalone == Some(true);
        }
        (
            event,
            FragmentOrigin::input(origin.input_range().expect("ordinary physical token"))
                .expect("ordered token span"),
            ordinary.state.owned_budget(),
        )
    } else {
        match session[0].phase.as_mut().expect("ready phase") {
            Phase::Declarations {
                controller,
                parser,
                raw,
            } => {
                let (_, end) = controller.current().expect("ready DTD end");
                let range = controller.start()..end.physical_context().end;
                parser.check_event_limit(range.start)?;
                let raw = std::str::from_utf8(raw)
                    .map_err(|_| Error::new(ErrorKind::InvalidState, range.start))?;
                parser.commit_doctype();
                (
                    Event::Doctype(Doctype::parse(raw)),
                    FragmentOrigin::input(range).expect("ordered declaration span"),
                    parser.owned_budget(),
                )
            }
            Phase::Content(controller) => {
                let publication = controller.lend_with_budget()?;
                let event = publication
                    .event
                    .expect("non-events were drained before readiness");
                (event, publication.origin, publication.budget)
            }
        }
    };
    *last_origin = Some(SourceOrigin::from_repr(origin));
    Ok(LentSession {
        event,
        origin,
        budget,
    })
}

fn phase_decode(
    phase: &mut Phase,
    decoded: Decoded<'_>,
    failure: &mut Option<SessionError>,
) -> Result<DecodeProgress, Error> {
    let result = (|| -> Result<DecodeProgress, SessionError> {
        let Decoded::Bytes(bytes) = decoded else {
            let at = match phase {
                Phase::Declarations { controller, .. } => controller.physical_position(),
                Phase::Content(controller) => controller.position().1,
            };
            return Err(Error::new(ErrorKind::InvalidState, at).into());
        };
        match phase {
            Phase::Declarations {
                controller,
                parser,
                raw,
            } => {
                let consumed = controller.advance(bytes, parser.owned_budget())?;
                capture(
                    raw,
                    &bytes[..consumed],
                    parser.owned_budget(),
                    controller.physical_position(),
                )?;
                Ok(DecodeProgress {
                    consumed,
                    suspended: controller.current().is_some() || controller.is_complete(),
                })
            }
            Phase::Content(controller) => {
                let step = controller.advance(bytes)?;
                Ok(DecodeProgress {
                    consumed: step.consumed,
                    suspended: step.outcome != ContentOutcome::NeedPhysicalInput,
                })
            }
        }
    })();
    result.map_err(|error| {
        *failure = Some(error);
        error.decoder_stop()
    })
}
