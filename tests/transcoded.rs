//! `Transcoded` input on the borrowing `Document` and `Reader`, checked
//! against the owned DOM, which decodes through the streaming parser.

use std::borrow::Cow;

use yaxml::options::{IGNORE_ENC, RECOVER};
use yaxml::{
    DecodeLimits, Document, Error, ErrorKind, Event, Limit, Limits, LineIndex, NodeKind,
    OwnedDocument, ParseOptions, Reader, Transcoded, Warning,
};

fn utf16(text: &str, little: bool, bom: bool) -> Vec<u8> {
    let units = bom.then_some(0xfeff).into_iter().chain(text.encode_utf16());
    units
        .flat_map(|unit| match little {
            true => unit.to_le_bytes(),
            false => unit.to_be_bytes(),
        })
        .collect()
}

fn error(kind: ErrorKind, offset: usize) -> Error {
    Error { kind, offset }
}

/// The root's text and the declared encoding, or the error, with offsets
/// mapped to the input; the reader must agree with the document.
type Outcome = Result<(String, Option<String>, Vec<Error>), Error>;

fn transcoded(input: &[u8], options: impl Fn() -> ParseOptions) -> Outcome {
    let text = Transcoded::new(input, &options())?;
    let map = |e: Error| error(e.kind, text.source_offset(e.offset));
    let read = (|| {
        let mut reader = Reader::transcoded(&text, options())?;
        while reader.read_event()?.is_some() {}
        Ok(reader.errors().to_vec())
    })();
    let document = Document::parse_transcoded(&text, options());
    assert_eq!(
        read.as_ref().err(),
        document.as_ref().err(),
        "reader and document disagree"
    );
    let document = document.map_err(map)?;
    assert_eq!(
        read.unwrap(),
        document.errors(),
        "reader and document errors"
    );
    let root = document.document_element().and_then(|root| root.content());
    Ok((
        root.unwrap_or_default(),
        document
            .declaration()
            .and_then(|d| d.encoding)
            .map(str::to_owned),
        document.errors().iter().copied().map(map).collect(),
    ))
}

fn owned(input: &[u8], options: ParseOptions) -> Outcome {
    let document = OwnedDocument::parse_with(input, options)?;
    let root = document.document_element().and_then(|root| root.content());
    Ok((
        root.unwrap_or_default(),
        document
            .declaration()
            .and_then(|d| d.encoding)
            .map(str::to_owned),
        document.errors().to_vec(),
    ))
}

