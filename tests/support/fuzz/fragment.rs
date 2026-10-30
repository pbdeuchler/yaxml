//! Content parsed in a node's context (`Editor::parse_content`), then
//! inserted. A failure (other than allocation) leaves the document as it
//! was and describes without panicking; under `RECOVER` only limits and
//! entity loops fail (docs/compatibility.md#recovery-recover).
//! The result is detached; appended under the node it was parsed in (or
//! else the document element, which may need declarations), the document
//! stays namespace-consistent and saves to a tree that reparses the same
//! (`domedit::check`).
//!
//! Input: context XML bytes, a 0x00 separator, four control bytes (parse
//! options, context node, edit limits, where to insert), then the content.

use yaxml::options::{DTDATTR, DTDLOAD, DTDVALID, NOBLANKS, NOCDATA, NOENT, NSCLEAN, RECOVER};
use yaxml::{
    EditErrorKind, EditLimits, Error, LineIndex, OwnedDocument, OwnedNode, OwnedNodeId,
    OwnedNodeKind, ParseOptions,
};

use super::domedit::{check, snapshot};

const OPTIONS: [u32; 8] = [
    0,
    RECOVER,
    NOENT,
    NOENT | RECOVER,
    NOBLANKS | NSCLEAN,
    NOCDATA | DTDATTR | DTDLOAD,
    RECOVER | NOBLANKS,
    DTDVALID,
];

fn limits(control: u8) -> EditLimits {
    match control % 4 {
        0 | 1 => EditLimits::default(),
        2 => EditLimits::default()
            .max_created_nodes(usize::from(control) / 4)
            .max_depth(usize::from(control) / 32 + 1),
        _ => EditLimits::default()
            .max_owned_bytes(usize::from(control) * 4)
            .max_namespace_work(usize::from(control) / 8),
    }
}

/// The element libxml2 parses in: the nearest at or above `node` along
/// its parent chain, which stops at entity content.
fn element(node: OwnedNode<'_>) -> Option<OwnedNode<'_>> {
    std::iter::successors(Some(node), |node| {
        node.parent()
            .filter(|parent| parent.kind() != OwnedNodeKind::EntityRef)
    })
    .find(|node| node.kind() == OwnedNodeKind::Element)
}

fn in_entity(node: OwnedNode<'_>) -> bool {
    std::iter::successors(node.parent(), |node| node.parent())
        .any(|node| node.kind() == OwnedNodeKind::EntityRef)
}

pub fn exercise(input: &[u8]) {
    let Some(split) = input.iter().position(|&byte| byte == 0) else {
        return;
    };
    let (xml, rest) = (&input[..split], &input[split + 1..]);
    let control = |at: usize| rest.get(at).copied().unwrap_or(0);
    let content = rest.get(4..).unwrap_or_default();
    let Ok(mut document) = OwnedDocument::parse_bytes(xml) else {
        return;
    };
    let options = OPTIONS[usize::from(control(0)) % OPTIONS.len()];
    let nodes: Vec<OwnedNodeId> = document.nodes().map(|node| node.id()).collect();
    let context = nodes[usize::from(control(1)) % nodes.len()];
    let limits = limits(control(2));
    let target = document.node(context).and_then(element);
    let target = match target.filter(|element| !in_entity(*element)) {
        Some(element) if control(3) % 4 != 0 => Some(element.id()),
        _ => document.document_element().map(|element| element.id()),
    };
    let before = snapshot(&document);
    let mut editor = document.edit(limits).expect("a fresh document");
    let parsed = editor.parse_content(context, content, ParseOptions::new(options));
    let fragment = match parsed {
        Ok(fragment) => fragment,
        Err(error) => {
            assert_ne!(error.kind, EditErrorKind::Allocation);
            if let EditErrorKind::Parse(kind) = error.kind {
                let offset = error.value_offset.expect("a parse error has an offset");
                assert!(offset <= content.len(), "{error:?} past the content");
                Error { kind, offset }.describe(&LineIndex::content(content));
                let fatal = matches!(
                    kind,
                    yaxml::ErrorKind::LimitExceeded(_)
                        | yaxml::ErrorKind::UnsupportedOption
                        | yaxml::ErrorKind::EntityLoop
                );
                assert!(options & RECOVER == 0 || fatal, "RECOVER failed: {error:?}");
            }
            assert_eq!(
                snapshot(&document),
                before,
                "a failed parse changed the tree"
            );
            return;
        }
    };
    // Recovered offsets count the text after invalid UTF-8 is repaired, as
    // for a document; describing clamps them.
    let index = LineIndex::content(content);
    for error in &fragment.errors {
        error.describe(&index);
    }
    let namespace = |error: &Error| {
        matches!(
            error.kind,
            yaxml::ErrorKind::Namespace | yaxml::ErrorKind::Warning(_)
        )
    };
    assert!(options & RECOVER != 0 || fragment.errors.iter().all(namespace));
    for &node in &fragment.nodes {
        let node = editor.document().node(node).expect("parsed nodes are live");
        assert!(node.parent().is_none() && node.next_sibling().is_none());
    }
    // Recovered content (an undeclared reference) need not reparse: it is
    // dropped, which leaves the tree as it was.
    let target = target.filter(|_| fragment.errors.is_empty());
    let mut handles = nodes;
    for &node in &fragment.nodes {
        let placed = target.map(|parent| editor.append_child(parent, node));
        match placed {
            Some(Ok(kept)) => handles.push(kept),
            Some(Err(error)) => {
                assert_ne!(error.kind, EditErrorKind::Allocation);
                editor
                    .remove_subtree(node)
                    .expect("a detached node is removable");
            }
            None => editor
                .remove_subtree(node)
                .expect("a detached node is removable"),
        }
    }
    if target.is_none() {
        assert_eq!(
            snapshot(&document),
            before,
            "detached nodes changed the tree"
        );
    }
    check(&document, &handles);
}
