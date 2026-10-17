use super::*;
use crate::stream::dtd::tests::{name, options};
use std::io::Cursor;

fn configuration() -> SessionLimits {
    let mut configured = options();
    configured.text.scratch.input_token_bytes = Limits::default().max_token_bytes;
    configured.text.scratch.max_bytes = configured.text.scratch.input_token_bytes
        + configured.text.scratch.replacement_token_bytes
        + configured
            .text
            .reference
            .max_bytes
            .min(configured.text.reference.max_scratch_bytes);
    configured
}

#[test]
fn cursor_adapter_lends_dtd_and_generated_body_before_repeated_eof() {
    let raw = b"<!DOCTYPE r [<!ENTITY e '<a/>tail'>]><r>pre&e;post</r>";
    let mut reader = DtdBufferedReader::new(Cursor::new(raw), configuration());
    assert_eq!(reader.position(), 0);
    let mut rows = Vec::new();
    while let Some((event, origin)) = reader.read_event().unwrap() {
        rows.push((name(event), origin));
    }
    assert_eq!(
        rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        [
            "dtd:r", "entity:e", "dtd:end", "+r", "Tpre", "=a", "Ttail", "Tpost", "-r"
        ]
    );
    assert!(rows[5].1.input_range().is_none());
    assert_eq!(reader.position(), raw.len());
    assert_eq!(reader.get_ref().position(), raw.len() as u64);
    assert!(reader.read_event().unwrap().is_none());
    assert_eq!(reader.into_inner().position(), raw.len() as u64);
}

#[test]
fn exact_input_limit_and_semantic_lend_error_commit_only_accepted_transport() {
    let raw = b"<!DOCTYPE r><r/>";
    let mut reader = DtdBufferedReader::with_limits(
        Cursor::new(raw),
        Limits {
            max_input_bytes: 15,
            ..Limits::default()
        },
        configuration(),
    );
    for expected in ["dtd:r", "dtd:end"] {
        assert_eq!(name(reader.read_event().unwrap().unwrap().0), expected);
    }
    let error = reader.read_event().unwrap_err();
    assert!(matches!(
        error,
        BufferedSessionError::Session(SessionError::Xml(Error {
            kind: ErrorKind::LimitExceeded(Limit::InputBytes),
            offset: 0
        }))
    ));
    assert_eq!(reader.position(), 15);
    assert_eq!(reader.get_ref().position(), 15);
    assert!(matches!(
        reader.read_event().unwrap_err(),
        BufferedSessionError::Session(SessionError::Xml(Error {
            kind: ErrorKind::LimitExceeded(Limit::InputBytes),
            ..
        }))
    ));

    let mut reader = DtdBufferedReader::with_decode_limits(
        Cursor::new(raw),
        Limits {
            max_events: 2,
            ..Limits::default()
        },
        DecodeLimits::default(),
        configuration(),
    );
    assert_eq!(name(reader.read_event().unwrap().unwrap().0), "dtd:r");
    assert_eq!(name(reader.read_event().unwrap().unwrap().0), "dtd:end");
    assert!(matches!(
        reader.read_event().unwrap_err(),
        BufferedSessionError::Session(SessionError::Content(_))
    ));
    assert_eq!(reader.get_ref().position(), raw.len() as u64);
    assert_eq!(reader.position(), raw.len());
}

