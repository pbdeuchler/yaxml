//! The one navigation abstraction the evaluator needs, for both DOMs.

use std::collections::{HashMap, HashSet, TryReserveError};
use std::fmt::Debug;
use std::hash::Hash;

use crate::fallible::{TryPush, collected};
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
    /// A kept entity reference. XPath never selects it or its content,
    /// whose text still counts in string values, as in libxml2.
    EntityRef(&'a str),
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
    fn ranks(&self) -> Result<Option<Vec<usize>>, TryReserveError>;
    /// `(element, attribute index)` of ID-typed attributes, sorted.
    fn ids(&self) -> &[(Self::Id, usize)];
    /// Whether a text item is a CDATA section.
    fn is_cdata(&self, id: Self::Id) -> bool;
    /// The node's source range.
    fn range(&self, id: Self::Id) -> Option<std::ops::Range<usize>>;
    /// The start of the node's source range.
    fn offset(&self, id: Self::Id) -> Option<usize> {
        self.range(id).map(|range| range.start)
    }
    /// Node origins are where libxml2 takes their line (HTML documents).
    fn marks_lines(&self) -> bool {
        false
    }
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
            Some(NodeKind::EntityRef(name)) => Item::EntityRef(name),
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

    fn ranks(&self) -> Result<Option<Vec<usize>>, TryReserveError> {
        Ok(None)
    }

    fn ids(&self) -> &[(NodeId, usize)] {
        Document::ids(self)
    }

    fn is_cdata(&self, id: NodeId) -> bool {
        self.node(id)
            .is_some_and(|node| matches!(node.kind(), NodeKind::Cdata(_)))
    }

    fn range(&self, id: NodeId) -> Option<std::ops::Range<usize>> {
        self.node(id)?.range()
    }

    fn dtd(&self) -> Option<&Dtd> {
        Document::dtd(self)
    }
}

impl<A: crate::allocator::Allocator> Navigate for OwnedDocument<A> {
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
            OwnedNodeKind::EntityRef => Item::EntityRef(node.entity_name().unwrap_or_default()),
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

    fn ranks(&self) -> Result<Option<Vec<usize>>, TryReserveError> {
        self.preorder_ranks()
    }

    fn ids(&self) -> &[(OwnedNodeId, usize)] {
        OwnedDocument::ids(self)
    }

    fn is_cdata(&self, id: OwnedNodeId) -> bool {
        self.node(id)
            .is_some_and(|node| node.kind() == OwnedNodeKind::Cdata)
    }

    fn range(&self, id: OwnedNodeId) -> Option<std::ops::Range<usize>> {
        self.node(id)?.range()
    }

