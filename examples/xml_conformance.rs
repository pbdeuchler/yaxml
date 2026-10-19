//! Run every case in a pinned, externally extracted XML Test Suite catalog.
#![forbid(unsafe_code)]

#[path = "support/conformance_catalog.rs"]
mod catalog;
#[path = "support/process.rs"]
mod process;
#[path = "support/conformance_worker.rs"]
mod worker;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const WORKER_HEADER: &str = "yaxml-xmlconf-worker-v1";
const CASE_SECONDS: u64 = 5;
const TOTAL_SECONDS: u64 = 300;

#[derive(Debug, Eq, PartialEq)]
struct Args {
    manifest: PathBuf,
    root: PathBuf,
    expected: usize,
    required_pass: Option<PathBuf>,
}

impl Args {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut manifest = None;
        let mut root = None;
        let mut expected = None;
        let mut required_pass = None;
        let mut args = args.into_iter();
        while let Some(option) = args.next() {
            let value = args
                .next()
                .filter(|s| !s.is_empty())
                .ok_or("option requires a value")?;
            if option == "--manifest" && manifest.is_none() {
                manifest = Some(PathBuf::from(value));
            } else if option == "--suite-root" && root.is_none() {
                root = Some(PathBuf::from(value));
            } else if option == "--required-pass" && required_pass.is_none() {
                required_pass = Some(PathBuf::from(value));
            } else if option == "--expected-cases" && expected.is_none() {
                let number = value.to_str().ok_or("case count must be UTF-8")?;
                if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err("case count must contain decimal digits".into());
                }
                let number = number.parse::<usize>().map_err(|_| "case count overflow")?;
                if !(1..=catalog::MAX_CASES).contains(&number) {
                    return Err("case count is outside the runner ceiling".into());
                }
                expected = Some(number);
            } else {
                return Err(format!(
                    "unknown or repeated option: {}",
                    option.to_string_lossy()
                ));
            }
        }
        Ok(Self {
            manifest: manifest.ok_or("--manifest is required")?,
            root: root.ok_or("--suite-root is required")?,
            expected: expected.ok_or("--expected-cases is required")?,
            required_pass,
        })
    }
}

fn bounded_read(input: impl Read, maximum: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input.take((maximum + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "runner input exceeds byte ceiling",
        ));
    }
    Ok(bytes)
}

fn read_file(path: &Path, maximum: usize) -> io::Result<Vec<u8>> {
    // Reject known special files before opening: opening a FIFO can block.
    // Recheck the opened handle too; this is not a hostile-filesystem sandbox.
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() > maximum as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "runner input must be a bounded regular file",
        ));
    }
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > maximum as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "runner input must be a bounded regular file",
        ));
    }
    bounded_read(file, maximum)
}

fn run_worker() -> Result<(), Box<dyn std::error::Error>> {
    use std::fmt::Write as _;
    let input = bounded_read(io::stdin().lock(), worker::MAX_CASE_BYTES)?;
    let mut output = format!("{WORKER_HEADER}\n");
    for surface in worker::SURFACES {
        match worker::observe(surface, &input)? {
            Ok(()) => writeln!(output, "{surface}\taccepted\t\t")?,
            Err(error) => writeln!(
                output,
                "{surface}\trejected\t{:?}\t{}",
                error.kind, error.offset
            )?,
        }
    }
    io::stdout().lock().write_all(output.as_bytes())?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Observation<'a> {
    error: Option<&'a str>,
    offset: Option<usize>,
}

fn known_error(kind: &str) -> bool {
    matches!(
        kind,
        "InvalidUtf8"
            | "InvalidUtf16"
            | "InvalidCharacter"
            | "UnexpectedEof"
            | "Syntax"
            | "InvalidName"
            | "MismatchedEndTag"
            | "DuplicateAttribute"
            | "Namespace"
            | "InvalidReference"
            | "UnknownEntity"
            | "EntityLoop"
            | "UnparsedEntity"
            | "ExternalEntityInAttribute"
            | "UnbalancedEntity"
            | "InvalidDtd"
            | "MissingRoot"
            | "MultipleRoots"
            | "TextOutsideRoot"
            | "InvalidDeclaration"
            | "UnsupportedEncoding"
            | "EncodingMismatch"
            | "Allocation"
            | "IdentifierExhausted"
            | "Aborted"
            | "InvalidState"
    ) || kind
        .strip_prefix("LimitExceeded(")
        .and_then(|s| s.strip_suffix(')'))
        .is_some_and(|limit| {
            matches!(
                limit,
                "InputBytes"
                    | "OutputBytes"
                    | "TokenBytes"
                    | "DecodedBytes"
                    | "DecodedTokenBytes"
                    | "Depth"
                    | "Attributes"
                    | "Events"
                    | "OwnedBytes"
                    | "DtdBytes"
                    | "EntityExpansion"
                    | "EntityReferences"
                    | "EntityDepth"
            )
        })
}

