use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use memchr::memchr;

use crate::escape::{copy_cow, needs_normalization, normalize};
use crate::tokenizer::TokenKind;
use crate::{Attribute, Declaration, Element, Error, ErrorKind, Event, Limit, Limits, Name};

const XML: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS: &str = "http://www.w3.org/2000/xmlns/";

/// Shared accounting for normalized strings and retained structural strings.
pub(crate) struct Budget {
    limits: Limits,
    owned_bytes: usize,
}

impl Budget {
    pub(crate) fn remaining_owned(&self) -> usize {
        self.limits.max_owned_bytes - self.owned_bytes
    }

    pub(crate) fn charge_owned(&mut self, bytes: usize, offset: usize) -> Result<(), Error> {
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

    /// Copies a string without tying its result to the temporary token lifetime.
    pub(crate) fn copy<'result>(
        &mut self,
        text: &str,
        offset: usize,
    ) -> Result<Cow<'result, str>, Error> {
        self.charge_owned(text.len(), offset)?;
        let mut copied = String::new();
        copied
            .try_reserve(text.len())
            .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
        copied.push_str(text);
        Ok(Cow::Owned(copied))
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum NamespaceId {
    Xml,
    Xmlns,
    Declared(usize),
}

struct NamespaceEntry {
    id: NamespaceId,
    bindings: usize,
}

struct Binding<'source> {
    prefix: Cow<'source, str>,
    uri: Cow<'source, str>,
    namespace: Option<NamespaceId>,
    previous: Option<usize>,
}

struct Frame<'source> {
    qualified: Cow<'source, str>,
    bindings_start: usize,
}

#[derive(Clone, Copy)]
enum Phase {
    Prolog,
    Content,
    Epilog,
}

/// XML semantics shared by borrowed slices and temporary streaming tokens.
/// Events never borrow owned state: retained namespaces are copied as needed.
pub(crate) struct ParserState<'source> {
    initial: usize,
    encoding: crate::encoding::SourceEncoding,
    declaration_seen: bool,
    token_limit: usize,
    token_limit_kind: Limit,
    phase: Phase,
    stack: Vec<Frame<'source>>,
    bindings: Vec<Binding<'source>>,
    namespace_index: HashMap<Cow<'source, str>, usize>,
    namespace_ids: HashMap<Cow<'source, str>, NamespaceEntry>,
    next_namespace: usize,
    budget: Budget,
    events: usize,
}

impl<'source> ParserState<'source> {
    pub(crate) fn owned_budget(&mut self) -> &mut Budget {
        &mut self.budget
    }

    pub(crate) fn new(limits: Limits, initial_offset: usize) -> Self {
        Self {
            initial: initial_offset,
            encoding: crate::encoding::SourceEncoding::Utf8,
            declaration_seen: false,
            token_limit: limits.max_token_bytes,
            token_limit_kind: Limit::TokenBytes,
            phase: Phase::Prolog,
            stack: Vec::new(),
            bindings: Vec::new(),
            namespace_index: HashMap::new(),
            namespace_ids: HashMap::new(),
            next_namespace: 0,
            budget: Budget {
                limits,
                owned_bytes: 0,
            },
            events: 0,
        }
    }

    pub(crate) fn set_initial_offset(&mut self, offset: usize) {
        self.initial = offset;
    }

    pub(crate) fn set_encoding(
        &mut self,
        encoding: crate::encoding::SourceEncoding,
        initial: usize,
        decoded_token_limit: usize,
    ) {
        self.encoding = encoding;
        self.set_initial_offset(initial);
        self.token_limit = decoded_token_limit;
        self.token_limit_kind = Limit::DecodedTokenBytes;
    }

    pub(crate) fn in_content(&self) -> bool {
        matches!(self.phase, Phase::Content)
    }

    /// The lending driver must skip non-events before borrowing its token for
    /// the return value. These bytes are necessarily valid UTF-8/XML whitespace.
    pub(crate) fn is_non_event(&self, kind: TokenKind, bytes: &[u8]) -> bool {
        !matches!(kind, TokenKind::Dtd | TokenKind::InvalidMarkup)
            && (bytes.is_empty()
                || (matches!(kind, TokenKind::Text)
                    && !self.in_content()
                    && (!self.encoding.requires_declaration() || self.declaration_seen)
                    && bytes.iter().all(|byte| space(*byte))))
    }

    pub(crate) fn check_event_limit(&self, offset: usize) -> Result<(), Error> {
        if self.events == self.budget.limits.max_events {
            Err(Error::new(ErrorKind::LimitExceeded(Limit::Events), offset))
        } else {
            Ok(())
        }
    }