#[test]
fn first_io_cause_is_returned_once_then_no_more_source_calls() {
    struct Failing {
        calls: usize,
    }
    impl io::Read for Failing {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("BufRead required")
        }
    }
    impl BufRead for Failing {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            self.calls += 1;
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "original input cause",
            ))
        }
        fn consume(&mut self, _: usize) {
            panic!("no bytes offered")
        }
    }
    let mut reader = DtdBufferedReader::new(Failing { calls: 0 }, configuration());
    match reader.read_event().unwrap_err() {
        BufferedSessionError::Io(error) => {
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(error.to_string(), "original input cause");
        }
        error => panic!("wrong cause: {error:?}"),
    }
    assert_eq!(reader.get_ref().calls, 1);
    for _ in 0..2 {
        assert!(matches!(
            reader.read_event().unwrap_err(),
            BufferedSessionError::Session(SessionError::Xml(Error {
                kind: ErrorKind::InvalidState,
                offset: 0
            }))
        ));
    }
    assert_eq!(reader.into_inner().calls, 1);
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Trace {
    fills: usize,
    consumes: Vec<usize>,
    consumed: usize,
}
#[derive(Clone, Copy, Default)]
struct Faults {
    boundary: Option<usize>,
    panic_fill: bool,
    panic_consume: Option<bool>,
    interrupt: bool,
}
struct Source {
    bytes: Vec<u8>,
    at: usize,
    available: usize,
    capacity: usize,
    faults: Faults,
    interrupt_next: bool,
    trace: std::rc::Rc<std::cell::RefCell<Trace>>,
}
impl Source {
    fn new(
        bytes: Vec<u8>,
        capacity: usize,
        faults: Faults,
    ) -> (Self, std::rc::Rc<std::cell::RefCell<Trace>>) {
        let trace = std::rc::Rc::new(std::cell::RefCell::new(Trace::default()));
        (
            Self {
                bytes,
                at: 0,
                available: 0,
                capacity,
                faults,
                interrupt_next: faults.interrupt,
                trace: trace.clone(),
            },
            trace,
        )
    }
}
#[derive(Debug)]
struct InputCause;
impl std::fmt::Display for InputCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("original cause")
    }
}
impl std::error::Error for InputCause {}
impl io::Read for Source {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("BufRead required")
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
        if self.faults.boundary == Some(self.at) {
            if self.faults.panic_fill {
                panic!("fill_buf stopped");
            }
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, InputCause));
        }
        let end = self
            .at
            .saturating_add(self.capacity)
            .min(self.bytes.len())
            .min(self.faults.boundary.unwrap_or(self.bytes.len()));
        self.available = end - self.at;
        Ok(&self.bytes[self.at..end])
    }
    fn consume(&mut self, amount: usize) {
        assert!(amount <= self.available, "consume exceeded offered window");
        assert_ne!(amount, 0, "no zero-byte source mutations");
        self.trace.borrow_mut().consumes.push(amount);
        if self.faults.panic_consume == Some(false) {
            panic!("consume stopped before progress");
        }
        self.at += amount;
        self.available -= amount;
        self.trace.borrow_mut().consumed += amount;
        if self.faults.panic_consume == Some(true) {
            panic!("consume stopped after progress");
        }
    }
}
fn session_error(error: BufferedSessionError) -> SessionError {
    match error {
        BufferedSessionError::Session(error) => error,
        error => panic!("expected session error: {error:?}"),
    }
}
fn assert_poisoned(
    reader: &mut DtdBufferedReader<Source>,
    trace: &std::rc::Rc<std::cell::RefCell<Trace>>,
) {
    let before = trace.borrow().clone();
    let position = reader.position();
    for _ in 0..3 {
        assert_eq!(
            session_error(reader.read_event().unwrap_err()),
            SessionError::Xml(Error::new(ErrorKind::InvalidState, position))
        );
    }
    assert_eq!(*trace.borrow(), before);
}

#[test]
fn held_end_and_generated_events_are_drained_before_a_failing_fill() {
    for generated in [false, true] {
        let raw = if generated {
            "<!DOCTYPE r [<!ENTITY e '<a/>tail'>]><r>pre&e;after</r>"
        } else {
            "<!DOCTYPE r><r/>"
        };
        let boundary = if generated {
            raw.find("&e;").unwrap() + 3
        } else {
            12
        };
        let (source, trace) = Source::new(
            raw.as_bytes().to_vec(),
            65536,
            Faults {
                boundary: Some(boundary),
                ..Faults::default()
            },
        );
        let mut reader = DtdBufferedReader::new(source, configuration());
        let expected: &[&str] = if generated {
            &["dtd:r", "entity:e", "dtd:end", "+r", "Tpre", "=a", "Ttail"]
        } else {
            &["dtd:r", "dtd:end"]
        };
        let mut fills_at_boundary = None;
        for expected in expected {
            let (event, _) = reader.read_event().unwrap().unwrap();
            assert_eq!(name(event), *expected);
            let state = trace.borrow();
            if state.consumed == boundary {
                match fills_at_boundary {
                    None => fills_at_boundary = Some(state.fills),
                    Some(fills) => assert_eq!(state.fills, fills),
                }
            }
        }
        assert_eq!(trace.borrow().consumed, boundary);
        assert_eq!(reader.position(), boundary);
        match reader.read_event().unwrap_err() {
            BufferedSessionError::Io(error) => assert!(error.get_ref().unwrap().is::<InputCause>()),
            error => panic!("wrong cause: {error:?}"),
        }
        assert_poisoned(&mut reader, &trace);
    }
}

