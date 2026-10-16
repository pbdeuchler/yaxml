#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::error::Error as _;
use std::fmt;
use std::io::{self, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use yaxml::{Element, ErrorKind, Event, Name, WriteError, Writer};

fn root() -> Event<'static> {
    Event::Empty(Element {
        name: Name::new("r", None).unwrap(),
        attributes: vec![],
    })
}

#[derive(Debug)]
struct Injected;

impl fmt::Display for Injected {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("injected output failure")
    }
}

impl std::error::Error for Injected {}

#[derive(Clone, Copy)]
enum Action {
    ShortAndInterrupted,
    Zero,
    Oversized,
    FailAfterTwo,
    PanicAfterTwo,
    FlushError,
    FlushInterrupted,
    FlushPanic,
    Normal,
}

#[derive(Default)]
struct Observed {
    bytes: Vec<u8>,
    writes: usize,
    flushes: usize,
}

struct Sink {
    action: Action,
    observed: Rc<RefCell<Observed>>,
}

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut observed = self.observed.borrow_mut();
        observed.writes += 1;
        let length = match self.action {
            Action::ShortAndInterrupted if observed.writes % 2 == 1 => {
                return Err(io::ErrorKind::Interrupted.into());
            }
            Action::ShortAndInterrupted => bytes.len().min(1),
            Action::Zero => return Ok(0),
            Action::Oversized => return Ok(bytes.len() + 1),
            Action::FailAfterTwo if observed.bytes.len() == 2 => {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, Injected));
            }
            Action::PanicAfterTwo if observed.bytes.len() == 2 => panic!("injected sink panic"),
            Action::FailAfterTwo | Action::PanicAfterTwo => bytes.len().min(2),
            _ => bytes.len(),
        };
        observed.bytes.extend_from_slice(&bytes[..length]);
        Ok(length)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.observed.borrow_mut().flushes += 1;
        match self.action {
            Action::FlushInterrupted if self.observed.borrow().flushes == 1 => {
                Err(io::ErrorKind::Interrupted.into())
            }
            Action::FlushError => Err(io::Error::new(io::ErrorKind::PermissionDenied, Injected)),
            Action::FlushPanic => panic!("injected flush panic"),
            _ => Ok(()),
        }
    }
}

fn writer(action: Action) -> (Writer<Sink>, Rc<RefCell<Observed>>) {
    let observed = Rc::new(RefCell::new(Observed::default()));
    let writer = Writer::new(Sink {
        action,
        observed: Rc::clone(&observed),
    });
    (writer, observed)
}

fn assert_terminal(writer: &mut Writer<Sink>, observed: &Rc<RefCell<Observed>>) {
    let before = (observed.borrow().writes, observed.borrow().flushes);
    let position = writer.position();
    for error in [
        writer.write_event(&root()).unwrap_err(),
        writer.finish().unwrap_err(),
    ] {
        assert!(matches!(error, WriteError::Xml(error) if error.kind == ErrorKind::InvalidState));
    }
    assert_eq!(writer.position(), position);
    assert_eq!(
        (observed.borrow().writes, observed.borrow().flushes),
        before
    );
}

#[test]
fn retries_short_and_interrupted_writes_and_flushes_once() {
    for action in [
        Action::ShortAndInterrupted,
        Action::FlushInterrupted,
        Action::Normal,
    ] {
        let (mut writer, observed) = writer(action);
        writer.write_event(&root()).unwrap();
        assert_eq!(writer.position(), 4);
        assert_eq!(observed.borrow().bytes, b"<r/>");
        assert_eq!(observed.borrow().flushes, 0);
        writer.finish().unwrap();
        writer.finish().unwrap();
        assert_eq!(
            observed.borrow().flushes,
            if matches!(action, Action::FlushInterrupted) {
                2
            } else {
                1
            }
        );
        assert_terminal(&mut writer, &observed);
        assert_eq!(writer.into_inner().observed.borrow().bytes, b"<r/>");
    }
}

