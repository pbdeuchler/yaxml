use std::borrow::Cow;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::dom::{Record, append_record};
use crate::parser::Budget;
use crate::{Attribute, Declaration, Error, ErrorKind, Event, Limit, Limits, Name, NodeId, Reader};
use crate::{DecodeLimits, EditError, EditErrorKind, EditLimit, PushParser};

const XMLNS: &str = "http://www.w3.org/2000/xmlns/";

static LAST_DOCUMENT_ID: AtomicUsize = AtomicUsize::new(0);

fn document_identity(counter: &AtomicUsize) -> Result<NonZeroUsize, Error> {
    let previous = counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |last| {
            last.checked_add(1)
        })
        .map_err(|_| Error::new(ErrorKind::IdentifierExhausted, 0))?;
    // The successful checked increment guarantees previous < usize::MAX.
    Ok(NonZeroUsize::MIN.saturating_add(previous))
}

/// A checked owned-tree handle, tied to one document's non-recycled identity.
/// Moving a document preserves its handles; separately parsed documents have
/// different identities. This is not a stable address or a serialized ID.
/// Borrowing-DOM IDs cannot be used for owned lookup:
///
/// ```compile_fail
/// use yaxml::{Document, OwnedDocument};
/// let borrowed = Document::parse("<r/>").unwrap();
/// let owned = OwnedDocument::parse("<r/>").unwrap();
/// owned.node(borrowed.root().id());
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OwnedNodeId {
    document: NonZeroUsize,
    slot: NodeId,
}

/// The content category of a node in an owned document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum OwnedNodeKind {
    Document,
    Element,
    Text,
    Cdata,
    Comment,
    ProcessingInstruction,
}

#[derive(Debug)]
enum Content {
    Document,
    Element {
        name: StoredName,
        attributes: Range<usize>,
    },
    Text(Range<usize>),
    Cdata(Range<usize>),
    Comment(Range<usize>),
    ProcessingInstruction {
        target: Range<usize>,
        data: Range<usize>,
    },
}

#[derive(Debug)]
struct StoredName {
    qualified: Range<usize>,
    prefix_length: Option<usize>,
    namespace: Option<Range<usize>>,
}

impl StoredName {
    fn view<'pool>(&self, pool: &'pool str) -> Name<'pool> {
        let qualified = &pool[self.qualified.clone()];
        let (prefix, local) = match self.prefix_length {
            Some(length) => (Some(&qualified[..length]), &qualified[length + 1..]),
            None => (None, qualified),
        };
        Name {
            qualified,
            prefix,
            local,
            namespace: self
                .namespace
                .as_ref()
                .map(|range| Cow::Borrowed(&pool[range.clone()])),
        }
    }
}

#[derive(Debug)]
struct StoredAttribute {
    name: StoredName,
    value: Range<usize>,
}

impl StoredAttribute {
    fn view<'pool>(&self, pool: &'pool str) -> Attribute<'pool> {
        Attribute {
            name: self.name.view(pool),
            value: Cow::Borrowed(&pool[self.value.clone()]),
        }
    }
}

#[derive(Debug)]
struct StoredDeclaration {
    version: Range<usize>,
    encoding: Option<Range<usize>>,
    standalone: Option<bool>,
}

impl StoredDeclaration {
    fn view<'pool>(&self, pool: &'pool str) -> Declaration<'pool> {
        Declaration {
            version: &pool[self.version.clone()],
            encoding: self.encoding.as_ref().map(|range| &pool[range.clone()]),
            standalone: self.standalone,
        }
    }
}

#[derive(Debug, Default)]
struct Pool(String);

