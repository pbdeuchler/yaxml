//! Private callback-only streaming composition. Public frontends keep Stream.
use super::*;
use crate::dtd_controller::content::{ContentController, ContentError, ContentOutcome};
use crate::dtd_controller::{
    ControllerCounters, ControllerError, ControllerLimits, DtdController, DtdEvent,
};
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

#[derive(Clone, Copy)]
enum Drain {
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
                    let standalone = &mut self.standalone;
                    let origin = &mut self.origin;
                    ordinary.emit(&mut |event, source, budget| {
                        if let Event::Declaration(declaration) = &event {
                            *standalone = declaration.standalone == Some(true);
                        }
                        *origin = Some(source);
                        emit(LocatedEvent::Xml(event), source, budget)
                    })?;
                    ordinary.release();
                    continue;
                }
                if self.finalizing {
                    if ordinary.finish_step()? {
                        continue;
                    }
                    ordinary.validate_finish()?;
                    self.finished = true;
                    return Ok(consumed);
                }
                let accepted = ordinary.advance(&input[consumed..])?;
                consumed += accepted;
                if ordinary.has_token() {
                    continue;
                }
                if consumed == input.len() {
                    return Ok(consumed);
                }
                if accepted == 0 {
                    return Err(Error::new(ErrorKind::InvalidState, ordinary.position()).into());
                }
            } else {
                let options = self.options;
                let session = self.session.first_mut().expect("one active DTD session");
                match session.drain(self.limits, self.decode, options, &mut self.origin, emit)? {
                    Drain::Complete => {
                        self.finished = true;
                        return Ok(consumed);
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
                        return Ok(consumed);
                    }
                    if progress.consumed == 0 {
                        return Err(Error::new(ErrorKind::InvalidState, session.position()).into());
                    }
                }
                // Suspension may have accepted the final transport byte, or
                // no new byte at all. Drain the held event/generated transition
                // before asking Decoder for any further physical input.
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
    fn remember<T>(
        &mut self,
        run: impl FnOnce(&mut Self) -> Result<T, SessionError>,
    ) -> Result<T, SessionError> {
        if let Some(failure) = self.failure {
            let error = match failure {
                Failure::Error(error) => error,
                Failure::Poisoned => SessionError::Poisoned {
                    physical_offset: self.position(),
                },
            };
            self.failure = Some(Failure::Error(error));
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
        origin: &mut Option<SourceOrigin>,
        emit: &mut impl FnMut(LocatedEvent<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<Drain, SessionError> {
        loop {
            match self.phase.as_mut().expect("owned semantic phase") {
                Phase::Declarations { controller, parser } => {
                    if let Some((event, source)) = controller.current() {
                        parser.check_event_limit(source.error_offset())?;
                        parser.commit_checked_dtd_event();
                        let source = SourceOrigin::from_repr(source);
                        *origin = Some(source);
                        emit(LocatedEvent::Dtd(event), source, parser.owned_budget())?;
                        controller.release();
                        continue;
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
                            controller.emit(&mut |event, source, budget| {
                                *origin = Some(source);
                                emit(LocatedEvent::Xml(event), source, budget)
                            })?
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
