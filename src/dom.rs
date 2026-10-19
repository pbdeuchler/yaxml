use std::borrow::Cow;
use std::ops::Range;

use crate::{
    Attribute, Declaration, Doctype, Element, Error, ErrorKind, Event, Limits, Name, Reader,
    SourceOrigin,
};

/// A document-local node index. IDs must only be used with their originating document.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId(pub(crate) usize);

/// The content of a DOM node. XML declarations are stored on the document.
#[derive(Debug)]
pub enum NodeKind<'input> {
    Document,
    /// The document type declaration, a child of the document root.
    Doctype(Doctype<'input>),
    Element(Element<'input>),
    Text(Cow<'input, str>),
    Cdata(Cow<'input, str>),
    Comment(Cow<'input, str>),
    ProcessingInstruction {
        target: Cow<'input, str>,
        data: Cow<'input, str>,
    },
}

#[derive(Debug)]
pub(crate) struct Record<K> {
    pub(crate) kind: K,
    pub(crate) origin: SourceOrigin,
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
            origin: SourceOrigin::input(0..length),
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
    origin: SourceOrigin,
) -> Result<NodeId, Error> {
    nodes
        .try_reserve(1)
        .map_err(|_| Error::new(ErrorKind::Allocation, origin.error_offset()))?;
    let id = NodeId(nodes.len());
    nodes.push(Record {
        kind,
        origin,
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
            let origin = reader
                .event_origin()
                .ok_or_else(|| Error::new(ErrorKind::InvalidState, reader.position()))?;
            let parent = parents.last().copied().unwrap_or(NodeId(0));
            match event {
                Event::Declaration(declaration) => document.declaration = Some(declaration),
                Event::Doctype(doctype) => {
                    document.append(parent, NodeKind::Doctype(doctype), origin)?;
                }
                Event::End(_) => {
                    let id = parents.last().copied().ok_or_else(|| {
                        Error::new(ErrorKind::MismatchedEndTag, origin.error_offset())
                    })?;
                    let subtree_end = document.nodes.len();
                    let record = &mut document.nodes[id.0];
                    record.origin = record.origin.close_element(origin)?;
                    parents.pop();
                    record.subtree_end = subtree_end;
                }
                Event::Start(element) => {
                    parents
                        .try_reserve(1)
                        .map_err(|_| Error::new(ErrorKind::Allocation, origin.error_offset()))?;
                    let id = document.append(parent, NodeKind::Element(element), origin)?;
                    if parent == NodeId(0) {
                        document.root_element = id;
                    }
                    parents.push(id);
                }
                Event::Empty(element) => {
                    let id = document.append(parent, NodeKind::Element(element), origin)?;
                    if parent == NodeId(0) {
                        document.root_element = id;
                    }
                }
                Event::Text(value) => {
                    document.append_text(parent, value, origin, reader.owned_budget())?;
                }
                Event::Cdata(value) => {
                    document.append(parent, NodeKind::Cdata(value), origin)?;
                }
                Event::Comment(value) => {
                    document.append(parent, NodeKind::Comment(value), origin)?;
                }
                Event::ProcessingInstruction { target, data } => {
                    document.append(
                        parent,
                        NodeKind::ProcessingInstruction { target, data },
                        origin,
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
        origin: SourceOrigin,
    ) -> Result<NodeId, Error> {
        append_record(&mut self.nodes, parent, kind, origin)
    }

    fn append_text(
        &mut self,
        parent: NodeId,
        value: Cow<'input, str>,
        origin: SourceOrigin,
        budget: &mut crate::parser::Budget,
    ) -> Result<Option<NodeId>, Error> {
        let last = self.nodes[parent.0].last_child;
        if value.is_empty() {
            return Ok(last.filter(|id| matches!(self.nodes[id.0].kind, NodeKind::Text(_))));
        }
        let Some(last) = last else {
            return self.append(parent, NodeKind::Text(value), origin).map(Some);
        };
        let record = &mut self.nodes[last.0];
        let NodeKind::Text(previous) = &mut record.kind else {
            return self.append(parent, NodeKind::Text(value), origin).map(Some);
        };
        let joined = record.origin.join_text(origin);
        append_text_value(previous, &value, origin.error_offset(), budget)?;
        record.origin = joined;
        Ok(Some(last))
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

    /// Returns the document type declaration when present.
    pub fn doctype(&self) -> Option<&Doctype<'input>> {
        self.root().children().find_map(|node| match node.kind() {
            NodeKind::Doctype(doctype) => Some(doctype),
            _ => None,
        })
    }
}

fn append_text_value<'input>(
    previous: &mut Cow<'input, str>,
    incoming: &str,
    offset: usize,
    budget: &mut crate::parser::Budget,
) -> Result<(), Error> {
    let needed = previous
        .len()
        .checked_add(incoming.len())
        .ok_or(Error::new(
            ErrorKind::LimitExceeded(crate::Limit::OwnedBytes),
            offset,
        ))?;
    match previous {
        Cow::Borrowed(prefix) => {
            let mut joined = String::new();
            reserve_text(&mut joined, needed, offset, budget)?;
            joined.push_str(prefix);
            joined.push_str(incoming);
            *previous = Cow::Owned(joined);
        }
        Cow::Owned(value) => {
            reserve_text(value, needed, offset, budget)?;
            value.push_str(incoming);
        }
    }
    Ok(())
}

fn reserve_text(
    value: &mut String,
    needed: usize,
    offset: usize,
    budget: &mut crate::parser::Budget,
) -> Result<(), Error> {
    if needed <= value.capacity() {
        return Ok(());
    }
    let target = value
        .capacity()
        .saturating_mul(2)
        .max(needed)
        .min(budget.remaining_owned());
    if target < needed {
        return Err(Error::new(
            ErrorKind::LimitExceeded(crate::Limit::OwnedBytes),
            offset,
        ));
    }
    budget.charge_owned(target, offset)?;
    value
        .try_reserve_exact(target - value.len())
        .map_err(|_| Error::new(ErrorKind::Allocation, offset))
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
            NodeKind::Document | NodeKind::Doctype(_) | NodeKind::Element(_) => None,
        }
    }

    /// Visits the node's string value as borrowed chunks without allocating.
    ///
    /// Documents and elements yield descendant Text/CDATA payloads in document
    /// order, ignoring comments and PIs. Leaves yield their own payload once,
    /// including an empty payload. Chunk boundaries are retained; `text()`
    /// continues to return only a direct leaf payload.
    ///
    /// ```
    /// let document = yaxml::Document::parse("<r>a<c>b</c><!--skip--><![CDATA[c]]></r>")?;
    /// assert_eq!(document.root_element().text_chunks().collect::<String>(), "abc");
    /// # Ok::<(), yaxml::Error>(())
    /// ```
    pub fn text_chunks(self) -> impl Iterator<Item = &'document str> + 'document {
        let descendants = self
            .descendants()
            .skip(1)
            .filter_map(|node| match node.kind() {
                NodeKind::Text(value) | NodeKind::Cdata(value) => Some(value.as_ref()),
                _ => None,
            });
        self.text().into_iter().chain(descendants)
    }

    /// Returns an element's attributes in source order, including namespace declarations.
    pub fn attributes(self) -> &'document [Attribute<'input>] {
        match self.kind() {
            NodeKind::Element(element) => &element.attributes,
            _ => &[],
        }
    }

    /// Exact original input span, including delimiters and the element's end tag.
    /// The range is independent of the node's normalized logical value.
    pub fn range(self) -> Option<Range<usize>> {
        self.origin().input_range()
    }

    /// Parsed origin, independent of the node's logical value.
    pub fn origin(self) -> SourceOrigin {
        self.record().origin
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

#[cfg(test)]
#[path = "../tests/internal/dom_text_join_author.rs"]
mod text_join_author_tests;

#[cfg(test)]
#[path = "../tests/internal/dom_text_join_spec.rs"]
mod text_join_spec_tests;
