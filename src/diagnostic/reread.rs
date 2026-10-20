//! libxml2's reading of the construct holding an error: a scan of the
//! markup skeleton to the error, then that construct re-read as libxml2's
//! parser reads it (`parser.c` error paths), giving libxml2's first error.

use std::borrow::Cow;

use super::LineIndex;
use super::code::*;
use crate::name::{name_char, name_start};
use crate::{ErrorKind, Limit};

// ---- the work budget ----

/// Bytes of input libxml2's amplification factor (5) lets a document expand
/// to, plus three passes over the input: the skeleton scan, construct
/// re-reads and declaration lookups.
const WORK_PER_BYTE: usize = 8;
/// Work every input may use, so small documents always get full context.
const WORK_FLOOR: usize = 64 * 1024;
/// The code of a description that ran out of work; never reported.
const EXHAUSTED: u16 = 0;

/// The re-reader's budget: bytes scanned, re-read and expanded (entity
/// replacement text, charged per expansion) for every error one call
/// describes. The first error to exhaust it, and every later one, gets
/// the context-free mapping instead.
pub(super) struct Work {
    used: std::cell::Cell<usize>,
    ceiling: usize,
}

impl Work {
    /// The budget for describing errors in `input_len` bytes of text.
    pub(super) fn new(input_len: usize) -> Self {
        let ceiling = WORK_PER_BYTE
            .saturating_mul(input_len)
            .saturating_add(WORK_FLOOR);
        Self {
            used: 0.into(),
            ceiling,
        }
    }

    /// Work used, and the ceiling it never exceeds.
    pub(super) fn spent(&self) -> (usize, usize) {
        (self.used.get(), self.ceiling)
    }

    /// Takes `bytes` of work, or fails without taking any when it is gone.
    pub(super) fn charge(&self, bytes: usize) -> Check {
        let used = self.used.get().saturating_add(bytes);
        if used > self.ceiling {
            return Err(fail(EXHAUSTED, 0, ""));
        }
        self.used.set(used);
        Ok(())
    }
}

// ---- the markup skeleton ----

/// The kind of construct holding an error position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Part {
    Text,
    StartTag,
    EndTag,
    Comment,
    Pi,
    Cdata,
    /// The DOCTYPE up to its subset, or the `]>` that closes the subset.
    Doctype,
    /// A markup declaration in the internal subset.
    Decl,
    /// Anything else in the internal subset.
    Subset,
}

/// A resumable forward scan of the markup skeleton.
#[derive(Clone, Default)]
pub(super) struct Scan {
    pos: usize,
    /// Start offsets of the open elements, outermost first.
    open: Vec<usize>,
    /// An element has started.
    rooted: bool,
    /// Inside the internal subset.
    subset: bool,
    /// The construct the last call stopped at, so a later error inside it
    /// does not search for its end again.
    held: Option<(Part, usize, Option<usize>)>,
    /// Content in a node's context: no prolog, epilog or DOCTYPE.
    content: bool,
}

/// The first of `stops` at or after `from` outside quotes.
pub(super) fn unquoted(t: &[u8], from: usize, stops: &[u8]) -> Option<usize> {
    let mut quote = None;
    let found = t.get(from..)?.iter().position(|&b| match quote {
        Some(q) => {
            quote = (b != q).then_some(q);
            false
        }
        None if matches!(b, b'"' | b'\'') => {
            quote = Some(b);
            false
        }
        None => stops.contains(&b),
    });
    found.map(|i| from + i)
}

/// The end of the first `pattern` at or after `from`.
fn after(t: &[u8], from: usize, pattern: &[u8]) -> Option<usize> {
    memchr::memmem::find(t.get(from..)?, pattern).map(|i| from + i + pattern.len())
}

const fn blank(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

/// The construct starting at `pos` and its end, when it is terminated.
fn part_at(t: &[u8], pos: usize, subset: bool) -> (Part, Option<usize>) {
    let r = &t[pos..];
    let past = |end: Option<usize>| end.map(|end| end + 1);
    match r {
        _ if r.starts_with(b"<?") => (Part::Pi, after(t, pos + 2, b"?>")),
        _ if r.starts_with(b"<!--") => (Part::Comment, after(t, pos + 4, b"-->")),
        _ if subset && r.starts_with(b"<!") => (Part::Decl, past(unquoted(t, pos + 2, b">"))),
        [b']', ..] if subset => (
            Part::Doctype,
            past(memchr::memchr(b'>', r).map(|i| pos + i)),
        ),
        [b, ..] if subset && blank(*b) => (Part::Text, Some(pos + 1)),
        [b'%', ..] if subset => (Part::Text, past(memchr::memchr(b';', r).map(|i| pos + i))),
        _ if subset => (Part::Subset, Some(pos + 1)),
        _ if r.starts_with(b"<![CDATA[") => (Part::Cdata, after(t, pos + 9, b"]]>")),
        _ if r.starts_with(b"<!DOCTYPE") => (Part::Doctype, past(unquoted(t, pos + 9, b"[>"))),
        _ if r.starts_with(b"</") => (Part::EndTag, past(memchr::memchr(b'>', r).map(|i| pos + i))),
        [b'<', ..] => (Part::StartTag, past(unquoted(t, pos + 1, b">"))),
        _ => (
            Part::Text,
            Some(memchr::memchr(b'<', r).map_or(t.len(), |i| pos + i)),
        ),
    }
}

impl Scan {
    pub(super) fn new(content: bool) -> Self {
        Self {
            content,
            ..Self::default()
        }
    }

    /// Moves past the terminated construct `part` from `pos` to `end`.
    fn advance(&mut self, t: &[u8], part: Part, pos: usize, end: usize) {
        match part {
            Part::StartTag => {
                self.rooted = true;
                if t[end - 2] != b'/' {
                    self.open.push(pos);
                }
            }
            Part::EndTag => {
                self.open.pop();
            }
            Part::Doctype if !self.content => self.subset = !self.subset && t[end - 1] == b'[',
            _ => {}
        }
        self.pos = end;
    }

    /// Advances to the construct holding text offset `at`: its part, start
    /// and end when terminated. Each byte searched is charged once.
    pub(super) fn to(
        &mut self,
        t: &[u8],
        at: usize,
        work: &Work,
    ) -> Result<(Part, usize, Option<usize>), Found> {
        while self.pos < t.len() {
            let pos = self.pos;
            let (part, end) = match self.held.take() {
                Some((part, held, end)) if held == pos => (part, end),
                _ => {
                    let (part, end) = part_at(t, pos, self.subset);
                    work.charge(end.unwrap_or(t.len()) - pos)?;
                    (part, end)
                }
            };
            match end {
                Some(end) if end <= at => self.advance(t, part, pos, end),
                _ => {
                    self.held = Some((part, pos, end));
                    return Ok((part, pos, end));
                }
            }
        }
        Ok((Part::Text, t.len(), None))
    }
}

// ---- libxml2's reading of one construct ----

/// libxml2's first error: code, position (text offset) and message.
pub(super) struct Found {
    pub(super) code: u16,
    pub(super) at: usize,
    /// Line and column in an entity's replacement text, when libxml2
    /// reports the error there rather than in the document.
    pub(super) place: Option<(usize, usize)>,
    pub(super) message: String,
    pub(super) str1: Option<String>,
    pub(super) str2: Option<String>,
    pub(super) int1: i64,
}

fn fail(code: u16, at: usize, message: impl Into<String>) -> Found {
    Found {
        code,
        at,
        place: None,
        message: message.into(),
        str1: None,
        str2: None,
        int1: 0,
    }
}

impl Found {
    fn strs(self, str1: &str, str2: Option<&str>) -> Self {
        Self {
            str1: Some(str1.into()),
            str2: str2.map(Into::into),
            ..self
        }
    }

    fn int(self, int1: i64) -> Self {
        Self { int1, ..self }
    }
}

type Check = Result<(), Found>;

/// `%.50s` of a byte range.
fn head(t: &[u8], from: usize, to: usize) -> Cow<'_, str> {
    String::from_utf8_lossy(&t[from..to.max(from).min(from + 50)])
}

/// libxml2's `IS_CHAR`.
const fn xml_char(c: u32) -> bool {
    matches!(c, 0x9 | 0xa | 0xd | 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff)
}

/// libxml2's parser cursor over the UTF-8 text.
pub(super) struct Cur<'t> {
    pub(super) t: &'t [u8],
    pub(super) i: usize,
    /// The lines of `t`, for messages that name a line.
    pub(super) lines: &'t LineIndex<'t>,
    /// The document, whose internal subset declares the entities.
    pub(super) doc: &'t LineIndex<'t>,
    pub(super) work: &'t Work,
}

