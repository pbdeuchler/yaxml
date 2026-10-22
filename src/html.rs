//! HTML parsing and serialization as libxml2 2.15.4's `HTMLparser.c` and
//! `HTMLtree.c` do.
//!
//! Tokenization is HTML5-style; tree construction is libxml2's legacy
//! algorithm (implied html/head/body, auto-close tables), not the WHATWG
//! tree builder. Malformed input is always recovered, and what libxml2
//! reports is collected as [`Diagnostic`]s. [`parse`] builds an
//! [`OwnedDocument`] marked as HTML; [`parse_events`] and [`EventParser`]
//! deliver libxml2's SAX callbacks as [`Event`]s instead. See the
//! [HTML contract](https://github.com/pbdeuchler/yaxml/blob/main/docs/html.md).
//!
//! ```
//! use yaxml::html;
//!
//! let parsed = html::parse(b"<p class=x>Hello<br>world<p>b &amp; c", html::Options::default())?;
//! let body = parsed.document.root_element().last_child().unwrap();
//! assert_eq!(body.name().unwrap().local(), "body");
//! assert_eq!(
//!     html::save(&parsed.document, false)?,
//!     b"<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.0 Transitional//EN\" \
//!       \"http://www.w3.org/TR/REC-html40/loose.dtd\">\n\
//!       <html><body><p class=\"x\">Hello<br>world</p><p>b &amp; c</p></body></html>\n"
//! );
//! # Ok::<(), yaxml::Error>(())
//! ```

use std::borrow::Cow;
use std::ops::ControlFlow;

use crate::diagnostic::{self, Domain, Level, code};
use crate::encoding::WINDOWS_1252;
use crate::owned::HtmlBuilder;
use crate::{Error, ErrorKind, Limit, Limits, LineIndex, OwnedDocument};

mod encoding;
mod entities;
mod serialize;
mod tables;
mod tokenizer;
mod tree;

use encoding::{Decoder, meta_charset, prescan, sniff};
use tokenizer::{Stop, TagEnd, Token, Tokenizer};
use tree::{Document, Events, Place, Reaction, Settings, Sink, Tree};

pub(crate) use serialize::save_node;
pub use serialize::{dump_node, save, to_html};
pub(crate) use tables::boolean_attribute;

/// `HTML_PARSE_RECOVER`: accepted; HTML parsing always recovers.
pub const RECOVER: u32 = 1 << 0;
/// `HTML_PARSE_NODEFDTD`: no default HTML 4.0 DOCTYPE.
pub const NODEFDTD: u32 = 1 << 2;
/// `HTML_PARSE_NOERROR`: collect no diagnostics.
pub const NOERROR: u32 = 1 << 5;
/// `HTML_PARSE_NOWARNING`: accepted; the HTML parser reports no warnings.
pub const NOWARNING: u32 = 1 << 6;
/// `HTML_PARSE_PEDANTIC`: accepted; it has no effect in libxml2 2.15.4 either.
pub const PEDANTIC: u32 = 1 << 7;
/// `HTML_PARSE_NOBLANKS`: drop ignorable blank text (libxml2's `areBlanks`).
pub const NOBLANKS: u32 = 1 << 8;
/// `HTML_PARSE_NONET`: accepted; parsing never uses the network.
pub const NONET: u32 = 1 << 11;
/// `HTML_PARSE_NOIMPLIED`: no implied html, head or body elements.
pub const NOIMPLIED: u32 = 1 << 13;
/// `HTML_PARSE_COMPACT`: accepted; storage is always compact.
pub const COMPACT: u32 = 1 << 16;
/// `HTML_PARSE_HUGE`: depth ceiling 2048 and value ceiling 1 GB, as in libxml2.
pub const HUGE: u32 = 1 << 19;
/// `HTML_PARSE_IGNORE_ENC`: ignore `<meta>` encoding declarations.
pub const IGNORE_ENC: u32 = 1 << 21;
/// `HTML_PARSE_BIG_LINES`: accepted; lines are never capped at 65535.
pub const BIG_LINES: u32 = 1 << 22;
/// `HTML_PARSE_HTML5`: report every tag as it is read, as libxml2 2.15.4
/// does: no implied elements, auto-closing, misplaced-tag checks or end-tag
/// matching. Meant for [`parse_events`]; a tree follows the unbalanced calls.
pub const HTML5: u32 = 1 << 26;
/// `XML_PARSE_NOENT`, which libxml2 accepts and ignores for HTML.
const NOENT: u32 = 1 << 1;
const SUPPORTED: u32 = RECOVER
    | NOENT
    | NODEFDTD
    | NOERROR
    | NOWARNING
    | PEDANTIC
    | NOBLANKS
    | NONET
    | NOIMPLIED
    | COMPACT
    | HUGE
    | IGNORE_ENC
    | BIG_LINES
    | HTML5;

