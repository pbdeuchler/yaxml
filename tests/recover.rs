#![forbid(unsafe_code)]
//! `options::RECOVER`: both DOMs build what libxml2 2.15.4's
//! `XML_PARSE_RECOVER` builds and list the errors recovered from; limits,
//! loops and a document without an element stay errors.

use std::ops::ControlFlow;
use yaxml::ErrorKind::{self, *};

const RELATIVE: ErrorKind = Warning(yaxml::Warning::RelativeNamespaceUri);
use yaxml::options::{NOENT, RECOVER};

use yaxml::{
    BufferedReader, Document, Limit, Limits, OwnedDocument, ParseOptions, PushParser, Reader,
    TextReader, save,
};

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
    // the end of input closes what is open
    ("<r><a>text", "<r><a>text</a></r>", &[UnexpectedEof]),
    ("<r><a b=\"x<y\"/></r>", "<r><a b=\"x&lt;y\"/></r>", &[Syntax]),
    // ... declared too, where only replacement text's `<` empties a value
    ("<!DOCTYPE r [<!ATTLIST r a CDATA #IMPLIED b NMTOKEN #IMPLIED>]><r a=\"x<y\" b=\" p<q \"/>",
     "<!DOCTYPE r [\n<!ATTLIST r a CDATA #IMPLIED>\n<!ATTLIST r b NMTOKEN #IMPLIED>\n]>\n<r a=\"x&lt;y\" b=\"p&lt;q\"/>", &[Syntax; 2]),
    ("<!DOCTYPE p:r [<!ATTLIST p:r xmlns:p CDATA #FIXED \"u\">]><p:r xmlns:p=\"a<b\"/>",
     "<!DOCTYPE p:r [\n<!ATTLIST p:r xmlns:p CDATA #FIXED \"u\">\n]>\n<p:r xmlns:p=\"a&lt;b\"/>", &[Syntax, Warning(yaxml::Warning::NamespaceUri)]),
    ("<!DOCTYPE r [<!ENTITY e \"&#60;\"><!ATTLIST r a CDATA #IMPLIED>]><r a=\"x&e;\"/>",
     "<!DOCTYPE r [\n<!ENTITY e \"&#60;\">\n<!ATTLIST r a CDATA #IMPLIED>\n]>\n<r a=\"\"/>", &[Syntax]),
    // duplicates are kept, as libxml2 keeps them
    ("<r a=\"1\" a=\"2\"/>", "<r a=\"1\" a=\"2\"/>", &[DuplicateAttribute]),
    // ... but only the first declaration of a prefix
    ("<r xmlns:p=\"a\" xmlns:p=\"b\"><p:x/></r>", "<r xmlns:p=\"a\"><p:x/></r>", &[DuplicateAttribute]),
    ("<!DOCTYPE r []><r xmlns=\"a\" xmlns=\"b\"/>", "<!DOCTYPE r>\n<r xmlns=\"a\"/>", &[RELATIVE, RELATIVE, DuplicateAttribute]),
    ("<r xmlns:p=\"u\" p:a=\"1\" p:a=\"2\"/>", "<r xmlns:p=\"u\" p:a=\"1\" p:a=\"2\"/>", &[DuplicateAttribute]),
    // unbound prefixes leave names in no namespace; unbindable declarations go
    ("<p:r/>", "<p:r/>", &[Namespace]),
    ("<r><p:a q:b=\"1\" b=\"2\"/></r>", "<r><p:a q:b=\"1\" b=\"2\"/></r>", &[Namespace, Namespace]),
    ("<r><a:b xmlns:a=\"\"/></r>", "<r><a:b/></r>", &[Namespace, Namespace]),
    ("<r xmlns:xml=\"u\"/>", "<r/>", &[Namespace]),
    ("<r><a:b:c/></r>", "<r><a:b:c/></r>", &[Namespace]),
    // references: dropped as far as libxml2 reads them, U+FFFD for bad characters
    ("<r>a & b &x c</r>", "<r>a  b  c</r>", &[InvalidReference, InvalidReference]),
    ("<r>&#x4G;</r>", "<r>G;</r>", &[InvalidReference]),
    ("<r>&#0;x&#1;y&#x110000;</r>", "<r>x&#xFFFD;y&#xFFFD;</r>", &[InvalidReference; 3]),
    ("<r a=\"&#xfffe;\"/>", "<r a=\"&#xFFFD;\"/>", &[InvalidReference]),
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
    // ... up to the first pseudo-attribute it rejects, skipping the rest
    ("<?xml version=\"1.0\" encoding=\"1.0\" encoding=\"UTF-16\"?><r/>", "<r/>", &[InvalidDeclaration]),
    ("<?xml version=\"1.0\" x=\"1\" standalone=\"maybe\"?><r/>", "<r/>", &[InvalidDeclaration]),
    // the DTD reads on where libxml2 requires a space but does not stop
    ("<!DOCTYPEr><r/>", "<!DOCTYPE r>\n<r/>", &[InvalidDtd]),
    ("<!DOCTYPE r [<!ELEMENT r(#PCDATA)><!ENTITY e SYSTEM \"x\"NDATA n><!ATTLIST r a CDATA #FIXED\"v\">]><r/>",
     "<!DOCTYPE r [\n<!ELEMENT r (#PCDATA)>\n<!ENTITY e SYSTEM \"x\" NDATA n>\n<!ATTLIST r a CDATA #FIXED \"v\">\n]>\n<r/>", &[InvalidDtd; 3]),
    ("<!DOCTYPE r [<!ENTITY%p\"x\"><!ENTITY e PUBLIC\"p\"\"s\">]><r/>",
     "<!DOCTYPE r [\n<!ENTITY % p \"x\">\n<!ENTITY e PUBLIC \"p\" \"s\">\n]>\n<r/>", &[InvalidDtd; 5]),
    // ... and past an entity's fragment ("Fragment not allowed"): a general
    // entity is declared, a parameter entity is not; a DOCTYPE's is no error
    ("<!DOCTYPE r [<!ENTITY e SYSTEM \"x#y\">]><r>t</r>", "<!DOCTYPE r [\n<!ENTITY e SYSTEM \"x#y\">\n]>\n<r>t</r>", &[InvalidDtd]),
    ("<!DOCTYPE r [<!ENTITY e PUBLIC \"p#\" \"x#y\" NDATA n><!NOTATION n SYSTEM \"q#r\">]><r/>",
     "<!DOCTYPE r [\n<!NOTATION n SYSTEM \"q#r\" >\n<!ENTITY e PUBLIC \"p#\" \"x#y\" NDATA n>\n]>\n<r/>", &[InvalidDtd]),
    ("<!DOCTYPE r [<!ENTITY % p SYSTEM \"x#y\">%p;<!ELEMENT r ANY>]><r/>",
     "<!DOCTYPE r [\n<!ELEMENT r ANY>\n]>\n<r/>", &[InvalidDtd, Warning(yaxml::Warning::UndeclaredEntity { error: false })]),
    ("<!DOCTYPE r SYSTEM \"x#y\"><r/>", "<!DOCTYPE r SYSTEM \"x#y\">\n<r/>", &[]),
    // a malformed header is reread from the input: the quote closing a
    // public literal can open the system one, past the scanner's `>`
    ("<!DOCTYPE r PUBLIC \"p\t\" a>\"><r/>", "<!DOCTYPE r PUBLIC \"p\" \" a>\">\n<r/>", &[InvalidDtd]),
    // ... else the DOCTYPE keeps the declarations before the error, and the
    // document resumes there: at markup that is no declaration
    ("<!DOCTYPE r [<r/>", "<!DOCTYPE r>\n<r/>", &[InvalidDtd]),
    // replacement text recovers as physical input does
    ("<!DOCTYPE r [<!ENTITY e \"<a b='&#38;u;'/>\">]><r>&e;</r>",
     "<!DOCTYPE r [\n<!ENTITY e \"<a b='&#38;u;'/>\">\n]>\n<r>&e;</r>", &[UnknownEntity]),
    ("<!DOCTYPE r [<!ENTITY e \"<p:a></p:a>\">]><r>&e;</r>",
     "<!DOCTYPE r [\n<!ENTITY e \"<p:a></p:a>\">\n]>\n<r>&e;</r>", &[Namespace]),
];

