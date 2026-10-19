use std::io::{self, BufRead};

use crate::stream::dtd::{SessionError, SessionOutcome, SessionStep, StreamingSession};
use crate::tokenizer::TokenKind;
use crate::{DecodeLimits, Error, ErrorKind, Event, Limit, Limits, ReadError, SourceOrigin};

#[derive(Clone, Copy)]
enum Status {
    Active,
    Finished,
    Xml(Error),
    Poisoned,
}

/// A bounded UTF-8/UTF-16 event reader over caller-provided buffered input.
///
/// Each event can borrow the current token until the next mutable operation.
/// No input is read at construction, and the caller's buffer is not copied in
/// full. Event ranges and positions count original source bytes relative to
/// construction; a text event's terminating `<` is consumed lookahead.
///
/// Read to `Ok(None)` to validate final EOF. Dropping the reader or extracting
/// its input does not drain or validate the remaining document. XML failures
/// are sticky. An I/O error preserves its original cause once; subsequent calls
/// and calls after a recovered source panic return `InvalidState` without I/O.
///
/// ```
/// use yaxml::{BufferedReader, Event};
/// let mut reader = BufferedReader::new(b"<r>text</r>".as_slice());
/// while let Some((event, origin)) = reader.read_event()? {
///     if let Event::Text(text) = event {
///         assert_eq!(text, "text");
///         assert_eq!(origin.input_range(), Some(3..7));
///     }
/// }
/// # Ok::<(), yaxml::ReadError>(())
/// ```
///
/// An event must stop borrowing the reader before it can advance:
///
/// ```compile_fail
/// use yaxml::BufferedReader;
/// let mut reader = BufferedReader::new(b"<r>text</r>".as_slice());
/// let (event, _) = reader.read_event().unwrap().unwrap();
/// reader.read_event().unwrap();
/// println!("{event:?}");
/// ```
pub struct BufferedReader<R: BufRead> {
    input: R,
    session: StreamingSession,
    max_input_bytes: usize,
    eof: bool,
    /// The previously lent event awaits acknowledgment before advancing.
    pending: bool,
    status: Status,
}

impl<R: BufRead> BufferedReader<R> {
    pub fn new(input: R) -> Self {
        Self::with_limits(input, Limits::default())
    }

    pub fn with_limits(input: R, limits: Limits) -> Self {
        Self::with_decode_limits(input, limits, DecodeLimits::default())
    }

    pub fn with_decode_limits(input: R, limits: Limits, decode_limits: DecodeLimits) -> Self {
        Self {
            input,
            session: StreamingSession::new(limits, decode_limits),
            max_input_bytes: limits.max_input_bytes,
            eof: false,
            pending: false,
            status: Status::Active,
        }
    }

    /// Original bytes consumed, including encoding carry and text lookahead.
    /// Bytes merely prefetched by the underlying source are excluded.
    pub fn position(&self) -> usize {
        self.session.position()
    }

    pub fn get_ref(&self) -> &R {
        &self.input
    }

    /// Returns the source at its consumed position, without draining it.
    pub fn into_inner(self) -> R {
        self.input
    }

    /// Advances to one logical event, borrowing the retained decoded token.
    /// Successful EOF repeats `None` without further source operations.
    pub fn read_event(&mut self) -> Result<Option<(Event<'_>, SourceOrigin)>, ReadError> {
        match self.status {
            Status::Finished => return Ok(None),
            Status::Xml(error) => return Err(ReadError::Xml(error)),
            Status::Poisoned => return Err(ReadError::Xml(self.invalid_state())),
            Status::Active => {}
        }
        // Poison before releasing tokens, mutating semantic state, or calling
        // the source. A caught panic cannot resume uncertain stream progress.
        self.status = Status::Poisoned;
        if let Some(stream) = self.session.plain() {
            // DTD-free documents lend straight from the ordinary token loop.
            if std::mem::take(&mut self.pending) {
                stream.release();
            }
            match self.prepare_plain() {
                Ok(Some(true)) => {
                    self.pending = true;
                    self.status = Status::Active;
                    return match self.session.lend_plain() {
                        Ok(event) => Ok(Some(event)),
                        Err(error) => {
                            self.status = Status::Xml(error);
                            Err(ReadError::Xml(error))
                        }
                    };
                }
                Ok(Some(false)) => {
                    self.status = Status::Finished;
                    return Ok(None);
                }
                // A parked DOCTYPE token hands over to the DTD session below.
                Ok(None) => {}
                Err(error) => {
                    if let ReadError::Xml(xml) = &error {
                        self.status = Status::Xml(*xml);
                    }
                    return Err(error);
                }
            }
        }
        let prepared = if self.pending {
            self.pending = false;
            self.session
                .acknowledge()
                .map_err(|error| ReadError::Xml(error.into()))
                .and_then(|()| self.prepare())
        } else {
            self.prepare()
        };
        match prepared {
            Ok(false) => {
                self.status = Status::Finished;
                return Ok(None);
            }
            Ok(true) => {}
            Err(error) => {
                if let ReadError::Xml(xml) = &error {
                    self.status = Status::Xml(*xml);
                }
                return Err(error);
            }
        }
        self.pending = true;
        self.status = Status::Active;
        match self.session.lend() {
            Ok(event) => Ok(Some(event)),
            Err(error) => {
                let error = Error::from(error);
                self.status = Status::Xml(error);
                Err(ReadError::Xml(error))
            }
        }
    }