/// HTML parser options: `htmlParserOption` flags and resource ceilings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Options {
    /// `HTML_PARSE_*` flags from this module's constants.
    pub flags: u32,
    /// Resource ceilings. As in libxml2, exceeding `max_depth` (trees only)
    /// or `max_token_bytes` (attribute values, comments, merged text) stops
    /// parsing with [`DiagnosticKind::ResourceLimit`] and keeps the tree
    /// built so far; the other ceilings fail with
    /// [`ErrorKind::LimitExceeded`].
    pub limits: Limits,
}

setters!(const Options { flags: u32, limits: Limits });

impl Default for Options {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Options {
    /// No flags and default limits.
    pub const DEFAULT: Self = Self::new(0);

    /// `flags` with default limits.
    pub const fn new(flags: u32) -> Self {
        Self {
            flags,
            limits: Limits::DEFAULT,
        }
    }

    fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }
}

/// A problem libxml2 reports while recovering. `offset` counts bytes of the
/// decoded text and is where libxml2's input is when it reports; its line,
/// column and report come from [`LineIndex::html`] or
/// [`Summary::line_index`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Diagnostic {
    /// The problem's category.
    pub kind: DiagnosticKind,
    /// Byte offset in the decoded text.
    pub offset: usize,
    /// libxml2's message, without its trailing newline.
    pub message: String,
    /// A text offset on the line libxml2 reports (its `input->line` lags
    /// within a character-data run).
    mark: usize,
    domain: Domain,
    level: Level,
    /// Reported before a `<meta>` or guessed charset re-decoded the text
    /// after it: libxml2's context line shows the bytes as they were.
    early: bool,
}

/// The category of an HTML [`Diagnostic`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DiagnosticKind {
    /// Invalid bytes in the input encoding (reported once).
    InvalidEncoding,
    /// A `<meta>` charset that is unknown or not ASCII-compatible.
    UnsupportedEncoding,
    /// A misplaced `<html>`, `<head>` or `<body>` start tag, which is dropped.
    MisplacedTag,
    /// An end tag without a matching open element, which is dropped.
    UnexpectedEndTag,
    /// An end tag that closed other elements or could not close its own.
    TagMismatch,
    /// A depth or length ceiling stopped parsing.
    ResourceLimit,
}

impl Diagnostic {
    fn new(
        kind: DiagnosticKind,
        offset: usize,
        mark: usize,
        message: String,
        (domain, level): (Domain, Level),
    ) -> Self {
        Self {
            kind,
            offset,
            message,
            mark,
            domain,
            level,
            early: false,
        }
    }

    /// libxml2's `xmlError` for this diagnostic, in the input `lines`
    /// indexes: its line, and a column counted in characters.
    pub fn describe(&self, lines: &LineIndex<'_>) -> diagnostic::Diagnostic {
        let code = match self.kind {
            DiagnosticKind::InvalidEncoding => code::INVALID_ENCODING,
            DiagnosticKind::UnsupportedEncoding => code::UNSUPPORTED_ENCODING,
            DiagnosticKind::MisplacedTag => code::HTML_STRUCTURE_ERROR,
            DiagnosticKind::UnexpectedEndTag | DiagnosticKind::TagMismatch => {
                code::TAG_NAME_MISMATCH
            }
            DiagnosticKind::ResourceLimit => code::RESOURCE_LIMIT,
        };
        diagnostic::Diagnostic {
            domain: self.domain,
            code,
            level: self.level,
            line: lines.text_line(self.mark),
            column: lines.text_column(self.offset),
            message: self.message.clone(),
            ..diagnostic::Diagnostic::default()
        }
    }

