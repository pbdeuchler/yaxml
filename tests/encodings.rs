//! UCS-4 and EBCDIC input (feature `encodings`), pinned against xmllint
//! 2.15.4 built with the macOS iconv: every surface decodes alike, push
//! input at any split agrees, and `save` writes the declared encoding.

use std::ops::ControlFlow;

use yaxml::{
    Document, Error, ErrorKind, OwnedDocument, ParseOptions, PushParser, Transcoded, save,
};

fn error(kind: ErrorKind, offset: usize) -> Error {
    Error { kind, offset }
}

fn ucs4(text: &str, little: bool) -> Vec<u8> {
    text.chars()
        .flat_map(|c| match little {
            true => u32::from(c).to_le_bytes(),
            false => u32::from(c).to_be_bytes(),
        })
        .collect()
}

fn hex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(u8::is_ascii_hexdigit).collect();
    let digit = |d: u8| (d as char).to_digit(16).unwrap() as u8;
    digits
        .chunks(2)
        .map(|pair| digit(pair[0]) << 4 | digit(pair[1]))
        .collect()
}

/// `<?xml version="1.0" encoding="IBM037"?>`, then `<r a="é">ü¢</r>`.
const IBM037: &str = "4c6fa794 9340a585 99a28996 957e7ff1 4bf07f40 85958396 84899587
    7e7fc9c2 d4f0f3f7 7f6f6e4c 9940817e 7f517f6e dc4a4c61 996e";
/// The same bytes declared IBM500, where 0x4a/0x5a are `[`/`]`.
const IBM500: &str = "4c6fa794 9340a585 99a28996 957e7ff1 4bf07f40 85958396 84899587
    7e7fc9c2 d4f5f0f0 7f6f6e4c 9940817e 7f517f6e 4adc5a4c 61996e";
/// `<?xml version="1.0"?><r>a</r>`, read in IBM037.
const UNDECLARED: &str = "4c6fa794 9340a585 99a28996 957e7ff1 4bf07f6f 6e4c996e 814c6199 6e";

type Outcome = Result<(String, Option<String>), Error>;

fn owned(input: &[u8]) -> Outcome {
    let document = OwnedDocument::parse_bytes(input)?;
    let root = document.document_element().and_then(|root| root.content());
    let label = document.declaration().and_then(|d| d.encoding);
    Ok((root.unwrap_or_default(), label.map(str::to_owned)))
}

fn transcoded(input: &[u8]) -> Outcome {
    let text = Transcoded::new(input, &ParseOptions::default())?;
    let document = Document::parse_transcoded(&text, ParseOptions::default())
        .map_err(|e| error(e.kind, text.source_offset(e.offset)))?;
    let root = document.document_element().and_then(|root| root.content());
    let label = document.declaration().and_then(|d| d.encoding);
    Ok((root.unwrap_or_default(), label.map(str::to_owned)))
}

fn pushed(input: &[u8], split: usize) -> Result<(), Error> {
    let mut parser = PushParser::new();
    let mut emit = |_: &yaxml::Event<'_>, _| ControlFlow::Continue(());
    parser.feed(&input[..split], &mut emit)?;
    parser.feed(&input[split..], &mut emit)?;
    parser.finish(&mut emit)
}

