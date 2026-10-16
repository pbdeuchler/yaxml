use std::borrow::Cow;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};

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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Permanent removal is added in the next plan phase"
        )
    )]
    Removed,
}

// usize::MAX is never a published slot. Keeping the sentinel private saves one
// word per link compared with Option<NodeId>, without narrowing public IDs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Link(usize);

impl Link {
    const NONE: Self = Self(usize::MAX);

    fn new(id: NodeId) -> Option<Self> {
        (id.0 != usize::MAX).then_some(Self(id.0))
    }

    fn get(self) -> Option<NodeId> {
        (self != Self::NONE).then_some(NodeId(self.0))
    }
}

#[derive(Debug)]
struct OwnedRecord {
    content: Content,
    source: Option<Range<usize>>,
    parent: Link,
    first_child: Link,
    last_child: Link,
    previous_sibling: Link,
    next_sibling: Link,
    subtree_size: usize,
}

impl OwnedRecord {
    fn root(source_length: usize) -> Self {
        Self {
            content: Content::Document,
            source: Some(0..source_length),
            parent: Link::NONE,
            first_child: Link::NONE,
            last_child: Link::NONE,
            previous_sibling: Link::NONE,
            next_sibling: Link::NONE,
            subtree_size: 1,
        }
    }
}

#[derive(Debug)]
struct StoredName {
    qualified: Range<usize>,
    prefix_length: Option<usize>,
    namespace: Option<Range<usize>>,
}

