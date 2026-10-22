//! `XML_PARSE_OLD10`: XML 1.0 Fourth Edition names and a fatal version
//! other than 1.0, on every surface. Expectations were checked against
//! `xmllint --oldxml10` 2.15.4.

use std::io::BufReader;
use std::ops::ControlFlow;

use yaxml::options::{OLD10, RECOVER};
use yaxml::{
    BufferedReader, Document, ErrorKind, Event, OwnedDocument, ParseOptions, PushParser, ReadError,
    Reader,
};

type Outcome = Result<(), ErrorKind>;

fn owned(input: &str, flags: u32) -> Outcome {
    OwnedDocument::parse_with(input.as_bytes(), ParseOptions::new(flags))
        .map(drop)
        .map_err(|error| error.kind)
}

fn document(input: &str, flags: u32) -> Outcome {
    Document::parse_with(input.as_bytes(), ParseOptions::new(flags))
        .map(drop)
        .map_err(|error| error.kind)
}

fn read(input: &str, flags: u32) -> Outcome {
    let mut reader = Reader::with_options(input.as_bytes(), ParseOptions::new(flags))
        .map_err(|error| error.kind)?;
    while reader.read_event().map_err(|error| error.kind)?.is_some() {}
    Ok(())
}

fn pushed(input: &str, flags: u32, chunk: usize) -> Outcome {
    let mut parser = PushParser::with_options(ParseOptions::new(flags)).map_err(|e| e.kind)?;
    let mut emit = |_: &Event<'_>, _| ControlFlow::Continue(());
    for piece in input.as_bytes().chunks(chunk) {
        parser.feed(piece, &mut emit).map_err(|error| error.kind)?;
    }
    parser.finish(&mut emit).map_err(|error| error.kind)
}

fn buffered(input: &str, flags: u32, capacity: usize) -> Outcome {
    let source = BufReader::with_capacity(capacity, input.as_bytes());
    let mut reader =
        BufferedReader::with_options(source, ParseOptions::new(flags)).map_err(|e| e.kind)?;
    loop {
        match reader.read_event() {
            Ok(Some(_)) => {}
            Ok(None) => return Ok(()),
            Err(ReadError::Xml(error)) => return Err(error.kind),
            Err(error) => panic!("{error}"),
        }
    }
}

#[cfg(feature = "textreader")]
fn text_reader(input: &str, flags: u32) -> Outcome {
    let mut reader = yaxml::TextReader::with_options(input.as_bytes(), ParseOptions::new(flags))
        .map_err(|error| error.kind)?;
    loop {
        match reader.read() {
            Ok(true) => {}
            Ok(false) => return Ok(()),
            Err(ReadError::Xml(error)) => return Err(error.kind),
            Err(error) => panic!("{error}"),
        }
    }
}

/// Every surface's outcome, checked equal.
fn parse(input: &str, flags: u32) -> Outcome {
    let expected = owned(input, flags);
    let checks = [
        ("document", document(input, flags)),
        ("reader", read(input, flags)),
        ("push/1", pushed(input, flags, 1)),
        ("push", pushed(input, flags, input.len())),
        ("buffered/1", buffered(input, flags, 1)),
        ("buffered", buffered(input, flags, 4096)),
        #[cfg(feature = "textreader")]
        ("textreader", text_reader(input, flags)),
    ];
    for (surface, outcome) in checks {
        assert_eq!(outcome, expected, "{surface} {input} {flags:#x}");
    }
    expected
}

#[test]
fn old10_reads_fourth_edition_names() {
    use ErrorKind::{InvalidDeclaration, InvalidName};
    // U+4CFF (CJK Extension A), U+0653 and U+0E5C are Fifth Edition name
    // characters only; é, à, U+0E9C and the extender U+B7 are in both.
    let cases: [(&str, Outcome); 25] = [
        ("<r/>", Ok(())),
        ("<é/>", Ok(())),
        ("<aຜ/>", Ok(())),
        ("<a·/>", Ok(())),
        ("<à/>", Ok(())),
        ("<?é x?><r/>", Ok(())),
        ("<!DOCTYPE r [<!ATTLIST r a (·x) #IMPLIED>]><r/>", Ok(())),
        ("<?xml version='1.0'?><r/>", Ok(())),
        ("<䳿/>", Err(InvalidName)),
        ("<r><a\u{e5c}/></r>", Err(InvalidName)),
        ("<r a\u{653}='1'/>", Err(InvalidName)),
        ("<r xmlns:䳿='u'/>", Err(InvalidName)),
        ("<?䳿 x?><r/>", Err(InvalidName)),
        ("<!DOCTYPE r [<?䳿 x?>]><r/>", Err(InvalidName)),
        ("<!DOCTYPE r [<!ELEMENT 䳿 ANY>]><r/>", Err(InvalidName)),
        ("<!DOCTYPE r [<!ELEMENT r (䳿)>]><r/>", Err(InvalidName)),
        (
            "<!DOCTYPE r [<!ATTLIST r a (䳿) #IMPLIED>]><r/>",
            Err(InvalidName),
        ),
        (
            "<!DOCTYPE r [<!ATTLIST 䳿 a CDATA #IMPLIED>]><r/>",
            Err(InvalidName),
        ),
        (
            "<!DOCTYPE r [<!ATTLIST r 䳿 CDATA #IMPLIED>]><r/>",
            Err(InvalidName),
        ),
        ("<!DOCTYPE r [<!ENTITY 䳿 'x'>]><r/>", Err(InvalidName)),
        (
            "<!DOCTYPE r [<!NOTATION 䳿 SYSTEM 'x'>]><r/>",
            Err(InvalidName),
        ),
        ("<!DOCTYPE 䳿><r/>", Err(InvalidName)),
        (
            "<!DOCTYPE r [<!ENTITY e '<䳿/>'>]><r>&e;</r>",
            Err(InvalidName),
        ),
        ("<?xml version='1.1'?><r/>", Err(InvalidDeclaration)),
        ("<?xml version='1.7'?><r/>", Err(InvalidDeclaration)),
    ];
    for (input, expected) in cases {
        // Every input is a Fifth Edition document.
        assert_eq!(parse(input, 0), Ok(()), "{input}");
        assert_eq!(parse(input, OLD10), expected, "{input}");
    }
}

