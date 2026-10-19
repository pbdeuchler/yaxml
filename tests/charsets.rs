//! Declared single-byte encodings on the owning and streaming input paths.

use std::io::Cursor;
use std::ops::{ControlFlow, Range};

use yaxml::{
    BufferedReader, DecodeLimits, Document, Error, ErrorKind, Event, Limit, Limits, OwnedDocument,
    PushParser, ReadError, Writer,
};

fn error(kind: ErrorKind, offset: usize) -> Error {
    Error { kind, offset }
}

/// libxml2 2.15.4 xmllint output per byte, recorded by tooling/charset_oracle.py.
fn oracle() -> impl Iterator<Item = (&'static str, Vec<u32>)> {
    include_str!("fixtures/charsets.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let (label, values) = line.split_once('\t').unwrap();
            let values = values.split(' ');
            (
                label,
                values
                    .map(|v| u32::from_str_radix(v, 16).unwrap())
                    .collect(),
            )
        })
}

fn document(label: &str, body: &[u8]) -> Vec<u8> {
    let head = format!("<?xml version='1.0' encoding='{label}'?><r a='");
    [head.as_bytes(), body, b"'>", body, b"</r>"].concat()
}

/// Text values and their original spans, delivered by one push parse.
fn push(input: &[u8], splits: &[usize]) -> Result<Vec<(String, Range<usize>)>, Error> {
    let mut parser = PushParser::new();
    let mut found = Vec::new();
    let mut emit = |event: Event<'_>, origin: yaxml::SourceOrigin| {
        let value = match event {
            Event::Text(text) => text.into_owned(),
            Event::Start(element) if !element.attributes.is_empty() => {
                element.attributes[0].value.clone().into_owned()
            }
            _ => return ControlFlow::Continue(()),
        };
        found.push((value, origin.input_range().unwrap()));
        ControlFlow::Continue(())
    };
    let mut start = 0;
    for &split in splits.iter().chain([&input.len()]) {
        parser.feed(&input[start..split], &mut emit)?;
        start = split;
    }
    parser.finish(&mut emit)?;
    Ok(found)
}

#[test]
fn every_mapped_byte_decodes_on_each_owning_and_streaming_path() {
    for (label, high) in oracle() {
        let (bytes, text): (Vec<u8>, String) = (0x80..=0xffu8)
            .zip(&high)
            .filter_map(|(byte, &scalar)| {
                Some((byte, char::from_u32(scalar).filter(|c| *c != '\0')?))
            })
            .chain([(b'x', 'x')])
            .unzip();
        let input = document(label, &bytes);
        let head = input.len() - 2 * bytes.len() - 6;
        let tag = head - 6;
        let found = push(&input, &[]).unwrap();
        assert_eq!(
            found,
            [
                (text.clone(), tag..head + bytes.len() + 2),
                (text.clone(), head + bytes.len() + 2..input.len() - 4),
            ],
            "{label}"
        );
        let owned = OwnedDocument::parse_bytes(&input).unwrap();
        let root = owned.root_element();
        let attribute = root.attributes().next().unwrap();
        assert_eq!(attribute.value, text, "{label}");
        assert_eq!(root.text_chunks().collect::<String>(), text, "{label}");
        let mut reader = BufferedReader::new(Cursor::new(&input));
        let mut texts = Vec::new();
        while let Some((event, _)) = reader.read_event().unwrap() {
            if let Event::Text(value) = event {
                texts.push(value.into_owned());
            }
        }
        assert_eq!(texts, std::slice::from_ref(&text), "{label}");
        // The writer emits UTF-8 and relabels the declaration.
        let mut output = Vec::new();
        Writer::new(&mut output)
            .write_owned_document(&owned)
            .unwrap();
        let expected = format!("<?xml version='1.0' encoding='UTF-8'?><r a='{text}'>{text}</r>");
        assert_eq!(
            String::from_utf8(output).unwrap().replace('"', "'"),
            expected
        );
    }
}

