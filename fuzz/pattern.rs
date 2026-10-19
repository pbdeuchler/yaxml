//! Pattern compile, DOM match and streaming. A testcase is a flags byte (low
//! three bits), a pattern, and optionally a NUL byte and the XML document to
//! match against.

use yaxml::pattern::{Limits, Pattern, StreamMatcher};
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
    stream: &mut StreamMatcher<'_>,
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
    for child in elements(node) {
        stream_walk(document, pattern, child, stream, exact);
    }
    stream.pop();
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
    let Ok(pattern) = Pattern::compile_with_limits(&source, NAMESPACES, flags, LIMITS) else {
        return;
    };
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
    if let Some(mut stream) = pattern.stream() {
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
