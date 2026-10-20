//! Parsing into owned documents, resolvers, and `iterparse` over the
//! streaming `TextReader`.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Cursor, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use pyo3::IntoPyObjectExt;
use pyo3::exceptions::{PyOSError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::sync::MutexExt;
use pyo3::types::{PyByteArray, PyBytes, PyString};
use yaxml::diagnostic::Level;
use yaxml::{
    EditLimits, LineIndex, NodeType, OwnedDocument, OwnedNodeId, ParseOptions, ReadError, Request,
    Resolver, ResourceKind, TextReader, options,
};

use crate::doc::{Doc, Name, NewElement, Tree, XMLNS};
use crate::element::{Document, Element, Matcher, comment_factory, pi_factory};
use crate::errors::{self, EditResult, Row, row};

/// The first Python exception a resolver or reader raised, re-raised
/// instead of the parse error it caused.
pub type Failure = Arc<Mutex<Option<PyErr>>>;

fn remember(failure: &Failure, error: PyErr) -> io::Error {
    let message = error.to_string();
    failure
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert(error);
    io::Error::other(message)
}

fn take(failure: &Failure) -> Option<PyErr> {
    failure
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
}

fn kind_name(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::DtdSubset => "dtd",
        ResourceKind::ParameterEntity => "parameter_entity",
        ResourceKind::GeneralEntity => "entity",
        ResourceKind::XInclude => "xinclude",
        ResourceKind::XIncludeText => "xinclude_text",
        ResourceKind::Catalog => "catalog",
        ResourceKind::Schema => "schema",
        ResourceKind::RelaxNg => "relaxng",
    }
}

fn bytes_of(value: &Bound<'_, PyAny>) -> PyResult<Option<Vec<u8>>> {
    if let Ok(bytes) = value.cast::<PyBytes>() {
        return Ok(Some(bytes.as_bytes().to_vec()));
    }
    if let Ok(bytes) = value.cast::<PyByteArray>() {
        return Ok(Some(bytes.to_vec()));
    }
    match value.cast::<PyString>() {
        Ok(text) => Ok(Some(text.to_str()?.as_bytes().to_vec())),
        Err(_) => Ok(None),
    }
}

/// A Python callable `(uri, public_id, kind) -> bytes | str | None`.
struct PyResolver {
    callable: Py<PyAny>,
    failure: Failure,
}

impl Resolver for PyResolver {
    fn resolve(&mut self, request: Request<'_>) -> io::Result<Option<Vec<u8>>> {
        Python::attach(|py| {
            let called = self.callable.bind(py).call1((
                request.uri,
                request.public_id,
                kind_name(request.kind),
            ));
            let value = called.map_err(|e| remember(&self.failure, e))?;
            if value.is_none() {
                return Ok(None);
            }
            match bytes_of(&value) {
                Ok(Some(bytes)) => Ok(Some(bytes)),
                Ok(None) => Err(remember(
                    &self.failure,
                    PyTypeError::new_err("a resolver returns bytes, str or None"),
                )),
                Err(e) => Err(remember(&self.failure, e)),
            }
        })
    }
}

/// `yaxml.FileResolver(root)`: serves files under one directory, never the
/// network (the core's `FileResolver`).
#[pyclass(frozen, module = "yaxml", name = "FileResolver")]
pub struct FileResolver {
    inner: yaxml::FileResolver,
    #[pyo3(get)]
    root: PathBuf,
}

#[pymethods]
impl FileResolver {
    #[new]
    fn new(root: PathBuf) -> PyResult<Self> {
        let inner =
            yaxml::FileResolver::new(&root).map_err(|e| PyOSError::new_err(e.to_string()))?;
        Ok(Self { inner, root })
    }
}

/// The core resolver for a Python resolver argument.
pub fn resolver(
    value: Option<&Bound<'_, PyAny>>,
) -> PyResult<(Option<Box<dyn Resolver>>, Failure)> {
    let failure = Failure::default();
    let Some(value) = value.filter(|v| !v.is_none()) else {
        return Ok((None, failure));
    };
    if let Ok(files) = value.cast::<FileResolver>() {
        return Ok((Some(Box::new(files.get().inner.clone())), failure));
    }
    if !value.is_callable() {
        return Err(PyTypeError::new_err(
            "resolver must be callable or a FileResolver",
        ));
    }
    let resolver = PyResolver {
        callable: value.clone().unbind(),
        failure: failure.clone(),
    };
    Ok((Some(Box::new(resolver)), failure))
}

