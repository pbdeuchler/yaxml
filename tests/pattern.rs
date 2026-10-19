#![forbid(unsafe_code)]
//! `yaxml::pattern` against libxml2 2.15.4's `test/pattern` suite (`runtest.c`
//! `patternTest` output, byte for byte) plus table-driven compile, match and
//! stream cases.

use std::fs;

use yaxml::pattern::{
    ErrorKind, Limit, Limits, Pattern, StreamMatcher, XPATH, XS_FIELD, XS_SELECTOR,
};
use yaxml::xpath::Node;
use yaxml::{Document, Node as DomNode, NodeKind, OwnedDocument, OwnedNodeKind};

const NS: &[(&str, &str)] = &[("p", "urn:p")];

// ---- libxml2 suite ---------------------------------------------------------

type Element<'d> = DomNode<'d, 'd>;

fn elements<'d>(node: Element<'d>) -> impl Iterator<Item = Element<'d>> {
    node.children()
        .filter(|child| matches!(child.kind(), NodeKind::Element(_)))
}

/// `xmlGetNodePath` for an element: an index counts same-named siblings, `*`
/// stands for default-namespace names.
fn path(node: Element<'_>) -> String {
    let name = node.name().unwrap();
    let generic = name.namespace().is_some() && name.prefix().is_none();
    let step = if generic { "*" } else { name.qualified() };
    let same = |n: &Element<'_>| {
        matches!(n.kind(), NodeKind::Element(_))
            && (generic
                || n.name().is_some_and(|other| {
                    other.qualified() == name.qualified()
                        && other.namespace().is_some() == name.namespace().is_some()
                }))
    };
    let parent = node.parent().unwrap();
    let siblings = |first| std::iter::successors(first, |n: &Element<'_>| n.next_sibling());
    let before = siblings(parent.first_child())
        .take_while(|n| n.id() != node.id())
        .filter(same)
        .count();
    let index = if before > 0 {
        before + 1
    } else {
        usize::from(siblings(node.next_sibling()).any(|n| same(&n)))
    };
    let parent = if matches!(parent.kind(), NodeKind::Element(_)) {
        path(parent)
    } else {
        String::new()
    };
    match index {
        0 => format!("{parent}/{step}"),
        n => format!("{parent}/{step}[{n}]"),
    }
}

/// One pattern over one document, as `patternNode` prints it.
fn run_pattern(line: &str, document: &Document<'_>) -> String {
    let root = document.root_element();
    let namespaces: Vec<_> = root
        .attributes()
        .iter()
        .filter_map(|a| Some((a.name.qualified().strip_prefix("xmlns:")?, &*a.value)))
        .take(10)
        .collect();
    let pattern = Pattern::compile(line, &namespaces, 0).expect("pattern compiles");
    let mut stream = pattern.stream();
    let mut out = String::new();
    fn visit(
        node: Element<'_>,
        line: &str,
        document: &Document<'_>,
        pattern: &Pattern,
        stream: &mut Option<StreamMatcher<'_>>,
        out: &mut String,
    ) {
        let matched = pattern.matches(document, node.id());
        let node_path = path(node);
        if matched {
            out.push_str(&format!("Node {node_path} matches pattern {line}\n"));
        }
        if let Some(stream) = stream {
            let name = node.name().unwrap();
            let pushed = stream.push(name.local(), name.namespace()).unwrap();
            if pushed != matched {
                out.push_str(&format!(
                    "xmlPatternMatch and xmlStreamPush disagree\n  pattern {line} node {node_path}\n"
                ));
            }
        }
        for child in elements(node) {
            visit(child, line, document, pattern, stream, out);
        }
        if let Some(stream) = stream {
            stream.pop();
        }
    }
    visit(root, line, document, &pattern, &mut stream, &mut out);
    out
}

