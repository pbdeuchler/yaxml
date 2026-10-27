//! A forced encoding (`ParseOptions::encoding`, the `encoding` argument of
//! `xmlCtxtReadMemory`), pinned against libxml2 2.15.4: it overrides the
//! BOM, the UTF-16 signature and the declaration, skips a mark that decodes
//! to U+FEFF, and every surface decodes alike.

use std::ops::ControlFlow;

use yaxml::encoding::Handler;
use yaxml::{
    BufferedReader, Document, ErrorKind, Event, OwnedDocument, ParseOptions, PushParser, ReadError,
    Reader, Transcoded,
};

fn forced(name: &str) -> ParseOptions {
    ParseOptions::DEFAULT.encoding(Some(Handler::open(name).unwrap()))
}

fn utf16(text: &str, little: bool, bom: bool) -> Vec<u8> {
    let mark = bom.then_some(0xfeff);
    (mark.into_iter().chain(text.encode_utf16()))
        .flat_map(|unit| match little {
            true => unit.to_le_bytes(),
            false => unit.to_be_bytes(),
        })
        .collect()
}

/// The root's content through the owned DOM.
fn owned(input: &[u8], name: &str) -> Result<String, ErrorKind> {
    let document = OwnedDocument::parse_with(input, forced(name)).map_err(|e| e.kind)?;
    Ok(document
        .document_element()
        .and_then(|r| r.content())
        .unwrap_or_default())
}

/// The root's content through `Transcoded` and the borrowed DOM.
fn borrowed(input: &[u8], name: &str) -> Result<String, ErrorKind> {
    let text = Transcoded::new(input, &forced(name)).map_err(|e| e.kind)?;
    let document = Document::parse_transcoded(&text, forced(name)).map_err(|e| e.kind)?;
    Ok(document
        .document_element()
        .and_then(|r| r.content())
        .unwrap_or_default())
}

/// The text events of a push parse fed a byte at a time.
fn pushed(input: &[u8], name: &str) -> Result<String, ErrorKind> {
    let mut parser = PushParser::with_options(forced(name)).map_err(|e| e.kind)?;
    let mut text = String::new();
    let mut emit = |event: &Event<'_>, _| {
        if let Event::Text(value) = event {
            text.push_str(value);
        }
        ControlFlow::Continue(())
    };
    for byte in input.chunks(1) {
        parser.feed(byte, &mut emit).map_err(|e| e.kind)?;
    }
    parser.finish(&mut emit).map_err(|e| e.kind)?;
    Ok(text)
}

/// The text events of a buffered reader.
fn buffered(input: &[u8], name: &str) -> Result<String, ErrorKind> {
    let mut reader = BufferedReader::with_options(input, forced(name)).map_err(|e| e.kind)?;
    let mut text = String::new();
    while let Some((event, _)) = reader.read_event().map_err(|e| match e {
        ReadError::Xml(error) => error.kind,
        _ => unreachable!("slices read"),
    })? {
        if let Event::Text(value) = event {
            text.push_str(&value);
        }
    }
    Ok(text)
}

/// Name, input, forced encoding, root content or error.
type Case<'a> = (&'a str, Vec<u8>, &'a str, Result<&'a str, ErrorKind>);

#[test]
fn forced_encodings_decode_like_libxml2() {
    let declared = |label: &str, body: &[u8]| {
        let mut input = format!("<?xml version='1.0' encoding='{label}'?>").into_bytes();
        input.extend_from_slice(body);
        input
    };
    let e = Ok("\u{e9}");
    let cases: [Case<'_>; 15] = [
        ("latin-1 bytes", b"<r>\xe9</r>".to_vec(), "ISO-8859-1", e),
        ("label as given", b"<r>\xe9</r>".to_vec(), "latin1", e),
        (
            "declared UTF-8",
            declared("UTF-8", b"<r>\xe9</r>"),
            "ISO-8859-1",
            e,
        ),
        (
            "declared UTF-16",
            declared("UTF-16", b"<r>\xe9</r>"),
            "ISO-8859-1",
            e,
        ),
        (
            "declared unknown",
            declared("bogus", b"<r>\xe9</r>"),
            "ISO-8859-1",
            e,
        ),
        (
            "declared Latin-1",
            declared("ISO-8859-1", "<r>é</r>".as_bytes()),
            "UTF-8",
            e,
        ),
        (
            "declared Latin-2",
            declared("ISO-8859-2", "<r>é</r>".as_bytes()),
            "UTF-8",
            e,
        ),
        (
            "UTF-8 BOM skipped",
            b"\xef\xbb\xbf<r>\xc3\xa9</r>".to_vec(),
            "UTF-8",
            e,
        ),
        (
            "UTF-16LE unmarked",
            utf16("<r>é</r>", true, false),
            "UTF-16LE",
            e,
        ),
        (
            "generic UTF-16",
            utf16("<r>é</r>", true, false),
            "UTF-16",
            e,
        ),
        (
            "UTF-16 BOM skipped",
            utf16("<r>é</r>", true, true),
            "UTF-16",
            e,
        ),
        (
            "UTF-16BE BOM",
            utf16("<r>é</r>", false, true),
            "UTF-16BE",
            e,
        ),
        (
            "UTF-16 without a label",
            utf16("<?xml version='1.0'?><r/>", true, false),
            "UTF-16LE",
            Ok(""),
        ),
        (
            "UTF-8 BOM read as Latin-1",
            b"\xef\xbb\xbf<r/>".to_vec(),
            "ISO-8859-1",
            Err(ErrorKind::TextOutsideRoot),
        ),
        (
            "ASCII high byte",
            b"<r>\xe9</r>".to_vec(),
            "US-ASCII",
            Err(ErrorKind::UnmappedByte),
        ),
    ];
    for (name, input, encoding, expected) in cases {
        let expected = expected.map(str::to_owned);
        assert_eq!(owned(&input, encoding), expected, "{name}: owned");
        assert_eq!(borrowed(&input, encoding), expected, "{name}: transcoded");
        assert_eq!(pushed(&input, encoding), expected, "{name}: push");
        assert_eq!(buffered(&input, encoding), expected, "{name}: buffered");
    }
}

#[test]
fn a_mismatched_mark_is_no_bom() {
    // libxml2: "Start tag expected" for each; a UTF-16 mark read as UTF-8
    // is invalid UTF-8, the reversed mark is U+FFFE.
    for (input, encoding) in [
        (utf16("<r/>", true, true), "UTF-8"),
        (utf16("<r/>", true, true), "UTF-16BE"),
    ] {
        assert!(owned(&input, encoding).is_err(), "{encoding}");
        assert!(borrowed(&input, encoding).is_err(), "{encoding}");
        assert!(pushed(&input, encoding).is_err(), "{encoding}");
    }
}

#[test]
fn borrowed_input_takes_a_forced_encoding_only_transcoded() {
    let unsupported = Some(ErrorKind::UnsupportedOption);
    let reader = Reader::with_options(b"<r/>", forced("UTF-8"));
    assert_eq!(reader.err().map(|e| e.kind), unsupported);
    let document = Document::parse_with(b"<r/>", forced("UTF-8"));
    assert_eq!(document.err().map(|e| e.kind), unsupported);
    assert_eq!(borrowed(b"<r/>", "UTF-8"), Ok(String::new()));
}

#[test]
fn unknown_names_fail_to_open() {
    // libxml2 warns "Unsupported encoding" and detects instead: the caller
    // decides here, since the handler is opened first.
    assert_eq!(Handler::open("nope"), Err(ErrorKind::UnsupportedEncoding));
}
