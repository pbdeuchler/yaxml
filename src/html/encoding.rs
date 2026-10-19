//! Character encodings for HTML, following libxml2 2.15.4: BOM sniffing
//! (`xmlDetectEncoding`), the `<meta>` declarations (`htmlCheckMeta`,
//! `htmlParseContentType`), the prescan (`htmlFindEncoding`) and the built-in
//! converters of `encoding.c` for the encodings implemented here.

use super::tables::{self, Charset, WINDOWS_1252};
use super::tokenizer::{Attribute, blank};
use crate::{Error, ErrorKind};

/// Converts input to the UTF-8 text the tokenizer reads. `Utf8` passes bytes
/// through; the tokenizer validates them as libxml2 does.
#[derive(Debug)]
pub(super) enum Decoder {
    Utf8,
    Windows1252,
    Utf16 { big_endian: bool, pending: Vec<u8> },
}

/// The start of the input: its decoder, BOM length and `doc->encoding`.
pub(super) struct Sniffed {
    pub(super) decoder: Decoder,
    pub(super) bom: usize,
    pub(super) has_encoding: bool,
    pub(super) name: Option<&'static str>,
}

/// `xmlDetectEncoding` for HTML (fewer than four bytes are never sniffed),
/// then the `<?xm` rule that selects UTF-8.
pub(super) fn sniff(prefix: &[u8]) -> Sniffed {
    let detected = |decoder, bom| Sniffed {
        decoder,
        bom,
        has_encoding: true,
        name: None,
    };
    let utf16 = |big_endian| Decoder::Utf16 {
        big_endian,
        pending: Vec::new(),
    };
    match prefix {
        [0xEF, 0xBB, 0xBF, _, ..] => detected(Decoder::Utf8, 3),
        [0xFE, 0xFF, _, _, ..] => detected(utf16(true), 2),
        [0xFF, 0xFE, _, _, ..] => detected(utf16(false), 2),
        [0x00, 0x3C, 0x00, 0x3F, ..] => detected(utf16(true), 0),
        [0x3C, 0x00, 0x3F, 0x00, ..] => detected(utf16(false), 0),
        [b'<', b'?', b'x', b'm', ..] => Sniffed {
            name: Some("UTF-8"),
            ..detected(Decoder::Utf8, 0)
        },
        _ => Sniffed {
            has_encoding: false,
            ..detected(Decoder::Utf8, 0)
        },
    }
}

impl Decoder {
    /// The input converter for a label (`xmlCreateCharEncodingHandler` with
    /// XML_ENC_HTML). None: libxml2 has no converter. ISO-8859-2..16 have one
    /// in libxml2 but not here, and fail explicitly.
    pub(super) fn for_label(label: &str) -> Result<Option<Self>, Error> {
        let utf16 = |big_endian| Self::Utf16 {
            big_endian,
            pending: Vec::new(),
        };
        Ok(match tables::charset(label) {
            Some(Charset::Utf8) => Some(Self::Utf8),
            Some(Charset::Ascii | Charset::Latin1 | Charset::Windows1252) => {
                Some(Self::Windows1252)
            }
            Some(Charset::Utf16 | Charset::Ucs2 | Charset::Utf16Le) => Some(utf16(false)),
            Some(Charset::Utf16Be) => Some(utf16(true)),
            Some(Charset::Iso8859) => {
                return Err(Error::new(ErrorKind::UnsupportedEncoding, 0));
            }
            Some(Charset::Html | Charset::Missing) | None => None,
        })
    }

    /// `htmlCharEncCheckAsciiCompatible`.
    pub(super) fn ascii_compatible(&self) -> bool {
        !matches!(self, Self::Utf16 { .. })
    }

    /// Appends decoded text and returns the output offset of the first
    /// malformed input, if any. Malformed UTF-16 becomes U+FFFD and a
    /// truncated final unit is dropped.
    pub(super) fn decode(&mut self, input: &[u8], last: bool, out: &mut Vec<u8>) -> Option<usize> {
        match self {
            Self::Utf8 => {
                out.extend_from_slice(input);
                None
            }
            Self::Windows1252 => {
                let mut buffer = [0; 4];
                for &byte in input {
                    let c = match byte {
                        0x80..=0x9F => u32::from(WINDOWS_1252[usize::from(byte - 0x80)]),
                        _ => u32::from(byte),
                    };
                    let c = char::from_u32(c).unwrap_or('\u{FFFD}');
                    out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
                }
                None
            }
            Self::Utf16 {
                big_endian,
                pending,
            } => {
                pending.extend_from_slice(input);
                let big_endian = *big_endian;
                let unit = |bytes: &[u8], i: usize| {
                    let pair = [bytes[i], bytes[i + 1]];
                    u32::from(if big_endian {
                        u16::from_be_bytes(pair)
                    } else {
                        u16::from_le_bytes(pair)
                    })
                };
                let mut malformed = None;
                let mut i = 0;
                let mut decoded = String::new();
                while i + 2 <= pending.len() {
                    let high = unit(pending, i);
                    let (c, width) = if !(0xD800..0xDC00).contains(&high) {
                        (char::from_u32(high), 2)
                    } else if i + 4 > pending.len() {
                        if !last {
                            break;
                        }
                        (None, 2)
                    } else {
                        let low = unit(pending, i + 2);
                        match low {
                            0xDC00..0xE000 => {
                                let code = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00);
                                (char::from_u32(code), 4)
                            }
                            _ => (None, 2),
                        }
                    };
                    if c.is_none() {
                        malformed = malformed.or(Some(out.len() + decoded.len()));
                    }
                    decoded.push(c.unwrap_or('\u{FFFD}'));
                    i += width;
                }
                pending.drain(..i);
                out.extend_from_slice(decoded.as_bytes());
                if last && !pending.is_empty() {
                    pending.clear();
                    malformed = malformed.or(Some(out.len()));
                }
                malformed
            }
        }
    }
}

