//! Differential test against `tooling/libxml2_uri.c` (libxml2 2.15.4). Runs only
//! when `YAXML_URI_HELPER` names the compiled helper; see `docs/uri.md`.
#![forbid(unsafe_code)]

#[path = "../examples/support/process.rs"]
mod process;

use std::fmt::Write;
use std::path::Path;
use std::time::Duration;

use process::ProcessLimits;
use yaxml::uri::{canonic_path, escape, normalize_path, relative, resolve, unescape};
use yaxml::{Uri, UriError};

const BASE: &str = "http://a/b/c/d;p?q";

/// Pieces whose concatenations hit the grammar's corners.
const TOKENS: &[&str] = &[
    "http",
    "file",
    "x",
    ":",
    "//",
    "/",
    "/",
    "..",
    ".",
    "a",
    "b",
    "c",
    "%41",
    "%2F",
    "%zz",
    "%",
    "%4",
    "?",
    "#",
    "@",
    "[",
    "]",
    "::1",
    "1.2.3.4",
    "256",
    "300",
    "01",
    ":80",
    ":0",
    ":",
    ":99999999999",
    ":2147483647",
    ":2147483648",
    " ",
    "\u{e9}",
    "~",
    ";",
    "=",
    "x.y",
    "d:",
    "\\",
    "%00",
    "%FF",
    "!",
    "'",
    "&",
    "+",
    ",",
    "$",
    "-",
    "_",
    "|",
    "{",
    "*",
    "(",
    "///",
    "../",
    "./",
    "g",
    "h",
    "/..",
    "/.",
];

fn lcg(state: &mut u64) -> usize {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*state >> 33) as usize
}