    /// The report `xmllint --html` prints for this diagnostic in `file`,
    /// whose input `lines` indexes.
    pub fn render(&self, file: &str, lines: &LineIndex<'_>) -> String {
        // Unconverted input shows its bytes at an invalid sequence.
        let bytes = self.kind == DiagnosticKind::InvalidEncoding && self.domain == Domain::Html;
        self.describe(lines).header(file) + &lines.html_context(self.offset, self.early, bytes)
    }
}

/// One libxml2 SAX callback (`htmlSAXHandler`). Names are lowercase; text
/// arrives in libxml2's pieces, so adjacent `Text` events may continue one
/// another.
#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Event<'a> {
    /// `internalSubset`, for every DOCTYPE wherever it appears; an empty
    /// name stands for a DOCTYPE without one.
    Doctype(crate::Doctype<'a>),
    /// `startElement`, including implied elements; valueless attributes
    /// have no value, the first of a repeated name is kept.
    Start {
        /// The element name.
        name: &'a str,
        /// `(name, value)` pairs in source order.
        attributes: &'a [(String, Option<String>)],
    },
    /// `endElement`.
    End(&'a str),
    /// `characters`.
    Text(&'a str),

    /// `cdataBlock`: raw text of `script`, `style` and similar elements.
    Cdata(&'a str),
    /// `comment`.
    Comment(&'a str),
}

/// A parsed HTML document and what recovery reported.
#[derive(Debug)]
pub struct Parsed {
    /// The document, marked as HTML.
    pub document: OwnedDocument,
    /// What recovery reported, in input order.
    pub diagnostics: Vec<Diagnostic>,
    /// A resource ceiling stopped parsing early; the tree holds what was built.
    pub stopped: bool,
}

/// What event parsing reported.
#[derive(Debug)]
#[non_exhaustive]
pub struct Summary {
    /// What recovery reported, in input order.
    pub diagnostics: Vec<Diagnostic>,
    /// A resource ceiling or the handler stopped parsing early.
    pub stopped: bool,
    decoding: Decoding,
}

impl Summary {
    /// The line index of `input`, the parsed bytes, as the parser decoded
    /// them: lines and reports for [`Diagnostic`]s.
    pub fn line_index<'s>(&self, input: &'s [u8]) -> LineIndex<'s> {
        LineIndex::decoded_html(input, &self.decoding)
    }
}

/// How the parser turned its input into the text offsets point into: a
/// byte-order mark skipped, a first converter, and at most one switch from
/// unconverted input to another converter at a text offset (a `<meta>`
/// charset or the windows-1252/prescan guess). Lines are counted from it
/// only when asked for.
#[derive(Clone, Debug)]
pub(crate) struct Decoding {
    skip: usize,
    initial: Decoder,
    switch: Option<(usize, Decoder)>,
}

impl Decoding {
    /// The decoded text, and the unconverted text when a switch re-decoded
    /// part of it.
    pub(crate) fn text<'a>(&self, input: &'a [u8]) -> (Cow<'a, [u8]>, Option<&'a [u8]>) {
        let body = &input[self.skip.min(input.len())..];
        let decode = |decoder: &Decoder, bytes: &[u8], text: &mut Vec<u8>| {
            decoder.fresh().decode(bytes, true, text);
        };
        match (&self.initial, &self.switch) {
            (Decoder::Utf8, None) => (Cow::Borrowed(body), None),
            (Decoder::Utf8, Some((at, decoder))) => {
                let at = (*at).min(body.len());
                let mut text = body[..at].to_vec();
                decode(decoder, &body[at..], &mut text);
                (Cow::Owned(text), Some(body))
            }
            (decoder, _) => {
                let mut text = Vec::new();
                decode(decoder, body, &mut text);
                (Cow::Owned(text), None)
            }
        }
    }
}

/// Parses a complete HTML document, like `htmlReadMemory`.
pub fn parse(input: &[u8], options: Options) -> Result<Parsed, Error> {
    let mut engine = Engine::new(options, document(options)?, false)?;
    engine.whole(Cow::Borrowed(input))?;
    engine.document()
}

/// An empty HTML document to build with
/// [`OwnedDocument::edit`](crate::OwnedDocument::edit), like
/// `htmlNewDocNoDtD(uri, public_id)`: an `html` DOCTYPE when either id is
/// given, else none. `htmlNewDoc(NULL, NULL)` passes the HTML 4.0
/// Transitional ids that [`parse`] implies.
///
/// ```
/// use yaxml::{EditLimits, html};
/// let source = html::parse(b"<p>x", html::Options::default())?.document;
/// let mut document = html::create(None, Some("-//P"))?;
/// let root = document.root().id();
/// let mut editor = document.edit(EditLimits::default())?;
/// let copy = editor.import(&source, source.root_element().id(), true)?;
/// editor.append_child(root, copy)?; // the document element
/// let saved = html::to_html(&document)?;
/// let expected = "<!DOCTYPE html PUBLIC \"-//P\">\n<html><body><p>x</p></body></html>\n";
/// assert_eq!(saved, expected.as_bytes());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Errors
///
/// Only allocation failure.
pub fn create(uri: Option<&str>, public_id: Option<&str>) -> Result<OwnedDocument, Error> {
    let doctype = (uri.is_some() || public_id.is_some()).then_some(crate::Doctype {
        name: "html",
        public_id,
        system_id: uri,
        internal_subset: None,
    });
    let decoding = Decoding {
        skip: 0,
        initial: Decoder::Utf8,
        switch: None,
    };
    HtmlBuilder::new(0, Limits::default())?.finish(doctype, None, decoding)
}

/// Parses `input` as content of an HTML node, like `xmlCtxtParseContent`.
///
/// Nothing is implied and a DOCTYPE is ignored; the nodes become the
/// returned document's top-level children, ready for
/// [`Editor::import`](crate::Editor::import). The input is decoded as
/// `encoding` (the context's [`OwnedDocument::html_encoding`]), or else
/// sniffed from `<meta>` or the first non-ASCII byte, never a byte-order
/// mark.
pub fn parse_fragment(
    input: &[u8],
    encoding: Option<&str>,
    options: Options,
) -> Result<Parsed, Error> {
    let mut engine = Engine::new(options, document(options)?.fragment(), true)?;
    engine.count(input.len())?;
    let decoder = encoding.and_then(Decoder::for_label);
    engine.encoding = encoding.filter(|_| decoder.is_some()).map(str::to_owned);
    engine.tokenizer.has_encoding = decoder.is_some();
    engine.begin(
        decoder.unwrap_or(Decoder::Utf8),
        Cow::Borrowed(input),
        0,
        true,
    );
    engine.run(true)?;
    engine.document()
}

/// Parses a complete HTML document into events without building a tree,
/// like `htmlCtxtReadMemory` with SAX callbacks. `handler` may stop parsing
/// by returning [`ControlFlow::Break`].
///
/// ```
/// use std::ops::ControlFlow;
/// use yaxml::html::{self, Event};
///
/// let input = b"<p>a</i>";
/// let mut tags = Vec::new();
/// let summary = html::parse_events(input, html::Options::default(), |event| {
///     if let Event::Start { name, .. } = event {
///         tags.push(name.to_owned());
///     }
///     ControlFlow::Continue(())
/// })?;
/// assert_eq!(tags, ["html", "body", "p"]);
/// assert_eq!(
///     summary.diagnostics[0].render("f.html", &summary.line_index(input)),
///     "f.html:1: HTML parser error : Unexpected end tag : i\n<p>a</i>\n        ^\n"
/// );
/// # Ok::<(), yaxml::Error>(())
/// ```
pub fn parse_events<F>(input: &[u8], options: Options, handler: F) -> Result<Summary, Error>
where
    F: FnMut(Event<'_>) -> ControlFlow<()>,
{
    let mut engine = Engine::new(options, Events::new(handler), false)?;
    engine.whole(Cow::Borrowed(input))?;
    Ok(engine.summary())
}

/// The tree builder `options` configure.
fn document(options: Options) -> Result<Document, Error> {
    let (value_limit, _, max_depth) = ceilings(&options);
    let builder = HtmlBuilder::new(0, options.limits)?;
    Ok(Document::new(builder, max_depth, value_limit))
}

/// Value, name and depth ceilings, raised by [`HUGE`].
fn ceilings(options: &Options) -> (usize, usize, usize) {
    let limits = options.limits;
    if options.has(HUGE) {
        let value = limits.max_token_bytes.max(1_000_000_000);
        (value, value, limits.max_depth.max(2048))
    } else {
        let value = limits.max_token_bytes;
        (value, value.min(50_000), limits.max_depth)
    }
}

/// Incremental parsing, like `htmlParseChunk`. The resulting document does
/// not depend on how the input is split.
pub struct PushParser(Push<Document>);

impl PushParser {
    /// A parser for `options`.
    ///
    /// # Errors
    ///
    /// Unsupported flags.
    pub fn new(options: Options) -> Result<Self, Error> {
        Ok(Self(Push::new(Engine::new(
            options,
            document(options)?,
            false,
        )?)))
    }

    /// Consumes a chunk. Errors are terminal; later calls return `InvalidState`.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<(), Error> {
        self.0.feed(chunk)
    }

    /// Signals the end of input and returns the document.
    pub fn finish(self) -> Result<Parsed, Error> {
        self.0.finish()?.document()
    }
}

/// Incremental event parsing, like `htmlParseChunk` with custom SAX
/// callbacks. Events do not depend on how the input is split.
pub struct EventParser<F: FnMut(Event<'_>) -> ControlFlow<()>>(Push<Events<F>>);

impl<F: FnMut(Event<'_>) -> ControlFlow<()>> EventParser<F> {
    /// A parser delivering events to `handler`.
    ///
    /// # Errors
    ///
    /// Unsupported flags.
    pub fn new(options: Options, handler: F) -> Result<Self, Error> {
        Ok(Self(Push::new(Engine::new(
            options,
            Events::new(handler),
            false,
        )?)))
    }

    /// Consumes a chunk. Errors are terminal; later calls return `InvalidState`.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<(), Error> {
        self.0.feed(chunk)
    }

    /// Signals the end of input.
    pub fn finish(self) -> Result<Summary, Error> {
        Ok(self.0.finish()?.summary())
    }
}

/// The push driver both push parsers share.
struct Push<S: Sink> {
    engine: Engine<'static, S>,
    raw: Vec<u8>,
    failed: bool,
}

impl<S: Sink> Push<S> {
    fn new(engine: Engine<'static, S>) -> Self {
        Self {
            engine,
            raw: Vec::new(),
            failed: false,
        }
    }

    fn feed(&mut self, chunk: &[u8]) -> Result<(), Error> {
        if self.failed {
            return Err(Error::new(ErrorKind::InvalidState, self.engine.offset()));
        }
        let result = self.push(chunk);
        self.failed = result.is_err();
        result
    }

    fn push(&mut self, chunk: &[u8]) -> Result<(), Error> {
        self.engine.count(chunk.len())?;
        if self.engine.decoder.is_none() {
            // Sniffing needs the first four bytes.
            self.raw.extend_from_slice(chunk);
            if self.raw.len() < 4 {
                return Ok(());
            }
            self.engine
                .start(Cow::Owned(std::mem::take(&mut self.raw)), false);
        } else {
            self.engine.append(chunk, false);
        }
        self.engine.run(false)?;
        self.engine.compact();
        Ok(())
    }

    fn finish(mut self) -> Result<Engine<'static, S>, Error> {
        if self.failed {
            return Err(Error::new(ErrorKind::InvalidState, self.engine.offset()));
        }
        if self.engine.decoder.is_none() {
            self.engine
                .start(Cow::Owned(std::mem::take(&mut self.raw)), true);
        } else {
            self.engine.append(&[], true);
        }
        self.engine.run(true)?;
        Ok(self.engine)
    }
}