fn observations(output: &[u8], input_bytes: usize) -> Result<Vec<Observation<'_>>, String> {
    let text = std::str::from_utf8(output).map_err(|_| "worker output is not UTF-8")?;
    if !text.ends_with('\n') || text.contains('\r') {
        return Err("worker records are not LF terminated".into());
    }
    let mut lines = text.lines();
    if lines.next() != Some(WORKER_HEADER) {
        return Err("wrong worker protocol".into());
    }
    let mut results = Vec::new();
    for surface in worker::SURFACES {
        let fields: Vec<_> = lines
            .next()
            .ok_or("missing worker result")?
            .split('\t')
            .collect();
        if fields.len() != 4 || fields[0] != surface {
            return Err("worker results are missing, reordered or malformed".into());
        }
        let observation = match fields[1] {
            "accepted" if fields[2].is_empty() && fields[3].is_empty() => Observation {
                error: None,
                offset: None,
            },
            "rejected" if known_error(fields[2]) => {
                if fields[3].is_empty() || !fields[3].bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err("invalid worker error offset".into());
                }
                let offset = fields[3]
                    .parse::<usize>()
                    .map_err(|_| "worker offset overflow")?;
                if offset > input_bytes {
                    return Err("worker offset exceeds input".into());
                }
                Observation {
                    error: Some(fields[2]),
                    offset: Some(offset),
                }
            }
            _ => return Err("unknown worker outcome".into()),
        };
        results.push(observation);
    }
    if lines.next().is_some() {
        return Err("extra worker output".into());
    }
    Ok(results)
}

