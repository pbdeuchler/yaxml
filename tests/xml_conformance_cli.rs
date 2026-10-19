#![forbid(unsafe_code)]

#[path = "../examples/support/process.rs"]
mod process;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, DirBuilder};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SURFACES: [&str; 5] = ["reader", "document", "owned", "push", "buffered"];
const MAX_CASE_BYTES: usize = 1 << 20;

struct Directory(PathBuf);

impl Directory {
    fn new() -> io::Result<Self> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        for attempt in 0..128 {
            let path = std::env::temp_dir().join(format!(
                "yaxml-xmlconf-cli-{}-{timestamp}-{attempt}",
                std::process::id()
            ));
            let mut builder = DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a unique conformance CLI test directory",
        ))
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn argument(path: &Path) -> &str {
    path.to_str().expect("test path must be UTF-8")
}

fn execute(program: &Path, args: &[&str], input: &[u8], seconds: u64) -> process::ProcessOutput {
    process::run(
        program,
        args,
        input,
        process::ProcessLimits {
            timeout: Duration::from_secs(seconds),
            max_output_bytes: 4 * 1024 * 1024,
        },
    )
    .unwrap_or_else(|error| panic!("failed to run {}: {error}", program.display()))
}

fn build_example(repository: &Path, target: &Path) -> PathBuf {
    let output = execute(
        Path::new(env!("CARGO")),
        &[
            "build",
            "--offline",
            "--locked",
            "--manifest-path",
            argument(&repository.join("Cargo.toml")),
            "--example",
            "xml_conformance",
            "--target-dir",
            argument(target),
        ],
        b"",
        60,
    );
    assert!(
        output.status.success(),
        "example build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    target
        .join("debug/examples")
        .join(format!("xml_conformance{}", std::env::consts::EXE_SUFFIX))
}

fn write_catalog(path: &Path, rows: &[&str]) {
    let mut catalog = format!("yaxml-xmlconf-v1\t{}\n", rows.len());
    for row in rows {
        catalog.push_str(row);
        catalog.push('\n');
    }
    fs::write(path, catalog).unwrap();
}

fn write_required_pass(path: &Path, pair_count: usize, rows: &[&str]) {
    let mut baseline = format!("yaxml-xmlconf-required-v1\t{}\t{pair_count}\n", rows.len());
    for row in rows {
        baseline.push_str(row);
        baseline.push('\n');
    }
    fs::write(path, baseline).unwrap();
}

fn run_catalog(
    command: &Path,
    manifest: &Path,
    root: &Path,
    expected: usize,
) -> process::ProcessOutput {
    execute(
        command,
        &[
            "--manifest",
            argument(manifest),
            "--suite-root",
            argument(root),
            "--expected-cases",
            &expected.to_string(),
        ],
        b"",
        30,
    )
}

fn run_catalog_with_required(
    command: &Path,
    manifest: &Path,
    root: &Path,
    expected: usize,
    required: &Path,
) -> process::ProcessOutput {
    execute(
        command,
        &[
            "--manifest",
            argument(manifest),
            "--suite-root",
            argument(root),
            "--expected-cases",
            &expected.to_string(),
            "--required-pass",
            argument(required),
        ],
        b"",
        30,
    )
}

#[derive(Debug)]
struct ResultRow<'a> {
    id: &'a str,
    surface: &'a str,
    kind: &'a str,
    status: &'a str,
    reason: &'a str,
    error: &'a str,
    offset: &'a str,
}

fn result_rows(output: &[u8], cases: usize) -> Vec<ResultRow<'_>> {
    let text = std::str::from_utf8(output).expect("result table must be UTF-8");
    assert!(text.ends_with('\n'));
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some(format!("yaxml-xmlconf-results-v1\t{cases}\t5").as_str())
    );
    assert_eq!(
        lines.next(),
        Some(
            "id\tsurface\ttype\tcatalog_path\trun_path\tpath_policy\tstatus\treason\terror\toffset"
        )
    );
    let rows: Vec<_> = lines
        .map(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            assert_eq!(fields.len(), 10, "{line:?}");
            assert_eq!(fields[3], fields[4]);
            assert_eq!(fields[5], "none");
            ResultRow {
                id: fields[0],
                surface: fields[1],
                kind: fields[2],
                status: fields[6],
                reason: fields[7],
                error: fields[8],
                offset: fields[9],
            }
        })
        .collect();
    assert_eq!(rows.len(), cases * SURFACES.len());
    let pairs: BTreeSet<_> = rows.iter().map(|row| (row.id, row.surface)).collect();
    assert_eq!(pairs.len(), rows.len(), "duplicate result row");
    let mut by_case = BTreeMap::<_, Vec<_>>::new();
    for row in &rows {
        by_case.entry(row.id).or_default().push(row.surface);
    }
    assert_eq!(by_case.len(), cases);
    for surfaces in by_case.values_mut() {
        surfaces.sort_unstable();
        let mut expected = SURFACES;
        expected.sort_unstable();
        assert_eq!(*surfaces, expected);
    }
    rows
}

