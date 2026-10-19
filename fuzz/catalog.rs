//! XML and SGML catalogs over fuzz-provided files. A testcase is
//! NUL-separated: the root catalog (`d/root`), then files served as `d/0`..`d/9`.
//! Queries are fixed identifiers plus every line of the last piece: `p…`
//! public, `s…` system, `u…` URI, otherwise `public|system`. The resolver
//! fails with an I/O error for `d/err`. The queries also run against
//! `Catalog::load` of `d/0` and under small input-derived limits.

use std::sync::{Arc, Mutex};

use yaxml::catalog::{Catalog, CatalogResolver, Limits};
use yaxml::{Document, Request, Resolver};

const LIMITS: Limits = Limits {
    max_loads: 16,
    max_bytes: 1 << 16,
    max_depth: 50,
};

const NAMES: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

const FIXED: [&str; 7] = [
    "p-//A//DTD X//EN",
    "shttp://e/x.dtd",
    "uhttp://e/x",
    "purn:publicid:-:A:DTD+X:EN",
    "p-//A//DTD X//EN|http://e/x.dtd",
    "x",
    "uurn:publicid:-:A:DTD+X:EN",
];

/// Requested URIs, response lengths and the byte budget each request carried.
type Requests = Vec<(String, usize, usize)>;
type Log = Arc<Mutex<Requests>>;

fn resolver(resources: &[&[u8]], log: &Log) -> Box<dyn Resolver> {
    let resources: Vec<Vec<u8>> = resources.iter().map(|piece| piece.to_vec()).collect();
    let log = log.clone();
    Box::new(move |request: Request<'_>| {
        let name = request.uri.strip_prefix("d/").unwrap_or("");
        let response = (NAMES.iter().position(|candidate| *candidate == name))
            .and_then(|index| resources.get(index).cloned());
        log.lock().unwrap().push((
            request.uri.to_owned(),
            response.as_ref().map_or(0, Vec::len),
            request.max_bytes,
        ));
        if name == "err" {
            return Err(std::io::Error::other("fuzz resolver failure"));
        }
        Ok(response)
    })
}

enum Query<'a> {
    Id(Option<&'a str>, Option<&'a str>),
    Uri(&'a str),
}

fn query(line: &str) -> Query<'_> {
    let rest = line.get(1..).unwrap_or("");
    match line.as_bytes().first() {
        Some(b'p') if !rest.contains('|') => Query::Id(Some(rest), None),
        Some(b's') => Query::Id(None, Some(rest)),
        Some(b'u') => Query::Uri(rest),
        _ => {
            let line = line.strip_prefix('p').unwrap_or(line);
            let (public, system) = line.split_once('|').unwrap_or((line, line));
            Query::Id(Some(public), Some(system))
        }
    }
}

/// Small load, byte and depth limits chosen by the input bytes.
fn tight(input: &[u8]) -> Limits {
    let hash = input.iter().fold(0u32, |hash, &byte| {
        hash.wrapping_mul(31).wrapping_add(byte.into())
    });
    Limits {
        max_loads: (hash & 3) as usize,
        max_bytes: (hash >> 2 & 1023) as usize,
        max_depth: (hash >> 12 & 3) as usize,
    }
}

/// Every answer in order, and the requests that produced them.
fn run(
    root: Option<&[u8]>,
    resources: &[&[u8]],
    queries: &[Query<'_>],
    limits: Limits,
) -> (Vec<Option<String>>, Requests) {
    let log = Log::default();
    let built = match root {
        Some(root) => Catalog::parse(root, limits, "d/root", resolver(resources, &log)),
        None => Catalog::load("d/0", limits, resolver(resources, &log)),
    };
    let answers = match built {
        Ok(mut catalog) => {
            assert!(format!("{catalog:?}").starts_with("Catalog"));
            assert_eq!(catalog.resolve(None, None), None);
            (queries.iter())
                .map(|query| match *query {
                    Query::Id(public, system) => catalog.resolve(public, system),
                    Query::Uri(uri) => catalog.resolve_uri(uri),
                })
                .collect()
        }
        Err(error) => {
            assert!(!error.to_string().is_empty());
            Vec::new()
        }
    };
    let requests = log.lock().unwrap().clone();
    // Each request offers what earlier accepted responses left; a larger
    // response is rejected and spends nothing.
    let mut left = limits.max_bytes;
    for (_, length, offered) in &requests {
        assert_eq!(
            *offered, left,
            "byte budget is not cumulative: {requests:?}"
        );
        if length <= offered {
            left -= length;
        }
    }
    assert!(
        requests.len() <= limits.max_loads,
        "load budget ignored: {requests:?}"
    );
    (answers, requests)
}

pub fn exercise(input: &[u8]) {
    let mut pieces = input.split(|byte| *byte == 0);
    let root = pieces.next().unwrap_or_default();
    let resources: Vec<&[u8]> = pieces.collect();
    let text = String::from_utf8_lossy(resources.last().copied().unwrap_or(root));
    let lines: Vec<&str> = FIXED.iter().copied().chain(text.lines().take(32)).collect();
    let queries: Vec<Query<'_>> = lines.iter().map(|line| query(line)).collect();
    let (answers, requests) = run(Some(root), &resources, &queries, LIMITS);
    // Resolution is deterministic, lazy loading included.
    assert_eq!(
        run(Some(root), &resources, &queries, LIMITS),
        (answers, requests)
    );
    run(Some(root), &resources, &queries, tight(input));
    let defaults = Limits {
        max_loads: LIMITS.max_loads,
        max_bytes: LIMITS.max_bytes,
        ..Limits::default()
    };
    run(None, &resources, &queries, defaults);

    // The adapter asks the catalog exactly as libxml2's entity loader does.
    let Some(Query::Id(Some(public), Some(system))) = queries.get(4) else {
        unreachable!("fixed query")
    };
    let (Ok(catalog), Ok(mut twin)) = (
        Catalog::parse(
            root,
            LIMITS,
            "d/root",
            resolver(&resources, &Log::default()),
        ),
        Catalog::parse(
            root,
            LIMITS,
            "d/root",
            resolver(&resources, &Log::default()),
        ),
    ) else {
        return;
    };
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    let inner = move |request: Request<'_>| {
        record.lock().unwrap().push(request.uri.to_owned());
        Ok(None)
    };
    let document = format!("<!DOCTYPE r PUBLIC '{public}' '{system}'><r/>");
    let resolver = CatalogResolver::new(catalog, Box::new(inner));
    if Document::parse_with(
        document.as_bytes(),
        loading(Default::default(), "", Box::new(resolver)),
    )
    .is_ok()
    {
        let expected = (twin.resolve(Some(public), Some(system)))
            .or_else(|| twin.resolve_uri(system))
            .unwrap_or_else(|| (*system).to_owned());
        assert_eq!(*seen.lock().unwrap(), [expected]);
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
