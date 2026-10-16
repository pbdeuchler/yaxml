#[path = "../examples/support/process.rs"]
mod process;

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use process::{ProcessLimits, run};

fn scratch_directories() -> BTreeSet<PathBuf> {
    let prefix = format!("yaxml-oracle-{}-", std::process::id());
    fs::read_dir(std::env::temp_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&prefix)
        })
        .collect()
}

#[test]
fn bounded_process_execution_and_cleanup() {
    let executable = std::env::current_exe().unwrap();
    let before = scratch_directories();
    let ordinary = ProcessLimits {
        timeout: Duration::from_secs(5),
        max_output_bytes: 16 * 1024,
    };
    let input = b"EOF input: \0\xff ; $(this-is-not-a-shell)\n";
    let output = run(
        &executable,
        &["--exact", "child_echo", "--ignored", "--nocapture"],
        input,
        ordinary,
    )
    .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.ends_with(input));
    assert_eq!(output.stderr, b"oracle stderr");
    assert_eq!(scratch_directories(), before);
    let exact_size = output.stdout.len() + output.stderr.len();
    for (ceiling, accepted) in [(exact_size, true), (exact_size - 1, false)] {
        let result = run(
            &executable,
            &["--exact", "child_echo", "--ignored", "--nocapture"],
            input,
            ProcessLimits {
                max_output_bytes: ceiling,
                ..ordinary
            },
        );
        if accepted {
            let actual = result.unwrap();
            assert!(actual.status.success());
            assert_eq!(actual.stdout, output.stdout);
            assert_eq!(actual.stderr, output.stderr);
        } else {
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::FileTooLarge);
        }
        assert_eq!(scratch_directories(), before);
    }

    let output = run(
        &executable,
        &["--exact", "child_nonzero", "--ignored", "--nocapture"],
        b"",
        ordinary,
    )
    .unwrap();
    assert_eq!(output.status.code(), Some(17));
    assert!(output.stdout.ends_with(b"ordinary failure"));
    assert_eq!(scratch_directories(), before);

    for (name, limits, expected) in [
        (
            "child_hang",
            ProcessLimits {
                timeout: Duration::from_millis(100),
                ..ordinary
            },
            io::ErrorKind::TimedOut,
        ),
        (
            "child_large_stdout",
            ProcessLimits {
                max_output_bytes: 1536,
                ..ordinary
            },
            io::ErrorKind::FileTooLarge,
        ),
        (
            "child_large_stderr",
            ProcessLimits {
                max_output_bytes: 1536,
                ..ordinary
            },
            io::ErrorKind::FileTooLarge,
        ),
        (
            "child_combined_output",
            ProcessLimits {
                max_output_bytes: 1536,
                ..ordinary
            },
            io::ErrorKind::FileTooLarge,
        ),
    ] {
        let start = Instant::now();
        let error = run(
            &executable,
            &["--exact", name, "--ignored", "--nocapture"],
            b"",
            limits,
        )
        .unwrap_err();
        assert_eq!(error.kind(), expected, "{name}: {error}");
        // This is a liveness deadline, not an assertion about timer precision.
        assert!(start.elapsed() < Duration::from_secs(5), "{name}");
        assert_eq!(scratch_directories(), before, "{name}");
    }

    let missing = std::env::temp_dir().join(format!(
        "yaxml-nonexistent-executable-{}",
        std::process::id()
    ));
    assert_eq!(
        run(&missing, &[], b"", ordinary).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    assert_eq!(scratch_directories(), before);

    for limits in [
        ProcessLimits {
            timeout: Duration::ZERO,
            ..ordinary
        },
        ProcessLimits {
            timeout: Duration::MAX,
            ..ordinary
        },
        ProcessLimits {
            max_output_bytes: 0,
            ..ordinary
        },
    ] {
        assert_eq!(
            run(Path::new("unused"), &[], b"", limits)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(scratch_directories(), before);
    }
}

// Ignored tests are invoked as real subprocesses by the test above. Direct
// writes bypass libtest's per-test capture; exit avoids its result footer.
#[test]
#[ignore]
fn child_echo() {
    assert_eq!(std::env::var_os("XML_CATALOG_FILES"), Some("".into()));
    assert_eq!(std::env::var_os("SGML_CATALOG_FILES"), Some("".into()));
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input).unwrap();
    io::stdout().write_all(&input).unwrap();
    io::stderr().write_all(b"oracle stderr").unwrap();
    std::process::exit(0);
}

#[test]
#[ignore]
fn child_nonzero() {
    io::stdout().write_all(b"ordinary failure").unwrap();
    std::process::exit(17);
}

#[test]
#[ignore]
fn child_hang() {
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[test]
#[ignore]
fn child_large_stdout() {
    let block = [b'x'; 8192];
    loop {
        io::stdout().write_all(&block).unwrap();
    }
}

#[test]
#[ignore]
fn child_large_stderr() {
    let block = [b'x'; 8192];
    loop {
        io::stderr().write_all(&block).unwrap();
    }
}

#[test]
#[ignore]
fn child_combined_output() {
    io::stdout().write_all(&[b'x'; 1024]).unwrap();
    io::stderr().write_all(&[b'y'; 1024]).unwrap();
    std::process::exit(0);
}
