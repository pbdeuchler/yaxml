//! Replay one testcase from stdin, or each named file. This uses no fuzz runtime.
#![forbid(unsafe_code)]

#[path = "../fuzz/driver.rs"]
mod driver;
#[path = "support/infoset.rs"]
mod infoset;
#[path = "support/process.rs"]
mod process;

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read};
use std::path::PathBuf;
use std::time::Duration;
use yaxml::{Document, Error, ErrorKind, Limit, OwnedDocument, WriteError, Writer, WriterLimits};

use infoset::{Projection, ProjectionError};
use process::ProcessLimits;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse(std::env::args_os().skip(1))?;
    let oracle = args
        .oracle
        .map(|path| Oracle::new(path, args.allow_other_version))
        .transpose()?;
    let mut counts = Counts::default();
    if args.files.is_empty() {
        let input = bounded_read(io::stdin().lock())?;
        counts.record(replay(&input, oracle.as_ref())?, "stdin");
    } else {
        for file in args.files {
            let input = bounded_read(File::open(&file)?)?;
            counts.record(
                replay(&input, oracle.as_ref())?,
                &file.display().to_string(),
            );
        }
    }
    if oracle.is_some() {
        eprintln!(
            "oracle summary: semantic_agreements={} rejection_agreements={} acceptance_only={} skipped_policy={} inconclusive={} disagreements={}",
            counts.semantic,
            counts.rejected,
            counts.acceptance_only,
            counts.skipped,
            counts.inconclusive,
            counts.disagreements
        );
    }
    if counts.disagreements != 0 || counts.inconclusive != 0 {
        return Err(
            "differential disagreements or inconclusive results require classification".into(),
        );
    }
    Ok(())
}

#[derive(Debug, Default, Eq, PartialEq)]
struct Args {
    oracle: Option<PathBuf>,
    allow_other_version: bool,
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
            } else if arg == "--allow-other-libxml2" && !result.allow_other_version {
                result.allow_other_version = true;
            } else if arg.to_string_lossy().starts_with('-') {
                return Err(format!(
                    "unknown or repeated option: {}",
                    arg.to_string_lossy()
                ));
            } else {
                result.files.push(arg.into());
            }
        }
        if result.allow_other_version && result.oracle.is_none() {
            return Err("--allow-other-libxml2 requires --libxml2".into());
        }
        Ok(result)
    }
}

fn replay(input: &[u8], oracle: Option<&Oracle>) -> io::Result<Comparison> {
    let outcome = driver::exercise(input);
    let Some(oracle) = oracle else {
        return Ok(Comparison::NotCompared);
    };
    if matches!(
        outcome.error.map(|error| error.kind),
        Some(
            ErrorKind::UnsupportedEncoding
                | ErrorKind::EncodingMismatch
                | ErrorKind::InvalidUtf8
                | ErrorKind::LimitExceeded(_)
        )
    ) || input
        .windows(b"<!DOCTYPE".len())
        .any(|window| window == b"<!DOCTYPE")
    {
        return Ok(Comparison::SkippedPolicy);
    }
    oracle.compare(input, outcome.error)
}

fn classify(code: Option<i32>, diagnostics_empty: bool) -> Option<bool> {
    match code {
        Some(0) if diagnostics_empty => Some(true),
        Some(4) => Some(false),
        _ => None,
    }
}

struct Oracle {
    path: PathBuf,
    limits: ProcessLimits,
}

impl Oracle {
    fn new(path: PathBuf, allow_other_version: bool) -> io::Result<Self> {
        let version = process::run(
            &path,
            &["--version"],
            b"",
            ProcessLimits {
                timeout: Duration::from_secs(5),
                max_output_bytes: 16 * 1024,
            },
        )?;
        if !version.status.success() {
            return Err(io::Error::other("xmllint --version failed"));
        }
        let mut report = version.stdout;
        report.extend_from_slice(&version.stderr);
        validate_version(&report, allow_other_version)?;
        eprintln!(
            "comparison executable: {}\nmode: {}\n{}",
            path.display(),
            if allow_other_version {
                "exploratory"
            } else {
                "pinned"
            },
            String::from_utf8_lossy(&report)
        );
        Ok(Self {
            path,
            limits: ProcessLimits {
                timeout: Duration::from_secs(5),
                max_output_bytes: infoset::MAX_EXPECTED_BYTES,
            },
        })
    }

