#![forbid(unsafe_code)]
use std::io::{self, Write};

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let version = args == ["--version"];
    if !version && !args.is_empty() {
        std::process::exit(2);
    }
    let mode = std::env::var("YAXML_CREATE_ORACLE_FAKE").unwrap();
    let prefix = if version { "version" } else { "body" };
    let exit_failure = mode == format!("{prefix}-exit");
    if mode == format!("{prefix}-stderr") {
        eprintln!("fixture diagnostic");
    }
    let mut bytes = if version {
        b"YAXML-CREATE-1 compiled=21504 runtime=21504 tree=1 output=1\n".to_vec()
    } else {
        include_bytes!("create_oracle_expected.tsv").to_vec()
    };
    if mode == format!("{prefix}-changed") {
        let index = if version { 45 } else { 12 };
        bytes[index] ^= 1;
    }
    if mode == format!("{prefix}-extra") {
        bytes.push(b'\n');
    }
    if mode == format!("{prefix}-truncated") {
        bytes.pop();
    }
    io::stdout().lock().write_all(&bytes).unwrap();
    if exit_failure {
        std::process::exit(7);
    }
}
