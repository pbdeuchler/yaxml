#![forbid(unsafe_code)]
//! `options::RECOVER`: both DOMs build what libxml2 2.15.4's
//! `XML_PARSE_RECOVER` builds and list the errors recovered from; limits,
//! loops and a document without an element stay errors.

use yaxml::ErrorKind::{self, *};
use yaxml::options::{NOENT, RECOVER};
use yaxml::{Document, Limit, Limits, OwnedDocument, ParseOptions, save};

/// Both DOMs' `save` output and recovered error kinds, or the error.
fn recover(input: &[u8], flags: u32) -> Result<(String, Vec<ErrorKind>), ErrorKind> {
    let options = || ParseOptions::new(RECOVER | flags);
    let saved = |bytes: Vec<u8>| String::from_utf8(bytes).unwrap();
    let owned = OwnedDocument::parse_with(input, options()).map_err(|error| error.kind);
    let owned = owned.map(|document| {
        let kinds = document.errors().iter().map(|error| error.kind).collect();
        (
            saved(save::to_vec(&document, &Default::default()).unwrap()),
            kinds,
        )
    });
    let borrowed = Document::parse_with(input, options()).map_err(|error| error.kind);
    let borrowed = borrowed.map(|document| {
        let kinds = document.errors().iter().map(|error| error.kind).collect();
        (
            saved(save::to_vec(&document, &Default::default()).unwrap()),
            kinds,
        )
    });
    assert_eq!(owned, borrowed, "DOMs disagree on {input:?}");
    owned
}

const DECL: &str = "<?xml version=\"1.0\"?>\n";

