use std::io::{self, BufRead};
use std::ops::Range;

use crate::stream::Stream;
use crate::{DecodeLimits, Error, ErrorKind, Event, Limit, Limits, ReadError};

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
/// while let Some((event, range)) = reader.read_event()? {
///     if let Event::Text(text) = event {
///         assert_eq!(text, "text");
///         assert_eq!(range, 3..7);
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
    stream: Stream,
    max_input_bytes: usize,
    eof: bool,
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
            stream: Stream::new(limits, decode_limits),
            max_input_bytes: limits.max_input_bytes,
            eof: false,
            status: Status::Active,
        }
    }

    /// Original bytes consumed, including encoding carry and text lookahead.
    /// Bytes merely prefetched by the underlying source are excluded.
    pub fn position(&self) -> usize {
        self.stream.position()
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
    pub fn read_event(&mut self) -> Result<Option<(Event<'_>, Range<usize>)>, ReadError> {
        match self.status {
            Status::Finished => return Ok(None),
            Status::Xml(error) => return Err(ReadError::Xml(error)),
            Status::Poisoned => {
                return Err(ReadError::Xml(Error::new(
                    ErrorKind::InvalidState,
                    self.position(),
                )));
            }
            Status::Active => {}
        }
        // Poison before releasing tokens, mutating semantic state, or calling
        // the source. A caught panic cannot resume uncertain stream progress.
        self.status = Status::Poisoned;
        self.stream.release();
        match self.prepare() {
            Err(error) => {
                if let ReadError::Xml(xml) = &error {
                    self.status = Status::Xml(*xml);
                }
                return Err(error);
            }
            Ok(false) => {
                self.status = Status::Finished;
                return Ok(None);
            }
            Ok(true) => {}
        }
        match self.stream.event() {
            Ok(event) => {
                self.status = Status::Active;
                Ok(Some(event))
            }
            Err(error) => {
                self.status = Status::Xml(error);
                Err(ReadError::Xml(error))
            }
        }
    }

    /// Locate an event before lending from the token. Non-events cannot hold a
    /// borrow across the loop; their classification belongs to ParserState.
    fn prepare(&mut self) -> Result<bool, ReadError> {
        loop {
            if self.stream.has_token() {
                if self.stream.is_non_event() {
                    self.stream.release();
                    continue;
                }
                return Ok(true);
            }
            if self.eof {
                if self.stream.finish_step().map_err(ReadError::Xml)? {
                    continue;
                }
                self.stream.validate_finish().map_err(ReadError::Xml)?;
                return Ok(false);
            }
            // Replaying saved lookahead or decoded carry need not touch R.
            self.stream.advance(&[]).map_err(ReadError::Xml)?;
            if self.stream.has_token() {
                continue;
            }
            let before = self.position();
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
                let remaining = self.max_input_bytes - before;
                if remaining == 0 {
                    return Err(ReadError::Xml(Error::new(
                        ErrorKind::LimitExceeded(Limit::InputBytes),
                        0,
                    )));
                }
                self.stream.advance(&input[..input.len().min(remaining)])
            };
            // On lexical failure, the decoder/tokenizer positions still record
            // the consumed prefix. Commit it before exposing the XML error.
            let consumed = self.position() - before;
            if consumed > 0 {
                self.input.consume(consumed);
            }
            result.map_err(ReadError::Xml)?;
        }
    }
}
