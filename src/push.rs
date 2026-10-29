use std::ops::{ControlFlow, Range};

use crate::blanks::Filter;
use crate::options::Flags;
use crate::parser::Budget;
use crate::stream::dtd::StreamingSession;
use crate::{DecodeLimits, Error, ErrorKind, Event, Limits, ParseOptions, ReadError, SourceOrigin};

/// An incremental parser fed byte chunks of any size, in bounded memory.
///
/// Input may be in any supported
/// [encoding](https://github.com/pbdeuchler/yaxml/blob/main/docs/encoding.md)
/// or one [`ParseOptions::converters`] decodes, split anywhere. Events do
/// not depend on the split: each text run is one event. Callbacks receive
/// events lent for the call, so copy what you keep. Events before an error
/// are delivered before the error is found.
///
/// ```
/// use std::ops::ControlFlow;
/// use yaxml::{Event, PushParser};
///
/// let xml = b"<root xmlns='urn:example'><item key='v'>text&amp;more</item></root>";
/// let mut parser = PushParser::new();
/// let mut text = String::new();
/// for chunk in xml.chunks(7) {
///     parser.feed(chunk, |event, _origin| {
///         if let Event::Text(value) = event {
///             text.push_str(value);
///         }
///         ControlFlow::Continue(())
///     })?;
/// }
/// parser.finish(|_, _| ControlFlow::Continue(()))?;
/// assert_eq!(text, "text&more");
/// # Ok::<(), yaxml::Error>(())
/// ```
pub struct PushParser {
    session: StreamingSession,
    /// `NOBLANKS`: the callbacks get no ignorable blank text.
    blanks: Option<Box<Filter>>,
}

impl PushParser {
    /// A parser with default options.
    pub fn new() -> Self {
        Self::with_limits(Limits::default())
    }

    /// A parser with custom limits and no flags.
    pub fn with_limits(limits: Limits) -> Self {
        Self::with_decode_limits(limits, DecodeLimits::default())
    }

    /// A parser with custom parse and decode limits and no flags.
    pub fn with_decode_limits(limits: Limits, decode_limits: DecodeLimits) -> Self {
        let options = ParseOptions::default().with_limits(limits);
        Self::with_options(options.with_decode_limits(decode_limits))
            .expect("convenience flags are supported")
    }

