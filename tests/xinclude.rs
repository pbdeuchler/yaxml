#![forbid(unsafe_code)]
//! XInclude against libxml2 2.15.4's `test/XInclude` suite, plus XPointer,
//! fixup, fallback, cycle and budget cases over in-memory resources.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use yaxml::xinclude::{self, ErrorKind, Options};
use yaxml::{
    FileResolver, Limit, Limits, OwnedDocument, Request, Resolver, ResourceKind, Writer, save,
};

const DEFAULT_SOURCE: &str =
    "/Users/pbdeuchler/workspace/src/github.com/pbdeuchler/yaxml-oracle/libxml2-2.15.4";

/// `xmlDocDumpMemory`, as `xmllint --xinclude` prints the result.
fn dump(document: &OwnedDocument) -> Vec<u8> {
    save::to_vec(document, &save::Options::default()).unwrap()
}

#[test]
fn libxml2_suite() {
    let source = std::env::var_os("YAXML_LIBXML2_SRC").map_or(DEFAULT_SOURCE.into(), PathBuf::from);
    let root = source.join("test/XInclude");
    if !root.is_dir() {
        eprintln!("skipping: no libxml2 source at {}", source.display());
        return;
    }
    let (mut mismatches, mut total) = (Vec::new(), 0);
    let mut errors = Vec::new();
    for directory in ["docs", "without-reader", "issue1120"] {
        let mut names: Vec<_> = fs::read_dir(root.join(directory))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        for name in names {
            let base = format!("{directory}/{name}");
            let input = fs::read(root.join(&base)).unwrap();
            // A missing result file means libxml2 fails and prints nothing.
            let expected = fs::read(source.join("result/XInclude").join(&name)).unwrap_or_default();
            let resolver = || Box::new(FileResolver::new(&root).unwrap());
            let document =
                OwnedDocument::parse_with(&input, loading(Limits::default(), &base, resolver()))
                    .unwrap();
            let result = xinclude::process(&document, &base, resolver(), &Options::default());
            let ours = result.as_ref().map_or_else(|_| Vec::new(), dump);
            if let Err(error) = result {
                errors.push((base.clone(), error.kind));
            }
            total += 1;
            if ours != expected {
                mismatches.push(format!(
                    "{base}:\nexpected:\n{}\nyaxml:\n{}",
                    String::from_utf8_lossy(&expected),
                    String::from_utf8_lossy(&ours)
                ));
            }
        }
    }
    eprintln!(
        "libxml2 XInclude suite: {} of {total} match",
        total - mismatches.len()
    );
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    // The suite's failing inputs, with libxml2's first reported error.
    assert_eq!(
        errors,
        [
            ("docs/invalid_char.xml", ErrorKind::InvalidCharacter),
            ("docs/nodes3.xml", ErrorKind::FragmentIdentifier),
            ("without-reader/loop.xml", ErrorKind::Recursion),
            ("without-reader/max-recurse.xml", ErrorKind::RecursionDepth),
        ]
        .map(|(base, kind)| (String::from(base), kind)),
    );
}

const XI: &str = "xmlns:xi='http://www.w3.org/2001/XInclude'";

type Requests = Arc<Mutex<Vec<(ResourceKind, String)>>>;

/// Serves `files` from memory and records every request. `err.xml` fails.
fn memory(files: &[(&str, &[u8])]) -> (Box<dyn Resolver>, Requests) {
    let files: HashMap<String, Vec<u8>> = files
        .iter()
        .map(|(name, bytes)| (name.to_string(), bytes.to_vec()))
        .collect();
    let requests = Requests::default();
    let log = requests.clone();
    let resolver = move |request: Request<'_>| {
        log.lock()
            .unwrap()
            .push((request.kind, request.uri.to_owned()));
        match request.uri {
            "err.xml" => Err(std::io::Error::other("denied")),
            uri => Ok(files.get(uri).cloned()),
        }
    };
    (Box::new(resolver), requests)
}

/// Processes `main` (at `main.xml`) and writes the result compactly.
fn run_with(
    main: &str,
    files: &[(&str, &[u8])],
    options: &Options,
) -> Result<String, xinclude::Error> {
    let (resolver, _) = memory(files);
    let document = OwnedDocument::parse_with(
        main.as_bytes(),
        loading(Limits::default(), "main.xml", memory(files).0),
    )
    .unwrap();
    let result = xinclude::process(&document, "main.xml", resolver, options)?;
    let mut writer = Writer::new(Vec::new());
    writer.write_owned_document(&result).unwrap();
    Ok(String::from_utf8(writer.into_inner()).unwrap())
}

