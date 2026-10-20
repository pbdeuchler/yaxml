//! One shared document per Python tree: the owned DOM behind a lock, the
//! input's line index (for `sourceline` and validator lines), lxml's tails of
//! parentless nodes, and the registry that gives each node one live proxy.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::iter::successors;
use std::sync::{Arc, Mutex, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use pyo3::prelude::*;
use pyo3::sync::{MutexExt, RwLockExt};
use pyo3::types::{PyWeakrefMethods, PyWeakrefReference};
use yaxml::{
    AttributeSpec, EditError, EditErrorKind, EditLimits, Editor, ElementItem, ElementSpec,
    LineIndex, NameSpec, NamespaceSpec, OwnedDocument, OwnedNode, OwnedNodeId, OwnedNodeKind,
    PrefixPolicy,
};

use crate::element::Element;

/// Editing ceilings for every binding edit: the core defaults.
pub const LIMITS: EditLimits = EditLimits::DEFAULT;

pub struct Tree {
    pub doc: OwnedDocument,
    /// The parsed input, owned by its index; `None` for built trees.
    pub lines: Option<LineIndex<'static>>,
    /// Detached text nodes holding the tail of a node without an element
    /// parent: lxml keeps a tail on unlinked and top-level nodes, an arena
    /// tree cannot link siblings without a parent.
    pub tails: HashMap<OwnedNodeId, Vec<OwnedNodeId>>,
}

#[derive(Default)]
struct Registry {
    proxies: HashMap<OwnedNodeId, Py<PyWeakrefReference>, BuildHasherDefault<IdHasher>>,
    prune_at: usize,
}

/// A multiplicative hasher for node handles (two words); SipHash cost a
/// measurable share of proxy creation.
#[derive(Default)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    fn write(&mut self, bytes: &[u8]) {
        bytes.iter().for_each(|&b| self.write_u64(u64::from(b)));
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

fn upgrade<'py>(py: Python<'py>, weak: &Py<PyWeakrefReference>) -> Option<Bound<'py, Element>> {
    weak.bind(py).upgrade_as::<Element>().ok().flatten()
}

pub struct Doc {
    tree: RwLock<Tree>,
    registry: Mutex<Registry>,
}

pub enum Place {
    Append(OwnedNodeId),
    Before(OwnedNodeId),
    After(OwnedNodeId),
}

/// An expanded name with its lexical choice: `prefix` `None` is automatic.
#[derive(Clone, Debug)]
pub struct Name {
    pub namespace: Option<String>,
    pub local: String,
    pub prefix: Option<Option<String>>,
}

/// A fresh element: name, attributes and declarations, owned.
#[derive(Clone, Debug, Default)]
pub struct NewElement {
    pub name: Option<Name>,
    pub attributes: Vec<(Name, String)>,
    pub namespaces: Vec<(Option<String>, String)>,
}

impl Name {
    pub fn clark(namespace: Option<String>, local: String) -> Self {
        Self {
            namespace,
            local,
            prefix: None,
        }
    }

    /// An exact prefix, else one the element declares for the URI (only
    /// prefixed ones for attributes), else automatic.
    fn spec<'a>(
        &'a self,
        attribute: bool,
        declared: &'a [(Option<String>, String)],
    ) -> NameSpec<'a> {
        let prefix = match (&self.prefix, &self.namespace) {
            (Some(exact), _) => PrefixPolicy::Exact(exact.as_deref()),
            (None, Some(uri)) => (declared.iter())
                .find(|(p, u)| u == uri && (p.is_some() || !attribute))
                .map_or(PrefixPolicy::Automatic, |(p, _)| {
                    PrefixPolicy::Exact(p.as_deref())
                }),
            (None, None) if attribute => PrefixPolicy::Exact(None),
            (None, None) => PrefixPolicy::Automatic,
        };
        NameSpec {
            local: &self.local,
            namespace: self.namespace.as_deref(),
            prefix,
        }
    }
}

type Bindings = [(Option<String>, String)];

