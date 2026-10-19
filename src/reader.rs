use std::{borrow::Cow, ops::Range};

use memchr::{memchr, memmem};

use crate::dtd::DtdLimits;
use crate::dtd_controller::content::{BorrowedContentController, ContentOutcome};
use crate::dtd_controller::{BorrowedDtdController, DtdController, DtdEvent};
use crate::escape::invalid_char;
use crate::options::Flags;
use crate::parser::{Budget, ParserState, space};
use crate::resolve::Loader;
use crate::tokenizer::{TokenKind, Tokenizer, tag_close};
use crate::{DecodeLimits, Error, ErrorKind, Limit, Name, ParseOptions, SourceOrigin};

/// Inclusive resource ceilings. An element's start tag counts as one token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub max_input_bytes: usize,
    pub max_token_bytes: usize,
    pub max_depth: usize,
    pub max_attributes: usize,
    /// Number of emitted events, including the declaration and empty elements.
    pub max_events: usize,
    /// Cumulative requested string capacity for normalization, namespace copies,
    /// streaming names and full new owned-DOM pool capacities, including buffers
    /// already released. Borrowed slice text is free; retaining owned storage
    /// can spend more of this budget.
    pub max_owned_bytes: usize,
    /// Source bytes in the document type declaration, from `<!DOCTYPE` to `>`.
    pub max_dtd_bytes: usize,
    /// Entity expansion always allowed: replacement text entered (in content,
    /// attribute values and the internal subset), copied default attributes,
    /// plus a fixed cost per reference. Beyond it, expansion may not exceed
    /// `max_entity_amplification` times the input consumed so far, as with
    /// libxml2's `xmlParserEntityCheck`.
    pub max_entity_expansion_bytes: usize,
    /// Expansion-to-consumed-input ratio permitted above the fixed allowance.
    pub max_entity_amplification: usize,
    /// Cumulative entity references expanded, including nested references.
    pub max_entity_references: usize,
    /// Nesting depth of entity references being expanded.
    pub max_entity_depth: usize,
    /// External resources requested from a [`Resolver`](crate::Resolver).
    pub max_external_loads: usize,
    /// Cumulative bytes returned by a [`Resolver`](crate::Resolver).
    pub max_external_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_bytes: 64 * 1024 * 1024,
            max_token_bytes: 1024 * 1024,
            max_depth: 256,
            max_attributes: 1024,
            max_events: 1_000_000,
            max_owned_bytes: 64 * 1024 * 1024,
            max_dtd_bytes: 16 * 1024 * 1024,
            max_entity_expansion_bytes: 1_000_000,
            max_entity_amplification: 5,
            max_entity_references: 1024 * 1024,
            max_entity_depth: 40,
            max_external_loads: 256,
            max_external_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attribute<'a> {
    pub name: Name<'a>,
    pub value: Cow<'a, str>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Element<'a> {
    pub name: Name<'a>,
    /// Source order, including namespace declarations in the XMLNS namespace.
    pub attributes: Attributes<'a>,
}

/// An element's attributes in source order: an owned list, or one lent from a
/// streaming parser's reused storage for the duration of an event borrow.
/// Both dereference to a slice; `into_owned` keeps a lent list.
#[derive(Clone, Debug)]
pub enum Attributes<'a> {
    Borrowed(&'a [Attribute<'a>]),
    Owned(Vec<Attribute<'a>>),
}

impl<'a> Attributes<'a> {
    /// Mutable access to an owned list, copying a lent one first.
    pub fn to_mut(&mut self) -> &mut Vec<Attribute<'a>> {
        if let Self::Borrowed(attributes) = self {
            *self = Self::Owned(attributes.to_vec());
        }
        match self {
            Self::Owned(attributes) => attributes,
            Self::Borrowed(_) => unreachable!("converted to owned"),
        }
    }

    /// The list as an owned vector, copying a lent one.
    pub fn into_owned(self) -> Vec<Attribute<'a>> {
        match self {
            Self::Borrowed(attributes) => attributes.to_vec(),
            Self::Owned(attributes) => attributes,
        }
    }
}

