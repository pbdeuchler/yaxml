#![forbid(unsafe_code)]
//! XML and SGML catalogs: libxml2 2.15.4's `test/catalogs` suite through an
//! `xmlcatalog --shell` emulation, and every entry type and traversal rule.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use yaxml::catalog::{Catalog, CatalogResolver, Error, Limits};
use yaxml::{Document, FileResolver, Request, Resolver, ResourceKind};

const DEFAULT_SOURCE: &str =
    "/Users/pbdeuchler/workspace/src/github.com/pbdeuchler/yaxml-oracle/libxml2-2.15.4";

/// `xmlcatalog --shell` for the commands the suite uses.
fn shell(catalog: &mut Catalog, script: &str) -> String {
    let mut out = String::new();
    for line in script.lines() {
        out.push_str("> ");
        let line = line.trim_start_matches([' ', '\t']);
        let (command, mut rest) = line.split_once([' ', '\t']).unwrap_or((line, ""));
        let mut args = Vec::new();
        while !rest.is_empty() {
            rest = rest.trim_start_matches([' ', '\t']);
            match rest.chars().next() {
                Some(quote @ ('\'' | '"')) => match rest[1..].split_once(quote) {
                    Some((arg, tail)) => (args.push(arg), rest = tail).1,
                    None => rest = "",
                },
                Some(_) => {
                    let (arg, tail) = rest.split_once([' ', '\t']).unwrap_or((rest, ""));
                    (args.push(arg), rest = tail).1
                }
                None => {}
            }
        }
        let answer = match (command, args.as_slice()) {
            ("public", [id]) => catalog
                .resolve(Some(id), None)
                .unwrap_or_else(|| format!("No entry for PUBLIC {id}")),
            ("system", [id]) => catalog
                .resolve(None, Some(id))
                .unwrap_or_else(|| format!("No entry for SYSTEM {id}")),
            ("resolve", [public, system]) => catalog
                .resolve(Some(public), Some(system))
                .unwrap_or_else(|| "Resolver failed to find an answer".into()),
            _ => panic!("unsupported shell line {line:?}"),
        };
        out.push_str(&answer);
        out.push('\n');
    }
    out + "> "
}

#[test]
fn libxml2_catalog_suite() {
    let source = std::env::var_os("YAXML_LIBXML2_SRC").map_or(DEFAULT_SOURCE.into(), PathBuf::from);
    if !source.exists() {
        eprintln!("libxml2 source missing; skipping the catalog suite");
        return;
    }
    let mut passed = 0;
    let mut scripts: Vec<_> = std::fs::read_dir(source.join("test/catalogs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "script")
        })
        .collect();
    scripts.sort();
    for script in scripts {
        let name = script.file_stem().unwrap().to_str().unwrap();
        for extension in ["xml", "sgml"] {
            let url = format!("./test/catalogs/{name}.{extension}");
            if !source.join(&url).exists() {
                continue;
            }
            let resolver = FileResolver::new(&source).unwrap();
            let mut catalog = Catalog::load(&url, Limits::default(), Box::new(resolver)).unwrap();
            let expected =
                std::fs::read_to_string(source.join("result/catalogs").join(name)).unwrap();
            let script = std::fs::read_to_string(&script).unwrap();
            assert_eq!(shell(&mut catalog, &script), expected, "{url}");
            passed += 1;
        }
    }
    eprintln!("libxml2 catalog suite: {passed} of 6 passed");
    assert_eq!(passed, 6);
}

type Log = Arc<Mutex<Vec<(ResourceKind, String)>>>;

/// Serves `files` by URI and records every request.
fn served(files: &[(&str, &str)]) -> (Box<dyn Resolver>, Log) {
    let files: HashMap<String, Vec<u8>> = files
        .iter()
        .map(|(uri, text)| (uri.to_string(), text.as_bytes().to_vec()))
        .collect();
    let log = Log::default();
    let record = log.clone();
    let resolver = move |request: Request<'_>| {
        record
            .lock()
            .unwrap()
            .push((request.kind, request.uri.to_owned()));
        Ok(files.get(request.uri).cloned())
    };
    (Box::new(resolver), log)
}

