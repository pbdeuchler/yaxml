use std::ops::Range;

use memchr::{memchr, memchr3};

use crate::{Error, ErrorKind, Limit};

/// Complete lexical units; semantic validation belongs to the shared parser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TokenKind {
    Bom,
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

/// Keeps one bounded token and a constant-size encoding preamble. The scan
/// state advances with each byte, including across one-byte input chunks.
pub(crate) struct Tokenizer {
    buffer: Vec<u8>,
    start: usize,
    position: usize,
    scan: Scan,
    preamble: [u8; 4],
    preamble_len: usize,
    checking_preamble: bool,
    limit: usize,
}

impl Tokenizer {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            buffer: Vec::new(),
            start: 0,
            position: 0,
            scan: Scan::Text,
            preamble: [0; 4],
            preamble_len: 0,
            checking_preamble: true,
            limit,
        }
    }

    pub(crate) fn position(&self) -> usize {
        self.position
    }

    pub(crate) fn feed(
        &mut self,
        input: &[u8],
        mut emit: impl FnMut(TokenKind, &[u8], Range<usize>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut cursor = 0;
        while cursor < input.len() {
            if !self.checking_preamble {
                let length = self.plain_run(&input[cursor..]);
                if length > 0 {
                    if self.buffer.is_empty() {
                        self.start = self.position;
                    }
                    let available = self.limit - self.buffer.len();
                    if length > available {
                        // Consume through the first offending byte, as the
                        // scalar path does, without allocating a rejected run.
                        self.position += available + 1;
                        return Err(Error::new(
                            ErrorKind::LimitExceeded(Limit::TokenBytes),
                            self.start,
                        ));
                    }
                    self.append_slice(&input[cursor..cursor + length])?;
                    self.position += length;
                    cursor += length;
                    continue;
                }
            }
            let byte = input[cursor];
            cursor += 1;
            let offset = self.position;
            self.position += 1;
            if self.checking_preamble {
                self.preamble[self.preamble_len] = byte;
                self.preamble_len += 1;
                let prefix = &self.preamble[..self.preamble_len];
                if prefix == [0xef, 0xbb, 0xbf] {
                    self.checking_preamble = false;
                    self.start = self.position;
                    emit(TokenKind::Bom, prefix, 0..self.position)?;
                } else if prefix == [0xff, 0xfe]
                    || prefix == [0xfe, 0xff]
                    || prefix == [0, 0, 0xfe, 0xff]
                    || prefix == [0, b'<', 0, b'?']
                {
                    return Err(Error::new(ErrorKind::UnsupportedEncoding, 0));
                } else if ![
                    &[0xef, 0xbb, 0xbf][..],
                    &[0xff, 0xfe][..],
                    &[0xfe, 0xff][..],
                    &[0, 0, 0xfe, 0xff][..],
                    &[0, b'<', 0, b'?'][..],
                ]
                .iter()
                .any(|candidate| candidate.starts_with(prefix))
                {
                    self.flush_preamble(&mut emit)?;
                }
            } else {
                self.consume(byte, offset, &mut emit)?;
            }
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
        mut emit: impl FnMut(TokenKind, &[u8], Range<usize>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if self.checking_preamble {
            self.flush_preamble(&mut emit)?;
        }
        match self.scan {
            Scan::Text if self.buffer.is_empty() => Ok(()),
            Scan::Text => self.emit(TokenKind::Text, self.position, &mut emit),
            Scan::Prefix if self.buffer.starts_with(b"<!") => {
                self.emit(TokenKind::InvalidMarkup, self.position, &mut emit)
            }
            _ => Err(Error::new(ErrorKind::UnexpectedEof, self.start)),
        }
    }

    fn flush_preamble(
        &mut self,
        emit: &mut impl FnMut(TokenKind, &[u8], Range<usize>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.checking_preamble = false;
        for offset in 0..self.preamble_len {
            self.consume(self.preamble[offset], offset, emit)?;
        }
        Ok(())
    }

    fn append(&mut self, byte: u8) -> Result<(), Error> {
        self.append_slice(std::slice::from_ref(&byte))
    }

    fn append_slice(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let Some(needed) = self
            .buffer
            .len()
            .checked_add(bytes.len())
            .filter(|n| *n <= self.limit)
        else {
            return Err(Error::new(
                ErrorKind::LimitExceeded(Limit::TokenBytes),
                self.start,
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
                .min(self.limit);
            self.buffer
                .try_reserve_exact(target - self.buffer.len())
                .map_err(|_| Error::new(ErrorKind::Allocation, self.start))?;
        }
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    fn consume(
        &mut self,
        byte: u8,
        offset: usize,
        emit: &mut impl FnMut(TokenKind, &[u8], Range<usize>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if matches!(self.scan, Scan::Text) && byte == b'<' && !self.buffer.is_empty() {
            self.emit(TokenKind::Text, offset, emit)?;
        }
        if self.buffer.is_empty() {
            self.start = offset;
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
        emit: &mut impl FnMut(TokenKind, &[u8], Range<usize>) -> Result<(), Error>,
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
        emit: &mut impl FnMut(TokenKind, &[u8], Range<usize>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        emit(kind, &self.buffer, self.start..end)?;
        self.buffer.clear();
        self.scan = Scan::Text;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/internal/tokenizer.rs"]
mod tests;
