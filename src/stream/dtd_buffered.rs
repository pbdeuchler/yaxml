//! Private buffered I/O composition over the one-shot DTD lending session.
use std::io::{self, BufRead};

use super::{
    LocatedEvent, SessionError, SessionLimits, SessionOutcome, SessionStep, StreamingSession,
};
use crate::{DecodeLimits, Error, ErrorKind, Limit, Limits, SourceOrigin};

#[derive(Debug)]
pub(crate) enum BufferedSessionError {
    Session(SessionError),
    Io(io::Error),
}

#[derive(Clone, Copy)]
enum Status {
    Active,
    Finished,
    Session(SessionError),
    Poisoned,
}

pub(crate) struct DtdBufferedReader<R: BufRead> {
    input: R,
    session: StreamingSession,
    max_input_bytes: usize,
    eof: bool,
    pending: bool,
    status: Status,
}

impl<R: BufRead> DtdBufferedReader<R> {
    pub(crate) fn new(input: R, session: SessionLimits) -> Self {
        Self::with_limits(input, Limits::default(), session)
    }
    pub(crate) fn with_limits(input: R, limits: Limits, session: SessionLimits) -> Self {
        Self::with_decode_limits(input, limits, DecodeLimits::default(), session)
    }
    pub(crate) fn with_decode_limits(
        input: R,
        limits: Limits,
        decode: DecodeLimits,
        session: SessionLimits,
    ) -> Self {
        Self {
            input,
            session: StreamingSession::new(limits, decode, session),
            max_input_bytes: limits.max_input_bytes,
            eof: false,
            pending: false,
            status: Status::Active,
        }
    }
    pub(crate) fn position(&self) -> usize {
        self.session.position()
    }
    pub(crate) fn event_origin(&self) -> Option<SourceOrigin> {
        self.session.event_origin()
    }
    pub(crate) fn get_ref(&self) -> &R {
        &self.input
    }
    pub(crate) fn into_inner(self) -> R {
        self.input
    }

    pub(crate) fn read_event(
        &mut self,
    ) -> Result<Option<(LocatedEvent<'_>, SourceOrigin)>, BufferedSessionError> {
        match self.status {
            Status::Finished => return Ok(None),
            Status::Session(error) => return Err(BufferedSessionError::Session(error)),
            Status::Poisoned => return Err(self.invalid_state()),
            Status::Active => {}
        }
        // A source or semantic unwind must not resume, even if it follows the
        // acknowledgment of an event returned by the preceding read.
        self.status = Status::Poisoned;
        if self.pending {
            if let Err(error) = self.session.acknowledge() {
                self.status = Status::Session(error);
                return Err(BufferedSessionError::Session(error));
            }
            self.pending = false;
        }
        match self.prepare() {
            Ok(false) => {
                self.status = Status::Finished;
                return Ok(None);
            }
            Ok(true) => {}
            Err(error) => {
                if let BufferedSessionError::Session(session) = &error {
                    self.status = Status::Session(*session);
                }
                return Err(error);
            }
        }
        match self.session.lend() {
            Ok(event) => {
                self.pending = true;
                self.status = Status::Active;
                Ok(Some(event))
            }
            Err(error) => {
                self.status = Status::Session(error);
                Err(BufferedSessionError::Session(error))
            }
        }
    }

    fn prepare(&mut self) -> Result<bool, BufferedSessionError> {
        loop {
            // Drain generated sources, saved lookahead and held DTD End before
            // asking the caller for another transport window.
            let before = self.position();
            let result = if self.eof {
                self.session.finish_step()
            } else {
                self.session.prepare(&[])
            };
            if checked_consumed(before, self.position(), 0, &result).is_none() {
                return Err(self.invalid_state());
            }
            match result.map_err(BufferedSessionError::Session)?.outcome {
                SessionOutcome::Ready => return Ok(true),
                SessionOutcome::Complete => return Ok(false),
                SessionOutcome::NeedInput => {}
            }
            if self.eof {
                return Err(self.invalid_state());
            }
            let before = self.position();
            let (result, offered) = {
                let input = match self.input.fill_buf() {
                    Ok(input) => input,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(BufferedSessionError::Io(error)),
                };
                if input.is_empty() {
                    self.eof = true;
                    continue;
                }
                let remaining = self.max_input_bytes.saturating_sub(before);
                if remaining == 0 {
                    return Err(BufferedSessionError::Session(SessionError::Xml(
                        Error::new(ErrorKind::LimitExceeded(Limit::InputBytes), 0),
                    )));
                }
                let offered = input.len().min(remaining);
                (self.session.prepare(&input[..offered]), offered)
            };
            let consumed = checked_consumed(before, self.position(), offered, &result)
                .ok_or_else(|| self.invalid_state())?;
            // Commit accepted transport before lending or reporting a semantic
            // error. A caught consume panic leaves this adapter poisoned.
            if consumed != 0 {
                self.input.consume(consumed);
            }
            match result.map_err(BufferedSessionError::Session)?.outcome {
                SessionOutcome::Ready => return Ok(true),
                SessionOutcome::Complete => return Ok(false),
                SessionOutcome::NeedInput if consumed == 0 => return Err(self.invalid_state()),
                SessionOutcome::NeedInput => {}
            }
        }
    }
    fn invalid_state(&self) -> BufferedSessionError {
        BufferedSessionError::Session(SessionError::Xml(Error::new(
            ErrorKind::InvalidState,
            self.position(),
        )))
    }
}

/// Session errors have no Step, so their accepted prefix uses the same checked
/// physical-position delta as the public buffered reader's error path.
fn checked_consumed(
    before: usize,
    after: usize,
    offered: usize,
    result: &Result<SessionStep, SessionError>,
) -> Option<usize> {
    let delta = after.checked_sub(before)?;
    let consumed = result.as_ref().map_or(delta, |step| step.consumed);
    (consumed == delta && consumed <= offered).then_some(consumed)
}

#[path = "../../tests/internal/dtd_buffered_author.rs"]
mod tests;
