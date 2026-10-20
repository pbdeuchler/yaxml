//! libxml2's structured error record (`xmlError`), built on demand.
//!
//! [`Error`] stays the cheap `{ kind, offset }` value the parser returns; no
//! error site allocates or records more. [`Error::describe`] turns it into a
//! [`Diagnostic`] with libxml2's domain, code, level, line, column and
//! message, and the validators' diagnostics convert to the same shape.
//!
//! Messages need context the error does not carry (tag, attribute and entity
//! names, the open element and its line). `describe` recovers it from the
//! input: a forward scan of the markup skeleton finds the construct holding
//! the offset and the open elements, and that construct is re-read the way
//! libxml2's parser reads it, so its first error, position and wording are
//! libxml2's. The parser accepted everything before a strict error, so the
//! scan is exact there; after recovered errors it follows libxml2's repair in
//! the common case (an end tag closes the innermost element). Codes, domains
//! and the context-free fallback per [`ErrorKind`] are stable; see
//! docs/bindings.md#error-mapping.

use std::borrow::Cow;

use crate::xpath::tree::{Item, Navigate};
use crate::{Error, ErrorKind};

mod reread;

use reread::{Cur, Found, Scan, Work, found, local, unquoted};

/// `xmlErrorDomain`; `as u8` is the C constant.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum Domain {
    #[default]
    Parser = 1,
    Namespace = 3,
    Html = 5,
    Io = 8,
    XInclude = 11,
    XPath = 12,
    SchemasParser = 16,
    SchemasValidity = 17,
    RelaxNgValidity = 19,
    Valid = 23,
    SchematronValidity = 28,
}

/// `xmlErrorLevel`; `as u8` is the C constant.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum Level {
    Warning = 1,
    #[default]
    Error = 2,
    Fatal = 3,
}

/// One `xmlError`: what a libxml2 structured error handler receives.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Diagnostic {
    pub domain: Domain,
    /// The `xmlParserErrors` number (`xmlError.code`).
    pub code: u16,
    pub level: Level,
    /// 1-based line; 0 when unknown.
    pub line: usize,
    /// 1-based column in characters (`xmlError.int2`); 0 when unknown.
    pub column: usize,
    /// libxml2's text without its trailing newline (`xmlError.message` is
    /// this plus `"\n"`).
    pub message: String,
    pub str1: Option<String>,
    pub str2: Option<String>,
    pub int1: i64,
    /// Local name of the element `xmlError.node` names, which
    /// `xmlFormatError` prints as `element NAME: `.
    pub element: Option<String>,
    /// Raised in a nested entity's replacement text: `line` and `column`
    /// count in that text, `xmlError.file` is NULL and the report reads
    /// `Entity: line N`.
    pub entity: bool,
}

