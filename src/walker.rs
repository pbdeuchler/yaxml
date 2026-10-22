//! libxml2's tree walker (`xmlReaderWalker`): the [`TextReader`] cursor over
//! an existing [`OwnedDocument`].
//!
//! [`TextReader`]: crate::TextReader

use crate::xpath::tree::XMLNS_NAMESPACE as XMLNS;
use crate::{Error, NodeType, OwnedDocument, OwnedNode, OwnedNodeKind, ReadState, Standalone};

/// `xmlTextReaderState` as the tree functions use it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Start,
    Backtrack,
    End,
}

/// A cursor over the nodes of an [`OwnedDocument`], libxml2's
/// `xmlReaderWalker`: the reader accessors of [`TextReader`], answered from
/// the tree instead of a parse.
///
/// libxml2's tree walk is followed, quirks included:
///
/// - Each tree node is one node: adjacent text nodes are not merged,
///   the DOCTYPE is reported but not entered, and entity references are
///   not expanded.
/// - An element without children is empty and has no `EndElement`, however
///   it was written.
/// - [`next`](Self::next) from a last child also skips its parent's
///   `EndElement`, and from a last child two levels down ends the walk
///   after the node it moves to (`xmlTextReaderNextTree`).
/// - After the end, the last node stays current;
///   [`read_state`](Self::read_state) is always `Initial`.
/// - [`read_string`](Self::read_string) of an element is `None`.
///
/// Attributes are visited by index: namespace declarations first, then
/// attributes, each in source order.
///
/// [`TextReader`]: crate::TextReader
///
/// ```
/// use yaxml::{NodeType, OwnedDocument, Walker};
///
/// let document = OwnedDocument::parse("<r><a></a>text<b k='v'/></r>")?;
/// let mut walker = Walker::new(&document);
/// let mut seen = Vec::new();
/// while walker.read() {
///     seen.push((walker.depth(), walker.node_type(), walker.name().unwrap()));
/// }
/// assert_eq!(seen, [
///     (0, NodeType::Element, "r"),
///     (1, NodeType::Element, "a"),
///     (1, NodeType::Text, "#text"),
///     (1, NodeType::Element, "b"),
///     (0, NodeType::EndElement, "r"),
/// ]);
/// # Ok::<(), yaxml::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct Walker<'d> {
    document: &'d OwnedDocument,
    node: Option<OwnedNode<'d>>,
    state: State,
    depth: isize,
    /// libxml2's `curnode`, by index as [`Walker::get_attribute_no`] counts.
    attribute: Option<usize>,
}

impl<'d> Walker<'d> {
    /// A walker before the first node of `document` (`xmlReaderWalker`).
    pub fn new(document: &'d OwnedDocument) -> Self {
        Self {
            document,
            node: None,
            state: State::Start,
            depth: 0,
            attribute: None,
        }
    }

    /// Starts over on `document` (`xmlReaderNewWalker`).
    pub fn reset(&mut self, document: &'d OwnedDocument) {
        *self = Self::new(document);
    }

    /// Moves to the next node in document order; false at the end
    /// (`xmlTextReaderRead`, `xmlTextReaderReadTree`).
    pub fn read(&mut self) -> bool {
        self.attribute = None;
        if self.state == State::End {
            return false;
        }
        let Some(node) = self.node else {
            return self.first();
        };
        let entered = !matches!(
            node.kind(),
            OwnedNodeKind::DocumentType | OwnedNodeKind::EntityRef
        );
        if let Some(child) = node
            .first_child()
            .filter(|_| self.state != State::Backtrack && entered)
        {
            self.depth += 1;
            return self.start(child);
        }
        if let Some(next) = node.next_sibling() {
            return self.start(next);
        }
        match node.parent() {
            Some(parent) if parent.kind() == OwnedNodeKind::Document => {
                self.state = State::End;
                false
            }
            Some(parent) => {
                (self.node, self.depth, self.state) =
                    (Some(parent), self.depth - 1, State::Backtrack);
                true
            }
            None => {
                self.state = State::End;
                true
            }
        }
    }

