//! Differential test against `xmllint --html` from libxml2 2.15.4. Runs when
//! `YAXML_XMLLINT` names the pinned binary; `YAXML_LIBXML2_SOURCE` (the
//! libxml2-2.15.4 source tree) adds its test/HTML documents and the
//! data-state inputs of test/html-tokenizer. See docs/html.md.
#![forbid(unsafe_code)]

#[path = "../../yaxml/tests/support/html_corpus.rs"]
mod corpus;
#[path = "support/html_dump.rs"]
mod dump;
#[path = "../examples/support/process.rs"]
mod process;

use std::path::{Path, PathBuf};
use std::time::Duration;

use process::ProcessLimits;
use yaxml::html::{self, Options};

struct Mode {
    name: &'static str,
    args: &'static [&'static str],
    flags: u32,
    format: bool,
    push: bool,
}

const MODES: &[Mode] = &[
    Mode {
        name: "default",
        args: &[],
        flags: 0,
        format: false,
        push: false,
    },
    Mode {
        name: "format",
        args: &["--format"],
        flags: html::NOBLANKS,
        format: true,
        push: false,
    },
    Mode {
        name: "push",
        args: &["--push"],
        flags: 0,
        format: false,
        push: true,
    },
    Mode {
        name: "noblanks",
        args: &["--noblanks"],
        flags: html::NOBLANKS,
        format: false,
        push: false,
    },
    Mode {
        name: "nodefdtd",
        args: &["--nodefdtd"],
        flags: html::NODEFDTD,
        format: false,
        push: false,
    },
];

/// The `--debug` tree, the serialization and the error reports on stderr.
#[derive(PartialEq)]
struct Outcome {
    tree: String,
    output: Vec<u8>,
    errors: String,
}

fn xmllint(program: &Path, args: &[&str], input: &[u8]) -> process::ProcessOutput {
    let limits = ProcessLimits {
        timeout: Duration::from_secs(30),
        max_output_bytes: 64 << 20,
    };
    let mut all = vec!["--html"];
    all.extend_from_slice(args);
    all.push("-");
    let output = process::run(program, &all, input, limits).expect("run xmllint");
    // 1 reports recoverable errors; anything else is a failure to parse.
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "{all:?}: {:?}",
        output.status
    );
    output
}

fn ours(input: &[u8], mode: &Mode) -> Outcome {
    let options = Options::new(mode.flags);
    let parsed = if mode.push {
        let mut parser = html::PushParser::new(options).unwrap();
        for chunk in input.chunks(3) {
            parser.feed(chunk).unwrap();
        }
        parser.finish().unwrap()
    } else {
        html::parse(input, options).unwrap()
    };
    let lines = yaxml::LineIndex::html(input, &parsed.document);
    Outcome {
        tree: dump::dump(&parsed.document),
        output: html::save(&parsed.document, mode.format).unwrap(),
        errors: parsed
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render("-", &lines))
            .collect(),
    }
}

fn theirs(program: &Path, input: &[u8], mode: &Mode) -> Outcome {
    let mut debug_args = mode.args.to_vec();
    debug_args.push("--debug");
    let tree = String::from_utf8_lossy(&xmllint(program, &debug_args, input).stdout)
        .lines()
        .filter(|line| !line.starts_with("URL="))
        .map(|line| format!("{line}\n"))
        .collect();
    let run = xmllint(program, mode.args, input);
    let errors = String::from_utf8_lossy(&run.stderr).into_owned();
    Outcome {
        tree,
        output: run.stdout,
        errors,
    }
}

/// libxml2's own HTML regression inputs and the html5lib tokenizer inputs it
/// ships, when the source tree is available.
fn upstream_cases(source: &Path) -> Vec<(String, Vec<u8>)> {
    let mut cases = Vec::new();
    if let Ok(entries) = std::fs::read_dir(source.join("test/HTML")) {
        let mut paths: Vec<PathBuf> = entries.map(|entry| entry.unwrap().path()).collect();
        paths.sort();
        for path in paths {
            let bytes = std::fs::read(&path).unwrap();
            cases.push((path.display().to_string(), bytes));
        }
    }
    if let Ok(entries) = std::fs::read_dir(source.join("test/html-tokenizer")) {
        let mut paths: Vec<PathBuf> = entries.map(|entry| entry.unwrap().path()).collect();
        paths.sort();
        for path in paths
            .into_iter()
            .filter(|path| path.extension().is_some_and(|e| e == "test"))
        {
            let bytes = std::fs::read(&path).unwrap();
            let mut rest = &bytes[..];
            // Records: "<n> <last-start-tag> <state> <length>\n<input>\n".
            while let Some(newline) = rest.iter().position(|&c| c == b'\n') {
                let header = String::from_utf8_lossy(&rest[..newline]).into_owned();
                let fields: Vec<&str> = header.split(' ').collect();
                let (Some(state), Some(Ok(length))) =
                    (fields.get(2), fields.get(3).map(|n| n.parse::<usize>()))
                else {
                    break;
                };
                let start = newline + 1;
                if *state == "0" && start + length <= rest.len() {
                    cases.push((
                        format!("{} #{}", path.display(), fields[0]),
                        rest[start..start + length].to_vec(),
                    ));
                }
                rest = &rest[(start + length + 1).min(rest.len())..];
            }
        }
    }
    cases
}