/// An in-scope binding of `uri` (prefixed for attributes); the element's own
/// declarations shadow inherited ones.
fn bound(
    declared: &Bindings,
    inherited: &Bindings,
    uri: &str,
    attribute: bool,
) -> Option<Option<String>> {
    let visible = inherited
        .iter()
        .filter(|(p, _)| !declared.iter().any(|(q, _)| q == p));
    (declared.iter().chain(visible))
        .find(|(p, u)| u == uri && (p.is_some() || !attribute))
        .map(|(p, _)| p.clone())
}

/// lxml's generated prefix: the first `nsN` bound nowhere in scope.
fn generated(declared: &Bindings, inherited: &Bindings) -> String {
    (0..)
        .map(|n| format!("ns{n}"))
        .find(|c| {
            !declared
                .iter()
                .chain(inherited)
                .any(|(p, _)| p.as_deref() == Some(c))
        })
        .expect("an unused prefix")
}

fn in_scope(scope: Option<OwnedNode<'_>>) -> Vec<(Option<String>, String)> {
    (scope
        .map(|s| s.namespaces())
        .unwrap_or_default()
        .into_iter())
    .map(|b| (b.prefix.map(str::to_owned), b.uri.to_owned()))
    .collect()
}

impl NewElement {
    /// Chooses prefixes for automatic namespaced names as lxml does: an
    /// in-scope binding of the URI, else `ns0`, `ns1`, ... declared here.
    pub fn assign_prefixes(&mut self, scope: Option<OwnedNode<'_>>) {
        let inherited = in_scope(scope);
        let attributes = self.attributes.iter_mut().map(|(name, _)| (name, true));
        for (name, attribute) in self.name.iter_mut().map(|n| (n, false)).chain(attributes) {
            let (None, Some(uri)) = (&name.prefix, &name.namespace) else {
                continue;
            };
            let prefix = bound(&self.namespaces, &inherited, uri, attribute).unwrap_or_else(|| {
                let prefix = generated(&self.namespaces, &inherited);
                self.namespaces.push((Some(prefix.clone()), uri.clone()));
                Some(prefix)
            });
            name.prefix = Some(prefix);
        }
    }

    pub fn with_spec<R>(&self, f: impl FnOnce(ElementSpec<'_>) -> R) -> R {
        let declared = &self.namespaces;
        let items: Vec<ElementItem<'_>> = (declared.iter())
            .map(|(prefix, uri)| {
                ElementItem::Namespace(NamespaceSpec {
                    prefix: prefix.as_deref(),
                    uri,
                })
            })
            .chain(self.attributes.iter().map(|(name, value)| {
                ElementItem::Attribute(AttributeSpec {
                    name: name.spec(true, declared),
                    value,
                })
            }))
            .collect();
        let name = self.name.as_ref().expect("an element name");
        f(ElementSpec {
            name: name.spec(false, declared),
            items: &items,
        })
    }
}

pub fn elementish(node: &OwnedNode<'_>) -> bool {
    matches!(
        node.kind(),
        OwnedNodeKind::Element
            | OwnedNodeKind::Comment
            | OwnedNodeKind::ProcessingInstruction
            | OwnedNodeKind::EntityRef
    )
}

fn texty(node: &OwnedNode<'_>) -> bool {
    matches!(node.kind(), OwnedNodeKind::Text | OwnedNodeKind::Cdata)
}

/// The run of Text/CDATA starting at `start` (lxml's `_collectText` scope).
pub fn text_run<'d>(start: Option<OwnedNode<'d>>) -> impl Iterator<Item = OwnedNode<'d>> {
    successors(start.filter(texty), |n| n.next_sibling().filter(texty))
}

/// lxml's text value of a run: `None` without nodes, else their concatenation.
pub fn collect<'d>(run: impl Iterator<Item = OwnedNode<'d>>) -> Option<String> {
    run.fold(None, |text: Option<String>, node| {
        Some(text.unwrap_or_default() + node.text().unwrap_or(""))
    })
}

