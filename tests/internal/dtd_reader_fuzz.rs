//! Bounded deterministic driver for the private stable-input DTD reader.

use super::dtd::{DtdReader, FuzzFailure, FuzzPhase, FuzzSnapshot, ReaderEvent};
use super::*;
use crate::dtd::{DtdUnit, LiteralSource, RawSource};
use crate::dtd_controller::{ControllerCounters, ControllerLimits, TakenDtdEvent};
use crate::dtd_frames::FrameCounters;
use crate::dtd_reference::ReferenceLimits;
use crate::stream::dtd::{SessionError, SessionLimits};
use crate::tokenizer::local::LocalLimits;
use crate::tokenizer::text::{TextLimits, TextScratchLimits, TextUsage};
use std::io::{self, Read, Write};
use std::ops::Range;
use std::process::{Command, Stdio};

const MAX_INPUT: usize = 1024 * 1024;
const MAX_GENERATED: usize = 4 * 1024;
const MAX_EVENTS: usize = 128;
const MAX_DECLARATIONS: usize = 4;
const MAX_BODY_OPERATIONS: usize = 8;
const TOKEN: usize = 4096;
const WORK: usize = 64 * 1024 * 1024;

fn options() -> SessionLimits {
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
                input_token_bytes: Limits::default().max_token_bytes,
                replacement_token_bytes: TOKEN,
                max_bytes: Limits::default().max_token_bytes + TOKEN * 2,
            },
        },
        local: LocalLimits {
            max_token_bytes: TOKEN,
            max_scratch_bytes: TOKEN,
            max_work: WORK,
        },
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ExpectedKind {
    DtdBegin(String, RawSource),
    DtdItem(String, LiteralSource, RawSource),
    DtdEnd,
    Declaration(String, Option<String>, Option<bool>),
    Start(String, Option<String>, Vec<ExpectedAttribute>),
    Empty(String, Option<String>, Vec<ExpectedAttribute>),
    End(String, Option<String>),
    Text(String),
    Cdata(String),
    Comment(String),
    Pi(String, String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExpectedAttribute {
    name: String,
    namespace: Option<String>,
    value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExpectedOrigin {
    input: Option<Range<usize>>,
    replacement: Option<Range<usize>>,
    physical: Range<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExpectedEvent {
    kind: ExpectedKind,
    origin: ExpectedOrigin,
}

struct GeneratedCase {
    source: String,
    expected: Vec<ExpectedEvent>,
}

fn physical(kind: ExpectedKind, range: Range<usize>) -> ExpectedEvent {
    ExpectedEvent {
        kind,
        origin: ExpectedOrigin {
            input: Some(range.clone()),
            replacement: None,
            physical: range,
        },
    }
}

fn replacement(
    kind: ExpectedKind,
    local: Range<usize>,
    physical_range: Range<usize>,
) -> ExpectedEvent {
    ExpectedEvent {
        kind,
        origin: ExpectedOrigin {
            input: None,
            replacement: Some(local),
            physical: physical_range,
        },
    }
}

fn push_physical(case: &mut GeneratedCase, raw: &str, kind: ExpectedKind) -> Range<usize> {
    let start = case.source.len();
    case.source.push_str(raw);
    let range = start..case.source.len();
    case.expected.push(physical(kind, range.clone()));
    range
}

fn push_physical_text(case: &mut GeneratedCase, value: &str) {
    let start = case.source.len();
    case.source.push_str(value);
    let end = case.source.len();
    if let Some(ExpectedEvent {
        kind: ExpectedKind::Text(previous),
        origin:
            ExpectedOrigin {
                input: Some(input),
                replacement: None,
                physical,
            },
    }) = case.expected.last_mut()
        && input.end == start
    {
        previous.push_str(value);
        input.end = end;
        physical.end = end;
    } else {
        case.expected
            .push(physical(ExpectedKind::Text(value.to_owned()), start..end));
    }
}

fn control(input: &[u8], at: usize) -> u8 {
    input.get(at).copied().unwrap_or(0)
}

fn generated_case(input: &[u8]) -> GeneratedCase {
    let root = match control(input, 0) % 3 {
        0 => "r",
        1 => "root",
        _ => "x",
    };
    let declaration_count = usize::from(control(input, 1)) % (MAX_DECLARATIONS + 1);
    let body_count = usize::from(control(input, 2)) % (MAX_BODY_OPERATIONS + 1);
    let shape = control(input, 3) % 4;
    let mut case = GeneratedCase {
        source: String::new(),
        expected: Vec::new(),
    };

    if declaration_count == 0 {
        let raw = format!("<!DOCTYPE {root}>");
        let start = case.source.len();
        case.source.push_str(&raw);
        case.expected.push(physical(
            ExpectedKind::DtdBegin(raw.clone(), RawSource::Borrowed),
            start..case.source.len() - 1,
        ));
        let end = case.source.len() - 1..case.source.len();
        case.expected.push(physical(ExpectedKind::DtdEnd, end));
    } else {
        let header = format!("<!DOCTYPE {root} [");
        push_physical(
            &mut case,
            &header,
            ExpectedKind::DtdBegin(header.clone(), RawSource::Borrowed),
        );
        for index in 0..declaration_count {
            let letter = char::from(b'a' + control(input, 8 + index) % 26);
            let name = format!("e{index}{letter}");
            let value = match (shape, index) {
                (1, _) => String::new(),
                (2, _) => format!("<n{index}/>t"),
                (3, 0) => "A".to_owned(),
                (3, _) => {
                    let first = char::from(b'a' + control(input, 8) % 26);
                    format!("&e0{first};B")
                }
                _ => format!("V{index}"),
            };
            let raw = format!("<!ENTITY {name} '{value}'>");
            push_physical(
                &mut case,
                &raw,
                ExpectedKind::DtdItem(raw.clone(), LiteralSource::Physical, RawSource::Borrowed),
            );
        }
        push_physical(&mut case, "]>", ExpectedKind::DtdEnd);
    }

    let start_tag = format!("<{root}>");
    push_physical(
        &mut case,
        &start_tag,
        ExpectedKind::Start(root.to_owned(), None, Vec::new()),
    );
    for operation in 0..body_count {
        let choice = control(input, 16 + operation) % 4;
        match choice {
            0 => {
                let value = char::from(b'a' + control(input, 32 + operation) % 26).to_string();
                push_physical_text(&mut case, &value);
            }
            1 if declaration_count > 0 => {
                let index = usize::from(control(input, 32 + operation)) % declaration_count;
                let letter = char::from(b'a' + control(input, 8 + index) % 26);
                let name = format!("e{index}{letter}");
                let reference = format!("&{name};");
                let start = case.source.len();
                case.source.push_str(&reference);
                let occurrence = start..case.source.len();
                match (shape, index) {
                    (1, _) => {}
                    (2, _) => {
                        case.expected.push(replacement(
                            ExpectedKind::Empty(format!("n{index}"), None, Vec::new()),
                            0..format!("<n{index}/>").len(),
                            occurrence.clone(),
                        ));
                        let local = format!("<n{index}/>").len();
                        case.expected.push(replacement(
                            ExpectedKind::Text("t".to_owned()),
                            local..local + 1,
                            occurrence,
                        ));
                    }
                    (3, 0) => case.expected.push(replacement(
                        ExpectedKind::Text("A".to_owned()),
                        0..1,
                        occurrence,
                    )),
                    (3, _) => {
                        let first = char::from(b'a' + control(input, 8) % 26);
                        case.expected.push(replacement(
                            ExpectedKind::Text("A".to_owned()),
                            0..1,
                            occurrence.clone(),
                        ));
                        case.expected.push(replacement(
                            ExpectedKind::Text("B".to_owned()),
                            format!("&e0{first};").len()..format!("&e0{first};B").len(),
                            occurrence,
                        ));
                    }
                    _ => case.expected.push(replacement(
                        ExpectedKind::Text(format!("V{index}")),
                        0..2,
                        occurrence,
                    )),
                }
            }
            2 => {
                let raw = format!("<n{operation}/>");
                push_physical(
                    &mut case,
                    &raw,
                    ExpectedKind::Empty(format!("n{operation}"), None, Vec::new()),
                );
            }
            _ => {
                let raw = format!("<!--c{operation}-->");
                push_physical(
                    &mut case,
                    &raw,
                    ExpectedKind::Comment(format!("c{operation}")),
                );
            }
        }
    }
    let end_tag = format!("</{root}>");
    push_physical(
        &mut case,
        &end_tag,
        ExpectedKind::End(root.to_owned(), None),
    );
    assert!(case.source.len() <= MAX_GENERATED);
    assert!(case.expected.len() <= MAX_EVENTS);
    case
}

fn observed_kind(event: &ReaderEvent<'_>) -> ExpectedKind {
    match event {
        ReaderEvent::Dtd(TakenDtdEvent::Begin(unit)) => {
            ExpectedKind::DtdBegin(unit.raw().to_owned(), unit.raw_source())
        }
        ReaderEvent::Dtd(TakenDtdEvent::Item { unit, literals }) => {
            ExpectedKind::DtdItem(unit.raw().to_owned(), *literals, unit.raw_source())
        }
        ReaderEvent::Dtd(TakenDtdEvent::End) => ExpectedKind::DtdEnd,
        ReaderEvent::Xml(Event::Declaration(declaration)) => ExpectedKind::Declaration(
            declaration.version.to_owned(),
            declaration.encoding.map(str::to_owned),
            declaration.standalone,
        ),
        ReaderEvent::Xml(Event::Start(element)) => ExpectedKind::Start(
            element.name.qualified().to_owned(),
            element.name.namespace().map(str::to_owned),
            observed_attributes(&element.attributes),
        ),
        ReaderEvent::Xml(Event::Empty(element)) => ExpectedKind::Empty(
            element.name.qualified().to_owned(),
            element.name.namespace().map(str::to_owned),
            observed_attributes(&element.attributes),
        ),
        ReaderEvent::Xml(Event::End(name)) => ExpectedKind::End(
            name.qualified().to_owned(),
            name.namespace().map(str::to_owned),
        ),
        ReaderEvent::Xml(Event::Text(value)) => ExpectedKind::Text(value.to_string()),
        ReaderEvent::Xml(Event::Cdata(value)) => ExpectedKind::Cdata(value.to_string()),
        ReaderEvent::Xml(Event::Comment(value)) => ExpectedKind::Comment(value.to_string()),
        ReaderEvent::Xml(Event::ProcessingInstruction { target, data }) => {
            ExpectedKind::Pi(target.to_string(), data.to_string())
        }
    }
}

fn observed_attributes(attributes: &[Attribute<'_>]) -> Vec<ExpectedAttribute> {
    attributes
        .iter()
        .map(|attribute| ExpectedAttribute {
            name: attribute.name.qualified().to_owned(),
            namespace: attribute.name.namespace().map(str::to_owned),
            value: attribute.value.to_string(),
        })
        .collect()
}

fn observed_origin(origin: SourceOrigin) -> ExpectedOrigin {
    ExpectedOrigin {
        input: origin.input_range(),
        replacement: origin.replacement_range(),
        physical: origin.physical_context(),
    }
}

fn address_range(source: &str) -> Range<usize> {
    source.as_ptr() as usize..source.as_ptr() as usize + source.len()
}

#[expect(
    clippy::ptr_arg,
    reason = "the oracle must observe the Cow storage discriminant"
)]
fn check_cow_storage(
    value: &Cow<'_, str>,
    generated: bool,
    source: &str,
    occurrence: Option<&Range<usize>>,
) -> Result<(), &'static str> {
    match value {
        Cow::Borrowed(value) => {
            if generated {
                return Err("generated value borrowed physical input");
            }
            if !value.is_empty() {
                let base = source.as_ptr() as usize;
                let Some(offset) = (value.as_ptr() as usize).checked_sub(base) else {
                    return Err("borrowed value precedes the input");
                };
                let Some(end) = offset.checked_add(value.len()) else {
                    return Err("borrowed value range overflowed");
                };
                let occurrence = occurrence.ok_or("borrowed value lacks a physical occurrence")?;
                if offset < occurrence.start || end > occurrence.end {
                    return Err("borrowed value is outside its exact physical occurrence");
                }
            }
        }
        Cow::Owned(value) => {
            if !value.is_empty() && address_range(source).contains(&(value.as_ptr() as usize)) {
                return Err("owned value aliases the input");
            }
        }
    }
    Ok(())
}

fn check_name_storage(
    name: &Name<'_>,
    generated: bool,
    source: &str,
    occurrence: Option<&Range<usize>>,
) -> Result<(), &'static str> {
    check_cow_storage(name.qualified_source(), generated, source, occurrence)
}

fn check_event_storage(
    event: &ReaderEvent<'_>,
    origin: SourceOrigin,
    source: &str,
) -> Result<(), &'static str> {
    let input_range = origin.input_range();
    let generated = input_range.is_none();
    match event {
        ReaderEvent::Dtd(TakenDtdEvent::Begin(unit))
        | ReaderEvent::Dtd(TakenDtdEvent::Item { unit, .. }) => {
            let expected = if generated {
                RawSource::Owned
            } else {
                RawSource::Borrowed
            };
            check_raw_source(unit, expected, source, origin.input_range())?;
        }
        ReaderEvent::Dtd(TakenDtdEvent::End) => {}
        ReaderEvent::Xml(Event::Declaration(declaration)) => {
            check_cow_storage(
                &Cow::Borrowed(declaration.version),
                false,
                source,
                input_range.as_ref(),
            )?;
            if let Some(encoding) = declaration.encoding {
                check_cow_storage(
                    &Cow::Borrowed(encoding),
                    false,
                    source,
                    input_range.as_ref(),
                )?;
            }
        }
        ReaderEvent::Xml(Event::Start(element) | Event::Empty(element)) => {
            check_name_storage(&element.name, generated, source, input_range.as_ref())?;
            for attribute in &element.attributes {
                check_name_storage(&attribute.name, generated, source, input_range.as_ref())?;
                check_cow_storage(&attribute.value, generated, source, input_range.as_ref())?;
            }
        }
        ReaderEvent::Xml(Event::End(name)) => {
            check_name_storage(name, generated, source, input_range.as_ref())?;
        }
        ReaderEvent::Xml(Event::Text(value) | Event::Cdata(value) | Event::Comment(value)) => {
            check_cow_storage(value, generated, source, input_range.as_ref())?;
        }
        ReaderEvent::Xml(Event::ProcessingInstruction { target, data }) => {
            check_cow_storage(target, generated, source, input_range.as_ref())?;
            check_cow_storage(data, generated, source, input_range.as_ref())?;
        }
    }
    Ok(())
}

