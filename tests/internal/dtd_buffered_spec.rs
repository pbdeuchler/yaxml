//! Blind private BufRead specification, based only on the accepted session.
use super::buffered::{BufferedSessionError, DtdBufferedReader};
use super::*;
use crate::dtd::LiteralSource;
use crate::dtd_controller::content::ContentErrorKind;
use crate::dtd_reference::ReferenceLimits;
use crate::tokenizer::text::TextScratchLimits;
use crate::{BufferedReader, Limit, ReadError};
use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt;
use std::io::{self, BufRead, Read};
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

const DOC: &str = "<!DOCTYPE r [<!ENTITY e '<n/>x'><!ENTITY z ''>]><r>a&e;b&z;c</r>";
const MAIN: &str = "<!DOCTYPE r [<!ENTITY e '<n/>𝄞'><!ATTLIST n a CDATA '🙂'>]><r>α&e;β</r>";
const TINY: &str = "<!DOCTYPE r><r/>";
const TOKEN: usize = 4096;
const WORK: usize = 64 * 1024 * 1024;

fn options(input_ceiling: usize) -> SessionLimits {
    SessionLimits {
        declarations: ControllerLimits::default(),
        text: TextLimits {
            max_work: WORK,
            reference: ReferenceLimits {
                max_bytes: TOKEN,
                max_scratch_bytes: TOKEN,
                max_work: WORK,
            },
            scratch: TextScratchLimits {
                input_token_bytes: input_ceiling,
                replacement_token_bytes: TOKEN,
                max_bytes: input_ceiling + TOKEN * 2,
            },
        },
        local: LocalLimits {
            max_token_bytes: TOKEN,
            max_scratch_bytes: TOKEN,
            max_work: WORK,
        },
    }
}

fn limits() -> Limits {
    Limits {
        max_token_bytes: TOKEN,
        ..Limits::default()
    }
}
fn decode() -> DecodeLimits {
    DecodeLimits {
        max_decoded_token_bytes: TOKEN,
        ..DecodeLimits::default()
    }
}
type Adapter = DtdBufferedReader<Source>;
fn reader(source: Source) -> Adapter {
    DtdBufferedReader::with_decode_limits(source, limits(), decode(), options(TOKEN * 2))
}