/// Repairs only the slice reader makes (UTF-8 input without a DOCTYPE):
/// (input, saved after the declaration, errors).
#[rustfmt::skip]
const SLICE: &[(&str, &str, &[ErrorKind])] = &[
    // a start tag cut off by the end of input is an empty element
    ("<r><x", "<r><x/></r>", &[UnexpectedEof; 2]),
    // a malformed attribute ends the element there, and libxml2 reads on
    // from the error, the rest of the tag as text
    ("<r a=\"1\"b=\"2\"><c/></r>", "<r a=\"1\"/>", &[Syntax, TextOutsideRoot]),
    ("<r><a b=1>x</a></r>", "<r><a/>1&gt;x</r>", &[Syntax, MismatchedEndTag, MismatchedEndTag]),
    ("<A@/>", "<A/>", &[InvalidName, Syntax, TextOutsideRoot]),
    // markup that starts nothing loses its `<` and is reread from there
    ("<r>1 < 2</r>", "<r>1  2</r>", &[InvalidName]),
    // ... as is one with a character reference libxml2 reads as zero
    ("<r><a b=\"x&#0;y\" c=\"1\">t</a></r>", "<r><a/>y\" c=\"1\"&gt;t</r>", &[InvalidReference, MismatchedEndTag, MismatchedEndTag]),
    ("<r><a b=\"x&#12a;y\">t</a></r>", "<r><a/>a;y\"&gt;t</r>", &[InvalidReference, MismatchedEndTag, MismatchedEndTag]),
    // a character outside `Char` is dropped from text, U+FFFD in a value,
    // and ends a comment, PI or CDATA section, read on from as text
    ("<r>a\u{1}b</r>", "<r>ab</r>", &[InvalidCharacter]),
    ("<r a='x\u{1}y'/>", "<r a=\"x&#xFFFD;y\"/>", &[InvalidCharacter]),
    ("<r><!-- a\u{c} b --></r>", "<r> b --&gt;</r>", &[InvalidCharacter; 2]),
    ("<r><?pi a\u{c} b?></r>", "<r> b?&gt;</r>", &[InvalidCharacter; 2]),
    ("<r><![CDATA[x\u{2}y]]>z</r>", "<r>y]]&gt;z</r>", &[InvalidCharacter, Syntax, InvalidCharacter]),
    // an undeclared entity's reference is kept, empty
    ("<r>a&x;b&#65;</r>", "<r>a&x;bA</r>", &[UnknownEntity]),
];

