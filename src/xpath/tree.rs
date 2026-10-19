//! The one navigation abstraction the evaluator needs, for both DOMs.

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::hash::Hash;

use crate::valid::Dtd;

use crate::{Document, NodeId, NodeKind, OwnedDocument, OwnedNodeId, OwnedNodeKind};

pub(crate) const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

/// A tree XPath can navigate: [`Document`] and [`OwnedDocument`]. Sealed.
pub trait Tree: Navigate {}

impl<T: Navigate> Tree for T {}

/// An element or attribute name; an empty namespace means none.
#[derive(Clone, Copy, Debug)]
pub struct QName<'a> {
    pub qualified: &'a str,
    pub local: &'a str,
    pub namespace: Option<&'a str>,
}

impl<'a> QName<'a> {
    fn new((qualified, namespace): (&'a str, Option<&'a str>)) -> Self {
        Self {
            qualified,
            local: qualified.rsplit(':').next().unwrap_or(qualified),
            namespace: namespace.filter(|uri| !uri.is_empty()),
        }
    }

    pub(crate) fn is_namespace_declaration(&self) -> bool {
        self.namespace == Some(XMLNS_NAMESPACE)
    }

    /// The prefix a namespace declaration binds, `""` for the default namespace.
    pub(crate) fn declared_prefix(&self) -> Option<&'a str> {
        self.is_namespace_declaration()
            .then_some(if self.qualified == "xmlns" {
                ""
            } else {
                self.local
            })
    }
}

/// A tree node's XPath-visible content. CDATA is text.
#[derive(Clone, Copy, Debug)]
pub enum Item<'a> {
    Root,
    Element(QName<'a>),
    Text(&'a str),
    Comment(&'a str),
    Pi(&'a str, &'a str),
}

/// Primitive accessors. Lookups of foreign IDs return `None` or `Item::Root`.
pub trait Navigate {
    type Id: Copy + Eq + Ord + Hash + Debug;
    fn root(&self) -> Self::Id;
    fn contains(&self, id: Self::Id) -> bool;
    fn parent(&self, id: Self::Id) -> Option<Self::Id>;
    fn first_child(&self, id: Self::Id) -> Option<Self::Id>;
    fn next_sibling(&self, id: Self::Id) -> Option<Self::Id>;
    fn item(&self, id: Self::Id) -> Item<'_>;
    /// Attributes by source index, including namespace declarations.
    fn attribute(&self, id: Self::Id, index: usize) -> Option<(QName<'_>, &str)>;
    /// A dense per-document slot.
    fn slot(&self, id: Self::Id) -> usize;
    /// Document-order rank per slot when slots are not already in document order.
    fn ranks(&self) -> Option<Vec<usize>>;
    /// `(element, attribute index)` of ID-typed attributes, sorted.
    fn ids(&self) -> &[(Self::Id, usize)];
    /// Whether a text item is a CDATA section.
    fn is_cdata(&self, id: Self::Id) -> bool;
    /// The start of the node's source range.
    fn offset(&self, id: Self::Id) -> Option<usize>;
    /// Content XML output cannot represent (only possible in HTML documents).
    fn xml_violation(&self) -> Option<String> {
        None
    }
    /// The declarations the document's DTD made.
    fn dtd(&self) -> Option<&Dtd>;
}

impl Navigate for Document<'_> {
    type Id = NodeId;

    fn root(&self) -> NodeId {
        Document::root(self).id()
    }

    fn contains(&self, id: NodeId) -> bool {
        self.node(id).is_some()
    }

    fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id)?.parent().map(|node| node.id())
    }

    fn first_child(&self, id: NodeId) -> Option<NodeId> {
        visible(self.node(id)?.first_child())
    }

    fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        visible(self.node(id)?.next_sibling())
    }

    fn item(&self, id: NodeId) -> Item<'_> {
        match self.node(id).map(|node| node.kind()) {
            None | Some(NodeKind::Document | NodeKind::Doctype(_)) => Item::Root,
            Some(NodeKind::Element(element)) => Item::Element(QName::new((
                element.name.qualified(),
                element.name.namespace(),
            ))),
            Some(NodeKind::Text(text) | NodeKind::Cdata(text)) => Item::Text(text),
            Some(NodeKind::Comment(text)) => Item::Comment(text),
            Some(NodeKind::ProcessingInstruction { target, data }) => Item::Pi(target, data),
        }
    }

    fn attribute(&self, id: NodeId, index: usize) -> Option<(QName<'_>, &str)> {
        let attribute = self.node(id)?.attributes().get(index)?;
        let name = &attribute.name;
        Some((
            QName::new((name.qualified(), name.namespace())),
            &attribute.value,
        ))
    }

    fn slot(&self, id: NodeId) -> usize {
        id.0
    }

    fn ranks(&self) -> Option<Vec<usize>> {
        None
    }

    fn ids(&self) -> &[(NodeId, usize)] {
        Document::ids(self)
    }

    fn is_cdata(&self, id: NodeId) -> bool {
        self.node(id)
            .is_some_and(|node| matches!(node.kind(), NodeKind::Cdata(_)))
    }

    fn offset(&self, id: NodeId) -> Option<usize> {
        Some(self.node(id)?.range()?.start)
    }

    fn dtd(&self) -> Option<&Dtd> {
        Document::dtd(self)
    }
}

