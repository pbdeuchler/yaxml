use memchr::{memchr, memchr2, memchr3};

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

#[derive(Clone, Debug)]
enum ReadySpan {
    Input(TokenSpan),
    #[cfg(test)]
    Local(std::ops::Range<usize>),
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
    #[cfg(test)]
    local: bool,
    limit: usize,
    decoded_limit: usize,
    ready: Option<(TokenKind, ReadySpan)>,
    lookahead: Option<(usize, usize)>,
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
            #[cfg(test)]
            local: false,
            limit,
            decoded_limit,
            ready: None,
            lookahead: None,
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

    pub(crate) fn current(&self) -> Option<(TokenKind, &[u8], &TokenSpan)> {
        match self.ready.as_ref()? {
            (kind, ReadySpan::Input(span)) => Some((*kind, self.buffer.as_slice(), span)),
            #[cfg(test)]
            (_, ReadySpan::Local(_)) => None,
        }
    }

    fn local_mode(&self) -> bool {
        #[cfg(test)]
        {
            self.local
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    fn raw_length(&self) -> usize {
        if self.local_mode() {
            self.position - self.start
        } else {
            self.source_position - self.source_start
        }
    }

    fn diagnostic_start(&self) -> usize {
        if self.local_mode() {
            self.start
        } else {
            self.source_start
        }
    }

    pub(crate) fn release(&mut self) {
        if self.ready.take().is_some() {
            self.buffer.clear();
            self.scan = Scan::Text;
        }
    }

    /// Consume only through the next complete token. A text delimiter already
    /// counted in the positions is replayed from constant lookahead storage.
    pub(crate) fn advance(&mut self, input: &[u8]) -> Result<usize, Error> {
        self.advance_inner::<false>(input)
            .map(|(consumed, _)| consumed)
    }

    fn advance_inner<const REFERENCES: bool>(
        &mut self,
        input: &[u8],
    ) -> Result<(usize, bool), Error> {
        if self.ready.is_some() {
            return Ok((0, false));
        }
        if let Some((offset, source_offset)) = self.lookahead.take() {
            self.consume(b'<', offset, source_offset)?;
        }
        let mut cursor = 0;
        while cursor < input.len() {
            if REFERENCES && matches!(self.scan, Scan::Text) && input[cursor] == b'&' {
                return Ok((cursor, true));
            }
            if self.encoding == SourceEncoding::Utf8 {
                let length = self.plain_run::<REFERENCES>(&input[cursor..]);
                if length > 0 {
                    if self.buffer.is_empty() {
                        self.start = self.position;
                        self.source_start = self.source_position;
                    }
                    let available_raw = self.limit - self.raw_length();
                    let available_decoded = self.decoded_limit - self.buffer.len();
                    let available = available_raw.min(available_decoded);
                    if length > available {
                        // Consume through the first offending byte, as the
                        // scalar path does, without allocating a rejected run.
                        self.position += available + 1;
                        if !self.local_mode() {
                            self.source_position += available + 1;
                        }
                        return Err(Error::new(
                            ErrorKind::LimitExceeded(if available_raw <= available_decoded {
                                Limit::TokenBytes
                            } else {
                                Limit::DecodedTokenBytes
                            }),
                            self.diagnostic_start(),
                        ));
                    }
                    self.append_slice(&input[cursor..cursor + length])?;
                    self.position += length;
                    if !self.local_mode() {
                        self.source_position += length;
                    }
                    cursor += length;
                    continue;
                }
            }
            let byte = input[cursor];
            cursor += 1;
            let offset = self.position;
            let source_offset = self.source_position;
            self.position += 1;
            if !self.local_mode() {
                self.source_position += self.encoding.source_length(std::slice::from_ref(&byte));
            }
            self.consume(byte, offset, source_offset)?;
            if self.ready.is_some() {
                break;
            }
        }
        Ok((cursor, false))
    }

    /// Skip bytes that cannot change the current delimiter state. Each scan
    /// resumes at the next chunk; no accumulated token prefix is rescanned.
    fn plain_run<const REFERENCES: bool>(&self, input: &[u8]) -> usize {
        let boundary = match self.scan {
            Scan::Text if REFERENCES => memchr2(b'<', b'&', input),
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

    pub(crate) fn finish_step(&mut self) -> Result<(), Error> {
        self.advance(&[])?;
        if self.ready.is_some() {
            return Ok(());
        }
        match self.scan {
            Scan::Text if self.buffer.is_empty() => Ok(()),
            Scan::Text => {
                self.mark(TokenKind::Text, self.position);
                Ok(())
            }
            Scan::Prefix if self.buffer.starts_with(b"<!") => {
                self.mark(TokenKind::InvalidMarkup, self.position);
                Ok(())
            }
            _ => Err(Error::new(
                ErrorKind::UnexpectedEof,
                self.diagnostic_start(),
            )),
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
                self.diagnostic_start(),
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
                .map_err(|_| Error::new(ErrorKind::Allocation, self.diagnostic_start()))?;
        }
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    fn consume(&mut self, byte: u8, offset: usize, source_offset: usize) -> Result<(), Error> {
        if matches!(self.scan, Scan::Text) && byte == b'<' && !self.buffer.is_empty() {
            self.mark_at(TokenKind::Text, offset, source_offset);
            self.lookahead = Some((offset, source_offset));
            return Ok(());
        }
        if self.buffer.is_empty() {
            self.start = offset;
            self.source_start = source_offset;
        }
        if self.raw_length() > self.limit {
            return Err(Error::new(
                ErrorKind::LimitExceeded(Limit::TokenBytes),
                self.diagnostic_start(),
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
                    self.tag_byte(byte, offset);
                }
            }
            Scan::Prefix => {
                if self.buffer == b"<!--" {
                    self.scan = Scan::Comment { hyphens: 0 };
                } else if self.buffer == b"<![CDATA[" {
                    self.scan = Scan::Cdata { brackets: 0 };
                } else if self.buffer == b"<!DOCTYPE" {
                    self.mark(TokenKind::Dtd, offset + 1);
                } else if ![&b"<!--"[..], &b"<![CDATA["[..], &b"<!DOCTYPE"[..]]
                    .iter()
                    .any(|prefix| prefix.starts_with(&self.buffer))
                {
                    self.mark(TokenKind::InvalidMarkup, offset + 1);
                }
            }
            Scan::Tag { .. } => self.tag_byte(byte, offset),
            Scan::Comment { hyphens } => {
                if byte == b'>' && hyphens == 2 {
                    self.mark(TokenKind::Comment, offset + 1);
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
                    self.mark(TokenKind::Cdata, offset + 1);
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
                    self.mark(TokenKind::Instruction, offset + 1);
                } else {
                    self.scan = Scan::Instruction {
                        question: byte == b'?',
                    };
                }
            }
        }
        Ok(())
    }

    fn tag_byte(&mut self, byte: u8, offset: usize) {
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
                self.mark(kind, offset + 1);
            }
            _ => {}
        }
    }

    fn mark(&mut self, kind: TokenKind, end: usize) {
        self.mark_at(kind, end, self.source_position)
    }

    fn mark_at(&mut self, kind: TokenKind, end: usize, source_end: usize) {
        #[cfg(test)]
        if self.local {
            self.ready = Some((kind, ReadySpan::Local(self.start..end)));
            return;
        }
        self.ready = Some((
            kind,
            ReadySpan::Input(TokenSpan {
                decoded: self.start..end,
                source: self.source_start..source_end,
                encoding: self.encoding,
            }),
        ));
    }

    #[cfg(test)]
    fn feed(
        &mut self,
        mut input: &[u8],
        mut emit: impl FnMut(TokenKind, &[u8], TokenSpan) -> Result<(), Error>,
    ) -> Result<(), Error> {
        while !input.is_empty() {
            let consumed = self.advance(input)?;
            input = &input[consumed..];
            if let Some((kind, bytes, span)) = self.current() {
                emit(kind, bytes, span.clone())?;
                self.release();
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn finish(
        &mut self,
        mut emit: impl FnMut(TokenKind, &[u8], TokenSpan) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.finish_step()?;
        if let Some((kind, bytes, span)) = self.current() {
            emit(kind, bytes, span.clone())?;
            self.release();
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/internal/tokenizer.rs"]
mod tests;

#[cfg(test)]
pub(crate) mod local;

#[cfg(test)]
pub(crate) mod reference;
