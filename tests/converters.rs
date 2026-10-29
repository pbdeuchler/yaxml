//! Caller-supplied input converters ([`yaxml::Converters`]) on every surface.

use std::io::BufReader;
use std::ops::ControlFlow;
use std::sync::Arc;

use yaxml::{
    BufferedReader, Converter, Converters, Document, Error, ErrorKind, Event, OwnedDocument,
    ParseOptions, PushParser, ReadError, Reader, Transcoded, options,
};

/// Bytes 0x80..=0xFF are U+0400 + byte - 0x80; NUL is unmapped.
struct Shifted;

impl Converter for Shifted {
    fn decode(&mut self, input: &[u8], output: &mut String, _last: bool) -> bool {
        for &byte in input {
            match byte {
                0 => return false,
                0x80.. => output.push(char::from_u32(0x380 + u32::from(byte)).unwrap()),
                _ => output.push(char::from(byte)),
            }
        }
        true
    }
}

/// 0xFF and the next byte b are U+0100 + b, so a pair can straddle calls.
#[derive(Default)]
struct Pairs {
    lead: bool,
}

impl Converter for Pairs {
    fn decode(&mut self, input: &[u8], output: &mut String, last: bool) -> bool {
        for &byte in input {
            match (std::mem::take(&mut self.lead), byte) {
                (true, byte) => output.push(char::from_u32(0x100 + u32::from(byte)).unwrap()),
                (false, 0xff) => self.lead = true,
                (false, byte) => output.push(char::from(byte)),
            }
        }
        !(last && self.lead)
    }
}

/// `x-pairs` opens [`Pairs`], `x-unknown` nothing, any other label [`Shifted`].
struct Labels;

impl Converters for Labels {
    fn open(&self, label: &str) -> Option<Box<dyn Converter>> {
        match label.to_ascii_lowercase().as_str() {
            "x-unknown" => None,
            "x-pairs" => Some(Box::<Pairs>::default()),
            _ => Some(Box::new(Shifted)),
        }
    }
}

fn options(flags: u32, converters: bool) -> ParseOptions {
    let converters = converters.then(|| Arc::new(Labels) as Arc<dyn Converters>);
    ParseOptions::new(flags).converters(converters)
}

type Outcome = Result<String, Error>;

fn owned(input: &[u8], options: ParseOptions) -> Outcome {
    let document = OwnedDocument::parse_with(input, options)?;
    Ok(document.root_element().text_chunks().collect())
}

fn transcoded(input: &[u8], options: ParseOptions) -> Outcome {
    let text = Transcoded::new(input, &options)?;
    let document = Document::parse_transcoded(&text, options)?;
    Ok(document.root_element().text_chunks().collect())
}

fn read(input: &[u8], options: ParseOptions) -> Outcome {
    let text = Transcoded::new(input, &options)?;
    let mut reader = Reader::transcoded(&text, options)?;
    let mut out = String::new();
    while let Some(event) = reader.read_event()? {
        if let Event::Text(text) = event {
            out += &text;
        }
    }
    Ok(out)
}

fn pushed(input: &[u8], options: ParseOptions, chunk: usize) -> Outcome {
    let mut parser = PushParser::with_options(options)?;
    let mut out = String::new();
    let mut emit = |event: &Event<'_>, _| {
        if let Event::Text(text) = event {
            out += text;
        }
        ControlFlow::Continue(())
    };
    for piece in input.chunks(chunk.max(1)) {
        parser.feed(piece, &mut emit)?;
    }
    parser.finish(&mut emit)?;
    Ok(out)
}

fn buffered(input: &[u8], options: ParseOptions, capacity: usize) -> Outcome {
    let source = BufReader::with_capacity(capacity, input);
    let mut reader = BufferedReader::with_options(source, options)?;
    let mut out = String::new();
    loop {
        match reader.read_event() {
            Ok(Some((Event::Text(text), _))) => out += &text,
            Ok(Some(_)) => {}
            Ok(None) => return Ok(out),
            Err(ReadError::Xml(error)) => return Err(error),
            Err(error) => panic!("{error}"),
        }
    }
}

#[cfg(feature = "textreader")]
fn text_reader(input: &[u8], options: ParseOptions) -> Outcome {
    let mut reader = yaxml::TextReader::with_options(input, options)?;
    let mut out = String::new();
    loop {
        match reader.read() {
            Ok(true) if reader.node_type() == yaxml::NodeType::Text => {
                out += reader.value().unwrap_or_default();
            }
            Ok(true) => {}
            Ok(false) => return Ok(out),
            Err(ReadError::Xml(error)) => return Err(error),
            Err(error) => panic!("{error}"),
        }
    }
}

/// Every surface's outcome for `input`, checked equal; pushes and buffered
/// reads in every chunk size.
fn parse(input: &[u8], options: impl Fn() -> ParseOptions) -> Outcome {
    let expected = owned(input, options());
    let label = String::from_utf8_lossy(input);
    assert_eq!(transcoded(input, options()), expected, "Document {label}");
    assert_eq!(read(input, options()), expected, "Reader {label}");
    #[cfg(feature = "textreader")]
    assert_eq!(
        text_reader(input, options()),
        expected,
        "TextReader {label}"
    );
    for chunk in 1..=input.len() {
        assert_eq!(
            pushed(input, options(), chunk),
            expected,
            "push/{chunk} {label}"
        );
        assert_eq!(
            buffered(input, options(), chunk),
            expected,
            "buffered/{chunk} {label}"
        );
    }
    expected
}

