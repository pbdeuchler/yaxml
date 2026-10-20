//! Documents: parsing, navigation, XPath, output and validation.

use std::borrow::Cow;
use std::ffi::c_void;
use std::io;

use yaxml::xpath::{self, Context, Value, XPath};
use yaxml::{
    EditLimits, ElementSpec, LineIndex, Name, OwnedDocument, OwnedNode, OwnedNodeId, OwnedNodeKind,
    c14n, relaxng, save, schematron, xsd,
};

use crate::error::{Diagnostic, Fail, guard};
use crate::ffi::{Out, Records, WriteFn, slice, text};
use crate::options::{edit_limits, parse_options};
use crate::registry::{Registry, buffer, hand_out};
use crate::types::*;
use crate::*;

/// A document with the input it was parsed from (for line numbers and
/// diagnostics; empty for created documents) and its editing limits.
pub(crate) struct Doc {
    pub(crate) tree: OwnedDocument,
    pub(crate) source: Vec<u8>,
    pub(crate) limits: EditLimits,
}

pub(crate) static DOCS: Registry<Doc> = Registry::new();

pub(crate) enum Schema {
    Xsd(xsd::Schema),
    RelaxNg(relaxng::Schema),
    Schematron(schematron::Schema),
}

static SCHEMAS: Registry<Schema> = Registry::new();

pub(crate) fn live() -> usize {
    DOCS.len() + SCHEMAS.len()
}

pub(crate) fn read<R>(
    doc: *mut yaxml_doc,
    f: impl FnOnce(&Doc) -> Result<R, Fail>,
) -> Result<R, Fail> {
    DOCS.get(doc)?.read(f)
}

pub(crate) fn handle(id: OwnedNodeId) -> yaxml_node {
    let (doc, slot) = id.parts();
    yaxml_node {
        doc: doc as u64,
        slot: slot as u64,
    }
}

pub(crate) fn id(node: yaxml_node) -> Result<OwnedNodeId, Fail> {
    let (Ok(doc), Ok(slot)) = (usize::try_from(node.doc), usize::try_from(node.slot)) else {
        return Err(Fail::handle());
    };
    OwnedNodeId::from_parts(doc, slot).ok_or_else(Fail::handle)
}

pub(crate) fn view(tree: &OwnedDocument, node: yaxml_node) -> Result<OwnedNode<'_>, Fail> {
    tree.node(id(node)?).ok_or_else(Fail::handle)
}

/// A name's parts through `intern`. Owned-document names are pool views, so
/// with `yaxml_str::of` the strings live as long as the document's pool.
pub(crate) fn name_of(name: &Name<'_>, intern: &mut impl FnMut(&str) -> yaxml_str) -> yaxml_name {
    yaxml_name {
        qname: intern(name.qualified()),
        prefix: name.prefix().map_or(yaxml_str::NULL, &mut *intern),
        local: intern(name.local()),
        uri: name.namespace().map_or(yaxml_str::NULL, intern),
    }
}

/// An owned document's attribute values are pool views.
fn pooled(value: Cow<'_, str>) -> Result<&str, Fail> {
    match value {
        Cow::Borrowed(value) => Ok(value),
        Cow::Owned(_) => Err(Fail::new(ERR_XML, "internal error: owned attribute value")),
    }
}

/// `items` handed to C; empty is NULL, which owns nothing.
pub(crate) fn array<T: Send + 'static>(items: Vec<T>) -> (*const T, usize) {
    if items.is_empty() {
        return (std::ptr::null(), 0);
    }
    let (ptr, len) = (items.as_ptr(), items.len());
    hand_out(ptr.cast(), Box::new(items));
    (ptr, len)
}

/// Diagnostics handed to C as one allocation: the records and their messages.
fn diagnostics(list: Vec<Diagnostic>) -> yaxml_diagnostics {
    if list.is_empty() {
        return yaxml_diagnostics::default();
    }
    let mut text = String::with_capacity(list.iter().map(|d| d.message.len()).sum());
    let starts: Vec<usize> = (list.iter())
        .map(|d| {
            let start = text.len();
            text.push_str(&d.message);
            start
        })
        .collect();
    let base = text.as_ptr();
    let records: Vec<yaxml_diagnostic> = (list.iter().zip(starts))
        .map(|(d, start)| {
            d.record(yaxml_str {
                ptr: base.wrapping_add(start),
                len: d.message.len(),
            })
        })
        .collect();
    let (items, len) = (records.as_ptr(), records.len());
    hand_out(items.cast(), Box::new((Records(records), text)));
    yaxml_diagnostics {
        items,
        len,
        errors: list.iter().filter(|d| d.is_error()).count(),
    }
}