fn xml(body: &str) -> String {
    format!("<catalog xmlns='urn:oasis:names:tc:entity:xmlns:xml:catalog'>{body}</catalog>")
}

fn catalog(files: &[(&str, &str)]) -> (Catalog, Log) {
    let (resolver, log) = served(files);
    (
        Catalog::load(files[0].0, Limits::default(), resolver).unwrap(),
        log,
    )
}

enum Ask<'a> {
    Id(Option<&'a str>, Option<&'a str>),
    Uri(&'a str),
}

fn ask(catalog: &mut Catalog, query: &Ask<'_>) -> Option<String> {
    match *query {
        Ask::Id(public, system) => catalog.resolve(public, system),
        Ask::Uri(uri) => catalog.resolve_uri(uri),
    }
}

#[test]
fn xml_entries_resolve_like_libxml2() {
    use Ask::{Id, Uri};
    let cases: &[(&str, &str, Ask<'_>, Option<&str>)] = &[
        (
            "public",
            "<public publicId='-//A//X' uri='x.dtd'/>",
            Id(Some("-//A//X"), None),
            Some("d/x.dtd"),
        ),
        (
            "public normalized",
            "<public publicId=' -//A// \tX ' uri='x'/>",
            Id(Some("-//A//  X\n"), None),
            Some("d/x"),
        ),
        (
            "public miss",
            "<public publicId='-//A//X' uri='x'/>",
            Id(Some("-//A//Y"), None),
            None,
        ),
        (
            "system",
            "<system systemId='http://e/x' uri='y'/>",
            Id(None, Some("http://e/x")),
            Some("d/y"),
        ),
        (
            "system is exact",
            "<system systemId='http://e/x' uri='y'/>",
            Id(None, Some("http://e/x ")),
            None,
        ),
        (
            "system before public",
            "<public publicId='p' uri='pub'/><system systemId='s' uri='sys'/>",
            Id(Some("p"), Some("s")),
            Some("d/sys"),
        ),
        (
            "public after system misses",
            "<public publicId='p' uri='pub'/><system systemId='s' uri='sys'/>",
            Id(Some("p"), Some("other")),
            Some("d/pub"),
        ),
        (
            "prefer=system still matches public",
            "<group prefer='system'><public publicId='p' uri='pub'/></group>",
            Id(Some("p"), Some("s")),
            Some("d/pub"),
        ),
        (
            "rewriteSystem longest prefix",
            "<rewriteSystem systemIdStartString='http://e/' rewritePrefix='short/'/><rewriteSystem systemIdStartString='http://e/a/' rewritePrefix='long/'/><rewriteSystem systemIdStartString='http://e/a' rewritePrefix='mid/'/>",
            Id(None, Some("http://e/a/b.dtd")),
            Some("d/long/b.dtd"),
        ),
        (
            "system beats rewrite",
            "<rewriteSystem systemIdStartString='http://e/' rewritePrefix='r/'/><system systemId='http://e/x' uri='s'/>",
            Id(None, Some("http://e/x")),
            Some("d/s"),
        ),
        (
            "empty rewrite prefix never applies",
            "<rewriteSystem systemIdStartString='' rewritePrefix='r/'/>",
            Id(None, Some("x")),
            None,
        ),
        (
            "uri",
            "<uri name='http://e/s.xsl' uri='s.xsl'/>",
            Uri("http://e/s.xsl"),
            Some("d/s.xsl"),
        ),
        (
            "uri ignores system entries",
            "<system systemId='http://e/x' uri='y'/>",
            Uri("http://e/x"),
            None,
        ),
        (
            "resolve ignores uri entries",
            "<uri name='http://e/x' uri='y'/>",
            Id(None, Some("http://e/x")),
            None,
        ),
        (
            "rewriteURI longest prefix",
            "<rewriteURI uriStartString='http://e/' rewritePrefix='a/'/><rewriteURI uriStartString='http://e/x/' rewritePrefix='b/'/>",
            Uri("http://e/x/y"),
            Some("d/b/y"),
        ),
        (
            "absolute target",
            "<public publicId='p' uri='http://m/x.dtd'/>",
            Id(Some("p"), None),
            Some("http://m/x.dtd"),
        ),
        (
            "xml:base on entry",
            "<public publicId='p' uri='x' xml:base='http://b/c/'/>",
            Id(Some("p"), None),
            Some("http://b/c/x"),
        ),
        (
            "xml:base nests",
            "<group xml:base='http://b/'><public publicId='p' uri='x' xml:base='sub/'/></group>",
            Id(Some("p"), None),
            Some("http://b/sub/x"),
        ),
        (
            "relative xml:base",
            "<group xml:base='g/'><public publicId='p' uri='x'/></group>",
            Id(Some("p"), None),
            Some("d/g/x"),
        ),
        (
            "urn public unwrapped",
            "<public publicId='-//A//DTD X//EN' uri='x'/>",
            Id(Some("urn:publicid:-:A:DTD+X:EN"), None),
            Some("d/x"),
        ),
        (
            "urn escapes",
            "<public publicId='a+b:c/d;e%' uri='x'/>",
            Id(Some("urn:publicid:a%2Bb%3Ac%2Fd%3Be%25"), None),
            Some("d/x"),
        ),
        (
            "urn system becomes public",
            "<public publicId='-//A//X' uri='x'/>",
            Id(None, Some("urn:publicid:-:A:X")),
            Some("d/x"),
        ),
        (
            "urn system equal to public",
            "<public publicId='-//A//X' uri='x'/>",
            Id(Some("-//A//X"), Some("urn:publicid:-:A:X")),
            Some("d/x"),
        ),
        (
            "urn system with other public",
            "<public publicId='-//A//X' uri='x'/>",
            Id(Some("other"), Some("urn:publicid:-:A:X")),
            None,
        ),
        (
            "urn uri is a public lookup",
            "<public publicId='-//A//X' uri='x'/>",
            Uri("urn:publicid:-:A:X"),
            Some("d/x"),
        ),
        (
            "blank public is no public",
            "<public publicId='p' uri='x'/>",
            Id(Some(" \t"), None),
            None,
        ),
        (
            "missing attribute",
            "<public uri='x'/><system systemId='s'/>",
            Id(Some("p"), Some("s")),
            None,
        ),
        (
            "attribute in another namespace",
            "<public xmlns:o='urn:o' o:publicId='p' uri='x'/>",
            Id(Some("p"), None),
            Some("d/x"),
        ),
        (
            "foreign element ignored",
            "<x:public xmlns:x='urn:x' publicId='p' uri='x'/>",
            Id(Some("p"), None),
            None,
        ),
        (
            "nested entry ignored",
            "<public publicId='q' uri='q'><public publicId='p' uri='x'/></public>",
            Id(Some("p"), None),
            None,
        ),
        (
            "group members",
            "<group><group><system systemId='s' uri='g'/></group></group>",
            Id(None, Some("s")),
            Some("d/g"),
        ),
        (
            "suffix entries unsupported",
            "<systemSuffix systemIdSuffix='.dtd' uri='x'/>",
            Id(None, Some("a.dtd")),
            None,
        ),
        (
            "nothing asked",
            "<public publicId='p' uri='x'/>",
            Id(None, None),
            None,
        ),
    ];
    for (name, body, query, expected) in cases {
        let (mut catalog, log) = catalog(&[("d/c.xml", &xml(body))]);
        assert_eq!(ask(&mut catalog, query).as_deref(), *expected, "{name}");
        assert_eq!(log.lock().unwrap().len(), 1, "{name}: only the root loads");
    }
}

#[test]
fn delegates_and_next_catalogs_traverse_like_libxml2() {
    use Ask::{Id, Uri};
    let next_a = "<nextCatalog catalog='a.xml'/>";
    let a = xml(
        "<public publicId='-//A//X' uri='a'/><system systemId='http://e/x' uri='a'/><uri name='http://e/x' uri='a'/>",
    );
    let b = xml(
        "<public publicId='-//A//X' uri='b'/><system systemId='http://e/x' uri='b'/><uri name='http://e/x' uri='b'/>",
    );
    let cases: &[(&str, String, Ask<'_>, Option<&str>, usize)] = &[
        (
            "nextCatalog",
            xml(next_a),
            Id(Some("-//A//X"), None),
            Some("d/a"),
            2,
        ),
        (
            "nextCatalog for uri",
            xml(next_a),
            Uri("http://e/x"),
            Some("d/a"),
            2,
        ),
        (
            "local entry first",
            xml(&format!("{next_a}<public publicId='-//A//X' uri='root'/>")),
            Id(Some("-//A//X"), None),
            Some("d/root"),
            1,
        ),
        (
            "next in order",
            xml(
                "<nextCatalog catalog='missing.xml'/><nextCatalog catalog='b.xml'/><nextCatalog catalog='a.xml'/>",
            ),
            Id(Some("-//A//X"), None),
            Some("d/b"),
            3,
        ),
        (
            "repeated nextCatalog loads once",
            xml(
                "<nextCatalog catalog='missing.xml'/><nextCatalog catalog='missing.xml'/><nextCatalog catalog='./missing.xml'/>",
            ),
            Id(Some("x"), None),
            None,
            2,
        ),
        (
            "cycle ends",
            xml("<nextCatalog catalog='c.xml'/><nextCatalog catalog='a.xml'/>"),
            Id(Some("-//A//X"), None),
            Some("d/a"),
            2,
        ),
        (
            "delegatePublic",
            xml("<delegatePublic publicIdStartString='-//A//' catalog='b.xml'/>"),
            Id(Some("-//A//X"), None),
            Some("d/b"),
            2,
        ),
        (
            "delegatePublic needs prefer=public",
            xml(&format!(
                "<group prefer='system'><delegatePublic publicIdStartString='-//A//' catalog='b.xml'/></group>{next_a}"
            )),
            Id(Some("-//A//X"), None),
            Some("d/a"),
            2,
        ),
        (
            "delegate cuts nextCatalog",
            xml(&format!(
                "<delegatePublic publicIdStartString='-//A//' catalog='missing.xml'/>{next_a}"
            )),
            Id(Some("-//A//X"), None),
            None,
            2,
        ),
        (
            "delegates in order",
            xml(
                "<delegateSystem systemIdStartString='http://e/' catalog='missing.xml'/><delegateSystem systemIdStartString='http://e/x' catalog='b.xml'/><delegateSystem systemIdStartString='http://' catalog='a.xml'/>",
            ),
            Id(None, Some("http://e/x")),
            Some("d/b"),
            3,
        ),
        (
            "delegate catalog tried once",
            xml(
                "<delegateSystem systemIdStartString='http://e/' catalog='missing.xml'/><delegateSystem systemIdStartString='http://' catalog='missing.xml'/>",
            ),
            Id(None, Some("http://e/x")),
            None,
            2,
        ),
        (
            "delegate searches system only",
            xml("<delegateSystem systemIdStartString='http://e/' catalog='a.xml'/>"),
            Id(Some("-//A//X"), Some("http://e/y")),
            None,
            2,
        ),
        (
            "delegateURI",
            xml("<delegateURI uriStartString='http://e/' catalog='b.xml'/>"),
            Uri("http://e/x"),
            Some("d/b"),
            2,
        ),
        (
            "uri lookups also use delegateSystem",
            xml(
                "<delegateURI uriStartString='http://' catalog='missing.xml'/><delegateSystem systemIdStartString='http://e/' catalog='b.xml'/>",
            ),
            Uri("http://e/x"),
            Some("d/b"),
            3,
        ),
        (
            "delegateSystem alone does not trigger for uri",
            xml(&format!(
                "<delegateSystem systemIdStartString='http://e/' catalog='b.xml'/>{next_a}"
            )),
            Uri("http://e/x"),
            Some("d/a"),
            2,
        ),
        (
            "self-referencing delegate",
            xml("<delegateURI uriStartString='/foo' catalog='c.xml'/>"),
            Uri("/foo/x"),
            None,
            1,
        ),
        (
            "non-catalog file is broken",
            xml("<nextCatalog catalog='plain.xml'/>"),
            Id(Some("p"), None),
            None,
            2,
        ),
    ];
    let c = |root: &str| {
        let files = [
            ("d/c.xml", root.to_owned()),
            ("d/a.xml", a.clone()),
            ("d/b.xml", b.clone()),
            ("d/plain.xml", "<public publicId='p' uri='x'/>".to_owned()),
        ];
        let files: Vec<(&str, &str)> = files
            .iter()
            .map(|(uri, text)| (*uri, text.as_str()))
            .collect();
        catalog(&files)
    };
    for (name, root, query, expected, loads) in cases {
        let (mut catalog, log) = c(root);
        assert_eq!(ask(&mut catalog, query).as_deref(), *expected, "{name}");
        let log = log.lock().unwrap();
        assert_eq!(log.len(), *loads, "{name}: {log:?}");
        assert!(log.iter().all(|(kind, _)| *kind == ResourceKind::Catalog));
    }
}

#[test]
fn broken_delegates_stop_cutting_on_later_queries() {
    let root = xml(
        "<delegateSystem systemIdStartString='http://e/' catalog='missing.xml'/><nextCatalog catalog='a.xml'/>",
    );
    let a = xml("<system systemId='http://e/x' uri='a'/>");
    let (mut catalog, log) = catalog(&[("c.xml", &root), ("a.xml", &a)]);
    assert_eq!(catalog.resolve(None, Some("http://e/x")), None);
    assert_eq!(
        catalog.resolve(None, Some("http://e/x")).as_deref(),
        Some("a")
    );
    assert_eq!(
        catalog.resolve(None, Some("http://e/x")).as_deref(),
        Some("a")
    );
    assert_eq!(
        log.lock().unwrap().len(),
        3,
        "loaded files are cached, failures are not retried"
    );
}

#[test]
fn root_prefer_and_group_prefer_apply_to_delegates() {
    let a = xml("<public publicId='-//A//X' uri='a'/>");
    let cases = [
        ("", Some("a")),
        (" prefer='public'", Some("a")),
        (" prefer='system'", None),
        (" prefer='bogus'", Some("a")),
    ];
    for (prefer, expected) in cases {
        let root = format!(
            "<catalog xmlns='urn:oasis:names:tc:entity:xmlns:xml:catalog'{prefer}><delegatePublic publicIdStartString='-//A' catalog='a.xml'/></catalog>"
        );
        let (mut catalog, _) = catalog(&[("c.xml", &root), ("a.xml", &a)]);
        assert_eq!(
            catalog.resolve(Some("-//A//X"), None).as_deref(),
            expected,
            "{prefer}"
        );
    }
    // A loaded catalog inherits the referencing entry's preference.
    let root = xml("<group prefer='system'><nextCatalog catalog='n.xml'/></group>");
    let next = xml(
        "<delegatePublic publicIdStartString='-//A' catalog='a.xml'/><public publicId='-//A//X' uri='n'/>",
    );
    let (mut catalog, _) = catalog(&[("c.xml", &root), ("n.xml", &next), ("a.xml", &a)]);
    assert_eq!(catalog.resolve(Some("-//A//X"), None).as_deref(), Some("n"));
}

#[test]
fn limits_bound_loading_and_nesting() {
    let chain: Vec<(String, String)> = (0..6)
        .map(|i| {
            let body = match i {
                5 => "<public publicId='p' uri='end'/>".to_owned(),
                _ => format!("<nextCatalog catalog='{}.xml'/>", i + 1),
            };
            (format!("{i}.xml"), xml(&body))
        })
        .collect();
    let files: Vec<(&str, &str)> = chain
        .iter()
        .map(|(uri, text)| (uri.as_str(), text.as_str()))
        .collect();
    let total: usize = files.iter().map(|(_, text)| text.len()).sum();
    let cases = [
        ("defaults", Limits::default(), Some("end")),
        (
            "loads",
            {
                let mut cfg = Limits::default();
                cfg.max_loads = 5;
                cfg
            },
            None,
        ),
        (
            "enough loads",
            {
                let mut cfg = Limits::default();
                cfg.max_loads = 6;
                cfg
            },
            Some("end"),
        ),
        (
            "bytes",
            {
                let mut cfg = Limits::default();
                cfg.max_bytes = total - 1;
                cfg
            },
            None,
        ),
        (
            "enough bytes",
            {
                let mut cfg = Limits::default();
                cfg.max_bytes = total;
                cfg
            },
            Some("end"),
        ),
        (
            "depth",
            {
                let mut cfg = Limits::default();
                cfg.max_depth = 4;
                cfg
            },
            None,
        ),
        (
            "enough depth",
            {
                let mut cfg = Limits::default();
                cfg.max_depth = 5;
                cfg
            },
            Some("end"),
        ),
    ];
    for (name, limits, expected) in cases {
        let (resolver, _) = served(&files);
        let mut catalog = Catalog::load("0.xml", limits, resolver).unwrap();
        assert_eq!(
            catalog.resolve(Some("p"), None).as_deref(),
            expected,
            "{name}"
        );
    }
}

#[test]
fn building_a_catalog_fails_cleanly() {
    let failing = |_: Request<'_>| Err(io::Error::new(io::ErrorKind::PermissionDenied, "no"));
    assert_eq!(
        Catalog::load("c.xml", Limits::default(), Box::new(failing)).unwrap_err(),
        Error::Io(io::ErrorKind::PermissionDenied)
    );
    let cases: &[(&str, Limits, Error)] = &[
        ("missing.xml", Limits::default(), Error::Missing),
        (
            "c.xml",
            {
                let mut cfg = Limits::default();
                cfg.max_loads = 0;
                cfg
            },
            Error::LimitExceeded,
        ),
        (
            "c.xml",
            {
                let mut cfg = Limits::default();
                cfg.max_bytes = 3;
                cfg
            },
            Error::LimitExceeded,
        ),
        ("unclosed.xml", Limits::default(), Error::Malformed),
        ("foreign.xml", Limits::default(), Error::Malformed),
        ("bad.sgml", Limits::default(), Error::Malformed),
    ];
    let files = [
        ("c.xml", xml("")),
        (
            "unclosed.xml",
            "<catalog xmlns='urn:oasis:names:tc:entity:xmlns:xml:catalog'>".into(),
        ),
        ("foreign.xml", "<catalog/>".into()),
        ("bad.sgml", "-- unterminated".into()),
    ];
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(uri, text)| (*uri, text.as_str()))
        .collect();
    for (url, limits, error) in cases {
        let (resolver, _) = served(&files);
        assert_eq!(
            Catalog::load(url, *limits, resolver).unwrap_err(),
            *error,
            "{url}"
        );
    }
    assert_eq!(Error::Missing.to_string(), "catalog not found");
    assert_eq!(
        Error::Io(io::ErrorKind::NotFound).to_string(),
        "catalog could not be read: entity not found"
    );
    assert_eq!(
        Error::LimitExceeded.to_string(),
        "catalog load limit exceeded"
    );
    assert_eq!(Error::Malformed.to_string(), "malformed catalog");
    let (mut empty, _) = catalog(&[("c.xml", &xml(""))]);
    assert!(format!("{empty:?}").starts_with("Catalog"));
    assert_eq!(empty.resolve(Some("p"), Some("s")), None);
}

#[test]
fn sgml_catalogs_parse_like_libxml2() {
    use Ask::{Id, Uri};
    let cases: &[(&str, &str, Ask<'_>, Option<&str>)] = &[
        (
            "public",
            "PUBLIC \"-//A//X\" \"x.dtd\"",
            Id(Some("-//A//X"), None),
            Some("s/x.dtd"),
        ),
        (
            "public normalized both ways",
            "PUBLIC \" -//A//\n X\" 'x.dtd'\n",
            Id(Some("-//A// X "), None),
            Some("s/x.dtd"),
        ),
        (
            "system",
            "SYSTEM \"http://e/x\" x.dtd\n",
            Id(None, Some("http://e/x")),
            Some("s/x.dtd"),
        ),
        (
            "public before system",
            "SYSTEM s sys\nPUBLIC p pub\n",
            Id(Some("p"), Some("s")),
            Some("s/pub"),
        ),
        (
            "system as uri",
            "SYSTEM \"http://e/x\" \"x\"",
            Uri("http://e/x"),
            Some("s/x"),
        ),
        (
            "public is not a uri",
            "PUBLIC \"http://e/x\" \"x\"",
            Uri("http://e/x"),
            None,
        ),
        (
            "first entry wins",
            "PUBLIC p \"one\" PUBLIC p \"two\"",
            Id(Some("p"), None),
            Some("s/one"),
        ),
        (
            "kinds share keys",
            "SYSTEM p \"sys\" PUBLIC p \"pub\"",
            Id(Some("p"), None),
            None,
        ),
        (
            "base",
            "BASE \"http://b/\" PUBLIC p \"x\"",
            Id(Some("p"), None),
            Some("http://b/x"),
        ),
        (
            "catalog",
            "CATALOG \"n.sgml\"",
            Id(Some("-//N//X"), None),
            Some("s/n.dtd"),
        ),
        (
            "delegate kept out",
            "DELEGATE \"-//A//\" \"n.sgml\"",
            Id(Some("-//N//X"), None),
            None,
        ),
        (
            "ignored entries",
            "-- note -- OVERRIDE YES ENTITY e 'e.ent' DOCTYPE d \"d.dtd\" LINKTYPE l 'l' NOTATION n\tPUBLIC DOCUMENT 'd' SGMLDECL 's' UNKNOWN PUBLIC p x\n",
            Id(Some("p"), None),
            Some("s/x"),
        ),
        (
            "adjacent literal",
            "PUBLIC p \"x\"PUBLIC q \"y\"",
            Id(Some("q"), None),
            Some("s/y"),
        ),
        (
            "keyword without blank stops",
            "PUBLIC p x\nPUBLIC",
            Id(Some("p"), None),
            Some("s/x"),
        ),
        (
            "public without system",
            "PUBLIC \"p\"",
            Id(Some("p"), None),
            None,
        ),
        (
            "empty public dropped",
            "PUBLIC \" \" x\n",
            Id(Some(" "), None),
            None,
        ),
        ("empty file", "", Id(Some("p"), None), None),
    ];
    for (name, text, query, expected) in cases {
        let (mut catalog, _) = catalog(&[
            ("s/c.sgml", text),
            ("s/n.sgml", "PUBLIC \"-//N//X\" n.dtd\n"),
        ]);
        assert_eq!(ask(&mut catalog, query).as_deref(), *expected, "{name}");
    }
}

#[test]
fn sgml_syntax_errors_reject_the_catalog() {
    let cases = [
        "ENTITY %e 'x'\n",
        "-- open",
        "PUBLIC p x",
        "PUBLIC p\n",
        "PUBLIC \"p\tq\" x\n",
        "PUBLIC 'p x\n",
        "OVERRIDE 1\n",
        "1PUBLIC p x\n",
        &format!("{} x\n", "N".repeat(100)),
    ];
    for text in cases {
        let (resolver, _) = served(&[("c.sgml", text)]);
        assert_eq!(
            Catalog::load("c.sgml", Limits::default(), resolver).unwrap_err(),
            Error::Malformed,
            "{text:?}"
        );
    }
}

#[test]
fn sgml_catalog_entries_load_eagerly_once() {
    let files = [
        (
            "c.sgml",
            "CATALOG a.sgml\nCATALOG a.sgml\nCATALOG bad.sgml\nCATALOG missing.sgml\nPUBLIC late x\n",
        ),
        (
            "a.sgml",
            "CATALOG c.sgml\nCATALOG a.sgml\nPUBLIC a \"a.dtd\"\n",
        ),
        ("bad.sgml", "PUBLIC partial \"p.dtd\"\nENTITY %e x\n"),
    ];
    let (mut catalog, log) = catalog(&files);
    let loaded: Vec<String> = log
        .lock()
        .unwrap()
        .iter()
        .map(|(_, uri)| uri.clone())
        .collect();
    assert_eq!(
        loaded,
        ["c.sgml", "a.sgml", "c.sgml", "bad.sgml", "missing.sgml"]
    );
    for (public, expected) in [
        ("a", Some("a.dtd")),
        ("partial", Some("p.dtd")),
        ("late", Some("x")),
    ] {
        assert_eq!(
            catalog.resolve(Some(public), None).as_deref(),
            expected,
            "{public}"
        );
    }
    // Nesting beyond max_depth is skipped, not an error.
    let (resolver, _) = served(&[("c.sgml", "CATALOG c.sgml\nPUBLIC p x\n")]);
    let shallow = {
        let mut cfg = Limits::default();
        cfg.max_depth = 0;
        cfg
    };
    let mut catalog = Catalog::load("c.sgml", shallow, resolver).unwrap();
    assert_eq!(catalog.resolve(Some("p"), None).as_deref(), Some("x"));
}

#[test]
fn parse_takes_bytes_and_sniffs_the_format() {
    let none = |_: Request<'_>| Ok(None);
    let cases: &[(&[u8], Option<&str>)] = &[
        (b"<?xml version='1.0'?><catalog xmlns='urn:oasis:names:tc:entity:xmlns:xml:catalog'><public publicId='p' uri='x'/></catalog>", Some("x")),
        (b"\n\t<catalog xmlns='urn:oasis:names:tc:entity:xmlns:xml:catalog'><public publicId='p' uri='x'/></catalog>", Some("x")),
        (b"  PUBLIC p x\n", Some("x")),
        (b"-- x -- PUBLIC p x\n", Some("x")),
        (b"\0<catalog/>", None),
    ];
    for (input, expected) in cases {
        let mut catalog = Catalog::parse(input, Limits::default(), "", Box::new(none)).unwrap();
        assert_eq!(catalog.resolve(Some("p"), None).as_deref(), *expected);
    }
    let utf16: Vec<u8> = "\u{feff}<catalog xmlns='urn:oasis:names:tc:entity:xmlns:xml:catalog'><public publicId='p' uri='x'/></catalog>"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    assert!(
        Catalog::parse(&utf16, Limits::default(), "", Box::new(none))
            .unwrap()
            .resolve(Some("p"), None)
            .is_some(),
        "UTF-16 is sniffed by its '<' byte"
    );
}

#[test]
fn catalog_resolver_maps_requests_for_parsers() {
    let entries = xml("<public publicId='-//E//DTD R//EN' uri='local/r.dtd'/>\
         <uri name='http://e/u.ent' uri='local/u.ent'/>");
    let (loader, _) = served(&[("cat.xml", &entries)]);
    let catalog = Catalog::load("cat.xml", Limits::default(), loader).unwrap();
    let (inner, log) = served(&[
        (
            "local/r.dtd",
            "<!ENTITY u SYSTEM 'http://e/u.ent'><!ENTITY o SYSTEM 'other.ent'>",
        ),
        ("local/u.ent", "mapped"),
        ("http://e/other.ent", "+plain"),
    ]);
    let resolver = CatalogResolver::new(catalog, inner);
    let input = b"<!DOCTYPE r PUBLIC '-//E//DTD R//EN' 'http://e/r.dtd'><r>&u;&o;</r>";
    let document = Document::parse_with(
        input,
        loading(Default::default(), "doc.xml", Box::new(resolver)),
    )
    .unwrap();
    assert_eq!(
        document.root_element().text_chunks().collect::<String>(),
        "mapped+plain"
    );
    let requests: Vec<(ResourceKind, String)> = log.lock().unwrap().clone();
    assert_eq!(
        requests,
        [
            (ResourceKind::DtdSubset, "local/r.dtd".to_owned()),
            (ResourceKind::GeneralEntity, "local/u.ent".to_owned()),
            (ResourceKind::GeneralEntity, "http://e/other.ent".to_owned()),
        ]
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
