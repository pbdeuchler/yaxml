#![forbid(unsafe_code)]
//! `reset`/`reset_with` on `PushParser`, `BufferedReader` and `TextReader`:
//! a reused parser must behave exactly as a fresh one, and nothing a
//! previous document spent (budgets, sticky failure, resolver) may leak.

use std::io::{self, BufRead, Cursor, Read};
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use yaxml::options::{DTDLOAD, DTDVALID, LEGACY, NOENT, RECOVER};
use yaxml::{
    BufferedReader, ErrorKind, Event, Limit, Limits, ParseOptions, PushParser, ReadError, Request,
    Resolver, TextReader,
};

type Outcome = Result<Vec<String>, (ErrorKind, usize)>;

const GOOD: &[u8] = br#"<a xmlns:p="u" x="1"><p:b y="2">t</p:b><c/></a>"#;
const MISMATCH: &[u8] = b"<a><b></a>";
const DEEP: &[u8] = b"<a><a><a><a><a><a/></a></a></a></a></a>";
const ENTITIES: &[u8] = b"<!DOCTYPE r [<!ENTITY e 'xx'>]><r>&e;&e;&e;</r>";
const SMALL: &[u8] = b"<a/>";

fn feed(parser: &mut PushParser, chunk: &[u8], seen: &mut Vec<String>) -> Result<(), yaxml::Error> {
    parser.feed(chunk, |event: &Event<'_>, _| {
        seen.push(format!("{event:?}"));
        ControlFlow::Continue(())
    })
}

/// A whole document through `parser`: events, or the first error.
fn run(parser: &mut PushParser, document: &[u8]) -> Outcome {
    let mut seen = Vec::new();
    feed(parser, document, &mut seen)
        .and_then(|()| parser.finish(|_, _| ControlFlow::Continue(())))
        .map(|()| seen)
        .map_err(|error| (error.kind, error.offset))
}

fn options(limits: Limits, flags: u32) -> ParseOptions {
    ParseOptions::new(flags).with_limits(limits)
}

fn strict() -> Limits {
    Limits::default()
        .max_depth(4)
        .max_events(8)
        .max_input_bytes(60)
        .max_owned_bytes(64)
}

#[test]
fn reused_parser_equals_fresh_for_every_document_order() {
    let documents = [GOOD, MISMATCH, DEEP, ENTITIES, SMALL, b"", b"<a", GOOD];
    for (name, make) in [
        ("default", options(Limits::default(), 0)),
        ("strict", options(strict(), 0)),
        ("noent", options(Limits::default(), NOENT)),
    ]
    .map(|(name, o)| (name, (o.limits, o.flags)))
    {
        let configure = || options(make.0, make.1);
        let mut reused = PushParser::with_options(configure()).unwrap();
        for (index, document) in documents.iter().enumerate() {
            let mut fresh = PushParser::with_options(configure()).unwrap();
            assert_eq!(
                run(&mut reused, document),
                run(&mut fresh, document),
                "{name} document {index}"
            );
            reused.reset();
            assert_eq!(reused.position(), 0, "{name} {index}");
            assert!(reused.event_origin().is_none(), "{name} {index}");
        }
    }
}

#[test]
fn reset_discards_a_half_fed_document_and_a_finished_one() {
    let mut parser = PushParser::new();
    let mut seen = Vec::new();
    feed(&mut parser, b"<a><b", &mut seen).unwrap();
    parser.reset();
    assert_eq!(run(&mut parser, SMALL).unwrap().len(), 1);
    // Without a reset a finished parser refuses more input.
    assert_eq!(
        run(&mut parser, SMALL).unwrap_err().0,
        ErrorKind::InvalidState
    );
    parser.reset();
    assert!(run(&mut parser, SMALL).is_ok());
}

/// (limit, setter, document, flags): the setter is varied to find the
/// smallest value a fresh parser accepts.
type Setter = fn(&mut Limits, usize);
const BUDGETS: [(&str, Setter, &[u8], u32, Limit); 4] = [
    (
        "input",
        |l, n| l.max_input_bytes = n,
        GOOD,
        0,
        Limit::InputBytes,
    ),
    ("events", |l, n| l.max_events = n, GOOD, 0, Limit::Events),
    (
        "owned",
        |l, n| l.max_owned_bytes = n,
        GOOD,
        0,
        Limit::OwnedBytes,
    ),
    (
        "references",
        |l, n| l.max_entity_references = n,
        ENTITIES,
        NOENT,
        Limit::EntityReferences,
    ),
];