impl<'a> std::ops::Deref for Attributes<'a> {
    type Target = [Attribute<'a>];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Borrowed(attributes) => attributes,
            Self::Owned(attributes) => attributes,
        }
    }
}

impl Default for Attributes<'_> {
    fn default() -> Self {
        Self::Owned(Vec::new())
    }
}

impl<'a> From<Vec<Attribute<'a>>> for Attributes<'a> {
    fn from(attributes: Vec<Attribute<'a>>) -> Self {
        Self::Owned(attributes)
    }
}

impl<'a> FromIterator<Attribute<'a>> for Attributes<'a> {
    fn from_iter<I: IntoIterator<Item = Attribute<'a>>>(iter: I) -> Self {
        Self::Owned(iter.into_iter().collect())
    }
}

impl PartialEq for Attributes<'_> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl Eq for Attributes<'_> {}

impl<'s, 'a> IntoIterator for &'s Attributes<'a> {
    type Item = &'s Attribute<'a>;
    type IntoIter = std::slice::Iter<'s, Attribute<'a>>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a> IntoIterator for Attributes<'a> {
    type Item = Attribute<'a>;
    type IntoIter = std::vec::IntoIter<Attribute<'a>>;

    fn into_iter(self) -> Self::IntoIter {
        self.into_owned().into_iter()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Declaration<'a> {
    pub version: &'a str,
    pub encoding: Option<&'a str>,
    pub standalone: Option<bool>,
}

/// A document type declaration. Identifiers are literal contents without
/// quotes; the internal subset is the exact text between `[` and `]`.
/// Its declarations have already been applied to the events that follow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Doctype<'a> {
    pub name: &'a str,
    pub public_id: Option<&'a str>,
    pub system_id: Option<&'a str>,
    pub internal_subset: Option<&'a str>,
}

impl<'a> Doctype<'a> {
    /// Splits a complete, already validated declaration from `<!DOCTYPE` to `>`.
    pub(crate) fn parse(raw: &'a str) -> Self {
        const SPACE: &[char] = &[' ', '\t', '\r', '\n'];
        fn literal(input: &str) -> (&str, &str) {
            let input = input.trim_start_matches(SPACE);
            let quote = input.as_bytes()[0] as char;
            let end = 1 + input[1..].find(quote).expect("validated literal");
            (&input[1..end], &input[end + 1..])
        }
        let rest = raw["<!DOCTYPE".len()..].trim_start_matches(SPACE);
        let (name, rest) = rest.split_at(
            rest.find(|c| SPACE.contains(&c) || c == '[' || c == '>')
                .expect("validated name"),
        );
        let rest = rest.trim_start_matches(SPACE);
        let (public_id, system_id, rest) = if let Some(rest) = rest.strip_prefix("PUBLIC") {
            let (public, rest) = literal(rest);
            let (system, rest) = literal(rest);
            (Some(public), Some(system), rest)
        } else if let Some(rest) = rest.strip_prefix("SYSTEM") {
            let (system, rest) = literal(rest);
            (None, Some(system), rest)
        } else {
            (None, None, rest)
        };
        let internal_subset = rest
            .trim_start_matches(SPACE)
            .strip_prefix('[')
            .map(|subset| &subset[..subset.rfind(']').expect("validated subset")]);
        Self {
            name,
            public_id,
            system_id,
            internal_subset,
        }
    }
}

