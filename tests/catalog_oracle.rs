//! Differential test against `tooling/libxml2_catalog.c` (libxml2 2.15.4) on
//! generated XML and SGML catalog sets. Runs only when
//! `YAXML_CATALOG_HELPER` names the compiled helper; see `docs/catalog.md`.
#![forbid(unsafe_code)]

#[allow(dead_code)]
#[path = "../examples/support/process.rs"]
mod process;

use std::fmt::Write;
use std::path::Path;
use std::time::Duration;

use process::ProcessLimits;
use yaxml::FileResolver;
use yaxml::catalog::{Catalog, Limits};

const CASES: usize = 400;
const QUERIES: usize = 12;

const PUBLIC: &[&str] = &[
    "-//A//DTD X//EN",
    " -//A//DTD  X//EN ",
    "-//A//DTD\tX//EN",
    "-//A//DTD Y//EN",
    "-//A//",
    "-//B//ENT Z//EN",
    "toto",
    "",
    "urn:publicid:-:A:DTD+X:EN",
    "urn:publicid:-:A:DTD+Y:EN+",
    "urn:publicid:-:B:ENT+Z%3AQ:EN",
    "urn:publicid:urn%3Apublicid%3A-%3AA%3ADTD+X%3AEN",
];
const SYSTEM: &[&str] = &[
    "http://ex.org/a/x.dtd",
    "http://ex.org/a/",
    "http://ex.org/b/y.dtd",
    "http://ex.org/",
    "x.dtd",
    "toto",
    "urn:publicid:-:A:DTD+X:EN",
    "urn:publicid:-:A:DTD+Y:EN",
    "file:///tmp/z.dtd",
    "",
];
const PREFIX: &[&str] = &[
    "http://ex.org/",
    "http://ex.org/a/",
    "http://ex.org/a/x",
    "http://ex.org/b",
    "-//A//",
    "-//A//DTD",
    " -//B// ",
    "",
    "toto",
];
const TARGET: &[&str] = &[
    "t1.dtd",
    "/abs/t2.dtd",
    "http://m.org/t3.dtd",
    "sub/t4/",
    "../up.dtd",
    "file:///f/",
    "t 5.dtd",
];
const FILES: &[&str] = &["a.xml", "b.xml", "sub/c.xml"];
const REFS: &[&str] = &[
    "a.xml",
    "b.xml",
    "sub/c.xml",
    "c.xml",
    "root.xml",
    "missing.xml",
    "./a.xml",
    "../root.xml",
    "s.sgml",
];
const BASES: &[&str] = &["sub/", "../", "http://base.org/d/", "x/y.xml", "urn:z:"];

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as usize % n
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

fn attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\t', "&#9;")
        .replace('<', "&lt;")
}

fn entry(rng: &mut Rng, depth: usize) -> String {
    let base = match rng.chance(15) {
        true => format!(" xml:base=\"{}\"", rng.pick(BASES)),
        false => String::new(),
    };
    let (element, name, value) = match rng.below(12) {
        0 => ("public", "publicId", "uri"),
        1 => ("system", "systemId", "uri"),
        2 => ("rewriteSystem", "systemIdStartString", "rewritePrefix"),
        3 => ("delegatePublic", "publicIdStartString", "catalog"),
        4 => ("delegateSystem", "systemIdStartString", "catalog"),
        5 => ("uri", "name", "uri"),
        6 => ("rewriteURI", "uriStartString", "rewritePrefix"),
        7 => ("delegateURI", "uriStartString", "catalog"),
        8 => ("nextCatalog", "", "catalog"),
        9 if depth < 2 => {
            let prefer = [
                "",
                " prefer='system'",
                " prefer='public'",
                " prefer='bogus'",
            ][rng.below(4)];
            let children: String = (0..rng.below(4)).map(|_| entry(rng, depth + 1)).collect();
            return format!("<group{prefer}{base}>{children}</group>");
        }
        10 => return "<other xmlns='urn:x' uri='x'/>".into(),
        _ => ("public", "publicId", "uri"),
    };
    let key = match name {
        "" => String::new(),
        _ if rng.chance(5) => String::new(),
        _ => {
            let pool = match (element, name) {
                ("public", _) => PUBLIC,
                ("system", _) | ("uri", _) => SYSTEM,
                _ => PREFIX,
            };
            format!(" {name}=\"{}\"", attribute(rng.pick(pool)))
        }
    };
    let target = match value {
        "catalog" => rng.pick(REFS),
        _ => rng.pick(TARGET),
    };
    format!("<{element}{key} {value}=\"{}\"{base}/>", attribute(target))
}

fn xml_catalog(rng: &mut Rng) -> String {
    let prefer = ["", " prefer='system'", " prefer='public'", " prefer='x'"][rng.below(4)];
    let entries: String = (0..rng.below(14)).map(|_| entry(rng, 0)).collect();
    let text = format!(
        "<?xml version='1.0'?>\n<catalog xmlns='urn:oasis:names:tc:entity:xmlns:xml:catalog'{prefer}>{entries}</catalog>"
    );
    match rng.chance(3) {
        true => text[..text.len() / 2].to_owned(),
        false => text,
    }
}

