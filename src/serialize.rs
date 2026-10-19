use std::io::Write;

use crate::save::Tree;
use crate::{Attributes, Declaration, Document, Element, Event, OwnedDocument, WriteError, Writer};

pub(crate) use crate::save::owned_leaf;

pub(crate) fn borrowed<W: Write>(
    writer: &mut Writer<W>,
    document: &Document<'_>,
    max_attributes: usize,
) -> Result<(), WriteError> {
    write(
        writer,
        Tree::Borrowed(document.root()),
        document.declaration().copied(),
        max_attributes,
    )
}

pub(crate) fn owned<W: Write>(
    writer: &mut Writer<W>,
    document: &OwnedDocument,
    max_attributes: usize,
) -> Result<(), WriteError> {
    write(
        writer,
        Tree::Owned(document.root()),
        document.declaration(),
        max_attributes,
    )
}

fn write<W: Write>(
    writer: &mut Writer<W>,
    root: Tree<'_, '_>,
    declaration: Option<Declaration<'_>>,
    max_attributes: usize,
) -> Result<(), WriteError> {
    if let Some(mut declaration) = declaration {
        declaration.encoding = declaration.encoding.map(|_| "UTF-8");
        writer.write_event_inner(&Event::Declaration(declaration))?;
    }
    let mut attributes = Vec::new();
    let mut current = root.first_child();
    while let Some(mut node) = current {
        let child = node.first_child();
        if let Some(name) = node.name() {
            node.attributes(&mut attributes, max_attributes, writer.position())
                .map_err(WriteError::Xml)?;
            // The attribute metadata vector is reused for every element.
            let element = Element {
                name,
                attributes: Attributes::Borrowed(&attributes),
            };
            let event = if child.is_some() {
                Event::Start(element)
            } else {
                Event::Empty(element)
            };
            writer.write_event_inner(&event)?;
        } else if let Some(event) = node.leaf() {
            writer.write_event_inner(&event)?;
        }
        if child.is_some() {
            current = child;
            continue;
        }
        loop {
            if let Some(sibling) = node.next_sibling() {
                current = Some(sibling);
                break;
            }
            let Some(parent) = node.parent() else {
                return Ok(());
            };
            let Some(name) = parent.name() else {
                return Ok(());
            };
            writer.write_event_inner(&Event::End(name))?;
            node = parent;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/internal/serialize.rs"]
mod tests;
