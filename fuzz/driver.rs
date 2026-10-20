//! Shared, safe fuzz entry point. Panic on an invariant violation so external
//! runners preserve the failing input.

use std::collections::HashSet;
use std::ops::{ControlFlow, Range};
use yaxml::{Document, Error, ErrorKind, Event, Limits, NodeKind, PushParser, ReadError, Reader};

/// Events are bounded by input plus the permitted entity expansion: at most
/// one per two source or expanded bytes, plus EOF.
pub fn event_bound(input: usize) -> usize {
    expanded_bound(input).saturating_mul(2).saturating_add(1)
}

/// Input bytes plus the largest default entity-expansion allowance.
pub fn expanded_bound(input: usize) -> usize {
    let limits = Limits::default();
    input.saturating_add(
        limits
            .max_entity_expansion_bytes
            .max(input.saturating_mul(limits.max_entity_amplification)),
    )
}

/// Exact input span, or the outermost entity reference that produced the value.
pub fn span(origin: yaxml::SourceOrigin) -> Range<usize> {
    origin
        .input_range()
        .unwrap_or_else(|| origin.physical_context())
}

#[path = "writer.rs"]
mod writer;

#[path = "owned.rs"]
mod owned;

#[path = "encoding.rs"]
mod encoding;

#[path = "pull.rs"]
mod pull;

#[path = "serialize.rs"]
mod serialize;

#[path = "save.rs"]
mod save;

#[path = "domedit.rs"]
mod domedit;

#[path = "mutation.rs"]
mod mutation;

#[path = "construction.rs"]
mod construction;

#[path = "names.rs"]
mod names;

#[path = "datatypes.rs"]
mod datatypes;
#[path = "pattern.rs"]
mod pattern;
#[path = "regexp.rs"]
mod regexp;
#[path = "uri.rs"]
mod uri;
#[path = "xpath.rs"]
mod xpath;

#[path = "c14n.rs"]
mod c14n;

#[path = "dtd.rs"]
mod dtd;
#[path = "html.rs"]
mod html;
#[path = "valid.rs"]
mod valid;

#[path = "resolver.rs"]
mod resolver;

#[path = "textreader.rs"]
mod textreader;

#[path = "xinclude.rs"]
mod xinclude;

#[path = "catalog.rs"]
mod catalog;

#[path = "options.rs"]
mod options;
#[path = "relaxng.rs"]
mod relaxng;
#[path = "schematron.rs"]
mod schematron;
#[path = "xsd.rs"]
mod xsd;

/// Maximum testcase size accepted by the process driver.
pub const MAX_INPUT_BYTES: usize = 1 << 20;

/// Acceptance and event count for differential and regression reporting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Outcome {
    pub error: Option<Error>,
    pub events: usize,
}

/// Exercise every input surface. Each family is also a separate
/// coverage-guided target in `fuzz_targets/`.
pub fn exercise(input: &[u8]) -> Outcome {
    assert!(
        input.len() <= MAX_INPUT_BYTES,
        "fuzz testcase exceeds the driver budget"
    );
    exercise_names(input);
    exercise_uri(input);
    exercise_regexp(input);
    exercise_datatypes(input);
    exercise_html(input);
    exercise_xpath(input);
    exercise_pattern(input);
    exercise_xinclude(input);
    exercise_catalog(input);
    exercise_xsd(input);
    exercise_relaxng(input);
    exercise_schematron(input);
    exercise_c14n(input);
    exercise_valid(input);
    exercise_textreader(input);
    exercise_construction(input);
    exercise_mutation(input);
    exercise_domedit(input);
    exercise_pull(input, input.get(input.len() / 2).copied().unwrap_or(0));
    exercise_push(input);
    if let Some(outcome) = exercise_encoding(input) {
        return outcome;
    }
    exercise_serialize(input);
    exercise_save(input);
    exercise_owned(input);
    exercise_writer(input);
    exercise_limits(input);
    exercise_dom(input)
}

/// DOCTYPE-bearing input through every parser surface. Inputs that do not
/// start a DOCTYPE become an internal-subset prefix, entity replacement text
/// or attribute default, so mutations reach declaration and expansion paths.
#[allow(dead_code, reason = "used by the dtd target and fuzz smoke tests only")]
pub fn exercise_dtd(input: &[u8]) -> Outcome {
    if input.starts_with(b"<!DOCTYPE") {
        return exercise_doctype(input);
    }
    let (control, body) = input.split_first().unwrap_or((&0, &[]));
    if control % 4 == 3 {
        return exercise_resolver(*control, body);
    }
    let mut document = Vec::with_capacity(body.len() + 64);
    match control % 4 {
        0 => {
            document.extend_from_slice(b"<!DOCTYPE r [");
            document.extend_from_slice(body);
            document.extend_from_slice(b"]><r>&e;</r>");
        }
        1 => {
            document.extend_from_slice(b"<!DOCTYPE r [<!ENTITY e '");
            document.extend_from_slice(body);
            document.extend_from_slice(b"'>]><r a='&e;'>&e;&e;</r>");
        }
        _ => {
            document.extend_from_slice(b"<!DOCTYPE r SYSTEM 'x' [<!ENTITY % p '");
            document.extend_from_slice(body);
            document.extend_from_slice(b"'>%p;<!ATTLIST r a CDATA 'd'>]><r>&e;</r>");
        }
    }
    document.truncate(MAX_INPUT_BYTES);
    // A control byte with its high bit set also selects parser options.
    if control & 0x80 != 0 {
        let (flags, _) = options::split(&[*control]);
        options::exercise_flags(flags, &document);
    }
    exercise_doctype(&document)
}

