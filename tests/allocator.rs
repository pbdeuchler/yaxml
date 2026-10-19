//! Allocator-aware owned documents (`--features allocator_api`, Rust 1.100).
//!
//! `Allocator` is an unsafe trait, so these tests cannot define a counting or
//! capped allocator. They use `Global`, `System`, a borrowed `&System` and an
//! erased `Arc<dyn Allocator>`, and prove routing through allocator identity
//! and `Arc` reference counts. Allocation failure is covered by the crate's
//! private failpoint tests (src/owned/construction.rs).
#![cfg(feature = "allocator_api")]
#![allow(clippy::incompatible_msrv)]

use std::alloc::{Allocator, Global, System};
use std::sync::Arc;

use yaxml::options::{DTDVALID, NOBLANKS, NOXINCNODE, RECOVER, XINCLUDE};
use yaxml::xpath::{Context, XPath};
use yaxml::{
    EditError, EditErrorKind, EditLimits, ElementSpec, Error, ErrorKind, Limits, NameSpec,
    OwnedDocument, ParseOptions, PrefixPolicy, Writer, c14n,
};

type Shared = Arc<dyn Allocator + Send + Sync>;

/// What a parse produced, through the generic consumers: Writer output, an
/// XPath count, Canonical XML (or its error), recovered errors, DTD
/// validity diagnostics and the kept attribute references.
type Outcome = Result<(Vec<u8>, usize, String, usize, usize, Vec<String>), Error>;

fn outcome<A: Allocator>(document: Result<OwnedDocument<A>, Error>) -> Outcome {
    let document = document?;
    let mut writer = Writer::new(Vec::new());
    writer.write_owned_document(&document).expect("writes");
    let elements = XPath::compile("//*")
        .expect("compiles")
        .select_nodes(&document, document.root().id(), &Context::new())
        .expect("selects")
        .len();
    let canonical = c14n::to_string(&document, &c14n::Options::new(c14n::Mode::C14n10))
        .unwrap_or_else(|error| format!("{error:?}"));
    let unexpanded = document.nodes().flat_map(|node| {
        (0..node.attributes().len()).filter_map(move |at| node.unexpanded_attribute(at))
    });
    Ok((
        writer.into_inner(),
        elements,
        canonical,
        document.errors().len(),
        document.validity().len(),
        unexpanded.map(str::to_owned).collect(),
    ))
}

fn utf16le(text: &str) -> Vec<u8> {
    [0xFF, 0xFE]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
        .collect()
}

#[test]
fn parse_in_matches_parse_with_for_every_allocator() {
    let tiny = Limits {
        max_input_bytes: 4,
        ..Limits::default()
    };
    let cases: [(&str, Vec<u8>, u32, Limits, bool); 11] = [
        (
            "plain",
            b"<r a='1'>text<c/></r>".to_vec(),
            0,
            Limits::default(),
            true,
        ),
        (
            "namespaces",
            b"<p:r xmlns:p='urn:p' p:a='v'><p:c/>t</p:r>".to_vec(),
            0,
            Limits::default(),
            true,
        ),
        (
            "entity reference",
            b"<!DOCTYPE r [<!ENTITY e 'x<b/>'>]><r>&e;</r>".to_vec(),
            0,
            Limits::default(),
            true,
        ),
        (
            "attribute reference",
            b"<!DOCTYPE r [<!ENTITY e 'x'>]><r a='1&e;' b='2'><c d='&e;'/></r>".to_vec(),
            0,
            Limits::default(),
            true,
        ),
        (
            "validity diagnostics",
            b"<!DOCTYPE r [<!ELEMENT r EMPTY>]><r>bad</r>".to_vec(),
            DTDVALID,
            Limits::default(),
            true,
        ),
        (
            "noblanks edit",
            b"<r> <a/> </r>".to_vec(),
            NOBLANKS,
            Limits::default(),
            true,
        ),
        (
            "utf-16",
            utf16le("<r>\u{e9}</r>"),
            0,
            Limits::default(),
            true,
        ),
        (
            "recovered",
            b"<r><a></r>".to_vec(),
            RECOVER,
            Limits::default(),
            true,
        ),
        ("malformed", b"<r>".to_vec(), 0, Limits::default(), false),
        ("no root", b"<!--c-->".to_vec(), 0, Limits::default(), false),
        ("input limit", b"<r/><!---->".to_vec(), 0, tiny, false),
    ];
    let system = System;
    let shared: Shared = Arc::new(System);
    for (name, input, flags, limits, ok) in cases {
        let options = || ParseOptions::new(flags).with_limits(limits);
        let expected = outcome(OwnedDocument::parse_with(&input, options()));
        assert_eq!(expected.is_ok(), ok, "{name}: {expected:?}");
        let actual = [
            outcome(OwnedDocument::parse_in(&input, options(), Global)),
            outcome(OwnedDocument::parse_in(&input, options(), System)),
            outcome(OwnedDocument::parse_in(&input, options(), &system)),
            outcome(OwnedDocument::parse_in(&input, options(), shared.clone())),
        ];
        for (index, actual) in actual.into_iter().enumerate() {
            assert_eq!(actual, expected, "{name}, allocator {index}");
        }
    }
    assert_eq!(
        Arc::strong_count(&shared),
        1,
        "documents released their clones"
    );
}

