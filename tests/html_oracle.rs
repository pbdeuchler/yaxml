//! Differential test against `xmllint --html` from libxml2 2.15.4. Runs when
//! `YAXML_XMLLINT` names the pinned binary; `YAXML_LIBXML2_SOURCE` (the
//! libxml2-2.15.4 source tree) adds its test/HTML documents and the
//! data-state inputs of test/html-tokenizer. See docs/html.md.
#![forbid(unsafe_code)]

#[path = "support/html_corpus.rs"]
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

/// The `--debug` tree, the serialization and the number of reported errors.
#[derive(PartialEq)]
struct Outcome {
    tree: String,
    output: Vec<u8>,
    errors: usize,
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
    Outcome {
        tree: dump::dump(&parsed.document),
        output: html::save(&parsed.document, mode.format).unwrap(),
        errors: parsed.diagnostics.len(),
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
    let errors = String::from_utf8_lossy(&run.stderr)
        .lines()
        .filter(|line| line.contains(" error : "))
        .count();
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
        let (mut matched, mut errors) = (0, 0);
        for (name, input) in &cases {
            let ours = ours(input, mode);
            let theirs = theirs(&program, input, mode);
            errors += usize::from(ours.errors == theirs.errors);
            if ours.tree == theirs.tree && ours.output == theirs.output {
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
            "{}: {matched}/{} match xmllint; error counts agree on {errors}",
            mode.name,
            cases.len()
        );
    }
    let known: Vec<&str> = KNOWN_DIFFERENCES.to_vec();
    let unexpected: Vec<&String> = failures
        .iter()
        .filter(|f| !known.contains(&f.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "unexpected differences: {unexpected:#?}"
    );
}

/// Mismatches explained in docs/html.md ("Known differences").
const KNOWN_DIFFERENCES: &[&str] = &[];
