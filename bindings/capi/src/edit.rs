//! Editing: each call is one atomic `Editor` operation under the document's
//! exclusive lock, with the document's `EditLimits`.

use yaxml::{EditError, Editor, ElementSpec, NameSpec, PrefixPolicy};

use crate::doc::{DOCS, handle, id, read};
use crate::error::{Fail, guard};
use crate::ffi::{Out, get, text};
use crate::types::*;
use crate::*;

/// Reads one `yaxml_name_spec` argument.
///
/// # Safety
/// `spec` points to a valid name spec per yaxml.h.
pub(crate) unsafe fn name_spec<'a>(spec: *const yaxml_name_spec) -> Result<NameSpec<'a>, Fail> {
    // SAFETY: forwarded contract.
    let spec = unsafe { get(spec) }?;
    // SAFETY: forwarded contract: the spec's strings are valid.
    let local = unsafe { text(spec.local) }?.unwrap_or("");
    // SAFETY: as above.
    let namespace = unsafe { text(spec.uri) }?;
    // SAFETY: as above.
    let prefix = unsafe { text(spec.prefix) }?;
    Ok(NameSpec {
        local,
        namespace,
        prefix: match spec.automatic {
            0 => PrefixPolicy::Exact(prefix),
            _ => PrefixPolicy::Automatic,
        },
    })
}

