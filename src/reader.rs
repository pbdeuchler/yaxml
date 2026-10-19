use std::{borrow::Cow, ops::Range};

use memchr::memchr;

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
        let (doctype, end) = Self::header(raw);
        let internal_subset = raw[end..]
            .strip_prefix('[')
            .map(|subset| &subset[..subset.rfind(']').expect("validated subset")]);
        Self {
            internal_subset,
            ..doctype
        }
    }

    /// libxml2's `xmlParseDocTypeDecl`, which also reads a malformed header:
    /// the name (empty without one) and identifiers it keeps, and the offset
    /// in `raw`, from `<!DOCTYPE`, where `[` or `>` should follow.
    pub(crate) fn header(raw: &'a str) -> (Self, usize) {
        let skip =
            |at: usize| raw.len() - raw[at..].trim_start_matches([' ', '\t', '\r', '\n']).len();
        // A literal and the offset after it: up to its quote or, as libxml2
        // reads a public identifier, a character that is not a PubidChar.
        let literal = |at: usize, public: bool| {
            let quote = raw[at..]
                .chars()
                .next()
                .filter(|c| matches!(c, '"' | '\''))?;
            let body = &raw[at + 1..];
            let end = body
                .find(|c| c == quote || (public && !crate::dtd::pubid_char(c)))
                .unwrap_or(body.len());
            Some((
                &body[..end],
                at + 1 + end + usize::from(body[end..].starts_with(quote)),
            ))
        };
        let name_start = skip("<!DOCTYPE".len());
        let name_end = name_start + crate::parser::name_length(&raw[name_start..]);
        let mut at = skip(name_end);
        let identifier = |at: &mut usize, public| {
            let (value, end) = literal(*at, public)?;
            *at = skip(end);
            Some(value)
        };
        let (public_id, system_id) = match &raw[at..] {
            rest if rest.starts_with("SYSTEM") => {
                at = skip(at + 6);
                (None, identifier(&mut at, false))
            }
            rest if rest.starts_with("PUBLIC") => {
                at = skip(at + 6);
                let public = identifier(&mut at, true);
                (public, identifier(&mut at, false))
            }
            _ => (None, None),
        };
        let doctype = Self {
            name: &raw[name_start..name_end],
            public_id,
            system_id,
            internal_subset: None,
        };
        (doctype, at)
    }
}

/// The first `&Name;` in `text` that names no predefined entity: its offset
/// and the name.
fn undeclared_reference(text: &str) -> Option<(usize, &str)> {
    memchr::memchr_iter(b'&', text.as_bytes()).find_map(|at| {
        let rest = &text[at + 1..];
        let name = &rest[..crate::parser::name_length(rest)];
        let complete = !name.is_empty() && rest[name.len()..].starts_with(';');
        (complete && crate::escape::predefined(name).is_none()).then_some((at, name))
    })
}

