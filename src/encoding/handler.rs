//! libxml2's public encoding API (encoding.h) over the built-in charsets.

use super::{Charset, DecodeProgress, Decoded, Decoder, Output, SourceEncoding, charset, charsets};
use crate::ErrorKind;

/// libxml2's `xmlCharEncoding`: the encodings it names, with its numbers.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(i32)]
pub enum CharEncoding {
    /// An unknown label.
    Error = -1,
    /// Nothing detected.
    None = 0,
    /// UTF-8.
    Utf8 = 1,
    /// UTF-16, little-endian.
    Utf16Le = 2,
    /// UTF-16, big-endian.
    Utf16Be = 3,
    /// UCS-4, little-endian.
    Ucs4Le = 4,
    /// UCS-4, big-endian.
    Ucs4Be = 5,
    /// EBCDIC (IBM037).
    Ebcdic = 6,
    /// UCS-4 in byte order 2143.
    Ucs4_2143 = 7,
    /// UCS-4 in byte order 3412.
    Ucs4_3412 = 8,
    /// UCS-2.
    Ucs2 = 9,
    /// ISO-8859-1.
    Iso8859_1 = 10,
    /// ISO-8859-2.
    Iso8859_2 = 11,
    /// ISO-8859-3.
    Iso8859_3 = 12,
    /// ISO-8859-4.
    Iso8859_4 = 13,
    /// ISO-8859-5.
    Iso8859_5 = 14,
    /// ISO-8859-6.
    Iso8859_6 = 15,
    /// ISO-8859-7.
    Iso8859_7 = 16,
    /// ISO-8859-8.
    Iso8859_8 = 17,
    /// ISO-8859-9.
    Iso8859_9 = 18,
    /// ISO-2022-JP.
    Iso2022Jp = 19,
    /// Shift_JIS.
    ShiftJis = 20,
    /// EUC-JP.
    EucJp = 21,
    /// US-ASCII.
    Ascii = 22,
    /// UTF-16 of unknown byte order (`xmlParseCharEncoding` returns
    /// [`CharEncoding::Utf16Le`] for its labels).
    Utf16 = 23,
    /// libxml2's output-only HTML handler: no handler here.
    Html = 24,
    /// ISO-8859-10.
    Iso8859_10 = 25,
    /// ISO-8859-11.
    Iso8859_11 = 26,
    /// ISO-8859-13.
    Iso8859_13 = 27,
    /// ISO-8859-14.
    Iso8859_14 = 28,
    /// ISO-8859-15.
    Iso8859_15 = 29,
    /// ISO-8859-16.
    Iso8859_16 = 30,
    /// windows-1252.
    Windows1252 = 31,
}

impl CharEncoding {
    /// `xmlDetectCharEncoding`: the encoding a document's first bytes
    /// announce: a `<` or `<?` in UTF-8, UTF-16 or UCS-4, EBCDIC `<?xm`, or
    /// a byte order mark.
    ///
    /// ```
    /// use yaxml::encoding::CharEncoding;
    ///
    /// assert_eq!(CharEncoding::detect(b"<\0?\0x\0"), CharEncoding::Utf16Le);
    /// assert_eq!(CharEncoding::detect(b"\xef\xbb\xbf<r/>"), CharEncoding::Utf8);
    /// assert_eq!(CharEncoding::detect(b"<r/>"), CharEncoding::None);
    /// ```
    pub fn detect(input: &[u8]) -> Self {
        match input {
            [0, 0, 0, b'<', ..] => Self::Ucs4Be,
            [b'<', 0, 0, 0, ..] => Self::Ucs4Le,
            [0x4c, 0x6f, 0xa7, 0x94, ..] => Self::Ebcdic,
            [b'<', b'?', b'x', b'm', ..] | [0xef, 0xbb, 0xbf, ..] => Self::Utf8,
            [b'<', 0, b'?', 0, ..] | [0xff, 0xfe, ..] => Self::Utf16Le,
            [0, b'<', 0, b'?', ..] | [0xfe, 0xff, ..] => Self::Utf16Be,
            _ => Self::None,
        }
    }