/// The DTD-relevant subset of `exercise`; the full family is far slower.
/// A hash of the input also selects a small `Limits` profile and a push
/// cancellation point, so budget and abort paths inside DTDs are reached.
fn exercise_doctype(input: &[u8]) -> Outcome {
    exercise_valid(input);
    dtd::exercise(input);
    owned(input);
    exercise_serialize(input);
    exercise_push(input);
    let seed = resolver::hash(input);
    // Profile 0 is close to the defaults every other surface already ran.
    let limits = resolver::profile((seed >> 40) as u8, seed, input.len());
    if (seed >> 40) & 3 != 0 {
        exercise_with_limits(input, limits);
    }
    resolver::cancel(
        input,
        limits,
        (seed >> 44) as usize % 24,
        1 + (seed >> 50) as usize % 13,
    );
    dom(input)
}

/// Control bits 2-3 pick a template, 4-5 the resolver policy and 6-7 a
/// `Limits` profile. The remaining NUL-separated pieces are the resources
/// served as `s` (external subset), `e`, `p` and `0`..`9`. Templates:
/// 0 wraps the first piece as an internal subset that references them;
/// 1 is the same in a standalone document with attributes and whitespace;
/// 2 takes the first piece as the whole document; 3 loads `s` with
/// `valid::Dtd::load` and validates the first piece against it.
pub fn exercise_resolver(control: u8, input: &[u8]) -> Outcome {
    let mut pieces = input.split(|byte| *byte == 0);
    let first = pieces.next().unwrap_or_default();
    let resources: Vec<Vec<u8>> = pieces.map(<[u8]>::to_vec).collect();
    // The slice reader validates a whole document's characters first, so
    // other surfaces could fail later; resources share one decoder and stay raw.
    let first: String = String::from_utf8_lossy(first)
        .chars()
        .filter(|c| {
            (!c.is_control() || matches!(c, '\t' | '\n' | '\r'))
                && !matches!(c, '\u{fffe}' | '\u{ffff}')
        })
        .collect();
    let policy = resolver::Policy::from_bits(control >> 4);
    let seed = resolver::hash(input);
    let document = match control >> 2 & 3 {
        0 => [
            "<!DOCTYPE r SYSTEM 's' [<!ENTITY e SYSTEM 'e'><!ENTITY % p SYSTEM 'p'>",
            &first,
            "%p;]><r>&e;</r>",
        ]
        .concat(),
        1 => [
            "<?xml version='1.0' standalone='yes'?><!DOCTYPE r SYSTEM 's' [",
            &first,
            "]><r a=' x  y ' b='z'>\n <c/> </r>",
        ]
        .concat(),
        2 => first,
        _ => {
            let limits = resolver::profile(control >> 6, seed, first.len());
            return Outcome {
                error: resolver::exercise_load(first.as_bytes(), &resources, limits, policy),
                events: 0,
            };
        }
    };
    // Re-encoded resources also re-encode a whole document.
    let document = match (control >> 2 & 3, policy) {
        (2, resolver::Policy::Utf16) => resolver::utf16(document.as_bytes()),
        _ => document.into_bytes(),
    };
    let limits = resolver::profile(control >> 6, seed, document.len());
    Outcome {
        error: resolver::exercise(&document, &resources, limits, policy),
        events: 0,
    }
}

/// URI parse, serialization, escaping and resolution invariants.
pub fn exercise_uri(input: &[u8]) {
    uri::exercise(input);
}

/// XML Schema regular expression compilation and matching invariants.
pub fn exercise_regexp(input: &[u8]) {
    regexp::exercise(input);
}

/// XML Schema datatype parsing, comparison and facet invariants.
pub fn exercise_datatypes(input: &[u8]) {
    datatypes::exercise(input);
}

/// HTML parsing, push chunk independence and serialization.
pub fn exercise_html(input: &[u8]) {
    html::exercise(input);
}

/// Borrowed parse of UTF-8 input; recognized encodings have their own oracle.
fn parse_utf8(input: &[u8]) -> Option<Result<Document<'_>, Error>> {
    (!encoding::recognizes(input)).then(|| Document::parse_bytes(input))
}

/// Reader/DOM agreement, tree structure and stream projection, then the
/// selected parser options.
pub fn exercise_dom(input: &[u8]) -> Outcome {
    options::exercise(input);
    dom(input)
}

fn dom(input: &[u8]) -> Outcome {
    let mut stream = Vec::new();
    let outcome = read(input, None, |event| stream.push(event));
    let document = Document::parse_bytes(input);
    assert_eq!(
        outcome.error.is_none(),
        document.is_ok(),
        "reader/DOM acceptance mismatch for {input:?}"
    );
    match &document {
        Ok(document) => {
            check_document(document, input.len(), outcome.events);
            check_projection(document, &stream);
            let lines = yaxml::LineIndex::new(input);
            let last = lines.line(input.len());
            let numbered = document
                .root()
                .descendants()
                .filter_map(|node| node.line(&lines));
            assert!(numbered.into_iter().all(|line| (1..=last).contains(&line)));
        }
        Err(error) => check_error(*error, input),
    }
    if let Ok(text) = std::str::from_utf8(input) {
        assert_eq!(document.is_ok(), Document::parse(text).is_ok());
    }
    outcome
}

/// Owned parsing compared with the borrowing DOM, then the selected options.
pub fn exercise_owned(input: &[u8]) {
    options::exercise(input);
    owned(input);
}

fn owned(input: &[u8]) {
    if let Some(document) = parse_utf8(input) {
        owned::exercise(input, Limits::default(), document.as_ref().map_err(|e| *e));
    }
}

/// Complete-document serialization round trip.
pub fn exercise_serialize(input: &[u8]) {
    if let Some(Ok(document)) = parse_utf8(input) {
        serialize::borrowed(&document, input);
    }
}

/// `save::to_vec` options, encodings and subtrees: DOM agreement and
/// fixed-point output.
pub fn exercise_save(input: &[u8]) {
    save::exercise(input);
}

/// Push, chunked and `read_from` parity with the slice reader.
pub fn exercise_push(input: &[u8]) {
    check_finish_abort(input);
    if let Some(document) = parse_utf8(input) {
        check_streaming(input, document.err());
    }
}

