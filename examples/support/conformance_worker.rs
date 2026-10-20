//! Bounded, terminal parse observations for the XML conformance runner.

use std::io::{BufReader, Cursor};
use std::ops::ControlFlow;
use std::path::Path;

use yaxml::options::{DTDLOAD, LEGACY};
use yaxml::{
    BufferedReader, Document, Error, FileResolver, Limits, OwnedDocument, ParseOptions, PushParser,
    ReadError, Reader,
};

pub const SURFACES: [&str; 5] = ["reader", "document", "owned", "push", "buffered"];
pub const MAX_CASE_BYTES: usize = 1 << 20;

fn limits() -> Limits {
    {
        let mut cfg = Limits::default();
        cfg.max_input_bytes = MAX_CASE_BYTES;
        cfg
    }
}

/// Runs one public parsing surface through terminal EOF validation. With
/// `context` (suite root, case path), external resources load from the root.
///
/// The inner result is the parser's XML result. The outer error is reserved for
/// runner misuse or non-XML I/O failure and must not count as an XML rejection.
pub fn observe(
    surface: &str,
    input: &[u8],
    context: Option<(&Path, &str)>,
) -> Result<Result<(), Error>, String> {
    if input.len() > MAX_CASE_BYTES {
        return Err(format!(
            "case has {} bytes; maximum is {MAX_CASE_BYTES}",
            input.len()
        ));
    }
    // With a corpus root, entities and the external subset load as libxml2's
    // `NOENT | DTDLOAD | DTDATTR` load them.
    let options = || -> Result<ParseOptions, String> {
        let options = ParseOptions::default().with_limits(limits());
        let Some((root, path)) = context else {
            return Ok(options);
        };
        let resolver = FileResolver::new(root).map_err(|error| error.to_string())?;
        let options = {
            let mut cfg = options;
            cfg.flags = LEGACY | DTDLOAD;
            cfg
        };
        Ok(options.with_resolver(path, Box::new(resolver)))
    };

    match surface {
        "reader" => {
            let options = options()?;
            let result = (|| {
                let mut reader = Reader::with_options(input, options)?;
                while reader.read_event()?.is_some() {}
                Ok(())
            })();
            Ok(result)
        }
        "document" => Ok(Document::parse_with(input, options()?).map(drop)),
        "owned" => Ok(OwnedDocument::parse_with(input, options()?).map(drop)),
        "push" => {
            const CHUNKS: [usize; 3] = [1, 7, 4096];
            let mut parser =
                PushParser::with_options(options()?).map_err(|error| error.to_string())?;
            let mut cursor = 0;
            let mut next_chunk = 0;
            let result = (|| {
                while cursor < input.len() {
                    let length = CHUNKS[next_chunk % CHUNKS.len()].min(input.len() - cursor);
                    let end = cursor + length;
                    parser.feed(&input[cursor..end], |_, _| ControlFlow::Continue(()))?;
                    cursor = end;
                    next_chunk += 1;
                }
                parser.finish(|_, _| ControlFlow::Continue(()))
            })();
            Ok(result)
        }
        "buffered" => {
            let input = BufReader::with_capacity(17, Cursor::new(input));
            let mut reader = BufferedReader::with_options(input, options()?)
                .map_err(|error| error.to_string())?;
            loop {
                match reader.read_event() {
                    Ok(Some(_)) => {}
                    Ok(None) => return Ok(Ok(())),
                    Err(ReadError::Xml(error)) => return Ok(Err(error)),
                    Err(ReadError::Io(error)) => {
                        return Err(format!("buffered input failed: {error}"));
                    }
                    Err(error) => return Err(format!("buffered input failed: {error}")),
                }
            }
        }
        _ => Err(format!("unknown conformance surface: {surface}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yaxml::{ErrorKind, Limit};

    fn inner(surface: &str, input: &[u8]) -> Result<(), Error> {
        observe(surface, input, None).expect("literal case is within the worker contract")
    }

    fn utf16_le(input: &str) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xfe];
        for unit in input.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn every_surface_accepts_and_rejects_complete_utf8_documents() {
        for surface in SURFACES {
            assert_eq!(
                inner(surface, b"<r xmlns='u'><c/></r>"),
                Ok(()),
                "{surface}"
            );
            assert_eq!(
                inner(surface, b"<r>"),
                Err(Error {
                    kind: ErrorKind::UnexpectedEof,
                    offset: 3,
                }),
                "{surface}"
            );
            assert_eq!(
                inner(surface, b"<r><a></r>"),
                Err(Error {
                    kind: ErrorKind::MismatchedEndTag,
                    offset: 6,
                }),
                "{surface}"
            );
        }
    }

    #[test]
    fn owning_and_streaming_surfaces_decode_utf16() {
        let input = utf16_le("<?xml version='1.0' encoding='UTF-16'?><r/>");
        for surface in ["reader", "document"] {
            assert_eq!(
                inner(surface, &input),
                Err(Error {
                    kind: ErrorKind::UnsupportedEncoding,
                    offset: 0,
                }),
                "{surface}"
            );
        }
        for surface in ["owned", "push", "buffered"] {
            assert_eq!(inner(surface, &input), Ok(()), "{surface}");
        }
    }

    #[test]
    fn default_parser_resource_errors_remain_xml_results() {
        let mut input = String::new();
        for _ in 0..=Limits::default().max_depth {
            input.push_str("<r>");
        }
        for _ in 0..=Limits::default().max_depth {
            input.push_str("</r>");
        }
        for surface in SURFACES {
            assert!(
                matches!(
                    inner(surface, input.as_bytes()),
                    Err(Error {
                        kind: ErrorKind::LimitExceeded(Limit::Depth),
                        ..
                    })
                ),
                "{surface}"
            );
        }
    }

    #[test]
    fn push_observation_finishes_trailing_and_incomplete_input() {
        for input in [b"<r>".as_slice(), b"<r/>trailing".as_slice()] {
            let mut feed_only = PushParser::with_limits(limits());
            feed_only
                .feed(input, |_, _| ControlFlow::Continue(()))
                .expect("the terminal error must require finish");
            assert!(inner("push", input).is_err());
        }
    }

    #[test]
    fn caller_errors_precede_parser_construction() {
        assert!(
            observe("missing", b"<r/>", None)
                .unwrap_err()
                .contains("unknown")
        );

        let oversized = vec![b'x'; MAX_CASE_BYTES + 1];
        for surface in SURFACES {
            let error = observe(surface, &oversized, None).unwrap_err();
            assert!(error.contains("maximum"), "{surface}: {error}");
        }
    }
}
