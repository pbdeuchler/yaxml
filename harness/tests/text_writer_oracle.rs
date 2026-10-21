//! Differential test against `tooling/libxml2_textwriter.c` (libxml2
//! 2.15.4 `xmlTextWriter`): random call scripts, misuse included, and
//! well-formed ones must give the same per-call success and the same output
//! bytes. Runs only when `YAXML_TEXTWRITER_HELPER` names the compiled
//! helper (`YAXML_TEXTWRITER_CASES`, `YAXML_TEXTWRITER_SEED`); see
//! `docs/writer.md`.
#![forbid(unsafe_code)]

#[path = "../examples/support/process.rs"]
mod process;
#[path = "../../tests/support/fuzz/text_script.rs"]
mod text_script;

use std::path::Path;
use std::time::Duration;

use process::ProcessLimits;
use text_script::Call;
use yaxml::TextWriter;

fn unhex(field: &str) -> Vec<u8> {
    (0..field.len() / 2)
        .map(|i| u8::from_str_radix(&field[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

/// Per-call results (`0` or `1`) and the output, as the helper prints them.
fn replay(script: &[Call]) -> (String, Vec<u8>) {
    let mut writer = TextWriter::new(Vec::new());
    let results = script
        .iter()
        .map(|call| {
            if call.apply(&mut writer).is_ok() {
                '0'
            } else {
                '1'
            }
        })
        .collect();
    (results, writer.into_inner())
}

#[test]
fn call_scripts_match_libxml2() {
    let Some(helper) = std::env::var_os("YAXML_TEXTWRITER_HELPER") else {
        eprintln!("YAXML_TEXTWRITER_HELPER unset; skipping the libxml2 differential");
        return;
    };
    let cases: usize = std::env::var("YAXML_TEXTWRITER_CASES")
        .ok()
        .and_then(|cases| cases.parse().ok())
        .unwrap_or(20_000);
    let mut state: u64 = std::env::var("YAXML_TEXTWRITER_SEED")
        .ok()
        .and_then(|seed| seed.parse().ok())
        .unwrap_or(0x5eed);
    let mut next = |bound: usize| {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((state >> 33) as usize) % bound
    };
    let scripts: Vec<Vec<Call>> = (0..cases)
        .map(|case| {
            let calls = 1 + next(30);
            match case % 4 {
                0 => text_script::well_formed(&mut next, calls),
                _ => text_script::random(&mut next, &mut || None, calls),
            }
        })
        .collect();
    let mut requests = String::new();
    for script in &scripts {
        for (index, call) in script.iter().enumerate() {
            if index > 0 {
                requests.push(' ');
            }
            call.encode(&mut requests);
        }
        requests.push('\n');
    }
    let output = process::run(
        Path::new(&helper),
        &[],
        requests.as_bytes(),
        ProcessLimits {
            timeout: Duration::from_secs(300),
            max_output_bytes: 1 << 30,
        },
    )
    .expect("helper runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut mismatches = Vec::new();
    let (mut calls, mut stale) = (0, 0);
    for (script, line) in scripts.iter().zip(stdout.lines()) {
        calls += script.len();
        let (results, bytes) = line.split_once('\t').unwrap();
        let theirs = (
            results.to_owned(),
            if bytes == "." {
                Vec::new()
            } else {
                unhex(bytes)
            },
        );
        let ours = replay(script);
        if ours == theirs {
            continue;
        }
        // Documented divergence: a namespaced attribute call that fails
        // after queueing its declaration leaves an entry naming a freed
        // list link; libxml2 matches it against whatever element the
        // allocator places at that address next.
        let queued_and_failed = script.iter().zip(ours.0.chars()).any(|(call, result)| {
            matches!(call.code(), "SB" | "WB") && call.args[2].is_some() && result == '1'
        });
        if queued_and_failed {
            stale += 1;
        } else {
            mismatches.push((script, ours, theirs));
        }
    }
    assert_eq!(stdout.lines().count(), scripts.len());
    for (script, ours, theirs) in mismatches.iter().take(5) {
        let codes: Vec<String> = script
            .iter()
            .map(|call| format!("{}{:?}", call.code(), call.args))
            .collect();
        eprintln!(
            "script {codes:?}\n  ours   {} {:?}\n  theirs {} {:?}",
            ours.0,
            String::from_utf8_lossy(&ours.1),
            theirs.0,
            String::from_utf8_lossy(&theirs.1)
        );
    }
    eprintln!(
        "{} scripts, {calls} calls, {} identical, {stale} stale-namespace divergences, {} mismatches",
        scripts.len(),
        scripts.len() - stale - mismatches.len(),
        mismatches.len()
    );
    assert!(mismatches.is_empty());
}