fn at(setter: Setter, n: usize, flags: u32) -> ParseOptions {
    let mut limits = Limits::default();
    setter(&mut limits, n);
    options(limits, flags)
}

fn tightest(setter: Setter, document: &[u8], flags: u32) -> usize {
    (0..4096)
        .find(|&n| {
            run(
                &mut PushParser::with_options(at(setter, n, flags)).unwrap(),
                document,
            )
            .is_ok()
        })
        .expect("a finite limit suffices")
}

#[test]
fn budgets_start_over_and_stay_enforced_across_resets() {
    for (name, setter, document, flags, limit) in BUDGETS {
        let n = tightest(setter, document, flags);
        assert!(n > 0, "{name}: a limit of 0 must fail");
        let expected = ErrorKind::LimitExceeded(limit);

        // Exactly enough for one document: repeated documents never accumulate.
        let mut parser = PushParser::with_options(at(setter, n, flags)).unwrap();
        for round in 0..3 {
            assert!(run(&mut parser, document).is_ok(), "{name} round {round}");
            parser.reset();
        }

        // One short: the hit repeats after reset (limits persist), is sticky
        // until reset, and a relaxed reset_with then succeeds.
        let mut parser = PushParser::with_options(at(setter, n - 1, flags)).unwrap();
        for round in 0..2 {
            assert_eq!(
                run(&mut parser, document).unwrap_err().0,
                expected,
                "{name} {round}"
            );
            assert!(run(&mut parser, document).is_err(), "{name} sticky");
            parser.reset();
        }
        parser.reset_with(at(setter, n, flags)).unwrap();
        assert!(run(&mut parser, document).is_ok(), "{name} relaxed");

        // A success does not loosen a stricter reset_with.
        parser.reset_with(at(setter, n - 1, flags)).unwrap();
        assert_eq!(
            run(&mut parser, document).unwrap_err().0,
            expected,
            "{name} strict"
        );
    }
}

/// Counts requests and serves one empty subset.
fn counting(count: &Arc<AtomicUsize>) -> Box<dyn Resolver> {
    let count = count.clone();
    Box::new(move |_: Request<'_>| {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(Some(b"<!ELEMENT r EMPTY>".to_vec()))
    })
}

const EXTERNAL: &[u8] = b"<!DOCTYPE r SYSTEM 's.dtd'><r/>";

