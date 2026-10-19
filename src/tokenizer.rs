use std::ops::ControlFlow;

use memchr::{memchr, memchr_iter, memchr2, memchr3};

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
    Tag {
        quote: Option<u8>,
    },
    /// A comment, CDATA or PI body: it ends at the first `>` preceded by
    /// enough `marker` bytes; `run` counts those already trailing.
    Delimited {
        kind: TokenKind,
        marker: u8,
        run: u8,
    },
}

#[derive(Clone, Debug)]
enum ReadySpan {
    Input(TokenSpan),
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

    pub(crate) fn scratch_capacity_requirement(&self) -> usize {
        let configured = match self.encoding {
            SourceEncoding::Utf8 => self.limit,
            SourceEncoding::Utf16 { .. } => (self.limit / 2).saturating_mul(3),
            SourceEncoding::Byte(_) => self.limit.saturating_mul(3),
        }
        .min(self.decoded_limit);
        configured.max(self.buffer.capacity())
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

    /// Continues current coordinates in a declaration-selected encoding.
    pub(crate) fn switch(&mut self, encoding: SourceEncoding) {
        self.encoding = encoding;
    }

    pub(crate) fn current(&self) -> Option<(TokenKind, &[u8], &TokenSpan)> {
        match self.ready.as_ref()? {
            (kind, ReadySpan::Input(span)) => Some((*kind, self.buffer.as_slice(), span)),
            (_, ReadySpan::Local(_)) => None,
        }
    }

    fn local_mode(&self) -> bool {
        self.local
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
        // Short inputs rarely hold a whole token; scanning them once is cheaper.
        if input.len() >= ADOPT_WINDOW
            && let Some(consumed) = self.adopt(input)?
        {
            return Ok(consumed);
        }
        self.advance_inner::<false>(input)
            .map(|(consumed, _)| consumed)
    }

    /// Copies a token that is complete in `input` in one step, with the
    /// same span, counters and lookahead the byte-wise scan would produce.
    fn adopt(&mut self, input: &[u8]) -> Result<Option<usize>, Error> {
        let Some((next, replay)) = self.idle_utf8() else {
            return Ok(None);
        };
        let found = if replay {
            markup_after_lt(input).map(|(kind, used)| (kind, used, used + 1))
        } else {
            complete_token(input).map(|(kind, used)| (kind, used, used))
        };
        let Some((kind, used, length)) = found.filter(|found| found.2 <= self.borrow_limit())
        else {
            return Ok(None);
        };
        let start = next - usize::from(replay);
        if replay {
            self.append(b'<')?;
        }
        self.append_slice(&input[..used])?;
        self.skip_borrowed(kind, start + length);
        self.start = start;
        self.source_start = start;
        self.mark_at(kind, start + length, start + length);
        Ok(Some(used + usize::from(kind == TokenKind::Text)))
    }

    fn advance_inner<const REFERENCES: bool>(
        &mut self,
        input: &[u8],
    ) -> Result<(usize, bool), Error> {
        if self.ready.is_some() {
            return Ok((0, false));
        }
        if let Some((offset, source_offset)) = self.lookahead.take() {
            // The `<` that ended a text token starts the next token.
            self.start = offset;
            self.source_start = source_offset;
            if self.raw_length() > self.limit {
                return Err(Error::new(
                    ErrorKind::LimitExceeded(Limit::TokenBytes),
                    self.diagnostic_start(),
                ));
            }
            self.append(b'<')?;
            self.scan = Scan::Prefix;
        }
        let mut cursor = 0;
        while cursor < input.len() {
            let rest = &input[cursor..];
            if REFERENCES && matches!(self.scan, Scan::Text) && rest[0] == b'&' {
                return Ok((cursor, true));
            }
            let (length, kind) = self.step::<REFERENCES>(rest);
            self.extend(&rest[..length])?;
            cursor += length;
            match kind {
                None => continue,
                // Text ends before its `<`, which is consumed as lookahead.
                Some(TokenKind::Text) => {
                    let (offset, source_offset) = (self.position, self.source_position);
                    self.position += 1;
                    if !self.local_mode() {
                        self.source_position += self.encoding.source_length(b"<");
                    }
                    self.mark_at(TokenKind::Text, offset, source_offset);
                    self.lookahead = Some((offset, source_offset));
                    cursor += 1;
                }
                Some(TokenKind::Start) if self.buffer.starts_with(b"</") => {
                    self.mark(TokenKind::End, self.position);
                }
                Some(kind) => self.mark(kind, self.position),
            }
            break;
        }
        Ok((cursor, false))
    }

    /// One lexical step over the front of `input`: the bytes that extend the
    /// current token and the kind of token they complete (`Start` for any
    /// tag). Delimiters are found with `memchr` from the carried state, so
    /// no accumulated token prefix is rescanned. Text ends before its `<`;
    /// with `REFERENCES` it also stops before `&`.
    fn step<const REFERENCES: bool>(&mut self, input: &[u8]) -> (usize, Option<TokenKind>) {
        match self.scan {
            Scan::Text => {
                let found = if REFERENCES {
                    memchr2(b'<', b'&', input)
                } else {
                    memchr(b'<', input)
                };
                match found {
                    None => (input.len(), None),
                    Some(index) if input[index] == b'&' => (index, None),
                    Some(index) if index > 0 || !self.buffer.is_empty() => {
                        (index, Some(TokenKind::Text))
                    }
                    Some(_) => {
                        self.scan = Scan::Prefix;
                        (1, None)
                    }
                }
            }
            Scan::Prefix => match self.prefix(input[0]) {
                ControlFlow::Continue(scan) => {
                    self.scan = scan;
                    (1, None)
                }
                ControlFlow::Break(kind) => (1, Some(kind)),
            },
            Scan::Tag { quote: Some(quote) } => match memchr(quote, input) {
                Some(index) => {
                    self.scan = Scan::Tag { quote: None };
                    (index + 1, None)
                }
                None => (input.len(), None),
            },
            Scan::Tag { quote: None } => match memchr3(b'\'', b'"', b'>', input) {
                Some(index) if input[index] == b'>' => (index + 1, Some(TokenKind::Start)),
                Some(index) => {
                    self.scan = Scan::Tag {
                        quote: Some(input[index]),
                    };
                    (index + 1, None)
                }
                None => (input.len(), None),
            },
            Scan::Delimited { kind, marker, run } => match delimited_end(input, marker, run) {
                Ok(end) => (end, Some(kind)),
                Err(run) => {
                    self.scan = Scan::Delimited { kind, marker, run };
                    (input.len(), None)
                }
            },
        }
    }

    /// Classifies markup by its buffered leading bytes and the next `byte`.
    /// Earlier bytes matched the family that byte 2 selects, so only the
    /// newest byte is compared.
    fn prefix(&self, byte: u8) -> ControlFlow<TokenKind, Scan> {
        let length = self.buffer.len() + 1;
        if length == 2 {
            return ControlFlow::Continue(match byte {
                b'!' => Scan::Prefix,
                b'?' => Scan::Delimited {
                    kind: TokenKind::Instruction,
                    marker: b'?',
                    run: 0,
                },
                b'>' => return ControlFlow::Break(TokenKind::Start),
                b'\'' | b'"' => Scan::Tag { quote: Some(byte) },
                _ => Scan::Tag { quote: None },
            });
        }
        let (family, kind, marker): (&[u8], _, _) = match self.buffer.get(2).unwrap_or(&byte) {
            b'-' => (b"<!--", TokenKind::Comment, b'-'),
            b'[' => (b"<![CDATA[", TokenKind::Cdata, b']'),
            b'D' => (b"<!DOCTYPE", TokenKind::Dtd, 0),
            _ => (b"", TokenKind::InvalidMarkup, 0),
        };
        if family.get(length - 1) != Some(&byte) {
            ControlFlow::Break(TokenKind::InvalidMarkup)
        } else if length < family.len() {
            ControlFlow::Continue(Scan::Prefix)
        } else if kind == TokenKind::Dtd {
            ControlFlow::Break(kind)
        } else {
            ControlFlow::Continue(Scan::Delimited {
                kind,
                marker,
                run: 0,
            })
        }
    }

    /// Appends token bytes, enforcing the source and decoded ceilings.
    #[inline(always)]
    fn extend(&mut self, run: &[u8]) -> Result<(), Error> {
        if run.is_empty() {
            return Ok(());
        }
        if self.buffer.is_empty() {
            self.start = self.position;
            self.source_start = self.source_position;
        }
        let raw = self.width(run);
        if raw > self.limit - self.raw_length()
            || run.len() > self.decoded_limit - self.buffer.len()
        {
            return Err(self.overflow(run));
        }
        // Single bytes (one-byte chunks) skip the copy call.
        if run.len() == 1 {
            self.append(run[0])?;
        } else {
            self.append_slice(run)?;
        }
        self.position += run.len();
        if !self.local_mode() {
            self.source_position += raw;
        }
        Ok(())
    }

    /// Raw length of decoded token bytes: source bytes, or decoded bytes
    /// for local replacement text.
    fn width(&self, bytes: &[u8]) -> usize {
        match self.encoding {
            _ if self.local_mode() => bytes.len(),
            SourceEncoding::Utf8 => bytes.len(),
            encoding => encoding.source_length(bytes),
        }
    }

    /// A run that would exceed a ceiling is consumed through its first
    /// offending byte, as a byte-wise scan would, without being allocated.
    #[cold]
    #[inline(never)]
    fn overflow(&mut self, run: &[u8]) -> Error {
        let available_raw = self.limit - self.raw_length();
        let available_decoded = self.decoded_limit - self.buffer.len();
        let mut used = 0;
        let (through, limit) = run
            .iter()
            .enumerate()
            .find_map(|(index, byte)| {
                used += self.width(std::slice::from_ref(byte));
                if used > available_raw {
                    Some((index + 1, Limit::TokenBytes))
                } else {
                    (index >= available_decoded).then_some((index + 1, Limit::DecodedTokenBytes))
                }
            })
            .expect("an offending byte");
        self.position += through;
        if !self.local_mode() {
            self.source_position += self.width(&run[..through]);
        }
        Error::new(ErrorKind::LimitExceeded(limit), self.diagnostic_start())
    }

    /// While no token is buffered in plain UTF-8 mode, the absolute offset of
    /// the next byte and whether a text-terminating `<` awaits replay.
    pub(crate) fn idle_utf8(&self) -> Option<(usize, bool)> {
        (self.ready.is_none()
            && self.buffer.is_empty()
            && matches!(self.scan, Scan::Text)
            && !self.local
            && self.encoding == SourceEncoding::Utf8
            && self.position == self.source_position)
            .then_some((self.position, self.lookahead.is_some()))
    }

    /// Longest token both byte ceilings accept without diagnosing it.
    pub(crate) fn borrow_limit(&self) -> usize {
        self.limit.min(self.decoded_limit)
    }

    /// Accounts for an adopted token ending at absolute `end`.
    /// Text also consumes its terminating `<` as replayed lookahead, exactly
    /// as the incremental scanner does.
    pub(crate) fn skip_borrowed(&mut self, kind: TokenKind, end: usize) {
        self.lookahead = (kind == TokenKind::Text).then_some((end, end));
        self.position = end + usize::from(kind == TokenKind::Text);
        self.source_position = self.position;
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
        if self.buffer.len() < self.buffer.capacity() && self.buffer.len() < self.decoded_limit {
            self.buffer.push(byte);
            return Ok(());
        }
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
                    // A byte-charset byte produces at most three UTF-8 bytes.
                    SourceEncoding::Byte(_) => self.limit.saturating_mul(3),
                })
                .min(self.decoded_limit);
            self.buffer
                .try_reserve_exact(target - self.buffer.len())
                .map_err(|_| Error::new(ErrorKind::Allocation, self.diagnostic_start()))?;
        }
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    fn mark(&mut self, kind: TokenKind, end: usize) {
        self.mark_at(kind, end, self.source_position)
    }

    fn mark_at(&mut self, kind: TokenKind, end: usize, source_end: usize) {
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

/// Kind and length of a complete UTF-8 token at the start of `input`, using
/// the incremental scanner's delimiters. `None` when more input could change
/// the token, or for prefixes it alone classifies (DOCTYPE, invalid markup).
pub(crate) fn complete_token(input: &[u8]) -> Option<(TokenKind, usize)> {
    match input.split_first()? {
        (b'<', rest) => markup_after_lt(rest).map(|(kind, length)| (kind, length + 1)),
        _ => memchr(b'<', input).map(|at| (TokenKind::Text, at)),
    }
}

/// Complete markup whose `<` immediately precedes `rest`; the length excludes it.
fn markup_after_lt(rest: &[u8]) -> Option<(TokenKind, usize)> {
    let delimited = |kind, prefix: usize, end: &[u8]| {
        find(&rest[prefix..], end).map(|at| (kind, prefix + at + end.len()))
    };
    match rest {
        [] => None,
        [b'!', b'-', b'-', ..] => delimited(TokenKind::Comment, 3, b"-->"),
        [b'!', b'[', b'C', b'D', b'A', b'T', b'A', b'[', ..] => {
            delimited(TokenKind::Cdata, 8, b"]]>")
        }
        [b'!', ..] => None,
        [b'?', ..] => delimited(TokenKind::Instruction, 1, b"?>"),
        [first, ..] => {
            let kind = if *first == b'/' {
                TokenKind::End
            } else {
                TokenKind::Start
            };
            tag_close(rest).map(|length| (kind, length))
        }
    }
}

/// Smallest input worth checking for a whole token to adopt in one copy.
const ADOPT_WINDOW: usize = 32;

/// First occurrence of a short delimiter such as `-->`. Its first byte is
/// located with `memchr`, avoiding a substring searcher's per-call setup,
/// which dominates on the small tokens of typical documents.
pub(crate) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    memchr_iter(needle[0], haystack).find(|at| haystack[*at..].starts_with(needle))
}

