#![forbid(unsafe_code)]
//! External DTD subsets and entities through caller resolvers, their budgets,
//! the filesystem resolver's confinement, and DTD-declared IDs.

use std::collections::HashMap;
use std::io::{self, BufReader, Cursor};
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use yaxml::xpath::{Context, Tree, XPath};
use yaxml::{
    BufferedReader, Document, EditLimits, Error, ErrorKind, Event, FileResolver, Limit, Limits,
    OwnedDocument, PushParser, ReadError, Reader, Request, Resolver, ResourceKind, Writer,
};

type Log = Arc<Mutex<Vec<String>>>;
/// Resolved URI and contents.
type Files<'a> = &'a [(&'a str, &'a str)];
type Served = Result<Option<&'static [u8]>, io::ErrorKind>;

/// Serves `files` by resolved URI and records every request; `fail` errors.
fn memory(files: &[(&str, &str)], log: &Log) -> Box<dyn Resolver> {
    let files: HashMap<String, Vec<u8>> = files
        .iter()
        .map(|(uri, text)| (uri.to_string(), text.as_bytes().to_vec()))
        .collect();
    let log = log.clone();
    Box::new(move |request: Request<'_>| {
        log.lock().unwrap().push(format!(
            "{:?} {} {:?} {}",
            request.kind, request.uri, request.public_id, request.max_bytes
        ));
        match request.uri {
            "dir/fail" => Err(io::Error::other("refused")),
            uri => Ok(files.get(uri).cloned()),
        }
    })
}

fn written(writer: Writer<Vec<u8>>) -> String {
    String::from_utf8(writer.into_inner()).unwrap()
}

const SURFACES: [&str; 5] = ["reader", "document", "owned", "push", "buffered"];

/// One surface's serialized result and the requests it made.
fn run(
    surface: &str,
    input: &str,
    files: &[(&str, &str)],
    limits: Limits,
) -> (Result<String, Error>, Vec<String>) {
    let log = Log::default();
    let resolver = memory(files, &log);
    let bytes = input.as_bytes();
    let base = "dir/doc.xml";
    let mut writer = Writer::new(Vec::new());
    let result = match surface {
        "reader" => (|| {
            let mut reader = Reader::with_options(bytes, loading(limits, base, resolver))?;
            while let Some(event) = reader.read_event()? {
                writer.write_event(&event).unwrap();
            }
            Ok(written(writer))
        })(),
        "document" => {
            Document::parse_with(bytes, loading(limits, base, resolver)).map(|document| {
                writer.write_document(&document).unwrap();
                written(writer)
            })
        }
        "owned" => {
            OwnedDocument::parse_with(bytes, loading(limits, base, resolver)).map(|document| {
                writer.write_owned_document(&document).unwrap();
                written(writer)
            })
        }
        "push" => {
            let mut parser = PushParser::with_options(loading(limits, base, resolver)).unwrap();
            let mut emit = |event: &Event<'_>, _| {
                writer.write_event(event).unwrap();
                ControlFlow::Continue(())
            };
            let result = bytes
                .chunks(3)
                .try_for_each(|chunk| parser.feed(chunk, &mut emit))
                .and_then(|()| parser.finish(&mut emit));
            result.map(|()| written(writer))
        }
        _ => {
            let input = BufReader::with_capacity(5, Cursor::new(bytes));
            let mut reader =
                BufferedReader::with_options(input, loading(limits, base, resolver)).unwrap();
            loop {
                match reader.read_event() {
                    Ok(Some((event, _))) => writer.write_event(&event).unwrap(),
                    Ok(None) => break Ok(written(writer)),
                    Err(ReadError::Xml(error)) => break Err(error),
                    Err(error) => panic!("{error}"),
                }
            }
        }
    };
    let requests = log.lock().unwrap().clone();
    (result, requests)
}

/// Every surface must agree on the outcome and on the request sequence.
/// Returns the serialized document after its DOCTYPE.
fn all(input: &str, files: &[(&str, &str)], limits: Limits) -> Result<String, ErrorKind> {
    let (expected, requests) = run("reader", input, files, limits);
    for surface in SURFACES {
        let (result, made) = run(surface, input, files, limits);
        assert_eq!(
            result.as_ref().map_err(|error| error.kind),
            expected.as_ref().map_err(|error| error.kind),
            "{surface}: {input}"
        );
        assert_eq!(made, requests, "{surface}: {input}");
    }
    expected
        .map(|text| text[text.find("<r").unwrap()..].to_owned())
        .map_err(|error| error.kind)
}

