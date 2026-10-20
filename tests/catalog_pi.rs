#![forbid(unsafe_code)]
//! `CATALOG_PI`: a document's `<?oasis-xml-catalog catalog="URL"?>` adds a
//! catalog to a `CatalogResolver` for the rest of that parse, on every parsing
//! surface, and never loads anything the resolver forbids. The libxml2
//! differential (`YAXML_CATALOG_HELPER`, see docs/catalog.md) is
//! `catalog_pi_matches_libxml2` in yaxml-harness/tests/catalog_oracle.rs.

use std::collections::HashMap;
use std::io;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};

use yaxml::catalog::{Catalog, CatalogResolver, Limits};
use yaxml::options::{CATALOG_PI, DTDLOAD, NO_XXE, NOENT};
use yaxml::{
    BufferedReader, Document, Event, FileResolver, NodeType, OwnedDocument, ParseOptions,
    PushParser, Reader, Request, Resolver, ResourceKind, TextReader,
};

type Log = Arc<Mutex<Vec<(ResourceKind, String)>>>;

/// Serves `files` by URI and records every request.
fn served(files: &[(&str, &str)], log: &Log) -> Box<dyn Resolver> {
    let files: HashMap<String, Vec<u8>> = files
        .iter()
        .map(|(uri, text)| (uri.to_string(), text.as_bytes().to_vec()))
        .collect();
    let log = log.clone();
    Box::new(move |request: Request<'_>| {
        log.lock()
            .unwrap()
            .push((request.kind, request.uri.to_owned()));
        match request.uri {
            "denied.xml" => Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied")),
            uri => Ok(files.get(uri).cloned()),
        }
    })
}

/// A catalog document with literal `$body` entries.
macro_rules! catalog {
    ($body:expr) => {
        concat!(
            "<catalog xmlns='urn:oasis:names:tc:entity:xmlns:xml:catalog'>",
            $body,
            "</catalog>"
        )
    };
}

fn catalog_xml(body: &str) -> String {
    format!("<catalog xmlns='urn:oasis:names:tc:entity:xmlns:xml:catalog'>{body}</catalog>")
}

/// A resolver as `Reader` and the other surfaces take it: the catalogs load
/// through one log, the resources they map through another.
fn resolver(main: &str, files: &[(&str, &str)], limits: Limits) -> (Box<dyn Resolver>, Log, Log) {
    let (catalogs, resources) = (Log::default(), Log::default());
    let catalog = Catalog::parse(
        catalog_xml(main).as_bytes(),
        limits,
        "main.xml",
        served(files, &catalogs),
    )
    .unwrap();
    let resolver = CatalogResolver::new(catalog, served(files, &resources));
    (Box::new(resolver), catalogs, resources)
}

/// Every surface that can load a DTD, over `doc`: the character data of the
/// document, or `None` when parsing failed.
fn text_on_every_surface(
    doc: &str,
    options: impl Fn() -> ParseOptions,
) -> Vec<(&'static str, Option<String>)> {
    let document = Document::parse_with(doc.as_bytes(), options())
        .ok()
        .map(|document| document.root_element().text_chunks().collect::<String>());
    let owned = OwnedDocument::parse_with(doc.as_bytes(), options())
        .ok()
        .map(|document| document.root_element().text_chunks().collect::<String>());
    let reader = (|| {
        let mut reader = Reader::with_options(doc.as_bytes(), options()).ok()?;
        let mut text = String::new();
        while let Some(event) = reader.read_event().ok()? {
            if let Event::Text(chunk) = event {
                text.push_str(&chunk);
            }
        }
        Some(text)
    })();
    let buffered = (|| {
        let mut reader = BufferedReader::with_options(doc.as_bytes(), options()).ok()?;
        let mut text = String::new();
        while let Some((event, _)) = reader.read_event().ok()? {
            if let Event::Text(chunk) = event {
                text.push_str(&chunk);
            }
        }
        Some(text)
    })();
    let push = (|| {
        let mut parser = PushParser::with_options(options()).ok()?;
        let mut text = String::new();
        let mut emit = |event: &Event<'_>, _| {
            if let Event::Text(chunk) = event {
                text.push_str(chunk);
            }
            ControlFlow::Continue(())
        };
        // One byte at a time: a processing instruction can be split anywhere.
        for byte in doc.as_bytes().chunks(1) {
            parser.feed(byte, &mut emit).ok()?;
        }
        parser.finish(&mut emit).ok()?;
        Some(text)
    })();
    let pull = (|| {
        let mut reader = TextReader::with_options(doc.as_bytes(), options()).ok()?;
        let mut text = String::new();
        while reader.read().ok()? {
            if reader.node_type() == NodeType::Text {
                text.push_str(reader.value().unwrap_or_default());
            }
        }
        Some(text)
    })();
    vec![
        ("Document", document),
        ("OwnedDocument", owned),
        ("Reader", reader),
        ("BufferedReader", buffered),
        ("PushParser", push),
        ("TextReader", pull),
    ]
}

