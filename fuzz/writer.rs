//! Writer operations are caller input, independently of parsed XML bytes:
//! `Writer` events and sinks, and `TextWriter` call scripts.

use std::borrow::Cow;
use std::cell::Cell;
use std::io::{self, Write};
use std::ops::ControlFlow;

use yaxml::{
    Attribute, Declaration, Document, Element, ErrorKind, Event, Limit, Name, PushParser, Reader,
    WriteError, Writer, WriterLimits,
};

pub(super) fn exercise(input: &[u8], accepted: bool) {
    if accepted {
        roundtrip(input);
    }
    operations(input);
    output_schedule(input);
    text_scripts(input);
}

fn roundtrip(input: &[u8]) {
    // Escaping can expand an originally literal attribute quote sixfold, and
    // entity expansion can make parsed values larger than the input.
    let ceiling = super::expanded_bound(input.len())
        .saturating_mul(6)
        .saturating_add(1024);
    let mut writer = Writer::with_limits(
        Vec::new(),
        WriterLimits {
            max_output_bytes: ceiling,
            max_token_bytes: ceiling,
            ..WriterLimits::default()
        },
    );
    let mut reader = Reader::new(input).unwrap();
    while let Some(event) = reader.read_event().unwrap() {
        if let Err(error) = writer.write_event(&event) {
            // Owning open names/namespaces spends more than borrowing input.
            assert!(
                matches!(error, WriteError::Xml(error) if error.kind == ErrorKind::LimitExceeded(Limit::OwnedBytes)),
                "valid parsed event was rejected by writer: {error}"
            );
            assert_terminal(&mut writer);
            return;
        }
    }
    writer.finish().unwrap();
    writer.finish().unwrap();
    let position = writer.position();
    let output = writer.into_inner();
    assert_eq!(position, output.len());
    assert!(output.len() <= ceiling);
    limited(input, &output);
    let limits = yaxml::Limits {
        max_input_bytes: ceiling,
        max_token_bytes: ceiling,
        ..yaxml::Limits::default()
    };
    assert!(
        logical(Reader::with_limits(&output, limits).unwrap())
            == logical(Reader::new(input).unwrap()),
        "serialized values changed"
    );
    Document::with_limits(&output, limits).unwrap();
    let mut streaming = PushParser::with_limits(limits);
    streaming
        .feed(&output, |_, _| ControlFlow::Continue(()))
        .unwrap();
    streaming.finish(|_, _| ControlFlow::Continue(())).unwrap();
}

/// Replays the events under one input-selected ceiling, often below what the
/// complete output needs: the writer either produces the same bytes or fails
/// on a limit, after which it is terminal.
fn limited(input: &[u8], expected: &[u8]) {
    let control = super::selector(input);
    let fraction = |total: usize| total * usize::from(control >> 3) / 32;
    let generous = WriterLimits::default();
    let limits = match control & 7 {
        0 => WriterLimits {
            max_output_bytes: fraction(expected.len()),
            ..generous
        },
        1 => WriterLimits {
            max_token_bytes: fraction(expected.len()),
            ..generous
        },
        2 => WriterLimits {
            max_depth: usize::from(control >> 3) & 7,
            ..generous
        },
        3 => WriterLimits {
            max_attributes: usize::from(control >> 3) & 3,
            ..generous
        },
        4 => WriterLimits {
            max_events: usize::from(control >> 3),
            ..generous
        },
        5 => WriterLimits {
            max_owned_bytes: fraction(4 * expected.len()),
            ..generous
        },
        _ => return,
    };
    let limit_error = |error: &WriteError| matches!(error, WriteError::Xml(error) if matches!(error.kind, ErrorKind::LimitExceeded(_)));
    let mut writer = Writer::with_limits(Vec::new(), limits);
    let mut reader = Reader::new(input).unwrap();
    while let Some(event) = reader.read_event().unwrap() {
        if let Err(error) = writer.write_event(&event) {
            assert!(limit_error(&error), "limited writer failed with {error}");
            assert_terminal(&mut writer);
            return;
        }
    }
    match writer.finish() {
        Ok(()) => assert_eq!(writer.into_inner(), expected),
        Err(error) => assert!(limit_error(&error), "limited finish failed with {error}"),
    }
}

