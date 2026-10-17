use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::ops::Range;

use memchr::memchr;

use crate::escape::{copy_cow, needs_normalization, normalize_required};
use crate::name::{qname, validate_name};
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

struct Frame<'source> {
    qualified: Cow<'source, str>,
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
    #[cfg(test)]
    Defaulted { declaration: usize },
}

impl AttributeProvenance {
    pub(crate) fn offset(self, _fallback: usize) -> usize {
        match self {
            Self::Specified { name_offset, .. } => name_offset,
            #[cfg(test)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreparedTagErrorLocation {
    Input(usize),
    #[cfg(test)]
    Declaration {
        declaration: usize,
        offset: usize,
    },
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
            #[cfg(test)]
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

    fn into_input_error(self) -> Error {
        match self.location {
            PreparedTagErrorLocation::Input(offset) => Error::new(self.kind, offset),
            #[cfg(test)]
            PreparedTagErrorLocation::Declaration { .. } => {
                unreachable!("DTD-free preparation has no declaration provenance")
            }
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

    pub(crate) fn element_depth(&self) -> usize {
        self.stack.len()
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
        if !needs_normalization(raw, attribute, references) {
            return Ok(Cow::Borrowed(raw));
        }
        self.budget.charge_owned(raw.len(), offset)?;
        normalize_required(raw, offset, attribute, references).map(Cow::Owned)
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
            target: Cow::Borrowed(target),
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
        let raw = self.collect_raw_tag(raw, start)?;
        let prepared = self.prepare_physical_tag(raw)?;
        self.publish_prepared_tag(prepared, retain)
            .map_err(PreparedTagError::into_input_error)
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

    fn prepare_physical_tag<'event>(
        &mut self,
        raw: RawTag<'event>,
    ) -> Result<PreparedTag<'event>, Error> {
        let mut attributes = Vec::new();
        attributes
            .try_reserve(raw.attributes.len())
            .map_err(|_| Error::new(ErrorKind::Allocation, raw.start))?;
        for attribute in raw.attributes {
            debug_assert_eq!(
                attribute.value_end.checked_sub(attribute.value_offset),
                Some(attribute.literal.len())
            );
            let value = self.normalized(attribute.literal, attribute.value_offset, true, true)?;
            attributes.push(PreparedAttribute {
                name: Name::from_validated_parts(
                    Cow::Borrowed(attribute.qualified),
                    attribute.prefix_length,
                    None,
                ),
                value,
                provenance: AttributeProvenance::Specified {
                    name_offset: attribute.name_offset,
                    value_offset: attribute.value_offset,
                },
            });
        }
        Ok(PreparedTag {
            name: Name::from_validated_parts(Cow::Borrowed(raw.qualified), raw.prefix_length, None),
            attributes,
            empty: raw.empty,
            start: raw.start,
        })
    }

    pub(crate) fn publish_prepared_tag<'event, F>(
        &mut self,
        mut prepared: PreparedTag<'event>,
        retain: &mut F,
    ) -> Result<Event<'event>, PreparedTagError>
    where
        'source: 'event,
        F: FnMut(&'event str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
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
                if bound_prefix == "xmlns"
                    || attribute.value == XMLNS
                    || (bound_prefix == "xml" && attribute.value != XML)
                    || (bound_prefix != "xml" && attribute.value == XML)
                    || (!bound_prefix.is_empty() && attribute.value.is_empty())
                {
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
                        if let Some(entry) = self.namespace_ids.get(attribute.value.as_ref()) {
                            let bindings = entry.bindings.checked_add(1).ok_or_else(|| {
                                PreparedTagError::attribute(
                                    Error::new(ErrorKind::Allocation, offset),
                                    attribute.provenance,
                                )
                            })?;
                            (entry.id, bindings, true)
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
                let previous = self.namespace_index.get(bound_prefix).copied();
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
            attributes,
        };
        self.bindings.try_reserve(pending.len()).map_err(|_| {
            PreparedTagError::input(Error::new(ErrorKind::Allocation, prepared.start))
        })?;
        self.namespace_index
            .try_reserve(
                pending
                    .iter()
                    .filter(|binding| binding.previous.is_none())
                    .count(),
            )
            .map_err(|_| {
                PreparedTagError::input(Error::new(ErrorKind::Allocation, prepared.start))
            })?;
        self.namespace_ids
            .try_reserve(
                pending_namespaces
                    .values()
                    .filter(|namespace| !namespace.existing)
                    .count(),
            )
            .map_err(|_| {
                PreparedTagError::input(Error::new(ErrorKind::Allocation, prepared.start))
            })?;
        if !prepared.empty {
            self.stack.try_reserve(1).map_err(|_| {
                PreparedTagError::input(Error::new(ErrorKind::Allocation, prepared.start))
            })?;
        }

        self.next_namespace = next_namespace;
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
            if let Some(previous) = binding.previous {
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
        if prepared.empty {
            self.unwind_bindings(bindings_start);
            if self.stack.is_empty() {
                self.phase = Phase::Epilog;
            }
            Ok(Event::Empty(element))
        } else {
            self.stack.push(Frame {
                qualified: qualified.expect("nonempty tag retains its qualified name"),
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

    fn resolve_name<'event>(
        &mut self,
        qualified: &'event str,
        attribute: bool,
        offset: usize,
    ) -> Result<(Name<'event>, Option<NamespaceId>), Error>
    where
        'source: 'event,
    {
        let (prefix, _) = qname(qualified, offset)?;
        let mut name = Name::from_validated_parts(
            Cow::Borrowed(qualified),
            prefix.and_then(|prefix| std::num::NonZeroUsize::new(prefix.len())),
            None,
        );
        let identity = self.resolve_namespace(&mut name, attribute, offset)?;
        Ok((name, identity))
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
        name.set_namespace(namespace);
        Ok(identity)
    }

    fn resolve_namespace_with_pending<'event>(
        &mut self,
        name: &mut Name<'event>,
        attribute: bool,
        offset: usize,
        pending: &[PendingBinding<'source>],
        pending_prefixes: &HashMap<Cow<'source, str>, usize>,
    ) -> Result<Option<NamespaceId>, Error>
    where
        'source: 'event,
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
                let binding = planned
                    .map(|binding| (&binding.uri, binding.namespace))
                    .or_else(|| {
                        self.namespace_index.get(selected).map(|index| {
                            let binding = &self.bindings[*index];
                            (&binding.uri, binding.namespace)
                        })
                    });
                match binding {
                    Some((uri, identity)) if !uri.is_empty() => {
                        if let Cow::Owned(uri) = uri {
                            self.budget.charge_owned(uri.len(), offset)?;
                        }
                        (Some(copy_cow(uri, offset)?), identity)
                    }
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

struct ScannedAttribute<'a> {
    qualified: &'a str,
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
    fn attribute(&mut self) -> Result<Option<ScannedAttribute<'a>>, Error> {
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
        let value_end = self.offset + self.cursor;
        self.cursor += 1;
        Ok(Some(ScannedAttribute {
            qualified: name,
            literal: value,
            name_offset: self.offset + start,
            value_offset: self.offset + value_start,
            value_end,
        }))
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

#[cfg(test)]
#[path = "../tests/internal/parser.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/internal/dtd_tag_spec.rs"]
mod dtd_tag_spec;
