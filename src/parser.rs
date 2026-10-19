use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::ops::Range;

use memchr::{memchr, memmem};

use crate::escape::normalize_replacement_required;
use crate::escape::{needs_normalization, normalize_into};
use crate::name::{qname, validate_name};
use crate::tokenizer::TokenKind;
use crate::{Attribute, Declaration, Element, Error, ErrorKind, Event, Limit, Limits, Name};

const XML: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
/// Namespace scopes up to this many bindings are searched linearly.
const INDEXED_SCOPE: usize = 16;
/// Attribute counts up to this compare expanded names pairwise.
const SMALL_ATTRIBUTES: usize = 8;

/// Shared accounting for normalized strings and retained structural strings.
pub(crate) struct Budget {
    limits: Limits,
    owned_bytes: usize,
}

impl Budget {
    pub(crate) fn new(limits: Limits) -> Self {
        Self {
            limits,
            owned_bytes: 0,
        }
    }

    pub(crate) fn max_events(&self) -> usize {
        self.limits.max_events
    }

    pub(crate) fn max_attributes(&self) -> usize {
        self.limits.max_attributes
    }

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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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

struct PendingBinding<'source> {
    prefix: Cow<'source, str>,
    uri: Cow<'source, str>,
    namespace: Option<NamespaceId>,
    previous: Option<usize>,
}

#[derive(Clone, Copy)]
struct PendingNamespace {
    id: NamespaceId,
    bindings: usize,
    existing: bool,
}

/// An open element's QName: retained by the frontend, or copied into the
/// shared `frame_names` stack when streaming (one buffer, no per-element string).
enum FrameName<'source> {
    Retained(Cow<'source, str>),
    Pooled(Range<usize>),
}

struct Frame<'source> {
    qualified: FrameName<'source>,
    bindings_start: usize,
}

#[derive(Debug)]
pub(crate) struct RawAttribute<'event> {
    pub(crate) qualified: &'event str,
    pub(crate) prefix_length: Option<NonZeroUsize>,
    pub(crate) literal: &'event str,
    pub(crate) name_offset: usize,
    pub(crate) value_offset: usize,
    pub(crate) value_end: usize,
}

#[derive(Debug)]
pub(crate) struct RawTag<'event> {
    pub(crate) qualified: &'event str,
    pub(crate) prefix_length: Option<NonZeroUsize>,
    pub(crate) attributes: Vec<RawAttribute<'event>>,
    pub(crate) empty: bool,
    pub(crate) start: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AttributeProvenance {
    Specified {
        name_offset: usize,
        value_offset: usize,
    },
    Defaulted {
        declaration: usize,
    },
}

impl AttributeProvenance {
    pub(crate) fn offset(self, _fallback: usize) -> usize {
        match self {
            Self::Specified { name_offset, .. } => name_offset,
            Self::Defaulted { .. } => _fallback,
        }
    }
}

#[derive(Debug)]
pub(crate) struct PreparedAttribute<'event> {
    pub(crate) name: Name<'event>,
    pub(crate) value: Cow<'event, str>,
    pub(crate) provenance: AttributeProvenance,
}

#[derive(Debug)]
pub(crate) struct PreparedTag<'event> {
    pub(crate) name: Name<'event>,
    pub(crate) attributes: Vec<PreparedAttribute<'event>>,
    pub(crate) empty: bool,
    pub(crate) start: usize,
}

