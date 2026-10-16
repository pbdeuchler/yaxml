use std::ops::{ControlFlow, Range};

use crate::encoding::{Decoded, Decoder, TokenSpan};
use crate::escape::xml_char;
use crate::parser::{Budget, ParserState};
use crate::tokenizer::{TokenKind, Tokenizer};
use crate::{DecodeLimits, Error, ErrorKind, Event, Limit, Limits, ReadError};

/// Incremental XML 1.0 parsing with a bounded scratch token and no retained input.
///
/// `feed` accepts UTF-8 or UTF-16 at arbitrary byte boundaries, including inside
/// a code unit, surrogate pair, or markup delimiter.
/// `finish` declares final EOF and validates the complete document. Text is
/// emitted once per lexical run, so event boundaries do not depend on chunks.
/// A callback can borrow its event only for the call; copy values to retain them.
///
/// Source tokens are limited by `max_token_bytes`; decoded scratch tokens have
/// the separate `DecodeLimits` ceiling. Retained names, namespace
/// state, normalization, and owned namespace copies spend `max_owned_bytes`.
/// Streaming consequently needs more of that budget than the borrowing reader.
/// Inactive namespace entries are released when their scope ends.
///
/// Validation occurs as tokens arrive. Earlier events can be delivered before
/// a later invalid byte is discovered, and the first reported error can differ
/// from the slice reader's whole-input UTF-8/character validation.
pub struct PushParser {
    tokenizer: Tokenizer,
    decoder: Decoder,
    decode_limits: DecodeLimits,
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
        Self::with_decode_limits(limits, DecodeLimits::default())
    }

    /// Parses UTF-8 or UTF-16 with independent source and decoded ceilings.
    pub fn with_decode_limits(limits: Limits, decode_limits: DecodeLimits) -> Self {
        Self {
            tokenizer: Tokenizer::new(
                limits.max_token_bytes,
                decode_limits.max_decoded_token_bytes,
            ),
            decoder: Decoder::new(decode_limits.max_decoded_bytes),
            decode_limits,
            state: ParserState::new(limits, 0),
            limits,
            received: 0,
            range: 0..0,
            failure: None,
            finished: false,
        }
    }

    /// Source bytes consumed, including encoding detection and one scalar of lookahead
    /// when a `<` terminates text. On failure, a chunk's suffix can be unconsumed.
    pub fn position(&self) -> usize {
        self.tokenizer
            .source_position()
            .max(self.decoder.position())
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
        self.feed_with(input, |event, range, _| {
            if emit(event, range.clone()).is_break() {
                Err(Error::new(ErrorKind::Aborted, range.end))
            } else {
                Ok(())
            }
        })
    }

    pub(crate) fn feed_with(
        &mut self,
        input: &[u8],
        mut emit: impl FnMut(Event<'_>, Range<usize>, &mut Budget) -> Result<(), Error>,
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
        let tokenizer = &mut self.tokenizer;
        let decoded_limit = self.decode_limits.max_decoded_token_bytes;
        let result = self.decoder.feed(input, |decoded| {
            decode(tokenizer, state, range, decoded, decoded_limit, &mut emit)
        });
        self.remember(result)
    }

    /// Declares final EOF, emits any pending text, and checks document closure.
    /// Repeating a successful finish is harmless and delivers no further events.
    pub fn finish(
        &mut self,
        mut emit: impl FnMut(Event<'_>, Range<usize>) -> ControlFlow<()>,
    ) -> Result<(), Error> {
        self.finish_with(|event, range, _| {
            if emit(event, range.clone()).is_break() {
                Err(Error::new(ErrorKind::Aborted, range.end))
            } else {
                Ok(())
            }
        })
    }

    pub(crate) fn finish_with(
        &mut self,
        mut emit: impl FnMut(Event<'_>, Range<usize>, &mut Budget) -> Result<(), Error>,
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
        let tokenizer = &mut self.tokenizer;
        let decoded_limit = self.decode_limits.max_decoded_token_bytes;
        let result = self
            .decoder
            .finish(|decoded| decode(tokenizer, state, range, decoded, decoded_limit, &mut emit))
            .and_then(|()| {
                tokenizer.finish(|kind, bytes, source| {
                    dispatch(state, range, kind, bytes, source, &mut emit)
                })
            })
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
    span: TokenSpan,
    emit: &mut impl FnMut(Event<'_>, Range<usize>, &mut Budget) -> Result<(), Error>,
) -> Result<(), Error> {
    // An invalid markup prefix can be recognized on a UTF-8 leading byte,
    // before the scanner has retained the scalar's continuation bytes. The
    // shared parser rejects this kind without inspecting its payload.
    let raw = if matches!(kind, TokenKind::InvalidMarkup) {
        ""
    } else {
        std::str::from_utf8(bytes)
            .map_err(|e| Error::new(ErrorKind::InvalidUtf8, span.source.start + e.valid_up_to()))?
    };
    if let Some((relative, _)) = raw.char_indices().find(|(_, c)| !xml_char(*c)) {
        return Err(span.map_error(
            raw,
            Error::new(ErrorKind::InvalidCharacter, span.decoded.start + relative),
        ));
    }
    if let Some(event) = state
        .process_token(kind, raw, span.decoded.clone(), |raw, offset, budget| {
            budget.copy(raw, offset)
        })
        .map_err(|error| span.map_error(raw, error))?
    {
        *latest = span.source.clone();
        emit(event, span.source, state.owned_budget())?;
    }
    Ok(())
}

fn decode(
    tokenizer: &mut Tokenizer,
    state: &mut ParserState<'static>,
    latest: &mut Range<usize>,
    decoded: Decoded<'_>,
    decoded_limit: usize,
    emit: &mut impl FnMut(Event<'_>, Range<usize>, &mut Budget) -> Result<(), Error>,
) -> Result<(), Error> {
    match decoded {
        Decoded::Start { encoding, bom } => {
            tokenizer.configure(encoding, bom);
            let initial = if encoding == crate::encoding::SourceEncoding::Utf8 {
                bom
            } else {
                0
            };
            state.set_encoding(encoding, initial, decoded_limit);
            Ok(())
        }
        Decoded::Bytes(bytes) => tokenizer.feed(bytes, |kind, bytes, span| {
            dispatch(state, latest, kind, bytes, span, emit)
        }),
    }
}
