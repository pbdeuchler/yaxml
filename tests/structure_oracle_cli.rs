#![forbid(unsafe_code)]

#[path = "../examples/support/process.rs"]
mod process;

use std::fs::{self, DirBuilder};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct Directory(PathBuf);

impl Directory {
    fn new() -> io::Result<Self> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "yaxml-structure-cli-{}-{timestamp}",
            std::process::id()
        ));
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn argument(path: &Path) -> &str {
    path.to_str().expect("fixture path must be UTF-8")
}

fn run(program: &Path, args: &[&str]) -> process::ProcessOutput {
    process::run(
        program,
        args,
        b"",
        process::ProcessLimits {
            timeout: Duration::from_secs(60),
            max_output_bytes: 1024 * 1024,
        },
    )
    .unwrap()
}

fn built(output: process::ProcessOutput) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn actual_command_enforces_comparison_failure_and_classification_gates() {
    let directory = Directory::new().unwrap();
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = directory.0.join("target");
    built(run(
        Path::new(env!("CARGO")),
        &[
            "build",
            "--offline",
            "--locked",
            "--manifest-path",
            argument(&repository.join("Cargo.toml")),
            "--example",
            "structure_oracle",
            "--target-dir",
            argument(&target),
        ],
    ));
    let executable = target
        .join("debug/examples")
        .join(format!("structure_oracle{}", std::env::consts::EXE_SUFFIX));
    let fixture = directory
        .0
        .join(format!("fixture{}", std::env::consts::EXE_SUFFIX));
    let rustc = std::env::var_os("RUSTC").map_or_else(|| PathBuf::from("rustc"), PathBuf::from);
    built(run(
        &rustc,
        &[
            "-Funsafe_code",
            "--edition=2024",
            argument(&repository.join("tests/fixtures/fake_structure.rs")),
            "-o",
            argument(&fixture),
        ],
    ));
    for (mode, case, exploratory, success, counts, label) in [
        (
            "agree",
            "append-text",
            false,
            true,
            [1, 0, 0, 0, 0, 0],
            "exact tree and byte agreement",
        ),
        (
            "agree",
            "text-coalescing",
            false,
            true,
            [0, 1, 0, 0, 0, 0],
            "projected agreement",
        ),
        (
            "agree",
            "root-unlink",
            false,
            true,
            [0, 0, 1, 0, 0, 0],
            "intentional difference; libxml2 result verified",
        ),
        (
            "agree",
            "ownership-oom",
            false,
            true,
            [0, 0, 0, 1, 0, 0],
            "not compared",
        ),
        (
            "agree",
            "namespace-moves",
            false,
            true,
            [0, 0, 0, 1, 0, 0],
            "not compared",
        ),
        (
            "before",
            "append-text",
            false,
            false,
            [0, 0, 0, 0, 1, 0],
            "disagreement (initial kinds",
        ),
        (
            "before-bytes",
            "append-text",
            false,
            false,
            [0, 0, 0, 0, 1, 0],
            "disagreement (initial serialized bytes)",
        ),
        (
            "after",
            "append-text",
            false,
            false,
            [0, 0, 0, 0, 1, 0],
            "disagreement (result kinds",
        ),
        (
            "after-bytes",
            "append-text",
            false,
            false,
            [0, 0, 0, 0, 1, 0],
            "disagreement (result serialized bytes)",
        ),
        (
            "returned",
            "append-text",
            false,
            false,
            [0, 0, 0, 0, 1, 0],
            "disagreement (returned survivor classification)",
        ),
        (
            "truncated",
            "append-text",
            false,
            false,
            [0, 0, 0, 0, 0, 1],
            "inconclusive malformed protocol",
        ),
        (
            "trailing",
            "append-text",
            false,
            false,
            [0, 0, 0, 0, 0, 1],
            "inconclusive malformed protocol",
        ),
        (
            "status",
            "append-text",
            false,
            false,
            [0, 0, 0, 0, 0, 1],
            "inconclusive exit=Some(70)",
        ),
        (
            "diagnostics",
            "append-text",
            false,
            false,
            [0, 0, 0, 0, 0, 1],
            "inconclusive exit=Some(0)",
        ),
        (
            "other-version",
            "append-text",
            true,
            true,
            [1, 0, 0, 0, 0, 0],
            "exact tree and byte agreement",
        ),
    ] {
        let helper = directory
            .0
            .join(format!("{mode}{}", std::env::consts::EXE_SUFFIX));
        if !helper.exists() {
            fs::copy(&fixture, &helper).unwrap();
        }
        let mut args = vec!["--libxml2-helper", argument(&helper), case];
        if exploratory {
            args.push("--allow-other-libxml2");
        }
        let output = run(&executable, &args);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.success(), success, "{mode} {case}: {stderr}");
        assert!(output.stdout.is_empty());
        let [
            exact,
            projected,
            intentional,
            skipped,
            disagreements,
            inconclusive,
        ] = counts;
        let summary = format!(
            "structural summary: exact={exact} projected={projected} intentional={intentional} skipped={skipped} disagreements={disagreements} inconclusive={inconclusive}"
        );
        assert_eq!(
            stderr
                .lines()
                .filter(|line| line.starts_with("structural summary:"))
                .collect::<Vec<_>>(),
            [summary.as_str()],
            "{stderr}"
        );
        assert!(stderr.contains(label), "{stderr}");
        assert!(stderr.contains(if exploratory {
            "mode: exploratory\n"
        } else {
            "mode: pinned\n"
        }));
    }
    for mode in ["other-version", "runtime-mismatch"] {
        let helper = directory
            .0
            .join(format!("{mode}{}", std::env::consts::EXE_SUFFIX));
        if !helper.exists() {
            fs::copy(&fixture, &helper).unwrap();
        }
        let output = run(
            &executable,
            &["--libxml2-helper", argument(&helper), "append-text"],
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(!output.status.success());
        assert!(stderr.contains("requires libxml2 2.15.4"), "{stderr}");
        assert!(!stderr.contains("structural summary:"));
    }
    let output = run(
        &executable,
        &["--libxml2-helper", "missing", "unknown-case"],
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("unknown structural case")
    );
    let path = directory.0.clone();
    drop(directory);
    assert!(!path.exists());
}