fn check_raw_source(
    unit: &DtdUnit<'_>,
    expected: RawSource,
    source: &str,
    input_range: Option<Range<usize>>,
) -> Result<(), &'static str> {
    if unit.raw_source() != expected {
        return Err("DTD raw storage class differs");
    }
    match expected {
        RawSource::Borrowed => {
            let range = input_range.ok_or("borrowed DTD raw lacks an input range")?;
            let base = source.as_ptr() as usize;
            let address = unit.raw().as_ptr() as usize;
            let Some(offset) = address.checked_sub(base) else {
                return Err("borrowed DTD raw precedes the input");
            };
            let Some(end) = offset.checked_add(unit.raw().len()) else {
                return Err("borrowed DTD raw range overflowed");
            };
            if offset != range.start || end > source.len() || unit.raw() != &source[offset..end] {
                return Err("borrowed DTD raw is not the exact source occurrence");
            }
        }
        RawSource::Owned => {
            if !unit.raw().is_empty()
                && address_range(source).contains(&(unit.raw().as_ptr() as usize))
            {
                return Err("owned DTD raw aliases the input");
            }
        }
    }
    Ok(())
}

fn check_snapshot(expected: FuzzSnapshot, actual: FuzzSnapshot) -> Result<(), &'static str> {
    if expected.phase != actual.phase {
        return Err("phase");
    }
    if expected.failure != actual.failure {
        return Err("failure");
    }
    if expected.physical_position != actual.physical_position {
        return Err("physical position");
    }
    if expected.last_origin != actual.last_origin {
        return Err("last origin");
    }
    if expected.finished != actual.finished {
        return Err("finished");
    }
    if expected.controller != actual.controller {
        return Err("controller counters");
    }
    if expected.frames != actual.frames {
        return Err("frame counters");
    }
    if expected.text != actual.text {
        return Err("text usage");
    }
    Ok(())
}