/// Whether `node` sits under an element (or entity reference), where its tail
/// is the following text siblings.
pub fn parented(node: OwnedNode<'_>) -> bool {
    node.parent()
        .is_some_and(|p| p.kind() != OwnedNodeKind::Document)
}

fn in_entity(node: OwnedNode<'_>) -> bool {
    successors(node.parent(), |n| n.parent()).any(|n| n.kind() == OwnedNodeKind::EntityRef)
}

pub fn clark(node: OwnedNode<'_>) -> Option<String> {
    let name = node.name()?;
    Some(match name.namespace() {
        Some(ns) => format!("{{{ns}}}{}", name.local()),
        None => name.local().to_owned(),
    })
}

pub const fn failure(kind: EditErrorKind) -> EditError {
    EditError {
        kind,
        value_offset: None,
        location: None,
    }
}

impl Tree {
    pub fn new(doc: OwnedDocument, lines: Option<LineIndex<'static>>) -> Self {
        Self {
            doc,
            lines,
            tails: HashMap::new(),
        }
    }

    pub fn node(&self, id: OwnedNodeId) -> Result<OwnedNode<'_>, EditError> {
        self.doc
            .node(id)
            .ok_or(failure(EditErrorKind::InvalidHandle))
    }

    fn editor(&mut self) -> Result<Editor<'_>, EditError> {
        self.doc.edit(LIMITS)
    }

    /// The text nodes that make up a node's tail.
    pub fn tail_ids(&self, id: OwnedNodeId) -> Vec<OwnedNodeId> {
        match self.doc.node(id) {
            Some(node) if parented(node) => text_run(node.next_sibling()).map(|n| n.id()).collect(),
            Some(_) => self.tails.get(&id).cloned().unwrap_or_default(),
            None => Vec::new(),
        }
    }

    pub fn tail(&self, id: OwnedNodeId) -> Option<String> {
        collect(
            self.tail_ids(id)
                .into_iter()
                .filter_map(|t| self.doc.node(t)),
        )
    }

    pub fn set_text(
        &mut self,
        id: OwnedNodeId,
        value: Option<&str>,
        cdata: bool,
    ) -> Result<(), EditError> {
        let node = self.node(id)?;
        match node.kind() {
            OwnedNodeKind::Element => {
                let old: Vec<_> = text_run(node.first_child()).map(|n| n.id()).collect();
                let mut editor = self.editor()?;
                if let Some(value) = value {
                    let text = match cdata {
                        true => editor.append_cdata(id, value)?,
                        false => editor.append_text(id, value)?,
                    };
                    let first = editor.document().node(id).and_then(|n| n.first_child());
                    if let Some(first) = first.map(|f| f.id()).filter(|&f| f != text) {
                        editor.insert_before(first, text)?;
                    }
                }
                old.into_iter().try_for_each(|t| editor.remove_subtree(t))
            }
            OwnedNodeKind::Comment | OwnedNodeKind::ProcessingInstruction => {
                self.editor()?.set_node_value(id, value.unwrap_or(""))
            }
            _ => Err(failure(EditErrorKind::WrongNodeKind)),
        }
    }

    pub fn set_tail(&mut self, id: OwnedNodeId, value: Option<&str>) -> Result<(), EditError> {
        let node = self.node(id)?;
        let old = self.tail_ids(id);
        if parented(node) {
            let parent = node.parent().expect("parented").id();
            let mut editor = self.editor()?;
            if let Some(value) = value {
                let text = editor.append_text(parent, value)?;
                editor.insert_after(id, text)?;
            }
            return old.into_iter().try_for_each(|t| editor.remove_subtree(t));
        }
        let host = match node.kind() {
            OwnedNodeKind::Element => id,
            _ => self.doc.root_element().id(),
        };
        let mut editor = self.editor()?;
        let made = match value {
            Some(value) => {
                let text = editor.append_text(host, value)?;
                editor.unlink(text)?;
                vec![text]
            }
            None => Vec::new(),
        };
        old.into_iter().try_for_each(|t| editor.remove_subtree(t))?;
        match made.is_empty() {
            true => self.tails.remove(&id),
            false => self.tails.insert(id, made),
        };
        Ok(())
    }

    /// Moves a node (with its tail) within this document.
    pub fn place(&mut self, node: OwnedNodeId, at: Place) -> Result<(), EditError> {
        let side = !parented(self.node(node)?);
        let tail = self.tail_ids(node);
        let mut editor = self.editor()?;
        put(&mut editor, node, &at)?;
        let mut previous = node;
        for t in tail {
            editor.insert_after(previous, t)?;
            previous = t;
        }
        if side {
            self.tails.remove(&node);
        }
        Ok(())
    }

    /// Unlinks a node from its parent; its tail goes with it.
    pub fn detach(&mut self, node: OwnedNodeId) -> Result<(), EditError> {
        if !parented(self.node(node)?) {
            return Ok(());
        }
        let tail = self.tail_ids(node);
        let mut editor = self.editor()?;
        editor.unlink(node)?;
        tail.iter().try_for_each(|&t| editor.unlink(t))?;
        if !tail.is_empty() {
            self.tails.insert(node, tail);
        }
        Ok(())
    }

    /// Removes a node and its tail for good.
    pub fn delete(&mut self, node: OwnedNodeId) -> Result<(), EditError> {
        let tail = self.tail_ids(node);
        let mut editor = self.editor()?;
        editor.remove_subtree(node)?;
        tail.into_iter()
            .try_for_each(|t| editor.remove_subtree(t))?;
        self.tails.remove(&node);
        Ok(())
    }

    /// Removes every attribute (declarations stay, as in lxml's `clear`).
    pub fn clear_attributes(&mut self, id: OwnedNodeId) -> Result<(), EditError> {
        let names: Vec<(Option<String>, String)> = (self.node(id)?.attributes())
            .filter(|a| a.name.namespace() != Some(XMLNS))
            .map(|a| {
                (
                    a.name.namespace().map(str::to_owned),
                    a.name.local().to_owned(),
                )
            })
            .collect();
        let mut editor = self.editor()?;
        names
            .iter()
            .try_for_each(|(ns, local)| editor.remove_attribute(id, ns.as_deref(), local))
    }

    /// The prefix to spell `uri` with on an existing element, declaring a
    /// generated one there when nothing in scope binds it.
    pub fn prefix_for(
        &mut self,
        id: OwnedNodeId,
        uri: &str,
        attribute: bool,
    ) -> Result<Option<String>, EditError> {
        let inherited = in_scope(Some(self.node(id)?));
        if let Some(prefix) = bound(&[], &inherited, uri, attribute) {
            return Ok(prefix);
        }
        let prefix = generated(&[], &inherited);
        self.editor()?.declare_namespace(id, Some(&prefix), uri)?;
        Ok(Some(prefix))
    }

    pub fn append_element(
        &mut self,
        parent: OwnedNodeId,
        new: &NewElement,
    ) -> Result<OwnedNodeId, EditError> {
        let mut new = new.clone();
        new.assign_prefixes(Some(self.node(parent)?));
        let mut editor = self.editor()?;
        new.with_spec(|spec| editor.append_element(parent, spec))
    }

    /// A deep copy of `node` (and its tail) as a new document: an element
    /// becomes its document element, any other node a detached node.
    pub fn copy_out(&self, node: OwnedNodeId) -> Result<(Tree, OwnedNodeId), EditError> {
        let kind = self.node(node)?.kind();
        let placeholder = NewElement {
            name: Some(Name::clark(None, "_".into())),
            ..NewElement::default()
        };
        let mut doc = placeholder.with_spec(|spec| OwnedDocument::create(spec, LIMITS))?;
        let mut editor = doc.edit(LIMITS)?;
        let copy = editor.import(&self.doc, node, true)?;
        let tails = (self.tail_ids(node).into_iter())
            .map(|t| editor.import(&self.doc, t, true))
            .collect::<Result<Vec<_>, _>>()?;
        if kind == OwnedNodeKind::Element {
            let root = editor.document().root_element().id();
            editor.replace(root, copy)?;
            editor.remove_subtree(root)?;
        }
        let mut tree = Tree::new(doc, None);
        if !tails.is_empty() {
            tree.tails.insert(copy, tails);
        }
        Ok((tree, copy))
    }
}