    /// A parser with libxml2 parser options; see [`crate::options`].
    ///
    /// # Errors
    ///
    /// Unknown or unimplemented flags, and `DTDVALID` and `XINCLUDE`, which
    /// need a tree, are [`ErrorKind::UnsupportedOption`].
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
        session.set_input(&options);
        if let Some(resolver) = options.resolver {
            session.set_resolver(options.base.as_deref().unwrap_or_default(), resolver);
        }
        let blanks = flags.drops_blanks().then(Box::default);
        Self { session, blanks }
    }

    /// Starts a new document with the same flags and limits, reusing
    /// allocations (`xmlCtxtResetPush`). All other state, the resolver,
    /// converters and forced encoding included, is dropped.
    pub fn reset(&mut self) {
        self.session.reset_same();
        self.blanks = self.blanks.take().map(|_| Box::default());
    }

    /// [`PushParser::reset`] with new options.
    ///
    /// # Errors
    ///
    /// Unsupported options leave the parser unchanged.
    pub fn reset_with(&mut self, options: ParseOptions) -> Result<(), Error> {
        let flags = options.check(false, 0)?;
        let (limits, decode) = options.effective_limits();
        self.session.reset(limits, decode, flags);
        self.blanks = flags.drops_blanks().then(Box::default);
        self.session.set_input(&options);
        if let Some(resolver) = options.resolver {
            self.session
                .set_resolver(options.base.as_deref().unwrap_or_default(), resolver);
        }
        Ok(())
    }

    pub(crate) fn catalog(&self) -> Option<&crate::dtd_catalog::DtdCatalog> {
        self.session.catalog()
    }

    pub(crate) fn take_dtd(&mut self) -> Option<crate::valid::Dtd> {
        self.session.take_dtd()
    }

    /// The declarations of the document's DTD, once its DOCTYPE was read.
    pub fn dtd(&self) -> Option<&crate::valid::Dtd> {
        self.catalog().map(|catalog| catalog.dtd())
    }

    pub(crate) fn budget(&mut self) -> Option<&mut Budget> {
        self.session.budget()
    }

    /// The errors kept so far, in input order (the first 100, as libxml2
    /// keeps): namespace errors and warnings, which never stop a parse
    /// (libxml2's `xmlNsErr`, `ErrorKind::Warning`), and under `RECOVER` every
    /// well-formedness error recovered from. An error no site repairs stops a
    /// recovering parse: what is open closes with `End` events, and later input
    /// is ignored.
    ///
    /// ```
    /// use std::ops::ControlFlow;
    /// use yaxml::{Event, ParseOptions, PushParser, options};
    ///
    /// let mut parser = PushParser::with_options(ParseOptions::new(options::RECOVER))?;
    /// let mut ends = Vec::new();
    /// let mut emit = |event: &Event<'_>, _| {
    ///     if let Event::End(name) = event {
    ///         ends.push(name.qualified().to_owned());
    ///     }
    ///     ControlFlow::Continue(())
    /// };
    /// parser.feed(b"<r><a>text</b>", &mut emit)?;
    /// parser.finish(&mut emit)?;
    /// // `</b>` ends `a`, as in libxml2; the end of input closes `r`.
    /// assert_eq!(ends, ["a", "r"]);
    /// assert_eq!(parser.errors().len(), 2);
    /// # Ok::<(), yaxml::Error>(())
    /// ```
    pub fn errors(&self) -> &[Error] {
        self.session.errors()
    }

    /// Source bytes consumed, lookahead included. After a failure, part of
    /// the last chunk may be unconsumed.
    pub fn position(&self) -> usize {
        self.session.position()
    }

    /// Origin of the latest delivered event; kept after the end or a failure.
    pub fn event_origin(&self) -> Option<SourceOrigin> {
        self.session.event_origin()
    }

    /// Input span of the latest delivered event.
    pub fn event_range(&self) -> Option<Range<usize>> {
        self.event_origin().and_then(SourceOrigin::input_range)
    }

    /// Feeds everything `reader` yields, then finishes. The parser may
    /// already have been fed a prefix. Interrupted reads are retried.
    ///
    /// # Errors
    ///
    /// An earlier error, or `InvalidState` after a finish, is returned
    /// before any I/O.
    pub fn read_from(
        mut self,
        mut input: impl std::io::Read,
        mut emit: impl FnMut(&Event<'_>, SourceOrigin) -> ControlFlow<()>,
    ) -> Result<(), ReadError> {
        if let Some(error) = self.session.terminal() {
            return Err(ReadError::Xml(error));
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

    /// Parses a chunk, calling `emit` for each complete event. An empty
    /// chunk does nothing.
    ///
    /// # Errors
    ///
    /// Errors are sticky. `Break` from `emit` stops with `Aborted`; feeding
    /// after [`finish`](Self::finish) is `InvalidState`. A chunk that would
    /// exceed `max_input_bytes` fails before any of it is parsed.
    pub fn feed(
        &mut self,
        input: &[u8],
        mut emit: impl FnMut(&Event<'_>, SourceOrigin) -> ControlFlow<()>,
    ) -> Result<(), Error> {
        if self.blanks.is_some() {
            return self.feed_filtered(input, emit);
        }
        self.feed_with(input, |event, origin, _| {
            if emit(event, origin).is_break() {
                Err(Error::new(ErrorKind::Aborted, origin.cancellation_offset()))
            } else {
                Ok(())
            }
        })
    }

    /// [`Self::feed`] under `NOBLANKS`.
    #[inline(never)]
    fn feed_filtered(
        &mut self,
        input: &[u8],
        mut emit: impl FnMut(&Event<'_>, SourceOrigin) -> ControlFlow<()>,
    ) -> Result<(), Error> {
        let Self { session, blanks } = self;
        let blanks = blanks.as_deref_mut().expect("NOBLANKS parser");
        session.advance(input, &mut |event, origin, budget| {
            filtered(blanks, event, origin, budget, &mut emit)
        })
    }

    pub(crate) fn feed_with(
        &mut self,
        input: &[u8],
        mut emit: impl FnMut(&Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<(), Error> {
        // The session keeps errors and completion sticky, and poisons itself
        // around each call, so a recovered callback panic cannot reuse
        // partially delivered state.
        self.session.advance(input, &mut emit)
    }

    /// Ends the input, emits pending text and checks the document is
    /// complete. Repeating it is a no-op.
    pub fn finish(
        &mut self,
        mut emit: impl FnMut(&Event<'_>, SourceOrigin) -> ControlFlow<()>,
    ) -> Result<(), Error> {
        if self.blanks.is_some() {
            return self.finish_filtered(emit);
        }
        self.finish_with(|event, origin, _| call(&mut emit, event, origin))
    }

    /// [`Self::finish`] under `NOBLANKS`.
    #[inline(never)]
    fn finish_filtered(
        &mut self,
        mut emit: impl FnMut(&Event<'_>, SourceOrigin) -> ControlFlow<()>,
    ) -> Result<(), Error> {
        let Self { session, blanks } = self;
        let blanks = blanks.as_deref_mut().expect("NOBLANKS parser");
        session
            .finish(&mut |event, origin, budget| filtered(blanks, event, origin, budget, &mut emit))
    }

    pub(crate) fn finish_with(
        &mut self,
        mut emit: impl FnMut(&Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.session.finish(&mut emit)
    }
}

impl Default for PushParser {
    fn default() -> Self {
        Self::new()
    }
}

/// `event` to a public callback, whose `Break` aborts.
#[inline(always)]
fn call(
    emit: &mut impl FnMut(&Event<'_>, SourceOrigin) -> ControlFlow<()>,
    event: &Event<'_>,
    origin: SourceOrigin,
) -> Result<(), Error> {
    match emit(event, origin) {
        ControlFlow::Break(()) => Err(Error::new(ErrorKind::Aborted, origin.cancellation_offset())),
        ControlFlow::Continue(()) => Ok(()),
    }
}

/// [`call`] under `NOBLANKS`: the blank text the filter held and keeps
/// comes first, and ignorable blank text not at all.
#[inline(always)]
fn filtered(
    blanks: &mut Filter,
    event: &Event<'_>,
    origin: SourceOrigin,
    budget: &Budget,
    emit: &mut impl FnMut(&Event<'_>, SourceOrigin) -> ControlFlow<()>,
) -> Result<(), Error> {
    let (kept, passes) = blanks.event(event, origin, budget.content())?;
    if let Some((text, at)) = kept {
        call(emit, &Event::Text(text.into()), at)?;
    }
    match passes {
        true => call(emit, event, origin),
        false => Ok(()),
    }
}