    /// `xmlParseCharEncoding`: the encoding of a label, ASCII
    /// case-insensitively, from libxml2's label table; [`CharEncoding::Error`]
    /// for an unknown one.
    ///
    /// ```
    /// use yaxml::encoding::CharEncoding;
    ///
    /// assert_eq!(CharEncoding::parse("Latin1"), CharEncoding::Iso8859_1);
    /// assert_eq!(CharEncoding::parse("UTF-16"), CharEncoding::Utf16Le);
    /// assert_eq!(CharEncoding::parse("Shift_JIS"), CharEncoding::ShiftJis);
    /// assert_eq!(CharEncoding::parse("klingon"), CharEncoding::Error);
    /// ```
    pub fn parse(name: &str) -> Self {
        match Self::labelled(name) {
            Self::Utf16 => Self::Utf16Le,
            encoding => encoding,
        }
    }

    /// `xmlGetCharEncodingName`: the canonical name, `UTF-16` and `UCS-4`
    /// for either byte order; None for [`CharEncoding::Error`],
    /// [`CharEncoding::None`] and the unnamed UCS-4 orders.
    pub fn name(self) -> Option<&'static str> {
        match self {
            Self::Utf16Le | Self::Utf16Be => Some("UTF-16"),
            Self::Ucs4Le | Self::Ucs4Be => Some("UCS-4"),
            _ => self.handler_name(),
        }
    }

    /// The label table entry, before `xmlParseCharEncoding`'s UTF-16 remap.
    pub(super) fn labelled(name: &str) -> Self {
        let key = name.bytes().map(|b| b.to_ascii_lowercase());
        charsets::LABELS
            .binary_search_by(|(label, _)| label.bytes().cmp(key.clone()))
            .map_or(Self::Error, |index| charsets::LABELS[index].1)
    }

    /// The name of libxml2's default handler (`defaultHandlers`).
    fn handler_name(self) -> Option<&'static str> {
        Some(match self {
            Self::Error | Self::None | Self::Ucs4_2143 | Self::Ucs4_3412 => return None,
            Self::Utf8 => "UTF-8",
            Self::Utf16Le => "UTF-16LE",
            Self::Utf16Be => "UTF-16BE",
            Self::Ucs4Le => "UCS-4LE",
            Self::Ucs4Be => "UCS-4BE",
            Self::Ebcdic => "IBM037",
            Self::Ucs2 => "UCS-2",
            Self::Iso8859_1 => "ISO-8859-1",
            Self::Iso8859_2 => "ISO-8859-2",
            Self::Iso8859_3 => "ISO-8859-3",
            Self::Iso8859_4 => "ISO-8859-4",
            Self::Iso8859_5 => "ISO-8859-5",
            Self::Iso8859_6 => "ISO-8859-6",
            Self::Iso8859_7 => "ISO-8859-7",
            Self::Iso8859_8 => "ISO-8859-8",
            Self::Iso8859_9 => "ISO-8859-9",
            Self::Iso2022Jp => "ISO-2022-JP",
            Self::ShiftJis => "Shift_JIS",
            Self::EucJp => "EUC-JP",
            Self::Ascii => "US-ASCII",
            Self::Utf16 => "UTF-16",
            Self::Html => "HTML",
            Self::Iso8859_10 => "ISO-8859-10",
            Self::Iso8859_11 => "ISO-8859-11",
            Self::Iso8859_13 => "ISO-8859-13",
            Self::Iso8859_14 => "ISO-8859-14",
            Self::Iso8859_15 => "ISO-8859-15",
            Self::Iso8859_16 => "ISO-8859-16",
            Self::Windows1252 => "windows-1252",
        })
    }

    /// The converter libxml2 builds in for it, without iconv/ICU.
    pub(crate) fn builtin(self) -> Option<Charset> {
        Some(match self {
            Self::Utf8 => Charset::Utf8,
            Self::Utf16 => Charset::Utf16(None),
            Self::Utf16Le => Charset::Utf16(Some(true)),
            Self::Utf16Be => Charset::Utf16(Some(false)),
            Self::Ascii => Charset::Byte(&charsets::US_ASCII),
            Self::Iso8859_1 => Charset::Byte(&charsets::ISO_8859_1),
            Self::Iso8859_2 => Charset::Byte(&charsets::ISO_8859_2),
            Self::Iso8859_3 => Charset::Byte(&charsets::ISO_8859_3),
            Self::Iso8859_4 => Charset::Byte(&charsets::ISO_8859_4),
            Self::Iso8859_5 => Charset::Byte(&charsets::ISO_8859_5),
            Self::Iso8859_6 => Charset::Byte(&charsets::ISO_8859_6),
            Self::Iso8859_7 => Charset::Byte(&charsets::ISO_8859_7),
            Self::Iso8859_8 => Charset::Byte(&charsets::ISO_8859_8),
            Self::Iso8859_9 => Charset::Byte(&charsets::ISO_8859_9),
            Self::Iso8859_10 => Charset::Byte(&charsets::ISO_8859_10),
            Self::Iso8859_11 => Charset::Byte(&charsets::ISO_8859_11),
            Self::Iso8859_13 => Charset::Byte(&charsets::ISO_8859_13),
            Self::Iso8859_14 => Charset::Byte(&charsets::ISO_8859_14),
            Self::Iso8859_15 => Charset::Byte(&charsets::ISO_8859_15),
            Self::Iso8859_16 => Charset::Byte(&charsets::ISO_8859_16),
            Self::Windows1252 => Charset::Byte(&charsets::WINDOWS_1252),
            _ => return None,
        })
    }
}