/// `xmlParserErrors` numbers the mapping produces (include/libxml/xmlerror.h).
pub mod code {
    pub const INTERNAL_ERROR: u16 = 1;
    pub const NO_MEMORY: u16 = 2;
    pub const DOCUMENT_EMPTY: u16 = 4;
    pub const DOCUMENT_END: u16 = 5;
    pub const INVALID_HEX_CHARREF: u16 = 6;
    pub const INVALID_DEC_CHARREF: u16 = 7;
    pub const INVALID_CHARREF: u16 = 8;
    pub const INVALID_CHAR: u16 = 9;
    pub const ENTITYREF_SEMICOL_MISSING: u16 = 23;
    pub const PEREF_NO_NAME: u16 = 24;
    pub const PEREF_SEMICOL_MISSING: u16 = 25;
    pub const UNDECLARED_ENTITY: u16 = 26;
    pub const UNPARSED_ENTITY: u16 = 28;
    pub const ENTITY_IS_EXTERNAL: u16 = 29;
    pub const UNSUPPORTED_ENCODING: u16 = 32;
    pub const STRING_NOT_STARTED: u16 = 33;
    pub const STRING_NOT_CLOSED: u16 = 34;
    pub const ENTITY_NOT_FINISHED: u16 = 37;
    pub const LT_IN_ATTRIBUTE: u16 = 38;
    pub const ATTRIBUTE_NOT_STARTED: u16 = 39;
    pub const ATTRIBUTE_NOT_FINISHED: u16 = 40;
    pub const ATTRIBUTE_WITHOUT_VALUE: u16 = 41;
    pub const ATTRIBUTE_REDEFINED: u16 = 42;
    pub const LITERAL_NOT_STARTED: u16 = 43;
    pub const LITERAL_NOT_FINISHED: u16 = 44;
    pub const COMMENT_NOT_FINISHED: u16 = 45;
    pub const PI_NOT_STARTED: u16 = 46;
    pub const PI_NOT_FINISHED: u16 = 47;
    pub const NOTATION_NOT_STARTED: u16 = 48;
    pub const NOTATION_NOT_FINISHED: u16 = 49;
    pub const ATTLIST_NOT_STARTED: u16 = 50;
    pub const ATTLIST_NOT_FINISHED: u16 = 51;
    pub const MIXED_NOT_STARTED: u16 = 52;
    pub const ELEMCONTENT_NOT_STARTED: u16 = 54;
    pub const ELEMCONTENT_NOT_FINISHED: u16 = 55;
    pub const XMLDECL_NOT_FINISHED: u16 = 57;
    pub const DOCTYPE_NOT_FINISHED: u16 = 61;
    pub const MISPLACED_CDATA_END: u16 = 62;
    pub const CDATA_NOT_FINISHED: u16 = 63;
    pub const RESERVED_XML_NAME: u16 = 64;
    pub const SPACE_REQUIRED: u16 = 65;
    pub const SEPARATOR_REQUIRED: u16 = 66;
    pub const NMTOKEN_REQUIRED: u16 = 67;
    pub const NAME_REQUIRED: u16 = 68;
    pub const GT_REQUIRED: u16 = 73;
    pub const EQUAL_REQUIRED: u16 = 75;
    pub const TAG_NAME_MISMATCH: u16 = 76;
    pub const TAG_NOT_FINISHED: u16 = 77;
    pub const STANDALONE_VALUE: u16 = 78;
    pub const ENCODING_NAME: u16 = 79;
    pub const HYPHEN_IN_COMMENT: u16 = 80;
    pub const INVALID_ENCODING: u16 = 81;
    pub const VALUE_REQUIRED: u16 = 84;
    pub const NOT_WELL_BALANCED: u16 = 85;
    pub const ENTITY_CHAR_ERROR: u16 = 87;
    pub const ENTITY_PE_INTERNAL: u16 = 88;
    pub const ENTITY_LOOP: u16 = 89;
    pub const VERSION_MISSING: u16 = 96;
    pub const NOTATION_PROCESSING: u16 = 105;
    pub const UNKNOWN_VERSION: u16 = 108;
    pub const USER_STOP: u16 = 111;
    pub const RESOURCE_LIMIT: u16 = 114;
    pub const ARGUMENT: u16 = 115;
    pub const INT_SUBSET_NOT_FINISHED: u16 = 118;
    pub const NS_XML_NAMESPACE: u16 = 200;
    pub const NS_UNDEFINED_NAMESPACE: u16 = 201;
    pub const NS_QNAME: u16 = 202;
    pub const NS_ATTRIBUTE_REDEFINED: u16 = 203;
    pub const NS_COLON: u16 = 205;
    pub const HTML_STRUCTURE_ERROR: u16 = 800;
    pub const IO_LOAD_ERROR: u16 = 1549;
    pub const XINCLUDE_BUILD_FAILED: u16 = 1609;
}

use code::*;

/// The domain and level libxml2 raises a parser code with.
const fn raised(code: u16) -> (Domain, Level) {
    match code {
        NS_XML_NAMESPACE..=NS_COLON => (Domain::Namespace, Level::Error),
        INVALID_ENCODING => (Domain::Io, Level::Fatal),
        1500..=1599 => (Domain::Io, Level::Error),
        1600..=1699 => (Domain::XInclude, Level::Error),
        _ => (Domain::Parser, Level::Fatal),
    }
}