impl<'t> Cur<'t> {
    fn b(&self) -> u8 {
        self.t.get(self.i).copied().unwrap_or(0)
    }

    fn nb(&self, k: usize) -> u8 {
        self.t.get(self.i + k).copied().unwrap_or(0)
    }

    fn has(&self, s: &[u8]) -> bool {
        self.t[self.i.min(self.t.len())..].starts_with(s)
    }

    fn eof(&self) -> bool {
        self.i >= self.t.len()
    }

    /// The character here and its length, with `\r\n` read as one `\n` as
    /// `xmlCurrentChar` does; `None` at the end or on invalid UTF-8.
    pub(super) fn ch(&self) -> Option<(char, usize)> {
        let rest = self.t.get(self.i..)?;
        if rest.starts_with(b"\r\n") {
            return Some(('\n', 2));
        }
        let len = match *rest.first()? {
            0..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            _ => 4,
        };
        let c = std::str::from_utf8(rest.get(..len)?).ok()?.chars().next()?;
        Some((c, len))
    }

    /// `SKIP_BLANKS`: whether any blank was skipped.
    fn blanks(&mut self) -> bool {
        let start = self.i;
        while blank(self.b()) && !self.eof() {
            self.i += 1;
        }
        self.i > start
    }

    fn run(
        &mut self,
        first: impl Fn(char) -> bool,
        rest: impl Fn(char) -> bool,
    ) -> Option<&'t str> {
        let start = self.i;
        while let Some((_, len)) = self
            .ch()
            .filter(|&(c, _)| if self.i == start { first(c) } else { rest(c) })
        {
            self.i += len;
        }
        (self.i > start).then(|| std::str::from_utf8(&self.t[start..self.i]).unwrap_or_default())
    }

    /// `xmlParseName` (colon) or `xmlParseNCName`.
    pub(super) fn name(&mut self, colon: bool) -> Option<&'t str> {
        self.run(|c| name_start(c, colon), |c| name_char(c, colon))
    }

    fn nmtoken(&mut self) -> Option<&'t str> {
        self.run(|c| name_char(c, true), |c| name_char(c, true))
    }

    fn line(&self, at: usize) -> usize {
        self.lines.text_line(at)
    }

    fn here(&self, code: u16, message: impl Into<String>) -> Found {
        fail(code, self.i, message)
    }

    fn fail<T>(&self, code: u16, message: impl Into<String>) -> Result<T, Found> {
        Err(self.here(code, message))
    }

    /// A cursor at `i` in the document `lines` indexes.
    pub(super) fn new(lines: &'t LineIndex<'t>, work: &'t Work, i: usize) -> Self {
        Self {
            t: lines.text(),
            i,
            lines,
            doc: lines,
            work,
        }
    }

    /// A cursor at the start of entity text `lines` indexes, in this
    /// document, charged for reading that text.
    fn inner(&self, lines: &'t LineIndex<'t>) -> Result<Self, Found> {
        self.work.charge(lines.text().len())?;
        Ok(Self {
            doc: self.doc,
            ..Self::new(lines, self.work, 0)
        })
    }
}

pub(super) fn local(qname: &str) -> &str {
    qname.rsplit_once(':').map_or(qname, |(_, local)| local)
}

/// `xmlParseQNameHashed`: the QName, `None` without one, or libxml2's
/// namespace error for a malformed one.
fn qname<'t>(c: &mut Cur<'t>) -> Result<Option<&'t str>, Found> {
    let start = c.i;
    let first = c.name(false);
    let second = first.is_some() && c.b() == b':' && {
        c.i += 1;
        c.name(false).is_some()
    };
    if first.is_some() && c.b() != b':' && (second || c.t[c.i - 1] != b':') {
        return Ok(Some(
            std::str::from_utf8(&c.t[start..c.i]).unwrap_or_default(),
        ));
    }
    if first.is_none() && c.b() != b':' {
        return Ok(None);
    }
    c.nmtoken();
    let name = String::from_utf8_lossy(&c.t[start..c.i]);
    Err(c
        .here(NS_QNAME, format!("Failed to parse QName '{name}'"))
        .strs(&name, None))
}

/// A reference at `at`: libxml2's syntax error in it, or the semantic error
/// `kind` names when the parser failed on this reference.
fn reference(c: &Cur<'_>, kind: ErrorKind, at: usize) -> Option<Found> {
    let mut r = Cur { i: c.i + 1, ..*c };
    if r.b() == b'#' {
        let hex = r.nb(1) == b'x';
        r.i += 1 + usize::from(hex);
        let mut value: u32 = 0;
        while r.b() != b';' {
            let digit = (r.b() as char).to_digit(if hex { 16 } else { 10 });
            let Some(digit) = digit.filter(|_| !r.eof()) else {
                return Some(if hex {
                    r.here(INVALID_HEX_CHARREF, "CharRef: invalid hexadecimal value")
                } else {
                    r.here(INVALID_DEC_CHARREF, "CharRef: invalid decimal value")
                });
            };
            value = (value * if hex { 16 } else { 10 } + digit).min(0x11_0000);
            r.i += 1;
        }
        r.i += 1;
        return if value >= 0x11_0000 {
            let message = "xmlParseCharRef: character reference out of bounds";
            Some(r.here(INVALID_CHAR, message).int(value.into()))
        } else {
            let message = format!("xmlParseCharRef: invalid xmlChar value {value}");
            (!xml_char(value)).then(|| r.here(INVALID_CHAR, message).int(value.into()))
        };
    }
    let Some(name) = r.name(true) else {
        return Some(r.here(NAME_REQUIRED, "xmlParseEntityRef: no name"));
    };
    if r.b() != b';' {
        return Some(r.here(ENTITYREF_SEMICOL_MISSING, "EntityRef: expecting ';'"));
    }
    r.i += 1;
    (c.i == at).then_some(())?;
    let found = match kind {
        ErrorKind::UnknownEntity => {
            r.here(UNDECLARED_ENTITY, format!("Entity '{name}' not defined"))
        }
        ErrorKind::UnparsedEntity => r.here(
            UNPARSED_ENTITY,
            format!("Entity reference to unparsed entity {name}"),
        ),
        ErrorKind::ExternalEntityInAttribute => r.here(
            ENTITY_IS_EXTERNAL,
            format!("Attribute references external entity '{name}'"),
        ),
        _ => return None,
    };
    Some(found.strs(name, None))
}

/// `xmlParseAttValueInternal`: the value with references replaced and
/// blanks normalized, expanding internal entities as libxml2 does.
fn att_value(c: &mut Cur<'_>, kind: ErrorKind, at: usize) -> Result<String, Found> {
    let quote = c.b();
    if !matches!(quote, b'"' | b'\'') {
        return c.fail(ATTRIBUTE_NOT_STARTED, "AttValue: \" or ' expected");
    }
    c.i += 1;
    let mut value = String::new();
    loop {
        match c.ch() {
            None if c.eof() => return c.fail(ATTRIBUTE_NOT_FINISHED, "AttValue: ' expected"),
            Some((_, 1)) if c.b() == quote => break,
            Some(('<', _)) => {
                return c.fail(
                    LT_IN_ATTRIBUTE,
                    "Unescaped '<' not allowed in attributes values",
                );
            }
            Some(('&', _)) => {
                if let Some(found) = reference(c, kind, at) {
                    return Err(found);
                }
                let amp = c.i;
                c.i = after(c.t, c.i, b";").unwrap_or(c.i + 1);
                let name = String::from_utf8_lossy(&c.t[amp + 1..c.i - 1]);
                value += &entity_text(c, &name, &mut Vec::new())?;
            }
            Some((ch, len)) if xml_char(ch.into()) => {
                value.push(if ch.is_ascii_whitespace() { ' ' } else { ch });
                c.i += len;
            }
            _ => return c.fail(INVALID_CHAR, "invalid character in attribute value"),
        }
    }
    c.i += 1;
    Ok(value)
}

/// What the internal subset declares an entity as.
enum Declared {
    No,
    External { unparsed: bool },
    Internal(String),
}