fn assert_case(
    rows: &[ResultRow<'_>],
    id: &str,
    kind: &str,
    status: &str,
    reason: &str,
    error: &str,
    offset: &str,
) {
    let selected: Vec<_> = rows.iter().filter(|row| row.id == id).collect();
    assert_eq!(selected.len(), SURFACES.len());
    for row in selected {
        assert_eq!(row.kind, kind, "{id} {}", row.surface);
        assert_eq!(row.status, status, "{id} {}", row.surface);
        assert_eq!(row.reason, reason, "{id} {}", row.surface);
        assert_eq!(row.error, error, "{id} {}", row.surface);
        assert_eq!(row.offset, offset, "{id} {}", row.surface);
    }
}

#[test]
fn actual_command_preserves_full_inventory_exit_policy_and_worker_bounds() {
    let directory = Directory::new().unwrap();
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    let command = build_example(repository, &directory.0.join("target"));
    let suite = directory.0.join("suite");
    fs::create_dir(&suite).unwrap();

    for (name, bytes) in [
        ("valid.xml", b"<r/>".as_slice()),
        ("invalid.xml", b"<r><c/></r>"),
        ("not-wf.xml", b"<r>"),
        ("dtd.xml", b"<!DOCTYPE r><r/>"),
        (
            "encoding.xml",
            b"<?xml version='1.0' encoding='Shift_JIS'?><r/>",
        ),
        ("namespace-off.xml", b"<p:r/>"),
        ("xml11.xml", b"<?xml version='1.1'?><r/>"),
        ("old.xml", b"<r/>"),
        ("error.xml", b"<r>"),
    ] {
        fs::write(suite.join(name), bytes).unwrap();
    }
    let rows = [
        "valid\tvalid\tnone\tXML1.0\t1.0\t5\tyes\tvalid.xml\tvalid.xml\t\t\t2.1",
        "invalid\tinvalid\tnone\tXML1.0\t1.0\t5\tyes\tinvalid.xml\tinvalid.xml\t\t\t2.1",
        "not-wf\tnot-wf\tnone\tXML1.0\t1.0\t5\tyes\tnot-wf.xml\tnot-wf.xml\t\t\t2.1",
        "dtd\tvalid\tnone\tXML1.0\t1.0\t5\tyes\tdtd.xml\tdtd.xml\t\t\t2.8",
        "encoding\tvalid\tnone\tXML1.0\t1.0\t5\tyes\tencoding.xml\tencoding.xml\t\t\t4.3.3",
        "namespace-off\tvalid\tnone\tXML1.0\t1.0\t5\tno\tnamespace-off.xml\tnamespace-off.xml\t\t\t2.3",
        "xml11\tvalid\tnone\tXML1.1\t1.1\t\tyes\txml11.xml\txml11.xml\t\t\t2.8",
        "old\tvalid\tnone\tXML1.0\t1.0\t1\tyes\told.xml\told.xml\t\t\t2.1",
        "error\terror\tnone\tXML1.0\t1.0\t5\tyes\terror.xml\terror.xml\t\t\t2.1",
    ];
    let manifest = directory.0.join("catalog.tsv");
    write_catalog(&manifest, &rows);
    let output = run_catalog(&command, &manifest, &suite, rows.len());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let observed = result_rows(&output.stdout, rows.len());
    assert_case(
        &observed,
        "valid",
        "valid",
        "pass",
        "well-formedness-acceptance",
        "",
        "",
    );
    assert_case(
        &observed,
        "invalid",
        "invalid",
        "pass",
        "well-formedness-acceptance",
        "",
        "",
    );
    assert_case(
        &observed,
        "not-wf",
        "not-wf",
        "pass",
        "well-formedness-rejection",
        "UnexpectedEof",
        "3",
    );
    assert_case(
        &observed,
        "dtd",
        "valid",
        "gap",
        "dtd-processing",
        "UnsupportedDtd",
        "0",
    );
    assert_case(
        &observed,
        "encoding",
        "valid",
        "gap",
        "encoding-support",
        "UnsupportedEncoding",
        "20",
    );
    assert_case(
        &observed,
        "namespace-off",
        "valid",
        "gap",
        "namespace-off-profile",
        "Namespace",
        "1",
    );
    assert_case(
        &observed,
        "xml11",
        "valid",
        "unscored",
        "unsupported-recommendation-or-version",
        "",
        "",
    );
    assert_case(
        &observed,
        "old",
        "valid",
        "unscored",
        "historical-edition",
        "",
        "",
    );
    assert_case(
        &observed,
        "error",
        "error",
        "unscored",
        "discretionary-error",
        "UnexpectedEof",
        "3",
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    for surface in SURFACES {
        assert!(
            stderr.contains(&format!(
                "surface={surface} pass=3 mismatch=0 gap=3 unscored=3 inconclusive=0\n"
            )),
            "{stderr}"
        );
    }
    assert!(stderr.contains("cases=9 result_rows=45 expected_rows=45\n"));

    let required = directory.0.join("required.tsv");
    write_required_pass(
        &required,
        15,
        &["valid\t11111", "invalid\t11111", "not-wf\t11111"],
    );
    let output = run_catalog_with_required(&command, &manifest, &suite, rows.len(), &required);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        result_rows(&output.stdout, rows.len()).len(),
        rows.len() * 5
    );
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("required_pass_pairs=15 required_pass_regressions=0\n")
    );

    write_required_pass(&required, 1, &["dtd\t10000"]);
    let output = run_catalog_with_required(&command, &manifest, &suite, rows.len(), &required);
    assert!(!output.status.success());
    let observed = result_rows(&output.stdout, rows.len());
    assert_case(
        &observed,
        "dtd",
        "valid",
        "gap",
        "dtd-processing",
        "UnsupportedDtd",
        "0",
    );
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("required_pass_pairs=1 required_pass_regressions=1\n")
    );

    fs::write(suite.join("accepted.xml"), b"<r/>").unwrap();
    let mismatch = directory.0.join("mismatch.tsv");
    write_catalog(
        &mismatch,
        &["mismatch\tnot-wf\tnone\tXML1.0\t1.0\t5\tyes\taccepted.xml\taccepted.xml\t\t\t2.1"],
    );
    let output = run_catalog(&command, &mismatch, &suite, 1);
    assert!(!output.status.success());
    let rows = result_rows(&output.stdout, 1);
    assert_case(
        &rows,
        "mismatch",
        "not-wf",
        "mismatch",
        "unexpected-acceptance",
        "",
        "",
    );

    fs::write(suite.join("oversize.xml"), vec![b'x'; MAX_CASE_BYTES + 1]).unwrap();
    fs::create_dir(suite.join("directory.xml")).unwrap();
    let unavailable = directory.0.join("unavailable.tsv");
    write_catalog(
        &unavailable,
        &[
            "missing\tvalid\tnone\tXML1.0\t1.0\t5\tyes\tmissing.xml\tmissing.xml\t\t\t2.1",
            "oversize\tvalid\tnone\tXML1.0\t1.0\t5\tyes\toversize.xml\toversize.xml\t\t\t2.1",
            "directory\tvalid\tnone\tXML1.0\t1.0\t5\tyes\tdirectory.xml\tdirectory.xml\t\t\t2.1",
        ],
    );
    let output = run_catalog(&command, &unavailable, &suite, 3);
    assert!(!output.status.success());
    let rows = result_rows(&output.stdout, 3);
    for id in ["missing", "oversize", "directory"] {
        assert_case(
            &rows,
            id,
            "valid",
            "inconclusive",
            "input-or-worker-failure",
            "",
            "",
        );
    }

    let worker_ok = execute(&command, &["--worker"], b"<r/>", 5);
    assert!(worker_ok.status.success());
    assert!(worker_ok.stderr.is_empty());
    let worker_text = String::from_utf8(worker_ok.stdout).unwrap();
    assert_eq!(worker_text.lines().next(), Some("yaxml-xmlconf-worker-v1"));
    for surface in SURFACES {
        assert!(worker_text.contains(&format!("{surface}\taccepted\t\t\n")));
    }
    assert_eq!(worker_text.lines().count(), 6);

    let worker_bad = execute(&command, &["--worker"], b"<r>", 5);
    assert!(worker_bad.status.success());
    assert!(worker_bad.stderr.is_empty());
    let worker_text = String::from_utf8(worker_bad.stdout).unwrap();
    assert_eq!(worker_text.lines().count(), 6);
    for surface in SURFACES {
        assert!(worker_text.contains(&format!("{surface}\trejected\tUnexpectedEof\t3\n")));
    }

    let worker_oversize = execute(&command, &["--worker"], &vec![b'x'; MAX_CASE_BYTES + 1], 5);
    assert!(!worker_oversize.status.success());
    assert!(worker_oversize.stdout.is_empty());
    assert!(!worker_oversize.stderr.is_empty());
}