/// Event round trips plus caller-generated writer operations and sinks.
pub fn exercise_writer(input: &[u8]) {
    writer::exercise(input, Document::parse_bytes(input).is_ok());
}

/// Input-derived resource ceilings across every parsing API.
pub fn exercise_limits(input: &[u8]) -> Outcome {
    let byte = |index| input.get(index).copied().unwrap_or(0) as usize;
    exercise_with_limits(
        input,
        Limits {
            max_input_bytes: if byte(5) & 1 == 0 {
                MAX_INPUT_BYTES
            } else {
                input.len().saturating_sub(1)
            },
            max_token_bytes: byte(0),
            max_depth: byte(1) & 15,
            max_attributes: byte(2) & 15,
            max_events: byte(3),
            max_owned_bytes: byte(4),
            max_entity_expansion_bytes: byte(6),
            ..Limits::default()
        },
    )
}

/// UTF-16/UTF-32 detection and decoding, returning the outcome for recognized
/// encodings; other input exercises derived decoded ceilings instead.
pub fn exercise_encoding(input: &[u8]) -> Option<Outcome> {
    if encoding::recognizes(input) {
        return Some(encoding::exercise(input));
    }
    let byte = |index| input.get(index).copied().unwrap_or(0) as usize;
    encoding::exercise_decode_limits(
        input,
        yaxml::DecodeLimits {
            max_decoded_bytes: byte(6),
            max_decoded_token_bytes: byte(7),
        },
    );
    None
}

/// A pending text token is emitted only when final EOF is declared. Model that
/// boundary separately so callback cancellation cannot be mistaken for the
/// incomplete document's later `UnexpectedEof` failure.
fn check_finish_abort(input: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    if input.first().is_none_or(|byte| byte & 7 != b'<' & 7) {
        return;
    }

    let mut source = Vec::with_capacity(4 + input.len().min(16));
    source.extend_from_slice(b"<r>");
    source.extend(
        input
            .iter()
            .take(16)
            .map(|byte| HEX[usize::from(byte & 15)]),
    );
    let text = std::str::from_utf8(&source[3..]).unwrap();
    let mut parser = PushParser::new();
    let mut starts = 0;
    parser
        .feed(&source, |event, origin| {
            let range = span(origin);
            assert!(matches!(event, Event::Start(_)), "unexpected pre-EOF event");
            assert_eq!(range, 0..3);
            starts += 1;
            ControlFlow::Continue(())
        })
        .unwrap();
    assert_eq!(starts, 1, "missing generated start event");

    let mut texts = 0;
    let error = parser
        .finish(|event, origin| {
            let range = span(origin);
            let Event::Text(actual) = event else {
                panic!("unexpected finish event")
            };
            assert_eq!(actual, text);
            assert_eq!(range, 3..source.len());
            texts += 1;
            ControlFlow::Break(())
        })
        .expect_err("finish cancellation was ignored");
    assert_eq!(texts, 1, "finish delivered text more than once");
    check_abort(error, source.len());
    assert_eq!(parser.position(), source.len());
    assert_eq!(
        parser.feed(&[], |_, _| panic!("callback after cancellation")),
        Err(error)
    );
    assert_eq!(
        parser.finish(|_, _| panic!("callback after cancellation")),
        Err(error)
    );
}

fn check_abort(error: Error, expected_offset: usize) {
    assert_eq!(error.kind, ErrorKind::Aborted, "wrong cancellation kind");
    assert_eq!(
        error.offset, expected_offset,
        "cancellation offset did not end the delivered event"
    );
}

/// Exercise random structural, attribute and namespace edit scripts over a
/// parsed document; see `domedit.rs` for the testcase format.
pub fn exercise_domedit(input: &[u8]) {
    assert!(input.len() <= MAX_INPUT_BYTES);
    domedit::exercise(input);
}

/// Exercise caller-created edits independently from the XML grammar bytes.
pub fn exercise_mutation(input: &[u8]) {
    assert!(input.len() <= MAX_INPUT_BYTES);
    mutation::exercise(input);
}

/// Exercise XPath compilation and evaluation over both DOMs; see `xpath.rs`
/// for the testcase format.
pub fn exercise_xpath(input: &[u8]) {
    assert!(input.len() <= MAX_INPUT_BYTES);
    xpath::exercise(input);
}

/// Exercise pattern compilation, DOM matching and streaming; see `pattern.rs`
/// for the testcase format.
/// XML Schema construction and validation; see `xsd.rs` for the format.
pub fn exercise_xsd(input: &[u8]) {
    assert!(input.len() <= MAX_INPUT_BYTES);
    xsd::exercise(input);
}

pub fn exercise_pattern(input: &[u8]) {
    assert!(input.len() <= MAX_INPUT_BYTES);
    pattern::exercise(input);
}

/// Exercise XInclude over fuzz-provided resources; see `xinclude.rs` for the
/// testcase format.
pub fn exercise_xinclude(input: &[u8]) {
    assert!(input.len() <= MAX_INPUT_BYTES);
    xinclude::exercise(input);
}

/// Exercise XML and SGML catalogs over fuzz-provided files; see `catalog.rs`
/// for the testcase format.
pub fn exercise_catalog(input: &[u8]) {
    assert!(input.len() <= MAX_INPUT_BYTES);
    catalog::exercise(input);
}

/// Exercise RELAX NG schema building and validation over both DOMs; see
/// `relaxng.rs` for the testcase format.
pub fn exercise_relaxng(input: &[u8]) {
    assert!(input.len() <= MAX_INPUT_BYTES);
    relaxng::exercise(input);
}

/// Exercise Schematron schema building and validation in every output mode
/// over both DOMs; see `schematron.rs` for the testcase format.
pub fn exercise_schematron(input: &[u8]) {
    assert!(input.len() <= MAX_INPUT_BYTES);
    schematron::exercise(input);
}

