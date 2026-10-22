use std::borrow::Cow;
use std::collections::TryReserveError;
use std::ops::Range;

use crate::options::Flags;
use crate::parser::Budget;
use crate::xpath::tree::{self, NamespaceBinding};
use crate::{
    Attribute, Declaration, Doctype, Element, Error, ErrorKind, Event, Limits, Name, ParseOptions,
    Reader, SourceOrigin,
};

/// A node's index in its [`Document`]; meaningless in any other document.
///
/// Stored as index + 1 so `Option<NodeId>`, a node record's every link, is
/// one word.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId(std::num::NonZeroUsize);

impl NodeId {
    /// The ID of the node at `index`.
    pub(crate) const fn new(index: usize) -> Self {
        match std::num::NonZeroUsize::new(index.wrapping_add(1)) {
            Some(id) => Self(id),
            None => panic!("node index overflow"),
        }
    }

    /// The node's index.
    pub(crate) const fn index(self) -> usize {
        self.0.get() - 1
    }
}

impl std::fmt::Debug for NodeId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("NodeId")
            .field(&self.index())
            .finish()
    }
}

/// The content of a [`Node`]. The XML declaration lives on the [`Document`].
#[derive(Debug)]
pub enum NodeKind<'input> {
    /// The synthetic document root.
    Document,
    /// The document type declaration, a child of the document root.
    Doctype(Doctype<'input>),
    /// An element.
    Element(Element<'input>),
    /// Character data.
    Text(Cow<'input, str>),
    /// A CDATA section.
    Cdata(Cow<'input, str>),
    /// A comment.
    Comment(Cow<'input, str>),
    /// A processing instruction.
    ProcessingInstruction {
        /// The target name.
        target: Cow<'input, str>,
        /// Everything after the target and its separating whitespace.
        data: Cow<'input, str>,
    },
    /// A kept general entity reference (libxml2's `XML_ENTITY_REF_NODE`),
    /// parsed without `NOENT`. Its children are the replacement text's nodes;
    /// an unloaded or undeclared entity has none.
    EntityRef(Cow<'input, str>),
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
    let id = NodeId::new(nodes.len());
    nodes.push(Record {
        kind,
        origin,
        parent: Some(parent),
        first_child: None,
        last_child: None,
        next_sibling: None,
        subtree_end: id.index() + 1,
    });
    if let Some(previous) = nodes[parent.index()].last_child {
        nodes[previous.index()].next_sibling = Some(id);
    } else {
        nodes[parent.index()].first_child = Some(id);
    }
    nodes[parent.index()].last_child = Some(id);
    Ok(id)
}

/// An immutable XML tree that borrows unchanged strings from its input.
///
/// Parsing and dropping never recurse, so depth is bounded only by
/// [`Limits::max_depth`]. Use [`OwnedDocument`](crate::OwnedDocument) to
/// edit a tree or keep it past its input.
///
/// ```
/// use yaxml::Document;
///
/// let document = Document::parse("<r xmlns='urn:x'><item key='v'>a&amp;b</item></r>")?;
/// let item = document.root_element().first_child().unwrap();
/// assert_eq!(item.name().unwrap().namespace(), Some("urn:x"));
/// assert_eq!(item.attribute("key"), Some("v"));
/// assert_eq!(item.content().as_deref(), Some("a&b"));
/// # Ok::<(), yaxml::Error>(())
/// ```
#[derive(Debug)]
pub struct Document<'input> {
    nodes: Vec<Record<NodeKind<'input>>>,
    root_element: NodeId,
    declaration: Option<Declaration<'input>>,
    /// `(element, attribute index)` of ID-typed attributes, in document order.
    ids: Vec<(NodeId, usize)>,
    /// `(element, attribute index, form)` of attribute values that keep
    /// general entity references (no `NOENT`), in document order; `save`
    /// writes the form, everything else reads the substituted value.
    kept: Vec<(NodeId, usize, String)>,
    dtd: Option<Box<crate::valid::Dtd>>,
    validity: Vec<crate::valid::Diagnostic<NodeId>>,
    errors: Vec<Error>,
    /// `ParseOptions::base`: libxml2's `doc->URL`.
    url: Option<String>,
}

impl<'input> Document<'input> {
    /// Parses UTF-8 XML with default options.
    pub fn parse(input: &'input str) -> Result<Self, Error> {
        Self::parse_bytes(input.as_bytes())
    }

    /// Parses XML bytes, rejecting invalid UTF-8 and unsupported encodings.
    pub fn parse_bytes(input: &'input [u8]) -> Result<Self, Error> {
        Self::with_limits(input, Limits::default())
    }

    /// Parses with custom limits and no flags.
    pub fn with_limits(input: &'input [u8], limits: Limits) -> Result<Self, Error> {
        Self::parse_with(input, ParseOptions::default().with_limits(limits))
    }

    /// Parses with libxml2 parser options; see [`crate::options`].
    /// `XINCLUDE` needs an owned tree and is `UnsupportedOption` here.
    pub fn parse_with(input: &'input [u8], options: ParseOptions) -> Result<Self, Error> {
        Self::borrowing(input, None, options)
    }

    /// Parses [`Transcoded`](crate::Transcoded) input with libxml2 parser
    /// options, as [`Reader::transcoded`] reads it.
    pub fn parse_transcoded(
        input: &'input crate::Transcoded<'_>,
        options: ParseOptions,
    ) -> Result<Self, Error> {
        Self::borrowing(input.as_bytes(), input.decoded.as_ref(), options)
    }

    fn borrowing(
        input: &'input [u8],
        decoded: Option<&(crate::encoding::SourceEncoding, Option<Error>)>,
        options: ParseOptions,
    ) -> Result<Self, Error> {
        let flags = options.check(true, crate::options::XINCLUDE)?;
        let url = options.base.clone();
        // About one node per `<`: reserving up front saves the Vec's regrowth
        // copies (-6..8% on SVG-like input). A hint only, within `max_events`.
        let nodes = memchr::memchr_iter(b'<', input)
            .count()
            .min(options.limits.max_events);
        let reader = Reader::borrowing(input, decoded, flags, options)?;
        let document = Self::from_reader(reader, input.len(), nodes, flags)?;
        Ok(Self { url, ..document })
    }

    /// The document URL (`ParseOptions::base`, libxml2's `doc->URL`), against
    /// which [`Node::base_uri`] resolves.
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    fn from_reader(
        mut reader: Reader<'input>,
        length: usize,
        hint: usize,
        flags: Flags,
    ) -> Result<Self, Error> {
        let mut nodes = Vec::new();
        (nodes.try_reserve(hint + 1))
            .or_else(|_| nodes.try_reserve(1))
            .map_err(|_| Error::new(ErrorKind::Allocation, 0))?;
        nodes.push(Record::root(NodeKind::Document, length));
        let mut document = Self {
            nodes,
            root_element: NodeId::new(0),
            declaration: None,
            ids: Vec::new(),
            kept: Vec::new(),
            dtd: None,
            validity: Vec::new(),
            errors: Vec::new(),
            url: None,
        };
        let mut parents = Vec::new();
        let end = loop {
            let event = match reader.read_event() {
                Ok(Some(event)) => event,
                Ok(None) => break Ok(()),
                Err(error) => break Err(error),
            };
            let origin = reader
                .event_origin()
                .ok_or_else(|| Error::new(ErrorKind::InvalidState, reader.position()))?;
            let parent = parents.last().copied().unwrap_or(NodeId::new(0));
            let opens = matches!(event, Event::Start(_));
            match event {
                Event::Declaration(declaration) => document.declaration = Some(declaration),
                Event::Doctype(doctype) => {
                    document.append(parent, NodeKind::Doctype(doctype), origin)?;
                }
                // The parser balances ends, so `parent` is what they close.
                Event::End(_) => {
                    let subtree_end = document.nodes.len();
                    let record = &mut document.nodes[parent.index()];
                    record.origin = record.origin.close_element(origin);
                    parents.pop();
                    record.subtree_end = subtree_end;
                }
                Event::EntityStart(name) => {
                    parents
                        .try_reserve(1)
                        .map_err(|_| Error::new(ErrorKind::Allocation, origin.error_offset()))?;
                    parents.push(document.append(parent, NodeKind::EntityRef(name), origin)?);
                }
                Event::EntityEnd => {
                    parents.pop();
                    document.nodes[parent.index()].subtree_end = document.nodes.len();
                }
                Event::Start(element) | Event::Empty(element) => {
                    if opens {
                        crate::parser::reserve_frame(&mut parents, origin.error_offset())?;
                    }
                    let id = NodeId::new(document.nodes.len());
                    let kept = reader.owned_budget().kept();
                    if !kept.is_empty() {
                        let offset = origin.error_offset();
                        document
                            .kept
                            .try_reserve(kept.len())
                            .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
                        let kept = kept.drain(..).map(|(index, form)| (id, index, form));
                        document.kept.extend(kept);
                    }
                    reader
                        .owned_budget()
                        .charge_attributes(element.attributes.len(), origin.error_offset())?;
                    document.append_element(parent, element, origin)?;
                    if parent == NodeId::new(0) {
                        document.root_element = id;
                    }
                    if opens {
                        parents.push(id);
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
        };
        // Recovery stops at an error no site repairs; the reader closed what
        // was open.
        document.errors = Budget::conclude(reader.budget(), end)?;
        // Recovery keeps a document without an element, as libxml2 does
        // unless the input is empty (otherwise the reader failed already).
        if document.root_element == NodeId::new(0) && length == 0 {
            let missing = Error::new(ErrorKind::MissingRoot, reader.position());
            return Err(document.errors.first().copied().unwrap_or(missing));
        }
        document.nodes[0].subtree_end = document.nodes.len();
        let dtd = reader.take_dtd().map(Box::new);
        let mut document = Self { dtd, ..document };
        if flags.drops_blanks() {
            let refused = |_| Error::new(ErrorKind::Allocation, reader.position());
            let blanks = crate::xpath::tree::blanks(&document, false).map_err(refused)?;
            document.remove_leaves(&blanks).map_err(refused)?;
        }
        // Without DTD-declared ID attributes or removed nodes, the `xml:id`s
        // noted while building are exactly what a full traversal would find.
        let traverse = flags.drops_blanks() || crate::xpath::tree::declares_ids(document.dtd());
        document.ids = match (flags.records_ids(), traverse) {
            (false, _) => Vec::new(),
            (true, true) => crate::xpath::tree::find_ids(&document, document.dtd())
                .map_err(|_| Error::new(ErrorKind::Allocation, reader.position()))?,
            (true, false) => std::mem::take(&mut document.ids),
        };
        if flags.validates() {
            document.validity = document.validate();
        }
        Ok(document)
    }

    /// Removes leaves (sorted, never the root or an element with children),
    /// keeping preorder storage and relinking the survivors.
    fn remove_leaves(&mut self, leaves: &[NodeId]) -> Result<(), TryReserveError> {
        if leaves.is_empty() {
            return Ok(());
        }
        // `kept[i]`: survivors before slot `i`, which is slot `i`'s new index.
        let mut kept = Vec::new();
        kept.try_reserve_exact(self.nodes.len() + 1)?;
        let mut removed = leaves.iter().peekable();
        let mut count = 0;
        for index in 0..=self.nodes.len() {
            kept.push(count);
            match removed.peek() {
                Some(leaf) if leaf.index() == index => drop(removed.next()),
                _ => count += 1,
            }
        }
        let map = |id: NodeId| NodeId::new(kept[id.index()]);
        let mut slot = 0;
        self.nodes.retain_mut(|record| {
            let keep = kept[slot + 1] != kept[slot];
            slot += 1;
            record.parent = record.parent.map(map);
            record.subtree_end = kept[record.subtree_end];
            (record.first_child, record.last_child, record.next_sibling) = (None, None, None);
            keep
        });
        for index in 1..self.nodes.len() {
            let parent = self.nodes[index].parent.expect("non-root parent").index();
            match self.nodes[parent].last_child {
                Some(previous) => {
                    self.nodes[previous.index()].next_sibling = Some(NodeId::new(index))
                }
                None => self.nodes[parent].first_child = Some(NodeId::new(index)),
            }
            self.nodes[parent].last_child = Some(NodeId::new(index));
        }
        self.root_element = map(self.root_element);
        for (element, _, _) in &mut self.kept {
            *element = map(*element);
        }
        Ok(())
    }

    /// The errors the parse kept, in the order found (at most 100, like
    /// libxml2): namespace errors and warnings, which never stop a parse
    /// (libxml2's `xmlNsErr`, `ErrorKind::Warning`), and under `RECOVER` every
    /// well-formedness error recovered from; empty for a namespace-well-formed
    /// document.
    pub fn errors(&self) -> &[Error] {
        &self.errors
    }

    /// Validity diagnostics found by `DTDVALID`; empty without it.
    pub fn validity(&self) -> &[crate::valid::Diagnostic<NodeId>] {
        &self.validity
    }

    /// The declarations of the internal and loaded external subsets.
    pub fn dtd(&self) -> Option<&crate::valid::Dtd> {
        self.dtd.as_deref()
    }

    /// Checks the document against its DTD, as libxml2's `XML_PARSE_DTDVALID`.
    /// Never fails: an empty result means valid; see [`crate::valid`].
    pub fn validate(&self) -> Vec<crate::valid::Diagnostic<NodeId>> {
        crate::valid::validate_document(self, self.dtd())
    }

    /// The element whose `xml:id` or DTD-declared `ID` attribute has this
    /// value; the first in document order wins, as in libxml2's `xmlGetID`.
    pub fn element_by_id(&self, value: &str) -> Option<Node<'_, 'input>> {
        crate::xpath::tree::element_by_id(self, &self.ids, value).and_then(|id| self.node(id))
    }

    pub(crate) fn ids(&self) -> &[(NodeId, usize)] {
        &self.ids
    }

    /// The form of `element`'s attribute `index` with its general entity
    /// references kept, when it has any.
    fn kept(&self, element: NodeId, index: usize) -> Option<&str> {
        let key = (element, index);
        let found = self
            .kept
            .binary_search_by(|(id, at, _)| (*id, *at).cmp(&key));
        found.ok().map(|found| self.kept[found].2.as_str())
    }

    /// Appends an element, noting its `xml:id` attributes in document order.
    fn append_element(
        &mut self,
        parent: NodeId,
        element: Element<'input>,
        origin: SourceOrigin,
    ) -> Result<NodeId, Error> {
        let id = NodeId::new(self.nodes.len());
        for (index, attribute) in element.attributes.iter().enumerate() {
            if attribute.name.qualified() == "xml:id" {
                self.ids
                    .try_reserve(1)
                    .map_err(|_| Error::new(ErrorKind::Allocation, origin.error_offset()))?;
                self.ids.push((id, index));
            }
        }
        self.append(parent, NodeKind::Element(element), origin)
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
        let last = self.nodes[parent.index()].last_child;
        if value.is_empty() {
            return Ok(last.filter(|id| matches!(self.nodes[id.index()].kind, NodeKind::Text(_))));
        }
        let Some(last) = last else {
            return self.append(parent, NodeKind::Text(value), origin).map(Some);
        };
        let record = &mut self.nodes[last.index()];
        let NodeKind::Text(previous) = &mut record.kind else {
            return self.append(parent, NodeKind::Text(value), origin).map(Some);
        };
        let joined = record.origin.join_text(origin);
        append_text_value(previous, &value, origin.error_offset(), budget)?;
        record.origin = joined;
        Ok(Some(last))
    }

    /// The synthetic document root, parent of the document element and of
    /// the comments and PIs around it.
    pub fn root(&self) -> Node<'_, 'input> {
        Node {
            document: self,
            id: NodeId::new(0),
        }
    }

    /// The document element, or the synthetic root for a `RECOVER` parse
    /// that found none ([`Self::document_element`] tells them apart).
    pub fn root_element(&self) -> Node<'_, 'input> {
        Node {
            document: self,
            id: self.root_element,
        }
    }

    /// The document element, or `None` for a `RECOVER` parse that recovered
    /// none (libxml2's `xmlDocGetRootElement`, DOM's `documentElement`).
    pub fn document_element(&self) -> Option<Node<'_, 'input>> {
        (self.root_element != NodeId::new(0)).then(|| self.root_element())
    }

    /// The node `id` names, or `None` when out of range. An ID from another
    /// document may name an unrelated node.
    pub fn node(&self, id: NodeId) -> Option<Node<'_, 'input>> {
        self.nodes
            .get(id.index())
            .map(|_| Node { document: self, id })
    }

    /// Visits all nodes in document order, including the synthetic root.
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = Node<'_, 'input>> + '_ {
        self.root().descendants()
    }

    /// The XML declaration.
    pub fn declaration(&self) -> Option<&Declaration<'input>> {
        self.declaration.as_ref()
    }

    /// The document type declaration.
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
    let needed = previous.len() + incoming.len();
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
    pub(crate) fn document(self) -> &'document Document<'input> {
        self.document
    }

    fn record(self) -> &'document Record<NodeKind<'input>> {
        &self.document.nodes[self.id.index()]
    }

    /// The node's ID in its document.
    pub fn id(self) -> NodeId {
        self.id
    }

    /// The node's content.
    pub fn kind(self) -> &'document NodeKind<'input> {
        &self.record().kind
    }

    /// An element's name; `None` for other kinds.
    pub fn name(self) -> Option<&'document Name<'input>> {
        match self.kind() {
            NodeKind::Element(element) => Some(&element.name),
            _ => None,
        }
    }

    /// A Text, CDATA, comment or PI payload; `None` for other kinds. See
    /// [`content`](Self::content) for an element's text.
    pub fn text(self) -> Option<&'document str> {
        match self.kind() {
            NodeKind::Text(value) | NodeKind::Cdata(value) | NodeKind::Comment(value) => {
                Some(value)
            }
            NodeKind::ProcessingInstruction { data, .. } => Some(data),
            NodeKind::Document
            | NodeKind::Doctype(_)
            | NodeKind::Element(_)
            | NodeKind::EntityRef(_) => None,
        }
    }

    /// The entity an entity-reference node names, or `None` for other kinds.
    pub fn entity_name(self) -> Option<&'document str> {
        match self.kind() {
            NodeKind::EntityRef(name) => Some(name),
            _ => None,
        }
    }

    /// The node's string value as borrowed chunks, without allocating.
    ///
    /// Containers yield descendant Text/CDATA in document order; leaves yield
    /// their own payload once, even when empty.
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

    /// An element's attributes in source order, namespace declarations included.
    pub fn attributes(self) -> &'document [Attribute<'input>] {
        match self.kind() {
            NodeKind::Element(element) => &element.attributes,
            _ => &[],
        }
    }

    /// The attribute at `position` with its general entity references kept
    /// (`&name;`, a literal `&` as `&#38;`), as libxml2 keeps it without
    /// `NOENT` and [`save`](crate::save) writes it; `None` when it has none.
    /// [`attributes`](Self::attributes) holds the substituted value.
    ///
    /// ```
    /// use yaxml::{Document, ParseOptions, options::NOENT};
    /// let xml = b"<!DOCTYPE r [<!ENTITY e 'x'>]><r a='1&e;&amp;' b='2'/>";
    /// let document = Document::parse_with(xml, ParseOptions::default())?;
    /// let root = document.root_element();
    /// assert_eq!(root.attributes()[0].value, "1x&");
    /// assert_eq!(root.unexpanded_attribute(0), Some("1&e;&#38;"));
    /// assert_eq!(root.unexpanded_attribute(1), None);
    /// let substituted = Document::parse_with(xml, ParseOptions::new(NOENT))?;
    /// assert_eq!(substituted.root_element().unexpanded_attribute(0), None);
    /// # Ok::<(), yaxml::Error>(())
    /// ```
    pub fn unexpanded_attribute(self, position: usize) -> Option<&'document str> {
        self.document.kept(self.id, position)
    }

    /// The node's input span, delimiters and an element's end tag included;
    /// `None` for content produced by entity expansion.
    pub fn range(self) -> Option<Range<usize>> {
        self.origin().input_range()
    }

    /// Where the node came from in the input.
    pub fn origin(self) -> SourceOrigin {
        self.record().origin
    }

    /// The parent; `None` only for the synthetic root.
    pub fn parent(self) -> Option<Self> {
        self.record().parent.map(|id| Self {
            document: self.document,
            id,
        })
    }

    /// The first child.
    pub fn first_child(self) -> Option<Self> {
        self.record().first_child.map(|id| Self {
            document: self.document,
            id,
        })
    }

    /// The last child.
    pub fn last_child(self) -> Option<Self> {
        self.record().last_child.map(|id| Self {
            document: self.document,
            id,
        })
    }

    /// The next sibling.
    pub fn next_sibling(self) -> Option<Self> {
        self.record().next_sibling.map(|id| Self {
            document: self.document,
            id,
        })
    }

    /// The previous sibling.
    pub fn previous_sibling(self) -> Option<Self> {
        let parent = self.record().parent?;
        let mut id = NodeId::new(self.id.index() - 1);
        while id != parent {
            let up = self.document.nodes[id.index()].parent?;
            if up == parent {
                return self.document.node(id);
            }
            id = up;
        }
        None
    }

    /// The children in document order.
    pub fn children(self) -> impl Iterator<Item = Self> + 'document {
        std::iter::successors(self.first_child(), |node| node.next_sibling())
    }

    /// This node and its descendants in document order.
    pub fn descendants(self) -> impl ExactSizeIterator<Item = Self> + 'document {
        (self.id.index()..self.record().subtree_end).map(move |index| Self {
            document: self.document,
            id: NodeId::new(index),
        })
    }

    /// `xmlGetProp`: the value of the first attribute whose name (local name
    /// when namespaced) is `name`, in any namespace, else the DTD's default
    /// for it; namespace declarations are not attributes.
    pub fn attribute(self, name: &str) -> Option<&'document str> {
        tree::property(self.document, self.id, name, None)
    }

    /// `xmlGetNsProp`: the attribute `local` in `namespace` (`None`: no
    /// namespace), else the DTD's default for it.
    pub fn attribute_ns(self, local: &str, namespace: Option<&str>) -> Option<&'document str> {
        tree::property(self.document, self.id, local, Some(namespace))
    }

    /// `xmlNodeGetContent`: the payload of a leaf, or the concatenated
    /// Text/CDATA of a document, element or entity reference; `None` for the
    /// DOCTYPE. [`text_chunks`](Self::text_chunks) visits it without allocating.
    pub fn content(self) -> Option<String> {
        let doctype = matches!(self.kind(), NodeKind::Doctype(_));
        (!doctype).then(|| self.text_chunks().collect())
    }

    /// `xmlNodeListGetString`, as [`OwnedNode::list_string`](crate::OwnedNode::list_string).
    pub fn list_string(self, substitute: bool) -> String {
        let ascii = self
            .document
            .declaration()
            .is_none_or(|declaration| declaration.encoding.is_none());
        tree::list_string(
            self.document,
            self.id,
            (!substitute).then_some((false, ascii)),
        )
    }

    /// `xmlGetNodePath`: a location path such as `/r/p:a[2]/text()`; `None`
    /// for the DOCTYPE, entity references and their content.
    pub fn path(self) -> Option<String> {
        tree::path(self.document, self.id)
    }

    /// `xmlNodeGetBase`: `xml:base` values from this node outwards resolved
    /// against each other and the document [`url`](Document::url).
    pub fn base_uri(self) -> Option<String> {
        tree::base(self.document, self.id, self.document.url())
    }

    /// `xmlNodeGetLang`: the nearest `xml:lang`.
    pub fn lang(self) -> Option<&'document str> {
        tree::lang(self.document, self.id)
    }

    /// `xmlNodeGetSpacePreserve`: the nearest `xml:space` of an element,
    /// true for `preserve`, false for `default`; other values are skipped.
    pub fn space_preserve(self) -> Option<bool> {
        tree::space_preserve(self.document, self.id)
    }

    /// `xmlSearchNs`: the URI `prefix` (`None`: the default namespace) is
    /// bound to here; `Some("")` under `xmlns=""`. `xml` is always bound.
    pub fn search_ns(self, prefix: Option<&str>) -> Option<&'document str> {
        tree::search_ns(self.document, self.id, prefix)
    }

    /// `xmlSearchNsByHref`: the innermost in-scope binding of `uri`.
    pub fn search_ns_by_href(self, uri: &str) -> Option<NamespaceBinding<'document>> {
        tree::search_ns_by_href(self.document, self.id, uri)
    }

    /// `xmlGetNsList`: every in-scope declaration, innermost first.
    pub fn namespaces(self) -> Vec<NamespaceBinding<'document>> {
        tree::ns_list(self.document, self.id)
    }
}

#[cfg(test)]
#[path = "../tests/internal/dom_text_join_author.rs"]
mod text_join_author_tests;

#[cfg(test)]
#[path = "../tests/internal/dom_text_join_spec.rs"]
mod text_join_spec_tests;
