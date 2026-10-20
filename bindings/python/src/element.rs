//! `_Element` (lxml's element proxy), `_Attrib`, `_Document` and the
//! element factories.

use std::iter::successors;
use std::sync::{Arc, Mutex, PoisonError};

use pyo3::IntoPyObjectExt;
use pyo3::exceptions::{PyIndexError, PyKeyError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PySlice, PyString, PyTuple};
use yaxml::{
    EditError, NameSpec, OwnedDocument, OwnedNode, OwnedNodeId, OwnedNodeKind, PrefixPolicy,
};

use crate::doc::{
    Doc, LIMITS, Name, NewElement, Place, Tree, XMLNS, clark, collect, elementish, lock_pair,
    remap, text_run, transplant,
};
use crate::errors::{self, EditResult, Row};
use crate::query;

#[derive(Clone)]
pub struct Loc {
    pub doc: Arc<Doc>,
    pub id: OwnedNodeId,
}

/// A proxy for an element, comment, processing instruction or entity
/// reference. The registry keeps one live proxy per node, so `is`, `==` and
/// `hash` are identity, as in lxml.
#[pyclass(frozen, weakref, module = "yaxml", name = "_Element")]
pub struct Element {
    loc: Mutex<Loc>,
}

/// `etree.CDATA`: a value that `.text` stores as a CDATA section.
#[pyclass(frozen, module = "yaxml", name = "CDATA")]
pub struct Cdata {
    text: String,
}

#[pymethods]
impl Cdata {
    #[new]
    fn new(text: String) -> Self {
        Self { text }
    }
}

/// Parses `{ns}local` (Clark notation) or `local`.
pub fn split_clark(tag: &str) -> PyResult<(Option<String>, String)> {
    let (namespace, local) = match tag.strip_prefix('{') {
        Some(rest) => match rest.split_once('}') {
            Some((ns, local)) => ((!ns.is_empty()).then(|| ns.to_owned()), local),
            None => return Err(PyValueError::new_err(format!("Invalid tag name {tag:?}"))),
        },
        None => (None, tag),
    };
    if local.is_empty() || local.contains(['{', '}']) {
        return Err(PyValueError::new_err(format!("Invalid tag name {tag:?}")));
    }
    Ok((namespace, local.to_owned()))
}

fn factory<'py>(py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyAny>> {
    py.import("yaxml._yaxml")?.getattr(name)
}

/// One `tag` argument of the `iter*` family, as lxml's `_MultiTagMatcher` reads it.
enum Test {
    Elements,
    Kind(OwnedNodeKind),
    /// `namespace`: `None` any, `Some(None)` no namespace; `local`: `None` any.
    Name {
        namespace: Option<Option<String>>,
        local: Option<String>,
    },
}

/// Empty: everything (elements, comments, PIs, entities).
pub struct Matcher(Vec<Test>);

impl Matcher {
    pub fn new(py: Python<'_>, tags: &[Bound<'_, PyAny>]) -> PyResult<Self> {
        let mut tests = Vec::new();
        for tag in tags.iter().filter(|t| !t.is_none()) {
            if let Ok(text) = tag.cast::<PyString>() {
                let text = text.to_str()?;
                tests.push(match text {
                    "*" | "{*}*" => Test::Elements,
                    _ => {
                        let (namespace, local) = match text.strip_prefix("{*}") {
                            Some(local) => (None, local),
                            None => match text.strip_prefix('{').and_then(|r| r.split_once('}')) {
                                Some((ns, local)) => {
                                    (Some((!ns.is_empty()).then(|| ns.to_owned())), local)
                                }
                                None => (Some(None), text),
                            },
                        };
                        let local = (local != "*").then(|| local.to_owned());
                        Test::Name { namespace, local }
                    }
                });
                continue;
            }
            let kind = [
                ("Comment", OwnedNodeKind::Comment),
                (
                    "ProcessingInstruction",
                    OwnedNodeKind::ProcessingInstruction,
                ),
                ("Entity", OwnedNodeKind::EntityRef),
            ]
            .into_iter()
            .find(|(name, _)| factory(py, name).is_ok_and(|f| f.is(tag)));
            match kind {
                Some((_, kind)) => tests.push(Test::Kind(kind)),
                None => {
                    return Err(PyTypeError::new_err(
                        "tag must be a string or a node factory",
                    ));
                }
            }
        }
        Ok(Self(tests))
    }

    pub fn matches(&self, node: OwnedNode<'_>) -> bool {
        if self.0.is_empty() {
            return elementish(&node);
        }
        self.0.iter().any(|test| match test {
            Test::Elements => node.kind() == OwnedNodeKind::Element,
            Test::Kind(kind) => node.kind() == *kind,
            Test::Name { namespace, local } => node.name().is_some_and(|name| {
                local.as_deref().is_none_or(|l| l == name.local())
                    && namespace
                        .as_ref()
                        .is_none_or(|ns| ns.as_deref() == name.namespace())
            }),
        })
    }
}

/// Preorder that does not enter entity references (their content belongs
/// to the entity, as in lxml's iterators).
pub fn preorder(root: OwnedNode<'_>) -> Vec<OwnedNode<'_>> {
    let (mut out, mut stack) = (Vec::new(), vec![root]);
    while let Some(node) = stack.pop() {
        out.push(node);
        if node.kind() != OwnedNodeKind::EntityRef {
            stack.extend(successors(node.last_child(), |c| c.previous_sibling()));
        }
    }
    out
}

pub fn children(node: OwnedNode<'_>) -> impl DoubleEndedIterator<Item = OwnedNode<'_>> {
    node.children()
        .filter(elementish)
        .collect::<Vec<_>>()
        .into_iter()
}

fn siblings<'d>(node: OwnedNode<'d>, preceding: bool) -> impl Iterator<Item = OwnedNode<'d>> {
    let step = move |n: &OwnedNode<'d>| match preceding {
        true => n.previous_sibling(),
        false => n.next_sibling(),
    };
    successors(step(&node), move |n| step(n)).filter(|n| elementish(n))
}