#[test]
fn transcoded_parses_agree_with_the_owned_dom() {
    let declared = |label: &str, body: &[u8]| {
        [
            format!("<?xml version='1.0' encoding='{label}'?>").as_bytes(),
            body,
        ]
        .concat()
    };
    let ok =
        |text: &str, encoding: Option<&str>| Ok((text.into(), encoding.map(Into::into), vec![]));
    let doc = "<?xml version='1.0' encoding='UTF-16'?><r>\u{e9}\u{1f600}</r>";
    let mismatch = "<?xml version='1.0' encoding='ISO-8859-1'?><r/>";
    let mut lone = utf16("<r>a", true, true);
    lone.extend_from_slice(&0xdc00_u16.to_le_bytes());
    lone.extend(utf16("</r>", true, false));
    let cases: Vec<(&str, Vec<u8>, u32, Outcome)> = vec![
        (
            "utf-8",
            b"<r>caf\xc3\xa9</r>".to_vec(),
            0,
            ok("caf\u{e9}", None),
        ),
        (
            "utf-8 bom",
            b"\xef\xbb\xbf<r>a</r>".to_vec(),
            0,
            ok("a", None),
        ),
        (
            "utf-16le bom",
            utf16(doc, true, true),
            0,
            ok("\u{e9}\u{1f600}", Some("UTF-16")),
        ),
        (
            "utf-16be bom",
            utf16(doc, false, true),
            0,
            ok("\u{e9}\u{1f600}", Some("UTF-16")),
        ),
        (
            "utf-16le signature",
            utf16(
                "<?xml version='1.0' encoding='UTF-16LE'?><r>a</r>",
                true,
                false,
            ),
            0,
            ok("a", Some("UTF-16LE")),
        ),
        (
            "utf-16be signature",
            utf16(
                "<?xml version='1.0' encoding='UTF-16BE'?><r>a</r>",
                false,
                false,
            ),
            0,
            ok("a", Some("UTF-16BE")),
        ),
        // libxml2 keeps the BOM's encoding, records its name and warns.
        (
            "bom outranks label",
            utf16(mismatch, true, true),
            0,
            Ok((
                String::new(),
                Some("UTF-16LE".into()),
                vec![error(ErrorKind::Warning(Warning::EncodingMismatch), 42)],
            )),
        ),
        (
            "signature without label",
            utf16("<?xml version='1.0'?><r/>", true, false),
            0,
            Err(error(ErrorKind::EncodingMismatch, 0)),
        ),
        (
            "signature without declaration",
            utf16("<?pi?><r/>", false, false),
            0,
            Err(error(ErrorKind::EncodingMismatch, 0)),
        ),
        (
            "latin-1",
            declared("ISO-8859-1", b"<r>caf\xe9</r>"),
            0,
            ok("caf\u{e9}", Some("ISO-8859-1")),
        ),
        (
            "latin-2",
            declared("ISO-8859-2", b"<r>\xb1</r>"),
            0,
            ok("\u{105}", Some("ISO-8859-2")),
        ),
        (
            "windows-1252",
            declared("windows-1252", b"<r>\x80</r>"),
            0,
            ok("\u{20ac}", Some("windows-1252")),
        ),
        (
            "alias",
            declared("latin1", b"<r>\xff</r>"),
            0,
            ok("\u{ff}", Some("latin1")),
        ),
        (
            "ignored label",
            declared("ISO-8859-1", b"<r>\xc3\xa9</r>"),
            IGNORE_ENC,
            ok("\u{e9}", None),
        ),
        (
            "unmapped byte",
            declared("US-ASCII", b"<r>\x80</r>"),
            0,
            Err(error(ErrorKind::UnmappedByte, 44)),
        ),
        (
            "lone low surrogate",
            lone.clone(),
            0,
            Err(error(ErrorKind::InvalidUtf16, 10)),
        ),
        (
            "odd final byte",
            [utf16("<r/>", true, true), vec![0]].concat(),
            0,
            Err(error(ErrorKind::InvalidUtf16, 10)),
        ),
        (
            "ucs-4 2143 signature",
            b"\0\0<\0\0\0r\0\0\0/\0\0\0>\0".to_vec(),
            0,
            Err(error(ErrorKind::UnsupportedEncoding, 0)),
        ),
        (
            "mismatched end tag",
            utf16("<r>\u{e9}</s>", true, true),
            0,
            Err(error(ErrorKind::MismatchedEndTag, 10)),
        ),
        // Kept errors count input bytes on every surface.
        (
            "namespace error",
            utf16("<\u{e9}:r/>", true, true),
            0,
            Ok((String::new(), None, vec![error(ErrorKind::Namespace, 4)])),
        ),
        // Recovery drops the label of a malformed declaration: no switch.
        (
            "recovered declaration",
            b"<?xml version<'1.0' encoding='latin1'?><r>\xe9</r>".to_vec(),
            RECOVER,
            Ok((
                String::new(),
                None,
                vec![
                    error(ErrorKind::InvalidDeclaration, 13),
                    error(ErrorKind::InvalidDeclaration, 5),
                    error(ErrorKind::InvalidUtf8, 42),
                ],
            )),
        ),
        (
            "recovered latin-1 tag",
            declared("ISO-8859-1", b"<r>\xe9</s>"),
            RECOVER,
            Ok((
                "\u{e9}".into(),
                Some("ISO-8859-1".into()),
                vec![error(ErrorKind::MismatchedEndTag, 47)],
            )),
        ),
    ];
    for (name, input, flags, expected) in cases {
        let actual = transcoded(&input, || ParseOptions::new(flags));
        assert_eq!(actual, expected, "{name}");
        assert_eq!(
            actual,
            owned(&input, ParseOptions::new(flags)),
            "{name}: owned DOM"
        );
    }
    // Recovery parses the text decoded before a decoding error, keeping
    // `a` as `xmllint --recover` does; the streams drop the pending text.
    let errors = vec![error(ErrorKind::InvalidUtf16, 10)];
    let recover = || ParseOptions::new(RECOVER);
    assert_eq!(
        transcoded(&lone, recover),
        Ok(("a".into(), None, errors.clone()))
    );
    assert_eq!(owned(&lone, recover()), Ok((String::new(), None, errors)));
}

#[test]
fn limits_apply_to_the_source_and_the_decoded_text() {
    let input = utf16("<r>\u{e9}\u{e9}\u{e9}\u{e9}</r>", true, true);
    let limited = |limits: Limits, decode: DecodeLimits| {
        let options = || {
            ParseOptions::default()
                .with_limits(limits)
                .with_decode_limits(decode)
        };
        transcoded(&input, options).map(drop)
    };
    let (limits, decode) = (Limits::DEFAULT, DecodeLimits::DEFAULT);
    let cases = [
        (limits.max_input_bytes(input.len()), decode, Ok(())),
        (
            limits.max_input_bytes(input.len() - 1),
            decode,
            Err(error(ErrorKind::LimitExceeded(Limit::InputBytes), 0)),
        ),
        // `<r>` and four two-byte characters are 11 bytes of UTF-8.
        (
            limits,
            decode.max_decoded_bytes(11),
            Err(error(ErrorKind::LimitExceeded(Limit::DecodedBytes), 0)),
        ),
        (limits, decode.max_decoded_bytes(15), Ok(())),
        (
            limits,
            decode.max_decoded_token_bytes(7),
            Err(error(ErrorKind::LimitExceeded(Limit::DecodedTokenBytes), 8)),
        ),
        (limits, decode.max_decoded_token_bytes(8), Ok(())),
    ];
    for (limits, decode, expected) in cases {
        assert_eq!(limited(limits, decode), expected, "{limits:?} {decode:?}");
    }
}

