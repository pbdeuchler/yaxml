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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourceEncoding {
    Utf8,
    Utf16 { little_endian: bool, bom: bool },
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
            SourceEncoding::Utf16 { .. } => {
                let relative = error
                    .offset
                    .saturating_sub(self.decoded.start)
                    .min(raw.len());
                // Error locations originate in the shared UTF-8 grammar. A
                // defensive floor also handles a location inside a scalar.
                let prefix = &raw[..raw.floor_char_boundary(relative)];
                self.source.start + prefix.encode_utf16().count() * 2
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

/// The sole transport boundary. It retains no decoded input and no offset map.
pub(crate) struct Decoder {
    encoding: Option<SourceEncoding>,
    preamble: [u8; 4],
    preamble_len: usize,
    position: usize,
    decoded: usize,
    limit: usize,
    byte: Option<(u8, usize)>,
    high: Option<(u16, usize)>,
}

impl Decoder {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            encoding: None,
            preamble: [0; 4],
            preamble_len: 0,
            position: 0,
            decoded: 0,
            limit,
            byte: None,
            high: None,
        }
    }

    pub(crate) fn position(&self) -> usize {
        self.position
    }

    pub(crate) fn feed(
        &mut self,
        mut input: &[u8],
        mut emit: impl FnMut(Decoded<'_>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        while self.encoding.is_none() && !input.is_empty() {
            self.preamble[self.preamble_len] = input[0];
            self.preamble_len += 1;
            self.position += 1;
            input = &input[1..];
            if let Some(selection) = select(&self.preamble[..self.preamble_len], false) {
                self.start(selection?, &mut emit)?;
            }
        }
        if let Some(encoding) = self.encoding {
            self.convert(input, encoding, false, &mut emit)?;
        }
        Ok(())
    }

    pub(crate) fn finish(
        &mut self,
        mut emit: impl FnMut(Decoded<'_>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if self.encoding.is_none() {
            let selection = select(&self.preamble[..self.preamble_len], true)
                .expect("final encoding selection");
            self.start(selection?, &mut emit)?;
        }
        if let Some((_, offset)) = self.high {
            return Err(Error::new(ErrorKind::InvalidUtf16, offset));
        }
        if let Some((_, offset)) = self.byte {
            return Err(Error::new(ErrorKind::InvalidUtf16, offset));
        }
        Ok(())
    }

    fn start(
        &mut self,
        (encoding, bom): (SourceEncoding, usize),
        emit: &mut impl FnMut(Decoded<'_>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.encoding = Some(encoding);
        emit(Decoded::Start { encoding, bom })?;
        let preamble = self.preamble;
        self.convert(&preamble[bom..self.preamble_len], encoding, true, emit)
    }

    fn convert(
        &mut self,
        input: &[u8],
        encoding: SourceEncoding,
        already_consumed: bool,
        emit: &mut impl FnMut(Decoded<'_>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        match encoding {
            SourceEncoding::Utf8 => {
                let allowed = input.len().min(self.limit - self.decoded);
                if allowed > 0 {
                    emit(Decoded::Bytes(&input[..allowed]))?;
                    self.decoded += allowed;
                    if !already_consumed {
                        self.position += allowed;
                    }
                }
                if allowed < input.len() {
                    if !already_consumed {
                        self.position += 1;
                    }
                    return Err(Error::new(ErrorKind::LimitExceeded(Limit::DecodedBytes), 0));
                }
            }
            SourceEncoding::Utf16 { little_endian, .. } => {
                let start = if already_consumed {
                    self.position - input.len()
                } else {
                    self.position
                };
                for (index, &byte) in input.iter().enumerate() {
                    let offset = start + index;
                    if !already_consumed {
                        self.position += 1;
                    }
                    let Some((first, unit_offset)) = self.byte.take() else {
                        self.byte = Some((byte, offset));
                        continue;
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
                        continue;
                    } else if (0xdc00..=0xdfff).contains(&unit) {
                        return Err(Error::new(ErrorKind::InvalidUtf16, unit_offset));
                    } else {
                        u32::from(unit)
                    };
                    let character = char::from_u32(scalar).expect("validated UTF-16 scalar");
                    if character.len_utf8() > self.limit - self.decoded {
                        return Err(Error::new(ErrorKind::LimitExceeded(Limit::DecodedBytes), 0));
                    }
                    let mut buffer = [0; 4];
                    emit(Decoded::Bytes(
                        character.encode_utf8(&mut buffer).as_bytes(),
                    ))?;
                    self.decoded += character.len_utf8();
                }
            }
        }
        Ok(())
    }
}

const UTF32: [[u8; 4]; 8] = [
    [0, 0, 0xfe, 0xff],
    [0xff, 0xfe, 0, 0],
    [0, 0, 0xff, 0xfe],
    [0xfe, 0xff, 0, 0],
    [0, 0, 0, b'<'],
    [b'<', 0, 0, 0],
    [0, 0, b'<', 0],
    [0, b'<', 0, 0],
];

fn select(prefix: &[u8], final_input: bool) -> Option<Result<(SourceEncoding, usize), Error>> {
    if UTF32.iter().any(|candidate| prefix == candidate) {
        return Some(Err(Error::new(ErrorKind::UnsupportedEncoding, 0)));
    }
    if !final_input && UTF32.iter().any(|candidate| candidate.starts_with(prefix)) {
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
        || UTF32.iter().any(|signature| input.starts_with(signature))
}

#[cfg(test)]
#[path = "../tests/internal/encoding.rs"]
mod tests;