/// A Python iterator over a snapshot (lxml's `iter*` return iterators).
fn iterator<'py, T: IntoPyObject<'py>>(
    py: Python<'py>,
    items: Vec<T>,
) -> PyResult<Bound<'py, PyAny>> {
    Ok(PyList::new(py, items)?.try_iter()?.into_any())
}

fn proxies(py: Python<'_>, doc: &Arc<Doc>, ids: Vec<OwnedNodeId>) -> PyResult<Vec<Py<Element>>> {
    doc.proxies(py, &ids)
}

fn text_value(value: Option<&Bound<'_, PyAny>>) -> PyResult<Option<(String, bool)>> {
    let Some(value) = value.filter(|v| !v.is_none()) else {
        return Ok(None);
    };
    if let Ok(cdata) = value.cast::<Cdata>() {
        return Ok(Some((cdata.get().text.clone(), true)));
    }
    match value.cast::<PyString>() {
        Ok(text) => Ok(Some((text.to_str()?.to_owned(), false))),
        Err(_) => Err(PyTypeError::new_err("text must be a string, CDATA or None")),
    }
}

/// Moves `child` to `at` in `doc`; across documents per docs/bindings.md.
pub fn put(py: Python<'_>, child: &Element, doc: &Arc<Doc>, at: Place) -> PyResult<()> {
    let c = child.loc();
    if Arc::ptr_eq(&c.doc, doc) {
        let result = doc.write(py).place(c.id, at);
        return result.py(py);
    }
    let result = {
        let (mut source, mut target) = lock_pair(py, &c.doc, doc);
        transplant(&mut source, c.id, &mut target, at)
    };
    remap(py, &c.doc, doc, &result.py(py)?);
    Ok(())
}

/// Removes nodes for good unless a proxy can still see them, then unlinks.
fn discard(py: Python<'_>, doc: &Arc<Doc>, ids: Vec<OwnedNodeId>) -> PyResult<()> {
    let result = {
        let mut tree = doc.write(py);
        ids.into_iter().try_for_each(|id| {
            let subtree: Vec<_> = tree.node(id)?.descendants().map(|n| n.id()).collect();
            match doc.observed(py, subtree) {
                true => tree.detach(id),
                false => tree.delete(id),
            }
        })
    };
    result.py(py)
}

fn mapping_items(value: Option<&Bound<'_, PyAny>>) -> PyResult<Vec<(String, String)>> {
    let Some(value) = value.filter(|v| !v.is_none()) else {
        return Ok(Vec::new());
    };
    let items = match value.cast::<PyDict>() {
        Ok(dict) => dict.items().into_any(),
        Err(_) => value.call_method0("items")?,
    };
    items.try_iter()?.map(|item| item?.extract()).collect()
}

pub fn new_element(
    tag: &str,
    attrib: Option<&Bound<'_, PyAny>>,
    nsmap: Option<&Bound<'_, PyAny>>,
    extra: Option<&Bound<'_, PyDict>>,
) -> PyResult<NewElement> {
    let (namespace, local) = split_clark(tag)?;
    let mut attributes = Vec::new();
    for (key, value) in mapping_items(attrib)?
        .into_iter()
        .chain(mapping_items(extra.map(|e| e.as_any()))?)
    {
        let (namespace, local) = split_clark(&key)?;
        attributes.push((Name::clark(namespace, local), value));
    }
    let namespaces = match nsmap.filter(|n| !n.is_none()) {
        Some(map) => {
            let items = match map.cast::<PyDict>() {
                Ok(dict) => dict.items().into_any(),
                Err(_) => map.call_method0("items")?,
            };
            let pairs: Vec<(Option<String>, String)> = items
                .try_iter()?
                .map(|item| item?.extract())
                .collect::<PyResult<_>>()?;
            pairs
                .into_iter()
                .map(|(prefix, uri)| (prefix.filter(|p| !p.is_empty()), uri))
                .collect()
        }
        None => Vec::new(),
    };
    Ok(NewElement {
        name: Some(Name::clark(namespace, local)),
        attributes,
        namespaces,
    })
}

fn new_document(py: Python<'_>, new: &NewElement) -> PyResult<Py<Element>> {
    let mut new = new.clone();
    new.assign_prefixes(None);
    let created = new.with_spec(|spec| OwnedDocument::create(spec, LIMITS));
    let doc = created.py(py)?;
    let id = doc.root_element().id();
    Doc::new(Tree::new(doc, None)).proxy(py, id)
}

