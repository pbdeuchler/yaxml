#![forbid(unsafe_code)]
//! Entity amplification follows libxml2's `xmlParserEntityCheck`: expansion may
//! exceed a fixed allowance only up to a factor of the input consumed. These
//! checks are wall-clock independent: they assert error kinds, delivered-event
//! counts and the exact, linear cost of each expansion on all five surfaces.

use std::io::{self, BufReader, Cursor};
use std::ops::ControlFlow;

use yaxml::{
    BufferedReader, Document, Error, ErrorKind, Limit, Limits, OwnedDocument, PushParser,
    ReadError, Reader, Request, Resolver,
};

const SURFACES: [&str; 5] = ["reader", "document", "owned", "push", "buffered"];

/// Serves `ext` (relative to `doc.xml`) to every surface.
fn resolver() -> Box<dyn Resolver> {
    Box::new(|request: Request<'_>| -> io::Result<Option<Vec<u8>>> {
        Ok((request.uri == "ext").then(|| b"xxxxxxxxxx".to_vec()))
    })
}

/// Parses with one surface; `Ok` carries the number of delivered events.
fn run(surface: &str, input: &[u8], limits: Limits) -> Result<usize, Error> {
    let base = "doc.xml";
    match surface {
        "reader" => {
            let mut reader = Reader::with_options(input, loading(limits, base, resolver()))?;
            let mut events = 0;
            while reader.read_event()?.is_some() {
                events += 1;
            }
            Ok(events)
        }
        "document" => Document::parse_with(input, loading(limits, base, resolver()))
            .map(|document| document.nodes().len()),
        "owned" => OwnedDocument::parse_with(input, loading(limits, base, resolver()))
            .map(|document| document.nodes().len()),
        "push" => {
            let mut parser = PushParser::with_options(loading(limits, base, resolver())).unwrap();
            let mut events = 0;
            let mut count = |_: &yaxml::Event<'_>, _| {
                events += 1;
                ControlFlow::Continue(())
            };
            input
                .chunks(7)
                .try_for_each(|chunk| parser.feed(chunk, &mut count))
                .and_then(|()| parser.finish(&mut count))?;
            Ok(events)
        }
        _ => {
            let source = BufReader::with_capacity(5, Cursor::new(input));
            let mut reader =
                BufferedReader::with_options(source, loading(limits, base, resolver())).unwrap();
            let mut events = 0;
            loop {
                match reader.read_event() {
                    Ok(Some(_)) => events += 1,
                    Ok(None) => return Ok(events),
                    Err(ReadError::Xml(error)) => return Err(error),
                    Err(error) => panic!("{error}"),
                }
            }
        }
    }
}

/// Every surface's outcome, requiring one shared acceptance and error kind.
fn kind(input: &[u8], limits: Limits) -> Option<ErrorKind> {
    let expected = run("reader", input, limits).err().map(|error| error.kind);
    for surface in SURFACES {
        assert_eq!(
            run(surface, input, limits).err().map(|error| error.kind),
            expected,
            "{surface}"
        );
    }
    expected
}

fn repeat(prefix: &str, unit: &str, count: usize, suffix: &str) -> Vec<u8> {
    format!("{prefix}{}{suffix}", unit.repeat(count)).into_bytes()
}

fn laughs(leaf: &str) -> Vec<u8> {
    let mut input = format!("<!DOCTYPE r [<!ENTITY l0 '{leaf}'>");
    for level in 1..10 {
        let previous = format!("&l{};", level - 1).repeat(10);
        input.push_str(&format!("<!ENTITY l{level} '{previous}'>"));
    }
    input.push_str("]><r a='&l9;'>&l9;</r>");
    input.into_bytes()
}

#[test]
fn classic_attacks_stop_within_the_default_allowance_on_every_surface() {
    let quadratic = format!("<!DOCTYPE r [<!ENTITY a '{}'>]>", "x".repeat(10_000));
    let parameter = {
        let mut input = String::from("<!DOCTYPE r [<!ENTITY % p0 '<!---->'>");
        for level in 1..30 {
            let previous = format!("&#37;p{};", level - 1).repeat(2);
            input.push_str(&format!("<!ENTITY % p{level} '{previous}'>"));
        }
        input.push_str("%p29;]><r/>");
        input.into_bytes()
    };
    let cases: [(&str, Vec<u8>); 7] = [
        ("billion laughs", laughs("lol")),
        ("empty laughs", laughs("")),
        ("parameter laughs", parameter),
        (
            "quadratic content",
            repeat(&format!("{quadratic}<r>"), "&a;", 2_000, "</r>"),
        ),
        (
            "quadratic attribute",
            repeat(&format!("{quadratic}<r a='"), "&a;", 2_000, "'/>"),
        ),
        (
            "external amplification",
            repeat(
                "<!DOCTYPE r [<!ENTITY e SYSTEM 'ext'>]><r>",
                "&e;",
                60_000,
                "</r>",
            ),
        ),
        (
            "default amplification",
            repeat(
                &format!(
                    "<!DOCTYPE r [<!ATTLIST e a CDATA '{}'>]><r>",
                    "x".repeat(2_000)
                ),
                "<e/>",
                2_000,
                "</r>",
            ),
        ),
    ];
    let limits = Limits::default();
    for (name, input) in cases {
        // The allowance is max(floor, 5 x consumed), so no surface can have
        // produced more than one event per fixed-cost unit of it.
        let allowance = limits
            .max_entity_expansion_bytes
            .max(input.len() * limits.max_entity_amplification);
        // An event ceiling at that bound would be hit first by a runaway.
        let bounded = Limits {
            max_events: allowance / 20 + input.len(),
            ..limits
        };
        for surface in SURFACES {
            assert_eq!(
                run(surface, &input, bounded).map_err(|error| error.kind),
                Err(ErrorKind::LimitExceeded(Limit::EntityExpansion)),
                "{name} on {surface}"
            );
        }
    }
}

