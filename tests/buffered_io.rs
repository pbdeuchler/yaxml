use std::cell::RefCell;
use std::error::Error as _;
use std::fmt;
use std::io::{self, BufRead, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use yaxml::{BufferedReader, DecodeLimits, Error, ErrorKind, Event, Limit, Limits, ReadError};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Trace {
    fills: usize,
    consumes: Vec<usize>,
    consumed: usize,
}

#[derive(Clone, Copy, Default)]
struct Faults {
    error_at: Option<usize>,
    panic_fill_at: Option<usize>,
    panic_consume: Option<bool>, // false: before consuming; true: after.
    interrupt: bool,
}

struct Source {
    bytes: Vec<u8>,
    offset: usize,
    capacity: usize,
    available: usize,
    faults: Faults,
    interrupt_next: bool,
    trace: Rc<RefCell<Trace>>,
}

impl Source {
    fn new(bytes: Vec<u8>, capacity: usize, faults: Faults) -> (Self, Rc<RefCell<Trace>>) {
        let trace = Rc::new(RefCell::new(Trace::default()));
        (
            Self {
                bytes,
                offset: 0,
                capacity,
                available: 0,
                faults,
                interrupt_next: faults.interrupt,
                trace: trace.clone(),
            },
            trace,
        )
    }
}

#[derive(Debug)]
struct Injected(u32);
impl fmt::Display for Injected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "injected input failure {}", self.0)
    }
}
impl std::error::Error for Injected {}

impl Read for Source {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("BufferedReader called Read instead of BufRead");
    }
}

impl BufRead for Source {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.trace.borrow_mut().fills += 1;
        if self.interrupt_next {
            self.interrupt_next = false;
            return Err(io::ErrorKind::Interrupted.into());
        }
        self.interrupt_next = self.faults.interrupt;
        if self.faults.panic_fill_at == Some(self.offset) {
            panic!("injected fill_buf panic");
        }
        if self.faults.error_at == Some(self.offset) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                Injected(73),
            ));
        }
        let mut end = self
            .offset
            .saturating_add(self.capacity)
            .min(self.bytes.len());
        for boundary in [self.faults.error_at, self.faults.panic_fill_at]
            .into_iter()
            .flatten()
        {
            if self.offset < boundary {
                end = end.min(boundary);
            }
        }
        self.available = end - self.offset;
        Ok(&self.bytes[self.offset..end])
    }

    fn consume(&mut self, count: usize) {
        assert!(
            count <= self.available,
            "consume exceeds last exposed prefix"
        );
        self.trace.borrow_mut().consumes.push(count);
        if self.faults.panic_consume == Some(false) {
            panic!("injected consume panic before progress");
        }
        self.offset += count;
        self.available -= count;
        self.trace.borrow_mut().consumed += count;
        if self.faults.panic_consume == Some(true) {
            panic!("injected consume panic after progress");
        }
    }
}

fn xml(error: ReadError) -> Error {
    match error {
        ReadError::Xml(error) => error,
        other => panic!("expected XML failure, got {other}"),
    }
}

fn assert_poisoned(reader: &mut BufferedReader<Source>, trace: &Rc<RefCell<Trace>>) {
    let before = trace.borrow().clone();
    let error = xml(reader.read_event().unwrap_err());
    assert_eq!(error.kind, ErrorKind::InvalidState);
    for _ in 0..3 {
        assert_eq!(xml(reader.read_event().unwrap_err()), error);
    }
    assert_eq!(
        *trace.borrow(),
        before,
        "source access after terminal I/O failure or panic"
    );
}

fn utf16(input: &str, little: bool) -> Vec<u8> {
    std::iter::once(0xfeff)
        .chain(input.encode_utf16())
        .flat_map(|unit| {
            if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            }
        })
        .collect()
}

#[test]
fn constructors_getters_drop_and_early_extraction_do_not_touch_the_source() {
    for constructor in 0..3 {
        for extract in [false, true] {
            let (source, trace) = Source::new(
                b"<r/>unread".to_vec(),
                65536,
                Faults {
                    panic_fill_at: Some(0),
                    ..Faults::default()
                },
            );
            let reader = match constructor {
                0 => BufferedReader::new(source),
                1 => BufferedReader::with_limits(source, Limits::default()),
                _ => BufferedReader::with_decode_limits(
                    source,
                    Limits::default(),
                    DecodeLimits::default(),
                ),
            };
            assert_eq!(reader.position(), 0);
            assert_eq!(reader.get_ref().offset, 0);
            if extract {
                assert_eq!(reader.into_inner().offset, 0);
            } else {
                drop(reader);
            }
            assert_eq!(*trace.borrow(), Trace::default());
        }
    }
    for extract in [false, true] {
        let (source, trace) = Source::new(b"<r/>unread".to_vec(), 65536, Faults::default());
        let mut reader = BufferedReader::new(source);
        let (event, range) = reader.read_event().unwrap().unwrap();
        assert!(matches!(event, Event::Empty(_)));
        assert_eq!(range.input_range(), Some(0..4));
        let before = trace.borrow().clone();
        assert_eq!(before.consumed, 4);
        if extract {
            let source = reader.into_inner();
            assert_eq!(&source.bytes[source.offset..], b"unread");
        } else {
            drop(reader);
        }
        assert_eq!(*trace.borrow(), before);
    }
}