impl Pool {
    fn store_edit(
        &mut self,
        value: &str,
        spent: &mut usize,
        limit: usize,
    ) -> Result<Range<usize>, EditError> {
        let exceeded = EditError::new(EditErrorKind::LimitExceeded(EditLimit::OwnedBytes), None);
        let start = self.0.len();
        let needed = start.checked_add(value.len()).ok_or(exceeded)?;
        if needed > self.0.capacity() {
            let remaining = limit.checked_sub(*spent).ok_or(exceeded)?;
            let target = self
                .0
                .capacity()
                .saturating_mul(2)
                .max(needed)
                .min(remaining);
            if target < needed {
                return Err(exceeded);
            }
            // Full replacement capacity is cumulative work, including failed
            // reservations. Parsed capacity is the uncharged editing baseline.
            *spent += target;
            self.0
                .try_reserve_exact(target - start)
                .map_err(|_| EditError::new(EditErrorKind::Allocation, None))?;
        }
        self.0.push_str(value);
        Ok(start..needed)
    }

    fn store(
        &mut self,
        value: &str,
        offset: usize,
        budget: &mut Budget,
    ) -> Result<Range<usize>, Error> {
        let start = self.0.len();
        let needed = start.checked_add(value.len()).ok_or(Error::new(
            ErrorKind::LimitExceeded(Limit::OwnedBytes),
            offset,
        ))?;
        if needed > self.0.capacity() {
            let target = self
                .0
                .capacity()
                .saturating_mul(2)
                .max(needed)
                .min(budget.remaining_owned());
            if target < needed {
                return Err(Error::new(
                    ErrorKind::LimitExceeded(Limit::OwnedBytes),
                    offset,
                ));
            }
            // Charge the entire replacement buffer, even if the allocator can
            // grow in place. Earlier requests remain cumulative work.
            budget.charge_owned(target, offset)?;
            self.0
                .try_reserve_exact(target - start)
                .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
        }
        self.0.push_str(value);
        Ok(start..needed)
    }

    fn name(
        &mut self,
        name: &Name<'_>,
        offset: usize,
        budget: &mut Budget,
    ) -> Result<StoredName, Error> {
        Ok(StoredName {
            qualified: self.store(name.qualified, offset, budget)?,
            prefix_length: name.qualified.find(':'),
            namespace: name
                .namespace
                .as_deref()
                .map(|namespace| self.store(namespace, offset, budget))
                .transpose()?,
        })
    }
}

/// An XML tree that owns its logical strings as UTF-8.
///
/// Nodes and attributes contain ranges into one string pool, so moving the
/// document or growing its storage creates no self-references. Parsing,
/// traversal and destruction are iterative. Source bytes are not retained.
#[derive(Debug)]
pub struct OwnedDocument {
    identity: NonZeroUsize,
    nodes: Vec<Record<Content>>,
    attributes: Vec<StoredAttribute>,
    pool: Pool,
    root_element: NodeId,
    declaration: Option<StoredDeclaration>,
    edit_owned_bytes: usize,
}

impl OwnedDocument {
    pub fn parse(input: &str) -> Result<Self, Error> {
        Self::parse_bytes(input.as_bytes())
    }

    /// Parses UTF-8 or UTF-16 XML independently of the source's lifetime.
    pub fn parse_bytes(input: &[u8]) -> Result<Self, Error> {
        Self::with_limits(input, Limits::default())
    }

    /// Parses with aggregate string accounting for parser normalization and
    /// full new pool-capacity requests. Owned storage can exhaust this budget
    /// before the borrowing DOM; metadata and allocator overhead are separate.
    pub fn with_limits(input: &[u8], limits: Limits) -> Result<Self, Error> {
        Self::with_decode_limits(input, limits, DecodeLimits::default())
    }

    /// Parses UTF-8 or UTF-16 while bounding source, decoded and retained bytes.
    pub fn with_decode_limits(
        input: &[u8],
        limits: Limits,
        decode: DecodeLimits,
    ) -> Result<Self, Error> {
        if input.len() > limits.max_input_bytes {
            return Err(Error::new(ErrorKind::LimitExceeded(Limit::InputBytes), 0));
        }
        let mut builder = OwnedBuilder::new(input.len())?;

        let position = if crate::encoding::needs_decoding(input) {
            let mut parser = PushParser::with_decode_limits(limits, decode);
            parser.feed_with(input, |event, range, budget| {
                builder.consume(event, range, budget)
            })?;
            parser.finish_with(|event, range, budget| builder.consume(event, range, budget))?;
            parser.position()
        } else {
            let mut reader = Reader::with_decode_limits(input, limits, decode)?;
            while let Some(event) = reader.read_event()? {
                let range = reader.event_range();
                builder.consume(event, range, reader.owned_budget())?;
            }
            reader.position()
        };
        builder.finish(position)
    }

