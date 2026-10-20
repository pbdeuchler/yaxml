//! Canonical XML over both DOMs. A testcase is one selector byte followed by
//! an XML document: bits 0-1 pick the mode, bit 2 keeps comments, bit 3
//! selects a node-set with the expression chosen by bits 4-6, and bit 7 adds
//! an InclusiveNamespaces prefix list.
//!
//! `c14n::write` must produce the same bytes, and a sink that fails after
//! an input-derived byte count must surface an I/O error after writing a
//! prefix. A document that is not XML is parsed as HTML and canonicalized.

use std::io::{self, Write};

use yaxml::c14n::{self, Mode, Options};
use yaxml::xpath::{Context, Tree, XPath};
use yaxml::{Document, OwnedDocument, html};

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

/// Accepts `room` bytes, then fails every write.
struct Failing {
    written: Vec<u8>,
    room: usize,
}

impl Write for Failing {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = bytes.len().min(self.room - self.written.len());
        if length == 0 && !bytes.is_empty() {
            return Err(io::Error::other("fuzz sink full"));
        }
        self.written.extend_from_slice(&bytes[..length]);
        Ok(length)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Every error displays, and only I/O errors have a source.
fn check_error(error: &c14n::Error) {
    assert!(!error.to_string().is_empty());
    assert_eq!(
        std::error::Error::source(error).is_some(),
        matches!(error, c14n::Error::Io(_))
    );
}

/// `write` agrees with `to_string`, through a sink that may fail.
fn written<T: Tree>(
    tree: &T,
    options: &Options<'_, T::Id>,
    expected: &Result<String, c14n::Error>,
    room: usize,
) {
    let mut sink = Failing {
        written: Vec::new(),
        room,
    };
    let result = c14n::write(tree, options, &mut sink);
    match (expected, result) {
        (Ok(expected), Ok(())) => assert_eq!(sink.written, expected.as_bytes()),
        (Ok(expected), Err(error @ c14n::Error::Io(_))) => {
            check_error(&error);
            assert!(expected.len() > room && expected.as_bytes().starts_with(&sink.written))
        }
        (Err(expected), Err(error)) => {
            check_error(&error);
            // The sink can fail before the canonicalization error.
            if !matches!(error, c14n::Error::Io(_)) {
                assert_eq!(expected.to_string(), error.to_string());
            }
        }
        (expected, result) => panic!("write {result:?} differs from to_string {expected:?}"),
    }
}

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
    let options = {
        let mut cfg = Options::new(mode);
        cfg.with_comments = selector & 4 != 0;
        cfg.nodes = nodes.as_deref();
        cfg.inclusive_prefixes = if selector & 128 != 0 { &PREFIXES } else { &[] };
        cfg
    };
    let result = c14n::to_string(tree, &options);
    if let Err(error) = &result {
        check_error(error);
    }
    let room = usize::from(selector).wrapping_mul(97) % 12_000;
    written(tree, &options, &result, room);
    Some(result)
}

pub fn exercise(input: &[u8]) {
    let Some((&selector, xml)) = input.split_first() else {
        return;
    };
    let Ok(owned) = OwnedDocument::parse_bytes(xml) else {
        if let Ok(parsed) = html::parse(xml, html::Options::new(html::NOERROR)) {
            let document = &parsed.document;
            canonical(document, document.root().id(), selector);
        }
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