/// Exercise canonicalization over both DOMs; see `c14n.rs` for the testcase format.
pub fn exercise_c14n(input: &[u8]) {
    assert!(input.len() <= MAX_INPUT_BYTES);
    c14n::exercise(input);
}

/// Exercise DTD validation over both DOMs; see `valid.rs` for the testcase format.
pub fn exercise_valid(input: &[u8]) {
    assert!(input.len() <= MAX_INPUT_BYTES);
    valid::exercise(input);
}

/// Exercise the libxml2-style node cursor; see `textreader.rs` for the testcase format.
pub fn exercise_textreader(input: &[u8]) {
    assert!(input.len() <= MAX_INPUT_BYTES);
    textreader::exercise(input);
}

/// Exercise validated names and owned PI targets before XML/encoding dispatch.
pub fn exercise_names(input: &[u8]) {
    assert!(input.len() <= MAX_INPUT_BYTES);
    #[cfg(test)]
    dispatch_tests::record_name_call();
    names::exercise(input);
}

/// Exercise rooted creation and fresh-element insertion independently from
/// whether the input bytes form XML.
pub fn exercise_construction(input: &[u8]) {
    assert!(input.len() <= MAX_INPUT_BYTES);
    #[cfg(test)]
    dispatch_tests::record_construction_call();
    construction::exercise(input);
}

#[cfg(test)]
mod dispatch_tests {
    use super::exercise;
    use std::cell::Cell;

    thread_local! {
        static CONSTRUCTION_CALLS: Cell<usize> = const { Cell::new(0) };
        static NAME_CALLS: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn record_name_call() {
        NAME_CALLS.with(|calls| calls.set(calls.get() + 1));
    }

    pub(super) fn record_construction_call() {
        CONSTRUCTION_CALLS.with(|calls| calls.set(calls.get() + 1));
    }

    #[test]
    fn construction_runs_before_grammar_and_encoding_dispatch_for_every_input() {
        for input in [
            b"".as_slice(),
            b"\x00",
            b"\x2a\x00\x00\x00",
            b"\x2b\x00\x00\x00",
            b"\xff\xfe<\x00r\x00/\x00>\x00",
            b"<r/>",
        ] {
            let before = CONSTRUCTION_CALLS.with(Cell::get);
            exercise(input);
            assert_eq!(
                CONSTRUCTION_CALLS.with(Cell::get),
                before + 1,
                "construction was skipped for {input:?}"
            );
        }
    }

    #[test]
    fn names_run_before_grammar_and_encoding_dispatch_for_every_input() {
        for input in [
            b"".as_slice(),
            b"\x00",
            b"\xff\xfe<\x00r\x00/\x00>\x00",
            "π:δ".as_bytes(),
            b"<r/>",
        ] {
            let before = NAME_CALLS.with(Cell::get);
            exercise(input);
            assert_eq!(
                NAME_CALLS.with(Cell::get),
                before + 1,
                "name construction was skipped for {input:?}"
            );
        }
    }
}

/// Exercise pull scheduling independently from the XML grammar bytes.
pub fn exercise_pull(input: &[u8], control: u8) {
    assert!(input.len() <= MAX_INPUT_BYTES);
    pull::exercise(input, control);
}

/// Exercise the explicit policy boundary with the same budget in both APIs.
pub fn exercise_with_limits(input: &[u8], limits: Limits) -> Outcome {
    let _ = pull::with_limits(input, limits, yaxml::DecodeLimits::default());
    if encoding::recognizes(input) {
        return encoding::exercise_with_limits(input, limits);
    }
    let mut stream = Vec::new();
    let outcome = read(input, Some(limits), |event| stream.push(event));
    let document = Document::with_limits(input, limits);
    // Joining entity-split text runs into one DOM node needs owned storage
    // that the borrowing reader never spends.
    if outcome.error.is_none()
        && matches!(&document, Err(error) if error.kind == ErrorKind::LimitExceeded(yaxml::Limit::OwnedBytes))
    {
        return outcome;
    }
    assert_eq!(
        outcome.error.is_none(),
        document.is_ok(),
        "reader/DOM policy acceptance mismatch for {input:?}"
    );
    owned::exercise(input, limits, document.as_ref().map_err(|error| *error));
    match document {
        Ok(document) => {
            check_document(&document, input.len(), outcome.events);
            check_projection(&document, &stream);
        }
        Err(error) => check_error(error, input),
    }
    let incremental = drive_push(input, &[input.len()], limits, |_, _| {});
    if !matches!(&incremental, Err(error) if error.kind == ErrorKind::LimitExceeded(yaxml::Limit::OwnedBytes))
    {
        assert_eq!(
            incremental.is_ok(),
            outcome.error.is_none(),
            "stream/slice policy acceptance differs for {input:?}"
        );
    }
    outcome
}

fn read<'input>(
    input: &'input [u8],
    limits: Option<Limits>,
    mut emit: impl FnMut(Event<'input>),
) -> Outcome {
    let mut reader = match limits.map_or_else(
        || Reader::new(input),
        |limits| Reader::with_limits(input, limits),
    ) {
        Ok(reader) => reader,
        Err(error) => {
            check_error(error, input);
            return Outcome {
                error: Some(error),
                events: 0,
            };
        }
    };
    let mut events = 0;
    let mut previous = reader.position();
    loop {
        let result = reader.read_event();
        let position = reader.position();
        assert!(
            (previous..=input.len()).contains(&position),
            "reader offset moved backwards or outside input"
        );
        previous = position;
        match result {
            Ok(Some(event)) => {
                let origin = reader.event_origin().unwrap();
                check_origin(origin, input.len());
                assert_eq!(reader.event_range(), origin.input_range());
                let range = span(origin);
                assert!(range.start <= range.end && range.end <= position);
                events += 1;
                assert!(
                    events <= event_bound(input.len()),
                    "reader made no bounded progress"
                );
                emit(event);
            }
            Ok(None) => {
                assert!(
                    reader
                        .read_event()
                        .expect("EOF must remain terminal")
                        .is_none()
                );
                return Outcome {
                    error: None,
                    events,
                };
            }
            Err(error) => {
                check_error(error, input);
                assert_eq!(
                    reader.read_event().expect_err("error must remain terminal"),
                    error
                );
                return Outcome {
                    error: Some(error),
                    events,
                };
            }
        }
    }
}

/// Compare the complete logical stream with the DOM, including values and names.
/// This detects conversion loss even though both APIs share lexical parsing.
pub fn check_projection(document: &Document<'_>, stream: &[Event<'_>]) {
    let mut nodes = document.nodes().skip(1);
    let mut parents = Vec::new();
    let mut declaration = None;
    let mut index = 0;
    while let Some(event) = stream.get(index) {
        index += 1;
        // Entity expansion can split one DOM text node into adjacent events.
        if let Event::Text(first) = event {
            let mut text = first.to_string();
            while let Some(Event::Text(next)) = stream.get(index) {
                text.push_str(next);
                index += 1;
            }
            if text.is_empty() {
                continue;
            }
            let node = nodes.next().expect("DOM lost a text node");
            let expected_parent = parents.last().copied().unwrap_or(document.root().id());
            assert_eq!(node.parent().unwrap().id(), expected_parent);
            assert!(
                matches!(node.kind(), NodeKind::Text(actual) if *actual == text),
                "DOM payload differs from stream"
            );
            continue;
        }
        match event {
            Event::Declaration(value) => {
                assert!(
                    declaration.replace(value).is_none(),
                    "duplicate declaration"
                );
                continue;
            }
            Event::End(name) => {
                let parent = parents.pop().expect("end event without open element");
                let node = document.node(parent).unwrap();
                assert!(node.name() == Some(name), "end name differs from DOM");
                continue;
            }
            Event::EntityEnd => {
                let parent = parents.pop().expect("entity end without open reference");
                let node = document.node(parent).unwrap();
                assert!(
                    node.entity_name().is_some(),
                    "entity end closes a non-reference"
                );
                continue;
            }
            _ => {}
        }
        let node = nodes.next().expect("DOM lost an event node");
        let expected_parent = parents.last().copied().unwrap_or(document.root().id());
        assert_eq!(
            node.parent().unwrap().id(),
            expected_parent,
            "DOM changed event nesting"
        );
        match (event, node.kind()) {
            (Event::Start(expected) | Event::Empty(expected), NodeKind::Element(actual)) => {
                assert!(
                    actual == expected,
                    "element names or attributes differ from stream"
                );
                check_attributes(&expected.attributes);
                if matches!(event, Event::Start(_)) {
                    parents.push(node.id());
                }
            }
            (Event::EntityStart(expected), NodeKind::EntityRef(actual)) => {
                assert!(actual == expected, "entity name differs from stream");
                parents.push(node.id());
            }
            (Event::Doctype(expected), NodeKind::Doctype(actual)) => {
                assert_eq!(actual, expected, "DOCTYPE differs from stream");
            }
            (Event::Cdata(expected), NodeKind::Cdata(actual))
            | (Event::Comment(expected), NodeKind::Comment(actual)) => {
                assert!(actual == expected, "DOM payload differs from stream");
            }
            (
                Event::ProcessingInstruction {
                    target: expected_target,
                    data: expected_data,
                },
                NodeKind::ProcessingInstruction {
                    target: actual_target,
                    data: actual_data,
                },
            ) => {
                assert!(
                    actual_target == expected_target,
                    "PI target differs from stream"
                );
                assert!(actual_data == expected_data, "PI data differs from stream");
            }
            _ => panic!("DOM node kind differs from stream"),
        }
    }
    assert!(parents.is_empty(), "stream did not close every element");
    assert!(nodes.next().is_none(), "DOM added nodes absent from stream");
    assert_eq!(
        document.declaration(),
        declaration,
        "DOM declaration differs from stream"
    );
    // The DOM coalesces adjacent Text events produced by entity expansion.
    assert!(
        document.root().text_chunks().collect::<String>()
            == stream
                .iter()
                .filter_map(|event| match event {
                    Event::Text(value) | Event::Cdata(value) => Some(value.as_ref()),
                    _ => None,
                })
                .collect::<String>(),
        "document string-value chunks differ from reader payloads"
    );
}