    fn marks_lines(&self) -> bool {
        self.is_html()
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
pub(crate) fn find_ids<T: Navigate>(
    tree: &T,
    dtd: Option<&Dtd>,
) -> Result<Vec<(T::Id, usize)>, TryReserveError> {
    let mut declared: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (element, attribute) in dtd.into_iter().flat_map(Dtd::id_attributes) {
        declared.entry(element).or_default().insert(attribute);
    }
    let ids = preorder(tree, Some(tree.root()), None)
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
        });
    let mut ids = collected(ids)?;
    ids.sort_unstable();
    Ok(ids)
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

fn visible_owned<A: crate::allocator::Allocator>(
    node: Option<crate::OwnedNode<'_, A>>,
) -> Option<OwnedNodeId> {
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

/// `xmlNodeListGetString`: the Text and CDATA of `id` and its following
/// siblings, with each entity reference's content (`escape` `None`) or as
/// `&name;` with the text escaped (`escape` is `(html, ascii)`, see
/// `save::escape_text`). Other nodes are skipped.
pub(crate) fn list_string<T: Navigate>(
    tree: &T,
    id: T::Id,
    escape: Option<(bool, bool)>,
) -> String {
    let mut out = String::new();
    for id in std::iter::successors(Some(id), |&id| tree.next_sibling(id)) {
        match (tree.item(id), escape) {
            (Item::Text(text), None) => out.push_str(text),
            (Item::Text(text), Some((html, ascii))) => {
                crate::save::escape_text(&mut out, text, html, ascii)
            }
            (Item::EntityRef(_), None) => {
                for id in descendants(tree, id) {
                    if let Item::Text(text) = tree.item(id) {
                        out.push_str(text);
                    }
                }
            }
            (Item::EntityRef(name), Some(_)) => {
                out.push('&');
                out.push_str(name);
                out.push(';');
            }
            _ => {}
        }
    }
    out
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

/// [`preorder`] as XPath sees it: entity references are never entered.
pub(crate) fn visible_preorder<T: Navigate>(
    tree: &T,
    first: Option<T::Id>,
    stop: Option<T::Id>,
) -> impl Iterator<Item = T::Id> + '_ {
    std::iter::successors(first, move |&id| {
        let opaque = matches!(tree.item(id), Item::EntityRef(_));
        (!opaque)
            .then(|| tree.first_child(id))
            .flatten()
            .or_else(|| skip_subtree(tree, id, stop))
    })
}

/// [`descendants`] as XPath sees them, without entity-reference content.
pub(crate) fn visible_descendants<T: Navigate>(
    tree: &T,
    id: T::Id,
) -> impl Iterator<Item = T::Id> + '_ {
    visible_preorder(tree, tree.first_child(id), Some(id))
}

/// In-scope namespaces as (prefix, URI), `""` naming the default namespace.
/// Order follows libxml2: `xml` first, then `xmlGetNsList` reversed.
/// A default-namespace undeclaration (`xmlns=""`) is a node with an empty URI,
/// as `xmlGetNsListSafe` lists it for `xmlXPathNextNamespace`.
pub(crate) fn namespaces<T: Navigate>(tree: &T, element: T::Id) -> Vec<(&str, &str)> {
    if !matches!(tree.item(element), Item::Element(_)) {
        return Vec::new();
    }
    let chain = std::iter::successors(Some(element), |&id| tree.parent(id));
    let inherited = declarations(tree, chain).filter(|&(prefix, _)| prefix != "xml");
    let mut found = vec![("xml", XML_NAMESPACE)];
    found.extend(inherited.collect::<Vec<_>>().into_iter().rev());
    found
}

/// The first declaration of each prefix along `chain`, innermost first, as
/// `xmlGetNsList` lists them; `""` names the default namespace.
fn declarations<'t, T: Navigate>(
    tree: &'t T,
    chain: impl Iterator<Item = T::Id> + 't,
) -> impl Iterator<Item = (&'t str, &'t str)> + 't {
    let mut seen = Vec::new();
    chain
        .flat_map(move |id| attributes(tree, id))
        .filter_map(|(_, name, uri)| Some((name.declared_prefix()?, uri)))
        .filter(move |&(prefix, _)| {
            let new = !seen.contains(&prefix);
            seen.extend(new.then_some(prefix));
            new
        })
}

/// A namespace binding: `prefix` `None` is the default namespace, whose
/// `uri` is empty where `xmlns=""` undeclares it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NamespaceBinding<'a> {
    /// The bound prefix; `None` for the default namespace.
    pub prefix: Option<&'a str>,
    /// The namespace URI.
    pub uri: &'a str,
}

impl<'a> NamespaceBinding<'a> {
    fn new((prefix, uri): (&'a str, &'a str)) -> Self {
        let prefix = (!prefix.is_empty()).then_some(prefix);
        Self { prefix, uri }
    }
}

/// The node and its ancestors along libxml2's parent chain, where the parent
/// of entity content is the entity declaration: the chain stops below an
/// entity reference.
pub(crate) fn scope<T: Navigate>(tree: &T, id: T::Id) -> impl Iterator<Item = T::Id> + '_ {
    std::iter::successors(Some(id), |&id| {
        tree.parent(id)
            .filter(|&parent| !matches!(tree.item(parent), Item::EntityRef(_)))
    })
}

/// [`scope`]'s elements from the nearest one, as `xmlSearchNs` climbs.
fn element_scope<T: Navigate>(tree: &T, id: T::Id) -> impl Iterator<Item = T::Id> + '_ {
    let element = move |id: &T::Id| matches!(tree.item(*id), Item::Element(_));
    scope(tree, id)
        .skip_while(move |id| !element(id))
        .take_while(element)
}