impl ErrorKind {
    /// The libxml2 domain and code for this kind when no input is at hand;
    /// [`Error::describe`] refines it from the input. New kinds only add rows.
    pub const fn libxml2_code(self) -> (Domain, u16) {
        let code = match self {
            Self::InvalidUtf8
            | Self::InvalidUtf16
            | Self::UnmappedByte
            | Self::EncodingMismatch => INVALID_ENCODING,
            Self::InvalidCharacter => INVALID_CHAR,
            Self::UnexpectedEof => TAG_NOT_FINISHED,
            Self::Syntax | Self::InvalidName => NAME_REQUIRED,
            Self::MismatchedEndTag => TAG_NAME_MISMATCH,
            Self::DuplicateAttribute => ATTRIBUTE_REDEFINED,
            Self::Namespace => NS_UNDEFINED_NAMESPACE,
            Self::InvalidReference => INVALID_CHARREF,
            Self::UnknownEntity => UNDECLARED_ENTITY,
            Self::EntityLoop => ENTITY_LOOP,
            Self::UnparsedEntity => UNPARSED_ENTITY,
            Self::ExternalEntityInAttribute => ENTITY_IS_EXTERNAL,
            Self::UnbalancedEntity => NOT_WELL_BALANCED,
            Self::InvalidDtd => DOCTYPE_NOT_FINISHED,
            Self::ExternalResource => IO_LOAD_ERROR,
            Self::XInclude => XINCLUDE_BUILD_FAILED,
            Self::MissingRoot => DOCUMENT_EMPTY,
            Self::MultipleRoots | Self::TextOutsideRoot => DOCUMENT_END,
            Self::InvalidDeclaration => XMLDECL_NOT_FINISHED,
            Self::UnsupportedEncoding => UNSUPPORTED_ENCODING,
            Self::UnsupportedOption => ARGUMENT,
            Self::LimitExceeded(_) | Self::IdentifierExhausted => RESOURCE_LIMIT,
            Self::Allocation => NO_MEMORY,
            Self::Aborted => USER_STOP,
            Self::InvalidState => INTERNAL_ERROR,
        };
        (raised(code).0, code)
    }
}

// ---- positions ----

/// Offset-to-position index over one input, counting as libxml2 does: lines
/// end at `\n`, columns count characters from 1, and a leading byte order
/// mark is not a column. UTF-16 input (by BOM or `<?` pattern) and input
/// declaring a built-in single-byte charset are indexed through their UTF-8
/// transcoding, so offsets stay input bytes.
pub struct LineIndex<'s> {
    input: Cow<'s, [u8]>,
    /// The input transcoded to UTF-8; `None` when the input is UTF-8 itself.
    text: Option<Vec<u8>>,
    /// Text offset of each line's first byte.
    starts: Vec<usize>,
    /// Transcoded input: its decoder and the input offset of each line's first byte.
    decoded: Option<(Decoder, Vec<usize>)>,
    /// HTML input re-decoded after a charset switch, before conversion:
    /// what earlier reports show.
    raw: Option<&'s [u8]>,
}

