//! External subsets and entities served from fuzz-provided memory: every
//! surface agrees, requests stay within the load and byte budgets, and
//! repeated parses issue the same requests. Selectors choose the resource
//! policy (decline, fail, UTF-16, a sandboxed `FileResolver`) and small
//! `Limits` profiles so budget and failure branches are reachable.

use std::io::{BufReader, Cursor};
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use yaxml::valid::Dtd;
use yaxml::{
    BufferedReader, Document, Error, ErrorKind, FileResolver, Limits, OwnedDocument, PushParser,
    ReadError, Reader, Request, Resolver,
};

pub(super) const LIMITS: Limits = Limits {
    max_input_bytes: 1 << 20,
    max_token_bytes: 1 << 16,
    max_depth: 64,
    max_attributes: 64,
    max_events: 1 << 16,
    max_owned_bytes: 1 << 22,
    max_dtd_bytes: 1 << 16,
    max_entity_expansion_bytes: 1 << 16,
    max_entity_amplification: 5,
    max_entity_references: 1 << 12,
    max_entity_depth: 8,
    max_external_loads: 8,
    max_external_bytes: 1 << 14,
};

/// Resources are named `s`, `e`, `p` and `0`..`9` relative to the base.
const NAMES: [&str; 13] = [
    "s", "e", "p", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
];

/// How the resolver answers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Policy {
    /// Serve named pieces, decline anything else.
    Serve,
    /// Serve named pieces, fail anything else.
    Fail,
    /// Serve named pieces re-encoded as UTF-16LE with a BOM.
    Utf16,
    /// A `FileResolver` over a fixed sandbox directory.
    Files,
}

impl Policy {
    pub(super) fn from_bits(bits: u8) -> Self {
        [Self::Serve, Self::Fail, Self::Utf16, Self::Files][usize::from(bits & 3)]
    }
}

/// FNV-1a: selectors that do not consume grammar bytes.
pub(super) fn hash(input: &[u8]) -> u64 {
    input.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
    })
}

/// Small ceilings, chosen per family so each limit can be the first to trip.
pub(super) fn profile(profile: u8, seed: u64, len: usize) -> Limits {
    let small = |shift: u32, modulus: u64| ((seed >> shift) % modulus) as usize;
    match profile & 3 {
        0 => LIMITS,
        1 => Limits {
            max_external_loads: small(0, 4),
            max_external_bytes: small(2, 96),
            ..LIMITS
        },
        2 => Limits {
            max_entity_expansion_bytes: small(0, 128),
            max_entity_amplification: small(8, 3),
            max_entity_references: small(10, 12),
            max_entity_depth: small(14, 4),
            ..LIMITS
        },
        _ => Limits {
            max_dtd_bytes: small(0, 160),
            max_token_bytes: 4 + small(8, 48),
            max_owned_bytes: small(14, 512),
            max_depth: small(24, 6),
            max_attributes: small(28, 4),
            max_events: 2 + small(32, 24),
            max_input_bytes: match seed >> 39 & 1 {
                0 => LIMITS.max_input_bytes,
                _ => len.saturating_sub(small(40, 24)),
            },
            ..LIMITS
        },
    }
}

/// A per-process sandbox: `d/s`, `d/e`, `d/p` and `d/0` are files, `d/dir`
/// is a directory and `d/up` links to the filesystem root.
fn sandbox() -> &'static PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("yaxml-fuzz-files-{}", std::process::id()));
        let files = root.join("d");
        std::fs::create_dir_all(files.join("dir")).expect("sandbox directory");
        for (name, text) in [
            (
                "s",
                "<!ELEMENT r ANY><!ATTLIST r a CDATA 'd'><!ENTITY x 'X'>%p;",
            ),
            ("e", "<?xml version='1.0' encoding='UTF-8'?>e<b/>"),
            ("p", "<!ENTITY y SYSTEM '0'>"),
            ("0", "\u{feff}zero"),
        ] {
            std::fs::write(files.join(name), text).expect("sandbox file");
        }
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink("/", files.join("up"));
        root
    })
}