#[test]
fn parse_in_rejects_xinclude_whose_tree_is_global() {
    let input = b"<r xmlns:xi='http://www.w3.org/2001/XInclude'/>";
    let unsupported = Err((ErrorKind::UnsupportedOption, 0));
    let cases = [
        (0, Ok(())),
        (XINCLUDE, unsupported),
        (XINCLUDE | NOXINCNODE, unsupported),
    ];
    let system = System;
    for (flags, expected) in cases {
        let options = || ParseOptions::new(flags);
        assert!(
            OwnedDocument::parse_with(input, options()).is_ok(),
            "{flags}"
        );
        let local = OwnedDocument::parse_in(input, options(), &system)
            .map(drop)
            .map_err(|error| (error.kind, error.offset));
        assert_eq!(local, expected, "{flags}");
        let global = OwnedDocument::parse_in(input, options(), Global)
            .map(drop)
            .map_err(|error| (error.kind, error.offset));
        assert_eq!(global, expected, "{flags}");
    }
}

#[test]
fn every_storage_vector_holds_the_supplied_allocator() {
    // One clone per allocator-backed vector: nodes, attributes and kept
    // attribute references.
    const HELD: usize = 3;
    let shared: Shared = Arc::new(System);
    let parsed = OwnedDocument::parse_in(b"<r a='1'/>", ParseOptions::default(), shared.clone())
        .expect("parses");
    assert!(Arc::ptr_eq(parsed.allocator(), &shared));
    assert_eq!(Arc::strong_count(&shared), 1 + HELD);
    let created = OwnedDocument::create_in(root("r"), EditLimits::default(), shared.clone())
        .expect("creates");
    assert_eq!(Arc::strong_count(&shared), 1 + 2 * HELD);
    drop((parsed, created));
    assert_eq!(Arc::strong_count(&shared), 1);

    let failed = OwnedDocument::parse_in(b"<r>", ParseOptions::default(), shared.clone());
    assert_eq!(failed.map(drop), OwnedDocument::parse("<r>").map(drop));
    assert_eq!(
        Arc::strong_count(&shared),
        1,
        "a failed parse keeps nothing"
    );

    let system = System;
    let borrowed =
        OwnedDocument::parse_in(b"<r/>", ParseOptions::default(), &system).expect("parses");
    assert!(std::ptr::eq(*borrowed.allocator(), &system));
}

fn root(local: &str) -> ElementSpec<'_> {
    ElementSpec {
        name: NameSpec {
            local,
            namespace: None,
            prefix: PrefixPolicy::Exact(None),
        },
        items: &[],
    }
}

/// Builds and edits one document, recording each edit's error, then writes it.
fn edited<A: Allocator + Clone>(allocator: A) -> (Vec<u8>, Vec<Option<EditErrorKind>>) {
    let limits = EditLimits::default();
    let mut document = OwnedDocument::create_in(root("r"), limits, allocator).expect("creates");
    let foreign = OwnedDocument::parse("<r/>")
        .expect("parses")
        .root_element()
        .id();
    let parent = document.root_element().id();
    let mut editor = document.edit(limits).expect("opens");
    let child = editor.append_element(parent, root("c")).expect("appends");
    let text = editor.append_text(child, "a & b").expect("appends");
    let gone = editor.append_comment(parent, "gone").expect("appends");
    let results = [
        editor.set_node_value(text, "x < y"),
        editor.remove_subtree(gone),
        editor.remove_subtree(gone),
        editor.append_text(foreign, "x").map(drop),
        editor.append_element(text, root("leaf")).map(drop),
        editor.remove_subtree(parent),
        editor.set_node_value(text, "\u{0}"),
    ];
    let errors = results
        .into_iter()
        .map(|result| result.err().map(|EditError { kind, .. }| kind))
        .collect();
    let mut writer = Writer::new(Vec::new());
    writer
        .write_owned_document(editor.document())
        .expect("writes");
    (writer.into_inner(), errors)
}

#[test]
fn created_documents_edit_alike_in_every_allocator() {
    let expected = (
        b"<r><c>x &lt; y</c></r>".to_vec(),
        vec![
            None,
            None,
            Some(EditErrorKind::InvalidHandle),
            Some(EditErrorKind::InvalidHandle),
            Some(EditErrorKind::WrongNodeKind),
            Some(EditErrorKind::ProtectedNode),
            Some(EditErrorKind::InvalidCharacter),
        ],
    );
    let system = System;
    let shared: Shared = Arc::new(System);
    assert_eq!(edited(Global), expected);
    assert_eq!(edited(System), expected);
    assert_eq!(edited(&system), expected);
    assert_eq!(edited(shared.clone()), expected);
    assert_eq!(Arc::strong_count(&shared), 1);
}

#[test]
fn create_in_reports_the_same_errors_as_create() {
    let cases = [
        (root("r"), EditLimits::default(), None),
        (
            root("1r"),
            EditLimits::default(),
            Some(EditErrorKind::InvalidName),
        ),
        (
            root("r"),
            EditLimits {
                max_created_nodes: 0,
                ..EditLimits::default()
            },
            Some(EditErrorKind::LimitExceeded(yaxml::EditLimit::CreatedNodes)),
        ),
    ];
    let system = System;
    for (spec, limits, expected) in cases {
        let kind = |result: Result<(), EditError>| result.err().map(|error| error.kind);
        assert_eq!(
            kind(OwnedDocument::create(spec, limits).map(drop)),
            expected
        );
        let local = OwnedDocument::create_in(spec, limits, &system).map(drop);
        assert_eq!(kind(local), expected);
    }
}

#[test]
fn erased_and_borrowed_documents_are_send_and_sync() {
    fn shareable<T: Send + Sync>() {}
    shareable::<OwnedDocument<Shared>>();
    shareable::<OwnedDocument<&'static System>>();
    shareable::<OwnedDocument<System>>();
}