#[rustfmt::skip]
const CASES: &[(&str, &str, &[ErrorKind])] = &[
    // well-formed input recovers nothing
    ("<r a='1'><b/>t&amp;</r>", "<r a=\"1\"><b/>t&amp;</r>", &[]),
    // end tags close the open element, whatever they name
    ("<r><a></b>x</a></r>", "<r><a/>x</r>", &[MismatchedEndTag; 3]),
    ("<r><a><b></a>t</r>", "<r><a><b/>t</a></r>", &[MismatchedEndTag, MismatchedEndTag, UnexpectedEof]),
    ("<r><a></a b>t</r>", "<r><a/>t</r>", &[Syntax]),
    // the end of input closes what is open, a cut-off start tag included
    ("<r><a>text", "<r><a>text</a></r>", &[UnexpectedEof]),
    ("<r><x", "<r><x/></r>", &[UnexpectedEof; 2]),
    // a malformed attribute ends the element there
    ("<r a=\"1\"b=\"2\"><c/></r>", "<r a=\"1\"/>", &[Syntax, MultipleRoots]),
    ("<r><a b=1>x</a></r>", "<r><a/>x</r>", &[Syntax, MismatchedEndTag, MismatchedEndTag]),
    ("<A@/>", "<A/>", &[InvalidName, Syntax]),
    ("<r><a b=\"x<y\"/></r>", "<r><a b=\"x&lt;y\"/></r>", &[Syntax]),
    // duplicates are kept, as libxml2 keeps them
    ("<r a=\"1\" a=\"2\"/>", "<r a=\"1\" a=\"2\"/>", &[DuplicateAttribute]),
    // ... but only the first declaration of a prefix
    ("<r xmlns:p=\"a\" xmlns:p=\"b\"><p:x/></r>", "<r xmlns:p=\"a\"><p:x/></r>", &[DuplicateAttribute]),
    ("<!DOCTYPE r []><r xmlns=\"a\" xmlns=\"b\"/>", "<!DOCTYPE r>\n<r xmlns=\"a\"/>", &[DuplicateAttribute]),
    ("<r xmlns:p=\"u\" p:a=\"1\" p:a=\"2\"/>", "<r xmlns:p=\"u\" p:a=\"1\" p:a=\"2\"/>", &[DuplicateAttribute]),
    // unbound prefixes leave names in no namespace; unbindable declarations go
    ("<p:r/>", "<p:r/>", &[Namespace]),
    ("<r><p:a q:b=\"1\" b=\"2\"/></r>", "<r><p:a q:b=\"1\" b=\"2\"/></r>", &[Namespace, Namespace]),
    ("<r><a:b xmlns:a=\"\"/></r>", "<r><a:b/></r>", &[Namespace, Namespace]),
    ("<r xmlns:xml=\"u\"/>", "<r/>", &[Namespace]),
    ("<r><a:b:c/></r>", "<r><a:b:c/></r>", &[InvalidName]),
    // references: dropped as far as libxml2 reads them, U+FFFD for bad characters
    ("<r>a & b &x c</r>", "<r>a  b  c</r>", &[InvalidReference, InvalidReference]),
    ("<r>&#x4G;</r>", "<r>G;</r>", &[InvalidReference]),
    ("<r>&#0;x&#1;y&#x110000;</r>", "<r>x&#xFFFD;y&#xFFFD;</r>", &[InvalidReference; 3]),
    ("<r a=\"&#xfffe;\"/>", "<r a=\"&#xFFFD;\"/>", &[InvalidReference]),
    ("<r>&undeclared;</r>", "<r/>", &[UnknownEntity]),
    ("<!DOCTYPE r []><r>&undeclared;</r>", "<!DOCTYPE r>\n<r>&undeclared;</r>", &[UnknownEntity]),
    ("<!DOCTYPE r [<!ENTITY e SYSTEM \"x\" NDATA n>]><r>&e;</r>", "<!DOCTYPE r [\n<!ENTITY e SYSTEM \"x\" NDATA n>\n]>\n<r>&e;</r>", &[UnparsedEntity]),
    ("<!DOCTYPE r [<!ENTITY e SYSTEM \"x\">]><r a=\"&e;\"/>", "<!DOCTYPE r [\n<!ENTITY e SYSTEM \"x\">\n]>\n<r a=\"\"/>", &[ExternalEntityInAttribute]),
    // text, comments and processing instructions are kept
    ("<r>]]></r>", "<r>]]&gt;</r>", &[Syntax]),
    ("<r><!-- a -- b --></r>", "<r><!-- a -- b --></r>", &[Syntax]),
    ("<r><?xml version=\"1.0\"?><?XML x?></r>", "<r><?xml version=\"1.0\"?><?XML x?></r>", &[InvalidDeclaration; 2]),
    ("<r><?pi+data?></r>", "<r><?pi +data?></r>", &[InvalidName]),
    // markup that starts nothing is text after `<` (or `<?`)
    ("<r><?+x?>y</r>", "<r>+x?&gt;y</r>", &[InvalidName]),
    ("<r><!foo>x</r>", "<r>!foo&gt;x</r>", &[Syntax]),
    ("<r><!DOCTYPE x></r>", "<r>!DOCTYPE x&gt;</r>", &[Syntax]),
    ("<!DOCTYPE r []><r><!ELEMENT x></r>", "<!DOCTYPE r>\n<r>!ELEMENT x&gt;</r>", &[Syntax]),
    // parsing stops at content after the root, keeping comments before it
    ("<r>x</r>tail<s/>", "<r>x</r>", &[TextOutsideRoot]),
    ("<r/><!-- c --><s/>", "<r/>\n<!-- c -->", &[MultipleRoots]),
    // a declaration keeps what it can
    ("<?xml version=\"1.0\" standalone=\"maybe\"?><r/>", "<r/>", &[InvalidDeclaration]),
    ("<?xml version=\"1.0\" encoding=\"bogus\"?><r/>", "<r/>", &[UnsupportedEncoding]),
    // the valid prefix before an invalid character or UTF-8 sequence is kept
    ("<r>a\u{1}b</r>", "<r>a</r>", &[InvalidCharacter]),
    ("<r a='\u{1}'/>", "<r/>", &[UnexpectedEof, UnexpectedEof, InvalidCharacter]),
];

#[test]
fn recovery_builds_libxml2s_tree_and_lists_the_errors() {
    for &(input, saved, errors) in CASES {
        let expected = (format!("{DECL}{saved}\n"), errors.to_vec());
        assert_eq!(recover(input.as_bytes(), 0), Ok(expected), "{input}");
        let strict = Document::parse(input).map(drop).map_err(|error| error.kind);
        assert_eq!(strict.is_ok(), errors.is_empty(), "{input}");
        if let Err(kind) = strict {
            assert!(errors.contains(&kind), "strict error of {input}: {kind:?}");
        }
    }
}

#[test]
fn recovery_reads_invalid_utf8_up_to_the_sequence() {
    let input = b"<r>ab\xffcd</r>";
    let expected = format!("{DECL}<r>ab</r>\n");
    assert_eq!(recover(input, 0), Ok((expected, vec![InvalidUtf8])));
}