/// A detached leaf in a document of its own (lxml's `Comment()` and `PI()`).
fn new_leaf(
    py: Python<'_>,
    make: impl FnOnce(&mut yaxml::Editor<'_>, OwnedNodeId) -> Result<OwnedNodeId, EditError>,
) -> PyResult<Py<Element>> {
    let placeholder = NewElement {
        name: Some(Name::clark(None, "_".into())),
        ..NewElement::default()
    };
    let result = (|| {
        let mut doc = placeholder.with_spec(|spec| OwnedDocument::create(spec, LIMITS))?;
        let root = doc.root_element().id();
        let mut editor = doc.edit(LIMITS)?;
        let leaf = make(&mut editor, root)?;
        editor.unlink(leaf)?;
        Ok((doc, leaf))
    })();
    let (doc, leaf) = result.py(py)?;
    Doc::new(Tree::new(doc, None)).proxy(py, leaf)
}

#[pyfunction(name = "Element", signature = (tag, attrib = None, nsmap = None, **extra))]
pub fn element_factory(
    py: Python<'_>,
    tag: &str,
    attrib: Option<&Bound<'_, PyAny>>,
    nsmap: Option<&Bound<'_, PyAny>>,
    extra: Option<&Bound<'_, PyDict>>,
) -> PyResult<Py<Element>> {
    new_document(py, &new_element(tag, attrib, nsmap, extra)?)
}

#[pyfunction(name = "SubElement", signature = (parent, tag, attrib = None, nsmap = None, **extra))]
pub fn sub_element(
    py: Python<'_>,
    parent: &Element,
    tag: &str,
    attrib: Option<&Bound<'_, PyAny>>,
    nsmap: Option<&Bound<'_, PyAny>>,
    extra: Option<&Bound<'_, PyDict>>,
) -> PyResult<Py<Element>> {
    let new = new_element(tag, attrib, nsmap, extra)?;
    let loc = parent.loc();
    let id = parent.edit(py, |tree, id| tree.append_element(id, &new))?;
    loc.doc.proxy(py, id)
}

#[pyfunction(name = "Comment", signature = (text = None))]
pub fn comment_factory(py: Python<'_>, text: Option<&str>) -> PyResult<Py<Element>> {
    let text = text.unwrap_or("");
    new_leaf(py, |editor, root| editor.append_comment(root, text))
}

#[pyfunction(name = "ProcessingInstruction", signature = (target, text = None))]
pub fn pi_factory(py: Python<'_>, target: &str, text: Option<&str>) -> PyResult<Py<Element>> {
    let text = text.unwrap_or("");
    new_leaf(py, |editor, root| {
        editor.append_processing_instruction(root, target, text)
    })
}

#[pyfunction(name = "Entity")]
pub fn entity_factory(_name: &str) -> PyResult<()> {
    Err(PyValueError::new_err(
        "creating entity references is not supported; Entity is only a tag for iter()",
    ))
}

enum Tag {
    Name(String),
    Factory(&'static str),
}

impl Element {
    pub fn at(doc: Arc<Doc>, id: OwnedNodeId) -> Self {
        Self {
            loc: Mutex::new(Loc { doc, id }),
        }
    }

    pub fn loc(&self) -> Loc {
        self.loc
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn relocate(&self, doc: Arc<Doc>, id: OwnedNodeId) {
        *self.loc.lock().unwrap_or_else(PoisonError::into_inner) = Loc { doc, id };
    }

    /// Runs `f` on this node under the read lock; Python objects are built
    /// from its result after the lock is released.
    pub fn view<R>(
        &self,
        py: Python<'_>,
        f: impl FnOnce(&Tree, OwnedNode<'_>) -> R,
    ) -> PyResult<R> {
        let loc = self.loc();
        let tree = loc.doc.read(py);
        let result = tree.doc.node(loc.id).map(|node| f(&tree, node));
        drop(tree);
        result.ok_or_else(|| errors::stale(py))
    }

    pub fn edit<R>(
        &self,
        py: Python<'_>,
        f: impl FnOnce(&mut Tree, OwnedNodeId) -> Result<R, EditError>,
    ) -> PyResult<R> {
        let loc = self.loc();
        let result = f(&mut loc.doc.write(py), loc.id);
        result.py(py)
    }

    fn select(
        &self,
        py: Python<'_>,
        f: impl FnOnce(OwnedNode<'_>) -> Vec<OwnedNodeId>,
    ) -> PyResult<Vec<Py<Element>>> {
        let loc = self.loc();
        let ids = self.view(py, |_, node| f(node))?;
        proxies(py, &loc.doc, ids)
    }

    fn one(
        &self,
        py: Python<'_>,
        f: impl FnOnce(OwnedNode<'_>) -> Option<OwnedNode<'_>>,
    ) -> PyResult<Option<Py<Element>>> {
        let loc = self.loc();
        let id = self.view(py, |_, node| f(node).map(|n| n.id()))?;
        id.map(|id| loc.doc.proxy(py, id)).transpose()
    }

    fn kids(&self, py: Python<'_>) -> PyResult<Vec<OwnedNodeId>> {
        self.view(py, |_, node| children(node).map(|n| n.id()).collect())
    }

    fn attribute_get(&self, py: Python<'_>, key: &str) -> PyResult<Option<String>> {
        let (namespace, local) = split_clark(key)?;
        self.view(py, |_, node| {
            node.attribute_ns(&local, namespace.as_deref())
                .map(str::to_owned)
        })
    }

    fn attribute_set(&self, py: Python<'_>, key: &str, value: &str) -> PyResult<()> {
        let (namespace, local) = split_clark(key)?;
        self.edit(py, |tree, id| {
            let prefix = match &namespace {
                Some(uri) => tree.prefix_for(id, uri, true)?,
                None => None,
            };
            let name = NameSpec {
                local: &local,
                namespace: namespace.as_deref(),
                prefix: PrefixPolicy::Exact(prefix.as_deref()),
            };
            tree.doc.edit(LIMITS)?.set_attribute(id, name, value)
        })
    }

    fn attribute_delete(&self, py: Python<'_>, key: &str) -> PyResult<()> {
        let (namespace, local) = split_clark(key)?;
        let loc = self.loc();
        let result =
            loc.doc.write(py).doc.edit(LIMITS).and_then(|mut editor| {
                editor.remove_attribute(loc.id, namespace.as_deref(), &local)
            });
        result.map_err(|e| match e.kind {
            yaxml::EditErrorKind::MissingAttribute => PyKeyError::new_err(key.to_owned()),
            _ => errors::edit(py, e),
        })
    }

    pub fn attribute_items(&self, py: Python<'_>) -> PyResult<Vec<(String, String)>> {
        self.view(py, |_, node| {
            (node.attributes())
                .filter(|a| a.name.namespace() != Some(XMLNS))
                .map(|a| {
                    let key = match a.name.namespace() {
                        Some(ns) => format!("{{{ns}}}{}", a.name.local()),
                        None => a.name.local().to_owned(),
                    };
                    (key, a.value.into_owned())
                })
                .collect()
        })
    }

    /// The element-like child at a Python index.
    fn child_at(&self, py: Python<'_>, index: isize) -> PyResult<OwnedNodeId> {
        let kids = self.kids(py)?;
        let at = if index < 0 {
            index + kids.len() as isize
        } else {
            index
        };
        usize::try_from(at)
            .ok()
            .and_then(|at| kids.get(at).copied())
            .ok_or_else(|| PyIndexError::new_err("list index out of range"))
    }

    fn slice_ids(&self, py: Python<'_>, slice: &Bound<'_, PySlice>) -> PyResult<Vec<OwnedNodeId>> {
        let kids = self.kids(py)?;
        let s = slice.indices(kids.len() as isize)?;
        let mut out = Vec::new();
        let mut i = s.start;
        while (s.step > 0 && i < s.stop) || (s.step < 0 && i > s.stop) {
            out.push(kids[i as usize]);
            i += s.step;
        }
        Ok(out)
    }

    fn insert_at(&self, py: Python<'_>, index: isize, child: &Element) -> PyResult<()> {
        let loc = self.loc();
        let kids = self.kids(py)?;
        let at = if index < 0 {
            (index + kids.len() as isize).max(0)
        } else {
            index
        } as usize;
        let place = match kids.get(at) {
            Some(&sibling) => Place::Before(sibling),
            None => Place::Append(loc.id),
        };
        put(py, child, &loc.doc, place)
    }

    fn tagged(&self, py: Python<'_>) -> PyResult<Tag> {
        self.view(py, |_, node| match node.kind() {
            OwnedNodeKind::Comment => Tag::Factory("Comment"),
            OwnedNodeKind::ProcessingInstruction => Tag::Factory("ProcessingInstruction"),
            OwnedNodeKind::EntityRef => Tag::Factory("Entity"),
            _ => Tag::Name(clark(node).unwrap_or_default()),
        })
    }
}

#[pymethods]
impl Element {
    #[getter]
    fn tag(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match self.tagged(py)? {
            Tag::Name(name) => name.into_py_any(py),
            Tag::Factory(name) => Ok(factory(py, name)?.unbind()),
        }
    }

    #[setter]
    fn set_tag(&self, py: Python<'_>, tag: &str) -> PyResult<()> {
        let (namespace, local) = split_clark(tag)?;
        self.edit(py, |tree, id| {
            let chosen = match &namespace {
                Some(uri) => Some(tree.prefix_for(id, uri, false)?),
                None => None,
            };
            let name = NameSpec {
                local: &local,
                namespace: namespace.as_deref(),
                prefix: chosen.as_ref().map_or(PrefixPolicy::Automatic, |p| {
                    PrefixPolicy::Exact(p.as_deref())
                }),
            };
            tree.doc.edit(LIMITS)?.set_name(id, name)
        })
    }

    #[getter]
    fn text(&self, py: Python<'_>) -> PyResult<Option<String>> {
        self.view(py, |_, node| match node.kind() {
            OwnedNodeKind::Element => collect(text_run(node.first_child())),
            OwnedNodeKind::EntityRef => node.entity_name().map(|n| format!("&{n};")),
            _ => node.text().map(str::to_owned),
        })
    }

    #[setter]
    fn set_text(&self, py: Python<'_>, value: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        let value = text_value(value)?;
        self.edit(py, |tree, id| match &value {
            Some((text, cdata)) => tree.set_text(id, Some(text), *cdata),
            None => tree.set_text(id, None, false),
        })
    }

    #[getter]
    fn tail(&self, py: Python<'_>) -> PyResult<Option<String>> {
        self.view(py, |tree, node| tree.tail(node.id()))
    }

    #[setter]
    fn set_tail(&self, py: Python<'_>, value: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        let value = text_value(value)?.map(|(text, _)| text);
        self.edit(py, |tree, id| tree.set_tail(id, value.as_deref()))
    }

    #[getter]
    fn attrib(slf: &Bound<'_, Self>) -> Attrib {
        Attrib {
            element: slf.clone().unbind(),
        }
    }

    #[getter]
    fn prefix(&self, py: Python<'_>) -> PyResult<Option<String>> {
        self.view(py, |_, node| {
            node.name().and_then(|n| n.prefix().map(str::to_owned))
        })
    }

    /// In-scope namespaces: prefix (`None` for the default) to URI.
    #[getter]
    fn nsmap<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let pairs: Vec<(Option<String>, String)> = self.view(py, |_, node| {
            let mut bindings = node.namespaces();
            bindings.reverse();
            bindings
                .into_iter()
                .map(|b| (b.prefix.map(str::to_owned), b.uri.to_owned()))
                .collect()
        })?;
        let map = PyDict::new(py);
        for (prefix, uri) in pairs.into_iter().filter(|(_, uri)| !uri.is_empty()) {
            map.set_item(prefix, uri)?;
        }
        Ok(map)
    }

    /// `xmlGetLineNo` of a parsed node; `None` for built or copied nodes.
    #[getter]
    fn sourceline(&self, py: Python<'_>) -> PyResult<Option<usize>> {
        self.view(py, |tree, node| node.line(tree.lines.as_ref()?))
    }

    #[getter]
    fn base(&self, py: Python<'_>) -> PyResult<Option<String>> {
        self.view(py, |_, node| node.base_uri())
    }

    /// A processing instruction's target.
    #[getter]
    fn target(&self, py: Python<'_>) -> PyResult<Option<String>> {
        self.view(py, |_, node| {
            node.processing_instruction_target().map(str::to_owned)
        })
    }

    /// An entity reference's name.
    #[getter]
    fn name(&self, py: Python<'_>) -> PyResult<Option<String>> {
        self.view(py, |_, node| node.entity_name().map(str::to_owned))
    }

    #[pyo3(signature = (key, default = None))]
    fn get(&self, py: Python<'_>, key: &str, default: Option<Py<PyAny>>) -> PyResult<Py<PyAny>> {
        match self.attribute_get(py, key)? {
            Some(value) => value.into_py_any(py),
            None => Ok(default.unwrap_or_else(|| py.None())),
        }
    }

    fn set(&self, py: Python<'_>, key: &str, value: &str) -> PyResult<()> {
        self.attribute_set(py, key, value)
    }

    fn keys(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        Ok(self
            .attribute_items(py)?
            .into_iter()
            .map(|(k, _)| k)
            .collect())
    }

    fn values(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        Ok(self
            .attribute_items(py)?
            .into_iter()
            .map(|(_, v)| v)
            .collect())
    }

    fn items(&self, py: Python<'_>) -> PyResult<Vec<(String, String)>> {
        self.attribute_items(py)
    }

    fn __len__(&self, py: Python<'_>) -> PyResult<usize> {
        self.view(py, |_, node| children(node).count())
    }

    fn __iter__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let loc = self.loc();
        Ok(PyList::new(py, proxies(py, &loc.doc, self.kids(py)?)?)?
            .try_iter()?
            .into_any())
    }

    fn __reversed__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let loc = self.loc();
        let mut ids = self.kids(py)?;
        ids.reverse();
        Ok(PyList::new(py, proxies(py, &loc.doc, ids)?)?
            .try_iter()?
            .into_any())
    }

    fn __getitem__(&self, py: Python<'_>, index: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let loc = self.loc();
        if let Ok(slice) = index.cast::<PySlice>() {
            return proxies(py, &loc.doc, self.slice_ids(py, slice)?)?.into_py_any(py);
        }
        let id = self.child_at(py, index.extract()?)?;
        loc.doc.proxy(py, id)?.into_py_any(py)
    }

    fn __setitem__(
        &self,
        py: Python<'_>,
        index: &Bound<'_, PyAny>,
        value: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let loc = self.loc();
        if let Ok(slice) = index.cast::<PySlice>() {
            let old = self.slice_ids(py, slice)?;
            let start = slice.indices(self.__len__(py)? as isize)?.start;
            let new: Vec<Bound<'_, Element>> = value
                .try_iter()?
                .map(|item| Ok(item?.cast_into::<Element>()?))
                .collect::<PyResult<_>>()?;
            discard(py, &loc.doc, old)?;
            for (offset, element) in new.iter().enumerate() {
                self.insert_at(py, start + offset as isize, element.get())?;
            }
            return Ok(());
        }
        let old = self.child_at(py, index.extract()?)?;
        let new = value.cast::<Element>()?;
        put(py, new.get(), &loc.doc, Place::Before(old))?;
        discard(py, &loc.doc, vec![old])
    }

    fn __delitem__(&self, py: Python<'_>, index: &Bound<'_, PyAny>) -> PyResult<()> {
        let loc = self.loc();
        let ids = match index.cast::<PySlice>() {
            Ok(slice) => self.slice_ids(py, slice)?,
            Err(_) => vec![self.child_at(py, index.extract()?)?],
        };
        discard(py, &loc.doc, ids)
    }

    fn __contains__(&self, py: Python<'_>, element: &Bound<'_, PyAny>) -> PyResult<bool> {
        let Ok(element) = element.cast::<Element>() else {
            return Ok(false);
        };
        let (mine, theirs) = (self.loc(), element.get().loc());
        Ok(Arc::ptr_eq(&mine.doc, &theirs.doc)
            && self.view(py, |tree, _| {
                tree.doc
                    .node(theirs.id)
                    .and_then(|n| n.parent())
                    .is_some_and(|p| p.id() == mine.id)
            })?)
    }

    fn append(&self, py: Python<'_>, element: &Element) -> PyResult<()> {
        let loc = self.loc();
        put(py, element, &loc.doc, Place::Append(loc.id))
    }

    fn extend(&self, py: Python<'_>, elements: &Bound<'_, PyAny>) -> PyResult<()> {
        let elements: Vec<Bound<'_, Element>> = (elements.try_iter()?)
            .map(|item| Ok(item?.cast_into::<Element>()?))
            .collect::<PyResult<_>>()?;
        elements.iter().try_for_each(|e| self.append(py, e.get()))
    }

    fn insert(&self, py: Python<'_>, index: isize, element: &Element) -> PyResult<()> {
        self.insert_at(py, index, element)
    }

    fn remove(&self, py: Python<'_>, element: &Bound<'_, Element>) -> PyResult<()> {
        if !self.__contains__(py, element.as_any())? {
            return Err(PyValueError::new_err(
                "Element is not a child of this node.",
            ));
        }
        let loc = element.get().loc();
        let result = loc.doc.write(py).detach(loc.id);
        result.py(py)
    }

    /// Adds `element` as the next sibling, after this element's tail.
    fn addnext(&self, py: Python<'_>, element: &Element) -> PyResult<()> {
        let loc = self.loc();
        let place = self.view(py, |_, node| match siblings(node, false).next() {
            Some(next) => Place::Before(next.id()),
            None => Place::After(
                successors(Some(node), |n| n.next_sibling())
                    .last()
                    .unwrap_or(node)
                    .id(),
            ),
        })?;
        put(py, element, &loc.doc, place)
    }

    fn addprevious(&self, py: Python<'_>, element: &Element) -> PyResult<()> {
        let loc = self.loc();
        put(py, element, &loc.doc, Place::Before(loc.id))
    }

    fn replace(&self, py: Python<'_>, old: &Bound<'_, Element>, new: &Element) -> PyResult<()> {
        if !self.__contains__(py, old.as_any())? {
            return Err(PyValueError::new_err(
                "Element is not a child of this node.",
            ));
        }
        let loc = old.get().loc();
        put(py, new, &loc.doc, Place::Before(loc.id))?;
        let result = loc.doc.write(py).detach(loc.id);
        result.py(py)
    }

    #[pyo3(signature = (keep_tail = false))]
    fn clear(&self, py: Python<'_>, keep_tail: bool) -> PyResult<()> {
        let loc = self.loc();
        self.edit(py, |tree, id| {
            if tree.node(id)?.kind() == OwnedNodeKind::Element {
                tree.set_text(id, None, false)?;
                tree.clear_attributes(id)?;
            }
            match keep_tail {
                true => Ok(()),
                false => tree.set_tail(id, None),
            }
        })?;
        discard(py, &loc.doc, self.kids(py)?)
    }

    #[pyo3(signature = (child, start = None, stop = None))]
    fn index(
        &self,
        py: Python<'_>,
        child: &Element,
        start: Option<usize>,
        stop: Option<usize>,
    ) -> PyResult<usize> {
        let target = child.loc();
        let kids = self.kids(py)?;
        let end = stop.unwrap_or(kids.len()).min(kids.len());
        let begin = start.unwrap_or(0).min(end);
        (begin..end)
            .find(|&i| kids[i] == target.id && Arc::ptr_eq(&target.doc, &self.loc().doc))
            .ok_or_else(|| PyValueError::new_err("Element is not in list"))
    }

    fn getparent(&self, py: Python<'_>) -> PyResult<Option<Py<Element>>> {
        self.one(py, |node| {
            node.parent()
                .filter(|p| p.kind() != OwnedNodeKind::Document)
        })
    }

    fn getnext(&self, py: Python<'_>) -> PyResult<Option<Py<Element>>> {
        self.one(py, |node| siblings(node, false).next())
    }

    fn getprevious(&self, py: Python<'_>) -> PyResult<Option<Py<Element>>> {
        self.one(py, |node| siblings(node, true).next())
    }

    fn getroottree(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let document = Document::new(self.loc().doc, Vec::new());
        Ok(py
            .import("yaxml")?
            .getattr("_ElementTree")?
            .call1((document,))?
            .unbind())
    }

    #[pyo3(signature = (*tags, tag = None))]
    fn iter<'py>(
        &self,
        py: Python<'py>,
        tags: &Bound<'_, PyTuple>,
        tag: Option<Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let items = {
            let matcher = Matcher::new(py, &tags.iter().chain(tag).collect::<Vec<_>>())?;
            self.select(py, |node| {
                preorder(node)
                    .into_iter()
                    .filter(|n| matcher.matches(*n))
                    .map(|n| n.id())
                    .collect()
            })
        }?;
        iterator(py, items)
    }

    #[pyo3(signature = (*tags, tag = None))]
    fn iterdescendants<'py>(
        &self,
        py: Python<'py>,
        tags: &Bound<'_, PyTuple>,
        tag: Option<Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let items = {
            let matcher = Matcher::new(py, &tags.iter().chain(tag).collect::<Vec<_>>())?;
            self.select(py, |node| {
                preorder(node)
                    .into_iter()
                    .skip(1)
                    .filter(|n| matcher.matches(*n))
                    .map(|n| n.id())
                    .collect()
            })
        }?;
        iterator(py, items)
    }

    #[pyo3(signature = (*tags, tag = None, reversed = false))]
    fn iterchildren<'py>(
        &self,
        py: Python<'py>,
        tags: &Bound<'_, PyTuple>,
        tag: Option<Bound<'_, PyAny>>,
        reversed: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let items = {
            let matcher = Matcher::new(py, &tags.iter().chain(tag).collect::<Vec<_>>())?;
            self.select(py, |node| {
                let mut ids: Vec<_> = children(node)
                    .filter(|n| matcher.matches(*n))
                    .map(|n| n.id())
                    .collect();
                if reversed {
                    ids.reverse();
                }
                ids
            })
        }?;
        iterator(py, items)
    }

    #[pyo3(signature = (*tags, tag = None))]
    fn iterancestors<'py>(
        &self,
        py: Python<'py>,
        tags: &Bound<'_, PyTuple>,
        tag: Option<Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let items = {
            let matcher = Matcher::new(py, &tags.iter().chain(tag).collect::<Vec<_>>())?;
            self.select(py, |node| {
                successors(node.parent(), |n| n.parent())
                    .filter(|n| matcher.matches(*n))
                    .map(|n| n.id())
                    .collect()
            })
        }?;
        iterator(py, items)
    }

    #[pyo3(signature = (*tags, tag = None, preceding = false))]
    fn itersiblings<'py>(
        &self,
        py: Python<'py>,
        tags: &Bound<'_, PyTuple>,
        tag: Option<Bound<'_, PyAny>>,
        preceding: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let items = {
            let matcher = Matcher::new(py, &tags.iter().chain(tag).collect::<Vec<_>>())?;
            self.select(py, |node| {
                siblings(node, preceding)
                    .filter(|n| matcher.matches(*n))
                    .map(|n| n.id())
                    .collect()
            })
        }?;
        iterator(py, items)
    }

    /// Text of matching elements and the tails inside this subtree, in
    /// document order (lxml's `ElementTextIterator`).
    #[pyo3(signature = (*tags, tag = None, with_tail = true))]
    fn itertext<'py>(
        &self,
        py: Python<'py>,
        tags: &Bound<'_, PyTuple>,
        tag: Option<Bound<'_, PyAny>>,
        with_tail: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let items = {
            let matcher = Matcher::new(py, &tags.iter().chain(tag).collect::<Vec<_>>())?;
            self.view(py, |_, root| {
                let mut out = Vec::new();
                let mut stack = vec![(root, false)];
                while let Some((node, done)) = stack.pop() {
                    let wanted = matcher.matches(node);
                    if done {
                        if with_tail && node.id() != root.id() && wanted {
                            out.extend(collect(text_run(node.next_sibling())));
                        }
                        continue;
                    }
                    if node.kind() == OwnedNodeKind::Element && wanted {
                        out.extend(collect(text_run(node.first_child())));
                    }
                    stack.push((node, true));
                    if node.kind() == OwnedNodeKind::Element {
                        stack.extend(children(node).rev().map(|c| (c, false)));
                    }
                }
                out
            })
        }?;
        iterator(py, items)
    }

    #[pyo3(signature = (path, namespaces = None))]
    fn find(
        &self,
        py: Python<'_>,
        path: &str,
        namespaces: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Option<Py<Element>>> {
        Ok(query::elementpath(py, self, path, namespaces)?
            .into_iter()
            .next())
    }

    #[pyo3(signature = (path, namespaces = None))]
    fn findall(
        &self,
        py: Python<'_>,
        path: &str,
        namespaces: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Vec<Py<Element>>> {
        query::elementpath(py, self, path, namespaces)
    }

    #[pyo3(signature = (path, namespaces = None))]
    fn iterfind<'py>(
        &self,
        py: Python<'py>,
        path: &str,
        namespaces: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        Ok(
            PyList::new(py, query::elementpath(py, self, path, namespaces)?)?
                .try_iter()?
                .into_any(),
        )
    }

    #[pyo3(signature = (path, default = None, namespaces = None))]
    fn findtext(
        &self,
        py: Python<'_>,
        path: &str,
        default: Option<Py<PyAny>>,
        namespaces: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        match query::elementpath(py, self, path, namespaces)?
            .into_iter()
            .next()
        {
            Some(found) => found.get().text(py)?.unwrap_or_default().into_py_any(py),
            None => Ok(default.unwrap_or_else(|| py.None())),
        }
    }

    #[pyo3(signature = (_path, namespaces = None, **variables))]
    fn xpath(
        &self,
        py: Python<'_>,
        _path: &str,
        namespaces: Option<&Bound<'_, PyAny>>,
        variables: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Py<PyAny>> {
        let loc = self.loc();
        let compiled = query::XPath::compile(_path, namespaces)?;
        compiled.run(py, &loc.doc, loc.id, variables)
    }

    #[pyo3(signature = (tag, attrib = None, nsmap = None, **extra))]
    fn makeelement(
        &self,
        py: Python<'_>,
        tag: &str,
        attrib: Option<&Bound<'_, PyAny>>,
        nsmap: Option<&Bound<'_, PyAny>>,
        extra: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Py<Element>> {
        element_factory(py, tag, attrib, nsmap, extra)
    }

    fn __copy__(&self, py: Python<'_>) -> PyResult<Py<Element>> {
        let copied = self.view(py, |tree, node| tree.copy_out(node.id()))?;
        let (tree, id) = copied.py(py)?;
        Doc::new(tree).proxy(py, id)
    }

    fn __deepcopy__(&self, py: Python<'_>, _memo: &Bound<'_, PyAny>) -> PyResult<Py<Element>> {
        self.__copy__(py)
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let address = std::ptr::from_ref(self) as usize;
        Ok(match self.tagged(py)? {
            Tag::Name(name) => format!("<Element {name} at {address:#x}>"),
            Tag::Factory("Comment") => format!("<!--{}-->", self.text(py)?.unwrap_or_default()),
            Tag::Factory("Entity") => self.text(py)?.unwrap_or_default(),
            Tag::Factory(_) => format!(
                "<?{} {}?>",
                self.target(py)?.unwrap_or_default(),
                self.text(py)?.unwrap_or_default()
            ),
        })
    }

    fn _document(&self) -> Document {
        Document::new(self.loc().doc, Vec::new())
    }

    #[pyo3(signature = (encoding = None, pretty = false, method = "xml"))]
    fn _serialize(
        &self,
        py: Python<'_>,
        encoding: Option<&str>,
        pretty: bool,
        method: &str,
    ) -> PyResult<Vec<u8>> {
        let loc = self.loc();
        query::serialize(py, &loc.doc, Some(loc.id), encoding, pretty, method)
    }

    #[pyo3(signature = (exclusive = false, with_comments = false, inclusive_ns_prefixes = None, version = "1.0"))]
    fn _c14n(
        &self,
        py: Python<'_>,
        exclusive: bool,
        with_comments: bool,
        inclusive_ns_prefixes: Option<Vec<String>>,
        version: &str,
    ) -> PyResult<Vec<u8>> {
        let loc = self.loc();
        query::c14n(
            py,
            &loc.doc,
            Some(loc.id),
            exclusive,
            with_comments,
            inclusive_ns_prefixes.unwrap_or_default(),
            version,
        )
    }
}

/// lxml's `_Attrib`: a live mapping over an element's attributes (namespace
/// declarations excluded), keyed in Clark notation.
#[pyclass(frozen, mapping, module = "yaxml", name = "_Attrib")]
pub struct Attrib {
    element: Py<Element>,
}

#[pymethods]
impl Attrib {
    fn __getitem__(&self, py: Python<'_>, key: &str) -> PyResult<String> {
        let element = self.element.get();
        let has = element
            .attribute_items(py)?
            .into_iter()
            .find(|(k, _)| canonical(k) == canonical(key));
        has.map(|(_, v)| v)
            .ok_or_else(|| PyKeyError::new_err(key.to_owned()))
    }

    fn __setitem__(&self, py: Python<'_>, key: &str, value: &str) -> PyResult<()> {
        self.element.get().attribute_set(py, key, value)
    }

    fn __delitem__(&self, py: Python<'_>, key: &str) -> PyResult<()> {
        self.element.get().attribute_delete(py, key)
    }

    fn __contains__(&self, py: Python<'_>, key: &str) -> PyResult<bool> {
        Ok(self.__getitem__(py, key).is_ok())
    }

    fn __len__(&self, py: Python<'_>) -> PyResult<usize> {
        Ok(self.element.get().attribute_items(py)?.len())
    }

    fn __iter__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        Ok(PyList::new(py, self.keys(py)?)?.try_iter()?.into_any())
    }

    fn __bool__(&self, py: Python<'_>) -> PyResult<bool> {
        Ok(self.__len__(py)? > 0)
    }

    fn __eq__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        let mine = PyDict::new(py);
        for (k, v) in self.items(py)? {
            mine.set_item(k, v)?;
        }
        let theirs = match other.cast::<Attrib>() {
            Ok(attrib) => attrib.get().dict(py)?.into_any(),
            Err(_) => other.clone(),
        };
        mine.as_any().eq(theirs)
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(self.dict(py)?.repr()?.to_string())
    }

    #[pyo3(signature = (key, default = None))]
    fn get(&self, py: Python<'_>, key: &str, default: Option<Py<PyAny>>) -> PyResult<Py<PyAny>> {
        match self.__getitem__(py, key) {
            Ok(value) => value.into_py_any(py),
            Err(_) => Ok(default.unwrap_or_else(|| py.None())),
        }
    }

    #[pyo3(signature = (key, *default))]
    fn pop(&self, py: Python<'_>, key: &str, default: &Bound<'_, PyTuple>) -> PyResult<Py<PyAny>> {
        match self.__getitem__(py, key) {
            Ok(value) => {
                self.__delitem__(py, key)?;
                value.into_py_any(py)
            }
            Err(error) => match default.len() {
                0 => Err(error),
                _ => Ok(default.get_item(0)?.unbind()),
            },
        }
    }

    fn update(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<()> {
        mapping_items(Some(other))?
            .into_iter()
            .try_for_each(|(k, v)| self.__setitem__(py, &k, &v))
    }

    fn clear(&self, py: Python<'_>) -> PyResult<()> {
        self.element
            .get()
            .edit(py, |tree, id| tree.clear_attributes(id))
    }

    fn keys(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        self.element.get().keys(py)
    }

    fn values(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        self.element.get().values(py)
    }

    fn items(&self, py: Python<'_>) -> PyResult<Vec<(String, String)>> {
        self.element.get().attribute_items(py)
    }
}

impl Attrib {
    fn dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        for (k, v) in self.items(py)? {
            dict.set_item(k, v)?;
        }
        Ok(dict)
    }
}

