use memchr::{memchr, memchr3};

use crate::encoding::{SourceEncoding, TokenSpan};
use crate::{Error, ErrorKind, Limit};

/// Complete lexical units; semantic validation belongs to the shared parser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TokenKind {
    Start,
    End,
    Text,
    Comment,
    Cdata,
    Instruction,
    Dtd,
    InvalidMarkup,
}

#[derive(Clone, Copy, Debug)]
enum Scan {
    Text,
    Prefix,
    Tag { quote: Option<u8> },
    Comment { hyphens: u8 },
    Cdata { brackets: u8 },
    Instruction { question: bool },
}

/// Keeps one bounded decoded token. The scan
/// state advances with each byte, including across one-byte input chunks.
pub(crate) struct Tokenizer {
    buffer: Vec<u8>,
    start: usize,
    position: usize,
    scan: Scan,
    source_start: usize,
    source_position: usize,
    encoding: SourceEncoding,
    limit: usize,
    decoded_limit: usize,
}

impl Tokenizer {
    pub(crate) fn new(limit: usize, decoded_limit: usize) -> Self {
        Self {
            buffer: Vec::new(),
            start: 0,
            position: 0,
            scan: Scan::Text,
            source_start: 0,
            source_position: 0,
            encoding: SourceEncoding::Utf8,
            limit,
            decoded_limit,
        }
    }

    pub(crate) fn source_position(&self) -> usize {
        self.source_position
    }

    pub(crate) fn configure(&mut self, encoding: SourceEncoding, bom: usize) {
        self.encoding = encoding;
        self.source_start = bom;
        self.source_position = bom;
        self.position = if encoding == SourceEncoding::Utf8 {
            bom
        } else {
            0
        };
        self.start = self.position;
    }

    pub(crate) fn feed(
        &mut self,
        input: &[u8],
        mut emit: impl FnMut(TokenKind, &[u8], TokenSpan) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut cursor = 0;
        while cursor < input.len() {
            if self.encoding == SourceEncoding::Utf8 {
                let length = self.plain_run(&input[cursor..]);
                if length > 0 {
                    if self.buffer.is_empty() {
                        self.start = self.position;
                        self.source_start = self.source_position;
                    }
                    let available_raw = self.limit - (self.source_position - self.source_start);
                    let available_decoded = self.decoded_limit - self.buffer.len();
                    let available = available_raw.min(available_decoded);
                    if length > available {
                        // Consume through the first offending byte, as the
                        // scalar path does, without allocating a rejected run.
                        self.position += available + 1;
                        self.source_position += available + 1;
                        return Err(Error::new(
                            ErrorKind::LimitExceeded(if available_raw <= available_decoded {
                                Limit::TokenBytes
                            } else {
                                Limit::DecodedTokenBytes
                            }),
                            self.source_start,
                        ));
                    }
                    self.append_slice(&input[cursor..cursor + length])?;
                    self.position += length;
                    self.source_position += length;
                    cursor += length;
                    continue;
                }
            }
            let byte = input[cursor];
            cursor += 1;
            let offset = self.position;
            let source_offset = self.source_position;
            self.position += 1;
            self.source_position += self.encoding.source_length(std::slice::from_ref(&byte));
            self.consume(byte, offset, source_offset, &mut emit)?;
        }
        Ok(())
    }

    /// Skip bytes that cannot change the current delimiter state. Each scan
    /// resumes at the next chunk; no accumulated token prefix is rescanned.
    fn plain_run(&self, input: &[u8]) -> usize {
        let boundary = match self.scan {
            Scan::Text => memchr(b'<', input),
            Scan::Tag { quote: Some(quote) } => memchr(quote, input),
            Scan::Tag { quote: None } => memchr3(b'\'', b'"', b'>', input),
            Scan::Comment { hyphens: 0 } => memchr(b'-', input),
            Scan::Cdata { brackets: 0 } => memchr(b']', input),
            Scan::Instruction { question: false } => memchr(b'?', input),
            _ => return 0,
        };
        boundary.unwrap_or(input.len())
    }

    pub(crate) fn finish(
        &mut self,
        mut emit: impl FnMut(TokenKind, &[u8], TokenSpan) -> Result<(), Error>,
    ) -> Result<(), Error> {
        match self.scan {
            Scan::Text if self.buffer.is_empty() => Ok(()),
            Scan::Text => self.emit(TokenKind::Text, self.position, &mut emit),
            Scan::Prefix if self.buffer.starts_with(b"<!") => {
                self.emit(TokenKind::InvalidMarkup, self.position, &mut emit)
            }
            _ => Err(Error::new(ErrorKind::UnexpectedEof, self.source_start)),
        }
    }

    fn append(&mut self, byte: u8) -> Result<(), Error> {
        self.append_slice(std::slice::from_ref(&byte))
    }