#[test]
fn push_results_are_independent_of_every_byte_boundary() {
    for (label, body) in [
        ("ISO-8859-1", &b"\xe9t\xe9"[..]),
        ("latin2", b"\xb1\xe6x"),
        ("ISO-8859-5", b"\xd0\xb0"),
        ("windows-1252", b"\x80\x9f"),
        ("US-ASCII", b"plain"),
    ] {
        let input = document(label, body);
        let expected = push(&input, &[]).unwrap();
        for first in 0..=input.len() {
            assert_eq!(push(&input, &[first]).unwrap(), expected, "{label} {first}");
            for second in first..=input.len().min(first + 3) {
                assert_eq!(push(&input, &[first, second]).unwrap(), expected);
            }
        }
    }
}

#[test]
fn failures_report_original_byte_offsets() {
    let unmapped = |label: &str, body: &[u8], offset| {
        (
            document(label, body),
            error(ErrorKind::UnmappedByte, offset),
        )
    };
    let head = document("ISO-8859-3", b"").len() - 6;
    let cases = [
        unmapped("ISO-8859-3", b"\xa5", head),
        unmapped("iso-8859-3", b"x\xbe", head + 1),
        unmapped("US-ASCII", b"\x80", document("US-ASCII", b"").len() - 6),
        unmapped("ISO-8859-6", b"\xa1", document("ISO-8859-6", b"").len() - 6),
        unmapped(
            "ISO-8859-11",
            b"\xff",
            document("ISO-8859-11", b"").len() - 6,
        ),
        (
            b"<?xml version='1.0' encoding='latin1'?><r>\xe9\xe9</s>".to_vec(),
            error(ErrorKind::MismatchedEndTag, 44),
        ),
        (
            b"<?xml version='1.0' encoding='latin1'?><r>\xe9\x01</r>".to_vec(),
            error(ErrorKind::InvalidCharacter, 43),
        ),
        (
            b"<?xml version='1.0' encoding='Shift_JIS'?><r/>".to_vec(),
            error(ErrorKind::UnsupportedEncoding, 20),
        ),
        (
            b"<?xml version='1.0' encoding='latin-1'?><r/>".to_vec(),
            error(ErrorKind::UnsupportedEncoding, 20),
        ),
        (
            b"\xef\xbb\xbf<?xml version='1.0' encoding='latin1'?><r/>".to_vec(),
            error(ErrorKind::UnsupportedEncoding, 23),
        ),
        (
            [
                &[0xff, 0xfe][..],
                &"<?xml version='1.0' encoding='latin1'?><r/>"
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>(),
            ]
            .concat(),
            error(ErrorKind::EncodingMismatch, 42),
        ),
        (
            b"Lo\xa7\x94\x93@\xa5\x85\x99".to_vec(),
            error(ErrorKind::UnsupportedEncoding, 0),
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(push(&input, &[]).unwrap_err(), expected, "{input:?}");
        for split in 0..=input.len() {
            assert_eq!(push(&input, &[split]).unwrap_err(), expected, "{split}");
        }
        assert_eq!(OwnedDocument::parse_bytes(&input).unwrap_err(), expected);
        let mut reader = BufferedReader::new(Cursor::new(&input));
        let error = loop {
            match reader.read_event() {
                Ok(Some(_)) => {}
                Ok(None) => panic!("accepted {input:?}"),
                Err(ReadError::Xml(error)) => break error,
                Err(error) => panic!("{error}"),
            }
        };
        assert_eq!(error, expected);
    }
}

#[test]
fn borrowing_reader_keeps_its_utf8_contract() {
    let ascii = document("ISO-8859-1", b"x");
    assert_eq!(
        Document::parse_bytes(&ascii).unwrap_err(),
        error(ErrorKind::UnsupportedEncoding, 20)
    );
    assert!(OwnedDocument::parse_bytes(&ascii).is_ok());
}

#[test]
fn decoded_ceilings_count_utf8_output() {
    let input = document("ISO-8859-15", b"\xa4\xa4");
    let limit = |max_decoded_bytes| {
        OwnedDocument::with_decode_limits(
            &input,
            Limits::default(),
            DecodeLimits {
                max_decoded_bytes,
                ..DecodeLimits::default()
            },
        )
        .map(|_| ())
    };
    // Each U+20AC is three UTF-8 bytes for one source byte.
    let decoded = input.len() + 4 * 2;
    assert_eq!(limit(decoded), Ok(()));
    assert_eq!(
        limit(decoded - 1),
        Err(error(ErrorKind::LimitExceeded(Limit::DecodedBytes), 0))
    );
}
