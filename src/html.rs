//! HTML parsing and serialization with libxml2 2.15.4's behavior.
//!
//! The tokenizer follows libxml2's HTML5-style tokenization; tree construction
//! is libxml2's own legacy algorithm (implied html/head/body, start-tag
//! auto-close tables, end-tag priorities), not the WHATWG tree builder.
//! Malformed input is recovered, as libxml2 always does for HTML, and the
//! problems libxml2 reports are collected as [`Diagnostic`]s. Documents are
//! [`OwnedDocument`]s marked as HTML; [`parse_events`] and [`EventParser`]
//! deliver libxml2's SAX callbacks as [`Event`]s instead. See `docs/html.md`
//! for the contract.
//!
//! ```
//! use yaxml::html;
//!
//! let parsed = html::parse(b"<p class=x>Hello<br>world", html::Options::default())?;
//! let body = parsed.document.root_element().last_child().unwrap();
//! assert_eq!(body.name().unwrap().local(), "body");
//! assert_eq!(
//!     html::save(&parsed.document, false)?,
//!     b"<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.0 Transitional//EN\" \
//!       \"http://www.w3.org/TR/REC-html40/loose.dtd\">\n\
//!       <html><body><p class=\"x\">Hello<br>world</p></body></html>\n"
//! );
//! # Ok::<(), yaxml::Error>(())
//! ```

use std::borrow::Cow;
use std::ops::ControlFlow;

use crate::diagnostic::{self, Domain, Level, code};
use crate::encoding::WINDOWS_1252;
use crate::owned::HtmlBuilder;
use crate::{Error, ErrorKind, Limit, Limits, OwnedDocument};

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
pub use serialize::{save, to_html};
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

/// `htmlParserOption` flags plus resource ceilings.
///
/// `max_depth` is libxml2's open-element ceiling for trees (256 by default,
/// 2048 with [`HUGE`]) and `max_token_bytes` bounds attribute values,
/// comments and merged text nodes; exceeding either stops parsing with a
/// [`DiagnosticKind::ResourceLimit`] and keeps the tree built so far, as
/// libxml2 does. Events have no depth ceiling, as with libxml2's SAX. The
/// other ceilings (input bytes, attributes per tag, nodes as `max_events`,
/// owned bytes) fail with [`ErrorKind::LimitExceeded`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Options {
    pub flags: u32,
    pub limits: Limits,
}

setters!(const Options { flags: u32, limits: Limits });

impl Default for Options {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Options {
    pub const DEFAULT: Self = Self::new(0);

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

/// A problem libxml2 reports while recovering, where its input was then:
/// `offset` counts bytes of the decoded UTF-8 text, `line` is libxml2's line
/// and `column` counts characters from that line's start.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub offset: usize,
    pub line: usize,
    pub column: usize,
    /// libxml2's message, without its trailing newline.
    pub message: String,
    domain: Domain,
    level: Level,
    /// The context lines `xmlFormatError` prints after the message.
    context: String,
}

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
        line: usize,
        message: String,
        (domain, level): (Domain, Level),
    ) -> Self {
        Self {
            kind,
            offset,
            line,
            column: 0,
            message,
            domain,
            level,
            context: String::new(),
        }
    }

    /// libxml2's `xmlError` for this diagnostic.
    pub fn describe(&self) -> diagnostic::Diagnostic {
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
            line: self.line,
            column: self.column,
            message: self.message.clone(),
            ..diagnostic::Diagnostic::default()
        }
    }

    /// The report `xmllint --html` prints for this diagnostic in `file`.
    pub fn render(&self, file: &str) -> String {
        self.describe().header(file) + &self.context
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
        name: &'a str,
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
    pub document: OwnedDocument,
    pub diagnostics: Vec<Diagnostic>,
    /// A resource ceiling stopped parsing early; the tree holds what was built.
    pub stopped: bool,
}

/// What event parsing reported.
#[derive(Debug)]
#[non_exhaustive]
pub struct Summary {
    pub diagnostics: Vec<Diagnostic>,
    /// A resource ceiling or the handler stopped parsing early.
    pub stopped: bool,
}

/// Parses a complete HTML document, like `htmlReadMemory`.
pub fn parse(input: &[u8], options: Options) -> Result<Parsed, Error> {
    let mut engine = Engine::new(options, document(options)?, false)?;
    engine.whole(Cow::Borrowed(input))?;
    engine.document()
}

/// Parses `input` as content in an HTML document, like
/// `xmlParseInNodeContext`/`xmlCtxtParseContent` on an HTML node: no implied
/// elements, a DOCTYPE is ignored, and the nodes become the top-level
/// children of the returned document (copy them over with
/// [`Editor::import`](crate::Editor::import)). The input is read in the
/// context document's `encoding` ([`OwnedDocument::html_encoding`]); without
/// one it is sniffed from `<meta>` or the first non-ASCII byte, never from a
/// byte-order mark.
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