impl PreparedTag<'_> {
    fn try_into_static(self) -> Result<PreparedTag<'static>, Error> {
        let start = self.start;
        let name = self.name.try_into_static(start + 1)?;
        let mut attributes = Vec::new();
        attributes
            .try_reserve(self.attributes.len())
            .map_err(|_| Error::new(ErrorKind::Allocation, start))?;
        for attribute in self.attributes {
            let offset = attribute.provenance.offset(start);
            let name = attribute.name.try_into_static(offset)?;
            let Cow::Owned(value) = attribute.value else {
                return Err(Error::new(ErrorKind::InvalidState, offset));
            };
            attributes.push(PreparedAttribute {
                name,
                value: Cow::Owned(value),
                provenance: attribute.provenance,
            });
        }
        Ok(PreparedTag {
            name,
            attributes,
            empty: self.empty,
            start,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DtdLeafKind {
    Text,
    Cdata,
    Comment,
    ProcessingInstruction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReplacementError {
    pub(crate) kind: ErrorKind,
    pub(crate) local_offset: usize,
}

impl From<Error> for ReplacementError {
    fn from(error: Error) -> Self {
        Self {
            kind: error.kind,
            local_offset: error.offset,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreparedTagErrorLocation {
    Input(usize),
    Declaration { declaration: usize, offset: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PreparedTagError {
    pub(crate) kind: ErrorKind,
    pub(crate) location: PreparedTagErrorLocation,
}

impl PreparedTagError {
    fn input(error: Error) -> Self {
        Self {
            kind: error.kind,
            location: PreparedTagErrorLocation::Input(error.offset),
        }
    }

    fn attribute(error: Error, provenance: AttributeProvenance) -> Self {
        let location = match provenance {
            AttributeProvenance::Specified { name_offset, .. } => {
                PreparedTagErrorLocation::Input(name_offset)
            }
            AttributeProvenance::Defaulted { declaration } => {
                PreparedTagErrorLocation::Declaration {
                    declaration,
                    offset: 0,
                }
            }
        };
        Self {
            kind: error.kind,
            location,
        }
    }
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
    switchable: bool,
    declaration_seen: bool,
    doctype_seen: bool,
    token_limit: usize,
    token_limit_kind: Limit,
    phase: Phase,
    stack: Vec<Frame<'source>>,
    frame_names: String,
    /// Empty attribute list whose capacity a callback frontend returned.
    attribute_storage: Vec<Attribute<'static>>,
    copies_names: bool,
    bindings: Vec<Binding<'source>>,
    /// Prefix -> active binding, maintained only above `INDEXED_SCOPE`
    /// bindings; smaller scopes search `bindings` from the innermost entry.
    namespace_index: HashMap<Cow<'source, str>, usize>,
    indexed: bool,
    namespace_ids: HashMap<Cow<'source, str>, NamespaceEntry>,
    next_namespace: usize,
    budget: Budget,
    events: usize,
}

impl<'source> ParserState<'source> {
    pub(crate) fn owned_budget(&mut self) -> &mut Budget {
        &mut self.budget
    }

    pub(crate) fn element_depth(&self) -> usize {
        self.stack.len()
    }

    pub(crate) fn new(limits: Limits, initial_offset: usize) -> Self {
        Self {
            initial: initial_offset,
            encoding: crate::encoding::SourceEncoding::Utf8,
            switchable: false,
            declaration_seen: false,
            doctype_seen: false,
            token_limit: limits.max_token_bytes,
            token_limit_kind: Limit::TokenBytes,
            phase: Phase::Prolog,
            stack: Vec::new(),
            frame_names: String::new(),
            attribute_storage: Vec::new(),
            copies_names: false,
            bindings: Vec::new(),
            namespace_index: HashMap::new(),
            indexed: false,
            namespace_ids: HashMap::new(),
            next_namespace: 0,
            budget: Budget::new(limits),
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

    /// Streaming frontends copy every retained name. Open element names then
    /// share one charged buffer, and unindexed prefixes are charged directly.
    pub(crate) fn copying_names(mut self) -> Self {
        self.copies_names = true;
        self
    }

    /// The active binding index for `prefix`.
    fn bound(&self, prefix: &str) -> Option<usize> {
        if self.indexed {
            self.namespace_index.get(prefix).copied()
        } else {
            self.bindings
                .iter()
                .rposition(|binding| binding.prefix == prefix)
        }
    }

    /// The interned identity of an active URI and its binding count (counts
    /// are kept only while indexed; linear scopes search the binding stack).
    fn interned(&self, uri: &str) -> Option<(NamespaceId, usize)> {
        if self.indexed {
            return self
                .namespace_ids
                .get(uri)
                .map(|entry| (entry.id, entry.bindings));
        }
        self.bindings
            .iter()
            .rev()
            .find_map(|binding| match binding.namespace {
                Some(id @ NamespaceId::Declared(_)) if binding.uri == uri => Some((id, 0)),
                _ => None,
            })
    }

    /// Builds the prefix index once a scope outgrows linear search. Keys
    /// were charged when their bindings were retained.
    fn index_bindings(&mut self, offset: usize) -> Result<(), Error> {
        if self.indexed || self.bindings.len() <= INDEXED_SCOPE {
            return Ok(());
        }
        let allocation = || Error::new(ErrorKind::Allocation, offset);
        let key = |value: &Cow<'source, str>| -> Result<Cow<'source, str>, Error> {
            Ok(match value {
                Cow::Borrowed(value) => Cow::Borrowed(*value),
                Cow::Owned(value) => {
                    let mut key = String::new();
                    key.try_reserve(value.len()).map_err(|_| allocation())?;
                    key.push_str(value);
                    Cow::Owned(key)
                }
            })
        };
        let mut index = HashMap::new();
        let mut ids: HashMap<Cow<'source, str>, NamespaceEntry> = HashMap::new();
        index
            .try_reserve(self.bindings.len())
            .map_err(|_| allocation())?;
        ids.try_reserve(self.bindings.len())
            .map_err(|_| allocation())?;
        for (position, binding) in self.bindings.iter().enumerate() {
            index.insert(key(&binding.prefix)?, position);
            if let Some(id @ NamespaceId::Declared(_)) = binding.namespace {
                if let Some(entry) = ids.get_mut(binding.uri.as_ref()) {
                    entry.bindings += 1;
                } else {
                    ids.insert(key(&binding.uri)?, NamespaceEntry { id, bindings: 1 });
                }
            }
        }
        self.namespace_index = index;
        self.namespace_ids = ids;
        self.indexed = true;
        Ok(())
    }

    /// Reuses returned attribute capacity. Both conversions collect in place:
    /// the vector is empty, so only its allocation changes lifetime.
    fn take_attributes<'event>(&mut self) -> Vec<Attribute<'event>> {
        std::mem::take(&mut self.attribute_storage)
            .into_iter()
            .map(|_| unreachable!("stored attribute lists are empty"))
            .collect()
    }

    /// Keeps the capacity of a list whose event has been delivered.
    pub(crate) fn recycle_attributes(&mut self, mut attributes: Vec<Attribute<'_>>) {
        if attributes.capacity() <= self.attribute_storage.capacity() {
            return;
        }
        attributes.clear();
        self.attribute_storage = attributes
            .into_iter()
            .map(|_| unreachable!("cleared above"))
            .collect();
    }

    fn open_name(&self) -> Option<&str> {
        self.stack.last().map(|frame| match &frame.qualified {
            FrameName::Retained(name) => name.as_ref(),
            FrameName::Pooled(range) => &self.frame_names[range.clone()],
        })
    }

    fn pop_frame(&mut self) -> Frame<'source> {
        let frame = self.stack.pop().expect("matched frame");
        if let FrameName::Pooled(range) = &frame.qualified {
            self.frame_names.truncate(range.start);
        }
        frame
    }

    /// A BOM-less decoding stream can follow a declared byte charset.
    pub(crate) fn allow_switch(&mut self) {
        self.switchable = true;
    }

    pub(crate) fn encoding(&self) -> crate::encoding::SourceEncoding {
        self.encoding
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

    pub(crate) fn check_doctype_start(&self, offset: usize) -> Result<(), Error> {
        if self.encoding.requires_declaration() && !self.declaration_seen {
            return Err(Error::new(ErrorKind::EncodingMismatch, offset));
        }
        if self.doctype_seen || !matches!(self.phase, Phase::Prolog) {
            return Err(Error::new(ErrorKind::Syntax, offset));
        }
        self.check_event_limit(offset)
    }

    pub(crate) fn commit_doctype(&mut self) {
        debug_assert!(self.events < self.budget.limits.max_events);
        self.doctype_seen = true;
        self.events += 1;
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
            TokenKind::Start if self.element_depth() >= self.budget.limits.max_depth => {
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
            return Err(Error::new(ErrorKind::Syntax, start));
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
        // Every arm yields the result type returned here, so a tag's event
        // passes through without being re-laid-out.
        let event = match kind {
            TokenKind::Start => self.start_element(raw, start, &mut retain),
            TokenKind::End => self.end_element(raw, start),
            TokenKind::Text => {
                if let Some(relative) = memmem::find(raw.as_bytes(), b"]]>") {
                    return Err(Error::new(ErrorKind::Syntax, start + relative));
                }
                Ok(Some(Event::Text(self.normalized(raw, start, false, true)?)))
            }
            TokenKind::Comment => {
                let body = delimited_body(raw, "<!--", "-->", start)?;
                if memmem::find(body.as_bytes(), b"--").is_some() || body.ends_with('-') {
                    return Err(Error::new(ErrorKind::Syntax, start));
                }
                Ok(Some(Event::Comment(self.normalized(
                    body,
                    start + 4,
                    false,
                    false,
                )?)))
            }
            TokenKind::Cdata => {
                if !self.in_content() {
                    return Err(Error::new(ErrorKind::TextOutsideRoot, start));
                }
                let body = delimited_body(raw, "<![CDATA[", "]]>", start)?;
                Ok(Some(Event::Cdata(self.normalized(
                    body,
                    start + 9,
                    false,
                    false,
                )?)))
            }
            TokenKind::Instruction => self.instruction(raw, start),
            TokenKind::Dtd | TokenKind::InvalidMarkup => {
                return Err(Error::new(ErrorKind::Syntax, start));
            }
        };
        if event.is_ok() {
            self.events += 1;
        }
        event
    }

    fn normalized<'event>(
        &mut self,
        raw: &'event str,
        offset: usize,
        attribute: bool,
        references: bool,
    ) -> Result<Cow<'event, str>, Error> {
        if !needs_normalization(raw, attribute, references) {
            return Ok(Cow::Borrowed(raw));
        }
        self.budget.charge_owned(raw.len(), offset)?;
        normalize_into(raw, offset, attribute, references, String::new()).map(Cow::Owned)
    }

    fn instruction<'event>(
        &mut self,
        raw: &'event str,
        start: usize,
    ) -> Result<Option<Event<'event>>, Error> {
        let body = delimited_body(raw, "<?", "?>", start)?;
        let name_end = body.bytes().position(space).unwrap_or(body.len());
        let target = &body[..name_end];
        validate_name(target, start + 2, false)?;
        if target.eq_ignore_ascii_case("xml") {
            if target != "xml" || start != self.initial {
                return Err(Error::new(ErrorKind::InvalidDeclaration, start));
            }
            let (declaration, encoding_offset) =
                parse_declaration(&body[name_end..], start + 2 + name_end, false)?;
            self.encoding = validate_encoding(
                self.encoding,
                self.switchable,
                declaration.encoding,
                encoding_offset.unwrap_or(start),
            )?;
            self.declaration_seen = true;
            return Ok(Some(Event::Declaration(declaration)));
        }
        if self.encoding.requires_declaration() && !self.declaration_seen {
            return Err(Error::new(ErrorKind::EncodingMismatch, start));
        }
        let data = body[name_end..].trim_start_matches([' ', '\t', '\r', '\n']);
        let offset = start + raw.len() - 2 - data.len();
        Ok(Some(Event::ProcessingInstruction {
            target: Cow::Borrowed(target),
            data: self.normalized(data, offset, false, false)?,
        }))
    }

    fn start_element<'event, F>(
        &mut self,
        raw: &'event str,
        start: usize,
        retain: &mut F,
    ) -> Result<Option<Event<'event>>, Error>
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
        let (qualified, split) = scan.scan_name()?;
        let (element_prefix, _) = split_qname(qualified, split, start + 1)?;
        let element_prefix = element_prefix.and_then(|prefix| NonZeroUsize::new(prefix.len()));
        let bindings_start = self.bindings.len();
        let mut attributes = self.take_attributes();
        while let Some(attribute) = scan.attribute()? {
            let name = attribute.qualified;
            let value = attribute.literal;
            let offset = attribute.name_offset;
            let value_offset = attribute.value_offset;
            if attributes.len() == self.budget.limits.max_attributes {
                return Err(Error::new(
                    ErrorKind::LimitExceeded(Limit::Attributes),
                    offset,
                ));
            }
            let (prefix, local) = split_qname(name, attribute.split, offset)?;
            let value = self.normalized(value, value_offset, true, true)?;
            let declaration = if name == "xmlns" {
                Some("")
            } else if prefix == Some("xmlns") {
                Some(local)
            } else {
                None
            };
            if let Some(bound_prefix) = declaration {
                if invalid_namespace_binding(bound_prefix, &value) {
                    return Err(Error::new(ErrorKind::Namespace, offset));
                }
                let namespace = self.intern_namespace(&value, offset, retain)?;
                self.bindings
                    .try_reserve(1)
                    .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
                let retained_prefix = retain(bound_prefix, offset, &mut self.budget)?;
                let uri = self.retain_value(&value, offset, retain)?;
                let previous = self.bound(bound_prefix);
                if !self.indexed {
                    if previous.is_none() && self.copies_names {
                        self.budget.charge_owned(bound_prefix.len(), offset)?;
                    }
                } else if let Some(slot) = self.namespace_index.get_mut(bound_prefix) {
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
                self.index_bindings(offset)?;
            }
            attributes
                .try_reserve(1)
                .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
            attributes.push(Attribute {
                name: Name::from_validated_parts(
                    Cow::Borrowed(name),
                    prefix.and_then(|prefix| std::num::NonZeroUsize::new(prefix.len())),
                    None,
                ),
                value,
            });
        }
        let name = self.resolve_name(qualified, element_prefix, start + 1)?;
        // Expanded-name uniqueness: equal URIs have equal namespace IDs, so
        // short lists compare resolved names directly instead of hashing.
        if attributes.len() <= SMALL_ATTRIBUTES {
            for index in 0..attributes.len() {
                self.resolve_namespace(&mut attributes[index].name, true, start)?;
                let (prior, rest) = attributes.split_at(index);
                let name = &rest[0].name;
                if prior.iter().any(|other| {
                    other.name.local() == name.local() && other.name.namespace() == name.namespace()
                }) {
                    return Err(Error::new(ErrorKind::DuplicateAttribute, start));
                }
            }
        } else {
            let mut expanded = HashSet::new();
            expanded
                .try_reserve(attributes.len())
                .map_err(|_| Error::new(ErrorKind::Allocation, start))?;
            for attribute in &mut attributes {
                let namespace = self.resolve_namespace(&mut attribute.name, true, start)?;
                if !expanded.insert((attribute.name.local(), namespace)) {
                    return Err(Error::new(ErrorKind::DuplicateAttribute, start));
                }
            }
        }
        let element = Element {
            name,
            attributes: attributes.into(),
        };
        if empty {
            self.unwind_bindings(bindings_start);
            if self.stack.is_empty() {
                self.phase = Phase::Epilog;
            }
            Ok(Some(Event::Empty(element)))
        } else {
            self.stack
                .try_reserve(1)
                .map_err(|_| Error::new(ErrorKind::Allocation, start))?;
            let qualified = if self.copies_names {
                self.budget.charge_owned(qualified.len(), start + 1)?;
                self.frame_names
                    .try_reserve(qualified.len())
                    .map_err(|_| Error::new(ErrorKind::Allocation, start + 1))?;
                let begin = self.frame_names.len();
                self.frame_names.push_str(qualified);
                FrameName::Pooled(begin..self.frame_names.len())
            } else {
                FrameName::Retained(retain(qualified, start + 1, &mut self.budget)?)
            };
            self.stack.push(Frame {
                qualified,
                bindings_start,
            });
            self.phase = Phase::Content;
            Ok(Some(Event::Start(element)))
        }
    }

    pub(crate) fn process_dtd_start<'event, F>(
        &mut self,
        raw: &'event str,
        start: usize,
        catalog: &mut crate::dtd_catalog::DtdCatalog,
        source: crate::dtd::LiteralSource,
        retain: &mut F,
    ) -> Result<Event<'event>, crate::dtd_tag::DtdTagError>
    where
        'source: 'event,
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        self.process_dtd_start_policy(raw, start, catalog, source, false, retain)
    }

    pub(crate) fn process_stable_dtd_start<'event, F>(
        &mut self,
        raw: &'event str,
        start: usize,
        catalog: &mut crate::dtd_catalog::DtdCatalog,
        retain: &mut F,
    ) -> Result<Event<'event>, crate::dtd_tag::DtdTagError>
    where
        'source: 'event,
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        self.process_dtd_start_policy(
            raw,
            start,
            catalog,
            crate::dtd::LiteralSource::Physical,
            true,
            retain,
        )
    }

    fn process_dtd_start_policy<'event, F>(
        &mut self,
        raw: &'event str,
        start: usize,
        catalog: &mut crate::dtd_catalog::DtdCatalog,
        source: crate::dtd::LiteralSource,
        borrow_physical: bool,
        retain: &mut F,
    ) -> Result<Event<'event>, crate::dtd_tag::DtdTagError>
    where
        'source: 'event,
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        if raw.len() > self.token_limit {
            return Err(Error::new(ErrorKind::LimitExceeded(self.token_limit_kind), start).into());
        }
        if self.encoding.requires_declaration() && !self.declaration_seen {
            return Err(Error::new(ErrorKind::EncodingMismatch, start).into());
        }
        self.check_event_limit(start)?;
        self.check_token_start(TokenKind::Start, start)?;
        let raw = self.collect_raw_tag(raw, start)?;
        let prepared = if borrow_physical {
            crate::dtd_tag::prepare_stable_dtd_tag(catalog, &mut self.budget, raw)?
        } else {
            crate::dtd_tag::prepare_dtd_tag(catalog, &mut self.budget, raw, source)?
        };
        let event = self.publish_prepared_tag(prepared, retain)?;
        self.events += 1;
        Ok(event)
    }

    pub(crate) fn process_dtd_start_owned(
        &mut self,
        raw: &str,
        start: usize,
        catalog: &mut crate::dtd_catalog::DtdCatalog,
    ) -> Result<Event<'static>, crate::dtd_tag::DtdTagError> {
        if raw.len() > self.token_limit {
            return Err(Error::new(ErrorKind::LimitExceeded(self.token_limit_kind), start).into());
        }
        if self.encoding.requires_declaration() && !self.declaration_seen {
            return Err(Error::new(ErrorKind::EncodingMismatch, start).into());
        }
        self.check_event_limit(start)?;
        self.check_token_start(TokenKind::Start, start)?;
        let raw = self.collect_raw_tag(raw, start)?;
        let prepared = crate::dtd_tag::prepare_dtd_tag(
            catalog,
            &mut self.budget,
            raw,
            crate::dtd::LiteralSource::Replacement,
        )?
        .try_into_static()?;
        let event = self.publish_prepared_tag_owned(prepared)?;
        self.events += 1;
        Ok(event)
    }

    pub(crate) fn process_dtd_leaf_owned(
        &mut self,
        kind: DtdLeafKind,
        raw: &str,
        local_range: Range<usize>,
    ) -> Result<Event<'static>, ReplacementError> {
        self.process_dtd_leaf_owned_inner(kind, raw, local_range)
            .map_err(ReplacementError::from)
    }

    fn process_dtd_leaf_owned_inner(
        &mut self,
        kind: DtdLeafKind,
        raw: &str,
        local_range: Range<usize>,
    ) -> Result<Event<'static>, Error> {
        let start = local_range.start;
        if local_range.end.checked_sub(start) != Some(raw.len()) {
            return Err(Error::new(ErrorKind::InvalidState, start));
        }
        if raw.len() > self.token_limit {
            return Err(Error::new(
                ErrorKind::LimitExceeded(self.token_limit_kind),
                start,
            ));
        }
        if self.encoding.requires_declaration() && !self.declaration_seen {
            return Err(Error::new(ErrorKind::EncodingMismatch, start));
        }
        if matches!(kind, DtdLeafKind::Text) && !self.in_content() {
            let offset = raw
                .bytes()
                .position(|byte| !space(byte))
                .map_or(start, |relative| start + relative);
            return Err(Error::new(ErrorKind::TextOutsideRoot, offset));
        }
        self.check_event_limit(start)?;
        let token_kind = match kind {
            DtdLeafKind::Text => TokenKind::Text,
            DtdLeafKind::Cdata => TokenKind::Cdata,
            DtdLeafKind::Comment => TokenKind::Comment,
            DtdLeafKind::ProcessingInstruction => TokenKind::Instruction,
        };
        self.check_token_start(token_kind, start)?;
        let event = match kind {
            DtdLeafKind::Text => {
                if let Some(relative) = memmem::find(raw.as_bytes(), b"]]>") {
                    return Err(Error::new(ErrorKind::Syntax, start + relative));
                }
                Event::Text(self.normalized_replacement_owned(raw, start)?)
            }
            DtdLeafKind::Comment => {
                let body = delimited_body(raw, "<!--", "-->", start)?;
                if memmem::find(body.as_bytes(), b"--").is_some() || body.ends_with('-') {
                    return Err(Error::new(ErrorKind::Syntax, start));
                }
                Event::Comment(self.budget.copy(body, start + 4)?)
            }
            DtdLeafKind::Cdata => {
                let body = delimited_body(raw, "<![CDATA[", "]]>", start)?;
                Event::Cdata(self.budget.copy(body, start + 9)?)
            }
            DtdLeafKind::ProcessingInstruction => {
                let body = delimited_body(raw, "<?", "?>", start)?;
                let name_end = body.bytes().position(space).unwrap_or(body.len());
                let target = &body[..name_end];
                validate_name(target, start + 2, false)?;
                if target.eq_ignore_ascii_case("xml") {
                    return Err(Error::new(ErrorKind::InvalidDeclaration, start));
                }
                let data = body[name_end..].trim_start_matches([' ', '\t', '\r', '\n']);
                let offset = start + raw.len() - 2 - data.len();
                Event::ProcessingInstruction {
                    target: self.budget.copy(target, start + 2)?,
                    data: self.budget.copy(data, offset)?,
                }
            }
        };
        self.events += 1;
        Ok(event)
    }

    fn normalized_replacement_owned(
        &mut self,
        raw: &str,
        offset: usize,
    ) -> Result<Cow<'static, str>, Error> {
        if !raw.as_bytes().contains(&b'&') {
            return self.budget.copy(raw, offset);
        }
        self.budget.charge_owned(raw.len(), offset)?;
        normalize_replacement_required(raw, offset).map(Cow::Owned)
    }

    pub(crate) fn process_dtd_end_owned(
        &mut self,
        raw: &str,
        start: usize,
    ) -> Result<Event<'static>, ReplacementError> {
        self.process_dtd_end_owned_inner(raw, start)
            .map_err(ReplacementError::from)
    }

    fn process_dtd_end_owned_inner(
        &mut self,
        raw: &str,
        start: usize,
    ) -> Result<Event<'static>, Error> {
        if raw.len() > self.token_limit {
            return Err(Error::new(
                ErrorKind::LimitExceeded(self.token_limit_kind),
                start,
            ));
        }
        if self.encoding.requires_declaration() && !self.declaration_seen {
            return Err(Error::new(ErrorKind::EncodingMismatch, start));
        }
        self.check_event_limit(start)?;
        self.check_token_start(TokenKind::End, start)?;
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
        if self.open_name() != Some(qualified) {
            return Err(Error::new(ErrorKind::MismatchedEndTag, start));
        }
        let name = self.resolve_end_name_owned(qualified, start + 2)?;
        let frame = self.pop_frame();
        self.unwind_bindings(frame.bindings_start);
        if self.stack.is_empty() {
            self.phase = Phase::Epilog;
        }
        self.events += 1;
        Ok(Event::End(name))
    }

    pub(crate) fn collect_raw_tag<'event>(
        &self,
        raw: &'event str,
        start: usize,
    ) -> Result<RawTag<'event>, Error> {
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
        let (prefix, _) = qname(qualified, start + 1)?;
        let prefix_length = prefix.and_then(|value| NonZeroUsize::new(value.len()));
        let mut attributes = Vec::new();
        let mut names = HashSet::new();
        while let Some(attribute) = scan.attribute()? {
            let name = attribute.qualified;
            let offset = attribute.name_offset;
            if attributes.len() == self.budget.max_attributes() {
                return Err(Error::new(
                    ErrorKind::LimitExceeded(Limit::Attributes),
                    offset,
                ));
            }
            let (prefix, _) = qname(name, offset)?;
            if names.len() == names.capacity() {
                names
                    .try_reserve(1)
                    .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
            }
            if !names.insert(name) {
                return Err(Error::new(ErrorKind::DuplicateAttribute, offset));
            }
            attributes
                .try_reserve(1)
                .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
            attributes.push(RawAttribute {
                qualified: name,
                prefix_length: prefix.and_then(|value| NonZeroUsize::new(value.len())),
                literal: attribute.literal,
                name_offset: offset,
                value_offset: attribute.value_offset,
                value_end: attribute.value_end,
            });
        }
        Ok(RawTag {
            qualified,
            prefix_length,
            attributes,
            empty,
            start,
        })
    }

    pub(crate) fn publish_prepared_tag<'event, F>(
        &mut self,
        prepared: PreparedTag<'event>,
        retain: &mut F,
    ) -> Result<Event<'event>, PreparedTagError>
    where
        'source: 'event,
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        self.publish_prepared_tag_with(prepared, retain, &mut |uri, offset, budget| match uri {
            Cow::Borrowed(uri) => Ok(Cow::Borrowed(*uri)),
            Cow::Owned(uri) => budget.copy(uri, offset),
        })
    }

    fn publish_prepared_tag_owned(
        &mut self,
        prepared: PreparedTag<'static>,
    ) -> Result<Event<'static>, PreparedTagError> {
        self.publish_prepared_tag_with(
            prepared,
            &mut |_, offset, _| Err(Error::new(ErrorKind::InvalidState, offset)),
            &mut |uri, offset, budget| budget.copy(uri, offset),
        )
    }

    fn publish_prepared_tag_with<'event, F, N>(
        &mut self,
        mut prepared: PreparedTag<'event>,
        retain: &mut F,
        namespace_output: &mut N,
    ) -> Result<Event<'event>, PreparedTagError>
    where
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
        N: FnMut(&Cow<'source, str>, usize, &mut Budget) -> Result<Cow<'event, str>, Error>,
    {
        let bindings_start = self.bindings.len();
        let mut next_namespace = self.next_namespace;
        let mut pending = Vec::new();
        let mut pending_prefixes: HashMap<Cow<'source, str>, usize> = HashMap::new();
        let mut pending_namespaces: HashMap<Cow<'source, str>, PendingNamespace> = HashMap::new();
        let namespace_declarations = prepared
            .attributes
            .iter()
            .filter(|attribute| {
                attribute.name.qualified() == "xmlns" || attribute.name.prefix() == Some("xmlns")
            })
            .count();
        pending.try_reserve(namespace_declarations).map_err(|_| {
            PreparedTagError::input(Error::new(ErrorKind::Allocation, prepared.start))
        })?;
        pending_prefixes
            .try_reserve(namespace_declarations)
            .map_err(|_| {
                PreparedTagError::input(Error::new(ErrorKind::Allocation, prepared.start))
            })?;
        pending_namespaces
            .try_reserve(namespace_declarations)
            .map_err(|_| {
                PreparedTagError::input(Error::new(ErrorKind::Allocation, prepared.start))
            })?;
        for attribute in &prepared.attributes {
            let name = &attribute.name;
            let declaration = if name.qualified() == "xmlns" {
                Some(("", 0..0))
            } else if name.prefix() == Some("xmlns") {
                let local = name.local();
                Some((
                    local,
                    name.qualified().len() - local.len()..name.qualified().len(),
                ))
            } else {
                None
            };
            if let Some((bound_prefix, prefix_range)) = declaration {
                if invalid_namespace_binding(bound_prefix, &attribute.value) {
                    return Err(PreparedTagError::attribute(
                        Error::new(
                            ErrorKind::Namespace,
                            attribute.provenance.offset(prepared.start),
                        ),
                        attribute.provenance,
                    ));
                }
                let offset = attribute.provenance.offset(prepared.start);
                let namespace = if attribute.value.is_empty() {
                    None
                } else if attribute.value == XML {
                    Some(NamespaceId::Xml)
                } else if let Some(namespace) = pending_namespaces.get_mut(attribute.value.as_ref())
                {
                    namespace.bindings = namespace.bindings.checked_add(1).ok_or_else(|| {
                        PreparedTagError::attribute(
                            Error::new(ErrorKind::Allocation, offset),
                            attribute.provenance,
                        )
                    })?;
                    Some(namespace.id)
                } else {
                    let (id, bindings, existing) =
                        if let Some((id, count)) = self.interned(attribute.value.as_ref()) {
                            let bindings = count.checked_add(1).ok_or_else(|| {
                                PreparedTagError::attribute(
                                    Error::new(ErrorKind::Allocation, offset),
                                    attribute.provenance,
                                )
                            })?;
                            (id, bindings, true)
                        } else {
                            let id = NamespaceId::Declared(next_namespace);
                            next_namespace = next_namespace.checked_add(1).ok_or_else(|| {
                                PreparedTagError::attribute(
                                    Error::new(ErrorKind::Allocation, offset),
                                    attribute.provenance,
                                )
                            })?;
                            (id, 1, false)
                        };
                    let key = self
                        .retain_value(&attribute.value, offset, retain)
                        .map_err(|error| {
                            PreparedTagError::attribute(error, attribute.provenance)
                        })?;
                    pending_namespaces.insert(
                        key,
                        PendingNamespace {
                            id,
                            bindings,
                            existing,
                        },
                    );
                    Some(id)
                };
                let prefix_key = self
                    .retain_name_range(
                        name.qualified_source(),
                        prefix_range.clone(),
                        offset,
                        retain,
                    )
                    .map_err(|error| PreparedTagError::attribute(error, attribute.provenance))?;
                let prefix = self
                    .retain_name_range(
                        name.qualified_source(),
                        prefix_range.clone(),
                        offset,
                        retain,
                    )
                    .map_err(|error| PreparedTagError::attribute(error, attribute.provenance))?;
                let uri = self
                    .retain_value(&attribute.value, offset, retain)
                    .map_err(|error| PreparedTagError::attribute(error, attribute.provenance))?;
                let previous = self.bound(bound_prefix);
                let index = pending.len();
                let prior = pending_prefixes.insert(prefix_key, index);
                debug_assert!(prior.is_none());
                pending.push(PendingBinding {
                    prefix,
                    uri,
                    namespace,
                    previous,
                });
            }
        }

        self.resolve_namespace_with_pending(
            &mut prepared.name,
            false,
            prepared.start + 1,
            &pending,
            &pending_prefixes,
            namespace_output,
        )
        .map_err(PreparedTagError::input)?;
        let mut expanded = HashSet::new();
        expanded
            .try_reserve(prepared.attributes.len())
            .map_err(|_| {
                PreparedTagError::input(Error::new(ErrorKind::Allocation, prepared.start))
            })?;
        for attribute in &mut prepared.attributes {
            let offset = attribute.provenance.offset(prepared.start);
            let namespace = self
                .resolve_namespace_with_pending(
                    &mut attribute.name,
                    true,
                    offset,
                    &pending,
                    &pending_prefixes,
                    namespace_output,
                )
                .map_err(|error| PreparedTagError::attribute(error, attribute.provenance))?;
            if !expanded.insert((attribute.name.local(), namespace)) {
                return Err(PreparedTagError::attribute(
                    Error::new(ErrorKind::DuplicateAttribute, offset),
                    attribute.provenance,
                ));
            }
        }
        let qualified = if prepared.empty {
            None
        } else {
            Some(
                self.retain_value(prepared.name.qualified_source(), prepared.start + 1, retain)
                    .map_err(PreparedTagError::input)?,
            )
        };
        let mut attributes = Vec::new();
        attributes
            .try_reserve(prepared.attributes.len())
            .map_err(|_| {
                PreparedTagError::input(Error::new(ErrorKind::Allocation, prepared.start))
            })?;
        for attribute in prepared.attributes {
            attributes.push(Attribute {
                name: attribute.name,
                value: attribute.value,
            });
        }
        let element = Element {
            name: prepared.name,
            attributes: attributes.into(),
        };
        self.bindings.try_reserve(pending.len()).map_err(|_| {
            PreparedTagError::input(Error::new(ErrorKind::Allocation, prepared.start))
        })?;
        self.namespace_index
            .try_reserve(if self.indexed {
                pending
                    .iter()
                    .filter(|binding| binding.previous.is_none())
                    .count()
            } else {
                0
            })
            .map_err(|_| {
                PreparedTagError::input(Error::new(ErrorKind::Allocation, prepared.start))
            })?;
        self.namespace_ids
            .try_reserve(if self.indexed {
                pending_namespaces
                    .values()
                    .filter(|namespace| !namespace.existing)
                    .count()
            } else {
                0
            })
            .map_err(|_| {
                PreparedTagError::input(Error::new(ErrorKind::Allocation, prepared.start))
            })?;
        if !prepared.empty {
            self.stack.try_reserve(1).map_err(|_| {
                PreparedTagError::input(Error::new(ErrorKind::Allocation, prepared.start))
            })?;
        }

        self.next_namespace = next_namespace;
        // Linear scopes find identities on the binding stack instead.
        let pending_namespaces = pending_namespaces.into_iter().filter(|_| self.indexed);
        for (key, namespace) in pending_namespaces {
            if namespace.existing {
                self.namespace_ids
                    .get_mut(key.as_ref())
                    .expect("planned existing namespace")
                    .bindings = namespace.bindings;
            } else {
                self.namespace_ids.insert(
                    key,
                    NamespaceEntry {
                        id: namespace.id,
                        bindings: namespace.bindings,
                    },
                );
            }
        }
        for binding in pending {
            let (prefix_key, _) = pending_prefixes
                .remove_entry(binding.prefix.as_ref())
                .expect("planned namespace prefix");
            let index = self.bindings.len();
            if !self.indexed {
                self.bindings.push(Binding {
                    prefix: binding.prefix,
                    uri: binding.uri,
                    namespace: binding.namespace,
                    previous: binding.previous,
                });
            } else if let Some(previous) = binding.previous {
                *self
                    .namespace_index
                    .get_mut(binding.prefix.as_ref())
                    .expect("planned existing prefix") = index;
                self.bindings.push(Binding {
                    prefix: binding.prefix,
                    uri: binding.uri,
                    namespace: binding.namespace,
                    previous: Some(previous),
                });
            } else {
                self.namespace_index.insert(prefix_key, index);
                self.bindings.push(Binding {
                    prefix: binding.prefix,
                    uri: binding.uri,
                    namespace: binding.namespace,
                    previous: None,
                });
            }
        }
        self.index_bindings(prepared.start)
            .map_err(PreparedTagError::input)?;
        if prepared.empty {
            self.unwind_bindings(bindings_start);
            if self.stack.is_empty() {
                self.phase = Phase::Epilog;
            }
            Ok(Event::Empty(element))
        } else {
            self.stack.push(Frame {
                qualified: FrameName::Retained(
                    qualified.expect("nonempty tag retains its qualified name"),
                ),
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
        if !self.indexed {
            if let Some((id, _)) = self.interned(uri) {
                return Ok(Some(id));
            }
            // Charge the URI exactly as retaining it as an index key would.
            if matches!(uri, Cow::Owned(_)) || self.copies_names {
                self.budget.charge_owned(uri.len(), offset)?;
            }
            let id = NamespaceId::Declared(self.next_namespace);
            self.next_namespace = self
                .next_namespace
                .checked_add(1)
                .ok_or(Error::new(ErrorKind::Allocation, offset))?;
            return Ok(Some(id));
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

    #[allow(
        clippy::ptr_arg,
        reason = "the Cow variant determines whether a token borrow can be retained"
    )]
    fn retain_name_range<'event, F>(
        &mut self,
        value: &Cow<'event, str>,
        range: Range<usize>,
        offset: usize,
        retain: &mut F,
    ) -> Result<Cow<'source, str>, Error>
    where
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
    {
        match value {
            Cow::Borrowed(raw) => retain(&raw[range], offset, &mut self.budget),
            Cow::Owned(value) => self.budget.copy(&value[range], offset),
        }
    }

    /// Resolves an element QName whose prefix split was already validated.
    fn resolve_name<'event>(
        &mut self,
        qualified: &'event str,
        prefix_length: Option<NonZeroUsize>,
        offset: usize,
    ) -> Result<Name<'event>, Error>
    where
        'source: 'event,
    {
        let mut name = Name::from_validated_parts(Cow::Borrowed(qualified), prefix_length, None);
        self.resolve_namespace(&mut name, false, offset)?;
        Ok(name)
    }

    fn resolve_end_name_owned(
        &mut self,
        qualified: &str,
        offset: usize,
    ) -> Result<Name<'static>, Error> {
        let (prefix, _) = qname(qualified, offset)?;
        let prefix_length = prefix.and_then(|prefix| NonZeroUsize::new(prefix.len()));
        let namespace = match prefix {
            Some("xmlns") => return Err(Error::new(ErrorKind::Namespace, offset)),
            Some("xml") => Some(Cow::Borrowed(XML)),
            _ => match self.bound(prefix.unwrap_or("")) {
                Some(index) if !self.bindings[index].uri.is_empty() => {
                    Some(self.budget.copy(&self.bindings[index].uri, offset)?)
                }
                _ if prefix.is_some() => return Err(Error::new(ErrorKind::Namespace, offset)),
                _ => None,
            },
        };
        Ok(Name::from_validated_parts(
            self.budget.copy(qualified, offset)?,
            prefix_length,
            namespace,
        ))
    }

    fn resolve_namespace<'event>(
        &mut self,
        name: &mut Name<'event>,
        attribute: bool,
        offset: usize,
    ) -> Result<Option<NamespaceId>, Error>
    where
        'source: 'event,
    {
        let prefix = name.prefix();
        let qualified = name.qualified();
        let (namespace, identity) = match prefix {
            Some("xmlns") if attribute => (Some(Cow::Borrowed(XMLNS)), Some(NamespaceId::Xmlns)),
            Some("xmlns") => return Err(Error::new(ErrorKind::Namespace, offset)),
            Some("xml") => (Some(Cow::Borrowed(XML)), Some(NamespaceId::Xml)),
            None if attribute && qualified == "xmlns" => {
                (Some(Cow::Borrowed(XMLNS)), Some(NamespaceId::Xmlns))
            }
            None if attribute => (None, None),
            _ => match self.bound(prefix.unwrap_or("")) {
                Some(index) if !self.bindings[index].uri.is_empty() => {
                    if let Cow::Owned(uri) = &self.bindings[index].uri {
                        self.budget.charge_owned(uri.len(), offset)?;
                    }
                    (
                        Some(match &self.bindings[index].uri {
                            Cow::Borrowed(uri) => Cow::Borrowed(*uri),
                            Cow::Owned(uri) => {
                                let mut copy = String::new();
                                copy.try_reserve(uri.len())
                                    .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
                                copy.push_str(uri);
                                Cow::Owned(copy)
                            }
                        }),
                        self.bindings[index].namespace,
                    )
                }
                _ if prefix.is_some() => return Err(Error::new(ErrorKind::Namespace, offset)),
                _ => (None, None),
            },
        };
        name.set_namespace(namespace);
        Ok(identity)
    }

    fn resolve_namespace_with_pending<'event, N>(
        &mut self,
        name: &mut Name<'event>,
        attribute: bool,
        offset: usize,
        pending: &[PendingBinding<'source>],
        pending_prefixes: &HashMap<Cow<'source, str>, usize>,
        namespace_output: &mut N,
    ) -> Result<Option<NamespaceId>, Error>
    where
        N: FnMut(&Cow<'source, str>, usize, &mut Budget) -> Result<Cow<'event, str>, Error>,
    {
        let prefix = name.prefix();
        let qualified = name.qualified();
        let selected = prefix.unwrap_or("");
        let planned = pending_prefixes.get(selected).map(|index| &pending[*index]);
        let (namespace, identity) = match prefix {
            Some("xmlns") if attribute => (Some(Cow::Borrowed(XMLNS)), Some(NamespaceId::Xmlns)),
            Some("xmlns") => return Err(Error::new(ErrorKind::Namespace, offset)),
            Some("xml") => (Some(Cow::Borrowed(XML)), Some(NamespaceId::Xml)),
            None if attribute && qualified == "xmlns" => {
                (Some(Cow::Borrowed(XMLNS)), Some(NamespaceId::Xmlns))
            }
            None if attribute => (None, None),
            _ => {
                let active = self.bound(selected);
                let binding = planned
                    .map(|binding| (&binding.uri, binding.namespace))
                    .or_else(|| {
                        active.map(|index| {
                            let binding = &self.bindings[index];
                            (&binding.uri, binding.namespace)
                        })
                    });
                match binding {
                    Some((uri, identity)) if !uri.is_empty() => (
                        Some(namespace_output(uri, offset, &mut self.budget)?),
                        identity,
                    ),
                    _ if prefix.is_some() => {
                        return Err(Error::new(ErrorKind::Namespace, offset));
                    }
                    _ => (None, None),
                }
            }
        };
        name.set_namespace(namespace);
        Ok(identity)
    }

    fn end_element<'event>(
        &mut self,
        raw: &'event str,
        start: usize,
    ) -> Result<Option<Event<'event>>, Error>
    where
        'source: 'event,
    {
        let body = delimited_body(raw, "</", ">", start)?;
        let mut scan = TagScanner {
            raw: body,
            cursor: 0,
            offset: start + 2,
        };
        // The open element's name is a validated QName: an exact match needs
        // no rescan, and any other spelling takes the diagnosing path.
        let open = self.open_name();
        let trimmed = body.trim_end_matches([' ', '\t', '\r', '\n']);
        let qualified = if open == Some(trimmed) {
            trimmed
        } else {
            let qualified = scan.name()?;
            scan.skip_space();
            if scan.cursor != body.len() {
                return Err(Error::new(ErrorKind::Syntax, start));
            }
            if open != Some(qualified) {
                return Err(Error::new(ErrorKind::MismatchedEndTag, start));
            }
            qualified
        };
        let prefix_length = memchr(b':', qualified.as_bytes()).and_then(NonZeroUsize::new);
        let name = self.resolve_name(qualified, prefix_length, start + 2)?;
        let frame = self.pop_frame();
        self.unwind_bindings(frame.bindings_start);
        if self.stack.is_empty() {
            self.phase = Phase::Epilog;
        }
        Ok(Some(Event::End(name)))
    }

    fn unwind_bindings(&mut self, start: usize) {
        while self.bindings.len() > start {
            let binding = self.bindings.pop().expect("binding scope");
            if self.indexed {
                if let Some(previous) = binding.previous {
                    *self
                        .namespace_index
                        .get_mut(binding.prefix.as_ref())
                        .expect("active namespace prefix") = previous;
                } else {
                    self.namespace_index.remove(binding.prefix.as_ref());
                }
            }
            if self.indexed && matches!(binding.namespace, Some(NamespaceId::Declared(_))) {
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
        if self.indexed && self.bindings.len() <= INDEXED_SCOPE / 2 {
            self.namespace_index = HashMap::new();
            self.namespace_ids = HashMap::new();
            self.indexed = false;
        }
    }
}

/// A QName's prefix and local part, from a scanner-proven split when present.
fn split_qname(
    name: &str,
    split: Option<Option<NonZeroUsize>>,
    offset: usize,
) -> Result<(Option<&str>, &str), Error> {
    match split {
        Some(None) => Ok((None, name)),
        Some(Some(prefix)) => Ok((Some(&name[..prefix.get()]), &name[prefix.get() + 1..])),
        None => qname(name, offset),
    }
}

fn invalid_namespace_binding(prefix: &str, uri: &str) -> bool {
    prefix == "xmlns"
        || uri == XMLNS
        || (prefix == "xml" && uri != XML)
        || (prefix != "xml" && uri == XML)
        || (!prefix.is_empty() && uri.is_empty())
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
/// ASCII classes for one-pass Name scanning; non-ASCII bytes are class 0.
const START: u8 = 1;
const CONTINUE: u8 = 2;
const COLON: u8 = 4;
const DELIMITER: u8 = 8;
const NAME_BYTES: [u8; 256] = {
    let mut table = [0; 256];
    let mut byte = 0;
    while byte < 128 {
        let c = byte as u8;
        table[byte] = if c.is_ascii_alphabetic() || c == b'_' {
            START | CONTINUE
        } else if c.is_ascii_digit() || c == b'-' || c == b'.' {
            CONTINUE
        } else if c == b':' {
            COLON
        } else if matches!(
            c,
            b' ' | b'\t' | b'\r' | b'\n' | b'=' | b'/' | b'>' | b'\'' | b'"'
        ) {
            DELIMITER
        } else {
            0
        };
        byte += 1;
    }
    table
};

struct TagScanner<'a> {
    raw: &'a str,
    cursor: usize,
    offset: usize,
}

struct ScannedAttribute<'a> {
    qualified: &'a str,
    /// The proven QName split, when the scanner established it.
    split: Option<Option<NonZeroUsize>>,
    literal: &'a str,
    name_offset: usize,
    value_offset: usize,
    value_end: usize,
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
        self.scan_name().map(|(name, _)| name)
    }

    /// Scans and validates a Name in one pass. For an ASCII name it also
    /// proves the QName split, returned as `Some(prefix length)`; `None`
    /// leaves `qname` to establish it (and its exact error).
    fn scan_name(&mut self) -> Result<(&'a str, Option<Option<NonZeroUsize>>), Error> {
        let bytes = self.raw.as_bytes();
        let start = self.cursor;
        let (mut valid, mut colons, mut colon) = (true, 0, 0);
        while let Some(&byte) = bytes.get(self.cursor) {
            let class = NAME_BYTES[usize::from(byte)];
            if class & DELIMITER != 0 {
                break;
            }
            let wanted = if self.cursor == start {
                START
            } else {
                CONTINUE
            };
            valid &= class & (wanted | COLON) != 0;
            if class & COLON != 0 {
                colons += 1;
                colon = self.cursor - start;
            }
            self.cursor += 1;
        }
        let name = &self.raw[start..self.cursor];
        if !valid || name.is_empty() {
            validate_name(name, self.offset + start, true)?;
        }
        // A valid Name is a QName when its only colon is followed by a
        // name-start character; prefix characters are already valid.
        let split = match colons {
            0 => Some(None),
            1 if colon > 0
                && name[colon + 1..]
                    .chars()
                    .next()
                    .is_some_and(|c| crate::name::name_start(c, false)) =>
            {
                Some(NonZeroUsize::new(colon))
            }
            _ => None,
        };
        Ok((name, split))
    }

    // Returns the name's source offset; values are still unnormalized input.
    fn attribute(&mut self) -> Result<Option<ScannedAttribute<'a>>, Error> {
        let spaced = self.skip_space();
        if self.cursor == self.raw.len() {
            return Ok(None);
        }
        if !spaced {
            return Err(Error::new(ErrorKind::Syntax, self.offset + self.cursor));
        }
        let start = self.cursor;
        let (name, split) = self.scan_name()?;
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
        let value_end = self.offset + self.cursor;
        self.cursor += 1;
        Ok(Some(ScannedAttribute {
            qualified: name,
            split,
            literal: value,
            name_offset: self.offset + start,
            value_offset: self.offset + value_start,
            value_end,
        }))
    }
}