    pub(crate) fn validate_finish(&self, offset: usize) -> Result<(), Error> {
        if self.encoding.requires_declaration() && !self.declaration_seen {
            return Err(Error::new(ErrorKind::EncodingMismatch, offset));
        }
        match self.phase {
            Phase::Prolog => Err(Error::new(ErrorKind::MissingRoot, offset)),
            Phase::Content => Err(Error::new(ErrorKind::UnexpectedEof, offset)),
            Phase::Epilog => Ok(()),
        }
    }

    pub(crate) fn check_token_start(&self, kind: TokenKind, offset: usize) -> Result<(), Error> {
        match kind {
            TokenKind::Start if matches!(self.phase, Phase::Epilog) => {
                Err(Error::new(ErrorKind::MultipleRoots, offset))
            }
            TokenKind::Start if self.stack.len() >= self.budget.limits.max_depth => {
                Err(Error::new(ErrorKind::LimitExceeded(Limit::Depth), offset))
            }
            TokenKind::Cdata if !self.in_content() => {
                Err(Error::new(ErrorKind::TextOutsideRoot, offset))
            }
            _ => Ok(()),
        }
    }

    /// `retain` borrows slice input or copies streaming structural strings.
    /// Token bytes are already UTF-8 and XML-character validated by the frontend.
    pub(crate) fn process_token<'event, F>(
        &mut self,
        kind: TokenKind,
        raw: &'event str,
        range: Range<usize>,
        mut retain: F,
    ) -> Result<Option<Event<'event>>, Error>
    where
        'source: 'event,
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        let start = range.start;
        if matches!(kind, TokenKind::Dtd | TokenKind::InvalidMarkup) {
            self.check_event_limit(start)?;
            return Err(Error::new(
                if matches!(kind, TokenKind::Dtd) {
                    ErrorKind::UnsupportedDtd
                } else {
                    ErrorKind::Syntax
                },
                start,
            ));
        }
        if raw.len() > self.token_limit {
            return Err(Error::new(
                ErrorKind::LimitExceeded(self.token_limit_kind),
                start,
            ));
        }
        if self.is_non_event(kind, raw.as_bytes()) {
            return Ok(None);
        }
        if self.encoding.requires_declaration()
            && !self.declaration_seen
            && !matches!(kind, TokenKind::Instruction)
        {
            return Err(Error::new(ErrorKind::EncodingMismatch, start));
        }
        if matches!(kind, TokenKind::Text) && !self.in_content() {
            return match raw.bytes().position(|byte| !space(byte)) {
                Some(relative) => Err(Error::new(ErrorKind::TextOutsideRoot, start + relative)),
                None => Err(Error::new(ErrorKind::TextOutsideRoot, start)),
            };
        }
        self.check_event_limit(start)?;
        self.check_token_start(kind, start)?;
        let event = match kind {
            TokenKind::Start => self.start_element(raw, start, &mut retain)?,
            TokenKind::End => self.end_element(raw, start)?,
            TokenKind::Text => {
                if let Some(relative) = raw.find("]]>") {
                    return Err(Error::new(ErrorKind::Syntax, start + relative));
                }
                Event::Text(self.normalized(raw, start, false, true)?)
            }
            TokenKind::Comment => {
                let body = delimited_body(raw, "<!--", "-->", start)?;
                if body.contains("--") || body.ends_with('-') {
                    return Err(Error::new(ErrorKind::Syntax, start));
                }
                Event::Comment(self.normalized(body, start + 4, false, false)?)
            }
            TokenKind::Cdata => {
                if !self.in_content() {
                    return Err(Error::new(ErrorKind::TextOutsideRoot, start));
                }
                let body = delimited_body(raw, "<![CDATA[", "]]>", start)?;
                Event::Cdata(self.normalized(body, start + 9, false, false)?)
            }
            TokenKind::Instruction => self.instruction(raw, start)?,
            TokenKind::Dtd => return Err(Error::new(ErrorKind::UnsupportedDtd, start)),
            TokenKind::InvalidMarkup => return Err(Error::new(ErrorKind::Syntax, start)),
        };
        self.events += 1;
        Ok(Some(event))
    }

    fn normalized<'event>(
        &mut self,
        raw: &'event str,
        offset: usize,
        attribute: bool,
        references: bool,
    ) -> Result<Cow<'event, str>, Error> {
        if needs_normalization(raw, attribute, references) {
            self.budget.charge_owned(raw.len(), offset)?;
        }
        normalize(raw, offset, attribute, references)
    }

    fn instruction<'event>(
        &mut self,
        raw: &'event str,
        start: usize,
    ) -> Result<Event<'event>, Error> {
        let body = delimited_body(raw, "<?", "?>", start)?;
        let name_end = body.bytes().position(space).unwrap_or(body.len());
        let target = &body[..name_end];
        validate_name(target, start + 2, false)?;
        if target.eq_ignore_ascii_case("xml") {
            if target != "xml" || start != self.initial {
                return Err(Error::new(ErrorKind::InvalidDeclaration, start));
            }
            let (declaration, encoding_offset) =
                parse_declaration(&body[name_end..], start + 2 + name_end)?;
            validate_encoding(
                self.encoding,
                declaration.encoding,
                encoding_offset.unwrap_or(start),
            )?;
            self.declaration_seen = true;
            return Ok(Event::Declaration(declaration));
        }
        if self.encoding.requires_declaration() && !self.declaration_seen {
            return Err(Error::new(ErrorKind::EncodingMismatch, start));
        }
        let data = body[name_end..].trim_start_matches([' ', '\t', '\r', '\n']);
        let offset = start + raw.len() - 2 - data.len();
        Ok(Event::ProcessingInstruction {
            target,
            data: self.normalized(data, offset, false, false)?,
        })
    }

    fn start_element<'event, F>(
        &mut self,
        raw: &'event str,
        start: usize,
        retain: &mut F,
    ) -> Result<Event<'event>, Error>
    where
        'source: 'event,
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        if raw.len() < 2 || !raw.starts_with('<') || !raw.ends_with('>') {
            return Err(Error::new(ErrorKind::Syntax, start));
        }
        let empty = raw.as_bytes()[raw.len() - 2] == b'/';
        let content_end = raw.len() - if empty { 2 } else { 1 };
        let mut scan = TagScanner {
            raw: &raw[1..content_end],
            cursor: 0,
            offset: start + 1,
        };
        let qualified = scan.name()?;
        qname(qualified, start + 1)?;
        let bindings_start = self.bindings.len();
        let mut attributes = Vec::new();
        while let Some((name, value, offset, value_offset)) = scan.attribute()? {
            if attributes.len() == self.budget.limits.max_attributes {
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
                let namespace = self.intern_namespace(&value, offset, retain)?;
                self.bindings
                    .try_reserve(1)
                    .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
                let retained_prefix = retain(bound_prefix, offset, &mut self.budget)?;
                let uri = self.retain_value(&value, offset, retain)?;
                let previous = self.namespace_index.get(bound_prefix).copied();
                if let Some(slot) = self.namespace_index.get_mut(bound_prefix) {
                    *slot = self.bindings.len();
                } else {
                    self.namespace_index
                        .try_reserve(1)
                        .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
                    let key = retain(bound_prefix, offset, &mut self.budget)?;
                    self.namespace_index.insert(key, self.bindings.len());
                }
                self.bindings.push(Binding {
                    prefix: retained_prefix,
                    uri,
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
            let qualified = retain(qualified, start + 1, &mut self.budget)?;
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
        reason = "borrowed input and owned normalized values require different retention"
    )]
    fn retain_value<'event, F>(
        &mut self,
        value: &Cow<'event, str>,
        offset: usize,
        retain: &mut F,
    ) -> Result<Cow<'source, str>, Error>
    where
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        match value {
            Cow::Borrowed(raw) => retain(raw, offset, &mut self.budget),
            Cow::Owned(value) => self.budget.copy(value, offset),
        }
    }

    #[allow(
        clippy::ptr_arg,
        reason = "the interner preserves borrowed input and charges owned copies"
    )]
    fn intern_namespace<'event, F>(
        &mut self,
        uri: &Cow<'event, str>,
        offset: usize,
        retain: &mut F,
    ) -> Result<Option<NamespaceId>, Error>
    where
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        if uri.is_empty() {
            return Ok(None);
        }
        if uri == XML {
            return Ok(Some(NamespaceId::Xml));
        }
        if let Some(entry) = self.namespace_ids.get_mut(uri.as_ref()) {
            entry.bindings = entry
                .bindings
                .checked_add(1)
                .ok_or(Error::new(ErrorKind::Allocation, offset))?;
            return Ok(Some(entry.id));
        }
        self.namespace_ids
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
        let key = self.retain_value(uri, offset, retain)?;
        let id = NamespaceId::Declared(self.next_namespace);
        self.next_namespace = self
            .next_namespace
            .checked_add(1)
            .ok_or(Error::new(ErrorKind::Allocation, offset))?;
        self.namespace_ids
            .insert(key, NamespaceEntry { id, bindings: 1 });
        Ok(Some(id))
    }

    fn resolve_name<'event>(
        &mut self,
        qualified: &'event str,
        attribute: bool,
        offset: usize,
    ) -> Result<(Name<'event>, Option<NamespaceId>), Error>
    where
        'source: 'event,
    {
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
                Some(index) if !self.bindings[index].uri.is_empty() => {
                    if let Cow::Owned(uri) = &self.bindings[index].uri {
                        self.budget.charge_owned(uri.len(), offset)?;
                    }
                    (
                        Some(copy_cow(&self.bindings[index].uri, offset)?),
                        self.bindings[index].namespace,
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

    fn end_element<'event>(
        &mut self,
        raw: &'event str,
        start: usize,
    ) -> Result<Event<'event>, Error>
    where
        'source: 'event,
    {
        let body = delimited_body(raw, "</", ">", start)?;
        let mut scan = TagScanner {
            raw: body,
            cursor: 0,
            offset: start + 2,
        };
        let qualified = scan.name()?;
        scan.skip_space();
        if scan.cursor != body.len() {
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
        Ok(Event::End(name))
    }

    fn unwind_bindings(&mut self, start: usize) {
        while self.bindings.len() > start {
            let binding = self.bindings.pop().expect("binding scope");
            if let Some(previous) = binding.previous {
                *self
                    .namespace_index
                    .get_mut(binding.prefix.as_ref())
                    .expect("active namespace prefix") = previous;
            } else {
                self.namespace_index.remove(binding.prefix.as_ref());
            }
            if matches!(binding.namespace, Some(NamespaceId::Declared(_))) {
                let entry = self
                    .namespace_ids
                    .get_mut(binding.uri.as_ref())
                    .expect("active namespace URI");
                entry.bindings -= 1;
                if entry.bindings == 0 {
                    self.namespace_ids.remove(binding.uri.as_ref());
                }
            }
        }
    }
}

fn delimited_body<'a>(
    raw: &'a str,
    prefix: &str,
    suffix: &str,
    offset: usize,
) -> Result<&'a str, Error> {
    raw.strip_prefix(prefix)
        .and_then(|body| body.strip_suffix(suffix))
        .ok_or(Error::new(ErrorKind::Syntax, offset))
}

pub(crate) fn space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
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

fn parse_declaration(raw: &str, offset: usize) -> Result<(Declaration<'_>, Option<usize>), Error> {
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
    let mut encoding_offset = None;
    while let Some((name, value, position, _)) = scan
        .attribute()
        .map_err(|e| Error::new(ErrorKind::InvalidDeclaration, e.offset))?
    {
        match name {
            "version"
                if count == 0
                    && value.strip_prefix("1.").is_some_and(|minor| {
                        !minor.is_empty() && minor.bytes().all(|b| b.is_ascii_digit())
                    }) =>
            {
                declaration.version = value;
            }
            "encoding" if count == 1 => {
                if !value
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphabetic)
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
                {
                    return Err(Error::new(ErrorKind::InvalidDeclaration, position));
                }
                encoding_offset = Some(position);
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
    Ok((declaration, encoding_offset))
}

fn validate_encoding(
    source: crate::encoding::SourceEncoding,
    label: Option<&str>,
    offset: usize,
) -> Result<(), Error> {
    use crate::encoding::SourceEncoding;
    let Some(label) = label else {
        return if source.requires_declaration() {
            Err(Error::new(ErrorKind::EncodingMismatch, offset))
        } else {
            Ok(())
        };
    };
    let utf8 = label.eq_ignore_ascii_case("UTF-8");
    let generic = label.eq_ignore_ascii_case("UTF-16");
    let le = label.eq_ignore_ascii_case("UTF-16LE");
    let be = label.eq_ignore_ascii_case("UTF-16BE");
    let agrees = match source {
        SourceEncoding::Utf8 => utf8,
        SourceEncoding::Utf16 { little_endian, bom } => {
            (bom && generic) || (little_endian && le) || (!little_endian && be)
        }
    };
    if agrees {
        return Ok(());
    }
    let kind = if matches!(source, SourceEncoding::Utf8) || !(utf8 || generic || le || be) {
        ErrorKind::UnsupportedEncoding
    } else {
        ErrorKind::EncodingMismatch
    };
    Err(Error::new(kind, offset))
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

pub(crate) fn validate_name(name: &str, offset: usize, colon: bool) -> Result<(), Error> {
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

#[cfg(test)]
#[path = "../tests/internal/parser.rs"]
mod tests;
