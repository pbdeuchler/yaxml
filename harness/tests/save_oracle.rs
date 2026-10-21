//! Differential test of `save::to_vec` against the pinned libxml2 2.15.4
//! `xmllint --noent --dtdattr` over the W3C XML test suite's valid documents
//! and libxml2's `test/*.xml`. Runs when `YAXML_XMLLINT` names the binary;
//! `YAXML_XMLCONF` and `YAXML_LIBXML2_SRC` override the corpus locations.
//! `--format` is compared on the tree `--noblanks` leaves, since yaxml's XML
//! parser has no NOBLANKS option. Differences are classified; see
//! docs/writer.md for the recorded rates.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use yaxml::{ErrorKind, FileResolver, Limits, OwnedDocument, save};

const WORKSPACE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

struct Mode {
    name: &'static str,
    args: &'static [&'static str],
    flags: u32,
    encoding: Option<&'static str>,
}

const MODES: &[Mode] = &[
    Mode {
        name: "default",
        args: &[],
        flags: 0,
        encoding: None,
    },
    Mode {
        name: "format",
        args: &["--format"],
        flags: save::FORMAT,
        encoding: None,
    },
    Mode {
        name: "wsnonsig",
        args: &["--pretty", "2"],
        flags: save::WSNONSIG,
        encoding: None,
    },
    Mode {
        name: "utf-16",
        args: &["--encode", "UTF-16"],
        flags: 0,
        encoding: Some("UTF-16"),
    },
    Mode {
        name: "utf-16be",
        args: &["--encode", "UTF-16BE"],
        flags: 0,
        encoding: Some("UTF-16BE"),
    },
    Mode {
        name: "iso-8859-1",
        args: &["--encode", "ISO-8859-1"],
        flags: 0,
        encoding: Some("ISO-8859-1"),
    },
    Mode {
        name: "ascii",
        args: &["--encode", "ASCII"],
        flags: 0,
        encoding: Some("ASCII"),
    },
    Mode {
        name: "windows-1252",
        args: &["--encode", "windows-1252"],
        flags: 0,
        encoding: Some("windows-1252"),
    },
];

fn xmllint(program: &Path, args: &[&str], path: &Path) -> Option<Vec<u8>> {
    let output = Command::new(program)
        .args(["--noent", "--dtdattr", "--nonet"])
        .args(args)
        .arg(path)
        .output()
        .expect("run xmllint");
    output.status.success().then_some(output.stdout)
}

