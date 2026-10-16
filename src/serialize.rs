use std::borrow::Cow;
use std::io::Write;

use crate::{
    Attribute, Declaration, Document, Element, Error, ErrorKind, Event, Limit, Name, Node,
    NodeKind, OwnedDocument, OwnedNode, OwnedNodeKind, WriteError, Writer,
};

// A concrete bridge between the two immutable storage forms. Every value view
// borrows its document, including strings that the borrowing DOM normalized.
#[derive(Clone, Copy)]
enum TreeNode<'document, 'input> {
    Borrowed(Node<'document, 'input>),
    Owned(OwnedNode<'document>),
}

impl<'document, 'input> TreeNode<'document, 'input> {
    fn first_child(self) -> Option<Self> {
        match self {
            Self::Borrowed(node) => node.first_child().map(Self::Borrowed),
            Self::Owned(node) => node.first_child().map(Self::Owned),
        }
    }

    fn next_sibling(self) -> Option<Self> {
        match self {
            Self::Borrowed(node) => node.next_sibling().map(Self::Borrowed),
            Self::Owned(node) => node.next_sibling().map(Self::Owned),
        }
    }

    fn parent(self) -> Option<Self> {
        match self {
            Self::Borrowed(node) => node.parent().map(Self::Borrowed),
            Self::Owned(node) => node.parent().map(Self::Owned),
        }
    }

    fn name(self) -> Option<Name<'document>> {
        match self {
            Self::Borrowed(node) => node.name().map(Name::as_borrowed),
            Self::Owned(node) => node.name(),
        }
    }

    fn leaf(self) -> Option<Event<'document>> {
        match self {
            Self::Borrowed(node) => match node.kind() {
                NodeKind::Text(value) => Some(Event::Text(Cow::Borrowed(value))),
                NodeKind::Cdata(value) => Some(Event::Cdata(Cow::Borrowed(value))),
                NodeKind::Comment(value) => Some(Event::Comment(Cow::Borrowed(value))),
                NodeKind::ProcessingInstruction { target, data } => {
                    Some(Event::ProcessingInstruction {
                        target: Cow::Borrowed(target),
                        data: Cow::Borrowed(data),
                    })
                }
                NodeKind::Document | NodeKind::Element(_) => None,
            },
            Self::Owned(node) => match node.kind() {
                OwnedNodeKind::Text => Some(Event::Text(Cow::Borrowed(node.text()?))),
                OwnedNodeKind::Cdata => Some(Event::Cdata(Cow::Borrowed(node.text()?))),
                OwnedNodeKind::Comment => Some(Event::Comment(Cow::Borrowed(node.text()?))),
                OwnedNodeKind::ProcessingInstruction => Some(Event::ProcessingInstruction {
                    target: Cow::Borrowed(node.processing_instruction_target()?),
                    data: Cow::Borrowed(node.text()?),
                }),
                OwnedNodeKind::Document | OwnedNodeKind::Element => None,
            },
        }
    }

    fn attributes(
        self,
        attributes: &mut Vec<Attribute<'document>>,
        limit: usize,
        offset: usize,
    ) -> Result<(), Error> {
        attributes.clear();
        let count = match self {
            Self::Borrowed(node) => node.attributes().len(),
            Self::Owned(node) => node.attributes().len(),
        };
        if count > limit {
            return Err(Error::new(
                ErrorKind::LimitExceeded(Limit::Attributes),
                offset,
            ));
        }
        attributes
            .try_reserve_exact(count)
            .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
        match self {
            Self::Borrowed(node) => {
                attributes.extend(node.attributes().iter().map(|attribute| Attribute {
                    name: attribute.name.as_borrowed(),
                    value: Cow::Borrowed(&attribute.value),
                }))
            }
            Self::Owned(node) => attributes.extend(node.attributes()),
        }
        Ok(())
    }
}

pub(crate) fn borrowed<W: Write>(
    writer: &mut Writer<W>,
    document: &Document<'_>,
    max_attributes: usize,
) -> Result<(), WriteError> {
    write(
        writer,
        TreeNode::Borrowed(document.root()),
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
        TreeNode::Owned(document.root()),
        document.declaration(),
        max_attributes,
    )
}

fn write<W: Write>(
    writer: &mut Writer<W>,
    root: TreeNode<'_, '_>,
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
            let element = Element { name, attributes };
            let event = if child.is_some() {
                Event::Start(element)
            } else {
                Event::Empty(element)
            };
            writer.write_event_inner(&event)?;
            // Reuse only metadata; none of its strings have been copied.
            attributes = match event {
                Event::Start(element) | Event::Empty(element) => element.attributes,
                _ => unreachable!(),
            };
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