pub const XMLNS: &str = "http://www.w3.org/2000/xmlns/";

fn put(editor: &mut Editor<'_>, node: OwnedNodeId, at: &Place) -> Result<(), EditError> {
    match *at {
        Place::Append(parent) => editor.append_child(parent, node).map(drop),
        Place::Before(sibling) => editor.insert_before(sibling, node).map(drop),
        Place::After(sibling) => editor.insert_after(sibling, node).map(drop),
    }
}

/// Moves `node` (with its tail) from `src` into `dst` as docs/bindings.md
/// describes: import, insert, then remove from the source. Returns the
/// source-to-copy handle pairs for remapping proxies. A document element,
/// or a node inside entity content, is copied and stays where it was.
pub fn transplant(
    src: &mut Tree,
    node: OwnedNodeId,
    dst: &mut Tree,
    at: Place,
) -> Result<Vec<(OwnedNodeId, OwnedNodeId)>, EditError> {
    let source = src.node(node)?;
    let side = !parented(source);
    let copy_only = src.doc.document_element().map(|e| e.id()) == Some(node) || in_entity(source);
    let tail = src.tail_ids(node);
    let mut editor = dst.editor()?;
    let mut made = Vec::new();
    let inserted = (|| {
        let copy = editor.import(&src.doc, node, true)?;
        made.push(copy);
        for &t in &tail {
            made.push(editor.import(&src.doc, t, true)?);
        }
        put(&mut editor, copy, &at)?;
        made.windows(2)
            .try_for_each(|pair| editor.insert_after(pair[0], pair[1]).map(drop))?;
        Ok(copy)
    })();
    let copy = match inserted {
        Ok(copy) => copy,
        Err(error) => {
            made.into_iter().for_each(|m| {
                let _ = editor.remove_subtree(m);
            });
            return Err(error);
        }
    };
    let pairs = pairs(&src.doc, node, editor.document(), copy);
    if !copy_only {
        let mut editor = src.editor()?;
        editor.remove_subtree(node)?;
        tail.into_iter()
            .try_for_each(|t| editor.remove_subtree(t))?;
        if side {
            src.tails.remove(&node);
        }
    }
    Ok(pairs)
}