/// A control byte that depends on every input byte (FNV-1a), for policy
/// choices that grammar-shaped inputs could not otherwise vary.
pub(crate) fn selector(input: &[u8]) -> u8 {
    let hash = input.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
    });
    (hash >> 56) as u8
}

/// Every `Attributes` conversion preserves the list, lent or owned.
fn check_attributes(attributes: &yaxml::Attributes<'_>) {
    let lent = yaxml::Attributes::Borrowed(attributes);
    let mut owned = lent.clone();
    owned.to_mut().reverse();
    owned.to_mut().reverse();
    assert!(owned == lent && matches!(owned, yaxml::Attributes::Owned(_)));
    let collected: yaxml::Attributes<'_> = lent.clone().into_iter().collect();
    assert!(collected == *attributes);
    assert_eq!(lent.into_owned(), owned.into_owned());
    let mut empty = yaxml::Attributes::default();
    assert!(empty.to_mut().is_empty());
}

/// An exact input range and a replacement range exclude each other; the
/// physical context covers the exact range and lies inside the input.
fn check_origin(origin: yaxml::SourceOrigin, len: usize) {
    let context = origin.physical_context();
    assert!(context.start <= context.end && context.end <= len);
    if let Some(range) = origin.input_range() {
        assert!(origin.replacement_range().is_none(), "{origin:?}");
        assert_eq!(range, context);
    } else if let Some(range) = origin.replacement_range() {
        assert!(range.start <= range.end);
    }
}

