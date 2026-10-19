#![forbid(unsafe_code)]

#[allow(dead_code)]
#[path = "../examples/support/parse_benchmark.rs"]
mod benchmark;
#[path = "../examples/support/process.rs"]
mod process;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("yaxml-parse-cli-{}-{stamp}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn path(path: &Path) -> &str {
    path.to_str().expect("test paths are UTF-8")
}

fn execute(program: &Path, args: &[&str]) -> process::ProcessOutput {
    process::run(
        program,
        args,
        b"",
        process::ProcessLimits {
            timeout: Duration::from_secs(60),
            max_output_bytes: 1024 * 1024,
        },
    )
    .unwrap_or_else(|error| panic!("{}: {error}", program.display()))
}

fn build(directory: &Directory) -> (PathBuf, PathBuf) {
    let target = directory.0.join("target");
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = execute(
        Path::new(env!("CARGO")),
        &[
            "build",
            "--offline",
            "--locked",
            "--manifest-path",
            path(&repository.join("Cargo.toml")),
            "--example",
            "parse_bench",
            "--example",
            "parse_benchmark",
            "--target-dir",
            path(&target),
        ],
    );
    assert!(
        output.status.success(),
        "build: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let binary = |name: &str| {
        target
            .join("debug/examples")
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
    };
    (binary("parse_bench"), binary("parse_benchmark"))
}

