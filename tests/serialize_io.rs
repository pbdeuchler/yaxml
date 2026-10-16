#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::error::Error as _;
use std::fmt;
use std::io::{self, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use yaxml::{Document, ErrorKind, Event, Limit, OwnedDocument, WriteError, Writer, WriterLimits};

const INPUT: &str = "<r a='é'>x<c/></r>";
const OUTPUT: &[u8] = "<r a=\"é\">x<c/></r>".as_bytes();

#[derive(Clone, Copy, Debug)]
enum Fault {
    None,
    ShortInterrupted,
    Zero,
    Oversized,
    Error(usize),
    Panic(usize),
    FlushError,
    FlushPanic,
    FlushInterrupted,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Observed {
    bytes: Vec<u8>,
    writes: usize,
    flushes: usize,
}

struct Sink {
    fault: Fault,
    observed: Rc<RefCell<Observed>>,
}

#[derive(Debug)]
struct Injected(u32);
impl fmt::Display for Injected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "injected serialization failure {}", self.0)
    }
}
impl std::error::Error for Injected {}

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut observed = self.observed.borrow_mut();
        observed.writes += 1;
        let count = match self.fault {
            Fault::ShortInterrupted if observed.writes % 2 == 1 => {
                return Err(io::ErrorKind::Interrupted.into());
            }
            Fault::ShortInterrupted => bytes.len().min(1),
            Fault::Zero => return Ok(0),
            Fault::Oversized => return Ok(bytes.len() + 1),
            Fault::Error(at) if observed.bytes.len() == at => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    Injected(73),
                ));
            }
            Fault::Panic(at) if observed.bytes.len() == at => panic!("injected write panic"),
            Fault::Error(at) | Fault::Panic(at) => bytes.len().min(at - observed.bytes.len()),
            _ => bytes.len(),
        };
        observed.bytes.extend_from_slice(&bytes[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut observed = self.observed.borrow_mut();
        observed.flushes += 1;
        match self.fault {
            Fault::FlushError => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                Injected(73),
            )),
            Fault::FlushPanic => panic!("injected flush panic"),
            Fault::FlushInterrupted if observed.flushes == 1 => {
                Err(io::ErrorKind::Interrupted.into())
            }
            _ => Ok(()),
        }
    }
}

fn writer(fault: Fault, limits: WriterLimits) -> (Writer<Sink>, Rc<RefCell<Observed>>) {
    let observed = Rc::new(RefCell::new(Observed::default()));
    let writer = Writer::with_limits(
        Sink {
            fault,
            observed: Rc::clone(&observed),
        },
        limits,
    );
    (writer, observed)
}

fn write_document(writer: &mut Writer<Sink>, owned: bool) -> Result<(), WriteError> {
    if owned {
        writer.write_owned_document(&OwnedDocument::parse(INPUT).unwrap())
    } else {
        writer.write_document(&Document::parse(INPUT).unwrap())
    }
}

fn xml(error: WriteError) -> yaxml::Error {
    match error {
        WriteError::Xml(error) => error,
        other => panic!("expected XML error, got {other}"),
    }
}

fn terminal(writer: &mut Writer<Sink>, observed: &Rc<RefCell<Observed>>) {
    let before = observed.borrow().clone();
    let position = writer.position();
    for error in [
        write_document(writer, false).unwrap_err(),
        write_document(writer, true).unwrap_err(),
        writer.write_event(&Event::Text("".into())).unwrap_err(),
        writer.finish().unwrap_err(),
    ] {
        let error = xml(error);
        assert_eq!(error.kind, ErrorKind::InvalidState);
        assert_eq!(error.offset, position);
    }
    assert_eq!(*observed.borrow(), before, "I/O after terminal result");
    assert_eq!(writer.position(), position);
}

#[test]
fn whole_document_success_finishes_flushes_and_rejects_later_operations() {
    for owned in [false, true] {
        for fault in [
            Fault::None,
            Fault::ShortInterrupted,
            Fault::FlushInterrupted,
        ] {
            let (mut writer, observed) = writer(fault, WriterLimits::default());
            assert_eq!(*observed.borrow(), Observed::default());
            write_document(&mut writer, owned).unwrap();
            assert_eq!(observed.borrow().bytes, OUTPUT);
            assert_eq!(writer.position(), OUTPUT.len());
            assert_eq!(
                observed.borrow().flushes,
                if matches!(fault, Fault::FlushInterrupted) {
                    2
                } else {
                    1
                }
            );
            let before = observed.borrow().clone();
            writer.finish().unwrap();
            writer.finish().unwrap();
            assert_eq!(*observed.borrow(), before);
            terminal(&mut writer, &observed);
            let before = observed.borrow().clone();
            drop(writer.into_inner());
            assert_eq!(*observed.borrow(), before);
        }
    }
}