#[test]
fn ucs4_and_ebcdic_decode_like_libxml2_with_iconv() {
    let ok = |text: &str, label: Option<&str>| Ok((text.into(), label.map(Into::into)));
    let declared =
        |label: &str| format!("<?xml version=\"1.0\" encoding=\"{label}\"?><r a=\"é\">ü€</r>");
    let mut cut = ucs4("<r/>", false);
    cut.extend([0, 0]);
    let cases: Vec<(&str, Vec<u8>, Outcome)> = vec![
        (
            "ucs-4 be",
            ucs4(&declared("UCS-4"), false),
            ok("ü€", Some("UCS-4")),
        ),
        (
            "ucs-4 le",
            ucs4(&declared("UCS-4"), true),
            ok("ü€", Some("UCS-4")),
        ),
        // Detection wins; libxml2 records any label unchecked, unknown too.
        (
            "latin-1 label",
            ucs4(&declared("ISO-8859-1"), true),
            ok("ü€", Some("ISO-8859-1")),
        ),
        (
            "unknown label",
            ucs4(&declared("x-none"), false),
            ok("ü€", Some("x-none")),
        ),
        ("ucs-4 undeclared", ucs4("<r>é</r>", false), ok("é", None)),
        // 2.15.4 detects no UCS-4 BOM ("Document is empty") nor 2143 order.
        (
            "ucs-4 bom",
            ucs4("\u{feff}<r/>", false),
            Err(error(ErrorKind::UnsupportedEncoding, 0)),
        ),
        (
            "2143 order",
            b"\0\0<\0\0\0r\0\0\0/\0\0\0>\0".to_vec(),
            Err(error(ErrorKind::UnsupportedEncoding, 0)),
        ),
        (
            "past the scalars",
            [ucs4("<r>", false), vec![0, 0x11, 0, 0], ucs4("</r>", false)].concat(),
            Err(error(ErrorKind::InvalidUtf16, 12)),
        ),
        (
            "surrogate",
            [ucs4("<r>", true), vec![0, 0xd8, 0, 0], ucs4("</r>", true)].concat(),
            Err(error(ErrorKind::InvalidUtf16, 12)),
        ),
        ("cut unit", cut, Err(error(ErrorKind::InvalidUtf16, 16))),
        ("ibm037", hex(IBM037), ok("ü¢", Some("IBM037"))),
        // Read in IBM037 up to `?>`, then in the declared page.
        ("ibm500", hex(IBM500), ok("[ü]", Some("IBM500"))),
        ("ebcdic undeclared", hex(UNDECLARED), ok("a", None)),
        // libxml2 fails "detecting EBCDIC" for a label iconv lacks.
        (
            "ibm1047",
            hex(&IBM037.replace("c9c2 d4f0f3f7", "c9c2 d4f1f0f4f7")),
            Err(error(ErrorKind::UnsupportedEncoding, 20)),
        ),
        (
            "non-ebcdic label",
            hex(&IBM037.replace("c9c2 d4f0f3f7", "e4e3 c660f8")),
            Err(error(ErrorKind::UnsupportedEncoding, 20)),
        ),
    ];
    for (name, input, expected) in cases {
        assert_eq!(owned(&input), expected, "{name}");
        assert_eq!(transcoded(&input), expected, "{name}: transcoded");
        for split in 0..=input.len() {
            assert_eq!(
                pushed(&input, split).err(),
                expected.as_ref().err().copied(),
                "{name}: split {split}"
            );
        }
    }
}

#[test]
fn save_writes_the_declared_ucs4_or_ebcdic_encoding() {
    let save = |input: &[u8]| {
        let document = OwnedDocument::parse_bytes(input).unwrap();
        save::to_vec(&document, &save::Options::default()).unwrap()
    };
    // xmllint's output: the input with a newline after the declaration and at the end.
    let ebcdic = hex(IBM037);
    let split = ebcdic.windows(2).position(|w| w == [0x6f, 0x6e]).unwrap() + 2;
    let newline = 0x25;
    let expected = [&ebcdic[..split], &[newline], &ebcdic[split..], &[newline]].concat();
    assert_eq!(save(&ebcdic), expected);
    let document = "<?xml version=\"1.0\" encoding=\"{}\"?>\n<r a=\"é\">ü€</r>\n";
    for (label, little, bom) in [
        ("UCS-4", false, ""),
        ("UCS-4LE", true, ""),
        ("UTF-32", false, "\u{feff}"),
    ] {
        let text = document.replace("{}", label);
        let input = ucs4(text.trim_end(), little);
        assert_eq!(
            save(&input),
            ucs4(&format!("{bom}{text}"), little),
            "{label}"
        );
    }
}