#[test]
fn interruptions_and_short_visible_windows_preserve_exact_consumption() {
    for bytes in [
        b"<r>text<c/></r>".to_vec(),
        utf16("<r>text<c/></r>", true),
        utf16("<r>text<c/></r>", false),
    ] {
        for capacity in [1, 2, 3, 7, 65536] {
            let (source, trace) = Source::new(
                bytes.clone(),
                capacity,
                Faults {
                    interrupt: true,
                    ..Faults::default()
                },
            );
            let mut reader = BufferedReader::new(source);
            let mut events = 0;
            let mut text = String::new();
            while let Some((event, _)) = reader.read_event().unwrap() {
                events += 1;
                if let Event::Text(value) = event {
                    text.push_str(&value);
                }
            }
            assert_eq!(events, 4);
            assert_eq!(text, "text");
            assert_eq!(reader.position(), bytes.len());
            assert_eq!(trace.borrow().consumed, bytes.len());
            assert_eq!(trace.borrow().consumes.iter().sum::<usize>(), bytes.len());
            let before = trace.borrow().clone();
            for _ in 0..3 {
                assert!(reader.read_event().unwrap().is_none());
            }
            assert_eq!(*trace.borrow(), before);
        }
    }
}

#[test]
fn mid_token_io_failures_preserve_the_original_error_without_partial_events() {
    let mut cases = vec![
        (b"<r>abc".to_vec(), 6, 1),
        (b"<r a='abc".to_vec(), 9, 0),
        (b"<r/>".to_vec(), 4, 1),
        (b"<r/>".to_vec(), 0, 0),
    ];
    for little in [false, true] {
        cases.push((utf16("<r>🙂</r>", little), 11, 1)); // In the low-surrogate code unit.
        cases.push((utf16("<r/>", little), 1, 0)); // In the BOM.
    }
    for (bytes, failure, expected_events) in cases {
        for capacity in [1, 7, 65536] {
            let (source, trace) = Source::new(
                bytes.clone(),
                capacity,
                Faults {
                    error_at: Some(failure),
                    ..Faults::default()
                },
            );
            let mut reader = BufferedReader::new(source);
            let mut events = 0;
            let error = loop {
                match reader.read_event() {
                    Ok(Some(_)) => events += 1,
                    Ok(None) => panic!("I/O failure incorrectly became EOF"),
                    Err(error) => break error,
                }
            };
            assert_eq!(events, expected_events, "partial token was delivered");
            assert_eq!(reader.position(), failure);
            assert_eq!(trace.borrow().consumed, failure);
            assert!(error.source().is_some());
            let ReadError::Io(error) = error else {
                panic!("I/O failure became an XML error");
            };
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(error.to_string(), "injected input failure 73");
            assert_eq!(
                error
                    .get_ref()
                    .unwrap()
                    .downcast_ref::<Injected>()
                    .unwrap()
                    .0,
                73
            );
            assert_poisoned(&mut reader, &trace);
        }
    }
}

#[test]
fn recovered_source_panics_poison_without_later_fill_or_consume() {
    for faults in [
        Faults {
            panic_fill_at: Some(0),
            ..Faults::default()
        },
        Faults {
            panic_fill_at: Some(6),
            ..Faults::default()
        },
        Faults {
            panic_consume: Some(false),
            ..Faults::default()
        },
        Faults {
            panic_consume: Some(true),
            ..Faults::default()
        },
    ] {
        let (source, trace) = Source::new(b"<r>text</r>".to_vec(), 65536, faults);
        let mut reader = BufferedReader::new(source);
        let result = catch_unwind(AssertUnwindSafe(|| {
            while reader.read_event().unwrap().is_some() {}
        }));
        assert!(result.is_err());
        assert_poisoned(&mut reader, &trace);
    }
}