/// libxml2's I/O error for malformed converted input, raised where the parser
/// is: at the token holding the first malformed character, so that it does
/// not depend on how much input was converted. Reported once, as invalid
/// UTF-8 is.
fn converted<S: Sink>(tree: &mut Tree<S>, invalid: &mut bool, offset: usize) {
    if !std::mem::replace(invalid, true) {
        let message = "Invalid bytes in character encoding".to_owned();
        let kind = DiagnosticKind::InvalidEncoding;
        tree.report(kind, offset, offset, message, (Domain::Io, Level::Fatal));
    }
}

/// Invalid input met by the step at `tree.at`: a converter's first malformed
/// character inside it, or invalid UTF-8 where the scan started at `invalid`
/// (`text`: a character-data step, whose line is the run's).
#[cold]
#[inline(never)]
fn irregular<S: Sink>(
    tree: &mut Tree<S>,
    reported: &mut bool,
    malformed: &mut Option<usize>,
    invalid: Option<usize>,
    text: bool,
) {
    let at = tree.at;
    if malformed.is_some_and(|malformed| malformed < at.end) {
        *malformed = None;
        converted(tree, reported, at.start);
    }
    if let Some(invalid) = invalid.filter(|_| !*reported) {
        *reported = true;
        let offset = at.start + invalid;
        // Within a character-data run libxml2's line lags.
        let mark = if text {
            tree.run.unwrap_or(at.start)
        } else {
            offset
        };
        let message = "Invalid bytes in character encoding".to_owned();
        let raised = (Domain::Html, Level::Error);
        tree.report(
            DiagnosticKind::InvalidEncoding,
            offset,
            mark,
            message,
            raised,
        );
    }
}

