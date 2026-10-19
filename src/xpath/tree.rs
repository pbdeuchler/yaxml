//! The one navigation abstraction the evaluator needs, for both DOMs.

use std::fmt::Debug;
use std::hash::Hash;

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
        self.node(id)?.first_child().map(|node| node.id())
    }

    fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.node(id)?.next_sibling().map(|node| node.id())
    }

    fn item(&self, id: NodeId) -> Item<'_> {
        match self.node(id).map(|node| node.kind()) {
            None | Some(NodeKind::Document) => Item::Root,
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
}

impl Navigate for OwnedDocument {
    type Id = OwnedNodeId;

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
        self.node(id)?.first_child().map(|node| node.id())
    }

    fn next_sibling(&self, id: OwnedNodeId) -> Option<OwnedNodeId> {
        self.node(id)?.next_sibling().map(|node| node.id())
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
            OwnedNodeKind::Document => Item::Root,
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

pub(crate) fn attributes<T: Navigate>(
    tree: &T,
    id: T::Id,
) -> impl Iterator<Item = (usize, QName<'_>, &str)> + '_ {
    (0..).map_while(move |index| {
        tree.attribute(id, index)
            .map(|(name, value)| (index, name, value))
    })
}