    fn append_slice(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let Some(needed) = self
            .buffer
            .len()
            .checked_add(bytes.len())
            .filter(|n| *n <= self.decoded_limit)
        else {
            return Err(Error::new(
                ErrorKind::LimitExceeded(Limit::DecodedTokenBytes),
                self.source_start,
            ));
        };
        // Explicit geometric growth caps requested scratch capacity at the
        // token ceiling; reserving one byte exactly would be quadratic.
        if needed > self.buffer.capacity() {
            let target = self
                .buffer
                .capacity()
                .saturating_mul(2)
                .max(64)
                .max(needed)
                .min(match self.encoding {
                    SourceEncoding::Utf8 => self.limit,
                    // A UTF-16 code unit produces at most three UTF-8 bytes;
                    // surrogate pairs produce fewer bytes per source byte.
                    SourceEncoding::Utf16 { .. } => (self.limit / 2).saturating_mul(3),
                })
                .min(self.decoded_limit);
            self.buffer
                .try_reserve_exact(target - self.buffer.len())
                .map_err(|_| Error::new(ErrorKind::Allocation, self.source_start))?;
        }
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    fn consume(
        &mut self,
        byte: u8,
        offset: usize,
        source_offset: usize,
        emit: &mut impl FnMut(TokenKind, &[u8], TokenSpan) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if matches!(self.scan, Scan::Text) && byte == b'<' && !self.buffer.is_empty() {
            self.emit_at(TokenKind::Text, offset, source_offset, emit)?;
        }
        if self.buffer.is_empty() {
            self.start = offset;
            self.source_start = source_offset;
        }
        if self.source_position - self.source_start > self.limit {
            return Err(Error::new(
                ErrorKind::LimitExceeded(Limit::TokenBytes),
                self.source_start,
            ));
        }
        self.append(byte)?;
        match self.scan {
            Scan::Text if byte == b'<' => self.scan = Scan::Prefix,
            Scan::Text => {}
            Scan::Prefix if self.buffer.len() == 2 && byte != b'!' => {
                if byte == b'?' {
                    self.scan = Scan::Instruction { question: false };
                } else {
                    self.scan = Scan::Tag { quote: None };
                    self.tag_byte(byte, offset, emit)?;
                }
            }
            Scan::Prefix => {
                if self.buffer == b"<!--" {
                    self.scan = Scan::Comment { hyphens: 0 };
                } else if self.buffer == b"<![CDATA[" {
                    self.scan = Scan::Cdata { brackets: 0 };
                } else if self.buffer == b"<!DOCTYPE" {
                    self.emit(TokenKind::Dtd, offset + 1, emit)?;
                } else if ![&b"<!--"[..], &b"<![CDATA["[..], &b"<!DOCTYPE"[..]]
                    .iter()
                    .any(|prefix| prefix.starts_with(&self.buffer))
                {
                    self.emit(TokenKind::InvalidMarkup, offset + 1, emit)?;
                }
            }
            Scan::Tag { .. } => self.tag_byte(byte, offset, emit)?,
            Scan::Comment { hyphens } => {
                if byte == b'>' && hyphens == 2 {
                    self.emit(TokenKind::Comment, offset + 1, emit)?;
                } else {
                    self.scan = Scan::Comment {
                        hyphens: if byte == b'-' {
                            (hyphens + 1).min(2)
                        } else {
                            0
                        },
                    };
                }
            }
            Scan::Cdata { brackets } => {
                if byte == b'>' && brackets == 2 {
                    self.emit(TokenKind::Cdata, offset + 1, emit)?;
                } else {
                    self.scan = Scan::Cdata {
                        brackets: if byte == b']' {
                            (brackets + 1).min(2)
                        } else {
                            0
                        },
                    };
                }
            }
            Scan::Instruction { question } => {
                if byte == b'>' && question {
                    self.emit(TokenKind::Instruction, offset + 1, emit)?;
                } else {
                    self.scan = Scan::Instruction {
                        question: byte == b'?',
                    };
                }
            }
        }
        Ok(())
    }

    fn tag_byte(
        &mut self,
        byte: u8,
        offset: usize,
        emit: &mut impl FnMut(TokenKind, &[u8], TokenSpan) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let Scan::Tag { quote } = self.scan else {
            unreachable!("tag scanner state")
        };
        match (quote, byte) {
            (Some(q), b) if q == b => self.scan = Scan::Tag { quote: None },
            (None, b'\'' | b'"') => self.scan = Scan::Tag { quote: Some(byte) },
            (None, b'>') => {
                let kind = if self.buffer.starts_with(b"</") {
                    TokenKind::End
                } else {
                    TokenKind::Start
                };
                self.emit(kind, offset + 1, emit)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn emit(
        &mut self,
        kind: TokenKind,
        end: usize,
        emit: &mut impl FnMut(TokenKind, &[u8], TokenSpan) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.emit_at(kind, end, self.source_position, emit)
    }

    fn emit_at(
        &mut self,
        kind: TokenKind,
        end: usize,
        source_end: usize,
        emit: &mut impl FnMut(TokenKind, &[u8], TokenSpan) -> Result<(), Error>,
    ) -> Result<(), Error> {
        emit(
            kind,
            &self.buffer,
            TokenSpan {
                decoded: self.start..end,
                source: self.source_start..source_end,
                encoding: self.encoding,
            },
        )?;
        self.buffer.clear();
        self.scan = Scan::Text;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/internal/tokenizer.rs"]
mod tests;