struct Case {
    name: &'static str,
    doc: &'static str,
    /// Files served to both resolvers.
    files: &'static [(&'static str, &'static str)],
    /// Entries of the main (global) catalog.
    main: &'static str,
    flags: u32,
    expected: Option<&'static str>,
}

const R_DTD: &str = "http://x.invalid/r.dtd";
const C1: &str = catalog!("<system systemId='http://x.invalid/r.dtd' uri='d1.dtd'/>");
const C2: &str = catalog!(
    "<system systemId='http://x.invalid/r.dtd' uri='d2.dtd'/>\
     <system systemId='http://x.invalid/y.ent' uri='y.ent'/>"
);
const NEXT: &str = catalog!("<nextCatalog catalog='c2.xml'/>");
const SHARED: &[(&str, &str)] = &[
    (R_DTD, "<!ENTITY e 'orig'>"),
    ("c1.xml", C1),
    ("c2.xml", C2),
    ("next.xml", NEXT),
    ("d1.dtd", "<!ENTITY e 'one'>"),
    ("d2.dtd", "<!ENTITY e 'two'>"),
    ("dmain.dtd", "<!ENTITY e 'main'>"),
    ("x.ent", "from-x"),
    ("y.ent", "from-y"),
    ("http://x.invalid/x.ent", "orig-x"),
    ("http://x.invalid/y.ent", "orig-y"),
];