fn run(main: &str, files: &[(&str, &[u8])]) -> Result<String, xinclude::Error> {
    run_with(main, files, &Options::default())
}

fn wrap(content: &str) -> String {
    format!("<r {XI}>{content}</r>")
}

fn include(attributes: &str) -> String {
    wrap(&format!("<xi:include {attributes}/>"))
}

fn included(content: &str) -> String {
    format!("<r xmlns:xi=\"http://www.w3.org/2001/XInclude\">{content}</r>")
}

const FILES: &[(&str, &[u8])] = &[
    ("a.xml", b"<a><b>1</b><b>2</b></a>"),
    ("sub/a.xml", b"<a xml:base='x/'><c/></a>"),
    ("ids.xml", b"<!DOCTYPE d [<!ATTLIST e k ID #IMPLIED>]><d><e k='one'/><e k='two'/></d>"),
    ("ns.xml", b"<p:a xmlns:p='urn:p'><p:b q:c='1' xmlns:q='urn:q'/><x xmlns='urn:d'><y/></x></p:a>"),
    ("plain.xml", b"<y><z/></y>"),
    ("unbound.xml", b"<x xmlns='urn:d'><y q:a='1'/><z xmlns=''/></x>"),
    ("lang.xml", b"<l xml:lang='en'><m/></l>"),
    ("abs.xml", b"<h xml:base='http://example.org/h/'/>"),
    ("t.txt", b"a < b & c\r\n"),
    ("latin.txt", b"caf\xe9"),
    ("utf16.txt", b"\xff\xfeh\x00i\x00"),
    ("bad.txt", b"\xff"),
    ("ctl.txt", b"\x01"),
    ("bad.xml", b"<a>"),
    ("self.xml", b"<s xmlns:xi='http://www.w3.org/2001/XInclude'><xi:include href='self2.xml'/></s>"),
    ("self2.xml", b"<s2 xmlns:xi='http://www.w3.org/2001/XInclude'><xi:include href='self.xml'/></s2>"),
    ("nested.xml", b"<n xmlns:xi='http://www.w3.org/2001/XInclude'><xi:include href='t.txt' parse='text'/></n>"),
];