#[test]
fn libxml2_pattern_suite_output_is_identical() {
    let directory = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pattern");
    for name in ["conj", "multiple", "namespaces", "simple"] {
        let read =
            |extension| fs::read_to_string(format!("{directory}/{name}.{extension}")).unwrap();
        let xml = read("xml");
        let document = Document::parse(&xml).unwrap();
        let actual: String = read("pat")
            .lines()
            .map(str::trim_end)
            // libxml2 reports an empty line as "failed to compile" and goes on.
            .filter(|line| !line.is_empty())
            .map(|line| run_pattern(line, &document))
            .collect();
        assert_eq!(actual, read("out"), "{name}");
    }
}

// ---- compilation -----------------------------------------------------------

#[test]
fn compile_errors() {
    let cases: &[(&str, u32, ErrorKind)] = &[
        ("", 0, ErrorKind::Syntax),
        ("|a", 0, ErrorKind::Syntax),
        ("a||b", 0, ErrorKind::Syntax),
        ("|", 0, ErrorKind::Syntax),
        ("a/", 0, ErrorKind::Syntax),
        ("/", 0, ErrorKind::Syntax),
        (".//", 0, ErrorKind::Syntax),
        ("a//", 0, ErrorKind::Syntax),
        ("a b", 0, ErrorKind::Syntax),
        ("a*", 0, ErrorKind::Syntax),
        ("a[1]", 0, ErrorKind::Syntax),
        ("@", 0, ErrorKind::Syntax),
        ("p:", 0, ErrorKind::Syntax),
        ("p: a", 0, ErrorKind::Syntax),
        ("p :a", 0, ErrorKind::Syntax),
        ("self::a", 0, ErrorKind::Syntax),
        ("q:a", 0, ErrorKind::UnboundPrefix),
        ("@q:a", 0, ErrorKind::UnboundPrefix),
        // `xmlPatScanName` swallows the colon, so `child::` takes no prefix.
        ("child::q:*", 0, ErrorKind::Syntax),
        ("@a", XS_SELECTOR, ErrorKind::Syntax),
        ("attribute::a", XS_SELECTOR, ErrorKind::Syntax),
        ("/a", XS_SELECTOR, ErrorKind::Syntax),
        ("a//b", XS_FIELD, ErrorKind::Syntax),
        ("./ /a", XS_FIELD, ErrorKind::Syntax),
        (".x", XS_FIELD, ErrorKind::Syntax),
        ("a/", XS_FIELD, ErrorKind::Syntax),
        ("./", XS_FIELD, ErrorKind::Syntax),
    ];
    for &(source, flags, kind) in cases {
        let error = Pattern::compile(source, NS, flags).expect_err(source);
        assert_eq!(error.kind, kind, "{source:?} flags {flags}");
    }
    let tiny = Limits {
        max_bytes: 8,
        max_steps: 3,
        max_depth: 8,
    };
    for (source, limit) in [("a/b/c/d", Limit::Steps), ("abcdefghi", Limit::Bytes)] {
        let error = Pattern::compile_with_limits(source, &[], 0, tiny).unwrap_err();
        assert_eq!(error.kind, ErrorKind::LimitExceeded(limit), "{source}");
    }
    assert_eq!(
        Pattern::compile("a|b/", &[], 0).unwrap_err().offset,
        Some(4)
    );
}

#[test]
fn compile_accepts() {
    let cases: &[(&str, u32)] = &[
        ("a|", 0),
        ("a | b", 0),
        (" a / b ", 0),
        ("a/b|/c//d", 0),
        ("//a", 0),
        (".//a", 0),
        ("child::a", 0),
        ("child::*", 0),
        ("child:: a:b", 0),
        ("a/@b", 0),
        ("@p:*", 0),
        ("attribute::*", 0),
        ("xml:lang", 0),
        ("@xml:lang", 0),
        ("a\0 garbage |", 0),
        ("a/b", XPATH),
        (".", XS_SELECTOR),
        (".//a/b", XS_SELECTOR),
        ("./a|b/c", XS_SELECTOR),
        ("@a", XS_FIELD),
        ("a/@b", XS_FIELD),
    ];
    for &(source, flags) in cases {
        Pattern::compile(source, NS, flags).expect(source);
    }
}

