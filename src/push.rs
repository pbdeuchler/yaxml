use std::ops::{ControlFlow, Range};

use crate::options::{CONVENIENCE, Flags};
use crate::parser::Budget;
use crate::stream::dtd::StreamingSession;
use crate::{
    DecodeLimits, Error, ErrorKind, Event, Limit, Limits, ParseOptions, ReadError, SourceOrigin,
};

/// Incremental XML 1.0 parsing with a bounded scratch token and no retained input.
///
/// `feed` accepts UTF-8, UTF-16 or a declared single-byte encoding (see
/// `docs/encoding.md`) at arbitrary byte boundaries, including inside
/// a code unit, surrogate pair, or markup delimiter.
/// `finish` declares final EOF and validates the complete document. Text is
/// emitted once per lexical run, so event boundaries do not depend on chunks.
/// A callback can borrow its event only for the call; copy values to retain them.
/// Start-tag attribute lists are lent from storage the parser reuses
/// ([`Attributes::Borrowed`](crate::Attributes)); `into_owned` keeps one.
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
    session: StreamingSession,
    limits: Limits,
    received: usize,
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
        let options = ParseOptions::new(CONVENIENCE).with_limits(limits);
        Self::with_options(options.with_decode_limits(decode_limits))
            .expect("convenience flags are supported")
    }

    /// A parser with libxml2 parser options; see [`crate::options`]. Tree-only
    /// flags (`DTDVALID`, `NOBLANKS`, `XINCLUDE`) are `UnsupportedOption`.
    pub fn with_options(options: ParseOptions) -> Result<Self, Error> {
        let flags = options.check(false, 0)?;
        let (limits, decode) = options.effective_limits();
        Ok(Self::configured(limits, decode, flags, options))
    }

    /// A parser for already checked flags and effective limits.
    pub(crate) fn configured(
        limits: Limits,
        decode: DecodeLimits,
        flags: Flags,
        options: ParseOptions,
    ) -> Self {
        let mut session = StreamingSession::new(limits, decode, flags);
        if let Some(resolver) = options.resolver {
            session.set_resolver(options.base.as_deref().unwrap_or_default(), resolver);
        }
        Self {
            session,
            limits,
            received: 0,
            failure: None,
            finished: false,
        }
    }

    /// Loads the external subset and external parsed entities through
    /// `resolver`, adding `DTDLOAD | DTDATTR | NOENT` to the options; `base` is the
    /// document's URI or path for relative system IDs.
    pub fn with_resolver(mut self, base: &str, resolver: Box<dyn crate::Resolver>) -> Self {
        self.session.enable_loading();
        self.session.set_resolver(base, resolver);
        self
    }

    pub(crate) fn catalog(&self) -> Option<&crate::dtd_catalog::DtdCatalog> {
        self.session.catalog()
    }

    /// Source bytes consumed, including encoding detection and one scalar of lookahead
    /// when a `<` terminates text. On failure, a chunk's suffix can be unconsumed.
    pub fn position(&self) -> usize {
        self.session.position()
    }

    /// Origin of the latest delivered event; absent before the first event.
    /// Stored before the callback, and retained after cancellation, failure or EOF.
    pub fn event_origin(&self) -> Option<SourceOrigin> {
        self.session.event_origin()
    }

    /// Exact original input span of the latest delivered event, when available.
    pub fn event_range(&self) -> Option<Range<usize>> {
        self.event_origin().and_then(SourceOrigin::input_range)
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
        mut emit: impl FnMut(Event<'_>, SourceOrigin) -> ControlFlow<()>,
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
        mut emit: impl FnMut(Event<'_>, SourceOrigin) -> ControlFlow<()>,
    ) -> Result<(), Error> {
        self.feed_with(input, |event, origin, _| {
            if emit(event, origin).is_break() {
                Err(Error::new(ErrorKind::Aborted, origin.cancellation_offset()))
            } else {
                Ok(())
            }
        })
    }

    pub(crate) fn feed_with(
        &mut self,
        input: &[u8],
        mut emit: impl FnMut(Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
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
        let result = self.session.advance(input, &mut emit).map(drop);
        self.remember(result)
    }

    /// Declares final EOF, emits any pending text, and checks document closure.
    /// Repeating a successful finish is harmless and delivers no further events.
    pub fn finish(
        &mut self,
        mut emit: impl FnMut(Event<'_>, SourceOrigin) -> ControlFlow<()>,
    ) -> Result<(), Error> {
        self.finish_with(|event, origin, _| {
            if emit(event, origin).is_break() {
                Err(Error::new(ErrorKind::Aborted, origin.cancellation_offset()))
            } else {
                Ok(())
            }
        })
    }

    pub(crate) fn finish_with(
        &mut self,
        mut emit: impl FnMut(Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.finished {
            return Ok(());
        }
        self.failure = Some(Error::new(ErrorKind::InvalidState, self.position()));
        let result = self.session.finish(&mut emit);
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