    /// Returns the synthetic root, including surrounding comments and PIs.
    pub fn root(&self) -> OwnedNode<'_> {
        OwnedNode {
            document: self,
            id: NodeId(0),
        }
    }

    pub fn root_element(&self) -> OwnedNode<'_> {
        OwnedNode {
            document: self,
            id: self.root_element,
        }
    }

    /// Looks up a checked owned handle, rejecting foreign or out-of-range IDs.
    pub fn node(&self, id: OwnedNodeId) -> Option<OwnedNode<'_>> {
        if id.document != self.identity {
            return None;
        }
        self.nodes.get(id.slot.0).map(|_| OwnedNode {
            document: self,
            id: id.slot,
        })
    }

    pub fn nodes(&self) -> impl ExactSizeIterator<Item = OwnedNode<'_>> + '_ {
        self.root().descendants()
    }

    pub fn declaration(&self) -> Option<Declaration<'_>> {
        self.declaration
            .as_ref()
            .map(|declaration| declaration.view(&self.pool.0))
    }

    /// Opens a bounded value editor. The owned-byte counter persists across
    /// editors; a ceiling below earlier editing expenditure is rejected.
    /// Parsed string capacity is the baseline and is not charged again.
    ///
    /// ```
    /// use yaxml::{EditLimits, OwnedDocument};
    /// let mut document = OwnedDocument::parse("<r>old</r>")?;
    /// let text = document.root_element().first_child().unwrap().id();
    /// document.edit(EditLimits::default())?.set_node_value(text, "new")?;
    /// assert_eq!(document.node(text).unwrap().text(), Some("new"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// Views cannot be used across mutation:
    ///
    /// ```compile_fail
    /// use yaxml::{EditLimits, OwnedDocument};
    /// let mut document = OwnedDocument::parse("<r>old</r>").unwrap();
    /// let view = document.root_element().first_child().unwrap();
    /// let mut editor = document.edit(EditLimits::default()).unwrap();
    /// editor.set_node_value(view.id(), "new").unwrap();
    /// ```
    pub fn edit(&mut self, limits: EditLimits) -> Result<Editor<'_>, EditError> {
        if self.edit_owned_bytes > limits.max_owned_bytes {
            return Err(EditError::new(
                EditErrorKind::LimitExceeded(EditLimit::OwnedBytes),
                None,
            ));
        }
        Ok(Editor {
            document: self,
            limits,
        })
    }
}

/// Inclusive ceilings for replacement values and cumulative editing capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditLimits {
    /// Replacement UTF-8 bytes, checked before scanning. Default: 1 MiB.
    pub max_value_bytes: usize,
    /// Full new pool-capacity requests across all editors. Default: 64 MiB.
    /// Includes earlier released buffers and failed reservation charges, but
    /// excludes parsed baseline capacity, metadata and allocator overhead.
    pub max_owned_bytes: usize,
}