/// Events with adjacent Text coalesced and entity boundaries dropped: entity
/// expansion can split one run.
fn logical(mut reader: Reader<'_>) -> Vec<String> {
    let mut events: Vec<String> = Vec::new();
    while let Some(event) = reader.read_event().unwrap() {
        match (&event, events.last_mut()) {
            // The writer expands references; their boundaries carry no value.
            (Event::EntityStart(_) | Event::EntityEnd, _) => {}
            (Event::Text(text), Some(last)) if last.starts_with("Text:") => last.push_str(text),
            (Event::Text(text), _) => events.push(format!("Text:{text}")),
            _ => events.push(format!("{event:?}")),
        }
    }
    events
}

fn name(qualified: &str) -> Name<'_> {
    Name::new(qualified, None).unwrap()
}

fn empty() -> Event<'static> {
    Event::Empty(Element {
        name: name("r"),
        attributes: Vec::new().into(),
    })
}

fn assert_terminal<W: Write>(writer: &mut Writer<W>) {
    let position = writer.position();
    for result in [
        writer.finish(),
        writer.write_event(&empty()),
        writer.finish(),
    ] {
        assert!(
            matches!(result, Err(WriteError::Xml(error)) if error.kind == ErrorKind::InvalidState)
        );
    }
    assert_eq!(writer.position(), position);
}

fn operations(input: &[u8]) {
    let byte = |index| input.get(index).copied().unwrap_or(0) as usize;
    let mut limits = WriterLimits {
        max_output_bytes: 16 * 1024,
        max_token_bytes: 4096,
        ..WriterLimits::default()
    };
    if byte(0) & 1 == 0 {
        limits.max_output_bytes = byte(1) * 8;
        limits.max_token_bytes = byte(2);
        limits.max_depth = byte(3) & 7;
        limits.max_attributes = byte(4) & 3;
        limits.max_events = byte(5);
        limits.max_owned_bytes = byte(6) * 16;
    }
    let mut writer = Writer::with_limits(Vec::new(), limits);
    // Seven policy/I/O bytes, an operation count, up to 31 operation bytes,
    // then at most 96 bytes of shared logical data. Keep control bytes out of
    // the payload so mutations can reach valid sequences and invalid values.
    let count = (byte(7) & 31).min(input.len().saturating_sub(8));
    let start = 8.min(input.len());
    let payload_start = start + count;
    let payload =
        String::from_utf8_lossy(&input[payload_start..input.len().min(payload_start + 96)]);
    let mut failed = false;
    for control in input[start..payload_start].iter().copied() {
        let event = match control & 7 {
            0..=2 => {
                let qualified = if control & 64 != 0 {
                    payload.as_ref()
                } else {
                    "r"
                };
                let (qualified, namespace) = match (control >> 4) & 3 {
                    1 => ("different", None),
                    2 => ("p:r", None),
                    3 => (qualified, Some(Cow::Borrowed("urn:undeclared"))),
                    _ => (qualified, None),
                };
                let element_name = match Name::new(qualified, namespace) {
                    Ok(name) => name,
                    Err(error) => {
                        assert_eq!(error.kind, ErrorKind::InvalidName);
                        super::check_display(&error);
                        // No writer operation occurred; continue the sequence without poisoning it.
                        continue;
                    }
                };
                if control & 7 == 0 {
                    let qualified = if control & 128 != 0 {
                        payload.as_ref()
                    } else if control & 8 == 0 {
                        "a"
                    } else {
                        "xmlns"
                    };
                    let attribute_name = match Name::new(qualified, None) {
                        Ok(name) => name,
                        Err(error) => {
                            assert_eq!(error.kind, ErrorKind::InvalidName);
                            super::check_display(&error);
                            continue;
                        }
                    };
                    Event::Start(Element {
                        name: element_name,
                        attributes: vec![Attribute {
                            name: attribute_name,
                            value: Cow::Borrowed(&payload),
                        }]
                        .into(),
                    })
                } else if control & 7 == 1 {
                    Event::Empty(Element {
                        name: element_name,
                        attributes: Vec::new().into(),
                    })
                } else {
                    Event::End(element_name)
                }
            }
            3 => Event::Text(Cow::Borrowed(&payload)),
            4 => Event::Cdata(Cow::Borrowed(&payload)),
            5 => Event::Comment(Cow::Borrowed(&payload)),
            6 => Event::ProcessingInstruction {
                target: Cow::Borrowed(if control & 8 == 0 { "pi" } else { &payload }),
                data: Cow::Borrowed(&payload),
            },
            _ => Event::Declaration(Declaration {
                version: if control & 8 == 0 { "1.0" } else { &payload },
                encoding: if control & 16 == 0 {
                    None
                } else {
                    Some(&payload)
                },
                standalone: if control & 32 == 0 { None } else { Some(false) },
            }),
        };
        // Reparse alone is insufficient: early raw delimiters can turn part of
        // a caller's value into other legal markup or text. These values must
        // be rejected even if that altered document would still be well formed.
        let forbidden_delimiter = match &event {
            Event::Cdata(value) => value.contains("]]>"),
            Event::Comment(value) => value.contains("--") || value.ends_with('-'),
            Event::ProcessingInstruction { data, .. } => data.contains("?>"),
            _ => false,
        };
        let inconsistent_metadata = match &event {
            Event::Start(element) | Event::Empty(element) => {
                // This generator cannot supply a coherent XMLNS attribute, so
                // no successful operation can establish a namespace binding.
                element.name.namespace().is_some()
                    || element.attributes.iter().any(|attribute| {
                        (attribute.name.qualified() == "xmlns"
                            && attribute.name.namespace() != Some("http://www.w3.org/2000/xmlns/"))
                            || (attribute.name.prefix().is_none()
                                && attribute.name.qualified() != "xmlns"
                                && attribute.name.namespace().is_some())
                    })
            }
            Event::End(name) => name.namespace().is_some(),
            _ => false,
        };
        let previous = writer.position();
        let result = writer.write_event(&event);
        assert!(
            !(forbidden_delimiter || inconsistent_metadata) || result.is_err(),
            "writer accepted forbidden delimiters or inconsistent name metadata"
        );
        if let Err(error) = result {
            assert!(
                matches!(error, WriteError::Xml(_)),
                "Vec sink returned I/O error"
            );
            super::check_display(&error);
            assert!(std::error::Error::source(&error).is_some());
            assert_eq!(writer.position(), previous, "invalid event reached output");
            assert_terminal(&mut writer);
            failed = true;
            break;
        }
        assert!((previous..=limits.max_output_bytes).contains(&writer.position()));
    }
    if !failed {
        match writer.finish() {
            Ok(()) => {
                writer.finish().unwrap();
                assert_terminal_after_finish(&mut writer);
            }
            Err(error) => {
                assert!(
                    matches!(error, WriteError::Xml(_)),
                    "Vec flush returned I/O error"
                );
                assert_terminal(&mut writer);
                failed = true;
            }
        }
    }
    let position = writer.position();
    let output = writer.into_inner();
    assert_eq!(position, output.len());
    assert!(output.len() <= limits.max_output_bytes);
    if !failed {
        Document::parse_bytes(&output).unwrap();
    }
}