/// libxml2's `xmlCharEncError`: why a conversion stopped.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(i32)]
pub enum EncError {
    /// Never returned here; libxml2's code for a bad argument.
    Internal = -1,
    /// The input holds a byte sequence the charset does not map.
    Input = -2,
    /// Never returned here: output buffers grow.
    Space = -3,
    /// The output could not be allocated.
    Memory = -4,
}

/// libxml2's `xmlCharEncodingHandler`: a converter between a charset and
/// UTF-8 in both directions, for the built-in charsets (feature `encodings`
/// adds EBCDIC and UCS-4, as the iconv libxml2 uses on macOS provides).
///
/// ```
/// use yaxml::encoding::Handler;
///
/// let latin2 = Handler::open("latin2").unwrap();
/// let mut text = String::new();
/// assert_eq!(latin2.decode(b"\xa3\xf3d\xbc", &mut text), Ok(4));
/// assert_eq!(text, "\u{141}\u{f3}d\u{17a}");
/// let mut bytes = Vec::new();
/// assert_eq!(latin2.encode("\u{141}\u{f3}d\u{17a} \u{263a}", &mut bytes), Ok(()));
/// assert_eq!(bytes, b"\xa3\xf3d\xbc &#9786;");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Handler {
    name: Box<str>,
    charset: Charset,
}

impl Handler {
    /// `xmlLookupCharEncodingHandler`: the default handler of `encoding`;
    /// `Ok(None)` for UTF-8, which needs none, and
    /// [`ErrorKind::UnsupportedEncoding`] when nothing converts it,
    /// [`CharEncoding::None`] and [`CharEncoding::Error`] included.
    pub fn lookup(encoding: CharEncoding) -> Result<Option<Self>, ErrorKind> {
        if encoding == CharEncoding::Utf8 {
            return Ok(None);
        }
        let name = encoding
            .handler_name()
            .ok_or(ErrorKind::UnsupportedEncoding)?;
        Self::open(name).map(Some)
    }