#[test]
fn a_catalog_pi_adds_a_catalog_for_the_rest_of_the_parse() {
    let files = SHARED;
    let cases = [
        Case {
            name: "before the DOCTYPE, it maps the external subset",
            doc: "<?oasis-xml-catalog catalog=\"c1.xml\"?><!DOCTYPE r SYSTEM 'http://x.invalid/r.dtd'><r>&e;</r>",
            files,
            main: "",
            flags: CATALOG_PI | NOENT | DTDLOAD,
            expected: Some("one"),
        },
        Case {
            name: "without the flag it is only a processing instruction",
            doc: "<?oasis-xml-catalog catalog=\"c1.xml\"?><!DOCTYPE r SYSTEM 'http://x.invalid/r.dtd'><r>&e;</r>",
            files,
            main: "",
            flags: NOENT | DTDLOAD,
            expected: Some("orig"),
        },
        Case {
            name: "single quotes and spaces around the equals sign",
            doc: "<?oasis-xml-catalog  catalog = 'c1.xml' ?><!DOCTYPE r SYSTEM 'http://x.invalid/r.dtd'><r>&e;</r>",
            files,
            main: "",
            flags: CATALOG_PI | NOENT | DTDLOAD,
            expected: Some("one"),
        },
        Case {
            name: "the first catalog that answers wins",
            doc: "<?oasis-xml-catalog catalog=\"c2.xml\"?><?oasis-xml-catalog catalog=\"c1.xml\"?><!DOCTYPE r SYSTEM 'http://x.invalid/r.dtd'><r>&e;</r>",
            files,
            main: "",
            flags: CATALOG_PI | NOENT | DTDLOAD,
            expected: Some("two"),
        },
        Case {
            name: "a catalog without an answer falls through to the next one",
            doc: "<?oasis-xml-catalog catalog=\"missing.xml\"?><?oasis-xml-catalog catalog=\"denied.xml\"?><?oasis-xml-catalog catalog=\"c1.xml\"?><!DOCTYPE r SYSTEM 'http://x.invalid/r.dtd'><r>&e;</r>",
            files,
            main: "",
            flags: CATALOG_PI | NOENT | DTDLOAD,
            expected: Some("one"),
        },
        Case {
            name: "document catalogs come before the main one",
            doc: "<?oasis-xml-catalog catalog=\"c1.xml\"?><!DOCTYPE r SYSTEM 'http://x.invalid/r.dtd'><r>&e;</r>",
            files,
            main: "<system systemId='http://x.invalid/r.dtd' uri='dmain.dtd'/>",
            flags: CATALOG_PI | NOENT | DTDLOAD,
            expected: Some("one"),
        },
        Case {
            name: "the main catalog answers what they do not",
            doc: "<?oasis-xml-catalog catalog=\"c2.xml\"?><!DOCTYPE r SYSTEM 'http://x.invalid/r.dtd' [<!ENTITY m SYSTEM 'http://x.invalid/m.ent'>]><r>&m;</r>",
            files: &[("c2.xml", catalog!("")), ("mapped.ent", "main-m")],
            main: "<system systemId='http://x.invalid/m.ent' uri='mapped.ent'/>",
            flags: CATALOG_PI | NOENT | DTDLOAD,
            expected: Some("main-m"),
        },
        Case {
            name: "a nextCatalog of a document catalog is followed",
            doc: "<?oasis-xml-catalog catalog=\"next.xml\"?><!DOCTYPE r SYSTEM 'http://x.invalid/r.dtd'><r>&e;</r>",
            files,
            main: "",
            flags: CATALOG_PI | NOENT | DTDLOAD,
            expected: Some("two"),
        },
        Case {
            name: "after the DOCTYPE, it maps external entities",
            doc: "<!DOCTYPE r [<!ENTITY x SYSTEM 'http://x.invalid/x.ent'>]><?oasis-xml-catalog catalog=\"cx.xml\"?><r>&x;</r>",
            files: &[
                (
                    "cx.xml",
                    catalog!("<system systemId='http://x.invalid/x.ent' uri='x.ent'/>"),
                ),
                ("x.ent", "from-x"),
                ("http://x.invalid/x.ent", "orig-x"),
            ],
            main: "",
            flags: CATALOG_PI | NOENT | DTDLOAD,
            expected: Some("from-x"),
        },
        Case {
            name: "in content, it applies only to what follows",
            doc: "<!DOCTYPE r [<!ENTITY x SYSTEM 'http://x.invalid/x.ent'><!ENTITY y SYSTEM 'http://x.invalid/y.ent'><!ENTITY w SYSTEM 'http://x.invalid/y.ent'>]><r>&x;&y;<?oasis-xml-catalog catalog=\"c2.xml\"?>&w;</r>",
            files,
            main: "",
            flags: CATALOG_PI | NOENT | DTDLOAD,
            expected: Some("orig-xorig-yfrom-y"),
        },
        Case {
            name: "in an entity's replacement text",
            doc: "<!DOCTYPE r [<!ENTITY p \"<?oasis-xml-catalog catalog='c2.xml'?>\"><!ENTITY y SYSTEM 'http://x.invalid/y.ent'>]><r>&p;&y;</r>",
            files,
            main: "",
            flags: CATALOG_PI | NOENT | DTDLOAD,
            expected: Some("from-y"),
        },
        Case {
            name: "syntax errors add nothing",
            doc: "<?oasis-xml-catalog catalog=c1.xml?><?oasis-xml-catalog catalog=\"c1.xml\" x?><?oasis-xml-catalog cat=\"c1.xml\"?><?oasis-xml-catalog?><?other catalog=\"c1.xml\"?><!DOCTYPE r SYSTEM 'http://x.invalid/r.dtd'><r>&e;</r>",
            files,
            main: "",
            flags: CATALOG_PI | NOENT | DTDLOAD,
            expected: Some("orig"),
        },
        Case {
            name: "NO_XXE loads nothing, catalogs included",
            doc: "<?oasis-xml-catalog catalog=\"c1.xml\"?><!DOCTYPE r SYSTEM 'http://x.invalid/r.dtd'><r>&e;</r>",
            files,
            main: "",
            flags: CATALOG_PI | NOENT | DTDLOAD | NO_XXE,
            expected: Some(""),
        },
        Case {
            name: "no DOCTYPE, nothing to load",
            doc: "<?oasis-xml-catalog catalog=\"c1.xml\"?><r>text</r>",
            files,
            main: "",
            flags: CATALOG_PI | NOENT | DTDLOAD,
            expected: Some("text"),
        },
    ];
    for case in cases {
        let results = std::cell::RefCell::new(Vec::new());
        let options = || {
            let (resolver, catalogs, resources) =
                resolver(case.main, case.files, Limits::default());
            results.borrow_mut().push((catalogs, resources));
            ParseOptions::new(case.flags).with_resolver("doc.xml", resolver)
        };
        for (surface, text) in text_on_every_surface(case.doc, options) {
            assert_eq!(text.as_deref(), case.expected, "{}: {surface}", case.name);
        }
    }
}

