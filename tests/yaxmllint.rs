//! `yaxmllint` end to end: each option family, exit codes, and (with
//! `YAXML_XMLLINT` naming the pinned xmllint) a stdout and exit code
//! differential over libxml2's own test inputs.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

const DEFAULT_SOURCE: &str =
    "/Users/pbdeuchler/workspace/src/github.com/pbdeuchler/yaxml-oracle/libxml2-2.15.4";

struct Run {
    stdout: Vec<u8>,
    stderr: String,
    code: i32,
}

fn scratch(files: &[(&str, &str)]) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "yaxmllint-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    for (name, text) in files {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_program(
    program: &Path,
    args: &[&str],
    dir: &Path,
    stdin: &str,
    env: &[(&str, &str)],
) -> Run {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(dir)
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    Run {
        stdout: output.stdout,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code().unwrap(),
    }
}

fn yaxmllint(args: &[&str], dir: &Path, stdin: &str, env: &[(&str, &str)]) -> Run {
    run_program(
        Path::new(env!("CARGO_BIN_EXE_yaxmllint")),
        args,
        dir,
        stdin,
        env,
    )
}

const BARE: &str = "<a><b>x</b><b>y</b></a>";
const A: &str = r#"<a><b k="1">x</b><b>y</b></a>"#;
const DTD_OK: &str = "<!DOCTYPE a [<!ELEMENT a (b)><!ELEMENT b (#PCDATA)>]><a><b>x</b></a>";
const DTD_BAD: &str = "<!DOCTYPE a [<!ELEMENT a (b)><!ELEMENT b (#PCDATA)>]><a><c/></a>";
const RNG: &str = r#"<element name="a" xmlns="http://relaxng.org/ns/structure/1.0"><oneOrMore><element name="b"><text/></element></oneOrMore></element>"#;
const XSD: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="a"><xs:complexType><xs:sequence><xs:element name="b" type="xs:string" maxOccurs="unbounded"/></xs:sequence></xs:complexType></xs:element></xs:schema>"#;
const XI: &str =
    r#"<r xmlns:xi="http://www.w3.org/2001/XInclude"><xi:include href="inc.xml"/></r>"#;
const XI_MISSING: &str =
    r#"<r xmlns:xi="http://www.w3.org/2001/XInclude"><xi:include href="no.xml"/></r>"#;
const EXT: &str = r#"<!DOCTYPE r SYSTEM "r.dtd"><r>&e;</r>"#;
const CATALOG: &str = r#"<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog"><rewriteSystem systemIdStartString="http://example.com/" rewritePrefix="sub/"/></catalog>"#;