/// Length through the first `>` outside quoted attribute values.
pub(crate) fn tag_close(bytes: &[u8]) -> Option<usize> {
    let mut at = 0;
    loop {
        at += memchr3(b'\'', b'"', b'>', &bytes[at..])?;
        if bytes[at] == b'>' {
            return Some(at + 1);
        }
        let quote = bytes[at];
        at += 1;
        at += memchr(quote, &bytes[at..])? + 1;
    }
}

/// End of a comment, CDATA or PI body: the length through the first `>`
/// preceded by enough `marker` bytes (two, or one `?`), counting `run`
/// markers that already trail before `input`. Otherwise the trailing run.
fn delimited_end(input: &[u8], marker: u8, run: u8) -> Result<usize, u8> {
    let needed = if marker == b'?' { 1 } else { 2 };
    let trailing = |bytes: &[u8], carried: u8| {
        let count = bytes
            .iter()
            .rev()
            .take(needed)
            .take_while(|byte| **byte == marker)
            .count();
        let carried = if count == bytes.len() { carried } else { 0 };
        (count as u8 + carried).min(needed as u8)
    };
    let mut from = 0;
    // Markers before the first `>` may continue the carried run.
    let mut carried = run;
    while let Some(index) = memchr(b'>', &input[from..]) {
        let close = from + index;
        if usize::from(trailing(&input[from..close], carried)) == needed {
            return Ok(close + 1);
        }
        from = close + 1;
        carried = 0;
    }
    Err(trailing(&input[from..], carried))
}

#[cfg(test)]
#[path = "../tests/internal/tokenizer.rs"]
mod tests;

pub(crate) mod local;

pub(crate) mod reference;

pub(crate) mod text;