/// Logical events borrow the input except where XML normalization changes it.
#[derive(Debug, Eq, PartialEq)]
pub enum Event<'a> {
    Declaration(Declaration<'a>),
    Doctype(Doctype<'a>),
    Start(Element<'a>),
    Empty(Element<'a>),
    End(Name<'a>),
    Text(Cow<'a, str>),
    Cdata(Cow<'a, str>),
    Comment(Cow<'a, str>),
    ProcessingInstruction {
        target: Cow<'a, str>,
        data: Cow<'a, str>,
    },
    /// A general entity reference in content, reported when entities are not
    /// substituted (no `NOENT`), like libxml2's SAX `reference` callback. The
    /// replacement text's events follow, then the matching [`Event::EntityEnd`];
    /// a reference that is not loaded or declared has none.
    EntityStart(Cow<'a, str>),
    /// The end of the innermost [`Event::EntityStart`].
    EntityEnd,
}

#[expect(
    clippy::large_enum_variant,
    reason = "the phase occupies one fallibly reserved out-of-line slot"
)]
enum Phase<'input> {
    Declarations {
        controller: BorrowedDtdController<'input>,
        parser: ParserState<'input>,
    },
    Content(BorrowedContentController<'input>),
}

/// Strict, namespace-aware XML 1.0 event reader over a complete UTF-8 input.
///
/// This borrows the complete input; bounded-memory I/O streaming is a separate API.
/// Top-level whitespace is skipped, while whitespace inside the root is kept.
/// After an error, every call returns that same error. EOF repeatedly returns
/// `Ok(None)`. Encodings other than UTF-8 are explicitly unsupported.
///
/// A document type declaration is reported as one [`Event::Doctype`]. Its
/// internal subset supplies entity replacement, default attributes and
/// attribute-type normalization; external entities and subsets are not read.
/// A document without a DOCTYPE never leaves the plain token loop.
pub struct Reader<'a> {
    source: &'a str,
    ordinary: Option<SliceReader<'a>>,
    session: Vec<Phase<'a>>,
    limits: Limits,
    decode: DecodeLimits,
    options: DtdLimits,
    standalone: bool,
    loader: Option<Loader>,
    doctype_start: usize,
    origin: Option<SourceOrigin>,
    physical: usize,
    failure: Option<Error>,
    finished: bool,
}

impl<'a> Reader<'a> {
    pub fn new(input: &'a [u8]) -> Result<Self, Error> {
        Self::with_limits(input, Limits::default())
    }

    pub fn with_limits(input: &'a [u8], limits: Limits) -> Result<Self, Error> {
        Self::with_options(input, ParseOptions::default().with_limits(limits))
    }

    /// Reads with libxml2 parser options; see [`crate::options`]. Tree-only
    /// flags (`DTDVALID`, `NOBLANKS`, `XINCLUDE`) are `UnsupportedOption`.
    /// Borrowed UTF-8 needs no decoding, so `decode_limits` do not apply.
    pub fn with_options(input: &'a [u8], options: ParseOptions) -> Result<Self, Error> {
        let flags = options.check(false, 0)?;
        let (limits, _) = options.effective_limits();
        let decode = DecodeLimits {
            max_decoded_bytes: usize::MAX,
            max_decoded_token_bytes: limits.max_token_bytes,
        };
        Self::configured(input, limits, decode, flags, options)
    }

    /// A reader for already checked flags and effective limits.
    pub(crate) fn configured(
        input: &'a [u8],
        limits: Limits,
        decode: DecodeLimits,
        flags: Flags,
        options: ParseOptions,
    ) -> Result<Self, Error> {
        let ordinary = SliceReader::with_decode_limits(input, limits, decode, flags)?;
        let base = options.base.as_deref().unwrap_or_default();
        let loader = options
            .resolver
            .map(|resolver| Loader::new(base, resolver, &limits).with_flags(flags));
        Ok(Self {
            source: ordinary.input,
            physical: ordinary.position(),
            ordinary: Some(ordinary),
            session: Vec::new(),
            limits,
            decode,
            options: DtdLimits {
                flags,
                ..DtdLimits::new(limits, decode)
            },
            standalone: false,
            loader,
            doctype_start: 0,
            origin: None,
            failure: None,
            finished: false,
        })
    }

    /// Element/attribute names declared `ID`, once content has started.
    pub(crate) fn catalog(&self) -> Option<&crate::dtd_catalog::DtdCatalog> {
        match self.session.first() {
            Some(Phase::Content(controller)) => Some(controller.catalog()),
            _ => None,
        }
    }

    pub(crate) fn owned_budget(&mut self) -> &mut Budget {
        if let Some(ordinary) = &mut self.ordinary {
            return ordinary.state.owned_budget();
        }
        match self.session.first_mut().expect("active DTD phase") {
            Phase::Declarations { parser, .. } => parser.owned_budget(),
            Phase::Content(controller) => controller.owned_budget(),
        }
    }

    /// Byte position after the most recently consumed input.
    pub fn position(&self) -> usize {
        if let Some(ordinary) = &self.ordinary {
            return ordinary.position();
        }
        match self.session.first() {
            Some(Phase::Declarations { controller, .. }) => controller.physical_position(),
            Some(Phase::Content(controller)) => controller.position().1,
            None => self.physical,
        }
    }

    /// Origin of the latest emitted event; absent before the first event.
    /// EOF and failures preserve the last delivered origin.
    pub fn event_origin(&self) -> Option<SourceOrigin> {
        self.origin
    }

    /// Exact original input span of the latest emitted event, when available.
    pub fn event_range(&self) -> Option<Range<usize>> {
        self.event_origin().and_then(SourceOrigin::input_range)
    }

    pub fn read_event(&mut self) -> Result<Option<Event<'a>>, Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.finished {
            return Ok(None);
        }
        // The DTD-free token loop stays on plain `Error` results; only a
        // DOCTYPE moves the reader onto the out-of-line session path.
        let result = match &mut self.ordinary {
            Some(ordinary) => match ordinary.read_step() {
                Ok(Some(event)) => {
                    if let Event::Declaration(declaration) = &event {
                        self.standalone = declaration.standalone == Some(true);
                    }
                    self.origin = ordinary.origin;
                    return Ok(Some(event));
                }
                Ok(None) => match ordinary.doctype.take() {
                    None => {
                        self.finished = true;
                        return Ok(None);
                    }
                    Some(start) => self.start_dtd(start).and_then(|()| self.read_session()),
                },
                Err(error) => Err(error),
            },
            None => self.read_session(),
        };
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }

    #[inline(never)]
    fn read_session(&mut self) -> Result<Option<Event<'a>>, Error> {
        loop {
            if matches!(self.session.first(), Some(Phase::Declarations { controller, .. }) if controller.is_complete())
            {
                self.start_content()?;
            }
            match self.session.first_mut().expect("active DTD phase") {
                Phase::Declarations { controller, parser } => {
                    while controller.current().is_none() {
                        controller.advance_borrowed(parser.owned_budget())?;
                    }
                    let (event, origin) = controller.current().expect("ready declaration");
                    let end = origin.physical_context().end;
                    let complete = matches!(event, DtdEvent::End);
                    if complete {
                        parser.check_event_limit(self.doctype_start)?;
                        parser.commit_doctype();
                    }
                    controller.release();
                    if complete {
                        let range = self.doctype_start..end;
                        self.origin = Some(SourceOrigin::input(range.clone()));
                        return Ok(Some(Event::Doctype(Doctype::parse(&self.source[range]))));
                    }
                }
                Phase::Content(controller) => match controller.advance_borrowed()?.outcome {
                    ContentOutcome::EventReady => {
                        let (event, origin) = controller.take_event()?;
                        if let Some(event) = event {
                            self.origin = Some(origin);
                            return Ok(Some(event));
                        }
                    }
                    ContentOutcome::Progress => {}
                    ContentOutcome::Complete => {
                        self.finished = true;
                        return Ok(None);
                    }
                    ContentOutcome::NeedPhysicalInput => {
                        return Err(Error::new(ErrorKind::InvalidState, controller.position().1));
                    }
                },
            }
        }
    }

    fn start_dtd(&mut self, start: usize) -> Result<(), Error> {
        self.session
            .try_reserve_exact(1)
            .map_err(|_| Error::new(ErrorKind::Allocation, start))?;
        self.physical = self.position();
        self.doctype_start = start;
        let ordinary = self.ordinary.take().expect("checked ordinary owner");
        let mut controller =
            DtdController::stable_utf8(self.source, 0, start, self.options, self.standalone);
        if let Some(loader) = self.loader.take() {
            controller.set_loader(loader);
        }
        self.session.push(Phase::Declarations {
            controller,
            parser: ordinary.state,
        });
        Ok(())
    }

    fn start_content(&mut self) -> Result<(), Error> {
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
        let controller =
            BorrowedContentController::stable_utf8(body, parser, tokenizer, self.source)?;
        self.session.push(Phase::Content(controller));
        Ok(())
    }
}

/// The DTD-free token loop over a validated UTF-8 slice.
struct SliceReader<'a> {
    input: &'a str,
    cursor: usize,
    origin: Option<SourceOrigin>,
    state: ParserState<'a>,
    /// Start of a DOCTYPE that ended the plain token loop.
    doctype: Option<usize>,
    limits: Limits,
    token_limit_kind: Limit,
}

impl<'a> SliceReader<'a> {
    fn with_decode_limits(
        input: &'a [u8],
        limits: Limits,
        decode: DecodeLimits,
        flags: Flags,
    ) -> Result<Self, Error> {
        if input.len() > limits.max_input_bytes {
            return Err(Error::new(ErrorKind::LimitExceeded(Limit::InputBytes), 0));
        }
        if input.starts_with(&[0xff, 0xfe])
            || input.starts_with(&[0xfe, 0xff])
            || input.starts_with(&[0, 0, 0xfe, 0xff])
            || input.starts_with(&[0, b'<', 0, b'?'])
        {
            return Err(Error::new(ErrorKind::UnsupportedEncoding, 0));
        }
        let input = std::str::from_utf8(input)
            .map_err(|e| Error::new(ErrorKind::InvalidUtf8, e.valid_up_to()))?;
        if let Some(i) = invalid_char(input) {
            return Err(Error::new(ErrorKind::InvalidCharacter, i));
        }
        let initial = if input.starts_with('\u{feff}') { 3 } else { 0 };
        if input.len() - initial > decode.max_decoded_bytes {
            return Err(Error::new(ErrorKind::LimitExceeded(Limit::DecodedBytes), 0));
        }
        let mut reader = Self {
            input,
            cursor: initial,
            origin: None,
            doctype: None,
            state: ParserState::new(limits, initial).with_flags(flags),
            limits,
            token_limit_kind: Limit::TokenBytes,
        };
        if decode.max_decoded_token_bytes < limits.max_token_bytes {
            reader.limits.max_token_bytes = decode.max_decoded_token_bytes;
            reader.token_limit_kind = Limit::DecodedTokenBytes;
            reader.state.set_encoding(
                crate::encoding::SourceEncoding::Utf8,
                initial,
                decode.max_decoded_token_bytes,
            );
        }
        Ok(reader)
    }

    fn position(&self) -> usize {
        self.cursor
    }

    /// The next event, or `None` at EOF or at a DOCTYPE (recorded in
    /// `doctype`). Events return in the reader's own result type, so they are
    /// not rewrapped on the way out.
    fn read_step(&mut self) -> Result<Option<Event<'a>>, Error> {
        loop {
            if !self.state.in_content() {
                let start = self.cursor;
                while self
                    .input
                    .as_bytes()
                    .get(self.cursor)
                    .is_some_and(|b| space(*b))
                {
                    self.cursor += 1;
                }
                self.check_token(start, self.cursor)?;
            }
            let start = self.cursor;
            if start == self.input.len() {
                self.state.validate_finish(start)?;
                return Ok(None);
            }
            self.state.check_event_limit(start)?;
            let tail = &self.input[start..];
            let (kind, end) = if !tail.starts_with('<') {
                let rest = &self.input.as_bytes()[start..];
                let end = start + memchr(b'<', rest).unwrap_or(rest.len());
                self.check_token(start, end)?;
                (TokenKind::Text, end)
            } else if tail.starts_with("<!--") {
                (TokenKind::Comment, self.delimited(start, 4, b"-->")?)
            } else if tail.starts_with("<![CDATA[") {
                self.state.check_token_start(TokenKind::Cdata, start)?;
                (TokenKind::Cdata, self.delimited(start, 9, b"]]>")?)
            } else if tail.starts_with("<?") {
                (TokenKind::Instruction, self.delimited(start, 2, b"?>")?)
            } else if tail.starts_with("</") {
                (TokenKind::End, self.tag_end(start)?)
            } else if tail.starts_with("<!DOCTYPE") {
                self.check_token(start, start + 9)?;
                self.state.check_doctype_start(start)?;
                self.doctype = Some(start);
                return Ok(None);
            } else if tail.starts_with("<!") {
                (TokenKind::InvalidMarkup, start + 2)
            } else {
                self.state.check_token_start(TokenKind::Start, start)?;
                (TokenKind::Start, self.tag_end(start)?)
            };
            let result =
                self.state
                    .process_token(kind, &self.input[start..end], start..end, |raw, _, _| {
                        Ok(Cow::Borrowed(raw))
                    });
            // Preserve the slice reader's cursor at lexical/structural failures.
            // Value normalization and declaration parsing consume their raw token.
            let consumed_error = result.as_ref().is_err_and(|error| {
                matches!(
                    kind,
                    TokenKind::Text
                        | TokenKind::Comment
                        | TokenKind::Cdata
                        | TokenKind::Instruction
                ) && matches!(
                    error.kind,
                    ErrorKind::InvalidReference
                        | ErrorKind::UnknownEntity
                        | ErrorKind::Allocation
                        | ErrorKind::LimitExceeded(Limit::OwnedBytes)
                        | ErrorKind::InvalidDeclaration
                        | ErrorKind::UnsupportedEncoding
                )
            });
            if result.is_ok() || consumed_error {
                self.cursor = end;
            }
            let event = result?;
            if let Some(event) = event {
                self.origin = Some(SourceOrigin::input(start..end));
                return Ok(Some(event));
            }
        }
    }

    fn check_token(&self, start: usize, end: usize) -> Result<(), Error> {
        if end - start > self.limits.max_token_bytes {
            Err(Error::new(
                ErrorKind::LimitExceeded(self.token_limit_kind),
                start,
            ))
        } else {
            Ok(())
        }
    }

    fn delimited(&self, start: usize, prefix: usize, delimiter: &[u8]) -> Result<usize, Error> {
        let available = self.input.len() - start;
        let length = available.min(self.limits.max_token_bytes);
        if prefix <= length
            && let Some(relative) = memmem::find(
                &self.input.as_bytes()[start + prefix..start + length],
                delimiter,
            )
        {
            return Ok(start + prefix + relative + delimiter.len());
        }
        Err(Error::new(
            if available > self.limits.max_token_bytes {
                ErrorKind::LimitExceeded(self.token_limit_kind)
            } else {
                ErrorKind::UnexpectedEof
            },
            start,
        ))
    }

    fn tag_end(&self, start: usize) -> Result<usize, Error> {
        let length = (self.input.len() - start).min(self.limits.max_token_bytes);
        if let Some(end) = tag_close(&self.input.as_bytes()[start..start + length]) {
            return Ok(start + end);
        }
        Err(Error::new(
            if self.input.len() - start > self.limits.max_token_bytes {
                ErrorKind::LimitExceeded(self.token_limit_kind)
            } else {
                ErrorKind::UnexpectedEof
            },
            start,
        ))
    }
}