/// A declining resolver for APIs that require one.
pub fn or_none(resolver: Option<Box<dyn Resolver>>) -> Box<dyn Resolver> {
    resolver.unwrap_or_else(|| Box::new(|_: Request<'_>| Ok(None)))
}

/// Parses `data` (bytes or str) into an `_Document`. A `str` is UTF-8 with
/// its declared encoding ignored.
#[pyfunction(signature = (data, *, flags = 0, resolver = None, base_url = None))]
pub fn parse_document(
    py: Python<'_>,
    data: &Bound<'_, PyAny>,
    flags: u32,
    resolver: Option<&Bound<'_, PyAny>>,
    base_url: Option<String>,
) -> PyResult<Document> {
    let text = data.is_instance_of::<PyString>();
    let input =
        bytes_of(data)?.ok_or_else(|| PyTypeError::new_err("can only parse bytes or str"))?;
    let flags = flags | if text { options::IGNORE_ENC } else { 0 };
    let (resolver, failure) = self::resolver(resolver)?;
    let options = ParseOptions::new(flags)
        .base(base_url.clone())
        .resolver(resolver);
    let parsed = py.detach(|| OwnedDocument::parse_with(&input, options));
    let filename = base_url.as_deref();
    let doc = match parsed {
        Ok(doc) => doc,
        Err(error) => {
            return Err(
                take(&failure).unwrap_or_else(|| errors::syntax(py, error, &input, filename))
            );
        }
    };
    let lines = LineIndex::owned(input);
    let mut rows: Vec<Row> = (lines.describe_all(doc.errors()).iter())
        .map(|d| row(d, filename))
        .collect();
    let validity: Vec<Row> = (doc.validity().iter())
        .map(|d| row(&d.describe(&lines), filename))
        .collect();
    if validity.iter().any(|r| r.3 >= Level::Error as u8) && flags & options::RECOVER == 0 {
        return Err(errors::syntax_from(py, &validity));
    }
    rows.extend(validity);
    Ok(Document::new(Doc::new(Tree::new(doc, Some(lines))), rows))
}

/// Reads a Python file object (`read(n)` returning bytes or str).
struct PyRead {
    file: Py<PyAny>,
    pending: Vec<u8>,
    failure: Failure,
}

impl Read for PyRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pending.is_empty() {
            let chunk = Python::attach(|py| {
                let value = self
                    .file
                    .bind(py)
                    .call_method1("read", (buf.len().max(1),))?;
                bytes_of(&value)?
                    .ok_or_else(|| PyTypeError::new_err("read() must return bytes or str"))
            });
            self.pending = chunk.map_err(|e| remember(&self.failure, e))?;
            self.pending.reverse();
        }
        let n = buf.len().min(self.pending.len());
        for slot in &mut buf[..n] {
            *slot = self.pending.pop().expect("n bytes pending");
        }
        Ok(n)
    }
}

const EVENTS: [&str; 6] = ["start", "end", "start-ns", "end-ns", "comment", "pi"];

/// Limits for trees built from parser events: the parser's `Limits`
/// already bounded them.
const BUILD: EditLimits = EditLimits::DEFAULT
    .max_value_bytes(usize::MAX)
    .max_name_bytes(usize::MAX)
    .max_supplied_bytes(usize::MAX)
    .max_attributes(usize::MAX)
    .max_depth(usize::MAX)
    .max_namespace_work(usize::MAX);

struct State {
    reader: TextReader<Box<dyn BufRead + Send>>,
    input: Option<Arc<[u8]>>,
    wanted: [bool; 6],
    matcher: Matcher,
    doc: Option<Arc<Doc>>,
    /// Open elements and the declarations each made.
    stack: Vec<(OwnedNodeId, usize)>,
    queue: VecDeque<Py<PyAny>>,
    root: Option<OwnedNodeId>,
    done: bool,
    failure: Failure,
}

/// lxml's `iterparse`: `(event, element)` pairs while the tree grows.
#[pyclass(module = "yaxml", name = "iterparse")]
pub struct IterParse {
    state: Mutex<State>,
}

impl State {
    fn event(&mut self, py: Python<'_>, index: usize, payload: Py<PyAny>) -> PyResult<()> {
        if self.wanted[index] {
            self.queue
                .push_back((EVENTS[index], payload).into_py_any(py)?);
        }
        Ok(())
    }

    fn element_event(&mut self, py: Python<'_>, index: usize, id: OwnedNodeId) -> PyResult<()> {
        let doc = self.doc.clone().expect("a document once elements exist");
        let matched = doc
            .read(py)
            .doc
            .node(id)
            .is_some_and(|n| self.matcher.matches(n));
        if self.wanted[index] && matched {
            let element = doc.proxy(py, id)?.into_py_any(py)?;
            self.event(py, index, element)?;
        }
        Ok(())
    }