impl Navigate for OwnedDocument {
    type Id = OwnedNodeId;

    fn xml_violation(&self) -> Option<String> {
        OwnedDocument::xml_violation(self)
    }

    fn root(&self) -> OwnedNodeId {
        OwnedDocument::root(self).id()
    }

    fn contains(&self, id: OwnedNodeId) -> bool {
        self.node(id).is_some()
    }

    fn parent(&self, id: OwnedNodeId) -> Option<OwnedNodeId> {
        self.node(id)?.parent().map(|node| node.id())
    }

    fn first_child(&self, id: OwnedNodeId) -> Option<OwnedNodeId> {
        visible_owned(self.node(id)?.first_child())
    }

    fn next_sibling(&self, id: OwnedNodeId) -> Option<OwnedNodeId> {
        visible_owned(self.node(id)?.next_sibling())
    }

    fn item(&self, id: OwnedNodeId) -> Item<'_> {
        let Some(node) = self.node(id) else {
            return Item::Root;
        };
        let text = node.text().unwrap_or_default();
        match node.kind() {
            OwnedNodeKind::Element => node
                .name_parts()
                .map_or(Item::Root, |name| Item::Element(QName::new(name))),
            OwnedNodeKind::Text | OwnedNodeKind::Cdata => Item::Text(text),
            OwnedNodeKind::Comment => Item::Comment(text),
            OwnedNodeKind::ProcessingInstruction => Item::Pi(
                node.processing_instruction_target().unwrap_or_default(),
                text,
            ),
            OwnedNodeKind::Document | OwnedNodeKind::DocumentType => Item::Root,
        }
    }

    fn attribute(&self, id: OwnedNodeId, index: usize) -> Option<(QName<'_>, &str)> {
        let (name, value) = self.node(id)?.attribute_parts(index)?;
        Some((QName::new(name), value))
    }

    fn slot(&self, id: OwnedNodeId) -> usize {
        id.slot()
    }

    fn ranks(&self) -> Option<Vec<usize>> {
        self.preorder_ranks()
    }

    fn ids(&self) -> &[(OwnedNodeId, usize)] {
        OwnedDocument::ids(self)
    }

    fn is_cdata(&self, id: OwnedNodeId) -> bool {
        self.node(id)
            .is_some_and(|node| node.kind() == OwnedNodeKind::Cdata)
    }

    fn offset(&self, id: OwnedNodeId) -> Option<usize> {
        Some(self.node(id)?.range()?.start)
    }

    fn dtd(&self) -> Option<&Dtd> {
        OwnedDocument::dtd(self)
    }
}

/// Whether the DTD declares any ID-typed attribute; otherwise only `xml:id`
/// attributes are IDs, and parsers record those while building.
pub(crate) fn declares_ids(dtd: Option<&Dtd>) -> bool {
    dtd.is_some_and(|dtd| dtd.id_attributes().next().is_some())
}