/// Inputs from which, as in libxml2, no element is recovered: (input, saved
/// after the declaration, errors).
#[rustfmt::skip]
const ROOTLESS: &[(&str, &str, &[ErrorKind])] = &[
    ("text", "", &[TextOutsideRoot]),
    ("<!-- c -->", "<!-- c -->\n", &[MissingRoot]),
    ("<?xml VERSION=\"1.0\"?>x<r/>", "", &[InvalidDeclaration, InvalidDeclaration, TextOutsideRoot]),
    ("<1/>", "", &[InvalidName]),
    // a DTD error ends the DOCTYPE, which keeps the declarations before it,
    // and the document resumes where libxml2's does
    ("<!DOCTYPE .r><r/>", "<!DOCTYPE >\n", &[InvalidName, TextOutsideRoot]),
    ("<!DOCTYPE r PUBLIC \"[\" \"x\"><r/>", "<!DOCTYPE r PUBLIC \"\" \"\">\n", &[InvalidDtd, TextOutsideRoot]),
    // ... or one running to the end of the input
    ("<!DOCTYPE r PUBLIC \"p\t\" a><r/>", "<!DOCTYPE r PUBLIC \"p\" \" a><r/>\">\n", &[InvalidDtd, MissingRoot]),
    ("<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r a NAME #IMPLIED><!ELEMENT s ANY>]><r/>",
     "<!DOCTYPE r [\n<!ELEMENT r EMPTY>\n]>\n", &[InvalidDtd, TextOutsideRoot]),
    ("<!DOCTYPE r [<!ENTITY e 'x'><!ELEMENT r ANY>&e;]><r/>",
     "<!DOCTYPE r [\n<!ENTITY e \"x\">\n<!ELEMENT r ANY>\n]>\n", &[InvalidDtd, TextOutsideRoot]),
    // ... with what libxml2 declares before an error in a declaration: an
    // entity whose value it read, the attribute definitions it read
    ("<!DOCTYPE r [<!ENTITY e \"w\" -- c --><!ELEMENT r ANY>]><r/>",
     "<!DOCTYPE r [\n<!ENTITY e \"w\">\n]>\n", &[InvalidDtd, TextOutsideRoot]),
    ("<!DOCTYPE r [<!ATTLIST r a CDATA #REQUIRED \"v\" b CDATA #IMPLIED>]><r/>",
     "<!DOCTYPE r [\n<!ATTLIST r a CDATA #REQUIRED>\n]>\n", &[InvalidName, TextOutsideRoot]),
    ("<!DOCTYPE r [<!ATTLIST r a CDATA #REQUIRED b NAME #IMPLIED>]><r/>",
     "<!DOCTYPE r [\n<!ATTLIST r a CDATA #REQUIRED>\n]>\n", &[InvalidDtd, TextOutsideRoot]),
    // ... but not an attribute default holding `<`
    ("<!DOCTYPE r [<!ATTLIST r a CDATA \"x<y\">]><r/>", "<!DOCTYPE r>\n", &[InvalidDtd, UnexpectedEof]),
    // ... nor an element it did not read to its end
    ("<!DOCTYPE r [<!ELEMENT r ANY x>]><r/>", "<!DOCTYPE r>\n", &[InvalidDtd, TextOutsideRoot]),
];

#[test]
fn recovery_builds_libxml2s_tree_and_lists_the_errors() {
    for &(input, saved, errors) in CASES.iter().chain(SLICE) {
        let expected = (format!("{DECL}{saved}\n"), errors.to_vec());
        assert_eq!(recover(input.as_bytes(), 0), Ok(expected), "{input}");
        // Strict parsing keeps namespace errors too, and stops at any other.
        let strict = Document::parse(input).map_err(|error| error.kind);
        let namespace = errors.iter().all(|kind| *kind == Namespace);
        assert_eq!(strict.is_ok(), namespace, "{input}");
        match strict {
            Ok(document) => {
                let kinds: Vec<_> = document.errors().iter().map(|error| error.kind).collect();
                assert_eq!(kinds, errors, "{input}");
            }
            Err(kind) => assert!(errors.contains(&kind), "strict error of {input}: {kind:?}"),
        }
    }
}

#[test]
fn a_malformed_start_tag_keeps_its_attlist_defaults() {
    use yaxml::options::DTDATTR;
    // libxml2 names an element by its longest Name and defaults it from
    // that name's ATTLIST, however the tag goes on.
    #[rustfmt::skip]
    let cases = [
        ("<!DOCTYPE r [<!ATTLIST r a CDATA 'x'>]><r/>", "<r a=\"x\"/>"),
        ("<!DOCTYPE r [<!ATTLIST r a CDATA 'x'>]><r'x'>", "<r a=\"x\"/>"),
        ("<!DOCTYPE r [<!ATTLIST r a CDATA 'x'><!ATTLIST b<r'x'>", "<r a=\"x\"/>"),
        ("<!DOCTYPE q [<!ATTLIST r a CDATA 'x'>]><q'x'>", "<q/>"),
        ("<!DOCTYPE \u{e9} [<!ATTLIST \u{e9} a CDATA 'x'>]><\u{e9}/>", "<\u{e9} a=\"x\"/>"),
        ("<!DOCTYPE \u{e9} [<!ATTLIST \u{e9} a CDATA 'x'>]><\u{e9}'x'>", "<\u{e9} a=\"x\"/>"),
    ];
    for (input, root) in cases {
        let (saved, _) = recover(input.as_bytes(), DTDATTR).unwrap();
        assert_eq!(saved.lines().last(), Some(root), "{input}");
    }
}

