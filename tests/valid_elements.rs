//! `valid::valid_elements` against libxml2 2.15.4's `xmlValidGetValidElements`
//! at every insertion point of every element, in both DOMs. The expected
//! dumps were recorded with harness/tooling/libxml2_valid_elements.c.

use std::fmt::Write;

use yaxml::valid::{self, ContentKind, Dtd};
use yaxml::{Document, NodeKind, OwnedDocument, OwnedNodeKind};

fn names(result: Option<Vec<&str>>) -> String {
    result.map_or(" -1".into(), |names| {
        names.iter().map(|name| format!(" {name}")).collect()
    })
}

/// The dump of `libxml2_valid_elements` for one DOM: `dump!(document, element kind pattern)`.
macro_rules! dump {
    ($document:expr, $element:pat) => {{
        let document = $document;
        let dtd = document.dtd().unwrap();
        let mut out = String::new();
        let mut stack = vec![document.root()];
        let mut ordinal = 0;
        while let Some(node) = stack.pop() {
            let children: Vec<_> = node.children().collect();
            // Entity references are not entered, as the helper walks.
            let element = matches!(node.kind(), $element);
            if element || node.parent().is_none() {
                stack.extend(children.iter().rev().copied());
            }
            if !element {
                continue;
            }
            writeln!(out, "{ordinal} {}", node.name().unwrap().qualified()).unwrap();
            ordinal += 1;
            let id = |index: usize| children.get(index).map(|child| child.id());
            let mut line = |what: String, prev, next| {
                let result = valid::valid_elements(&document, dtd, prev, next);
                writeln!(out, "  {what}:{}", names(result)).unwrap();
            };
            if let Some(last) = children.len().checked_sub(1) {
                line("prepend".into(), None, id(0));
                line("append".into(), id(last), None);
            }
            for index in 0..children.len() {
                let prev = index.checked_sub(1).and_then(id);
                line(format!("before {index}"), prev, id(index));
                line(format!("replace {index}"), prev, id(index + 1));
            }
        }
        out
    }};
}

#[test]
fn valid_elements_match_libxml2() {
    for name in ["models", "ids"] {
        let path = format!("tests/fixtures/valid_elements/{name}");
        let input = std::fs::read_to_string(format!("{path}.xml")).unwrap();
        let expected = std::fs::read_to_string(format!("{path}.txt")).unwrap();
        let owned = dump!(
            OwnedDocument::parse(&input).unwrap(),
            OwnedNodeKind::Element
        );
        assert_eq!(owned, expected, "{name}: owned");
        let borrowed = dump!(Document::parse(&input).unwrap(), NodeKind::Element(_));
        assert_eq!(borrowed, expected, "{name}: borrowed");
    }
}

#[test]
fn potential_children_list_each_model_name_once() {
    let dtd = Dtd::parse(
        b"<!ELEMENT e ((a, b) | (a, c)*)+> <!ELEMENT m (#PCDATA | x | y)*> \
          <!ELEMENT p (#PCDATA)> <!ELEMENT n EMPTY> <!ELEMENT y ANY> \
          <!ATTLIST u k CDATA #IMPLIED>",
    )
    .unwrap();
    let cases: [(&str, ContentKind, &[&str]); 6] = [
        ("e", ContentKind::Element, &["a", "b", "c"]),
        ("m", ContentKind::Mixed, &["#PCDATA", "x", "y"]),
        ("p", ContentKind::Mixed, &["#PCDATA"]),
        ("n", ContentKind::Empty, &[]),
        ("y", ContentKind::Any, &[]),
        ("u", ContentKind::Undefined, &[]),
    ];
    for (name, kind, expected) in cases {
        let element = dtd.element(name).unwrap();
        assert_eq!(element.kind, kind, "{name}");
        assert_eq!(
            element.potential_children().collect::<Vec<_>>(),
            expected,
            "{name}"
        );
    }
}

#[test]
fn valid_elements_reject_points_without_a_declared_parent() {
    let input = "<!DOCTYPE r [<!ELEMENT r (a)*> <!ELEMENT a EMPTY>]><r><a/></r><!--x-->";
    let document = OwnedDocument::parse(input).unwrap();
    let dtd = document.dtd().unwrap();
    let root = document.document_element().unwrap();
    let a = root.first_child().map(|a| a.id());
    let comment = root.next_sibling().map(|c| c.id());
    let undeclared = OwnedDocument::parse("<!DOCTYPE q [<!ELEMENT q ANY>]><q><a/></q>").unwrap();
    let cases = [
        ("no point", None, None, None),
        ("top level", None, comment, None),
        ("valid", a, None, Some(vec!["a"])),
    ];
    for (name, prev, next, expected) in cases {
        assert_eq!(
            valid::valid_elements(&document, dtd, prev, next),
            expected,
            "{name}"
        );
    }
    // `r` is not declared by the other document's DTD.
    let other = undeclared.dtd().unwrap();
    assert_eq!(valid::valid_elements(&document, other, a, None), None);
}
