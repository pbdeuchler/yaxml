//! `Error::describe` against libxml2 over libxml2's `test/errors` and the
//! XMLTS not-wf documents. `YAXML_ERRORS_HELPER` names the compiled
//! `tooling/libxml2_errors.c` (codes, lines and messages of libxml2's first
//! error) and `YAXML_XMLLINT` the pinned xmllint (its first `error :` line on
//! stderr, compared with `Diagnostic::render`). Skipped without them;
//! `YAXML_VERBOSE` lists the disagreements.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use yaxml::{LineIndex, OwnedDocument, ParseOptions};

const DEFAULT_SOURCE: &str =
    "/Users/pbdeuchler/workspace/src/github.com/pbdeuchler/yaxml-oracle/libxml2-2.15.4";
const DEFAULT_SUITES: &str = "/Users/pbdeuchler/workspace/src/github.com/pbdeuchler/yaxml-suites";

/// libxml2's first error-level record for a file: code, line, message.
type Record = (u16, usize, String);

fn helper_records(helper: &Path, dir: &Path, files: &[String]) -> Vec<Option<Record>> {
    let output = Command::new(helper)
        .args(files)
        .current_dir(dir)
        .output()
        .expect("run the helper");
    let text = String::from_utf8_lossy(&output.stdout);
    files
        .iter()
        .map(|file| {
            text.lines().find_map(|line| {
                let fields: Vec<_> = line.splitn(10, '\t').collect();
                let level: u8 = fields.get(3)?.parse().ok()?;
                (fields[0] == file && level >= 2).then(|| {
                    let message = fields[9].trim_end().to_owned();
                    (
                        fields[2].parse().unwrap_or(0),
                        fields[4].parse().unwrap_or(0),
                        message,
                    )
                })
            })
        })
        .collect()
}

/// xmllint's first `error :` line for a file.
fn xmllint_line(xmllint: &Path, dir: &Path, file: &str) -> Option<String> {
    let output = Command::new(xmllint)
        .args(["--noout", file])
        .current_dir(dir)
        .output()
        .ok()?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    stderr
        .lines()
        .find(|line| line.contains(" error : "))
        .map(str::to_owned)
}

fn rate(agree: usize, total: usize) -> String {
    format!(
        "{agree}/{total} ({:.2}%)",
        100.0 * agree as f64 / total.max(1) as f64
    )
}

#[test]
fn describe_against_libxml2() {
    let Some(helper) = env::var_os("YAXML_ERRORS_HELPER").map(PathBuf::from) else {
        eprintln!("YAXML_ERRORS_HELPER unset; skipping the error differential");
        return;
    };
    let xmllint = env::var_os("YAXML_XMLLINT").map(PathBuf::from);
    let verbose = env::var_os("YAXML_VERBOSE").is_some();
    let source = env::var_os("YAXML_LIBXML2_SRC")
        .map_or(DEFAULT_SOURCE.into(), PathBuf::from)
        .join("test");
    let suites = env::var_os("YAXML_SUITES").map_or(DEFAULT_SUITES.into(), PathBuf::from);
    let mut errors: Vec<String> = fs::read_dir(source.join("errors"))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    errors.retain(|name| name.ends_with(".xml"));
    errors.sort();
    let errors: Vec<_> = errors
        .into_iter()
        .map(|name| format!("errors/{name}"))
        .collect();
    let mut xmlts: Vec<String> = fs::read_to_string(suites.join("xmlconf-catalog.tsv"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            (fields.get(1) == Some(&"not-wf"))
                .then(|| fields.get(8).map(|path| path.to_string()))?
        })
        .collect();
    xmlts.dedup();
    for (label, dir, files) in [
        ("errors", source, errors),
        ("XMLTS not-wf", suites.join("xmlconf"), xmlts),
    ] {
        let records = helper_records(&helper, &dir, &files);
        let (mut total, mut located, mut worded, mut rendered, mut lint_total) = (0, 0, 0, 0, 0);
        for (file, record) in files.iter().zip(records) {
            let Some((code, line, message)) = record else {
                continue;
            };
            total += 1;
            let input = fs::read(dir.join(file)).unwrap_or_default();
            let ours = OwnedDocument::parse_with(&input, ParseOptions::default())
                .err()
                .map(|error| {
                    let lines = LineIndex::new(&input);
                    let diagnostic = error.describe(&lines);
                    let first = diagnostic
                        .render(file, &lines)
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_owned();
                    (diagnostic, first)
                });
            let (same_place, same_words) = ours.as_ref().map_or((false, false), |(d, _)| {
                (
                    (d.code, d.line) == (code, line),
                    d.message.replace('\n', " ").trim_end() == message,
                )
            });
            located += usize::from(same_place);
            worded += usize::from(same_words);
            if verbose && !(same_place && same_words) {
                let ours = ours.as_ref().map(|(d, _)| (d.code, d.line, &d.message));
                eprintln!("{file}: libxml2 {code} line {line} {message:?}; yaxml {ours:?}");
            }
            if let Some(xmllint) = &xmllint
                && let Some(theirs) = xmllint_line(xmllint, &dir, file)
            {
                lint_total += 1;
                let same = ours.as_ref().is_some_and(|(_, first)| *first == theirs);
                rendered += usize::from(same);
                if verbose && !same {
                    eprintln!(
                        "  xmllint {theirs:?}\n  yaxml   {:?}",
                        ours.map(|(_, first)| first)
                    );
                }
            }
        }
        eprintln!(
            "{label}: code+line {}, message {}, rendered first error line {}",
            rate(located, total),
            rate(worded, total),
            rate(rendered, lint_total)
        );
        assert!(
            located * 10 >= total * 9 && worded * 10 >= total * 9,
            "{label} regressed"
        );
    }
}