fn check_error(error: Error, input: &[u8]) {
    assert!(
        error.offset <= input.len(),
        "error offset points outside the input: {error}"
    );
    check_display(&error);
    check_diagnostics(input, &[error]);
}

/// libxml2's description of failures: positioned in the input, worded, and
/// rendered as xmllint's report.
pub fn check_diagnostics(input: &[u8], errors: &[Error]) {
    let lines = yaxml::LineIndex::new(input);
    let last = lines.line(input.len());
    let (described, work, ceiling) = lines.describe_counted(errors);
    // The re-reader's work, entity expansion included, stays linear in the
    // input (whose UTF-8 transcoding is at most three times as long).
    assert!(
        work <= ceiling && ceiling <= 8 * 3 * input.len() + 64 * 1024,
        "{work} {ceiling}"
    );
    assert!(described.len() <= errors.len());
    // A charge never exceeds the text, so a description without a column
    // (out of work) means the budget is all but spent.
    let spent = work + 3 * input.len() >= ceiling;
    for diagnostic in described {
        assert!(
            diagnostic.line >= 1 && (diagnostic.column >= 1 || spent),
            "{diagnostic:?}"
        );
        assert!(
            diagnostic.entity || diagnostic.line <= last,
            "{diagnostic:?}"
        );
        assert!(!diagnostic.message.is_empty(), "{diagnostic:?}");
        let report = diagnostic.render("f", &lines);
        assert!(
            report.starts_with("f:") || report.starts_with("Entity: line"),
            "{report}"
        );
    }
}

// Formatting is also a fallible output boundary. Exercise actual errors with
// sinks that reject the first write or the final byte, then accept everything.
fn check_display(error: &impl std::fmt::Display) {
    use std::fmt::Write;

    struct Bounded {
        output: String,
        limit: usize,
    }

    impl Write for Bounded {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            if text.len() > self.limit - self.output.len() {
                return Err(std::fmt::Error);
            }
            self.output.push_str(text);
            Ok(())
        }
    }

    let expected = error.to_string();
    assert!(!expected.is_empty(), "empty error display");
    for limit in [0, expected.len() - 1, expected.len()] {
        let mut sink = Bounded {
            output: String::new(),
            limit,
        };
        let result = write!(&mut sink, "{error}");
        assert_eq!(
            result.is_ok(),
            limit == expected.len(),
            "error display did not propagate formatter failure"
        );
        assert!(expected.starts_with(&sink.output));
        if result.is_ok() {
            assert_eq!(sink.output, expected);
        }
    }
}

fn check_document(document: &Document<'_>, len: usize, events: usize) {
    let nodes: Vec<_> = document.nodes().collect();
    assert!(nodes.len() <= events + 1);
    assert!(matches!(document.root().kind(), NodeKind::Document));
    // Only a recovered document lacks an element.
    let element = document.document_element();
    assert!(element.is_some() || !document.errors().is_empty());
    if let Some(element) = element {
        assert!(matches!(element.kind(), NodeKind::Element(_)));
        assert_eq!(element.parent().unwrap().id(), document.root().id());
    }
    let _ = document.declaration();
    assert_eq!(document.root().range(), Some(0..len));
    assert!(document.root().parent().is_none());
    assert!(document.root().next_sibling().is_none());

    let mut seen = HashSet::new();
    let mut pending = vec![document.root()];
    let mut visited = Vec::new();
    while let Some(node) = pending.pop() {
        assert!(seen.insert(node.id()), "DOM cycle or duplicate child");
        assert!(
            visited.len() < nodes.len(),
            "DOM traversal exceeded stored node count"
        );
        visited.push(node.id());
        assert_eq!(document.node(node.id()).unwrap().id(), node.id());
        check_origin(node.origin(), len);
        let range = span(node.origin());
        assert!(range.start <= range.end && range.end <= len);
        let children: Vec<_> = node.children().take(nodes.len() + 1).collect();
        assert!(children.len() <= nodes.len(), "sibling cycle");
        assert_eq!(
            node.first_child().map(|n| n.id()),
            children.first().map(|n| n.id())
        );
        assert_eq!(
            node.last_child().map(|n| n.id()),
            children.last().map(|n| n.id())
        );
        let mut previous_end = range.start;
        let mut subtree_size = 1;
        for (index, child) in children.iter().copied().enumerate() {
            assert_eq!(child.parent().unwrap().id(), node.id());
            assert_eq!(
                child.next_sibling().map(|n| n.id()),
                children.get(index + 1).map(|n| n.id())
            );
            // Nodes expanded from one entity reference share its physical span.
            if let Some(child_range) = child.range() {
                assert!(child_range.start >= previous_end && child_range.end <= range.end);
                previous_end = child_range.end;
            }
            subtree_size += child.descendants().len();
        }
        assert_eq!(node.descendants().len(), subtree_size);
        match node.kind() {
            NodeKind::Element(_) => {
                assert!(node.name().is_some());
                assert!(node.text().is_none());
            }
            NodeKind::Document | NodeKind::Doctype(_) => {
                assert!(node.name().is_none());
                assert!(node.text().is_none());
                assert!(node.attributes().is_empty());
            }
            NodeKind::EntityRef(name) => {
                assert_eq!(node.entity_name(), Some(name.as_ref()));
                assert!(node.name().is_none());
                assert!(node.text().is_none());
                assert!(node.attributes().is_empty());
            }
            _ => {
                assert!(node.name().is_none());
                assert!(node.text().is_some());
                assert!(node.attributes().is_empty());
                assert!(children.is_empty());
            }
        }
        pending.extend(children.into_iter().rev());
    }
    assert_eq!(visited, nodes.iter().map(|n| n.id()).collect::<Vec<_>>());
    assert_eq!(
        document.doctype(),
        nodes.iter().find_map(|node| match node.kind() {
            NodeKind::Doctype(doctype) => Some(doctype),
            _ => None,
        })
    );
    // Without DTD-declared IDs, `xml:id` lookup finds the first element in
    // document order carrying that value.
    if document.dtd().is_none() {
        for node in &nodes {
            for attribute in node.attributes() {
                if attribute.name.qualified() == "xml:id" {
                    let found = document.element_by_id(&attribute.value).expect("xml:id");
                    assert!(found.id() <= node.id());
                }
            }
        }
        assert!(document.element_by_id("\u{0}").is_none());
    }
    if nodes.len() > 2 {
        let smaller = Document::parse("<r/>").unwrap();
        assert!(smaller.node(nodes.last().unwrap().id()).is_none());
    }
}

