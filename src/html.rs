//! HTML parsing and serialization with libxml2 2.15.4's behavior.
//!
//! The tokenizer follows libxml2's HTML5-style tokenization; tree construction
//! is libxml2's own legacy algorithm (implied html/head/body, start-tag
//! auto-close tables, end-tag priorities), not the WHATWG tree builder.
//! Malformed input is recovered, as libxml2 always does for HTML, and the
//! problems libxml2 reports are collected as [`Diagnostic`]s. Documents are
//! [`OwnedDocument`]s marked as HTML. See `docs/html.md` for the contract.
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
use tree::{Reaction, Settings, Tree};

pub use serialize::{save, to_html};

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
/// `HTML_PARSE_BIG_LINES`: accepted; nodes carry no line numbers.
pub const BIG_LINES: u32 = 1 << 22;
/// `HTML_PARSE_HTML5`: libxml2's HTML5 mode is not implemented; rejected.
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
    | BIG_LINES;

/// `htmlParserOption` flags plus resource ceilings.
///
/// `max_depth` is libxml2's open-element ceiling (256 by default, 2048 with
/// [`HUGE`]) and `max_token_bytes` bounds attribute values, comments and
/// merged text nodes; exceeding either stops parsing with a
/// [`DiagnosticKind::ResourceLimit`] and keeps the tree built so far, as
/// libxml2 does. The other ceilings (input bytes, attributes per tag, nodes
/// as `max_events`, owned bytes) fail with [`ErrorKind::LimitExceeded`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Options {
    pub flags: u32,
    pub limits: Limits,
}

impl Options {
    pub fn new(flags: u32) -> Self {
        Self {
            flags,
            limits: Limits::default(),
        }
    }

    fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }
}

/// A document type declaration (`xmlDtd` name and identifiers).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Doctype {
    pub name: Option<String>,
    pub public_id: Option<String>,
    pub system_id: Option<String>,
}

/// A problem libxml2 reports while recovering. Offsets count bytes of the
/// decoded UTF-8 text at the start of the token concerned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub offset: usize,
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

/// A parsed HTML document and what recovery reported.
#[derive(Debug)]
pub struct Parsed {
    pub document: OwnedDocument,
    pub diagnostics: Vec<Diagnostic>,
    /// A resource ceiling stopped parsing early; the tree holds what was built.
    pub stopped: bool,
}

/// Parses a complete HTML document, like `htmlReadMemory`.
pub fn parse(input: &[u8], options: Options) -> Result<Parsed, Error> {
    let mut engine = Engine::new(options)?;
    engine.count(input.len())?;
    engine.start(Cow::Borrowed(input), true);
    engine.run(true)?;
    engine.finish()
}

/// Incremental parsing, like `htmlParseChunk`. The resulting document does
/// not depend on how the input is split.
pub struct PushParser {
    engine: Engine<'static>,
    raw: Vec<u8>,
    failed: bool,
}

impl PushParser {
    pub fn new(options: Options) -> Result<Self, Error> {
        Ok(Self {
            engine: Engine::new(options)?,
            raw: Vec::new(),
            failed: false,
        })
    }

    /// Consumes a chunk. Errors are terminal; later calls return `InvalidState`.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<(), Error> {
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

    /// Signals the end of input and returns the document.
    pub fn finish(mut self) -> Result<Parsed, Error> {
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
        self.engine.finish()
    }
}

/// Shared pull/push driver: decoded text, the tokenizer and the tree.
struct Engine<'a> {
    text: Cow<'a, [u8]>,
    pos: usize,
    /// Text bytes compacted away, for diagnostic offsets.
    dropped: usize,
    decoder: Option<Decoder>,
    tokenizer: Tokenizer,
    tree: Tree,
    options: Options,
    /// `doc->encoding`.
    encoding: Option<String>,
    input_bytes: usize,
    /// Rescan an incomplete token only once this much text is pending, so
    /// small chunks cost amortized linear time.
    retry: usize,
    /// Where malformed input was first seen; libxml2 reports it once.
    invalid_at: Option<usize>,
    stopped: bool,
}

