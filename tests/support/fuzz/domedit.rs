//! Random edit scripts over two parses of one document: moves, unlink,
//! replace, removal, creation, attribute membership, names, namespace
//! declarations, compaction, imports from the other document, copies within
//! one, whole-document clones and swapping which document is edited. After
//! every operation both trees are checked: links are consistent, every name
//! resolves to its stored namespace, a failed edit changed neither, removed
//! handles stay rejected, and saving then reparsing gives the same tree. A
//! copy resolves every name on its own and, when deep, saves as its source
//! does; a clone saves as its original.
//!
//! Input: XML bytes, a 0x00 separator, then 4-byte operations
//! `[kind, a, b, c]`; `a` and `b` select handles from every node seen so far
//! (removed ones included; an import's `b` selects from the other document),
//! `c` selects names, values and whether a copy is deep. The first script
//! byte chooses default or tight `EditLimits`.

use yaxml::{
    EditError, EditErrorKind, EditLimits, ElementSpec, NameSpec, OwnedDocument, OwnedNode,
    OwnedNodeId, OwnedNodeKind, PrefixPolicy,
};

const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
const URIS: [Option<&str>; 4] = [None, Some("urn:p"), Some("urn:q"), Some("")];
const PREFIXES: [PrefixPolicy<'static>; 5] = [
    PrefixPolicy::Automatic,
    PrefixPolicy::Exact(None),
    PrefixPolicy::Exact(Some("p")),
    PrefixPolicy::Exact(Some("q")),
    PrefixPolicy::Exact(Some("xml")),
];

fn name(selector: u8) -> NameSpec<'static> {
    let selector = usize::from(selector);
    NameSpec {
        local: ["a", "b", "id", "lang"][selector % 4],
        namespace: [None, Some("urn:p"), Some("urn:q"), Some(XML)][selector / 4 % 4],
        prefix: PREFIXES[selector / 16 % PREFIXES.len()],
    }
}

const XML: &str = "http://www.w3.org/XML/1998/namespace";

fn limits(control: u8) -> EditLimits {
    match control % 4 {
        0 => EditLimits::default(),
        1 => EditLimits::default().max_namespace_work(usize::from(control) / 4),
        2 => EditLimits::default()
            .max_depth(usize::from(control) / 16 + 1)
            .max_created_attributes(usize::from(control) / 4),
        _ => EditLimits::default()
            .max_owned_bytes(usize::from(control) * 8)
            .max_value_bytes(usize::from(control) / 8),
    }
}

/// Everything a failed edit must leave unchanged.
fn snapshot(document: &OwnedDocument) -> (Vec<u8>, Vec<(OwnedNodeId, Option<String>)>) {
    let saved = yaxml::save::to_vec(document, &yaxml::save::Options::default())
        .expect("a well-formed tree saves");
    let nodes = document
        .nodes()
        .map(|node| (node.id(), node.path()))
        .collect();
    (saved, nodes)
}

