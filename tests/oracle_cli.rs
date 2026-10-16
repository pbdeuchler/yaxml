#![forbid(unsafe_code)]

#[path = "../examples/support/process.rs"]
mod process;

use std::fs::{self, DirBuilder};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use process::{ProcessLimits, ProcessOutput};

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> io::Result<Self> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        for attempt in 0..128 {
            let path = std::env::temp_dir().join(format!(
                "yaxml-oracle-cli-{}-{timestamp}-{attempt}",
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
            "could not create a unique CLI test directory",
        ))
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn path_argument(path: &Path) -> &str {
    path.to_str()
        .expect("subprocess argument path is not UTF-8")
}

fn execute(program: &Path, args: &[&str], input: &[u8], seconds: u64) -> ProcessOutput {
    process::run(
        program,
        args,
        input,
        ProcessLimits {
            timeout: Duration::from_secs(seconds),
            max_output_bytes: 1024 * 1024,
        },
    )
    .unwrap_or_else(|error| panic!("failed to run {}: {error}", program.display()))
}

fn assert_build(output: ProcessOutput, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn replay_command_preserves_oracle_dispositions_and_exit_status() {
    // This is a command regression: build and execute the actual example so
    // changes in main's exit policy cannot hide behind helper-only tests.
    // Cargo and rustc are required, as in the crate's normal CI toolchain.
    let directory = TemporaryDirectory::new().unwrap();
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = repository.join("Cargo.toml");
    let target = directory.0.join("build");
    assert_build(
        execute(
            Path::new(env!("CARGO")),
            &[
                "build",
                "--offline",
                "--locked",
                "--manifest-path",
                path_argument(&manifest),
                "--example",
                "fuzz_replay",
                "--target-dir",
                path_argument(&target),
            ],
            b"",
            60,
        ),
        "replay example build",
    );
    let executable = target
        .join("debug/examples")
        .join(format!("fuzz_replay{}", std::env::consts::EXE_SUFFIX));
    let fixture_source = repository.join("tests/fixtures/fake_xmllint.rs");
    let fixture = directory
        .0
        .join(format!("oracle-base{}", std::env::consts::EXE_SUFFIX));
    let rustc = std::env::var_os("RUSTC").map_or_else(|| PathBuf::from("rustc"), PathBuf::from);
    assert_build(
        execute(
            &rustc,
            &[
                "-Funsafe_code",
                "--edition=2024",
                path_argument(&fixture_source),
                "-o",
                path_argument(&fixture),
            ],
            b"",
            60,
        ),
        "fake oracle build",
    );
    for (mode, input, exploratory, success, label, counts) in [
        (
            "agree",
            b"<r/>".as_slice(),
            false,
            true,
            "acceptance and infoset agreement",
            [1, 0, 0, 0, 0, 0],
        ),
        (
            "reject",
            b"<r>",
            false,
            true,
            "rejection agreement",
            [0, 1, 0, 0, 0, 0],
        ),
        (
            "disagree",
            b"<r/>",
            false,
            false,
            "disagreement (infoset values or structure)",
            [0, 0, 0, 0, 0, 1],
        ),
        (
            "reject",
            b"<r/>",
            false,
            false,
            "disagreement (acceptance)",
            [0, 0, 0, 0, 0, 1],
        ),
        (
            "agree",
            b"<r>",
            false,
            false,
            "disagreement (acceptance)",
            [0, 0, 0, 0, 0, 1],
        ),
        (
            "inconclusive",
            b"<r/>",
            false,
            false,
            "inconclusive oracle exit or diagnostics",
            [0, 0, 0, 0, 1, 0],
        ),
        (
            "diagnostics",
            b"<r/>",
            false,
            false,
            "inconclusive oracle exit or diagnostics",
            [0, 0, 0, 0, 1, 0],
        ),
        (
            "other-version",
            b"<r/>",
            true,
            true,
            "acceptance and infoset agreement",
            [1, 0, 0, 0, 0, 0],
        ),
    ] {
        let oracle = directory
            .0
            .join(format!("{mode}{}", std::env::consts::EXE_SUFFIX));
        if !oracle.exists() {
            fs::copy(&fixture, &oracle).unwrap();
        }
        let mut args = vec!["--libxml2", path_argument(&oracle)];
        if exploratory {
            args.push("--allow-other-libxml2");
        }
        let output = execute(&executable, &args, input, 15);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            output.status.success(),
            success,
            "{mode} {input:?}: {stderr}"
        );
        assert!(output.stdout.is_empty(), "replay unexpectedly wrote stdout");
        let [
            semantic,
            rejected,
            acceptance_only,
            skipped,
            inconclusive,
            disagreements,
        ] = counts;
        let summary = format!(
            "oracle summary: semantic_agreements={semantic} rejection_agreements={rejected} acceptance_only={acceptance_only} skipped_policy={skipped} inconclusive={inconclusive} disagreements={disagreements}"
        );
        assert_eq!(
            stderr
                .lines()
                .filter(|line| line.starts_with("oracle summary:"))
                .collect::<Vec<_>>(),
            [summary.as_str()],
            "{stderr}"
        );
        assert!(stderr.contains(&format!("stdin: {label}\n")), "{stderr}");
        let expected_mode = if exploratory { "exploratory" } else { "pinned" };
        assert!(
            stderr.contains(&format!("mode: {expected_mode}\n")),
            "{stderr}"
        );
    }
    let other_version = directory
        .0
        .join(format!("other-version{}", std::env::consts::EXE_SUFFIX));
    let output = execute(
        &executable,
        &["--libxml2", path_argument(&other_version)],
        b"<r/>",
        15,
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!output.status.success(), "wrong pinned version succeeded");
    assert!(
        stderr.contains("oracle must use libxml2 2.15.4; found 21503"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("oracle summary:"),
        "unvalidated oracle was counted"
    );
    let path = directory.0.clone();
    drop(directory);
    assert!(!path.exists(), "CLI test artifacts were not cleaned up");
}
