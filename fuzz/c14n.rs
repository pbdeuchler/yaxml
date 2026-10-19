//! Canonical XML over both DOMs. A testcase is one selector byte followed by
//! an XML document: bits 0-1 pick the mode, bit 2 keeps comments, bit 3
//! selects a node-set with the expression chosen by bits 4-6, and bit 7 adds
//! an InclusiveNamespaces prefix list.

use yaxml::c14n::{self, Mode, Options};
use yaxml::xpath::{Context, Tree, XPath};
use yaxml::{Document, OwnedDocument};

const EXPRESSIONS: [&str; 8] = [
    "(//. | //@* | //namespace::*)[ancestor-or-self::*[2]]",
    "//node()[not(self::comment())] | //@*",
    "/*/*//node() | //namespace::*",
    "//@* | //text() | //processing-instruction()",
    "(//. | //@* | //namespace::*)[not(ancestor-or-self::p:*)]",
    "//*[@xml:lang or @xml:base] | //*/namespace::*[name() != 'xml']",
    "//*[position() mod 2 = 1] | //@*[position() = 1]",
    "/ | //comment() | //*[last()]",
];

const PREFIXES: [&str; 3] = ["#default", "p", "xml"];

fn canonical<T: Tree>(tree: &T, root: T::Id, selector: u8) -> Option<Result<String, c14n::Error>> {
    let mode =
        [Mode::C14n10, Mode::C14n11, Mode::Exclusive10, Mode::C14n10][usize::from(selector & 3)];
    let nodes = match selector & 8 {
        0 => None,
        _ => {
            let expression = EXPRESSIONS[usize::from(selector >> 4 & 7)];
            let context = Context::new().with_namespace("p", "urn:p");
            // Selection failures (limits) are XPath's concern, not canonicalization's.
            let xpath = XPath::compile(expression).expect("fixed expression compiles");
            Some(xpath.select_nodes(tree, root, &context).ok()?)
        }
    };
    let options = Options {
        with_comments: selector & 4 != 0,
        nodes: nodes.as_deref(),
        inclusive_prefixes: if selector & 128 != 0 { &PREFIXES } else { &[] },
        ..Options::new(mode)
    };
    Some(c14n::to_string(tree, &options))
}

pub fn exercise(input: &[u8]) {
    let Some((&selector, xml)) = input.split_first() else {
        return;
    };
    let Ok(owned) = OwnedDocument::parse_bytes(xml) else {
        return;
    };
    let Some(result) = canonical(&owned, owned.root().id(), selector) else {
        return;
    };
    if let Ok(borrowed) = Document::parse_bytes(xml) {
        let other = canonical(&borrowed, borrowed.root().id(), selector);
        assert_eq!(
            format!("{other:?}"),
            format!("{:?}", Some(&result)),
            "DOM disagreement"
        );
    }
    // A whole-document canonical form is its own canonical form. Raw namespace
    // URIs (`&`) can make it malformed, as in libxml2; such output is skipped.
    if let (0, Ok(canonical_form)) = (selector & 8, &result)
        && let Ok(reparsed) = Document::parse(canonical_form)
    {
        let again = canonical(&reparsed, reparsed.root().id(), selector);
        assert_eq!(
            again.and_then(Result::ok).as_ref(),
            Some(canonical_form),
            "canonical form is not idempotent"
        );
    }
}