/// The internal subset's first declaration of entity `name` (a parameter
/// entity with `parameter`); internal entities with character references
/// replaced, as their replacement text has them.
fn declared(r: &Cur<'_>, name: &str, parameter: bool) -> Result<Declared, Found> {
    let (lines, t) = (r.doc, r.doc.text());
    let doctype = memchr::memmem::find(t, b"<!DOCTYPE");
    r.work.charge(doctype.unwrap_or(t.len()))?;
    let Some(mut pos) = doctype
        .and_then(|doctype| unquoted(t, doctype + 9, b"[>"))
        .filter(|&i| t[i] == b'[')
    else {
        return Ok(Declared::No);
    };
    pos += 1;
    while let (part, Some(end)) = part_at(t, pos, true) {
        r.work.charge(end - pos)?;
        if matches!(part, Part::Doctype | Part::Subset) {
            break;
        }
        let mut c = Cur::new(lines, r.work, pos + 8);
        if part == Part::Decl && t[pos..].starts_with(b"<!ENTITY") {
            c.blanks();
            let declares = c.b() == b'%';
            c.i += usize::from(declares);
            c.blanks();
            if declares == parameter && c.name(true) == Some(name) {
                c.blanks();
                let quote = c.b();
                return Ok(
                    match memchr::memchr(quote, &t[c.i + 1..end])
                        .filter(|_| matches!(quote, b'"' | b'\''))
                    {
                        Some(close) => Declared::Internal(expand(
                            &String::from_utf8_lossy(&t[c.i + 1..c.i + 1 + close]),
                            false,
                        )),
                        None => Declared::External {
                            unparsed: memchr::memmem::find(&t[c.i..end], b"NDATA").is_some(),
                        },
                    },
                );
            }
        }
        pos = end;
    }
    Ok(Declared::No)
}

/// Whether undeclared entities are only warnings: the document has an
/// external subset or parameter-entity references (`XML_WAR_UNDECLARED_ENTITY`).
fn lenient(r: &Cur<'_>) -> Result<bool, Found> {
    let t = r.doc.text();
    let doctype = memchr::memmem::find(t, b"<!DOCTYPE");
    r.work.charge(doctype.unwrap_or(t.len()))?;
    let Some(doctype) = doctype else {
        return Ok(false);
    };
    let end = after(t, doctype, b"]>")
        .or_else(|| unquoted(t, doctype + 9, b">"))
        .unwrap_or(t.len());
    r.work.charge(end - doctype)?;
    let head = unquoted(t, doctype + 9, b"[>").unwrap_or(end);
    let body = &t[doctype..end];
    Ok(memchr::memmem::find(&t[doctype..head], b"SYSTEM")
        .or(memchr::memmem::find(&t[doctype..head], b"PUBLIC"))
        .is_some()
        || memchr::memchr(b'%', body).is_some())
}

/// The text a reference contributes to an attribute value
/// (`xmlParseEntityRefInternal`, `xmlExpandEntityInAttValue`); errors are
/// libxml2's, at the reference's end.
fn entity_text(c: &Cur<'_>, name: &str, seen: &mut Vec<String>) -> Result<String, Found> {
    if let Some(number) = name.strip_prefix('#') {
        return Ok(expand(&format!("&#{number};"), false));
    }
    if let Some(c) = predefined(name) {
        return Ok(c.into());
    }
    let text = match declared(c, name, false)? {
        Declared::Internal(text) => text,
        Declared::No if lenient(c)? => return Ok(String::new()),
        Declared::No => {
            return Err(c
                .here(UNDECLARED_ENTITY, format!("Entity '{name}' not defined"))
                .strs(name, None));
        }
        Declared::External { unparsed: true } => {
            let message = format!("Entity reference to unparsed entity {name}");
            return Err(c.here(UNPARSED_ENTITY, message).strs(name, None));
        }
        Declared::External { .. } => {
            let message = format!("Attribute references external entity '{name}'");
            return Err(c.here(ENTITY_IS_EXTERNAL, message).strs(name, None));
        }
    };
    if seen.iter().any(|open| open == name) || seen.len() > 20 {
        return c.fail(ENTITY_LOOP, "Detected an entity reference loop");
    }
    if text.contains('<') {
        let message = format!("'<' in entity '{name}' is not allowed in attributes values");
        return Err(c.here(LT_IN_ATTRIBUTE, message).strs(name, None));
    }
    let inner = LineIndex::utf8(text.as_bytes());
    let mut r = c.inner(&inner)?;
    seen.push(name.to_owned());
    let mut out = String::new();
    let mut from = 0;
    while let Some(amp) = memchr::memchr(b'&', &r.t[from..]).map(|amp| from + amp) {
        out.push_str(&text[from..amp]);
        r.i = amp + 1;
        let character = r.b() == b'#';
        if !character && r.name(true).is_none() {
            return c.fail(NAME_REQUIRED, "xmlParseStringEntityRef: no name");
        }
        let end = memchr::memchr(b';', &r.t[r.i..]).map(|end| r.i + end);
        let Some(end) = end.filter(|_| character || r.b() == b';') else {
            return c.fail(ENTITYREF_SEMICOL_MISSING, "EntityRef: expecting ';'");
        };
        out += &entity_text(c, &text[amp + 1..end], seen)?;
        from = end + 1;
    }
    seen.pop();
    Ok(out + &text[from..])
}

/// An attribute of a start tag: qualified name and value.
struct Attr<'t> {
    name: &'t str,
    value: String,
}

/// The character a predefined entity stands for.
fn predefined(name: &str) -> Option<char> {
    let entities = [
        ("lt", '<'),
        ("gt", '>'),
        ("amp", '&'),
        ("apos", '\''),
        ("quot", '"'),
    ];
    entities
        .iter()
        .find(|(entity, _)| *entity == name)
        .map(|&(_, c)| c)
}

/// Character references (and with `predefined` the predefined entities)
/// replaced, as an attribute value or an entity literal reads them.
fn expand(raw: &str, predefined_too: bool) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let end = rest.find(';').map_or(rest.len(), |semicolon| semicolon + 1);
        let body = rest[1..end].trim_end_matches(';');
        let c = match body.strip_prefix('#') {
            Some(hex) if hex.starts_with('x') => u32::from_str_radix(&hex[1..], 16)
                .ok()
                .and_then(char::from_u32),
            Some(decimal) => decimal.parse().ok().and_then(char::from_u32),
            None if predefined_too => predefined(body),
            None => None,
        };
        match c {
            Some(c) if rest[..end].ends_with(';') => out.push(c),
            _ => out.push_str(&rest[..end]),
        }
        rest = &rest[end..];
    }
    out + rest
}

impl Attr<'_> {
    /// The prefix a namespace declaration binds (`""` for the default).
    fn declares(&self) -> Option<&str> {
        match self.name.split_once(':') {
            None if self.name == "xmlns" => Some(""),
            Some(("xmlns", prefix)) => Some(prefix),
            _ => None,
        }
    }
}

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

