//! Resource budgets beyond byte counts: the document-wide attribute ceiling
//! on every frontend, `HUGE`'s relationship to it, and size series for the
//! paths that were superlinear (see docs/security.md, "Work and memory").

use std::time::{Duration, Instant};

use yaxml::options::{DTDATTR, DTDVALID, HUGE, NOBLANKS};
use yaxml::{
    BufferedReader, Document, ErrorKind, Limit, Limits, NodeType, OwnedDocument, ParseOptions,
    PushParser, ReadError, Reader, TextReader,
};

const EXCEEDED: ErrorKind = ErrorKind::LimitExceeded(Limit::TotalAttributes);

/// (name, retains attributes, parse): only DOM builders keep attributes, so only they are limited.
type Frontend = (
    &'static str,
    bool,
    fn(&[u8], ParseOptions) -> Result<(), ErrorKind>,
);

fn kind(error: yaxml::Error) -> ErrorKind {
    error.kind
}

fn read(error: ReadError) -> ErrorKind {
    match error {
        ReadError::Xml(error) => error.kind,
        other => panic!("slice input failed: {other}"),
    }
}

const FRONTENDS: [Frontend; 7] = [
    ("reader", false, |input, options| {
        let mut reader = Reader::with_options(input, options).map_err(kind)?;
        while reader.read_event().map_err(kind)?.is_some() {}
        Ok(())
    }),
    ("document", true, |input, options| {
        Document::parse_with(input, options).map(drop).map_err(kind)
    }),
    ("owned", true, |input, options| {
        OwnedDocument::parse_with(input, options)
            .map(drop)
            .map_err(kind)
    }),
    ("push", false, |input, options| {
        let mut parser = PushParser::with_options(options).map_err(kind)?;
        parser
            .feed(input, |_, _| std::ops::ControlFlow::Continue(()))
            .map_err(kind)?;
        parser
            .finish(|_, _| std::ops::ControlFlow::Continue(()))
            .map_err(kind)
    }),
    ("buffered", false, |input, options| {
        let mut reader = BufferedReader::with_options(input, options).map_err(kind)?;
        while reader.read_event().map_err(read)?.is_some() {}
        Ok(())
    }),
    ("textreader", false, |input, options| {
        let mut reader = TextReader::with_options(input, options).map_err(kind)?;
        while reader.read().map_err(read)? {}
        Ok(())
    }),
    // `expand` builds an owned document from the subtree under the reader's limits.
    ("textreader expand", true, |input, options| {
        let mut reader = TextReader::with_options(input, options).map_err(kind)?;
        while reader.read().map_err(read)? {
            if reader.node_type() == NodeType::Element {
                reader.expand().map_err(read)?;
                break;
            }
        }
        Ok(())
    }),
];

/// (input, flags, attributes the document counts).
const DOCUMENTS: &[(&str, u32, usize)] = &[
    ("<r/>", 0, 0),
    ("<r a='1'/>", 0, 1),
    ("<r a='1' b='2'/>", 0, 2),
    // Cumulative across elements, not per element.
    ("<r a='1'><c b='2'/><c b='3'></c></r>", 0, 3),
    // Namespace declarations are attributes.
    ("<r xmlns='u' xmlns:p='v'><p:c/></r>", 0, 2),
    // Defaulted attributes count only when they are applied.
    ("<!DOCTYPE r [<!ATTLIST r a CDATA 'x'>]><r/>", 0, 0),
    ("<!DOCTYPE r [<!ATTLIST r a CDATA 'x'>]><r/>", DTDATTR, 1),
    (
        "<!DOCTYPE r [<!ATTLIST r a CDATA 'x'>]><r b='1'/>",
        DTDATTR,
        2,
    ),
];

#[test]
fn total_attributes_ceiling_is_inclusive_for_every_dom_builder_and_ignored_by_streams() {
    for (input, flags, count) in DOCUMENTS {
        for (name, retains, run) in FRONTENDS {
            for ceiling in [count.saturating_sub(1), *count, count + 1] {
                let limits = Limits::DEFAULT.max_total_attributes(ceiling);
                let options = ParseOptions::DEFAULT.flags(*flags).limits(limits);
                let result = run(input.as_bytes(), options);
                let label = format!("{name} {input:?} flags={flags:#x} ceiling={ceiling}");
                match retains && ceiling < *count {
                    true => assert_eq!(result, Err(EXCEEDED), "{label}"),
                    false => assert_eq!(result, Ok(()), "{label}"),
                }
            }
        }
    }
}

#[test]
fn per_element_ceiling_still_reports_attributes_first() {
    let limits = Limits::DEFAULT.max_attributes(1).max_total_attributes(0);
    let options = ParseOptions::DEFAULT.limits(limits);
    let result = Document::parse_with(b"<r a='1' b='2'/>", options)
        .map(drop)
        .map_err(kind);
    assert_eq!(result, Err(ErrorKind::LimitExceeded(Limit::Attributes)));
}

#[test]
fn huge_keeps_the_document_attribute_ceiling() {
    // libxml2 has no such ceiling, so `HUGE` relaxes nothing here; callers raise it.
    let limits = Limits::DEFAULT.max_total_attributes(1);
    let input = b"<r a='1' b='2'/>";
    let strict = || ParseOptions::DEFAULT.flags(HUGE).limits(limits);
    assert_eq!(
        Document::parse_with(input, strict())
            .map(drop)
            .map_err(kind),
        Err(EXCEEDED)
    );
    assert_eq!(strict().effective_limits().0.max_total_attributes, 1);
    let raised = ParseOptions::DEFAULT
        .flags(HUGE)
        .limits(limits.max_total_attributes(2));
    assert!(Document::parse_with(input, raised).is_ok());
}