fn validate_origin(origin: SourceOrigin, source_len: usize) {
    let physical = origin.physical_context();
    assert!(physical.start <= physical.end && physical.end <= source_len);
    if let Some(input) = origin.input_range() {
        assert!(input.start <= input.end && input.end <= source_len);
        assert_eq!(input, physical);
        assert!(origin.replacement_range().is_none());
    } else {
        assert!(origin.replacement_range().is_some());
    }
}

fn assert_terminal_sticky(reader: &mut DtdReader<'_>, terminal: Result<(), SessionError>) {
    let snapshot = reader.fuzz_snapshot();
    for _ in 0..3 {
        match terminal {
            Ok(()) => assert!(reader.read_event().unwrap().is_none()),
            Err(expected) => assert_eq!(reader.read_event().unwrap_err(), expected),
        }
        check_snapshot(snapshot, reader.fuzz_snapshot()).unwrap();
    }
}

fn replay_arbitrary(input: &[u8]) {
    let Some(mut reader) = DtdReader::new(input, options()).ok() else {
        return;
    };
    loop {
        match reader.read_event() {
            Ok(Some((event, origin))) => {
                validate_origin(origin, input.len());
                assert_eq!(reader.event_origin(), Some(origin));
                assert_eq!(reader.event_range(), origin.input_range());
                assert!(reader.position() <= input.len());
                check_event_storage(&event, origin, std::str::from_utf8(input).unwrap()).unwrap();
            }
            Ok(None) => {
                assert_terminal_sticky(&mut reader, Ok(()));
                return;
            }
            Err(error) => {
                assert_eq!(reader.fuzz_snapshot().failure, Some(FuzzFailure::Error));
                assert_terminal_sticky(&mut reader, Err(error));
                return;
            }
        }
    }
}