    fn compare(&self, input: &[u8], error: Option<Error>) -> io::Result<Comparison> {
        let (projection, limit) = if error.is_some() {
            (None, None)
        } else {
            match Projection::from_input(input) {
                Ok(projection) => (Some(projection), None),
                Err(ProjectionError::Limit(name)) => (None, Some(name)),
                Err(error) => return Err(io::Error::other(error)),
            }
        };
        let mut args = vec![
            "--nonet",
            "--nocatalogs",
            "--strict-namespace",
            "--nowarning",
        ];
        if let Some(projection) = &projection {
            args.extend(["--nocdata", "--xpath", projection.query(), "-"]);
        } else {
            args.extend(["--noout", "-"]);
        }
        let output = process::run(&self.path, &args, input, self.limits)?;
        if classify(output.status.code(), output.stderr.is_empty()).is_none() {
            eprintln!(
                "oracle exit={:?}, diagnostic bytes={}: {}",
                output.status.code(),
                output.stderr.len(),
                String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(4096)])
            );
        }
        let comparison = compare_output(
            classify(output.status.code(), output.stderr.is_empty()),
            error.is_none(),
            &output.stdout,
            projection.as_ref().map(Projection::expected),
            limit,
        );
        if comparison != Comparison::Semantic {
            return Ok(comparison);
        }
        let Some(serialized) = serialize_for_oracle(input)? else {
            return Ok(Comparison::AcceptanceOnly("serialized storage bytes"));
        };
        // Reuse the original tree's query and expected values. The independent
        // oracle evaluates the new bytes without reparsing them through yaxml.
        let output = process::run(&self.path, &args, &serialized, self.limits)?;
        Ok(serialized_comparison(compare_output(
            classify(output.status.code(), output.stderr.is_empty()),
            true,
            &output.stdout,
            projection.as_ref().map(Projection::expected),
            None,
        )))
    }
}

fn serialize_for_oracle(input: &[u8]) -> io::Result<Option<Vec<u8>>> {
    // UTF-16 can decode to at most three UTF-8 bytes per code unit; escaping a
    // quote can then expand sixfold. The input is already capped at one MiB.
    let ceiling = input.len().saturating_mul(9).saturating_add(1024);
    let mut writer = Writer::with_limits(
        Vec::new(),
        WriterLimits {
            max_output_bytes: ceiling,
            max_token_bytes: ceiling,
            ..WriterLimits::default()
        },
    );
    let result = match Document::parse_bytes(input) {
        Ok(document) => writer.write_document(&document),
        Err(_) => match OwnedDocument::parse_bytes(input) {
            Ok(document) => writer.write_owned_document(&document),
            Err(error) => Err(WriteError::Xml(error)),
        },
    };
    match result {
        Ok(()) => Ok(Some(writer.into_inner())),
        Err(WriteError::Xml(error))
            if error.kind == ErrorKind::LimitExceeded(Limit::OwnedBytes) =>
        {
            Ok(None)
        }
        Err(error) => Err(io::Error::other(error)),
    }
}

fn serialized_comparison(comparison: Comparison) -> Comparison {
    match comparison {
        Comparison::Disagreement("acceptance") => Comparison::Disagreement("serialized acceptance"),
        Comparison::Disagreement(_) => {
            Comparison::Disagreement("serialized infoset values or structure")
        }
        other => other,
    }
}

