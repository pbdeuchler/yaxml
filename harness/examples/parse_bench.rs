#![forbid(unsafe_code)]

#[path = "support/parse_observer.rs"]
mod parse_observer;

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::hint::black_box;
use std::io::{BufReader, Cursor, Read};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parse_observer::{Observation, Observer};
use yaxml::{
    BufferedReader, Document, Event, OwnedDocument, ParseOptions, PushParser, Reader, options,
};

const VERSION_LINE: &str = "YAXML-PARSE-BENCH-1\tengine=yaxml\tversion=0.1.0\n";
const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_AGGREGATE_BYTES: usize = 512 * 1024 * 1024;
const MAX_ITERATIONS: usize = 10_000;
const MAX_CHUNK: usize = 65_536;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Dom,
    Owned,
    Reader,
    Push,
    Buffered,
}

impl Mode {
    #[cfg(test)]
    const ALL: [Self; 5] = [
        Self::Dom,
        Self::Owned,
        Self::Reader,
        Self::Push,
        Self::Buffered,
    ];

    fn parse(value: &OsStr) -> Result<Self, String> {
        match value.to_str() {
            Some("dom") => Ok(Self::Dom),
            Some("owned") => Ok(Self::Owned),
            Some("reader") => Ok(Self::Reader),
            Some("push") => Ok(Self::Push),
            Some("buffered") => Ok(Self::Buffered),
            _ => Err("mode must be dom, owned, reader, push or buffered".into()),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Dom => "dom",
            Self::Owned => "owned",
            Self::Reader => "reader",
            Self::Push => "push",
            Self::Buffered => "buffered",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct Config {
    mode: Mode,
    input: PathBuf,
    iterations: usize,
    chunk: usize,
    /// The `dtd` profile: a DOCTYPE is accepted and entities are substituted
    /// (`NOENT`), as the C helper's profile does.
    flags: u32,
}

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Version,
    Run(Config),
}

#[derive(Default)]
struct Times {
    parse_ns: u64,
    observe_ns: u64,
    drop_ns: u64,
}

fn main() {
    if let Err(error) = entry(std::env::args_os().skip(1).collect()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn entry(args: Vec<OsString>) -> Result<(), String> {
    match parse_args(&args)? {
        Command::Version => {
            print!("{VERSION_LINE}");
            Ok(())
        }
        Command::Run(config) => {
            let input = read_input(&config.input)?;
            std::str::from_utf8(&input)
                .map_err(|error| format!("input is not valid UTF-8: {error}"))?;
            validate_workload(input.len(), config.iterations)?;
            let (observation, times) = benchmark(&config, &input)?;
            println!("{}", result_row(&config, input.len(), observation, &times));
            Ok(())
        }
    }
}

fn parse_args(args: &[OsString]) -> Result<Command, String> {
    if args == [OsString::from("--version")] {
        return Ok(Command::Version);
    }
    let (args, flags) = match args {
        [rest @ .., profile] if args.len() == 5 && profile == "dtd" => (rest, options::NOENT),
        _ => (args, 0),
    };
    let [mode, input, iterations, chunk] = args else {
        return Err("usage: parse_bench MODE INPUT ITERATIONS CHUNK [dtd]".into());
    };
    Ok(Command::Run(Config {
        mode: Mode::parse(mode)?,
        input: PathBuf::from(input),
        iterations: decimal(iterations, "iterations", MAX_ITERATIONS)?,
        chunk: decimal(chunk, "chunk", MAX_CHUNK)?,
        flags,
    }))
}

fn decimal(value: &OsStr, name: &str, maximum: usize) -> Result<usize, String> {
    let value = value
        .to_str()
        .ok_or_else(|| format!("{name} must be UTF-8 decimal digits"))?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{name} must contain decimal digits"));
    }
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("{name} is too large"))?;
    if !(1..=maximum).contains(&parsed) {
        return Err(format!("{name} is outside 1..={maximum}"));
    }
    Ok(parsed)
}

fn read_input(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("cannot inspect input: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_INPUT_BYTES as u64 {
        return Err("input must be a regular file no larger than 8 MiB".into());
    }
    let file = File::open(path).map_err(|error| format!("cannot open input: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect opened input: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_INPUT_BYTES as u64 {
        return Err("opened input must be a regular file no larger than 8 MiB".into());
    }
    let initial = usize::try_from(metadata.len()).map_err(|_| "input length does not fit usize")?;
    let mut input = Vec::new();
    input
        .try_reserve_exact(initial)
        .map_err(|_| "cannot reserve input buffer")?;
    file.take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut input)
        .map_err(|error| format!("cannot read input: {error}"))?;
    if input.len() > MAX_INPUT_BYTES {
        return Err("input grew beyond the 8 MiB ceiling while reading".into());
    }
    Ok(input)
}

fn validate_workload(bytes: usize, iterations: usize) -> Result<(), String> {
    let aggregate = bytes
        .checked_mul(iterations)
        .ok_or("aggregate input byte count overflow")?;
    if aggregate > MAX_AGGREGATE_BYTES {
        return Err("aggregate input bytes exceed 512 MiB".into());
    }
    Ok(())
}

fn benchmark(config: &Config, input: &[u8]) -> Result<(Observation, Times), String> {
    validate_profile(input)?;
    let mut first = None;
    let mut totals = Times::default();
    for _ in 0..config.iterations {
        let (observed, parse, observe, drop) =
            measure_once(config.mode, input, config.chunk, config.flags)?;
        if let Some(expected) = first {
            if observed != expected {
                return Err("semantic observation changed between iterations".into());
            }
        } else {
            first = Some(observed);
        }
        add_time(&mut totals.parse_ns, parse, "parse")?;
        add_time(&mut totals.observe_ns, observe, "observe")?;
        add_time(&mut totals.drop_ns, drop, "drop")?;
    }
    Ok((first.ok_or("iteration count produced no sample")?, totals))
}

fn validate_profile(input: &[u8]) -> Result<(), String> {
    let mut reader = Reader::new(input)
        .map_err(|error| format!("input is outside the benchmark profile: {error}"))?;
    let event = reader
        .read_event()
        .map_err(|error| format!("input is outside the benchmark profile: {error}"))?;
    if let Some(Event::Declaration(declaration)) = event
        && declaration.version != "1.0"
    {
        return Err("benchmark input must use XML version 1.0".into());
    }
    Ok(())
}

/// Rejects a parse that kept a namespace error, as the libxml2 helper fails
/// on any reported error.
fn kept(errors: &[yaxml::Error]) -> Result<(), String> {
    (errors.iter())
        .find(|error| !matches!(error.kind, yaxml::ErrorKind::Warning(w) if w != yaxml::Warning::NamespaceUri))
        .map_or(Ok(()), |error| Err(format!("namespace error: {error}")))
}

fn add_time(total: &mut u64, elapsed: Duration, phase: &str) -> Result<(), String> {
    let elapsed = u64::try_from(elapsed.as_nanos())
        .map_err(|_| format!("{phase} duration does not fit the result protocol"))?;
    *total = total
        .checked_add(elapsed)
        .ok_or_else(|| format!("{phase} duration overflow"))?;
    Ok(())
}

fn measure_once(
    mode: Mode,
    input: &[u8],
    chunk: usize,
    flags: u32,
) -> Result<(Observation, Duration, Duration, Duration), String> {
    let options = || ParseOptions::new(flags);
    match mode {
        Mode::Dom => {
            let started = Instant::now();
            let document = Document::parse_with(black_box(input), options())
                .map_err(|error| format!("DOM parse failed: {error}"))?;
            kept(document.errors())?;
            black_box(&document);
            let parse = started.elapsed();
            let started = Instant::now();
            let observed = parse_observer::borrowed_document(black_box(&document))?;
            black_box(&observed);
            let observe = started.elapsed();
            let started = Instant::now();
            drop(document);
            let drop = started.elapsed();
            Ok((observed, parse, observe, drop))
        }
        Mode::Owned => {
            let started = Instant::now();
            let document = OwnedDocument::parse_with(black_box(input), options())
                .map_err(|error| format!("owned DOM parse failed: {error}"))?;
            kept(document.errors())?;
            black_box(&document);
            let parse = started.elapsed();
            let started = Instant::now();
            let observed = parse_observer::owned_document(black_box(&document))?;
            black_box(&observed);
            let observe = started.elapsed();
            let started = Instant::now();
            drop(document);
            let drop = started.elapsed();
            Ok((observed, parse, observe, drop))
        }
        Mode::Reader => measure_reader(input, options()),
        Mode::Push => measure_push(input, chunk, options()),
        Mode::Buffered => measure_buffered(input, chunk, options()),
    }
}

fn measure_reader(
    input: &[u8],
    options: ParseOptions,
) -> Result<(Observation, Duration, Duration, Duration), String> {
    let started = Instant::now();
    let mut reader = Reader::with_options(black_box(input), options)
        .map_err(|error| format!("reader construction failed: {error}"))?;
    let mut observer = Observer::new();
    while let Some(event) = reader
        .read_event()
        .map_err(|error| format!("reader parse failed: {error}"))?
    {
        observer.event(event);
    }
    if reader.position() != input.len() {
        return Err("reader did not consume the complete input".into());
    }
    kept(reader.errors())?;
    let observed = observer.finish();
    black_box(&observed);
    let parse = started.elapsed();
    let started = Instant::now();
    drop(reader);
    let drop = started.elapsed();
    Ok((observed, parse, Duration::ZERO, drop))
}

fn measure_push(
    input: &[u8],
    chunk: usize,
    options: ParseOptions,
) -> Result<(Observation, Duration, Duration, Duration), String> {
    let started = Instant::now();
    let mut parser = PushParser::with_options(options)
        .map_err(|error| format!("push parser construction failed: {error}"))?;
    let mut observer = Observer::new();
    for bytes in black_box(input).chunks(chunk) {
        parser
            .feed(bytes, |event, _| {
                observer.event(event);
                ControlFlow::Continue(())
            })
            .map_err(|error| format!("push parse failed: {error}"))?;
    }
    parser
        .finish(|event, _| {
            observer.event(event);
            ControlFlow::Continue(())
        })
        .map_err(|error| format!("push finish failed: {error}"))?;
    if parser.position() != input.len() {
        return Err("push parser did not consume the complete input".into());
    }
    kept(parser.errors())?;
    let observed = observer.finish();
    black_box(&observed);
    let parse = started.elapsed();
    let started = Instant::now();
    drop(parser);
    let drop = started.elapsed();
    Ok((observed, parse, Duration::ZERO, drop))
}

fn measure_buffered(
    input: &[u8],
    chunk: usize,
    options: ParseOptions,
) -> Result<(Observation, Duration, Duration, Duration), String> {
    let started = Instant::now();
    let source = BufReader::with_capacity(chunk, Cursor::new(black_box(input)));
    let mut reader = BufferedReader::with_options(source, options)
        .map_err(|error| format!("buffered reader construction failed: {error}"))?;
    let mut observer = Observer::new();
    while let Some((event, _)) = reader
        .read_event()
        .map_err(|error| format!("buffered parse failed: {error}"))?
    {
        observer.event(event);
    }
    if reader.position() != input.len() {
        return Err("buffered reader did not consume the complete input".into());
    }
    kept(reader.errors())?;
    let observed = observer.finish();
    black_box(&observed);
    let parse = started.elapsed();
    let started = Instant::now();
    drop(reader);
    let drop = started.elapsed();
    Ok((observed, parse, Duration::ZERO, drop))
}

fn result_row(config: &Config, bytes: usize, observed: Observation, times: &Times) -> String {
    format!(
        "YAXML-PARSE-BENCH-1\tengine=yaxml\tmode={}\tbytes={bytes}\titerations={}\tchunk={}\tparse_ns={}\tobserve_ns={}\tdrop_ns={}\telements={}\tattributes={}\tnamespaces={}\ttext_bytes={}\tcomments={}\tpis={}\thash={:016x}",
        config.mode.as_str(),
        config.iterations,
        config.chunk,
        times.parse_ns,
        times.observe_ns,
        times.drop_ns,
        observed.elements,
        observed.attributes,
        observed.namespaces,
        observed.text_bytes,
        observed.comments,
        observed.pis,
        observed.hash,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEMANTIC_XML: &[u8] = b"<?xml version='1.0'?><!--pre--><?top d?><p:r a='x&#x9;y' xmlns='urn:d' p:b='&amp;' xmlns:p='urn:p'>x&amp;<![CDATA[y]]>&#x7a;<p:e/><!--c--><?pi data?></p:r><!--post-->";

    fn sample(mode: Mode, input: &[u8], chunk: usize) -> Result<Observation, String> {
        measure_once(mode, input, chunk, 0).map(|sample| sample.0)
    }

    #[test]
    fn dtd_profile_substitutes_entities_in_every_mode() {
        let input =
            b"<!DOCTYPE r [<!ENTITY e 'x<i>y</i>'><!ATTLIST r a CDATA 'd'>]><r>&e;&amp;</r>";
        let mut expected = None;
        for mode in Mode::ALL {
            let observed = measure_once(mode, input, 3, options::NOENT).unwrap().0;
            assert_eq!(
                (observed.elements, observed.attributes, observed.text_bytes),
                (2, 0, 3),
                "{mode:?}"
            );
            assert_eq!(*expected.get_or_insert(observed), observed, "{mode:?}");
        }
    }

    #[test]
    fn every_mode_matches_normalized_namespace_and_text_semantics() {
        let mut expected = None;
        for mode in Mode::ALL {
            let observed = sample(mode, SEMANTIC_XML, 1).unwrap();
            assert_eq!(observed.elements, 2, "{mode:?}");
            assert_eq!(observed.attributes, 2, "{mode:?}");
            assert_eq!(observed.namespaces, 2, "{mode:?}");
            assert_eq!(observed.text_bytes, 4, "{mode:?}");
            assert_eq!(observed.comments, 3, "{mode:?}");
            assert_eq!(observed.pis, 2, "{mode:?}");
            assert_eq!(*expected.get_or_insert(observed), observed, "{mode:?}");
        }
    }

    #[test]
    fn empty_text_segments_do_not_split_coalesced_text() {
        let input = b"<r><![CDATA[]]>a<![CDATA[]]>b</r>";
        let expected = sample(Mode::Reader, input, 1).unwrap();
        assert_eq!(expected.text_bytes, 2);
        for mode in Mode::ALL {
            assert_eq!(sample(mode, input, 2).unwrap(), expected, "{mode:?}");
        }
    }

    #[test]
    fn every_mode_rejects_malformed_and_dtd_inputs() {
        for input in [
            b"<r>".as_slice(),
            b"<!DOCTYPE r [<!ELEMENT>]><r/>".as_slice(),
        ] {
            for mode in Mode::ALL {
                assert!(sample(mode, input, 1).is_err(), "{mode:?} {input:?}");
            }
        }
    }

    #[test]
    fn benchmark_profile_rejects_forward_and_noncanonical_versions() {
        let config = Config {
            mode: Mode::Reader,
            input: "ignored.xml".into(),
            iterations: 1,
            chunk: 1,
            flags: 0,
        };
        assert!(benchmark(&config, b"<?xml version='1.0'?><r/>").is_ok());
        assert!(benchmark(&config, b"<r/>").is_ok());
        for input in [
            b"<?xml version='1.1'?><r/>".as_slice(),
            b"<?xml version='1.00'?><r/>",
        ] {
            assert!(benchmark(&config, input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn arguments_are_exact_and_decimal_bounds_are_closed() {
        let run = |mode: &str, iterations: &str, chunk: &str| {
            parse_args(&[mode, "input.xml", iterations, chunk].map(OsString::from))
        };
        assert_eq!(
            parse_args(&[OsString::from("--version")]),
            Ok(Command::Version)
        );
        assert_eq!(
            run("dom", "1", "65536"),
            Ok(Command::Run(Config {
                mode: Mode::Dom,
                input: "input.xml".into(),
                iterations: 1,
                chunk: 65_536,
                flags: 0,
            }))
        );
        assert_eq!(
            parse_args(&["push", "input.xml", "2", "7", "dtd"].map(OsString::from)),
            Ok(Command::Run(Config {
                mode: Mode::Push,
                input: "input.xml".into(),
                iterations: 2,
                chunk: 7,
                flags: options::NOENT,
            }))
        );
        assert!(run("buffered", "10000", "1").is_ok());
        for bad in [
            Vec::new(),
            vec![OsString::from("--version"), OsString::from("extra")],
            vec![OsString::from("dom"), OsString::from("input.xml")],
            ["dom", "input.xml", "1", "1", "plain"]
                .map(OsString::from)
                .to_vec(),
        ] {
            assert!(parse_args(&bad).is_err(), "{bad:?}");
        }
        for (mode, iterations, chunk) in [
            ("unknown", "1", "1"),
            ("dom", "", "1"),
            ("dom", "+1", "1"),
            ("dom", "0", "1"),
            ("dom", "10001", "1"),
            ("dom", "1", "0"),
            ("dom", "1", "65537"),
            ("dom", "1", "x"),
        ] {
            assert!(run(mode, iterations, chunk).is_err());
        }
    }

    #[test]
    fn workload_accepts_exact_aggregate_limit_and_rejects_more() {
        assert!(validate_workload(MAX_INPUT_BYTES, 64).is_ok());
        assert!(validate_workload(MAX_INPUT_BYTES, 65).is_err());
        assert!(validate_workload(usize::MAX, 2).is_err());
    }

    #[test]
    fn result_row_has_fixed_field_order_and_hash_width() {
        let config = Config {
            mode: Mode::Push,
            input: "ignored.xml".into(),
            iterations: 2,
            chunk: 7,
            flags: 0,
        };
        let observed = Observation {
            elements: 1,
            attributes: 2,
            namespaces: 3,
            text_bytes: 4,
            comments: 5,
            pis: 6,
            hash: 0xab,
        };
        let times = Times {
            parse_ns: 7,
            observe_ns: 0,
            drop_ns: 8,
        };
        assert_eq!(
            result_row(&config, 9, observed, &times),
            "YAXML-PARSE-BENCH-1\tengine=yaxml\tmode=push\tbytes=9\titerations=2\tchunk=7\tparse_ns=7\tobserve_ns=0\tdrop_ns=8\telements=1\tattributes=2\tnamespaces=3\ttext_bytes=4\tcomments=5\tpis=6\thash=00000000000000ab"
        );
    }
}