#[test]
fn exact_input_quota_probes_eof_without_consuming_an_excess_byte() {
    for (root, start) in [
        (b"<r/>".to_vec(), 0),
        (utf16("<r/>", true), 2),
        (utf16("<r/>", false), 2),
    ] {
        let quota = root.len();
        for capacity in [1, 4, 65536] {
            for tail in [b"".as_slice(), b"tail"] {
                let bytes = [root.as_slice(), tail].concat();
                let (source, trace) = Source::new(bytes, capacity, Faults::default());
                let mut reader =
                    BufferedReader::with_limits(source, Limits::default().max_input_bytes(quota));
                let (event, range) = reader.read_event().unwrap().unwrap();
                assert!(matches!(event, Event::Empty(_)));
                assert_eq!(range.input_range(), Some(start..quota));
                assert_eq!(reader.position(), quota);
                let before = trace.borrow().clone();
                let result = reader.read_event();
                if tail.is_empty() {
                    assert!(result.unwrap().is_none());
                } else {
                    let error = xml(result.unwrap_err());
                    assert_eq!(
                        error,
                        Error {
                            kind: ErrorKind::LimitExceeded(Limit::InputBytes),
                            offset: 0
                        }
                    );
                    assert_eq!(xml(reader.read_event().unwrap_err()), error);
                }
                assert_eq!(reader.position(), quota);
                let after = trace.borrow().clone();
                assert_eq!(after.consumed, quota);
                assert_eq!(
                    after.fills,
                    before.fills + 1,
                    "exact quota must make one EOF probe"
                );
                let source = reader.into_inner();
                assert_eq!(&source.bytes[source.offset..], tail);
            }
        }
        let (source, trace) = Source::new(
            root,
            65536,
            Faults {
                error_at: Some(quota),
                ..Faults::default()
            },
        );
        let mut reader =
            BufferedReader::with_limits(source, Limits::default().max_input_bytes(quota));
        assert!(reader.read_event().unwrap().is_some());
        let before = trace.borrow().fills;
        assert!(matches!(reader.read_event().unwrap_err(), ReadError::Io(_)));
        assert_eq!(trace.borrow().fills, before + 1);
        assert_eq!(reader.position(), quota);
        assert_poisoned(&mut reader, &trace);
    }
}

#[test]
fn input_quota_counts_the_delimiter_needed_to_deliver_text() {
    for (prefix, through_lookahead) in [("<r>text", false), ("<r>text<", true)] {
        for (bytes, quota) in [
            (b"<r>text<c/></r>".to_vec(), prefix.len()),
            (utf16("<r>text<c/></r>", true), 2 + 2 * prefix.len()),
            (utf16("<r>text<c/></r>", false), 2 + 2 * prefix.len()),
        ] {
            for capacity in [1, 7, 65536] {
                let (source, trace) = Source::new(bytes.clone(), capacity, Faults::default());
                let mut reader =
                    BufferedReader::with_limits(source, Limits::default().max_input_bytes(quota));
                assert!(matches!(
                    reader.read_event().unwrap().unwrap().0,
                    Event::Start(_)
                ));
                if through_lookahead {
                    assert!(
                        matches!(reader.read_event().unwrap().unwrap().0, Event::Text(text) if text == "text")
                    );
                    assert_eq!(reader.position(), quota);
                }
                let error = xml(reader.read_event().unwrap_err());
                assert_eq!(
                    error,
                    Error {
                        kind: ErrorKind::LimitExceeded(Limit::InputBytes),
                        offset: 0
                    }
                );
                assert_eq!(reader.position(), quota);
                assert_eq!(trace.borrow().consumed, quota);
                let before = trace.borrow().clone();
                assert_eq!(xml(reader.read_event().unwrap_err()), error);
                assert_eq!(*trace.borrow(), before);
                let source = reader.into_inner();
                assert_eq!(&source.bytes[source.offset..], &bytes[quota..]);
            }
        }
    }
}

#[test]
fn zero_input_quota_distinguishes_empty_source_from_excess_input() {
    for (bytes, expected) in [
        (b"".as_slice(), ErrorKind::MissingRoot),
        (b"<r/>", ErrorKind::LimitExceeded(Limit::InputBytes)),
    ] {
        let (source, trace) = Source::new(bytes.to_vec(), 65536, Faults::default());
        let mut reader = BufferedReader::with_limits(source, Limits::default().max_input_bytes(0));
        let error = xml(reader.read_event().unwrap_err());
        assert_eq!(
            error,
            Error {
                kind: expected,
                offset: 0
            }
        );
        assert_eq!(reader.position(), 0);
        assert_eq!(trace.borrow().consumed, 0);
        let before = trace.borrow().clone();
        assert_eq!(xml(reader.read_event().unwrap_err()), error);
        assert_eq!(*trace.borrow(), before);
    }
}

#[test]
fn syntax_failure_stops_io_and_preserves_the_unread_suffix() {
    let bytes = b"<r></x>DO-NOT-CONSUME".to_vec();
    for capacity in [1, 7, 65536] {
        let (source, trace) = Source::new(bytes.clone(), capacity, Faults::default());
        let mut reader = BufferedReader::new(source);
        assert!(reader.read_event().unwrap().is_some());
        let error = xml(reader.read_event().unwrap_err());
        assert_eq!(
            error,
            Error {
                kind: ErrorKind::MismatchedEndTag,
                offset: 3
            }
        );
        assert_eq!(reader.position(), 7);
        assert_eq!(trace.borrow().consumed, 7);
        let before = trace.borrow().clone();
        for _ in 0..3 {
            assert_eq!(xml(reader.read_event().unwrap_err()), error);
        }
        assert_eq!(*trace.borrow(), before);
        let source = reader.into_inner();
        assert_eq!(&source.bytes[source.offset..], b"DO-NOT-CONSUME");
    }
}
