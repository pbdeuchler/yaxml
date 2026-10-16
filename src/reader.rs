use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    ops::Range,
};

use memchr::{memchr, memmem};

use crate::escape::{copy_cow, needs_normalization, normalize, xml_char};
use crate::{Error, ErrorKind, Limit};

const XML: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS: &str = "http://www.w3.org/2000/xmlns/";

/// Inclusive resource ceilings. An element's start tag counts as one token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub max_input_bytes: usize,
    pub max_token_bytes: usize,
    pub max_depth: usize,
    pub max_attributes: usize,
    /// Number of emitted events, including the declaration and empty elements.
    pub max_events: usize,
    /// Cumulative requested string capacity for normalization and namespace
    /// copies, including buffers released while reading. Borrowed text is free.
    pub max_owned_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_bytes: 64 * 1024 * 1024,
            max_token_bytes: 1024 * 1024,
            max_depth: 256,
            max_attributes: 1024,
            max_events: 1_000_000,
            max_owned_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Both lexical spelling and expanded name; an empty namespace is `None`.
#[derive(Debug, Eq, PartialEq)]
pub struct Name<'a> {
    pub qualified: &'a str,
    pub prefix: Option<&'a str>,
    pub local: &'a str,
    pub namespace: Option<Cow<'a, str>>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Attribute<'a> {
    pub name: Name<'a>,
    pub value: Cow<'a, str>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Element<'a> {
    pub name: Name<'a>,
    /// Source order, including namespace declarations in the XMLNS namespace.
    pub attributes: Vec<Attribute<'a>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Declaration<'a> {
    pub version: &'a str,
    pub encoding: Option<&'a str>,
    pub standalone: Option<bool>,
}

/// Logical events borrow the input except where XML normalization changes it.
#[derive(Debug, Eq, PartialEq)]
pub enum Event<'a> {
    Declaration(Declaration<'a>),
    Start(Element<'a>),
    Empty(Element<'a>),
    End(Name<'a>),
    Text(Cow<'a, str>),
    Cdata(Cow<'a, str>),
    Comment(Cow<'a, str>),
    ProcessingInstruction { target: &'a str, data: Cow<'a, str> },
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum NamespaceId {
    Xml,
    Xmlns,
    Declared(usize),
}

struct Binding<'a> {
    prefix: &'a str,
    uri: Cow<'a, str>,
    namespace: Option<NamespaceId>,
    previous: Option<usize>,
}

struct Frame<'a> {
    qualified: &'a str,
    bindings_start: usize,
}

#[derive(Clone, Copy)]
enum Phase {
    Prolog,
    Content,
    Epilog,
}

/// Strict, namespace-aware XML 1.0 event reader over a complete UTF-8 input.
///
/// This borrows the complete input; bounded-memory I/O streaming is a later API.
/// Top-level whitespace is skipped, while whitespace inside the root is kept.
/// After an error, every call returns that same error. EOF repeatedly returns
/// `Ok(None)`. DTDs and encodings other than UTF-8 are explicitly unsupported.
pub struct Reader<'a> {
    input: &'a str,
    cursor: usize,
    initial: usize,
    range: Range<usize>,
    phase: Phase,
    stack: Vec<Frame<'a>>,
    bindings: Vec<Binding<'a>>,
    namespace_index: HashMap<&'a str, usize>,
    namespace_ids: HashMap<Cow<'a, str>, NamespaceId>,
    limits: Limits,
    events: usize,
    owned_bytes: usize,
    failure: Option<Error>,
    finished: bool,
}

impl<'a> Reader<'a> {
    pub fn new(input: &'a [u8]) -> Result<Self, Error> {
        Self::with_limits(input, Limits::default())
    }

    pub fn with_limits(input: &'a [u8], limits: Limits) -> Result<Self, Error> {
        if input.len() > limits.max_input_bytes {
            return Err(Error::new(ErrorKind::LimitExceeded(Limit::InputBytes), 0));
        }
        if input.starts_with(&[0xff, 0xfe])
            || input.starts_with(&[0xfe, 0xff])
            || input.starts_with(&[0, 0, 0xfe, 0xff])
            || input.starts_with(&[0, b'<', 0, b'?'])
        {
            return Err(Error::new(ErrorKind::UnsupportedEncoding, 0));
        }
        let input = std::str::from_utf8(input)
            .map_err(|e| Error::new(ErrorKind::InvalidUtf8, e.valid_up_to()))?;
        if let Some((i, _)) = input.char_indices().find(|(_, c)| !xml_char(*c)) {
            return Err(Error::new(ErrorKind::InvalidCharacter, i));
        }
        let initial = if input.starts_with('\u{feff}') { 3 } else { 0 };
        Ok(Self {
            input,
            cursor: initial,
            initial,
            range: initial..initial,
            phase: Phase::Prolog,
            stack: Vec::new(),
            bindings: Vec::new(),
            namespace_index: HashMap::new(),
            namespace_ids: HashMap::new(),
            limits,
            events: 0,
            owned_bytes: 0,
            failure: None,
            finished: false,
        })
    }

    /// Byte position after the most recently consumed input.
    pub fn position(&self) -> usize {
        self.cursor
    }

    /// Raw source span of the latest emitted event, including markup delimiters.
    pub fn event_range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub fn read_event(&mut self) -> Result<Option<Event<'a>>, Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.finished {
            return Ok(None);
        }
        let result = self.read_next();
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }

    fn read_next(&mut self) -> Result<Option<Event<'a>>, Error> {
        // Whitespace between top-level markup is not a document text node.
        while !matches!(self.phase, Phase::Content) {
            let start = self.cursor;
            while self
                .input
                .as_bytes()
                .get(self.cursor)
                .is_some_and(|b| space(*b))
            {
                self.cursor += 1;
            }
            self.check_token(start, self.cursor)?;
            if self.cursor == start {
                break;
            }
        }
        let start = self.cursor;
        if start == self.input.len() {
            return match self.phase {
                Phase::Prolog => Err(Error::new(ErrorKind::MissingRoot, start)),
                Phase::Content => Err(Error::new(ErrorKind::UnexpectedEof, start)),
                Phase::Epilog => {
                    self.finished = true;
                    Ok(None)
                }
            };
        }
        if self.events == self.limits.max_events {
            return Err(Error::new(ErrorKind::LimitExceeded(Limit::Events), start));
        }
        let tail = &self.input[start..];
        let event = if !tail.starts_with('<') {
            self.text()?
        } else if tail.starts_with("<!--") {
            let end = self.delimited(start, 4, b"-->")?;
            let raw = &self.input[start + 4..end - 3];
            if raw.contains("--") || raw.ends_with('-') {
                return Err(Error::new(ErrorKind::Syntax, start));
            }
            self.cursor = end;
            Event::Comment(self.normalized(raw, start + 4, false, false)?)
        } else if tail.starts_with("<![CDATA[") {
            if !matches!(self.phase, Phase::Content) {
                return Err(Error::new(ErrorKind::TextOutsideRoot, start));
            }
            let end = self.delimited(start, 9, b"]]>")?;
            self.cursor = end;
            Event::Cdata(self.normalized(
                &self.input[start + 9..end - 3],
                start + 9,
                false,
                false,
            )?)
        } else if tail.starts_with("<?") {
            self.instruction()?
        } else if tail.starts_with("</") {
            self.end_element()?
        } else if tail.starts_with("<!DOCTYPE") {
            return Err(Error::new(ErrorKind::UnsupportedDtd, start));
        } else if tail.starts_with("<!") {
            return Err(Error::new(ErrorKind::Syntax, start));
        } else {
            self.start_element()?
        };
        self.events += 1;
        self.range = start..self.cursor;
        Ok(Some(event))
    }

    fn check_token(&self, start: usize, end: usize) -> Result<(), Error> {
        if end - start > self.limits.max_token_bytes {
            Err(Error::new(
                ErrorKind::LimitExceeded(Limit::TokenBytes),
                start,
            ))
        } else {
            Ok(())
        }
    }

    fn charge_owned(&mut self, bytes: usize, offset: usize) -> Result<(), Error> {
        self.owned_bytes = self
            .owned_bytes
            .checked_add(bytes)
            .filter(|total| *total <= self.limits.max_owned_bytes)
            .ok_or(Error::new(
                ErrorKind::LimitExceeded(Limit::OwnedBytes),
                offset,
            ))?;
        Ok(())
    }

    fn normalized(
        &mut self,
        raw: &'a str,
        offset: usize,
        attribute: bool,
        references: bool,
    ) -> Result<Cow<'a, str>, Error> {
        if needs_normalization(raw, attribute, references) {
            self.charge_owned(raw.len(), offset)?;
        }
        normalize(raw, offset, attribute, references)
    }

    fn delimited(&self, start: usize, prefix: usize, delimiter: &[u8]) -> Result<usize, Error> {
        let available = self.input.len() - start;
        let length = available.min(self.limits.max_token_bytes);
        if prefix <= length
            && let Some(relative) = memmem::find(
                &self.input.as_bytes()[start + prefix..start + length],
                delimiter,
            )
        {
            return Ok(start + prefix + relative + delimiter.len());
        }
        Err(Error::new(
            if available > self.limits.max_token_bytes {
                ErrorKind::LimitExceeded(Limit::TokenBytes)
            } else {
                ErrorKind::UnexpectedEof
            },
            start,
        ))
    }

    fn tag_end(&self, start: usize) -> Result<usize, Error> {
        let mut quote = None;
        for (relative, &byte) in self.input.as_bytes()[start..]
            .iter()
            .take(self.limits.max_token_bytes)
            .enumerate()
        {
            match (quote, byte) {
                (Some(q), b) if q == b => quote = None,
                (None, b'\'' | b'"') => quote = Some(byte),
                (None, b'>') => return Ok(start + relative + 1),
                _ => {}
            }
        }
        Err(Error::new(
            if self.input.len() - start > self.limits.max_token_bytes {
                ErrorKind::LimitExceeded(Limit::TokenBytes)
            } else {
                ErrorKind::UnexpectedEof
            },
            start,
        ))
    }

    fn text(&mut self) -> Result<Event<'a>, Error> {
        let start = self.cursor;
        let rest = &self.input.as_bytes()[start..];
        let length = memchr(b'<', rest).unwrap_or(rest.len());
        let end = start + length;
        self.check_token(start, end)?;
        if !matches!(self.phase, Phase::Content) {
            return Err(Error::new(ErrorKind::TextOutsideRoot, start));
        }
        let raw = &self.input[start..end];
        if let Some(i) = raw.find("]]>") {
            return Err(Error::new(ErrorKind::Syntax, start + i));
        }
        self.cursor = end;
        Ok(Event::Text(self.normalized(raw, start, false, true)?))
    }

    fn instruction(&mut self) -> Result<Event<'a>, Error> {
        let start = self.cursor;
        let end = self.delimited(start, 2, b"?>")?;
        let raw = &self.input[start + 2..end - 2];
        let name_end = raw.bytes().position(space).unwrap_or(raw.len());
        let target = &raw[..name_end];
        validate_name(target, start + 2, false)?;
        self.cursor = end;
        if target.eq_ignore_ascii_case("xml") {
            if target != "xml" || start != self.initial {
                return Err(Error::new(ErrorKind::InvalidDeclaration, start));
            }
            return Ok(Event::Declaration(parse_declaration(
                &raw[name_end..],
                start + 2 + name_end,
            )?));
        }
        let raw_data = raw[name_end..].trim_start_matches([' ', '\t', '\r', '\n']);
        let data_offset = end - 2 - raw_data.len();
        Ok(Event::ProcessingInstruction {
            target,
            data: self.normalized(raw_data, data_offset, false, false)?,
        })
    }

    fn start_element(&mut self) -> Result<Event<'a>, Error> {
        let start = self.cursor;
        if matches!(self.phase, Phase::Epilog) {
            return Err(Error::new(ErrorKind::MultipleRoots, start));
        }
        if self.stack.len() >= self.limits.max_depth {
            return Err(Error::new(ErrorKind::LimitExceeded(Limit::Depth), start));
        }
        let end = self.tag_end(start)?;
        let empty = self.input.as_bytes()[end - 2] == b'/';
        let content_end = end - if empty { 2 } else { 1 };
        let mut scan = TagScanner {
            raw: &self.input[start + 1..content_end],
            cursor: 0,
            offset: start + 1,
        };
        let qualified = scan.name()?;
        qname(qualified, start + 1)?;
        let bindings_start = self.bindings.len();
        let mut attributes = Vec::new();
        while let Some((name, value, offset, value_offset)) = scan.attribute()? {
            if attributes.len() == self.limits.max_attributes {
                return Err(Error::new(
                    ErrorKind::LimitExceeded(Limit::Attributes),
                    offset,
                ));
            }
            let (prefix, local) = qname(name, offset)?;
            let value = self.normalized(value, value_offset, true, true)?;
            let declaration = if name == "xmlns" {
                Some("")
            } else if prefix == Some("xmlns") {
                Some(local)
            } else {
                None
            };
            if let Some(bound_prefix) = declaration {
                if bound_prefix == "xmlns"
                    || value == XMLNS
                    || (bound_prefix == "xml" && value != XML)
                    || (bound_prefix != "xml" && value == XML)
                    || (!bound_prefix.is_empty() && value.is_empty())
                {
                    return Err(Error::new(ErrorKind::Namespace, offset));
                }
                let namespace = self.intern_namespace(&value, offset)?;
                self.bindings
                    .try_reserve(1)
                    .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
                if let Cow::Owned(uri) = &value {
                    self.charge_owned(uri.len(), offset)?;
                }
                self.namespace_index
                    .try_reserve(1)
                    .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
                let previous = self
                    .namespace_index
                    .insert(bound_prefix, self.bindings.len());
                self.bindings.push(Binding {
                    prefix: bound_prefix,
                    uri: copy_cow(&value, offset)?,
                    namespace,
                    previous,
                });
            }
            attributes
                .try_reserve(1)
                .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
            attributes.push(Attribute {
                name: Name {
                    qualified: name,
                    prefix,
                    local,
                    namespace: None,
                },
                value,
            });
        }
        let (name, _) = self.resolve_name(qualified, false, start + 1)?;
        let mut expanded = HashSet::new();
        expanded
            .try_reserve(attributes.len())
            .map_err(|_| Error::new(ErrorKind::Allocation, start))?;
        for attribute in &mut attributes {
            let (name, namespace) = self.resolve_name(attribute.name.qualified, true, start)?;
            attribute.name = name;
            if !expanded.insert((attribute.name.local, namespace)) {
                return Err(Error::new(ErrorKind::DuplicateAttribute, start));
            }
        }
        self.cursor = end;
        let element = Element { name, attributes };
        if empty {
            self.unwind_bindings(bindings_start);
            if self.stack.is_empty() {
                self.phase = Phase::Epilog;
            }
            Ok(Event::Empty(element))
        } else {
            self.stack
                .try_reserve(1)
                .map_err(|_| Error::new(ErrorKind::Allocation, start))?;
            self.stack.push(Frame {
                qualified,
                bindings_start,
            });
            self.phase = Phase::Content;
            Ok(Event::Start(element))
        }
    }

    #[allow(
        clippy::ptr_arg,
        reason = "the interner must preserve borrowed input and account for owned copies"
    )]
    fn intern_namespace(
        &mut self,
        uri: &Cow<'a, str>,
        offset: usize,
    ) -> Result<Option<NamespaceId>, Error> {
        if uri.is_empty() {
            return Ok(None);
        }
        if uri == XML {
            return Ok(Some(NamespaceId::Xml));
        }
        if let Some(id) = self.namespace_ids.get(uri.as_ref()) {
            return Ok(Some(*id));
        }
        self.namespace_ids
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
        if let Cow::Owned(value) = uri {
            self.charge_owned(value.len(), offset)?;
        }
        let id = NamespaceId::Declared(self.namespace_ids.len());
        self.namespace_ids.insert(copy_cow(uri, offset)?, id);
        Ok(Some(id))
    }

    fn resolve_name(
        &mut self,
        qualified: &'a str,
        attribute: bool,
        offset: usize,
    ) -> Result<(Name<'a>, Option<NamespaceId>), Error> {
        let (prefix, local) = qname(qualified, offset)?;
        let (namespace, identity) = match prefix {
            Some("xmlns") if attribute => (Some(Cow::Borrowed(XMLNS)), Some(NamespaceId::Xmlns)),
            Some("xmlns") => return Err(Error::new(ErrorKind::Namespace, offset)),
            Some("xml") => (Some(Cow::Borrowed(XML)), Some(NamespaceId::Xml)),
            None if attribute && qualified == "xmlns" => {
                (Some(Cow::Borrowed(XMLNS)), Some(NamespaceId::Xmlns))
            }
            None if attribute => (None, None),
            _ => match self.namespace_index.get(prefix.unwrap_or("")).copied() {
                Some(i) if !self.bindings[i].uri.is_empty() => {
                    if let Cow::Owned(uri) = &self.bindings[i].uri {
                        self.charge_owned(uri.len(), offset)?;
                    }
                    (
                        Some(copy_cow(&self.bindings[i].uri, offset)?),
                        self.bindings[i].namespace,
                    )
                }
                _ if prefix.is_some() => return Err(Error::new(ErrorKind::Namespace, offset)),
                _ => (None, None),
            },
        };
        Ok((
            Name {
                qualified,
                prefix,
                local,
                namespace,
            },
            identity,
        ))
    }

    fn end_element(&mut self) -> Result<Event<'a>, Error> {
        let start = self.cursor;
        let end = self.tag_end(start)?;
        let raw = &self.input[start + 2..end - 1];
        let mut scan = TagScanner {
            raw,
            cursor: 0,
            offset: start + 2,
        };
        let qualified = scan.name()?;
        scan.skip_space();
        if scan.cursor != raw.len() {
            return Err(Error::new(ErrorKind::Syntax, start));
        }
        if self
            .stack
            .last()
            .is_none_or(|frame| frame.qualified != qualified)
        {
            return Err(Error::new(ErrorKind::MismatchedEndTag, start));
        }
        let (name, _) = self.resolve_name(qualified, false, start + 2)?;
        let frame = self.stack.pop().expect("matched frame");
        self.unwind_bindings(frame.bindings_start);
        if self.stack.is_empty() {
            self.phase = Phase::Epilog;
        }
        self.cursor = end;
        Ok(Event::End(name))
    }

    fn unwind_bindings(&mut self, start: usize) {
        while self.bindings.len() > start {
            let binding = self.bindings.pop().expect("binding scope");
            if let Some(previous) = binding.previous {
                self.namespace_index.insert(binding.prefix, previous);
            } else {
                self.namespace_index.remove(binding.prefix);
            }
        }
    }
}