/// Recovery from a DTD error at `at` in `raw`, the declaration read so far
/// from `<!DOCTYPE`, with the internal subset kept up to `unit` (offsets
/// into `raw`): the DOCTYPE keeps the header and those declarations, and the
/// document resumes where libxml2's does: at the error in the subset, else
/// after the header's `>` or where the header ends.
pub(crate) fn recovered_doctype(raw: &str, unit: Option<usize>, at: usize) -> (Doctype<'_>, usize) {
    let boundary = |at: usize| {
        (0..=at.min(raw.len()))
            .rev()
            .find(|&at| raw.is_char_boundary(at))
    };
    let (mut doctype, end) = Doctype::header(raw);
    let resume = match (raw.as_bytes().get(end), unit) {
        (Some(b'['), Some(unit)) => {
            let kept = boundary(unit).unwrap_or(0).max(end + 1);
            doctype.internal_subset = Some(&raw[end + 1..kept]);
            boundary(at).unwrap_or(0).max(end + 1)
        }
        (Some(b'>'), _) => end + 1,
        _ => end,
    };
    (doctype, resume)
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
    /// Under `RECOVER`, the invalid UTF-8 sequence that ends the parsed
    /// prefix of the input, and the earlier character outside `Char` that
    /// ends it after a DOCTYPE.
    cut: Option<Error>,
    dtd_cut: Option<Error>,
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
        let (ordinary, cut, character) =
            SliceReader::with_decode_limits(input, limits, decode, flags)?;
        // The DTD stack does not repair characters: it reads the prefix
        // before the first one outside `Char`.
        let dtd_cut = character.or(cut);
        let base = options.base.as_deref().unwrap_or_default();
        let loader = options
            .resolver
            .map(|resolver| Loader::new(base, resolver, &limits).with_flags(flags));
        Ok(Self {
            source: &ordinary.input[..dtd_cut.map_or(ordinary.input.len(), |cut| cut.offset)],
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
            cut,
            dtd_cut,
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
        self.budget().expect("active DTD phase")
    }

    /// The parse's budget; absent only after a failed DTD handoff.
    pub(crate) fn budget(&mut self) -> Option<&mut Budget> {
        if let Some(ordinary) = &mut self.ordinary {
            return Some(ordinary.state.owned_budget());
        }
        Some(match self.session.first_mut()? {
            Phase::Declarations { parser, .. } => parser.owned_budget(),
            Phase::Content(controller) => controller.owned_budget(),
        })
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
                        Ok(None)
                    }
                    Some(start) => self.start_dtd(start).and_then(|()| self.read_session()),
                },
                Err(error) => Err(error),
            },
            None => self.read_session(),
        };
        // Reading the prefix before a cut ends with the cut's error.
        let cut = match self.ordinary {
            Some(_) => self.cut,
            None => self.dtd_cut,
        };
        let result = match (result, cut) {
            (Ok(None), Some(cut)) => Err(cut),
            (Err(error), Some(cut)) if error.offset >= cut.offset => Err(cut),
            (result, _) => result,
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
                        if let Err(error) = controller.advance_borrowed(parser.owned_budget()) {
                            controller.recover(error, parser.owned_budget())?;
                        }
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
                        let start = self.doctype_start;
                        let (doctype, end) = match controller.cut() {
                            None => (Doctype::parse(&self.source[start..end]), end),
                            Some((unit, at)) => {
                                let local = |offset: usize| offset.saturating_sub(start);
                                let raw = &self.source[start..];
                                let (doctype, resume) =
                                    recovered_doctype(raw, unit.map(local), local(at));
                                controller.resume_at(start + resume, start + resume);
                                (doctype, start + resume)
                            }
                        };
                        self.origin = Some(SourceOrigin::input(start..end));
                        return Ok(Some(Event::Doctype(doctype)));
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
        // The DTD stack neither rereads nor repairs characters.
        self.session.push(Phase::Declarations {
            controller,
            parser: ordinary.state.rereading(false, false),
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
    /// The end of a recovered undeclared entity reference.
    reference_end: Option<SourceOrigin>,
}

impl<'a> SliceReader<'a> {
    /// The reader and, under `RECOVER`, the invalid UTF-8 that ends its
    /// input and the first character outside `Char`, which it repairs.
    fn with_decode_limits(
        input: &'a [u8],
        limits: Limits,
        decode: DecodeLimits,
        flags: Flags,
    ) -> Result<(Self, Option<Error>, Option<Error>), Error> {
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
        let (input, utf8) = match std::str::from_utf8(input) {
            Ok(input) => (input, None),
            Err(e) => (
                std::str::from_utf8(&input[..e.valid_up_to()]).expect("valid prefix"),
                Some(Error::new(ErrorKind::InvalidUtf8, e.valid_up_to())),
            ),
        };
        let character = invalid_char(input).map(|i| Error::new(ErrorKind::InvalidCharacter, i));
        // Strict parsing reports invalid UTF-8 first; recovery parses the
        // prefix before invalid UTF-8, which ends the document, and repairs
        // characters outside `Char` as libxml2 does.
        if let Some(error) = utf8.or(character).filter(|_| !flags.recovers()) {
            return Err(error);
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
            state: ParserState::new(limits, initial)
                .with_flags(flags)
                .rereading(true, character.is_some()),
            limits,
            token_limit_kind: Limit::TokenBytes,
            reference_end: None,
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
        Ok((reader, utf8, character))
    }

    fn position(&self) -> usize {
        self.cursor
    }

    /// The next event, or `None` at EOF or at a DOCTYPE (recorded in
    /// `doctype`). Events return in the reader's own result type, so they are
    /// not rewrapped on the way out.
    fn read_step(&mut self) -> Result<Option<Event<'a>>, Error> {
        if self.reference_end.is_some() {
            return Ok(Some(self.end_reference()));
        }
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
                if self.state.keeps_undeclared() && self.state.in_content() {
                    match self.undeclared(start, end)? {
                        Ok(event) => return Ok(Some(event)),
                        Err(end) => (TokenKind::Text, end),
                    }
                } else {
                    (TokenKind::Text, end)
                }
            } else if tail.starts_with("<!--") {
                (TokenKind::Comment, self.delimited(start, 4, b"-->")?)
            } else if tail.starts_with("<![CDATA[") {
                self.state.check_token_start(TokenKind::Cdata, start)?;
                (TokenKind::Cdata, self.delimited(start, 9, b"]]>")?)
            } else if tail.starts_with("<?") {
                (TokenKind::Instruction, self.delimited(start, 2, b"?>")?)
            } else if tail.starts_with("</") {
                (TokenKind::End, self.tag_end(start)?)
            // Recovery reads a DOCTYPE inside the root as invalid markup.
            } else if tail.starts_with("<!DOCTYPE")
                && !(self.state.recovers() && self.state.in_content())
            {
                self.check_token(start, start + 9)?;
                self.state.check_doctype_start(start)?;
                self.doctype = Some(start);
                return Ok(None);
            } else if tail.starts_with("<!") {
                (TokenKind::InvalidMarkup, start + 2)
            } else {
                self.state.check_token_start(TokenKind::Start, start)?;
                // Recovery closes a start tag cut off by the end of input.
                let end = match self.tag_end(start) {
                    Err(error)
                        if error.kind == ErrorKind::UnexpectedEof && self.state.recovers() =>
                    {
                        self.input.len()
                    }
                    end => end?,
                };
                (TokenKind::Start, end)
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
                // Recovery rereads the rest of a token libxml2 ends early,
                // always from past its start.
                let rewind = self.state.take_rewind().map(|at| {
                    let at = at.min(end);
                    (start + 1..=at)
                        .rev()
                        .find(|&at| self.input.is_char_boundary(at))
                        .unwrap_or(end)
                });
                self.cursor = rewind.unwrap_or(end);
                debug_assert!(self.cursor > start, "no progress at {start}");
            }
            let event = result?;
            if let Some(event) = event {
                // A tag ended early spans what was read of it.
                self.origin = Some(SourceOrigin::input(start..self.cursor.min(end)));
                return Ok(Some(event));
            }
        }
    }

    /// The end of a recovered undeclared entity reference.
    #[cold]
    fn end_reference(&mut self) -> Event<'a> {
        self.origin = self.reference_end.take();
        Event::EntityEnd
    }

    /// Recovery keeps a reference to an undeclared entity in the text from
    /// `start` to `end`, as libxml2 does without `NOENT`, as an empty
    /// reference: its start event, or where the text before it ends.
    #[cold]
    fn undeclared(&mut self, start: usize, end: usize) -> Result<Result<Event<'a>, usize>, Error> {
        let input = self.input;
        Ok(match undeclared_reference(&input[start..end]) {
            Some((0, name)) => {
                let end = start + name.len() + 2;
                let event = self.state.undeclared_reference(name, start)?;
                let origin = SourceOrigin::input(start..end);
                (self.cursor, self.origin) = (end, Some(origin));
                self.reference_end = Some(origin);
                Ok(event)
            }
            Some((at, _)) => Err(start + at),
            None => Err(end),
        })
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
            && let Some(relative) = crate::tokenizer::find(
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
