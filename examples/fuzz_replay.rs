//! Replay one testcase from stdin, or each named file. This uses no fuzz runtime.
#![forbid(unsafe_code)]

#[path = "../fuzz/driver.rs"]
mod driver;

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use yaxml::ErrorKind;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse(std::env::args_os().skip(1))?;
    if let Some(oracle) = &args.oracle {
        let version = Command::new(oracle).arg("--version").output()?;
        if !version.status.success() {
            return Err("xmllint --version failed".into());
        }
        eprintln!(
            "comparison executable: {}\n{}{}",
            oracle.display(),
            String::from_utf8_lossy(&version.stdout),
            String::from_utf8_lossy(&version.stderr)
        );
    }
    let mut mismatches = 0;
    if args.files.is_empty() {
        let input = bounded_read(io::stdin().lock())?;
        mismatches += replay(&input, "stdin", args.oracle.as_deref())? as usize;
    } else {
        for file in args.files {
            let input = bounded_read(File::open(&file)?)?;
            mismatches +=
                replay(&input, &file.display().to_string(), args.oracle.as_deref())? as usize;
        }
    }
    if mismatches != 0 {
        return Err(
            format!("{mismatches} differential disagreements require classification").into(),
        );
    }
    Ok(())
}

#[derive(Debug, Default, Eq, PartialEq)]
struct Args {
    oracle: Option<PathBuf>,
    files: Vec<PathBuf>,
}

impl Args {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut result = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--" {
                result.files.extend(args.map(PathBuf::from));
                break;
            } else if arg == "--libxml2" && result.oracle.is_none() {
                result.oracle = Some(
                    args.next()
                        .filter(|path| !path.is_empty())
                        .ok_or("--libxml2 requires an executable path")?
                        .into(),
                );
            } else if arg.to_string_lossy().starts_with('-') {
                return Err(format!(
                    "unknown or repeated option: {}",
                    arg.to_string_lossy()
                ));
            } else {
                result.files.push(arg.into());
            }
        }
        Ok(result)
    }
}

fn replay(input: &[u8], label: &str, oracle: Option<&std::path::Path>) -> io::Result<bool> {
    let outcome = driver::exercise(input);
    let Some(oracle) = oracle else {
        return Ok(false);
    };
    if matches!(
        outcome.error.map(|error| error.kind),
        Some(
            ErrorKind::UnsupportedDtd
                | ErrorKind::UnsupportedEncoding
                | ErrorKind::InvalidUtf8
                | ErrorKind::LimitExceeded(_)
        )
    ) || input
        .windows(b"<!DOCTYPE".len())
        .any(|window| window == b"<!DOCTYPE")
    {
        eprintln!("{label}: skipped unsupported feature, encoding, or unmatched limit policy");
        return Ok(false);
    }
    let mut child = Command::new(oracle)
        .args(["--nonet", "--noout", "-"])
        .env("XML_CATALOG_FILES", "")
        .env("SGML_CATALOG_FILES", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let output = std::thread::scope(|scope| -> io::Result<_> {
        let writer = scope.spawn(move || stdin.write_all(input));
        let output = child.wait_with_output()?;
        if let Err(error) = writer.join().expect("input writer panicked")
            && error.kind() != io::ErrorKind::BrokenPipe
        {
            return Err(error);
        }
        Ok(output)
    })?;
    match classify(output.status.code(), output.stderr.is_empty()) {
        Some(accepted) => {
            let mismatch = accepted != outcome.error.is_none();
            eprintln!(
                "{label}: {} (yaxml={:?}, libxml2 accepted={accepted})",
                if mismatch {
                    "disagreement"
                } else {
                    "agreement"
                },
                outcome.error
            );
            Ok(mismatch)
        }
        None => {
            eprintln!(
                "{label}: inconclusive oracle result (exit={:?}, diagnostic bytes={})",
                output.status.code(),
                output.stderr.len()
            );
            Ok(false)
        }
    }
}

fn classify(code: Option<i32>, diagnostics_empty: bool) -> Option<bool> {
    match code {
        Some(0) if diagnostics_empty => Some(true),
        Some(4) => Some(false),
        _ => None,
    }
}

fn bounded_read(reader: impl Read) -> io::Result<Vec<u8>> {
    let mut input = Vec::new();
    reader
        .take((driver::MAX_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut input)?;
    if input.len() > driver::MAX_INPUT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "fuzz input exceeds one MiB",
        ));
    }
    Ok(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_input() {
        for len in [0, 1, driver::MAX_INPUT_BYTES] {
            let input = vec![b'x'; len];
            assert_eq!(bounded_read(input.as_slice()).unwrap(), input);
        }
        assert_eq!(
            bounded_read(vec![0; driver::MAX_INPUT_BYTES + 1].as_slice())
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn io_failure() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(
                    io::ErrorKind::Other,
                    "injected read failure",
                ))
            }
        }
        assert_eq!(
            bounded_read(Broken).unwrap_err().kind(),
            io::ErrorKind::Other
        );
    }

    #[test]
    fn arguments() {
        for (args, expected) in [
            (vec![], Ok(Args::default())),
            (
                vec!["case.xml"],
                Ok(Args {
                    oracle: None,
                    files: vec!["case.xml".into()],
                }),
            ),
            (
                vec!["--libxml2", "xmllint", "--", "-case"],
                Ok(Args {
                    oracle: Some("xmllint".into()),
                    files: vec!["-case".into()],
                }),
            ),
            (
                vec!["--libxml2"],
                Err("--libxml2 requires an executable path".to_owned()),
            ),
            (
                vec!["--bad"],
                Err("unknown or repeated option: --bad".to_owned()),
            ),
            (
                vec!["--libxml2", "xmllint", "--libxml2", "again"],
                Err("unknown or repeated option: --libxml2".to_owned()),
            ),
        ] {
            assert_eq!(Args::parse(args.into_iter().map(OsString::from)), expected);
        }
    }

    #[test]
    fn oracle_result_classification() {
        for (code, empty, expected) in [
            (Some(0), true, Some(true)),
            (Some(0), false, None),
            (Some(4), false, Some(false)),
            (Some(9), true, None),
            (None, false, None),
        ] {
            assert_eq!(classify(code, empty), expected);
        }
    }
}