/// `{}local` and `local` name the same attribute.
fn canonical(key: &str) -> &str {
    key.strip_prefix("{}").unwrap_or(key)
}

/// The document behind an `_ElementTree`.
#[pyclass(frozen, module = "yaxml", name = "_Document")]
pub struct Document {
    pub doc: Arc<Doc>,
    /// Errors a recovering parse repaired, as error-log rows.
    pub errors: Vec<Row>,
}

impl Document {
    pub fn new(doc: Arc<Doc>, errors: Vec<Row>) -> Self {
        Self { doc, errors }
    }
}

#[pymethods]
impl Document {
    fn getroot(&self, py: Python<'_>) -> PyResult<Option<Py<Element>>> {
        let id = self.doc.read(py).doc.document_element().map(|e| e.id());
        id.map(|id| self.doc.proxy(py, id)).transpose()
    }

    /// The top-level comments and processing instructions around the root.
    fn _toplevel(&self, py: Python<'_>) -> PyResult<Vec<Py<Element>>> {
        let ids: Vec<_> = (self.doc.read(py).doc.root().children())
            .filter(elementish)
            .map(|n| n.id())
            .collect();
        proxies(py, &self.doc, ids)
    }

    fn errors(&self) -> Vec<Row> {
        self.errors.clone()
    }