#[test]
fn consuming_or_dropping_an_unfinished_writer_does_not_flush_or_close_tags() {
    for consume in [false, true] {
        let (mut writer, observed) = writer(Action::Normal);
        let Event::Empty(element) = root() else {
            unreachable!()
        };
        writer.write_event(&Event::Start(element)).unwrap();
        if consume {
            drop(writer.into_inner());
        } else {
            drop(writer);
        }
        assert_eq!(observed.borrow().bytes, b"<r>");
        assert_eq!(
            (observed.borrow().writes, observed.borrow().flushes),
            (1, 0)
        );
    }
}

#[test]
fn invalid_write_counts_and_partial_failures_are_terminal() {
    for (action, expected, position, bytes) in [
        (Action::Zero, io::ErrorKind::WriteZero, 0, b"".as_slice()),
        (Action::Oversized, io::ErrorKind::InvalidData, 0, b""),
        (
            Action::FailAfterTwo,
            io::ErrorKind::PermissionDenied,
            2,
            b"<r",
        ),
    ] {
        let (mut writer, observed) = writer(action);
        let error = writer.write_event(&root()).unwrap_err();
        let WriteError::Io(cause) = &error else {
            panic!("expected I/O error, got {error}");
        };
        assert_eq!(cause.kind(), expected);
        assert_eq!(error.source().unwrap().to_string(), cause.to_string());
        if matches!(action, Action::FailAfterTwo) {
            assert!(
                cause
                    .get_ref()
                    .unwrap()
                    .downcast_ref::<Injected>()
                    .is_some()
            );
            assert!(error.to_string().contains("injected output failure"));
        }
        assert_eq!(writer.position(), position);
        assert_eq!(observed.borrow().bytes, bytes);
        assert_terminal(&mut writer, &observed);
    }
}

#[test]
fn flush_failures_preserve_the_original_error_and_stop_io() {
    let (mut writer, observed) = writer(Action::FlushError);
    writer.write_event(&root()).unwrap();
    let error = writer.finish().unwrap_err();
    let WriteError::Io(cause) = &error else {
        panic!("expected flush I/O failure");
    };
    assert_eq!(cause.kind(), io::ErrorKind::PermissionDenied);
    assert!(
        cause
            .get_ref()
            .unwrap()
            .downcast_ref::<Injected>()
            .is_some()
    );
    assert_eq!(
        error.source().unwrap().to_string(),
        "injected output failure"
    );
    assert_eq!(writer.position(), 4);
    assert_eq!(observed.borrow().flushes, 1);
    assert_terminal(&mut writer, &observed);
}

#[test]
fn recovered_sink_panics_poison_the_writer() {
    for action in [Action::PanicAfterTwo, Action::FlushPanic] {
        let (mut writer, observed) = writer(action);
        let result = catch_unwind(AssertUnwindSafe(|| {
            writer.write_event(&root()).unwrap();
            writer.finish().unwrap();
        }));
        assert!(result.is_err());
        assert_eq!(
            writer.position(),
            if matches!(action, Action::PanicAfterTwo) {
                2
            } else {
                4
            }
        );
        assert_terminal(&mut writer, &observed);
    }
}

#[test]
fn xml_errors_render_their_cause_and_do_not_touch_the_sink() {
    let (mut writer, observed) = writer(Action::Normal);
    let error = writer.finish().unwrap_err();
    let WriteError::Xml(cause) = &error else {
        panic!("missing root should be an XML error");
    };
    assert_eq!(cause.kind, ErrorKind::MissingRoot);
    assert_eq!(cause.offset, 0);
    assert_eq!(error.to_string(), cause.to_string());
    assert_eq!(error.source().unwrap().to_string(), cause.to_string());
    assert_eq!(
        (observed.borrow().writes, observed.borrow().flushes),
        (0, 0)
    );
    assert_terminal(&mut writer, &observed);
}