    /// Drains generated sources and saved lookahead before asking the caller's
    /// source for another window. Accepted bytes are consumed before an error.
    fn prepare(&mut self) -> Result<bool, ReadError> {
        loop {
            let before = self.position();
            let result = if self.eof {
                self.session.finish_step()
            } else {
                self.session.prepare(&[])
            };
            if checked_consumed(before, self.position(), 0, &result).is_none() {
                return Err(ReadError::Xml(self.invalid_state()));
            }
            match result
                .map_err(|error| ReadError::Xml(error.into()))?
                .outcome
            {
                SessionOutcome::Ready => return Ok(true),
                SessionOutcome::Complete => return Ok(false),
                SessionOutcome::NeedInput => {}
            }
            if self.eof {
                return Err(ReadError::Xml(self.invalid_state()));
            }
            let before = self.position();
            let (result, offered) = {
                let input = match self.input.fill_buf() {
                    Ok(input) => input,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(ReadError::Io(error)),
                };
                if input.is_empty() {
                    self.eof = true;
                    continue;
                }
                let remaining = self.max_input_bytes.saturating_sub(before);
                if remaining == 0 {
                    return Err(ReadError::Xml(Error::new(
                        ErrorKind::LimitExceeded(Limit::InputBytes),
                        0,
                    )));
                }
                let offered = input.len().min(remaining);
                (self.session.prepare(&input[..offered]), offered)
            };
            let consumed = checked_consumed(before, self.position(), offered, &result)
                .ok_or_else(|| ReadError::Xml(self.invalid_state()))?;
            // Commit accepted transport before lending or reporting a semantic
            // error. A caught consume panic leaves this reader poisoned.
            if consumed != 0 {
                self.input.consume(consumed);
            }
            match result
                .map_err(|error| ReadError::Xml(error.into()))?
                .outcome
            {
                SessionOutcome::Ready => return Ok(true),
                SessionOutcome::Complete => return Ok(false),
                SessionOutcome::NeedInput if consumed == 0 => {
                    return Err(ReadError::Xml(self.invalid_state()));
                }
                SessionOutcome::NeedInput => {}
            }
        }
    }

    /// The DTD-free counterpart of `prepare`: `Some(ready)` like `prepare`, or
    /// `None` once a DOCTYPE token is parked for the session.
    fn prepare_plain(&mut self) -> Result<Option<bool>, ReadError> {
        loop {
            let stream = self.session.plain().expect("DTD-free stream");
            match stream.token_kind() {
                Some(TokenKind::Dtd) => return Ok(None),
                Some(_) if stream.is_non_event() => {
                    stream
                        .emit(&mut |_, _, _| unreachable!("classified non-event"))
                        .map_err(ReadError::Xml)?;
                    stream.release();
                    continue;
                }
                Some(_) => return Ok(Some(true)),
                None => {}
            }
            if self.eof {
                if stream.finish_step().map_err(ReadError::Xml)? {
                    continue;
                }
                stream.validate_finish().map_err(ReadError::Xml)?;
                return Ok(Some(false));
            }
            // Replaying saved lookahead or decoded carry need not touch R.
            stream.advance(&[]).map_err(ReadError::Xml)?;
            if stream.has_token() {
                continue;
            }
            let before = stream.position();
            let result = {
                let input = match self.input.fill_buf() {
                    Ok(input) => input,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(ReadError::Io(error)),
                };
                if input.is_empty() {
                    self.eof = true;
                    continue;
                }
                let remaining = self.max_input_bytes.saturating_sub(before);
                if remaining == 0 {
                    return Err(ReadError::Xml(Error::new(
                        ErrorKind::LimitExceeded(Limit::InputBytes),
                        0,
                    )));
                }
                stream.advance(&input[..input.len().min(remaining)])
            };
            // Commit accepted transport before reporting a lexical error.
            let consumed = stream.position() - before;
            let stalled = consumed == 0 && !stream.has_token();
            if consumed != 0 {
                self.input.consume(consumed);
            }
            result.map_err(ReadError::Xml)?;
            if stalled {
                return Err(ReadError::Xml(self.invalid_state()));
            }
        }
    }

    fn invalid_state(&self) -> Error {
        Error::new(ErrorKind::InvalidState, self.position())
    }
}

/// Session errors have no Step, so their accepted prefix uses the checked
/// physical-position delta.
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
