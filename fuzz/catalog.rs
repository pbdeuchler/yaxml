//! XML and SGML catalogs over fuzz-provided files. A testcase is
//! NUL-separated: the root catalog (`d/root`), then files served as `d/0`..`d/9`.
//! Queries are fixed identifiers plus every line of the last piece: `p…`
//! public, `s…` system, `u…` URI, otherwise `public|system`. The resolver
//! fails with an I/O error for `d/err`. The queries also run against
//! `Catalog::load` of `d/0` and under small input-derived limits.
//!
//! Lines of the last piece starting `+` or `-` also edit the catalogs: `+type|orig|replace`
//! adds (`orig` `-` is none), `-value` removes. The edited root catalog dumps
//! to a catalog that loads (XML) and dumps the same once it has been loaded
//! once; edits cleaned to absolute names against an empty catalog dump, load
//! and answer exactly as the edited catalog.

use std::sync::{Arc, Mutex};

use yaxml::catalog::{Catalog, CatalogResolver, Limits};
use yaxml::{Document, Request, Resolver};

const LIMITS: Limits = Limits::DEFAULT
    .max_loads(16)
    .max_bytes(1 << 16)
    .max_depth(50);

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
    Limits::DEFAULT
        .max_loads((hash & 3) as usize)
        .max_bytes((hash >> 2 & 1023) as usize)
        .max_depth((hash >> 12 & 3) as usize)
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

/// The edit a `+` or `-` line asks for, applied; whether the catalog took it.
fn edit(catalog: &mut Catalog, line: &str) {
    if let Some(add) = line.strip_prefix('+') {
        let mut fields = add.splitn(3, '|');
        let (kind, orig, replace) = (fields.next(), fields.next(), fields.next());
        let orig = orig.filter(|orig| *orig != "-");
        let added = catalog.add(kind.unwrap_or(""), orig, replace.unwrap_or(""));
        assert!(added.is_ok() || added == Err(yaxml::catalog::Error::Rejected));
    } else if let Some(value) = line.strip_prefix('-') {
        catalog.remove(value);
    }
}

/// `text` restricted to characters every catalog syntax and URI takes as they are.
fn clean(text: &str) -> String {
    (text.chars())
        .filter(|c| c.is_ascii_alphanumeric() || "./_-".contains(*c))
        .take(24)
        .collect()
}

/// An empty XML catalog and an empty SGML one.
fn empty(sgml: bool) -> Catalog {
    let text: &[u8] = if sgml {
        b""
    } else {
        b"<catalog xmlns='urn:oasis:names:tc:entity:xmlns:xml:catalog'/>"
    };
    Catalog::parse(
        text,
        Limits::DEFAULT,
        "d/dump",
        resolver(&[], &Log::default()),
    )
    .unwrap()
}

/// Edits cleaned to absolute names dump, load and answer like the edited
/// catalog (XML: every entry type; SGML: `PUBLIC` and `SYSTEM`, the entries a
/// catalog keeps when it loads).
fn strict_round_trip(lines: &[&str], sgml: bool) {
    const XML: [&str; 9] = [
        "public",
        "system",
        "rewriteSystem",
        "delegatePublic",
        "delegateSystem",
        "uri",
        "rewriteURI",
        "delegateURI",
        "nextCatalog",
    ];
    let mut catalog = empty(sgml);
    let mut names = vec!["p-//A//DTD X//EN".to_owned(), "shttp://e/x.dtd".to_owned()];
    for (index, line) in lines.iter().enumerate() {
        let line = if let Some(add) = line.strip_prefix('+') {
            let mut fields = add.splitn(3, '|');
            let kind = fields.next().unwrap_or("").len();
            let (orig, replace) = (fields.next().unwrap_or(""), fields.next().unwrap_or(""));
            // Loading drops an SGML `PUBLIC` that normalizes to nothing.
            let orig = if sgml && clean(orig).is_empty() {
                "x"
            } else {
                orig
            };
            let kind = if sgml {
                ["PUBLIC", "SYSTEM"][kind % 2]
            } else {
                XML[kind % XML.len()]
            };
            names.push(format!("s{}", clean(orig)));
            names.push(format!("p{}", clean(orig)));
            names.push(format!("u{}", clean(orig)));
            // Distinct replacements: loading drops a repeated `nextCatalog`.
            format!(
                "+{kind}|{}|http://h/{}-{index}",
                clean(orig),
                clean(replace)
            )
        } else {
            format!("-{}", clean(&line[line.len().min(1)..]))
        };
        edit(&mut catalog, &line);
    }
    let dumped = catalog.dump();
    let reloaded = Catalog::parse(
        dumped.as_bytes(),
        Limits::DEFAULT,
        "d/dump",
        resolver(&[], &Log::default()),
    );
    let Ok(mut reloaded) = reloaded else {
        panic!("a dump of edits does not load: {dumped}");
    };
    assert_eq!(reloaded.dump(), dumped);
    for name in names {
        let query = query(&name);
        let ask = |catalog: &mut Catalog| match query {
            Query::Id(public, system) => catalog.resolve(public, system),
            Query::Uri(uri) => catalog.resolve_uri(uri),
        };
        assert_eq!(ask(&mut catalog), ask(&mut reloaded), "{name} in {dumped}");
    }
}