/// The decoders of transcoded input.
#[derive(Clone, Copy)]
enum Decoder {
    Utf16 { big: bool },
    Byte(&'static [u16; 128]),
}

impl Decoder {
    /// The character `bytes` start with and its input length; U+FFFD for an
    /// unmapped byte or an unpaired surrogate.
    fn next(self, bytes: &[u8]) -> Option<(char, usize)> {
        let replaced = |c: Option<char>| c.unwrap_or(char::REPLACEMENT_CHARACTER);
        match self {
            Self::Byte(table) => {
                let &b = bytes.first()?;
                let c = if b < 0x80 {
                    u32::from(b)
                } else {
                    u32::from(table[usize::from(b) - 0x80])
                };
                Some((
                    replaced(char::from_u32(c).filter(|&c| c != '\0' || b == 0)),
                    1,
                ))
            }
            Self::Utf16 { big } => {
                let units = bytes.as_chunks::<2>().0.iter().take(2).map(|&pair| {
                    if big {
                        u16::from_be_bytes(pair)
                    } else {
                        u16::from_le_bytes(pair)
                    }
                });
                let c = char::decode_utf16(units).next()?.ok();
                Some((replaced(c), 2 * c.map_or(1, char::len_utf16)))
            }
        }
    }
}

impl<'s> LineIndex<'s> {
    pub fn new(input: &'s [u8]) -> Self {
        use crate::encoding::{Charset, charset, declared_label};
        let decoder = match input {
            [0xfe, 0xff, ..] | [0, b'<', 0, b'?', ..] => Decoder::Utf16 { big: true },
            [0xff, 0xfe, ..] | [b'<', 0, b'?', 0, ..] => Decoder::Utf16 { big: false },
            _ => match declared_label(input).and_then(charset) {
                Some(Charset::Byte(table)) => Decoder::Byte(table),
                _ => {
                    let mut index = Self::utf8(input);
                    index.starts[0] = if input.starts_with(b"\xef\xbb\xbf") {
                        3
                    } else {
                        0
                    };
                    return index;
                }
            },
        };
        let bom = if matches!(input, [0xfe, 0xff, ..] | [0xff, 0xfe, ..]) {
            2
        } else {
            0
        };
        let (mut text, mut starts, mut raw, mut at) = (String::new(), vec![0], vec![bom], bom);
        while let Some((c, length)) = decoder.next(&input[at..]) {
            text.push(c);
            at += length;
            if c == '\n' {
                starts.push(text.len());
                raw.push(at);
            }
        }
        Self {
            input: Cow::Borrowed(input),
            text: Some(text.into_bytes()),
            starts,
            decoded: Some((decoder, raw)),
            raw: None,
        }
    }

    /// An index over UTF-8 text, whatever it declares.
    fn utf8(text: &'s [u8]) -> Self {
        let starts = std::iter::once(0)
            .chain(memchr::memchr_iter(b'\n', text).map(|i| i + 1))
            .collect();
        Self {
            input: Cow::Borrowed(text),
            text: None,
            starts,
            decoded: None,
            raw: None,
        }
    }

    /// An index that owns its input, for callers that keep it beside a
    /// long-lived tree (a language binding's `sourceline`).
    pub fn owned(input: Vec<u8>) -> LineIndex<'static> {
        let LineIndex {
            text,
            starts,
            decoded,
            ..
        } = LineIndex::new(&input);
        LineIndex {
            input: Cow::Owned(input),
            text,
            starts,
            decoded,
            raw: None,
        }
    }

    /// The indexed input.
    pub fn input(&self) -> &[u8] {
        &self.input
    }

    /// The input as UTF-8.
    fn text(&self) -> &[u8] {
        self.text.as_deref().unwrap_or(&self.input)
    }

    /// The text offset of an input offset (clamped to the input).
    fn to_text(&self, offset: usize) -> usize {
        let Some((decoder, raw)) = &self.decoded else {
            return offset.min(self.text().len());
        };
        let line = raw.partition_point(|&start| start <= offset).max(1) - 1;
        let (mut at, mut width) = (raw[line], 0);
        while let Some((c, length)) = decoder
            .next(&self.input[at..])
            .filter(|&(_, l)| at + l <= offset)
        {
            at += length;
            width += c.len_utf8();
        }
        (self.starts[line] + width).min(self.text().len())
    }

    /// 1-based line of an input offset.
    pub fn line(&self, offset: usize) -> usize {
        self.text_line(self.to_text(offset))
    }

    /// 1-based column of an input offset, in characters.
    pub fn column(&self, offset: usize) -> usize {
        self.text_column(self.to_text(offset))
    }

    /// The index of an HTML document's input: offsets count bytes of the
    /// text the HTML parser decoded (BOM skipped, `<meta>` and guessed
    /// charsets applied), which is what HTML node origins and
    /// [`html::Diagnostic`](crate::html::Diagnostic) offsets are. `document`
    /// must be the one parsed from `input` ([`html::parse`](crate::html::parse)
    /// or [`html::parse_fragment`](crate::html::parse_fragment)); for another
    /// document this is [`LineIndex::new`].
    pub fn html(input: &'s [u8], document: &crate::OwnedDocument) -> Self {
        match document.html_decoding() {
            Some(decoding) => Self::decoded_html(input, decoding),
            None => Self::new(input),
        }
    }