/// Shared pull/push driver: decoded text, the tokenizer and the tree.
struct Engine<'a, S: Sink> {
    text: Cow<'a, [u8]>,
    pos: usize,
    /// Text bytes compacted away, for diagnostic offsets.
    dropped: usize,
    decoder: Option<Decoder>,
    tokenizer: Tokenizer,
    tree: Tree<S>,
    options: Options,
    /// `doc->encoding`.
    encoding: Option<String>,
    input_bytes: usize,
    /// Rescan an incomplete token only once this much text is pending, so
    /// small chunks cost amortized linear time.
    retry: usize,
    decoding: Decoding,
    /// Invalid input was reported; libxml2 reports it once.
    invalid: bool,
    /// Where converted text is first malformed, until reported.
    malformed: Option<usize>,
    stopped: bool,
}

impl<'a, S: Sink> Engine<'a, S> {
    fn new(options: Options, sink: S, fragment: bool) -> Result<Self, Error> {
        if options.flags & !SUPPORTED != 0 {
            return Err(Error::new(ErrorKind::UnsupportedOption, 0));
        }
        let (value_limit, name_limit, _) = ceilings(&options);
        let settings = Settings {
            // In context, libxml2 adds no implied elements.
            implied: !options.has(NOIMPLIED) && !fragment,
            keep_blanks: !options.has(NOBLANKS),
            report: !options.has(NOERROR),
            html5: options.has(HTML5),
        };
        let mut tree = Tree::new(sink, settings);
        let mut tokenizer = Tokenizer::new(value_limit, name_limit, options.limits.max_attributes);
        if fragment {
            tree.fragment();
            tokenizer.fragment();
        }
        Ok(Self {
            text: Cow::Borrowed(&[]),
            pos: 0,
            dropped: 0,
            decoder: None,
            tokenizer,
            tree,
            options,
            encoding: None,
            input_bytes: 0,
            retry: 0,
            decoding: Decoding {
                skip: 0,
                initial: Decoder::Utf8,
                switch: None,
            },
            invalid: false,
            malformed: None,
            stopped: false,
        })
    }