#[test]
fn matches_xmllint() {
    let Some(program) = std::env::var_os("YAXML_XMLLINT").map(PathBuf::from) else {
        eprintln!("YAXML_XMLLINT unset; skipping the libxml2 HTML differential");
        return;
    };
    let mut cases: Vec<(String, Vec<u8>)> = corpus::CASES
        .iter()
        .enumerate()
        .map(|(index, case)| (format!("corpus #{index}"), case.to_vec()))
        .collect();
    if let Some(source) = std::env::var_os("YAXML_LIBXML2_SOURCE") {
        cases.extend(upstream_cases(Path::new(&source)));
    }
    let verbose = std::env::var_os("YAXML_HTML_VERBOSE").is_some();
    let mut failures = Vec::new();
    for mode in MODES {
        let mut matched = 0;
        for (name, input) in &cases {
            let ours = ours(input, mode);
            let theirs = theirs(&program, input, mode);
            if ours == theirs {
                matched += 1;
                continue;
            }
            failures.push(format!("{} {name}", mode.name));
            if verbose {
                eprintln!(
                    "--- {} {name}: {:?}",
                    mode.name,
                    String::from_utf8_lossy(input)
                );
                if ours.tree != theirs.tree {
                    eprintln!("ours tree:\n{}xmllint tree:\n{}", ours.tree, theirs.tree);
                }
                if ours.errors != theirs.errors {
                    eprintln!("errors: ours\n{}xmllint\n{}", ours.errors, theirs.errors);
                }
                if ours.output != theirs.output {
                    eprintln!(
                        "ours:    {:?}\nxmllint: {:?}",
                        String::from_utf8_lossy(&ours.output),
                        String::from_utf8_lossy(&theirs.output)
                    );
                }
            }
        }
        eprintln!(
            "{}: {matched}/{} match xmllint (tree, output, stderr)",
            mode.name,
            cases.len()
        );
    }
    let unexpected: Vec<&String> = failures
        .iter()
        .filter(|f| {
            !KNOWN_DIFFERENCES
                .iter()
                .any(|(mode, case)| f.starts_with(&format!("{mode} ")) && f.ends_with(case))
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "unexpected differences: {unexpected:#?}"
    );
}

/// Mismatches explained in docs/html.md ("Known differences"): mode and
/// case-name suffix. libxml2's push contexts depend on its chunking (xmllint
/// pull and push print different context lines for this document).
const KNOWN_DIFFERENCES: &[(&str, &str)] = &[("push", "test/HTML/doc3.htm")];

/// `tooling/libxml2_html.c`'s string escaping.
fn escaped(out: &mut String, text: Option<&str>) {
    out.push('\t');
    let Some(text) = text else {
        out.push('-');
        return;
    };
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\x{:02X}", u32::from(c))),
            c => out.push(c),
        }
    }
}

/// The helper's error records, without the column (libxml2's skips tag
/// names; ours counts characters), and the columns apart.
fn errors(diagnostics: &[html::Diagnostic], lines: &yaxml::LineIndex<'_>) -> (String, Vec<usize>) {
    let mut out = String::new();
    let mut columns = Vec::new();
    for diagnostic in diagnostics {
        let record = diagnostic.describe(lines);
        out += &format!(
            "error\t{}\t{}\t{}\t{}",
            record.domain as u8, record.code, record.level as u8, record.line
        );
        escaped(&mut out, Some(&record.message));
        out.push('\n');
        columns.push(record.column);
    }
    (out, columns)
}