    pub(crate) fn decoded_html(input: &'s [u8], decoding: &crate::html::Decoding) -> Self {
        let (text, raw) = decoding.text(input);
        let starts = std::iter::once(0)
            .chain(memchr::memchr_iter(b'\n', &text).map(|i| i + 1))
            .collect();
        // Unconverted text is the input itself (after a BOM).
        let (input, text) = match text {
            Cow::Borrowed(body) => (Cow::Borrowed(body), None),
            Cow::Owned(text) => (Cow::Borrowed(input), Some(text)),
        };
        Self {
            input,
            text,
            starts,
            decoded: None,
            raw,
        }
    }

    /// libxml2's context lines for an HTML report at text offset `at`, from
    /// the text before a charset switch when it was raised `early`.
    pub(crate) fn html_context(&self, at: usize, early: bool, bytes: bool) -> String {
        let text = self.raw.filter(|_| early).unwrap_or(self.text());
        let at = at.min(text.len());
        // The window reaches 80 bytes back and forward.
        let (from, to) = (at.saturating_sub(100), (at + 100).min(text.len()));
        context(&text[from..to], at - from, bytes)
    }

    pub(crate) fn text_line(&self, at: usize) -> usize {
        self.starts.partition_point(|&start| start <= at).max(1)
    }

    pub(crate) fn text_column(&self, at: usize) -> usize {
        let start = self.starts[self.text_line(at) - 1].min(at);
        1 + self.text()[start..at]
            .iter()
            .filter(|&&b| b & 0xc0 != 0x80)
            .count()
    }

    /// The text offset of a 1-based line and column.
    fn text_at(&self, line: usize, column: usize) -> usize {
        let start = self.starts[line.clamp(1, self.starts.len()) - 1];
        let rest = &self.text()[start.min(self.text().len())..];
        let width = rest
            .iter()
            .enumerate()
            .filter(|&(_, &b)| b & 0xc0 != 0x80)
            .nth(column.saturating_sub(1))
            .map_or(rest.len(), |(i, _)| i);
        start + width
    }

    /// libxml2's line for the element whose start tag begins at text offset
    /// `start`: where the tag ends, as `xmlSAX2StartElementNs` records it.
    fn tag_line(&self, start: usize) -> usize {
        self.text_line(unquoted(self.text(), start + 1, b">").unwrap_or(self.text().len()))
    }

    /// `xmlGetLineNo` for a parsed node: elements count where their start
    /// tag ends, text, comments and PIs where they end, other nodes take
    /// their previous sibling's line or else their parent element's.
    pub(crate) fn node_line<T: Navigate>(&self, tree: &T, id: T::Id) -> Option<usize> {
        let own = |id| match tree.item(id) {
            Item::Element(_) => Some(true),
            Item::Text(_) if !tree.is_cdata(id) => Some(false),
            Item::Comment(_) | Item::Pi(..) => Some(false),
            _ => None,
        };
        let Some(element) = own(id) else {
            let parent = tree.parent(id)?;
            let siblings =
                std::iter::successors(tree.first_child(parent), |&s| tree.next_sibling(s));
            // Navigation hides a DOCTYPE from its siblings: it has none.
            let previous = siblings
                .take_while(|&s| s != id)
                .last()
                .filter(|&s| tree.next_sibling(s) == Some(id) && own(s).is_some());
            let from = previous.or((own(parent) == Some(true)).then_some(parent))?;
            return self.node_line(tree, from);
        };
        let range = tree.range(id)?;
        if tree.marks_lines() {
            // The text offset whose line libxml2 recorded.
            return Some(self.text_line(range.start));
        }
        let range = Some(range).filter(|range| range.start < self.input.len())?;
        Some(if element {
            self.tag_line(self.to_text(range.start))
        } else {
            self.line(range.end)
        })
    }
}