#[test]
fn recovery_reads_invalid_utf8_as_libxml2() {
    // (input, owned DOM saved after the declaration): libxml2 reads each
    // byte that starts no UTF-8 sequence as U+FFFD, also inside a name.
    #[rustfmt::skip]
    let cases: &[(&[u8], &str)] = &[
        (b"<r>ab\xffcd</r>", "<r>ab&#xFFFD;cd</r>"),
        (b"<r a=\"x\xffy\"><![CDATA[\xc3]]><!--\xed\xa0\x80--></r>",
         "<r a=\"x&#xFFFD;y\"><![CDATA[\u{fffd}]]><!--\u{fffd}\u{fffd}\u{fffd}--></r>"),
        (b"<r>\xf0\x90\x80x</r>", "<r>&#xFFFD;&#xFFFD;&#xFFFD;x</r>"),
        (b"<r\xff/>", "<r\u{fffd}/>"),
    ];
    let options = || ParseOptions::new(RECOVER);
    for &(input, saved) in cases {
        let owned = OwnedDocument::parse_with(input, options()).unwrap();
        let kinds: Vec<_> = owned.errors().iter().map(|error| error.kind).collect();
        assert_eq!(kinds, [InvalidUtf8], "{input:?}");
        let bytes = save::to_vec(&owned, &Default::default()).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            format!("{DECL}{saved}\n")
        );
    }
    // The borrowed DOM, which borrows its text, keeps what precedes it.
    let borrowed = Document::parse_with(b"<r>ab\xffcd</r>", options()).unwrap();
    let saved = save::to_vec(&borrowed, &Default::default()).unwrap();
    assert_eq!(
        String::from_utf8(saved).unwrap(),
        format!("{DECL}<r>ab</r>\n")
    );
    assert_eq!(borrowed.errors()[0].kind, InvalidUtf8);
    // Without RECOVER, it is an error.
    let strict = OwnedDocument::parse_bytes(b"<r>ab\xffcd</r>").map(drop);
    assert_eq!(strict.map_err(|error| error.kind), Err(InvalidUtf8));
}

