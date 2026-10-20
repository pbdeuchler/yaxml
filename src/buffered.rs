use std::io::{self, BufRead};

use std::collections::VecDeque;

use crate::blanks::Filter;
use crate::options::Flags;
use crate::stream::dtd::{SessionOutcome, StreamingSession};
use crate::tokenizer::TokenKind;
use crate::{
    DecodeLimits, Error, ErrorKind, Event, Limit, Limits, ParseOptions, ReadError, SourceOrigin,
};

#[derive(Clone, Copy)]
enum Status {
    Active,
    /// Under `RECOVER`, after an error no site repairs: what is open closes
    /// at its offset.
    Closing(usize),
    Finished,
    Xml(Error),
    Poisoned,
}

/// A pull parser over any [`BufRead`], in bounded memory.
///
/// Events borrow the reader until the next read. Input is read lazily and
/// consumed only as far as the parser needs (a text event's closing `<` is
/// lookahead), so the rest stays in the source. Ranges and positions count
/// source bytes from construction. Only reading to `Ok(None)` checks the
/// document is complete. XML errors are sticky; an I/O error is returned
/// once, then `InvalidState`. See the
/// [streaming contract](https://github.com/pbdeuchler/yaxml/blob/main/docs/streaming.md).
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
    /// `RECOVER` or `NOBLANKS`: events are copied out of the session.
    detached: Option<Box<Detached>>,
}

impl<R: BufRead> BufferedReader<R> {
    /// A reader with default options.
    pub fn new(input: R) -> Self {
        Self::with_limits(input, Limits::default())
    }

    /// A reader with custom limits and no flags.
    pub fn with_limits(input: R, limits: Limits) -> Self {
        Self::with_decode_limits(input, limits, DecodeLimits::default())
    }

    /// A reader with custom parse and decode limits and no flags.
    pub fn with_decode_limits(input: R, limits: Limits, decode_limits: DecodeLimits) -> Self {
        let options = ParseOptions::default().with_limits(limits);
        Self::with_options(input, options.with_decode_limits(decode_limits))
            .expect("convenience flags are supported")
    }

    /// A reader with libxml2 parser options; see [`crate::options`].
    ///
    /// # Errors
    ///
    /// `DTDVALID` and `XINCLUDE`, which need a tree, are
    /// [`ErrorKind::UnsupportedOption`].
    pub fn with_options(input: R, options: ParseOptions) -> Result<Self, Error> {
        let flags = options.check(false, 0)?;
        let (limits, decode) = options.effective_limits();
        let mut session = StreamingSession::new(limits, decode, flags);
        if let Some(resolver) = options.resolver {
            session.set_resolver(options.base.as_deref().unwrap_or_default(), resolver);
        }
        Ok(Self {
            input,
            session,
            max_input_bytes: limits.max_input_bytes,
            eof: false,
            pending: false,
            status: Status::Active,
            detached: Detached::new(flags),
        })
    }

    /// Starts a new document on `input` with the same flags and limits,
    /// reusing allocations (`xmlCtxtReset`), and returns the previous source.
    /// All other state, the resolver included, is dropped.
    pub fn reset(&mut self, input: R) -> R {
        self.session.reset_same();
        self.restart(input)
    }

    /// [`BufferedReader::reset`] with new options.
    ///
    /// # Errors
    ///
    /// Unsupported options leave the reader unchanged and drop `input`.
    pub fn reset_with(&mut self, input: R, options: ParseOptions) -> Result<R, Error> {
        let flags = options.check(false, 0)?;
        let (limits, decode) = options.effective_limits();
        self.session.reset(limits, decode, flags);
        if let Some(resolver) = options.resolver {
            self.session
                .set_resolver(options.base.as_deref().unwrap_or_default(), resolver);
        }
        self.max_input_bytes = limits.max_input_bytes;
        Ok(self.restart(input))
    }