impl LineIndex<'_> {
    fn diagnostic(&self, found: Found, work: &Work) -> Diagnostic {
        let (domain, level) = raised(found.code);
        let line = self.text_line(found.at);
        // Counting a column's characters is charged too; without work it is unknown.
        let start = self.starts[line - 1].min(found.at);
        let column = || {
            work.charge(found.at - start)
                .map_or(0, |()| self.text_column(found.at))
        };
        let (line, column) = found.place.unwrap_or_else(|| (line, column()));
        Diagnostic {
            domain,
            code: found.code,
            level,
            line,
            column,
            message: found.message,
            str1: found.str1,
            str2: found.str2,
            int1: found.int1,
            entity: found.place.is_some(),
            ..Diagnostic::default()
        }
    }

    /// The errors of a recovered parse (at most libxml2's 100) as libxml2
    /// reports them: each described, without the end-of-input errors libxml2
    /// raises only for a still well-formed document. One forward scan
    /// serves errors in increasing offset order, as the parse records them.
    ///
    /// All of them share one work budget, linear in the input (eight bytes
    /// of work per input byte, libxml2's amplification factor of five for
    /// entity expansion and three passes, plus 64 KiB): what is scanned to
    /// reach an error, re-read and expanded for it. Once it is spent the
    /// remaining errors get the context-free [`ErrorKind::libxml2_code`],
    /// a generic message and no column.
    pub fn describe_all(&self, errors: &[Error]) -> Vec<Diagnostic> {
        self.describe_counted(errors).0
    }

    /// [`describe_all`](Self::describe_all) and the work it used against its
    /// ceiling, for tests and fuzzing.
    #[doc(hidden)]
    pub fn describe_counted(&self, errors: &[Error]) -> (Vec<Diagnostic>, usize, usize) {
        let work = Work::new(self.text().len());
        let mut scan = Scan::default();
        let mut described: Vec<Diagnostic> = Vec::new();
        for error in errors {
            let at = self.to_text(error.offset);
            let found = found(error.kind, self, at, &mut scan, &work);
            let diagnostic = self.diagnostic(found, &work);
            let late = matches!(diagnostic.code, TAG_NOT_FINISHED | DOCUMENT_EMPTY);
            if !(late
                && described
                    .iter()
                    .any(|earlier| earlier.level == Level::Fatal))
            {
                described.push(diagnostic);
            }
        }
        let (used, ceiling) = work.spent();
        (described, used, ceiling)
    }

    /// Where libxml2's input cursor is when it raises `code` at `at`: for a
    /// misplaced `]]>` the ASCII fast path of `xmlParseCharDataInternal` has
    /// not moved past the text run's start or its last `\r\n`. Character
    /// data holds no `<`, so the run starts after the `>` that ends the
    /// markup before it.
    fn cursor(&self, code: u16, at: usize) -> usize {
        let t = self.text();
        if code != MISPLACED_CDATA_END {
            return at;
        }
        let markup = memchr::memrchr(b'<', &t[..at]).unwrap_or(0);
        let start = memchr::memchr(b'>', &t[markup..at]).map_or(markup, |gt| markup + gt + 1);
        let run = &t[start..at];
        if run
            .iter()
            .any(|&b| b >= 0x80 || (b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r')))
        {
            return at;
        }
        memchr::memmem::rfind(run, b"\r\n").map_or(start, |cr| start + cr + 1)
    }

    /// libxml2's context lines for a text offset (`xmlParserInputGetWindow`):
    /// up to 80 bytes of its line and a caret under it.
    fn window(&self, cur: usize) -> (Cow<'_, str>, String) {
        const SIZE: usize = 80;
        let t = self.text();
        let eol = |p: usize| matches!(t.get(p), Some(b'\n' | b'\r'));
        let mut p = cur.min(t.len());
        while p > 0 && eol(p) {
            p -= 1;
        }
        let mut n = 0;
        while n < SIZE && p > 0 && !eol(p) {
            p -= 1;
            n += 1;
        }
        if n > 0 && eol(p) {
            p += 1;
        } else {
            while p < cur && t[p] & 0xc0 == 0x80 {
                p += 1;
            }
        }
        let (start, col) = (p, cur.saturating_sub(p));
        let (mut n, work) = (0, Work::new(0));
        while let Some((_, len)) = Cur::new(self, &work, start + n)
            .ch()
            .filter(|&(c, _)| !matches!(c, '\0' | '\n' | '\r'))
        {
            if n + len > SIZE {
                break;
            }
            n += len;
        }
        let col = if col >= n {
            if n < SIZE { n } else { SIZE - 1 }
        } else {
            col
        };
        let content = &t[start..start + n];
        let caret = content[..col]
            .iter()
            .map(|&b| if b == b'\t' { '\t' } else { ' ' })
            .chain(['^'])
            .collect();
        (String::from_utf8_lossy(content), caret)
    }
}

impl Error {
    /// libxml2's `xmlError` for this error in the input `lines` indexes.
    ///
    /// ```
    /// use yaxml::{Document, LineIndex};
    /// let input = "<a>\n  <b></c>\n</a>";
    /// let error = Document::parse(input).unwrap_err();
    /// let diagnostic = error.describe(&LineIndex::new(input.as_bytes()));
    /// assert_eq!((diagnostic.code, diagnostic.line), (76, 2));
    /// assert_eq!(diagnostic.message, "Opening and ending tag mismatch: b line 2 and c");
    /// ```
    pub fn describe(&self, lines: &LineIndex<'_>) -> Diagnostic {
        let at = lines.to_text(self.offset);
        let work = Work::new(lines.text().len());
        lines.diagnostic(
            found(self.kind, lines, at, &mut Scan::default(), &work),
            &work,
        )
    }
}

impl Diagnostic {
    /// `xmlFormatError`'s report: `FILE:LINE: [element NAME: ]DOMAIN LEVEL :
    /// MESSAGE`, then for parser, namespace and I/O errors the input's line
    /// around the position with a caret (and the bytes, for an encoding error).
    pub fn render(&self, file: &str, lines: &LineIndex<'_>) -> String {
        let mut out = self.header(file);
        if self.line == 0
            || self.entity
            || !matches!(self.domain, Domain::Parser | Domain::Namespace | Domain::Io)
        {
            return out;
        }
        let at = lines.text_at(self.line, self.column);
        let cursor = lines.cursor(self.code, at);
        // Only input read without a converter shows its bytes.
        out += &lines.context(
            cursor,
            self.code == INVALID_ENCODING && lines.decoded.is_none(),
        );
        out
    }

    /// The report's first line: `FILE:LINE: [element NAME: ]DOMAIN LEVEL : MESSAGE`.
    pub(crate) fn header(&self, file: &str) -> String {
        let mut out = match self.line {
            line if self.entity => format!("Entity: line {line}: "),
            0 => format!("{file}: "),
            line => format!("{file}:{line}: "),
        };
        if let Some(element) = &self.element {
            out += &format!("element {element}: ");
        }
        out += match self.domain {
            Domain::Parser => "parser ",
            Domain::Namespace => "namespace ",
            Domain::Html => "HTML parser ",
            Domain::Io => "I/O ",
            Domain::XInclude => "XInclude ",
            Domain::XPath => "XPath ",
            Domain::SchemasParser => "Schemas parser ",
            Domain::Valid => "validity ",
            Domain::SchemasValidity => "Schemas validity ",
            Domain::RelaxNgValidity => "Relax-NG validity ",
            Domain::SchematronValidity => "schematron ",
        };
        out += if self.level == Level::Warning {
            "warning : "
        } else {
            "error : "
        };
        out += &self.message;
        out.push('\n');
        out
    }
}

/// libxml2's context lines at `cur` in UTF-8 `text`, as `xmlFormatError`
/// prints them, with the next bytes first when `bytes`.
pub(crate) fn context(text: &[u8], cur: usize, bytes: bool) -> String {
    LineIndex::utf8(text).context(cur, bytes)
}

impl LineIndex<'_> {
    /// The input's bytes at `at` (for an invalid encoding), its line and a caret.
    fn context(&self, at: usize, bytes: bool) -> String {
        let mut out = String::new();
        if bytes && at < self.text().len() {
            out += "Bytes:";
            out.extend(
                self.text()[at..]
                    .iter()
                    .take(4)
                    .map(|b| format!(" 0x{b:02X}")),
            );
            out.push('\n');
        }
        let (content, caret) = self.window(at);
        out + &format!("{content}\n{caret}\n")
    }
}