    fn fail(&mut self, py: Python<'_>, error: ReadError) -> PyErr {
        self.done = true;
        if let Some(error) = take(&self.failure) {
            return error;
        }
        match (error, &self.input) {
            (ReadError::Xml(error), Some(input)) => errors::syntax(py, error, input, None),
            (ReadError::Xml(error), None) => errors::bare(py, error),
            (error, _) => PyOSError::new_err(error.to_string()),
        }
    }

    fn start(&mut self, py: Python<'_>) -> PyResult<()> {
        let reader = &mut self.reader;
        let empty = reader.is_empty_element();
        let name = Name {
            namespace: reader.namespace_uri().map(str::to_owned),
            local: reader.local_name().unwrap_or_default().to_owned(),
            prefix: Some(reader.prefix().map(str::to_owned)),
        };
        let mut new = NewElement {
            name: Some(name),
            ..NewElement::default()
        };
        for index in 0..reader.attribute_count() {
            reader.move_to_attribute_no(index);
            let local = reader.local_name().unwrap_or_default().to_owned();
            let value = reader.value().unwrap_or_default().to_owned();
            let prefix = reader.prefix().map(str::to_owned);
            match reader.namespace_uri() {
                Some(XMLNS) => new.namespaces.push((prefix.map(|_| local), value)),
                namespace => new.attributes.push((
                    Name {
                        namespace: namespace.map(str::to_owned),
                        local,
                        prefix: Some(prefix),
                    },
                    value,
                )),
            }
        }
        reader.move_to_element();
        let declared: Vec<_> = new.namespaces.clone();
        let made = match (&self.doc, self.stack.last()) {
            (Some(doc), Some(&(parent, _))) => {
                let result = doc.write(py).doc.edit(BUILD).and_then(|mut editor| {
                    new.with_spec(|spec| editor.append_element(parent, spec))
                });
                result.py(py)?
            }
            _ => {
                let created = new
                    .with_spec(|spec| OwnedDocument::create(spec, BUILD))
                    .py(py)?;
                let id = created.root_element().id();
                self.doc = Some(Doc::new(Tree::new(created, None)));
                self.root = Some(id);
                id
            }
        };
        for (prefix, uri) in declared.iter().cloned() {
            let pair = (prefix.unwrap_or_default(), uri).into_py_any(py)?;
            self.event(py, 2, pair)?;
        }
        self.element_event(py, 0, made)?;
        self.stack.push((made, declared.len()));
        if empty {
            self.end(py)?;
        }
        Ok(())
    }

    fn end(&mut self, py: Python<'_>) -> PyResult<()> {
        let Some((id, declared)) = self.stack.pop() else {
            return Ok(());
        };
        self.element_event(py, 1, id)?;
        for _ in 0..declared {
            self.event(py, 3, py.None())?;
        }
        Ok(())
    }

    /// The parent for content here: the open element, or after the root
    /// the document; `None` in the prolog.
    fn parent(&self, py: Python<'_>) -> Option<OwnedNodeId> {
        match self.stack.last() {
            Some(&(id, _)) => Some(id),
            None => Some(self.doc.as_ref()?.read(py).doc.root().id()),
        }
    }

    fn leaf(&mut self, py: Python<'_>, kind: NodeType) -> PyResult<()> {
        let value = self.reader.value().unwrap_or_default().to_owned();
        let target = self.reader.name().unwrap_or_default().to_owned();
        let parent = self.parent(py);
        if matches!(
            kind,
            NodeType::Text
                | NodeType::Cdata
                | NodeType::SignificantWhitespace
                | NodeType::Whitespace
        ) {
            let (Some(doc), Some(&(parent, _))) = (&self.doc, self.stack.last()) else {
                return Ok(());
            };
            let result = doc
                .write(py)
                .doc
                .edit(BUILD)
                .and_then(|mut editor| match kind {
                    NodeType::Cdata => editor.append_cdata(parent, &value),
                    _ => editor.append_text(parent, &value),
                });
            return result.map(drop).py(py);
        }
        let comment = kind == NodeType::Comment;
        let index = if comment { 4 } else { 5 };
        let (Some(doc), Some(parent)) = (self.doc.clone(), parent) else {
            if self.wanted[index] {
                let leaf = match comment {
                    true => comment_factory(py, Some(&value))?,
                    false => pi_factory(py, &target, Some(&value))?,
                };
                self.event(py, index, leaf.into_py_any(py)?)?;
            }
            return Ok(());
        };
        let result = doc
            .write(py)
            .doc
            .edit(BUILD)
            .and_then(|mut editor| match comment {
                true => editor.append_comment(parent, &value),
                false => editor.append_processing_instruction(parent, &target, &value),
            });
        let id = result.py(py)?;
        if self.wanted[index] {
            let leaf = doc.proxy(py, id)?.into_py_any(py)?;
            self.event(py, index, leaf)?;
        }
        Ok(())
    }