pub(crate) fn drive_push(
    input: &[u8],
    schedule: &[usize],
    limits: Limits,
    mut emit: impl FnMut(&Event<'_>, Range<usize>),
) -> Result<(), Error> {
    let control = input.get(input.len() / 2).copied().unwrap_or(0);
    let mut parser = if limits == Limits::default() && control & 1 == 0 {
        PushParser::default()
    } else {
        PushParser::with_limits(limits)
    };
    let mut latest = None;
    let mut capture = |event: &Event<'_>, origin: yaxml::SourceOrigin| {
        let range = span(origin);
        latest = Some(range.clone());
        emit(event, range);
        ControlFlow::Continue(())
    };
    let mut previous = 0;
    let mut offset: usize = 0;
    let mut result = Ok(());
    for length in schedule.iter().copied().chain(std::iter::once(input.len())) {
        let end = offset.saturating_add(length).min(input.len());
        result = parser.feed(&input[offset..end], &mut capture);
        assert!((previous..=end).contains(&parser.position()));
        previous = parser.position();
        offset = end;
        if result.is_err() {
            break;
        }
    }
    if result.is_ok() {
        result = parser.finish(capture);
    }
    assert!(parser.position() <= input.len());
    if let Some(range) = latest {
        let origin = parser.event_origin().unwrap();
        assert_eq!(span(origin), range);
        assert_eq!(parser.event_range(), origin.input_range());
    }
    match result {
        Ok(()) => {
            parser
                .finish(|_, _| panic!("finish emitted twice"))
                .unwrap();
            let position = parser.position();
            if control & 1 == 0 {
                let suffix = if control & 2 == 0 {
                    &[][..]
                } else {
                    &input[..input.len().min(1)]
                };
                let error = parser
                    .feed(suffix, |_, _| panic!("callback after final EOF"))
                    .unwrap_err();
                assert_eq!(error.kind, ErrorKind::InvalidState);
                assert_eq!(error.offset, position);
                assert_eq!(parser.feed(&[], |_, _| panic!()).unwrap_err(), error);
                assert_eq!(parser.finish(|_, _| panic!()).unwrap_err(), error);
                assert_terminal_io(parser, error);
            } else {
                assert_terminal_io(
                    parser,
                    Error {
                        kind: ErrorKind::InvalidState,
                        offset: position,
                    },
                );
            }
            Ok(())
        }
        Err(error) => {
            check_error(error, input);
            assert_eq!(
                parser
                    .feed(&[], |_, _| panic!("callback after error"))
                    .unwrap_err(),
                error
            );
            assert_eq!(
                parser
                    .finish(|_, _| panic!("callback after error"))
                    .unwrap_err(),
                error
            );
            if control & 1 == 0 {
                assert_terminal_io(parser, error);
            }
            Err(error)
        }
    }
}

fn assert_terminal_io(parser: PushParser, expected: Error) {
    struct ForbiddenRead;
    impl std::io::Read for ForbiddenRead {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("terminal parser touched its I/O source");
        }
    }
    match parser.read_from(ForbiddenRead, |_, _| {
        panic!("terminal parser emitted a callback")
    }) {
        Err(ReadError::Xml(error)) => assert_eq!(error, expected),
        result => panic!("terminal parser returned {result:?}"),
    }
}

fn check_streaming(input: &[u8], slice_error: Option<Error>) {
    let mut expected = slice_error.is_none().then(|| Reader::new(input).unwrap());
    let mut schedule = Vec::new();
    let mut remaining = input.len();
    while remaining != 0 {
        let byte = input[schedule.len() % input.len()] as usize;
        let width = (1 + byte % 31)
            .max(input.len().div_ceil(4096))
            .min(remaining);
        schedule.push(width);
        remaining -= width;
    }
    let result = drive_push(input, &schedule, Limits::default(), |actual, range| {
        if let Some(expected) = &mut expected {
            let event = expected
                .read_event()
                .unwrap()
                .expect("extra streaming event");
            assert!(*actual == event, "typed streaming event differs from slice");
            assert_eq!(range, span(expected.event_origin().unwrap()));
        }
    });
    // Streaming owns structural names that the slice reader can borrow.
    if !matches!(&result, Err(error) if error.kind == ErrorKind::LimitExceeded(yaxml::Limit::OwnedBytes))
    {
        assert_eq!(
            result.is_ok(),
            slice_error.is_none(),
            "stream/slice acceptance differs for {input:?}"
        );
    }
    if result.is_ok() {
        let expected = expected.as_mut().expect("stream accepted invalid XML");
        assert!(
            expected.read_event().unwrap().is_none(),
            "missing streaming event"
        );
    }
    let mut expected = slice_error.is_none().then(|| Reader::new(input).unwrap());
    let result = PushParser::new().read_from(std::io::Cursor::new(input), |actual, origin| {
        let range = span(origin);
        if let Some(expected) = &mut expected {
            let event = expected.read_event().unwrap().expect("extra I/O event");
            assert!(*actual == event, "typed I/O event differs from slice");
            assert_eq!(range, span(expected.event_origin().unwrap()));
        }
        ControlFlow::Continue(())
    });
    match result {
        Ok(()) => {
            let expected = expected.as_mut().expect("I/O parser accepted invalid XML");
            assert!(
                expected.read_event().unwrap().is_none(),
                "missing I/O event"
            );
        }
        Err(ReadError::Xml(error)) => {
            check_error(error, input);
            assert!(
                slice_error.is_some()
                    || error.kind == ErrorKind::LimitExceeded(yaxml::Limit::OwnedBytes)
            );
        }
        Err(error) => panic!("cursor produced I/O failure: {error}"),
    }
    check_io_schedule(input);
}