// ---- validators and tree nodes ----

impl LineIndex<'_> {
    /// A validator's diagnostic about the element starting at `offset`, as
    /// libxml2 raises it: the element's line as `xmlGetLineNo` counts it, its
    /// name when `named`, no column.
    fn validity(
        &self,
        domain: Domain,
        code: u16,
        warning: bool,
        offset: Option<usize>,
        message: &str,
        named: bool,
    ) -> Diagnostic {
        let at = offset
            .map(|offset| self.to_text(offset))
            .filter(|&at| at < self.text().len());
        let element = at.filter(|_| named).and_then(|at| {
            let work = Work::new(0);
            let name = Cur::new(self, &work, at + 1).name(true)?;
            Some(local(name).to_owned())
        });
        Diagnostic {
            domain,
            code,
            level: if warning {
                Level::Warning
            } else {
                Level::Error
            },
            line: at.map_or(0, |at| self.tag_line(at)),
            message: message.trim_end_matches('\n').to_owned(),
            element,
            ..Diagnostic::default()
        }
    }
}

impl<Id> crate::valid::Diagnostic<Id> {
    /// This diagnostic as libxml2's `xmlError` (`XML_FROM_VALID`), with the
    /// line of its element in the validated input `lines` indexes.
    pub fn describe(&self, lines: &LineIndex<'_>) -> Diagnostic {
        lines.validity(
            Domain::Valid,
            self.code as u16,
            self.code.is_warning(),
            self.offset,
            &self.message,
            true,
        )
    }
}