#[test]
fn external_subsets_and_entities_load_and_expand_on_every_surface() {
    let cases: &[(&str, Files, &str)] = &[
        (
            "<!DOCTYPE r SYSTEM 's.dtd'><r>&e;</r>",
            &[(
                "dir/s.dtd",
                "<?xml version='1.0' encoding='UTF-8'?><!ATTLIST r a CDATA 'd'><!ENTITY e 'x'>",
            )],
            "<r a=\"d\">x</r>",
        ),
        (
            "<!DOCTYPE r [<!ENTITY e SYSTEM 'e.ent'>]><r>&e;&e;</r>",
            &[("dir/e.ent", "<?xml encoding='UTF-8'?>t<b/>\r\n")],
            "<r>t<b/>\nt<b/>\n</r>",
        ),
        (
            "<!DOCTYPE r [<!ENTITY % p SYSTEM 'p.ent'>%p;]><r>&e;</r>",
            &[("dir/p.ent", "<!ENTITY e 'from-pe'>")],
            "<r>from-pe</r>",
        ),
        (
            "<!DOCTYPE r SYSTEM 's.dtd'><r>&a;&b;</r>",
            &[(
                "dir/s.dtd",
                "<!ENTITY % on 'INCLUDE'><![%on;[<!ENTITY a 'A'>]]>\
                 <![IGNORE[<!ENTITY b 'no'><![INCLUDE[ x ]]>]]><!ENTITY b 'B'>",
            )],
            "<r>AB</r>",
        ),
        (
            "<!DOCTYPE r SYSTEM 's.dtd'><r/>",
            &[(
                "dir/s.dtd",
                "<!ENTITY % name 'r'><!ENTITY % type 'CDATA'><!ATTLIST %name; a %type; 'v'>",
            )],
            "<r a=\"v\"/>",
        ),
        (
            "<!DOCTYPE r SYSTEM 'sub/s.dtd'><r>&e;</r>",
            &[
                ("dir/sub/s.dtd", "<!ENTITY e SYSTEM 'e.ent'>"),
                ("dir/sub/e.ent", "relative"),
            ],
            "<r>relative</r>",
        ),
        (
            "<!DOCTYPE r SYSTEM 's.dtd'><r>&e;</r>",
            &[
                (
                    "dir/s.dtd",
                    "<!ENTITY % v SYSTEM 'v.ent'><!ENTITY e 'a%v;b'>",
                ),
                ("dir/v.ent", "-in-literal-"),
            ],
            "<r>a-in-literal-b</r>",
        ),
        (
            "<!DOCTYPE r SYSTEM 's.dtd' [<!ENTITY e 'internal'>]><r>&e;</r>",
            &[("dir/s.dtd", "<!ENTITY e 'external'>")],
            "<r>internal</r>",
        ),
        (
            "<!DOCTYPE r SYSTEM 'missing.dtd' [<!ENTITY e SYSTEM 'gone'>]><r>[&e;]</r>",
            &[],
            "<r>[]</r>",
        ),
    ];
    for (input, files, expected) in cases {
        assert_eq!(
            all(input, files, Limits::default()).as_deref(),
            Ok(*expected),
            "{input}"
        );
    }
}

#[test]
fn resolver_receives_resolved_uris_public_ids_and_remaining_bytes() {
    let input = "<!DOCTYPE r PUBLIC '-//p//DTD' 's.dtd' [<!ENTITY % p SYSTEM 'p.ent'>%p;]>\
                 <r>&g;&g;</r>";
    let files = [
        ("dir/s.dtd", "<!ENTITY q 'unused'>"),
        ("dir/p.ent", "<!ENTITY g PUBLIC '-//g//EN' '../g.ent'>"),
        ("g.ent", "g"),
    ];
    let limits = Limits::default().max_external_bytes(1000);
    for surface in SURFACES {
        let (result, requests) = run(surface, input, &files, limits);
        assert!(result.unwrap().ends_with("<r>gg</r>"), "{surface}");
        assert_eq!(
            requests,
            [
                "ParameterEntity dir/p.ent None 1000",
                "DtdSubset dir/s.dtd Some(\"-//p//DTD\") 960",
                "GeneralEntity g.ent Some(\"-//g//EN\") 940",
            ],
            "{surface}"
        );
    }
}

