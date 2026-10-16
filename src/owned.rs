use std::borrow::Cow;
use std::ops::Range;

use crate::dom::{Record, append_record};
use crate::{Attribute, Declaration, Error, ErrorKind, Event, Limit, Limits, Name, NodeId, Reader};

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
        reader: &mut Reader<'_>,
    ) -> Result<Range<usize>, Error> {
        let start = self.0.len();
        let needed = start.checked_add(value.len()).ok_or(Error::new(
            ErrorKind::LimitExceeded(Limit::OwnedBytes),
            offset,
        ))?;
        if needed > self.0.capacity() {
            let budget = reader.owned_budget();
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
        reader: &mut Reader<'_>,
    ) -> Result<StoredName, Error> {
        Ok(StoredName {
            qualified: self.store(name.qualified, offset, reader)?,
            prefix_length: name.qualified.find(':'),
            namespace: name
                .namespace
                .as_deref()
                .map(|namespace| self.store(namespace, offset, reader))
                .transpose()?,
        })
    }
}

/// An immutable UTF-8 XML tree that owns all of its logical strings.
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

    /// Parses UTF-8 XML independently of the source's lifetime.
    pub fn parse_bytes(input: &[u8]) -> Result<Self, Error> {
        Self::with_limits(input, Limits::default())
    }

    /// Parses with aggregate string accounting for parser normalization and
    /// full new pool-capacity requests. Owned storage can exhaust this budget
    /// before the borrowing DOM; metadata and allocator overhead are separate.
    pub fn with_limits(input: &[u8], limits: Limits) -> Result<Self, Error> {
        let mut reader = Reader::with_limits(input, limits)?;
        let mut nodes = Vec::new();
        nodes
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorKind::Allocation, 0))?;
        nodes.push(Record::root(Content::Document, input.len()));
        let mut document = Self {
            nodes,
            attributes: Vec::new(),
            pool: Pool::default(),
            root_element: NodeId(0),
            declaration: None,
        };
        let mut parents = Vec::new();
        while let Some(event) = reader.read_event()? {
            let starts = matches!(&event, Event::Start(_));
            let range = reader.event_range();
            let offset = range.start;
            let parent = parents.last().copied().unwrap_or(NodeId(0));
            let content = match event {
                Event::Declaration(declaration) => {
                    document.declaration = Some(StoredDeclaration {
                        version: document
                            .pool
                            .store(declaration.version, offset, &mut reader)?,
                        encoding: declaration
                            .encoding
                            .map(|encoding| document.pool.store(encoding, offset, &mut reader))
                            .transpose()?,
                        standalone: declaration.standalone,
                    });
                    continue;
                }
                Event::End(_) => {
                    let id = parents
                        .pop()
                        .ok_or_else(|| Error::new(ErrorKind::MismatchedEndTag, offset))?;
                    let end = document.nodes.len();
                    let record = &mut document.nodes[id.0];
                    record.range.end = range.end;
                    record.subtree_end = end;
                    continue;
                }
                Event::Start(element) | Event::Empty(element) => {
                    let name = document.pool.name(&element.name, offset, &mut reader)?;
                    let start = document.attributes.len();
                    document
                        .attributes
                        .try_reserve(element.attributes.len())
                        .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
                    for attribute in element.attributes {
                        document.attributes.push(StoredAttribute {
                            name: document.pool.name(&attribute.name, offset, &mut reader)?,
                            value: document.pool.store(&attribute.value, offset, &mut reader)?,
                        });
                    }
                    Content::Element {
                        name,
                        attributes: start..document.attributes.len(),
                    }
                }
                Event::Text(value) => {
                    Content::Text(document.pool.store(&value, offset, &mut reader)?)
                }
                Event::Cdata(value) => {
                    Content::Cdata(document.pool.store(&value, offset, &mut reader)?)
                }
                Event::Comment(value) => {
                    Content::Comment(document.pool.store(&value, offset, &mut reader)?)
                }
                Event::ProcessingInstruction { target, data } => Content::ProcessingInstruction {
                    target: document.pool.store(target, offset, &mut reader)?,
                    data: document.pool.store(&data, offset, &mut reader)?,
                },
            };
            let id = append_record(&mut document.nodes, parent, content, range)?;
            if matches!(document.nodes[id.0].kind, Content::Element { .. }) {
                if parent == NodeId(0) {
                    document.root_element = id;
                }
                if starts {
                    parents
                        .try_reserve(1)
                        .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
                    parents.push(id);
                }
            }
        }
        if document.root_element == NodeId(0) {
            return Err(Error::new(ErrorKind::MissingRoot, reader.position()));
        }
        document.nodes[0].subtree_end = document.nodes.len();
        Ok(document)
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