/// Builds a document with `n` uses of one expansion.
type Builder = fn(usize) -> Vec<u8>;

/// Smallest fixed allowance that accepts `input` with amplification disabled.
fn threshold(input: &[u8]) -> usize {
    let limits = |floor| Limits {
        max_entity_expansion_bytes: floor,
        max_entity_amplification: 0,
        ..Limits::default()
    };
    let (mut low, mut high) = (0, 1 << 22);
    assert_eq!(kind(input, limits(high)), None);
    while low < high {
        let middle = (low + high) / 2;
        match kind(input, limits(middle)) {
            None => high = middle,
            Some(ErrorKind::LimitExceeded(Limit::EntityExpansion)) => low = middle + 1,
            Some(other) => panic!("unexpected {other:?}"),
        }
    }
    low
}

#[test]
fn every_expansion_costs_a_constant_amount_per_use() {
    let cases: [(&str, Builder); 5] = [
        ("content entity", |n| {
            repeat(
                "<!DOCTYPE r [<!ENTITY e 'xxxxxxxxxx'>]><r>",
                "&e;",
                n,
                "</r>",
            )
        }),
        ("attribute entity", |n| {
            repeat(
                "<!DOCTYPE r [<!ENTITY e 'xxxxxxxxxx'>]><r a='",
                "&e;",
                n,
                "'/>",
            )
        }),
        ("default attribute", |n| {
            repeat(
                "<!DOCTYPE r [<!ATTLIST e a CDATA 'xxxxxxxxxx'>]><r>",
                "<e/>",
                n,
                "</r>",
            )
        }),
        ("parameter entity", |n| {
            repeat("<!DOCTYPE r [<!ENTITY % p '<!--x-->'>", "%p;", n, "]><r/>")
        }),
        ("external entity", |n| {
            repeat(
                "<!DOCTYPE r [<!ENTITY e SYSTEM 'ext'>]><r>",
                "&e;",
                n,
                "</r>",
            )
        }),
    ];
    for (name, document) in cases {
        let [one, two, four, eight] = [1, 2, 4, 8].map(|n| threshold(&document(n)));
        let unit = two - one;
        // Ten replacement bytes plus libxml2's fixed cost of 20, bounded.
        assert!((20..=40).contains(&unit), "{name}: {unit} per use");
        assert_eq!(four - two, 2 * unit, "{name} is not linear");
        assert_eq!(eight - four, 4 * unit, "{name} is not linear");
    }
}

#[test]
fn amplification_scales_with_consumed_input_above_the_floor() {
    let input = |n| {
        repeat(
            "<!DOCTYPE r [<!ENTITY e 'xxxxxxxxxx'>]><r>",
            "&e;",
            n,
            "</r>",
        )
    };
    let limits = |amplification| Limits {
        max_entity_expansion_bytes: 0,
        max_entity_amplification: amplification,
        ..Limits::default()
    };
    // Each use consumes three input bytes and costs about thirty: ratio ~10.
    assert_eq!(
        kind(&input(100), limits(5)),
        Some(ErrorKind::LimitExceeded(Limit::EntityExpansion))
    );
    assert_eq!(kind(&input(100), limits(20)), None);
    assert_eq!(kind(b"<r>no entities</r>", limits(0)), None);
    assert_eq!(
        kind(&input(1), limits(0)),
        Some(ErrorKind::LimitExceeded(Limit::EntityExpansion))
    );
}

/// Complete DTD processing (`NOENT | DTDATTR | DTDLOAD`) through `resolver`.
fn loading(
    limits: yaxml::Limits,
    base: &str,
    resolver: Box<dyn yaxml::Resolver>,
) -> yaxml::ParseOptions {
    let flags = yaxml::options::LEGACY | yaxml::options::DTDLOAD;
    yaxml::ParseOptions::new(flags)
        .with_limits(limits)
        .with_resolver(base, resolver)
}