pub fn exercise(input: &[u8]) {
    let Some(split) = input.iter().position(|&byte| byte == 0) else {
        return;
    };
    let (xml, script) = (&input[..split], &input[split + 1..]);
    let (Ok(mut document), Ok(mut other)) = (
        OwnedDocument::parse_bytes(xml),
        OwnedDocument::parse_bytes(xml),
    ) else {
        return;
    };
    let limits = limits(script.first().copied().unwrap_or(0));
    let foreign = OwnedDocument::parse("<f/>").unwrap().root_element().id();
    let mut handles: Vec<OwnedNodeId> = document.nodes().map(|node| node.id()).collect();
    let mut others: Vec<OwnedNodeId> = other.nodes().map(|node| node.id()).collect();
    for operation in script
        .get(1..)
        .unwrap_or_default()
        .as_chunks::<4>()
        .0
        .iter()
        .take(64)
    {
        let pick = |selector: u8| handles[usize::from(selector) % handles.len()];
        let (a, b, c) = (pick(operation[1]), pick(operation[2]), operation[3]);
        let (kind, deep) = (operation[0] % 19, c % 2 == 0);
        let source = others[usize::from(operation[2]) % others.len()];
        if kind == 18 {
            std::mem::swap(&mut document, &mut other);
            std::mem::swap(&mut handles, &mut others);
            continue;
        }
        let before = (snapshot(&document), snapshot(&other));
        let mut editor = document.edit(limits).expect("limits only grow");
        let result: Result<Option<OwnedNodeId>, EditError> = match kind {
            0 => editor.append_child(a, b).map(Some),
            1 => editor.insert_before(a, b).map(Some),
            2 => editor.insert_after(a, b).map(Some),
            3 => editor.unlink(a).map(|()| None),
            4 => editor.replace(a, b).map(|()| None),
            // Removing the DOCTYPE can orphan entity references, which then
            // cannot reparse; that removal predates these edits.
            5 if editor.document().node(a).map(OwnedNode::kind)
                == Some(OwnedNodeKind::DocumentType) =>
            {
                Ok(None)
            }
            5 => editor.remove_subtree(a).map(|()| None),
            6 => editor
                .append_text(a, ["x", " ", "", "&<"][usize::from(c) % 4])
                .map(Some),
            7 => {
                let element = ElementSpec {
                    name: name(c),
                    items: &[],
                };
                editor.append_element(a, element).map(Some)
            }
            8 => editor.set_attribute(a, name(c), "v").map(|()| None),
            9 => {
                let NameSpec {
                    local, namespace, ..
                } = name(c);
                let (namespace, local) = match c % 3 {
                    0 => (Some(XMLNS), ["p", "q", "xmlns"][usize::from(c) / 3 % 3]),
                    _ => (namespace, local),
                };
                editor.remove_attribute(a, namespace, local).map(|()| None)
            }
            10 => {
                let prefix = [None, Some("p"), Some("q"), Some("xml")][usize::from(c) % 4];
                let uri = URIS[usize::from(c) / 4 % 4].unwrap_or(XML);
                editor.declare_namespace(a, prefix, uri).map(|()| None)
            }
            11 => editor.set_name(a, name(c)).map(|()| None),
            12 => editor.compact().map(|()| None),
            13 => editor.append_comment(a, "k").map(Some),
            14 => editor.unlink(foreign).map(|()| None),
            15 => editor.import(&other, source, deep).map(Some),
            16 => editor.copy(a, deep).map(Some),
            _ => {
                let url = [None, Some("file:///d/x.xml"), Some("y.xml")][usize::from(c) % 3];
                if editor.set_url(url).is_ok() {
                    assert_eq!(editor.document().url(), url);
                }
                let clone = editor.document().try_clone().expect("cloning succeeds");
                assert_eq!(clone.url(), editor.document().url());
                assert_eq!(
                    snapshot(&clone).0,
                    before.0.0,
                    "a clone saves as its original"
                );
                assert_eq!(shape(&clone), shape(editor.document()));
                Ok(None)
            }
        };
        if std::env::var_os("YAXML_DOMEDIT_TRACE").is_some() {
            eprintln!("{operation:?} {a:?} {b:?} -> {result:?}");
        }
        match result {
            Ok(Some(copy)) if matches!(kind, 15 | 16) => {
                let original = match kind {
                    15 => other.node(source),
                    _ => document.node(a),
                };
                copied(
                    &document,
                    copy,
                    original.expect("copied from a live node"),
                    deep,
                );
                handles.push(copy);
            }
            Ok(created) => handles.extend(created),
            Err(error) => {
                assert_ne!(error.kind, EditErrorKind::Allocation);
                let after = (snapshot(&document), snapshot(&other));
                assert_eq!(after, before, "failed edit {error:?} changed a tree");
            }
        }
        let stale: Vec<_> = handles
            .iter()
            .copied()
            .filter(|&handle| document.node(handle).is_none())
            .collect();
        for stale in stale {
            let error = document.edit(limits).unwrap().unlink(stale).unwrap_err();
            assert_eq!(error.kind, EditErrorKind::InvalidHandle);
        }
        check(&document, &handles);
        check(&other, &others);
    }
}

/// A fresh copy is detached, has no source range, resolves every name
/// within itself and, deep and without expanded references, saves as its
/// source subtree does (`save` declares a subtree's missing namespaces on
/// its root, where a copy declares them).
fn copied(document: &OwnedDocument, copy: OwnedNodeId, original: OwnedNode<'_>, deep: bool) {
    let node = document.node(copy).expect("a copy is live");
    assert!(node.parent().is_none() && node.range().is_none());
    for element in node.descendants().filter(|node| !in_entity(*node)) {
        let Some(name) = element.name() else {
            continue;
        };
        let bound = element
            .search_ns(name.prefix())
            .filter(|uri| !uri.is_empty());
        assert_eq!(bound, name.namespace(), "copied {:?}", name.qualified());
    }
    let expanded = original.descendants().len() != node.descendants().len();
    if deep && !expanded {
        let options = yaxml::save::Options::default();
        let ours = yaxml::save::to_vec(node, &options).ok();
        let theirs = yaxml::save::to_vec(original, &options).ok();
        assert_eq!(ours, theirs, "a copy saves as its source");
    }
}