    #[getter]
    fn url(&self, py: Python<'_>) -> Option<String> {
        self.doc.read(py).doc.url().map(str::to_owned)
    }

    #[getter]
    fn xml_version(&self, py: Python<'_>) -> String {
        let tree = self.doc.read(py);
        tree.doc
            .declaration()
            .map_or("1.0", |d| d.version)
            .to_owned()
    }

    #[getter]
    fn encoding(&self, py: Python<'_>) -> String {
        let tree = self.doc.read(py);
        tree.doc
            .declaration()
            .and_then(|d| d.encoding)
            .unwrap_or("UTF-8")
            .to_owned()
    }

    #[getter]
    fn standalone(&self, py: Python<'_>) -> Option<bool> {
        self.doc
            .read(py)
            .doc
            .declaration()
            .and_then(|d| d.standalone)
    }

    #[getter]
    fn root_name(&self, py: Python<'_>) -> Option<String> {
        let tree = self.doc.read(py);
        match tree.doc.doctype() {
            Some(doctype) => Some(doctype.name.to_owned()),
            None => tree
                .doc
                .document_element()
                .and_then(|e| e.name().map(|n| n.qualified().to_owned())),
        }
    }

    #[getter]
    fn public_id(&self, py: Python<'_>) -> Option<String> {
        self.doc
            .read(py)
            .doc
            .doctype()
            .and_then(|d| d.public_id.map(str::to_owned))
    }

