use super::*;
use std::borrow::Cow;

fn buffer(limit: usize) -> TokenBuffer {
    TokenBuffer {
        text: String::new(),
        offset: 7,
        limit,
        ceiling: Limit::TokenBytes,
    }
}

fn name() -> Name<'static> {
    Name::new("p:r", Some(Cow::Borrowed("urn:r"))).unwrap()
}

fn element() -> Element<'static> {
    Element {
        name: name(),
        attributes: vec![crate::Attribute {
            name: name(),
            value: Cow::Borrowed("value"),
        }],
    }
}

#[test]
fn scratch_capacity_growth_and_limits() {
    for limit in [0, 1, 3, 63, 64, 65, 1024] {
        let mut buffer = buffer(limit);
        buffer.push("").unwrap();
        for _ in 0..limit {
            buffer.push("x").unwrap();
            assert!(buffer.text.capacity() <= limit);
        }
        assert_eq!(buffer.text.len(), limit);
        assert_eq!(
            buffer.push("x").unwrap_err(),
            Error::new(ErrorKind::LimitExceeded(Limit::TokenBytes), 7)
        );
        assert_eq!(buffer.text.len(), limit);
        assert!(buffer.check_size(usize::MAX).is_err());
    }
    let mut buffer = buffer(1024);
    buffer.push(&"x".repeat(900)).unwrap();
    let capacity = buffer.text.capacity();
    buffer.text.clear();
    buffer.limit = 1;
    buffer.ceiling = Limit::OutputBytes;
    buffer.push("x").unwrap();
    assert_eq!(buffer.text.capacity(), capacity);
    assert_eq!(
        buffer.push("x").unwrap_err().kind,
        ErrorKind::LimitExceeded(Limit::OutputBytes)
    );
}

#[test]
fn escaping_preserves_logical_values_and_bounds_expansion() {
    for (value, attribute, expected) in [
        ("", false, ""),
        ("ordinary é🙂", false, "ordinary é🙂"),
        ("&<>\r\n\t\"", false, "&amp;&lt;&gt;&#13;\n\t\""),
        ("&<>\r\n\t\"", true, "&amp;&lt;>&#13;&#10;&#9;&quot;"),
    ] {
        let mut actual = buffer(expected.len());
        actual.escaped(value, attribute).unwrap();
        assert_eq!(actual.text, expected);
        if !expected.is_empty() {
            let mut short = buffer(expected.len() - 1);
            assert_eq!(
                short.escaped(value, attribute).unwrap_err().kind,
                ErrorKind::LimitExceeded(Limit::TokenBytes)
            );
            assert!(short.text.len() <= short.limit);
        }
    }
    let mut empty = buffer(0);
    assert!(empty.escaped("a", false).is_err());
    assert_eq!(empty.text.capacity(), 0);
}

#[test]
fn name_coherence_checks_spelling_and_namespace() {
    check_name(&name(), &name(), 9).unwrap();
    for (supplied, expected) in [
        (
            Name::new("q:r", Some(Cow::Borrowed("urn:r"))).unwrap(),
            ErrorKind::InvalidName,
        ),
        (
            Name::new("p:other", Some(Cow::Borrowed("urn:r"))).unwrap(),
            ErrorKind::InvalidName,
        ),
        (Name::new("p:r", None).unwrap(), ErrorKind::Namespace),
    ] {
        assert_eq!(
            check_name(&supplied, &name(), 9).unwrap_err(),
            Error::new(expected, 9)
        );
    }
}

#[test]
fn element_coherence_checks_structure_and_values() {
    check_element(&element(), &element(), 3).unwrap();
    let mut parsed = element();
    parsed.attributes.clear();
    assert_eq!(
        check_element(&element(), &parsed, 3).unwrap_err().kind,
        ErrorKind::InvalidName
    );
    let mut parsed = element();
    parsed.attributes[0].name = Name::new("p:other", Some(Cow::Borrowed("urn:r"))).unwrap();
    assert_eq!(
        check_element(&element(), &parsed, 3).unwrap_err().kind,
        ErrorKind::InvalidName
    );
    let mut parsed = element();
    parsed.attributes[0].value = Cow::Borrowed("changed");
    assert_eq!(
        check_element(&element(), &parsed, 3).unwrap_err().kind,
        ErrorKind::Syntax
    );
}