    /// Skips the current node's subtree (`xmlTextReaderNext`,
    /// `xmlTextReaderNextTree`).
    #[expect(clippy::should_implement_trait, reason = "libxml2's xmlTextReaderNext")]
    pub fn next(&mut self) -> bool {
        self.attribute = None;
        if self.state == State::End {
            return false;
        }
        let Some(node) = self.node else {
            return self.first();
        };
        if self.state != State::Backtrack {
            if let Some(next) = node.next_sibling() {
                return self.start(next);
            }
            self.state = State::Backtrack;
            self.read();
        }
        let node = self.node.expect("a current node");
        if let Some(next) = node.next_sibling() {
            return self.start(next);
        }
        if let Some(parent) = node.parent() {
            if parent.kind() == OwnedNodeKind::Document && !self.document.is_html() {
                self.state = State::End;
                return false;
            }
            (self.node, self.depth, self.state) = (Some(parent), self.depth - 1, State::Backtrack);
            self.next();
        }
        self.state = State::End;
        true
    }

    /// Moves to the current node's next sibling, not entering its subtree;
    /// false without one (`xmlTextReaderNextSibling`).
    pub fn next_sibling(&mut self) -> bool {
        self.attribute = None;
        if self.state == State::End {
            return false;
        }
        match self.node.map(OwnedNode::next_sibling) {
            None => self.next(),
            Some(Some(next)) => self.start(next),
            Some(None) => false,
        }
    }

    fn first(&mut self) -> bool {
        match self.document.root().first_child() {
            Some(first) => self.start(first),
            None => {
                self.state = State::End;
                false
            }
        }
    }

    fn start(&mut self, node: OwnedNode<'d>) -> bool {
        (self.node, self.state) = (Some(node), State::Start);
        true
    }

    /// Always [`ReadState::Initial`]: libxml2 never changes a walker's mode.
    pub fn read_state(&self) -> ReadState {
        ReadState::Initial
    }