fn check(document: &OwnedDocument, handles: &[OwnedNodeId]) {
    // Links: every live handle is its parent's child, siblings agree, and
    // subtree sizes match a walk.
    let root = document.root();
    assert_eq!(root.descendants().len(), walk(root));
    for &handle in handles {
        let Some(node) = document.node(handle) else {
            continue;
        };
        if let Some(parent) = node.parent() {
            assert!(parent.children().any(|child| child.id() == handle));
        }
        if let Some(next) = node.next_sibling() {
            assert_eq!(next.previous_sibling().map(|n| n.id()), Some(handle));
        }
        let children: Vec<_> = node.children().map(|child| child.id()).collect();
        assert_eq!(
            node.last_child().map(|child| child.id()),
            children.last().copied()
        );
        assert_eq!(node.descendants().len(), walk(node));
    }
    // Every attached name resolves where it stands.
    for node in document.nodes().filter(|node| !in_entity(*node)) {
        let Some(element) = node.name() else {
            continue;
        };
        let namespace = node
            .search_ns(element.prefix())
            .filter(|uri| !uri.is_empty());
        assert_eq!(
            namespace,
            element.namespace(),
            "element {:?}",
            element.qualified()
        );
        for attribute in node.attributes() {
            let name = &attribute.name;
            if name.namespace() == Some(XMLNS) || name.prefix().is_none() {
                continue;
            }
            assert_eq!(
                node.search_ns(name.prefix()),
                name.namespace(),
                "{:?}",
                name.qualified()
            );
        }
    }
    // Saving and reparsing gives the same tree.
    let saved = yaxml::save::to_vec(document, &yaxml::save::Options::default()).unwrap();
    let reparsed = OwnedDocument::parse_bytes(&saved).expect("saved output reparses");
    assert_eq!(
        shape(document),
        shape(&reparsed),
        "{}",
        String::from_utf8_lossy(&saved)
    );
}

/// Subtree size by following links, independent of stored sizes.
fn walk(root: OwnedNode<'_>) -> usize {
    fn next<'d>(node: &OwnedNode<'d>, root: OwnedNodeId) -> Option<OwnedNode<'d>> {
        node.first_child().or_else(|| {
            std::iter::successors(Some(*node), |node| node.parent())
                .take_while(|node| node.id() != root)
                .find_map(|node| node.next_sibling())
        })
    }
    std::iter::successors(Some(root), |node| next(node, root.id())).count()
}

fn in_entity(node: OwnedNode<'_>) -> bool {
    std::iter::successors(node.parent(), |node| node.parent())
        .any(|node| node.kind() == OwnedNodeKind::EntityRef)
}

#[derive(Debug, Eq, PartialEq)]
enum Shape {
    Element(
        String,
        Option<String>,
        Vec<(String, Option<String>, String)>,
    ),
    End,
    Text(String),
    Other(OwnedNodeKind, Option<String>),
}

/// Preorder shape with adjacent Text joined and empty Text dropped, as a
/// reparse sees them; entity references are opaque.
fn shape(document: &OwnedDocument) -> Vec<Shape> {
    let mut shapes = Vec::new();
    let mut stack = vec![(document.root(), false)];
    while let Some((node, closing)) = stack.pop() {
        if closing {
            shapes.push(Shape::End);
            continue;
        }
        match node.kind() {
            OwnedNodeKind::Element => {
                let name = node.name().unwrap();
                let mut attributes: Vec<_> = node
                    .attributes()
                    .map(|attribute| {
                        let name = &attribute.name;
                        let namespace = name.namespace().map(str::to_owned);
                        (
                            name.qualified().to_owned(),
                            namespace,
                            attribute.value.to_string(),
                        )
                    })
                    .collect();
                attributes.sort();
                let namespace = name.namespace().map(str::to_owned);
                shapes.push(Shape::Element(
                    name.qualified().to_owned(),
                    namespace,
                    attributes,
                ));
            }
            OwnedNodeKind::Text => {
                let text = node.text().unwrap();
                match shapes.last_mut() {
                    Some(Shape::Text(previous)) => previous.push_str(text),
                    _ => shapes.push(Shape::Text(text.to_owned())),
                }
                if text.is_empty() {
                    continue;
                }
            }
            kind => {
                let value = node.text().or(node.entity_name()).map(str::to_owned);
                shapes.push(Shape::Other(kind, value));
            }
        }
        if matches!(
            node.kind(),
            OwnedNodeKind::Element | OwnedNodeKind::Document
        ) {
            stack.push((node, true));
            let children: Vec<_> = node.children().collect();
            stack.extend(children.into_iter().rev().map(|child| (child, false)));
        }
    }
    shapes.retain(|shape| !matches!(shape, Shape::Text(text) if text.is_empty()));
    shapes
}
