//! XML and SGML catalogs over fuzz-provided files. A testcase is
//! NUL-separated: the root catalog (`d/root`), then files served as `d/0`..`d/9`.
//! Queries are fixed identifiers plus every line of the last piece: `p…`
//! public, `s…` system, `u…` URI, otherwise `public|system`.

use std::sync::{Arc, Mutex};

use yaxml::catalog::{Catalog, CatalogResolver, Limits};
use yaxml::{Document, Request, Resolver};

const LIMITS: Limits = Limits {
    max_loads: 16,
    max_bytes: 1 << 16,
    max_depth: 50,
};

const NAMES: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

const FIXED: [&str; 6] = [
    "p-//A//DTD X//EN",
    "shttp://e/x.dtd",
    "uhttp://e/x",
    "purn:publicid:-:A:DTD+X:EN",
    "p-//A//DTD X//EN|http://e/x.dtd",
    "x",
];

/// Requested URIs and response lengths.
type Log = Arc<Mutex<Vec<(String, usize)>>>;

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
        ));
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

/// Every answer in order, and the requests that produced them.
fn run(
    root: &[u8],
    resources: &[&[u8]],
    queries: &[Query<'_>],
) -> (Vec<Option<String>>, Vec<(String, usize)>) {
    let log = Log::default();
    let answers = match Catalog::parse(root, LIMITS, "d/root", resolver(resources, &log)) {
        Ok(mut catalog) => {
            assert_eq!(catalog.resolve(None, None), None);
            (queries.iter())
                .map(|query| match *query {
                    Query::Id(public, system) => catalog.resolve(public, system),
                    Query::Uri(uri) => catalog.resolve_uri(uri),
                })
                .collect()
        }
        Err(_) => Vec::new(),
    };
    let requests = log.lock().unwrap().clone();
    // Only the response that exhausts the byte budget may exceed it.
    let served: usize = requests
        .iter()
        .rev()
        .skip(1)
        .map(|(_, length)| length)
        .sum();
    assert!(
        requests.len() <= LIMITS.max_loads && served <= LIMITS.max_bytes,
        "budgets ignored: {requests:?}"
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
    let (answers, requests) = run(root, &resources, &queries);
    // Resolution is deterministic, lazy loading included.
    assert_eq!(run(root, &resources, &queries), (answers, requests));

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
    if Document::with_resolver(
        document.as_bytes(),
        Default::default(),
        "",
        Box::new(resolver),
    )
    .is_ok()
    {
        let expected = (twin.resolve(Some(public), Some(system)))
            .or_else(|| twin.resolve_uri(system))
            .unwrap_or_else(|| (*system).to_owned());
        assert_eq!(*seen.lock().unwrap(), [expected]);
    }
}
