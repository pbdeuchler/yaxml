//! Character encodings for HTML, following libxml2 2.15.4: BOM sniffing
//! (`xmlDetectEncoding`), the `<meta>` declarations (`htmlCheckMeta`,
//! `htmlParseContentType`), the prescan (`htmlFindEncoding`) and the built-in
//! converters of `encoding.c` for the encodings implemented here.

use super::tables;
use super::tokenizer::{Attribute, blank};
use crate::encoding::{Charset, ISO_8859_1, Output, US_ASCII, WINDOWS_1252, charset};
use crate::{Error, ErrorKind};

/// Converts input to the UTF-8 text the tokenizer reads. `Utf8` passes bytes
/// through; the tokenizer validates them as libxml2 does.
#[derive(Clone, Debug)]
pub(super) enum Decoder {
    Utf8,
    /// A libxml2 single-byte table for bytes 0x80..=0xFF; 0 is unmapped.
    Byte(&'static [u16; 128]),
    Utf16 {
        big_endian: bool,
        pending: Vec<u8>,
    },
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
    /// XML_ENC_HTML, which reads ASCII and Latin-1 as windows-1252 and
    /// unmarked UTF-16 as little-endian). None: libxml2 has no converter.
    pub(super) fn for_label(label: &str) -> Option<Self> {
        Some(match charset(label)? {
            Charset::Utf8 => Self::Utf8,
            Charset::Byte(table) if table == US_ASCII || table == ISO_8859_1 => {
                Self::Byte(WINDOWS_1252)
            }
            Charset::Byte(table) => Self::Byte(table),
            Charset::Utf16(little_endian) => Self::Utf16 {
                big_endian: little_endian == Some(false),
                pending: Vec::new(),
            },
            Charset::Ebcdic(_) | Charset::Ucs4(_) => return None,
        })
    }

    /// The same converter with no input pending.
    pub(super) fn fresh(&self) -> Self {
        match self {
            Self::Utf8 => Self::Utf8,
            Self::Byte(table) => Self::Byte(table),
            Self::Utf16 { big_endian, .. } => Self::Utf16 {
                big_endian: *big_endian,
                pending: Vec::new(),
            },
        }
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
            Self::Byte(table) => {
                let mut malformed = None;
                let mut buffer = [0; 4];
                for &byte in input {
                    let code = match byte {
                        0x80.. => u32::from(table[usize::from(byte - 0x80)]),
                        _ => u32::from(byte),
                    };
                    let c = match char::from_u32(code).filter(|_| code != 0 || byte == 0) {
                        Some(c) => c,
                        None => {
                            malformed = malformed.or(Some(out.len()));
                            '\u{FFFD}'
                        }
                    };
                    out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
                }
                malformed
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

/// The output converter for `label`; `None` is libxml2's "HTML" converter
/// (ASCII with HTML 4 entity names), used when a document has no encoding.
pub(super) fn output(label: Option<&str>) -> Result<Option<Output>, Error> {
    match label {
        None => Ok(None),
        Some(label) if label.eq_ignore_ascii_case("html") => Ok(None),
        Some(label) => Output::for_label(label)
            .map(Some)
            .ok_or(Error::new(ErrorKind::UnsupportedEncoding, 0)),
    }
}

/// Appends UTF-8 `text` converted to `out`, without a byte-order mark;
/// unrepresentable characters become decimal character references, or
/// entity names for the "HTML" converter.
pub(super) fn encode(output: Option<Output>, text: &[u8], out: &mut Vec<u8>) {
    let ascii = !matches!(output, Some(Output::Utf16 { .. })) && text.is_ascii();
    if output == Some(Output::Utf8) || ascii {
        return out.extend_from_slice(text);
    }
    let text = String::from_utf8_lossy(text);
    out.reserve(text.len());
    for c in text.chars() {
        let pushed = match output {
            Some(output) => output.push(out, c),
            None => {
                c.is_ascii() && {
                    out.push(c as u8);
                    true
                }
            }
        };
        if !pushed {
            let code = u32::from(c);
            match output.map_or_else(|| tables::html4_entity(code), |_| None) {
                Some(name) => out.extend_from_slice(format!("&{name};").as_bytes()),
                None => out.extend_from_slice(format!("&#{code};").as_bytes()),
            }
        }
    }
}
