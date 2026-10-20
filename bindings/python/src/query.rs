//! XPath (`XPath`, `.xpath()`, ElementPath `find*`), serialization and C14N.

use std::sync::Arc;

use pyo3::IntoPyObjectExt;
use pyo3::exceptions::{PyLookupError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyString};
use yaxml::xpath::{self, Context, Node, Value};
use yaxml::{OwnedNodeId, OwnedNodeKind, c14n, save};

use crate::doc::{Doc, elementish};
use crate::element::{Document, Element};
use crate::errors::raise;

const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// `etree.XPath`: a compiled expression with its prefix bindings.
#[pyclass(frozen, module = "yaxml", name = "XPath")]
pub struct XPath {
    compiled: xpath::XPath,
    namespaces: Vec<(String, String)>,
    #[pyo3(get)]
    path: String,
}

/// What a node-set member becomes in Python, read under the lock.
enum Item {
    Node(OwnedNodeId),
    Text(String),
    Namespace(Option<String>, String),
    Tree,
}

fn namespaces(map: Option<&Bound<'_, PyAny>>) -> PyResult<Vec<(String, String)>> {
    let Some(map) = map.filter(|m| !m.is_none()) else {
        return Ok(Vec::new());
    };
    let items = match map.cast::<PyDict>() {
        Ok(dict) => dict.items().into_any(),
        Err(_) => map.call_method0("items")?,
    };
    let mut out = Vec::new();
    for item in items.try_iter()? {
        let (prefix, uri): (Option<String>, String) = item?.extract()?;
        match prefix.filter(|p| !p.is_empty()) {
            Some(prefix) => out.push((prefix, uri)),
            None => {
                return Err(PyTypeError::new_err(
                    "empty namespace prefix is not supported in XPath",
                ));
            }
        }
    }
    Ok(out)
}

/// The document and context node of an XPath target: an element, a
/// `_Document`, or anything with a `_doc` (`_ElementTree`).
pub fn target(py: Python<'_>, target: &Bound<'_, PyAny>) -> PyResult<(Arc<Doc>, OwnedNodeId)> {
    if let Ok(element) = target.cast::<Element>() {
        let loc = element.get().loc();
        return Ok((loc.doc, loc.id));
    }
    let document = match target.cast::<Document>() {
        Ok(document) => document.clone(),
        Err(_) => (target.getattr("_doc").ok())
            .and_then(|doc| doc.cast_into::<Document>().ok())
            .ok_or_else(|| PyTypeError::new_err("expected an element or element tree"))?,
    };
    let doc = document.get().doc.clone();
    let root = doc.read(py).doc.root().id();
    Ok((doc, root))
}

fn variable(doc: &Arc<Doc>, value: &Bound<'_, PyAny>) -> PyResult<Value<OwnedNodeId>> {
    if let Ok(flag) = value.cast::<PyBool>() {
        return Ok(Value::Boolean(flag.is_true()));
    }
    if value.is_instance_of::<PyInt>() || value.is_instance_of::<PyFloat>() {
        return Ok(Value::Number(value.extract()?));
    }
    if let Ok(text) = value.cast::<PyString>() {
        return Ok(Value::String(text.to_str()?.to_owned()));
    }
    let elements: Vec<Bound<'_, PyAny>> = match value.cast::<Element>() {
        Ok(_) => vec![value.clone()],
        Err(_) => value.try_iter()?.collect::<PyResult<_>>()?,
    };
    let mut nodes = Vec::new();
    for element in elements {
        let loc = element.cast::<Element>()?.get().loc();
        if !Arc::ptr_eq(&loc.doc, doc) {
            return Err(PyValueError::new_err(
                "XPath variable nodes must be in the evaluated document",
            ));
        }
        nodes.push(Node::Tree(loc.id));
    }
    nodes.sort();
    Ok(Value::NodeSet(nodes))
}

impl XPath {
    pub fn compile(path: &str, map: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        let namespaces = namespaces(map)?;
        let compiled = xpath::XPath::compile(path).map_err(|e| {
            let (code, message) = e.libxml2(path);
            Python::attach(|py| raise(py, "XPathSyntaxError", (message, code)))
        })?;
        Ok(Self {
            compiled,
            namespaces,
            path: path.to_owned(),
        })
    }