#[test]
fn reset_drops_the_resolver_and_its_load_budget() {
    let flags = LEGACY | DTDLOAD;
    let count = Arc::new(AtomicUsize::new(0));
    let one_load = Limits::default().max_external_loads(1);
    let resolving =
        |c: &Arc<AtomicUsize>| options(one_load, flags).with_resolver("d/doc.xml", counting(c));

    let mut parser = PushParser::with_options(resolving(&count)).unwrap();
    assert!(run(&mut parser, EXTERNAL).is_ok());
    assert_eq!(count.load(Ordering::SeqCst), 1);

    // reset keeps flags and limits but not the resolver: nothing is loaded.
    parser.reset();
    assert!(run(&mut parser, EXTERNAL).is_ok());
    assert_eq!(count.load(Ordering::SeqCst), 1);

    // A new resolver is used, and the spent load budget (1 of 1) is fresh.
    for expected in [2, 3] {
        parser.reset_with(resolving(&count)).unwrap();
        assert!(run(&mut parser, EXTERNAL).is_ok());
        assert_eq!(count.load(Ordering::SeqCst), expected);
    }

    // The load limit still applies to the new document: zero loads allowed.
    let none = one_load.max_external_loads(0);
    parser
        .reset_with(options(none, flags).with_resolver("d/doc.xml", counting(&count)))
        .unwrap();
    assert_eq!(
        run(&mut parser, EXTERNAL).unwrap_err().0,
        ErrorKind::LimitExceeded(Limit::ExternalLoads)
    );
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[test]
fn reset_with_unsupported_options_changes_nothing() {
    let mut parser = PushParser::new();
    let mut seen = Vec::new();
    feed(&mut parser, b"<a>", &mut seen).unwrap();
    for flags in [DTDVALID, 1 << 30] {
        assert_eq!(
            parser
                .reset_with(ParseOptions::new(flags))
                .unwrap_err()
                .kind,
            ErrorKind::UnsupportedOption,
            "{flags:#x}"
        );
    }
    // The in-flight document is intact.
    feed(&mut parser, b"</a>", &mut seen).unwrap();
    parser.finish(|_, _| ControlFlow::Continue(())).unwrap();
    assert_eq!(seen.len(), 2);
}

/// A source that yields `GOOD` once and then fails.
#[derive(Debug)]
struct Flaky(Cursor<&'static [u8]>, bool);

impl Read for Flaky {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl BufRead for Flaky {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.1 && self.0.position() as usize == self.0.get_ref().len() {
            return Err(io::Error::other("boom"));
        }
        self.0.fill_buf()
    }
    fn consume(&mut self, amount: usize) {
        self.0.consume(amount);
    }
}

fn drain<R: BufRead>(reader: &mut BufferedReader<R>) -> Result<usize, ReadError> {
    let mut events = 0;
    while reader.read_event()?.is_some() {
        events += 1;
    }
    Ok(events)
}

#[test]
fn buffered_reader_reset_clears_failures_and_returns_the_source() {
    let flaky = |data: &'static [u8], fail| Flaky(Cursor::new(data), fail);
    let mut reader = BufferedReader::new(flaky(b"<a><b></a>", false));
    assert!(matches!(drain(&mut reader), Err(ReadError::Xml(_))));
    assert!(
        matches!(drain(&mut reader), Err(ReadError::Xml(_))),
        "sticky"
    );

    let previous = reader.reset(flaky(b"<a/>", false));
    assert_eq!(*previous.0.get_ref(), b"<a><b></a>");
    assert_eq!(reader.position(), 0);
    assert_eq!(drain(&mut reader).unwrap(), 1);
    assert_eq!(drain(&mut reader).unwrap(), 0, "finished stays finished");

    // An I/O failure is cleared too.
    reader.reset(flaky(b"<a>", true));
    assert!(matches!(drain(&mut reader), Err(ReadError::Io(_))));
    reader.reset(flaky(b"<a/>", false));
    assert_eq!(drain(&mut reader).unwrap(), 1);

    // Limits persist across reset and are replaced by reset_with.
    let mut reader = BufferedReader::with_options(
        flaky(b"<a><b/></a>", false),
        ParseOptions::default().with_limits(Limits::default().max_events(2)),
    )
    .unwrap();
    for _ in 0..2 {
        assert!(
            matches!(drain(&mut reader), Err(ReadError::Xml(e)) if e.kind == ErrorKind::LimitExceeded(Limit::Events))
        );
        reader.reset(flaky(b"<a><b/></a>", false));
    }
    reader
        .reset_with(flaky(b"<a><b/></a>", false), ParseOptions::default())
        .unwrap();
    assert_eq!(drain(&mut reader).unwrap(), 3);
    assert_eq!(
        reader
            .reset_with(flaky(b"<a/>", false), ParseOptions::new(DTDVALID))
            .unwrap_err()
            .kind,
        ErrorKind::UnsupportedOption
    );
}

fn names<R: BufRead>(reader: &mut TextReader<R>) -> Result<Vec<String>, ReadError> {
    let mut seen = Vec::new();
    while reader.read()? {
        seen.push(reader.name().unwrap_or_default().to_owned());
    }
    Ok(seen)
}

#[test]
fn text_reader_reset_starts_a_clean_document() {
    let mut reader = TextReader::new(Cursor::new(b"<a><b></a>".as_slice()));
    assert!(names(&mut reader).is_err());
    assert!(names(&mut reader).is_err(), "sticky");

    reader.reset(Cursor::new(b"<r><i/>t</r>".as_slice()));
    assert_eq!(names(&mut reader).unwrap(), ["r", "i", "#text", "r"]);

    // reset_with replaces the document URI and options.
    let options = ParseOptions::default().base(Some("n.xml".into()));
    reader
        .reset_with(Cursor::new(b"<r/>".as_slice()), options)
        .unwrap();
    assert!(reader.read().unwrap());
    assert_eq!(reader.base_uri().as_deref(), Some("n.xml"));

    assert_eq!(
        reader
            .reset_with(Cursor::new(b"<r/>".as_slice()), ParseOptions::new(RECOVER))
            .unwrap_err()
            .kind,
        ErrorKind::UnsupportedOption
    );
    // The failed reset left the reader where it was.
    assert!(!reader.read().unwrap(), "`<r/>` was already read");
}