fn validate_version(report: &[u8], allow_other_version: bool) -> io::Result<()> {
    let report = std::str::from_utf8(report).map_err(io::Error::other)?;
    let version = report
        .split_once("using libxml version ")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .filter(|version| version.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| io::Error::other("unrecognized xmllint version report"))?;
    if !allow_other_version && version != infoset::LIBXML2_COMPILED_VERSION {
        return Err(io::Error::other(format!(
            "oracle must use libxml2 {}; found {version}; --allow-other-libxml2 enables exploratory comparison",
            infoset::LIBXML2_VERSION
        )));
    }
    let features = report
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("compiled with:"))
        .ok_or_else(|| io::Error::other("missing xmllint build features"))?;
    for feature in ["Tree", "XPath"] {
        if !features.split_whitespace().any(|value| value == feature) {
            return Err(io::Error::other(format!(
                "oracle requires {feature} support"
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
enum Comparison {
    NotCompared,
    SkippedPolicy,
    Rejected,
    Semantic,
    AcceptanceOnly(&'static str),
    Disagreement(&'static str),
    Inconclusive,
}

fn compare_output(
    oracle_accepted: Option<bool>,
    accepted: bool,
    output: &[u8],
    expected: Option<&[u8]>,
    limit: Option<&'static str>,
) -> Comparison {
    match oracle_accepted {
        None => Comparison::Inconclusive,
        Some(oracle_accepted) if oracle_accepted != accepted => {
            Comparison::Disagreement("acceptance")
        }
        Some(false) => Comparison::Rejected,
        Some(true) => match expected {
            Some(expected) if expected == output => Comparison::Semantic,
            Some(_) => Comparison::Disagreement("infoset values or structure"),
            None => Comparison::AcceptanceOnly(limit.unwrap_or("projection unavailable")),
        },
    }
}

#[derive(Default)]
struct Counts {
    semantic: usize,
    rejected: usize,
    acceptance_only: usize,
    skipped: usize,
    inconclusive: usize,
    disagreements: usize,
}

impl Counts {
    fn record(&mut self, comparison: Comparison, label: &str) {
        let description = match comparison {
            Comparison::NotCompared => return,
            Comparison::SkippedPolicy => {
                self.skipped += 1;
                "skipped unsupported feature, encoding agreement, or unmatched limit policy"
                    .to_owned()
            }
            Comparison::Rejected => {
                self.rejected += 1;
                "rejection agreement".to_owned()
            }
            Comparison::Semantic => {
                self.semantic += 1;
                "input and serialized output acceptance and infoset agreement".to_owned()
            }
            Comparison::AcceptanceOnly(reason) => {
                self.acceptance_only += 1;
                format!("acceptance agreement; semantic comparison skipped ({reason})")
            }
            Comparison::Disagreement(reason) => {
                self.disagreements += 1;
                format!("disagreement ({reason})")
            }
            Comparison::Inconclusive => {
                self.inconclusive += 1;
                "inconclusive oracle exit or diagnostics".to_owned()
            }
        };
        eprintln!("{label}: {description}");
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
                    allow_other_version: false,
                    files: vec!["case.xml".into()],
                }),
            ),
            (
                vec!["--libxml2", "xmllint", "--", "-case"],
                Ok(Args {
                    oracle: Some("xmllint".into()),
                    allow_other_version: false,
                    files: vec!["-case".into()],
                }),
            ),
            (
                vec!["--allow-other-libxml2", "--libxml2", "xmllint"],
                Ok(Args {
                    oracle: Some("xmllint".into()),
                    allow_other_version: true,
                    files: vec![],
                }),
            ),
            (
                vec!["--allow-other-libxml2"],
                Err("--allow-other-libxml2 requires --libxml2".into()),
            ),
            (
                vec!["--allow-other-libxml2", "--allow-other-libxml2"],
                Err("unknown or repeated option: --allow-other-libxml2".into()),
            ),
            (
                vec!["--libxml2", ""],
                Err("--libxml2 requires an executable path".into()),
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

    #[test]
    fn serialized_oracle_bytes_and_dispositions() {
        for (input, expected) in [
            ("<r></r>", "<r/>"),
            (
                "<?xml version='1.0' encoding='uTf-8'?><r><![CDATA[]]></r>",
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><r><![CDATA[]]></r>",
            ),
        ] {
            assert_eq!(
                serialize_for_oracle(input.as_bytes()).unwrap().unwrap(),
                expected.as_bytes()
            );
        }
        let text = "<?xml version='1.0' encoding='UTF-16'?><r>😀</r>";
        for little in [true, false] {
            let mut input = if little {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in text.encode_utf16() {
                input.extend_from_slice(&if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            assert_eq!(
                serialize_for_oracle(&input).unwrap().unwrap(),
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><r>😀</r>".as_bytes()
            );
        }
        assert!(serialize_for_oracle(b"<r>").is_err());
        for (comparison, expected) in [
            (Comparison::Semantic, Comparison::Semantic),
            (Comparison::Inconclusive, Comparison::Inconclusive),
            (
                Comparison::Disagreement("acceptance"),
                Comparison::Disagreement("serialized acceptance"),
            ),
            (
                Comparison::Disagreement("infoset values or structure"),
                Comparison::Disagreement("serialized infoset values or structure"),
            ),
        ] {
            assert_eq!(serialized_comparison(comparison), expected);
        }
    }

    #[test]
    fn pinned_version_and_capabilities() {
        for (report, exploratory, accepted) in [
            (
                "xmllint: using libxml version 21504\n   compiled with: Tree XPath Output\n",
                false,
                true,
            ),
            (
                "using libxml version 21503\ncompiled with: Tree XPath",
                false,
                false,
            ),
            (
                "using libxml version 21503\ncompiled with: Tree XPath",
                true,
                true,
            ),
            (
                "using libxml version 21504\ncompiled with: Tree",
                false,
                false,
            ),
            (
                "using libxml version 21504\ncompiled with: XPath",
                true,
                false,
            ),
            ("using libxml version 21504", false, false),
            (
                "using libxml version broken\ncompiled with: Tree XPath",
                true,
                false,
            ),
            ("unrecognized program", true, false),
        ] {
            assert_eq!(
                validate_version(report.as_bytes(), exploratory).is_ok(),
                accepted,
                "{report}"
            );
        }
        assert!(validate_version(b"\xff", true).is_err());
    }

    #[test]
    fn comparisons_and_summary_preserve_every_disposition() {
        for (oracle, accepted, actual, expected, limit, result) in [
            (
                None,
                true,
                b"".as_slice(),
                None,
                None,
                Comparison::Inconclusive,
            ),
            (
                Some(true),
                false,
                b"",
                None,
                None,
                Comparison::Disagreement("acceptance"),
            ),
            (
                Some(false),
                true,
                b"",
                Some(b"value".as_slice()),
                None,
                Comparison::Disagreement("acceptance"),
            ),
            (Some(false), false, b"", None, None, Comparison::Rejected),
            (
                Some(true),
                true,
                b"value",
                Some(b"value".as_slice()),
                None,
                Comparison::Semantic,
            ),
            (
                Some(true),
                true,
                b"changed",
                Some(b"value".as_slice()),
                None,
                Comparison::Disagreement("infoset values or structure"),
            ),
            (
                Some(true),
                true,
                b"",
                None,
                Some("query bytes"),
                Comparison::AcceptanceOnly("query bytes"),
            ),
            (
                Some(true),
                true,
                b"",
                None,
                None,
                Comparison::AcceptanceOnly("projection unavailable"),
            ),
        ] {
            assert_eq!(
                compare_output(oracle, accepted, actual, expected, limit),
                result
            );
        }
        let mut counts = Counts::default();
        for comparison in [
            Comparison::NotCompared,
            Comparison::SkippedPolicy,
            Comparison::Rejected,
            Comparison::Semantic,
            Comparison::AcceptanceOnly("query bytes"),
            Comparison::Disagreement("acceptance"),
            Comparison::Inconclusive,
        ] {
            counts.record(comparison, "testcase");
        }
        assert_eq!(
            (
                counts.semantic,
                counts.rejected,
                counts.acceptance_only,
                counts.skipped,
                counts.inconclusive,
                counts.disagreements
            ),
            (1, 1, 1, 1, 1, 1)
        );
    }

    #[test]
    fn replay_policy_skips_do_not_start_the_oracle() {
        let oracle = Oracle {
            path: "not-an-executable".into(),
            limits: ProcessLimits {
                timeout: Duration::from_secs(1),
                max_output_bytes: 1024,
            },
        };
        assert_eq!(replay(b"<r/>", None).unwrap(), Comparison::NotCompared);
        let mut mismatch = vec![0xff, 0xfe];
        for unit in "<?xml version='1.0' encoding='UTF-8'?><r/>".encode_utf16() {
            mismatch.extend_from_slice(&unit.to_le_bytes());
        }
        for input in [
            b"<!DOCTYPE r><r/>".as_slice(),
            b"\xff",
            b"\0\0\xfe\xff",
            &mismatch,
        ] {
            assert_eq!(
                replay(input, Some(&oracle)).unwrap(),
                Comparison::SkippedPolicy
            );
        }
        assert_eq!(
            replay(b"<r>", Some(&oracle)).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        for input in [
            b"\xff\xfe<\0r\0/\0>\0".as_slice(),
            b"\xff\xfe\0\xdc",
            b"\xff\xfe",
        ] {
            assert_eq!(
                replay(input, Some(&oracle)).unwrap_err().kind(),
                io::ErrorKind::NotFound,
                "supported and malformed UTF-16 must reach the oracle"
            );
        }
    }
}