/// Valid documents (outside the canonical `out` directories) and the OASIS
/// pass cases, with the corpus root their resolver may read.
fn corpus() -> Vec<(PathBuf, PathBuf)> {
    let root = |variable: &str, default: &str| {
        std::env::var_os(variable).map_or_else(|| Path::new(WORKSPACE).join(default), PathBuf::from)
    };
    let xmlconf = root("YAXML_XMLCONF", "yaxml-suites/xmlconf");
    let libxml2 = root("YAXML_LIBXML2_SRC", "yaxml-oracle/libxml2-2.15.4");
    let mut files = Vec::new();
    let mut pending = vec![xmlconf.clone()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for path in entries.map(|entry| entry.unwrap().path()) {
            let text = path.to_string_lossy();
            if path.is_dir() {
                pending.push(path);
            } else if text.ends_with(".xml")
                && !text.contains("/out/")
                && (text.contains("/valid/") || text.contains("/oasis/") && text.contains("pass"))
            {
                files.push((path, xmlconf.clone()));
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(libxml2.join("test")) {
        files.extend(
            entries
                .map(|entry| entry.unwrap().path())
                .filter(|path| {
                    path.extension().is_some_and(|e| e == "xml")
                        || path
                            .file_name()
                            .is_some_and(|name| name.to_string_lossy().starts_with("xhtml"))
                })
                .map(|path| (path, libxml2.clone())),
        );
    }
    files.sort();
    files
}

fn parse(input: &[u8], path: &Path, root: &Path) -> Option<OwnedDocument> {
    let base = path.strip_prefix(root).ok()?.to_string_lossy().into_owned();
    let resolver = Box::new(FileResolver::new(root).ok()?);
    let remote = input.windows(7).any(|window| window == b"http://");
    match OwnedDocument::parse_with(input, loading(Limits::default(), &base, resolver)) {
        // `--nonet` leaves remote DTDs unread.
        Err(error) if remote && error.kind == ErrorKind::ExternalResource => {
            OwnedDocument::parse_bytes(input).ok()
        }
        result => result.ok(),
    }
}

/// Output as text: UTF-16 decoded, other encodings lossily.
fn decode(output: &[u8], encoding: Option<&str>) -> String {
    let little = match encoding {
        Some("UTF-16") => true,
        Some("UTF-16BE") => false,
        _ => return String::from_utf8_lossy(output).into_owned(),
    };
    let units: Vec<u16> = output
        .chunks(2)
        .map(|pair| match pair {
            [a, b] if little => u16::from_le_bytes([*a, *b]),
            [a, b] => u16::from_be_bytes([*a, *b]),
            _ => 0xFFFD,
        })
        .collect();
    String::from_utf16_lossy(&units).replace('\u{FEFF}', "")
}

/// Output without its DOCTYPE, and the DOCTYPE's lines sorted.
fn split_doctype(text: &str) -> (String, Vec<&str>) {
    let Some(start) = text.find("<!DOCTYPE") else {
        return (text.to_owned(), Vec::new());
    };
    let end = text[start..]
        .find("]>\n")
        .map(|end| start + end + 3)
        .or_else(|| text[start..].find(">\n").map(|end| start + end + 2))
        .unwrap_or(text.len());
    let mut lines: Vec<&str> = text[start..end].lines().collect();
    lines.sort_unstable();
    (format!("{}{}", &text[..start], &text[end..]), lines)
}

/// Why yaxml's output differs from libxml2's.
fn classify(
    ours: &Result<Vec<u8>, yaxml::Error>,
    theirs: &[u8],
    input: &[u8],
    encoding: Option<&str>,
) -> &'static str {
    let Ok(ours) = ours else {
        return "error: unrepresentable in markup";
    };
    let (ours, theirs) = (decode(ours, encoding), decode(theirs, encoding));
    let (our_body, our_dtd) = split_doctype(&ours);
    let (their_body, their_dtd) = split_doctype(&theirs);
    if our_body == their_body {
        if our_dtd == their_dtd {
            return "subset: notation hash order";
        }
        if input.contains(&b'%') {
            return "subset: external parameter entity";
        }
        return "subset: other";
    }
    if their_body.replace(" ?>", "?>") == our_body.replace(" ?>", "?>") {
        return "tree: PI with empty data";
    }
    "tree: other"
}

#[test]
fn matches_xmllint() {
    let Some(program) = std::env::var_os("YAXML_XMLLINT").map(PathBuf::from) else {
        eprintln!("YAXML_XMLLINT unset; skipping the libxml2 save differential");
        return;
    };
    let files = corpus();
    let verbose = std::env::var_os("YAXML_SAVE_VERBOSE").is_some();
    let mut report = String::new();
    let mut unexplained = Vec::new();
    for mode in MODES {
        let (mut compared, mut matched, mut skipped) = (0, 0, 0);
        let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
        for (path, root) in &files {
            let Some(theirs) = xmllint(&program, mode.args, path) else {
                skipped += 1;
                continue;
            };
            let original = std::fs::read(path).unwrap();
            let input = if mode.flags & save::FORMAT != 0 {
                xmllint(&program, &["--noblanks"], path).unwrap()
            } else {
                original.clone()
            };
            let Some(document) = parse(&input, path, root) else {
                skipped += 1;
                continue;
            };
            compared += 1;
            let options = save::Options::default()
                .flags(mode.flags)
                .encoding(mode.encoding);
            let ours = save::to_vec(&document, &options);
            if ours.as_deref() == Ok(&theirs[..]) {
                matched += 1;
                continue;
            }
            let class = classify(&ours, &theirs, &original, mode.encoding);
            *classes.entry(class).or_default() += 1;
            if class.ends_with("other") {
                unexplained.push(format!("{} {}", mode.name, path.display()));
            }
            if verbose {
                eprintln!(
                    "{} {} [{class}]\n--- libxml2\n{}\n--- yaxml\n{}",
                    mode.name,
                    path.display(),
                    String::from_utf8_lossy(&theirs),
                    ours.map_or_else(|e| e.to_string(), |o| String::from_utf8_lossy(&o).into()),
                );
            }
        }
        report.push_str(&format!(
            "{}: {matched}/{compared} identical ({skipped} rejected by either parser) {classes:?}\n",
            mode.name
        ));
    }
    eprint!("{report}");
    // Tree differences come from the parsers (libxml2 quirks in entity and
    // attribute normalization); subset ones from external parameter entities
    // and hash order. Anything else is a serializer regression.
    let allowed = [
        "xmltest/valid/sa/068.xml",
        "xmltest/valid/sa/108.xml",
        "ibm/xml-1.1/valid/P02/ibm02v04.xml",
        "ibm/valid/P70/ibm70v01.xml",
        "ibm/xml-1.1/valid/P03/ibm03v05.xml",
    ];
    unexplained.retain(|case| !allowed.iter().any(|allowed| case.ends_with(allowed)));
    assert!(unexplained.is_empty(), "{}", unexplained.join("\n"));
}

/// `xmllint --html --xmlout` over libxml2's HTML documents: the XML
/// serializer with `AS_XML`, including names and comments XML forbids.
#[test]
fn html_as_xml_matches_xmllint() {
    let Some(program) = std::env::var_os("YAXML_XMLLINT").map(PathBuf::from) else {
        eprintln!("YAXML_XMLLINT unset; skipping the libxml2 save differential");
        return;
    };
    let source = std::env::var_os("YAXML_LIBXML2_SRC").map_or_else(
        || Path::new(WORKSPACE).join("yaxml-oracle/libxml2-2.15.4"),
        PathBuf::from,
    );
    let Ok(entries) = std::fs::read_dir(source.join("test/HTML")) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.map(|entry| entry.unwrap().path()).collect();
    paths.sort();
    let (mut compared, mut mismatches) = (0, Vec::new());
    for format in [false, true] {
        for path in &paths {
            let mut args = vec!["--html", "--xmlout"];
            if format {
                args.push("--format");
            }
            let output = Command::new(&program)
                .args(&args)
                .arg(path)
                .output()
                .unwrap();
            if !matches!(output.status.code(), Some(0 | 1)) {
                continue;
            }
            let flags = if format { yaxml::html::NOBLANKS } else { 0 };
            let input = std::fs::read(path).unwrap();
            let document = yaxml::html::parse(&input, yaxml::html::Options::new(flags))
                .unwrap()
                .document;
            let options = save::Options::new(save::AS_XML | if format { save::FORMAT } else { 0 });
            compared += 1;
            if save::to_vec(&document, &options).as_deref() != Ok(&output.stdout[..]) {
                mismatches.push(format!("format={format} {}", path.display()));
            }
        }
    }
    eprintln!(
        "html --xmlout: {}/{compared} identical\n{}",
        compared - mismatches.len(),
        mismatches.join("\n")
    );
    assert!(mismatches.is_empty());
}

/// libxml2's default parse (no `NOENT`, no `DTDATTR`): kept entity references
/// serialize as `&name;`. Reported, with the differing documents listed under
/// `YAXML_SAVE_VERBOSE`; attribute-value references are a known difference.
#[test]
fn kept_references_match_xmllint_default() {
    let Some(program) = std::env::var_os("YAXML_XMLLINT").map(PathBuf::from) else {
        eprintln!("YAXML_XMLLINT unset; skipping the default-parse save differential");
        return;
    };
    let verbose = std::env::var_os("YAXML_SAVE_VERBOSE").is_some();
    let (mut compared, mut matched, mut references) = (0, 0, 0);
    for (path, root) in corpus() {
        let output = Command::new(&program)
            .arg("--nonet")
            .arg(&path)
            .output()
            .expect("run xmllint");
        if !output.status.success() {
            continue;
        }
        let input = std::fs::read(&path).unwrap();
        let base = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let options = yaxml::ParseOptions::default()
            .with_resolver(&base, Box::new(FileResolver::new(&root).unwrap()));
        let Ok(document) = OwnedDocument::parse_with(&input, options) else {
            continue;
        };
        compared += 1;
        references += usize::from(document.nodes().any(|node| node.entity_name().is_some()));
        let ours = save::to_vec(&document, &save::Options::default()).unwrap();
        if ours == output.stdout {
            matched += 1;
        } else if verbose {
            eprintln!("differs: {}", path.display());
        }
    }
    eprintln!(
        "default parse: {matched}/{compared} identical to xmllint ({references} keep references)"
    );
    assert!(compared > 0 && matched > 0);
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