struct TagScanner<'a> {
    raw: &'a str,
    cursor: usize,
    offset: usize,
}

impl<'a> TagScanner<'a> {
    fn skip_space(&mut self) -> bool {
        let start = self.cursor;
        while self
            .raw
            .as_bytes()
            .get(self.cursor)
            .is_some_and(|b| space(*b))
        {
            self.cursor += 1;
        }
        start != self.cursor
    }

    fn name(&mut self) -> Result<&'a str, Error> {
        let start = self.cursor;
        while let Some(byte) = self.raw.as_bytes().get(self.cursor) {
            if space(*byte) || matches!(byte, b'=' | b'/' | b'>' | b'\'' | b'"') {
                break;
            }
            self.cursor += 1;
        }
        let name = &self.raw[start..self.cursor];
        validate_name(name, self.offset + start, true)?;
        Ok(name)
    }

    // Returns the name's source offset; values are still unnormalized input.
    fn attribute(&mut self) -> Result<Option<(&'a str, &'a str, usize, usize)>, Error> {
        let spaced = self.skip_space();
        if self.cursor == self.raw.len() {
            return Ok(None);
        }
        if !spaced {
            return Err(Error::new(ErrorKind::Syntax, self.offset + self.cursor));
        }
        let start = self.cursor;
        let name = self.name()?;
        self.skip_space();
        if self.raw.as_bytes().get(self.cursor) != Some(&b'=') {
            return Err(Error::new(ErrorKind::Syntax, self.offset + self.cursor));
        }
        self.cursor += 1;
        self.skip_space();
        let quote = *self
            .raw
            .as_bytes()
            .get(self.cursor)
            .filter(|b| matches!(b, b'\'' | b'"'))
            .ok_or(Error::new(ErrorKind::Syntax, self.offset + self.cursor))?;
        self.cursor += 1;
        let value_start = self.cursor;
        let length = memchr(quote, &self.raw.as_bytes()[value_start..]).ok_or(Error::new(
            ErrorKind::UnexpectedEof,
            self.offset + value_start,
        ))?;
        self.cursor += length;
        let value = &self.raw[value_start..self.cursor];
        if let Some(i) = memchr(b'<', value.as_bytes()) {
            return Err(Error::new(ErrorKind::Syntax, self.offset + value_start + i));
        }
        self.cursor += 1;
        Ok(Some((
            name,
            value,
            self.offset + start,
            self.offset + value_start,
        )))
    }
}