/// The namespace-declaration checks of `xmlParseStartTag2`.
fn declaration(c: &Cur<'_>, attr: &Attr<'_>, seen: &[Attr<'_>]) -> Check {
    let Some(prefix) = attr.declares() else {
        return Ok(());
    };
    let fault = |message: String| c.fail(NS_XML_NAMESPACE, message);
    let uri = attr.value.as_str();
    if seen.iter().any(|other| other.declares() == Some(prefix)) {
        let (p, l) = if prefix.is_empty() {
            (None, "xmlns")
        } else {
            (Some("xmlns"), prefix)
        };
        let message = format!("Attribute {} redefined", attr.name);
        return Err(c
            .here(ATTRIBUTE_REDEFINED, message)
            .strs(p.unwrap_or(l), p.map(|_| l)));
    }
    match prefix {
        "" if uri == XML_NS => fault("xml namespace URI cannot be the default namespace".into()),
        "" if uri == XMLNS_NS => fault("reuse of the xmlns namespace name is forbidden".into()),
        "" => Ok(()),
        "xml" if uri != XML_NS => fault("xml namespace prefix mapped to wrong URI".into()),
        "xml" => Ok(()),
        _ if uri == XML_NS => fault("xml namespace URI mapped to wrong prefix".into()),
        "xmlns" => fault("redefinition of the xmlns prefix is forbidden".into()),
        _ if uri == XMLNS_NS => fault("reuse of the xmlns namespace name is forbidden".into()),
        _ if uri.is_empty() => fault(format!(
            "xmlns:{prefix}: Empty XML namespace is not allowed"
        )),
        _ => Ok(()),
    }
}

/// `xmlParseStartTag2` and the end of `xmlParseElementStart` for the tag at
/// `start`; the attributes when the tag is well-formed.
fn start_tag<'t>(
    c: &mut Cur<'t>,
    open: &[usize],
    kind: ErrorKind,
    at: usize,
) -> Result<Vec<Attr<'t>>, Found> {
    let start = c.i;
    c.i += 1;
    let Some(name) = qname(c)? else {
        return c.fail(NAME_REQUIRED, "StartTag: invalid element name");
    };
    // The element's namespace error, at its name, follows its attributes'.
    let before_element = |found| match at == start + 1 {
        true => Ok(()),
        false => Err(found),
    };
    c.blanks();
    let mut attrs: Vec<Attr<'t>> = Vec::new();
    let ended = |c: &Cur<'_>| c.b() == b'>' || (c.b() == b'/' && c.nb(1) == b'>');
    while !ended(c) && (c.b() >= 0x20 || blank(c.b())) && !c.eof() {
        let Some(attr) = qname(c)? else {
            return c.fail(NAME_REQUIRED, "error parsing attribute name");
        };
        c.blanks();
        if c.b() != b'=' {
            let message = format!("Specification mandates value for attribute {}", local(attr));
            return Err(c
                .here(ATTRIBUTE_WITHOUT_VALUE, message)
                .strs(local(attr), None));
        }
        c.i += 1;
        c.blanks();
        let attr = Attr {
            name: attr,
            value: att_value(c, kind, at)?,
        };
        declaration(c, &attr, &attrs).or_else(before_element)?;
        attrs.push(attr);
        if ended(c) {
            break;
        }
        if !c.blanks() {
            return c.fail(SPACE_REQUIRED, "attributes construct error");
        }
    }
    // Declarations in scope, innermost last; reading the ancestors is charged.
    for &tag in open {
        c.work
            .charge(unquoted(c.t, tag + 1, b">").unwrap_or(c.t.len()) - tag)?;
    }
    let mut scope: Vec<Attr<'t>> = Vec::new();
    for &tag in open {
        scope.extend(attributes(&mut Cur { i: tag, ..*c })?);
    }
    scope.extend(attrs.iter().map(|a| Attr {
        name: a.name,
        value: a.value.clone(),
    }));
    let lookup = |prefix: &str| match prefix {
        "xml" => Some(XML_NS.to_owned()),
        "xmlns" => None,
        _ => scope
            .iter()
            .rev()
            .find(|a| a.declares() == Some(prefix))
            .map(|a| a.value.clone())
            .filter(|uri| !["", XML_NS, XMLNS_NS].contains(&uri.as_str())),
    };
    let element = local(name);
    let plain: Vec<_> = attrs
        .iter()
        .filter(|a| a.declares().is_none() && at != start + 1)
        .collect();
    let mut resolved = Vec::with_capacity(plain.len());
    for attr in &plain {
        let uri = match attr.name.split_once(':') {
            None => None,
            Some((prefix, local)) => {
                let uri = lookup(prefix);
                if uri.is_none() {
                    let message = format!(
                        "Namespace prefix {prefix} for {local} on {element} is not defined"
                    );
                    return Err(c
                        .here(NS_UNDEFINED_NAMESPACE, message)
                        .strs(prefix, Some(local)));
                }
                uri
            }
        };
        resolved.push(uri);
    }
    for (i, attr) in plain.iter().enumerate() {
        let twin = (0..i)
            .find(|&j| local(plain[j].name) == local(attr.name) && resolved[j] == resolved[i]);
        let Some(j) = twin else { continue };
        let (prefix, name) = attr
            .name
            .split_once(':')
            .map_or((None, attr.name), |(p, l)| (Some(p), l));
        return Err(if plain[j].name == attr.name {
            let message = format!("Attribute {} redefined", attr.name);
            c.here(ATTRIBUTE_REDEFINED, message)
                .strs(prefix.unwrap_or(name), prefix.map(|_| name))
        } else {
            let uri = resolved[i].clone().unwrap_or_default();
            let message = format!("Namespaced Attribute {name} in '{uri}' redefined");
            c.here(NS_ATTRIBUTE_REDEFINED, message)
                .strs(name, Some(&uri))
        });
    }
    if let Some((prefix, _)) = name.split_once(':').filter(|(p, _)| lookup(p).is_none()) {
        let message = format!("Namespace prefix {prefix} on {element} is not defined");
        return Err(c
            .here(NS_UNDEFINED_NAMESPACE, message)
            .strs(prefix, Some(element)));
    }
    if !ended(c) {
        let line = c.line(start);
        let message = format!("Couldn't find end of Start Tag {element} line {line}");
        return Err(c
            .here(GT_REQUIRED, message)
            .strs(element, None)
            .int(line as i64));
    }
    c.i += if c.b() == b'>' { 1 } else { 2 };
    Ok(attrs)
}

/// The attributes of the well-formed start tag at `c`.
/// Only running out of work is an error here.
fn attributes<'t>(c: &mut Cur<'t>) -> Result<Vec<Attr<'t>>, Found> {
    c.i += 1;
    c.name(true);
    let mut attrs = Vec::new();
    loop {
        c.blanks();
        let Some(name) = c.name(true) else {
            return Ok(attrs);
        };
        c.blanks();
        c.i += usize::from(c.b() == b'=');
        c.blanks();
        match att_value(c, ErrorKind::Syntax, usize::MAX) {
            Ok(value) => attrs.push(Attr { name, value }),
            Err(found) if found.code == EXHAUSTED => return Err(found),
            Err(_) => return Ok(attrs),
        }
    }
}

/// `xmlParseEndTag2` for the end tag at `c` closing the element at `open`.
fn end_tag(c: &mut Cur<'_>, open: usize) -> Check {
    let opened = {
        let mut o = Cur { i: open + 1, ..*c };
        o.name(true).unwrap_or_default()
    };
    c.i += 2;
    let name = c.name(true);
    c.blanks();
    if c.b() != b'>' {
        return c.fail(GT_REQUIRED, "expected '>'");
    }
    c.i += 1;
    if name == Some(opened) {
        return Ok(());
    }
    let (opened, name, line) = (
        local(opened),
        name.map_or("unparsable", local),
        c.line(open),
    );
    let message = format!("Opening and ending tag mismatch: {opened} line {line} and {name}");
    Err(c
        .here(TAG_NAME_MISMATCH, message)
        .strs(opened, Some(name))
        .int(line as i64))
}

/// `xmlParseComment` with its ASCII fast path and `xmlParseCommentComplex`.
fn comment(c: &mut Cur<'_>) -> Check {
    c.i += 4;
    let start = c.i;
    let mut fast = true;
    loop {
        if c.has(b"--") {
            if c.nb(2) == b'>' {
                c.i += 3;
                return Ok(());
            }
            return Err(match (fast, c.i == start) {
                (true, true) => c.here(HYPHEN_IN_COMMENT, "Double hyphen within comment"),
                (true, false) => {
                    let message = format!(
                        "Double hyphen within comment: <!--{}",
                        head(c.t, start, c.i)
                    );
                    c.here(HYPHEN_IN_COMMENT, message)
                }
                (false, _) => fail(
                    HYPHEN_IN_COMMENT,
                    c.i + 2,
                    "Comment must not contain '--' (double-hyphen)",
                ),
            });
        }
        match c.ch() {
            None if c.eof() && fast => {
                return c.fail(COMMENT_NOT_FINISHED, "Comment not terminated");
            }
            None if c.eof() => {
                let message = format!(
                    "Comment not terminated \n<!--{}",
                    head(c.t, start, c.i.saturating_sub(2))
                );
                return c.fail(COMMENT_NOT_FINISHED, message);
            }
            Some(('\0', _)) => return c.fail(INVALID_CHAR, NUL),
            Some((ch, len)) if xml_char(ch.into()) => {
                let ascii = matches!(c.b(), b'\t' | b'\n' | 0x20..=0x7f) || c.has(b"\r\n");
                fast &= ascii;
                c.i += len;
            }
            Some((ch, _)) => {
                let message = format!("xmlParseComment: invalid xmlChar value {}", ch as u32);
                return Err(c.here(INVALID_CHAR, message).int(ch as i64));
            }
            None => return c.fail(INVALID_ENCODING, "Invalid bytes in character encoding"),
        }
    }
}