#[test]
fn inclusions_match_libxml2() {
    let cases: &[(&str, String, &str)] = &[
        (
            "document",
            include("href='a.xml'"),
            "<a><b>1</b><b>2</b></a>",
        ),
        (
            "base fixup in a subdirectory",
            include("href='sub/a.xml'"),
            r#"<a xml:base="sub/x/"><c/></a>"#,
        ),
        (
            "absolute bases are kept absolute",
            include("href='abs.xml'"),
            r#"<h xml:base="http://example.org/h/"/>"#,
        ),
        (
            "shorthand pointer",
            include("href='ids.xml' xpointer='two'"),
            r#"<!DOCTYPE r><r xmlns:xi="http://www.w3.org/2001/XInclude"><e k="two"/></r>"#,
        ),
        (
            "element scheme",
            include("href='a.xml' xpointer='element(/1/2)'"),
            "<b>2</b>",
        ),
        (
            "xpointer scheme",
            include("href='a.xml' xpointer='xpointer(//b/text())'"),
            "12",
        ),
        (
            "legacy fragment identifier",
            include("href='a.xml#xpointer(//b[1])'"),
            "<b>1</b>",
        ),
        (
            "local pointer",
            format!("<r {XI}><k>v</k><xi:include xpointer='xpointer(//k)'/></r>"),
            "<k>v</k><k>v</k>",
        ),
        (
            "empty selection",
            include("href='a.xml' xpointer='nothing'"),
            r#"<r xmlns:xi="http://www.w3.org/2001/XInclude"/>"#,
        ),
        (
            "text",
            include("href='t.txt' parse='text'"),
            "a &lt; b &amp; c&#13;\n",
        ),
        (
            "text in a declared charset",
            include("href='latin.txt' parse='text' encoding='ISO-8859-1'"),
            "café",
        ),
        (
            "UTF-16 text keeps its BOM",
            include("href='utf16.txt' parse='text' encoding='UTF-16'"),
            "\u{feff}hi",
        ),
        (
            "nested text include",
            include("href='nested.xml'"),
            "<n xmlns:xi=\"http://www.w3.org/2001/XInclude\">a &lt; b &amp; c&#13;\n</n>",
        ),
        (
            "fallback for a missing resource",
            wrap("<xi:include href='missing.xml'><xi:fallback><f/>t</xi:fallback></xi:include>"),
            "<f/>t",
        ),
        (
            "fallback for a failing resolver",
            wrap("<xi:include href='err.xml'><xi:fallback>f</xi:fallback></xi:include>"),
            "f",
        ),
        (
            "fallback for a malformed document",
            wrap("<xi:include href='bad.xml'><xi:fallback/></xi:include>"),
            r#"<r xmlns:xi="http://www.w3.org/2001/XInclude"/>"#,
        ),
        (
            "nested fallback inclusion",
            wrap(
                "<xi:include href='missing.xml'><xi:fallback><xi:include href='plain.xml'/></xi:fallback></xi:include>",
            ),
            "<y><z/></y>",
        ),
        (
            "namespaces declared outside the copy",
            include("href='ns.xml' xpointer='xmlns(p=urn:p) xmlns(d=urn:d) xpointer(//p:b|//d:x)'"),
            r#"<p:b xmlns:q="urn:q" xmlns:p="urn:p" q:c="1"/><x xmlns="urn:d"><y/></x>"#,
        ),
        (
            "namespaces from xmlns()",
            include("href='ns.xml' xpointer='xmlns(t=urn:p) xpointer(//t:b) '"),
            r#"<p:b xmlns:q="urn:q" xmlns:p="urn:p" q:c="1"/>"#,
        ),
        (
            "default namespace declared outside the copy",
            include("href='ns.xml' xpointer='xmlns(d=urn:d) xpointer(//d:y)'"),
            r#"<y xmlns="urn:d"/>"#,
        ),
        (
            "a prefix a namespace error left unbound stays undeclared",
            format!(
                "<r xmlns='urn:r' {XI}><xi:include href='unbound.xml' xpointer='xmlns(d=urn:d) xpointer(//d:y|//z)'/></r>"
            ),
            r#"<r xmlns="urn:r" xmlns:xi="http://www.w3.org/2001/XInclude"><y xmlns="urn:d" q:a="1"/><z xmlns=""/></r>"#,
        ),
        (
            "unqualified element under a default namespace: libxml2 omits xmlns=\"\"",
            format!("<r xmlns='urn:r' {XI}><xi:include href='plain.xml'/></r>"),
            r#"<r xmlns="urn:r" xmlns:xi="http://www.w3.org/2001/XInclude"><y xmlns=""><z/></y></r>"#,
        ),
        (
            "IDs from an included DTD keep a DOCTYPE",
            format!("<r {XI}><xi:include href='ids.xml' xpointer='one'/></r>"),
            r#"<!DOCTYPE r><r xmlns:xi="http://www.w3.org/2001/XInclude"><e k="one"/></r>"#,
        ),
    ];
    for (label, main, expected) in cases {
        let expected = match expected.starts_with("<r ") || expected.starts_with("<!") {
            true => expected.to_string(),
            false => included(expected),
        };
        let output = run(main, FILES).unwrap_or_else(|error| panic!("{label}: {error}"));
        assert_eq!(output, expected, "{label}");
    }
}