const SHIFTED: &[u8] = b"<?xml version='1.0' encoding='x-shifted'?><r>\x90\x91</r>";

#[test]
fn converters_read_declared_labels_on_every_surface() {
    let declared = |label: &str, body: &[u8]| {
        [
            format!("<?xml version=\"1.0\" encoding=\"{label}\"?>").as_bytes(),
            body,
        ]
        .concat()
    };
    let unmapped = |input: &[u8]| {
        Err(Error {
            kind: ErrorKind::UnmappedByte,
            offset: input.len(),
        })
    };
    let cases: [(Vec<u8>, Outcome); 8] = [
        (SHIFTED.to_vec(), Ok("\u{410}\u{411}".into())),
        // A pair split across chunks; the label matches in any case.
        (
            declared("X-Pairs", b"<r>a\xff\x41b</r>"),
            Ok("a\u{141}b".into()),
        ),
        // Built-in charsets keep precedence over a converter for any label.
        (declared("ISO-8859-1", b"<r>\xe9</r>"), Ok("\u{e9}".into())),
        (declared("UTF-8", b"<r>\xc3\xa9</r>"), Ok("\u{e9}".into())),
        // No declaration, or a PI that only starts like one: UTF-8.
        (b"<r>\xc3\xa9</r>".to_vec(), Ok("\u{e9}".into())),
        (b"<?xm-pi x?><r>\xc3\xa9</r>".to_vec(), Ok("\u{e9}".into())),
        // An unmapped byte fails at its offset in the converted text, an
        // incomplete sequence at the end of the input.
        (
            declared("x-shifted", b"<r>\x90\x00</r>"),
            unmapped(&declared("x-shifted", b"<r>\xd0\x90")),
        ),
        (
            declared("x-pairs", b"<r/>\xff"),
            unmapped(&declared("x-pairs", b"<r/>")),
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(
            parse(&input, || options(0, true)),
            expected,
            "{}",
            String::from_utf8_lossy(&input)
        );
    }
}

#[test]
fn labels_no_converter_opens_stay_unsupported() {
    for converters in [false, true] {
        for label in ["x-unknown", "x-shifted"] {
            let input = format!("<?xml version='1.0' encoding='{label}'?><r/>");
            let outcome = parse(input.as_bytes(), || options(0, converters));
            match (converters, label) {
                (true, "x-shifted") => assert_eq!(outcome, Ok(String::new())),
                _ => assert_eq!(outcome.unwrap_err().kind, ErrorKind::UnsupportedEncoding),
            }
        }
    }
}

#[test]
fn ignore_enc_disables_converters() {
    let ignored = parse(SHIFTED, || options(options::IGNORE_ENC, true));
    assert_eq!(
        ignored,
        parse(SHIFTED, || options(options::IGNORE_ENC, false))
    );
    assert!(ignored.is_err());
}

#[test]
fn recovery_keeps_an_unmapped_byte_as_built_in_charsets_do() {
    // As for an ISO-8859-7 0xFF: the error is kept at its converted offset.
    let input = b"<?xml version='1.0' encoding='x-shifted'?><r>\x90\x00</r>";
    let document = OwnedDocument::parse_with(input, options(options::RECOVER, true)).unwrap();
    let offset = input.len() - "\x00</r>".len() + 1;
    assert_eq!(
        document.errors(),
        [Error {
            kind: ErrorKind::UnmappedByte,
            offset
        }]
    );
}

#[test]
fn converted_input_reaches_the_dtd_and_spans_conversion_windows() {
    let dtd = b"<?xml version='1.0' encoding='x-shifted'?><!DOCTYPE r [<!ENTITY e '\x90'>]><r>&e;\x91</r>";
    assert_eq!(
        parse(dtd, || options(options::NOENT, true)),
        Ok("\u{410}\u{411}".into())
    );
    let long = [&SHIFTED[..SHIFTED.len() - 6], &[0x90; 40_000], b"</r>"].concat();
    let expected = Ok("\u{410}".repeat(40_000));
    assert_eq!(owned(&long, options(0, true)), expected);
    assert_eq!(transcoded(&long, options(0, true)), expected);
    for chunk in [1, 7, 4096, long.len()] {
        assert_eq!(
            pushed(&long, options(0, true), chunk),
            expected,
            "push/{chunk}"
        );
        assert_eq!(
            buffered(&long, options(0, true), chunk),
            expected,
            "buffered/{chunk}"
        );
    }
    // A declaration past 4 KiB is read as UTF-8, whose label is unknown.
    let padded = format!("<?xml version='1.0'{:4096} encoding='x-shifted'?><r/>", "");
    let outcome = owned(padded.as_bytes(), options(0, true));
    assert_eq!(outcome.unwrap_err().kind, ErrorKind::UnsupportedEncoding);
}
