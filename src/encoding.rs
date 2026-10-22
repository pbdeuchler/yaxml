//! Input decoding and libxml2's encoding API (encoding.h): [`CharEncoding`]
//! names and detects encodings, a [`Handler`] converts between a built-in
//! charset and UTF-8 ([encoding](https://github.com/pbdeuchler/yaxml/blob/main/docs/encoding.md)).
//! The parsers decode by themselves; [`Converters`] plug other charsets into a
//! parse, and [`Transcoded`] hands decoded text to the borrowing parsers.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;

use crate::{Error, ErrorKind, Limit, ParseOptions};

/// Ceilings on the UTF-8 that input decoding produces; a byte order mark
/// counts toward neither.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct DecodeLimits {
    /// Total UTF-8 bytes produced, excluding the transport BOM (default 64 MiB).
    pub max_decoded_bytes: usize,
    /// UTF-8 bytes in one lexical token (default 1 MiB).
    pub max_decoded_token_bytes: usize,
}

setters!(const DecodeLimits { max_decoded_bytes: usize, max_decoded_token_bytes: usize });

impl DecodeLimits {
    /// 64 MiB in total and 1 MiB per token.
    pub const DEFAULT: Self = Self {
        max_decoded_bytes: 64 * 1024 * 1024,
        max_decoded_token_bytes: 1024 * 1024,
    };
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Input as UTF-8 for the borrowing parsers ([`Reader::transcoded`],
/// [`Document::parse_transcoded`]): UTF-8 input as it is, without a copy;
/// UTF-16 and declared single-byte input decoded once, as the streaming
/// parsers decode it ([encoding](https://github.com/pbdeuchler/yaxml/blob/main/docs/encoding.md)).
/// Keep it alive beside the tree, which borrows it.
///
/// Errors and origins of a transcoded parse count bytes of this text, as
/// its [`LineIndex::transcoded`] does; [`Transcoded::source_offset`] maps
/// one to the input.
///
/// ```
/// use yaxml::{Document, LineIndex, ParseOptions, Transcoded};
///
/// let latin1 = b"<?xml version='1.0' encoding='ISO-8859-1'?>\n<r>caf\xe9</r>";
/// let input = Transcoded::new(latin1, &ParseOptions::default())?;
/// let document = Document::parse_transcoded(&input, ParseOptions::default())?;
/// let text = document.root_element().first_child().unwrap();
/// assert_eq!(text.content().as_deref(), Some("caf\u{e9}"));
/// assert_eq!(text.range(), Some(47..52));
/// assert_eq!(input.source_offset(52), 51);
/// assert_eq!(text.line(&LineIndex::transcoded(&input)), Some(2));
/// # Ok::<(), yaxml::Error>(())
/// ```
///
/// [`Reader::transcoded`]: crate::Reader::transcoded
/// [`Document::parse_transcoded`]: crate::Document::parse_transcoded
/// [`LineIndex::transcoded`]: crate::LineIndex::transcoded
#[derive(Clone, Debug)]
pub struct Transcoded<'a> {
    text: Cow<'a, [u8]>,
    /// For decoded text: the source encoding and, under `RECOVER`, the
    /// decoding error that ends the text.
    pub(crate) decoded: Option<(SourceEncoding, Option<Error>)>,
}

impl<'a> Transcoded<'a> {
    /// Detects `input`'s encoding from its BOM, signature or declaration and
    /// decodes it unless it is UTF-8, within `options`' `max_input_bytes`
    /// and [`DecodeLimits`]. Under `IGNORE_ENC` a declared charset is ignored.
    ///
    /// # Errors
    ///
    /// `LimitExceeded` (`InputBytes`, `DecodedBytes`), `Allocation`, and the
    /// decoding errors at their input offset: `InvalidUtf16`, `UnmappedByte`
    /// and `UnsupportedEncoding`. Under `RECOVER` a decoding error ends the
    /// text instead, and the parse recovers from it there.
    pub fn new(input: &'a [u8], options: &ParseOptions) -> Result<Self, Error> {
        let (limits, decode) = options.effective_limits();
        if input.len() > limits.max_input_bytes {
            return Err(Error::new(ErrorKind::LimitExceeded(Limit::InputBytes), 0));
        }
        let converters = options.input_converters();
        if !needs_decoding(input, converters.is_some()) {
            return Ok(Self {
                text: Cow::Borrowed(input),
                decoded: None,
            });
        }
        let recovers = options.has(crate::options::RECOVER);
        let switch = declared(input)
            .filter(|_| !options.has(crate::options::IGNORE_ENC))
            .filter(|_| !recovers || recovered_label(&ascii_head(input)).is_some());
        let mut text = Vec::new();
        let (encoding, end) = transcode(
            input,
            switch,
            converters,
            decode.max_decoded_bytes,
            &mut text,
        );
        let cut = match end {
            Ok(()) => None,
            Err(error)
                if recovers
                    && !matches!(
                        error.kind,
                        ErrorKind::LimitExceeded(_) | ErrorKind::Allocation
                    ) =>
            {
                Some(Error::new(error.kind, text.len()))
            }
            Err(error) => return Err(error),
        };
        Ok(Self {
            text: Cow::Owned(text),
            decoded: Some((encoding, cut)),
        })
    }

    /// The input offset of text offset `offset`, which starts a character:
    /// linear in `offset` for decoded text.
    pub fn source_offset(&self, offset: usize) -> usize {
        let Some((encoding, _)) = self.decoded else {
            return offset;
        };
        let bom = match encoding {
            SourceEncoding::Utf16 { bom: true, .. } => 2,
            _ => 0,
        };
        bom + encoding.source_length(&self.text[..offset.min(self.text.len())])
    }