impl StoredName {
    #[inline]
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
    nodes: Vec<OwnedRecord>,
    attributes: Vec<StoredAttribute>,
    pool: Pool,
    root_element: NodeId,
    declaration: Option<StoredDeclaration>,
    edit_owned_bytes: usize,
    // Parsing produces contiguous preorder. A future structural edit clears
    // this permanently; value-only edits leave the fast path valid.
    slots_are_preorder: bool,
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
    #[inline]
    pub fn node(&self, id: OwnedNodeId) -> Option<OwnedNode<'_>> {
        if id.document != self.identity {
            return None;
        }
        let record = self.nodes.get(id.slot.0)?;
        // Contiguous parsed slots are all live. Structural edits permanently
        // disable that invariant before any slot can become Removed.
        (self.slots_are_preorder || !matches!(record.content, Content::Removed)).then_some(
            OwnedNode {
                document: self,
                id: id.slot,
            },
        )
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
        if node.document != self.document.identity
            || self
                .document
                .nodes
                .get(node.slot.0)
                .is_none_or(|record| matches!(record.content, Content::Removed))
        {
            return Err(EditError::new(EditErrorKind::InvalidHandle, None));
        }
        Ok(node.slot.0)
    }

    /// Replaces Text, CDATA, Comment or PI data; PI targets remain unchanged.
    /// Non-text lexical values reject raw CR and their closing delimiters.
    /// Comments also reject a trailing dash; PI data rejects leading XML space.
    pub fn set_node_value(&mut self, node: OwnedNodeId, value: &str) -> Result<(), EditError> {
        let index = self.resolve(node)?;
        let (target, kind) = match &mut self.document.nodes[index].content {
            Content::Text(range) => (range, ValueKind::Text),
            Content::Cdata(range) => (range, ValueKind::Cdata),
            Content::Comment(range) => (range, ValueKind::Comment),
            Content::ProcessingInstruction { data, .. } => (data, ValueKind::Pi),
            Content::Removed | Content::Document | Content::Element { .. } => {
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
        let attributes = match &self.document.nodes[index].content {
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
    nodes: Vec<OwnedRecord>,
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
        nodes.push(OwnedRecord::root(source_length));
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
                record
                    .source
                    .as_mut()
                    .expect("parsed node has provenance")
                    .end = range.end;
                // Parsing still appends in preorder. Closing an element gives
                // its count directly, without updating every open ancestor.
                record.subtree_size = end - id.0;
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
        let id = self.append(parent, content, range)?;
        if matches!(self.nodes[id.0].content, Content::Element { .. }) {
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

    fn append(
        &mut self,
        parent: NodeId,
        content: Content,
        source: Range<usize>,
    ) -> Result<NodeId, Error> {
        let allocation = Error::new(ErrorKind::Allocation, source.start);
        let id = NodeId(self.nodes.len());
        let link = Link::new(id).ok_or(allocation)?;
        let parent_link = Link::new(parent).ok_or(allocation)?;
        self.nodes.try_reserve(1).map_err(|_| allocation)?;
        let previous_sibling = self.nodes[parent.0].last_child;
        self.nodes.push(OwnedRecord {
            content,
            source: Some(source),
            parent: parent_link,
            first_child: Link::NONE,
            last_child: Link::NONE,
            previous_sibling,
            next_sibling: Link::NONE,
            subtree_size: 1,
        });
        if let Some(previous) = self.nodes[id.0].previous_sibling.get() {
            self.nodes[previous.0].next_sibling = link;
        } else {
            self.nodes[parent.0].first_child = link;
        }
        self.nodes[parent.0].last_child = link;
        Ok(id)
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
        self.nodes[0].subtree_size = self.nodes.len();
        Ok(OwnedDocument {
            identity: document_identity(counter)?,
            nodes: self.nodes,
            attributes: self.attributes,
            pool: self.pool,
            root_element: self.root_element,
            declaration: self.declaration,
            edit_owned_bytes: 0,
            slots_are_preorder: true,
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
    #[inline]
    fn record(self) -> &'document OwnedRecord {
        &self.document.nodes[self.id.0]
    }

    pub fn id(self) -> OwnedNodeId {
        OwnedNodeId {
            document: self.document.identity,
            slot: self.id,
        }
    }

    #[inline]
    pub fn kind(self) -> OwnedNodeKind {
        match &self.record().content {
            Content::Removed => unreachable!("owned node is live"),
            Content::Document => OwnedNodeKind::Document,
            Content::Element { .. } => OwnedNodeKind::Element,
            Content::Text(_) => OwnedNodeKind::Text,
            Content::Cdata(_) => OwnedNodeKind::Cdata,
            Content::Comment(_) => OwnedNodeKind::Comment,
            Content::ProcessingInstruction { .. } => OwnedNodeKind::ProcessingInstruction,
        }
    }

    #[inline]
    pub fn name(self) -> Option<Name<'document>> {
        match &self.record().content {
            Content::Element { name, .. } => Some(name.view(&self.document.pool.0)),
            _ => None,
        }
    }

    /// Returns the direct Text, CDATA, comment or PI payload, without joining
    /// descendant text. Elements and the document root return None.
    #[inline]
    pub fn text(self) -> Option<&'document str> {
        let range = match &self.record().content {
            Content::Text(range) | Content::Cdata(range) | Content::Comment(range) => range,
            Content::ProcessingInstruction { data, .. } => data,
            _ => return None,
        };
        Some(&self.document.pool.0[range.clone()])
    }

    pub fn processing_instruction_target(self) -> Option<&'document str> {
        match &self.record().content {
            Content::ProcessingInstruction { target, .. } => {
                Some(&self.document.pool.0[target.clone()])
            }
            _ => None,
        }
    }

    /// Visits attributes in source order, including namespace declarations.
    /// Each view borrows the pool without allocating.
    #[inline]
    pub fn attributes(self) -> impl ExactSizeIterator<Item = Attribute<'document>> + 'document {
        let range = match &self.record().content {
            Content::Element { attributes, .. } => attributes.clone(),
            _ => 0..0,
        };
        range.map(move |index| self.document.attributes[index].view(&self.document.pool.0))
    }

    /// Original source location, independent of current logical content and
    /// pool storage. Parsed nodes retain Some(range); future created nodes may
    /// have no source provenance. This cannot index the owned string pool.
    #[inline]
    pub fn range(self) -> Option<Range<usize>> {
        self.record().source.clone()
    }

    pub fn parent(self) -> Option<Self> {
        self.record().parent.get().map(|id| Self {
            document: self.document,
            id,
        })
    }

    pub fn first_child(self) -> Option<Self> {
        self.record().first_child.get().map(|id| Self {
            document: self.document,
            id,
        })
    }

    pub fn last_child(self) -> Option<Self> {
        self.record().last_child.get().map(|id| Self {
            document: self.document,
            id,
        })
    }

    pub fn next_sibling(self) -> Option<Self> {
        self.record().next_sibling.get().map(|id| Self {
            document: self.document,
            id,
        })
    }

    pub fn children(self) -> impl Iterator<Item = Self> + 'document {
        std::iter::successors(self.first_child(), |node| node.next_sibling())
    }

    /// Visits this node and its descendants in preorder without recursion.
    #[inline]
    pub fn descendants(self) -> impl ExactSizeIterator<Item = Self> + 'document {
        let count = self.record().subtree_size;
        if self.document.slots_are_preorder {
            Descendants::Contiguous {
                document: self.document,
                slots: self.id.0..self.id.0 + count,
            }
        } else {
            Descendants::Linked(LinkedDescendants {
                next: Some(self),
                root: self.id,
                remaining: count,
            })
        }
    }
}

enum Descendants<'document> {
    Contiguous {
        document: &'document OwnedDocument,
        slots: Range<usize>,
    },
    Linked(LinkedDescendants<'document>),
}

impl<'document> Iterator for Descendants<'document> {
    type Item = OwnedNode<'document>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Contiguous { document, slots } => slots.next().map(|index| OwnedNode {
                document,
                id: NodeId(index),
            }),
            Self::Linked(iterator) => iterator.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Contiguous { slots, .. } => slots.size_hint(),
            Self::Linked(iterator) => iterator.size_hint(),
        }
    }

    fn count(self) -> usize {
        self.len()
    }
}

impl ExactSizeIterator for Descendants<'_> {}
impl std::iter::FusedIterator for Descendants<'_> {}

struct LinkedDescendants<'document> {
    next: Option<OwnedNode<'document>>,
    root: NodeId,
    remaining: usize,
}

impl<'document> Iterator for LinkedDescendants<'document> {
    type Item = OwnedNode<'document>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let node = self.next?;
        self.remaining -= 1;
        self.next = if self.remaining == 0 {
            None
        } else if let Some(child) = node.first_child() {
            Some(child)
        } else {
            let mut current = node;
            loop {
                if current.id == self.root {
                    break None;
                }
                if let Some(sibling) = current.next_sibling() {
                    break Some(sibling);
                }
                match current.parent() {
                    Some(parent) => current = parent,
                    None => break None,
                }
            }
        };
        Some(node)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }

    fn count(self) -> usize {
        self.remaining
    }
}

impl ExactSizeIterator for LinkedDescendants<'_> {}
impl std::iter::FusedIterator for LinkedDescendants<'_> {}

#[cfg(test)]
#[path = "../tests/internal/owned.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/internal/editor.rs"]
mod edit_tests;