impl Default for EditLimits {
    fn default() -> Self {
        Self {
            max_value_bytes: 1024 * 1024,
            max_owned_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Exclusive, atomic edits to existing logical values. An editor remains usable
/// after an error. Operations preserve topology, names and source provenance.
/// Dropping an editor performs no work. Equal replacements take the edit path.
pub struct Editor<'document> {
    document: &'document mut OwnedDocument,
    limits: EditLimits,
}

impl Editor<'_> {
    pub fn document(&self) -> &OwnedDocument {
        self.document
    }

    fn resolve(&self, node: OwnedNodeId) -> Result<usize, EditError> {
        if node.document != self.document.identity || node.slot.0 >= self.document.nodes.len() {
            return Err(EditError::new(EditErrorKind::InvalidHandle, None));
        }
        Ok(node.slot.0)
    }

    /// Replaces Text, CDATA, Comment or PI data; PI targets remain unchanged.
    /// Non-text lexical values reject raw CR and their closing delimiters.
    /// Comments also reject a trailing dash; PI data rejects leading XML space.
    pub fn set_node_value(&mut self, node: OwnedNodeId, value: &str) -> Result<(), EditError> {
        let index = self.resolve(node)?;
        let (target, kind) = match &mut self.document.nodes[index].kind {
            Content::Text(range) => (range, ValueKind::Text),
            Content::Cdata(range) => (range, ValueKind::Cdata),
            Content::Comment(range) => (range, ValueKind::Comment),
            Content::ProcessingInstruction { data, .. } => (data, ValueKind::Pi),
            Content::Document | Content::Element { .. } => {
                return Err(EditError::new(EditErrorKind::WrongNodeKind, None));
            }
        };
        validate_value(value, kind, self.limits.max_value_bytes)?;
        let replacement = self.document.pool.store_edit(
            value,
            &mut self.document.edit_owned_bytes,
            self.limits.max_owned_bytes,
        )?;
        *target = replacement;
        Ok(())
    }

    /// Replaces one existing attribute by expanded name, retaining its spelling
    /// and position. Some("") differs from None; selector names are not parsed.
    /// Missing attributes and XMLNS declaration edits are rejected.
    pub fn set_attribute_value(
        &mut self,
        element: OwnedNodeId,
        namespace: Option<&str>,
        local: &str,
        value: &str,
    ) -> Result<(), EditError> {
        let index = self.resolve(element)?;
        let attributes = match &self.document.nodes[index].kind {
            Content::Element { attributes, .. } => attributes,
            _ => return Err(EditError::new(EditErrorKind::WrongNodeKind, None)),
        };
        let target = attributes
            .clone()
            .find(|index| {
                let name = self.document.attributes[*index]
                    .name
                    .view(&self.document.pool.0);
                name.local == local && name.namespace.as_deref() == namespace
            })
            .ok_or(EditError::new(EditErrorKind::MissingAttribute, None))?;
        if namespace == Some(XMLNS) {
            return Err(EditError::new(EditErrorKind::NamespaceDeclaration, None));
        }
        validate_value(value, ValueKind::Text, self.limits.max_value_bytes)?;
        let replacement = self.document.pool.store_edit(
            value,
            &mut self.document.edit_owned_bytes,
            self.limits.max_owned_bytes,
        )?;
        self.document.attributes[target].value = replacement;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum ValueKind {
    Text,
    Cdata,
    Comment,
    Pi,
}

fn validate_value(value: &str, kind: ValueKind, limit: usize) -> Result<(), EditError> {
    if value.len() > limit {
        return Err(EditError::new(
            EditErrorKind::LimitExceeded(EditLimit::ValueBytes),
            None,
        ));
    }
    if let Some((index, _)) = value
        .char_indices()
        .find(|(_, character)| !crate::escape::xml_char(*character))
    {
        return Err(EditError::new(EditErrorKind::InvalidCharacter, Some(index)));
    }
    let invalid = match kind {
        ValueKind::Text => None,
        ValueKind::Cdata => value.find("]]>").into_iter().chain(value.find('\r')).min(),
        ValueKind::Comment => value
            .find("--")
            .into_iter()
            .chain(value.find('\r'))
            .chain(value.ends_with('-').then(|| value.len() - 1))
            .min(),
        ValueKind::Pi => value
            .find("?>")
            .into_iter()
            .chain(value.find('\r'))
            .chain(
                value
                    .as_bytes()
                    .first()
                    .is_some_and(|byte| crate::parser::space(*byte))
                    .then_some(0),
            )
            .min(),
    };
    if let Some(index) = invalid {
        return Err(EditError::new(EditErrorKind::InvalidValue, Some(index)));
    }
    Ok(())
}

/// Concrete shared sink: all strings are copied using the parser's budget.
struct OwnedBuilder {
    nodes: Vec<Record<Content>>,
    attributes: Vec<StoredAttribute>,
    pool: Pool,
    root_element: NodeId,
    declaration: Option<StoredDeclaration>,
    parents: Vec<NodeId>,
}

impl OwnedBuilder {
    fn new(source_length: usize) -> Result<Self, Error> {
        let mut nodes = Vec::new();
        nodes
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorKind::Allocation, 0))?;
        nodes.push(Record::root(Content::Document, source_length));
        Ok(Self {
            nodes,
            attributes: Vec::new(),
            pool: Pool::default(),
            root_element: NodeId(0),
            declaration: None,
            parents: Vec::new(),
        })
    }

    fn consume(
        &mut self,
        event: Event<'_>,
        range: Range<usize>,
        budget: &mut Budget,
    ) -> Result<(), Error> {
        let starts = matches!(&event, Event::Start(_));

        let offset = range.start;
        let parent = self.parents.last().copied().unwrap_or(NodeId(0));
        let content = match event {
            Event::Declaration(declaration) => {
                self.declaration = Some(StoredDeclaration {
                    version: self.pool.store(declaration.version, offset, budget)?,
                    encoding: declaration
                        .encoding
                        .map(|encoding| self.pool.store(encoding, offset, budget))
                        .transpose()?,
                    standalone: declaration.standalone,
                });
                return Ok(());
            }
            Event::End(_) => {
                let id = self
                    .parents
                    .pop()
                    .ok_or_else(|| Error::new(ErrorKind::MismatchedEndTag, offset))?;
                let end = self.nodes.len();
                let record = &mut self.nodes[id.0];
                record.range.end = range.end;
                record.subtree_end = end;
                return Ok(());
            }
            Event::Start(element) | Event::Empty(element) => {
                let name = self.pool.name(&element.name, offset, budget)?;
                let start = self.attributes.len();
                self.attributes
                    .try_reserve(element.attributes.len())
                    .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
                for attribute in element.attributes {
                    self.attributes.push(StoredAttribute {
                        name: self.pool.name(&attribute.name, offset, budget)?,
                        value: self.pool.store(&attribute.value, offset, budget)?,
                    });
                }
                Content::Element {
                    name,
                    attributes: start..self.attributes.len(),
                }
            }
            Event::Text(value) => Content::Text(self.pool.store(&value, offset, budget)?),
            Event::Cdata(value) => Content::Cdata(self.pool.store(&value, offset, budget)?),
            Event::Comment(value) => Content::Comment(self.pool.store(&value, offset, budget)?),
            Event::ProcessingInstruction { target, data } => Content::ProcessingInstruction {
                target: self.pool.store(target, offset, budget)?,
                data: self.pool.store(&data, offset, budget)?,
            },
        };
        let id = append_record(&mut self.nodes, parent, content, range)?;
        if matches!(self.nodes[id.0].kind, Content::Element { .. }) {
            if parent == NodeId(0) {
                self.root_element = id;
            }
            if starts {
                self.parents
                    .try_reserve(1)
                    .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
                self.parents.push(id);
            }
        }
        Ok(())
    }

    fn finish(self, position: usize) -> Result<OwnedDocument, Error> {
        self.finish_with_counter(position, &LAST_DOCUMENT_ID)
    }

    fn finish_with_counter(
        mut self,
        position: usize,
        counter: &AtomicUsize,
    ) -> Result<OwnedDocument, Error> {
        if self.root_element == NodeId(0) {
            return Err(Error::new(ErrorKind::MissingRoot, position));
        }
        self.nodes[0].subtree_end = self.nodes.len();
        Ok(OwnedDocument {
            identity: document_identity(counter)?,
            nodes: self.nodes,
            attributes: self.attributes,
            pool: self.pool,
            root_element: self.root_element,
            declaration: self.declaration,
            edit_owned_bytes: 0,
        })
    }
}

/// A read-only view bounded by its owned document's lifetime.
#[derive(Clone, Copy, Debug)]
pub struct OwnedNode<'document> {
    document: &'document OwnedDocument,
    id: NodeId,
}

