//! Compare bounded leaf insertion and removal with an external libxml2 helper.
#![forbid(unsafe_code)]

#[path = "support/process.rs"]
mod process;
#[path = "support/structure.rs"]
mod structure;

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use structure::{CheckError, Outcome};

#[derive(Default, Debug, Eq, PartialEq)]
struct Args {
    helper: Option<PathBuf>,
    exploratory: bool,
    cases: Vec<String>,
}

impl Args {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, &'static str> {
        let mut result = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--libxml2-helper" && result.helper.is_none() {
                result.helper = Some(
                    args.next()
                        .filter(|arg| !arg.is_empty())
                        .ok_or("--libxml2-helper requires a path")?
                        .into(),
                );
            } else if arg == "--allow-other-libxml2" && !result.exploratory {
                result.exploratory = true;
            } else if arg.to_string_lossy().starts_with('-') {
                return Err("unknown or repeated structural oracle option");
            } else {
                result
                    .cases
                    .push(arg.into_string().map_err(|_| "case name must be UTF-8")?);
            }
        }
        if result.helper.is_none() {
            return Err("--libxml2-helper PATH is required");
        }
        Ok(result)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse(std::env::args_os().skip(1))?;
    let cases = structure::cases();
    for name in &args.cases {
        if !cases.iter().any(|case| case.name == name) {
            return Err(format!("unknown structural case: {name}").into());
        }
    }
    let path = args.helper.unwrap();
    let limits = process::ProcessLimits {
        timeout: Duration::from_secs(5),
        max_output_bytes: structure::MAX_OUTPUT,
    };
    let version = process::run(
        &path,
        &["--version"],
        b"",
        process::ProcessLimits {
            max_output_bytes: 16 * 1024,
            ..limits
        },
    )?;
    if !version.status.success() || !version.stderr.is_empty() {
        return Err("structural oracle version command failed".into());
    }
    structure::validate_version(&version.stdout, args.exploratory)?;
    eprintln!(
        "structural executable: {}\nmode: {}\n{}",
        path.display(),
        if args.exploratory {
            "exploratory"
        } else {
            "pinned"
        },
        String::from_utf8_lossy(&version.stdout)
    );
    let mut counts = [0usize; 6];
    for case in cases
        .iter()
        .filter(|case| args.cases.is_empty() || args.cases.iter().any(|name| name == case.name))
    {
        if let Some(Outcome::Skipped(reason)) = case.skipped() {
            counts[3] += 1;
            eprintln!("{}: not compared ({reason})", case.name);
            continue;
        }
        let expected = case.expected()?;
        let output = process::run(&path, &[], &case.request(), limits)?;
        if !output.status.success() || !output.stderr.is_empty() {
            counts[5] += 1;
            eprintln!(
                "{}: inconclusive exit={:?} diagnostic_bytes={}",
                case.name,
                output.status.code(),
                output.stderr.len()
            );
            continue;
        }
        match expected.verify(&output.stdout) {
            Ok(Outcome::Exact) => {
                counts[0] += 1;
                eprintln!("{}: exact tree and byte agreement", case.name);
            }
            Ok(Outcome::Projected(reason)) => {
                counts[1] += 1;
                eprintln!("{}: projected agreement ({reason})", case.name);
            }
            Ok(Outcome::Difference(reason)) => {
                counts[2] += 1;
                eprintln!(
                    "{}: intentional difference; libxml2 result verified ({reason})",
                    case.name
                );
            }
            Ok(Outcome::Skipped(_)) => unreachable!(),
            Err(CheckError::Mismatch(reason)) => {
                counts[4] += 1;
                eprintln!("{}: disagreement ({reason})", case.name);
            }
            Err(CheckError::Protocol) => {
                counts[5] += 1;
                eprintln!("{}: inconclusive malformed protocol", case.name);
            }
        }
    }
    eprintln!(
        "structural summary: exact={} projected={} intentional={} skipped={} disagreements={} inconclusive={}",
        counts[0], counts[1], counts[2], counts[3], counts[4], counts[5]
    );
    if counts[4] != 0 || counts[5] != 0 {
        return Err(io::Error::other(
            "structural disagreements or inconclusive results require classification",
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_explicit_and_unambiguous() {
        for (args, valid) in [
            (vec![], false),
            (vec!["--allow-other-libxml2"], false),
            (vec!["--libxml2-helper"], false),
            (vec!["--libxml2-helper", ""], false),
            (vec!["--libxml2-helper", "helper", "append-text"], true),
            (
                vec!["--allow-other-libxml2", "--libxml2-helper", "helper"],
                true,
            ),
            (
                vec!["--libxml2-helper", "helper", "--libxml2-helper", "other"],
                false,
            ),
            (
                vec![
                    "--libxml2-helper",
                    "helper",
                    "--allow-other-libxml2",
                    "--allow-other-libxml2",
                ],
                false,
            ),
        ] {
            assert_eq!(
                Args::parse(args.into_iter().map(OsString::from)).is_ok(),
                valid
            );
        }
    }
}