fn parse_declaration(raw: &str, offset: usize) -> Result<Declaration<'_>, Error> {
    let mut scan = TagScanner {
        raw,
        cursor: 0,
        offset,
    };
    let mut declaration = Declaration {
        version: "1.0",
        encoding: None,
        standalone: None,
    };
    let mut count = 0;
    while let Some((name, value, position, _)) = scan
        .attribute()
        .map_err(|e| Error::new(ErrorKind::InvalidDeclaration, e.offset))?
    {
        match name {
            "version" if count == 0 && value == "1.0" => declaration.version = value,
            "encoding" if count == 1 => {
                if !value.eq_ignore_ascii_case("UTF-8") {
                    return Err(Error::new(ErrorKind::UnsupportedEncoding, position));
                }
                declaration.encoding = Some(value);
            }
            "standalone" if count > 0 && declaration.standalone.is_none() => {
                declaration.standalone = match value {
                    "yes" => Some(true),
                    "no" => Some(false),
                    _ => return Err(Error::new(ErrorKind::InvalidDeclaration, position)),
                };
            }
            _ => return Err(Error::new(ErrorKind::InvalidDeclaration, position)),
        }
        count += 1;
    }
    if count == 0 {
        return Err(Error::new(ErrorKind::InvalidDeclaration, offset));
    }
    Ok(declaration)
}