#[test]
fn real_utf16_windows_and_interruptions_preserve_events_origins_and_consumption() {
    let raw = "<?xml version='1.0' encoding='UTF-16'?><!--π--><!DOCTYPE r [<!ENTITY e '<a/>𝄞&#13;tail'><!ATTLIST a n NMTOKENS ' one  two '>]><r>π&e;post&#13;&amp;</r>";
    for little in [false, true] {
        for bom in [false, true] {
            let raw = raw.replace(
                "UTF-16",
                if bom {
                    "UTF-16"
                } else if little {
                    "UTF-16LE"
                } else {
                    "UTF-16BE"
                },
            );
            let mut bytes = Vec::new();
            if bom {
                bytes.extend(if little { [0xff, 0xfe] } else { [0xfe, 0xff] });
            }
            for unit in raw.encode_utf16() {
                bytes.extend(if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            let mut session =
                StreamingSession::new(Limits::default(), DecodeLimits::default(), configuration());
            let mut expected = Vec::new();
            session
                .advance(&bytes, &mut |event, origin, _| {
                    expected.push((name(event), origin));
                    Ok(())
                })
                .unwrap();
            session
                .finish(&mut |event, origin, _| {
                    expected.push((name(event), origin));
                    Ok(())
                })
                .unwrap();
            assert_eq!(
                expected.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
                [
                    "xml:None",
                    "Cπ",
                    "dtd:r",
                    "entity:e",
                    "attlist",
                    "dtd:end",
                    "+r",
                    "Tπ",
                    "=a n=one two",
                    "T𝄞\rtail",
                    "Tpost\r&",
                    "-r"
                ]
            );
            for capacity in [1, 2, 3, 7, 65536] {
                let (source, trace) = Source::new(
                    bytes.clone(),
                    capacity,
                    Faults {
                        interrupt: true,
                        ..Faults::default()
                    },
                );
                let mut reader = DtdBufferedReader::new(source, configuration());
                let mut rows = Vec::new();
                while let Some((event, origin)) = reader.read_event().unwrap() {
                    rows.push((name(event), origin));
                }
                assert_eq!(
                    rows, expected,
                    "little={little} bom={bom} capacity={capacity}"
                );
                assert_eq!(reader.position(), bytes.len());
                assert_eq!(trace.borrow().consumed, bytes.len());
                assert_eq!(reader.session.dtd_counters(), session.dtd_counters());
                let before = trace.borrow().clone();
                assert!(reader.read_event().unwrap().is_none());
                assert!(reader.read_event().unwrap().is_none());
                assert_eq!(*trace.borrow(), before);
            }
        }
    }
}

#[test]
fn xml_prepare_errors_commit_the_complete_accepted_prefix_before_returning() {
    for (raw, consumed) in [
        ("<!DOCTYPE r [x]><r/>", 14),
        (
            "<!DOCTYPE r [<!ENTITY e 'x' X>]><r/>",
            "<!DOCTYPE r [<!ENTITY e 'x' X>".len(),
        ),
    ] {
        let (source, trace) = Source::new(raw.as_bytes().to_vec(), 65536, Faults::default());
        let mut reader = DtdBufferedReader::new(source, configuration());
        assert_eq!(name(reader.read_event().unwrap().unwrap().0), "dtd:r");
        let error = session_error(reader.read_event().unwrap_err());
        assert!(matches!(error, SessionError::Dtd(_)));
        assert_eq!(reader.position(), consumed);
        assert_eq!(trace.borrow().consumed, consumed);
        let before = trace.borrow().clone();
        assert_eq!(session_error(reader.read_event().unwrap_err()), error);
        assert_eq!(*trace.borrow(), before);
    }
}

#[test]
fn fill_and_consume_panics_remain_poisoned_without_any_later_io() {
    for faults in [
        Faults {
            boundary: Some(0),
            panic_fill: true,
            ..Faults::default()
        },
        Faults {
            boundary: Some(12),
            panic_fill: true,
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
        let (source, trace) = Source::new(b"<!DOCTYPE r><r/>".to_vec(), 65536, faults);
        let mut reader = DtdBufferedReader::new(source, configuration());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            loop {
                if reader.read_event().unwrap().is_none() {
                    break;
                }
            }
        }));
        assert!(result.is_err());
        assert!(matches!(reader.status, Status::Poisoned));
        assert_poisoned(&mut reader, &trace);
    }
}