/// `xml:{local}` on an element (`xmlNodeGetAttrValue(.., XML_XML_NAMESPACE)`).
pub(crate) fn xml_attribute<'t, T: Navigate>(
    tree: &'t T,
    id: T::Id,
    local: &str,
) -> Option<&'t str> {
    attributes(tree, id)
        .find(|(_, name, _)| name.namespace == Some(XML_NAMESPACE) && name.local == local)
        .map(|(_, _, value)| value)
}

fn inherited<'t, T: Navigate>(
    tree: &'t T,
    id: T::Id,
    local: &'t str,
) -> impl Iterator<Item = &'t str> + 't {
    scope(tree, id).filter_map(move |id| xml_attribute(tree, id, local))
}

/// `xmlNodeGetLang`: the nearest `xml:lang`.
pub(crate) fn lang<T: Navigate>(tree: &T, id: T::Id) -> Option<&str> {
    inherited(tree, id, "lang").next()
}

/// `xmlNodeGetSpacePreserve` for elements: the nearest `xml:space` that is
/// `preserve` (true) or `default` (false); other values are skipped.
pub(crate) fn space_preserve<T: Navigate>(tree: &T, id: T::Id) -> Option<bool> {
    matches!(tree.item(id), Item::Element(_)).then_some(())?;
    inherited(tree, id, "space").find_map(|value| match value {
        "preserve" => Some(true),
        "default" => Some(false),
        _ => None,
    })
}

/// `xmlNodeGetBase`: `id`'s `xml:base` values resolved outwards, then against
/// the document `url`. Entity content stops at its reference, as libxml2
/// stops at an internal entity's declaration.
pub(crate) fn base<T: Navigate>(tree: &T, id: T::Id, url: Option<&str>) -> Option<String> {
    resolve_base(inherited(tree, id, "base"), url)
}

/// The fold of [`base`] over `xml:base` values, innermost first; `http://`,
/// `ftp://` and `urn:` results stop early. `None` when a step fails.
pub(crate) fn resolve_base<'a>(
    bases: impl Iterator<Item = &'a str>,
    url: Option<&str>,
) -> Option<String> {
    let mut base: Option<String> = None;
    for outer in bases {
        let combined = match base {
            Some(inner) => crate::uri::resolve(&inner, outer).ok()?,
            None => outer.to_owned(),
        };
        if ["http://", "ftp://", "urn:"]
            .iter()
            .any(|scheme| combined.starts_with(scheme))
        {
            return Some(combined);
        }
        base = Some(combined);
    }
    match (base, url) {
        (Some(base), Some(url)) => crate::uri::resolve(&base, url).ok(),
        (base, url) => base.or_else(|| url.map(str::to_owned)),
    }
}

/// `xmlSearchNs`: the URI bound to `prefix` (`None`: the default namespace,
/// `""` where undeclared by `xmlns=""`); `xml` is always bound.
pub(crate) fn search_ns<'t, T: Navigate>(
    tree: &'t T,
    id: T::Id,
    prefix: Option<&str>,
) -> Option<&'t str> {
    if prefix == Some("xml") {
        return Some(XML_NAMESPACE);
    }
    let prefix = prefix.unwrap_or("");
    declarations(tree, element_scope(tree, id))
        .find(|&(bound, _)| bound == prefix)
        .map(|(_, uri)| uri)
}

/// `xmlSearchNsByHref`: the innermost declaration of `uri` whose prefix is
/// not redeclared closer to `id`.
pub(crate) fn search_ns_by_href<'t, T: Navigate>(
    tree: &'t T,
    id: T::Id,
    uri: &str,
) -> Option<NamespaceBinding<'t>> {
    if uri == XML_NAMESPACE {
        return Some(NamespaceBinding::new(("xml", XML_NAMESPACE)));
    }
    declarations(tree, element_scope(tree, id))
        .find(|&(_, bound)| bound == uri)
        .map(NamespaceBinding::new)
}