fn run_generated(case: GeneratedCase) {
    let mut reader = DtdReader::new(case.source.as_bytes(), options()).unwrap();
    let mut held = Vec::new();
    for expected in &case.expected {
        let (event, origin) = reader.read_event().unwrap().expect("expected event");
        assert_eq!(observed_kind(&event), expected.kind);
        assert_eq!(observed_origin(origin), expected.origin);
        check_event_storage(&event, origin, &case.source).unwrap();
        held.push((event, origin));
    }
    assert!(reader.read_event().unwrap().is_none());
    assert_terminal_sticky(&mut reader, Ok(()));
    drop(reader);
    assert_eq!(held.len(), case.expected.len());
    for ((event, origin), expected) in held.iter().zip(&case.expected) {
        assert_eq!(observed_kind(event), expected.kind);
        assert_eq!(observed_origin(*origin), expected.origin);
        check_event_storage(event, *origin, &case.source).unwrap();
    }
}

/// Exercise the private reader with bounded direct and independently generated input.
pub(crate) fn exercise(input: &[u8]) {
    let input = &input[..input.len().min(MAX_INPUT)];
    replay_arbitrary(input);
    run_generated(generated_case(&input[..input.len().min(64)]));
}

fn read_bounded(mut input: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(MAX_INPUT + 1);
    input
        .by_ref()
        .take((MAX_INPUT + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_INPUT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "DTD fuzz input exceeds 1 MiB",
        ));
    }
    Ok(bytes)
}