fn assert_terminal_after_finish<W: Write>(writer: &mut Writer<W>) {
    assert!(
        matches!(writer.write_event(&empty()), Err(WriteError::Xml(error)) if error.kind == ErrorKind::InvalidState)
    );
    assert_terminal(writer);
}

fn output_schedule(input: &[u8]) {
    struct Sink<'a> {
        bytes: Vec<u8>,
        calls: &'a Cell<usize>,
        flushes: &'a Cell<usize>,
        mode: u8,
        boundary: usize,
    }
    impl Write for Sink<'_> {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            let call = self.calls.get();
            self.calls.set(call + 1);
            if self.mode == 1 && call == 0 {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if self.mode == 2 {
                return Ok(0);
            }
            if self.mode == 3 {
                return Ok(input.len() + 1);
            }
            if self.mode == 6 || (self.mode == 4 && self.bytes.len() >= self.boundary) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "fuzz output failure",
                ));
            }
            let mut length = input.len().min(1 + self.boundary % 3);
            if self.mode == 4 {
                length = length.min(self.boundary - self.bytes.len());
            }
            self.bytes.extend_from_slice(&input[..length]);
            Ok(length)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes.set(self.flushes.get() + 1);
            if self.mode == 7 && self.flushes.get() == 1 {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if self.mode == 5 {
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "fuzz flush failure",
                ))
            } else {
                Ok(())
            }
        }
    }
    let calls = Cell::new(0);
    let flushes = Cell::new(0);
    let mode = input.first().copied().unwrap_or(0) % 8;
    let boundary = input.get(1).copied().unwrap_or(0) as usize % 6;
    let sink = Sink {
        bytes: Vec::new(),
        calls: &calls,
        flushes: &flushes,
        mode,
        boundary,
    };
    let mut writer = Writer::new(sink);
    let result = writer.write_event(&empty()).and_then(|()| writer.finish());
    if let Err(error) = &result {
        super::check_display(error);
        assert!(std::error::Error::source(error).is_some());
    }
    match result {
        Ok(()) => {
            assert!(matches!(mode, 0 | 1 | 4 | 7));
            assert!(mode != 4 || boundary >= 4);
            assert_eq!(writer.position(), 4);
            let expected_flushes = if mode == 7 { 2 } else { 1 };
            assert_eq!(flushes.get(), expected_flushes);
            writer.finish().unwrap();
            assert_eq!(flushes.get(), expected_flushes);
        }
        Err(WriteError::Io(error)) => {
            let expected = match mode {
                2 => io::ErrorKind::WriteZero,
                3 => io::ErrorKind::InvalidData,
                4 | 6 => io::ErrorKind::PermissionDenied,
                5 => io::ErrorKind::BrokenPipe,
                _ => panic!("unexpected output failure: {error}"),
            };
            assert_eq!(error.kind(), expected);
            if matches!(mode, 4 | 6) {
                assert_eq!(error.to_string(), "fuzz output failure");
            }
            if mode == 5 {
                assert_eq!(error.to_string(), "fuzz flush failure");
            }
            let (writes, flushed) = (calls.get(), flushes.get());
            assert_terminal(&mut writer);
            assert_eq!(
                (calls.get(), flushes.get()),
                (writes, flushed),
                "I/O after terminal error"
            );
        }
        Err(error) => panic!("valid empty element hit XML failure: {error}"),
    }
    let position = writer.position();
    let sink = writer.into_inner();
    assert_eq!(position, sink.bytes.len());
    assert!(sink.bytes.len() <= 4);
}