#[test]
fn recovery_decodes_utf16_like_utf8() {
    let decoded = |case: &&(&str, &str, &[ErrorKind])| case.0.is_ascii();
    let options = save::Options::DEFAULT.encoding(Some("UTF-8"));
    for &(input, _, errors) in CASES.iter().chain(SLICE).chain(ROOTLESS).filter(decoded) {
        let utf16: Vec<u8> = "\u{feff}"
            .encode_utf16()
            .chain(input.encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect();
        let [utf8, utf16] = [input.as_bytes(), &utf16]
            .map(|bytes| OwnedDocument::parse_with(bytes, ParseOptions::new(RECOVER)).unwrap());
        // A BOM-detected encoding outranks the label, so UTF-16 never
        // reports a label it cannot switch to, but warns about any other.
        let mismatch = Warning(yaxml::Warning::EncodingMismatch);
        let kinds: Vec<_> = (utf16.errors().iter().map(|error| error.kind))
            .filter(|kind| *kind != mismatch)
            .collect();
        let errors: Vec<_> = errors
            .iter()
            .filter(|k| **k != UnsupportedEncoding)
            .collect();
        assert_eq!(kinds.iter().collect::<Vec<_>>(), errors, "{input}");
        let [utf8, utf16] = [utf8, utf16].map(|document| save::to_vec(&document, &options));
        assert_eq!(utf8, utf16, "{input}");
    }
}

#[test]
fn a_rootless_document_adopts_its_first_element() {
    use yaxml::{EditErrorKind, EditLimits};
    let source = OwnedDocument::parse("<r><a/></r>").unwrap();
    let element = source.root_element().id();
    let mut document = OwnedDocument::parse_with(b"<!--c-->", ParseOptions::new(RECOVER)).unwrap();
    let (root, comment) = (
        document.root().id(),
        document.root().first_child().unwrap().id(),
    );
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let first = editor.import(&source, element, true).unwrap();
    assert_eq!(editor.insert_before(comment, first), Ok(first));
    assert_eq!(
        editor.document().document_element().map(|node| node.id()),
        Some(first)
    );
    // ... but only one: libxml2's xmlAddChild would add a second
    let second = editor.import(&source, element, false).unwrap();
    let refused = editor.append_child(root, second).unwrap_err();
    assert_eq!(refused.kind, EditErrorKind::Hierarchy);
    let saved = String::from_utf8(save::to_vec(&document, &save::Options::default()).unwrap());
    assert_eq!(saved.unwrap(), format!("{DECL}<r><a/></r>\n<!--c-->\n"));
}

#[test]
fn recovery_without_an_element_keeps_the_prolog() {
    for &(input, saved, errors) in ROOTLESS {
        let expected = (format!("{DECL}{saved}"), errors.to_vec());
        assert_eq!(recover(input.as_bytes(), 0), Ok(expected), "{input}");
        let options = || ParseOptions::new(RECOVER);
        let borrowed = Document::parse_with(input.as_bytes(), options()).unwrap();
        let owned = OwnedDocument::parse_with(input.as_bytes(), options()).unwrap();
        assert!(borrowed.document_element().is_none() && owned.document_element().is_none());
        assert_eq!(
            borrowed.root_element().id(),
            borrowed.root().id(),
            "{input}"
        );
        assert_eq!(owned.root_element().id(), owned.root().id(), "{input}");
        assert!(Document::parse(input).is_err(), "{input}");
    }
    // ... as long as there is input, as libxml2 fails on an empty document
    assert_eq!(recover(b"", 0), Err(MissingRoot));
    let document = Document::parse_with(b"<r/>", ParseOptions::new(RECOVER)).unwrap();
    assert_eq!(
        document.document_element().map(|r| r.id()),
        Some(document.root_element().id())
    );
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
    let limits = Limits::default().max_attributes(4).max_events(3);
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

#[test]
fn errors_after_a_repaired_character_point_into_the_input() {
    // Recovery drops characters outside `Char` from text and replaces them
    // with U+FFFD in values; later errors still name input offsets.
    #[rustfmt::skip]
    let cases: &[(&str, &[(ErrorKind, usize)])] = &[
        ("<r>a\x01b&#x;</r>", &[(InvalidCharacter, 4), (InvalidReference, 6)]),
        ("<r>\x01\x01&</r>", &[(InvalidCharacter, 3), (InvalidCharacter, 4), (InvalidReference, 5)]),
        ("<r a='\x01&#x;'/>", &[(InvalidCharacter, 6), (InvalidReference, 7), (TextOutsideRoot, 11)]),
        ("<r a='\x01\x01&#x;\x01'/>", &[(InvalidCharacter, 6), (InvalidCharacter, 7), (InvalidCharacter, 12), (InvalidReference, 8), (TextOutsideRoot, 12)]),
        ("<r>a&#x;</r>", &[(InvalidReference, 4)]),
    ];
    let errors = |stream: Result<Stream, ErrorKind>| -> Vec<(ErrorKind, usize)> {
        let (_, errors) = stream.unwrap();
        errors
            .iter()
            .map(|error| (error.kind, error.offset))
            .collect()
    };
    for &(input, expected) in cases {
        assert_eq!(errors(sliced(input.as_bytes(), 0)), expected, "{input:?}");
        assert_eq!(
            errors(pushed(&[input.as_bytes()], 0)),
            expected,
            "{input:?}"
        );
        // UTF-16BE: a BOM, then two bytes per ASCII character.
        let utf16: Vec<u8> = [0xfe, 0xff]
            .into_iter()
            .chain(input.encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        let doubled: Vec<_> = expected
            .iter()
            .map(|&(kind, at)| (kind, 2 + 2 * at))
            .collect();
        assert_eq!(errors(pushed(&[&utf16], 0)), doubled, "{input:?}");
    }
}

/// A recovering stream's events with their input ranges, then its errors.
type Stream = (Vec<String>, Vec<yaxml::Error>);

fn line(event: &yaxml::Event<'_>, origin: Option<yaxml::SourceOrigin>) -> String {
    format!(
        "{event:?} {:?}",
        origin.and_then(yaxml::SourceOrigin::input_range)
    )
}

/// The slice reader's events, or its error.
fn sliced(input: &[u8], flags: u32) -> Result<Stream, ErrorKind> {
    let mut reader = Reader::with_options(input, ParseOptions::new(RECOVER | flags))
        .map_err(|error| error.kind)?;
    let mut lines = Vec::new();
    while let Some(event) = reader.read_event().map_err(|error| error.kind)? {
        lines.push(line(&event, reader.event_origin()));
    }
    Ok((lines, reader.errors().to_vec()))
}

/// The push parser's events with `input` fed in `chunks`, or its error.
fn pushed(chunks: &[&[u8]], flags: u32) -> Result<Stream, ErrorKind> {
    let mut parser = PushParser::with_options(ParseOptions::new(RECOVER | flags)).unwrap();
    let mut lines = Vec::new();
    let mut emit = |event: &yaxml::Event<'_>, origin| {
        lines.push(line(event, Some(origin)));
        ControlFlow::Continue(())
    };
    for chunk in chunks {
        parser.feed(chunk, &mut emit).map_err(|error| error.kind)?;
    }
    parser.finish(&mut emit).map_err(|error| error.kind)?;
    Ok((lines, parser.errors().to_vec()))
}

/// The buffered reader's events over a source that yields `window` bytes
/// at a time, or its error.
fn pulled(input: &[u8], window: usize, flags: u32) -> Result<Stream, ErrorKind> {
    let source = std::io::BufReader::with_capacity(window, input);
    let mut reader = BufferedReader::with_options(source, ParseOptions::new(RECOVER | flags))
        .map_err(|error| error.kind)?;
    let mut lines = Vec::new();
    while let Some((event, origin)) = reader
        .read_event()
        .map_err(|error| error.to_string())
        .map_err(|_| InvalidState)?
    {
        lines.push(line(&event, Some(origin)));
    }
    Ok((lines, reader.errors().to_vec()))
}

/// The text reader's nodes (depth, type, name, value), or its error.
fn text_nodes(input: &[u8], window: usize) -> Result<(Vec<String>, Vec<yaxml::Error>), ErrorKind> {
    let source = std::io::BufReader::with_capacity(window, input);
    let mut reader =
        TextReader::with_options(source, ParseOptions::new(RECOVER)).map_err(|error| error.kind)?;
    let mut nodes = Vec::new();
    while reader.read().map_err(|_| InvalidState)? {
        let node = (
            reader.depth(),
            reader.node_type() as i32,
            reader.name(),
            reader.value(),
        );
        nodes.push(format!("{node:?}"));
    }
    Ok((nodes, reader.errors().to_vec()))
}

/// Chunk boundaries that a test feeds: one byte at a time, small primes,
/// and every two-way split.
fn splits(input: &[u8]) -> Vec<Vec<&[u8]>> {
    let mut splits: Vec<Vec<&[u8]>> = [1, 2, 3, 5, 7, 64]
        .iter()
        .map(|&size| input.chunks(size).collect())
        .collect();
    splits.extend((0..=input.len()).map(|at| vec![&input[..at], &input[at..]]));
    splits
}

#[test]
fn streams_recover_as_the_slice_reader_whatever_the_chunks() {
    let inputs = CASES
        .iter()
        .chain(SLICE)
        .chain(ROOTLESS)
        .map(|case| case.0.as_bytes());
    let invalid: &[&[u8]] = &[
        b"<r>ab\xffcd</r>",
        b"<r>\xc3",
        b"<r><a b='\xff'/></r>",
        b"<r>x</r>\xff",
        b"<r>a<!\xc3\xa9>b</r>",
        b"\xff",
        // Invalid markup ends inside a scalar: the text after it fails
        // where its token ends, past a reference the input cuts off.
        b"<!DOCTYPE c [<!ATTLIST e r CDATA ''>]><c><![CD\xc3\xa9&",
        b"<!DOCTYPE c [<!ATTLIST e r CDATA ''>]><c><![CD\xef\xbb\xbfx&",
        b"<!DOCTYPE c [<!ATTLIST e r CDATA ''>]><c><![CD\xc3\xa9</c>",
    ];
    for input in inputs.chain(invalid.iter().copied()) {
        for flags in [0, NOENT] {
            let expected = sliced(input, flags);
            for chunks in splits(input) {
                assert_eq!(pushed(&chunks, flags), expected, "{input:?} in {chunks:?}");
            }
            for window in [1, 2, 3, 8, 4096] {
                assert_eq!(
                    pulled(input, window, flags),
                    expected,
                    "{input:?} by {window}"
                );
            }
        }
        let nodes = text_nodes(input, 4096);
        for window in [1, 3, 7] {
            assert_eq!(text_nodes(input, window), nodes, "{input:?} by {window}");
        }
        let errors = |stream: Result<Stream, ErrorKind>| stream.map(|stream| stream.1);
        assert_eq!(errors(nodes), errors(sliced(input, 0)), "{input:?}");
    }
}

#[test]
fn a_stopped_stream_closes_what_is_open_and_reads_no_more() {
    // (input, closing events)
    let cases: &[(&[u8], &[&str])] = &[
        (b"<r><a>text", &["a", "r"]),
        (b"<r><p:a xmlns:p='u'><b>", &["b", "p:a", "r"]),
        (b"<r>\xff<a>", &["r"]),
    ];
    for &(input, closed) in cases {
        let mut parser = PushParser::with_options(ParseOptions::new(RECOVER)).unwrap();
        let mut ends = Vec::new();
        let mut emit = |event: &yaxml::Event<'_>, origin: yaxml::SourceOrigin| {
            if let yaxml::Event::End(name) = event {
                assert!(origin.input_range().is_some_and(|range| range.is_empty()));
                ends.push(name.qualified().to_owned());
            }
            ControlFlow::Continue(())
        };
        parser.feed(input, &mut emit).unwrap();
        parser.finish(&mut emit).unwrap();
        assert_eq!(ends, closed, "{input:?}");
        assert!(!parser.errors().is_empty());
        // Recovery past the end of the document reads nothing more.
        let mut late = PushParser::with_options(ParseOptions::new(RECOVER)).unwrap();
        late.feed(b"<r/><s/>", |_, _| ControlFlow::Continue(()))
            .unwrap();
        let more = late.feed(b"<t>", |_, _| panic!("read past the stop"));
        assert_eq!(more, Ok(()));
        assert_eq!(late.finish(|_, _| panic!("read past the stop")), Ok(()));
        assert_eq!(late.errors()[0].kind, MultipleRoots);
    }
    // Without RECOVER, the error is returned and sticky; nothing is recorded.
    let mut strict = PushParser::new();
    let error = strict.feed(b"<r><a>", |_, _| ControlFlow::Continue(()));
    assert_eq!(error, Ok(()));
    assert_eq!(
        strict
            .finish(|_, _| ControlFlow::Continue(()))
            .map_err(|e| e.kind),
        Err(UnexpectedEof)
    );
    assert!(strict.errors().is_empty());
}

#[test]
fn a_parse_stopped_in_replacement_text_closes_past_the_reference() {
    // (input, the root element's range, the closing `End` events' offset)
    let cases: &[(&str, std::ops::Range<usize>, usize)] = &[
        (
            "<!DOCTYPE r [<!ENTITY e '<a>x</a>'>]><r>&e;</r>",
            37..47,
            47,
        ),
        // an element left open, or a malformed end tag, in the entity
        ("<!DOCTYPE r [<!ENTITY e '<a>x'>]><r>&e;", 33..39, 39),
        (
            "<!DOCTYPE r [<!ENTITY e '<a></-a>'>]><r>&e;</r>",
            37..43,
            43,
        ),
        // an error outside replacement text closes at its offset
        ("<!DOCTYPE r [<!ENTITY e 'x'>]><r>&e;\u{1}</r>", 30..36, 36),
    ];
    for (input, root, at) in cases {
        let options = || ParseOptions::new(RECOVER);
        let borrowed = Document::parse_with(input.as_bytes(), options()).unwrap();
        let owned = OwnedDocument::parse_with(input.as_bytes(), options()).unwrap();
        let element = borrowed.document_element().unwrap();
        assert_eq!(element.range().as_ref(), Some(root), "{input}");
        let owned_element = owned.document_element().unwrap();
        assert_eq!(owned_element.range().as_ref(), Some(root), "{input}");
        // Every child's range lies inside its parent's.
        for node in element.descendants() {
            let (Some(child), Some(parent)) = (node.range(), node.parent().and_then(|p| p.range()))
            else {
                continue;
            };
            assert!(
                parent.start <= child.start && child.end <= parent.end,
                "{input}"
            );
        }
        let mut parser = PushParser::with_options(options()).unwrap();
        let mut ends = Vec::new();
        let mut emit = |event: &yaxml::Event<'_>, origin: yaxml::SourceOrigin| {
            if let (yaxml::Event::End(_), Some(range)) = (event, origin.input_range()) {
                ends.push(range.end);
            }
            ControlFlow::Continue(())
        };
        parser.feed(input.as_bytes(), &mut emit).unwrap();
        parser.finish(&mut emit).unwrap();
        assert_eq!(ends.last(), Some(at), "{input}");
    }
}