/// Answers a read-only node query: `f` sees the document and the node.
fn query<T: Default>(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    out: *mut T,
    err: *mut yaxml_error,
    f: impl FnOnce(&Doc, OwnedNode<'_>) -> Result<Option<T>, Fail>,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        let value = read(doc, |d| f(d, view(&d.tree, node)?))?;
        // SAFETY: yaxml.h: `out` is valid for writing the result type.
        Ok(unsafe { out.reply(value) })
    })
}

// ---- documents ----

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_parse(
    bytes: *const u8,
    len: usize,
    options: *const yaxml_parse_options,
    out: *mut *mut yaxml_doc,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        // SAFETY: yaxml.h: `bytes` holds `len` bytes for the call.
        let input = unsafe { slice(bytes, len) }?;
        // SAFETY: yaxml.h: `options` is NULL or a valid options struct.
        let options = unsafe { parse_options(options) }?;
        let tree = OwnedDocument::parse_with(input, options).map_err(|e| Fail::xml(e, input))?;
        let doc = DOCS.insert(Doc {
            tree,
            source: input.to_vec(),
            limits: EditLimits::DEFAULT,
        });
        // SAFETY: yaxml.h: `out` is valid for writing a handle.
        unsafe { out.put(doc) };
        Ok(OK)
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_doc_create(
    root: *const yaxml_name_spec,
    out: *mut *mut yaxml_doc,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        // SAFETY: yaxml.h: `root` points to a valid name spec.
        let name = unsafe { edit::name_spec(root) }?;
        let spec = ElementSpec { name, items: &[] };
        let tree = OwnedDocument::create(spec, EditLimits::DEFAULT).map_err(Fail::edit)?;
        let doc = DOCS.insert(Doc {
            tree,
            source: Vec::new(),
            limits: EditLimits::DEFAULT,
        });
        // SAFETY: yaxml.h: `out` is valid for writing a handle.
        unsafe { out.put(doc) };
        Ok(OK)
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_doc_clone(
    doc: *mut yaxml_doc,
    out: *mut *mut yaxml_doc,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        let copy = read(doc, |d| {
            Ok(Doc {
                tree: d.tree.try_clone().map_err(Fail::edit)?,
                source: d.source.clone(),
                limits: d.limits,
            })
        })?;
        // SAFETY: yaxml.h: `out` is valid for writing a handle.
        unsafe { out.put(DOCS.insert(copy)) };
        Ok(OK)
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_doc_free(doc: *mut yaxml_doc) -> i32 {
    guard(std::ptr::null_mut(), || DOCS.remove(doc).map(|()| OK))
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_doc_root(
    doc: *mut yaxml_doc,
    out: *mut yaxml_node,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        let root = read(doc, |d| Ok(handle(d.tree.root().id())))?;
        // SAFETY: yaxml.h: `out` is valid for writing a node.
        unsafe { out.put(root) };
        Ok(OK)
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_doc_element(
    doc: *mut yaxml_doc,
    out: *mut yaxml_node,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        let element = read(doc, |d| {
            Ok(d.tree.document_element().map(|e| handle(e.id())))
        })?;
        // SAFETY: yaxml.h: `out` is valid for writing a node.
        Ok(unsafe { out.reply(element) })
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_doc_element_by_id(
    doc: *mut yaxml_doc,
    value: yaxml_str,
    out: *mut yaxml_node,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        // SAFETY: yaxml.h: `value` is a valid string.
        let value = unsafe { text(value) }?.unwrap_or("");
        let element = read(doc, |d| {
            Ok(d.tree.element_by_id(value).map(|e| handle(e.id())))
        })?;
        // SAFETY: yaxml.h: `out` is valid for writing a node.
        Ok(unsafe { out.reply(element) })
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_doc_diagnostics(
    doc: *mut yaxml_doc,
    out: *mut yaxml_diagnostics,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        let list = read(doc, |d| {
            let lines = LineIndex::new(&d.source);
            let mut list: Vec<Diagnostic> = (lines.describe_all(d.tree.errors()).iter())
                .map(Diagnostic::from)
                .collect();
            list.extend(
                d.tree
                    .validity()
                    .iter()
                    .map(|v| (&v.describe(&lines)).into()),
            );
            Ok(list)
        })?;
        // SAFETY: yaxml.h: `out` is valid for writing diagnostics.
        unsafe { out.put(diagnostics(list)) };
        Ok(OK)
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_doc_set_edit_limits(
    doc: *mut yaxml_doc,
    limits: *const yaxml_edit_limits,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        // SAFETY: yaxml.h: `limits` is NULL or a valid limits struct.
        let limits = unsafe { edit_limits(limits) }?;
        DOCS.get(doc)?.write(|d| {
            d.limits = limits;
            Ok(OK)
        })
    })
}

// ---- nodes ----

fn kind_of(node: OwnedNode<'_>, html: bool) -> i32 {
    match node.kind() {
        OwnedNodeKind::Document if html => 13,
        OwnedNodeKind::Document => 9,
        OwnedNodeKind::DocumentType => 14,
        OwnedNodeKind::Element => 1,
        OwnedNodeKind::Text => 3,
        OwnedNodeKind::Cdata => 4,
        OwnedNodeKind::EntityRef => 5,
        OwnedNodeKind::ProcessingInstruction => 7,
        OwnedNodeKind::Comment => 8,
        _ => 0,
    }
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_node_kind(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    out: *mut i32,
    err: *mut yaxml_error,
) -> i32 {
    query(doc, node, out, err, |d, n| {
        Ok(Some(kind_of(n, d.tree.is_html())))
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_node_relative(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    relation: i32,
    out: *mut yaxml_node,
    err: *mut yaxml_error,
) -> i32 {
    query(doc, node, out, err, |_, n| {
        let related = match relation {
            1 => n.parent(),
            2 => n.first_child(),
            3 => n.last_child(),
            4 => n.next_sibling(),
            5 => n.previous_sibling(),
            _ => return Err(Fail::argument("unknown relation")),
        };
        Ok(related.map(|r| handle(r.id())))
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_node_name(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    out: *mut yaxml_name,
    err: *mut yaxml_error,
) -> i32 {
    let simple = |s: &str| yaxml_name {
        qname: yaxml_str::of(s),
        local: yaxml_str::of(s),
        ..yaxml_name::default()
    };
    query(doc, node, out, err, |_, n| {
        Ok(match n.kind() {
            OwnedNodeKind::Element => n.name().map(|name| name_of(&name, &mut yaxml_str::of)),
            OwnedNodeKind::ProcessingInstruction => n.processing_instruction_target().map(simple),
            OwnedNodeKind::EntityRef => n.entity_name().map(simple),
            _ => None,
        })
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_node_value(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    out: *mut yaxml_str,
    err: *mut yaxml_error,
) -> i32 {
    query(doc, node, out, err, |_, n| Ok(n.text().map(yaxml_str::of)))
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_node_content(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    out: *mut yaxml_buffer,
    err: *mut yaxml_error,
) -> i32 {
    query(doc, node, out, err, |_, n| {
        Ok(n.content().map(|c| buffer(c.into_bytes())))
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_node_path(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    out: *mut yaxml_buffer,
    err: *mut yaxml_error,
) -> i32 {
    query(doc, node, out, err, |_, n| {
        Ok(n.path().map(|p| buffer(p.into_bytes())))
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_node_line(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    out: *mut u64,
    err: *mut yaxml_error,
) -> i32 {
    query(doc, node, out, err, |d, n| {
        Ok(n.line(&LineIndex::new(&d.source)).map(|l| l as u64))
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_node_attribute_count(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    out: *mut usize,
    err: *mut yaxml_error,
) -> i32 {
    query(doc, node, out, err, |_, n| Ok(Some(n.attributes().len())))
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_node_attribute_at(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    index: usize,
    out: *mut yaxml_attribute,
    err: *mut yaxml_error,
) -> i32 {
    query(doc, node, out, err, |_, n| {
        let Some(attribute) = n.attributes().nth(index) else {
            return Ok(None);
        };
        Ok(Some(yaxml_attribute {
            name: name_of(&attribute.name, &mut yaxml_str::of),
            value: yaxml_str::of(pooled(attribute.value)?),
        }))
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_node_attribute(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    name: yaxml_str,
    out: *mut yaxml_str,
    err: *mut yaxml_error,
) -> i32 {
    query(doc, node, out, err, |_, n| {
        // SAFETY: yaxml.h: `name` is a valid string.
        let name = unsafe { text(name) }?.unwrap_or("");
        Ok(n.attribute(name).map(yaxml_str::of))
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_node_attribute_ns(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    local: yaxml_str,
    uri: yaxml_str,
    out: *mut yaxml_str,
    err: *mut yaxml_error,
) -> i32 {
    query(doc, node, out, err, |_, n| {
        // SAFETY: yaxml.h: `local` is a valid string.
        let local = unsafe { text(local) }?.unwrap_or("");
        // SAFETY: yaxml.h: `uri` is a valid string or NULL.
        let uri = unsafe { text(uri) }?;
        Ok(n.attribute_ns(local, uri).map(yaxml_str::of))
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_node_namespace_uri(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    prefix: yaxml_str,
    out: *mut yaxml_str,
    err: *mut yaxml_error,
) -> i32 {
    query(doc, node, out, err, |_, n| {
        // SAFETY: yaxml.h: `prefix` is a valid string or NULL.
        let prefix = unsafe { text(prefix) }?;
        Ok(n.search_ns(prefix).map(yaxml_str::of))
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_node_namespace_prefix(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    uri: yaxml_str,
    out: *mut yaxml_str,
    err: *mut yaxml_error,
) -> i32 {
    query(doc, node, out, err, |_, n| {
        // SAFETY: yaxml.h: `uri` is a valid string.
        let uri = unsafe { text(uri) }?.unwrap_or("");
        let binding = n.search_ns_by_href(uri);
        Ok(binding.map(|b| yaxml_str::opt(b.prefix)))
    })
}

// ---- XPath ----

fn item(node: &xpath::Node<OwnedNodeId>) -> yaxml_xpath_item {
    let (id, r#type, index) = match *node {
        xpath::Node::Tree(id) => (id, 0, 0),
        xpath::Node::Attribute(id, index) => (id, 2, index),
        xpath::Node::Namespace(id, index) => (id, 18, index),
    };
    yaxml_xpath_item {
        node: handle(id),
        r#type,
        index: u32::try_from(index).unwrap_or(u32::MAX),
    }
}

fn xpath_node(
    tree: &OwnedDocument,
    item: &yaxml_xpath_item,
) -> Result<xpath::Node<OwnedNodeId>, Fail> {
    let id = view(tree, item.node)?.id();
    let index = item.index as usize;
    match item.r#type {
        0 => Ok(xpath::Node::Tree(id)),
        2 => Ok(xpath::Node::Attribute(id, index)),
        18 => Ok(xpath::Node::Namespace(id, index)),
        _ => Err(Fail::argument("unknown XPath item type")),
    }
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_xpath_eval(
    doc: *mut yaxml_doc,
    context: yaxml_node,
    expression: yaxml_str,
    namespaces: *const yaxml_ns,
    namespace_count: usize,
    out: *mut yaxml_xpath_value,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        // SAFETY: yaxml.h: `expression` is a valid string.
        let expression = unsafe { text(expression) }?.unwrap_or("");
        // SAFETY: yaxml.h: `namespaces` holds `namespace_count` bindings.
        let bindings = unsafe { slice(namespaces, namespace_count) }?;
        let mut scope = Context::new();
        for binding in bindings {
            // SAFETY: yaxml.h: each binding holds valid strings.
            let prefix = unsafe { text(binding.prefix) }?.unwrap_or("");
            // SAFETY: as above.
            let uri = unsafe { text(binding.uri) }?.unwrap_or("");
            scope = scope.with_namespace(prefix, uri);
        }
        let compiled = XPath::compile(expression).map_err(|e| Fail::xpath(e, expression))?;
        let value = read(doc, |d| {
            let node = view(&d.tree, context)?.id();
            (compiled.evaluate(&d.tree, node, &scope)).map_err(|e| Fail::xpath(e, expression))
        })?;
        let mut result = yaxml_xpath_value::default();
        match value {
            Value::NodeSet(nodes) => {
                result.r#type = 1;
                (result.items, result.len) = array(nodes.iter().map(item).collect());
            }
            Value::Boolean(b) => (result.r#type, result.boolean) = (2, b.into()),
            Value::Number(n) => (result.r#type, result.number) = (3, n),
            Value::String(s) => (result.r#type, result.string) = (4, buffer(s.into_bytes())),
        }
        // SAFETY: yaxml.h: `out` is valid for writing a value.
        unsafe { out.put(result) };
        Ok(OK)
    })
}

// ---- output ----

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_save(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    flags: u32,
    encoding: yaxml_str,
    out: *mut yaxml_buffer,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        // SAFETY: yaxml.h: `encoding` is a valid string or NULL.
        let encoding = unsafe { text(encoding) }?;
        let options = save::Options::new(flags).encoding(encoding);
        let bytes = read(doc, |d| {
            save::to_vec(view(&d.tree, node)?, &options).map_err(Fail::kind)
        })?;
        // SAFETY: yaxml.h: `out` is valid for writing a buffer.
        unsafe { out.put(buffer(bytes)) };
        Ok(OK)
    })
}

/// An `io::Write` over a C write callback.
struct Sink {
    write: WriteFn,
    user: *mut c_void,
}

impl io::Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // SAFETY: yaxml.h: `write` is a yaxml_write_fn accepting `user`.
        match unsafe { ffi::write(self.write, self.user, bytes) } {
            0 => Ok(bytes.len()),
            _ => Err(io::Error::other("write callback failed")),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_save_to(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    flags: u32,
    encoding: yaxml_str,
    write: Option<WriteFn>,
    user: *mut c_void,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let write = write.ok_or_else(|| Fail::null("NULL write callback"))?;
        // SAFETY: yaxml.h: `encoding` is a valid string or NULL.
        let encoding = unsafe { text(encoding) }?;
        let options = save::Options::new(flags).encoding(encoding);
        read(doc, |d| {
            let sink = Sink { write, user };
            save::to_writer(view(&d.tree, node)?, &options, sink).map_err(Fail::write)
        })?;
        Ok(OK)
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_c14n(
    doc: *mut yaxml_doc,
    mode: i32,
    with_comments: i32,
    nodes: *const yaxml_xpath_item,
    node_count: usize,
    inclusive_prefixes: *const yaxml_str,
    prefix_count: usize,
    out: *mut yaxml_buffer,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        let mode = match mode {
            0 => c14n::Mode::C14n10,
            1 => c14n::Mode::Exclusive10,
            2 => c14n::Mode::C14n11,
            _ => return Err(Fail::argument("unknown C14N mode")),
        };
        let items = match nodes.is_null() {
            true => None,
            // SAFETY: yaxml.h: `nodes` holds `node_count` items.
            false => Some(unsafe { slice(nodes, node_count) }?),
        };
        // SAFETY: yaxml.h: `inclusive_prefixes` holds `prefix_count` strings.
        let raw = unsafe { slice(inclusive_prefixes, prefix_count) }?;
        let prefixes = (raw.iter())
            // SAFETY: yaxml.h: each prefix is a valid string.
            .map(|&p| unsafe { text(p) }.map(Option::unwrap_or_default))
            .collect::<Result<Vec<&str>, Fail>>()?;
        let canonical = read(doc, |d| {
            let set = (items.iter().flat_map(|items| items.iter()))
                .map(|i| xpath_node(&d.tree, i))
                .collect::<Result<Vec<_>, Fail>>()?;
            let options = c14n::Options::new(mode)
                .with_comments(with_comments != 0)
                .nodes(items.map(|_| set.as_slice()))
                .inclusive_prefixes(&prefixes);
            c14n::to_string(&d.tree, &options).map_err(Fail::c14n)
        })?;
        // SAFETY: yaxml.h: `out` is valid for writing a buffer.
        unsafe { out.put(buffer(canonical.into_bytes())) };
        Ok(OK)
    })
}

// ---- validation ----

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_validate_dtd(
    doc: *mut yaxml_doc,
    out: *mut yaxml_diagnostics,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        let list = read(doc, |d| {
            let lines = LineIndex::new(&d.source);
            Ok(d.tree
                .validate()
                .iter()
                .map(|v| (&v.describe(&lines)).into())
                .collect())
        })?;
        // SAFETY: yaxml.h: `out` is valid for writing diagnostics.
        unsafe { out.put(diagnostics(list)) };
        Ok(OK)
    })
}

fn compile(kind: i32, input: &[u8], base: &str) -> Result<Schema, Fail> {
    let refuse = |_: yaxml::Request<'_>| Ok(None);
    match kind {
        1 => xsd::Schema::parse(input, base)
            .map(Schema::Xsd)
            .map_err(|e| {
                let code = (e.diagnostics.iter()).find(|d| !d.code.is_warning());
                Fail::with(
                    ERR_SCHEMA,
                    16,
                    code.map_or(0, |d| d.code as i32),
                    e.to_string(),
                )
            }),
        2 => relaxng::Schema::parse(input, base, Box::new(refuse), &relaxng::Limits::DEFAULT)
            .map(Schema::RelaxNg)
            .map_err(|e| {
                let code = e.0.first().map_or(0, |d| d.code as i32);
                Fail::with(ERR_SCHEMA, 18, code, e.to_string())
            }),
        3 => schematron::Schema::parse(input, base, Box::new(refuse), &schematron::Limits::DEFAULT)
            .map(Schema::Schematron)
            .map_err(|e| {
                let first = e.errors.first().map(|d| Diagnostic::from(&d.describe()));
                let (domain, code) = first.map_or((0, 0), |d| (d.domain, d.code));
                Fail::with(ERR_SCHEMA, domain, code, e.to_string())
            }),
        _ => Err(Fail::argument("unknown schema kind")),
    }
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_schema_new(
    kind: i32,
    bytes: *const u8,
    len: usize,
    base: yaxml_str,
    out: *mut *mut yaxml_schema,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        // SAFETY: yaxml.h: `bytes` holds `len` bytes for the call.
        let input = unsafe { slice(bytes, len) }?;
        // SAFETY: yaxml.h: `base` is a valid string or NULL.
        let base = unsafe { text(base) }?.unwrap_or("");
        let schema = compile(kind, input, base)?;
        // SAFETY: yaxml.h: `out` is valid for writing a handle.
        unsafe { out.put(SCHEMAS.insert(schema)) };
        Ok(OK)
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_schema_free(schema: *mut yaxml_schema) -> i32 {
    guard(std::ptr::null_mut(), || SCHEMAS.remove(schema).map(|()| OK))
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_validate(
    doc: *mut yaxml_doc,
    schema: *mut yaxml_schema,
    out: *mut yaxml_diagnostics,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let out = Out::new(out)?;
        let list = SCHEMAS.get(schema)?.read(|schema| {
            read(doc, |d| {
                let (tree, lines) = (&d.tree, LineIndex::new(&d.source));
                Ok(match schema {
                    Schema::Xsd(s) => s
                        .validate(tree)
                        .iter()
                        .map(|v| (&v.describe(&lines)).into())
                        .collect(),
                    Schema::RelaxNg(s) => s
                        .validate(tree)
                        .iter()
                        .map(|v| (&v.describe(&lines)).into())
                        .collect(),
                    Schema::Schematron(s) => {
                        let run = s.validate(tree, schematron::OUT_ERROR, &d.source);
                        let mut list: Vec<Diagnostic> = (run.output.iter())
                            .filter_map(|o| match o {
                                schematron::Output::Error(e) => Some((&e.describe()).into()),
                                _ => None,
                            })
                            .collect();
                        // xmlSchematronValidateDoc's -1: no rules, or the budget ran out.
                        if run.errors.is_none() && !list.iter().any(Diagnostic::is_error) {
                            list.push(
                                Fail::with(0, 17, 1, "Schematron validation failed").diagnostic,
                            );
                        }
                        list
                    }
                })
            })
        })?;
        // SAFETY: yaxml.h: `out` is valid for writing diagnostics.
        unsafe { out.put(diagnostics(list)) };
        Ok(OK)
    })
}
