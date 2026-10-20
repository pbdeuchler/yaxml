//! Streaming composition of the ordinary token loop with DTD and entity phases.
use super::*;
use crate::Doctype;
use crate::dtd::DtdLimits;
use crate::dtd_controller::content::{ContentController, ContentOutcome};
use crate::dtd_controller::{DtdController, DtdEvent};
use crate::dtd_frames::FragmentOrigin;
use crate::options::Flags;

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
    /// Decoded input a recovered declaration read past where the document
    /// resumes, fed to the content phase before further input.
    replay: Vec<u8>,
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
#[derive(Clone, Copy)]
enum Failure {
    Error(Error),
    Poisoned,
}

/// Length-zero/one slot keeps the large DTD/body state off the ordinary stack.
/// `advance` counts pushed chunks against `max_input_bytes`; pull callers
/// (`prepare`) bound their own reads.
pub(crate) struct StreamingSession {
    ordinary: Option<Stream>,
    session: Vec<DtdSession>,
    limits: Limits,
    decode: DecodeLimits,
    options: DtdLimits,
    /// Transport bytes supplied to `advance`, against `max_input_bytes`.
    received: usize,
    standalone: bool,
    loader: Option<crate::resolve::Loader>,
    origin: Option<SourceOrigin>,
    physical: usize,
    finalizing: bool,
    finished: bool,
    failure: Option<Failure>,
}

