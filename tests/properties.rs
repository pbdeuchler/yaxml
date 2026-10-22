//! `properties()` against libxml2 2.15.4's `doc->properties`, recorded with
//! `xmlReadMemory` (url `file:///tmp/x.xml`, nothing beside it).

use yaxml::options::{DTDVALID, RECOVER};
use yaxml::{
    Document, EditLimits, ElementSpec, NameSpec, OwnedDocument, ParseOptions, PrefixPolicy,
};

const EMPTY: &str = "<!DOCTYPE r [<!ELEMENT r EMPTY>]>";

#[test]
fn parsed_properties_match_libxml2() {
    let invalid = format!("{EMPTY}<r>x</r>");
    let valid = format!("{EMPTY}<r/>");
    let ns_errors = "<r>".to_owned() + &"<a:b/>".repeat(120) + "<c></r>";
    let cases: &[(u32, &str, Option<u32>)] = &[
        (0, "<r/>", Some(11)),
        (RECOVER, "<r/>", Some(11)),
        (0, "<a:r/>", Some(9)),
        (RECOVER, "<a:r/>", Some(9)),
        (0, r#"<r xmlns="rel"/>"#, Some(11)),
        (0, r#"<r xmlns:p="a b"/>"#, Some(9)),
        (0, r#"<r xmlns:p=""/>"#, Some(9)),
        (0, r#"<r a="1" a:b="2"/>"#, Some(9)),
        (0, r#"<?xml version="1.1"?><r xml:space="bad"/>"#, Some(11)),
        (0, &invalid, Some(11)),
        (DTDVALID, &invalid, Some(3)),
        (DTDVALID | RECOVER, &invalid, Some(3)),
        (DTDVALID, &valid, Some(11)),
        (DTDVALID, "<r/>", Some(3)),
        (0, r#"<!DOCTYPE r SYSTEM "missing.dtd"><r>&e;</r>"#, Some(3)),
        (
            0,
            r#"<!DOCTYPE r [<!ENTITY % p SYSTEM "m.dtd"> %p;]><r>&e;</r>"#,
            Some(3),
        ),
        (
            0,
            "<!DOCTYPE r [<!ELEMENT r EMPTY><!ELEMENT r ANY>]><r/>",
            Some(3),
        ),
        (
            0,
            r#"<!DOCTYPE r [<!NOTATION n SYSTEM "a"><!NOTATION n SYSTEM "b">]><r/>"#,
            Some(3),
        ),
        (
            0,
            r#"<!DOCTYPE r [<!ATTLIST r a CDATA "1"><!ATTLIST r a CDATA "2">]><r/>"#,
            Some(11),
        ),
        (
            0,
            r#"<!DOCTYPE r [<!ENTITY e "1"><!ENTITY e "2">]><r/>"#,
            Some(11),
        ),
        (0, "<r>&e;</r>", None),
        (RECOVER, "<r>&e;</r>", Some(0)),
        (RECOVER, "<r><a></r>", Some(0)),
        // The first error to clear a bit is kept past the 100-error cap.
        (RECOVER, &ns_errors, Some(0)),
    ];
    for &(flags, input, expected) in cases {
        let options = || ParseOptions::new(flags).base(Some("file:///tmp/x.xml".into()));
        let borrowed = Document::parse_with(input.as_bytes(), options()).map(|d| d.properties());
        let owned = OwnedDocument::parse_with(input.as_bytes(), options()).map(|d| d.properties());
        assert_eq!(borrowed.ok(), expected, "{flags} {input}");
        assert_eq!(owned.ok(), expected, "{flags} {input}: owned");
    }
}

#[test]
fn built_and_html_documents_match_libxml2() {
    let root = ElementSpec {
        name: NameSpec {
            local: "r",
            namespace: None,
            prefix: PrefixPolicy::Automatic,
        },
        items: &[],
    };
    let created = OwnedDocument::create(root, EditLimits::DEFAULT).unwrap();
    assert_eq!(created.properties(), 32);
    // xmlCopyDoc builds the copy with xmlNewDoc.
    let parsed = OwnedDocument::parse("<r/>").unwrap();
    assert_eq!(parsed.try_clone().unwrap().properties(), 32);
    #[cfg(feature = "html")]
    for input in ["<p>x", "<p>x</q>"] {
        let html = yaxml::html::parse(input.as_bytes(), Default::default())
            .unwrap()
            .document;
        assert_eq!(html.properties(), 128, "{input}");
        assert_eq!(html.try_clone().unwrap().properties(), 32, "{input}");
    }
}