/// Runs one editor operation on `doc`.
fn edit<R>(
    doc: *mut yaxml_doc,
    f: impl FnOnce(&mut Editor<'_>) -> Result<R, EditError>,
) -> Result<R, Fail> {
    DOCS.get(doc)?.write(|d| {
        let limits = d.limits;
        let mut editor = d.tree.edit(limits).map_err(Fail::edit)?;
        f(&mut editor).map_err(Fail::edit)
    })
}

/// An edit that returns a node, written to `out` (required unless `optional`).
fn edit_node(
    out: *mut yaxml_node,
    optional: bool,
    err: *mut yaxml_error,
    f: impl FnOnce() -> Result<yaxml::OwnedNodeId, Fail>,
) -> i32 {
    guard(err, || {
        let out = match optional {
            true => Out::optional(out)?,
            false => Some(Out::new(out)?),
        };
        let node = handle(f()?);
        if let Some(out) = out {
            // SAFETY: yaxml.h: `out` is NULL (optional only) or valid for writing a node.
            unsafe { out.put(node) };
        }
        Ok(OK)
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_append_element(
    doc: *mut yaxml_doc,
    parent: yaxml_node,
    name: *const yaxml_name_spec,
    out: *mut yaxml_node,
    err: *mut yaxml_error,
) -> i32 {
    edit_node(out, false, err, || {
        // SAFETY: yaxml.h: `name` points to a valid name spec.
        let name = unsafe { name_spec(name) }?;
        let parent = id(parent)?;
        edit(doc, |e| {
            e.append_element(parent, ElementSpec { name, items: &[] })
        })
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_append_leaf(
    doc: *mut yaxml_doc,
    parent: yaxml_node,
    kind: i32,
    target: yaxml_str,
    value: yaxml_str,
    out: *mut yaxml_node,
    err: *mut yaxml_error,
) -> i32 {
    edit_node(out, false, err, || {
        // SAFETY: yaxml.h: `target` is a valid string or NULL.
        let target = unsafe { text(target) }?;
        // SAFETY: yaxml.h: `value` is a valid string or NULL.
        let value = unsafe { text(value) }?;
        let (parent, value, target) = (id(parent)?, value.unwrap_or(""), target.unwrap_or(""));
        if ![3, 4, 7, 8].contains(&kind) {
            return Err(Fail::argument("unknown leaf kind"));
        }
        edit(doc, |e| match kind {
            3 => e.append_text(parent, value),
            4 => e.append_cdata(parent, value),
            7 => e.append_processing_instruction(parent, target, value),
            _ => e.append_comment(parent, value),
        })
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_set_value(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    value: yaxml_str,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        // SAFETY: yaxml.h: `value` is a valid string or NULL.
        let value = unsafe { text(value) }?;
        let (node, value) = (id(node)?, value.unwrap_or(""));
        edit(doc, |e| e.set_node_value(node, value)).map(|()| OK)
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_set_attribute(
    doc: *mut yaxml_doc,
    element: yaxml_node,
    name: *const yaxml_name_spec,
    value: yaxml_str,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        // SAFETY: yaxml.h: `name` points to a valid name spec.
        let name = unsafe { name_spec(name) }?;
        // SAFETY: yaxml.h: `value` is a valid string or NULL.
        let value = unsafe { text(value) }?;
        let (element, value) = (id(element)?, value.unwrap_or(""));
        edit(doc, |e| e.set_attribute(element, name, value)).map(|()| OK)
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_remove_attribute(
    doc: *mut yaxml_doc,
    element: yaxml_node,
    uri: yaxml_str,
    local: yaxml_str,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        // SAFETY: yaxml.h: `uri` is a valid string or NULL.
        let uri = unsafe { text(uri) }?;
        // SAFETY: yaxml.h: `local` is a valid string.
        let local = unsafe { text(local) }?;
        let (element, local) = (id(element)?, local.unwrap_or(""));
        match edit(doc, |e| e.remove_attribute(element, uri, local)) {
            Err(fail) if fail.diagnostic.code == 4 && fail.status == ERR_EDIT => Ok(NONE),
            result => result.map(|()| OK),
        }
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_declare_namespace(
    doc: *mut yaxml_doc,
    element: yaxml_node,
    prefix: yaxml_str,
    uri: yaxml_str,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        // SAFETY: yaxml.h: `prefix` is a valid string or NULL.
        let prefix = unsafe { text(prefix) }?;
        // SAFETY: yaxml.h: `uri` is a valid string.
        let uri = unsafe { text(uri) }?;
        let (element, uri) = (id(element)?, uri.unwrap_or(""));
        edit(doc, |e| e.declare_namespace(element, prefix, uri)).map(|()| OK)
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_set_name(
    doc: *mut yaxml_doc,
    element: yaxml_node,
    name: *const yaxml_name_spec,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        // SAFETY: yaxml.h: `name` points to a valid name spec.
        let name = unsafe { name_spec(name) }?;
        let element = id(element)?;
        edit(doc, |e| e.set_name(element, name)).map(|()| OK)
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_set_url(
    doc: *mut yaxml_doc,
    url: yaxml_str,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        // SAFETY: yaxml.h: `url` is a valid string or NULL.
        let url = unsafe { text(url) }?;
        edit(doc, |e| e.set_url(url)).map(|()| OK)
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_move(
    doc: *mut yaxml_doc,
    operation: i32,
    target: yaxml_node,
    node: yaxml_node,
    out: *mut yaxml_node,
    err: *mut yaxml_error,
) -> i32 {
    edit_node(out, true, err, || {
        let (target, node) = (id(target)?, id(node)?);
        if !(1..=4).contains(&operation) {
            return Err(Fail::argument("unknown move operation"));
        }
        edit(doc, |e| match operation {
            1 => e.append_child(target, node),
            2 => e.insert_before(target, node),
            3 => e.insert_after(target, node),
            _ => e.replace(target, node).map(|()| node),
        })
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_unlink(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let node = id(node)?;
        edit(doc, |e| e.unlink(node)).map(|()| OK)
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_remove(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    err: *mut yaxml_error,
) -> i32 {
    guard(err, || {
        let node = id(node)?;
        edit(doc, |e| e.remove_subtree(node)).map(|()| OK)
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_copy(
    doc: *mut yaxml_doc,
    node: yaxml_node,
    deep: i32,
    out: *mut yaxml_node,
    err: *mut yaxml_error,
) -> i32 {
    edit_node(out, false, err, || {
        let node = id(node)?;
        edit(doc, |e| e.copy(node, deep != 0))
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_import(
    doc: *mut yaxml_doc,
    source: *mut yaxml_doc,
    node: yaxml_node,
    deep: i32,
    out: *mut yaxml_node,
    err: *mut yaxml_error,
) -> i32 {
    edit_node(out, false, err, || {
        let node = id(node)?;
        read(source, |s| {
            edit(doc, |e| e.import(&s.tree, node, deep != 0))
        })
    })
}

/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_compact(doc: *mut yaxml_doc, err: *mut yaxml_error) -> i32 {
    guard(err, || edit(doc, |e| e.compact()).map(|()| OK))
}