impl StreamingSession {
    pub(crate) fn new(limits: Limits, decode: DecodeLimits, flags: Flags) -> Self {
        Self::with_stream(Stream::new(limits, decode, flags), limits, decode, flags)
    }
    fn with_stream(ordinary: Stream, limits: Limits, decode: DecodeLimits, flags: Flags) -> Self {
        Self {
            ordinary: Some(ordinary),
            session: Vec::new(),
            limits,
            decode,
            options: DtdLimits {
                flags,
                ..DtdLimits::new(limits, decode)
            },
            received: 0,
            standalone: false,
            loader: None,
            origin: None,
            physical: 0,
            finalizing: false,
            finished: false,
            failure: None,
        }
    }
    /// Starts over with new limits and flags, keeping only the plain stream's
    /// emptied allocations: counters, failure, DTD phase and loader are gone.
    pub(crate) fn reset(&mut self, limits: Limits, decode: DecodeLimits, flags: Flags) {
        let stream = match self.ordinary.take() {
            Some(stream) => stream.recycled(limits, decode, flags),
            None => Stream::new(limits, decode, flags),
        };
        *self = Self::with_stream(stream, limits, decode, flags);
    }
    /// Starts over with other flags, keeping the resolver: only before input.
    pub(crate) fn set_flags(&mut self, flags: Flags) {
        let loader = self.loader.take();
        self.reset(self.limits, self.decode, flags);
        self.loader = loader.map(|loader| loader.with_flags(flags));
    }
    /// [`Self::reset`] with the current limits and flags.
    pub(crate) fn reset_same(&mut self) {
        self.reset(self.limits, self.decode, self.options.flags);
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
    /// The finished document's declarations, moved out of the session.
    pub(crate) fn take_dtd(&mut self) -> Option<crate::valid::Dtd> {
        let catalog = match self.session.first_mut()?.phase.as_mut()? {
            Phase::Content(controller) => controller.catalog_mut(),
            Phase::Declarations { controller, .. } => controller.catalog_mut(),
        };
        Some(catalog.take_dtd())
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

    /// The DTD-free stream, before a DOCTYPE hands over to the session.
    pub(crate) fn plain(&mut self) -> Option<&mut Stream> {
        self.ordinary.as_mut()
    }
    /// Lends the plain stream's ready token, recording what a DTD session needs.
    /// Returns the reader's own result type so the event is not rewrapped.
    pub(crate) fn lend_plain(
        &mut self,
    ) -> Result<Option<(Event<'_>, SourceOrigin)>, crate::ReadError> {
        let ordinary = self.ordinary.as_mut().expect("DTD-free stream");
        let at = ordinary.position();
        let origin = super::token_origin(&ordinary.tokenizer);
        let mut event = super::parse_event(&ordinary.tokenizer, &mut ordinary.state)
            .and_then(|event| event.ok_or_else(|| Error::new(ErrorKind::InvalidState, at)))
            .map_err(crate::ReadError::Xml)?;
        ordinary.state.lend_event(&mut event);
        if let Event::Declaration(declaration) = &event {
            self.standalone = declaration.standalone == Some(true);
        }
        self.origin = Some(origin);
        Ok(Some((event, origin)))
    }
    /// Park one source token; refeed any unconsumed transport suffix after acknowledgment.
    pub(crate) fn prepare(&mut self, input: &[u8]) -> Result<SessionStep, Error> {
        self.remember(|s| s.prepare_inner(input))
    }
    /// Seal physical EOF while allowing a parked Ready token to be lent once.
    pub(crate) fn finish_step(&mut self) -> Result<SessionStep, Error> {
        self.remember(|s| {
            s.finalizing = true;
            s.prepare_inner(&[])
        })
    }
    /// Publish once and lend source storage until the returned borrow ends.
    pub(crate) fn lend(&mut self) -> Result<(Event<'_>, SourceOrigin), Error> {
        self.failure = Some(Failure::Poisoned);
        match lend_current(&mut self.session, &mut self.origin) {
            Ok(publication) => {
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
        match self.session[0].phase.as_mut().expect("lent phase") {
            Phase::Declarations { controller, .. } => controller.release(),
            Phase::Content(controller) => controller.acknowledge(),
        }
        Ok(())
    }
    /// The stored error, or `InvalidState` once finished, if feeding would fail.
    pub(crate) fn terminal(&mut self) -> Option<Error> {
        self.stored_failure().or_else(|| {
            self.finalizing
                .then(|| Error::new(ErrorKind::InvalidState, self.position()))
        })
    }
    /// Pushes a whole chunk, checked against the cumulative input ceiling
    /// before any of its bytes is processed.
    pub(crate) fn advance(
        &mut self,
        input: &[u8],
        emit: &mut impl FnMut(&Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.remember(|s| {
            if s.finalizing {
                return Err(Error::new(ErrorKind::InvalidState, s.position()));
            }
            s.received = s
                .received
                .checked_add(input.len())
                .filter(|total| *total <= s.limits.max_input_bytes)
                .ok_or(Error::new(
                    ErrorKind::LimitExceeded(crate::Limit::InputBytes),
                    0,
                ))?;
            s.drive(input, emit).map(drop)
        })
    }
    pub(crate) fn finish(
        &mut self,
        emit: &mut impl FnMut(&Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<(), Error> {
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
        emit: &mut impl FnMut(&Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<usize, Error> {
        let mut consumed = 0;
        if let Some(ordinary) = &mut self.ordinary {
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
            consumed += self.drive_chunk(&input[consumed..], emit)?;
            let step = self.prepare_inner(&input[consumed..])?;
            consumed += step.consumed;
            match step.outcome {
                SessionOutcome::Ready => {
                    let publication = lend_current(&mut self.session, &mut self.origin)?;
                    emit(
                        &publication.event,
                        SourceOrigin::from_repr(publication.origin),
                        publication.budget,
                    )?;
                    self.acknowledge_inner()?;
                }
                SessionOutcome::NeedInput | SessionOutcome::Complete => return Ok(consumed),
            }
        }
    }
    /// Content in plain UTF-8 chunks read in place, as the DTD-free stream
    /// reads it (see `ContentControllerCore::drive_chunk`).
    fn drive_chunk(
        &mut self,
        input: &[u8],
        emit: &mut impl FnMut(&Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<usize, Error> {
        let Some(session) = self.session.first_mut().filter(|_| !self.finalizing) else {
            return Ok(0);
        };
        let (Some(Phase::Content(controller)), Some(allowance)) =
            (&mut session.phase, session.decoder.utf8_allowance())
        else {
            return Ok(0);
        };
        if !session.replay.is_empty() || session.decoder_finished {
            return Ok(0);
        }
        let (decoder, origin) = (&mut session.decoder, &mut self.origin);
        controller.drive_chunk(
            &input[..input.len().min(allowance)],
            &mut |length| decoder.skip_utf8(length),
            &mut |event, at, budget| {
                *origin = Some(at);
                emit(event, at, budget)
            },
        )
    }

    fn prepare_inner(&mut self, input: &[u8]) -> Result<SessionStep, Error> {
        let step = |consumed, outcome| Ok(SessionStep { consumed, outcome });
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
                    self.finished = outcome == SessionOutcome::Complete;
                    return step(consumed, outcome);
                }
            }
            if self.finalizing {
                session.finish_decoder()?;
                continue;
            }
            // The decoder takes all of its input unless the phase suspends.
            let progress = session.advance_decoder(&input[consumed..])?;
            consumed += progress.consumed;
            if !progress.suspended {
                return step(consumed, SessionOutcome::NeedInput);
            }
        }
    }
    fn start_dtd(&mut self) -> Result<(), Error> {
        let ordinary = self.ordinary.as_ref().expect("ordinary phase");
        let (_, _, span) = ordinary.tokenizer.current().expect("DTD prefix ready");
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
        // The DTD session delivers events it does not lend.
        state.stop_lending();
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
            replay: Vec::new(),
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
                            controller,
                            parser,
                            mut raw,
                        } = self.phase.take().expect("declaration owner")
                        else {
                            unreachable!("checked declaration phase")
                        };
                        let first = controller.decoded_position() - raw.len();
                        let body = controller.into_body_state()?;
                        self.replay = raw.split_off(body.resume_position().0 - first);
                        let content = ContentController::new(
                            body,
                            parser,
                            Tokenizer::new(limits.max_token_bytes, decode.max_decoded_token_bytes),
                        )?;
                        self.phase = Some(Phase::Content(content));
                        continue;
                    }
                    let result = match self.decoder_finished {
                        true => controller.finish_step(parser.owned_budget()),
                        false => controller.advance(&[], parser.owned_budget()).map(drop),
                    };
                    if let Err(error) = result {
                        controller.recover(error, parser.owned_budget())?;
                    }
                    if controller.current().is_none() && !controller.is_complete() {
                        return Ok(SessionOutcome::NeedInput);
                    }
                }
                Phase::Content(controller) => {
                    let step = if !self.replay.is_empty() {
                        let step = controller.advance(&self.replay)?;
                        self.replay.drain(..step.consumed);
                        step
                    } else if self.decoder_finished {
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
                        ContentOutcome::NeedPhysicalInput => return Ok(SessionOutcome::NeedInput),
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
    /// Drains the decoder at EOF; a phase still wanting input after that
    /// has stalled.
    fn finish_decoder(&mut self) -> Result<(), Error> {
        if self.decoder_finished {
            return Err(Error::new(ErrorKind::InvalidState, self.position()));
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
            parser.commit_doctype();
            // A recovered declaration can end inside a character.
            let first = controller.decoded_position() - raw.len();
            let valid = std::str::from_utf8(raw).map_or_else(|e| e.valid_up_to(), str::len);
            let raw = std::str::from_utf8(&raw[..valid]).expect("valid UTF-8 prefix");
            let (doctype, range) = match controller.cut() {
                None => (Doctype::parse(raw), range),
                Some((unit, at)) => {
                    let (encoding, start) = (controller.encoding(), range.start);
                    let index = |at| decoded_index(raw, encoding, start, at);
                    let (doctype, resume) =
                        crate::reader::recovered_doctype(raw, unit.map(index), index(at));
                    let physical = start + encoding.source_length(&raw.as_bytes()[..resume]);
                    controller.resume_at(first + resume, physical);
                    (doctype, start..physical)
                }
            };
            (
                Event::Doctype(doctype),
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

/// The index in `raw`, decoded from physical offset `start`, of physical
/// offset `at`.
fn decoded_index(raw: &str, encoding: SourceEncoding, start: usize, at: usize) -> usize {
    let mut physical = start;
    raw.char_indices()
        .find(|&(index, character)| {
            let reached = physical >= at;
            physical +=
                encoding.source_length(&raw.as_bytes()[index..index + character.len_utf8()]);
            reached
        })
        .map_or(raw.len(), |(index, _)| index)
}

fn phase_decode(phase: &mut Phase, decoded: Decoded<'_>) -> Result<DecodeProgress, Error> {
    // The encoding was detected before the DOCTYPE token: only bytes follow.
    let bytes = match decoded {
        Decoded::Bytes(bytes) => bytes,
        Decoded::Start { .. } => &[],
    };
    match phase {
        Phase::Declarations {
            controller,
            parser,
            raw,
        } => {
            // Recovery replays what the scanner consumed before an error.
            let before = controller.decoded_position();
            let result = controller.advance(bytes, parser.owned_budget());
            let consumed = controller.decoded_position() - before;
            capture(
                raw,
                &bytes[..consumed],
                parser.owned_budget(),
                controller.physical_position(),
            )?;
            if let Err(error) = result {
                controller.recover(error, parser.owned_budget())?;
            }
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