fn check_io_schedule(input: &[u8]) {
    use std::{
        cell::Cell,
        io::{self, Read},
    };
    struct Scheduled<'a> {
        input: &'a [u8],
        calls: &'a Cell<usize>,
        fail: usize,
        interrupt: bool,
    }
    if input.get(2).copied().unwrap_or(0) & 7 == 0 {
        struct Oversized<'a> {
            calls: &'a Cell<usize>,
            extra: usize,
        }
        impl Read for Oversized<'_> {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                self.calls.set(self.calls.get() + 1);
                Ok(out.len() + self.extra)
            }
        }
        let calls = Cell::new(0);
        let source = Oversized {
            calls: &calls,
            extra: 1 + input.first().copied().unwrap_or(0) as usize,
        };
        match PushParser::default()
            .read_from(source, |_, _| panic!("oversized count delivered XML"))
        {
            Err(ReadError::Io(error)) => {
                assert_eq!(error.kind(), io::ErrorKind::InvalidData);
                assert_eq!(error.to_string(), "reader returned an oversized byte count");
            }
            result => panic!("oversized read count returned {result:?}"),
        }
        assert_eq!(calls.get(), 1, "read again after invalid byte count");
    }
    impl Read for Scheduled<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let call = self.calls.get();
            self.calls.set(call + 1);
            if self.interrupt {
                self.interrupt = false;
                return Err(io::ErrorKind::Interrupted.into());
            }
            if call == self.fail {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "fuzz injected I/O failure",
                ));
            }
            let length = self.input.len().min(out.len()).min(7);
            out[..length].copy_from_slice(&self.input[..length]);
            self.input = &self.input[length..];
            Ok(length)
        }
    }
    let calls = Cell::new(0);
    let fail = 1 + input.get(1).copied().unwrap_or(0) as usize % 4;
    // Independently step the XML parser to locate the adapter's last permitted
    // read. The source's first call is Interrupted; call `fail` is the I/O error.
    let expected_calls = {
        let mut parser = PushParser::new();
        let mut offset = 0;
        let mut terminal = fail + 1;
        for call in 1..fail {
            let end = (offset + 7).min(input.len());
            let result = if offset == end {
                parser.finish(|_, _| ControlFlow::Break(()))
            } else {
                parser.feed(&input[offset..end], |_, _| ControlFlow::Break(()))
            };
            if result.is_err() || offset == end {
                terminal = call + 1;
                break;
            }
            offset = end;
        }
        terminal
    };
    let reader = Scheduled {
        input,
        calls: &calls,
        fail,
        interrupt: true,
    };
    let mut cancelled = None;
    let result = PushParser::new().read_from(reader, |_, origin| {
        let range = span(origin);
        cancelled = Some((calls.get(), range.end));
        ControlFlow::Break(())
    });
    assert_eq!(
        calls.get(),
        expected_calls,
        "I/O adapter read beyond its terminal operation"
    );
    if let Err(error) = &result {
        check_display(error);
        assert!(std::error::Error::source(error).is_some());
    }
    match result {
        Err(ReadError::Io(error)) => {
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(error.to_string(), "fuzz injected I/O failure");
            assert_eq!(calls.get(), fail + 1);
        }
        Err(ReadError::Xml(error)) => {
            check_error(error, input);
            if let Some((call, end)) = cancelled {
                assert_eq!(error.kind, ErrorKind::Aborted);
                assert_eq!(error.offset, end);
                assert_eq!(calls.get(), call, "read after callback cancellation");
            }
            assert!(
                calls.get() <= fail + 1,
                "read after terminal parser failure"
            );
        }
        Ok(()) => assert!(cancelled.is_none()),
        Err(error) => panic!("unexpected read error: {error}"),
    }
}

#[cfg(test)]
mod formatting_tests {
    use super::{check_abort, check_display};
    use yaxml::{EditError, EditErrorKind, Error, ErrorKind, ReadError, WriteError};

    #[test]
    fn real_errors_obey_formatter_failure_before_and_during_offsets() {
        let error = Error {
            kind: ErrorKind::InvalidCharacter,
            offset: usize::MAX,
        };
        check_display(&error);
        check_display(&ReadError::Xml(error));
        check_display(&ReadError::Io(std::io::Error::other("read failure")));
        check_display(&WriteError::Xml(error));
        check_display(&WriteError::Io(std::io::Error::other("write failure")));
        for value_offset in [None, Some(0), Some(usize::MAX)] {
            check_display(&EditError {
                kind: EditErrorKind::InvalidValue,
                value_offset,
                location: None,
            });
        }
        check_display(&"Unicode: 中😀");
    }

    #[test]
    #[should_panic(expected = "error display did not propagate formatter failure")]
    fn oracle_rejects_a_formatter_that_swallows_sink_errors() {
        struct Swallowed;
        impl std::fmt::Display for Swallowed {
            fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let _ = output.write_str("ignored failure");
                Ok(())
            }
        }
        check_display(&Swallowed);
    }

    #[test]
    #[should_panic(expected = "empty error display")]
    fn oracle_rejects_an_empty_error_display() {
        check_display(&"");
    }

    #[test]
    fn cancellation_oracle_accepts_only_the_delivered_event_end() {
        check_abort(
            Error {
                kind: ErrorKind::Aborted,
                offset: 7,
            },
            7,
        );
    }

    #[test]
    #[should_panic(expected = "cancellation offset did not end the delivered event")]
    fn cancellation_oracle_rejects_a_stale_offset() {
        check_abort(
            Error {
                kind: ErrorKind::Aborted,
                offset: 6,
            },
            7,
        );
    }
}