fn generated(count: usize) -> Vec<String> {
    let mut state = 0x5eed;
    (0..count)
        .map(|_| {
            (0..1 + lcg(&mut state) % 7)
                .map(|_| TOKENS[lcg(&mut state) % TOKENS.len()])
                .collect()
        })
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

fn decode(field: &str) -> Option<Vec<u8>> {
    match field {
        "-" => None,
        "0" => Some(Vec::new()),
        _ => Some(
            (0..field.len() / 2)
                .map(|i| u8::from_str_radix(&field[2 * i..2 * i + 2], 16).unwrap())
                .collect(),
        ),
    }
}

/// What the C helper printed for one request.
fn fields(line: &str) -> Option<Vec<Option<Vec<u8>>>> {
    let mut parts = line.split('\t');
    (parts.next() == Some("OK")).then(|| parts.map(decode).collect())
}

/// Rust and C agree on `expect`, treating undecodable UTF-8 as expected loss.
fn agree(
    request: &str,
    got: Result<String, UriError>,
    line: &str,
    skipped: &mut usize,
) -> Option<String> {
    let c = fields(line).map(|f| f.into_iter().next().flatten());
    match (got, c) {
        (Err(UriError::InvalidUtf8), _) => {
            *skipped += 1;
            None
        }
        (Err(_), None) => None,
        (Ok(r), Some(Some(c))) if r.as_bytes() == c => None,
        (got, c) => Some(format!("{request:?}: yaxml {got:?}, libxml2 {c:?}")),
    }
}

fn compare_parse(request: &str, raw: bool, line: &str, skipped: &mut usize) -> Option<String> {
    let got = if raw {
        Uri::parse_raw(request)
    } else {
        Uri::parse(request)
    };
    let c = fields(line);
    let (u, f) = match (got, c) {
        (Err(UriError::InvalidUtf8), _) => {
            *skipped += 1;
            return None;
        }
        (Err(_), None) => return None,
        (Ok(u), Some(f)) => (u, f),
        (got, c) => return Some(format!("{request:?}: yaxml {got:?}, libxml2 {c:?}")),
    };
    let port = match (u.port, u.empty_authority) {
        (Some(p), _) => p.to_string(),
        (None, true) => "-1".into(),
        (None, false) => "0".into(),
    };
    let want = [
        Some(u.to_string()),
        u.scheme.clone(),
        u.user.clone(),
        u.host.clone(),
        Some(port),
        u.path.clone(),
        u.query.clone(),
        u.fragment.clone(),
    ];
    let same = want.len() == f.len()
        && want
            .iter()
            .zip(&f)
            .all(|(w, c)| w.as_deref().map(str::as_bytes) == c.as_deref());
    (!same).then(|| format!("{request:?} raw={raw}: yaxml {want:?}, libxml2 {f:?}"))
}

#[test]
fn matches_libxml2_on_curated_and_generated_cases() {
    let Some(helper) = std::env::var_os("YAXML_URI_HELPER") else {
        eprintln!("YAXML_URI_HELPER unset; skipping the libxml2 differential");
        return;
    };
    let strings: Vec<String> = [
        "",
        "g",
        "g:h",
        "./g",
        "g/",
        "/g",
        "//g",
        "?y",
        "g?y",
        "#s",
        "g#s",
        "g?y#s",
        ";x",
        "g;x",
        "g;x?y#s",
        ".",
        "./",
        "..",
        "../",
        "../g",
        "../..",
        "../../",
        "../../g",
        "../../../g",
        "../../../../g",
        "/./g",
        "/../g",
        "g.",
        ".g",
        "g..",
        "..g",
        "./../g",
        "./g/.",
        "g/./h",
        "g/../h",
        "g;x=1/./y",
        "g;x=1/../y",
        "g?y/./x",
        "g?y/../x",
        "g#s/./x",
        "g#s/../x",
        "http:g",
        "http://a",
        "http://a/",
        "http://a/b",
        "http://a:80/b",
        "http://u@h:8/p?q#f",
        "http://@/",
        "http://@h/p%20q",
        "//@x/y",
        "x://@",
        "http://[::1]:80/",
        "http://[::1",
        "http://1.2.3.4/",
        "http://1.2.3.4x/",
        "http://h:/",
        "file:///d:/x y",
        "file:///tmp/a%20b",
        "/tmp/a/b",
        "a/b/c",
        "a/b/../c",
        "a//b",
        "../a",
        "dir/doc.xml",
        "dir/sub/doc.xml",
        "doc.xml#frag",
        "a%20b",
        "mailto:a@b",
        "x:y:z",
        "http://a/b/c/d;p?q",
        "http://a/b/c/",
        "http://a/b/",
        "http://a/x/y/z",
        "http://b/c",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain(generated(3000))
    .collect();
    let bases: Vec<&str> = vec![
        BASE,
        "http://a/b/c/d;p?q#f",
        "http://a",
        "http://a/",
        "http://u@h:8/p",
        "file:///a/b.xml",
        "file:///",
        "",
        "a/b/c.xml",
        "/a/b/c.xml",
        "c.xml",
        "http://a/b/c/",
        "http://[::1]/a",
        "http://:80/x",
        "dir/",
        "x:///y",
    ];
    let lists = ["", "/", "/;&=+$,", ":@", "%"];

    let mut requests = Vec::new(); // (operation, request label, line)
    let mut add = |op: &str, a: &str, b: Option<&str>| {
        let line = match b {
            Some(b) => format!("{op}\t{}\t{}\n", encode(a), encode(b)),
            None => format!("{op}\t{}\n", encode(a)),
        };
        requests.push((op.to_owned(), a.to_owned(), b.map(str::to_owned), line));
    };
    for (i, s) in strings.iter().enumerate() {
        for op in ["parse", "raw", "norm", "unesc", "canon"] {
            add(op, s, None);
        }
        add("esc", s, Some(lists[i % lists.len()]));
        for base in &bases {
            add("build", s, Some(base));
            if !s.is_empty() {
                add("rel", s, Some(base));
            }
        }
        if let Some(other) = strings.get(i * 7 % strings.len()) {
            add("build", s, Some(other));
            if !s.is_empty() {
                add("rel", s, Some(other));
            }
        }
    }
    let input: String = requests.iter().map(|r| r.3.as_str()).collect();
    let output = process::run(
        Path::new(&helper),
        &[],
        input.as_bytes(),
        ProcessLimits {
            timeout: Duration::from_secs(120),
            max_output_bytes: 256 << 20,
        },
    )
    .expect("helper runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), requests.len(), "one response line per request");

    let (mut skipped, mut mismatches) = (0, Vec::new());
    for ((op, a, b, _), line) in requests.iter().zip(lines) {
        let b = b.as_deref().unwrap_or("");
        let mismatch = match op.as_str() {
            "parse" => compare_parse(a, false, line, &mut skipped),
            "raw" => compare_parse(a, true, line, &mut skipped),
            "build" => agree(&format!("{a:?} {b:?}"), resolve(a, b), line, &mut skipped),
            "rel" => agree(&format!("{a:?} {b:?}"), relative(a, b), line, &mut skipped),
            "norm" => agree(a, Ok(normalize_path(a)), line, &mut skipped),
            "unesc" => agree(a, unescape(a), line, &mut skipped),
            "esc" => agree(
                &format!("{a:?} {b:?}"),
                Ok(escape(a, b)),
                line,
                &mut skipped,
            ),
            _ => agree(a, Ok(canonic_path(a)), line, &mut skipped),
        };
        mismatches.extend(mismatch.map(|m| format!("{op} {m}")));
    }
    eprintln!(
        "{} requests, {skipped} skipped (non-UTF-8 decode), {} mismatches",
        requests.len(),
        mismatches.len()
    );
    assert!(requests.len() > 5000);
    assert!(
        mismatches.is_empty(),
        "{}",
        mismatches[..mismatches.len().min(40)].join("\n")
    );
}