    fn restart(&mut self, input: R) -> R {
        self.detached = Detached::new(self.session.flags());
        self.eof = false;
        self.pending = false;
        self.status = Status::Active;
        std::mem::replace(&mut self.input, input)
    }

    /// The well-formedness errors a `RECOVER` parse recovered from so far, in
    /// input order (the first 100, as libxml2 keeps); empty without `RECOVER`.
    pub fn errors(&self) -> &[Error] {
        self.session.errors()
    }

    /// Source bytes consumed, lookahead included.
    pub fn position(&self) -> usize {
        self.session.position()
    }

    /// The declarations of the document's DTD, once its DOCTYPE was read.
    pub fn dtd(&self) -> Option<&crate::valid::Dtd> {
        self.session.catalog().map(|catalog| catalog.dtd())
    }

    pub(crate) fn budget(&mut self) -> Option<&mut crate::parser::Budget> {
        self.session.budget()
    }

    /// Changes the flags of a reader that has not been read yet.
    pub(crate) fn set_flags(&mut self, flags: crate::options::Flags) {
        self.session.set_flags(flags);
        self.detached = Detached::new(flags);
    }

    /// The source.
    pub fn get_ref(&self) -> &R {
        &self.input
    }

    /// The source, positioned after the consumed bytes.
    pub fn into_inner(self) -> R {
        self.input
    }

    /// The next event and its origin, or `None` at the end of the document.
    pub fn read_event(&mut self) -> Result<Option<(Event<'_>, SourceOrigin)>, ReadError> {
        if self.detached.is_some() {
            return self.read_detached();
        }
        self.read_lent::<false>()
    }

    /// [`Self::read_event`] under `RECOVER` or `NOBLANKS`: each event is
    /// copied out of the session, so that an error met reading it can stop a
    /// recovering parse (which then closes what is open) instead of ending
    /// reading, and blank text can wait for the event that decides it.
    #[cold]
    #[inline(never)]
    fn read_detached(&mut self) -> Result<Option<(Event<'_>, SourceOrigin)>, ReadError> {
        let (next, origin) = loop {
            let detached = self.detached.as_deref_mut().expect("detached reader");
            if let Some(next) = detached.ready.pop_front() {
                break next;
            }
            let recovers = detached.recovers;
            let next = match self.status {
                Status::Closing(at) => (self.session.close_next(at).map_err(ReadError::Xml))
                    .and_then(|next| {
                        next.map(|(event, origin, _)| detach(event, origin))
                            .transpose()
                    }),
                _ if recovers => (self.read_lent::<true>()).and_then(|next| {
                    next.map(|(event, origin)| detach(event, origin))
                        .transpose()
                }),
                _ => (self.read_lent::<false>()).and_then(|next| {
                    next.map(|(event, origin)| detach(event, origin))
                        .transpose()
                }),
            };
            match next {
                Ok(Some((event, origin))) => self.queue(event, origin)?,
                // Recovery reads the token on as other tokens.
                Ok(None) if matches!(self.status, Status::Active) => {}
                Ok(None) => {
                    self.status = Status::Finished;
                    return Ok(None);
                }
                Err(ReadError::Xml(error))
                    if recovers && !matches!(self.status, Status::Closing(_)) =>
                {
                    let Some(at) = self.session.stop(error) else {
                        self.status = Status::Xml(error);
                        return Err(ReadError::Xml(error));
                    };
                    (self.status, self.pending) = (Status::Closing(at), false);
                }
                Err(error) => {
                    if let ReadError::Xml(error) = error {
                        self.status = Status::Xml(error);
                    }
                    return Err(error);
                }
            }
        };
        let event = match next {
            Ok(event) => event,
            Err(declared) => {
                let detached = self.detached.as_deref_mut().expect("detached reader");
                detached.held.insert(declared).event()
            }
        };
        Ok(Some((event, origin)))
    }