/// `xmlParsePI` with `xmlParsePITarget`.
fn pi(c: &mut Cur<'_>) -> Check {
    c.i += 2;
    let Some(target) = c.name(true) else {
        return c.fail(PI_NOT_STARTED, "xmlParsePI : no target name");
    };
    if target
        .as_bytes()
        .get(..3)
        .is_some_and(|head| head.eq_ignore_ascii_case(b"xml"))
    {
        if target == "xml" {
            let message = "XML declaration allowed only at the start of the document";
            return c.fail(RESERVED_XML_NAME, message);
        } else if target.len() == 3 {
            return c.fail(RESERVED_XML_NAME, "Invalid PI name");
        }
    }
    if target.contains(':') {
        let message = format!("colons are forbidden from PI names '{target}'");
        return Err(c.here(NS_COLON, message).strs(target, None));
    }
    if c.has(b"?>") {
        c.i += 2;
        return Ok(());
    }
    if !c.blanks() {
        let message = format!("ParsePI: PI {target} space expected");
        return Err(c.here(SPACE_REQUIRED, message).strs(target, None));
    }
    loop {
        if c.has(b"?>") {
            c.i += 2;
            return Ok(());
        }
        match c.ch() {
            Some(('\0', _)) => return c.fail(INVALID_CHAR, NUL),
            Some((ch, len)) if xml_char(ch.into()) => c.i += len,
            _ => {
                let message = format!("ParsePI: PI {target} never end ...");
                return Err(c.here(PI_NOT_FINISHED, message).strs(target, None));
            }
        }
    }
}

/// `xmlParseCDSect`.
fn cdata(c: &mut Cur<'_>) -> Check {
    c.i += 9;
    let start = c.i;
    // Offsets of the last two characters read, as libxml2's `r` and `s`.
    let (mut last, mut read) = ([start; 2], 0);
    loop {
        if c.has(b"]]>") {
            c.i += 3;
            return Ok(());
        }
        match c.ch() {
            Some(('\0', _)) => return c.fail(INVALID_CHAR, NUL),
            Some((ch, len)) if xml_char(ch.into()) => {
                last = [last[1], c.i];
                c.i += len;
                read += 1;
            }
            // libxml2 has no message for this code.
            _ if read < 2 => return c.fail(CDATA_NOT_FINISHED, "Unregistered error message"),
            _ => {
                let message = format!("CData section not finished\n{}", head(c.t, start, last[0]));
                return c.fail(CDATA_NOT_FINISHED, message);
            }
        }
    }
}

/// `xmlCurrentChar`'s report of a NUL byte; its info string ends in a newline.
const NUL: &str = "Invalid character: Char 0x0 out of allowed range\n";

/// A quoted value after `keyword`, `=` and blanks, read by `value`: its
/// range, or `None` without the keyword.
fn quoted(
    c: &mut Cur<'_>,
    keyword: &[u8],
    value: fn(&mut Cur<'_>) -> Check,
) -> Result<Option<(usize, usize)>, Found> {
    c.blanks();
    if !c.has(keyword) {
        return Ok(None);
    }
    c.i += keyword.len();
    c.blanks();
    if c.b() != b'=' {
        return c.fail(EQUAL_REQUIRED, "expected '='");
    }
    c.i += 1;
    c.blanks();
    let quote = c.b();
    if !matches!(quote, b'"' | b'\'') {
        return c.fail(STRING_NOT_STARTED, "String not started expecting ' or \"");
    }
    c.i += 1;
    let start = c.i;
    value(c)?;
    if c.b() != quote {
        return c.fail(STRING_NOT_CLOSED, "String not closed expecting \" or '");
    }
    c.i += 1;
    Ok(Some((start, c.i - 1)))
}

/// `xmlParseXMLDecl` with its version, encoding and standalone parts.
fn xml_decl(c: &mut Cur<'_>) -> Check {
    c.i += 5;
    if !blank(c.b()) {
        return c.fail(SPACE_REQUIRED, "Blank needed after '<?xml'");
    }
    let version = quoted(c, b"version", |c| {
        if c.b().is_ascii_digit() && c.nb(1) == b'.' {
            c.i += 2;
            while c.b().is_ascii_digit() {
                c.i += 1;
            }
        }
        Ok(())
    })?;
    let version = version.map(|(from, to)| String::from_utf8_lossy(&c.t[from..to]));
    match version.as_deref() {
        None | Some("") => {
            return c.fail(VERSION_MISSING, "Malformed declaration expecting version");
        }
        Some(v) if !v.starts_with("1.") => {
            return Err(c
                .here(UNKNOWN_VERSION, format!("Unsupported version '{v}'"))
                .strs(v, None));
        }
        _ => {}
    }
    // Whether a blank separates another part; `?>` ends the declaration.
    let more = |c: &Cur<'_>| match () {
        _ if blank(c.b()) => Ok(true),
        _ if c.has(b"?>") => Ok(false),
        _ => c.fail(SPACE_REQUIRED, "Blank needed here"),
    };
    if !more(c)? {
        return Ok(());
    }
    let encoding = quoted(c, b"encoding", |c| {
        if !c.b().is_ascii_alphabetic() {
            return c.fail(ENCODING_NAME, "Invalid XML encoding name");
        }
        while c.b().is_ascii_alphanumeric() || matches!(c.b(), b'.' | b'_' | b'-') {
            c.i += 1;
        }
        Ok(())
    })?;
    if encoding.is_some() && !more(c)? {
        return Ok(());
    }
    quoted(c, b"standalone", |c| {
        let length = [b"no".as_slice(), b"yes"]
            .into_iter()
            .find(|v| c.has(v))
            .map(<[u8]>::len);
        c.i += length
            .ok_or_else(|| c.here(STANDALONE_VALUE, "standalone accepts only 'yes' or 'no'"))?;
        Ok(())
    })?;
    c.blanks();
    if !c.has(b"?>") {
        return c.fail(
            XMLDECL_NOT_FINISHED,
            "parsing XML declaration: '?>' expected",
        );
    }
    c.i += 2;
    Ok(())
}

// ---- the internal subset ----

/// `xmlParseSystemLiteral`, or `xmlParsePubidLiteral` with `pubid`.
fn literal(c: &mut Cur<'_>, pubid: bool) -> Check {
    let quote = c.b();
    if !matches!(quote, b'"' | b'\'') {
        return c.fail(LITERAL_NOT_STARTED, "SystemLiteral \" or ' expected");
    }
    c.i += 1;
    let allowed = |ch: char| {
        if pubid {
            ch.is_ascii_alphanumeric() || " \r\n-'()+,./:=?;!*#@$_%".contains(ch)
        } else {
            xml_char(ch.into())
        }
    };
    while c.b() != quote || c.eof() {
        match c.ch() {
            Some((ch, len)) if allowed(ch) => c.i += len,
            _ => {
                return c.fail(
                    LITERAL_NOT_FINISHED,
                    "Unfinished System or Public ID \" or ' expected",
                );
            }
        }
    }
    c.i += 1;
    Ok(())
}

/// `xmlParseExternalID`: whether one was there.
fn external_id(c: &mut Cur<'_>, strict: bool) -> Result<bool, Found> {
    let system = c.has(b"SYSTEM");
    if !system && !c.has(b"PUBLIC") {
        return Ok(false);
    }
    c.i += 6;
    if !c.blanks() {
        let keyword = if system { "SYSTEM" } else { "PUBLIC" };
        return Err(c.here(SPACE_REQUIRED, format!("Space required after '{keyword}'")));
    }
    if !system {
        literal(c, true)?;
        let spaced = c.blanks();
        if strict && !spaced {
            return c.fail(SPACE_REQUIRED, "Space required after the Public Identifier");
        }
        if !(strict || spaced && matches!(c.b(), b'"' | b'\'')) {
            return Ok(true);
        }
    }
    literal(c, false)?;
    Ok(true)
}

/// A required blank, or libxml2's "Space required" error.
fn space(c: &mut Cur<'_>, after: &str) -> Check {
    if c.blanks() {
        Ok(())
    } else {
        Err(c.here(SPACE_REQUIRED, format!("Space required after {after}")))
    }
}

/// `xmlParseEntityValue` and the checks of `xmlExpandPEsInEntityValue`.
fn entity_value(c: &mut Cur<'_>) -> Check {
    let quote = c.b();
    c.i += 1;
    let start = c.i;
    while c.b() != quote || c.eof() {
        match c.b() {
            _ if c.eof() => return c.fail(ENTITY_NOT_FINISHED, "(null)"),
            0 => return c.fail(INVALID_CHAR, "invalid character in entity value"),
            _ => c.i += 1,
        }
    }
    // The value is checked once read, at the closing quote.
    let mut v = Cur { i: start, ..*c };
    while v.i < c.i {
        match v.ch() {
            Some(('%', _)) => {
                v.i += 1;
                return Err(match v.name(true) {
                    None => c.here(NAME_REQUIRED, "xmlParseStringPEReference: no name"),
                    Some(_) if v.b() != b';' => {
                        c.here(ENTITYREF_SEMICOL_MISSING, "EntityRef: expecting ';'")
                    }
                    Some(_) => c.here(
                        ENTITY_PE_INTERNAL,
                        "PEReferences forbidden in internal subset",
                    ),
                });
            }
            Some(('&', _)) if v.nb(1) == b'#' => {
                if let Some(found) = reference(&v, ErrorKind::Syntax, usize::MAX) {
                    let message = found
                        .message
                        .replace("xmlParseCharRef", "xmlParseStringCharRef");
                    return Err(Found {
                        at: c.i,
                        message,
                        ..found
                    });
                }
                v.i = after(v.t, v.i, b";").unwrap_or(c.i);
            }
            Some(('&', _)) => {
                v.i += 1;
                if v.name(true).is_none() || v.b() != b';' {
                    let message = "EntityValue: '&' forbidden except for entities references";
                    return c.fail(ENTITY_CHAR_ERROR, message);
                }
                v.i += 1;
            }
            Some((ch, len)) if xml_char(ch.into()) => v.i += len,
            _ => return c.fail(INVALID_CHAR, "invalid character in entity value"),
        }
    }
    c.i += 1;
    Ok(())
}

