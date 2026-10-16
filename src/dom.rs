use std::borrow::Cow;
use std::ops::Range;

use crate::{Attribute, Declaration, Element, Error, ErrorKind, Event, Limits, Name, Reader};

/// A document-local node index. IDs must only be used with their originating document.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId(pub(crate) usize);

/// The content of a DOM node. XML declarations are stored on the document.
#[derive(Debug)]
pub enum NodeKind<'input> {
    Document,
    Element(Element<'input>),
    Text(Cow<'input, str>),
    Cdata(Cow<'input, str>),
    Comment(Cow<'input, str>),
    ProcessingInstruction {
        target: &'input str,
        data: Cow<'input, str>,
    },
}

#[derive(Debug)]
pub(crate) struct Record<K> {
    pub(crate) kind: K,
    pub(crate) range: Range<usize>,
    pub(crate) parent: Option<NodeId>,
    pub(crate) first_child: Option<NodeId>,
    pub(crate) last_child: Option<NodeId>,
    pub(crate) next_sibling: Option<NodeId>,
    pub(crate) subtree_end: usize,
}

impl<K> Record<K> {
    pub(crate) fn root(kind: K, length: usize) -> Self {
        Self {
            kind,
            range: 0..length,
            parent: None,
            first_child: None,
            last_child: None,
            next_sibling: None,
            subtree_end: 1,
        }
    }
}

pub(crate) fn append_record<K>(
    nodes: &mut Vec<Record<K>>,
    parent: NodeId,
    kind: K,
    range: Range<usize>,
) -> Result<NodeId, Error> {
    nodes
        .try_reserve(1)
        .map_err(|_| Error::new(ErrorKind::Allocation, range.start))?;
    let id = NodeId(nodes.len());
    nodes.push(Record {
        kind,
        range,
        parent: Some(parent),
        first_child: None,
        last_child: None,
        next_sibling: None,
        subtree_end: id.0 + 1,
    });
    if let Some(previous) = nodes[parent.0].last_child {
        nodes[previous.0].next_sibling = Some(id);
    } else {
        nodes[parent.0].first_child = Some(id);
    }
    nodes[parent.0].last_child = Some(id);
    Ok(id)
}

/// An immutable, flat XML tree that borrows unchanged strings from its input.
///
/// Parsing and destruction use no recursion. A successful document has one
/// element beneath its synthetic document root.
#[derive(Debug)]
pub struct Document<'input> {
    nodes: Vec<Record<NodeKind<'input>>>,
    root_element: NodeId,
    declaration: Option<Declaration<'input>>,
}

impl<'input> Document<'input> {
    /// Parses UTF-8 XML with the default resource limits.
    pub fn parse(input: &'input str) -> Result<Self, Error> {
        Self::parse_bytes(input.as_bytes())
    }

    /// Parses XML bytes, rejecting invalid UTF-8 and unsupported encodings.
    pub fn parse_bytes(input: &'input [u8]) -> Result<Self, Error> {
        Self::with_limits(input, Limits::default())
    }

    /// Parses a complete input with explicit resource ceilings.
    pub fn with_limits(input: &'input [u8], limits: Limits) -> Result<Self, Error> {
        let mut reader = Reader::with_limits(input, limits)?;
        let mut nodes = Vec::new();
        nodes
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorKind::Allocation, 0))?;
        nodes.push(Record::root(NodeKind::Document, input.len()));
        let mut document = Self {
            nodes,
            root_element: NodeId(0),
            declaration: None,
        };
        let mut parents = Vec::new();
        while let Some(event) = reader.read_event()? {
            let range = reader.event_range();
            let parent = parents.last().copied().unwrap_or(NodeId(0));
            match event {
                Event::Declaration(declaration) => document.declaration = Some(declaration),
                Event::End(_) => {
                    let id = parents
                        .pop()
                        .ok_or_else(|| Error::new(ErrorKind::MismatchedEndTag, range.start))?;
                    let subtree_end = document.nodes.len();
                    let record = &mut document.nodes[id.0];
                    record.range.end = range.end;
                    record.subtree_end = subtree_end;
                }
                Event::Start(element) => {
                    parents
                        .try_reserve(1)
                        .map_err(|_| Error::new(ErrorKind::Allocation, range.start))?;
                    let id = document.append(parent, NodeKind::Element(element), range)?;
                    if parent == NodeId(0) {
                        document.root_element = id;
                    }
                    parents.push(id);
                }
                Event::Empty(element) => {
                    let id = document.append(parent, NodeKind::Element(element), range)?;
                    if parent == NodeId(0) {
                        document.root_element = id;
                    }
                }
                Event::Text(value) => {
                    document.append(parent, NodeKind::Text(value), range)?;
                }
                Event::Cdata(value) => {
                    document.append(parent, NodeKind::Cdata(value), range)?;
                }
                Event::Comment(value) => {
                    document.append(parent, NodeKind::Comment(value), range)?;
                }
                Event::ProcessingInstruction { target, data } => {
                    document.append(
                        parent,
                        NodeKind::ProcessingInstruction { target, data },
                        range,
                    )?;
                }
            }
        }
        if document.root_element == NodeId(0) {
            return Err(Error::new(ErrorKind::MissingRoot, reader.position()));
        }
        document.nodes[0].subtree_end = document.nodes.len();
        Ok(document)
    }

    fn append(
        &mut self,
        parent: NodeId,
        kind: NodeKind<'input>,
        range: Range<usize>,
    ) -> Result<NodeId, Error> {
        append_record(&mut self.nodes, parent, kind, range)
    }

    /// Returns the synthetic document root, including surrounding comments and PIs.
    pub fn root(&self) -> Node<'_, 'input> {
        Node {
            document: self,
            id: NodeId(0),
        }
    }

    /// Returns the single document element, guaranteed by successful parsing.
    pub fn root_element(&self) -> Node<'_, 'input> {
        Node {
            document: self,
            id: self.root_element,
        }
    }

    /// Looks up a document-local ID, returning `None` when it is out of range.
    ///
    /// IDs carry no document identity. An in-range ID from a different document
    /// can refer to an unrelated node; callers must retain the originating document.
    pub fn node(&self, id: NodeId) -> Option<Node<'_, 'input>> {
        self.nodes.get(id.0).map(|_| Node { document: self, id })
    }

    /// Visits all nodes in document order, including the synthetic root.
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = Node<'_, 'input>> + '_ {
        self.root().descendants()
    }

    /// Returns the XML declaration when present.
    pub fn declaration(&self) -> Option<&Declaration<'input>> {
        self.declaration.as_ref()
    }
}

