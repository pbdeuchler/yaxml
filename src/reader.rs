use std::{borrow::Cow, ops::Range};

use memchr::memchr;

use crate::dtd::DtdLimits;
use crate::dtd_controller::content::BorrowedContentController;
use crate::dtd_controller::{BorrowedDtdController, DtdController, DtdEvent};
use crate::encoding::SourceEncoding;
use crate::escape::invalid_char;
use crate::options::Flags;
use crate::parser::{Budget, ParserState, space};
use crate::resolve::Loader;
use crate::tokenizer::{TokenKind, tag_close};
use crate::{DecodeLimits, Error, ErrorKind, Limit, Name, ParseOptions, SourceOrigin, Transcoded};

/// Inclusive resource ceilings shared by every parser and DOM.
///
/// Build one with field setters: `Limits::DEFAULT.max_depth(64)`.
/// [`options::HUGE`](crate::options::HUGE) lifts them as libxml2's
/// `XML_PARSE_HUGE` does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Limits {
    /// Total input bytes.
    pub max_input_bytes: usize,
    /// Raw bytes in one token; a whole start tag is one token.
    pub max_token_bytes: usize,
    /// Element nesting depth.
    pub max_depth: usize,
    /// Attributes on one element, namespace declarations included.
    pub max_attributes: usize,
    /// Attributes a DOM keeps across the whole document, namespace
    /// declarations and defaults included. Streaming readers ignore it.
    pub max_total_attributes: usize,
    /// Number of emitted events, including the declaration and empty elements.
    pub max_events: usize,
    /// Cumulative string capacity requested for copies (normalized values,
    /// names, owned-DOM storage), released buffers included. Borrowed text
    /// is free.
    pub max_owned_bytes: usize,
    /// Source bytes in the document type declaration, from `<!DOCTYPE` to `>`.
    pub max_dtd_bytes: usize,
    /// Entity expansion bytes always allowed; beyond it, expansion may not
    /// exceed `max_entity_amplification` times the input read so far
    /// (libxml2's `xmlParserEntityCheck`).
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

setters!(const Limits { max_input_bytes: usize, max_token_bytes: usize, max_depth: usize, max_attributes: usize, max_total_attributes: usize, max_events: usize, max_owned_bytes: usize, max_dtd_bytes: usize, max_entity_expansion_bytes: usize, max_entity_amplification: usize, max_entity_references: usize, max_entity_depth: usize, max_external_loads: usize, max_external_bytes: usize });

impl Limits {
    /// The default ceilings (libxml2's where it has one).
    pub const DEFAULT: Self = Self {
        max_input_bytes: 64 * 1024 * 1024,
        max_token_bytes: 1024 * 1024,
        max_depth: 256,
        max_attributes: 1024,
        max_total_attributes: 4_000_000,
        max_events: 1_000_000,
        max_owned_bytes: 64 * 1024 * 1024,
        max_dtd_bytes: 16 * 1024 * 1024,
        max_entity_expansion_bytes: 1_000_000,
        max_entity_amplification: 5,
        max_entity_references: 1024 * 1024,
        max_entity_depth: 20,
        max_external_loads: 256,
        max_external_bytes: 16 * 1024 * 1024,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// An attribute of a start tag.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attribute<'a> {
    /// The expanded name.
    pub name: Name<'a>,
    /// The normalized value, entity references substituted.
    pub value: Cow<'a, str>,
}

/// A start or empty-element tag.
#[derive(Debug, Eq, PartialEq)]
pub struct Element<'a> {
    /// The expanded name.
    pub name: Name<'a>,
    /// Source order, including namespace declarations in the XMLNS namespace.
    pub attributes: Attributes<'a>,
}

/// An element's attributes in source order, owned or borrowed; dereferences
/// to a slice.
#[derive(Clone, Debug)]
pub enum Attributes<'a> {
    /// A list lent by a tree or parser.
    Borrowed(&'a [Attribute<'a>]),
    /// An owned list.
    Owned(Vec<Attribute<'a>>),
}

