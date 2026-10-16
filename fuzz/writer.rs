//! Writer operations are caller input, independently of parsed XML bytes.

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
}

fn roundtrip(input: &[u8]) {
    // Escaping can expand an originally literal attribute quote sixfold.
    let ceiling = input.len().saturating_mul(6).saturating_add(1024);
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
    let limits = yaxml::Limits {
        max_input_bytes: ceiling,
        max_token_bytes: ceiling,
        ..yaxml::Limits::default()
    };
    let mut reader = Reader::with_limits(&output, limits).unwrap();
    let mut original = Reader::new(input).unwrap();
    loop {
        let actual = reader.read_event().unwrap();
        let expected = original.read_event().unwrap();
        assert!(actual == expected, "serialized values changed");
        if actual.is_none() {
            break;
        }
    }
    Document::with_limits(&output, limits).unwrap();
    let mut streaming = PushParser::with_limits(limits);
    streaming
        .feed(&output, |_, _| ControlFlow::Continue(()))
        .unwrap();
    streaming.finish(|_, _| ControlFlow::Continue(())).unwrap();
}

fn name(qualified: &str) -> Name<'_> {
    Name {
        qualified,
        prefix: None,
        local: qualified,
        namespace: None,
    }
}

fn empty() -> Event<'static> {
    Event::Empty(Element {
        name: name("r"),
        attributes: Vec::new(),
    })
}

fn inconsistent_name_fields(name: &Name<'_>) -> bool {
    let (prefix, local) = name
        .qualified
        .split_once(':')
        .map_or((None, name.qualified), |(prefix, local)| {
            (Some(prefix), local)
        });
    name.prefix != prefix || name.local != local
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
        let mut element_name = name(if control & 64 == 0 { "r" } else { &payload });
        match (control >> 4) & 3 {
            1 => element_name.local = "different",
            2 => element_name.prefix = Some("p"),
            3 => element_name.namespace = Some(Cow::Borrowed("urn:undeclared")),
            _ => {}
        }
        let event = match control & 7 {
            0 => Event::Start(Element {
                name: element_name,
                attributes: vec![Attribute {
                    name: name(if control & 128 != 0 {
                        &payload
                    } else if control & 8 == 0 {
                        "a"
                    } else {
                        "xmlns"
                    }),
                    value: Cow::Borrowed(&payload),
                }],
            }),
            1 => Event::Empty(Element {
                name: element_name,
                attributes: Vec::new(),
            }),
            2 => Event::End(element_name),
            3 => Event::Text(Cow::Borrowed(&payload)),
            4 => Event::Cdata(Cow::Borrowed(&payload)),
            5 => Event::Comment(Cow::Borrowed(&payload)),
            6 => Event::ProcessingInstruction {
                target: if control & 8 == 0 { "pi" } else { &payload },
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
                inconsistent_name_fields(&element.name)
                    || element.name.namespace.is_some()
                    || element.attributes.iter().any(|attribute| {
                        inconsistent_name_fields(&attribute.name)
                            || (attribute.name.qualified == "xmlns"
                                && attribute.name.namespace.as_deref()
                                    != Some("http://www.w3.org/2000/xmlns/"))
                            || (attribute.name.prefix.is_none()
                                && attribute.name.qualified != "xmlns"
                                && attribute.name.namespace.is_some())
                    })
            }
            Event::End(name) => inconsistent_name_fields(name) || name.namespace.is_some(),
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