fn successful_row(helper: &Path, mode: &str, input: &Path, chunk: &str) -> benchmark::Row {
    let output = execute(helper, &[mode, path(input), "2", chunk]);
    assert!(
        output.status.success(),
        "{mode}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    benchmark::Row::parse(&output.stdout).unwrap()
}

fn rejected(helper: &Path, args: &[&str]) {
    let output = execute(helper, args);
    assert!(!output.status.success(), "unexpected success: {args:?}");
    assert!(
        output.stdout.is_empty(),
        "partial sample published: {args:?}"
    );
    assert!(
        !output.stderr.is_empty(),
        "missing failure diagnostic: {args:?}"
    );
}

fn literal_anchor(helper: &Path, input: &Path, modes: &[&str]) {
    // Independently framed 295-byte transcript, also evaluated with Ruby's
    // little-endian Q< packing. No parser result supplies the expected fields.
    let xml = "<!--pre--><?top d?><p:r a='x&#x9;y' xmlns='urn:d' p:b='&amp;' xmlns:p='urn:p'>x&amp;<![CDATA[y]]>&#x7a;<p:e/><!--c--><?pi data?></p:r><!--post-->";
    fs::write(input, xml).unwrap();
    for mode in modes {
        let observed = successful_row(helper, mode, input, "1");
        assert_eq!(observed.observations, [2, 2, 2, 4, 3, 2], "{mode}");
        assert_eq!(observed.hash, 0x63bbd2c6aa2a2ec9, "{mode}");
    }
}

#[test]
fn rust_helper_observes_complete_documents_and_rejects_invalid_requests() {
    let directory = Directory::new();
    let (rust, driver) = build(&directory);
    let version = execute(&rust, &["--version"]);
    assert!(version.status.success());
    assert!(version.stderr.is_empty());
    assert_eq!(
        version.stdout,
        b"YAXML-PARSE-BENCH-1\tengine=yaxml\tversion=0.1.0\n"
    );
    let input = directory.0.join("input.xml");
    for fixture in benchmark::FIXTURES {
        fs::write(&input, benchmark::fixture(fixture).unwrap()).unwrap();
        let expected = successful_row(&rust, "dom", &input, "7");
        for mode in benchmark::MODES {
            for chunk in ["1", "7", "8192"] {
                let observed = successful_row(&rust, mode, &input, chunk);
                expected
                    .same_observation(&observed)
                    .unwrap_or_else(|error| panic!("{fixture}/{mode}/{chunk}: {error}"));
                assert_eq!(observed.mode, mode);
                assert_eq!(observed.iterations, 2);
                assert_eq!(observed.chunk.to_string(), chunk);
                if mode != "dom" && mode != "owned" {
                    assert_eq!(observed.times[1], 0);
                }
            }
        }
    }
    literal_anchor(&rust, &input, &benchmark::MODES);
    fs::write(&input, b"<r/>").unwrap();
    for args in [
        vec![],
        vec!["--version", "extra"],
        vec!["unknown", path(&input), "1", "1"],
        vec!["dom", path(&input), "0", "1"],
        vec!["dom", path(&input), "10001", "1"],
        vec!["dom", path(&input), "1", "0"],
        vec!["dom", path(&input), "1", "65537"],
        vec!["dom", path(&input), "+1", "1"],
        vec!["dom", path(&input), "1", "1", "extra"],
        vec!["dom", path(&directory.0), "1", "1"],
    ] {
        rejected(&rust, &args);
    }
    for bytes in [
        b"".as_slice(),
        b"<r>",
        b"<r/>tail",
        b"<!DOCTYPE r><r/>",
        b"<?xml version='1.1'?><r/>",
        b"<?xml version='1.00'?><r/>",
        b"<?xml version='1.0' encoding='UTF-7'?><r/>",
        b"<?xml version='1.0' encoding='Shift_JIS'?><r/>",
        b"<p:r/>",
        b"<r a='1' a='2'/>",
        &[0xff],
        &[0xff, 0xfe, b'<', 0, b'r', 0, b'/', 0, b'>', 0],
    ] {
        fs::write(&input, bytes).unwrap();
        for mode in benchmark::MODES {
            rejected(&rust, &[mode, path(&input), "1", "7"]);
        }
    }
    fs::write(&input, vec![b' '; benchmark::MAX_INPUT + 1]).unwrap();
    rejected(&rust, &["dom", path(&input), "1", "7"]);
    fs::write(&input, vec![b' '; benchmark::MAX_INPUT]).unwrap();
    rejected(&rust, &["dom", path(&input), "10000", "7"]);
    fs::remove_file(&input).unwrap();
    rejected(&rust, &["dom", path(&input), "1", "7"]);

    // Two real Rust helpers have different roles: the C-version check must not
    // allow a plausible but incorrectly labelled executable to supply samples.
    let base = [
        "--libxml2-helper",
        path(&rust),
        "--yaxml-helper",
        path(&rust),
        "--fixture",
        "tiny",
        "--mode",
        "dom",
    ];
    rejected(&driver, &base);
    let mut bad = base.to_vec();
    bad.extend(["--iterations", "0"]);
    rejected(&driver, &bad);
}

#[test]
#[ignore = "requires YAXML_PARSE_LIBXML2_HELPER built with pinned libxml2 2.15.4"]
fn pinned_helpers_and_driver_agree_on_all_versioned_fixtures_and_failure_paths() {
    let c = PathBuf::from(
        std::env::var_os("YAXML_PARSE_LIBXML2_HELPER")
            .expect("set YAXML_PARSE_LIBXML2_HELPER for this explicit test"),
    );
    let directory = Directory::new();
    let (rust, driver) = build(&directory);
    let version = execute(&c, &["--version"]);
    assert!(version.status.success());
    assert!(version.stderr.is_empty());
    assert_eq!(
        version.stdout,
        b"YAXML-PARSE-BENCH-1\tengine=libxml2\tversion=21504\n"
    );
    let input = directory.0.join("input.xml");
    literal_anchor(&c, &input, &["dom", "reader", "push", "buffered"]);
    for fixture in benchmark::FIXTURES {
        fs::write(&input, benchmark::fixture(fixture).unwrap()).unwrap();
        let expected = successful_row(&rust, "dom", &input, "7");
        for mode in ["dom", "reader", "push", "buffered"] {
            for chunk in ["1", "7", "8192"] {
                let observed = successful_row(&c, mode, &input, chunk);
                expected
                    .same_observation(&observed)
                    .unwrap_or_else(|error| panic!("{fixture}/{mode}/{chunk}: {error}"));
            }
        }
    }
    for mode in benchmark::MODES {
        let output = execute(
            &driver,
            &[
                "--libxml2-helper",
                path(&c),
                "--yaxml-helper",
                path(&rust),
                "--fixture",
                "tiny",
                "--mode",
                mode,
                "--iterations",
                "2",
                "--chunk-size",
                "1",
                "--samples",
                "2",
            ],
        );
        assert!(
            output.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let output = String::from_utf8(output.stdout).unwrap();
        let rows: Vec<_> = output.lines().collect();
        assert_eq!(rows.len(), 5);
        assert!(rows[0].ends_with("status=preflight_complete"));
        for (row, prefix) in rows[1..].iter().zip([
            "sample=1\torder=yaxml_first\tYAXML-PARSE-BENCH-1\tengine=yaxml",
            "sample=1\torder=yaxml_first\tYAXML-PARSE-BENCH-1\tengine=libxml2",
            "sample=2\torder=libxml2_first\tYAXML-PARSE-BENCH-1\tengine=libxml2",
            "sample=2\torder=libxml2_first\tYAXML-PARSE-BENCH-1\tengine=yaxml",
        ]) {
            assert!(row.starts_with(prefix), "{row}");
        }
    }
    // Exercise semantic framing independently of the throughput corpus. These
    // spellings make the SAX/DOM normalization and namespace-order differences
    // observable even when the logical document is unchanged.
    for xml in [
        "<r a='v' xmlns:p='urn:p' p:b='x' xmlns:q='urn:q'><p:e q:a='y'/></r>",
        "<r xmlns='urn:root'><x xmlns=''><y/></x></r>",
        "<r a='&amp;#38; &quot; &#x9; &#13; &#x1F600;'>a<![CDATA[b]]>&amp;c<![CDATA[]]>d</r>",
        "<!--before--><?start?><r/><?end ?><!--after-->",
        "<?xml version='1.0' encoding='utf-8'?><r>é😀</r>",
        "<r xmlns:p='urn:a&amp;b'><p:e p:a='v'/></r>",
    ] {
        fs::write(&input, xml).unwrap();
        let expected = successful_row(&rust, "dom", &input, "1");
        for mode in ["dom", "reader", "push", "buffered"] {
            let observed = successful_row(&c, mode, &input, "1");
            expected
                .same_observation(&observed)
                .unwrap_or_else(|error| panic!("independent spelling {xml:?}/{mode}: {error}"));
        }
    }
    for bytes in [
        b"".as_slice(),
        b"<r>",
        b"<r/>tail",
        b"<!DOCTYPE r><r/>",
        b"<?xml version='1.1'?><r/>",
        b"<?xml version='1.00'?><r/>",
        b"<?xml version='1.0' encoding='UTF-7'?><r/>",
        b"<?xml version='1.0' encoding='Shift_JIS'?><r/>",
        b"<p:r/>",
        b"<r a='1' a='2'/>",
        &[0xff],
        &[0xff, 0xfe, b'<', 0, b'r', 0, b'/', 0, b'>', 0],
    ] {
        fs::write(&input, bytes).unwrap();
        for mode in ["dom", "reader", "push", "buffered"] {
            rejected(&c, &[mode, path(&input), "1", "7"]);
        }
    }
    for args in [
        vec![],
        vec!["--version", "extra"],
        vec!["owned", path(&input), "1", "1"],
        vec!["dom", path(&input), "0", "1"],
        vec!["dom", path(&input), "10001", "1"],
        vec!["dom", path(&input), "1", "0"],
        vec!["dom", path(&input), "1", "65537"],
        vec!["dom", path(&input), "+1", "1"],
        vec!["dom", path(&input), "1", "1", "extra"],
        vec!["dom", path(&directory.0), "1", "1"],
    ] {
        rejected(&c, &args);
    }
}

#[test]
fn driver_failure_gates_preserve_only_complete_verified_pairs() {
    let directory = Directory::new();
    let (_, driver) = build(&directory);
    let rust = directory
        .0
        .join(format!("r-helper{}", std::env::consts::EXE_SUFFIX));
    let c = directory
        .0
        .join(format!("c-helper{}", std::env::consts::EXE_SUFFIX));
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/parse_benchmark_helper.rs");
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let compiled = execute(
        Path::new(&compiler),
        &["--edition=2024", path(&source), "-o", path(&rust)],
    );
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    fs::copy(&rust, &c).unwrap();
    for behavior in [
        "normal",
        "schema",
        "wrong-work",
        "semantic",
        "exit",
        "large-output",
        "input-change",
        "timeout",
        "stderr",
        "late-exit",
    ] {
        fs::write(directory.0.join("behavior"), behavior).unwrap();
        for engine in ["yaxml", "libxml2"] {
            let counter = directory.0.join(format!("{engine}.calls"));
            if counter.exists() {
                fs::remove_file(counter).unwrap();
            }
        }
        let output = execute(
            &driver,
            &[
                "--libxml2-helper",
                path(&c),
                "--yaxml-helper",
                path(&rust),
                "--fixture",
                "tiny",
                "--mode",
                "dom",
                "--iterations",
                "1",
                "--samples",
                "2",
                "--chunk-size",
                "7",
                "--timeout-ms",
                "1000",
            ],
        );
        let text = String::from_utf8(output.stdout).unwrap();
        let rows: Vec<_> = text.lines().collect();
        assert!(
            rows[0].ends_with("status=preflight_complete"),
            "{behavior}: {text}"
        );
        if behavior == "normal" {
            assert!(output.status.success());
            assert!(output.stderr.is_empty());
            assert_eq!(rows.len(), 5);
        } else {
            assert!(!output.status.success(), "{behavior}");
            assert!(!output.stderr.is_empty(), "{behavior}");
            assert_eq!(
                rows.len(),
                if behavior == "late-exit" { 3 } else { 1 },
                "{behavior}: {text}"
            );
        }
    }
}