#[test]
fn malformed_or_refused_external_resources_fail() {
    let cases: &[(&str, Files, ErrorKind)] = &[
        (
            "<!DOCTYPE r SYSTEM 'fail'><r/>",
            &[],
            ErrorKind::ExternalResource,
        ),
        (
            "<!DOCTYPE r [<!ENTITY e SYSTEM 'fail'>]><r>&e;</r>",
            &[],
            ErrorKind::ExternalResource,
        ),
        (
            "<!DOCTYPE r [<!ENTITY e SYSTEM 'e'>]><r a='&e;'/>",
            &[("dir/e", "x")],
            ErrorKind::ExternalEntityInAttribute,
        ),
        (
            "<!DOCTYPE r [<!ENTITY e SYSTEM 'e'>]><r>&e;</r>",
            &[("dir/e", "<a>")],
            ErrorKind::UnbalancedEntity,
        ),
        (
            "<!DOCTYPE r [<!ENTITY e SYSTEM 'e'>]><r>&e;</r>",
            &[("dir/e", "\u{1}")],
            ErrorKind::InvalidCharacter,
        ),
        (
            "<!DOCTYPE r SYSTEM 's'><r/>",
            &[("dir/s", "<!BAD>")],
            ErrorKind::InvalidDtd,
        ),
        (
            "<!DOCTYPE r SYSTEM 's'><r/>",
            &[("dir/s", "<![INCLUDE[")],
            ErrorKind::InvalidDtd,
        ),
        (
            "<!DOCTYPE r SYSTEM 's'><r/>",
            &[("dir/s", "]]>")],
            ErrorKind::InvalidDtd,
        ),
        (
            "<!DOCTYPE r SYSTEM 's'><r/>",
            &[("dir/s", "<![MAYBE[]]>")],
            ErrorKind::InvalidDtd,
        ),
        (
            "<!DOCTYPE r SYSTEM 's'><r/>",
            &[("dir/s", "<![IGNORE[ never closed")],
            ErrorKind::InvalidDtd,
        ),
        (
            "<!DOCTYPE r SYSTEM 's'><r/>",
            &[("dir/s", "<!ENTITY e 'x'")],
            ErrorKind::InvalidDtd,
        ),
        (
            "<!DOCTYPE r [<!ENTITY % a SYSTEM 'a'>%a;]><r/>",
            &[("dir/a", "<!ENTITY % b SYSTEM 'b'>%b;"), ("dir/b", "%a;")],
            ErrorKind::EntityLoop,
        ),
        (
            "<!DOCTYPE r [<!ENTITY e SYSTEM 'e'>]><r>&e;</r>",
            &[("dir/e", "<i>&e;</i>")],
            ErrorKind::EntityLoop,
        ),
        (
            "<?xml version='1.0' standalone='yes'?><!DOCTYPE r SYSTEM 's'><r/>",
            &[("dir/s", "%undeclared;")],
            ErrorKind::UnknownEntity,
        ),
        (
            "<?xml version='1.0' standalone='yes'?><!DOCTYPE r SYSTEM 's'><r>&e;</r>",
            &[("dir/s", "<!ENTITY e 'external'>")],
            ErrorKind::UnknownEntity,
        ),
        (
            "<!DOCTYPE r SYSTEM 's'><r/>",
            &[("dir/s", "<!ENTITY % p '<!ELEMENT r '>%p;ANY>")],
            ErrorKind::InvalidDtd,
        ),
    ];
    for (input, files, kind) in cases {
        assert_eq!(all(input, files, Limits::default()), Err(*kind), "{input}");
    }
}