/// Splits helper records into everything but errors, errors without their
/// column, and the columns.
fn split(records: &str) -> (String, String, Vec<usize>) {
    let (mut other, mut errors, mut columns) = (String::new(), String::new(), Vec::new());
    for line in records.lines() {
        if let Some(rest) = line.strip_prefix("error\t") {
            let fields: Vec<&str> = rest.splitn(6, '\t').collect();
            // `Diagnostic::message` drops the newline most messages end with.
            let message = fields[5].strip_suffix("\\n").unwrap_or(fields[5]);
            errors += &format!(
                "error\t{}\t{}\t{}\t{}\t{message}\n",
                fields[0], fields[1], fields[2], fields[3]
            );
            columns.push(fields[4].parse().unwrap());
        } else if !line.starts_with("ERROR 5013") {
            other += line;
            other.push('\n');
        }
    }
    (other, errors, columns)
}

fn sax_records(input: &[u8], flags: u32) -> (String, String, Vec<usize>) {
    let mut out = String::new();
    let summary = html::parse_events(input, Options::new(flags), |event| {
        match event {
            html::Event::Doctype(doctype) => {
                out.push_str("doctype");
                escaped(&mut out, Some(doctype.name).filter(|name| !name.is_empty()));
                escaped(&mut out, doctype.public_id);
                escaped(&mut out, doctype.system_id);
            }
            html::Event::Start { name, attributes } => {
                out.push_str("start");
                escaped(&mut out, Some(name));
                for (name, value) in attributes {
                    escaped(&mut out, Some(name));
                    if let Some(value) = value {
                        out.push('=');
                        escaped(&mut out, Some(value));
                    }
                }
            }
            html::Event::End(name) => {
                out.push_str("end");
                escaped(&mut out, Some(name));
            }
            html::Event::Text(text) => {
                out.push_str("text");
                escaped(&mut out, Some(text));
            }

            html::Event::Cdata(text) => {
                out.push_str("cdata");
                escaped(&mut out, Some(text));
            }
            html::Event::Comment(text) => {
                out.push_str("comment");
                escaped(&mut out, Some(text));
            }
            _ => unreachable!("new event"),
        }
        out.push('\n');
        std::ops::ControlFlow::Continue(())
    })
    .unwrap();
    let (errors, columns) = errors(&summary.diagnostics, &summary.line_index(input));
    (out, errors, columns)
}

fn line_records(input: &[u8], flags: u32) -> (String, String, Vec<usize>) {
    use yaxml::{LineIndex, OwnedNodeKind};
    let parsed = html::parse(input, Options::new(flags)).unwrap();
    let lines = LineIndex::html(input, &parsed.document);
    let mut out = String::new();
    for node in parsed.document.root().descendants().skip(1) {
        let depth = std::iter::successors(node.parent(), |n| n.parent()).count() - 1;
        let (kind, name) = match node.kind() {
            OwnedNodeKind::Element => (1, node.name().map(|name| name.qualified().to_owned())),
            OwnedNodeKind::Text => (3, Some("text".to_owned())),
            OwnedNodeKind::Cdata => (4, None),
            OwnedNodeKind::Comment => (8, Some("comment".to_owned())),
            OwnedNodeKind::DocumentType => (
                14,
                node.doctype()
                    .map(|d| d.name.to_owned())
                    .filter(|name| !name.is_empty()),
            ),
            kind => panic!("{kind:?}"),
        };
        out += &format!("node\t{depth}\t{kind}");
        escaped(&mut out, name.as_deref());
        let line = node.line(&lines).map_or(-1, |line| line as i64);
        out += &format!("\t{line}\n");
    }
    let (errors, columns) = errors(&parsed.diagnostics, &lines);
    (out, errors, columns)
}

fn fragment_records(input: &[u8], flags: u32) -> (String, String, Vec<usize>) {
    let parsed = html::parse_fragment(input, None, Options::new(flags)).unwrap();
    let list = dump::dump_list(&parsed.document);
    // xmlParseInNodeContext returns the last error's code for an empty list.
    let lines = yaxml::LineIndex::html(input, &parsed.document);
    let code = match parsed.diagnostics.last() {
        Some(last) if list.is_empty() => last.describe(&lines).code,
        _ => 0,
    };
    let (errors, columns) = errors(&parsed.diagnostics, &lines);
    (format!("result\t{code}\n{list}"), errors, columns)
}

