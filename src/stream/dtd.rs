//! Private callback-only streaming composition. Public frontends keep Stream.
use super::*;
use crate::dtd_controller::content::{ContentController, ContentError, ContentOutcome};
use crate::dtd_controller::{
    ControllerCounters, ControllerError, ControllerLimits, DtdController, DtdEvent,
};
use crate::dtd_frames::FragmentOrigin;
use crate::tokenizer::local::LocalLimits;
use crate::tokenizer::text::TextLimits;

#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionLimits {
    pub(crate) declarations: ControllerLimits,
    pub(crate) text: TextLimits,
    pub(crate) local: LocalLimits,
}

#[derive(Debug)]
pub(crate) enum LocatedEvent<'event> {
    Xml(Event<'event>),
    Dtd(DtdEvent<'event>),
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

#[expect(
    clippy::large_enum_variant,
    reason = "the entire phase already occupies one fallibly reserved, out-of-line session slot"
)]
enum Phase {
    Declarations {
        controller: DtdController,
        parser: ParserState<'static>,
    },
    Content(ContentController),
}
struct DtdSession {
    decoder: Decoder,
    phase: Option<Phase>,
    decoder_finished: bool,
    physical: usize,
}

/// Zero-sized policy for the one fallible allocation at the DTD phase handoff.
pub(crate) trait SessionSlotReservation {
    fn reserve_one<T>(&self, session: &mut Vec<T>) -> bool;
}

pub(crate) struct TryReserveSessionSlot;
impl SessionSlotReservation for TryReserveSessionSlot {
    fn reserve_one<T>(&self, session: &mut Vec<T>) -> bool {
        session.try_reserve_exact(1).is_ok()
    }
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
pub(crate) struct StreamingSession<R = TryReserveSessionSlot> {
    ordinary: Option<Stream>,
    session: Vec<DtdSession>,
    reservation: R,
    limits: Limits,
    decode: DecodeLimits,
    options: SessionLimits,
    standalone: bool,
    origin: Option<SourceOrigin>,
    physical: usize,
    finalizing: bool,
    finished: bool,
    failure: Option<Failure>,
    delivery: Delivery,
}

impl StreamingSession {
    pub(crate) fn new(limits: Limits, decode: DecodeLimits, options: SessionLimits) -> Self {
        Self::with_reservation(limits, decode, options, TryReserveSessionSlot)
    }
}

impl<R: SessionSlotReservation> StreamingSession<R> {
    fn with_reservation(
        limits: Limits,
        decode: DecodeLimits,
        options: SessionLimits,
        reservation: R,
    ) -> Self {
        Self {
            ordinary: Some(Stream::new(limits, decode)),
            session: Vec::new(),
            reservation,
            limits,
            decode,
            options,
            standalone: false,
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
    pub(crate) fn dtd_counters(&self) -> Option<ControllerCounters> {
        match self.session.first()?.phase.as_ref()? {
            Phase::Declarations { controller, .. } => Some(controller.counters()),
            Phase::Content(controller) => Some(controller.counters()),
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
    pub(crate) fn lend(&mut self) -> Result<(LocatedEvent<'_>, SourceOrigin), SessionError> {
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
        emit: &mut impl FnMut(LocatedEvent<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
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
        emit: &mut impl FnMut(LocatedEvent<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
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
        emit: &mut impl FnMut(LocatedEvent<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
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
                    )
                    .map_err(|error| {
                        if publication.content {
                            SessionError::Content(crate::dtd_controller::content::callback_failure(
                                error,
                                publication.origin,
                            ))
                        } else {
                            SessionError::Xml(error)
                        }
                    })?;
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
        if !self.reservation.reserve_one(&mut self.session) {
            return Err(SessionError::Allocation {
                physical_offset: physical_start,
            });
        }
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
        // Existing scanner work charges the nine-byte grammar rescan. These are
        // already accepted transport bytes: never resend them through Decoder.
        let result = controller.advance(
            tokenizer.current().expect("owned prefix").1,
            state.owned_budget(),
        );
        drop(tokenizer);
        self.session.push(DtdSession {
            decoder,
            phase: Some(Phase::Declarations {
                controller,
                parser: state,
            }),
            decoder_finished: false,
            physical: self.physical,
        });
        result?;
        Ok(())
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
                Phase::Declarations { controller, parser } => {
                    if controller.current().is_some() {
                        return Ok(Drain::Ready);
                    }
                    if controller.is_complete() {
                        self.physical = self.position();
                        let Phase::Declarations { controller, parser } =
                            self.phase.take().expect("declaration owner")
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
    event: LocatedEvent<'event>,
    origin: FragmentOrigin,
    budget: &'event mut Budget,
    content: bool,
}
fn lend_current<'event>(
    ordinary: &'event mut Option<Stream>,
    session: &'event mut [DtdSession],
    standalone: &mut bool,
    last_origin: &mut Option<SourceOrigin>,
) -> Result<LentSession<'event>, SessionError> {
    let (event, origin, budget, content) = if let Some(ordinary) = ordinary {
        let at = ordinary.position();
        let (event, origin) = super::parse(&ordinary.tokenizer, &mut ordinary.state)?
            .ok_or_else(|| Error::new(ErrorKind::InvalidState, at))?;
        if let Event::Declaration(declaration) = &event {
            *standalone = declaration.standalone == Some(true);
        }
        (
            LocatedEvent::Xml(event),
            FragmentOrigin::input(origin.input_range().expect("ordinary physical token"))
                .expect("ordered token span"),
            ordinary.state.owned_budget(),
            false,
        )
    } else {
        match session[0].phase.as_mut().expect("ready phase") {
            Phase::Declarations { controller, parser } => {
                let (event, origin) = controller.current().expect("ready DTD event");
                parser.check_event_limit(origin.error_offset())?;
                parser.commit_checked_dtd_event();
                (
                    LocatedEvent::Dtd(event),
                    origin,
                    parser.owned_budget(),
                    false,
                )
            }
            Phase::Content(controller) => {
                let publication = controller.lend_with_budget()?;
                let origin = publication.origin;
                let budget = publication.budget;
                let event = publication
                    .event
                    .expect("non-events were drained before readiness");
                (LocatedEvent::Xml(event), origin, budget, true)
            }
        }
    };
    *last_origin = Some(SourceOrigin::from_repr(origin));
    Ok(LentSession {
        event,
        origin,
        budget,
        content,
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
            Phase::Declarations { controller, parser } => {
                let consumed = controller.advance(bytes, parser.owned_budget())?;
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

#[path = "../../tests/internal/dtd_stream_author.rs"]
mod tests;

#[path = "../../tests/internal/dtd_stream_spec.rs"]
mod spec_tests;

#[path = "../../tests/internal/dtd_stream_utf16_author.rs"]
mod utf16_tests;

#[path = "../../tests/internal/dtd_stream_utf16_spec.rs"]
mod utf16_spec_tests;

#[path = "../../tests/internal/dtd_lending_author.rs"]
mod lending_tests;

#[path = "../../tests/internal/dtd_lending_spec.rs"]
mod lending_spec_tests;