impl<'a> Attributes<'a> {
    /// Mutable access to an owned list, copying a lent one first.
    pub fn to_mut(&mut self) -> &mut Vec<Attribute<'a>> {
        match *self {
            Self::Owned(ref mut attributes) => attributes,
            Self::Borrowed(attributes) => {
                *self = Self::Owned(attributes.to_vec());
                self.to_mut()
            }
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

/// The XML declaration, `<?xml ...?>`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Declaration<'a> {
    /// The `version` pseudo-attribute.
    pub version: &'a str,
    /// The `encoding` pseudo-attribute, as written.
    pub encoding: Option<&'a str>,
    /// The `standalone` pseudo-attribute.
    pub standalone: Option<bool>,
}

/// A document type declaration, already applied to the events that follow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Doctype<'a> {
    /// The root element name it declares.
    pub name: &'a str,
    /// The public identifier, without quotes.
    pub public_id: Option<&'a str>,
    /// The system identifier, without quotes.
    pub system_id: Option<&'a str>,
    /// The exact text between `[` and `]`.
    pub internal_subset: Option<&'a str>,
}

impl<'a> Doctype<'a> {
    /// Whether it names XHTML 1.0 by one of the three Strict, Frameset or
    /// Transitional public or system identifiers (`xmlIsXHTML`), which makes
    /// [`save`](crate::save) apply XHTML rules.
    ///
    /// ```
    /// let document = yaxml::Document::parse(
    ///     "<!DOCTYPE html PUBLIC '-//W3C//DTD XHTML 1.0 Strict//EN' 'x.dtd'><html/>",
    /// )?;
    /// assert!(document.doctype().unwrap().is_xhtml());
    /// # Ok::<(), yaxml::Error>(())
    /// ```
    pub fn is_xhtml(&self) -> bool {
        const PUBLIC: [&str; 3] = [
            "-//W3C//DTD XHTML 1.0 Strict//EN",
            "-//W3C//DTD XHTML 1.0 Frameset//EN",
            "-//W3C//DTD XHTML 1.0 Transitional//EN",
        ];
        const SYSTEM: [&str; 3] = [
            "http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd",
            "http://www.w3.org/TR/xhtml1/DTD/xhtml1-frameset.dtd",
            "http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd",
        ];
        self.public_id.is_some_and(|id| PUBLIC.contains(&id))
            || self.system_id.is_some_and(|id| SYSTEM.contains(&id))
    }

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
        let name_end = name_start + crate::parser::name_length(&raw[name_start..], false);
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

/// One parse event; borrows the input except where normalization changes it.
#[derive(Debug, Eq, PartialEq)]
pub enum Event<'a> {
    /// The XML declaration.
    Declaration(Declaration<'a>),
    /// The document type declaration.
    Doctype(Doctype<'a>),
    /// A start tag.
    Start(Element<'a>),
    /// An empty-element tag, `<a/>`; no [`Event::End`] follows.
    Empty(Element<'a>),
    /// An end tag.
    End(Name<'a>),
    /// Character data, references substituted.
    Text(Cow<'a, str>),
    /// A CDATA section's content.
    Cdata(Cow<'a, str>),
    /// A comment's content.
    Comment(Cow<'a, str>),
    /// A processing instruction.
    ProcessingInstruction {
        /// The target name.
        target: Cow<'a, str>,
        /// Everything after the target and its separating whitespace.
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

enum Phase<'input> {
    Declarations {
        controller: BorrowedDtdController<'input>,
        parser: ParserState<'input>,
    },
    Content(BorrowedContentController<'input>),
}

/// How a reader's document ended.
#[derive(Clone, Copy)]
enum End {
    Finished,
    Failed(Error),
    /// Under `RECOVER`, at an error no site repairs: what is open closes
    /// at its offset.
    Stopped(usize),
}

/// A pull parser over complete UTF-8 input, borrowing it.
///
/// Namespace-aware, and strict unless `RECOVER` is set (see
/// [`Reader::errors`]). Whitespace outside the root element is skipped.
/// Errors are sticky, and EOF repeats `Ok(None)`. For other
/// encodings or I/O sources, use [`PushParser`](crate::PushParser) or
/// [`BufferedReader`](crate::BufferedReader).
///
/// ```
/// use yaxml::{Event, Reader};
///
/// let mut reader = Reader::new(b"<log><entry>a</entry><entry>b</entry></log>")?;
/// let mut entries = 0;
/// while let Some(event) = reader.read_event()? {
///     if let Event::Start(element) = event {
///         entries += usize::from(element.name.local() == "entry");
///     }
/// }
/// assert_eq!(entries, 2);
/// # Ok::<(), yaxml::Error>(())
/// ```
pub struct Reader<'a> {
    source: &'a str,
    ordinary: Option<SliceReader<'a>>,
    session: Vec<Phase<'a>>,
    decode: DecodeLimits,
    options: DtdLimits,
    standalone: bool,
    loader: Option<Loader>,
    doctype_start: usize,
    origin: Option<SourceOrigin>,
    physical: usize,
    /// How reading ended, once it has.
    end: Option<End>,
    /// Under `RECOVER`, the invalid UTF-8 sequence that ends the parsed
    /// prefix of the input, and the earlier character outside `Char` that
    /// ends it after a DOCTYPE.
    cut: Option<Error>,
    dtd_cut: Option<Error>,
}

impl<'a> Reader<'a> {
    /// A reader with default options.
    ///
    /// # Errors
    ///
    /// Fails when the input is not UTF-8, holds a character outside XML's
    /// `Char`, or exceeds `max_input_bytes`.
    pub fn new(input: &'a [u8]) -> Result<Self, Error> {
        Self::with_limits(input, Limits::default())
    }

    /// A reader with custom limits and no flags.
    ///
    /// # Errors
    ///
    /// As [`Reader::new`].
    pub fn with_limits(input: &'a [u8], limits: Limits) -> Result<Self, Error> {
        Self::with_options(input, ParseOptions::default().with_limits(limits))
    }

    /// A reader with libxml2 parser options; see [`crate::options`].
    /// `decode_limits` do not apply, since nothing is decoded.
    ///
    /// # Errors
    ///
    /// As [`Reader::new`] (under `RECOVER` invalid UTF-8 and characters
    /// outside `Char` are recovered from instead); `DTDVALID`, `NOBLANKS`,
    /// `XINCLUDE` and a forced [`ParseOptions::encoding`] (read
    /// [`Transcoded`] input instead) are [`ErrorKind::UnsupportedOption`].
    pub fn with_options(input: &'a [u8], options: ParseOptions) -> Result<Self, Error> {
        let flags = options.check(false, crate::options::NOBLANKS)?;
        Self::borrowing(input, None, flags, options)
    }

    /// A reader over [`Transcoded`] input, with libxml2 parser options as
    /// [`Reader::with_options`]. Offsets count bytes of the transcoded text.
    ///
    /// # Errors
    ///
    /// As [`Reader::with_options`]; a decoding error [`Transcoded::new`]
    /// kept under `RECOVER` ends the text and is recovered from there, and
    /// without `RECOVER` is returned.
    pub fn transcoded(input: &'a Transcoded<'_>, options: ParseOptions) -> Result<Self, Error> {
        let flags = options.check(false, crate::options::NOBLANKS)?;
        Self::borrowing(input.as_bytes(), input.decoded.as_ref(), flags, options)
    }

    /// A reader over `input`: UTF-8 as given (`decoded` absent; decode
    /// limits do not apply), or text decoded from an encoding and ended by
    /// a recovered decoding error (`Transcoded::decoded`).
    pub(crate) fn borrowing(
        input: &'a [u8],
        decoded: Option<&(SourceEncoding, Option<Error>)>,
        flags: Flags,
        options: ParseOptions,
    ) -> Result<Self, Error> {
        if decoded.is_none() && options.encoding.is_some() {
            // Only `Transcoded::new` decodes in a forced encoding.
            return Err(Error::new(ErrorKind::UnsupportedOption, 0));
        }
        let (limits, decode) = options.effective_limits();
        let (limits, decode) = match decoded {
            // `Transcoded::new` checked the source's length.
            Some(_) => (
                limits.max_input_bytes(limits.max_input_bytes.max(input.len())),
                decode,
            ),
            None => (
                limits,
                DecodeLimits {
                    max_decoded_bytes: usize::MAX,
                    max_decoded_token_bytes: limits.max_token_bytes,
                },
            ),
        };
        Self::configured(input, decoded, limits, decode, flags, options)
    }

    /// A reader for already checked flags and effective limits.
    pub(crate) fn configured(
        input: &'a [u8],
        decoded: Option<&(SourceEncoding, Option<Error>)>,
        limits: Limits,
        decode: DecodeLimits,
        flags: Flags,
        options: ParseOptions,
    ) -> Result<Self, Error> {
        let (mut ordinary, cut, character) =
            SliceReader::with_decode_limits(input, decoded, limits, decode, flags)?;
        if options.encoding.is_some() {
            ordinary.state.force_encoding();
        }
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
            decode,
            options: DtdLimits {
                flags,
                ..DtdLimits::new(limits, decode)
            },
            standalone: false,
            loader,
            doctype_start: 0,
            origin: None,
            end: None,
            cut,
            dtd_cut,
        })
    }

    /// This fresh reader reads content in a node's context
    /// (`xmlCtxtParseContent`): `bindings` in scope, from the first byte (a
    /// byte-order mark is text), and with `dtd`'s entities, when there is
    /// one, through the DTD stack as a document's content would be.
    pub(crate) fn in_context(
        mut self,
        bindings: &[(&str, &str)],
        dtd: Option<&crate::valid::Dtd>,
    ) -> Result<Self, Error> {
        let mut ordinary = self.ordinary.take().expect("a fresh reader");
        ordinary.cursor = 0;
        ordinary.state = ordinary.state.in_context(bindings)?;
        let Some(dtd) = dtd else {
            self.ordinary = Some(ordinary);
            return Ok(self);
        };
        let body = DtdController::seeded(self.options, dtd, self.loader.take())?;
        let decoded = self.decode.max_decoded_token_bytes;
        let parser = ordinary.state.rereading(false, false);
        let controller =
            BorrowedContentController::stable_utf8(body, parser, self.source, decoded)?;
        self.session.push(Phase::Content(controller));
        Ok(self)
    }

    /// The declarations of the document's DTD, once its DOCTYPE was read
    /// (its internal subset); complete after the document element's start tag (the external subset
    /// loads while it is read).
    ///
    /// ```
    /// let mut reader = yaxml::Reader::new(b"<!DOCTYPE r [<!ENTITY a 'v'>]><r/>")?;
    /// assert!(reader.dtd().is_none());
    /// reader.read_event()?; // the DOCTYPE
    /// assert_eq!(reader.dtd().unwrap().entity("a").unwrap().content, Some("v"));
    /// # Ok::<(), yaxml::Error>(())
    /// ```
    pub fn dtd(&self) -> Option<&crate::valid::Dtd> {
        match self.session.first()? {
            Phase::Declarations { controller, .. } => Some(controller.catalog().dtd()),
            Phase::Content(controller) => Some(controller.catalog().dtd()),
        }
    }

    /// The finished document's declarations, moved out of the reader.
    pub(crate) fn take_dtd(&mut self) -> Option<crate::valid::Dtd> {
        match self.session.first_mut() {
            Some(Phase::Content(controller)) => Some(controller.catalog_mut().take_dtd()),
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

    /// The errors kept so far, in input order (the first 100, as libxml2
    /// keeps): namespace errors and warnings, which never stop a parse
    /// (libxml2's `xmlNsErr`, `ErrorKind::Warning`), and under `RECOVER` every
    /// well-formedness error recovered from.
    pub fn errors(&self) -> &[Error] {
        if let Some(ordinary) = &self.ordinary {
            return ordinary.state.errors();
        }
        match self.session.first() {
            Some(Phase::Declarations { parser, .. }) => parser.errors(),
            Some(Phase::Content(controller)) => controller.errors(),
            None => &[],
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

    /// The next event, or `None` at the end of the document.
    pub fn read_event(&mut self) -> Result<Option<Event<'a>>, Error> {
        if let Some(end) = self.end {
            return self.ended(end);
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
                        self.end = Some(End::Finished);
                        Ok(None)
                    }
                    Some(start) => self.start_dtd(start).and_then(|()| self.read_session()),
                },
                Err(error) => Err(error),
            },
            // Without a cut, the session's result is the reader's.
            None if self.dtd_cut.is_none() => {
                let result = self.read_session();
                if let Err(error) = result {
                    return self.stop(error);
                }
                return result;
            }
            None => self.read_session(),
        };
        // Reading the prefix before a cut ends with the cut's error, unless
        // memory ran out first.
        let cut = match self.ordinary {
            Some(_) => self.cut,
            None => self.dtd_cut,
        };
        let result = match (result, cut) {
            (Ok(None), Some(cut)) => Err(cut),
            (Err(error), Some(cut))
                if error.offset >= cut.offset && error.kind != ErrorKind::Allocation =>
            {
                Err(cut)
            }
            (result, _) => result,
        };
        if let Err(error) = result {
            return self.stop(error);
        }
        result
    }

    /// A read after the end: `None`, the error again, or the next closing event.
    #[cold]
    #[inline(never)]
    fn ended(&mut self, end: End) -> Result<Option<Event<'a>>, Error> {
        match end {
            End::Finished => Ok(None),
            End::Failed(error) => Err(error),
            End::Stopped(at) => self.close_next(at),
        }
    }

    /// An error no site repaired. Under `RECOVER` it stops reading: it is
    /// recorded, and what is open closes (`close_next`). Otherwise it is
    /// returned again by every later read.
    #[cold]
    #[inline(never)]
    fn stop(&mut self, error: Error) -> Result<Option<Event<'a>>, Error> {
        let error = match self.budget().map(|budget| budget.recover(error)) {
            Some(Ok(())) => {
                let at = match self.session.first() {
                    Some(Phase::Content(controller)) => controller.stopped_at(error.offset),
                    _ => error.offset,
                };
                self.end = Some(End::Stopped(at));
                return self.close_next(at);
            }
            // A refused allocation, not `error`, ends the parse.
            Some(Err(refused)) => refused,
            None => error,
        };
        self.end = Some(End::Failed(error));
        Err(error)
    }

    /// The next event closing, at `at` where reading stopped, what is still open.
    #[cold]
    #[inline(never)]
    fn close_next(&mut self, at: usize) -> Result<Option<Event<'a>>, Error> {
        let event = match (&mut self.ordinary, self.session.first_mut()) {
            (Some(ordinary), _) => ordinary.state.close_open(at),
            (None, Some(Phase::Content(controller))) => controller.close_open(at),
            _ => Ok(None),
        };
        match &event {
            Ok(Some(_)) => self.origin = Some(SourceOrigin::input(at..at)),
            Ok(None) => self.end = Some(End::Finished),
            Err(error) => self.end = Some(End::Failed(*error)),
        }
        event
    }

    #[inline(never)]
    fn read_session(&mut self) -> Result<Option<Event<'a>>, Error> {
        let Some(Phase::Content(controller)) = self.session.first_mut() else {
            return self.read_declarations();
        };
        let event = controller.next_stable();
        match &event {
            Ok(Some(_)) => self.origin = Some(controller.origin()),
            Ok(None) => self.end = Some(End::Finished),
            Err(_) => {}
        }
        event
    }

    /// The internal subset's events, then content's first event. Out of
    /// line, so the per-event content path keeps a small frame.
    #[cold]
    #[inline(never)]
    fn read_declarations(&mut self) -> Result<Option<Event<'a>>, Error> {
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
                Phase::Content(_) => return self.read_session(),
            }
        }
    }