/// `xmlParseEntityDecl`.
fn entity_decl(c: &mut Cur<'_>) -> Check {
    c.i += 8;
    space(c, "'<!ENTITY'")?;
    let parameter = c.b() == b'%';
    if parameter {
        c.i += 1;
        space(c, "'%%'")?;
    }
    let Some(name) = c.name(true) else {
        return c.fail(NAME_REQUIRED, "xmlParseEntityDecl: no name");
    };
    if name.contains(':') {
        let message = format!("colons are forbidden from entities names '{name}'");
        return Err(c.here(NS_COLON, message).strs(name, None));
    }
    space(c, "the entity name")?;
    if matches!(c.b(), b'"' | b'\'') {
        entity_value(c)?;
    } else {
        if !external_id(c, true)? {
            return c.fail(VALUE_REQUIRED, "Entity value required");
        }
        if !parameter {
            if c.b() != b'>' && !c.blanks() {
                return c.fail(SPACE_REQUIRED, "Space required before 'NDATA'");
            }
            if c.has(b"NDATA") {
                c.i += 5;
                space(c, "'NDATA'")?;
                c.name(true);
            }
        }
    }
    c.blanks();
    if c.b() != b'>' {
        let message = format!("xmlParseEntityDecl: entity {name} not terminated");
        return Err(c.here(ENTITY_NOT_FINISHED, message).strs(name, None));
    }
    c.i += 1;
    Ok(())
}

/// `xmlParseNotationDecl`.
fn notation_decl(c: &mut Cur<'_>) -> Check {
    c.i += 10;
    space(c, "'<!NOTATION'")?;
    let Some(name) = c.name(true) else {
        return c.fail(NOTATION_NOT_STARTED, "NOTATION: Name expected here");
    };
    if name.contains(':') {
        let message = format!("colons are forbidden from notation names '{name}'");
        return Err(c.here(NS_COLON, message).strs(name, None));
    }
    space(c, "the NOTATION name'")?;
    let identified = external_id(c, false)?;
    c.blanks();
    if c.b() != b'>' {
        return c.fail(
            NOTATION_NOT_FINISHED,
            "'>' required to close NOTATION declaration",
        );
    }
    c.i += 1;
    if !identified {
        let message = format!("SAX.xmlSAX2NotationDecl({name}) externalID or PublicID missing");
        return c.fail(NOTATION_PROCESSING, message);
    }
    Ok(())
}

/// A parenthesized `|` list of names (`xmlParseNotationType`) or name tokens
/// (`xmlParseEnumerationType`).
fn enumeration(c: &mut Cur<'_>, notation: bool) -> Check {
    let [open, token, close] = if notation {
        [
            (NOTATION_NOT_STARTED, "NOTATION: Name expected here"),
            (NAME_REQUIRED, "Name expected in NOTATION declaration"),
            (
                NOTATION_NOT_FINISHED,
                "'>' required to close NOTATION declaration",
            ),
        ]
    } else {
        [
            (
                ATTLIST_NOT_STARTED,
                "'(' required to start ATTLIST enumeration",
            ),
            (NMTOKEN_REQUIRED, "NmToken expected in ATTLIST enumeration"),
            (
                ATTLIST_NOT_FINISHED,
                "')' required to finish ATTLIST enumeration",
            ),
        ]
    };
    if c.b() != b'(' {
        return c.fail(open.0, open.1);
    }
    loop {
        // Past the `(` or `|`.
        c.i += 1;
        c.blanks();
        if (if notation { c.name(true) } else { c.nmtoken() }).is_none() {
            return c.fail(token.0, token.1);
        }
        c.blanks();
        if c.b() != b'|' {
            break;
        }
    }
    if c.b() != b')' {
        return c.fail(close.0, close.1);
    }
    c.i += 1;
    Ok(())
}

/// `xmlParseAttributeListDecl` with the attribute type and default.
fn attlist_decl(c: &mut Cur<'_>) -> Check {
    c.i += 9;
    space(c, "'<!ATTLIST'")?;
    if c.name(true).is_none() {
        return c.fail(NAME_REQUIRED, "ATTLIST: no name for Element");
    }
    c.blanks();
    while c.b() != b'>' {
        if c.name(true).is_none() {
            return c.fail(NAME_REQUIRED, "ATTLIST: no name for Attribute");
        }
        space(c, "the attribute name")?;
        const TYPES: [&[u8]; 8] = [
            b"CDATA",
            b"IDREFS",
            b"IDREF",
            b"ID",
            b"ENTITY",
            b"ENTITIES",
            b"NMTOKENS",
            b"NMTOKEN",
        ];
        if let Some(keyword) = TYPES.iter().find(|keyword| c.has(keyword)) {
            c.i += keyword.len();
        } else if c.has(b"NOTATION") {
            c.i += 8;
            space(c, "'NOTATION'")?;
            enumeration(c, true)?;
        } else {
            enumeration(c, false)?;
        }
        space(c, "the attribute type")?;
        if c.has(b"#REQUIRED") || c.has(b"#IMPLIED") {
            c.i += if c.has(b"#REQUIRED") { 9 } else { 8 };
        } else {
            if c.has(b"#FIXED") {
                c.i += 6;
                space(c, "'#FIXED'")?;
            }
            att_value(c, ErrorKind::Syntax, usize::MAX)?;
        }
        if c.b() != b'>' {
            space(c, "the attribute default value")?;
        }
    }
    c.i += 1;
    Ok(())
}

/// `xmlParseElementChildrenContentDeclPriv` after its `(`, `depth` groups deep.
fn children(c: &mut Cur<'_>, depth: usize) -> Check {
    if depth > 256 {
        let message = format!(
            "xmlParseElementChildrenContentDecl : depth {depth} too deep, use XML_PARSE_HUGE"
        );
        return Err(c.here(RESOURCE_LIMIT, message).int(depth as i64));
    }
    let particle = |c: &mut Cur<'_>| -> Check {
        c.blanks();
        if c.b() == b'(' {
            c.i += 1;
            return children(c, depth + 1);
        }
        if c.name(true).is_none() {
            return c.fail(
                ELEMCONTENT_NOT_STARTED,
                "ContentDecl : Name or '(' expected",
            );
        }
        c.i += usize::from(matches!(c.b(), b'?' | b'*' | b'+'));
        Ok(())
    };
    particle(c)?;
    let mut separator = None;
    loop {
        c.blanks();
        match c.b() {
            b')' => break,
            b @ (b',' | b'|') if separator.is_none_or(|s| s == b) => separator = Some(b),
            b',' | b'|' => {
                let expected = separator.map_or(',', char::from);
                let message = format!("xmlParseElementChildrenContentDecl : '{expected}' expected");
                return Err(c.here(SEPARATOR_REQUIRED, message).int(expected as i64));
            }
            _ => {
                return c.fail(
                    ELEMCONTENT_NOT_FINISHED,
                    "ContentDecl : ',' '|' or ')' expected",
                );
            }
        }
        c.i += 1;
        particle(c)?;
    }
    c.i += 1;
    c.i += usize::from(matches!(c.b(), b'?' | b'*' | b'+'));
    Ok(())
}