/// Parses an XML declaration, or with `text` an external entity's text
/// declaration (optional version, required encoding, no standalone).
pub(crate) fn parse_declaration(
    raw: &str,
    offset: usize,
    text: bool,
) -> Result<(Declaration<'_>, Option<usize>), Error> {
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
    while let Some(attribute) = scan
        .attribute()
        .map_err(|e| Error::new(ErrorKind::InvalidDeclaration, e.offset))?
    {
        let name = attribute.qualified;
        let value = attribute.literal;
        let position = attribute.name_offset;
        match name {
            "version"
                if count == 0
                    && value.strip_prefix("1.").is_some_and(|minor| {
                        !minor.is_empty() && minor.bytes().all(|b| b.is_ascii_digit())
                    }) =>
            {
                declaration.version = value;
            }
            "encoding" if count == usize::from(count > 0 || !text) && encoding_offset.is_none() => {
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
            "standalone" if !text && count > 0 && declaration.standalone.is_none() => {
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
    if count == 0 || (text && encoding_offset.is_none()) {
        return Err(Error::new(ErrorKind::InvalidDeclaration, offset));
    }
    Ok((declaration, encoding_offset))
}

/// Returns the encoding for the input after the declaration. Only a BOM-less
/// decoding stream (`switchable`) can continue in a declared byte charset.
fn validate_encoding(
    source: crate::encoding::SourceEncoding,
    switchable: bool,
    label: Option<&str>,
    offset: usize,
) -> Result<crate::encoding::SourceEncoding, Error> {
    use crate::encoding::{Charset, SourceEncoding};
    let Some(label) = label else {
        return if source.requires_declaration() {
            Err(Error::new(ErrorKind::EncodingMismatch, offset))
        } else {
            Ok(source)
        };
    };
    let kind = match (source, crate::encoding::charset(label)) {
        (_, None) => ErrorKind::UnsupportedEncoding,
        (SourceEncoding::Utf8, Some(Charset::Utf8)) => return Ok(source),
        (SourceEncoding::Utf8, Some(Charset::Byte(table))) if switchable => {
            return Ok(SourceEncoding::Byte(table));
        }
        (SourceEncoding::Utf16 { little_endian, bom }, Some(Charset::Utf16(order)))
            if order.map_or(bom, |order| order == little_endian) =>
        {
            return Ok(source);
        }
        (SourceEncoding::Utf16 { .. }, Some(_)) => ErrorKind::EncodingMismatch,
        _ => ErrorKind::UnsupportedEncoding,
    };
    Err(Error::new(kind, offset))
}

#[cfg(test)]
#[path = "../tests/internal/parser.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/internal/dtd_tag_spec.rs"]
mod dtd_tag_spec;

#[cfg(test)]
#[path = "../tests/internal/dtd_owned_leaf_spec.rs"]
mod dtd_owned_leaf_spec;