#[test]
fn external_resources_share_the_document_budgets() {
    // Billion laughs split across external entities.
    let mut files = vec![("dir/l0".to_owned(), "lol".repeat(10))];
    for level in 1..8 {
        files.push((
            format!("dir/l{level}"),
            format!("&l{};", level - 1).repeat(10),
        ));
    }
    let declarations: String = (0..8)
        .map(|level| format!("<!ENTITY l{level} SYSTEM 'l{level}'>"))
        .collect();
    let laughs = format!("<!DOCTYPE r [{declarations}]><r>&l7;</r>");
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(uri, text)| (uri.as_str(), text.as_str()))
        .collect();
    let cases: [(String, Files, Limits, ErrorKind); 5] = [
        (
            laughs,
            &files,
            Limits::default().max_entity_expansion_bytes(64 * 1024),
            ErrorKind::LimitExceeded(Limit::EntityExpansion),
        ),
        (
            "<!DOCTYPE r [<!ENTITY a SYSTEM 'a'><!ENTITY b SYSTEM 'b'>]><r>&a;&b;</r>".into(),
            &[("dir/a", "1"), ("dir/b", "2")],
            Limits::default().max_external_loads(1),
            ErrorKind::LimitExceeded(Limit::ExternalLoads),
        ),
        (
            "<!DOCTYPE r [<!ENTITY a SYSTEM 'a'>]><r>&a;</r>".into(),
            &[("dir/a", "huge")],
            Limits::default().max_external_bytes(3),
            ErrorKind::LimitExceeded(Limit::ExternalBytes),
        ),
        (
            "<!DOCTYPE r [<!ENTITY % a SYSTEM 'a'>%a;]><r/>".into(),
            &[("dir/a", "<!ENTITY % b SYSTEM 'b'>%b;"), ("dir/b", "")],
            Limits::default().max_entity_depth(1),
            ErrorKind::LimitExceeded(Limit::EntityDepth),
        ),
        (
            "<!DOCTYPE r SYSTEM 's'><r/>".into(),
            &[(
                "dir/s",
                "<!ENTITY e 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'>",
            )],
            Limits::default().max_token_bytes(32),
            ErrorKind::LimitExceeded(Limit::TokenBytes),
        ),
    ];
    for (input, files, limits, kind) in cases {
        assert_eq!(all(&input, files, limits), Err(kind), "{input}");
    }
    // At the ceilings, the same kind of document succeeds.
    let ok = Limits::default()
        .max_external_loads(2)
        .max_external_bytes(2);
    let input = "<!DOCTYPE r [<!ENTITY a SYSTEM 'a'><!ENTITY b SYSTEM 'b'>]><r>&a;&b;</r>";
    assert_eq!(
        all(input, &[("dir/a", "1"), ("dir/b", "2")], ok).as_deref(),
        Ok("<r>12</r>")
    );
}

fn fixture_root(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("yaxml-resolver-{}-{name}", std::process::id()));
    let root = base.join("root");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    root
}

#[test]
fn without_a_resolver_nothing_is_loaded() {
    let root = fixture_root("plain");
    std::fs::write(root.join("s.dtd"), "<!ATTLIST r a CDATA 'd'>").unwrap();
    std::fs::write(root.join("e.ent"), "text").unwrap();
    let input = b"<!DOCTYPE r SYSTEM 's.dtd' [<!ENTITY e SYSTEM 'e.ent'>]><r>[&e;]</r>";
    let document = Document::parse_bytes(input).unwrap();
    assert!(document.root_element().attributes().is_empty());
    assert_eq!(
        document.root_element().text_chunks().collect::<String>(),
        "[]"
    );
    let log = Log::default();
    let document = Document::parse_with(
        b"<r>&amp;</r>",
        loading(Limits::default(), "", memory(&[], &log)),
    )
    .unwrap();
    assert_eq!(
        document.root_element().text_chunks().collect::<String>(),
        "&"
    );
    assert!(log.lock().unwrap().is_empty(), "no identifier, no request");
}

fn request(uri: &str, max_bytes: usize) -> Request<'_> {
    Request {
        kind: ResourceKind::GeneralEntity,
        uri,
        public_id: None,
        max_bytes,
    }
}