impl<Id> crate::relaxng::Diagnostic<Id> {
    /// This diagnostic as libxml2's `xmlError` (`XML_FROM_RELAXNGV`).
    pub fn describe(&self, lines: &LineIndex<'_>) -> Diagnostic {
        lines.validity(
            Domain::RelaxNgValidity,
            self.code as u16,
            false,
            self.offset,
            &self.message,
            true,
        )
    }
}

impl<Id> crate::xsd::Diagnostic<Id> {
    /// This diagnostic as libxml2's `xmlError` (`XML_FROM_SCHEMASV`, which
    /// names no element).
    pub fn describe(&self, lines: &LineIndex<'_>) -> Diagnostic {
        let (code, warning) = (self.code as u16, self.code.is_warning());
        lines.validity(
            Domain::SchemasValidity,
            code,
            warning,
            self.offset,
            &self.message,
            false,
        )
    }
}

impl<Id> crate::schematron::Diagnostic<Id> {
    /// This diagnostic as libxml2's `xmlError`: `XML_FROM_SCHEMATRONV` for
    /// asserts and reports, otherwise the domain libxml2 raises it in; the
    /// line is the one validation recorded.
    pub fn describe(&self) -> Diagnostic {
        use crate::schematron::Code;
        let domain = match self.code {
            Code::Assert | Code::Report => Domain::SchematronValidity,
            Code::Internal => Domain::SchemasValidity,
            Code::XPath(_) => Domain::XPath,
            _ => Domain::SchemasParser,
        };
        Diagnostic {
            domain,
            code: self.code.number(),
            line: self.line,
            message: self.message.trim_end_matches('\n').to_owned(),
            element: self.element.clone(),
            ..Diagnostic::default()
        }
    }
}

impl crate::Node<'_, '_> {
    /// `xmlGetLineNo` in the input `lines` indexes: elements count where
    /// their start tag ends, text, comments and PIs where they end, others
    /// take their previous sibling's or parent element's line.
    pub fn line(self, lines: &LineIndex<'_>) -> Option<usize> {
        lines.node_line(self.document(), self.id())
    }
}

// Kept here, beside `Node::line`, rather than in the owned module.
impl<A: crate::allocator::Allocator> crate::OwnedNode<'_, A> {
    /// `xmlGetLineNo` in the input `lines` indexes, as [`crate::Node::line`];
    /// `None` for constructed nodes. HTML nodes keep the line the parser
    /// recorded, so `lines` is not consulted for them.
    pub fn line(self, lines: &LineIndex<'_>) -> Option<usize> {
        lines.node_line(self.document(), self.id())
    }
}