// ---- DOM matching ----------------------------------------------------------

const DOCUMENT: &str = "<r xmlns:p='urn:p'><a><b/><p:b/></a><b/><a x='1' p:y='2'/></r>";

/// Preorder element ordinals matched, identical on both DOMs.
fn hits(pattern: &Pattern, xml: &str) -> Vec<usize> {
    let borrowed = Document::parse(xml).unwrap();
    let owned = OwnedDocument::parse(xml).unwrap();
    let left: Vec<_> = borrowed
        .nodes()
        .filter(|n| matches!(n.kind(), NodeKind::Element(_)))
        .enumerate()
        .filter(|(_, n)| pattern.matches(&borrowed, n.id()))
        .map(|(i, _)| i)
        .collect();
    let right: Vec<_> = owned
        .nodes()
        .filter(|n| n.kind() == OwnedNodeKind::Element)
        .enumerate()
        .filter(|(_, n)| pattern.matches(&owned, n.id()))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(left, right, "DOMs disagree");
    left
}

#[test]
fn matches_elements_on_both_doms() {
    // Elements: 0 r, 1 a, 2 b, 3 p:b, 4 b, 5 a.
    let cases: &[(&str, &[usize])] = &[
        ("b", &[2, 4]),
        ("a/b", &[2]),
        ("/r", &[0]),
        ("/r/a", &[1, 5]),
        ("/r/a/b", &[2]),
        ("//b", &[2, 4]),
        ("r//b", &[2, 4]),
        ("/r//b", &[2, 4]),
        ("/r/a//b", &[2]),
        ("p:b", &[3]),
        ("p:*", &[3]),
        ("a/p:*", &[3]),
        ("*", &[0, 1, 2, 3, 4, 5]),
        (".", &[0, 1, 2, 3, 4, 5]),
        ("*/b", &[2, 4]),
        ("*//b", &[2, 4]),
        ("a|b", &[1, 2, 4, 5]),
        ("child::b", &[2, 4]),
        ("a/child::b", &[2]),
        ("child::p:b", &[]),
        ("xml:b", &[]),
        ("/a", &[]),
        ("/b", &[]),
        ("a/a", &[]),
        ("b/a", &[]),
        ("c", &[]),
        ("@x", &[]),
        ("a//a", &[]),
        ("r/a/b|r/b", &[2, 4]),
    ];
    for &(source, expected) in cases {
        let pattern = Pattern::compile(source, NS, 0).expect(source);
        assert_eq!(hits(&pattern, DOCUMENT), expected, "{source}");
    }
}

#[test]
fn matches_attributes_and_non_elements() {
    let document = Document::parse(DOCUMENT).unwrap();
    let last = document.root_element().last_child().unwrap();
    let id = last.id();
    // Attribute slots: x=1 is index 0, p:y=2 index 1 (the root's xmlns:p is not here).
    let cases: &[(&str, Node<_>, bool)] = &[
        ("@x", Node::Attribute(id, 0), true),
        ("@y", Node::Attribute(id, 1), false),
        ("@p:y", Node::Attribute(id, 1), true),
        ("@p:*", Node::Attribute(id, 1), true),
        ("@p:*", Node::Attribute(id, 0), false),
        ("@*", Node::Attribute(id, 1), true),
        ("a/@x", Node::Attribute(id, 0), true),
        ("b/@x", Node::Attribute(id, 0), false),
        ("/r/a/@x", Node::Attribute(id, 0), true),
        ("//@x", Node::Attribute(id, 0), true),
        ("attribute::x", Node::Attribute(id, 0), true),
        ("@x", Node::Attribute(id, 5), false),
        ("@x", Node::Tree(id), false),
        ("a", Node::Attribute(id, 0), false),
        ("a", Node::Namespace(id, 0), false),
        ("a", Node::Tree(document.root().id()), false),
        ("/r", Node::Tree(document.root().id()), false),
    ];
    for (source, node, expected) in cases {
        let pattern = Pattern::compile(source, NS, 0).expect(source);
        assert_eq!(
            pattern.matches(&document, *node),
            *expected,
            "{source} {node:?}"
        );
    }
}