/// Parses a complete HTML document into events, like `htmlCtxtReadMemory`
/// with custom SAX callbacks: `handler` receives each one and may stop
/// parsing by returning [`ControlFlow::Break`]. No tree is built.
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
pub struct EventParser<F>(Push<Events<F>>);

impl<F: FnMut(Event<'_>) -> ControlFlow<()>> EventParser<F> {
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
struct Push<S> {
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
fn converted<S: Sink>(tree: &mut Tree<S>, invalid: &mut bool, offset: usize, line: usize) {
    if !std::mem::replace(invalid, true) {
        let message = "Invalid bytes in character encoding".to_owned();
        let kind = DiagnosticKind::InvalidEncoding;
        tree.report(kind, offset, line, message, (Domain::Io, Level::Fatal));
    }
}

/// Line feeds in `text`. Most tokens are short, where a plain count (which
/// vectorizes) beats `memchr_iter`'s setup.
fn newlines(text: &[u8]) -> usize {
    text.iter().filter(|&&byte| byte == b'\n').count()
}

/// Line counting over the decoded text, up to `counted`, and the last
/// column computed, so columns cost the distance between diagnostics.
struct Lines {
    /// The line `counted` is on.
    line: usize,
    counted: usize,
    /// A text offset and its 1-based column.
    column: (usize, usize),
}

/// Shared pull/push driver: decoded text, the tokenizer and the tree.
struct Engine<'a, S> {
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
    lines: Lines,
    /// The current character-data run started on this line.
    run_line: Option<usize>,
    /// Invalid input was reported; libxml2 reports it once.
    invalid: bool,
    /// Where converted text is first malformed, until reported.
    malformed: Option<usize>,
    /// Diagnostics given a column so far.
    annotated: usize,
    /// Diagnostics still waiting for the text their context shows, and
    /// whether it starts with the input bytes (unconverted input).
    pending: Vec<(usize, bool)>,
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
            lines: Lines {
                line: 1,
                counted: 0,
                column: (0, 1),
            },
            run_line: None,
            invalid: false,
            malformed: None,
            annotated: 0,
            pending: Vec::new(),
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
        if self.pos > 4096 && self.pos > self.text.len() / 2 && self.pending.is_empty() {
            // Later columns count from here.
            self.column_at(self.offset());
            self.text.to_mut().drain(..self.pos);
            self.dropped += self.pos;
            self.pos = 0;
        }
    }

    /// The decoded text from global offset `from` to `to`.
    fn span(&self, from: usize, to: usize) -> &[u8] {
        &self.text[from - self.dropped..to - self.dropped]
    }

    /// libxml2's line at `to`, at or after everything counted.
    fn line_at(&self, to: usize) -> usize {
        self.lines.line + newlines(self.span(self.lines.counted, to))
    }

    /// 1-based characters from the start of `to`'s line, counted from the
    /// last column computed.
    fn column_at(&mut self, to: usize) -> usize {
        let chars = |bytes: &[u8]| bytes.iter().filter(|&&b| b & 0xC0 != 0x80).count();
        let (from, column) = self.lines.column;
        let column = if to >= from {
            let span = self.span(from, to);
            memchr::memrchr(b'\n', span).map_or(column + chars(span), |newline| {
                1 + chars(&span[newline + 1..])
            })
        } else if memchr::memchr(b'\n', self.span(to, from)).is_none() {
            column - chars(self.span(to, from))
        } else {
            let before = self.span(self.dropped, to);
            1 + chars(
                memchr::memrchr(b'\n', before).map_or(before, |newline| &before[newline + 1..]),
            )
        };
        self.lines.column = (to, column);
        column
    }

    /// Gives new diagnostics their column; their context follows once the
    /// text it shows is there.
    fn annotate(&mut self) {
        let utf8 = matches!(self.decoder, None | Some(Decoder::Utf8));
        for index in self.annotated..self.tree.diagnostics.len() {
            let diagnostic = &self.tree.diagnostics[index];
            let (offset, invalid) = (
                diagnostic.offset,
                diagnostic.kind == DiagnosticKind::InvalidEncoding,
            );
            let column = self.column_at(offset);
            let bytes = invalid && utf8;
            self.tree.diagnostics[index].column = column;
            self.pending.push((index, bytes));
        }
        self.annotated = self.tree.diagnostics.len();
    }

    /// `xmlParserInputGetWindow`'s lines for pending diagnostics: up to 80
    /// bytes of text on each side, so push input waits for 84 more bytes or
    /// a line end.
    fn contextualize(&mut self, eof: bool) {
        if self.pending.is_empty() {
            return;
        }
        let mut pending = std::mem::take(&mut self.pending);
        pending.retain(|&(index, bytes)| {
            let at = self.tree.diagnostics[index].offset - self.dropped;
            let after = &self.text[at.min(self.text.len())..];
            let ready = eof
                || after.len() >= 84
                || memchr::memchr2(b'\n', b'\r', &after[..after.len().min(84)]).is_some();
            if ready {
                let from = at.saturating_sub(100);
                let to = (at + 100).min(self.text.len());
                self.tree.diagnostics[index].context =
                    diagnostic::context(&self.text[from..to], at - from, bytes);
            }
            !ready
        });
        self.pending = pending;
    }

    /// Re-decodes the text from `at` with a newly chosen encoding.
    fn switch(&mut self, decoder: Decoder, at: usize, last: bool) {
        self.tokenizer.has_encoding = true;
        let mut decoder = decoder;
        let mut malformed = None;
        if !matches!(decoder, Decoder::Utf8) {
            // Earlier reports show the text as it was.
            self.contextualize(true);
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
        let gt = (at.end - 1, at.end_line);
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

    fn error(&mut self, kind: DiagnosticKind, (offset, line): (usize, usize), message: String) {
        let raised = (Domain::Html, Level::Error);
        self.tree.report(kind, offset, line, message, raised);
    }

    fn located(&self, error: Error) -> Error {
        Error::new(error.kind, self.offset())
    }

    /// Where the token `used` bytes long, after `skipped` blanks, lies.
    fn place(&self, skipped: usize, used: usize) -> Place {
        let start = self.offset() + skipped;
        let end = self.offset() + used;
        let line = self.lines.line + newlines(self.span(self.offset(), start));
        let end_line = line + newlines(self.span(start, end));
        Place {
            start,
            end,
            line,
            end_line,
            run_line: self.run_line.unwrap_or(line),
        }
    }

    fn run(&mut self, eof: bool) -> Result<(), Error> {
        let result = self.steps(eof);
        // Malformed input after the last token, such as a cut-off unit.
        if eof && !self.stopped && self.malformed.take().is_some() {
            let (offset, line) = (self.offset(), self.line_at(self.offset()));
            converted(&mut self.tree, &mut self.invalid, offset, line);
            self.annotate();
        }
        self.contextualize(eof);
        result
    }

    fn steps(&mut self, eof: bool) -> Result<(), Error> {
        while !self.stopped {
            let rest = &self.text[self.pos..];
            let available = rest.len();
            if !eof && available < self.retry {
                return Ok(());
            }
            self.tokenizer.invalid.set(None);
            let step = self.tokenizer.next(rest, eof);
            let invalid = self.tokenizer.invalid.take();
            match step {
                Ok(None) => return Ok(()),
                Ok(Some((token, mut used))) => {
                    let at = self.place(self.tokenizer.skipped, used);
                    self.run_line = self.tokenizer.text_step.then_some(at.run_line);
                    self.tree.at = at;
                    if self.malformed.is_some_and(|malformed| malformed < at.end) {
                        self.malformed = None;
                        converted(&mut self.tree, &mut self.invalid, at.start, at.line);
                    }
                    if let Some(invalid) = invalid.filter(|_| !self.invalid) {
                        self.invalid = true;
                        let offset = self.offset() + invalid;
                        let line = if self.tokenizer.text_step {
                            at.run_line
                        } else {
                            self.line_at(offset)
                        };
                        let message = "Invalid bytes in character encoding".to_owned();
                        let raised = (Domain::Html, Level::Error);
                        let kind = DiagnosticKind::InvalidEncoding;
                        self.tree.report(kind, offset, line, message, raised);
                    }
                    let mut charset = None;
                    if let Some(token) = token {
                        charset = match &token {
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
                    }
                    self.pos += used;
                    // A reread tag end holds no line feed.
                    self.lines.line = at.end_line;
                    self.lines.counted = self.offset();
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
                    let (offset, line) = (self.offset(), self.line_at(self.offset()));
                    let raised = (Domain::Html, Level::Error);
                    self.tree
                        .halt(offset, line, "value too long".to_owned(), raised);
                    self.stopped = true;
                }
                Err(Stop::Attributes) => {
                    return Err(
                        self.located(Error::new(ErrorKind::LimitExceeded(Limit::Attributes), 0))
                    );
                }
            }
            // libxml2 shows the text as it is when it reports.
            if self.annotated < self.tree.diagnostics.len() || !self.pending.is_empty() {
                self.annotate();
                self.contextualize(eof);
            }
        }
        Ok(())
    }

    fn summary(self) -> Summary {
        let (_, diagnostics, stopped) = self.tree.finish();
        Summary {
            diagnostics,
            stopped,
        }
    }
}

impl Engine<'_, Document> {
    fn document(self) -> Result<Parsed, Error> {
        let default_doctype = !self.options.has(NODEFDTD);
        let (document, diagnostics, stopped) = self.tree.finish();
        Ok(Parsed {
            document: document.finish(default_doctype, self.encoding)?,
            diagnostics,
            stopped,
        })
    }
}

#[cfg(test)]
#[path = "../tests/internal/html_events.rs"]
mod tests;