/// Every edit line, on the root catalog and cleaned on empty ones.
fn editing(root: &[u8], resources: &[&[u8]], lines: &[&str]) {
    let edits: Vec<&str> = (lines.iter().copied())
        .filter(|line| line.starts_with(['+', '-']))
        .take(32)
        .collect();
    strict_round_trip(&edits, false);
    strict_round_trip(&edits, true);
    let log = Log::default();
    let Ok(mut catalog) = Catalog::parse(root, LIMITS, "d/root", resolver(resources, &log)) else {
        return;
    };
    // Lookups first, so the dump sees what they loaded and what failed to.
    lines.iter().for_each(|line| drop(ask(&mut catalog, line)));
    edits.iter().for_each(|line| edit(&mut catalog, line));
    let dumped = catalog.dump();
    assert_eq!(catalog.dump(), dumped);
    if dumped.starts_with("<?xml") {
        // Escaping keeps any value loadable, and loading drops only what does
        // not resolve, so a second round trip changes nothing.
        let load = |text: &str| {
            Catalog::parse(
                text.as_bytes(),
                LIMITS,
                "d/dump",
                resolver(&[], &Log::default()),
            )
        };
        let once = load(&dumped)
            .unwrap_or_else(|error| panic!("{error}: {dumped}"))
            .dump();
        assert_eq!(load(&once).unwrap().dump(), once);
    }
}

fn ask(catalog: &mut Catalog, line: &str) -> Option<String> {
    match query(line) {
        Query::Id(public, system) => catalog.resolve(public, system),
        Query::Uri(uri) => catalog.resolve_uri(uri),
    }
}

pub fn exercise(input: &[u8]) {
    let mut pieces = input.split(|byte| *byte == 0);
    let root = pieces.next().unwrap_or_default();
    let resources: Vec<&[u8]> = pieces.collect();
    let text = String::from_utf8_lossy(resources.last().copied().unwrap_or(root));
    let lines: Vec<&str> = FIXED.iter().copied().chain(text.lines().take(32)).collect();
    editing(root, &resources, &lines);
    let queries: Vec<Query<'_>> = lines.iter().map(|line| query(line)).collect();
    let (answers, requests) = run(Some(root), &resources, &queries, LIMITS);
    // Resolution is deterministic, lazy loading included.
    assert_eq!(
        run(Some(root), &resources, &queries, LIMITS),
        (answers, requests)
    );
    run(Some(root), &resources, &queries, tight(input));
    let defaults = Limits::default()
        .max_loads(LIMITS.max_loads)
        .max_bytes(LIMITS.max_bytes);
    run(None, &resources, &queries, defaults);
    // Loading the root itself can fail in the resolver or on the load budget.
    let failed = |uri, limits| Catalog::load(uri, limits, resolver(&resources, &Log::default()));
    let io = failed("d/err", defaults).map(drop);
    assert!(matches!(io, Err(yaxml::catalog::Error::Io(_))), "{io:?}");
    let none = defaults.max_loads(0);
    let limit = failed("d/0", none).map(drop);
    assert_eq!(limit, Err(yaxml::catalog::Error::LimitExceeded));
    for error in [io, limit] {
        assert!(!error.unwrap_err().to_string().is_empty());
    }

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