#[test]
fn utf8_input_is_borrowed_and_parses_as_before() {
    for input in [
        &b"<r>text</r>"[..],
        b"\xef\xbb\xbf<r>text</r>",
        b"<r>\xff</r>",
        b"<r>",
    ] {
        let text = Transcoded::new(input, &ParseOptions::default()).unwrap();
        let document = Document::parse_transcoded(&text, ParseOptions::default());
        assert_eq!(
            document.as_ref().err(),
            Document::parse_bytes(input).as_ref().err()
        );
        assert_eq!(text.source_offset(4), 4);
        let Ok(document) = document else { continue };
        let node = document.root_element().first_child().unwrap();
        let NodeKind::Text(Cow::Borrowed(value)) = node.kind() else {
            panic!("copied text")
        };
        assert!(input.as_ptr_range().contains(&value.as_ptr()));
    }
}

#[test]
fn a_single_byte_charsets_declaration_maps_as_utf8() {
    // The declaration is read as UTF-8 before its charset takes over, so
    // UTF-8 in it counts its bytes, and the rest one byte per character.
    type Errors = &'static [(ErrorKind, usize)];
    #[rustfmt::skip]
    let cases: &[(&[u8], u32, Errors)] = &[
        (b"", 0, &[(ErrorKind::MismatchedEndTag, 47)]),
        (b"\xef\xbb\xbf", RECOVER, &[(ErrorKind::InvalidDeclaration, 41), (ErrorKind::MismatchedEndTag, 50)]),
        (b" \xc3\xa9", RECOVER, &[(ErrorKind::InvalidDeclaration, 44), (ErrorKind::MismatchedEndTag, 50)]),
    ];
    for &(inside, flags, expected) in cases {
        let head = b"<?xml version='1.0' encoding='ISO-8859-1'";
        let input = [&head[..], inside, b"?><r>\xe9</s>"].concat();
        let input = &input[..];
        let options = || ParseOptions::new(flags);
        let text = Transcoded::new(input, &options()).unwrap();
        let errors: Vec<_> = match Document::parse_transcoded(&text, options()) {
            Ok(document) => document.errors().to_vec(),
            Err(failure) => vec![failure],
        };
        let mapped: Vec<_> = (errors.iter())
            .map(|e| (e.kind, text.source_offset(e.offset)))
            .collect();
        assert_eq!(mapped, expected, "{input:?}");
        let owned: Vec<_> = match OwnedDocument::parse_with(input, options()) {
            Ok(document) => document
                .errors()
                .iter()
                .map(|e| (e.kind, e.offset))
                .collect(),
            Err(failure) => vec![(failure.kind, failure.offset)],
        };
        assert_eq!(owned, expected, "{input:?}");
    }
}

#[test]
fn transcoded_offsets_describe_libxml2_lines_and_map_to_the_input() {
    let input = utf16("<r>\n\u{e9}\n<a></b></r>", false, true);
    let text = Transcoded::new(&input, &ParseOptions::default()).unwrap();
    let failure = Document::parse_transcoded(&text, ParseOptions::default()).unwrap_err();
    let owned = OwnedDocument::parse_bytes(&input).unwrap_err();
    assert_eq!(text.source_offset(failure.offset), owned.offset);
    let transcoded = failure.describe(&LineIndex::transcoded(&text));
    let expected = owned.describe(&LineIndex::new(&input));
    assert_eq!(
        (transcoded.line, transcoded.column, &transcoded.message),
        (expected.line, expected.column, &expected.message)
    );
    assert_eq!(transcoded.line, 3);
}

#[test]
fn borrowing_constructors_still_reject_undecoded_input() {
    let input = utf16("<r/>", true, true);
    let unsupported = Err(error(ErrorKind::UnsupportedEncoding, 0));
    assert_eq!(Document::parse_bytes(&input).map(drop), unsupported);
    assert_eq!(Reader::new(&input).map(drop), unsupported);
    let text = Transcoded::new(&input, &ParseOptions::default()).unwrap();
    let mut reader = Reader::transcoded(&text, ParseOptions::default()).unwrap();
    assert!(matches!(reader.read_event(), Ok(Some(Event::Empty(_)))));
}
