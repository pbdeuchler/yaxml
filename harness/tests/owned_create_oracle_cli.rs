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
            "yaxml-create-oracle-cli-{}-{timestamp}",
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
    path.to_str().expect("test path must be UTF-8")
}

fn build(program: &Path, args: &[&str]) {
    let output = process::run(
        program,
        args,
        b"",
        process::ProcessLimits {
            timeout: Duration::from_secs(60),
            max_output_bytes: 1024 * 1024,
        },
    )
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn actual_command_rejects_bad_version_status_diagnostics_and_changed_results() {
    let directory = Directory::new().unwrap();
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = directory.0.join("target");
    build(
        Path::new(env!("CARGO")),
        &[
            "build",
            "--offline",
            "--locked",
            "--manifest-path",
            argument(&repository.join("Cargo.toml")),
            "--example",
            "owned_create_oracle",
            "--target-dir",
            argument(&target),
        ],
    );
    let command = target.join("debug/examples").join(format!(
        "owned_create_oracle{}",
        std::env::consts::EXE_SUFFIX
    ));
    let fixture = directory
        .0
        .join(format!("fixture{}", std::env::consts::EXE_SUFFIX));
    let rustc = std::env::var_os("RUSTC").map_or_else(|| PathBuf::from("rustc"), PathBuf::from);
    build(
        &rustc,
        &[
            "-Funsafe_code",
            "--edition=2024",
            argument(&repository.join("tests/fixtures/fake_create_oracle.rs")),
            "-o",
            argument(&fixture),
        ],
    );
    for mode in [
        "good",
        "version-exit",
        "version-stderr",
        "version-changed",
        "version-extra",
        "version-truncated",
        "body-exit",
        "body-stderr",
        "body-changed",
        "body-extra",
        "body-truncated",
    ] {
        let output = process::run_with_environment(
            &command,
            &["--libxml2-helper", argument(&fixture)],
            b"",
            process::ProcessLimits {
                timeout: Duration::from_secs(10),
                max_output_bytes: 65536,
            },
            &[("YAXML_CREATE_ORACLE_FAKE".to_owned(), mode.to_owned())],
            &[],
            None,
        )
        .unwrap();
        if mode == "good" {
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stderr.is_empty());
            assert_eq!(
                output.stdout,
                include_bytes!("fixtures/create_oracle_expected.tsv")
            );
        } else {
            assert!(!output.status.success(), "accepted fixture mode {mode}");
            assert!(
                output.stdout.is_empty(),
                "printed success before validating {mode}"
            );
            assert!(
                !output.stderr.is_empty(),
                "missing failure diagnostic for {mode}"
            );
        }
    }
}
