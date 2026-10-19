//! Pattern compile, DOM match and streaming. A testcase is a flags byte (low
//! three bits), a pattern, and optionally a NUL byte and the XML document to
//! match against. A recompilation under small input-derived limits
//! either fails on a limit or streams until its depth limit.

use yaxml::pattern::{ErrorKind, Limit, Limits, Pattern, StreamMatcher};
use yaxml::xpath::Node;
use yaxml::{Document, Node as Dom, NodeKind, OwnedDocument, OwnedNodeKind};

const DOCUMENT: &str = "<r xmlns:p='urn:p' xmlns='urn:d'><a x='1' p:y='2'><b/><p:b/></a><b p:x='3'><a/></b><a><a><b/></a></a></r>";

const NAMESPACES: &[(&str, &str)] = &[("p", "urn:p"), ("d", "urn:d")];

const LIMITS: Limits = Limits {
    max_bytes: 4096,
    max_steps: 32,
    max_depth: 1024,
};

type Element<'d> = Dom<'d, 'd>;

fn elements(node: Element<'_>) -> impl Iterator<Item = Element<'_>> {
    node.children()
        .filter(|child| matches!(child.kind(), NodeKind::Element(_)))
}

/// Every element and attribute in order, streamed in the exact order a reader
/// delivers them; each streamed result must equal the DOM match.
fn stream_walk(
    document: &Document<'_>,
    pattern: &Pattern,
    node: Element<'_>,
    stream: &mut StreamMatcher,
    exact: bool,
) {
    // Attributes are not nodes of `//.`-like patterns, which libxml2 streams
    // inconsistently; only elements are compared for them.
    let attributes = exact && !stream.wants_any_node();
    let name = node.name().expect("element");
    let pushed = stream.push(name.local(), name.namespace()).expect("depth");
    if exact {
        assert_eq!(
            pushed,
            pattern.matches(document, node.id()),
            "element {name:?}"
        );
    }
    for (index, attribute) in node.attributes().iter().enumerate() {
        let qualified = attribute.name.qualified();
        if qualified == "xmlns" || qualified.starts_with("xmlns:") {
            continue;
        }
        let local = attribute.name.local();
        let pushed = stream
            .push_attr(local, attribute.name.namespace())
            .expect("depth");
        if attributes {
            let expected = pattern.matches(document, Node::Attribute(node.id(), index));
            assert_eq!(pushed, expected, "attribute {qualified:?}");
        }
        stream.pop();
    }
    for child in node.children() {
        if matches!(child.kind(), NodeKind::Element(_)) {
            stream_walk(document, pattern, child, stream, exact);
        } else if stream.wants_any_node() {
            stream.push_other().expect("depth");
            stream.pop();
        }
    }
    stream.pop();
}

/// Streams the first-child chain under small limits: only the depth limit,
/// and only beyond `max_depth` open elements, may stop it. (Like libxml2,
/// an XPath-mode `.` alternative tracks no depth, so it never stops.)
fn deep_walk(stream: &mut StreamMatcher, node: Element<'_>, depth: usize, max_depth: usize) {
    let name = node.name().expect("element");
    match stream.push(name.local(), name.namespace()) {
        Ok(_) => {
            if let Some(child) = elements(node).next() {
                deep_walk(stream, child, depth + 1, max_depth);
            }
            stream.pop();
        }
        Err(error) => {
            assert_eq!(error.kind, ErrorKind::LimitExceeded(Limit::Depth));
            assert!(depth > max_depth && !error.to_string().is_empty());
        }
    }
}

pub fn exercise(input: &[u8]) {
    let Some((&control, rest)) = input.split_first() else {
        return;
    };
    let flags = u32::from(control & 7);
    let (source, xml) = match rest.iter().position(|&byte| byte == 0) {
        Some(at) => (&rest[..at], &rest[at + 1..]),
        None => (rest, DOCUMENT.as_bytes()),
    };
    let source = String::from_utf8_lossy(source);
    let hash = rest.iter().fold(0u32, |hash, &byte| {
        hash.wrapping_mul(31).wrapping_add(byte.into())
    });
    let small = Limits {
        max_bytes: source.len().saturating_sub((hash & 1) as usize),
        max_steps: 1 + (hash >> 1 & 3) as usize,
        max_depth: 1 + (hash >> 3 & 3) as usize,
    };
    let tight = Pattern::compile_with_limits(&source, NAMESPACES, flags, small);
    let pattern = match Pattern::compile_with_limits(&source, NAMESPACES, flags, LIMITS) {
        Ok(pattern) => pattern,
        Err(error) => {
            assert!(!error.to_string().is_empty());
            return;
        }
    };
    assert!(Pattern::compile(&source, NAMESPACES, flags).is_ok());
    if let Err(error) = &tight {
        assert!(matches!(error.kind, ErrorKind::LimitExceeded(_)), "{error}");
    }
    let (Ok(borrowed), Ok(owned)) = (Document::parse_bytes(xml), OwnedDocument::parse_bytes(xml))
    else {
        return;
    };
    let left: Vec<_> = borrowed
        .nodes()
        .filter(|n| matches!(n.kind(), NodeKind::Element(_)))
        .map(|n| pattern.matches(&borrowed, n.id()))
        .collect();
    let right: Vec<_> = owned
        .nodes()
        .filter(|n| n.kind() == OwnedNodeKind::Element)
        .map(|n| pattern.matches(&owned, n.id()))
        .collect();
    assert_eq!(left, right, "DOM disagreement for {source:?}");
    // Nodes of a larger document are foreign: lookups find nothing, and no
    // match panics.
    if hash & 15 == 0 {
        let foreign = format!("<r>{}</r>", "<a/>".repeat(2000));
        let big = OwnedDocument::parse(&foreign).unwrap();
        let last = big.nodes().last().unwrap().id();
        let big = Document::parse(&foreign).unwrap();
        let id = big.nodes().last().unwrap().id();
        for index in 0..2 {
            for node in [
                Node::Tree(last),
                Node::Attribute(last, index),
                Node::Namespace(last, index),
            ] {
                pattern.matches(&owned, node);
            }
            for node in [
                Node::Tree(id),
                Node::Attribute(id, index),
                Node::Namespace(id, index),
            ] {
                pattern.matches(&borrowed, node);
            }
        }
    }
    let (element, owned_element) = (borrowed.root_element().id(), owned.root_element().id());
    assert_eq!(
        pattern.matches(&borrowed, Node::Namespace(element, 0)),
        pattern.matches(&owned, Node::Namespace(owned_element, 0))
    );
    if let Some(mut stream) = tight.as_ref().ok().and_then(Pattern::stream) {
        stream.reset();
        deep_walk(&mut stream, borrowed.root_element(), 1, small.max_depth);
    }
    if let Some(mut stream) = pattern.stream() {
        stream.reset();
        // IDC and XPath modes anchor at the first pushed node, not anywhere.
        stream_walk(
            &borrowed,
            &pattern,
            borrowed.root_element(),
            &mut stream,
            flags == 0,
        );
    }
}