#[test]
#[ignore = "exact bounded-stdin entry for the private feedback campaign"]
pub(crate) fn replay_stdin() {
    let stdin = io::stdin();
    let bytes = read_bounded(stdin.lock()).expect("read bounded fuzz input");
    exercise(&bytes);
}

const CONTROL_CORPUS: &[&[u8]] = &[
    b"",
    b"\xff",
    &[0, 4, 8, 0, 0, 0, 0, 0],
    &[1, 4, 8, 1, 1, 1, 1, 1],
    &[2, 4, 8, 2, 2, 2, 2, 2],
    &[0, 1, 8, 3, 3, 3, 3, 3],
];

const SEMANTIC_CORPUS: &[&[u8]] = &[
    b"<r/>",
    b"<!DOCTYPE r><r/>",
    b"<!DOCTYPE r [<!ENTITY e '<n/>x'><!ENTITY z ''>]><r>a&e;b&z;c</r>",
    b"<!DOCTYPE r [<!ENTITY % p \"<!ENTITY e 'x'>\">%p;]><r>&e;</r>",
    b"<!DOCTYPE r [<!ENTITY i 'x'><!ENTITY o '&i;&i;'>]><r>&o;</r>",
    b"<!DOCTYPE p:r [<!ATTLIST p:r xmlns:p CDATA 'urn:p' p:a CDATA 'v'>]><p:r/>",
    b"<?xml version='1.0' standalone='yes'?><!DOCTYPE r SYSTEM 'unread'><r/>",
    b"<!DOCTYPE r [<!--unterminated",
    b"<!DOCTYPE r [<!ENTITY e 'x'>]><r>&e",
];

#[test]
fn fixed_corpus_and_small_prefixes_are_bounded_and_deterministic() {
    for seed in CONTROL_CORPUS {
        exercise(seed);
    }
    for seed in SEMANTIC_CORPUS {
        exercise(seed);
        if seed.len() <= 96 {
            for end in 0..=seed.len() {
                exercise(&seed[..end]);
            }
        }
    }
}

#[test]
fn fixed_replacement_insertion_and_deletion_smoke_is_bounded() {
    for seed in SEMANTIC_CORPUS {
        let positions = seed.len().min(16);
        for position in 0..positions {
            for replacement in [0, b'<', b'&', 0xff] {
                let mut mutated = seed.to_vec();
                mutated[position] = replacement;
                exercise(&mutated);
            }

            let mut inserted = seed.to_vec();
            inserted.insert(position, b'&');
            exercise(&inserted);

            let mut deleted = seed.to_vec();
            deleted.remove(position);
            exercise(&deleted);
        }
    }
}

#[test]
fn generator_respects_all_frozen_caps() {
    for seed in CONTROL_CORPUS {
        let case = generated_case(seed);
        assert!(case.source.len() <= MAX_GENERATED);
        assert!(case.expected.len() <= MAX_EVENTS);
        assert!(case.source.matches("<!ENTITY ").count() <= MAX_DECLARATIONS);
    }
}

#[test]
fn nested_generator_uses_the_first_entity_name_and_exact_local_ranges() {
    let mut controls = [0_u8; 64];
    controls[1] = 2;
    controls[2] = 1;
    controls[3] = 3;
    controls[8] = 0;
    controls[9] = 1;
    controls[16] = 1;
    controls[32] = 1;
    let case = generated_case(&controls);
    assert!(case.source.contains("<!ENTITY e1b '&e0a;B'>"));
    run_generated(case);
}