    fn step(&mut self, py: Python<'_>) -> PyResult<()> {
        match self.reader.read() {
            Ok(true) => {}
            Ok(false) => {
                self.done = true;
                return Ok(());
            }
            Err(error) => return Err(self.fail(py, error)),
        }
        match self.reader.node_type() {
            NodeType::Element => self.start(py),
            NodeType::EndElement => self.end(py),
            kind @ (NodeType::Text
            | NodeType::Cdata
            | NodeType::SignificantWhitespace
            | NodeType::Whitespace
            | NodeType::Comment
            | NodeType::ProcessingInstruction) => self.leaf(py, kind),
            _ => Ok(()),
        }
    }
}

#[pymethods]
impl IterParse {
    #[new]
    #[pyo3(signature = (source, events = None, tag = None, *, flags = 0, resolver = None, base_url = None))]
    fn new(
        py: Python<'_>,
        source: &Bound<'_, PyAny>,
        events: Option<Vec<String>>,
        tag: Option<Bound<'_, PyAny>>,
        flags: u32,
        resolver: Option<&Bound<'_, PyAny>>,
        base_url: Option<String>,
    ) -> PyResult<Self> {
        let (resolver, failure) = self::resolver(resolver)?;
        let mut input = None;
        let reader: Box<dyn BufRead + Send> = if let Ok(bytes) = source.cast::<PyBytes>() {
            let bytes = Arc::<[u8]>::from(bytes.as_bytes());
            input = Some(bytes.clone());
            Box::new(Cursor::new(bytes))
        } else if source.hasattr("read")? {
            let file = PyRead {
                file: source.clone().unbind(),
                pending: Vec::new(),
                failure: failure.clone(),
            };
            Box::new(BufReader::with_capacity(64 * 1024, file))
        } else {
            let path: PathBuf = source.extract()?;
            let file = std::fs::File::open(&path)
                .map_err(|e| PyOSError::new_err(format!("{}: {e}", path.display())))?;
            Box::new(BufReader::with_capacity(64 * 1024, file))
        };
        let events = events.unwrap_or_else(|| vec!["end".into()]);
        let mut wanted = [false; 6];
        for event in &events {
            let index = EVENTS
                .iter()
                .position(|e| e == event)
                .ok_or_else(|| PyValueError::new_err(format!("invalid event name '{event}'")))?;
            wanted[index] = true;
        }
        // Recovered events can name unbound prefixes, which the tree built
        // here cannot hold, so the streaming reader does not recover.
        if flags & options::RECOVER != 0 {
            let unsupported = yaxml::Error {
                kind: yaxml::ErrorKind::UnsupportedOption,
                offset: 0,
            };
            return Err(errors::bare(py, unsupported));
        }
        let options = ParseOptions::new(flags).base(base_url).resolver(resolver);
        let reader = TextReader::with_options(reader, options).map_err(|e| errors::bare(py, e))?;
        Ok(Self {
            state: Mutex::new(State {
                reader,
                input,
                wanted,
                matcher: Matcher::new(py, &tag.into_iter().collect::<Vec<_>>())?,
                doc: None,
                stack: Vec::new(),
                queue: VecDeque::new(),
                root: None,
                done: false,
                failure,
            }),
        })
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        let mut state = self
            .state
            .lock_py_attached(py)
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(event) = state.queue.pop_front() {
                return Ok(Some(event));
            }
            if state.done {
                return Ok(None);
            }
            state.step(py)?;
        }
    }

    /// The root element once parsing has started.
    #[getter]
    fn root(&self, py: Python<'_>) -> PyResult<Option<Py<Element>>> {
        let state = self
            .state
            .lock_py_attached(py)
            .unwrap_or_else(PoisonError::into_inner);
        match (&state.doc, state.root) {
            (Some(doc), Some(id)) => Ok(Some(doc.proxy(py, id)?)),
            _ => Ok(None),
        }
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<FileResolver>()?;
    m.add_class::<IterParse>()?;
    m.add_function(wrap_pyfunction!(parse_document, m)?)
}