#[test]
fn document_catalogs_load_through_the_catalogs_own_resolver_only() {
    let files = SHARED;
    let doc = "<?oasis-xml-catalog catalog=\"c1.xml\"?><?oasis-xml-catalog catalog=\"denied.xml\"?>\
               <!DOCTYPE r SYSTEM 'http://x.invalid/r.dtd'><r>&e;</r>";
    let (resolver, catalogs, resources) = resolver("", files, Limits::default());
    let options =
        ParseOptions::new(CATALOG_PI | NOENT | DTDLOAD).with_resolver("doc.xml", resolver);
    let document = Document::parse_with(doc.as_bytes(), options).unwrap();
    assert_eq!(
        document.root_element().text_chunks().collect::<String>(),
        "one"
    );
    // The document's URLs are requested as catalogs, as given and in order, and
    // the first catalog's answer ends the lookup.
    assert_eq!(
        *catalogs.lock().unwrap(),
        [(ResourceKind::Catalog, "c1.xml".to_owned())]
    );
    assert_eq!(
        *resources.lock().unwrap(),
        [(ResourceKind::DtdSubset, "d1.dtd".to_owned())]
    );

    // A catalog the resolver refuses is skipped: the lookup goes on.
    let doc = "<?oasis-xml-catalog catalog=\"denied.xml\"?><!DOCTYPE r SYSTEM 'http://x.invalid/r.dtd'><r>&e;</r>";
    let (resolver, catalogs, resources) = resolver_with("", files);
    let options =
        ParseOptions::new(CATALOG_PI | NOENT | DTDLOAD).with_resolver("doc.xml", resolver);
    let document = Document::parse_with(doc.as_bytes(), options).unwrap();
    assert_eq!(
        document.root_element().text_chunks().collect::<String>(),
        "orig"
    );
    assert_eq!(
        *catalogs.lock().unwrap(),
        [(ResourceKind::Catalog, "denied.xml".to_owned())]
    );
    assert_eq!(
        *resources.lock().unwrap(),
        [(ResourceKind::DtdSubset, R_DTD.to_owned())]
    );
}

fn resolver_with(main: &str, files: &[(&str, &str)]) -> (Box<dyn Resolver>, Log, Log) {
    resolver(main, files, Limits::default())
}