#[test]
fn event_coherence_rejects_changed_kinds_and_declaration_fields() {
    let declaration = crate::Declaration {
        version: "1.0",
        encoding: Some("UTF-8"),
        standalone: None,
    };
    for (supplied, parsed, expected) in [
        (
            Event::Declaration(declaration),
            Some(Event::Declaration(declaration)),
            Ok(()),
        ),
        (
            Event::Declaration(declaration),
            Some(Event::Declaration(crate::Declaration {
                standalone: Some(true),
                ..declaration
            })),
            Err(Error::new(ErrorKind::InvalidDeclaration, 11)),
        ),
        (
            Event::ProcessingInstruction {
                target: Cow::Borrowed("pi"),
                data: Cow::Borrowed(""),
            },
            Some(Event::ProcessingInstruction {
                target: Cow::Borrowed("other"),
                data: Cow::Borrowed(""),
            }),
            Err(Error::new(ErrorKind::InvalidName, 11)),
        ),
        (
            Event::Start(element()),
            Some(Event::Empty(element())),
            Err(Error::new(ErrorKind::Syntax, 11)),
        ),
        (Event::Text(Cow::Borrowed("")), None, Ok(())),
    ] {
        assert_eq!(check_event(&supplied, parsed.as_ref(), 11), expected);
    }
}

#[test]
fn complete_document_guards_include_empty_events_and_finish() {
    let document = Document::parse("<r/>").unwrap();
    let owned = OwnedDocument::parse("<r/>").unwrap();
    let mut writer = Writer::new(Vec::new());
    assert!(matches!(writer.status, Status::Fresh));
    writer.write_event(&Event::Text(Cow::Borrowed(""))).unwrap();
    assert!(matches!(writer.status, Status::Active));
    assert!(matches!(
        writer.write_document(&document),
        Err(WriteError::Xml(Error {
            kind: ErrorKind::InvalidState,
            offset: 0
        }))
    ));
    assert!(matches!(writer.status, Status::Failed));
    assert!(writer.into_inner().is_empty());

    let mut writer = Writer::new(Vec::new());
    writer.write_owned_document(&owned).unwrap();
    assert!(matches!(writer.status, Status::Finished));
    writer.finish().unwrap();
    assert!(matches!(
        writer.write_document(&document),
        Err(WriteError::Xml(Error {
            kind: ErrorKind::InvalidState,
            offset: 4
        }))
    ));
    assert!(matches!(writer.status, Status::Failed));
    assert_eq!(writer.into_inner(), b"<r/>");
}

#[test]
fn complete_document_flush_failure_and_panic_stay_terminal() {
    struct Sink {
        bytes: Vec<u8>,
        panic: bool,
        flushes: usize,
    }
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            assert!(!self.panic, "flush panic");
            Err(io::Error::other("flush failure"))
        }
    }
    let document = Document::parse("<r/>").unwrap();
    for panic in [false, true] {
        let mut writer = Writer::new(Sink {
            bytes: Vec::new(),
            panic,
            flushes: 0,
        });
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            writer.write_document(&document)
        }));
        if panic {
            assert!(result.is_err());
        } else {
            assert!(
                matches!(result.unwrap(), Err(WriteError::Io(error)) if error.to_string() == "flush failure")
            );
        }
        assert_eq!(writer.position(), 4);
        assert!(matches!(writer.status, Status::Failed));
        assert!(matches!(
            writer.finish(),
            Err(WriteError::Xml(Error {
                kind: ErrorKind::InvalidState,
                ..
            }))
        ));
        let sink = writer.into_inner();
        assert_eq!(sink.bytes, b"<r/>");
        assert_eq!(sink.flushes, 1);
    }
}