#[test]
fn actual_command_rejects_cli_catalog_and_root_failures_without_dropping_rows() {
    let directory = Directory::new().unwrap();
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    let command = build_example(repository, &directory.0.join("target"));
    let suite = directory.0.join("suite");
    fs::create_dir(&suite).unwrap();
    fs::write(suite.join("valid.xml"), b"<r/>").unwrap();
    let manifest = directory.0.join("catalog.tsv");
    write_catalog(
        &manifest,
        &["one\tvalid\tnone\tXML1.0\t1.0\t5\tyes\tvalid.xml\tvalid.xml\t\t\t2.1"],
    );

    for args in [
        Vec::<&str>::new(),
        vec!["--unknown", "value"],
        vec!["--manifest", argument(&manifest)],
        vec![
            "--manifest",
            argument(&manifest),
            "--manifest",
            argument(&manifest),
            "--suite-root",
            argument(&suite),
            "--expected-cases",
            "1",
        ],
    ] {
        let output = execute(&command, &args, b"", 5);
        assert!(!output.status.success(), "accepted arguments {args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(!output.stderr.is_empty(), "{args:?}");
    }

    let wrong_count = run_catalog(&command, &manifest, &suite, 2);
    assert!(!wrong_count.status.success());
    assert!(wrong_count.stdout.is_empty());
    assert!(!wrong_count.stderr.is_empty());

    let non_file_manifest = run_catalog(&command, &suite, &suite, 1);
    assert!(!non_file_manifest.status.success());
    assert!(non_file_manifest.stdout.is_empty());
    assert!(!non_file_manifest.stderr.is_empty());

    let required = directory.0.join("required.tsv");
    let malformed_required = [
        (
            "unknown ID",
            "yaxml-xmlconf-required-v1\t1\t1\nunknown\t10000\n",
        ),
        ("missing header", "one\t10000\n"),
        ("missing row", "yaxml-xmlconf-required-v1\t1\t1\n"),
        (
            "duplicate ID",
            "yaxml-xmlconf-required-v1\t2\t2\none\t10000\none\t01000\n",
        ),
        (
            "invalid mask",
            "yaxml-xmlconf-required-v1\t1\t1\none\t1000x\n",
        ),
        ("zero mask", "yaxml-xmlconf-required-v1\t1\t0\none\t00000\n"),
    ];
    for (label, contents) in malformed_required {
        fs::write(&required, contents).unwrap();
        let output = run_catalog_with_required(&command, &manifest, &suite, 1, &required);
        assert!(!output.status.success(), "accepted {label}");
        assert!(output.stdout.is_empty(), "{label}");
        assert!(!output.stderr.is_empty(), "{label}");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let outside = directory.0.join("outside.xml");
        fs::write(&outside, b"<r/>").unwrap();
        symlink(&outside, suite.join("escape.xml")).unwrap();
        let escape = directory.0.join("escape.tsv");
        write_catalog(
            &escape,
            &["escape\tvalid\tnone\tXML1.0\t1.0\t5\tyes\tescape.xml\tescape.xml\t\t\t2.1"],
        );
        let output = run_catalog(&command, &escape, &suite, 1);
        assert!(!output.status.success());
        let rows = result_rows(&output.stdout, 1);
        assert_case(
            &rows,
            "escape",
            "valid",
            "inconclusive",
            "input-or-worker-failure",
            "",
            "",
        );
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("case escapes suite root")
        );
    }
}
