//! `Walker` against libxml2 2.15.4's `xmlReaderWalker`: every accessor on
//! every node, for each way of moving. The expected dumps were recorded with
//! harness/tooling/libxml2_walker.c (`libxml2_walker FILE 0 MODE`).

use std::fmt::Write;

use yaxml::{EditLimits, NodeType, OwnedDocument, OwnedNodeKind, ParseOptions, Walker};

fn or<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or("-".into(), |value| value.to_string())
}

/// One line per node as the C helper prints it.
fn line(walker: &Walker<'_>, out: &mut String) {
    write!(
        out,
        "{} {} {} {} {} {} e{} v{} a{} h{} [{}]",
        walker.depth(),
        walker.node_type() as i32,
        or(walker.name()),
        or(walker.local_name()),
        or(walker.prefix()),
        or(walker.namespace_uri()),
        u8::from(walker.is_empty_element()),
        u8::from(walker.has_value()),
        walker.attribute_count(),
        u8::from(walker.has_attributes()),
        or(walker.value()),
    )
    .unwrap();
}

/// A move's C result: 1 moved, 0 not, -1 where libxml2 reports an error.
fn code(moved: bool, applies: bool) -> i32 {
    if moved { 1 } else { i32::from(applies) - 1 }
}

/// Attribute lookups and moves, then back to the element.
fn accessors(walker: &mut Walker<'_>, out: &mut String) {
    out.push_str("  get");
    for name in ["p:k", "xmlns", "xmlns:p", "k", "a", "b", "xml:lang"] {
        write!(out, " {}", or(walker.get_attribute(name))).unwrap();
    }
    let xmlns = "http://www.w3.org/2000/xmlns/";
    write!(
        out,
        " ns {} {} no {} {} {}",
        or(walker.get_attribute_ns("k", Some("urn:p"))),
        or(walker.get_attribute_ns("p", Some(xmlns))),
        or(walker.get_attribute_no(0)),
        or(walker.get_attribute_no(2)),
        or(walker.get_attribute_no(9)),
    )
    .unwrap();
    let element = walker
        .current_node()
        .is_some_and(|node| node.kind() == OwnedNodeKind::Element);
    let moved = walker.move_to_attribute("xml:lang");
    write!(out, " mv {} {}", code(moved, true), or(walker.name())).unwrap();
    write!(out, " in {}", or(walker.get_attribute("a"))).unwrap();
    walker.move_to_element();
    let moved = walker.move_to_attribute_ns("k", "urn:p");
    write!(out, " {} {}", code(moved, true), or(walker.name())).unwrap();
    walker.move_to_element();
    for index in [1, 9] {
        let moved = walker.move_to_attribute_no(index);
        write!(out, " {} {}", code(moved, element), or(walker.name())).unwrap();
    }
    let moved = walker.move_to_first_attribute();
    write!(out, " {} {}", code(moved, true), or(walker.name())).unwrap();
    let (back, again) = (walker.move_to_element(), walker.move_to_element());
    writeln!(
        out,
        " el {} {} look {} {}",
        u8::from(back),
        u8::from(again),
        or(walker.lookup_namespace(Some("p"))),
        or(walker.lookup_namespace(Some("xml"))),
    )
    .unwrap();
}

fn walk(document: &OwnedDocument, mode: u8) -> String {
    let mut walker = Walker::new(document);
    let mut out = String::new();
    let standalone = walker.standalone() as i32;
    writeln!(
        out,
        "state {} ver {} enc {} sa {standalone}",
        walker.read_state() as i32,
        walker.xml_version(),
        or(walker.encoding()),
    )
    .unwrap();
    let first = code(
        walker.move_to_first_attribute(),
        walker.current_node().is_some(),
    );
    writeln!(
        out,
        "before {} {} {} {first}",
        walker.depth(),
        walker.node_type() as i32,
        or(walker.name()),
    )
    .unwrap();
    let moved = loop {
        let skip = mode == 3
            && walker.node_type() == NodeType::Element
            && matches!(walker.name(), Some("a" | "d" | "y"));
        let moved = match mode {
            1 => walker.next(),
            2 => walker.next_sibling(),
            _ if skip => walker.next(),
            _ => walker.read(),
        };
        if !moved {
            break moved;
        }
        line(&walker, &mut out);
        writeln!(
            out,
            " lang={} base={} ns={} string={}\n  inner={}\n  outer={}",
            or(walker.xml_lang()),
            or(walker.base_uri()),
            or(walker.lookup_namespace(None)),
            or(walker.read_string()),
            or(walker.read_inner_xml().unwrap()),
            or(walker.read_outer_xml().unwrap()),
        )
        .unwrap();
        accessors(&mut walker, &mut out);
        while walker.move_to_next_attribute() {
            out.push_str("  @");
            line(&walker, &mut out);
            writeln!(out, " decl{}", u8::from(walker.is_namespace_decl())).unwrap();
        }
        walker.move_to_element();
    };
    writeln!(
        out,
        "end {} state {} type {}",
        u8::from(moved),
        walker.read_state() as i32,
        walker.node_type() as i32
    )
    .unwrap();
    out
}

#[test]
fn walks_match_libxml2_reader_walker() {
    let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/walker/");
    for name in ["mixed", "plain", "bare", "declared"] {
        let input = std::fs::read(format!("{fixtures}{name}.xml")).unwrap();
        let options = ParseOptions::DEFAULT.base(Some(format!("{name}.xml")));
        let document = OwnedDocument::parse_with(&input, options).unwrap();
        for mode in 0..4 {
            let path = format!("{fixtures}{name}.{mode}.txt");
            let actual = walk(&document, mode);
            if std::env::var_os("YAXML_WALKER_WRITE").is_some() {
                std::fs::write(format!("{path}.actual"), &actual).unwrap();
            }
            let expected = std::fs::read_to_string(&path).unwrap();
            assert_eq!(actual, expected, "{name} mode {mode}");
        }
    }
}

#[test]
fn walker_reads_the_tree_as_edited() {
    let mut document = OwnedDocument::parse("<r>a</r>").unwrap();
    let root = document.document_element().unwrap().id();
    let mut editor = document.edit(EditLimits::DEFAULT).unwrap();
    editor.append_text(root, "b").unwrap();
    let mut walker = Walker::new(&document);
    assert!(walker.current_node().is_none());
    let mut seen = Vec::new();
    while walker.read() {
        seen.push((
            walker.node_type(),
            walker.value(),
            walker.is_empty_element(),
        ));
    }
    // Adjacent text nodes stay two nodes; the walk keeps the last one.
    assert_eq!(
        seen,
        [
            (NodeType::Element, None, false),
            (NodeType::Text, Some("a"), false),
            (NodeType::Text, Some("b"), false),
            (NodeType::EndElement, None, false),
        ]
    );
    assert!(!walker.read() && !walker.next() && !walker.next_sibling());
    assert_eq!(walker.node_type(), NodeType::EndElement);
    assert_eq!(walker.current_node().map(|node| node.id()), Some(root));
    assert!(std::ptr::eq(walker.document(), &document));
    assert_eq!(walker.quote_char(), '"');

    let other = OwnedDocument::parse("<o/>").unwrap();
    walker.reset(&other);
    assert_eq!(walker.node_type(), NodeType::None);
    assert!(walker.next_sibling());
    assert_eq!(
        (walker.name(), walker.read_outer_xml().unwrap()),
        (Some("o"), Some("<o/>".into()))
    );
    assert!(!walker.next_sibling());
}