#[test]
fn bounded_reader_accepts_exact_limit_rejects_extra_and_handles_short_reads() {
    assert_eq!(
        read_bounded(&vec![b'x'; MAX_INPUT][..]).unwrap().len(),
        MAX_INPUT
    );
    assert_eq!(
        read_bounded(&vec![b'x'; MAX_INPUT + 1][..])
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );

    struct Chunked<'a> {
        source: &'a [u8],
        position: usize,
        chunk: usize,
    }
    impl Read for Chunked<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if self.position == self.source.len() {
                return Ok(0);
            }
            let end = (self.position + self.chunk)
                .min(self.source.len())
                .min(self.position + output.len());
            let length = end - self.position;
            output[..length].copy_from_slice(&self.source[self.position..end]);
            self.position = end;
            Ok(length)
        }
    }
    let source = b"<!DOCTYPE r><r/>";
    let actual = read_bounded(Chunked {
        source,
        position: 0,
        chunk: 2,
    })
    .unwrap();
    assert_eq!(actual, source);
}

fn run_exact_replay(chunks: &[&[u8]]) -> std::process::Output {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "reader::dtd_fuzz_tests::replay_stdin",
            "--ignored",
            "--test-threads=1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for chunk in chunks {
        stdin.write_all(chunk).unwrap();
        stdin.flush().unwrap();
    }
    drop(stdin);
    child.wait_with_output().unwrap()
}