/// Corresponding nodes of a subtree and its imported copy: preorder in step,
/// except that a reference to an entity the destination does not declare
/// was replaced by its content and has no counterpart.
fn pairs(
    src: &OwnedDocument,
    from: OwnedNodeId,
    dst: &OwnedDocument,
    to: OwnedNodeId,
) -> Vec<(OwnedNodeId, OwnedNodeId)> {
    let (Some(from), Some(to)) = (src.node(from), dst.node(to)) else {
        return Vec::new();
    };
    let mut copies = to.descendants().peekable();
    let mut out = Vec::new();
    for s in from.descendants() {
        let Some(d) = copies.peek() else { break };
        if s.kind() == OwnedNodeKind::EntityRef && d.kind() != OwnedNodeKind::EntityRef {
            continue;
        }
        out.push((s.id(), d.id()));
        copies.next();
    }
    out
}

impl Doc {
    pub fn new(tree: Tree) -> Arc<Self> {
        Arc::new(Self {
            tree: RwLock::new(tree),
            registry: Mutex::new(Registry {
                proxies: HashMap::default(),
                prune_at: 64,
            }),
        })
    }

    pub fn read(&self, py: Python<'_>) -> RwLockReadGuard<'_, Tree> {
        self.tree
            .read_py_attached(py)
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn write(&self, py: Python<'_>) -> RwLockWriteGuard<'_, Tree> {
        self.tree
            .write_py_attached(py)
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn registry(&self, py: Python<'_>) -> std::sync::MutexGuard<'_, Registry> {
        self.registry
            .lock_py_attached(py)
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether any of `ids` has a live proxy.
    pub fn observed(&self, py: Python<'_>, ids: impl IntoIterator<Item = OwnedNodeId>) -> bool {
        let registry = self.registry(py);
        (ids.into_iter()).any(|id| {
            registry
                .proxies
                .get(&id)
                .is_some_and(|w| w.bind(py).upgrade().is_some())
        })
    }

    fn register(
        &self,
        py: Python<'_>,
        registry: &mut Registry,
        id: OwnedNodeId,
        weak: Py<PyWeakrefReference>,
    ) {
        if registry.proxies.len() >= registry.prune_at {
            registry
                .proxies
                .retain(|_, w| w.bind(py).upgrade().is_some());
            registry.prune_at = (registry.proxies.len() * 2).max(64);
        }
        registry.proxies.insert(id, weak);
    }

    pub fn proxy(self: &Arc<Self>, py: Python<'_>, id: OwnedNodeId) -> PyResult<Py<Element>> {
        Ok(self.proxies(py, &[id])?.pop().expect("one proxy"))
    }

    /// The one live proxy of each node, made where missing. Python objects
    /// are allocated outside the registry lock.
    pub fn proxies(
        self: &Arc<Self>,
        py: Python<'_>,
        ids: &[OwnedNodeId],
    ) -> PyResult<Vec<Py<Element>>> {
        let mut found: Vec<Option<Py<Element>>> = {
            let registry = self.registry(py);
            (ids.iter())
                .map(|id| {
                    registry
                        .proxies
                        .get(id)
                        .and_then(|w| upgrade(py, w))
                        .map(Bound::unbind)
                })
                .collect()
        };
        let mut made = Vec::new();
        for (index, slot) in found.iter_mut().enumerate().filter(|(_, s)| s.is_none()) {
            let element = Bound::new(py, Element::at(self.clone(), ids[index]))?;
            made.push((index, PyWeakrefReference::new(element.as_any())?.unbind()));
            *slot = Some(element.unbind());
        }
        let mut losers = Vec::new();
        if !made.is_empty() {
            let mut registry = self.registry(py);
            for (index, weak) in made {
                // Another thread, or a repeated id, made one first: it wins.
                match registry
                    .proxies
                    .get(&ids[index])
                    .and_then(|w| upgrade(py, w))
                {
                    Some(first) => losers.extend(found[index].replace(first.unbind())),
                    None => self.register(py, &mut registry, ids[index], weak),
                }
            }
        }
        drop(losers);
        Ok(found
            .into_iter()
            .map(|p| p.expect("every slot filled"))
            .collect())
    }
}

/// Points the live proxies of moved nodes at their copies.
pub fn remap(py: Python<'_>, from: &Arc<Doc>, to: &Arc<Doc>, pairs: &[(OwnedNodeId, OwnedNodeId)]) {
    let moved: Vec<(Py<PyWeakrefReference>, OwnedNodeId)> = {
        let mut registry = from.registry(py);
        (pairs.iter())
            .filter_map(|(s, d)| registry.proxies.remove(s).map(|w| (w, *d)))
            .collect()
    };
    for (weak, id) in moved {
        if let Some(element) = upgrade(py, &weak) {
            element.get().relocate(to.clone(), id);
            to.register(py, &mut to.registry(py), id, weak);
        }
    }
}

/// Write locks on two documents, always taken in address order.
pub fn lock_pair<'a>(
    py: Python<'_>,
    a: &'a Doc,
    b: &'a Doc,
) -> (RwLockWriteGuard<'a, Tree>, RwLockWriteGuard<'a, Tree>) {
    if std::ptr::from_ref(a) < std::ptr::from_ref(b) {
        let first = a.write(py);
        (first, b.write(py))
    } else {
        let second = b.write(py);
        (a.write(py), second)
    }
}