    fn offset(&self) -> usize {
        self.dropped + self.pos
    }

    fn count(&mut self, bytes: usize) -> Result<(), Error> {
        self.input_bytes = self
            .input_bytes
            .checked_add(bytes)
            .filter(|total| *total <= self.options.limits.max_input_bytes)
            .ok_or(Error::new(
                ErrorKind::LimitExceeded(Limit::InputBytes),
                self.input_bytes,
            ))?;
        Ok(())
    }

    /// Parses a complete input.
    fn whole(&mut self, input: Cow<'a, [u8]>) -> Result<(), Error> {
        self.count(input.len())?;
        self.start(input, true);
        self.run(true)
    }

    /// Sniffs the first bytes and starts decoding.
    fn start(&mut self, input: Cow<'a, [u8]>, last: bool) {
        let sniffed = sniff(&input);
        self.tokenizer.has_encoding = sniffed.has_encoding;
        self.encoding = sniffed.name.map(str::to_owned);
        self.begin(sniffed.decoder, input, sniffed.bom, last);
    }

    /// Starts decoding after `skip` bytes.
    fn begin(&mut self, mut decoder: Decoder, input: Cow<'a, [u8]>, skip: usize, last: bool) {
        self.decoding.skip = skip;
        self.decoding.initial = decoder.fresh();
        let mut malformed = None;
        self.text = match (&mut decoder, input) {
            (Decoder::Utf8, Cow::Borrowed(input)) => Cow::Borrowed(&input[skip..]),
            (Decoder::Utf8, Cow::Owned(mut input)) => {
                input.drain(..skip);
                Cow::Owned(input)
            }
            (decoder, input) => {
                let mut text = Vec::new();
                malformed = decoder.decode(&input[skip..], last, &mut text);
                Cow::Owned(text)
            }
        };
        self.decoder = Some(decoder);
        self.malformed(malformed);
    }