#[test]
fn inclusion_failures_are_structured() {
    let cases: &[(&str, String, ErrorKind)] = &[
        ("no href", include(""), ErrorKind::NoHref),
        (
            "bad parse",
            include("href='a.xml' parse='html'"),
            ErrorKind::InvalidParse,
        ),
        ("bad href", include("href='%zz'"), ErrorKind::InvalidHref),
        (
            "fragment in the 2003 namespace",
            "<r xmlns:xi='http://www.w3.org/2003/XInclude'><xi:include href='a.xml#x'/></r>".into(),
            ErrorKind::FragmentIdentifier,
        ),
        (
            "local without pointer",
            include("href='main.xml'"),
            ErrorKind::Recursion,
        ),
        (
            "cycle across documents",
            include("href='self.xml'"),
            ErrorKind::Recursion,
        ),
        (
            "local loop",
            format!(
                "<r {XI}><a><xi:include xpointer='xpointer(//b)'/></a><b><xi:include xpointer='xpointer(//a)'/></b></r>"
            ),
            ErrorKind::Recursion,
        ),
        (
            "text of itself",
            include("xpointer='x' parse='text'"),
            ErrorKind::TextDocument,
        ),
        (
            "unknown encoding",
            include("href='t.txt' parse='text' encoding='EBCDIC'"),
            ErrorKind::UnknownEncoding,
        ),
        (
            "invalid UTF-8 text",
            include("href='bad.txt' parse='text'"),
            ErrorKind::InvalidCharacter,
        ),
        (
            "control character text",
            include("href='ctl.txt' parse='text'"),
            ErrorKind::InvalidCharacter,
        ),
        (
            "unmapped byte",
            include("href='bad.txt' parse='text' encoding='US-ASCII'"),
            ErrorKind::InvalidCharacter,
        ),
        (
            "XPointer syntax",
            include("href='a.xml' xpointer='xpointer(//b'"),
            ErrorKind::XPointer(yaxml::xpointer::Error::Syntax),
        ),
        (
            "attribute selection",
            include("href='ids.xml' xpointer='xpointer(//@k)'"),
            ErrorKind::XPointerResult,
        ),
        (
            "missing without fallback",
            include("href='missing.xml'"),
            ErrorKind::NoFallback(None),
        ),
        (
            "two top-level elements",
            format!("<xi:include {XI} href='a.xml' xpointer='xpointer(//b)'/>"),
            ErrorKind::MultipleRoots,
        ),
        (
            "include in include",
            wrap("<xi:include href='a.xml'><xi:include href='a.xml'/></xi:include>"),
            ErrorKind::IncludeInInclude,
        ),
        (
            "two fallbacks",
            wrap("<xi:include href='a.xml'><xi:fallback/><xi:fallback/></xi:include>"),
            ErrorKind::MultipleFallbacks,
        ),
        (
            "stray fallback",
            format!("<r {XI}><xi:fallback/></r>"),
            ErrorKind::FallbackNotInInclude,
        ),
    ];
    for (label, main, kind) in cases {
        let error = run(main, FILES).expect_err(label);
        assert_eq!(&error.kind, kind, "{label}");
        if *label != "cycle across documents" {
            assert_eq!(error.uri, "main.xml", "{label}");
        }
    }
    let error = run(&include("href='bad.xml'"), FILES).unwrap_err();
    assert!(
        matches!(error.kind, ErrorKind::NoFallback(Some(_))),
        "{error}"
    );
    // A cycle is reported in the document that closes it.
    assert_eq!(
        run(&include("href='self.xml'"), FILES).unwrap_err().uri,
        "self2.xml"
    );
}

#[test]
fn fixups_follow_options() {
    let no_base = Options::default().base_fixup(false);
    let lang = Options::default().lang_fixup(true);
    let lang_main = |href: &str| format!("<r xml:lang='fr' {XI}><xi:include href='{href}'/></r>");
    let cases: &[(&str, String, &Options, &str)] = &[
        (
            "NOBASEFIX",
            include("href='sub/a.xml'"),
            &no_base,
            r#"<a xml:base="x/"><c/></a>"#,
        ),
        (
            "same directory",
            include("href='plain.xml'"),
            &Options::default(),
            "<y><z/></y>",
        ),
        (
            "different language",
            lang_main("lang.xml"),
            &lang,
            r#"<l xml:lang="en"><m/></l>"#,
        ),
        (
            "undetermined language",
            lang_main("plain.xml"),
            &lang,
            r#"<y xml:lang=""><z/></y>"#,
        ),
        (
            "libxml2 omits language",
            lang_main("plain.xml"),
            &Options::default(),
            "<y><z/></y>",
        ),
    ];
    for (label, main, options, expected) in cases {
        let output = run_with(main, FILES, options).unwrap();
        let expected = match main.contains("xml:lang") {
            true => format!(
                r#"<r xml:lang="fr" xmlns:xi="http://www.w3.org/2001/XInclude">{expected}</r>"#
            ),
            false => included(expected),
        };
        // Declarations precede other attributes in the result.
        let expected = expected.replace(
            r#"<r xml:lang="fr" xmlns:xi="http://www.w3.org/2001/XInclude">"#,
            r#"<r xmlns:xi="http://www.w3.org/2001/XInclude" xml:lang="fr">"#,
        );
        assert_eq!(output, expected, "{label}");
    }
}