impl<'document> OwnedNode<'document> {
    fn record(self) -> &'document Record<Content> {
        &self.document.nodes[self.id.0]
    }

    pub fn id(self) -> OwnedNodeId {
        OwnedNodeId {
            document: self.document.identity,
            slot: self.id,
        }
    }

    pub fn kind(self) -> OwnedNodeKind {
        match &self.record().kind {
            Content::Document => OwnedNodeKind::Document,
            Content::Element { .. } => OwnedNodeKind::Element,
            Content::Text(_) => OwnedNodeKind::Text,
            Content::Cdata(_) => OwnedNodeKind::Cdata,
            Content::Comment(_) => OwnedNodeKind::Comment,
            Content::ProcessingInstruction { .. } => OwnedNodeKind::ProcessingInstruction,
        }
    }

    pub fn name(self) -> Option<Name<'document>> {
        match &self.record().kind {
            Content::Element { name, .. } => Some(name.view(&self.document.pool.0)),
            _ => None,
        }
    }

    /// Returns the direct Text, CDATA, comment or PI payload, without joining
    /// descendant text. Elements and the document root return None.
    pub fn text(self) -> Option<&'document str> {
        let range = match &self.record().kind {
            Content::Text(range) | Content::Cdata(range) | Content::Comment(range) => range,
            Content::ProcessingInstruction { data, .. } => data,
            Content::Document | Content::Element { .. } => return None,
        };
        Some(&self.document.pool.0[range.clone()])
    }

    pub fn processing_instruction_target(self) -> Option<&'document str> {
        match &self.record().kind {
            Content::ProcessingInstruction { target, .. } => {
                Some(&self.document.pool.0[target.clone()])
            }
            _ => None,
        }
    }

    /// Visits attributes in source order, including namespace declarations.
    /// Each view borrows the pool without allocating.
    pub fn attributes(self) -> impl ExactSizeIterator<Item = Attribute<'document>> + 'document {
        let range = match &self.record().kind {
            Content::Element { attributes, .. } => attributes.clone(),
            _ => 0..0,
        };
        range.map(move |index| self.document.attributes[index].view(&self.document.pool.0))
    }

    /// Original source location, independent of current logical content and
    /// pool storage. Parsed nodes retain Some(range); future created nodes may
    /// have no source provenance. This cannot index the owned string pool.
    pub fn range(self) -> Option<Range<usize>> {
        Some(self.record().range.clone())
    }

    pub fn parent(self) -> Option<Self> {
        self.record().parent.map(|id| Self {
            document: self.document,
            id,
        })
    }

    pub fn first_child(self) -> Option<Self> {
        self.record().first_child.map(|id| Self {
            document: self.document,
            id,
        })
    }

    pub fn last_child(self) -> Option<Self> {
        self.record().last_child.map(|id| Self {
            document: self.document,
            id,
        })
    }

    pub fn next_sibling(self) -> Option<Self> {
        self.record().next_sibling.map(|id| Self {
            document: self.document,
            id,
        })
    }

    pub fn children(self) -> impl Iterator<Item = Self> + 'document {
        std::iter::successors(self.first_child(), |node| node.next_sibling())
    }

    /// Visits this node and its descendants in preorder without recursion.
    pub fn descendants(self) -> impl ExactSizeIterator<Item = Self> + 'document {
        (self.id.0..self.record().subtree_end).map(move |index| Self {
            document: self.document,
            id: NodeId(index),
        })
    }
}

#[cfg(test)]
#[path = "../tests/internal/owned.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/internal/editor.rs"]
mod edit_tests;