// ---- size series ----------------------------------------------------------

fn repeat(head: &str, unit: impl Fn(usize) -> String, bytes: usize, tail: &str) -> String {
    let mut text = String::from(head);
    let mut index = 0;
    while text.len() < bytes {
        text += &unit(index);
        index += 1;
    }
    text + tail
}

fn wide() -> Limits {
    Limits::DEFAULT
        .max_input_bytes(1 << 30)
        .max_events(1 << 30)
        .max_total_attributes(1 << 30)
        .max_owned_bytes(1 << 30)
        .max_dtd_bytes(1 << 30)
        .max_token_bytes(1 << 30)
}

fn seconds(work: impl FnOnce()) -> Duration {
    let start = Instant::now();
    work();
    start.elapsed()
}

/// Each case once took quadratic time or worse in its input; at this size that
/// is minutes in a debug build, while linear parsing is far below the bound.
/// Raw timings are noisy, so the bound is generous and the check is one-sided.
#[test]
fn formerly_superlinear_paths_finish_within_a_generous_bound() {
    let sibling_blanks = repeat("<r>", |_| " <a/> ".into(), 300_000, "</r>");
    let mixed = {
        let names: String = (0..20_000).map(|i| format!("|a{i}")).collect();
        let head = format!("<!DOCTYPE r [<!ELEMENT r (#PCDATA{names})*><!ELEMENT a0 EMPTY>]><r>");
        repeat(&head, |_| "<a0/>t".into(), 400_000, "</r>")
    };
    let required = {
        let declared: String = (0..500).map(|i| format!(" r{i} CDATA #REQUIRED")).collect();
        let head =
            format!("<!DOCTYPE r [<!ELEMENT r ANY><!ELEMENT e EMPTY><!ATTLIST e{declared}>]><r>");
        let element = format!(
            "<e{}/>",
            (0..500).map(|i| format!(" r{i}='v'")).collect::<String>()
        );
        repeat(&head, |_| element.clone(), 1_000_000, "</r>")
    };
    let duplicate_tokens = {
        let names: String = (0..50_000).map(|i| format!("|a{i}")).collect();
        format!(
            "<!DOCTYPE r [<!ELEMENT r (#PCDATA{names})*><!ATTLIST r a (z{names}) #IMPLIED>]><r/>"
        )
    };
    for (name, input, flags) in [
        ("NOBLANKS siblings", sibling_blanks, NOBLANKS),
        ("mixed content children", mixed, DTDVALID),
        ("required attributes", required, DTDVALID),
        (
            "mixed and enumerated duplicates",
            duplicate_tokens,
            DTDVALID,
        ),
    ] {
        let options = || ParseOptions::DEFAULT.flags(flags).limits(wide());
        for (frontend, _, run) in &FRONTENDS[1..3] {
            let elapsed = seconds(|| {
                run(input.as_bytes(), options())
                    .unwrap_or_else(|e| panic!("{name} {frontend}: {e:?}"));
            });
            assert!(
                elapsed < Duration::from_secs(30),
                "{name} {frontend}: {elapsed:?}"
            );
        }
    }
}

/// The adversarial series behind docs/security.md: `cargo run --release
/// --example adversarial_scaling` prints times and log-log slopes at 1, 4 and
/// 16 MiB. This asserts only that doubling the input never more than triples
/// the time on two cheap generators, which any quadratic path would fail.
#[test]
#[ignore = "timing series; run with --release"]
fn doubling_input_does_not_quadruple_time() {
    let series = |input: fn(usize) -> String| {
        [1 << 20, 2 << 20, 4 << 20].map(|bytes| {
            let input = input(bytes);
            let options = ParseOptions::DEFAULT.limits(wide());
            seconds(|| drop(Document::parse_with(input.as_bytes(), options).unwrap()))
        })
    };
    for (name, generator) in [
        (
            "flat",
            (|n| repeat("<r>", |_| "<a/>".into(), n, "</r>")) as fn(usize) -> String,
        ),
        ("attrs8", |n| {
            repeat(
                "<r>",
                |_| "<e a='1' b='2' c='3' d='4' e='5' f='6' g='7' h='8'/>".into(),
                n,
                "</r>",
            )
        }),
    ] {
        let [a, _, c] = series(generator);
        assert!(
            c < a * 12 + Duration::from_millis(50),
            "{name}: {a:?} -> {c:?}"
        );
    }
}

/// libxml2's input-stack ceiling: 20 by default, 40 with `HUGE`.
#[test]
fn entity_depth_defaults_match_libxml2_and_huge_raises_them() {
    let nested = |levels: usize| {
        let mut dtd = String::from("<!ENTITY e0 'x'>");
        for i in 1..=levels {
            dtd += &format!("<!ENTITY e{i} '&e{}; '>", i - 1);
        }
        format!("<!DOCTYPE r [{dtd}]><r>&e{levels};</r>")
    };
    for (levels, flags, ok) in [
        (15, 0, true),
        (30, 0, false),
        (30, HUGE, true),
        (60, HUGE, false),
    ] {
        let options = ParseOptions::DEFAULT.flags(flags | yaxml::options::NOENT);
        let result = Document::parse_with(nested(levels).as_bytes(), options)
            .map(drop)
            .map_err(kind);
        match ok {
            true => assert_eq!(result, Ok(()), "{levels} {flags:#x}"),
            false => assert_eq!(
                result,
                Err(ErrorKind::LimitExceeded(Limit::EntityDepth)),
                "{levels} {flags:#x}"
            ),
        }
    }
    assert_eq!(Limits::DEFAULT.max_entity_depth, 20);
}