fn sgml_catalog(rng: &mut Rng) -> String {
    (0..rng.below(14))
        .map(|_| match rng.below(10) {
            0 | 1 => format!("PUBLIC \"{}\" \"{}\"\n", rng.pick(PUBLIC), rng.pick(TARGET)),
            2 => format!(
                "SYSTEM \"{}\" {}\n",
                rng.pick(SYSTEM),
                rng.pick(&["t1.dtd", "x.dtd"])
            ),
            3 => format!(
                "CATALOG \"{}\"\n",
                rng.pick(&["s.sgml", "t.sgml", "root.sgml", "no.sgml"])
            ),
            4 => format!("BASE \"{}\"\n", rng.pick(BASES)),
            5 => "DELEGATE \"-//A//\" \"a.sgml\"\n".into(),
            6 => "ENTITY e \"x.ent\" DOCTYPE d \"d.dtd\"\n".into(),
            7 => "-- comment -- OVERRIDE YES\n".into(),
            8 => format!("PUBLIC '{}' '{}'", rng.pick(PUBLIC), rng.pick(TARGET)),
            _ => format!(
                "SYSTEM {} {}\n",
                rng.pick(&["x.dtd", "toto"]),
                rng.pick(TARGET)
            ),
        })
        .collect()
}

fn encode(value: Option<&str>) -> String {
    match value {
        None => "-".into(),
        Some("") => "0".into(),
        Some(value) => value.bytes().fold(String::new(), |mut out, byte| {
            write!(out, "{byte:02x}").unwrap();
            out
        }),
    }
}

fn decode(field: &str) -> Option<String> {
    match field {
        "-" => None,
        "0" => Some(String::new()),
        _ => Some(
            String::from_utf8(
                (0..field.len() / 2)
                    .map(|i| u8::from_str_radix(&field[2 * i..2 * i + 2], 16).unwrap())
                    .collect(),
            )
            .unwrap(),
        ),
    }
}

enum Query {
    Resolve(Option<String>, Option<String>),
    Uri(String),
}

fn query(rng: &mut Rng) -> Query {
    if rng.below(4) == 0 {
        return Query::Uri(rng.pick(&[SYSTEM, PUBLIC].concat()).to_owned());
    }
    let mut either = |pool: &[&str]| rng.chance(70).then(|| rng.pick(pool).to_owned());
    Query::Resolve(either(PUBLIC), either(SYSTEM))
}

#[test]
fn generated_catalogs_match_libxml2() {
    let Some(helper) = std::env::var_os("YAXML_CATALOG_HELPER") else {
        eprintln!("YAXML_CATALOG_HELPER unset; skipping the libxml2 differential");
        return;
    };
    let directory =
        std::env::temp_dir().join(format!("yaxml-catalog-oracle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    let mut rng = Rng(0x5eed);
    let mut input = String::new();
    let mut cases = Vec::new();
    for case in 0..CASES {
        let root = directory.join(format!("c{case}"));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let sgml = case % 4 == 3;
        let root_name = if sgml { "root.sgml" } else { "root.xml" };
        let write = |name: &str, text: String| std::fs::write(root.join(name), text).unwrap();
        if sgml {
            for name in ["root.sgml", "s.sgml", "t.sgml"] {
                write(name, sgml_catalog(&mut rng));
            }
        } else {
            write("root.xml", xml_catalog(&mut rng));
            for name in FILES {
                write(name, xml_catalog(&mut rng));
            }
            write("s.sgml", "PUBLIC \"-//A//DTD X//EN\" \"s.dtd\"\n".into());
        }
        let url = format!("c{case}/{root_name}");
        writeln!(input, "load\t{}", encode(Some(&url))).unwrap();
        let queries: Vec<Query> = (0..QUERIES).map(|_| query(&mut rng)).collect();
        for query in &queries {
            match query {
                Query::Resolve(public, system) => {
                    let (public, system) = (encode(public.as_deref()), encode(system.as_deref()));
                    writeln!(input, "res\t{public}\t{system}").unwrap();
                }
                Query::Uri(uri) => writeln!(input, "uri\t{}", encode(Some(uri))).unwrap(),
            }
        }
        cases.push((url, queries));
    }
    let limits = ProcessLimits {
        timeout: Duration::from_secs(120),
        max_output_bytes: 1 << 24,
    };
    let output = process::run_with_environment(
        Path::new(&helper),
        &[],
        input.as_bytes(),
        limits,
        &[],
        &[],
        Some(&directory),
    )
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut lines = stdout.lines();
    let (mut compared, mut found, mut mismatches) = (0, 0, Vec::new());
    for (url, queries) in &cases {
        lines.next().unwrap();
        let resolver = FileResolver::new(&directory).unwrap();
        let mut catalog = Catalog::load(url, Limits::default(), Box::new(resolver)).ok();
        for query in queries {
            let ours = catalog.as_mut().and_then(|catalog| match query {
                Query::Resolve(public, system) => {
                    catalog.resolve(public.as_deref(), system.as_deref())
                }
                Query::Uri(uri) => catalog.resolve_uri(uri),
            });
            let theirs = decode(lines.next().unwrap());
            compared += 1;
            found += usize::from(theirs.is_some());
            if ours != theirs {
                let query = match query {
                    Query::Resolve(public, system) => format!("resolve {public:?} {system:?}"),
                    Query::Uri(uri) => format!("uri {uri:?}"),
                };
                mismatches.push(format!("{url} {query}: yaxml {ours:?}, libxml2 {theirs:?}"));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&directory);
    eprintln!(
        "catalog differential: {compared} queries over {CASES} catalog sets, {found} resolved by libxml2, {} mismatches",
        mismatches.len()
    );
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