/// `xmlGetNsList`: every in-scope declaration, innermost first.
pub(crate) fn ns_list<T: Navigate>(tree: &T, id: T::Id) -> Vec<NamespaceBinding<'_>> {
    let element = |&id: &T::Id| matches!(tree.item(id), Item::Element(_));
    let chain = scope(tree, id).filter(element);
    declarations(tree, chain)
        .map(NamespaceBinding::new)
        .collect()
}

/// libxml2's `name` field: the local name of a namespaced node, else the QName.
fn libxml2_name<'a>(name: &QName<'a>) -> &'a str {
    if name.namespace.is_some() {
        name.local
    } else {
        name.qualified
    }
}

fn prefix<'a>(name: &QName<'a>) -> Option<&'a str> {
    name.namespace?;
    let prefix = name.qualified.strip_suffix(name.local)?;
    prefix.strip_suffix(':')
}

/// `xmlGetProp` (`namespace` `None`: any namespace) and `xmlGetNsProp`
/// (`Some(None)`: no namespace), falling back to a DTD default value.
pub(crate) fn property<'t, T: Navigate>(
    tree: &'t T,
    id: T::Id,
    local: &str,
    namespace: Option<Option<&str>>,
) -> Option<&'t str> {
    let Item::Element(element) = tree.item(id) else {
        return None;
    };
    let own = attributes(tree, id).find(|(_, name, _)| {
        !name.is_namespace_declaration()
            && libxml2_name(name) == local
            && namespace.is_none_or(|uri| uri == name.namespace)
    });
    if let Some((_, _, value)) = own {
        return Some(value);
    }
    let dtd = tree.dtd()?;
    let default = |element: &str, attribute: &str| dtd.default_value(element, attribute);
    match namespace {
        None => default(libxml2_name(&element), local),
        Some(None) => default(element.qualified, local),
        Some(Some(XML_NAMESPACE)) => default(element.qualified, &format!("xml:{local}")),
        Some(Some(uri)) => ns_list(tree, id)
            .into_iter()
            .filter(|binding| binding.uri == uri)
            .find_map(|binding| match binding.prefix {
                Some(prefix) => default(element.qualified, &format!("{prefix}:{local}")),
                None => default(element.qualified, local),
            }),
    }
}

/// `xmlGetNodePath`: a location path, `None` for a DOCTYPE, an entity
/// reference or entity content.
pub(crate) fn path<T: Navigate>(tree: &T, id: T::Id) -> Option<String> {
    use std::fmt::Write as _;
    let chain: Vec<T::Id> = std::iter::successors(Some(id), |&id| tree.parent(id)).collect();
    let mut path = String::new();
    for &node in chain.iter().rev() {
        let item = tree.item(node);
        let matches = |other: Item<'_>| match (item, other) {
            (Item::Element(name), Item::Element(other)) => {
                (name.namespace.is_some() && prefix(&name).is_none())
                    || libxml2_name(&name) == libxml2_name(&other)
                        && name.namespace.is_some() == other.namespace.is_some()
                        && prefix(&name) == prefix(&other)
            }
            (Item::Text(_), Item::Text(_)) | (Item::Comment(_), Item::Comment(_)) => true,
            (Item::Pi(target, _), Item::Pi(other, _)) => target == other,
            _ => false,
        };
        match item {
            Item::Root if node == tree.root() => {
                if node == id {
                    path.push('/');
                }
                continue;
            }
            Item::Root | Item::EntityRef(_) => return None,
            Item::Element(name) => match (prefix(&name), name.namespace) {
                (Some(prefix), _) => write!(path, "/{prefix}:{}", name.local),
                (None, Some(_)) => write!(path, "/*"),
                (None, None) => write!(path, "/{}", name.qualified),
            },
            Item::Text(_) => write!(path, "/text()"),
            Item::Comment(_) => write!(path, "/comment()"),
            Item::Pi(target, _) => write!(path, "/processing-instruction('{target}')"),
        }
        .ok()?;
        let siblings = tree
            .parent(node)
            .and_then(|parent| tree.first_child(parent));
        let mut siblings = std::iter::successors(siblings, |&id| tree.next_sibling(id));
        let before = siblings
            .by_ref()
            .take_while(|&sibling| sibling != node)
            .filter(|&sibling| matches(tree.item(sibling)))
            .count();
        let occurrence = match before {
            0 => usize::from(siblings.any(|sibling| matches(tree.item(sibling)))),
            before => before + 1,
        };
        if occurrence > 0 {
            write!(path, "[{occurrence}]").ok()?;
        }
    }
    Some(path)
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

