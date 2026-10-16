use std::ops::{ControlFlow, Range};

use crate::escape::xml_char;
use crate::parser::ParserState;
use crate::tokenizer::{TokenKind, Tokenizer};
use crate::{Error, ErrorKind, Event, Limit, Limits, ReadError};

/// Incremental XML 1.0 parsing with a bounded scratch token and no retained input.
///
/// `feed` accepts arbitrary byte boundaries, including inside UTF-8 and markup.
/// `finish` declares final EOF and validates the complete document. Text is
/// emitted once per lexical run, so event boundaries do not depend on chunks.
/// A callback can borrow its event only for the call; copy values to retain them.
///
/// The scratch token is limited by `max_token_bytes`. Retained names, namespace
/// state, normalization, and owned namespace copies spend `max_owned_bytes`.
/// Streaming consequently needs more of that budget than the borrowing reader.
/// Inactive namespace entries are released when their scope ends.
///
/// Validation occurs as tokens arrive. Earlier events can be delivered before
/// a later invalid byte is discovered, and the first reported error can differ
/// from the slice reader's whole-input UTF-8/character validation.
pub struct PushParser {
    tokenizer: Tokenizer,
    state: ParserState<'static>,
    limits: Limits,
    received: usize,
    range: Range<usize>,
    failure: Option<Error>,
    finished: bool,
}

impl PushParser {
    pub fn new() -> Self {
        Self::with_limits(Limits::default())
    }

    pub fn with_limits(limits: Limits) -> Self {
        Self {
            tokenizer: Tokenizer::new(limits.max_token_bytes),
            state: ParserState::new(limits, 0),
            limits,
            received: 0,
            range: 0..0,
            failure: None,
            finished: false,
        }
    }

    /// Bytes consumed, including encoding detection and one byte of lookahead
    /// when a `<` terminates text. On failure, a chunk's suffix can be unconsumed.
    pub fn position(&self) -> usize {
        self.tokenizer.position()
    }

    /// Raw source span of the latest delivered event, including its delimiters.
    pub fn event_range(&self) -> Range<usize> {
        self.range.clone()
    }

    /// Consumes this parser and a caller-provided reader, delivering events until
    /// final EOF or failure. Interrupted reads are retried; other I/O errors are
    /// preserved. No document-controlled resource is opened by this operation.
    /// Existing state can precede the reader, for example after feeding a prefix.
    /// A terminal parser error is returned before I/O; a successfully finished
    /// parser returns `InvalidState` before touching the source.
    pub fn read_from(
        mut self,
        mut input: impl std::io::Read,
        mut emit: impl FnMut(Event<'_>, Range<usize>) -> ControlFlow<()>,
    ) -> Result<(), ReadError> {
        if let Some(error) = self.failure {
            return Err(ReadError::Xml(error));
        }
        if self.finished {
            return Err(ReadError::Xml(Error::new(
                ErrorKind::InvalidState,
                self.position(),
            )));
        }
        let mut buffer = [0; 8192];
        loop {
            match input.read(&mut buffer) {
                Ok(0) => return self.finish(emit).map_err(ReadError::Xml),
                Ok(length) => {
                    let chunk = buffer.get(..length).ok_or_else(|| {
                        ReadError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "reader returned an oversized byte count",
                        ))
                    })?;
                    self.feed(chunk, &mut emit).map_err(ReadError::Xml)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(ReadError::Io(error)),
            }
        }
    }

    /// Processes a chunk. Empty chunks are no-ops before final EOF.
    ///
    /// Returning `Break(())` from the callback delivers that event, then returns
    /// `Aborted` at its end offset. Every later call returns the same error.
    /// A feed after successful `finish`, including an empty feed, instead starts
    /// a terminal `InvalidState` error. The supplied chunk's full size is checked
    /// against the cumulative input ceiling before processing any of its bytes.
    pub fn feed(
        &mut self,
        input: &[u8],
        mut emit: impl FnMut(Event<'_>, Range<usize>) -> ControlFlow<()>,
    ) -> Result<(), Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.finished {
            return self.remember(Err(Error::new(ErrorKind::InvalidState, self.position())));
        }
        let Some(total) = self
            .received
            .checked_add(input.len())
            .filter(|total| *total <= self.limits.max_input_bytes)
        else {
            return self.remember(Err(Error::new(
                ErrorKind::LimitExceeded(Limit::InputBytes),
                0,
            )));
        };
        self.received = total;
        // A recovered callback panic must not permit reuse of partially
        // delivered tokens or mutated document state.
        self.failure = Some(Error::new(ErrorKind::InvalidState, self.position()));
        let state = &mut self.state;
        let range = &mut self.range;
        let result = self.tokenizer.feed(input, |kind, bytes, source| {
            dispatch(state, range, kind, bytes, source, &mut emit)
        });
        self.remember(result)
    }

    /// Declares final EOF, emits any pending text, and checks document closure.
    /// Repeating a successful finish is harmless and delivers no further events.
    pub fn finish(
        &mut self,
        mut emit: impl FnMut(Event<'_>, Range<usize>) -> ControlFlow<()>,
    ) -> Result<(), Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.finished {
            return Ok(());
        }
        self.failure = Some(Error::new(ErrorKind::InvalidState, self.position()));
        let state = &mut self.state;
        let range = &mut self.range;
        let result = self
            .tokenizer
            .finish(|kind, bytes, source| dispatch(state, range, kind, bytes, source, &mut emit))
            .and_then(|()| self.state.validate_finish(self.position()));
        if result.is_ok() {
            self.finished = true;
        }
        self.remember(result)
    }

    fn remember(&mut self, result: Result<(), Error>) -> Result<(), Error> {
        self.failure = result.err();
        result
    }
}

impl Default for PushParser {
    fn default() -> Self {
        Self::new()
    }
}

fn dispatch(
    state: &mut ParserState<'static>,
    latest: &mut Range<usize>,
    kind: TokenKind,
    bytes: &[u8],
    source: Range<usize>,
    emit: &mut impl FnMut(Event<'_>, Range<usize>) -> ControlFlow<()>,
) -> Result<(), Error> {
    let raw = std::str::from_utf8(bytes)
        .map_err(|e| Error::new(ErrorKind::InvalidUtf8, source.start + e.valid_up_to()))?;
    if let Some((relative, _)) = raw.char_indices().find(|(_, c)| !xml_char(*c)) {
        return Err(Error::new(
            ErrorKind::InvalidCharacter,
            source.start + relative,
        ));
    }
    if let Some(event) = state.process_token(kind, raw, source.clone(), |raw, offset, budget| {
        budget.copy(raw, offset)
    })? {
        *latest = source.clone();
        if emit(event, source.clone()).is_break() {
            return Err(Error::new(ErrorKind::Aborted, source.end));
        }
    }
    Ok(())
}