/// Runs the helper over every case file in one process: one record block per file.
fn helper_blocks(helper: &Path, mode: &str, flags: u32, files: &[PathBuf]) -> Vec<String> {
    let output = std::process::Command::new(helper)
        .arg(mode)
        .arg(flags.to_string())
        .args(files)
        .output()
        .expect("run the HTML helper");
    assert!(
        output.status.success(),
        "{mode} {flags}: {:?}",
        output.status
    );
    let text = String::from_utf8_lossy(&output.stdout);
    let mut blocks: Vec<String> = Vec::new();
    for line in text.split_inclusive('\n') {
        if line.starts_with("file\t") {
            blocks.push(String::new());
        } else if let Some(block) = blocks.last_mut() {
            block.push_str(line);
        }
    }
    assert_eq!(blocks.len(), files.len(), "{mode} {flags}");
    blocks
}

type Records = fn(&[u8], u32) -> (String, String, Vec<usize>);

/// SAX callbacks (including `HTML_PARSE_HTML5`), node lines and fragments
/// against libxml2 itself, through `tooling/libxml2_html.c`
/// (`YAXML_HTML_HELPER`; see docs/html.md).
#[test]
fn matches_libxml2_callbacks_lines_and_fragments() {
    let Some(helper) = std::env::var_os("YAXML_HTML_HELPER").map(PathBuf::from) else {
        eprintln!("YAXML_HTML_HELPER unset; skipping the libxml2 HTML callback differential");
        return;
    };
    let mut cases: Vec<(String, Vec<u8>)> = corpus::CASES
        .iter()
        .enumerate()
        .map(|(index, case)| (format!("corpus #{index}"), case.to_vec()))
        .collect();
    if let Some(source) = std::env::var_os("YAXML_LIBXML2_SOURCE") {
        cases.extend(upstream_cases(Path::new(&source)));
    }
    let directory = std::env::temp_dir().join(format!("yaxml-html-helper-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let files: Vec<PathBuf> = cases
        .iter()
        .enumerate()
        .map(|(index, (_, input))| {
            let path = directory.join(format!("{index}.html"));
            std::fs::write(&path, input).unwrap();
            path
        })
        .collect();
    let modes: &[(&str, &str, u32, Records)] = &[
        ("sax", "sax", 0, sax_records),
        ("sax-noblanks", "sax", html::NOBLANKS, sax_records),
        ("sax-noimplied", "sax", html::NOIMPLIED, sax_records),
        ("sax-html5", "sax", html::HTML5, sax_records),
        ("lines", "lines", 0, line_records),
        ("lines-html5", "lines", html::HTML5, line_records),
        ("fragment", "fragment", 0, fragment_records),
    ];
    let verbose = std::env::var_os("YAXML_HTML_VERBOSE").is_some();
    let mut failures = Vec::new();
    for &(name, mode, flags, records) in modes {
        let blocks = helper_blocks(&helper, mode, flags, &files);
        let (mut matched, mut errors_matched, mut columns, mut columns_matched) = (0, 0, 0, 0);
        for ((case, input), block) in cases.iter().zip(&blocks) {
            let (theirs, their_errors, their_columns) = split(block);
            let (ours, our_errors, our_columns) = records(input, flags);
            columns += their_columns.len();
            columns_matched += their_columns
                .iter()
                .zip(&our_columns)
                .filter(|(a, b)| a == b)
                .count();
            errors_matched += usize::from(their_errors == our_errors);
            if theirs == ours && their_errors == our_errors {
                matched += 1;
                continue;
            }
            failures.push(format!("{name} {case}"));
            if verbose {
                let input = String::from_utf8_lossy(input);
                let shown: String = input.chars().take(300).collect();
                eprintln!("--- {name} {case}: {shown:?}");
                for (ours, theirs) in [(&ours, &theirs), (&our_errors, &their_errors)] {
                    let mut lines = ours.lines().zip(theirs.lines()).enumerate();
                    match lines.find(|(_, (a, b))| a != b) {
                        Some((at, (a, b))) => eprintln!("line {at}: ours {a:?}\n  libxml2 {b:?}"),
                        None if ours != theirs => eprintln!(
                            "lengths: ours {} lines, libxml2 {}",
                            ours.lines().count(),
                            theirs.lines().count()
                        ),
                        None => {}
                    }
                }
            }
        }
        eprintln!(
            "{name}: {matched}/{} match libxml2 ({errors_matched} error lists; columns {columns_matched}/{columns})",
            cases.len()
        );
    }
    std::fs::remove_dir_all(&directory).ok();
    let unexpected: Vec<&String> = failures
        .iter()
        .filter(|f| !KNOWN_HELPER_DIFFERENCES.contains(&f.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "unexpected differences: {unexpected:#?}"
    );
}

/// Mismatches explained in docs/html.md ("Known differences").
const KNOWN_HELPER_DIFFERENCES: &[&str] = &[];