fn case_input(root: &Path, relative: &str) -> io::Result<Vec<u8>> {
    let path = root.join(relative).canonicalize()?;
    if !path.starts_with(root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "case escapes suite root",
        ));
    }
    read_file(&path, worker::MAX_CASE_BYTES)
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = read_file(&args.manifest, catalog::MAX_MANIFEST_BYTES)?;
    let cases = catalog::parse(&manifest, args.expected)?;
    let required_bytes = args
        .required_pass
        .as_ref()
        .map(|path| read_file(path, catalog::MAX_MANIFEST_BYTES))
        .transpose()?;
    let required = required_bytes
        .as_deref()
        .map(|bytes| catalog::required_passes(bytes, &cases))
        .transpose()?
        .unwrap_or_default();
    let root = args.root.canonicalize()?;
    if !root.is_dir() {
        return Err("suite root is not a directory".into());
    }
    let executable = std::env::current_exe()?;
    let expected_rows = cases
        .len()
        .checked_mul(worker::SURFACES.len())
        .ok_or("result count overflow")?;
    let mut counts = BTreeMap::new();
    let mut rows = 0;
    let mut failed = false;
    let mut regressions = 0usize;
    let start = Instant::now();
    let mut output = io::BufWriter::new(io::stdout().lock());
    writeln!(
        output,
        "yaxml-xmlconf-results-v1\t{}\t{}",
        cases.len(),
        worker::SURFACES.len()
    )?;
    writeln!(
        output,
        "id\tsurface\ttype\tcatalog_path\trun_path\tpath_policy\tstatus\treason\terror\toffset"
    )?;
    for case in cases {
        let (path, correction) = case.run_path();
        let result = (|| -> Result<_, String> {
            if start.elapsed() >= Duration::from_secs(TOTAL_SECONDS) {
                return Err("global-deadline".into());
            }
            let input = case_input(&root, path).map_err(|error| format!("input: {error}"))?;
            let remaining = Duration::from_secs(TOTAL_SECONDS).saturating_sub(start.elapsed());
            if remaining.is_zero() {
                return Err("global-deadline".into());
            }
            let captured = process::run(
                &executable,
                &["--worker"],
                &input,
                process::ProcessLimits {
                    timeout: Duration::from_secs(CASE_SECONDS).min(remaining),
                    max_output_bytes: 4096,
                },
            )
            .map_err(|error| format!("worker: {error}"))?;
            if !captured.status.success() || !captured.stderr.is_empty() {
                return Err("worker exit or diagnostics".into());
            }
            // Validate while captured bytes are live, then retain only bounded
            // owned outcome labels for the case's five output rows.
            Ok(observations(&captured.stdout, input.len())?
                .into_iter()
                .map(|observation| (observation.error.map(str::to_owned), observation.offset))
                .collect::<Vec<_>>())
        })();
        if let Err(detail) = &result {
            eprintln!("case {}: {detail}", case.id);
        }
        for (index, surface) in worker::SURFACES.into_iter().enumerate() {
            let (verdict, error, offset) = match &result {
                Ok(observed) => {
                    let (error, offset) = &observed[index];
                    (
                        catalog::classify(case, error.as_deref()),
                        error.as_deref().unwrap_or(""),
                        *offset,
                    )
                }
                Err(_) => (
                    catalog::Verdict {
                        status: "inconclusive",
                        reason: "input-or-worker-failure",
                    },
                    "",
                    None,
                ),
            };
            failed |= matches!(verdict.status, "mismatch" | "inconclusive");
            if required.expects(case.id, index) && verdict.status != "pass" {
                failed = true;
                regressions += 1;
                eprintln!(
                    "required pass regressed: {}/{surface} -> {} ({})",
                    case.id, verdict.status, verdict.reason
                );
            }
            *counts.entry((surface, verdict.status)).or_insert(0usize) += 1;
            writeln!(
                output,
                "{}\t{surface}\t{}\t{}\t{path}\t{correction}\t{}\t{}\t{error}\t{}",
                case.id,
                case.kind,
                case.path,
                verdict.status,
                verdict.reason,
                offset.map(|value| value.to_string()).unwrap_or_default()
            )?;
            rows += 1;
        }
    }
    output.flush()?;
    if rows != expected_rows {
        return Err("result inventory mismatch".into());
    }
    for surface in worker::SURFACES {
        eprint!("surface={surface}");
        for status in ["pass", "mismatch", "gap", "unscored", "inconclusive"] {
            eprint!(" {status}={}", counts.get(&(surface, status)).unwrap_or(&0));
        }
        eprintln!();
    }
    eprintln!(
        "cases={} result_rows={rows} expected_rows={expected_rows}",
        args.expected
    );
    eprintln!(
        "required_pass_pairs={} required_pass_regressions={regressions}",
        required.pairs
    );
    if failed {
        return Err(
            "conformance mismatches, regressions or inconclusive rows require classification"
                .into(),
        );
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args == [OsString::from("--worker")] {
        run_worker()
    } else {
        run(Args::parse(args)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_cli_options_and_count_bounds() {
        let valid = [
            "--manifest",
            "catalog.tsv",
            "--suite-root",
            "suite",
            "--expected-cases",
            "2585",
        ];
        assert_eq!(
            Args::parse(valid.map(OsString::from)).unwrap(),
            Args {
                manifest: "catalog.tsv".into(),
                root: "suite".into(),
                expected: 2585,
                required_pass: None,
            }
        );
        for args in [
            vec![],
            vec!["--manifest"],
            vec!["--unknown", "x"],
            vec!["--manifest", "a", "--manifest", "b"],
            vec!["--suite-root", ""],
            vec!["--expected-cases", "0"],
            vec!["--expected-cases", "10001"],
            vec!["--expected-cases", "9999999999999999999999999999999"],
            vec!["--expected-cases", "+1"],
            vec!["--expected-cases", "1.0"],
            vec!["--manifest", "a", "--suite-root", "b"],
        ] {
            assert!(Args::parse(args.into_iter().map(OsString::from)).is_err());
        }
        let mut with_baseline = valid.map(OsString::from).to_vec();
        with_baseline.extend(["--required-pass", "required.tsv"].map(OsString::from));
        assert_eq!(
            Args::parse(with_baseline.clone()).unwrap().required_pass,
            Some("required.tsv".into())
        );
        with_baseline.extend(["--required-pass", "again.tsv"].map(OsString::from));
        assert!(Args::parse(with_baseline).is_err());
    }

    #[test]
    fn worker_protocol_is_complete_ordered_and_typed() {
        let good = format!(
            "{WORKER_HEADER}\nreader\taccepted\t\t\ndocument\trejected\tSyntax\t3\nowned\trejected\tInvalidDtd\t0\npush\trejected\tLimitExceeded(Depth)\t4\nbuffered\trejected\tInvalidUtf16\t2\n"
        );
        let observed = observations(good.as_bytes(), 4).unwrap();
        assert_eq!(observed.len(), 5);
        assert_eq!(
            observed[0],
            Observation {
                error: None,
                offset: None
            }
        );
        assert_eq!(
            observed[1],
            Observation {
                error: Some("Syntax"),
                offset: Some(3)
            }
        );
        for bad in [
            good.replace(WORKER_HEADER, "other"),
            good.trim_end().to_owned(),
            good.replace("reader\taccepted", "document\taccepted"),
            good.replace("\n", "\r\n"),
            format!("{good}extra\n"),
            good.replace("Syntax", "Invented"),
            good.replace("Depth", "Invented"),
            good.replace("Syntax\t3", "Syntax\t5"),
            good.replace("Syntax\t3", "Syntax\t-1"),
            good.replace("Syntax\t3", "Syntax\t9999999999999999999999999"),
            good.replace("reader\taccepted\t\t", "reader\taccepted\tSyntax\t0"),
            good.replace("owned\trejected\tInvalidDtd\t0\n", ""),
        ] {
            assert!(observations(bad.as_bytes(), 4).is_err(), "{bad:?}");
        }
        assert!(observations(&[0xff], 4).is_err());
        assert!(!known_error("LimitExceeded(Depth))"));
    }

    #[test]
    fn bounded_input_accepts_the_ceiling_and_rejects_one_more_byte() {
        assert_eq!(bounded_read(b"abcd".as_slice(), 4).unwrap(), b"abcd");
        assert!(bounded_read(b"abcde".as_slice(), 4).is_err());
    }
}