/// ID-typed attributes of a freshly parsed tree: `xml:id` and those declared
/// `ID` for their element in the DTD, as libxml2's `xmlIsID`.
pub(crate) fn find_ids<T: Navigate>(tree: &T, dtd: Option<&Dtd>) -> Vec<(T::Id, usize)> {
    let mut declared: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (element, attribute) in dtd.into_iter().flat_map(Dtd::id_attributes) {
        declared.entry(element).or_default().insert(attribute);
    }
    let mut ids: Vec<_> = preorder(tree, Some(tree.root()), None)
        .filter_map(|id| match tree.item(id) {
            Item::Element(name) => Some((id, declared.get(name.qualified))),
            _ => None,
        })
        .flat_map(|(id, declared)| {
            attributes(tree, id)
                .filter(move |(_, name, _)| {
                    name.qualified == "xml:id"
                        || declared.is_some_and(|names| names.contains(name.qualified))
                })
                .map(move |(index, ..)| (id, index))
        })
        .collect();
    ids.sort_unstable();
    ids
}

/// The first element whose ID attribute currently has `value`.
pub(crate) fn element_by_id<T: Navigate>(
    tree: &T,
    ids: &[(T::Id, usize)],
    value: &str,
) -> Option<T::Id> {
    ids.iter()
        .find(|&&(id, index)| {
            tree.attribute(id, index)
                .is_some_and(|(_, found)| found == value)
        })
        .map(|&(id, _)| id)
}

// The XPath data model has no DOCTYPE node; navigation steps over it.
fn visible(node: Option<crate::Node<'_, '_>>) -> Option<NodeId> {
    std::iter::successors(node, |node| node.next_sibling())
        .find(|node| !matches!(node.kind(), NodeKind::Doctype(_)))
        .map(|node| node.id())
}

fn visible_owned(node: Option<crate::OwnedNode<'_>>) -> Option<OwnedNodeId> {
    std::iter::successors(node, |node| node.next_sibling())
        .find(|node| node.kind() != OwnedNodeKind::DocumentType)
        .map(|node| node.id())
}

/// The next preorder node after `id`'s subtree, never leaving `stop`'s subtree.
pub(crate) fn skip_subtree<T: Navigate>(
    tree: &T,
    mut id: T::Id,
    stop: Option<T::Id>,
) -> Option<T::Id> {
    loop {
        if Some(id) == stop {
            return None;
        }
        if let Some(sibling) = tree.next_sibling(id) {
            return Some(sibling);
        }
        id = tree.parent(id)?;
    }
}

/// Preorder from `first`, bounded by `stop`'s subtree when given.
pub(crate) fn preorder<T: Navigate>(
    tree: &T,
    first: Option<T::Id>,
    stop: Option<T::Id>,
) -> impl Iterator<Item = T::Id> + '_ {
    std::iter::successors(first, move |&id| {
        tree.first_child(id)
            .or_else(|| skip_subtree(tree, id, stop))
    })
}

pub(crate) fn descendants<T: Navigate>(tree: &T, id: T::Id) -> impl Iterator<Item = T::Id> + '_ {
    preorder(tree, tree.first_child(id), Some(id))
}

/// In-scope namespaces as (prefix, URI), `""` naming the default namespace.
/// Order follows libxml2: `xml` first, then `xmlGetNsList` reversed.
/// A default-namespace undeclaration (`xmlns=""`) is a node with an empty URI,
/// as `xmlGetNsListSafe` lists it for `xmlXPathNextNamespace`.
pub(crate) fn namespaces<T: Navigate>(tree: &T, element: T::Id) -> Vec<(&str, &str)> {
    if !matches!(tree.item(element), Item::Element(_)) {
        return Vec::new();
    }
    let mut found = vec![("xml", XML_NAMESPACE)];
    for owner in std::iter::successors(Some(element), |&id| tree.parent(id)) {
        for (_, name, uri) in attributes(tree, owner) {
            if let Some(prefix) = name.declared_prefix()
                && found.iter().all(|&(seen, _)| seen != prefix)
            {
                found.push((prefix, uri));
            }
        }
    }
    let inherited = found.split_off(1);
    found.extend(inherited.into_iter().rev());
    found
}

pub(crate) fn attributes<T: Navigate>(
    tree: &T,
    id: T::Id,
) -> impl Iterator<Item = (usize, QName<'_>, &str)> + '_ {
    (0..).map_while(move |index| {
        tree.attribute(id, index)
            .map(|(name, value)| (index, name, value))
    })
}
