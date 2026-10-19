//! XML output of HTML documents is well-formed or a structured error, never
//! silently malformed: names and text HTML allows but XML forbids.
#![forbid(unsafe_code)]

use yaxml::xpath::{Context, Value, XPath};
use yaxml::{OwnedDocument, Writer, c14n, html};

/// (input, representable as XML)
const CASES: &[(&[u8], bool)] = &[
    (b"<p a<b=1>x</p>", false),
    (b"<a\"b>x", false),
    (b"<p =x>", false),
    (b"<a:b>x", false),
    (b"<p z:a=1>", false),
    (b"<p xmlns=urn:x>", false),
    (b"<p xmlns:z='' z:a=1>", false),
    (b"<p>\x01\x0b</p>", false),
    (b"<p a='\x01'>", false),
    (b"<p>&#xFFFE;</p>", false),
    (b"<!--a--b-->", false),
    (b"<!--a--->", false),
    (b"<script>]]>x</script>", true),
    (b"<p a b='&lt;\"'>t&amp;</p><!--ok-->", true),
    (b"<!DOCTYPE html PUBLIC '\"' \"'\"><p>x", true),
    (b"<title>T</title><p>a<p>b", true),
];

#[test]
fn several_top_level_elements_are_not_one_xml_document() {
    let options = html::Options::new(html::NOIMPLIED);
    let document = html::parse(b"<a>x</a><b>y</b>", options).unwrap().document;
    let canonical = c14n::to_string(&document, &c14n::Options::new(c14n::Mode::C14n10));
    assert!(matches!(canonical, Err(c14n::Error::NotXml(_))));
    assert!(
        Writer::new(Vec::new())
            .write_owned_document(&document)
            .is_err()
    );
}

fn well_formed(output: &str) {
    if let Err(error) = OwnedDocument::parse(output) {
        panic!("malformed XML {output:?}: {error:?}");
    }
}

#[test]
fn writer_output_is_well_formed_or_an_error() {
    for (input, valid) in CASES {
        let document = html::parse(input, html::Options::default())
            .unwrap()
            .document;
        let mut writer = Writer::new(Vec::new());
        // The writer validates every event: XML-incompatible names, characters
        // and comments, CDATA holding "]]>" and DOCTYPE identifiers with both
        // quotes are rejected.
        if writer.write_owned_document(&document).is_ok() {
            assert!(valid, "{input:?}");
            well_formed(&String::from_utf8(writer.into_inner()).unwrap());
        }
    }
}

#[test]
fn c14n_output_is_well_formed_or_an_error() {
    for (input, valid) in CASES {
        let document = html::parse(input, html::Options::default())
            .unwrap()
            .document;
        let mut options = c14n::Options::new(c14n::Mode::C14n10);
        options.with_comments = true;
        match c14n::to_string(&document, &options) {
            Ok(output) => {
                assert!(valid, "{input:?} gave {output:?}");
                well_formed(&output);
            }
            Err(error) => {
                assert!(!valid, "{input:?}: {error}");
                assert!(matches!(error, c14n::Error::NotXml(_)), "{error:?}");
            }
        }
    }
}

#[test]
fn xpath_reads_html_names_verbatim() {
    let document = html::parse(b"<p a<b=1>x<P>y", html::Options::default())
        .unwrap()
        .document;
    let root = document.root().id();
    let count = XPath::compile("count(//p)").unwrap();
    let value = count.evaluate(&document, root, &Context::new()).unwrap();
    assert_eq!(value, Value::Number(2.0));
    let name = XPath::compile("name(//p/@*)").unwrap();
    let value = name.evaluate(&document, root, &Context::new()).unwrap();
    assert_eq!(value, Value::String("a<b".to_owned()));
}