#[test]
fn recovery_decodes_utf16_like_utf8() {
    // Only the slice reader passes a start tag cut off by the end of input,
    // and keeps text up to an invalid character rather than up to its token.
    let decoded = |case: &&(&str, &str, &[ErrorKind])| {
        case.0.is_ascii() && case.0 != "<r><x" && !case.2.contains(&InvalidCharacter)
    };
    let options = save::Options {
        encoding: Some("UTF-8"),
        ..Default::default()
    };
    for &(input, _, errors) in CASES.iter().filter(decoded) {
        let utf16: Vec<u8> = "\u{feff}"
            .encode_utf16()
            .chain(input.encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect();
        let [utf8, utf16] = [input.as_bytes(), &utf16]
            .map(|bytes| OwnedDocument::parse_with(bytes, ParseOptions::new(RECOVER)).unwrap());
        let kinds: Vec<_> = utf16.errors().iter().map(|error| error.kind).collect();
        assert_eq!(kinds, errors, "{input}");
        let [utf8, utf16] = [utf8, utf16].map(|document| save::to_vec(&document, &options));
        assert_eq!(utf8, utf16, "{input}");
    }
}

#[test]
fn recovery_without_an_element_fails_with_the_first_error() {
    // (input, error)
    let cases: &[(&str, ErrorKind)] = &[
        ("", MissingRoot),
        ("text", TextOutsideRoot),
        ("<!-- c -->", MissingRoot),
        ("<!DOCTYPEdoc><doc/>", InvalidDtd),
        ("<?xml VERSION=\"1.0\"?>x<r/>", InvalidDeclaration),
        ("<1/>", InvalidName),
    ];
    for &(input, error) in cases {
        assert_eq!(recover(input.as_bytes(), 0), Err(error), "{input}");
    }
}

#[test]
fn resource_limits_and_loops_stay_fatal() {
    let laughs = "<!DOCTYPE r [<!ENTITY a 'aaaaaaaaaa'><!ENTITY b '&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;'>\
        <!ENTITY c '&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;'><!ENTITY d '&c;&c;&c;&c;&c;&c;&c;&c;&c;&c;'>\
        <!ENTITY e '&d;&d;&d;&d;&d;&d;&d;&d;&d;&d;'><!ENTITY f '&e;&e;&e;&e;&e;&e;&e;&e;&e;&e;'>]><r>&f;</r>";
    let deep = "<a>".repeat(300);
    let attributes = format!(
        "<r {}/>",
        (0..5).map(|i| format!("a{i}='' ")).collect::<String>()
    );
    let limits = Limits {
        max_attributes: 4,
        max_events: 3,
        ..Limits::default()
    };
    // (input, limits, error)
    let cases = [
        (
            laughs,
            Limits::default(),
            LimitExceeded(Limit::EntityExpansion),
        ),
        (
            "<!DOCTYPE r [<!ENTITY a '&a;'>]><r>&a;</r>",
            Limits::default(),
            EntityLoop,
        ),
        (&deep, Limits::default(), LimitExceeded(Limit::Depth)),
        (&attributes, limits, LimitExceeded(Limit::Attributes)),
        (
            "<r><a></b><c></d></r>",
            limits,
            LimitExceeded(Limit::Events),
        ),
    ];
    for (input, limits, error) in cases {
        for flags in [RECOVER, RECOVER | NOENT] {
            let options = || ParseOptions::new(flags).with_limits(limits);
            let borrowed = Document::parse_with(input.as_bytes(), options()).map(drop);
            assert_eq!(borrowed.map_err(|e| e.kind), Err(error), "{input}");
            let owned = OwnedDocument::parse_with(input.as_bytes(), options()).map(drop);
            assert_eq!(owned.map_err(|e| e.kind), Err(error), "{input}");
        }
    }
}

#[test]
fn recovery_lists_at_most_100_errors_and_keeps_parsing() {
    let input = format!("<r>{}<end/></r>", "<a></b>".repeat(150));
    let document = Document::parse_with(input.as_bytes(), ParseOptions::new(RECOVER)).unwrap();
    assert_eq!(document.errors().len(), 100);
    let last = document.root_element().last_child().unwrap();
    assert_eq!(last.name().unwrap().local(), "end");
}

#[test]
fn recovered_errors_survive_xinclude_and_validation() {
    use yaxml::options::{DTDVALID, XINCLUDE};
    let input = b"<!DOCTYPE r [<!ELEMENT r EMPTY>]><r><a></b></r>";
    // (flags, validity diagnostics expected)
    for (flags, invalid) in [(XINCLUDE, false), (DTDVALID, true)] {
        let document =
            OwnedDocument::parse_with(input, ParseOptions::new(RECOVER | flags)).unwrap();
        let kinds: Vec<_> = document.errors().iter().map(|error| error.kind).collect();
        assert_eq!(kinds, [MismatchedEndTag]);
        assert_eq!(!document.validity().is_empty(), invalid);
    }
}
