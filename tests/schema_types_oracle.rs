//! Differential tests against `tooling/libxml2_schema_types.c` (libxml2
//! 2.15.4). Runs only when `YAXML_SCHEMA_TYPES_HELPER` names the compiled
//! helper; see `docs/schema-types.md`.
#![forbid(unsafe_code)]

#[path = "../examples/support/process.rs"]
mod process;

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;
use std::time::Duration;

use process::ProcessLimits;
use yaxml::regexp::{Error, Regex};

fn lcg(state: &mut u64) -> usize {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*state >> 33) as usize
}

fn soup(state: &mut u64, tokens: &[&str], max: usize) -> String {
    (0..lcg(state) % (max + 1))
        .map(|_| tokens[lcg(state) % tokens.len()])
        .collect()
}

fn encode(s: &str) -> String {
    if s.is_empty() {
        return "0".into();
    }
    s.bytes().fold(String::new(), |mut out, b| {
        write!(out, "{b:02x}").unwrap();
        out
    })
}

/// Sends `requests` (operation and fields) and returns one line per request.
fn ask(requests: &[Vec<String>]) -> Option<Vec<String>> {
    let helper = std::env::var_os("YAXML_SCHEMA_TYPES_HELPER")?;
    let input: String = requests
        .iter()
        .map(|r| {
            let fields: Vec<String> = r[1..].iter().map(|f| encode(f)).collect();
            format!("{}\t{}\n", r[0], fields.join("\t"))
        })
        .collect();
    let limits = ProcessLimits {
        timeout: Duration::from_secs(600),
        max_output_bytes: 64 << 20,
    };
    let output =
        process::run(Path::new(&helper), &[], input.as_bytes(), limits).expect("helper runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<String> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(lines.len(), requests.len(), "one response line per request");
    Some(lines)
}

/// Prints the tally and returns the unexplained differences.
fn report(name: &str, total: usize, classes: BTreeMap<&'static str, Vec<String>>) -> Vec<String> {
    let differing: usize = classes.values().map(Vec::len).sum();
    eprintln!(
        "{name}: {total} cases, {} agree, {differing} differ",
        total - differing
    );
    for (class, cases) in &classes {
        eprintln!(
            "  {class}: {} (e.g. {})",
            cases.len(),
            cases[..cases.len().min(3)].join("; ")
        );
    }
    classes.get("unexplained").cloned().unwrap_or_default()
}

const PATTERN_TOKENS: &[&str] = &[
    "a",
    "b",
    "c",
    "1",
    "é",
    "-",
    "^",
    ".",
    "\\d",
    "\\D",
    "\\s",
    "\\S",
    "\\w",
    "\\W",
    "\\i",
    "\\c",
    "\\p{L}",
    "\\p{Lu}",
    "\\P{Nd}",
    "\\p{IsBasicLatin}",
    "\\p{IsLatin-1Supplement}",
    "\\u0062",
    "[a-c]",
    "[^ab]",
    "[a-z-[aeiou]]",
    "[\\d-[1]]",
    "[\\P{L}]",
    "[",
    "]",
    "[^",
    "-[",
    "(",
    ")",
    "(",
    ")",
    "|",
    "|",
    "?",
    "*",
    "+",
    "{2}",
    "{1,3}",
    "{0,2}",
    "{2,}",
    "{0}",
    "{",
    "}",
    ",",
    "\\",
    "\\-",
    "\\.",
    "(a?)",
    "(a|ab)",
    "{1,2}",
    "(b*)",
];
const VALUE_TOKENS: &[&str] = &["a", "b", "c", "1", "2", " ", "-", "é", "A", "_", "^", "."];

#[test]
fn regexp_matches_libxml2() {
    let mut state = 0x5eed;
    let mut cases = Vec::new();
    for _ in 0..6000 {
        let pattern = soup(&mut state, PATTERN_TOKENS, 8);
        for _ in 0..8 {
            cases.push((pattern.clone(), soup(&mut state, VALUE_TOKENS, 6)));
        }
    }
    let requests: Vec<Vec<String>> = cases
        .iter()
        .map(|(p, v)| vec!["re".into(), p.clone(), v.clone()])
        .collect();
    let Some(lines) = ask(&requests) else {
        eprintln!("YAXML_SCHEMA_TYPES_HELPER unset; skipping the libxml2 differential");
        return;
    };
    let mut classes: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for ((pattern, value), line) in cases.iter().zip(&lines) {
        let ours = Regex::new(pattern);
        let class = match (&ours, line.as_str()) {
            (Err(_), "ERR") => continue,
            (Ok(regex), verdict) if verdict == "1" || verdict == "0" => {
                match regex.is_match(value) == (verdict == "1") {
                    true => continue,
                    // libxml2's determinism check applies the negation of `\P`
                    // twice, so it drops backtracking points it needs.
                    false if pattern.contains("\\P") => "libxml2 \\P determinism",
                    false => "unexplained",
                }
            }
            (Ok(_), "ERR") => "unexplained",
            (Ok(_), _) => "libxml2 match error",
            (Err(Error::TooComplex), _) => "budget",
            (Err(_), _) => "unexplained",
        };
        classes.entry(class).or_default().push(format!(
            "{pattern:?} on {value:?}: libxml2 {line}, yaxml {:?}",
            ours.map(|r| r.is_match(value))
        ));
    }
    let unexplained = report("regexp", cases.len(), classes);
    assert!(
        unexplained.is_empty(),
        "{}",
        unexplained[..unexplained.len().min(40)].join("\n")
    );
}