// The accepted public BufferedReader fixtures use this same Source/Trace/Faults
// structure. Added boundary permits every two-window split without false EOF.
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
    panic_consume: Option<bool>,
    interrupt: bool,
}
struct Source {
    bytes: Vec<u8>,
    offset: usize,
    capacity: usize,
    boundary: usize,
    available: usize,
    faults: Faults,
    interrupt_next: bool,
    trace: Rc<RefCell<Trace>>,
}
impl Source {
    fn new(
        bytes: Vec<u8>,
        capacity: usize,
        boundary: usize,
        faults: Faults,
    ) -> (Self, Rc<RefCell<Trace>>) {
        let trace = Rc::new(RefCell::new(Trace::default()));
        (
            Self {
                bytes,
                offset: 0,
                capacity,
                boundary,
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
        panic!("adapter must use BufRead, not Read::read")
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
        for boundary in [
            Some(self.boundary),
            self.faults.error_at,
            self.faults.panic_fill_at,
        ]
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
        assert!(count > 0, "adapter must not consume zero bytes");
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

fn encode(text: &str, little: Option<bool>, bom: bool) -> Vec<u8> {
    let Some(little) = little else {
        return [
            if bom { b"\xef\xbb\xbf".as_slice() } else { b"" },
            text.as_bytes(),
        ]
        .concat();
    };
    let mut bytes = Vec::new();
    if bom {
        bytes.extend(if little { [255, 254] } else { [254, 255] });
    }
    for unit in text.encode_utf16() {
        bytes.extend(if little {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}
fn transport(offset: usize, little: Option<bool>) -> usize {
    if little.is_some() {
        offset * 2 + 2
    } else {
        offset
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Value {
    Declaration(String, Option<String>, Option<bool>),
    Begin(String, bool),
    Item(String, LiteralSource),
    DtdEnd,
    Start(String, Vec<(String, String)>),
    Empty(String, Vec<(String, String)>),
    End(String),
    Text(String),
    Cdata(String),
    Comment(String),
    Pi(String, String),
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Origin {
    input: Option<Range<usize>>,
    local: Option<Range<usize>>,
    physical: Range<usize>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Row {
    value: Value,
    origin: Origin,
}
fn origin(value: SourceOrigin) -> Origin {
    Origin {
        input: value.input_range(),
        local: value.replacement_range(),
        physical: value.physical_context(),
    }
}
fn capture(event: LocatedEvent<'_>, source: SourceOrigin) -> Row {
    let value = match event {
        LocatedEvent::Dtd(DtdEvent::Begin(h)) => {
            Value::Begin(h.root_name().into(), h.has_internal_subset())
        }
        LocatedEvent::Dtd(DtdEvent::Item { unit, literals }) => {
            Value::Item(unit.raw().into(), literals)
        }
        LocatedEvent::Dtd(DtdEvent::End) => Value::DtdEnd,
        LocatedEvent::Xml(event) => match event {
            Event::Declaration(d) => Value::Declaration(
                d.version.into(),
                d.encoding.map(str::to_owned),
                d.standalone,
            ),
            Event::Start(e) => Value::Start(
                e.name.qualified().into(),
                e.attributes
                    .into_iter()
                    .map(|a| (a.name.qualified().into(), a.value.into_owned()))
                    .collect(),
            ),
            Event::Empty(e) => Value::Empty(
                e.name.qualified().into(),
                e.attributes
                    .into_iter()
                    .map(|a| (a.name.qualified().into(), a.value.into_owned()))
                    .collect(),
            ),
            Event::End(n) => Value::End(n.qualified().into()),
            Event::Text(s) => Value::Text(s.into_owned()),
            Event::Cdata(s) => Value::Cdata(s.into_owned()),
            Event::Comment(s) => Value::Comment(s.into_owned()),
            Event::ProcessingInstruction { target, data } => {
                Value::Pi(target.into_owned(), data.into_owned())
            }
        },
    };
    Row {
        value,
        origin: origin(source),
    }
}
fn physical(value: Value, span: Range<usize>) -> Row {
    Row {
        value,
        origin: Origin {
            input: Some(span.clone()),
            local: None,
            physical: span,
        },
    }
}
fn expected(little: Option<bool>) -> Vec<Row> {
    let values = [
        Value::Begin("r".into(), true),
        Value::Item("<!ENTITY e '<n/>x'>".into(), LiteralSource::Physical),
        Value::Item("<!ENTITY z ''>".into(), LiteralSource::Physical),
        Value::DtdEnd,
        Value::Start("r".into(), vec![]),
        Value::Text("a".into()),
        Value::Empty("n".into(), vec![]),
        Value::Text("x".into()),
        Value::Text("b".into()),
        Value::Text("c".into()),
        Value::End("r".into()),
    ];
    let spans = [
        0..13,
        13..32,
        32..46,
        46..48,
        48..51,
        51..52,
        52..55,
        52..55,
        55..56,
        59..60,
        60..64,
    ];
    values
        .into_iter()
        .zip(spans)
        .enumerate()
        .map(|(i, (value, span))| {
            let span = transport(span.start, little)..transport(span.end, little);
            let local = match i {
                6 => Some(0..4),
                7 => Some(4..5),
                _ => None,
            };
            Row {
                value,
                origin: Origin {
                    input: local.is_none().then(|| span.clone()),
                    local,
                    physical: span,
                },
            }
        })
        .collect()
}
fn read_all(reader: &mut Adapter) -> Result<Vec<Row>, BufferedSessionError> {
    let mut rows = Vec::new();
    for _ in 0..128 {
        let Some((event, source)) = reader.read_event()? else {
            return Ok(rows);
        };
        rows.push(capture(event, source));
        assert_eq!(
            reader.event_origin().map(origin),
            rows.last().map(|r| r.origin.clone())
        );
    }
    panic!("unbounded or duplicate publication")
}
fn session_error(error: BufferedSessionError) -> SessionError {
    match error {
        BufferedSessionError::Session(error) => error,
        BufferedSessionError::Io(error) => panic!("unexpected I/O: {error}"),
    }
}
fn sticky(reader: &mut Adapter, trace: &Rc<RefCell<Trace>>, expected: SessionError) {
    let before = trace.borrow().clone();
    let position = reader.position();
    let source = reader.event_origin();
    for _ in 0..3 {
        assert_eq!(session_error(reader.read_event().unwrap_err()), expected);
    }
    assert_eq!(
        *trace.borrow(),
        before,
        "source touched after terminal failure"
    );
    assert_eq!(reader.position(), position);
    assert_eq!(reader.event_origin(), source);
}
fn poisoned(reader: &mut Adapter, trace: &Rc<RefCell<Trace>>) {
    let expected = SessionError::Xml(Error::new(ErrorKind::InvalidState, reader.position()));
    sticky(reader, trace, expected);
}
fn injected(error: BufferedSessionError) {
    let BufferedSessionError::Io(error) = error else {
        panic!("input failure must remain Io")
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
}

#[test]
fn accepted_callback_control_pins_literal_dtd_origins_and_empty_entity_projection() {
    for little in [None, Some(true), Some(false)] {
        let bytes = encode(DOC, little, little.is_some());
        let mut session = StreamingSession::new(limits(), decode(), options(TOKEN * 2));
        let mut rows = Vec::new();
        assert_eq!(
            session
                .advance(&bytes, &mut |e, o, _| {
                    rows.push(capture(e, o));
                    Ok(())
                })
                .unwrap(),
            bytes.len()
        );
        session
            .finish(&mut |e, o, _| {
                rows.push(capture(e, o));
                Ok(())
            })
            .unwrap();
        assert_eq!(rows, expected(little));
    }
}

#[test]
fn public_reader_control_validates_source_windows_interruption_and_fault_identity() {
    for little in [None, Some(true), Some(false)] {
        let bytes = encode("<r>π🙂</r>", little, little.is_some());
        for capacity in [1, 2, 7, 65536] {
            for boundary in 0..=bytes.len() {
                let (source, trace) = Source::new(
                    bytes.clone(),
                    capacity,
                    boundary,
                    Faults {
                        interrupt: true,
                        ..Faults::default()
                    },
                );
                let mut public = BufferedReader::new(source);
                let mut text = String::new();
                let mut count = 0;
                while let Some((event, _)) = public.read_event().unwrap() {
                    count += 1;
                    if let Event::Text(s) = event {
                        text.push_str(&s);
                    }
                }
                assert_eq!(count, 3);
                assert_eq!(text, "π🙂");
                assert_eq!(public.position(), bytes.len());
                assert_eq!(trace.borrow().consumed, bytes.len());
            }
        }
    }
    let (source, trace) = Source::new(
        b"<r>abc".to_vec(),
        65536,
        usize::MAX,
        Faults {
            error_at: Some(6),
            ..Faults::default()
        },
    );
    let mut public = BufferedReader::new(source);
    assert!(public.read_event().unwrap().is_some());
    let ReadError::Io(error) = public.read_event().unwrap_err() else {
        panic!("control must report Io")
    };
    assert_eq!(
        error
            .get_ref()
            .unwrap()
            .downcast_ref::<Injected>()
            .unwrap()
            .0,
        73
    );
    let before = trace.borrow().clone();
    assert!(matches!(
        public.read_event().unwrap_err(),
        ReadError::Xml(Error {
            kind: ErrorKind::InvalidState,
            offset: 6
        })
    ));
    assert_eq!(*trace.borrow(), before);
}

#[test]
fn every_small_window_and_split_preserves_order_origins_and_consumed_positions() {
    for little in [None, Some(true), Some(false)] {
        let bytes = encode(DOC, little, little.is_some());
        for capacity in [1, 2, 3, 7, 65536] {
            for boundary in 0..=bytes.len() {
                let (source, trace) =
                    Source::new(bytes.clone(), capacity, boundary, Faults::default());
                let mut reader = reader(source);
                let mut rows = Vec::new();
                for raw_position in [13, 32, 46, 48, 51, 55, 55, 55, 59, 61, 64] {
                    let (event, source) = reader.read_event().unwrap().unwrap();
                    rows.push(capture(event, source));
                    assert_eq!(reader.position(), transport(raw_position, little));
                    assert_eq!(reader.get_ref().offset, reader.position());
                    assert_eq!(trace.borrow().consumed, reader.position());
                    assert_eq!(
                        trace.borrow().consumes.iter().sum::<usize>(),
                        reader.position()
                    );
                    assert_eq!(
                        reader.event_origin().map(origin),
                        rows.last().map(|r| r.origin.clone())
                    );
                }
                assert_eq!(
                    rows,
                    expected(little),
                    "{little:?} capacity{capacity} split{boundary}"
                );
                assert!(reader.read_event().unwrap().is_none());
                let before = trace.borrow().clone();
                for _ in 0..3 {
                    assert!(reader.read_event().unwrap().is_none());
                }
                assert_eq!(*trace.borrow(), before);
            }
        }
    }
}

#[test]
fn no_subset_end_and_generated_tail_are_delivered_before_any_source_retry() {
    for little in [None, Some(true), Some(false)] {
        for capacity in [1, 3, 65536] {
            let bytes = encode(TINY, little, little.is_some());
            let gate = transport(12, little);
            let (source, trace) = Source::new(
                bytes,
                capacity,
                usize::MAX,
                Faults {
                    error_at: Some(gate),
                    ..Faults::default()
                },
            );
            let mut adapter = reader(source);
            let (event, source) = adapter.read_event().unwrap().unwrap();
            assert_eq!(
                capture(event, source),
                physical(
                    Value::Begin("r".into(), false),
                    transport(0, little)..transport(11, little)
                )
            );
            assert_eq!(adapter.position(), gate);
            let before = trace.borrow().clone();
            let (event, source) = adapter.read_event().unwrap().unwrap();
            assert_eq!(
                capture(event, source),
                physical(Value::DtdEnd, transport(11, little)..gate)
            );
            assert_eq!(*trace.borrow(), before, "held DTD End must not read source");
            injected(adapter.read_event().unwrap_err());
            poisoned(&mut adapter, &trace);

            let bytes = encode(DOC, little, little.is_some());
            let gate = transport(55, little);
            let (source, trace) = Source::new(
                bytes,
                capacity,
                usize::MAX,
                Faults {
                    error_at: Some(gate),
                    ..Faults::default()
                },
            );
            let mut adapter = reader(source);
            let mut rows = Vec::new();
            for _ in 0..6 {
                let (e, o) = adapter.read_event().unwrap().unwrap();
                rows.push(capture(e, o));
            }
            assert_eq!(adapter.position(), gate);
            let before = trace.borrow().clone();
            for _ in 0..2 {
                let (e, o) = adapter.read_event().unwrap().unwrap();
                rows.push(capture(e, o));
                assert_eq!(*trace.borrow(), before);
            }
            assert_eq!(rows, expected(little)[..8]);
            injected(adapter.read_event().unwrap_err());
            assert_eq!(adapter.position(), gate);
            poisoned(&mut adapter, &trace);
        }
    }
}

#[test]
fn constructors_and_early_drop_or_extraction_never_drain_or_validate_suffix() {
    for constructor in 0..3 {
        for extract in [false, true] {
            let (source, trace) = Source::new(
                TINY.as_bytes().to_vec(),
                65536,
                usize::MAX,
                Faults {
                    panic_fill_at: Some(0),
                    ..Faults::default()
                },
            );
            let adapter = match constructor {
                0 => DtdBufferedReader::new(source, options(2 * 1024 * 1024)),
                1 => DtdBufferedReader::with_limits(source, limits(), options(TOKEN * 2)),
                _ => reader(source),
            };
            assert_eq!(adapter.position(), 0);
            assert_eq!(adapter.event_origin(), None);
            assert_eq!(adapter.get_ref().offset, 0);
            if extract {
                assert_eq!(adapter.into_inner().offset, 0);
            } else {
                drop(adapter);
            }
            assert_eq!(*trace.borrow(), Trace::default());
        }
    }
    for extract in [false, true] {
        let raw = b"<!DOCTYPE r><r/>unread-malformed";
        let (source, trace) = Source::new(raw.to_vec(), 65536, usize::MAX, Faults::default());
        let mut adapter = reader(source);
        assert!(matches!(
            adapter.read_event().unwrap().unwrap().0,
            LocatedEvent::Dtd(DtdEvent::Begin(_))
        ));
        let before = trace.borrow().clone();
        assert_eq!(before.consumed, 12);
        if extract {
            let source = adapter.into_inner();
            assert_eq!(&source.bytes[source.offset..], &raw[12..]);
        } else {
            drop(adapter);
        }
        assert_eq!(*trace.borrow(), before);
    }
}

#[test]
fn each_constructor_accepts_the_same_minimal_dtd_document() {
    for constructor in 0..3 {
        let (source, _) = Source::new(
            TINY.as_bytes().to_vec(),
            65536,
            usize::MAX,
            Faults::default(),
        );
        let mut adapter = match constructor {
            0 => DtdBufferedReader::new(source, options(2 * 1024 * 1024)),
            1 => DtdBufferedReader::with_limits(source, limits(), options(TOKEN * 2)),
            _ => reader(source),
        };
        let rows = read_all(&mut adapter).unwrap();
        assert_eq!(
            rows,
            [
                physical(Value::Begin("r".into(), false), 0..11),
                physical(Value::DtdEnd, 11..12),
                physical(Value::Empty("r".into(), vec![]), 12..16)
            ]
        );
    }
}

#[test]
fn interruptions_retry_without_losing_dtd_or_generated_events() {
    for little in [None, Some(true), Some(false)] {
        let bytes = encode(DOC, little, little.is_some());
        for capacity in [1, 2, 3, 7, 65536] {
            let (source, trace) = Source::new(
                bytes.clone(),
                capacity,
                usize::MAX,
                Faults {
                    interrupt: true,
                    ..Faults::default()
                },
            );
            let mut adapter = reader(source);
            assert_eq!(read_all(&mut adapter).unwrap(), expected(little));
            assert_eq!(adapter.position(), bytes.len());
            assert_eq!(trace.borrow().consumed, bytes.len());
        }
    }
}

#[test]
fn physical_text_borrows_while_generated_text_can_be_moved_out_and_retained() {
    let raw = "<!DOCTYPE r [<!ENTITY e 'x'>]><r>p&e;z</r>";
    let (source, _) = Source::new(
        raw.as_bytes().to_vec(),
        65536,
        usize::MAX,
        Faults::default(),
    );
    let mut adapter = reader(source);
    for _ in 0..4 {
        assert!(adapter.read_event().unwrap().is_some());
    }
    let (event, source) = adapter.read_event().unwrap().unwrap();
    let LocatedEvent::Xml(Event::Text(Cow::Borrowed(text))) = event else {
        panic!("physical text must borrow retained storage");
    };
    assert_eq!(text, "p");
    assert!(source.input_range().is_some());
    let copied = text.to_owned();
    let (event, source) = adapter.read_event().unwrap().unwrap();
    let LocatedEvent::Xml(Event::Text(Cow::Owned(retained))) = event else {
        panic!("generated text owns its fields");
    };
    assert_eq!(source.replacement_range(), Some(0..1));
    assert_eq!(source.input_range(), None);
    let (event, _) = adapter.read_event().unwrap().unwrap();
    assert!(matches!(
        event,
        LocatedEvent::Xml(Event::Text(Cow::Borrowed("z")))
    ));
    assert!(matches!(
        adapter.read_event().unwrap().unwrap().0,
        LocatedEvent::Xml(Event::End(_))
    ));
    assert!(adapter.read_event().unwrap().is_none());
    drop(adapter);
    assert_eq!(copied, "p");
    assert_eq!(retained, "x");
}

#[test]
fn physical_leaf_kinds_and_skipped_top_level_whitespace_keep_their_order() {
    let raw = "\r\n<!--p--><!DOCTYPE r><r><![CDATA[x]]><?t z?><!--c--></r> \t";
    let (source, _) = Source::new(raw.as_bytes().to_vec(), 1, usize::MAX, Faults::default());
    let mut adapter = reader(source);
    assert_eq!(
        read_all(&mut adapter)
            .unwrap()
            .into_iter()
            .map(|r| r.value)
            .collect::<Vec<_>>(),
        [
            Value::Comment("p".into()),
            Value::Begin("r".into(), false),
            Value::DtdEnd,
            Value::Start("r".into(), vec![]),
            Value::Cdata("x".into()),
            Value::Pi("t".into(), "z".into()),
            Value::Comment("c".into()),
            Value::End("r".into()),
        ]
    );
    assert_eq!(adapter.position(), raw.len());
}

#[test]
fn mid_token_io_failures_are_returned_once_without_partial_publications() {
    let mut cases = vec![
        (TINY.as_bytes().to_vec(), 0, 0),
        (TINY.as_bytes().to_vec(), 9, 0),
        (TINY.as_bytes().to_vec(), 11, 0),
        (TINY.as_bytes().to_vec(), 12, 2),
        (TINY.as_bytes().to_vec(), 14, 2),
        (TINY.as_bytes().to_vec(), 16, 3),
    ];
    for little in [true, false] {
        let bytes = encode("<!DOCTYPE r><r>🙂</r>", Some(little), true);
        for (at, count) in [(1, 0), (19, 0), (35, 3), (38, 4)] {
            cases.push((bytes.clone(), at, count));
        }
    }
    for (bytes, at, events) in cases {
        for capacity in [1, 7, 65536] {
            let (source, trace) = Source::new(
                bytes.clone(),
                capacity,
                usize::MAX,
                Faults {
                    error_at: Some(at),
                    ..Faults::default()
                },
            );
            let mut adapter = reader(source);
            let mut seen = 0;
            let failure = loop {
                match adapter.read_event() {
                    Ok(Some(_)) => seen += 1,
                    Ok(None) => panic!("I/O failure became EOF"),
                    Err(e) => break e,
                }
            };
            injected(failure);
            assert_eq!(seen, events, "unexpected partial event at{at}");
            assert_eq!(adapter.position(), at);
            assert_eq!(trace.borrow().consumed, at);
            poisoned(&mut adapter, &trace);
        }
    }
}

#[test]
fn recovered_fill_and_consume_panics_are_terminal_without_later_source_calls() {
    for faults in [
        Faults {
            panic_fill_at: Some(0),
            ..Faults::default()
        },
        Faults {
            panic_fill_at: Some(10),
            ..Faults::default()
        },
        Faults {
            panic_fill_at: Some(53),
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
        let (source, trace) = Source::new(DOC.as_bytes().to_vec(), 65536, usize::MAX, faults);
        let mut adapter = reader(source);
        let result = catch_unwind(AssertUnwindSafe(|| {
            read_all(&mut adapter).unwrap();
        }));
        assert!(result.is_err());
        poisoned(&mut adapter, &trace);
    }
}

#[test]
fn exact_input_budget_probes_eof_and_never_consumes_the_excess_byte() {
    for little in [None, Some(true), Some(false)] {
        let root = encode(TINY, little, little.is_some());
        for capacity in [1, 7, 65536] {
            for tail in [b"".as_slice(), b"unread"] {
                let (source, trace) = Source::new(
                    [root.as_slice(), tail].concat(),
                    capacity,
                    usize::MAX,
                    Faults::default(),
                );
                let mut adapter = DtdBufferedReader::with_limits(
                    source,
                    Limits {
                        max_input_bytes: root.len(),
                        ..limits()
                    },
                    options(TOKEN * 2),
                );
                for _ in 0..3 {
                    assert!(adapter.read_event().unwrap().is_some());
                }
                assert_eq!(adapter.position(), root.len());
                let before = trace.borrow().clone();
                let result = adapter.read_event();
                if tail.is_empty() {
                    assert!(result.unwrap().is_none());
                } else {
                    let failure = session_error(result.unwrap_err());
                    assert_eq!(
                        failure,
                        SessionError::Xml(Error::new(
                            ErrorKind::LimitExceeded(Limit::InputBytes),
                            0
                        ))
                    );
                    sticky(&mut adapter, &trace, failure);
                }
                assert_eq!(trace.borrow().fills, before.fills + 1);
                assert_eq!(trace.borrow().consumed, root.len());
                let source = adapter.into_inner();
                assert_eq!(&source.bytes[source.offset..], tail);
            }
        }
        let (source, trace) = Source::new(
            root.clone(),
            65536,
            usize::MAX,
            Faults {
                error_at: Some(root.len()),
                ..Faults::default()
            },
        );
        let mut adapter = DtdBufferedReader::with_limits(
            source,
            Limits {
                max_input_bytes: root.len(),
                ..limits()
            },
            options(TOKEN * 2),
        );
        for _ in 0..3 {
            assert!(adapter.read_event().unwrap().is_some());
        }
        let before = trace.borrow().fills;
        injected(adapter.read_event().unwrap_err());
        assert_eq!(trace.borrow().fills, before + 1);
        assert_eq!(adapter.position(), root.len());
        poisoned(&mut adapter, &trace);
    }
    for (bytes, kind) in [
        (b"".as_slice(), ErrorKind::MissingRoot),
        (b"<r/>", ErrorKind::LimitExceeded(Limit::InputBytes)),
    ] {
        let (source, trace) = Source::new(bytes.to_vec(), 65536, usize::MAX, Faults::default());
        let mut adapter = DtdBufferedReader::with_limits(
            source,
            Limits {
                max_input_bytes: 0,
                ..limits()
            },
            options(TOKEN * 2),
        );
        let failure = session_error(adapter.read_event().unwrap_err());
        assert_eq!(failure, SessionError::Xml(Error::new(kind, 0)));
        assert_eq!(trace.borrow().consumed, 0);
        sticky(&mut adapter, &trace, failure);
    }
}

#[test]
fn generated_work_drains_at_the_whole_input_budget_before_excess_is_reported() {
    for little in [None, Some(true), Some(false)] {
        let bytes = encode(DOC, little, little.is_some());
        let quota = transport(55, little);
        let (source, trace) = Source::new(bytes, 65536, usize::MAX, Faults::default());
        let mut adapter = DtdBufferedReader::with_limits(
            source,
            Limits {
                max_input_bytes: quota,
                ..limits()
            },
            options(TOKEN * 2),
        );
        let mut rows = Vec::new();
        for _ in 0..8 {
            let (e, o) = adapter.read_event().unwrap().unwrap();
            rows.push(capture(e, o));
        }
        assert_eq!(rows, expected(little)[..8]);
        assert_eq!(adapter.position(), quota);
        assert_eq!(trace.borrow().consumed, quota);
        let failure = session_error(adapter.read_event().unwrap_err());
        assert_eq!(
            failure,
            SessionError::Xml(Error::new(ErrorKind::LimitExceeded(Limit::InputBytes), 0))
        );
        sticky(&mut adapter, &trace, failure);
    }
}

#[test]
fn input_budget_counts_text_lookahead_and_can_stop_inside_a_transport_scalar() {
    for (quota, count) in [(60, 9), (61, 10)] {
        let (source, trace) = Source::new(
            DOC.as_bytes().to_vec(),
            65536,
            usize::MAX,
            Faults::default(),
        );
        let mut adapter = DtdBufferedReader::with_limits(
            source,
            Limits {
                max_input_bytes: quota,
                ..limits()
            },
            options(TOKEN * 2),
        );
        let mut rows = Vec::new();
        for _ in 0..count {
            let (e, o) = adapter.read_event().unwrap().unwrap();
            rows.push(capture(e, o));
        }
        assert_eq!(rows, expected(None)[..count]);
        let failure = session_error(adapter.read_event().unwrap_err());
        assert_eq!(
            failure,
            SessionError::Xml(Error::new(ErrorKind::LimitExceeded(Limit::InputBytes), 0))
        );
        assert_eq!(adapter.position(), quota);
        assert_eq!(trace.borrow().consumed, quota);
        sticky(&mut adapter, &trace, failure);
    }
    for little in [true, false] {
        for (quota, events) in [(0, 0), (1, 0), (2, 0), (33, 3), (35, 3)] {
            let bytes = encode("<!DOCTYPE r><r>🙂</r>", Some(little), true);
            let (source, trace) = Source::new(bytes, 65536, usize::MAX, Faults::default());
            let mut adapter = DtdBufferedReader::with_limits(
                source,
                Limits {
                    max_input_bytes: quota,
                    ..limits()
                },
                options(TOKEN * 2),
            );
            for _ in 0..events {
                assert!(adapter.read_event().unwrap().is_some());
            }
            let failure = session_error(adapter.read_event().unwrap_err());
            assert_eq!(
                failure,
                SessionError::Xml(Error::new(ErrorKind::LimitExceeded(Limit::InputBytes), 0))
            );
            assert_eq!(adapter.position(), quota);
            assert_eq!(trace.borrow().consumed, quota);
            sticky(&mut adapter, &trace, failure);
        }
    }
}

#[test]
fn event_and_decoded_budgets_are_inclusive_and_keep_typed_session_failures() {
    for maximum in [10, 11, 12] {
        let (source, trace) = Source::new(
            DOC.as_bytes().to_vec(),
            65536,
            usize::MAX,
            Faults::default(),
        );
        let mut adapter = DtdBufferedReader::with_limits(
            source,
            Limits {
                max_events: maximum,
                ..limits()
            },
            options(TOKEN * 2),
        );
        let mut rows = Vec::new();
        for _ in 0..maximum.min(11) {
            let (e, o) = adapter.read_event().unwrap().unwrap();
            rows.push(capture(e, o));
        }
        if maximum < 11 {
            let failure = session_error(adapter.read_event().unwrap_err());
            assert!(matches!(
                failure,
                SessionError::Content(ContentError {
                    kind: ContentErrorKind::Parser(ErrorKind::LimitExceeded(Limit::Events)),
                    ..
                })
            ));
            assert_eq!(failure.physical_offset(), 60);
            assert_eq!(rows, expected(None)[..10]);
            assert_eq!(trace.borrow().consumed, 64);
            sticky(&mut adapter, &trace, failure);
        } else {
            assert_eq!(rows, expected(None));
            assert!(adapter.read_event().unwrap().is_none());
        }
    }
    for little in [None, Some(true), Some(false)] {
        let bytes = encode(TINY, little, little.is_some());
        for maximum in [15, 16, 17] {
            let (source, trace) = Source::new(bytes.clone(), 1, usize::MAX, Faults::default());
            let mut adapter = DtdBufferedReader::with_decode_limits(
                source,
                limits(),
                DecodeLimits {
                    max_decoded_bytes: maximum,
                    ..decode()
                },
                options(TOKEN * 2),
            );
            if maximum < 16 {
                let failure = session_error(read_all(&mut adapter).unwrap_err());
                assert_eq!(
                    failure,
                    SessionError::Xml(Error::new(ErrorKind::LimitExceeded(Limit::DecodedBytes), 0))
                );
                sticky(&mut adapter, &trace, failure);
            } else {
                assert_eq!(read_all(&mut adapter).unwrap().len(), 3);
            }
        }
    }
}

#[test]
fn lexical_and_decoder_errors_commit_only_the_physically_accepted_prefix() {
    for little in [None, Some(true), Some(false)] {
        let raw = "<!DOCTYPE r><r>&bogus;</r>";
        let bytes = encode(raw, little, little.is_some());
        let (source, trace) = Source::new(bytes, 65536, usize::MAX, Faults::default());
        let mut adapter = reader(source);
        for _ in 0..3 {
            assert!(adapter.read_event().unwrap().is_some());
        }
        let failure = session_error(adapter.read_event().unwrap_err());
        assert_eq!(failure.physical_offset(), transport(15, little));
        assert_eq!(adapter.position(), transport(22, little));
        assert_eq!(trace.borrow().consumed, adapter.position());
        sticky(&mut adapter, &trace, failure);
    }
    for little in [true, false] {
        let mut bytes = encode("<!DOCTYPE r><r>", Some(little), true);
        bytes.extend(if little {
            0xdc00u16.to_le_bytes()
        } else {
            0xdc00u16.to_be_bytes()
        });
        bytes.extend([1, 2, 3, 4]);
        for capacity in [1, 65536] {
            let (source, trace) =
                Source::new(bytes.clone(), capacity, usize::MAX, Faults::default());
            let mut adapter = reader(source);
            for _ in 0..3 {
                assert!(adapter.read_event().unwrap().is_some());
            }
            let failure = session_error(adapter.read_event().unwrap_err());
            assert_eq!(
                failure,
                SessionError::Xml(Error::new(ErrorKind::InvalidUtf16, 32))
            );
            assert_eq!(adapter.position(), 34);
            assert_eq!(trace.borrow().consumed, 34);
            sticky(&mut adapter, &trace, failure);
        }
    }
}

#[test]
fn eof_text_is_delivered_before_the_sticky_missing_end_error() {
    let raw = "<!DOCTYPE r><r>tail";
    let (source, trace) = Source::new(
        raw.as_bytes().to_vec(),
        65536,
        usize::MAX,
        Faults::default(),
    );
    let mut adapter = reader(source);
    for _ in 0..3 {
        assert!(adapter.read_event().unwrap().is_some());
    }
    let (event, source) = adapter.read_event().unwrap().unwrap();
    assert_eq!(
        capture(event, source),
        physical(Value::Text("tail".into()), 15..19)
    );
    let before = trace.borrow().clone();
    let failure = session_error(adapter.read_event().unwrap_err());
    assert!(matches!(
        failure,
        SessionError::Content(ContentError {
            kind: ContentErrorKind::Parser(ErrorKind::UnexpectedEof),
            physical_offset: 19,
            ..
        })
    ));
    assert_eq!(*trace.borrow(), before, "EOF was already observed");
    sticky(&mut adapter, &trace, failure);
}

#[test]
fn non_bmp_dtd_values_defaults_and_body_origins_survive_each_physical_split() {
    let values = [
        Value::Begin("r".into(), true),
        Value::Item("<!ENTITY e '<n/>𝄞'>".into(), LiteralSource::Physical),
        Value::Item("<!ATTLIST n a CDATA '🙂'>".into(), LiteralSource::Physical),
        Value::DtdEnd,
        Value::Start("r".into(), vec![]),
        Value::Text("α".into()),
        Value::Empty("n".into(), vec![("a".into(), "🙂".into())]),
        Value::Text("𝄞".into()),
        Value::Text("β".into()),
        Value::End("r".into()),
    ];
    for little in [true, false] {
        let bytes = encode(MAIN, Some(little), true);
        let spans = [
            2..28,
            28..68,
            68..118,
            118..122,
            122..128,
            128..130,
            130..136,
            130..136,
            136..138,
            138..146,
        ];
        for capacity in [1, 3, 65536] {
            for boundary in 0..=bytes.len() {
                let (source, _) = Source::new(bytes.clone(), capacity, boundary, Faults::default());
                let mut adapter = reader(source);
                let rows = read_all(&mut adapter).unwrap();
                assert_eq!(
                    rows.iter().map(|r| &r.value).collect::<Vec<_>>(),
                    values.iter().collect::<Vec<_>>()
                );
                for (i, row) in rows.iter().enumerate() {
                    assert_eq!(row.origin.physical, spans[i]);
                    match i {
                        6 => {
                            assert_eq!(row.origin.input, None);
                            assert_eq!(row.origin.local, Some(0..4));
                        }
                        7 => {
                            assert_eq!(row.origin.input, None);
                            assert_eq!(row.origin.local, Some(4..8));
                        }
                        _ => {
                            assert_eq!(row.origin.input, Some(spans[i].clone()));
                            assert_eq!(row.origin.local, None);
                        }
                    }
                }
                assert_eq!(adapter.position(), 146);
            }
        }
    }
}

#[test]
fn declaration_spelling_and_bomless_utf16_survive_buffered_dtd_handoff() {
    for little in [true, false] {
        for bom in [false, true] {
            let label = if bom {
                "uTf-16"
            } else if little {
                "uTf-16lE"
            } else {
                "UTF-16BE"
            };
            let prefix = format!("<?xml version='1.0' encoding='{label}' standalone='yes'?>");
            let bytes = encode(&format!("{prefix}{TINY}"), Some(little), bom);
            for capacity in [1, 3, 65536] {
                let (source, trace) =
                    Source::new(bytes.clone(), capacity, usize::MAX, Faults::default());
                let mut adapter = reader(source);
                let rows = read_all(&mut adapter).unwrap();
                let shift = usize::from(bom) * 2 + prefix.len() * 2;
                assert_eq!(
                    rows,
                    [
                        physical(
                            Value::Declaration("1.0".into(), Some(label.into()), Some(true)),
                            usize::from(bom) * 2..shift
                        ),
                        physical(Value::Begin("r".into(), false), shift..shift + 22),
                        physical(Value::DtdEnd, shift + 22..shift + 24),
                        physical(Value::Empty("r".into(), vec![]), shift + 24..shift + 32)
                    ]
                );
                assert_eq!(trace.borrow().consumed, bytes.len());
            }
        }
    }
}