#[test]
fn stream_resource_limits_stay_fatal() {
    let deep = "<a>".repeat(300);
    let limits = Limits::default().max_events(3);
    // (input, limits, error)
    let cases = [
        (
            "<!DOCTYPE r [<!ENTITY a '&a;'>]><r>&a;</r>",
            Limits::default(),
            EntityLoop,
        ),
        (
            deep.as_str(),
            Limits::default(),
            LimitExceeded(Limit::Depth),
        ),
        (
            "<r><a></b><c></d></r>",
            limits,
            LimitExceeded(Limit::Events),
        ),
    ];
    for (input, limits, error) in cases {
        let options = || ParseOptions::new(RECOVER).with_limits(limits);
        let mut parser = PushParser::with_options(options()).unwrap();
        let result = parser
            .feed(input.as_bytes(), |_, _| ControlFlow::Continue(()))
            .and_then(|()| parser.finish(|_, _| ControlFlow::Continue(())));
        assert_eq!(result.map_err(|e| e.kind), Err(error), "{input}");
        let mut reader = BufferedReader::with_options(input.as_bytes(), options()).unwrap();
        let read = loop {
            match reader.read_event() {
                Ok(Some(_)) => {}
                Ok(None) => break Ok(()),
                Err(yaxml::ReadError::Xml(error)) => break Err(error.kind),
                Err(error) => panic!("{error}"),
            }
        };
        assert_eq!(read, Err(error), "{input}");
    }
}