#[path = "text_script.rs"]
mod text_script;

/// `TextWriter` call scripts from the input: random ones (misuse included)
/// under small limits, or well-formed ones whose output must reparse. No
/// call panics, output never passes `max_output_bytes`, and a limit or
/// I/O failure is terminal.
fn text_scripts(input: &[u8]) {
    let Some((&mode, rest)) = input.split_first() else {
        return;
    };
    let mut cursor = 0;
    let mut next = |bound: usize| {
        let byte = rest.get(cursor).copied().unwrap_or(0);
        cursor += 1;
        usize::from(byte) % bound
    };
    // Text arguments are slices from the end of the input.
    let mut tail = rest.len();
    let mut text = || {
        let length = usize::from(*rest.get(tail.checked_sub(1)?)?) % 24;
        let start = tail.checked_sub(1 + length)?;
        let slice = String::from_utf8_lossy(&rest[start..tail - 1]).into_owned();
        tail = start;
        Some(slice)
    };
    let calls = 1 + usize::from(mode >> 2) % 48;
    let well_formed = mode & 1 == 0;
    let limits = match mode & 2 == 0 || well_formed {
        true => WriterLimits::default(),
        false => WriterLimits {
            max_output_bytes: 16 + next(256) * 4,
            max_token_bytes: 8 + next(256),
            max_depth: 1 + next(8),
            max_attributes: next(4),
            ..WriterLimits::default()
        },
    };
    let script = match well_formed {
        true => text_script::well_formed(&mut next, calls),
        false => text_script::random(&mut next, &mut text, calls),
    };
    let mut writer = yaxml::TextWriter::with_limits(Vec::new(), limits);
    let mut terminal = false;
    for call in &script {
        let result = call.apply(&mut writer);
        // The indent setters cannot fail.
        if terminal && !matches!(call.code(), "IN" | "IS") {
            assert!(
                matches!(&result, Err(WriteError::Xml(error)) if error.kind == ErrorKind::InvalidState),
                "{result:?} after a terminal failure"
            );
        }
        if let Err(WriteError::Xml(error)) = &result
            && matches!(error.kind, ErrorKind::LimitExceeded(_))
        {
            terminal = true;
        }
    }
    let position = writer.position();
    let output = writer.into_inner();
    assert_eq!(position, output.len());
    assert!(output.len() <= limits.max_output_bytes);
    if well_formed && let Err(error) = yaxml::OwnedDocument::parse_bytes(&output) {
        panic!(
            "well-formed script output does not reparse ({error}): {script:?}\n{:?}",
            String::from_utf8_lossy(&output)
        );
    }
}