#[test]
fn a_root_confined_resolver_keeps_a_hostile_catalog_url_out() {
    let directory = std::env::temp_dir().join(format!("yaxml-catalog-pi-{}", std::process::id()));
    let root = directory.join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        directory.join("secret.xml"),
        catalog_xml("<system systemId='http://x.invalid/r.dtd' uri='root/d.dtd'/>"),
    )
    .unwrap();
    std::fs::write(root.join("d.dtd"), "<!ENTITY e 'mapped'>").unwrap();
    std::fs::write(root.join("r.dtd"), "<!ENTITY e 'plain'>").unwrap();
    let doc = |url: &str| {
        format!("<?oasis-xml-catalog catalog=\"{url}\"?><!DOCTYPE r SYSTEM 'r.dtd'><r>&e;</r>")
    };
    let parse = |url: &str| {
        let files = FileResolver::new(&root).unwrap();
        let empty = catalog_xml("");
        let catalog = Catalog::parse(
            empty.as_bytes(),
            Limits::default(),
            "main.xml",
            Box::new(files.clone()),
        )
        .unwrap();
        let resolver = CatalogResolver::new(catalog, Box::new(files));
        let options = ParseOptions::new(CATALOG_PI | NOENT | DTDLOAD)
            .with_resolver("doc.xml", Box::new(resolver));
        Document::parse_with(doc(url).as_bytes(), options)
            .map(|document| document.root_element().text_chunks().collect::<String>())
            .ok()
    };
    // The catalog above the root, and absolute and remote ones, are never read.
    for url in [
        "../secret.xml",
        &directory.join("secret.xml").display().to_string(),
        "http://x.invalid/c.xml",
        "file://host/c.xml",
    ] {
        assert_eq!(parse(url).as_deref(), Some("plain"), "{url}");
    }
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_resolver_that_does_not_act_on_the_instruction_is_unaffected() {
    let log = Log::default();
    let files = SHARED;
    let doc = "<?oasis-xml-catalog catalog=\"c1.xml\"?><!DOCTYPE r SYSTEM 'http://x.invalid/r.dtd'><r>&e;</r>";
    let options = ParseOptions::new(CATALOG_PI | NOENT | DTDLOAD)
        .with_resolver("doc.xml", served(files, &log));
    let document = Document::parse_with(doc.as_bytes(), options).unwrap();
    assert_eq!(
        document.root_element().text_chunks().collect::<String>(),
        "orig"
    );
    assert_eq!(
        *log.lock().unwrap(),
        [(ResourceKind::DtdSubset, R_DTD.to_owned())]
    );
}

#[test]
fn document_catalogs_are_bounded_by_the_loads_and_by_max_local() {
    let files = SHARED;
    // (limits, instructions before c1.xml, expected)
    let cases = [
        (Limits::DEFAULT.max_local(0), 0, "orig"),
        (Limits::DEFAULT.max_local(1), 0, "one"),
        (Limits::DEFAULT.max_local(1), 1, "orig"),
        (Limits::DEFAULT.max_local(2), 1, "one"),
        // Each missing catalog spends a load before c1.xml can be fetched.
        (Limits::DEFAULT.max_loads(2), 2, "orig"),
        (Limits::DEFAULT.max_loads(3), 2, "one"),
    ];
    for (limits, missing, expected) in cases {
        let prologue = "<?oasis-xml-catalog catalog=\"gone.xml\"?>".repeat(missing);
        let doc = format!(
            "{prologue}<?oasis-xml-catalog catalog=\"c1.xml\"?><!DOCTYPE r SYSTEM 'http://x.invalid/r.dtd'><r>&e;</r>"
        );
        let (resolver, ..) = resolver("", files, limits);
        let options =
            ParseOptions::new(CATALOG_PI | NOENT | DTDLOAD).with_resolver("doc.xml", resolver);
        let document = Document::parse_with(doc.as_bytes(), options).unwrap();
        assert_eq!(
            document.root_element().text_chunks().collect::<String>(),
            expected,
            "{limits:?} after {missing}"
        );
    }
}
