//! Token scanners ported from libxml2 2.15.4 `HTMLparser.c`: htmlParseCharData,
//! htmlParseData, htmlParseHTMLName, htmlParseStartTag/EndTag, comments and
//! DOCTYPE, which follow the HTML5 tokenization rules since 2.14.
//!
//! The tokenizer is a resumable state machine. Its state is the content mode
//! (`endCheckState`), the raw-text end tag name and the prolog phase. Each
//! step scans one token from the start of the unconsumed text and returns it
//! with the bytes it used, or reports `Stop::More` without consuming anything.
//! Reading past the end at EOF yields 0, like libxml2's NUL-terminated
//! buffers, so a token's bytes and boundaries never depend on chunking.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::HashSet;

use super::Doctype;
use super::entities::ENTITIES;
use super::tables::{Mode, WINDOWS_1252};

pub(super) type Attribute = (String, Option<String>);

#[derive(Debug, PartialEq)]
pub(super) enum Token<'a> {
    /// One `characters`/`cdataBlock` call; `next` is the following byte (0 at EOF).
    Text {
        text: Cow<'a, str>,
        mode: Mode,
        next: u8,
    },
    /// A `<` or `</` that starts no markup.
    Literal(&'static str),
    StartTag {
        name: String,
        attributes: Vec<Attribute>,
        end: TagEnd,
    },
    EndTag(String),
    Comment(String),
    /// `first` is true only before any content and earlier DOCTYPE.
    Doctype {
        doctype: Doctype,
        first: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TagEnd {
    Open,
    SelfClosing,
    /// Input ended inside the tag; libxml2 discards it.
    Eof,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum Stop {
    /// The token continues past the available input.
    More,
    /// The non-ASCII byte at this offset needs an encoding decision first.
    Switch(usize),
    /// A value exceeded its length ceiling (libxml2's XML_ERR_RESOURCE_LIMIT).
    TooLong,
    /// A tag has more than `Limits::max_attributes` attributes.
    Attributes,
}

type Scan<T> = Result<T, Stop>;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Phase {
    Misc,
    Prolog,
    Content,
}

/// What ends an `htmlParseData` value.
#[derive(Clone, Copy)]
enum Until {
    Quote(u8),
    QuoteOrGt(u8),
    Gt,
    BlankOrGt,
    CommentEnd,
}

pub(super) struct Tokenizer {
    mode: Mode,
    end_name: String,
    phase: Phase,
    pub(super) has_encoding: bool,
    /// Set at the first invalid byte sequence, which libxml2 reports once.
    pub(super) invalid: Cell<bool>,
    value_limit: usize,
    name_limit: usize,
    max_attributes: usize,
}

pub(super) fn blank(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r')
}

/// `buf[i]`, or 0 past the end at EOF; before EOF the token is incomplete.
fn at(buf: &[u8], i: usize, eof: bool) -> Scan<u8> {
    match buf.get(i) {
        Some(&c) => Ok(c),
        None if eof => Ok(0),
        None => Err(Stop::More),
    }
}

fn blanks(buf: &[u8], mut i: usize) -> usize {
    while buf.get(i).copied().is_some_and(blank) {
        i += 1;
    }
    i
}

/// Scanned bytes are valid UTF-8 by construction.
fn string(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

/// `htmlValidateUtf8`: the length of a valid sequence at `buf[i]`, or None.
fn utf8(buf: &[u8], i: usize, eof: bool) -> Scan<Option<usize>> {
    let c = u32::from(buf[i]);
    let size = match c {
        0..0xC2 => return Ok(None),
        0xC2..0xE0 => 2,
        0xE0..0xF0 => 3,
        _ => 4,
    };
    let Some(s) = buf.get(i..i + size) else {
        return if eof { Ok(None) } else { Err(Stop::More) };
    };
    let valid = match size {
        2 => s[1] & 0xC0 == 0x80,
        3 => {
            let v = c << 16 | u32::from(s[1]) << 8 | u32::from(s[2]);
            v & 0x00C0C0 == 0x008080 && v & 0x0F2000 != 0 && v & 0x0F2000 != 0x0D2000
        }
        _ => {
            let v = u32::from_be_bytes([s[0], s[1], s[2], s[3]]);
            v & 0x00C0_C0C0 == 0x0080_8080 && (0xF090_0000..0xF490_0000).contains(&v)
        }
    };
    Ok(valid.then_some(size))
}

/// `htmlCodePointToUtf8`, with the C1 remapping of numeric references.
fn code_point(cp: u32) -> char {
    let cp = match cp {
        0x80..=0x9F => u32::from(WINDOWS_1252[(cp - 0x80) as usize]),
        0 => 0xFFFD,
        _ => cp,
    };
    char::from_u32(cp).unwrap_or('\u{FFFD}')
}

/// A character reference at `buf[0] == b'&'`: its replacement and length, or
/// None when the `&` is literal. Named references follow htmlFindEntityPrefix:
/// the longest name that ends at `;`, or at a legacy name's permitted end.
fn reference(buf: &[u8], eof: bool, attribute: bool) -> Scan<Option<(Cow<'static, str>, usize)>> {
    if at(buf, 1, eof)? == b'#' {
        let (radix, mut i) = match at(buf, 2, eof)? {
            c if c | 0x20 == b'x' && at(buf, 3, eof)?.is_ascii_hexdigit() => (16, 3),
            c if c.is_ascii_digit() => (10, 2),
            _ => return Ok(None),
        };
        let mut cp = 0u32;
        while let Some(digit) = char::from(at(buf, i, eof)?).to_digit(radix) {
            cp = (cp * radix + digit).min(0x11_0000);
            i += 1;
        }
        if buf.get(i) == Some(&b';') {
            i += 1;
        }
        return Ok(Some((Cow::Owned(code_point(cp).to_string()), i)));
    }
    let run = buf[1..]
        .iter()
        .take(32)
        .take_while(|c| c.is_ascii_alphanumeric())
        .count();
    if !eof && run < 32 && 1 + run == buf.len() {
        return Err(Stop::More);
    }
    let name = &buf[1..1 + run];
    if !name.first().is_some_and(u8::is_ascii_alphabetic) {
        return Ok(None);
    }
    for length in (2..=run.min(31)).rev() {
        let Ok(index) = ENTITIES.binary_search_by(|entry| entry.0.as_bytes().cmp(&name[..length]))
        else {
            continue;
        };
        let (_, legacy, value) = ENTITIES[index];
        let term = buf.get(1 + length).copied().unwrap_or(0);
        let ends =
            term == b';' || legacy && (!attribute || !term.is_ascii_alphanumeric() && term != b'=');
        if ends {
            return Ok(Some((
                Cow::Borrowed(value),
                1 + length + usize::from(term == b';'),
            )));
        }
    }
    Ok(None)
}

/// The end of a comment at `buf[i] == b'-'`: `-->`, `--!>`, or a dash run
/// cut off by EOF. Returns the bytes to skip.
fn comment_end(buf: &[u8], i: usize, eof: bool) -> Scan<Option<usize>> {
    let rest = &buf[i..];
    let short = |skip| if eof { Ok(Some(skip)) } else { Err(Stop::More) };
    match rest {
        [_] => short(1),
        [_, b'-'] => short(2),
        [_, b'-', b'>', ..] => Ok(Some(3)),
        [_, b'-', b'!'] => short(3),
        [_, b'-', b'!', b'>', ..] => Ok(Some(4)),
        _ => Ok(None),
    }
}

fn keyword(buf: &[u8], i: usize, word: &[u8], eof: bool) -> Scan<bool> {
    for (k, &expected) in word.iter().enumerate() {
        if at(buf, i + k, eof)?.to_ascii_uppercase() != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Keeps the first of each attribute name, as htmlParseStartTag does.
fn dedupe(attributes: &mut Vec<Attribute>) {
    if attributes.len() < 2 {
        return;
    }
    let mut seen = HashSet::with_capacity(attributes.len());
    let keep: Vec<bool> = attributes
        .iter()
        .map(|(name, _)| seen.insert(name.as_str()))
        .collect();
    let mut keep = keep.into_iter();
    attributes.retain(|_| keep.next().unwrap_or(true));
}

impl Tokenizer {
    pub(super) fn new(value_limit: usize, name_limit: usize, max_attributes: usize) -> Self {
        Self {
            mode: Mode::Data,
            end_name: String::new(),
            phase: Phase::Misc,
            has_encoding: false,
            invalid: Cell::new(false),
            value_limit,
            name_limit,
            max_attributes,
        }
    }

    /// `htmlParseHTMLName`: lowercased ASCII, NUL and invalid bytes as U+FFFD,
    /// at most 100 bytes kept. Attribute names also stop at `=` after the first.
    fn name(&self, buf: &[u8], mut i: usize, eof: bool, attribute: bool) -> Scan<(String, usize)> {
        const MAX: usize = 100;
        let mut out = Vec::new();
        let replacement = |out: &mut Vec<u8>| {
            if out.len() + 3 <= MAX {
                out.extend_from_slice("\u{FFFD}".as_bytes());
            }
        };
        loop {
            let Some(&c) = buf.get(i) else {
                if eof {
                    break;
                }
                return Err(Stop::More);
            };
            if !out.is_empty() && (c == b'/' || c == b'>' || attribute && c == b'=' || blank(c)) {
                break;
            }
            let mut size = 1;
            if c == 0 {
                replacement(&mut out);
            } else if c < 0x80 {
                if out.len() < MAX {
                    out.push(c.to_ascii_lowercase());
                }
            } else if let Some(length) = utf8(buf, i, eof)? {
                if out.len() + length <= MAX {
                    out.extend_from_slice(&buf[i..i + length]);
                }
                size = length;
            } else {
                self.invalid.set(true);
                replacement(&mut out);
            }
            i += size;
        }
        Ok((string(out), i))
    }

    /// Sets the content mode after a start tag; raw text ends at `</name`.
    pub(super) fn enter(&mut self, mode: Mode, name: &str) {
        self.mode = mode;
        if mode != Mode::Data {
            name.clone_into(&mut self.end_name);
        }
    }

    /// Scans one step and returns its token, if any, with the bytes it used;
    /// None once EOF leaves nothing. Errors consume nothing and change no state.
    pub(super) fn next<'a>(
        &mut self,
        buf: &'a [u8],
        eof: bool,
    ) -> Scan<Option<(Option<Token<'a>>, usize)>> {
        let start = if self.phase == Phase::Content {
            0
        } else {
            blanks(buf, 0)
        };
        let rest = &buf[start..];
        if rest.is_empty() {
            return if eof { Ok(None) } else { Err(Stop::More) };
        }
        let scanned = if self.mode == Mode::Data && rest[0] == b'<' {
            self.markup(rest, eof)
        } else {
            self.text(rest, eof)
        };
        let (token, used) = scanned.map_err(|stop| match stop {
            Stop::Switch(offset) => Stop::Switch(start + offset),
            stop => stop,
        })?;
        self.phase = match (&token, self.phase) {
            (Some(Token::Doctype { .. }), Phase::Misc) => Phase::Prolog,
            (Some(Token::Doctype { .. } | Token::Comment(_)), phase) => phase,
            _ => Phase::Content,
        };
        Ok(Some((token, start + used)))
    }

    fn markup<'a>(&mut self, buf: &'a [u8], eof: bool) -> Scan<(Option<Token<'a>>, usize)> {
        let (token, used) = match at(buf, 1, eof)? {
            b'/' => return self.end_tag(buf, eof),
            b'!' if keyword(buf, 2, b"DOCTYPE", eof)? => self.doctype(buf, eof)?,
            b'!' if at(buf, 2, eof)? == b'-' && at(buf, 3, eof)? == b'-' => {
                self.comment(buf, eof)?
            }
            b'!' => self.bogus_comment(buf, 2, eof)?,
            b'?' => self.bogus_comment(buf, 1, eof)?,
            c if c.is_ascii_alphabetic() => self.start_tag(buf, eof)?,
            _ => (Token::Literal("<"), 1),
        };
        Ok((Some(token), used))
    }

    /// A replacement piece starting at `buf[i]`: a reference, NUL, CR or an
    /// invalid byte. NUL in data and CR before LF are dropped (empty text).
    fn replacement(
        &self,
        buf: &[u8],
        i: usize,
        eof: bool,
        mode: Mode,
    ) -> Scan<Option<(Cow<'static, str>, usize)>> {
        Ok(match buf[i] {
            b'&' if matches!(mode, Mode::Data | Mode::Rcdata) => {
                return reference(&buf[i..], eof, false);
            }
            0 if mode == Mode::Data => Some((Cow::Borrowed(""), 1)),
            0 => Some((Cow::Borrowed("\u{FFFD}"), 1)),
            b'\r' if at(buf, i + 1, eof)? == b'\n' => Some((Cow::Borrowed(""), 1)),
            b'\r' => Some((Cow::Borrowed("\n"), 1)),
            0x80..=0xFF if !self.has_encoding => return Err(Stop::Switch(i)),
            0x80..=0xFF if utf8(buf, i, eof)?.is_none() => {
                self.invalid.set(true);
                Some((Cow::Borrowed("\u{FFFD}"), 1))
            }
            _ => None,
        })
    }

    /// One htmlParseCharData chunk: text up to the next delimiter, or the
    /// replacement piece at the start.
    fn text<'a>(&mut self, buf: &'a [u8], eof: bool) -> Scan<(Option<Token<'a>>, usize)> {
        let mut mode = self.mode;
        if let Some((text, used)) = self.replacement(buf, 0, eof, mode)? {
            let next = at(buf, used, eof)?;
            let token = (!text.is_empty()).then_some(Token::Text { text, mode, next });
            return Ok((token, used));
        }
        let mut i = 0;
        let mut complete = false;
        while i < buf.len() {
            let c = buf[i];
            let mut size = 1;
            match c {
                b'<' if mode == Mode::Data => break,
                b'<' if mode != Mode::Plaintext => match self.raw_lt(buf, i, eof, mode)? {
                    Some(next) => mode = next,
                    None => {
                        complete = true;
                        break;
                    }
                },
                b'-' if matches!(mode, Mode::ScriptEsc1 | Mode::ScriptEsc2) => {
                    let mut j = i + 1;
                    if buf.get(j) == Some(&b'-') {
                        j += 1;
                        if buf.get(j) == Some(&b'>') {
                            mode = Mode::Script;
                        }
                    }
                    if !eof && j >= buf.len() {
                        return Err(Stop::More);
                    }
                }
                0x80..=0xFF if !self.has_encoding => break,
                0x80..=0xFF => match utf8(buf, i, eof)? {
                    Some(length) => size = length,
                    None => break,
                },
                b'&' | 0 | b'\r' if self.replacement(buf, i, eof, mode)?.is_some() => break,
                _ => {}
            }
            i += size;
        }
        if i == buf.len() && !eof {
            return Err(Stop::More);
        }
        self.mode = if complete { Mode::Data } else { mode };
        if i == 0 {
            return Ok((None, 0));
        }
        let next = buf.get(i).copied().unwrap_or(0);
        let text = String::from_utf8_lossy(&buf[..i]);
        Ok((Some(Token::Text { text, mode, next }), i))
    }

    /// `<` inside raw text: script comment escapes, or the end tag (None).
    fn raw_lt(&self, buf: &[u8], i: usize, eof: bool, mut mode: Mode) -> Scan<Option<Mode>> {
        let mut j = i + 1;
        if j < buf.len() {
            if mode == Mode::Script && buf[j] == b'!' {
                j += 1;
                if buf.get(j) == Some(&b'-') {
                    j += 1;
                    if buf.get(j) == Some(&b'-') {
                        mode = Mode::ScriptEsc1;
                    }
                }
            } else {
                let solidus = buf[j] == b'/';
                j += usize::from(solidus);
                if solidus || mode == Mode::ScriptEsc1 {
                    let name = self.end_name.as_bytes();
                    let mut k = 0;
                    while j < buf.len() && k < name.len() && name[k] == buf[j] | 0x20 {
                        k += 1;
                        j += 1;
                    }
                    if k == name.len()
                        && let Some(&c) = buf.get(j)
                        && (c == b'>' || c == b'/' || blank(c))
                    {
                        match mode {
                            Mode::ScriptEsc1 if !solidus => mode = Mode::ScriptEsc2,
                            Mode::ScriptEsc2 => mode = Mode::ScriptEsc1,
                            _ => return Ok(None),
                        }
                    }
                }
            }
        }
        if !eof && j >= buf.len() {
            return Err(Stop::More);
        }
        Ok(Some(mode))
    }

    /// `htmlParseData` from `buf[i]`: the value and where scanning resumes.
    fn data(
        &self,
        buf: &[u8],
        mut i: usize,
        eof: bool,
        until: Until,
        refs: bool,
        limit: usize,
    ) -> Scan<(String, usize)> {
        let mut out = Vec::new();
        let mut chunk = i;
        let (end, resume) = loop {
            let Some(&c) = buf.get(i) else {
                if eof {
                    break (i, i);
                }
                return Err(Stop::More);
            };
            let stop = match until {
                Until::Quote(quote) => c == quote,
                Until::QuoteOrGt(quote) => c == quote || c == b'>',
                Until::Gt => c == b'>',
                Until::BlankOrGt => blank(c) || c == b'>',
                Until::CommentEnd => c == b'-',
            };
            if stop {
                if !matches!(until, Until::CommentEnd) {
                    break (i, i);
                }
                if let Some(skip) = comment_end(buf, i, eof)? {
                    break (i, i + skip);
                }
            }
            let piece = match c {
                b'&' if refs => reference(&buf[i..], eof, true)?,
                0 => Some((Cow::Borrowed("\u{FFFD}"), 1)),
                b'\r' if at(buf, i + 1, eof)? == b'\n' => Some((Cow::Borrowed(""), 1)),
                b'\r' => Some((Cow::Borrowed("\n"), 1)),
                0x80..=0xFF if !self.has_encoding => return Err(Stop::Switch(i)),
                0x80..=0xFF => match utf8(buf, i, eof)? {
                    Some(length) => {
                        i += length - 1;
                        None
                    }
                    None => {
                        self.invalid.set(true);
                        Some((Cow::Borrowed("\u{FFFD}"), 1))
                    }
                },
                _ => None,
            };
            match piece {
                Some((text, used)) => {
                    out.extend_from_slice(&buf[chunk..i]);
                    out.extend_from_slice(text.as_bytes());
                    i += used;
                    chunk = i;
                }
                None => i += 1,
            }
            if out.len() + (i - chunk) > limit {
                return Err(Stop::TooLong);
            }
        };
        out.extend_from_slice(&buf[chunk..end]);
        Ok((string(out), resume))
    }

    fn attribute_value(&self, buf: &[u8], i: usize, eof: bool) -> Scan<(String, usize)> {
        match at(buf, i, eof)? {
            quote @ (b'"' | b'\'') => {
                let (value, end) =
                    self.data(buf, i + 1, eof, Until::Quote(quote), true, self.value_limit)?;
                Ok((value, end + usize::from(buf.get(end) == Some(&quote))))
            }
            _ => self.data(buf, i, eof, Until::BlankOrGt, true, self.value_limit),
        }
    }

    /// Attributes up to `>`, `/>` or EOF, as htmlParseStartTag/EndTag read them.
    fn attributes(
        &self,
        buf: &[u8],
        mut i: usize,
        eof: bool,
    ) -> Scan<(Vec<Attribute>, TagEnd, usize)> {
        let mut attributes = Vec::new();
        i = blanks(buf, i);
        loop {
            let c = at(buf, i, eof)?;
            if i == buf.len() {
                return Ok((attributes, TagEnd::Eof, i));
            }
            if c == b'>' {
                return Ok((attributes, TagEnd::Open, i + 1));
            }
            if c == b'/' {
                if at(buf, i + 1, eof)? == b'>' {
                    return Ok((attributes, TagEnd::SelfClosing, i + 2));
                }
                i = blanks(buf, i + 1);
                continue;
            }
            let (attribute, end) = self.name(buf, i, eof, true)?;
            i = blanks(buf, end);
            let value = if at(buf, i, eof)? == b'=' {
                let (value, end) = self.attribute_value(buf, blanks(buf, i + 1), eof)?;
                i = end;
                Some(value)
            } else {
                None
            };
            if attributes.len() == self.max_attributes {
                return Err(Stop::Attributes);
            }
            attributes.push((attribute, value));
            i = blanks(buf, i);
        }
    }

    fn start_tag<'a>(&self, buf: &'a [u8], eof: bool) -> Scan<(Token<'a>, usize)> {
        let (name, i) = self.name(buf, 1, eof, false)?;
        let (mut attributes, end, used) = self.attributes(buf, i, eof)?;
        dedupe(&mut attributes);
        Ok((
            Token::StartTag {
                name,
                attributes,
                end,
            },
            used,
        ))
    }

    fn end_tag<'a>(&self, buf: &'a [u8], eof: bool) -> Scan<(Option<Token<'a>>, usize)> {
        match buf.get(2) {
            None if eof => Ok((Some(Token::Literal("</")), 2)),
            None => Err(Stop::More),
            Some(b'>') => Ok((None, 3)),
            Some(c) if !c.is_ascii_alphabetic() => {
                let (token, used) = self.bogus_comment(buf, 2, eof)?;
                Ok((Some(token), used))
            }
            Some(_) => {
                let (name, i) = self.name(buf, 2, eof, false)?;
                let (_, end, used) = self.attributes(buf, i, eof)?;
                Ok(((end != TagEnd::Eof).then_some(Token::EndTag(name)), used))
            }
        }
    }

    fn bogus_comment<'a>(&self, buf: &'a [u8], i: usize, eof: bool) -> Scan<(Token<'a>, usize)> {
        let (text, end) = self.data(buf, i, eof, Until::Gt, false, self.value_limit)?;
        Ok((
            Token::Comment(text),
            end + usize::from(buf.get(end) == Some(&b'>')),
        ))
    }

    /// After `<!--`: `<!-->` and `<!--->` are empty comments.
    fn comment<'a>(&self, buf: &'a [u8], eof: bool) -> Scan<(Token<'a>, usize)> {
        match at(buf, 4, eof)? {
            b'>' => Ok((Token::Comment(String::new()), 5)),
            b'-' if at(buf, 5, eof)? == b'>' => Ok((Token::Comment(String::new()), 6)),
            _ => {
                let (text, end) =
                    self.data(buf, 4, eof, Until::CommentEnd, false, self.value_limit)?;
                Ok((Token::Comment(text), end))
            }
        }
    }

    /// `htmlParseDoctypeLiteral`: a quoted string, or None without a quote.
    fn literal(&self, buf: &[u8], i: usize, eof: bool) -> Scan<Option<(String, usize)>> {
        let quote = at(buf, i, eof)?;
        if quote != b'"' && quote != b'\'' {
            return Ok(None);
        }
        let (value, end) = self.data(
            buf,
            i + 1,
            eof,
            Until::QuoteOrGt(quote),
            false,
            self.name_limit,
        )?;
        Ok(Some((
            value,
            end + usize::from(buf.get(end) == Some(&quote)),
        )))
    }

    /// `htmlParseDocTypeDecl` after `<!DOCTYPE`, then htmlSkipBogusDoctype.
    fn doctype<'a>(&self, buf: &'a [u8], eof: bool) -> Scan<(Token<'a>, usize)> {
        let mut doctype = Doctype::default();
        let mut i = blanks(buf, 9);
        if at(buf, i, eof)? != b'>' && i < buf.len() {
            let (name, end) = self.data(buf, i, eof, Until::BlankOrGt, false, self.name_limit)?;
            doctype.name = Some(name);
            i = blanks(buf, end);
        }
        if keyword(buf, i, b"PUBLIC", eof)? {
            if let Some((public, end)) = self.literal(buf, blanks(buf, i + 6), eof)? {
                doctype.public_id = Some(public);
                i = blanks(buf, end);
                if let Some((system, end)) = self.literal(buf, i, eof)? {
                    doctype.system_id = Some(system);
                    i = end;
                }
            } else {
                i += 6;
            }
        } else if keyword(buf, i, b"SYSTEM", eof)? {
            i = blanks(buf, i + 6);
            if let Some((system, end)) = self.literal(buf, i, eof)? {
                doctype.system_id = Some(system);
                i = end;
            }
        }
        let end = match buf[i.min(buf.len())..].iter().position(|&c| c == b'>') {
            Some(offset) => i + offset + 1,
            None if eof => buf.len(),
            None => return Err(Stop::More),
        };
        let first = self.phase == Phase::Misc;
        Ok((Token::Doctype { doctype, first }, end))
    }
}

#[cfg(test)]
#[path = "../../tests/internal/html_tokenizer.rs"]
mod tests;