pub(super) fn utf16(bytes: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = vec![0xff, 0xfe];
    out.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    out
}

type Log = Arc<Mutex<Vec<(String, usize)>>>;

fn resolver(resources: &[Vec<u8>], policy: Policy, log: &Log) -> Box<dyn Resolver> {
    let resources = resources.to_vec();
    let log = log.clone();
    let mut files = (policy == Policy::Files).then(|| {
        let root = sandbox();
        assert!(
            FileResolver::new(root.join("d/s")).is_err(),
            "file root accepted"
        );
        FileResolver::new(root).expect("sandbox root")
    });
    Box::new(move |request: Request<'_>| {
        log.lock()
            .unwrap()
            .push((request.uri.to_owned(), request.max_bytes));
        if let Some(files) = &mut files {
            return files.resolve(request);
        }
        let name = request.uri.strip_prefix("d/").unwrap_or("");
        let found = NAMES
            .iter()
            .position(|candidate| *candidate == name)
            .and_then(|index| resources.get(index).cloned());
        match (found, policy) {
            (None, Policy::Fail) => Err(std::io::Error::other("fuzz resource failure")),
            (found, Policy::Utf16) => Ok(found.map(|bytes| utf16(&bytes))),
            (found, _) => Ok(found),
        }
    })
}

fn surface(
    index: usize,
    input: &[u8],
    limits: Limits,
    resolver: Box<dyn Resolver>,
) -> Result<usize, Error> {
    let base = "d/doc.xml";
    let mut events = 0;
    match index {
        0 => {
            let mut reader = Reader::with_options(input, loading(limits, base, resolver))?;
            while reader.read_event()?.is_some() {
                events += 1;
            }
        }
        1 => {
            events = Document::parse_with(input, loading(limits, base, resolver))?
                .nodes()
                .len()
        }
        2 => {
            events = OwnedDocument::parse_with(input, loading(limits, base, resolver))?
                .nodes()
                .len()
        }
        3 => {
            let mut parser = PushParser::with_options(loading(limits, base, resolver)).unwrap();
            let mut count = |_: &yaxml::Event<'_>, _| {
                events += 1;
                ControlFlow::Continue(())
            };
            for chunk in input.chunks(7) {
                parser.feed(chunk, &mut count)?;
            }
            parser.finish(&mut count)?;
        }
        _ => {
            let source = BufReader::with_capacity(11, Cursor::new(input));
            let mut reader =
                BufferedReader::with_options(source, loading(limits, base, resolver)).unwrap();
            loop {
                match reader.read_event() {
                    Ok(Some(_)) => events += 1,
                    Ok(None) => break,
                    Err(ReadError::Xml(error)) => return Err(error),
                    Err(error) => panic!("in-memory buffered input failed: {error}"),
                }
            }
        }
    }
    Ok(events)
}