#[test]
fn recovery_reads_an_old10_invalid_name_as_libxml2_does() {
    // No Fourth Edition name: the tag is text after `<`, the PI is text
    // after `<?`; a longer name is cut at its first invalid character.
    let cases = [
        ("<r><䳿/></r>", "䳿/>"),
        ("<r><?䳿 x?></r>", "䳿 x?>"),
        ("<r><a䳿/></r>", "䳿/>"),
    ];
    for (input, text) in cases {
        let document =
            OwnedDocument::parse_with(input.as_bytes(), ParseOptions::new(OLD10 | RECOVER))
                .unwrap();
        assert!(!document.errors().is_empty(), "{input}");
        let found: String = document.root_element().text_chunks().collect();
        assert_eq!(found, text, "{input}");
    }
}

#[test]
fn old10_parses_carry_the_old10_property() {
    use yaxml::properties::{NSVALID, OLD10 as PROPERTY, WELLFORMED};
    // A recovered error clears WELLFORMED, never OLD10 (xmlSAX2StartDocument).
    let cases = [
        ("<r/>", 0, WELLFORMED | NSVALID),
        ("<r/>", OLD10, WELLFORMED | NSVALID | PROPERTY),
        ("<r><䳿/></r>", OLD10 | RECOVER, PROPERTY),
    ];
    for (input, flags, expected) in cases {
        let options = || ParseOptions::new(flags);
        let owned = OwnedDocument::parse_with(input.as_bytes(), options()).unwrap();
        let borrowed = Document::parse_with(input.as_bytes(), options()).unwrap();
        let dtd_valid = yaxml::properties::DTDVALID * u32::from(expected & WELLFORMED != 0);
        assert_eq!(owned.properties(), expected | dtd_valid, "{input}");
        assert_eq!(borrowed.properties(), expected | dtd_valid, "{input}");
    }
    // A copy is user-built (xmlCopyDoc -> xmlNewDoc).
    let owned = OwnedDocument::parse_with(b"<r/>", ParseOptions::new(OLD10)).unwrap();
    let copy = owned.try_clone().unwrap();
    assert_eq!(copy.properties(), yaxml::properties::USERBUILT);
}

#[test]
fn old10_reads_external_declarations_as_fourth_edition() {
    use yaxml::options::DTDLOAD;
    let options = |flags: u32, subset: &'static str| {
        let resolver = move |_: yaxml::Request<'_>| Ok(Some(subset.as_bytes().to_vec()));
        ParseOptions::new(DTDLOAD | flags).with_resolver("doc.xml", Box::new(resolver))
    };
    let documents = [
        "<!DOCTYPE r SYSTEM 'x.dtd'><r/>",
        "<!DOCTYPE r [<!ENTITY % p SYSTEM 'x.dtd'> %p;]><r/>",
    ];
    let subsets = [
        ("<!ELEMENT r ANY>", Ok(())),
        ("<!ELEMENT 䳿 ANY>", Err(ErrorKind::InvalidName)),
        ("<?䳿 x?>", Err(ErrorKind::InvalidName)),
    ];
    for input in documents {
        for (subset, expected) in subsets {
            let parse = |flags| {
                let owned = OwnedDocument::parse_with(input.as_bytes(), options(flags, subset));
                let borrowed = Document::parse_with(input.as_bytes(), options(flags, subset));
                assert_eq!(owned.is_ok(), borrowed.is_ok(), "{input} {subset}");
                owned.map(drop).map_err(|error| error.kind)
            };
            assert_eq!(parse(0), Ok(()), "{input} {subset}");
            assert_eq!(parse(OLD10), expected, "{input} {subset}");
        }
    }
}