/// `xmllint --walker --debug` lines (depth, type, name, empty, has value,
/// value) of the tree that a recovering stream's events build, as libxml2's
/// reader walks it: an element without children is empty and has no end,
/// adjacent text is one node, and references' content is not visited.
#[derive(Default)]
struct Walker {
    out: String,
    depth: usize,
    start: Option<String>,
    text: Option<String>,
    hidden: usize,
}

impl Walker {
    fn line(&mut self, kind: u8, name: &str, empty: bool, value: Option<&str>) {
        let (empty, has) = (u8::from(empty), u8::from(value.is_some() || kind == 7));
        let line = format!("{} {kind} {name} {empty} {has}", self.depth);
        self.out.push_str(&line);
        if let Some(value) = value.filter(|value| kind != 7 || !value.is_empty()) {
            self.out.push(' ');
            self.out.push_str(value);
        }
        self.out.push('\n');
    }

    /// The pending start tag, which has children.
    fn opened(&mut self) {
        if let Some(name) = self.start.take() {
            self.line(1, &name, false, None);
            self.depth += 1;
        }
    }

    fn event(&mut self, event: &yaxml::Event<'_>) {
        use yaxml::Event;
        if self.hidden > 0 {
            match event {
                Event::EntityStart(_) => self.hidden += 1,
                Event::EntityEnd => self.hidden -= 1,
                _ => {}
            }
            return;
        }
        if let Event::Text(text) = event {
            self.opened();
            self.text.get_or_insert_default().push_str(text);
            return;
        }
        if let Some(text) = self.text.take() {
            let blank = text.bytes().all(|b| b" \t\r\n".contains(&b));
            self.line(if blank { 14 } else { 3 }, "#text", false, Some(&text));
        }
        if !matches!(event, Event::End(_)) {
            self.opened();
        }
        match event {
            Event::End(name) => match self.start.take() {
                Some(start) => self.line(1, &start, true, None),
                None => {
                    self.depth -= 1;
                    self.line(15, name.qualified(), false, None);
                }
            },
            Event::Start(element) => self.start = Some(element.name.qualified().to_owned()),
            Event::Empty(element) => self.line(1, element.name.qualified(), true, None),
            Event::Cdata(text) => self.line(4, "#cdata-section", false, Some(text)),
            Event::Comment(text) => self.line(8, "#comment", false, Some(text)),
            Event::ProcessingInstruction { target, data } => {
                self.line(7, target, false, Some(data))
            }
            Event::EntityStart(name) => {
                self.line(5, name, false, None);
                self.hidden = 1;
            }
            Event::Doctype(doctype) => self.line(10, doctype.name, false, None),
            Event::Declaration(_) | Event::EntityEnd | Event::Text(_) => {}
        }
    }
}

/// The text reader's `xmllint --stream --debug` lines.
fn streamed(input: &[u8], flags: u32) -> String {
    let options = ParseOptions::new(RECOVER | flags);
    let mut reader = TextReader::with_options(input, options).unwrap();
    let mut out = String::new();
    while let Ok(true) = reader.read() {
        let (depth, kind) = (reader.depth(), reader.node_type() as i32);
        let (empty, has) = (reader.is_empty_element(), reader.has_value());
        let name = reader.name().unwrap_or("--");
        out.push_str(&format!(
            "{depth} {kind} {name} {} {}",
            u8::from(empty),
            u8::from(has)
        ));
        if let Some(value) = reader.value() {
            out.push(' ');
            out.push_str(value);
        }
        out.push('\n');
    }
    out
}

/// The slice reader's events (the DOMs') as walked lines.
fn walked_slice(input: &[u8]) -> String {
    let mut walker = Walker::default();
    if let Ok(mut reader) = Reader::with_options(input, ParseOptions::new(RECOVER)) {
        while let Ok(Some(event)) = reader.read_event() {
            walker.event(&event);
        }
    }
    walker.out
}

/// The push parser's events as `walked` lines, or nothing on a fatal error.
fn walked_push(input: &[u8], flags: u32) -> String {
    let mut parser = PushParser::with_options(ParseOptions::new(RECOVER | flags)).unwrap();
    let mut walker = Walker::default();
    let mut emit = |event: &yaxml::Event<'_>, _| {
        walker.event(event);
        ControlFlow::Continue(())
    };
    let done = (input.chunks(4096))
        .try_for_each(|chunk| parser.feed(chunk, &mut emit))
        .and_then(|()| parser.finish(&mut emit));
    done.map_or_else(|_| String::new(), |()| walker.out)
}