/// Returns the reader's error after checking every other surface against it,
/// then validates both resolved DOMs.
pub(super) fn exercise(
    document: &[u8],
    resources: &[Vec<u8>],
    limits: Limits,
    policy: Policy,
) -> Option<Error> {
    let run = |index| {
        let log = Log::default();
        let result = surface(index, document, limits, resolver(resources, policy, &log));
        let requests = log.lock().unwrap().clone();
        assert!(
            requests.len() <= limits.max_external_loads,
            "load budget ignored"
        );
        assert!(
            requests.windows(2).all(|pair| pair[1].1 <= pair[0].1)
                && requests
                    .iter()
                    .all(|(_, left)| *left <= limits.max_external_bytes),
            "byte budget is not cumulative: {requests:?}"
        );
        if let Err(error) = &result {
            super::check_error(*error, document.len());
        }
        (result, requests)
    };
    // The slice reader and borrowed DOM take UTF-8 only; recognized encodings
    // compare the decoding surfaces against the push parser instead.
    let decoded = super::encoding::recognizes(document);
    let (reference, others): (usize, &[usize]) = match decoded {
        true => (3, &[2, 4]),
        false => (0, &[1, 2, 3, 4]),
    };
    let (expected, requests) = run(reference);
    // The DOMs count nodes rather than events; acceptance, error kind and
    // the request sequence must agree. Under token and owned-byte ceilings,
    // streaming surfaces retain names the slice reader borrows and meet a
    // different first limit, so only acceptance is compared there.
    let strict = limits.max_token_bytes == LIMITS.max_token_bytes
        && limits.max_owned_bytes == LIMITS.max_owned_bytes;
    let owned = |result: &Result<usize, Error>| matches!(result, Err(error) if error.kind == ErrorKind::LimitExceeded(yaxml::Limit::OwnedBytes));
    for &index in others {
        let (result, made) = run(index);
        if !strict {
            assert!(
                owned(&result) || owned(&expected) || result.is_ok() == expected.is_ok(),
                "surface {index} acceptance differs for {document:?}"
            );
            continue;
        }
        assert_eq!(
            (result.err().map(|error| error.kind), &made),
            (expected.err().map(|error| error.kind), &requests),
            "surface {index} disagrees for {document:?}"
        );
    }
    if expected.is_ok() && !decoded {
        let log = Log::default();
        let borrowed = Document::parse_with(
            document,
            loading(limits, "d/doc.xml", resolver(resources, policy, &log)),
        );
        let owned = OwnedDocument::parse_with(
            document,
            loading(limits, "d/doc.xml", resolver(resources, policy, &log)),
        );
        if let (Ok(borrowed), Ok(owned)) = (borrowed, owned) {
            super::valid::compare(&borrowed, &owned, None);
        }
    }
    expected.err()
}

/// `valid::Dtd::load` of resource `s` through the resolver; the document is
/// validated against it over both DOMs. Loading is deterministic.
pub(super) fn exercise_load(
    document: &[u8],
    resources: &[Vec<u8>],
    limits: Limits,
    policy: Policy,
) -> Option<Error> {
    let load = || {
        let log = Log::default();
        let dtd = Dtd::load("d/s", limits, resolver(resources, policy, &log));
        let requests = log.lock().unwrap().len();
        assert!(requests <= limits.max_external_loads, "load budget ignored");
        (dtd, requests)
    };
    let (dtd, requests) = load();
    let (again, repeated) = load();
    assert_eq!(requests, repeated, "Dtd::load requests differ");
    assert_eq!(dtd.as_ref().err(), again.as_ref().err());
    let dtd = match dtd {
        Ok(dtd) => dtd,
        Err(error) => {
            super::check_display(&error);
            return Some(error);
        }
    };
    if let (Ok(borrowed), Ok(owned)) = (
        Document::parse_bytes(document),
        OwnedDocument::parse_bytes(document),
    ) {
        super::valid::compare(&borrowed, &owned, Some(&dtd));
    }
    None
}

/// Cancels a push parse at event `stop` (fed in `width`-byte chunks): the
/// failure is `Aborted` at the end of the delivered event and stays sticky.
pub(super) fn cancel(input: &[u8], limits: Limits, stop: usize, width: usize) {
    let mut parser = PushParser::with_limits(limits);
    let mut seen = 0;
    let mut end = None;
    let mut callback = |_: &yaxml::Event<'_>, origin: yaxml::SourceOrigin| {
        seen += 1;
        if seen > stop {
            end = Some(super::span(origin).end);
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    };
    let mut result = Ok(());
    for chunk in input.chunks(width.max(1)) {
        result = parser.feed(chunk, &mut callback);
        if result.is_err() {
            break;
        }
    }
    if result.is_ok() {
        result = parser.finish(&mut callback);
    }
    if let Err(error) = result {
        super::check_error(error, input.len());
        if end.is_some() {
            assert_eq!(error.kind, ErrorKind::Aborted, "cancellation was ignored");
        }
        assert_eq!(parser.feed(&[], |_, _| panic!("after failure")), Err(error));
        assert_eq!(parser.finish(|_, _| panic!("after failure")), Err(error));
    } else {
        assert!(end.is_none(), "cancellation was ignored");
    }
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