    fn append(&mut self, chunk: &[u8], last: bool) {
        if let Some(decoder) = &mut self.decoder {
            let malformed = decoder.decode(chunk, last, self.text.to_mut());
            self.malformed(malformed);
        }
    }

    /// Records where a converter first met malformed input (a text offset).
    fn malformed(&mut self, at: Option<usize>) {
        self.malformed = self.malformed.or(at.map(|at| self.dropped + at));
    }

    /// Drops consumed push input once it dominates the buffer.
    fn compact(&mut self) {
        if self.pos > 4096 && self.pos > self.text.len() / 2 {
            self.text.to_mut().drain(..self.pos);
            self.dropped += self.pos;
            self.pos = 0;
        }
    }

    /// Re-decodes the text from `at` with a newly chosen encoding.
    fn switch(&mut self, decoder: Decoder, at: usize, last: bool) {
        self.tokenizer.has_encoding = true;
        let mut decoder = decoder;
        let mut malformed = None;
        if !matches!(decoder, Decoder::Utf8) {
            // Earlier reports show the text as it was.
            for diagnostic in &mut self.tree.diagnostics {
                diagnostic.early = true;
            }
            self.decoding.switch = Some((self.dropped + at, decoder.fresh()));
            self.malformed = self.malformed.filter(|&m| m < self.dropped + at);
            let tail = self.text[at..].to_vec();
            let text = self.text.to_mut();
            text.truncate(at);
            malformed = decoder.decode(&tail, last, text);
        }
        self.decoder = Some(decoder);
        self.malformed(malformed);
    }

    /// The first non-ASCII byte without a known encoding: htmlFindEncoding,
    /// else windows-1252. An unusable guess leaves the bytes as UTF-8.
    fn fallback(&mut self, at: usize) {
        let (decoder, name) = match prescan(&self.text[at..]) {
            None => (Decoder::Byte(WINDOWS_1252), "windows-1252".to_owned()),
            Some(label) => match Decoder::for_label(&label) {
                Some(Decoder::Utf8) | None => (Decoder::Utf8, "UTF-8".to_owned()),
                Some(decoder) => (decoder, label),
            },
        };
        self.encoding = Some(name);
        self.switch(decoder, at, true);
    }

    /// `htmlCheckMeta` and `xmlSetDeclaredEncoding` for a `<meta>` charset,
    /// which libxml2 checks with its input at the tag's `>`.
    fn declare(&mut self, label: String, last: bool, at: Place) {
        let gt = at.end - 1;
        match Decoder::for_label(&label) {
            Some(decoder) if decoder.ascii_compatible() => {
                self.encoding = Some(label);
                self.switch(decoder, self.pos, last);
            }
            Some(_) => {
                let message = format!("Encoding {label} isn't ASCII-compatible");
                self.error(DiagnosticKind::UnsupportedEncoding, gt, message);
            }
            None => {
                let message = format!("Unsupported encoding: {label}");
                self.error(DiagnosticKind::UnsupportedEncoding, gt, message);
            }
        }
    }

    fn error(&mut self, kind: DiagnosticKind, offset: usize, message: String) {
        let raised = (Domain::Html, Level::Error);
        self.tree.report(kind, offset, offset, message, raised);
    }

    fn located(&self, error: Error) -> Error {
        Error::new(error.kind, self.offset())
    }

    /// Where the token `used` bytes long, after `skipped` blanks, lies.
    fn place(&self, skipped: usize, used: usize) -> Place {
        Place {
            start: self.offset() + skipped,
            end: self.offset() + used,
        }
    }