    #[getter]
    fn system_url(&self, py: Python<'_>) -> Option<String> {
        self.doc
            .read(py)
            .doc
            .doctype()
            .and_then(|d| d.system_id.map(str::to_owned))
    }

    #[getter]
    fn internal_subset(&self, py: Python<'_>) -> Option<String> {
        self.doc
            .read(py)
            .doc
            .doctype()
            .and_then(|d| d.internal_subset.map(str::to_owned))
    }

    fn getpath(&self, py: Python<'_>, element: &Element) -> PyResult<Option<String>> {
        element.view(py, |_, node| node.path())
    }

    #[pyo3(signature = (_path, namespaces = None, **variables))]
    fn xpath(
        &self,
        py: Python<'_>,
        _path: &str,
        namespaces: Option<&Bound<'_, PyAny>>,
        variables: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Py<PyAny>> {
        let root = self.doc.read(py).doc.root().id();
        query::XPath::compile(_path, namespaces)?.run(py, &self.doc, root, variables)
    }

    #[pyo3(signature = (encoding = None, pretty = false, method = "xml"))]
    fn _serialize(
        &self,
        py: Python<'_>,
        encoding: Option<&str>,
        pretty: bool,
        method: &str,
    ) -> PyResult<Vec<u8>> {
        query::serialize(py, &self.doc, None, encoding, pretty, method)
    }

    #[pyo3(signature = (exclusive = false, with_comments = false, inclusive_ns_prefixes = None, version = "1.0"))]
    fn _c14n(
        &self,
        py: Python<'_>,
        exclusive: bool,
        with_comments: bool,
        inclusive_ns_prefixes: Option<Vec<String>>,
        version: &str,
    ) -> PyResult<Vec<u8>> {
        query::c14n(
            py,
            &self.doc,
            None,
            exclusive,
            with_comments,
            inclusive_ns_prefixes.unwrap_or_default(),
            version,
        )
    }

    fn __copy__(&self, py: Python<'_>) -> PyResult<Document> {
        let cloned = {
            let tree = self.doc.read(py);
            tree.doc.try_clone().map(|doc| Tree::new(doc, None))
        };
        Ok(Document::new(Doc::new(cloned.py(py)?), Vec::new()))
    }

    fn __deepcopy__(&self, py: Python<'_>, _memo: &Bound<'_, PyAny>) -> PyResult<Document> {
        self.__copy__(py)
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Element>()?;
    m.add_class::<Attrib>()?;
    m.add_class::<Document>()?;
    m.add_class::<Cdata>()?;
    m.add_function(wrap_pyfunction!(element_factory, m)?)?;
    m.add_function(wrap_pyfunction!(sub_element, m)?)?;
    m.add_function(wrap_pyfunction!(comment_factory, m)?)?;
    m.add_function(wrap_pyfunction!(pi_factory, m)?)?;
    m.add("PI", m.getattr("ProcessingInstruction")?)?;
    m.add_function(wrap_pyfunction!(entity_factory, m)?)?;
    Ok(())
}
