use std::borrow::Cow;
use std::ops::Range;

use crate::options::Flags;
use crate::parser::Budget;
use crate::xpath::tree::{self, NamespaceBinding};
use crate::{
    Attribute, Declaration, Doctype, Element, Error, ErrorKind, Event, Limits, Name, ParseOptions,
    Reader, SourceOrigin,
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
/// element beneath its synthetic document root; under `RECOVER` it can have
/// none.
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
        Self::parse_with(input, ParseOptions::default().with_limits(limits))
    }

    /// Parses with libxml2 parser options; see [`crate::options`].
    /// `XINCLUDE` needs an owned tree and is `UnsupportedOption` here.
    pub fn parse_with(input: &'input [u8], options: ParseOptions) -> Result<Self, Error> {
        let flags = options.check(true, crate::options::XINCLUDE)?;
        let (limits, _) = options.effective_limits();
        let decode = crate::DecodeLimits {
            max_decoded_bytes: usize::MAX,
            max_decoded_token_bytes: limits.max_token_bytes,
        };
        let url = options.base.clone();
        let reader = Reader::configured(input, limits, decode, flags, options)?;
        let document = Self::from_reader(reader, input.len(), flags)?;
        Ok(Self { url, ..document })
    }

    /// The document URL (`ParseOptions::base`, libxml2's `doc->URL`), against
    /// which [`Node::base_uri`] resolves.
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    fn from_reader(mut reader: Reader<'input>, length: usize, flags: Flags) -> Result<Self, Error> {
        let mut nodes = Vec::new();
        nodes
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorKind::Allocation, 0))?;
        nodes.push(Record::root(NodeKind::Document, length));
        let mut document = Self {
            nodes,
            root_element: NodeId(0),
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
            let parent = parents.last().copied().unwrap_or(NodeId(0));
            let opens = matches!(event, Event::Start(_));
            match event {
                Event::Declaration(declaration) => document.declaration = Some(declaration),
                Event::Doctype(doctype) => {
                    document.append(parent, NodeKind::Doctype(doctype), origin)?;
                }
                // The parser balances ends, so `parent` is what they close.
                Event::End(_) => {
                    let subtree_end = document.nodes.len();
                    let record = &mut document.nodes[parent.0];
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
                    document.nodes[parent.0].subtree_end = document.nodes.len();
                }
                Event::Start(element) | Event::Empty(element) => {
                    if opens {
                        crate::parser::reserve_frame(&mut parents, origin.error_offset())?;
                    }
                    let id = NodeId(document.nodes.len());
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
                    document.append_element(parent, element, origin)?;
                    if parent == NodeId(0) {
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
        // Recovery stops at an error no site repairs, closing what is open
        // where reading stopped (an element opened by entity replacement text
        // keeps its span there).
        document.errors = Budget::conclude(reader.budget(), end)?;
        let stop = SourceOrigin::input(reader.position()..reader.position());
        let count = document.nodes.len();
        for id in parents {
            let record = &mut document.nodes[id.0];
            record.subtree_end = count;
            if let NodeKind::Element(_) = record.kind {
                record.origin = record.origin.close_element(stop);
            }
        }
        // Recovery keeps a document without an element, as libxml2 does
        // unless the input is empty (otherwise the reader failed already).
        if document.root_element == NodeId(0) && length == 0 {
            let missing = Error::new(ErrorKind::MissingRoot, reader.position());
            return Err(document.errors.first().copied().unwrap_or(missing));
        }
        document.nodes[0].subtree_end = document.nodes.len();
        let dtd = reader
            .catalog()
            .map(|catalog| Box::new(catalog.dtd().clone()));
        let mut document = Self { dtd, ..document };
        if flags.drops_blanks() {
            let blanks = crate::xpath::tree::blanks(&document);
            document.remove_leaves(&blanks);
        }
        // Without DTD-declared ID attributes or removed nodes, the `xml:id`s
        // noted while building are exactly what a full traversal would find.
        let traverse = flags.drops_blanks() || crate::xpath::tree::declares_ids(document.dtd());
        document.ids = match (flags.records_ids(), traverse) {
            (false, _) => Vec::new(),
            (true, true) => crate::xpath::tree::find_ids(&document, document.dtd()),
            (true, false) => std::mem::take(&mut document.ids),
        };
        if flags.validates() {
            document.validity = document.validate();
        }
        Ok(document)
    }

    /// Removes leaves (sorted, never the root or an element with children),
    /// keeping preorder storage and relinking the survivors.
    fn remove_leaves(&mut self, leaves: &[NodeId]) {
        if leaves.is_empty() {
            return;
        }
        // `kept[i]`: survivors before slot `i`, which is slot `i`'s new index.
        let mut kept = Vec::with_capacity(self.nodes.len() + 1);
        let mut removed = leaves.iter().peekable();
        let mut count = 0;
        for index in 0..=self.nodes.len() {
            kept.push(count);
            match removed.peek() {
                Some(leaf) if leaf.0 == index => drop(removed.next()),
                _ => count += 1,
            }
        }
        let map = |id: NodeId| NodeId(kept[id.0]);
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
            let parent = self.nodes[index].parent.expect("non-root parent").0;
            match self.nodes[parent].last_child {
                Some(previous) => self.nodes[previous.0].next_sibling = Some(NodeId(index)),
                None => self.nodes[parent].first_child = Some(NodeId(index)),
            }
            self.nodes[parent].last_child = Some(NodeId(index));
        }
        self.root_element = map(self.root_element);
        for (element, _, _) in &mut self.kept {
            *element = map(*element);
        }
    }

    /// The well-formedness errors a `RECOVER` parse recovered from, in the
    /// order found (at most 100, like libxml2); empty for a well-formed
    /// document and without `RECOVER`.
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
        let id = NodeId(self.nodes.len());
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

    /// Returns the single document element, guaranteed by successful parsing
    /// except under `RECOVER`: a document that recovered none returns its
    /// synthetic root, as an HTML document without one does
    /// ([`Self::document_element`] tells them apart).
    pub fn root_element(&self) -> Node<'_, 'input> {
        Node {
            document: self,
            id: self.root_element,
        }
    }

    /// The document element, or `None` for a `RECOVER` parse that recovered
    /// none (libxml2's `xmlDocGetRootElement`, DOM's `documentElement`).
    pub fn document_element(&self) -> Option<Node<'_, 'input>> {
        (self.root_element != NodeId(0)).then(|| self.root_element())
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

    /// The attribute at `position` as libxml2 keeps it when parsing without
    /// `NOENT` (the attribute's text and entity-reference children, which
    /// `save` writes): `&name;` for each kept general entity reference, `&#38;`
    /// for a literal `&`, other references resolved. `None` when it kept no
    /// reference, the usual case; [`attributes`](Self::attributes) always has
    /// the substituted value, which XPath, validation and C14N use.
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

    /// Returns the previous direct sibling, if any. Storage is preorder, so
    /// the slot before this node is its parent or lies in that sibling's subtree.
    pub fn previous_sibling(self) -> Option<Self> {
        let parent = self.record().parent?;
        let mut id = NodeId(self.id.0 - 1);
        while id != parent {
            let up = self.document.nodes[id.0].parent?;
            if up == parent {
                return self.document.node(id);
            }
            id = up;
        }
        None
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