/// `xmlParseElementDecl` with its content model.
fn element_decl(c: &mut Cur<'_>) -> Check {
    c.i += 9;
    space(c, "'ELEMENT'")?;
    if c.name(true).is_none() {
        return c.fail(NAME_REQUIRED, "xmlParseElementDecl: no name for Element");
    }
    space(c, "the element name")?;
    if c.has(b"EMPTY") || c.has(b"ANY") {
        c.i += if c.has(b"ANY") { 3 } else { 5 };
    } else if c.b() == b'(' {
        c.i += 1;
        c.blanks();
        if !c.has(b"#PCDATA") {
            children(c, 1)?;
        } else {
            c.i += 7;
            c.blanks();
            if c.b() == b')' {
                c.i += 1 + usize::from(c.nb(1) == b'*');
            } else {
                while c.b() == b'|' {
                    c.i += 1;
                    c.blanks();
                    if c.name(true).is_none() {
                        let message = "xmlParseElementMixedContentDecl : Name expected";
                        return c.fail(NAME_REQUIRED, message);
                    }
                    c.blanks();
                }
                if !c.has(b")*") {
                    return Err(
                        c.here(MIXED_NOT_STARTED, "MixedContentDecl : '|' or ')*' expected")
                    );
                }
                c.i += 2;
            }
        }
    } else {
        let message = "xmlParseElementDecl: 'EMPTY', 'ANY' or '(' expected";
        return c.fail(ELEMCONTENT_NOT_STARTED, message);
    }
    c.blanks();
    if c.b() != b'>' {
        return c.fail(GT_REQUIRED, "expected '>'");
    }
    c.i += 1;
    Ok(())
}

/// `xmlParseMarkupDecl` at `<!` or `<?` in the internal subset.
fn markup_decl(c: &mut Cur<'_>) -> Check {
    match c.t.get(c.i + 1..c.i + 4).unwrap_or_default() {
        [b'?', ..] => pi(c),
        [b'!', b'-', b'-'] => comment(c),
        _ if c.has(b"<!ELEMENT") => element_decl(c),
        _ if c.has(b"<!ENTITY") => entity_decl(c),
        _ if c.has(b"<!ATTLIST") => attlist_decl(c),
        _ if c.has(b"<!NOTATION") => notation_decl(c),
        // An unknown keyword after these letters is skipped silently first.
        [b'!', b'E' | b'A' | b'N', _] => Err(fail(
            INT_SUBSET_NOT_FINISHED,
            c.i + 2,
            "Content error in the internal subset",
        )),
        _ => c.fail(
            INT_SUBSET_NOT_FINISHED,
            "Content error in the internal subset",
        ),
    }
}

/// `xmlParsePERefInternal` between declarations.
fn pe_reference(c: &mut Cur<'_>) -> Check {
    c.i += 1;
    if c.name(true).is_none() {
        return c.fail(PEREF_NO_NAME, "PEReference: no name");
    }
    if c.b() != b';' {
        return c.fail(PEREF_SEMICOL_MISSING, "PEReference: expecting ';'");
    }
    c.i += 1;
    Ok(())
}

/// `xmlParseInternalSubset` after its `[`, through the closing `>`; with
/// `nested`, a parameter entity's replacement text, which ends at its end.
fn subset(c: &mut Cur<'_>, nested: bool) -> Check {
    loop {
        c.blanks();
        match c.b() {
            _ if c.eof() && nested => return Ok(()),
            b']' if !nested && !c.eof() => break,
            b'<' if matches!(c.nb(1), b'!' | b'?') => markup_decl(c)?,
            b'%' => pe_reference(c)?,
            _ => {
                return c.fail(
                    INT_SUBSET_NOT_FINISHED,
                    "Content error in the internal subset",
                );
            }
        }
    }
    c.i += 1;
    c.blanks();
    if c.b() != b'>' {
        return c.fail(DOCTYPE_NOT_FINISHED, "DOCTYPE improperly terminated");
    }
    c.i += 1;
    Ok(())
}

/// `xmlParseDocTypeDecl` and the internal subset.
fn doctype(c: &mut Cur<'_>) -> Check {
    c.i += 9;
    space(c, "'DOCTYPE'")?;
    if c.name(true).is_none() {
        return c.fail(NAME_REQUIRED, "xmlParseDocTypeDecl : no DOCTYPE name !");
    }
    c.blanks();
    external_id(c, true)?;
    c.blanks();
    match c.b() {
        b'[' => {
            c.i += 1;
            subset(c, false)
        }
        b'>' => {
            c.i += 1;
            Ok(())
        }
        _ => c.fail(DOCTYPE_NOT_FINISHED, "DOCTYPE improperly terminated"),
    }
}

/// `Premature end of data in tag NAME line N` for the element open at `open`.
fn premature(c: &Cur<'_>, open: usize) -> Found {
    let name = local(Cur { i: open + 1, ..*c }.name(true).unwrap_or_default());
    let line = c.line(open);
    let message = format!("Premature end of data in tag {name} line {line}");
    c.here(TAG_NOT_FINISHED, message)
        .strs(name, None)
        .int(line as i64)
}

/// Character data and references from `c` to `end` (`xmlParseCharDataInternal`).
/// An error in the replacement text of a referenced internal entity is
/// reported where the reference to the entity holding it ends, in the
/// document or, for nested entities, in the referring entity's text.
/// `seen` are the entities whose text is being read (`depth` = its length).
fn text_run(
    c: &mut Cur<'_>,
    end: usize,
    kind: ErrorKind,
    at: usize,
    seen: &mut Vec<String>,
) -> Check {
    while c.i < end {
        match c.ch() {
            Some(('&', _)) => {
                if let Some(found) = reference(c, kind, at) {
                    return Err(found);
                }
                let amp = c.i;
                c.i = after(c.t, c.i, b";").unwrap_or(end);
                let name = String::from_utf8_lossy(&c.t[amp + 1..c.i - 1]).into_owned();
                if c.t[amp + 1] == b'#' || predefined(&name).is_some() {
                    continue;
                }
                let text = match declared(c, &name, false)? {
                    Declared::Internal(text) => text,
                    Declared::No if !seen.is_empty() && !lenient(c)? => {
                        let message = format!("Entity '{name}' not defined");
                        return Err(c.here(UNDECLARED_ENTITY, message).strs(&name, None));
                    }
                    _ => continue,
                };
                if seen.contains(&name) || seen.len() > 20 {
                    return c.fail(ENTITY_LOOP, "Detected an entity reference loop");
                }
                seen.push(name);
                let inner = LineIndex::utf8(text.as_bytes());
                let found = content(c.inner(&inner)?, seen);
                seen.pop();
                if let Some(found) = found {
                    if found.code == EXHAUSTED {
                        return Err(found);
                    }
                    let place = found.place.or_else(|| {
                        (!seen.is_empty()).then(|| (c.line(c.i), c.lines.text_column(c.i)))
                    });
                    return Err(Found {
                        at: if place.is_some() { found.at } else { c.i },
                        place,
                        ..found
                    });
                }
            }
            _ if c.has(b"]]>") => {
                return c.fail(MISPLACED_CDATA_END, "Sequence ']]>' not allowed in content");
            }
            Some(('\0', _)) => return c.fail(INVALID_CHAR, NUL),
            Some((ch, len)) if xml_char(ch.into()) => c.i += len,
            Some((ch, _)) => {
                let message = format!("PCDATA invalid Char value {}", ch as u32);
                return Err(c.here(INVALID_CHAR, message).int(ch as i64));
            }
            None => return c.fail(INVALID_ENCODING, "Invalid bytes in character encoding"),
        }
    }
    Ok(())
}

/// libxml2's first error in an entity's replacement text read as content.
fn content(at: Cur<'_>, seen: &mut Vec<String>) -> Option<Found> {
    let t = at.t;
    let mut scan = Scan::default();
    while scan.pos < t.len() {
        let pos = scan.pos;
        let (part, end) = part_at(t, pos, false);
        let mut c = Cur { i: pos, ..at };
        let checked = match part {
            Part::Text => text_run(
                &mut c,
                end.unwrap_or(t.len()),
                ErrorKind::Syntax,
                usize::MAX,
                seen,
            ),
            Part::EndTag => match scan.open.last() {
                Some(&open) => end_tag(&mut c, open),
                None => c.fail(NOT_WELL_BALANCED, "chunk is not well balanced"),
            },
            Part::Comment => comment(&mut c),
            Part::Pi => pi(&mut c),
            Part::Cdata => cdata(&mut c),
            _ => start_tag(&mut c, &scan.open, ErrorKind::Syntax, usize::MAX).map(drop),
        };
        if let Err(found) = checked {
            return Some(found);
        }
        scan.advance(t, part, pos, end?);
    }
    let &open = scan.open.last()?;
    Some(premature(&Cur { i: t.len(), ..at }, open))
}

// ---- the error at an offset ----

/// libxml2's error for a failure of `kind` at text offset `at`.
pub(super) fn found(
    kind: ErrorKind,
    lines: &LineIndex<'_>,
    at: usize,
    scan: &mut Scan,
    work: &Work,
) -> Found {
    let found = described(kind, lines, at, scan, work);
    if found.code == EXHAUSTED {
        fallback(kind, at)
    } else {
        found
    }
}