    pub fn run(
        &self,
        py: Python<'_>,
        doc: &Arc<Doc>,
        node: OwnedNodeId,
        variables: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Py<PyAny>> {
        let mut context = Context::new();
        for (prefix, uri) in &self.namespaces {
            context = context.with_namespace(prefix, uri);
        }
        for (name, value) in variables.into_iter().flat_map(|v| v.iter()) {
            context = context.with_variable(name.extract::<String>()?, variable(doc, &value)?);
        }
        if doc.read(py).doc.node(node).is_none() {
            return Err(crate::errors::stale(py));
        }
        let evaluated = {
            let tree = doc.read(py);
            let tree = &*tree;
            let value = py.detach(|| self.compiled.evaluate(&tree.doc, node, &context));
            value.map(|value| match value {
                Value::NodeSet(nodes) => Ok(nodes
                    .into_iter()
                    .map(|n| item(&tree.doc, n))
                    .collect::<Vec<Item>>()),
                other => Err(other),
            })
        };
        match evaluated {
            Err(e) => {
                let (code, message) = e.libxml2(&self.path);
                Err(raise(py, "XPathEvalError", (message, code)))
            }
            Ok(Err(Value::Boolean(b))) => b.into_py_any(py),
            Ok(Err(Value::Number(n))) => n.into_py_any(py),
            Ok(Err(Value::String(s))) => s.into_py_any(py),
            Ok(Err(Value::NodeSet(_))) => unreachable!("node-sets are converted above"),
            Ok(Ok(items)) => {
                let ids: Vec<_> = (items.iter())
                    .filter_map(|item| match item {
                        Item::Node(id) => Some(*id),
                        _ => None,
                    })
                    .collect();
                let mut nodes = doc.proxies(py, &ids)?.into_iter();
                let out: Vec<Py<PyAny>> = (items.into_iter())
                    .map(|item: Item| match item {
                        Item::Node(_) => nodes.next().expect("one proxy per node").into_py_any(py),
                        Item::Text(text) => text.into_py_any(py),
                        Item::Namespace(prefix, uri) => (prefix, uri).into_py_any(py),
                        Item::Tree => Ok(py
                            .import("yaxml")?
                            .getattr("_ElementTree")?
                            .call1((Document::new(doc.clone(), Vec::new()),))?
                            .unbind()),
                    })
                    .collect::<PyResult<_>>()?;
                out.into_py_any(py)
            }
        }
    }

    /// Element-like nodes the expression selects (for ElementPath).
    fn elements(
        &self,
        py: Python<'_>,
        doc: &Arc<Doc>,
        node: OwnedNodeId,
    ) -> PyResult<Vec<Py<Element>>> {
        let context = (self.namespaces.iter()).fold(Context::new(), |c, (prefix, uri)| {
            c.with_namespace(prefix, uri)
        });
        if doc.read(py).doc.node(node).is_none() {
            return Err(crate::errors::stale(py));
        }
        let selected = {
            let tree = doc.read(py);
            let nodes = self.compiled.select_nodes(&tree.doc, node, &context);
            nodes.map(|nodes| {
                (nodes.into_iter())
                    .filter_map(|n| match n {
                        Node::Tree(id) => tree.doc.node(id).filter(elementish).map(|n| n.id()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
        };
        let ids = selected.map_err(|e| {
            let (code, message) = e.libxml2(&self.path);
            raise(py, "XPathEvalError", (message, code))
        })?;
        doc.proxies(py, &ids)
    }
}

fn item(doc: &yaxml::OwnedDocument, node: Node<OwnedNodeId>) -> Item {
    match node {
        Node::Tree(id) => match doc.node(id) {
            Some(n) if n.kind() == OwnedNodeKind::Document => Item::Tree,
            Some(n) if elementish(&n) => Item::Node(id),
            Some(n) => Item::Text(n.text().unwrap_or("").to_owned()),
            None => Item::Text(String::new()),
        },
        Node::Attribute(id, index) => Item::Text(
            (doc.node(id).and_then(|n| n.attributes().nth(index)))
                .map_or_else(String::new, |a| a.value.into_owned()),
        ),
        Node::Namespace(_, 0) => Item::Namespace(Some("xml".into()), XML_NAMESPACE.into()),
        Node::Namespace(id, index) => {
            let mut bindings = doc.node(id).map(|n| n.namespaces()).unwrap_or_default();
            bindings.retain(|b| b.prefix != Some("xml"));
            bindings.reverse();
            match bindings.get(index - 1) {
                Some(b) => Item::Namespace(b.prefix.map(str::to_owned), b.uri.to_owned()),
                None => Item::Text(String::new()),
            }
        }
    }
}

#[pymethods]
impl XPath {
    #[new]
    #[pyo3(signature = (path, *, namespaces = None))]
    fn new(path: &str, namespaces: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        Self::compile(path, namespaces)
    }

    #[pyo3(signature = (_etree_or_element, **variables))]
    fn __call__(
        &self,
        py: Python<'_>,
        _etree_or_element: &Bound<'_, PyAny>,
        variables: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Py<PyAny>> {
        let (doc, node) = target(py, _etree_or_element)?;
        self.run(py, &doc, node, variables)
    }

    fn _elements(&self, py: Python<'_>, target: &Bound<'_, PyAny>) -> PyResult<Vec<Py<Element>>> {
        let (doc, node) = self::target(py, target)?;
        self.elements(py, &doc, node)
    }

    fn __repr__(&self) -> String {
        self.path.clone()
    }
}

/// ElementPath: `yaxml._elementpath.compile` translates it to an `XPath`.
pub fn elementpath(
    py: Python<'_>,
    element: &Element,
    path: &str,
    map: Option<&Bound<'_, PyAny>>,
) -> PyResult<Vec<Py<Element>>> {
    let compiled = py
        .import("yaxml._elementpath")?
        .getattr("compile")?
        .call1((path, map))?;
    let compiled = compiled.cast::<XPath>()?;
    let loc = element.loc();
    compiled.get().elements(py, &loc.doc, loc.id)
}

fn failed(py: Python<'_>, class: &str, error: impl std::fmt::Display) -> PyErr {
    raise(py, class, (error.to_string(),))
}

/// `xmlSaveTree` of a node, or `xmlSaveDoc` without the declaration for the
/// document (`id` `None`); `method` "xml", "html" or "text".
pub fn serialize(
    py: Python<'_>,
    doc: &Arc<Doc>,
    id: Option<OwnedNodeId>,
    encoding: Option<&str>,
    pretty: bool,
    method: &str,
) -> PyResult<Vec<u8>> {
    let flags = match method {
        "xml" => save::NO_DECL,
        "html" => save::AS_HTML,
        "text" => 0,
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown serialisation method {other:?}"
            )));
        }
    } | if pretty { save::FORMAT } else { 0 };
    let tree = doc.read(py);
    let node = match id {
        Some(id) => tree.doc.node(id).ok_or_else(|| crate::errors::stale(py))?,
        None => tree.doc.root(),
    };
    if method == "text" {
        let text = node.content().unwrap_or_default();
        return Ok(text.into_bytes());
    }
    let options = save::Options::new(flags).encoding(encoding);
    // lxml writes a tree's top-level nodes one after another, with a line
    // break only after the DOCTYPE; libxml2 breaks after each.
    let whole = id.is_none() && !pretty && method == "xml";
    let saved = py.detach(|| match whole {
        true => node.children().try_fold(Vec::new(), |mut out, child| {
            out.extend(save::to_vec(child, &options)?);
            if child.kind() == OwnedNodeKind::DocumentType {
                out.push(b'\n');
            }
            Ok(out)
        }),
        false => save::to_vec(node, &options),
    });
    drop(tree);
    saved.map_err(|e| match e.kind {
        yaxml::ErrorKind::UnsupportedEncoding => PyLookupError::new_err(format!(
            "unknown encoding: '{}'",
            encoding.unwrap_or_default()
        )),
        _ => failed(py, "SerialisationError", e),
    })
}

/// Canonical XML of the document, or of one node's subtree (its
/// descendants, attributes and namespace nodes, as lxml selects them).
pub fn c14n(
    py: Python<'_>,
    doc: &Arc<Doc>,
    id: Option<OwnedNodeId>,
    exclusive: bool,
    with_comments: bool,
    inclusive: Vec<String>,
    version: &str,
) -> PyResult<Vec<u8>> {
    let mode = match (exclusive, version) {
        (true, _) => c14n::Mode::Exclusive10,
        (false, "1.0") => c14n::Mode::C14n10,
        (false, "1.1") => c14n::Mode::C14n11,
        (false, other) => {
            return Err(PyValueError::new_err(format!(
                "unsupported C14N version {other:?}"
            )));
        }
    };
    let tree = doc.read(py);
    let nodes = match id {
        Some(id) => {
            let subtree = xpath::XPath::compile(
                "descendant-or-self::node() | descendant-or-self::*/@* | descendant-or-self::*/namespace::*",
            )
            .expect("constant expression");
            let selected = subtree.select_nodes(&tree.doc, id, &Context::new());
            Some(selected.map_err(|e| failed(py, "C14NError", e))?)
        }
        None => None,
    };
    let prefixes: Vec<&str> = inclusive.iter().map(String::as_str).collect();
    let options = c14n::Options::new(mode)
        .with_comments(with_comments)
        .nodes(nodes.as_deref())
        .inclusive_prefixes(&prefixes);
    let tree_ref = &*tree;
    let result = py.detach(|| c14n::to_string(&tree_ref.doc, &options));
    drop(tree);
    result
        .map(String::into_bytes)
        .map_err(|e| failed(py, "C14NError", e))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<XPath>()
}