#[test]
fn exact_stdin_entry_enforces_bound_and_accepts_split_writes() {
    let exact = vec![b'x'; MAX_INPUT];
    let output = run_exact_replay(&[&exact]);
    assert!(
        output.status.success(),
        "exact-bound replay failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let extra = vec![b'x'; MAX_INPUT + 1];
    let output = run_exact_replay(&[&extra]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("exceeds 1 MiB")
            || String::from_utf8_lossy(&output.stderr).contains("exceeds 1 MiB")
    );

    let output = run_exact_replay(&[
        b"<!DOC".as_slice(),
        b"TYPE r [<!ENTITY e 'x'>]>".as_slice(),
        b"<r>&e;</r>".as_slice(),
    ]);
    assert!(
        output.status.success(),
        "split replay failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn snapshot_oracle_observes_every_field() {
    let baseline = FuzzSnapshot {
        phase: FuzzPhase::Content,
        failure: Some(FuzzFailure::Error),
        physical_position: 11,
        last_origin: Some(SourceOrigin::input(3..5)),
        finished: true,
        controller: Some(ControllerCounters {
            work: 1,
            scanned_bytes: 2,
            owned_bytes: 3,
            physical_bytes: 4,
            reference_attempts: 5,
        }),
        frames: Some(FrameCounters {
            reference_attempts: 6,
            work: 7,
            expanded_bytes: 8,
        }),
        text: Some(TextUsage {
            work: 9,
            source_scanned: 10,
            reference_scanned: 11,
            reference_copied: 12,
            reference_classified: 13,
            appended_reference_copied: 14,
            validated: 15,
        }),
    };
    let variants = vec![
        FuzzSnapshot {
            phase: FuzzPhase::Ordinary,
            ..baseline
        },
        FuzzSnapshot {
            failure: None,
            ..baseline
        },
        FuzzSnapshot {
            physical_position: 12,
            ..baseline
        },
        FuzzSnapshot {
            last_origin: None,
            ..baseline
        },
        FuzzSnapshot {
            finished: false,
            ..baseline
        },
        FuzzSnapshot {
            controller: None,
            ..baseline
        },
        FuzzSnapshot {
            frames: None,
            ..baseline
        },
        FuzzSnapshot {
            text: None,
            ..baseline
        },
    ];
    for variant in variants {
        assert!(check_snapshot(baseline, variant).is_err());
    }
    assert_eq!(check_snapshot(baseline, baseline), Ok(()));
}

#[test]
fn raw_source_oracle_rejects_wrong_storage_class_and_occurrence() {
    let source = String::from("<!ENTITY e 'x'><!ENTITY e 'x'>");
    let first = 0..15;
    let borrowed =
        DtdUnit::parse(&source[first.clone()], crate::dtd::DtdLimits::default()).unwrap();
    assert_eq!(
        check_raw_source(&borrowed, RawSource::Borrowed, &source, Some(first.clone())),
        Ok(())
    );
    assert!(check_raw_source(&borrowed, RawSource::Owned, &source, None).is_err());
    assert!(check_raw_source(&borrowed, RawSource::Borrowed, &source, Some(15..30)).is_err());

    let owned = DtdUnit::parse(
        String::from("<!ENTITY e 'x'>"),
        crate::dtd::DtdLimits::default(),
    )
    .unwrap();
    assert_eq!(
        check_raw_source(&owned, RawSource::Owned, &source, None),
        Ok(())
    );
    assert!(check_raw_source(&owned, RawSource::Borrowed, &source, Some(first)).is_err());
}

#[test]
fn parameter_generated_declaration_has_independent_transcript_and_owned_raw() {
    let source = "<!DOCTYPE r [<!ENTITY % p \"<!ENTITY e 'x'>\">%p;]><r>&e;</r>";
    run_generated(GeneratedCase {
        source: source.to_owned(),
        expected: vec![
            physical(
                ExpectedKind::DtdBegin("<!DOCTYPE r [".to_owned(), RawSource::Borrowed),
                0..13,
            ),
            physical(
                ExpectedKind::DtdItem(
                    "<!ENTITY % p \"<!ENTITY e 'x'>\">".to_owned(),
                    LiteralSource::Physical,
                    RawSource::Borrowed,
                ),
                13..44,
            ),
            replacement(
                ExpectedKind::DtdItem(
                    "<!ENTITY e 'x'>".to_owned(),
                    LiteralSource::Replacement,
                    RawSource::Owned,
                ),
                0..15,
                44..47,
            ),
            physical(ExpectedKind::DtdEnd, 47..49),
            physical(ExpectedKind::Start("r".to_owned(), None, vec![]), 49..52),
            replacement(ExpectedKind::Text("x".to_owned()), 0..1, 52..55),
            physical(ExpectedKind::End("r".to_owned(), None), 55..59),
        ],
    });
}

#[test]
fn namespace_defaults_have_ordered_transcript_and_owned_generated_fields() {
    let source = "<!DOCTYPE p:r [<!ATTLIST p:r xmlns:p CDATA 'urn:p' p:a CDATA 'v'>]><p:r/>";
    run_generated(GeneratedCase {
        source: source.to_owned(),
        expected: vec![
            physical(
                ExpectedKind::DtdBegin("<!DOCTYPE p:r [".to_owned(), RawSource::Borrowed),
                0..15,
            ),
            physical(
                ExpectedKind::DtdItem(
                    "<!ATTLIST p:r xmlns:p CDATA 'urn:p' p:a CDATA 'v'>".to_owned(),
                    LiteralSource::Physical,
                    RawSource::Borrowed,
                ),
                15..65,
            ),
            physical(ExpectedKind::DtdEnd, 65..67),
            physical(
                ExpectedKind::Empty(
                    "p:r".to_owned(),
                    Some("urn:p".to_owned()),
                    vec![
                        ExpectedAttribute {
                            name: "xmlns:p".to_owned(),
                            namespace: Some("http://www.w3.org/2000/xmlns/".to_owned()),
                            value: "urn:p".to_owned(),
                        },
                        ExpectedAttribute {
                            name: "p:a".to_owned(),
                            namespace: Some("urn:p".to_owned()),
                            value: "v".to_owned(),
                        },
                    ],
                ),
                67..73,
            ),
        ],
    });

    let mut reader = DtdReader::new(source.as_bytes(), options()).unwrap();
    let element = loop {
        let (event, _) = reader.read_event().unwrap().unwrap();
        if let ReaderEvent::Xml(Event::Empty(element)) = event {
            break element;
        }
    };
    assert!(matches!(
        element.name.qualified_source(),
        Cow::Borrowed("p:r")
    ));
    assert_eq!(element.attributes.len(), 2);
    for attribute in element.attributes {
        assert!(matches!(attribute.name.qualified_source(), Cow::Owned(_)));
        assert!(matches!(attribute.value, Cow::Owned(_)));
    }
}

#[test]
fn repeated_nested_occurrences_are_owned_and_have_distinct_complete_origins() {
    let source = "<!DOCTYPE r [<!ENTITY i 'x'><!ENTITY o '&i;&i;'>]><r>&o;</r>";
    let mut reader = DtdReader::new(source.as_bytes(), options()).unwrap();
    let mut values = Vec::new();
    while let Some((event, origin)) = reader.read_event().unwrap() {
        if let ReaderEvent::Xml(Event::Text(value)) = event {
            assert!(matches!(value, Cow::Owned(ref text) if text == "x"));
            values.push((value, origin));
        }
    }
    assert_eq!(values.len(), 2);
    assert_eq!(values[0].1.input_range(), None);
    assert_eq!(values[0].1.replacement_range(), Some(0..1));
    assert_eq!(
        values[0].1.physical_context(),
        values[1].1.physical_context()
    );
    assert_ne!(values[0].1, values[1].1);
    drop(reader);
    assert_eq!(values[0].0, "x");
    assert_eq!(values[1].0, "x");
}

#[test]
fn extracted_generated_values_survive_events_reader_and_source_drop() {
    let retained = {
        let source = String::from(
            "<!DOCTYPE r [<!ENTITY e '<p:n a=\"v\">x</p:n>'>]><r xmlns:p='urn:p'>&e;</r>",
        );
        let mut reader = DtdReader::new(source.as_bytes(), options()).unwrap();
        let mut retained = Vec::new();
        while let Some((event, origin)) = reader.read_event().unwrap() {
            if origin.input_range().is_some() {
                continue;
            }
            match event {
                ReaderEvent::Xml(Event::Start(element)) => {
                    for attribute in element.attributes {
                        let Cow::Owned(value) = attribute.value else {
                            panic!("generated attribute value must own storage");
                        };
                        retained.push(value);
                    }
                }
                ReaderEvent::Xml(Event::Text(Cow::Owned(value))) => retained.push(value),
                ReaderEvent::Xml(Event::End(_)) => {}
                other => panic!("unexpected generated event: {other:?}"),
            }
        }
        retained
    };
    assert_eq!(retained, ["v", "x"]);
}

#[test]
fn snapshot_tracks_phase_handoff_origin_terminal_state_and_all_counters() {
    let source = "<!DOCTYPE r [<!ENTITY e 'x'>]><r>&e;</r>";
    let mut reader = DtdReader::new(source.as_bytes(), options()).unwrap();
    assert_eq!(
        reader.fuzz_snapshot(),
        FuzzSnapshot {
            phase: FuzzPhase::Ordinary,
            failure: None,
            physical_position: 0,
            last_origin: None,
            finished: false,
            controller: None,
            frames: None,
            text: None,
        }
    );

    let (_, begin) = reader.read_event().unwrap().unwrap();
    assert_eq!(
        reader.fuzz_snapshot(),
        FuzzSnapshot {
            phase: FuzzPhase::Declarations,
            failure: None,
            physical_position: 13,
            last_origin: Some(begin),
            finished: false,
            controller: Some(ControllerCounters {
                work: 39,
                scanned_bytes: 13,
                owned_bytes: 0,
                physical_bytes: 13,
                reference_attempts: 0,
            }),
            frames: None,
            text: None,
        }
    );

    reader.read_event().unwrap().unwrap();
    reader.read_event().unwrap().unwrap();
    let (_, root) = reader.read_event().unwrap().unwrap();
    assert_eq!(
        reader.fuzz_snapshot(),
        FuzzSnapshot {
            phase: FuzzPhase::Content,
            failure: None,
            physical_position: 33,
            last_origin: Some(root),
            finished: false,
            controller: Some(ControllerCounters {
                work: 96,
                scanned_bytes: 33,
                owned_bytes: 0,
                physical_bytes: 33,
                reference_attempts: 0,
            }),
            frames: Some(FrameCounters::default()),
            text: Some(TextUsage {
                work: 96,
                source_scanned: 3,
                reference_scanned: 0,
                reference_copied: 0,
                reference_classified: 0,
                appended_reference_copied: 0,
                validated: 3,
            }),
        }
    );

    let (_, generated) = reader.read_event().unwrap().unwrap();
    assert_eq!(
        reader.fuzz_snapshot(),
        FuzzSnapshot {
            phase: FuzzPhase::Content,
            failure: None,
            physical_position: 36,
            last_origin: Some(generated),
            finished: false,
            controller: Some(ControllerCounters {
                work: 110,
                scanned_bytes: 37,
                owned_bytes: 0,
                physical_bytes: 36,
                reference_attempts: 1,
            }),
            frames: Some(FrameCounters {
                reference_attempts: 1,
                work: 2,
                expanded_bytes: 1,
            }),
            text: Some(TextUsage {
                work: 110,
                source_scanned: 4,
                reference_scanned: 3,
                reference_copied: 3,
                reference_classified: 3,
                appended_reference_copied: 0,
                validated: 4,
            }),
        }
    );

    let (_, end) = reader.read_event().unwrap().unwrap();
    assert!(reader.read_event().unwrap().is_none());
    assert_eq!(
        reader.fuzz_snapshot(),
        FuzzSnapshot {
            phase: FuzzPhase::Content,
            failure: None,
            physical_position: 40,
            last_origin: Some(end),
            finished: true,
            controller: Some(ControllerCounters {
                work: 122,
                scanned_bytes: 41,
                owned_bytes: 0,
                physical_bytes: 40,
                reference_attempts: 1,
            }),
            frames: Some(FrameCounters {
                reference_attempts: 1,
                work: 2,
                expanded_bytes: 1,
            }),
            text: Some(TextUsage {
                work: 122,
                source_scanned: 8,
                reference_scanned: 3,
                reference_copied: 3,
                reference_classified: 3,
                appended_reference_copied: 0,
                validated: 8,
            }),
        }
    );
}