#[test]
fn resources_load_once_and_ids_survive() {
    let main = format!(
        "<r {XI}><xi:include href='ids.xml'/><xi:include href='ids.xml' xpointer='one'/>\
         <xi:include href='t.txt' parse='text'/><xi:include href='t.txt' parse='text'/></r>"
    );
    let (resolver, requests) = memory(FILES);
    let document = OwnedDocument::parse(&main).unwrap();
    let result = xinclude::process(&document, "main.xml", resolver, &Options::default()).unwrap();
    assert_eq!(
        *requests.lock().unwrap(),
        [
            (ResourceKind::XInclude, String::from("ids.xml")),
            (ResourceKind::XIncludeText, String::from("t.txt")),
        ]
    );
    // The DTD-declared ID survives the copy; the first element in document order wins.
    let found = result.element_by_id("two").unwrap();
    assert_eq!(found.attributes().next().unwrap().value, "two");
    assert_eq!(
        result
            .element_by_id("one")
            .unwrap()
            .parent()
            .unwrap()
            .name()
            .unwrap()
            .local(),
        "d"
    );
    // The input is unchanged.
    assert_eq!(document.root_element().children().count(), 4);
}

#[test]
fn budgets_stop_inclusion_bombs() {
    // Each level includes the next ten times: 10^5 leaves from six small documents.
    let level = |next: &str| {
        format!(
            "<l {XI}>{}</l>",
            format!("<xi:include href='{next}'/>").repeat(10)
        )
    };
    let documents: Vec<(String, Vec<u8>)> = (1..=5)
        .map(|index| {
            let next = match index {
                5 => String::from("<x/>"),
                _ => level(&format!("l{}.xml", index + 1)),
            };
            (format!("l{index}.xml"), next.into_bytes())
        })
        .collect();
    let files: Vec<(&str, &[u8])> = documents
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    let main = level("l1.xml");
    let limited = |limits: Limits| Options::default().limits(limits);
    let cases = [
        (limited(Limits::default().max_events(50_000)), Limit::Events),
        (
            limited(Limits::default().max_owned_bytes(4096)),
            Limit::OwnedBytes,
        ),
        (
            limited(Limits::default().max_external_loads(3)),
            Limit::ExternalLoads,
        ),
        (
            limited(Limits::default().max_external_bytes(100)),
            Limit::ExternalBytes,
        ),
    ];
    for (options, limit) in cases {
        let error = run_with(&main, &files, &options).unwrap_err();
        match error.kind {
            ErrorKind::Xml(error) => assert_eq!(error.kind, yaxml::ErrorKind::LimitExceeded(limit)),
            other => panic!("{limit:?}: {other:?}"),
        }
    }
    // Within budget, each document is still loaded once.
    let small = Options::default().max_depth(3);
    assert_eq!(
        run_with(&main, &files, &small).unwrap_err().kind,
        ErrorKind::RecursionDepth
    );
    let (resolver, requests) = memory(&files);
    let document = OwnedDocument::parse(&main).unwrap();
    let result = xinclude::process(&document, "main.xml", resolver, &Options::default()).unwrap();
    assert_eq!(requests.lock().unwrap().len(), 5);
    assert_eq!(
        result
            .nodes()
            .filter(|node| node.name().is_some_and(|name| name.local() == "x"))
            .count(),
        100_000
    );
}