    /// Queues a copied event after the blank text it decides `NOBLANKS` keeps.
    fn queue(
        &mut self,
        event: Result<Event<'static>, Declared>,
        origin: SourceOrigin,
    ) -> Result<(), ReadError> {
        let content = self.session.budget().and_then(|budget| budget.content());
        let detached = self.detached.as_deref_mut().expect("detached reader");
        let passes = match (&mut detached.blanks, &event) {
            (Some(blanks), Ok(lent)) => {
                let (kept, passes) =
                    (blanks.event(lent, origin, content)).map_err(ReadError::Xml)?;
                if let Some((text, at)) = kept {
                    detached.ready.push_back((Ok(Event::Text(text.into())), at));
                }
                passes
            }
            _ => true,
        };
        if passes {
            detached.ready.push_back((event, origin));
        }
        Ok(())
    }

    /// The next event, lent; `RECOVERS`: the DTD-free stream rereads.
    #[inline(always)]
    fn read_lent<const RECOVERS: bool>(
        &mut self,
    ) -> Result<Option<(Event<'_>, SourceOrigin)>, ReadError> {
        match self.status {
            Status::Finished => return Ok(None),
            Status::Xml(error) => return Err(ReadError::Xml(error)),
            Status::Poisoned => {
                let error = Error::new(ErrorKind::InvalidState, self.position());
                return Err(ReadError::Xml(error));
            }
            Status::Active | Status::Closing(_) => {}
        }
        // Poison before releasing tokens, mutating semantic state, or calling
        // the source. A caught panic cannot resume uncertain stream progress.
        self.status = Status::Poisoned;
        if let Some(stream) = self.session.plain() {
            // DTD-free documents lend straight from the ordinary token loop.
            if std::mem::take(&mut self.pending) {
                stream.release_in::<RECOVERS>();
            }
            match self.prepare_plain::<RECOVERS>() {
                Ok(Some(true)) => {
                    self.pending = true;
                    self.status = Status::Active;
                    let lent = match RECOVERS {
                        true => self.session.lend_recovering(),
                        false => self.session.lend_plain(),
                    };
                    if let Err(ReadError::Xml(error)) = &lent {
                        self.status = Status::Xml(*error);
                    }
                    return lent;
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
                .map_err(ReadError::Xml)
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
                self.status = Status::Xml(error);
                Err(ReadError::Xml(error))
            }
        }
    }

    /// Drains generated sources and saved lookahead before asking the caller's
    /// source for another window. Accepted bytes are consumed before an error.
    fn prepare(&mut self) -> Result<bool, ReadError> {
        loop {
            // At EOF the session drains or fails; it never asks for input.
            let result = if self.eof {
                self.session.finish_step()
            } else {
                self.session.prepare(&[])
            };
            match result.map_err(ReadError::Xml)?.outcome {
                SessionOutcome::Ready => return Ok(true),
                SessionOutcome::Complete => return Ok(false),
                SessionOutcome::NeedInput => {}
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
                let remaining = self.max_input_bytes.saturating_sub(before);
                if remaining == 0 {
                    return Err(ReadError::Xml(Error::new(
                        ErrorKind::LimitExceeded(Limit::InputBytes),
                        0,
                    )));
                }
                self.session.prepare(&input[..input.len().min(remaining)])
            };
            // Commit accepted transport before lending or reporting a semantic
            // error. A caught consume panic leaves this reader poisoned. The
            // session takes the whole window unless a token is ready.
            self.input.consume(self.position() - before);
            if result.map_err(ReadError::Xml)?.outcome == SessionOutcome::Ready {
                return Ok(true);
            }
        }
    }

    /// The DTD-free counterpart of `prepare`: `Some(ready)` like `prepare`, or
    /// `None` once a DOCTYPE token is parked for the session.
    #[inline(always)]
    fn prepare_plain<const RECOVERS: bool>(&mut self) -> Result<Option<bool>, ReadError> {
        loop {
            let stream = self.session.plain().expect("DTD-free stream");
            if RECOVERS && stream.ends_reference() {
                return Ok(Some(true));
            }
            match stream.token_kind() {
                Some(TokenKind::Dtd) if stream.hands_over() => return Ok(None),
                Some(_) if stream.is_non_event() => {
                    stream.skip_non_event().map_err(ReadError::Xml)?;
                    stream.release_in::<RECOVERS>();
                    continue;
                }
                Some(_) => return Ok(Some(true)),
                None => {}
            }
            if self.eof || (RECOVERS && stream.is_cut()) {
                if stream.finish_in::<RECOVERS>().map_err(ReadError::Xml)? {
                    continue;
                }
                stream.validate_finish().map_err(ReadError::Xml)?;
                return Ok(Some(false));
            }
            // Replaying saved lookahead or decoded carry need not touch R.
            stream.advance_in::<RECOVERS>(&[]).map_err(ReadError::Xml)?;
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
                stream.advance_in::<RECOVERS>(&input[..input.len().min(remaining)])
            };
            // Commit accepted transport before reporting a lexical error. The
            // stream takes the whole window unless a token is ready.
            self.input.consume(stream.position() - before);
            result.map_err(ReadError::Xml)?;
        }
    }
}