#[test]
fn construction_extraction_and_drop_do_not_drain_or_validate() {
    for constructor in 0..3 {
        for extract in [false, true] {
            let (source, trace) = Source::new(
                b"unread".to_vec(),
                65536,
                Faults {
                    boundary: Some(0),
                    panic_fill: true,
                    ..Faults::default()
                },
            );
            let reader = match constructor {
                0 => DtdBufferedReader::new(source, configuration()),
                1 => DtdBufferedReader::with_limits(source, Limits::default(), configuration()),
                _ => DtdBufferedReader::with_decode_limits(
                    source,
                    Limits::default(),
                    DecodeLimits::default(),
                    configuration(),
                ),
            };
            assert_eq!(reader.get_ref().at, 0);
            if extract {
                assert_eq!(reader.into_inner().at, 0);
            } else {
                drop(reader);
            }
            assert_eq!(*trace.borrow(), Trace::default());
        }
    }
    for extract in [false, true] {
        let (source, trace) =
            Source::new(b"<!DOCTYPE r><r/>unread".to_vec(), 65536, Faults::default());
        let mut reader = DtdBufferedReader::new(source, configuration());
        assert_eq!(name(reader.read_event().unwrap().unwrap().0), "dtd:r");
        assert!(reader.pending);
        let before = trace.borrow().clone();
        assert_eq!(before.consumed, 12, "consume precedes lending");
        if extract {
            assert_eq!(reader.into_inner().at, 12);
        } else {
            drop(reader);
        }
        assert_eq!(*trace.borrow(), before);
    }
}

#[test]
fn checked_progress_rejects_underflow_overshoot_and_mismatched_steps() {
    let error = Err(SessionError::Xml(Error::new(ErrorKind::Syntax, 6)));
    for (before, after, offered, reported, expected) in [
        (5, 7, 8, Some(2), Some(2)),
        (5, 7, 8, None, Some(2)),
        (7, 7, 0, Some(0), Some(0)),
        (7, 5, 8, None, None),
        (5, 8, 2, None, None),
        (5, 8, 3, Some(2), None),
        (usize::MAX - 1, usize::MAX, 1, Some(1), Some(1)),
        (usize::MAX, usize::MAX, 0, None, Some(0)),
    ] {
        let step = reported.map(|consumed| {
            Ok(SessionStep {
                consumed,
                outcome: SessionOutcome::Ready,
            })
        });
        assert_eq!(
            checked_consumed(before, after, offered, step.as_ref().unwrap_or(&error)),
            expected
        );
    }
}

#[test]
fn last_origin_tracks_successful_delivery_and_survives_eof_and_errors() {
    for suffix in ["</r>", "</x>"] {
        let raw = format!("<!DOCTYPE r [<!ENTITY e 'tail'>]><r>pre&e;{suffix}");
        let mut reader = DtdBufferedReader::new(Cursor::new(raw.as_bytes()), configuration());
        assert_eq!(reader.event_origin(), None);
        let mut last = None;
        let mut generated = false;
        loop {
            match reader.read_event() {
                Ok(Some((event, origin))) => {
                    generated |= origin.input_range().is_none();
                    drop(event);
                    assert_eq!(reader.event_origin(), Some(origin));
                    last = Some(origin);
                }
                Ok(None) => {
                    assert_eq!(suffix, "</r>");
                    break;
                }
                Err(error) => {
                    assert_eq!(suffix, "</x>");
                    let error = session_error(error);
                    assert_eq!(session_error(reader.read_event().unwrap_err()), error);
                    break;
                }
            }
        }
        assert!(generated);
        assert_eq!(reader.event_origin(), last);
    }
}