/// Expected stdout: all of it, a fragment of it, or nothing.
enum Out {
    Exact(&'static str),
    Has(&'static str),
    Empty,
}
use Out::*;

struct Case {
    name: &'static str,
    files: &'static [(&'static str, &'static str)],
    args: &'static [&'static str],
    stdin: &'static str,
    env: &'static [(&'static str, &'static str)],
    code: i32,
    out: Out,
    err: &'static str,
}

const fn case(name: &'static str, args: &'static [&'static str], code: i32, out: Out) -> Case {
    Case {
        name,
        files: &[("a.xml", A)],
        args,
        stdin: "",
        env: &[],
        code,
        out,
        err: "",
    }
}

const fn on(
    mut c: Case,
    files: &'static [(&'static str, &'static str)],
    err: &'static str,
) -> Case {
    c.files = files;
    c.err = err;
    c
}

const fn err(mut c: Case, err: &'static str) -> Case {
    c.err = err;
    c
}

#[rustfmt::skip]
fn cases() -> Vec<Case> {
    vec![
        // exit codes and parsing
        case("wellformed noout", &["--noout", "a.xml"], 0, Empty),
        case("default output", &["a.xml"], 0, Exact("<?xml version=\"1.0\"?>\n<a><b k=\"1\">x</b><b>y</b></a>\n")),
        err(on(case("malformed", &["a.xml"], 4, Empty), &[("a.xml", "<a>\n<b>\n</a>")], ""), "a.xml:3: parser error"),
        err(case("missing file", &["nope.xml"], 4, Empty), "failed to load"),
        err(case("no arguments", &[], 1, Empty), "Usage"),
        err(case("unknown option", &["--bogus", "a.xml"], 1, Empty), "Unknown option --bogus"),
        err(case("unsupported option", &["--shell", "a.xml"], 1, Empty), "unsupported option --shell"),
        err(case("missing value", &["a.xml", "--xpath"], 1, Empty), "missing value"),
        err(case("bad integer", &["--maxmem", "lots", "a.xml"], 1, Empty), "invalid integer"),
        err(case("memory limit", &["--maxmem", "10", "a.xml"], 9, Empty), "LimitExceeded"),
        case("version", &["--version"], 0, Has("yaxmllint using yaxml")),
        case("no-op options", &["--nonet", "--noent", "--nowarning", "--push", "--noout", "a.xml"], 0, Empty),
        case("huge", &["--huge", "--noout", "a.xml"], 0, Empty),
        case("timing", &["--timing", "--noout", "a.xml"], 0, Empty),
        err(on(case("last failure wins", &["good.xml", "bad.xml"], 4, Has("<good/>")), &[("good.xml", "<good/>"), ("bad.xml", "<bad>")], ""), "bad.xml:1"),
        Case { stdin: "<a/>", ..case("stdin", &["-"], 0, Exact("<?xml version=\"1.0\"?>\n<a/>\n")) },
        // output
        case("format", &["--format", "a.xml"], 0, Exact("<?xml version=\"1.0\"?>\n<a>\n  <b k=\"1\">x</b>\n  <b>y</b>\n</a>\n")),
        case("pretty 1", &["--pretty", "1", "a.xml"], 0, Has("\n  <b k=\"1\">x</b>\n")),
        on(case("noblanks", &["--noblanks", "a.xml"], 0, Exact("<?xml version=\"1.0\"?>\n<a><b/></a>\n")), &[("a.xml", "<a>\n <b/>\n</a>")], ""),
        on(case("noblanks keeps xml:space", &["--noblanks", "a.xml"], 0, Has("<a xml:space=\"preserve\"> <b/> </a>")), &[("a.xml", "<a xml:space='preserve'> <b/> </a>")], ""),
        on(case("encode", &["--encode", "ISO-8859-1", "a.xml"], 0, Has("encoding=\"ISO-8859-1\"")), &[("a.xml", "<a>x</a>")], ""),
        err(case("bad encoding", &["--encode", "nonsense", "a.xml"], 6, Empty), "failed save"),
        case("output file", &["-o", "out.xml", "a.xml"], 0, Empty),
        // xpath
        case("xpath nodes", &["--xpath", "//b", "a.xml"], 0, Exact("<b k=\"1\">x</b>\n<b>y</b>\n")),
        case("xpath count", &["--xpath", "count(//b)", "a.xml"], 0, Exact("2\n")),
        case("xpath string", &["--xpath", "string(//b)", "a.xml"], 0, Exact("x\n")),
        case("xpath attribute", &["--xpath", "//b/@k", "a.xml"], 0, Exact(" k=\"1\"\n")),
        case("xpath boolean", &["--xpath", "true()", "a.xml"], 0, Exact("true\n")),
        case("xpath fraction", &["--xpath", "1 div 3", "a.xml"], 0, Exact("0.333333\n")),
        case("xpath large", &["--xpath", "1000000", "a.xml"], 0, Exact("1e+06\n")),
        case("xpath six digits", &["--xpath", "100000", "a.xml"], 0, Exact("100000\n")),
        case("xpath small", &["--xpath", "0.00001", "a.xml"], 0, Exact("1e-05\n")),
        case("xpath infinity", &["--xpath", "1 div 0", "a.xml"], 0, Exact("Infinity\n")),
        case("xpath nan", &["--xpath", "0 div 0", "a.xml"], 0, Exact("NaN\n")),
        case("xpath0", &["--xpath0", "string(//b)", "a.xml"], 0, Exact("x\0")),
        err(case("xpath empty", &["--xpath", "//zz", "a.xml"], 11, Empty), "XPath set is empty"),
        case("xpath empty quiet", &["--quiet", "--xpath", "//zz", "a.xml"], 11, Empty),
        err(case("xpath syntax error", &["--xpath", "((", "a.xml"], 10, Empty), "XPath compilation failure"),
        err(case("xpath undefined prefix", &["--xpath", "//p:b", "a.xml"], 10, Empty), "XPath evaluation failure"),
        // canonical forms
        on(case("c14n", &["--c14n", "a.xml"], 0, Exact("<a a=\"2\" b=\"1\"><!--c--></a>")), &[("a.xml", "<a b='1' a=\"2\"><!--c--></a>")], ""),
        on(case("exc-c14n", &["--exc-c14n", "a.xml"], 0, Exact("<r><p:e xmlns:p=\"urn:p\"></p:e></r>")), &[("a.xml", "<r xmlns:p='urn:p'><p:e/></r>")], ""),
        on(case("c14n11", &["--c14n11", "a.xml"], 0, Exact("<a></a>")), &[("a.xml", "<a/>")], ""),
        case("c14n of malformed", &["--c14n", "bad.xml"], 4, Empty),
        // html
        on(case("html", &["--html", "a.html"], 0, Has("<p>hi</p>")), &[("a.html", "<p>hi")], ""),
        on(case("html noout", &["--html", "--noout", "a.html"], 0, Empty), &[("a.html", "<p>hi")], ""),
        case("html missing", &["--html", "nope.html"], 4, Empty),
        // DTD validation
        on(case("valid", &["--valid", "--noout", "a.xml"], 0, Empty), &[("a.xml", DTD_OK)], ""),
        err(on(case("invalid", &["--valid", "--noout", "a.xml"], 3, Empty), &[("a.xml", DTD_BAD)], ""), "validity error"),
        err(case("valid without DTD is reported, not fatal", &["--valid", "--noout", "a.xml"], 0, Empty), "no DTD found"),
        err(case("postvalid without DTD", &["--postvalid", "--noout", "a.xml"], 3, Empty), "does not validate"),
        err(on(case("invalid still prints", &["--valid", "a.xml"], 3, Has("<c/>")), &[("a.xml", DTD_BAD)], ""), "validity error"),
        on(case("postvalid", &["--postvalid", "--noout", "a.xml"], 0, Empty), &[("a.xml", DTD_OK)], ""),
        err(on(case("postvalid fails", &["--postvalid", "--noout", "a.xml"], 3, Empty), &[("a.xml", DTD_BAD)], ""), "does not validate"),
        on(case("dtdvalid", &["--dtdvalid", "a.dtd", "--noout", "a.xml"], 0, Empty), &[("a.xml", "<a><b>x</b></a>"), ("a.dtd", "<!ELEMENT a (b)><!ELEMENT b (#PCDATA)>")], ""),
        err(on(case("dtdvalid fails", &["--dtdvalid", "a.dtd", "--noout", "a.xml"], 3, Empty), &[("a.xml", "<a><c/></a>"), ("a.dtd", "<!ELEMENT a (b)><!ELEMENT b (#PCDATA)>")], ""), "does not validate against a.dtd"),
        err(case("dtdvalid missing", &["--dtdvalid", "nope.dtd", "--noout", "a.xml"], 2, Empty), "Could not parse DTD"),
        // RELAX NG and XML Schema
        err(on(case("relaxng", &["--relaxng", "s.rng", "--noout", "a.xml"], 0, Empty), &[("a.xml", BARE), ("s.rng", RNG)], ""), "a.xml validates"),
        on(case("relaxng quiet", &["--quiet", "--relaxng", "s.rng", "--noout", "a.xml"], 0, Empty), &[("a.xml", BARE), ("s.rng", RNG)], ""),
        err(on(case("relaxng fails", &["--relaxng", "s.rng", "--noout", "a.xml"], 3, Empty), &[("a.xml", "<a><c/></a>"), ("s.rng", RNG)], ""), "fails to validate"),
        err(on(case("relaxng bad schema", &["--relaxng", "s.rng", "--noout", "a.xml"], 5, Empty), &[("a.xml", BARE), ("s.rng", "<element")], ""), "failed to compile"),
        err(case("relaxng missing schema", &["--relaxng", "nope.rng", "--noout", "a.xml"], 5, Empty), "failed to compile"),
        err(on(case("schema", &["--schema", "s.xsd", "--noout", "a.xml"], 0, Empty), &[("a.xml", BARE), ("s.xsd", XSD)], ""), "a.xml validates"),
        err(on(case("schema fails", &["--schema", "s.xsd", "--noout", "a.xml"], 3, Empty), &[("a.xml", "<a><c/></a>"), ("s.xsd", XSD)], ""), "fails to validate"),
        err(on(case("schema bad schema", &["--schema", "s.xsd", "--noout", "a.xml"], 5, Empty), &[("a.xml", BARE), ("s.xsd", "<xs:schema")], ""), "failed to compile"),
        err(case("schema missing schema", &["--schema", "nope.xsd", "--noout", "a.xml"], 5, Empty), "failed to compile"),
        // XInclude
        on(case("xinclude", &["--xinclude", "a.xml"], 0, Has("<r xmlns:xi=\"http://www.w3.org/2001/XInclude\"><i/></r>")), &[("a.xml", XI), ("inc.xml", "<i/>")], ""),
        err(on(case("xinclude missing prints anyway", &["--xinclude", "a.xml"], 1, Has("<r ")), &[("a.xml", XI_MISSING)], ""), "XInclude error"),
        // streaming
        case("stream", &["--stream", "a.xml"], 0, Empty),
        err(on(case("stream malformed", &["--stream", "a.xml"], 1, Empty), &[("a.xml", "<a>")], ""), "failed to parse"),
        // external resources
        on(case("noent alone does not load the DTD", &["--noent", "a.xml"], 0, Has("<r/>")), &[("a.xml", EXT), ("r.dtd", "<!ENTITY e 'loaded'>")], ""),
        on(case("loaddtd noent loads the DTD", &["--loaddtd", "--noent", "a.xml"], 0, Has("<r>loaded</r>")), &[("a.xml", EXT), ("r.dtd", "<!ENTITY e 'loaded'>")], ""),
        on(case("loaddtd keeps the reference", &["--loaddtd", "a.xml"], 0, Has("<r>&e;</r>")), &[("a.xml", EXT), ("r.dtd", "<!ENTITY e 'loaded'>")], ""),
        on(case("DTD not loaded by default", &["a.xml"], 0, Has("<r>&e;</r>")), &[("a.xml", EXT), ("r.dtd", "<!ENTITY e 'loaded'>")], ""),
        on(case("path searches directories", &["--loaddtd", "--noent", "--path", "sub", "a.xml"], 0, Has("<r>loaded</r>")), &[("a.xml", EXT), ("sub/r.dtd", "<!ENTITY e 'loaded'>")], ""),
        Case { env: &[("XML_CATALOG_FILES", "cat.xml")], ..on(case("catalogs", &["--catalogs", "--loaddtd", "--noent", "a.xml"], 0, Has("<r>loaded</r>")), &[("a.xml", r#"<!DOCTYPE r SYSTEM "http://example.com/r.dtd"><r>&e;</r>"#), ("cat.xml", CATALOG), ("sub/r.dtd", "<!ENTITY e 'loaded'>")], "") },
        Case { env: &[("XML_CATALOG_FILES", "cat.xml")], ..on(case("catalog ignored without --catalogs", &["--loaddtd", "--noent", "a.xml"], 0, Has("<r/>")), &[("a.xml", r#"<!DOCTYPE r SYSTEM "http://example.com/r.dtd"><r>&e;</r>"#), ("cat.xml", CATALOG), ("sub/r.dtd", "<!ENTITY e 'loaded'>")], "") },
    ]
}

#[test]
fn options() {
    for case in cases() {
        let dir = scratch(case.files);
        let run = yaxmllint(case.args, &dir, case.stdin, case.env);
        let stdout = String::from_utf8_lossy(&run.stdout);
        let name = case.name;
        assert_eq!(
            run.code, case.code,
            "{name}: exit code; stderr: {}",
            run.stderr
        );
        assert!(
            run.stderr.contains(case.err),
            "{name}: stderr {:?} lacks {:?}",
            run.stderr,
            case.err
        );
        match case.out {
            Exact(want) => assert_eq!(stdout, want, "{name}: stdout"),
            Has(want) => assert!(
                stdout.contains(want),
                "{name}: stdout {stdout:?} lacks {want:?}"
            ),
            Empty => assert_eq!(stdout, "", "{name}: stdout"),
        }
        fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn output_file_receives_the_document() {
    let dir = scratch(&[("a.xml", "<a/>")]);
    let run = yaxmllint(&["--output", "out.xml", "a.xml"], &dir, "", &[]);
    assert_eq!(run.code, 0);
    assert_eq!(
        fs::read_to_string(dir.join("out.xml")).unwrap(),
        "<?xml version=\"1.0\"?>\n<a/>\n"
    );
    let unwritable = yaxmllint(&["--output", "no/such/dir/out.xml", "a.xml"], &dir, "", &[]);
    assert_eq!(unwritable.code, 6);
}

#[test]
fn error_positions_name_line_and_column() {
    let dir = scratch(&[("a.xml", "<a>\n  <b></c>\n</a>")]);
    let run = yaxmllint(&["a.xml"], &dir, "", &[]);
    assert_eq!(run.code, 4);
    assert!(
        run.stderr.starts_with("a.xml:2: parser error : "),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("\n  <b></c>\n"), "{}", run.stderr);
}

// ---- differential against the pinned xmllint ----

fn source() -> PathBuf {
    std::env::var_os("YAXML_LIBXML2_SRC").map_or(DEFAULT_SOURCE.into(), PathBuf::from)
}

/// Files directly in `dir` (relative to the test directory) with `extension`.
fn files(dir: &str, extension: &str) -> Vec<String> {
    let root = source().join("test");
    let mut found: Vec<_> = fs::read_dir(root.join(dir))
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    found.sort();
    found
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == extension))
        .map(|p| {
            format!("{dir}/{}", p.file_name().unwrap().to_string_lossy())
                .trim_start_matches("./")
                .to_owned()
        })
        .collect()
}

/// Runs both programs on `args` and reports whether stdout and exit code agree.
fn agree(xmllint: &Path, args: &[&str]) -> bool {
    let dir = source().join("test");
    let (ours, theirs) = (
        yaxmllint(args, &dir, "", &[]),
        run_program(xmllint, args, &dir, "", &[]),
    );
    (ours.stdout, ours.code) == (theirs.stdout, theirs.code)
}

/// Compares `flags` on every input; returns (agreeing, total, disagreeing inputs).
fn compare(xmllint: &Path, flags: &[&str], inputs: &[Vec<String>]) -> (usize, usize, Vec<String>) {
    let failures: Vec<_> = inputs
        .iter()
        .filter(|input| {
            let mut args = flags.to_vec();
            args.extend(input.iter().map(String::as_str));
            !agree(xmllint, &args)
        })
        .map(|input| input.join(" "))
        .collect();
    (inputs.len() - failures.len(), inputs.len(), failures)
}

fn singles(names: Vec<String>) -> Vec<Vec<String>> {
    names.into_iter().map(|name| vec![name]).collect()
}

/// `(schema, instance)` pairs: `base_N.xml` instances check against `base.ext` or `base_N.ext`.
fn schema_pairs(dir: &str, extension: &str) -> Vec<Vec<String>> {
    let instances = files(dir, "xml");
    files(dir, extension)
        .iter()
        .flat_map(|schema| {
            let stem = schema
                .rsplit('/')
                .next()
                .unwrap()
                .trim_end_matches(&format!(".{extension}"));
            let base = stem.split('_').next().unwrap().to_owned();
            instances
                .iter()
                .filter(move |i| {
                    i.rsplit('/')
                        .next()
                        .unwrap()
                        .starts_with(&format!("{base}_"))
                })
                .map(|instance| vec![schema.clone(), instance.clone()])
                .collect::<Vec<_>>()
        })
        .collect()
}

/// XPath expressions from `test/XPath/{tests,expr}`: (expression, document) pairs.
fn xpath_pairs() -> Vec<Vec<String>> {
    let root = source().join("test/XPath");
    let names = |dir: &str| -> Vec<PathBuf> {
        fs::read_dir(root.join(dir))
            .map(|e| e.flatten().map(|e| e.path()).collect())
            .unwrap_or_default()
    };
    let documents: Vec<String> = names("docs")
        .iter()
        .map(|p| format!("XPath/docs/{}", p.file_name().unwrap().to_string_lossy()))
        .collect();
    let mut pairs = Vec::new();
    for (dir, fixed) in [("tests", None), ("expr", Some("XPath/docs/simple"))] {
        for file in names(dir) {
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            let document = fixed.map(String::from).or_else(|| {
                documents
                    .iter()
                    .filter(|d| name.starts_with(d.rsplit('/').next().unwrap()))
                    .max_by_key(|d| d.len())
                    .cloned()
            });
            let (Some(document), Ok(text)) = (document, fs::read_to_string(&file)) else {
                continue;
            };
            pairs.extend(
                text.lines()
                    .filter(|l| !l.is_empty())
                    .map(|expression| vec![expression.to_owned(), document.clone()]),
            );
        }
    }
    pairs
}

/// Inputs where yaxml's library semantics differ from libxml2's, so stdout or the
/// verdict cannot agree: undeclared-prefix and predefined-entity redeclaration
/// are fatal here but recoverable there, ISO-8859-5 is not decoded, and entity
/// references are never kept as tree nodes (`id2`, `id3`, `737840` fail
/// libxml2's post-parse validation because of them). Under `--valid` libxml2
/// also tolerates an external DTD that cannot be loaded and exits 0 on
/// `extra-content`, `trailing-null-1` and `truncated-utf16`, which fail to parse here.
fn known_gap(input: &str) -> bool {
    input.starts_with("namespaces/err_")
        || [
            "iso-8859-5.xml",
            "errors/ent_redecl.xml",
            "errors/ns-undeclared.xml",
            "valid/127772.xml",
            "valid/737840.xml",
            "valid/id2.xml",
            "valid/id3.xml",
            "undeclared-entity.xml",
            "wap.xml",
            "wml.xml",
            "errors/extra-content.xml",
            "errors/trailing-null-1.xml",
            "errors/truncated-utf16.xml",
        ]
        .contains(&input)
}

fn with_all_xml(dirs: &[&str]) -> Vec<Vec<String>> {
    singles(dirs.iter().flat_map(|dir| files(dir, "xml")).collect())
}

/// Label, flags, inputs and whether every input must agree.
type Family<'a> = (&'a str, Vec<&'a str>, Vec<Vec<String>>, bool);

#[test]
fn differential_against_xmllint() {
    let Some(xmllint) = std::env::var_os("YAXML_XMLLINT").map(PathBuf::from) else {
        eprintln!("YAXML_XMLLINT unset; skipping the xmllint differential");
        return;
    };
    let documents = with_all_xml(&[".", "valid", "namespaces", "errors"]);
    let supported: Vec<_> = documents
        .iter()
        .filter(|d| !known_gap(&d[0]))
        .cloned()
        .collect();
    let html = singles(files("HTML", "html"));
    let xpath: Vec<Vec<String>> = xpath_pairs()
        .into_iter()
        .map(|p| vec![p[0].clone(), p[1].clone()])
        .collect();
    // (label, flags, inputs, every input must agree?)
    let families: Vec<Family> = vec![
        ("--noout", vec!["--noout"], supported.clone(), true),
        (
            "--valid --noout",
            vec!["--valid", "--noout"],
            supported.clone(),
            true,
        ),
        (
            "--postvalid --noout",
            vec!["--postvalid", "--noout"],
            supported
                .iter()
                .filter(|d| d[0].starts_with("valid/"))
                .cloned()
                .collect(),
            true,
        ),
        (
            "--c14n",
            vec!["--c14n"],
            with_all_xml(&["c14n/with-comments"]),
            true,
        ),
        (
            "--exc-c14n",
            vec!["--exc-c14n"],
            with_all_xml(&["c14n/exc-without-comments"]),
            true,
        ),
        (
            "--c14n11",
            vec!["--c14n11"],
            with_all_xml(&["c14n/1-1-without-comments"]),
            true,
        ),
        ("--html", vec!["--html"], html.clone(), true),
        ("--html --noout", vec!["--html", "--noout"], html, true),
        (
            "--relaxng --noout",
            vec!["--noout", "--relaxng"],
            schema_pairs("relaxng", "rng"),
            true,
        ),
        (
            "--schema --noout",
            vec!["--noout", "--schema"],
            schema_pairs("schemas", "xsd"),
            true,
        ),
        // Reported only: yaxml always substitutes entities, xmllint keeps references without --noent.
        ("(default output)", vec![], documents.clone(), false),
        ("--format", vec!["--format"], documents, false),
    ];
    let mut strict_failures = Vec::new();
    for (label, flags, inputs, strict) in families {
        let (matched, total, failures) = compare(&xmllint, &flags, &inputs);
        eprintln!(
            "{label}: {matched}/{total} ({:.2}%)",
            100.0 * matched as f64 / total.max(1) as f64
        );
        assert!(
            total > 0,
            "{label}: no inputs found under {}",
            source().display()
        );
        if strict && !failures.is_empty() {
            strict_failures.push(format!("{label}: {failures:?}"));
        }
    }
    // --xpath takes the expression before the document, so it runs input by input.
    let xpath_failures: Vec<_> = xpath
        .iter()
        .filter(|pair| !agree(&xmllint, &["--xpath", &pair[0], &pair[1]]))
        .map(|pair| pair.join(" @ "))
        .collect();
    eprintln!(
        "--xpath: {}/{} ({:.2}%)",
        xpath.len() - xpath_failures.len(),
        xpath.len(),
        100.0 * (xpath.len() - xpath_failures.len()) as f64 / xpath.len().max(1) as f64
    );
    assert!(!xpath.is_empty(), "no XPath inputs");
    if !xpath_failures.is_empty() {
        strict_failures.push(format!("--xpath: {xpath_failures:?}"));
    }
    assert!(strict_failures.is_empty(), "{strict_failures:#?}");
}