/// A declaration or DOCTYPE copied out of the session, which its event borrows.
enum Declared {
    Declaration(String, Option<String>, Option<bool>),
    /// Name, public and system identifiers, internal subset.
    Doctype([Option<String>; 4]),
}

impl Declared {
    fn event(&self) -> Event<'_> {
        match self {
            Self::Declaration(version, encoding, standalone) => {
                Event::Declaration(crate::Declaration {
                    version,
                    encoding: encoding.as_deref(),
                    standalone: *standalone,
                })
            }
            Self::Doctype([name, public_id, system_id, internal_subset]) => {
                Event::Doctype(crate::Doctype {
                    name: name.as_deref().unwrap_or_default(),
                    public_id: public_id.as_deref(),
                    system_id: system_id.as_deref(),
                    internal_subset: internal_subset.as_deref(),
                })
            }
        }
    }
}

/// `event` copied out of the session, uncharged: owned, or a declaration
/// whose event borrows the copy.
fn detach(
    event: Event<'_>,
    origin: SourceOrigin,
) -> Result<(Result<Event<'static>, Declared>, SourceOrigin), ReadError> {
    let offset = origin.error_offset();
    let copy = |text: &str| crate::parser::copied(text, offset).map_err(ReadError::Xml);
    let detached = match event {
        Event::Declaration(declaration) => Err(Declared::Declaration(
            copy(declaration.version)?,
            declaration.encoding.map(copy).transpose()?,
            declaration.standalone,
        )),
        Event::Doctype(doctype) => {
            let parts = [Some(doctype.name), doctype.public_id, doctype.system_id];
            let [name, public_id, system_id] = parts.map(|part| part.map(copy).transpose());
            let internal_subset = doctype.internal_subset.map(copy).transpose()?;
            Err(Declared::Doctype([
                name?,
                public_id?,
                system_id?,
                internal_subset,
            ]))
        }
        event => Ok(crate::parser::owned_event(event, None, offset).map_err(ReadError::Xml)?),
    };
    Ok((detached, origin))
}

/// What `read_detached` reads through.
struct Detached {
    recovers: bool,
    blanks: Option<Filter>,
    /// Copied events to return, in order: at most a kept blank and the
    /// event after it.
    ready: VecDeque<(Result<Event<'static>, Declared>, SourceOrigin)>,
    /// The declaration or DOCTYPE returned last, which its event borrows.
    held: Option<Declared>,
}

impl Detached {
    /// The copies a reader under `flags` reads through, if any.
    fn new(flags: Flags) -> Option<Box<Self>> {
        (flags.recovers() || flags.drops_blanks()).then(|| {
            Box::new(Self {
                recovers: flags.recovers(),
                blanks: flags.drops_blanks().then(Filter::default),
                ready: VecDeque::with_capacity(2),
                held: None,
            })
        })
    }
}