    fn run(&mut self, eof: bool) -> Result<(), Error> {
        let result = self.steps(eof);
        // Malformed input after the last token, such as a cut-off unit.
        if eof && !self.stopped && self.malformed.take().is_some() {
            let offset = self.offset();
            converted(&mut self.tree, &mut self.invalid, offset);
        }
        result
    }

    fn steps(&mut self, eof: bool) -> Result<(), Error> {
        while !self.stopped {
            let rest = &self.text[self.pos..];
            let available = rest.len();
            if !eof && available < self.retry {
                return Ok(());
            }
            let step = self.tokenizer.next(rest, eof);
            let invalid = self.tokenizer.invalid.take();
            // The prolog's blanks are skipped before its tokens.
            let skipped = std::mem::take(&mut self.tokenizer.skipped);
            match step {
                Ok(None) => return Ok(()),
                Ok(Some((None, used))) => {
                    if invalid.is_some() || self.malformed.is_some() {
                        self.tree.at = self.place(skipped, used);
                        irregular(
                            &mut self.tree,
                            &mut self.invalid,
                            &mut self.malformed,
                            invalid,
                            false,
                        );
                    }
                    // A tokenless step of markup (`</>`) ends a run of
                    // character data; one of text (a dropped NUL) does not.
                    if self.text[self.pos + skipped] == b'<' {
                        self.tree.run = None;
                    }
                    self.pos += used;
                }
                Ok(Some((Some(token), mut used))) => {
                    let at = self.place(skipped, used);
                    self.tree.at = at;
                    if invalid.is_some() || self.malformed.is_some() {
                        let text = matches!(token, Token::Text { .. });
                        irregular(
                            &mut self.tree,
                            &mut self.invalid,
                            &mut self.malformed,
                            invalid,
                            text,
                        );
                    }
                    let charset = match &token {
                        Token::StartTag {
                            name,
                            attributes,
                            end,
                            ..
                        } if name == "meta"
                            && *end != TagEnd::Eof
                            && !self.tokenizer.has_encoding
                            && !self.options.has(IGNORE_ENC) =>
                        {
                            meta_charset(attributes)
                        }
                        _ => None,
                    };
                    let reaction = self.tree.consume(token)?;
                    self.retry = 0;
                    match reaction {
                        Reaction::Continue => {}
                        Reaction::Enter(mode) => {
                            self.tokenizer.enter(mode, self.tree.top_name());
                        }
                        // The tag's end is read again, as text.
                        Reaction::Reread(bytes) => used -= bytes,
                        Reaction::Halt => self.stopped = true,
                    }
                    self.pos += used;
                    if let Some(label) = charset {
                        self.declare(label, eof, at);
                    }
                }
                Err(Stop::More) => {
                    self.retry = available.saturating_mul(2).max(1);
                    return Ok(());
                }
                // Push input waits for EOF so the prescan sees the whole rest.
                Err(Stop::Switch(_)) if !eof => {
                    self.retry = usize::MAX;
                    return Ok(());
                }
                Err(Stop::Switch(at)) => self.fallback(self.pos + at),
                Err(Stop::TooLong) => {
                    let (offset, raised) = (self.offset(), (Domain::Html, Level::Error));
                    self.tree
                        .halt(offset, offset, "value too long".to_owned(), raised);
                    self.stopped = true;
                }
                Err(Stop::Attributes) => {
                    return Err(
                        self.located(Error::new(ErrorKind::LimitExceeded(Limit::Attributes), 0))
                    );
                }
            }
        }
        Ok(())
    }

    fn summary(self) -> Summary {
        let (_, diagnostics, stopped) = self.tree.finish();
        Summary {
            diagnostics,
            stopped,
            decoding: self.decoding,
        }
    }
}

impl Engine<'_, Document> {
    fn document(self) -> Result<Parsed, Error> {
        let default_doctype = !self.options.has(NODEFDTD);
        let (document, diagnostics, stopped) = self.tree.finish();
        Ok(Parsed {
            document: document.finish(default_doctype, self.encoding, self.decoding)?,
            diagnostics,
            stopped,
        })
    }
}

#[cfg(test)]
#[path = "../tests/internal/html_events.rs"]
mod tests;