    #[inline(never)]
    fn start_dtd(&mut self, start: usize) -> Result<(), Error> {
        self.session
            .try_reserve_exact(1)
            .map_err(|_| Error::new(ErrorKind::Allocation, start))?;
        self.physical = self.position();
        self.doctype_start = start;
        let mut ordinary = self.ordinary.take().expect("checked ordinary owner");
        let mut controller =
            DtdController::stable_utf8(self.source, 0, start, self.options, self.standalone);
        if let Some(mut loader) = self.loader.take() {
            (ordinary.state.take_catalogs().iter()).for_each(|url| loader.catalog_pi(url));
            controller.set_loader(loader);
        }
        // The DTD stack neither rereads nor repairs characters.
        self.session.push(Phase::Declarations {
            controller,
            parser: ordinary.state.rereading(false, false),
        });
        Ok(())
    }

    #[cold]
    #[inline(never)]
    fn start_content(&mut self) -> Result<(), Error> {
        self.physical = self.position();
        let Phase::Declarations { controller, parser } = self.session.pop().expect("completed DTD")
        else {
            unreachable!("declaration handoff")
        };
        let body = controller.into_body_state()?;
        let decoded = self.decode.max_decoded_token_bytes;
        let controller =
            BorrowedContentController::stable_utf8(body, parser, self.source, decoded)?;
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
    /// The reader and, under `RECOVER`, the invalid UTF-8 that ends its
    /// input and the first character outside `Char`, which it repairs.
    fn with_decode_limits(
        input: &'a [u8],
        decoded: Option<&(SourceEncoding, Option<Error>)>,
        limits: Limits,
        decode: DecodeLimits,
        flags: Flags,
    ) -> Result<(Self, Option<Error>, Option<Error>), Error> {
        if input.len() > limits.max_input_bytes {
            return Err(Error::new(ErrorKind::LimitExceeded(Limit::InputBytes), 0));
        }
        if decoded.is_none()
            && (input.starts_with(&[0xff, 0xfe])
                || input.starts_with(&[0xfe, 0xff])
                || input.starts_with(&[0, 0, 0xfe, 0xff])
                || input.starts_with(&[0, b'<', 0, b'?']))
        {
            return Err(Error::new(ErrorKind::UnsupportedEncoding, 0));
        }
        let (encoding, cut) = decoded.copied().unwrap_or((SourceEncoding::Utf8, None));
        // Decoded text is UTF-8; its decoding error (`cut`) ends it.
        let (input, utf8) = match std::str::from_utf8(input) {
            Ok(input) => (input, cut),
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
        // A decoded U+FEFF is content: the decoder removed the BOM.
        let bom = decoded.is_none() && input.starts_with('\u{feff}');
        let initial = if bom { 3 } else { 0 };
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
        };
        let decoded_limit = decode.max_decoded_token_bytes < limits.max_token_bytes;
        if decoded_limit {
            reader.limits.max_token_bytes = decode.max_decoded_token_bytes;
            reader.token_limit_kind = Limit::DecodedTokenBytes;
        }
        if decoded_limit || decoded.is_some() {
            let limit = decode.max_decoded_token_bytes;
            reader.state.set_encoding(encoding, initial, limit);
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
        // Strict reading compiles without the recovery checks.
        if self.state.rereads() {
            return self.read_step_recovering();
        }
        self.step::<false>()
    }

    /// [`SliceReader::read_step`] under `RECOVER`, out of the strict path.
    #[cold]
    #[inline(never)]
    fn read_step_recovering(&mut self) -> Result<Option<Event<'a>>, Error> {
        self.step::<true>()
    }

    /// The next event; `REREADS` is recovery (see [`ParserState::rereads`]).
    #[inline(always)]
    fn step<const REREADS: bool>(&mut self) -> Result<Option<Event<'a>>, Error> {
        // A kept undeclared reference ends where it started.
        if REREADS && self.state.take_reference_end() {
            return Ok(Some(Event::EntityEnd));
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
            let result = self.state.read_token::<REREADS, _>(
                kind,
                &self.input[start..end],
                start..end,
                |raw, _, _| Ok(Cow::Borrowed(raw)),
            );
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
                let rewind = if REREADS {
                    self.state.take_rewind()
                } else {
                    None
                };
                self.cursor = rewind.map_or(end, |at| {
                    (start + 1..=at.min(end))
                        .rev()
                        .find(|&at| self.input.is_char_boundary(at))
                        .unwrap_or(end)
                });
                debug_assert!(self.cursor > start, "no progress at {start}");
            }
            let event = result?;
            if let Some(event) = event {
                // A tag ended early spans what was read of it.
                self.origin = Some(SourceOrigin::input(start..self.cursor));
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