/// Streams under `RECOVER` against libxml2 2.15.4 (`YAXML_XMLLINT`), over
/// libxml2's `test/errors` and the XMLTS not-wf documents: the push
/// parser's tree against `xmllint --recover --push --walker --debug`
/// (libxml2's push parser) and `--recover --walker --debug` (its pull
/// parser, whose repairs the DOMs and streams share), also with
/// `--noblanks`, and the text reader against `--recover --stream --debug`
/// (libxml2's reader stops at its first error). Reports the rates.
#[test]
#[ignore = "requires YAXML_XMLLINT, the libxml2 source tree (YAXML_LIBXML2_SRC) and the XMLTS suites (YAXML_SUITES)"]
fn streams_recover_against_xmllint() {
    use std::path::PathBuf;
    use yaxml::options::NOBLANKS;
    let Some(xmllint) = std::env::var_os("YAXML_XMLLINT").map(PathBuf::from) else {
        eprintln!("YAXML_XMLLINT unset; skipping the stream --recover differential");
        return;
    };
    let source = std::env::var_os("YAXML_LIBXML2_SRC").map_or(
        "/Users/pbdeuchler/workspace/src/github.com/pbdeuchler/yaxml-oracle/libxml2-2.15.4".into(),
        PathBuf::from,
    );
    let suites = std::env::var_os("YAXML_SUITES").map_or(
        "/Users/pbdeuchler/workspace/src/github.com/pbdeuchler/yaxml-suites".into(),
        PathBuf::from,
    );
    let mut errors: Vec<String> = std::fs::read_dir(source.join("test/errors"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|name| name.into_string().ok())
        .filter(|name| name.ends_with(".xml"))
        .map(|name| format!("errors/{name}"))
        .collect();
    errors.sort();
    let catalog = std::fs::read_to_string(suites.join("xmlconf-catalog.tsv")).unwrap_or_default();
    let xmlts: Vec<String> = (catalog.lines())
        .filter_map(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            (fields.get(1) == Some(&"not-wf"))
                .then(|| fields.get(8).map(|path| path.to_string()))?
        })
        .collect();
    let rate = |agree: usize, total: usize| 100.0 * agree as f64 / total.max(1) as f64;
    let cases = [
        ("errors", source.join("test"), errors),
        ("XMLTS not-wf", suites.join("xmlconf"), xmlts),
    ];
    for (label, dir, inputs) in cases {
        let xmllint = |file: &str, args: &[&str]| {
            let output = std::process::Command::new(&xmllint)
                .args(args)
                .arg(file)
                .current_dir(&dir)
                .output()
                .unwrap();
            String::from_utf8_lossy(&output.stdout).into_owned()
        };
        // push vs libxml2's push and pull trees, --noblanks, text reader,
        // libxml2's push tree against its pull tree, and the DOMs' events
        // (the slice reader's) against the pull tree.
        let mut agree = [0; 6];
        for file in &inputs {
            let input = std::fs::read(dir.join(file)).unwrap_or_default();
            let pushed = walked_push(&input, 0);
            let push = xmllint(file, &["--recover", "--push", "--walker", "--debug"]);
            let pull = xmllint(file, &["--recover", "--walker", "--debug"]);
            let blanks = ["--recover", "--push", "--noblanks", "--walker", "--debug"];
            let checks = [
                pushed == push,
                pushed == pull,
                walked_push(&input, NOBLANKS) == xmllint(file, &blanks),
                streamed(&input, 0) == xmllint(file, &["--recover", "--stream", "--debug"]),
                push == pull,
                walked_slice(&input) == pull,
            ];
            for (count, agrees) in agree.iter_mut().zip(checks) {
                *count += usize::from(agrees);
            }
            if std::env::var_os("YAXML_DIFF").is_some() && !checks[1] {
                eprintln!("differs from libxml2's pull tree: {file}");
            }
        }
        let total = inputs.len();
        let [push, pull, blanks, text, libxml2, dom] =
            agree.map(|agree| (agree, rate(agree, total)));
        eprintln!(
            "--recover {label} ({total}): push vs --push {}/{:.2}%, vs pull {}/{:.2}%, \
             --noblanks {}/{:.2}%; text reader vs --stream {}/{:.2}%; libxml2 push vs pull \
             {}/{:.2}%; DOM events vs pull {}/{:.2}%",
            push.0,
            push.1,
            pull.0,
            pull.1,
            blanks.0,
            blanks.1,
            text.0,
            text.1,
            libxml2.0,
            libxml2.1,
            dom.0,
            dom.1
        );
    }
}

#[test]
fn recovered_errors_count_source_bytes_in_decoded_input() {
    // (input, where its first error is): ISO-8859-1 decodes `é` to two
    // UTF-8 bytes, on the DTD-free stream and on the DTD stack; an error in
    // an entity's replacement text is at the reference.
    let latin1 = b"<?xml version='1.0' encoding='ISO-8859-1'?>";
    let cases: [(Vec<u8>, &[u8]); 3] = [
        ([&latin1[..], b"<r>\xe9<a></b></r>"].concat(), b"</b>"),
        (
            [&latin1[..], b"<!DOCTYPE r><r>\xe9<a></b></r>"].concat(),
            b"</b>",
        ),
        (
            [
                &latin1[..],
                b"<!DOCTYPE r [<!ENTITY e '<p:a/>'>]><r>\xe9&e;</r>",
            ]
            .concat(),
            b"&e;",
        ),
    ];
    for (input, at) in cases {
        let offset = input.windows(at.len()).position(|w| w == at).unwrap();
        let mut parser = PushParser::with_options(ParseOptions::new(RECOVER)).unwrap();
        parser
            .feed(&input, |_, _| ControlFlow::Continue(()))
            .unwrap();
        parser.finish(|_, _| ControlFlow::Continue(())).unwrap();
        let owned = OwnedDocument::parse_with(&input, ParseOptions::new(RECOVER)).unwrap();
        for errors in [parser.errors(), owned.errors()] {
            assert_eq!(errors[0].offset, offset, "{input:?}");
            assert!(errors.iter().all(|error| error.offset <= input.len()));
        }
    }
}