#[test]
fn no_inclusion_keeps_the_parse_node_ceiling() {
    // `max_events` bounds the parse's nodes less the document node; walking
    // a document without inclusions must accept exactly what parsing does.
    // Each input has as many nodes as events (no element has an end tag).
    let events = yaxml::ErrorKind::LimitExceeded(Limit::Events);
    for (input, nodes) in [
        ("<!DOCTYPE r><r/>", 2),
        ("<r/>", 1),
        ("<?p?><!--c--><r/>", 3),
        (format!("<r {XI}/>").as_str(), 1),
    ] {
        for (max_events, accepted) in [(nodes - 1, false), (nodes, true)] {
            let limits = Limits::default().max_events(max_events);
            for flags in [0, yaxml::options::XINCLUDE] {
                let mut options = loading(limits, "main.xml", memory(&[]).0);
                options.flags |= flags;
                let result = OwnedDocument::parse_with(input.as_bytes(), options);
                assert_eq!(
                    result.err().map(|error| error.kind),
                    (!accepted).then_some(events),
                    "{input} {flags}"
                );
            }
            let document = OwnedDocument::parse(input).unwrap();
            let options = Options::default().limits(limits);
            let result = xinclude::process(&document, "main.xml", memory(&[]).0, &options);
            match accepted {
                true => assert_eq!(result.unwrap().nodes().count(), nodes + 1, "{input}"),
                false => assert!(
                    matches!(result.unwrap_err().kind, ErrorKind::Xml(error) if error.kind == events),
                    "{input}"
                ),
            }
        }
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

#[test]
fn included_entities_merge_like_libxml2() {
    // Recorded with xmllint --xinclude --loaddtd; libxml2 walks a hash table,
    // so its order differs, and it writes `&quot;` for a quote.
    let files: &[(&str, &[u8])] = &[
        ("ent.xml", b"<!DOCTYPE e [<!NOTATION n SYSTEM 'n'><!ENTITY i 'a&amp;b&#37;c&#38;#38;d'><!ENTITY u SYSTEM 'u.bin' NDATA n><!ENTITY % p 'x'><!ENTITY q '\"'><!ENTITY lt '&#38;#60;'>]><e/>"),
        ("nest.xml", b"<!DOCTYPE n [<!ENTITY mid 'm'>]><n><xi:include href='ent.xml' xmlns:xi='http://www.w3.org/2001/XInclude'/></n>"),
        ("s.dtd", b"<!ENTITY ext 'external'>"),
        ("es.xml", b"<!DOCTYPE e SYSTEM 's.dtd' [<!ENTITY int 'i'>]><e/>"),
        ("mb.xml", "<!DOCTYPE e [<!ENTITY m '\u{e9}&amp;\u{fc}&#37;'>]><e/>".as_bytes()),
    ];
    let ent = r#"<!ENTITY i "a&amp;b&#x25;c&#38;#38;d"><!ENTITY u SYSTEM "u.bin" NDATA n><!ENTITY q "&#34;">"#;
    let body =
        |content: &str| format!(r#"<r xmlns:xi="http://www.w3.org/2001/XInclude">{content}</r>"#);
    let cases: &[(&str, String, Result<String, ErrorKind>)] = &[
        (
            "general entities, not parameter or predefined ones",
            include("href='ent.xml'"),
            Ok(format!("<!DOCTYPE r [{ent}]>{}", body("<e/>"))),
        ),
        (
            "a declared name keeps its declaration",
            format!(
                "<!DOCTYPE r [<!ENTITY i 'mine'>]>{}",
                include("href='ent.xml'")
            ),
            Ok(format!(
                r#"<!DOCTYPE r [<!ENTITY i 'mine'><!ENTITY u SYSTEM "u.bin" NDATA n><!ENTITY q "&#34;">]>{}"#,
                body("<e/>")
            )),
        ),
        (
            "an equal unparsed entity is no mismatch",
            format!(
                "<!DOCTYPE r [<!NOTATION n SYSTEM 'n'><!ENTITY u SYSTEM 'u.bin' NDATA n>]>{}",
                include("href='ent.xml'")
            ),
            Ok(format!(
                r#"<!DOCTYPE r [<!NOTATION n SYSTEM 'n'><!ENTITY u SYSTEM 'u.bin' NDATA n><!ENTITY i "a&amp;b&#x25;c&#38;#38;d"><!ENTITY q "&#34;">]>{}"#,
                body("<e/>")
            )),
        ),
        (
            "another unparsed entity of that name",
            format!(
                "<!DOCTYPE r [<!NOTATION n SYSTEM 'n'><!ENTITY u SYSTEM 'other.bin' NDATA n>]>{}",
                include("href='ent.xml'")
            ),
            Err(ErrorKind::EntityMismatch("u".into())),
        ),
        (
            "an internal entity of that name",
            format!(
                "<!DOCTYPE r [<!ENTITY u 'text'>]>{}",
                include("href='ent.xml'")
            ),
            Err(ErrorKind::EntityMismatch("u".into())),
        ),
        (
            "entities of nested inclusions stay in their includer",
            include("href='nest.xml'"),
            Ok(format!(
                r#"<!DOCTYPE r [<!ENTITY mid "m">]>{}"#,
                body("<n><e/></n>")
            )),
        ),
        (
            "the external subset merges unless an identifier matches",
            include("href='es.xml'"),
            Ok(format!(
                r#"<!DOCTYPE r [<!ENTITY int "i">]>{}"#,
                body("<e/>")
            )),
        ),
        (
            "a shared external subset is not merged",
            format!("<!DOCTYPE r SYSTEM 's.dtd'>{}", include("href='es.xml'")),
            Ok(format!(
                r#"<!DOCTYPE r SYSTEM "s.dtd" [<!ENTITY int "i">]>{}"#,
                body("<e/>")
            )),
        ),
        (
            "a value with characters of more than one byte",
            include("href='mb.xml'"),
            Ok(format!(
                "<!DOCTYPE r [<!ENTITY m \"\u{e9}&amp;\u{fc}&#x25;\">]>{}",
                body("<e/>")
            )),
        ),
    ];
    for (label, main, expected) in cases {
        let result = run(main, files).map_err(|error| {
            assert_eq!(error.uri, "ent.xml", "{label}");
            error.kind
        });
        assert_eq!(&result, expected, "{label}");
    }
}

/// `xmlXIncludeProcessNode` on the last element named `at` (`None`: the
/// document node); results recorded with libxml2 2.15.4.
#[test]
fn subtree_inclusions_match_libxml2() {
    let main = wrap(
        "<a><xi:include href='plain.xml'/></a><c><xi:include href='plain.xml'/><xi:fallback/></c>\
         <xi:include href='t.txt' parse='text'/>",
    );
    let fallback = Err(ErrorKind::FallbackNotInInclude);
    let cases: &[(Option<&str>, Result<&str, ErrorKind>)] = &[
        (
            Some("a"),
            Ok(
                "<a><y><z/></y></a><c><xi:include href=\"plain.xml\"/><xi:fallback/></c><xi:include href=\"t.txt\" parse=\"text\"/>",
            ),
        ),
        (
            Some("xi:include"),
            Ok(
                "<a><xi:include href=\"plain.xml\"/></a><c><xi:include href=\"plain.xml\"/><xi:fallback/></c>a &lt; b &amp; c&#13;\n",
            ),
        ),
        (Some("r"), fallback.clone()),
        (Some("c"), fallback.clone()),
        (Some("xi:fallback"), fallback.clone()),
        (None, fallback),
    ];
    let (resolver, _) = memory(FILES);
    let options = loading(Limits::default(), "main.xml", resolver);
    let document = OwnedDocument::parse_with(main.as_bytes(), options).unwrap();
    for (at, expected) in cases {
        let node = match at {
            Some(at) => document
                .nodes()
                .filter(|n| n.name().is_some_and(|name| name.qualified() == *at))
                .last(),
            None => Some(document.root()),
        };
        let (resolver, _) = memory(FILES);
        let result = xinclude::process_node(
            &document,
            node.unwrap().id(),
            "main.xml",
            resolver,
            &Options::default(),
        )
        .expect("an attached node");
        let result = result.map(|result| {
            let mut writer = Writer::new(Vec::new());
            writer.write_owned_document(&result).unwrap();
            String::from_utf8(writer.into_inner()).unwrap()
        });
        let expected = expected.clone().map(included);
        assert_eq!(result.map_err(|error| error.kind), expected, "{at:?}");
    }
    // A detached or foreign node is not in the tree.
    let mut edited = document.try_clone().unwrap();
    let a = edited
        .nodes()
        .find(|n| n.name().is_some_and(|name| name.local() == "a"))
        .unwrap()
        .id();
    edited.edit(Default::default()).unwrap().unlink(a).unwrap();
    for (tree, node) in [(&edited, a), (&edited, document.root().id())] {
        let (resolver, _) = memory(FILES);
        assert!(
            xinclude::process_node(tree, node, "main.xml", resolver, &Options::default()).is_none()
        );
    }
}