#[test]
fn file_resolver_stays_inside_its_root() {
    let root = fixture_root("files");
    let outside = root.parent().unwrap().join("outside.ent");
    std::fs::write(root.join("inside.ent"), "inside").unwrap();
    std::fs::write(root.join("sub/nested.ent"), "nested").unwrap();
    std::fs::write(&outside, "secret").unwrap();
    let link = root.join("link.ent");
    std::fs::remove_file(&link).ok();
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let mut resolver = FileResolver::new(&root).unwrap();
    let canonical = root.canonicalize().unwrap();
    let file_uri = format!("file://{}/inside.ent", canonical.display());
    let absolute = format!("{}/sub/nested.ent", canonical.display());
    let cases: [(&str, Served); 12] = [
        ("inside.ent", Ok(Some(b"inside"))),
        ("sub/../sub/nested.ent", Ok(Some(b"nested"))),
        (&file_uri, Ok(Some(b"inside"))),
        (&absolute, Ok(Some(b"nested"))),
        ("missing.ent", Ok(None)),
        ("../outside.ent", Err(io::ErrorKind::PermissionDenied)),
        (
            "sub/../../outside.ent",
            Err(io::ErrorKind::PermissionDenied),
        ),
        ("/etc/passwd", Err(io::ErrorKind::PermissionDenied)),
        ("link.ent", Err(io::ErrorKind::PermissionDenied)),
        (
            "http://example.com/x.dtd",
            Err(io::ErrorKind::PermissionDenied),
        ),
        (
            "file://host/inside.ent",
            Err(io::ErrorKind::PermissionDenied),
        ),
        ("sub", Err(io::ErrorKind::InvalidInput)),
    ];
    for (uri, expected) in cases {
        let result = resolver.resolve(request(uri, 100));
        assert_eq!(
            result
                .as_ref()
                .map(|bytes| bytes.as_deref())
                .map_err(io::Error::kind),
            expected,
            "{uri}"
        );
    }
    // Reads stop one byte past the remaining budget.
    assert_eq!(
        resolver.resolve(request("inside.ent", 2)).unwrap().unwrap(),
        b"ins"
    );
    assert!(FileResolver::new(root.join("inside.ent")).is_err());
    assert!(FileResolver::new(root.join("absent")).is_err());
    // Through the parser an escape is a fatal resource error.
    let input = b"<!DOCTYPE r [<!ENTITY e SYSTEM '../outside.ent'>]><r>&e;</r>";
    let error = Document::parse_with(
        input,
        loading(Limits::default(), "doc.xml", Box::new(resolver)),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::ExternalResource);
}

fn selected<T: Tree>(tree: &T, root: T::Id, expression: &str) -> usize {
    XPath::compile(expression)
        .unwrap()
        .select_nodes(tree, root, &Context::new())
        .unwrap()
        .len()
}

#[test]
fn dtd_ids_drive_element_lookup_and_xpath_id_on_both_doms() {
    let input = "<!DOCTYPE r SYSTEM 's.dtd' [<!ATTLIST e id ID #IMPLIED c CDATA #IMPLIED>]>\
                 <r><e id='a' c='z'/><e id=' b '/><e id='a'/><f xml:id='x'/><g k='g'/></r>";
    let log = Log::default();
    let files = [("s.dtd", "<!ATTLIST g k ID #IMPLIED>")];
    let bytes = input.as_bytes();
    let borrowed =
        Document::parse_with(bytes, loading(Limits::default(), "", memory(&files, &log))).unwrap();
    let owned =
        OwnedDocument::parse_with(bytes, loading(Limits::default(), "", memory(&files, &log)))
            .unwrap();
    for (value, position) in [
        ("a", Some(0)),
        ("b", Some(1)),
        ("x", Some(3)),
        ("g", Some(4)),
        ("z", None),
        ("missing", None),
    ] {
        let found = borrowed.element_by_id(value).map(|node| {
            let mut children = borrowed.root_element().children();
            children.position(|child| child.id() == node.id()).unwrap()
        });
        assert_eq!(found, position, "borrowed {value}");
        let found = owned.element_by_id(value).map(|node| {
            let mut children = owned.root_element().children();
            children.position(|child| child.id() == node.id()).unwrap()
        });
        assert_eq!(found, position, "owned {value}");
    }
    for (expression, count) in [
        ("id('a b x g')", 4),
        ("id('a a')", 1),
        ("id(//e/@c)", 0),
        ("id('missing')", 0),
        ("id('')", 0),
        ("id(//f/@xml:id)", 1),
    ] {
        assert_eq!(
            selected(&borrowed, borrowed.root().id(), expression),
            count,
            "{expression}"
        );
        assert_eq!(
            selected(&owned, owned.root().id(), expression),
            count,
            "{expression}"
        );
    }
    // Without the external subset, `k` is not an ID.
    let plain = Document::parse(input).unwrap();
    assert!(plain.element_by_id("g").is_none());
    assert!(plain.element_by_id("a").is_some());
}

