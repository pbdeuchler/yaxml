//! Stable UTF-8 DTD events borrow only the constructor's immutable input.
use super::*;
use crate::DecodeLimits;
use crate::dtd_controller::content::{BorrowedContentController, ContentOutcome};
use crate::dtd_controller::{BorrowedDtdController, DtdController, TakenDtdEvent};
use crate::stream::dtd::{SessionError, SessionLimits};
use crate::tokenizer::Tokenizer;

#[derive(Debug)]
pub(crate) enum ReaderEvent<'input> {
    Xml(Event<'input>),
    Dtd(TakenDtdEvent<'input>),
}

#[expect(
    clippy::large_enum_variant,
    reason = "the optional phase occupies one fallibly reserved out-of-line slot"
)]
enum Phase<'input> {
    Declarations {
        controller: BorrowedDtdController<'input>,
        parser: ParserState<'input>,
    },
    Content(BorrowedContentController<'input>),
}
#[derive(Clone, Copy)]
enum Failure {
    Error(SessionError),
    Poisoned,
}

pub(crate) struct DtdReader<'input> {
    source: &'input str,
    ordinary: Option<Reader<'input>>,
    session: Vec<Phase<'input>>,
    limits: Limits,
    decode: DecodeLimits,
    options: SessionLimits,
    standalone: bool,
    origin: Option<SourceOrigin>,
    physical: usize,
    failure: Option<Failure>,
    finished: bool,
}

impl<'input> DtdReader<'input> {
    pub(crate) fn new(input: &'input [u8], options: SessionLimits) -> Result<Self, SessionError> {
        Self::with_limits(input, Limits::default(), options)
    }
    pub(crate) fn with_limits(
        input: &'input [u8],
        limits: Limits,
        options: SessionLimits,
    ) -> Result<Self, SessionError> {
        Self::with_decode_limits(input, limits, DecodeLimits::default(), options)
    }
    pub(crate) fn with_decode_limits(
        input: &'input [u8],
        limits: Limits,
        decode: DecodeLimits,
        options: SessionLimits,
    ) -> Result<Self, SessionError> {
        let ordinary = Reader::with_decode_limits(input, limits, decode)?;
        Ok(Self {
            source: ordinary.input,
            physical: ordinary.position(),
            ordinary: Some(ordinary),
            session: Vec::new(),
            limits,
            decode,
            options,
            standalone: false,
            origin: None,
            failure: None,
            finished: false,
        })
    }
    /// Ordinary Reader cursor before DTD; accepted primary-source high-water after it.
    pub(crate) fn position(&self) -> usize {
        if let Some(ordinary) = &self.ordinary {
            return ordinary.position();
        }
        match self.session.first() {
            Some(Phase::Declarations { controller, .. }) => controller.physical_position(),
            Some(Phase::Content(controller)) => controller.position().1,
            None => self.physical,
        }
    }
    pub(crate) fn event_origin(&self) -> Option<SourceOrigin> {
        self.origin
    }
    pub(crate) fn event_range(&self) -> Option<Range<usize>> {
        self.origin.and_then(SourceOrigin::input_range)
    }
    pub(crate) fn into_inner(self) -> &'input [u8] {
        self.source.as_bytes()
    }

    pub(crate) fn read_event(
        &mut self,
    ) -> Result<Option<(ReaderEvent<'input>, SourceOrigin)>, SessionError> {
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
        if self.finished {
            return Ok(None);
        }
        self.failure = Some(Failure::Poisoned);
        let result = self.read_next();
        match &result {
            Ok(event) => {
                if let Some((_, origin)) = event {
                    self.origin = Some(*origin);
                }
                self.failure = None;
            }
            Err(error) => self.failure = Some(Failure::Error(*error)),
        }
        result
    }
    fn read_next(&mut self) -> Result<Option<(ReaderEvent<'input>, SourceOrigin)>, SessionError> {
        loop {
            if let Some(ordinary) = &mut self.ordinary {
                match ordinary.read_step::<true>()? {
                    ReaderStep::Xml(event) => {
                        if let Event::Declaration(declaration) = &event {
                            self.standalone = declaration.standalone == Some(true);
                        }
                        let origin = ordinary.event_origin().expect("returned ordinary origin");
                        return Ok(Some((ReaderEvent::Xml(event), origin)));
                    }
                    ReaderStep::Doctype(start) => {
                        self.start_dtd(start)?;
                        continue;
                    }
                    ReaderStep::Complete => {
                        self.finished = true;
                        return Ok(None);
                    }
                }
            }
            if matches!(self.session.first(), Some(Phase::Declarations { controller, .. }) if controller.is_complete())
            {
                self.start_content()?;
            }
            match self.session.first_mut().expect("active DTD phase") {
                Phase::Declarations { controller, parser } => {
                    while controller.current().is_none() {
                        controller.advance_borrowed(parser.owned_budget())?;
                    }
                    let (_, origin) = controller.current().expect("ready declaration");
                    parser.check_event_limit(origin.error_offset())?;
                    parser.commit_checked_dtd_event();
                    let (event, origin) = controller.take_current().expect("checked declaration");
                    return Ok(Some((
                        ReaderEvent::Dtd(event),
                        SourceOrigin::from_repr(origin),
                    )));
                }
                Phase::Content(controller) => match controller.advance_borrowed()?.outcome {
                    ContentOutcome::EventReady => {
                        let (event, origin) = controller.take_event()?;
                        if let Some(event) = event {
                            return Ok(Some((ReaderEvent::Xml(event), origin)));
                        }
                    }
                    ContentOutcome::Progress => {}
                    ContentOutcome::Complete => {
                        self.finished = true;
                        return Ok(None);
                    }
                    ContentOutcome::NeedPhysicalInput => {
                        return Err(
                            Error::new(ErrorKind::InvalidState, controller.position().1).into()
                        );
                    }
                },
            }
        }
    }
    fn start_dtd(&mut self, start: usize) -> Result<(), SessionError> {
        self.session
            .try_reserve_exact(1)
            .map_err(|_| SessionError::Allocation {
                physical_offset: start,
            })?;
        let ordinary = self.ordinary.take().expect("checked ordinary owner");
        self.physical = ordinary.position();
        let controller = DtdController::stable_utf8(
            self.source,
            0,
            start,
            self.options.declarations,
            self.standalone,
        )?;
        self.session.push(Phase::Declarations {
            controller,
            parser: ordinary.state,
        });
        Ok(())
    }
    fn start_content(&mut self) -> Result<(), SessionError> {
        self.physical = self.position();
        let Phase::Declarations { controller, parser } = self.session.pop().expect("completed DTD")
        else {
            unreachable!("declaration handoff")
        };
        let body = controller.into_body_state()?;
        let tokenizer = Tokenizer::new(
            self.limits.max_token_bytes,
            self.decode.max_decoded_token_bytes,
        );
        let controller = BorrowedContentController::stable_utf8(
            body,
            parser,
            tokenizer,
            self.source,
            self.options.text,
            self.options.local,
        )?;
        self.session.push(Phase::Content(controller));
        Ok(())
    }
}

#[path = "../../tests/internal/dtd_reader_author.rs"]
mod tests;