    /// The current node, an attribute's element while on one
    /// (`xmlTextReaderCurrentNode`).
    pub fn current_node(&self) -> Option<OwnedNode<'d>> {
        self.node
    }

    /// The walked document (`xmlTextReaderCurrentDoc`).
    pub fn document(&self) -> &'d OwnedDocument {
        self.document
    }

    fn ends(&self) -> bool {
        matches!(self.state, State::End | State::Backtrack)
    }

    /// The source index of attribute `index`, declarations first.
    fn slot(node: OwnedNode<'d>, index: usize) -> Option<usize> {
        let declares = |slot: &usize| Self::declaration(node, *slot);
        let slots = 0..node.attributes().len();
        let declarations = slots.clone().filter(declares).count();
        match index < declarations {
            true => slots.filter(declares).nth(index),
            false => slots
                .filter(|slot| !declares(slot))
                .nth(index - declarations),
        }
    }

    fn declaration(node: OwnedNode<'d>, slot: usize) -> bool {
        node.attribute_parts(slot)
            .is_some_and(|((_, namespace), _)| namespace == Some(XMLNS))
    }

    /// The attribute under the cursor: (QName, namespace, value, declaration).
    fn attribute(&self) -> Option<(&'d str, Option<&'d str>, &'d str, bool)> {
        let node = self.node?;
        let slot = Self::slot(node, self.attribute?)?;
        let ((name, namespace), value) = node.attribute_parts(slot)?;
        Some((name, namespace, value, namespace == Some(XMLNS)))
    }

    /// The QName under the cursor and its namespace, for elements and attributes.
    fn qname(&self) -> Option<(&'d str, Option<&'d str>)> {
        match self.attribute() {
            Some((name, namespace, ..)) => Some((name, namespace)),
            None => self.node?.name_parts(),
        }
    }

    /// The current node's type (`xmlTextReaderNodeType`).
    pub fn node_type(&self) -> NodeType {
        let Some(node) = self.node else {
            return NodeType::None;
        };
        if self.attribute.is_some() {
            return NodeType::Attribute;
        }
        match node.kind() {
            OwnedNodeKind::Element if self.ends() => NodeType::EndElement,
            OwnedNodeKind::Element => NodeType::Element,
            OwnedNodeKind::Text if crate::blanks::is_blank(node.text().unwrap_or_default()) => {
                NodeType::SignificantWhitespace
            }
            OwnedNodeKind::Text => NodeType::Text,
            OwnedNodeKind::Cdata => NodeType::Cdata,
            OwnedNodeKind::EntityRef => NodeType::EntityReference,
            OwnedNodeKind::ProcessingInstruction => NodeType::ProcessingInstruction,
            OwnedNodeKind::Comment => NodeType::Comment,
            OwnedNodeKind::Document => NodeType::Document,
            OwnedNodeKind::DocumentType => NodeType::DocumentType,
        }
    }

    /// Qualified name; `#text`, `#cdata-section`, `#comment` or `#document`
    /// for those nodes; the target of a PI, the name of a DOCTYPE or entity.
    pub fn name(&self) -> Option<&'d str> {
        if let Some((name, _)) = self.qname() {
            return Some(name);
        }
        let node = self.node?;
        Some(match node.kind() {
            OwnedNodeKind::Text => "#text",
            OwnedNodeKind::Cdata => "#cdata-section",
            OwnedNodeKind::Comment => "#comment",
            OwnedNodeKind::Document => "#document",
            OwnedNodeKind::ProcessingInstruction => node.processing_instruction_target()?,
            OwnedNodeKind::EntityRef => node.entity_name()?,
            OwnedNodeKind::DocumentType => node.doctype()?.name,
            OwnedNodeKind::Element => unreachable!("elements have a QName"),
        })
    }

    /// `(prefix, local)` of a QName; a name in no namespace has no prefix,
    /// and a declaration's prefix is `xmlns`.
    fn split((name, namespace): (&'d str, Option<&'d str>)) -> (Option<&'d str>, &'d str) {
        match name.split_once(':') {
            Some((prefix, local)) if namespace.is_some() => (Some(prefix), local),
            _ => (None, name),
        }
    }

    /// Local part of an element or attribute name (`xmlns` or the declared
    /// prefix for a namespace declaration); otherwise [`name`](Self::name).
    pub fn local_name(&self) -> Option<&'d str> {
        match self.qname() {
            Some(qname) => Some(Self::split(qname).1),
            None => self.name(),
        }
    }

    /// The prefix of an element or attribute name.
    pub fn prefix(&self) -> Option<&'d str> {
        Self::split(self.qname()?).0
    }

    /// Namespace of an element or attribute; the XMLNS namespace for declarations.
    pub fn namespace_uri(&self) -> Option<&'d str> {
        self.qname()?.1
    }

    /// Attribute, text, CDATA, comment or PI content; a PI without data has
    /// none (`xmlTextReaderConstValue`).
    pub fn value(&self) -> Option<&'d str> {
        if let Some((_, _, value, _)) = self.attribute() {
            return Some(value);
        }
        let node = self.node?;
        match node.kind() {
            OwnedNodeKind::Text | OwnedNodeKind::Cdata | OwnedNodeKind::Comment => node.text(),
            OwnedNodeKind::ProcessingInstruction => node.text().filter(|data| !data.is_empty()),
            _ => None,
        }
    }

    /// Whether the node type carries a value (`xmlTextReaderHasValue`).
    pub fn has_value(&self) -> bool {
        self.attribute.is_some()
            || self.node.is_some_and(|node| {
                matches!(
                    node.kind(),
                    OwnedNodeKind::Text
                        | OwnedNodeKind::Cdata
                        | OwnedNodeKind::Comment
                        | OwnedNodeKind::ProcessingInstruction
                )
            })
    }

    /// Nesting depth: 0 for top-level nodes, +1 on an element's attributes.
    pub fn depth(&self) -> usize {
        // Only `next` on an HTML document reaches the document node, at -1.
        usize::try_from(self.depth).unwrap_or(0) + usize::from(self.attribute.is_some())
    }

    /// True on an element without children (`xmlTextReaderIsEmptyElement`).
    pub fn is_empty_element(&self) -> bool {
        self.attribute.is_none()
            && self.state != State::End
            && self.node.is_some_and(|node| {
                node.kind() == OwnedNodeKind::Element && node.first_child().is_none()
            })
    }

    /// True on an element, or its `EndElement`, that has attributes.
    pub fn has_attributes(&self) -> bool {
        self.attribute.is_none()
            && self
                .element()
                .is_some_and(|node| node.attributes().len() > 0)
    }

    /// Attributes, namespace declarations included, of the current element;
    /// 0 on an `EndElement` or an attribute.
    pub fn attribute_count(&self) -> usize {
        match self.attribute.is_some() || self.ends() {
            true => 0,
            false => self.element().map_or(0, |node| node.attributes().len()),
        }
    }

    /// The current node if it is an element, also while on its attributes.
    fn element(&self) -> Option<OwnedNode<'d>> {
        self.node
            .filter(|node| node.kind() == OwnedNodeKind::Element)
    }

    /// The index of the attribute matching `found`'s (prefix, local, namespace).
    fn find(&self, found: impl Fn(Option<&str>, &str, Option<&str>) -> bool) -> Option<usize> {
        let node = self.element()?;
        (0..node.attributes().len()).find(|&index| {
            Self::slot(node, index)
                .and_then(|slot| node.attribute_parts(slot))
                .is_some_and(|(qname, _)| {
                    let (prefix, local) = Self::split(qname);
                    found(prefix, local, qname.1)
                })
        })
    }

    /// Index of a QName, matching prefixes lexically (`xmlns` and `xmlns:p`
    /// name declarations).
    fn lexical(&self, name: &str) -> Option<usize> {
        let (prefix, local) = name
            .split_once(':')
            .map_or((None, name), |(p, l)| (Some(p), l));
        self.find(|p, l, _| p == prefix && l == local)
    }

    /// Index of the attribute with this local name and namespace (`None`: no
    /// namespace; the XMLNS namespace selects declarations).
    fn expanded(&self, local: &str, namespace: Option<&str>) -> Option<usize> {
        self.find(|_, l, n| l == local && n == namespace)
    }

    fn lookup(&self, index: Option<usize>) -> Option<&'d str> {
        let node = self.element().filter(|_| self.attribute.is_none())?;
        node.attribute_parts(Self::slot(node, index?)?)
            .map(|(_, value)| value)
    }

    /// Value of the attribute named by a QName: `xmlns` and `xmlns:p` name
    /// declarations, another prefix resolves in scope. None while on an
    /// attribute, as in libxml2.
    pub fn get_attribute(&self, name: &str) -> Option<&'d str> {
        let index = match name.split_once(':') {
            Some((prefix, local)) if prefix != "xmlns" => {
                let namespace = self.element()?.search_ns(Some(prefix))?;
                self.expanded(local, Some(namespace))
            }
            _ => self.lexical(name),
        };
        self.lookup(index)
    }

    /// Value of the attribute with this local name and namespace.
    pub fn get_attribute_ns(&self, local: &str, namespace: Option<&str>) -> Option<&'d str> {
        self.lookup(self.expanded(local, namespace))
    }

    /// Value of attribute `index`, declarations first.
    pub fn get_attribute_no(&self, index: usize) -> Option<&'d str> {
        self.lookup(Some(index))
    }

    fn move_to(&mut self, index: Option<usize>) -> bool {
        if index.is_some() {
            self.attribute = index;
        }
        index.is_some()
    }

    /// Moves to the attribute with this QName, matching prefixes lexically.
    pub fn move_to_attribute(&mut self, name: &str) -> bool {
        self.move_to(self.lexical(name))
    }

    /// Moves to the attribute with this local name and namespace.
    pub fn move_to_attribute_ns(&mut self, local: &str, namespace: &str) -> bool {
        self.move_to(self.expanded(local, Some(namespace)))
    }

    /// Moves to attribute `index`; out of range, returns to the element.
    pub fn move_to_attribute_no(&mut self, index: usize) -> bool {
        let Some(count) = self.element().map(|node| node.attributes().len()) else {
            return false;
        };
        self.attribute = (index < count).then_some(index);
        self.attribute.is_some()
    }

    /// Moves to the element's first attribute.
    pub fn move_to_first_attribute(&mut self) -> bool {
        let count = self.element().map_or(0, |node| node.attributes().len());
        self.move_to((count > 0).then_some(0))
    }

    /// Moves to the next attribute, or the first from the element.
    pub fn move_to_next_attribute(&mut self) -> bool {
        let next = self.attribute.map_or(0, |index| index + 1);
        let count = self.element().map_or(0, |node| node.attributes().len());
        self.move_to((next < count).then_some(next))
    }

    /// Moves from an attribute back to its element; false if not on one.
    pub fn move_to_element(&mut self) -> bool {
        self.attribute.take().is_some()
    }

    /// Whether the cursor is on a namespace declaration.
    pub fn is_namespace_decl(&self) -> bool {
        self.attribute()
            .is_some_and(|(.., declaration)| declaration)
    }

    /// The document's `standalone` (`xmlTextReaderStandalone`).
    pub fn standalone(&self) -> Standalone {
        match self
            .document
            .declaration()
            .map(|declaration| declaration.standalone)
        {
            None => Standalone::NoDeclaration,
            Some(None) => Standalone::Unspecified,
            Some(Some(false)) => Standalone::No,
            Some(Some(true)) => Standalone::Yes,
        }
    }

    /// The XML declaration's `version`, `1.0` without one.
    pub fn xml_version(&self) -> &'d str {
        self.document
            .declaration()
            .map_or("1.0", |declaration| declaration.version)
    }

    /// The declared encoding as written; `None` without a label.
    pub fn encoding(&self) -> Option<&'d str> {
        self.document.declaration()?.encoding
    }

    /// The current node's base URI (`xmlTextReaderBaseUri`).
    pub fn base_uri(&self) -> Option<String> {
        self.node?.base_uri()
    }

    /// Inherited `xml:lang` of the current node.
    pub fn xml_lang(&self) -> Option<&'d str> {
        self.node?.lang()
    }

    /// Namespace bound to `prefix` (`None`: the default namespace) at the
    /// current node; `xml` is always bound.
    pub fn lookup_namespace(&self, prefix: Option<&str>) -> Option<&'d str> {
        self.node?.search_ns(prefix)
    }

    /// The quotation mark of attribute values: always `"`.
    pub fn quote_char(&self) -> char {
        '"'
    }

    /// Text of a Text or CDATA node (`xmlTextReaderReadString`; libxml2's
    /// walker cannot expand an element, so elements have none).
    pub fn read_string(&self) -> Option<&'d str> {
        let node = self.node.filter(|_| self.attribute.is_none())?;
        match node.kind() {
            OwnedNodeKind::Text | OwnedNodeKind::Cdata => node.text(),
            _ => None,
        }
    }

    /// `xmlTextReaderReadInnerXml`: each child serialized with the
    /// namespace declarations it uses; empty for a DOCTYPE or an entity
    /// reference, whose libxml2 children are declarations.
    ///
    /// # Errors
    ///
    /// An allocation failure.
    pub fn read_inner_xml(&self) -> Result<Option<String>, Error> {
        let Some(node) = self.node else {
            return Ok(None);
        };
        let mut out = Vec::new();
        let mut child = node.first_child().filter(|_| {
            !matches!(
                node.kind(),
                OwnedNodeKind::DocumentType | OwnedNodeKind::EntityRef
            )
        });
        while let Some(current) = child {
            out.extend(dump(current)?);
            child = current.next_sibling();
        }
        Ok(Some(String::from_utf8(out).expect("UTF-8 output")))
    }

    /// `xmlTextReaderReadOuterXml`: the current node serialized with the
    /// namespace declarations it uses.
    ///
    /// # Errors
    ///
    /// An allocation failure.
    pub fn read_outer_xml(&self) -> Result<Option<String>, Error> {
        let Some(node) = self.node else {
            return Ok(None);
        };
        Ok(Some(String::from_utf8(dump(node)?).expect("UTF-8 output")))
    }
}

/// One node as `xmlNodeDumpOutput` writes a copy of it.
fn dump(node: OwnedNode<'_>) -> Result<Vec<u8>, Error> {
    crate::save::to_vec(node, &crate::save::Options::default())
}
