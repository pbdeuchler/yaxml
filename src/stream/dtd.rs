//! Streaming composition of the ordinary token loop with DTD and entity phases.
use super::*;
use crate::Doctype;
use crate::dtd::DtdLimits;
use crate::dtd_controller::content::{ContentController, ContentOutcome};
use crate::dtd_controller::{DtdController, DtdEvent};
use crate::dtd_frames::FragmentOrigin;
use crate::options::Flags;

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

#[derive(Clone, Copy)]
enum Failure {
    Error(Error),
    Poisoned,
}

/// Length-zero/one slot keeps the large DTD/body state off the ordinary stack.
/// Whole-input receipt/limits belong to the caller, as with Stream::advance.
pub(crate) struct StreamingSession {
    ordinary: Option<Stream>,
    session: Vec<DtdSession>,
    limits: Limits,
    decode: DecodeLimits,
    options: DtdLimits,
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
    pub(crate) fn new(limits: Limits, decode: DecodeLimits, flags: Flags) -> Self {
        Self {
            ordinary: Some(Stream::new(limits, decode, flags)),
            session: Vec::new(),
            limits,
            decode,
            options: DtdLimits {
                flags,
                ..DtdLimits::new(limits, decode)
            },
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
        let loader = crate::resolve::Loader::new(base, resolver, &self.limits);
        self.loader = Some(loader.with_flags(self.options.flags));
    }
    pub(crate) fn catalog(&self) -> Option<&crate::dtd_catalog::DtdCatalog> {
        match self.session.first()?.phase.as_ref()? {
            Phase::Content(controller) => Some(controller.catalog()),
            Phase::Declarations { controller, .. } => Some(controller.catalog()),
        }
    }
    /// The parse's budget; absent only after a failed DTD handoff.
    pub(crate) fn budget(&mut self) -> Option<&mut Budget> {
        if let Some(ordinary) = &mut self.ordinary {
            return Some(ordinary.state.owned_budget());
        }
        Some(match self.session.first_mut()?.phase.as_mut()? {
            Phase::Declarations { parser, .. } => parser.owned_budget(),
            Phase::Content(controller) => controller.owned_budget(),
        })
    }

    /// The DTD-free stream while no session delivery or failure is pending.
    pub(crate) fn plain(&mut self) -> Option<&mut Stream> {
        let idle = self.delivery == Delivery::Idle && self.failure.is_none() && !self.finished;
        self.ordinary.as_mut().filter(|_| idle)
    }
    /// Lends the plain stream's ready token, recording what a DTD session needs.
    /// Returns the reader's own result type so the event is not rewrapped.
    pub(crate) fn lend_plain(
        &mut self,
    ) -> Result<Option<(Event<'_>, SourceOrigin)>, crate::ReadError> {
        let ordinary = self.ordinary.as_mut().expect("DTD-free stream");
        let at = ordinary.position();
        let origin = super::token_origin(&ordinary.tokenizer);
        let event = super::parse_event(&ordinary.tokenizer, &mut ordinary.state)
            .and_then(|event| event.ok_or_else(|| Error::new(ErrorKind::InvalidState, at)))
            .map_err(crate::ReadError::Xml)?;
        if let Event::Declaration(declaration) = &event {
            self.standalone = declaration.standalone == Some(true);
        }
        self.origin = Some(origin);
        Ok(Some((event, origin)))
    }
    /// Park one source token; refeed any unconsumed transport suffix after acknowledgment.
    pub(crate) fn prepare(&mut self, input: &[u8]) -> Result<SessionStep, Error> {
        self.remember(|s| {
            if s.finished || (s.finalizing && (!input.is_empty() || s.delivery != Delivery::Ready))
            {
                return Err(Error::new(ErrorKind::InvalidState, s.position()));
            }
            s.prepare_inner(input)
        })
    }
    /// Seal physical EOF while allowing a parked Ready token to be lent once.
    pub(crate) fn finish_step(&mut self) -> Result<SessionStep, Error> {
        self.remember(|s| {
            if s.delivery == Delivery::Lent {
                return Err(Error::new(ErrorKind::InvalidState, s.position()));
            }
            s.finalizing = true;
            s.prepare_inner(&[])
        })
    }
    /// Publish once and lend source storage until the returned borrow ends.
    pub(crate) fn lend(&mut self) -> Result<(Event<'_>, SourceOrigin), Error> {
        if let Some(error) = self.stored_failure() {
            return Err(error);
        }
        if self.delivery != Delivery::Ready {
            let error = Error::new(ErrorKind::InvalidState, self.position());
            self.failure = Some(Failure::Error(error));
            return Err(error);
        }
        self.failure = Some(Failure::Poisoned);
        match lend_current(&mut self.session, &mut self.origin) {
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
    pub(crate) fn acknowledge(&mut self) -> Result<(), Error> {
        self.remember(Self::acknowledge_inner)
    }
    fn acknowledge_inner(&mut self) -> Result<(), Error> {
        if self.delivery != Delivery::Lent {
            return Err(Error::new(ErrorKind::InvalidState, self.position()));
        }
        match self.session[0].phase.as_mut().expect("lent phase") {
            Phase::Declarations { controller, .. } => controller.release(),
            Phase::Content(controller) => controller.acknowledge()?,
        }
        self.delivery = Delivery::Idle;
        Ok(())
    }
    pub(crate) fn advance(
        &mut self,
        input: &[u8],
        emit: &mut impl FnMut(Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<usize, Error> {
        self.remember(|s| {
            if s.finalizing || s.finished {
                return Err(Error::new(ErrorKind::InvalidState, s.position()));
            }
            s.drive(input, emit)
        })
    }
    pub(crate) fn finish(
        &mut self,
        emit: &mut impl FnMut(Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.remember(|s| {
            if s.finished {
                return Ok(());
            }
            if s.delivery == Delivery::Lent {
                return Err(Error::new(ErrorKind::InvalidState, s.position()));
            }
            s.finalizing = true;
            s.drive(&[], emit).map(|_| ())
        })
    }
    fn drive(
        &mut self,
        input: &[u8],
        emit: &mut impl FnMut(Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<usize, Error> {
        let mut consumed = 0;
        if let Some(ordinary) = &mut self.ordinary
            && self.delivery == Delivery::Idle
            && !self.finished
        {
            let (accepted, finished) = ordinary.drive(
                input,
                self.finalizing,
                &mut self.standalone,
                &mut self.origin,
                emit,
            )?;
            match finished {
                Some(finished) => {
                    self.finished = finished;
                    return Ok(accepted);
                }
                // A DOCTYPE token is parked; the session path takes it over.
                None => consumed = accepted,
            }
        }
        loop {
            let step = self.prepare_inner(&input[consumed..])?;
            consumed += step.consumed;
            match step.outcome {
                SessionOutcome::Ready => {
                    let publication = lend_current(&mut self.session, &mut self.origin)?;
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
    fn prepare_inner(&mut self, input: &[u8]) -> Result<SessionStep, Error> {
        let step = |consumed, outcome| Ok(SessionStep { consumed, outcome });
        match self.delivery {
            Delivery::Lent => return Err(Error::new(ErrorKind::InvalidState, self.position())),
            Delivery::Ready => return step(0, SessionOutcome::Ready),
            Delivery::Idle if self.finished => return step(0, SessionOutcome::Complete),
            Delivery::Idle => {}
        }
        // The plain stream comes here only to hand over its parked DOCTYPE token.
        if self.ordinary.is_some() {
            self.start_dtd()?;
        }
        let mut consumed = 0;
        loop {
            let session = self.session.first_mut().expect("one active DTD session");
            match session.drain(self.limits, self.decode)? {
                SessionOutcome::NeedInput => {}
                outcome => {
                    self.delivery = match outcome {
                        SessionOutcome::Ready => Delivery::Ready,
                        _ => Delivery::Idle,
                    };
                    self.finished = outcome == SessionOutcome::Complete;
                    return step(consumed, outcome);
                }
            }
            if self.finalizing {
                session.finish_decoder()?;
                continue;
            }
            let progress = session.advance_decoder(&input[consumed..])?;
            consumed += progress.consumed;
            if !progress.suspended {
                if consumed == input.len() {
                    return step(consumed, SessionOutcome::NeedInput);
                }
                if progress.consumed == 0 {
                    return Err(Error::new(ErrorKind::InvalidState, session.position()));
                }
            }
        }
    }
    fn start_dtd(&mut self) -> Result<(), Error> {
        let ordinary = self.ordinary.as_ref().expect("ordinary phase");
        let (_, raw, span) = ordinary.tokenizer.current().expect("DTD prefix ready");
        if raw != b"<!DOCTYPE" {
            return Err(Error::new(ErrorKind::InvalidState, span.source.start));
        }
        ordinary.state.check_doctype_start(span.source.start)?;
        let encoding = span.encoding;
        let physical_start = span.source.start;
        let decoded_start = span.decoded.start;
        self.session
            .try_reserve_exact(1)
            .map_err(|_| Error::new(ErrorKind::Allocation, physical_start))?;
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
            self.options,
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
            .and_then(|consumed| {
                capture(
                    &mut raw,
                    &prefix[..consumed],
                    state.owned_budget(),
                    physical_start,
                )
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
    fn stored_failure(&mut self) -> Option<Error> {
        let failure = self.failure?;
        let error = match failure {
            Failure::Error(error) => error,
            Failure::Poisoned => Error::new(ErrorKind::InvalidState, self.position()),
        };
        self.failure = Some(Failure::Error(error));
        Some(error)
    }
    fn remember<T>(&mut self, run: impl FnOnce(&mut Self) -> Result<T, Error>) -> Result<T, Error> {
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
    fn drain(&mut self, limits: Limits, decode: DecodeLimits) -> Result<SessionOutcome, Error> {
        loop {
            match self.phase.as_mut().expect("owned semantic phase") {
                Phase::Declarations {
                    controller, parser, ..
                } => {
                    // Only the declaration's end is published; its parts are internal.
                    match controller.current() {
                        Some((DtdEvent::End, _)) => return Ok(SessionOutcome::Ready),
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
                            ));
                        }
                        return Ok(SessionOutcome::NeedInput);
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
                                controller.skip_non_event()?;
                                continue;
                            }
                            return Ok(SessionOutcome::Ready);
                        }
                        ContentOutcome::Progress => {}
                        ContentOutcome::NeedPhysicalInput => {
                            if self.decoder_finished {
                                return Err(Error::new(
                                    ErrorKind::InvalidState,
                                    controller.position().1,
                                ));
                            }
                            return Ok(SessionOutcome::NeedInput);
                        }
                        ContentOutcome::Complete => return Ok(SessionOutcome::Complete),
                    }
                }
            }
        }
    }
    fn advance_decoder(&mut self, input: &[u8]) -> Result<DecodeProgress, Error> {
        let phase = self.phase.as_mut().expect("semantic phase");
        self.decoder
            .advance(input, |decoded| phase_decode(phase, decoded))
    }
    fn finish_decoder(&mut self) -> Result<(), Error> {
        if self.decoder_finished {
            return Ok(());
        }
        let phase = self.phase.as_mut().expect("semantic phase");
        if !self
            .decoder
            .finish_step(|decoded| phase_decode(phase, decoded))?
        {
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
    session: &'event mut [DtdSession],
    last_origin: &mut Option<SourceOrigin>,
) -> Result<LentSession<'event>, Error> {
    let (event, origin, budget) = match session[0].phase.as_mut().expect("ready phase") {
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
    };
    *last_origin = Some(SourceOrigin::from_repr(origin));
    Ok(LentSession {
        event,
        origin,
        budget,
    })
}

fn phase_decode(phase: &mut Phase, decoded: Decoded<'_>) -> Result<DecodeProgress, Error> {
    let Decoded::Bytes(bytes) = decoded else {
        let at = match phase {
            Phase::Declarations { controller, .. } => controller.physical_position(),
            Phase::Content(controller) => controller.position().1,
        };
        return Err(Error::new(ErrorKind::InvalidState, at));
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
}