    /// The UTF-8 text the borrowing parsers read.
    pub fn as_bytes(&self) -> &[u8] {
        &self.text
    }
}

/// Input converters for charsets the built-in tables lack, as libxml2 takes
/// them from iconv, ICU or `xmlCtxtSetCharEncConvImpl`
/// ([`ParseOptions::converters`]). A BOM-less document whose `<?xml`
/// declaration names a label no built-in charset has is read through the
/// converter [`Converters::open`] returns for it, from its first byte; the
/// built-in charsets keep precedence, and `IGNORE_ENC` disables converters.
///
/// Offsets, errors and [`Transcoded::source_offset`] of converted input
/// count bytes of the converted UTF-8 text.
///
/// ```
/// use std::sync::Arc;
/// use yaxml::{Converter, Converters, Document, ParseOptions, Transcoded};
///
/// /// A toy charset: bytes 0x80..=0xFF are U+0400 + byte - 0x80.
/// struct Shifted;
/// impl Converter for Shifted {
///     fn decode(&mut self, input: &[u8], output: &mut String, _last: bool) -> bool {
///         for &byte in input {
///             match byte {
///                 0x80.. => output.push(char::from_u32(0x380 + u32::from(byte)).unwrap()),
///                 0 => return false,
///                 _ => output.push(char::from(byte)),
///             }
///         }
///         true
///     }
/// }
/// struct Labels;
/// impl Converters for Labels {
///     fn open(&self, label: &str) -> Option<Box<dyn Converter>> {
///         label.eq_ignore_ascii_case("X-SHIFTED").then(|| Box::new(Shifted) as _)
///     }
/// }
///
/// let input = b"<?xml version='1.0' encoding='x-shifted'?><r>\x90</r>";
/// let options = ParseOptions::DEFAULT.converters(Some(Arc::new(Labels)));
/// let text = Transcoded::new(input, &options)?;
/// let document = Document::parse_transcoded(&text, options)?;
/// assert_eq!(document.root_element().text_chunks().collect::<String>(), "\u{410}");
/// # Ok::<(), yaxml::Error>(())
/// ```
pub trait Converters: Send + Sync {
    /// A converter for `label` as the declaration spells it, or `None`.
    fn open(&self, label: &str) -> Option<Box<dyn Converter>>;
}

/// One document's input conversion to UTF-8 (`xmlCharEncConvFunc`).
pub trait Converter: Send {
    /// Appends `input` converted to `output`, keeping an incomplete trailing
    /// sequence for the next call; `last` marks the end of the input, where
    /// such a sequence is an error. False, after appending the text before
    /// it, at a sequence the charset does not map (`UnmappedByte`).
    fn decode(&mut self, input: &[u8], output: &mut String, last: bool) -> bool;
}

/// A decoder's caller-supplied converters: while `sniffing`, `head` collects
/// a leading declaration through its `?>`, whose label selects a converter
/// or plain UTF-8; `head` (no converter) or `out` (converted) is then
/// forwarded from `cursor`; an unmapped sequence fails (`failed`) once the
/// text before it is forwarded.
pub(crate) struct Custom {
    converters: Arc<dyn Converters>,
    converter: Option<Box<dyn Converter>>,
    sniffing: bool,
    head: Vec<u8>,
    out: String,
    cursor: usize,
    last: bool,
    failed: Option<Error>,
}

/// A converted declaration longer than this is read as UTF-8.
const HEAD_LIMIT: usize = 4096;
/// Input bytes converted per call, bounding `Custom::out`.
const CONVERT_WINDOW: usize = 16 * 1024;

/// Replaces `custom.out` with `input` converted, whose text starts at
/// converted offset `at`; an unmapped sequence fails there (`failed`).
fn convert(custom: &mut Custom, input: &[u8], last: bool, at: usize) -> Result<(), Error> {
    let Custom {
        converter,
        out,
        cursor,
        failed,
        ..
    } = custom;
    let converter = converter.as_deref_mut().expect("converter");
    out.clear();
    *cursor = 0;
    out.try_reserve(input.len().saturating_mul(3))
        .map_err(|_| Error::new(ErrorKind::Allocation, at))?;
    if !converter.decode(input, out, last) {
        *failed = Some(Error::new(ErrorKind::UnmappedByte, at + out.len()));
    }
    custom.last = last;
    Ok(())
}

mod charsets;
mod handler;

pub use handler::{CharEncoding, EncError, Handler};

/// Tables the HTML parser treats specially: libxml2 decodes ASCII and Latin-1
/// labels as windows-1252 in HTML, and remaps C1 numeric references with it.
#[cfg(feature = "html")]
pub(crate) const WINDOWS_1252: &[u16; 128] = &charsets::WINDOWS_1252;
#[cfg(feature = "html")]
pub(crate) const ISO_8859_1: &[u16; 128] = &charsets::ISO_8859_1;
#[cfg(feature = "html")]
pub(crate) const US_ASCII: &[u16; 128] = &charsets::US_ASCII;

#[cfg(feature = "encodings")]
mod iconv;

/// The EBCDIC code page libxml2 reads a declaration in (`xmlDetectEBCDIC`).
#[cfg(feature = "encodings")]
pub(crate) const IBM037: &[u16; 256] = &iconv::CP037;

/// A built-in libxml2 declaration label: UTF-16 carries an optional byte
/// order, single-byte charsets map bytes 0x80..=0xFF and EBCDIC code pages
/// every byte (zero is unmapped). UCS-4 is an output charset only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Charset {
    Utf8,
    Utf16(Option<bool>),
    Byte(&'static [u16; 128]),
    #[cfg_attr(not(feature = "encodings"), allow(dead_code))]
    Ebcdic(&'static [u16; 256]),
    /// UCS-4 output; `None` = UTF-32, big-endian behind a BOM.
    #[cfg_attr(not(feature = "encodings"), allow(dead_code))]
    Ucs4(Option<bool>),
}

/// Resolves a label ASCII case-insensitively, as libxml2 does without
/// iconv/ICU, or with the macOS iconv for EBCDIC and UCS-4 (feature `encodings`).
pub(crate) fn charset(label: &str) -> Option<Charset> {
    #[cfg(feature = "encodings")]
    let optional = &iconv::LABELS;
    #[cfg(not(feature = "encodings"))]
    let optional: &[(&str, Charset)] = &[];
    CharEncoding::labelled(label).builtin().or_else(|| {
        optional
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(label))
            .map(|(_, charset)| *charset)
    })
}

/// An output converter `xmlOpenCharEncodingHandler` provides without
/// iconv/ICU. Generic UTF-16 is little-endian behind a BOM, as libxml2 writes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Output {
    Utf8,
    Utf16 { little_endian: bool, bom: bool },
    Byte(&'static [u16; 128]),
    Ebcdic(&'static [u16; 256]),
    Ucs4 { little_endian: bool, bom: bool },
}

impl From<Charset> for Output {
    fn from(charset: Charset) -> Self {
        match charset {
            Charset::Utf8 => Self::Utf8,
            Charset::Utf16(None) => Self::Utf16 {
                little_endian: true,
                bom: true,
            },
            Charset::Utf16(Some(little_endian)) => Self::Utf16 {
                little_endian,
                bom: false,
            },
            Charset::Byte(table) => Self::Byte(table),
            Charset::Ebcdic(table) => Self::Ebcdic(table),
            Charset::Ucs4(order) => Self::Ucs4 {
                little_endian: order == Some(true),
                bom: order.is_none(),
            },
        }
    }
}

impl Output {
    pub(crate) fn for_label(label: &str) -> Option<Self> {
        charset(label).map(Self::from)
    }

    /// The byte-order mark written before any output.
    pub(crate) fn bom(self) -> &'static [u8] {
        match self {
            Self::Utf16 { bom: true, .. } => &[0xFF, 0xFE],
            Self::Ucs4 { bom: true, .. } => &[0, 0, 0xFE, 0xFF],
            _ => &[],
        }
    }

    /// Appends `c`; false, appending nothing, when the charset lacks it.
    pub(crate) fn push(self, out: &mut Vec<u8>, c: char) -> bool {
        let code = u32::from(c);
        match self {
            Self::Utf8 => out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
            Self::Utf16 { little_endian, .. } => {
                for unit in c.encode_utf16(&mut [0; 2]) {
                    out.extend_from_slice(&if little_endian {
                        unit.to_le_bytes()
                    } else {
                        unit.to_be_bytes()
                    });
                }
            }
            Self::Byte(_) if code < 0x80 => out.push(code as u8),
            Self::Byte(table) => {
                // Most tables map their upper half to itself; try that first.
                let same = (code < 0x100 && u32::from(table[code as usize - 0x80]) == code)
                    .then_some(code as usize - 0x80);
                match same.or_else(|| table.iter().position(|&m| m != 0 && u32::from(m) == code)) {
                    Some(index) => out.push(0x80 + index as u8),
                    None => return false,
                }
            }
            Self::Ucs4 { little_endian, .. } => out.extend_from_slice(&if little_endian {
                code.to_le_bytes()
            } else {
                code.to_be_bytes()
            }),
            Self::Ebcdic(table) => match table.iter().position(|&m| m != 0 && u32::from(m) == code)
            {
                Some(index) => out.push(index as u8),
                None => return false,
            },
        }
        true
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourceEncoding {
    Utf8,
    Utf16 {
        little_endian: bool,
        bom: bool,
    },
    /// A single-byte charset selected by a BOM-less declaration.
    Byte(&'static [u16; 128]),
    /// UCS-4 detected from a `<` signature (feature `encodings`).
    #[cfg_attr(not(feature = "encodings"), allow(dead_code))]
    Ucs4 {
        little_endian: bool,
    },
    /// An EBCDIC code page: IBM037 until a declaration names another.
    #[cfg_attr(not(feature = "encodings"), allow(dead_code))]
    Ebcdic(&'static [u16; 256]),
    /// Converted by a caller's [`Converter`]: the tokenizer reads its UTF-8.
    Custom,
}

impl SourceEncoding {
    pub(crate) fn source_length(self, bytes: &[u8]) -> usize {
        match self {
            Self::Utf8 | Self::Custom => bytes.len(),
            Self::Utf16 { .. } => bytes
                .iter()
                .map(|b| match b {
                    0..=0x7f | 0xc0..=0xef => 2,
                    0xf0..=0xf7 => 4,
                    _ => 0,
                })
                .sum(),
            Self::Byte(_) | Self::Ebcdic(_) => {
                bytes.iter().filter(|b| (**b & 0xc0) != 0x80).count()
            }
            Self::Ucs4 { .. } => 4 * bytes.iter().filter(|b| (**b & 0xc0) != 0x80).count(),
        }
    }

    pub(crate) fn requires_declaration(self) -> bool {
        matches!(self, Self::Utf16 { bom: false, .. })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TokenSpan {
    pub(crate) decoded: Range<usize>,
    pub(crate) source: Range<usize>,
    pub(crate) encoding: SourceEncoding,
}

impl TokenSpan {
    pub(crate) fn map_error(&self, raw: &str, error: Error) -> Error {
        let offset = match self.encoding {
            SourceEncoding::Utf8 => error.offset,
            _ => {
                let relative = error
                    .offset
                    .saturating_sub(self.decoded.start)
                    .min(raw.len());
                // Error locations originate in the shared UTF-8 grammar. A
                // defensive floor also handles a location inside a scalar.
                let prefix = &raw[..raw.floor_char_boundary(relative)];
                self.source.start + self.encoding.source_length(prefix.as_bytes())
            }
        };
        Error::new(error.kind, offset)
    }
}

pub(crate) enum Decoded<'a> {
    Start {
        encoding: SourceEncoding,
        bom: usize,
    },
    Bytes(&'a [u8]),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DecodeProgress {
    pub(crate) consumed: usize,
    pub(crate) suspended: bool,
}

/// The sole transport boundary. It retains no decoded input and no offset map.
pub(crate) struct Decoder {
    encoding: Option<SourceEncoding>,
    preamble: [u8; 4],
    preamble_len: usize,
    preamble_cursor: usize,
    position: usize,
    decoded: usize,
    limit: usize,
    /// A code unit's bytes so far, and the offset of its first byte.
    unit: ([u8; 4], usize, usize),
    high: Option<(u32, usize)>,
    pending: [u8; 4],
    pending_cursor: usize,
    pending_len: usize,
    custom: Option<Box<Custom>>,
}

impl Decoder {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            encoding: None,
            preamble: [0; 4],
            preamble_len: 0,
            preamble_cursor: 0,
            position: 0,
            decoded: 0,
            limit,
            unit: ([0; 4], 0, 0),
            high: None,
            pending: [0; 4],
            pending_cursor: 0,
            pending_len: 0,
            custom: None,
        }
    }

    /// Reads a declared label no built-in charset has through `converters`;
    /// only before input.
    pub(crate) fn set_converters(&mut self, converters: Option<Arc<dyn Converters>>) {
        self.custom = converters.map(|converters| {
            Box::new(Custom {
                converters,
                converter: None,
                sniffing: true,
                head: Vec::new(),
                out: String::new(),
                cursor: 0,
                last: false,
                failed: None,
            })
        });
    }

    pub(crate) fn position(&self) -> usize {
        self.position
    }

    /// The input taken while a converter runs: token offsets then count
    /// converted text, so they say nothing about it.
    pub(crate) fn converting(&self) -> Option<usize> {
        (self.encoding == Some(SourceEncoding::Custom)).then_some(self.position)
    }

    /// Decoded bytes still allowed once plain UTF-8 forwarding is active.
    pub(crate) fn utf8_allowance(&self) -> Option<usize> {
        (self.encoding == Some(SourceEncoding::Utf8) && self.preamble_cursor == self.preamble_len)
            .then(|| self.limit - self.decoded)
    }

    /// Counts input taken directly from the chunk, as `forward_utf8` would.
    pub(crate) fn skip_utf8(&mut self, length: usize) {
        self.position += length;
        self.decoded += length;
    }

    pub(crate) fn advance(
        &mut self,
        input: &[u8],
        mut emit: impl FnMut(Decoded<'_>) -> Result<DecodeProgress, Error>,
    ) -> Result<DecodeProgress, Error> {
        // UTF-8 has no scalar carry. Once its preamble is drained, each token
        // suspension can return directly without reentering detection/carry.
        if self.encoding == Some(SourceEncoding::Utf8) && self.preamble_cursor == self.preamble_len
        {
            return self.forward_utf8(input, false, &mut emit);
        }
        if self.encoding == Some(SourceEncoding::Custom) {
            let progress = self.advance_custom(input, &mut emit)?;
            if progress.suspended || self.encoding == Some(SourceEncoding::Custom) {
                return Ok(progress);
            }
            // Plain UTF-8 or a built-in charset follows a replayed head.
            let rest = self.advance(&input[progress.consumed..], emit)?;
            return Ok(DecodeProgress {
                consumed: progress.consumed + rest.consumed,
                ..rest
            });
        }
        let mut cursor = 0;
        loop {
            let Some(encoding) = self.encoding else {
                if cursor == input.len() {
                    return Ok(DecodeProgress {
                        consumed: cursor,
                        suspended: false,
                    });
                }
                self.preamble[self.preamble_len] = input[cursor];
                self.preamble_len += 1;
                self.position += 1;
                cursor += 1;
                if let Some(selection) = select(&self.preamble[..self.preamble_len], false) {
                    let selection = selection?;
                    let read = &self.preamble[..self.preamble_len];
                    if self.custom.is_some()
                        && selection == (SourceEncoding::Utf8, 0)
                        && b"<?xml".starts_with(read)
                    {
                        // libxml2 reads the declaration's label before it
                        // switches to a converter.
                        let custom = self.custom.as_mut().expect("converters");
                        custom.head.extend_from_slice(read);
                        (self.preamble_len, self.encoding) = (0, Some(SourceEncoding::Custom));
                        let progress = self.advance(&input[cursor..], emit)?;
                        return Ok(DecodeProgress {
                            consumed: cursor + progress.consumed,
                            ..progress
                        });
                    }
                    self.custom = None;
                    self.start(selection, &mut emit)?;
                }
                continue;
            };
            if self.pending_cursor < self.pending_len {
                let remaining = &self.pending[self.pending_cursor..self.pending_len];
                let progress = emit(Decoded::Bytes(remaining))?;
                check_progress(progress, remaining.len(), self.position)?;
                self.pending_cursor += progress.consumed;
                if progress.suspended {
                    return Ok(DecodeProgress {
                        consumed: cursor,
                        suspended: true,
                    });
                }
                continue;
            }
            if self.preamble_cursor < self.preamble_len {
                match encoding {
                    // A declaration selects a byte charset only after the
                    // ASCII preamble has been forwarded as UTF-8.
                    SourceEncoding::Utf8 | SourceEncoding::Byte(_) => {
                        let preamble = self.preamble;
                        let progress = self.forward_utf8(
                            &preamble[self.preamble_cursor..self.preamble_len],
                            true,
                            &mut emit,
                        )?;
                        self.preamble_cursor += progress.consumed;
                        if progress.suspended {
                            return Ok(DecodeProgress {
                                consumed: cursor,
                                suspended: true,
                            });
                        }
                    }
                    _ => {
                        let offset = self.preamble_cursor;
                        let byte = self.preamble[offset];
                        self.preamble_cursor += 1;
                        self.unit_byte(encoding, byte, offset)?;
                    }
                }
                continue;
            }
            if cursor == input.len() {
                return Ok(DecodeProgress {
                    consumed: cursor,
                    suspended: false,
                });
            }
            match encoding {
                SourceEncoding::Utf8 => {
                    let progress = self.forward_utf8(&input[cursor..], false, &mut emit)?;
                    cursor += progress.consumed;
                    if progress.suspended {
                        return Ok(DecodeProgress {
                            consumed: cursor,
                            suspended: true,
                        });
                    }
                }
                SourceEncoding::Byte(table) => {
                    // ASCII runs are forwarded in bounded windows, so a token
                    // suspension never causes the remaining input to be rescanned;
                    // one high byte at a time goes through the table and carry.
                    let rest = &input[cursor..];
                    let window = &rest[..rest.len().min(ASCII_WINDOW)];
                    let run = window
                        .iter()
                        .position(|b| *b >= 0x80)
                        .unwrap_or(window.len());
                    if run > 0 {
                        let progress = self.forward_utf8(&rest[..run], false, &mut emit)?;
                        cursor += progress.consumed;
                        if progress.suspended {
                            return Ok(DecodeProgress {
                                consumed: cursor,
                                suspended: true,
                            });
                        }
                        continue;
                    }
                    let offset = self.position;
                    self.position += 1;
                    cursor += 1;
                    let scalar = char::from_u32(table[usize::from(rest[0] - 0x80)].into())
                        .filter(|scalar| *scalar != '\0')
                        .ok_or(Error::new(ErrorKind::UnmappedByte, offset))?;
                    self.carry(scalar)?;
                }
                _ => {
                    let offset = self.position;
                    self.position += 1;
                    let byte = input[cursor];
                    cursor += 1;
                    self.unit_byte(encoding, byte, offset)?;
                }
            }
        }
    }

    /// Applies a declaration-selected encoding to the bytes after the declaration.
    pub(crate) fn switch(&mut self, encoding: SourceEncoding) {
        self.encoding = Some(encoding);
        self.custom = None;
    }

    /// [`Decoder::advance`] while caller-supplied converters decide or convert.
    fn advance_custom(
        &mut self,
        input: &[u8],
        emit: &mut impl FnMut(Decoded<'_>) -> Result<DecodeProgress, Error>,
    ) -> Result<DecodeProgress, Error> {
        let mut custom = self.custom.take().expect("converters");
        let mut cursor = 0;
        let mut step = || loop {
            if !custom.sniffing {
                let pending = match custom.converter {
                    Some(_) => custom.out.as_bytes(),
                    None => &custom.head,
                };
                if custom.cursor < pending.len() {
                    let progress = self.forward_utf8(&pending[custom.cursor..], true, emit)?;
                    custom.cursor += progress.consumed;
                    if progress.suspended {
                        break Ok(DecodeProgress {
                            consumed: cursor,
                            suspended: true,
                        });
                    }
                    continue;
                }
                if let Some(error) = custom.failed {
                    break Err(error);
                }
                if custom.converter.is_none() {
                    // The head ends at its `?>`: what follows is UTF-8 or the
                    // built-in charset the declaration switches to.
                    self.encoding = Some(SourceEncoding::Utf8);
                    break Ok(DecodeProgress {
                        consumed: cursor,
                        suspended: false,
                    });
                }
            }
            let rest = &input[cursor..];
            if rest.is_empty() {
                break Ok(DecodeProgress {
                    consumed: cursor,
                    suspended: false,
                });
            }
            if custom.sniffing {
                let take = match custom.head.last() == Some(&b'?') && rest[0] == b'>' {
                    true => 1,
                    false => memchr::memmem::find(rest, b"?>").map_or(rest.len(), |end| end + 2),
                }
                .min(HEAD_LIMIT - custom.head.len());
                (custom.head)
                    .try_reserve(take)
                    .map_err(|_| Error::new(ErrorKind::Allocation, self.position))?;
                custom.head.extend_from_slice(&rest[..take]);
                (self.position, cursor) = (self.position + take, cursor + take);
                if custom.head.ends_with(b"?>") || custom.head.len() == HEAD_LIMIT {
                    self.decide(&mut custom, false, emit)?;
                }
                continue;
            }
            let window = &rest[..rest.len().min(CONVERT_WINDOW)];
            convert(&mut custom, window, false, self.decoded)?;
            (self.position, cursor) = (self.position + window.len(), cursor + window.len());
        };
        let progress = step();
        if self.encoding == Some(SourceEncoding::Custom) {
            self.custom = Some(custom);
        }
        progress
    }

    /// Ends sniffing: the head's label selects a converter, which converts
    /// the head too, or the head is read as UTF-8.
    fn decide(
        &mut self,
        custom: &mut Custom,
        last: bool,
        emit: &mut impl FnMut(Decoded<'_>) -> Result<DecodeProgress, Error>,
    ) -> Result<(), Error> {
        custom.sniffing = false;
        let label = declared_label(&custom.head).filter(|label| charset(label).is_none());
        custom.converter = label.and_then(|label| custom.converters.open(label));
        let encoding = match custom.converter {
            Some(_) => {
                let head = std::mem::take(&mut custom.head);
                convert(custom, &head, last, 0)?;
                SourceEncoding::Custom
            }
            None => SourceEncoding::Utf8,
        };
        let progress = emit(Decoded::Start { encoding, bom: 0 })?;
        check_progress(progress, 0, self.position)
    }

    /// Ends a converted input: decides an undecided head, then flushes the
    /// converter once its output so far is forwarded. True when it produced
    /// output to forward.
    fn finish_custom(
        &mut self,
        emit: &mut impl FnMut(Decoded<'_>) -> Result<DecodeProgress, Error>,
    ) -> Result<bool, Error> {
        let mut custom = self.custom.take().expect("converters");
        let mut flush = || {
            if custom.sniffing {
                self.decide(&mut custom, true, emit)?;
            }
            let flush = custom.converter.is_some()
                && !custom.last
                && custom.failed.is_none()
                && custom.cursor == custom.out.len();
            if flush {
                convert(&mut custom, &[], true, self.decoded)?;
            }
            Ok(flush)
        };
        let flushed = flush();
        self.custom = Some(custom);
        flushed
    }

    pub(crate) fn finish_step(
        &mut self,
        mut emit: impl FnMut(Decoded<'_>) -> Result<DecodeProgress, Error>,
    ) -> Result<bool, Error> {
        if self.encoding.is_none() {
            let selection = select(&self.preamble[..self.preamble_len], true)
                .expect("final encoding selection");
            self.custom = None;
            self.start(selection?, &mut emit)?;
        }
        if self.encoding == Some(SourceEncoding::Custom) {
            self.finish_custom(&mut emit)?;
        }
        if self.advance(&[], &mut emit)?.suspended {
            return Ok(true);
        }
        if self.encoding == Some(SourceEncoding::Custom)
            && self.finish_custom(&mut emit)?
            && self.advance(&[], &mut emit)?.suspended
        {
            return Ok(true);
        }
        if let Some((_, offset)) = self.high {
            return Err(Error::new(ErrorKind::InvalidUtf16, offset));
        }
        if let (_, 1.., offset) = self.unit {
            return Err(Error::new(ErrorKind::InvalidUtf16, offset));
        }
        Ok(false)
    }

    fn start(
        &mut self,
        (encoding, bom): (SourceEncoding, usize),
        emit: &mut impl FnMut(Decoded<'_>) -> Result<DecodeProgress, Error>,
    ) -> Result<(), Error> {
        self.encoding = Some(encoding);
        self.preamble_cursor = bom;
        let progress = emit(Decoded::Start { encoding, bom })?;
        check_progress(progress, 0, self.position)
    }

    fn forward_utf8(
        &mut self,
        input: &[u8],
        already_consumed: bool,
        emit: &mut impl FnMut(Decoded<'_>) -> Result<DecodeProgress, Error>,
    ) -> Result<DecodeProgress, Error> {
        let allowed = input.len().min(self.limit - self.decoded);
        if allowed > 0 {
            let progress = emit(Decoded::Bytes(&input[..allowed]))?;
            check_progress(progress, allowed, self.position)?;
            self.decoded += progress.consumed;
            if !already_consumed {
                self.position += progress.consumed;
            }
            if progress.suspended {
                return Ok(progress);
            }
        }
        if allowed < input.len() {
            if !already_consumed {
                self.position += 1;
            }
            return Err(Error::new(ErrorKind::LimitExceeded(Limit::DecodedBytes), 0));
        }
        Ok(DecodeProgress {
            consumed: allowed,
            suspended: false,
        })
    }

    /// Decodes one byte of an encoding the decoder does not forward as UTF-8.
    fn unit_byte(
        &mut self,
        encoding: SourceEncoding,
        byte: u8,
        offset: usize,
    ) -> Result<(), Error> {
        let (little_endian, width) = match encoding {
            SourceEncoding::Utf16 { little_endian, .. } => (little_endian, 2),
            SourceEncoding::Ucs4 { little_endian } => (little_endian, 4),
            SourceEncoding::Ebcdic(table) => {
                let scalar = char::from_u32(table[usize::from(byte)].into())
                    .filter(|scalar| *scalar != '\0')
                    .ok_or(Error::new(ErrorKind::UnmappedByte, offset))?;
                return self.carry(scalar);
            }
            SourceEncoding::Utf8 | SourceEncoding::Byte(_) | SourceEncoding::Custom => {
                unreachable!("forwarded as UTF-8")
            }
        };
        let (mut bytes, length, unit_offset) = self.unit;
        let unit_offset = if length == 0 { offset } else { unit_offset };
        bytes[length] = byte;
        if length + 1 < width {
            self.unit = (bytes, length + 1, unit_offset);
            return Ok(());
        }
        self.unit = ([0; 4], 0, 0);
        let ordered = |i: usize| bytes[if little_endian { width - 1 - i } else { i }];
        let unit = (0..width).fold(0, |unit, i| unit << 8 | u32::from(ordered(i)));
        let scalar = if width == 4 {
            unit
        } else if let Some((high, high_offset)) = self.high.take() {
            if !(0xdc00..=0xdfff).contains(&unit) {
                return Err(Error::new(ErrorKind::InvalidUtf16, high_offset));
            }
            0x10000 + ((high - 0xd800) << 10) + unit - 0xdc00
        } else if (0xd800..=0xdbff).contains(&unit) {
            self.high = Some((unit, unit_offset));
            return Ok(());
        } else {
            unit
        };
        // Lone UTF-16 low surrogates and UCS-4 values past the scalars fail here.
        let scalar = char::from_u32(scalar).ok_or(Error::new(ErrorKind::InvalidUtf16, unit_offset));
        self.carry(scalar?)
    }

    /// Where a character cut off at the end of the input starts.
    fn pending_start(&self) -> Option<usize> {
        let (_, length, offset) = self.unit;
        self.high
            .map(|(_, offset)| offset)
            .or((length > 0).then_some(offset))
    }

    fn carry(&mut self, character: char) -> Result<(), Error> {
        if character.len_utf8() > self.limit - self.decoded {
            return Err(Error::new(ErrorKind::LimitExceeded(Limit::DecodedBytes), 0));
        }
        self.pending_len = character.encode_utf8(&mut self.pending).len();
        self.pending_cursor = 0;
        // A complete scalar is charged before any of its bytes can
        // trigger a tokenizer suspension or malformed-prefix error.
        self.decoded += character.len_utf8();
        Ok(())
    }

    #[cfg(test)]
    fn feed(
        &mut self,
        input: &[u8],
        mut emit: impl FnMut(Decoded<'_>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.advance(input, |decoded| {
            let consumed = match &decoded {
                Decoded::Start { .. } => 0,
                Decoded::Bytes(bytes) => bytes.len(),
            };
            emit(decoded)?;
            Ok(DecodeProgress {
                consumed,
                suspended: false,
            })
        })
        .map(|_| ())
    }

    #[cfg(test)]
    fn finish(
        &mut self,
        mut emit: impl FnMut(Decoded<'_>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.finish_step(|decoded| {
            let consumed = match &decoded {
                Decoded::Start { .. } => 0,
                Decoded::Bytes(bytes) => bytes.len(),
            };
            emit(decoded)?;
            Ok(DecodeProgress {
                consumed,
                suspended: false,
            })
        })
        .map(|_| ())
    }
}

fn check_progress(progress: DecodeProgress, available: usize, offset: usize) -> Result<(), Error> {
    if progress.consumed > available || (!progress.suspended && progress.consumed != available) {
        return Err(Error::new(ErrorKind::InvalidState, offset));
    }
    Ok(())
}

/// Bytes scanned per byte-charset ASCII run; bounds rescanning after suspension.
const ASCII_WINDOW: usize = 256;

/// UCS-4 BOMs and `<` signatures, then EBCDIC `<?xm`: libxml2 has no built-in
/// decoder for them. Without the `encodings` feature none is decoded; with it
/// only the BOMs and the 2143/3412 byte orders stay unsupported.
const UNSUPPORTED: [[u8; 4]; 9] = [
    [0, 0, 0xfe, 0xff],
    [0xff, 0xfe, 0, 0],
    [0, 0, 0xff, 0xfe],
    [0xfe, 0xff, 0, 0],
    [0, 0, 0, b'<'],
    [b'<', 0, 0, 0],
    [0, 0, b'<', 0],
    [0, b'<', 0, 0],
    EBCDIC,
];

/// The UCS-4 `<` signatures and EBCDIC `<?xm` the `encodings` feature decodes.
#[cfg(feature = "encodings")]
const DETECTED: [([u8; 4], SourceEncoding); 3] = [
    (
        [0, 0, 0, b'<'],
        SourceEncoding::Ucs4 {
            little_endian: false,
        },
    ),
    (
        [b'<', 0, 0, 0],
        SourceEncoding::Ucs4 {
            little_endian: true,
        },
    ),
    (EBCDIC, SourceEncoding::Ebcdic(IBM037)),
];

const EBCDIC: [u8; 4] = [0x4c, 0x6f, 0xa7, 0x94];

fn select(prefix: &[u8], final_input: bool) -> Option<Result<(SourceEncoding, usize), Error>> {
    #[cfg(feature = "encodings")]
    if let Some((_, encoding)) = DETECTED.iter().find(|(signature, _)| prefix == signature) {
        return Some(Ok((*encoding, 0)));
    }
    if UNSUPPORTED.iter().any(|candidate| prefix == candidate) {
        return Some(Err(Error::new(ErrorKind::UnsupportedEncoding, 0)));
    }
    if !final_input
        && UNSUPPORTED
            .iter()
            .any(|candidate| candidate.starts_with(prefix))
    {
        return None;
    }
    if prefix.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Some(Ok((SourceEncoding::Utf8, 3)));
    }
    for (bom, little_endian) in [([0xff, 0xfe], true), ([0xfe, 0xff], false)] {
        if prefix.starts_with(&bom) {
            return Some(Ok((
                SourceEncoding::Utf16 {
                    little_endian,
                    bom: true,
                },
                2,
            )));
        }
    }
    for (signature, little_endian) in [([b'<', 0, b'?', 0], true), ([0, b'<', 0, b'?'], false)] {
        if prefix == signature {
            return Some(Ok((
                SourceEncoding::Utf16 {
                    little_endian,
                    bom: false,
                },
                0,
            )));
        }
    }
    if !final_input
        && [
            &[0xef, 0xbb, 0xbf][..],
            &[b'<', 0, b'?', 0],
            &[0, b'<', 0, b'?'],
        ]
        .iter()
        .any(|candidate| candidate.starts_with(prefix))
    {
        return None;
    }
    Some(Ok((SourceEncoding::Utf8, 0)))
}

/// Recovery's copy of input that is not UTF-8, as libxml2's
/// `xmlCurrentCharRecover` reads it: each byte that starts no valid sequence
/// is U+FFFD. `None` for UTF-8 input; otherwise the copy and the error at
/// the first such byte.
pub(crate) fn repair_utf8(input: &[u8]) -> Option<Result<(String, Error), Error>> {
    let first = std::str::from_utf8(input).err()?.valid_up_to();
    let allocation = || Error::new(ErrorKind::Allocation, first);
    let mut repaired = String::new();
    let mut rest = input;
    let repaired = loop {
        if repaired.try_reserve(rest.len().saturating_add(3)).is_err() {
            break Err(allocation());
        }
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                repaired.push_str(valid);
                break Ok(repaired);
            }
            Err(error) => {
                let (valid, invalid) = rest.split_at(error.valid_up_to());
                repaired.push_str(std::str::from_utf8(valid).expect("valid prefix"));
                repaired.push('\u{fffd}');
                rest = &invalid[1..];
            }
        }
    };
    Some(repaired.map(|text| (text, Error::new(ErrorKind::InvalidUtf8, first))))
}

/// Choose the owning constructor's path without decoding or allocating.
/// Whether `input` is not plain UTF-8: also, given `converters`, when its
/// declaration names a label no built-in charset has.
pub(crate) fn needs_decoding(input: &[u8], converters: bool) -> bool {
    converters && declared_label(input).is_some_and(|label| charset(label).is_none())
        || input.starts_with(&[0xff, 0xfe])
        || input.starts_with(&[0xfe, 0xff])
        || input.starts_with(&[0, b'<', 0, b'?'])
        || input.starts_with(&[b'<', 0, b'?', 0])
        || UNSUPPORTED
            .iter()
            .any(|signature| input.starts_with(signature))
        || declared(input).is_some()
}

/// The encoding a leading declaration switches `input` to after it: a
/// single-byte charset, or another code page for EBCDIC input.
pub(crate) fn declared(input: &[u8]) -> Option<SourceEncoding> {
    let ebcdic = input.starts_with(&EBCDIC);
    match (
        ebcdic,
        declared_label(&ascii_head(input)).and_then(charset)?,
    ) {
        (false, Charset::Byte(table)) => Some(SourceEncoding::Byte(table)),
        (true, Charset::Ebcdic(table)) => Some(SourceEncoding::Ebcdic(table)),
        _ => None,
    }
}

/// The input a leading declaration is read from: EBCDIC through its
/// `?>` in IBM037, as `xmlDetectEBCDIC` reads it.
fn ascii_head(input: &[u8]) -> Cow<'_, [u8]> {
    #[cfg(feature = "encodings")]
    if input.starts_with(&EBCDIC) {
        let end = memchr::memmem::find(input, &[0x6f, 0x6e]).map_or(input.len(), |end| end + 2);
        // IBM037 maps every byte to Latin-1.
        return Cow::Owned(
            input[..end]
                .iter()
                .map(|&b| IBM037[usize::from(b)] as u8)
                .collect(),
        );
    }
    Cow::Borrowed(input)
}

/// The label a leading declaration keeps when the parser recovers from its
/// errors, which can drop it.
fn recovered_label(input: &[u8]) -> Option<&str> {
    let end = memchr::memmem::find(input, b"?>")?;
    let body = std::str::from_utf8(input.get(5..end)?).ok()?;
    let (declaration, _) =
        crate::parser::parse_declaration(body, 5, false, &mut |_| Ok(())).ok()?;
    declaration.encoding
}

/// Decodes complete `input` into `output` as the streams do, in `switch`
/// after the declaration naming it (see [`declared`]): the detected encoding
/// and how decoding ended (`output` then holds the text before the error).
pub(crate) fn transcode(
    input: &[u8],
    switch: Option<SourceEncoding>,
    converters: Option<Arc<dyn Converters>>,
    limit: usize,
    output: &mut Vec<u8>,
) -> (SourceEncoding, Result<(), Error>) {
    let mut decoder = Decoder::new(limit);
    decoder.set_converters(converters);
    let mut encoding = SourceEncoding::Utf8;
    let mut sink = |decoded: Decoded<'_>| {
        let consumed = match decoded {
            Decoded::Start {
                encoding: found, ..
            } => {
                encoding = found;
                0
            }
            Decoded::Bytes(bytes) => {
                output
                    .try_reserve(bytes.len())
                    .map_err(|_| Error::new(ErrorKind::Allocation, 0))?;
                output.extend_from_slice(bytes);
                bytes.len()
            }
        };
        Ok(DecodeProgress {
            consumed,
            suspended: false,
        })
    };
    let close: &[u8] = match switch {
        Some(SourceEncoding::Ebcdic(_)) => &[0x6f, 0x6e],
        _ => b"?>",
    };
    let split = switch.map_or(0, |_| {
        memchr::memmem::find(input, close).expect("declared label is terminated") + 2
    });
    let end = (|| {
        decoder.advance(&input[..split], &mut sink)?;
        if let Some(switch) = switch {
            decoder.switch(switch);
        }
        decoder.advance(&input[split..], &mut sink)?;
        decoder.finish_step(&mut sink).map(drop)
    })();
    (switch.unwrap_or(encoding), end)
}

/// Locates a leading declaration's label only to route it to the decoder;
/// the parser still validates the declaration grammar.
pub(crate) fn declared_label(input: &[u8]) -> Option<&str> {
    let space = |b: &u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n');
    let skip = |b: &[u8]| -> usize { b.iter().position(|c| !space(c)).unwrap_or(b.len()) };
    let head = input.strip_prefix(b"<?xml")?;
    let head = &head[..memchr::memmem::find(head, b"?>")?];
    if !space(head.first()?) {
        return None;
    }
    let value = &head[memchr::memmem::find(head, b"encoding")? + 8..];
    let value = value[skip(value)..].strip_prefix(b"=")?;
    let (&quote, value) = value[skip(value)..]
        .split_first()
        .filter(|(quote, _)| matches!(quote, b'\'' | b'"'))?;
    std::str::from_utf8(&value[..memchr::memchr(quote, value)?]).ok()
}

#[cfg(test)]
#[path = "../tests/internal/encoding.rs"]
mod tests;