#[test]
fn whole_input_ceiling_is_inclusive_for_real_utf8_and_utf16_transport() {
    let raw = "<!DOCTYPE r><r/>";
    let mut transports = vec![raw.as_bytes().to_vec()];
    let mut utf8_bom = vec![0xef, 0xbb, 0xbf];
    utf8_bom.extend(raw.bytes());
    transports.push(utf8_bom);
    for little in [false, true] {
        let mut bytes = if little {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for unit in raw.encode_utf16() {
            bytes.extend(if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        transports.push(bytes);
    }
    for bytes in transports {
        for capacity in [1, 65536] {
            for max in [bytes.len() - 1, bytes.len(), bytes.len() + 1] {
                let (source, trace) = Source::new(bytes.clone(), capacity, Faults::default());
                let mut reader = DtdBufferedReader::with_limits(
                    source,
                    Limits {
                        max_input_bytes: max,
                        ..Limits::default()
                    },
                    configuration(),
                );
                for expected in ["dtd:r", "dtd:end"] {
                    assert_eq!(name(reader.read_event().unwrap().unwrap().0), expected);
                }
                if max < bytes.len() {
                    let error = session_error(reader.read_event().unwrap_err());
                    assert_eq!(
                        error,
                        SessionError::Xml(Error::new(
                            ErrorKind::LimitExceeded(Limit::InputBytes),
                            0
                        ))
                    );
                    assert_eq!(reader.position(), max);
                    assert_eq!(trace.borrow().consumed, max);
                    let before = trace.borrow().clone();
                    assert_eq!(session_error(reader.read_event().unwrap_err()), error);
                    assert_eq!(*trace.borrow(), before);
                } else {
                    assert_eq!(name(reader.read_event().unwrap().unwrap().0), "=r");
                    assert!(reader.read_event().unwrap().is_none());
                    assert_eq!(reader.position(), bytes.len());
                    assert_eq!(trace.borrow().consumed, bytes.len());
                    let before = trace.borrow().clone();
                    assert!(reader.read_event().unwrap().is_none());
                    assert_eq!(*trace.borrow(), before);
                }
            }
        }
    }
}

#[test]
fn malformed_utf16_carry_is_committed_before_its_exact_sticky_error() {
    for little in [false, true] {
        let mut prefix = if little {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for unit in "<!DOCTYPE r><r>𝄞".encode_utf16() {
            prefix.extend(if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        let anchor = prefix.len();
        for tail in [
            vec![0x41],
            if little {
                vec![0x00, 0xd8]
            } else {
                vec![0xd8, 0x00]
            },
            if little {
                vec![0x00, 0xdc]
            } else {
                vec![0xdc, 0x00]
            },
        ] {
            let mut bytes = prefix.clone();
            bytes.extend(tail);
            for capacity in [1, 65536] {
                let (source, trace) = Source::new(bytes.clone(), capacity, Faults::default());
                let mut reader = DtdBufferedReader::new(source, configuration());
                for expected in ["dtd:r", "dtd:end", "+r"] {
                    assert_eq!(name(reader.read_event().unwrap().unwrap().0), expected);
                }
                let expected = SessionError::Xml(Error::new(ErrorKind::InvalidUtf16, anchor));
                assert_eq!(session_error(reader.read_event().unwrap_err()), expected);
                assert_eq!(reader.position(), bytes.len());
                assert_eq!(trace.borrow().consumed, bytes.len());
                let before = trace.borrow().clone();
                assert_eq!(session_error(reader.read_event().unwrap_err()), expected);
                assert_eq!(*trace.borrow(), before);
            }
        }
    }
}