fn described(
    kind: ErrorKind,
    lines: &LineIndex<'_>,
    at: usize,
    scan: &mut Scan,
    work: &Work,
) -> Found {
    let t = lines.text();
    let c = Cur::new(lines, work, at);
    let semantic = match kind {
        ErrorKind::InvalidUtf8
        | ErrorKind::InvalidUtf16
        | ErrorKind::UnmappedByte
        | ErrorKind::EncodingMismatch => {
            Some(c.here(INVALID_ENCODING, "Invalid bytes in character encoding"))
        }
        ErrorKind::UnsupportedEncoding => {
            let mut d = Cur {
                i: lines.starts[0],
                ..c
            };
            let declared = d.has(b"<?xml") && blank(d.nb(5));
            Some(
                declared
                    .then(|| xml_decl(&mut d).err())
                    .flatten()
                    .unwrap_or_else(|| unsupported_encoding(&c)),
            )
        }
        ErrorKind::LimitExceeded(Limit::EntityExpansion) => {
            // At a start tag, libxml2 is adding default attributes after its attributes.
            let tag = (c.b() == b'<' && c.nb(1) != b'/')
                .then(|| unquoted(t, at + 1, b">"))
                .flatten();
            let at = tag.map_or(at, |end| end - usize::from(t[end - 1] == b'/'));
            let message =
                "Maximum entity amplification factor exceeded, see xmlCtxtSetMaxAmplification.";
            Some(fail(RESOURCE_LIMIT, at, message))
        }
        ErrorKind::EntityLoop if c.b() == b'%' => {
            let end = after(t, at, b";").unwrap_or(at);
            Some(fail(ENTITY_LOOP, end, "Detected an entity reference loop"))
        }
        ErrorKind::UnknownEntity if c.b() == b'%' => {
            let mut r = Cur { i: at + 1, ..c };
            let name = r.name(true).unwrap_or_default();
            r.i += usize::from(r.b() == b';');
            Some(
                r.here(
                    UNDECLARED_ENTITY,
                    format!("PEReference: %{name}; not found"),
                )
                .strs(name, None),
            )
        }
        ErrorKind::UnknownEntity
        | ErrorKind::UnparsedEntity
        | ErrorKind::ExternalEntityInAttribute
            if c.b() == b'&' =>
        {
            reference(&c, kind, at)
        }
        ErrorKind::LimitExceeded(limit) if limit != Limit::Depth => Some(fallback(kind, at)),
        ErrorKind::ExternalResource
        | ErrorKind::XInclude
        | ErrorKind::UnsupportedOption
        | ErrorKind::Allocation
        | ErrorKind::IdentifierExhausted
        | ErrorKind::Aborted
        | ErrorKind::InvalidState => Some(fallback(kind, at)),
        _ => None,
    };
    if let Some(found) = semantic {
        return found;
    }
    if at < scan.pos {
        *scan = Scan::new(scan.content);
    }
    let checked = scan.to(t, at, work).and_then(|(part, start, end)| {
        // Each re-read of a construct is charged again.
        work.charge(end.unwrap_or(t.len()) - start)?;
        reread(kind, &Cur::new(lines, work, start), at, part, scan)
    });
    checked.err().unwrap_or_else(|| fallback(kind, at))
}

/// Re-reads the construct at `start` as libxml2 does; its first error.
fn reread(kind: ErrorKind, c: &Cur<'_>, at: usize, part: Part, scan: &Scan) -> Check {
    let (t, lines, start) = (c.t, c.lines, c.i);
    let mut c = Cur { ..*c };
    // Content reads `<!DOCTYPE` as a start tag without a name.
    let part = match part {
        Part::Doctype if scan.content => Part::StartTag,
        part => part,
    };
    let document = lines.starts[0];
    match part {
        Part::Pi if start == document && !scan.content && c.has(b"<?xml") && blank(c.nb(5)) => {
            xml_decl(&mut c)
        }
        Part::Pi => pi(&mut c),
        Part::Comment => comment(&mut c),
        Part::Text if scan.subset => {
            if c.b() != b'%' {
                return Ok(());
            }
            pe_reference(&mut c)?;
            let name = std::str::from_utf8(&t[start + 1..c.i - 1]).unwrap_or_default();
            let Declared::Internal(text) = declared(&c, name, true)? else {
                return Ok(());
            };
            let inner = LineIndex::utf8(text.as_bytes());
            let checked = subset(&mut c.inner(&inner)?, true);
            checked.map_err(|found| Found { at: c.i, ..found })
        }
        Part::Decl => markup_decl(&mut c),
        Part::Subset => c.fail(
            INT_SUBSET_NOT_FINISHED,
            "Content error in the internal subset",
        ),
        Part::Doctype if c.b() == b']' => {
            c.i += 1;
            c.blanks();
            match c.b() {
                b'>' => Ok(()),
                _ => c.fail(DOCTYPE_NOT_FINISHED, "DOCTYPE improperly terminated"),
            }
        }
        _ if scan.content && scan.open.is_empty() && part == Part::EndTag => {
            c.fail(NOT_WELL_BALANCED, "chunk is not well balanced")
        }
        _ if scan.open.is_empty() && scan.rooted && !scan.content => {
            // The epilog after the root element.
            c.i = c.i.max(at);
            if part == Part::Text {
                c.blanks();
            }
            c.fail(DOCUMENT_END, "Extra content at the end of the document")
        }
        Part::Doctype => doctype(&mut c),
        Part::StartTag => start_tag(&mut c, &scan.open, kind, at).map(drop),
        _ if scan.open.is_empty() && !scan.content => {
            // The prolog before the root element.
            if part != Part::Text {
                c.i += 1;
                return c.fail(NAME_REQUIRED, "StartTag: invalid element name");
            }
            c.i = at;
            c.blanks();
            if c.i == document && matches!(c.b(), 0) {
                return c.fail(DOCUMENT_EMPTY, "Document is empty");
            }
            c.fail(DOCUMENT_EMPTY, "Start tag expected, '<' not found")
        }
        Part::EndTag => end_tag(&mut c, *scan.open.last().expect("an open element")),
        Part::Cdata => cdata(&mut c),
        Part::Text if at >= t.len() && !scan.open.is_empty() => Err(premature(
            &Cur { i: at, ..c },
            *scan.open.last().expect("open"),
        )),
        Part::Text => {
            // A reference error may be reported past its `&`: read from there.
            c.i = memchr::memrchr(b'&', &t[start..at]).map_or(at, |amp| start + amp);
            let end = memchr::memchr(b'<', &t[at..]).map_or(t.len(), |lt| at + lt);
            let from = c.i;
            text_run(&mut c, end, kind, from, &mut Vec::new())
        }
    }
}

/// `Unsupported encoding: NAME` after the declaration's encoding value.
fn unsupported_encoding(r: &Cur<'_>) -> Found {
    let (t, start) = (r.t, r.lines.starts[0]);
    let end = after(t, start, b"?>").unwrap_or(t.len());
    if let Err(exhausted) = r.work.charge(end - start) {
        return exhausted;
    }
    let mut c = Cur { i: start, ..*r };
    let value = memchr::memmem::find(&t[start..end], b"encoding").and_then(|at| {
        c.i = start + at;
        let label = |c: &mut Cur<'_>| {
            while !matches!(c.b(), b'"' | b'\'') && !c.eof() {
                c.i += 1;
            }
            Ok(())
        };
        quoted(&mut c, b"encoding", label).ok().flatten()
    });
    let name = value.map_or(Cow::Borrowed(""), |(from, to)| {
        String::from_utf8_lossy(&t[from..to])
    });
    c.here(
        UNSUPPORTED_ENCODING,
        format!("Unsupported encoding: {name}"),
    )
    .strs(&name, None)
}

/// The context-free mapping, for kinds the input cannot refine.
fn fallback(kind: ErrorKind, at: usize) -> Found {
    let message = match kind {
        ErrorKind::LimitExceeded(Limit::Depth) => "Excessive depth in document".into(),
        ErrorKind::LimitExceeded(limit) => format!("Resource limit exceeded: {limit:?}"),
        ErrorKind::Allocation => "Memory allocation failed".into(),
        ErrorKind::ExternalResource => "failed to load external entity".into(),
        ErrorKind::UnsupportedOption => "Invalid argument".into(),
        ErrorKind::Aborted => "Parser stopped by the application".into(),
        ErrorKind::InvalidState => "internal error".into(),
        kind => format!("{kind:?}"),
    };
    fail(kind.libxml2_code().1, at, message)
}