fn space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

fn qname(name: &str, offset: usize) -> Result<(Option<&str>, &str), Error> {
    if let Some((prefix, local)) = name.split_once(':') {
        validate_name(prefix, offset, false)?;
        validate_name(local, offset + prefix.len() + 1, false)?;
        Ok((Some(prefix), local))
    } else {
        validate_name(name, offset, false)?;
        Ok((None, name))
    }
}

fn validate_name(name: &str, offset: usize, colon: bool) -> Result<(), Error> {
    let mut chars = name.char_indices();
    let Some((_, first)) = chars.next() else {
        return Err(Error::new(ErrorKind::InvalidName, offset));
    };
    if !name_start(first, colon) {
        return Err(Error::new(ErrorKind::InvalidName, offset));
    }
    for (i, c) in chars {
        if !name_start(c, colon)
            && !matches!(c, '-' | '.' | '0'..='9' | '\u{b7}' | '\u{300}'..='\u{36f}' | '\u{203f}'..='\u{2040}')
        {
            return Err(Error::new(ErrorKind::InvalidName, offset + i));
        }
    }
    Ok(())
}

fn name_start(c: char, colon: bool) -> bool {
    (colon && c == ':')
        || matches!(c, 'A'..='Z' | 'a'..='z' | '_'
        | '\u{c0}'..='\u{d6}' | '\u{d8}'..='\u{f6}' | '\u{f8}'..='\u{2ff}'
        | '\u{370}'..='\u{37d}' | '\u{37f}'..='\u{1fff}' | '\u{200c}'..='\u{200d}'
        | '\u{2070}'..='\u{218f}' | '\u{2c00}'..='\u{2fef}' | '\u{3001}'..='\u{d7ff}'
        | '\u{f900}'..='\u{fdcf}' | '\u{fdf0}'..='\u{fffd}' | '\u{10000}'..='\u{effff}')
}