/// Whitespace-only Text nodes `XML_PARSE_NOBLANKS` drops, in document order,
/// by libxml2's `areBlanks`: never under `xml:space="preserve"`, nor after
/// text in an element without `xml:space` (libxml2 sets `*ctxt->space = -2`
/// when it keeps characters); in elements the DTD declares with element
/// content always; otherwise only when the text is not the only child and
/// neither the first child nor the node before it is text, and it is not
/// followed by an entity reference. In `content` (parsed in a node's
/// context), the root's children are those of libxml2's `#root` element,
/// where blanks before the end of input stay.
pub(crate) fn blanks<T: Navigate>(tree: &T, content: bool) -> Result<Vec<T::Id>, TryReserveError> {
    let text = |id: T::Id| matches!(tree.item(id), Item::Text(_)) && !tree.is_cdata(id);
    let blank = |id: T::Id| {
        matches!(tree.item(id), Item::Text(value) if value.bytes().all(|b| b" \t\r\n".contains(&b)))
            && !tree.is_cdata(id)
    };
    let mut found = Vec::new();
    for parent in preorder(tree, Some(tree.root()), None) {
        let name = match tree.item(parent) {
            Item::Element(name) => Some(name.qualified),
            Item::Root if content => None,
            _ => continue,
        };
        let space = space_preserve(tree, parent);
        if space == Some(true) {
            continue;
        }
        let declared = name.and_then(|name| tree.dtd()?.has_element_content(name));
        let children = std::iter::successors(tree.first_child(parent), |&id| tree.next_sibling(id));
        let several = children.clone().nth(1).is_some();
        // The first and last children kept.
        let (mut first, mut last) = (None, None);
        let mut spoken_kept = false;
        for child in children {
            let leading = first
                .zip(last)
                .is_none_or(|(first, last)| !text(last) && !text(first));
            // Outside declared element content, libxml2 sees the `&` of a
            // following reference: not blank.
            let next = tree.next_sibling(child);
            let reference = next.is_some_and(|next| matches!(tree.item(next), Item::EntityRef(_)))
                || (name.is_none() && next.is_none());
            let spoken = space.is_none() && spoken_kept;
            match blank(child) && !spoken && declared.unwrap_or(!reference && several && leading) {
                true => found.try_push(child)?,
                false => {
                    spoken_kept |= text(child) && tree.offset(child).is_some();
                    first.get_or_insert(child);
                    last = Some(child);
                }
            }
        }
    }
    found.sort_unstable();
    Ok(found)
}

/// The first of `parent`'s [`flattened_children`].
pub(crate) fn flattened_first<T: Navigate>(tree: &T, parent: T::Id) -> Option<T::Id> {
    past_references(tree, parent, tree.first_child(parent))
}

/// The flattened child of `parent` after `child`.
pub(crate) fn flattened_next<T: Navigate>(tree: &T, parent: T::Id, child: T::Id) -> Option<T::Id> {
    past_references(tree, parent, flattened_step(tree, parent, child))
}

fn is_reference<T: Navigate>(tree: &T, id: T::Id) -> bool {
    matches!(tree.item(id), Item::EntityRef(_))
}

/// The next node inside `parent`, entering entity references only.
fn flattened_step<T: Navigate>(tree: &T, parent: T::Id, node: T::Id) -> Option<T::Id> {
    if is_reference(tree, node)
        && let Some(first) = tree.first_child(node)
    {
        return Some(first);
    }
    let mut node = node;
    loop {
        if let Some(next) = tree.next_sibling(node) {
            return Some(next);
        }
        node = tree.parent(node).filter(|&up| up != parent)?;
    }
}

fn past_references<T: Navigate>(tree: &T, parent: T::Id, node: Option<T::Id>) -> Option<T::Id> {
    std::iter::successors(node, |&node| flattened_step(tree, parent, node))
        .find(|&node| !is_reference(tree, node))
}