#[test]
fn owned_id_lookup_follows_edits() {
    let mut document = OwnedDocument::parse(
        "<!DOCTYPE r [<!ATTLIST e id ID #IMPLIED>]><r><e id='a'/><e id='b'/></r>",
    )
    .unwrap();
    let first = document.element_by_id("a").unwrap().id();
    let second = document.element_by_id("b").unwrap().id();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    editor.set_attribute_value(first, None, "id", "c").unwrap();
    editor.remove_subtree(second).unwrap();
    assert_eq!(
        document.element_by_id("c").map(|node| node.id()),
        Some(first)
    );
    assert!(document.element_by_id("a").is_none());
    assert!(document.element_by_id("b").is_none());
}

/// Differential check against the pinned libxml2 2.15.4 `xmllint` with
/// `--loaddtd --noent --dtdattr`, reading the same files from disk.
#[test]
#[ignore = "requires YAXML_XMLLINT pointing at the pinned libxml2 2.15.4 xmllint"]
fn oracle_xmllint_loads_the_same_external_resources() {
    let xmllint = std::env::var_os("YAXML_XMLLINT").expect("YAXML_XMLLINT");
    let root = fixture_root("oracle");
    for (name, text) in ORACLE_FILES {
        std::fs::write(root.join(name), text).unwrap();
    }
    let mut differences = Vec::new();
    for input in ORACLE_CASES {
        let path = root.join("case.xml");
        std::fs::write(&path, input).unwrap();
        let output = std::process::Command::new(&xmllint)
            .args(["--loaddtd", "--noent", "--dtdattr", "--nonet", "--dropdtd"])
            .arg(&path)
            .output()
            .unwrap();
        let oracle = output.status.success().then(|| {
            let text = String::from_utf8(output.stdout).unwrap();
            let body = text
                .split_once("?>\n")
                .map_or(text.as_str(), |(_, body)| body);
            body.trim_end().to_owned()
        });
        let resolver = Box::new(FileResolver::new(&root).unwrap());
        let bytes = input.as_bytes();
        let ours = Document::parse_with(bytes, loading(Limits::default(), "case.xml", resolver))
            .ok()
            .map(|document| {
                let mut writer = Writer::new(Vec::new());
                writer.write_document(&document).unwrap();
                let text = written(writer);
                text[text.find("<r").unwrap()..].to_owned()
            });
        if oracle != ours {
            differences.push(format!(
                "{input}\n  libxml2: {oracle:?}\n  yaxml:   {ours:?}"
            ));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

const ORACLE_FILES: &[(&str, &str)] = &[
    (
        "s.dtd",
        "<!ATTLIST r a CDATA 'd'><!ENTITY e SYSTEM 'sub/e.ent'>\
         <!ENTITY % m 'INCLUDE'><![%m;[<!ENTITY i 'inc'>]]><![IGNORE[<!ENTITY i 'no'>]]>",
    ),
    ("sub/e.ent", "<?xml encoding='UTF-8'?><b>e</b>&n;"),
    ("sub/n.ent", "nested"),
    (
        "p.ent",
        "<!ENTITY n SYSTEM 'sub/n.ent'><!ENTITY % t 'CDATA'><!ATTLIST r t %t; 't'>",
    ),
    ("loop.ent", "%loop;"),
];

const ORACLE_CASES: &[&str] = &[
    "<!DOCTYPE r SYSTEM 's.dtd' [<!ENTITY % p SYSTEM 'p.ent'>%p;]><r>&e;|&i;</r>",
    "<!DOCTYPE r [<!ENTITY % p SYSTEM 'p.ent'>%p;]><r>&n;</r>",
    "<!DOCTYPE r [<!ENTITY e SYSTEM 'missing.ent'>]><r>[&e;]</r>",
    "<!DOCTYPE r [<!ENTITY % loop SYSTEM 'loop.ent'>%loop;]><r/>",
    "<!DOCTYPE r [<!ENTITY e SYSTEM 'sub/n.ent'>]><r a='&e;'/>",
    "<!DOCTYPE r SYSTEM 's.dtd'><r>&e;</r>",
];

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
