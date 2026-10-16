use std::borrow::Cow;
use std::ops::Range;

use crate::dom::{Record, append_record};
use crate::parser::Budget;
use crate::{Attribute, Declaration, Error, ErrorKind, Event, Limit, Limits, Name, NodeId, Reader};
use crate::{DecodeLimits, PushParser};

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

/// An immutable XML tree that owns its logical strings as UTF-8.
///
/// Nodes and attributes contain ranges into one string pool, so moving the
/// document or growing its storage creates no self-references. Parsing,
/// traversal and destruction are iterative. Source bytes are not retained.
#[derive(Debug)]
pub struct OwnedDocument {
    nodes: Vec<Record<Content>>,
    attributes: Vec<StoredAttribute>,
    pool: Pool,
    root_element: NodeId,
    declaration: Option<StoredDeclaration>,
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

    /// Looks up a document-local ID. IDs have no document identity; an in-range
    /// foreign ID can designate another node. Out-of-range IDs return None.
    pub fn node(&self, id: NodeId) -> Option<OwnedNode<'_>> {
        self.nodes
            .get(id.0)
            .map(|_| OwnedNode { document: self, id })
    }

    pub fn nodes(&self) -> impl ExactSizeIterator<Item = OwnedNode<'_>> + '_ {
        self.root().descendants()
    }

    pub fn declaration(&self) -> Option<Declaration<'_>> {
        self.declaration
            .as_ref()
            .map(|declaration| declaration.view(&self.pool.0))
    }
}

/// Concrete shared sink: all strings are copied using the parser's budget.
struct OwnedBuilder {
    document: OwnedDocument,
    parents: Vec<NodeId>,
}

impl OwnedBuilder {
    fn new(source_length: usize) -> Result<Self, Error> {
        let mut nodes = Vec::new();
        nodes
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorKind::Allocation, 0))?;
        nodes.push(Record::root(Content::Document, source_length));
        let document = OwnedDocument {
            nodes,
            attributes: Vec::new(),
            pool: Pool::default(),
            root_element: NodeId(0),
            declaration: None,
        };
        Ok(Self {
            document,
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
                self.document.declaration = Some(StoredDeclaration {
                    version: self
                        .document
                        .pool
                        .store(declaration.version, offset, budget)?,
                    encoding: declaration
                        .encoding
                        .map(|encoding| self.document.pool.store(encoding, offset, budget))
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
                let end = self.document.nodes.len();
                let record = &mut self.document.nodes[id.0];
                record.range.end = range.end;
                record.subtree_end = end;
                return Ok(());
            }
            Event::Start(element) | Event::Empty(element) => {
                let name = self.document.pool.name(&element.name, offset, budget)?;
                let start = self.document.attributes.len();
                self.document
                    .attributes
                    .try_reserve(element.attributes.len())
                    .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
                for attribute in element.attributes {
                    self.document.attributes.push(StoredAttribute {
                        name: self.document.pool.name(&attribute.name, offset, budget)?,
                        value: self.document.pool.store(&attribute.value, offset, budget)?,
                    });
                }
                Content::Element {
                    name,
                    attributes: start..self.document.attributes.len(),
                }
            }
            Event::Text(value) => Content::Text(self.document.pool.store(&value, offset, budget)?),
            Event::Cdata(value) => {
                Content::Cdata(self.document.pool.store(&value, offset, budget)?)
            }
            Event::Comment(value) => {
                Content::Comment(self.document.pool.store(&value, offset, budget)?)
            }
            Event::ProcessingInstruction { target, data } => Content::ProcessingInstruction {
                target: self.document.pool.store(target, offset, budget)?,
                data: self.document.pool.store(&data, offset, budget)?,
            },
        };
        let id = append_record(&mut self.document.nodes, parent, content, range)?;
        if matches!(self.document.nodes[id.0].kind, Content::Element { .. }) {
            if parent == NodeId(0) {
                self.document.root_element = id;
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

    fn finish(mut self, position: usize) -> Result<OwnedDocument, Error> {
        if self.document.root_element == NodeId(0) {
            return Err(Error::new(ErrorKind::MissingRoot, position));
        }
        self.document.nodes[0].subtree_end = self.document.nodes.len();
        Ok(self.document)
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

    pub fn id(self) -> NodeId {
        self.id
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

    /// Original source byte range, independent of normalized pool storage.
    pub fn range(self) -> Range<usize> {
        self.record().range.clone()
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