#[test]
fn deep_ancestor_chains_stay_polynomial() {
    let depth = 200;
    let xml = format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
    let pattern = Pattern::compile("a//a//a//a//a//a//a//a//a//b", &[], 0).unwrap();
    assert!(hits(&pattern, &xml).is_empty());
}

// ---- streaming ---------------------------------------------------------------

/// `+n` pushes an element, `@n` an attribute (then pops it), `-` pops, `t` pushes
/// a text node, `/` resets. Returns the result of every push.
fn trace(pattern: &Pattern, events: &[&str]) -> Vec<bool> {
    let mut stream = pattern.stream().expect("streamable");
    let mut results = Vec::new();
    for event in events {
        match event.split_at(1) {
            ("+", name) => results.push(stream.push(name, None).unwrap()),
            ("@", name) => {
                results.push(stream.push_attr(name, None).unwrap());
                stream.pop();
            }
            ("-", _) => stream.pop(),
            ("t", _) => results.push(stream.push_other().unwrap()),
            ("/", _) => results.push(stream.reset()),
            _ => unreachable!("{event}"),
        }
    }
    results
}

#[test]
fn stream_traces() {
    // (pattern, flags, events, result of each push)
    let cases: &[(&str, u32, &[&str], &[bool])] = &[
        (
            "a/b",
            0,
            &["+a", "+b", "-", "+c", "+b", "-", "-", "-", "+b"],
            &[false, true, false, false, false],
        ),
        (
            "a/b",
            0,
            &["+x", "+a", "+b", "-", "-", "-"],
            &[false, false, true],
        ),
        (
            "/a/b",
            0,
            &["+a", "+b", "-", "+b", "-", "-", "+a"],
            &[false, true, true, false],
        ),
        ("/a/b", 0, &["+x", "+a", "+b"], &[false, false, false]),
        ("/a", 0, &["+a", "+a", "-", "-", "+a"], &[true, false, true]),
        (
            "//b",
            0,
            &["+a", "+b", "+b", "-", "-"],
            &[false, true, true],
        ),
        (
            "a//b",
            0,
            &["+a", "+x", "+b", "-", "-", "-", "+b"],
            &[false, false, true, false],
        ),
        (
            "a//b",
            0,
            &["+a", "+a", "+x", "+b", "-", "-", "-", "-", "+x", "+b"],
            &[false, false, false, true, false, false],
        ),
        (
            "a|b",
            0,
            &["+a", "-", "+c", "+b", "-", "-", "+d"],
            &[true, false, true, false],
        ),
        (
            "a/b|c",
            0,
            &["+a", "+b", "-", "-", "+c"],
            &[false, true, true],
        ),
        ("*", 0, &["+a", "+b"], &[true, true]),
        (
            "a/*",
            0,
            &["+a", "+b", "+c", "-", "-", "-", "+z", "+b"],
            &[false, true, false, false, false],
        ),
        (
            "a/@x",
            0,
            &["+a", "@x", "@y", "+b", "@x"],
            &[false, true, false, false, false],
        ),
        (
            "@x",
            0,
            &["+a", "@x", "+b", "@x"],
            &[false, true, false, true],
        ),
        ("a", 0, &["+a", "@a"], &[true, false]),
        ("/a|/b", 0, &["+b", "-", "+a", "+b"], &[true, true, false]),
        (
            "/a",
            0,
            &["+a", "-", "/", "+b", "-", "/", "+a"],
            &[true, false, false, false, true],
        ),
        (
            "//.",
            0,
            &["/", "+a", "t", "-", "+b"],
            &[true, true, true, true],
        ),
        (
            "a//.",
            0,
            &["+b", "+a", "+c", "t", "-", "-"],
            &[false, true, true, true],
        ),
        (".", 0, &["/", "+a", "+b"], &[false, true, true]),
        ("a", 0, &["+b", "t", "+a"], &[false, false, true]),
        (".", XPATH, &["+a"], &[false]),
        (
            ".",
            XS_SELECTOR,
            &["+a", "+b", "-", "-", "+c"],
            &[true, false, true],
        ),
        (
            "a/b",
            XS_SELECTOR,
            &["+a", "+b", "+x", "+a"],
            &[false, false, false, false],
        ),
        (
            "b",
            XS_SELECTOR,
            &["+r", "+b", "-", "+c", "+b"],
            &[false, true, false, false],
        ),
        ("./b", XS_SELECTOR, &["+r", "+b"], &[false, true]),
        (
            "@x",
            XS_FIELD,
            &["+r", "@x", "@y", "+b", "@x"],
            &[false, true, false, false, false],
        ),
        ("b", XS_FIELD, &["+r", "+b", "+b"], &[false, true, false]),
    ];
    for &(source, flags, events, expected) in cases {
        let pattern = Pattern::compile(source, NS, flags).expect(source);
        assert_eq!(trace(&pattern, events), expected, "{source} flags {flags}");
    }
}

