use std::error::Error as _;
use std::io::{self, Read};
use std::ops::ControlFlow;

use yaxml::{ErrorKind, Event, Limit, Limits, PushParser, ReadError};

#[test]
fn consumes_reader_with_the_same_logical_events() {
    let input = b"<r a='v'>text&amp;more<c/></r>";
    let mut names = Vec::new();
    let mut text = String::new();
    PushParser::new()
        .read_from(input.as_slice(), |event, _| {
            match event {
                Event::Start(element) | Event::Empty(element) => {
                    names.push(element.name.qualified().to_owned())
                }
                Event::Text(value) => text.push_str(value),
                _ => {}
            }
            ControlFlow::Continue(())
        })
        .unwrap();
    assert_eq!(names, ["r", "c"]);
    assert_eq!(text, "text&more");

    let mut parser = PushParser::new();
    parser
        .feed(b"<r>", |_, _| ControlFlow::Continue(()))
        .unwrap();
    let mut range = 0..0;
    parser
        .read_from(b"</r>".as_slice(), |_, span| {
            range = span.input_range().expect("direct input event");
            ControlFlow::Continue(())
        })
        .unwrap();
    assert_eq!(range, 3..7);
}

#[test]
fn input_failures_preserve_xml_errors_and_limits() {
    for (input, expected) in [
        (b"".as_slice(), ErrorKind::MissingRoot),
        (b"<r>", ErrorKind::UnexpectedEof),
        (b"<r></x>", ErrorKind::MismatchedEndTag),
        (b"<r>\xff</r>", ErrorKind::InvalidUtf8),
    ] {
        let error = PushParser::new()
            .read_from(input, |_, _| ControlFlow::Continue(()))
            .unwrap_err();
        assert!(error.source().is_some());
        assert!(
            matches!(error,ReadError::Xml(e) if e.kind==expected),
            "{input:?}"
        );
    }
    let error = PushParser::with_limits(Limits::default().max_input_bytes(3))
        .read_from(b"<r/>".as_slice(), |_, _| ControlFlow::Continue(()))
        .unwrap_err();
    assert!(
        matches!(error,ReadError::Xml(e) if e.kind==ErrorKind::LimitExceeded(Limit::InputBytes))
    );
    let error = PushParser::new()
        .read_from(b"<r/>".as_slice(), |_, _| ControlFlow::Break(()))
        .unwrap_err();
    assert!(matches!(error,ReadError::Xml(e) if e.kind==ErrorKind::Aborted && e.offset==4));
}

#[test]
fn retries_interruptions_and_preserves_other_io_failures() {
    struct Chunks<'a> {
        input: &'a [u8],
        interrupt: bool,
    }
    impl Read for Chunks<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.interrupt = !self.interrupt;
            if self.interrupt {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let length = self.input.len().min(1);
            buffer[..length].copy_from_slice(&self.input[..length]);
            self.input = &self.input[length..];
            Ok(length)
        }
    }
    PushParser::new()
        .read_from(
            Chunks {
                input: b"<r/>",
                interrupt: false,
            },
            |_, _| ControlFlow::Continue(()),
        )
        .unwrap();
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected input failure",
            ))
        }
    }
    let error = PushParser::new()
        .read_from(Broken, |_, _| ControlFlow::Continue(()))
        .unwrap_err();
    assert!(error.to_string().contains("injected input failure"));
    assert!(error.source().is_some());
    assert!(
        matches!(error,ReadError::Io(e) if e.kind()==io::ErrorKind::PermissionDenied && e.to_string()=="injected input failure")
    );
}

#[test]
fn malformed_reader_byte_counts_return_io_error() {
    struct Oversized;
    impl Read for Oversized {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            Ok(buffer.len() + 1)
        }
    }
    let error = PushParser::new()
        .read_from(Oversized, |_, _| ControlFlow::Continue(()))
        .unwrap_err();
    assert!(matches!(error,ReadError::Io(e) if e.kind()==io::ErrorKind::InvalidData));
}

#[test]
fn reader_errors_render_their_xml_cause() {
    let error = PushParser::new()
        .read_from(b"<r>".as_slice(), |_, _| ControlFlow::Continue(()))
        .unwrap_err();
    let ReadError::Xml(xml) = &error else {
        panic!()
    };
    assert_eq!(error.to_string(), xml.to_string());
    assert_eq!(error.source().unwrap().to_string(), xml.to_string());
}

#[test]
fn terminal_parser_errors_stop_reading_immediately() {
    struct Once<'a> {
        bytes: &'a [u8],
        read: bool,
    }
    impl Read for Once<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            assert!(!self.read, "read after terminal parser failure");
            self.read = true;
            buffer[..self.bytes.len()].copy_from_slice(self.bytes);
            Ok(self.bytes.len())
        }
    }
    for (bytes, stop, expected) in [
        (b"<r></x>".as_slice(), false, ErrorKind::MismatchedEndTag),
        (b"<r/>", true, ErrorKind::Aborted),
    ] {
        let result = PushParser::new().read_from(Once { bytes, read: false }, |_, _| {
            if stop {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        assert!(matches!(result,Err(ReadError::Xml(e)) if e.kind==expected));
    }
}

#[test]
fn terminal_parsers_do_not_touch_new_readers() {
    struct Untouched;
    impl Read for Untouched {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("terminal parser touched I/O")
        }
    }
    for (input, cancel) in [(b"<r></x>".as_slice(), false), (b"<r/>", true)] {
        let mut parser = PushParser::new();
        let expected = parser
            .feed(input, |_, _| {
                if cancel {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            })
            .unwrap_err();
        let error = parser
            .read_from(Untouched, |_, _| panic!("callback after terminal failure"))
            .unwrap_err();
        assert!(matches!(error,ReadError::Xml(actual) if actual==expected));
    }
    let mut parser = PushParser::new();
    parser
        .feed(b"<r/>", |_, _| ControlFlow::Continue(()))
        .unwrap();
    parser.finish(|_, _| ControlFlow::Continue(())).unwrap();
    let error = parser
        .read_from(Untouched, |_, _| panic!("callback after final EOF"))
        .unwrap_err();
    assert!(
        matches!(error,ReadError::Xml(actual) if actual.kind==ErrorKind::InvalidState && actual.offset==4)
    );
}