    /// `xmlOpenCharEncodingHandler`/`xmlFindCharEncodingHandler`: the
    /// handler of a label, named as given; UTF-8 has an identity handler.
    /// [`ErrorKind::UnsupportedEncoding`] when no built-in charset has the
    /// label: libxml2 would try iconv/ICU (and a parse can use
    /// [`ParseOptions::converters`](crate::ParseOptions::converters)).
    pub fn open(name: &str) -> Result<Self, ErrorKind> {
        let charset = charset(name).ok_or(ErrorKind::UnsupportedEncoding)?;
        Ok(Self {
            name: name.into(),
            charset,
        })
    }

    /// The input decoding it selects.
    pub(crate) fn source(&self) -> SourceEncoding {
        match self.charset {
            Charset::Utf8 => SourceEncoding::Utf8,
            Charset::Utf16(order) => SourceEncoding::Utf16 {
                little_endian: order.unwrap_or(true),
                bom: false,
            },
            Charset::Byte(table) => SourceEncoding::Byte(table),
            Charset::Ebcdic(table) => SourceEncoding::Ebcdic(table),
            Charset::Ucs4(order) => SourceEncoding::Ucs4 {
                little_endian: order == Some(true),
            },
        }
    }

    /// The name it was opened under (`handler->name`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The byte order mark libxml2 writes before encoded output (its
    /// initialization call of `xmlCharEncOutFunc`): generic UTF-16 and UCS-4.
    pub fn bom(&self) -> &'static [u8] {
        Output::from(self.charset).bom()
    }

    /// `xmlCharEncInFunc`: appends `input` as UTF-8 to `out` and returns the
    /// bytes read, which stop short of a character cut off at the end, as
    /// libxml2 leaves it in the input buffer. Generic UTF-16 reads
    /// little-endian (a BOM is a character). `Err((EncError::Input, read))`
    /// stops at a byte sequence the charset does not map, or invalid UTF-8
    /// (which libxml2's UTF-8 handler copies unchecked).
    pub fn decode(&self, input: &[u8], out: &mut String) -> Result<usize, (EncError, usize)> {
        if out.try_reserve(input.len().saturating_mul(3)).is_err() {
            return Err((EncError::Memory, 0));
        }
        let encoding = match self.charset {
            Charset::Utf8 => {
                let (text, error) = match std::str::from_utf8(input) {
                    Ok(text) => (text, None),
                    Err(error) => {
                        let valid = error.valid_up_to();
                        let text = std::str::from_utf8(&input[..valid]).expect("valid prefix");
                        (text, error.error_len().map(|_| (EncError::Input, valid)))
                    }
                };
                out.push_str(text);
                return error.map_or(Ok(text.len()), Err);
            }
            _ => self.source(),
        };
        let mut decoder = Decoder::new(usize::MAX);
        decoder.switch(encoding);
        let emitted = decoder.advance(input, |decoded| {
            if let Decoded::Bytes(bytes) = decoded {
                // Every scalar arrives whole: the decoder emits its UTF-8.
                out.push_str(std::str::from_utf8(bytes).expect("decoded UTF-8"));
            }
            Ok(DecodeProgress {
                consumed: match decoded {
                    Decoded::Bytes(bytes) => bytes.len(),
                    Decoded::Start { .. } => 0,
                },
                suspended: false,
            })
        });
        match emitted {
            Ok(_) => Ok(decoder.pending_start().unwrap_or(input.len())),
            Err(error) => Err((EncError::Input, error.offset)),
        }
    }

    /// `xmlCharEncOutFunc`: appends `input` in the charset to `out`; a
    /// character it lacks becomes a decimal character reference, as libxml2
    /// writes it. Only [`EncError::Memory`] fails it.
    pub fn encode(&self, input: &str, out: &mut Vec<u8>) -> Result<(), EncError> {
        let output = Output::from(self.charset);
        out.try_reserve(input.len().saturating_mul(4))
            .map_err(|_| EncError::Memory)?;
        for c in input.chars() {
            if !output.push(out, c) {
                let reference = format!("&#{};", u32::from(c));
                out.try_reserve(reference.len() * 4)
                    .map_err(|_| EncError::Memory)?;
                reference.chars().for_each(|c| {
                    output.push(out, c);
                });
            }
        }
        Ok(())
    }
}
