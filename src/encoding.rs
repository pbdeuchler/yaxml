use std::ops::Range;

use crate::{Error, ErrorKind, Limit};

/// Ceilings for the UTF-8 representation produced by input decoding.
/// Transport byte-order marks are excluded from both counters.
#[derive(Clone, Copy, Debug)]
pub struct DecodeLimits {
    /// Total UTF-8 bytes produced, excluding the transport BOM (default 64 MiB).
    pub max_decoded_bytes: usize,
    /// UTF-8 bytes in one lexical token (default 1 MiB).
    pub max_decoded_token_bytes: usize,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_decoded_bytes: 64 * 1024 * 1024,
            max_decoded_token_bytes: 1024 * 1024,
        }
    }
}

mod charsets;

/// A built-in libxml2 declaration label: UTF-16 carries an optional byte
/// order, and single-byte charsets map bytes 0x80..=0xFF (zero is unmapped).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Charset {
    Utf8,
    Utf16(Option<bool>),
    Byte(&'static [u16; 128]),
}

/// Resolves a label as libxml2 does without iconv/ICU: ASCII case-insensitively.
pub(crate) fn charset(label: &str) -> Option<Charset> {
    charsets::LABELS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(label))
        .map(|(_, charset)| *charset)
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
}

impl SourceEncoding {
    pub(crate) fn source_length(self, bytes: &[u8]) -> usize {
        match self {
            Self::Utf8 => bytes.len(),
            Self::Utf16 { .. } => bytes
                .iter()
                .map(|b| match b {
                    0..=0x7f | 0xc0..=0xef => 2,
                    0xf0..=0xf7 => 4,
                    _ => 0,
                })
                .sum(),
            Self::Byte(_) => bytes.iter().filter(|b| (**b & 0xc0) != 0x80).count(),
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
    byte: Option<(u8, usize)>,
    high: Option<(u16, usize)>,
    pending: [u8; 4],
    pending_cursor: usize,
    pending_len: usize,
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
            byte: None,
            high: None,
            pending: [0; 4],
            pending_cursor: 0,
            pending_len: 0,
        }
    }

    pub(crate) fn position(&self) -> usize {
        self.position
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
                    self.start(selection?, &mut emit)?;
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
                    SourceEncoding::Utf16 { little_endian, .. } => {
                        let offset = self.preamble_cursor;
                        let byte = self.preamble[offset];
                        self.preamble_cursor += 1;
                        self.unit_byte(byte, offset, little_endian)?;
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
                SourceEncoding::Utf16 { little_endian, .. } => {
                    let offset = self.position;
                    self.position += 1;
                    let byte = input[cursor];
                    cursor += 1;
                    self.unit_byte(byte, offset, little_endian)?;
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
            }
        }
    }

    /// Applies a declaration-selected encoding to the bytes after the declaration.
    pub(crate) fn switch(&mut self, encoding: SourceEncoding) {
        self.encoding = Some(encoding);
    }

    pub(crate) fn finish_step(
        &mut self,
        mut emit: impl FnMut(Decoded<'_>) -> Result<DecodeProgress, Error>,
    ) -> Result<bool, Error> {
        if self.encoding.is_none() {
            let selection = select(&self.preamble[..self.preamble_len], true)
                .expect("final encoding selection");
            self.start(selection?, &mut emit)?;
        }
        if self.advance(&[], emit)?.suspended {
            return Ok(true);
        }
        if let Some((_, offset)) = self.high {
            return Err(Error::new(ErrorKind::InvalidUtf16, offset));
        }
        if let Some((_, offset)) = self.byte {
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

    fn unit_byte(&mut self, byte: u8, offset: usize, little_endian: bool) -> Result<(), Error> {
        let Some((first, unit_offset)) = self.byte.take() else {
            self.byte = Some((byte, offset));
            return Ok(());
        };
        let unit = if little_endian {
            u16::from_le_bytes([first, byte])
        } else {
            u16::from_be_bytes([first, byte])
        };
        let scalar = if let Some((high, high_offset)) = self.high.take() {
            if !(0xdc00..=0xdfff).contains(&unit) {
                return Err(Error::new(ErrorKind::InvalidUtf16, high_offset));
            }
            0x10000 + ((u32::from(high) - 0xd800) << 10) + u32::from(unit) - 0xdc00
        } else if (0xd800..=0xdbff).contains(&unit) {
            self.high = Some((unit, unit_offset));
            return Ok(());
        } else if (0xdc00..=0xdfff).contains(&unit) {
            return Err(Error::new(ErrorKind::InvalidUtf16, unit_offset));
        } else {
            u32::from(unit)
        };
        self.carry(char::from_u32(scalar).expect("validated UTF-16 scalar"))
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

/// UCS-4 BOMs and `<` signatures, then EBCDIC `<?xm`: libxml2 detects these
/// but has no built-in decoder for them.
const UNSUPPORTED: [[u8; 4]; 9] = [
    [0, 0, 0xfe, 0xff],
    [0xff, 0xfe, 0, 0],
    [0, 0, 0xff, 0xfe],
    [0xfe, 0xff, 0, 0],
    [0, 0, 0, b'<'],
    [b'<', 0, 0, 0],
    [0, 0, b'<', 0],
    [0, b'<', 0, 0],
    [0x4c, 0x6f, 0xa7, 0x94],
];

fn select(prefix: &[u8], final_input: bool) -> Option<Result<(SourceEncoding, usize), Error>> {
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

/// Choose the owning constructor's path without decoding or allocating.
pub(crate) fn needs_decoding(input: &[u8]) -> bool {
    input.starts_with(&[0xff, 0xfe])
        || input.starts_with(&[0xfe, 0xff])
        || input.starts_with(&[0, b'<', 0, b'?'])
        || input.starts_with(&[b'<', 0, b'?', 0])
        || UNSUPPORTED
            .iter()
            .any(|signature| input.starts_with(signature))
        || declared_label(input)
            .and_then(charset)
            .is_some_and(|charset| matches!(charset, Charset::Byte(_)))
}

/// Locates a leading declaration's label only to route it to the decoder;
/// the parser still validates the declaration grammar.
fn declared_label(input: &[u8]) -> Option<&str> {
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