#[test]
fn streams_are_namespace_aware() {
    let pattern = Pattern::compile("p:b|c", NS, 0).unwrap();
    let mut stream = pattern.stream().unwrap();
    assert_eq!(stream.push("b", Some("urn:p")), Ok(true));
    stream.pop();
    assert_eq!(stream.push("b", None), Ok(false));
    stream.pop();
    assert_eq!(stream.push("b", Some("")), Ok(false));
    stream.pop();
    assert_eq!(stream.push("c", Some("urn:p")), Ok(false));
    stream.pop();
    assert_eq!(stream.push("c", None), Ok(true));
}

#[test]
fn stream_availability_and_needs() {
    for (source, streamable, any_node) in [
        ("a/b", true, false),
        ("/a|/b", true, false),
        ("a|/b", true, false),
        ("/a|.//b", false, false),
        ("./a|/b", false, false),
        (".", true, true),
        ("//.", true, true),
        ("a//.", true, true),
        ("./.", false, false),
    ] {
        let pattern = Pattern::compile(source, NS, 0).expect(source);
        let stream = pattern.stream();
        assert_eq!(stream.is_some(), streamable, "{source}");
        if let Some(stream) = stream {
            assert_eq!(stream.wants_any_node(), any_node, "{source}");
        }
    }
}

#[test]
fn stream_depth_is_bounded() {
    let limits = Limits {
        max_depth: 3,
        ..Limits::default()
    };
    let pattern = Pattern::compile_with_limits("//a", &[], 0, limits).unwrap();
    let mut stream = pattern.stream().unwrap();
    for _ in 0..3 {
        assert_eq!(stream.push("a", None), Ok(true));
    }
    assert_eq!(
        stream.push("a", None).unwrap_err().kind,
        ErrorKind::LimitExceeded(Limit::Depth)
    );
    stream.pop();
    assert_eq!(stream.push("b", None), Ok(false));
}

#[test]
fn stream_state_is_independent_of_document_size() {
    // Nested states are deduplicated: a deep run of a//a//a stays fast and exact.
    let pattern = Pattern::compile("a//a//a//a//a//b", &[], 0).unwrap();
    let mut stream = pattern.stream().unwrap();
    for _ in 0..1000 {
        assert_eq!(stream.push("a", None), Ok(false));
    }
    assert_eq!(stream.push("b", None), Ok(true));
}

#[test]
fn error_display_names_kind_and_offset() {
    let error = Pattern::compile("a b", &[], 0).unwrap_err();
    assert_eq!(error.to_string(), "pattern error Syntax at byte 2");
}