/// `htmlParseContentType`: the byte range of the charset in a Content-Type value.
pub(super) fn content_charset(value: &str) -> Option<(usize, usize)> {
    let bytes = value.as_bytes();
    let mut p = 0;
    loop {
        p += bytes[p..].iter().position(|&c| c | 0x20 == b'c')? + 1;
        if !bytes
            .get(p..p + 6)
            .is_some_and(|word| word.eq_ignore_ascii_case(b"harset"))
        {
            continue;
        }
        p += 6;
        while bytes.get(p).copied().is_some_and(blank) {
            p += 1;
        }
        if bytes.get(p) != Some(&b'=') {
            continue;
        }
        p += 1;
        while bytes.get(p).copied().is_some_and(blank) {
            p += 1;
        }
        let quote = *bytes.get(p)?;
        if quote == b'"' || quote == b'\'' {
            p += 1;
            while bytes.get(p).copied().is_some_and(blank) {
                p += 1;
            }
            let start = p;
            let mut end = start;
            while *bytes.get(p)? != quote {
                if !blank(bytes[p]) {
                    end = p + 1;
                }
                p += 1;
            }
            return Some((start, end));
        }
        let start = p;
        while bytes.get(p).is_some_and(|&c| c != b';' && !blank(c)) {
            p += 1;
        }
        return Some((start, p));
    }
}

/// `htmlCheckMeta`: the encoding a `<meta>` start tag declares.
pub(super) fn meta_charset(attributes: &[Attribute]) -> Option<String> {
    let mut content_type = false;
    let mut content = None;
    for (name, value) in attributes {
        let Some(value) = value else { continue };
        if name.eq_ignore_ascii_case("http-equiv") && value.eq_ignore_ascii_case("Content-Type") {
            content_type = true;
        } else if name.eq_ignore_ascii_case("charset") {
            return Some(value.clone());
        } else if name.eq_ignore_ascii_case("content") {
            content = Some(value);
        }
    }
    let content = content.filter(|_| content_type)?;
    let (start, end) = content_charset(content)?;
    Some(content[start..end].to_owned())
}

fn find_ignore_case(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

/// `htmlFindEncoding`: the charset after `HTTP-EQUIV`, `CONTENT` and
/// `CHARSET=`, searched in order up to the first NUL.
pub(super) fn prescan(rest: &[u8]) -> Option<String> {
    let rest = &rest[..rest.iter().position(|&c| c == 0).unwrap_or(rest.len())];
    let mut p = find_ignore_case(rest, b"HTTP-EQUIV")?;
    p += find_ignore_case(&rest[p..], b"CONTENT")?;
    p += find_ignore_case(&rest[p..], b"CHARSET=")? + 8;
    let length = rest[p..]
        .iter()
        .take_while(|&&c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b':' | b'/'))
        .count();
    (length > 0).then(|| String::from_utf8_lossy(&rest[p..p + length]).into_owned())
}

/// The output converters `xmlOpenCharEncodingHandler` provides for the
/// encodings this crate can produce.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Output {
    Utf8,
    Latin1,
    Ascii,
    Windows1252,
    /// ASCII with HTML 4 entity names, used when a document has no encoding.
    Html,
}

impl Output {
    pub(super) fn for_label(label: Option<&str>) -> Result<Self, Error> {
        let Some(label) = label else {
            return Ok(Self::Html);
        };
        match tables::charset(label) {
            Some(Charset::Utf8) => Ok(Self::Utf8),
            Some(Charset::Latin1) => Ok(Self::Latin1),
            Some(Charset::Ascii) => Ok(Self::Ascii),
            Some(Charset::Windows1252) => Ok(Self::Windows1252),
            Some(Charset::Html) => Ok(Self::Html),
            _ => Err(Error::new(ErrorKind::UnsupportedEncoding, 0)),
        }
    }

    /// Converts UTF-8 output; unrepresentable characters become decimal
    /// character references, or entity names for `Html`.
    pub(super) fn encode(self, text: Vec<u8>) -> Vec<u8> {
        if self == Self::Utf8 || text.is_ascii() {
            return text;
        }
        let text = String::from_utf8(text)
            .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
        let mut out = Vec::with_capacity(text.len());
        for c in text.chars() {
            let code = u32::from(c);
            let byte = match self {
                _ if code < 0x80 => Some(code as u8),
                Self::Latin1 if code <= 0xFF => Some(code as u8),
                Self::Windows1252 if (0xA0..=0xFF).contains(&code) => Some(code as u8),
                Self::Windows1252 => WINDOWS_1252
                    .iter()
                    .position(|&mapped| u32::from(mapped) == code)
                    .map(|index| 0x80 + index as u8),
                _ => None,
            };
            let entity = (self == Self::Html)
                .then(|| tables::html4_entity(code))
                .flatten();
            match (byte, entity) {
                (Some(byte), _) => out.push(byte),
                (None, Some(name)) => out.extend_from_slice(format!("&{name};").as_bytes()),
                (None, None) => out.extend_from_slice(format!("&#{code};").as_bytes()),
            }
        }
        out
    }
}