#[test]
fn all_prior_event_calls_consume_document_freshness_before_any_new_io() {
    let source = Document::parse("<?xml version='1.0'?><r></r>").unwrap();
    let element = || yaxml::Element {
        name: yaxml::Name::new("r", None).unwrap(),
        attributes: vec![],
    };
    for owned in [false, true] {
        for previous in [
            Event::Text("".into()),
            Event::Text(" \r\n".into()),
            Event::Comment("before".into()),
            Event::Declaration(*source.declaration().unwrap()),
            Event::Start(element()),
            Event::Empty(element()),
        ] {
            let (mut writer, observed) = writer(Fault::None, WriterLimits::default());
            writer.write_event(&previous).unwrap();
            let before = observed.borrow().clone();
            let position = writer.position();
            let error = xml(write_document(&mut writer, owned).unwrap_err());
            assert_eq!(error.kind, ErrorKind::InvalidState);
            assert_eq!(error.offset, position);
            assert_eq!(*observed.borrow(), before);
            terminal(&mut writer, &observed);
        }
        let (mut writer, observed) = writer(Fault::None, WriterLimits::default());
        assert_eq!(
            xml(writer.finish().unwrap_err()).kind,
            ErrorKind::MissingRoot
        );
        terminal(&mut writer, &observed);
    }
}

#[test]
fn first_sink_error_preserves_cause_and_confirmed_partial_position() {
    for owned in [false, true] {
        for (fault, expected, position) in [
            (Fault::Zero, io::ErrorKind::WriteZero, 0),
            (Fault::Oversized, io::ErrorKind::InvalidData, 0),
            (Fault::Error(2), io::ErrorKind::PermissionDenied, 2),
            (Fault::Error(11), io::ErrorKind::PermissionDenied, 11),
            (
                Fault::FlushError,
                io::ErrorKind::PermissionDenied,
                OUTPUT.len(),
            ),
        ] {
            let (mut writer, observed) = writer(fault, WriterLimits::default());
            let error = write_document(&mut writer, owned).unwrap_err();
            let WriteError::Io(cause) = &error else {
                panic!("expected sink failure: {error}")
            };
            assert_eq!(cause.kind(), expected);
            assert_eq!(error.source().unwrap().to_string(), cause.to_string());
            if expected == io::ErrorKind::PermissionDenied {
                assert_eq!(
                    cause
                        .get_ref()
                        .unwrap()
                        .downcast_ref::<Injected>()
                        .unwrap()
                        .0,
                    73
                );
                assert!(
                    error
                        .to_string()
                        .contains("injected serialization failure 73")
                );
            }
            assert_eq!(writer.position(), position);
            assert_eq!(observed.borrow().bytes, &OUTPUT[..position]);
            assert_eq!(
                observed.borrow().flushes,
                usize::from(matches!(fault, Fault::FlushError))
            );
            terminal(&mut writer, &observed);
        }
    }
}

#[test]
fn recovered_write_and_final_flush_panics_leave_terminal_writers() {
    for owned in [false, true] {
        for (fault, position) in [
            (Fault::Panic(2), 2),
            (Fault::Panic(11), 11),
            (Fault::FlushPanic, OUTPUT.len()),
        ] {
            let (mut writer, observed) = writer(fault, WriterLimits::default());
            assert!(catch_unwind(AssertUnwindSafe(|| write_document(&mut writer, owned))).is_err());
            assert_eq!(writer.position(), position);
            assert_eq!(observed.borrow().bytes, &OUTPUT[..position]);
            terminal(&mut writer, &observed);
        }
    }
}

#[test]
fn xml_limits_emit_no_rejected_event_and_never_flush_on_failure() {
    for owned in [false, true] {
        for (limits, category, expected) in [
            (
                WriterLimits {
                    max_attributes: 0,
                    ..WriterLimits::default()
                },
                Limit::Attributes,
                b"".as_slice(),
            ),
            (
                WriterLimits {
                    max_output_bytes: 14,
                    ..WriterLimits::default()
                },
                Limit::OutputBytes,
                &OUTPUT[..11],
            ),
        ] {
            let (mut writer, observed) = writer(Fault::None, limits);
            assert_eq!(
                xml(write_document(&mut writer, owned).unwrap_err()).kind,
                ErrorKind::LimitExceeded(category)
            );
            assert_eq!(observed.borrow().bytes, expected);
            assert_eq!(writer.position(), expected.len());
            assert_eq!(observed.borrow().flushes, 0);
            terminal(&mut writer, &observed);
            let before = observed.borrow().clone();
            drop(writer);
            assert_eq!(*observed.borrow(), before);
        }
    }
}