impl<'a> Engine<'a> {
    fn new(options: Options) -> Result<Self, Error> {
        if options.flags & !SUPPORTED != 0 {
            return Err(Error::new(ErrorKind::UnsupportedOption, 0));
        }
        let limits = options.limits;
        let huge = options.has(HUGE);
        let value_limit = if huge {
            limits.max_token_bytes.max(1_000_000_000)
        } else {
            limits.max_token_bytes
        };
        let name_limit = if huge {
            value_limit
        } else {
            value_limit.min(50_000)
        };
        let settings = Settings {
            implied: !options.has(NOIMPLIED),
            keep_blanks: !options.has(NOBLANKS),
            report: !options.has(NOERROR),
            max_depth: if huge {
                limits.max_depth.max(2048)
            } else {
                limits.max_depth
            },
            text_limit: value_limit,
        };
        Ok(Self {
            text: Cow::Borrowed(&[]),
            pos: 0,
            dropped: 0,
            decoder: None,
            tokenizer: Tokenizer::new(value_limit, name_limit, limits.max_attributes),
            tree: Tree::new(HtmlBuilder::new(0, limits)?, settings),
            options,
            encoding: None,
            input_bytes: 0,
            retry: 0,
            invalid_at: None,
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

    /// Sniffs the first bytes and starts decoding.
    fn start(&mut self, input: Cow<'a, [u8]>, last: bool) {
        let sniffed = sniff(&input);
        self.tokenizer.has_encoding = sniffed.has_encoding;
        self.encoding = sniffed.name.map(str::to_owned);
        let mut decoder = sniffed.decoder;
        self.text = match (&mut decoder, input) {
            (Decoder::Utf8, Cow::Borrowed(input)) => Cow::Borrowed(&input[sniffed.bom..]),
            (Decoder::Utf8, Cow::Owned(mut input)) => {
                input.drain(..sniffed.bom);
                Cow::Owned(input)
            }
            (decoder, input) => {
                let mut text = Vec::new();
                let malformed = decoder.decode(&input[sniffed.bom..], last, &mut text);
                self.invalid(malformed);
                Cow::Owned(text)
            }
        };
        self.decoder = Some(decoder);
    }

    fn append(&mut self, chunk: &[u8], last: bool) {
        if let Some(decoder) = &mut self.decoder {
            let malformed = decoder.decode(chunk, last, self.text.to_mut());
            self.invalid(malformed);
        }
    }

    /// Records malformed input at a text offset; the earliest one is reported.
    fn invalid(&mut self, at: Option<usize>) {
        if let Some(at) = at.map(|at| self.dropped + at) {
            self.invalid_at = Some(self.invalid_at.map_or(at, |first| first.min(at)));
        }
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
        if !matches!(decoder, Decoder::Utf8) {
            let tail = self.text[at..].to_vec();
            let text = self.text.to_mut();
            text.truncate(at);
            let malformed = decoder.decode(&tail, last, text);
            self.invalid(malformed);
        }
        self.decoder = Some(decoder);
    }

    /// The first non-ASCII byte without a known encoding: htmlFindEncoding,
    /// else windows-1252. An unusable guess leaves the bytes as UTF-8.
    fn fallback(&mut self, at: usize) -> Result<(), Error> {
        let (decoder, name) = match prescan(&self.text[at..]) {
            None => (Decoder::Windows1252, "windows-1252".to_owned()),
            Some(label) => match Decoder::for_label(&label).map_err(|e| self.located(e))? {
                Some(Decoder::Utf8) | None => (Decoder::Utf8, "UTF-8".to_owned()),
                Some(decoder) => (decoder, label),
            },
        };
        self.encoding = Some(name);
        self.switch(decoder, at, true);
        Ok(())
    }

    /// `htmlCheckMeta` and `xmlSetDeclaredEncoding` for a `<meta>` charset.
    fn declare(&mut self, label: String, last: bool) -> Result<(), Error> {
        match Decoder::for_label(&label).map_err(|e| self.located(e))? {
            Some(decoder) if decoder.ascii_compatible() => {
                self.encoding = Some(label);
                self.switch(decoder, self.pos, last);
            }
            _ => self.tree.report(DiagnosticKind::UnsupportedEncoding),
        }
        Ok(())
    }

    fn located(&self, error: Error) -> Error {
        Error::new(error.kind, self.offset())
    }

    fn run(&mut self, eof: bool) -> Result<(), Error> {
        while !self.stopped {
            let rest = &self.text[self.pos..];
            let available = rest.len();
            if !eof && available < self.retry {
                return Ok(());
            }
            self.tree.offset = self.dropped + self.pos;
            match self.tokenizer.next(rest, eof) {
                Ok(None) => return Ok(()),
                Ok(Some((None, used))) => self.pos += used,
                Ok(Some((Some(token), used))) => {
                    let charset = match &token {
                        Token::StartTag {
                            name,
                            attributes,
                            end,
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
                    self.pos += used;
                    self.retry = 0;
                    match reaction {
                        Reaction::Continue => {}
                        Reaction::Enter(mode) => self.tokenizer.enter(mode, self.tree.top_name()),
                        Reaction::Halt => self.stopped = true,
                    }
                    if let Some(label) = charset {
                        self.declare(label, eof)?;
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
                Err(Stop::Switch(at)) => self.fallback(self.pos + at)?,
                Err(Stop::TooLong) => {
                    self.tree.halt();
                    self.stopped = true;
                }
                Err(Stop::Attributes) => {
                    return Err(
                        self.located(Error::new(ErrorKind::LimitExceeded(Limit::Attributes), 0))
                    );
                }
            }
            if self.tokenizer.invalid.get() {
                self.invalid(Some(self.pos));
            }
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Parsed, Error> {
        if let Some(at) = self.invalid_at {
            self.tree.offset = at;
            self.tree.report(DiagnosticKind::InvalidEncoding);
        }
        let (document, diagnostics, stopped) = self
            .tree
            .finish(!self.options.has(NODEFDTD), self.encoding)?;
        Ok(Parsed {
            document,
            diagnostics,
            stopped,
        })
    }
}