/// A read-only node view whose lifetime is bounded by its document.
#[derive(Clone, Copy, Debug)]
pub struct Node<'document, 'input> {
    document: &'document Document<'input>,
    id: NodeId,
}

impl<'document, 'input> Node<'document, 'input> {
    fn record(self) -> &'document Record<NodeKind<'input>> {
        &self.document.nodes[self.id.0]
    }

    /// Returns this node's document-local ID.
    pub fn id(self) -> NodeId {
        self.id
    }

    /// Returns the node content without allocating or copying normalized strings.
    pub fn kind(self) -> &'document NodeKind<'input> {
        &self.record().kind
    }

    /// Returns an element's name, or `None` for other node kinds.
    pub fn name(self) -> Option<&'document Name<'input>> {
        match self.kind() {
            NodeKind::Element(element) => Some(&element.name),
            _ => None,
        }
    }

    /// Returns a text, CDATA, comment, or PI payload.
    ///
    /// Elements and the document root return `None`; this method does not join
    /// descendant text or search for a first text child.
    pub fn text(self) -> Option<&'document str> {
        match self.kind() {
            NodeKind::Text(value) | NodeKind::Cdata(value) | NodeKind::Comment(value) => {
                Some(value)
            }
            NodeKind::ProcessingInstruction { data, .. } => Some(data),
            NodeKind::Document | NodeKind::Element(_) => None,
        }
    }

    /// Returns an element's attributes in source order, including namespace declarations.
    pub fn attributes(self) -> &'document [Attribute<'input>] {
        match self.kind() {
            NodeKind::Element(element) => &element.attributes,
            _ => &[],
        }
    }

    /// Returns the source byte range, including delimiters and the element's end tag.
    pub fn range(self) -> Range<usize> {
        self.record().range.clone()
    }

    /// Returns the parent; only the synthetic document root has no parent.
    pub fn parent(self) -> Option<Self> {
        self.record().parent.map(|id| Self {
            document: self.document,
            id,
        })
    }

    /// Returns the first direct child, if any.
    pub fn first_child(self) -> Option<Self> {
        self.record().first_child.map(|id| Self {
            document: self.document,
            id,
        })
    }

    /// Returns the last direct child, if any.
    pub fn last_child(self) -> Option<Self> {
        self.record().last_child.map(|id| Self {
            document: self.document,
            id,
        })
    }

    /// Returns the next direct sibling, if any.
    pub fn next_sibling(self) -> Option<Self> {
        self.record().next_sibling.map(|id| Self {
            document: self.document,
            id,
        })
    }

    /// Iterates direct children in source order without a traversal stack.
    pub fn children(self) -> impl Iterator<Item = Self> + 'document {
        std::iter::successors(self.first_child(), |node| node.next_sibling())
    }

    /// Visits this node and its descendants in preorder, without recursion.
    pub fn descendants(self) -> impl ExactSizeIterator<Item = Self> + 'document {
        (self.id.0..self.record().subtree_end).map(move |index| Self {
            document: self.document,
            id: NodeId(index),
        })
    }
}
