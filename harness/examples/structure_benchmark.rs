//! Alternate matched structural-edit samples between yaxml and pinned libxml2.
#![forbid(unsafe_code)]

#[path = "support/process.rs"]
mod process;
#[path = "support/structure_benchmark.rs"]
mod structure_benchmark;

use std::fs::{self, DirBuilder};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use structure_benchmark::{Config, Row};

struct RequestDirectory(PathBuf);

impl RequestDirectory {
    fn new() -> io::Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        for attempt in 0..128 {
            let path = std::env::temp_dir().join(format!(
                "yaxml-structure-bench-{}-{stamp}-{attempt}",
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
            "could not create a private matched benchmark directory",
        ))
    }

    fn request(&self) -> PathBuf {
        self.0.join("fixture.request")
    }
}

impl Drop for RequestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn env_pairs(config: &Config) -> [(String, String); 9] {
    [
        ("YAXML_STRUCTURE_CASE".into(), config.case.clone()),
        ("YAXML_STRUCTURE_ENCODING".into(), config.encoding.clone()),
        ("YAXML_STRUCTURE_WIDTH".into(), config.width.to_string()),
        ("YAXML_STRUCTURE_DEPTH".into(), config.depth.to_string()),
        (
            "YAXML_STRUCTURE_PAYLOAD_BYTES".into(),
            config.payload_bytes.to_string(),
        ),
        (
            "YAXML_STRUCTURE_ITERATIONS".into(),
            config.iterations.to_string(),
        ),
        (
            "YAXML_STRUCTURE_OPERATIONS".into(),
            config.operations.to_string(),
        ),
        (
            "YAXML_STRUCTURE_TRAVERSAL_PASSES".into(),
            config.traversal_passes.to_string(),
        ),
        (
            "YAXML_STRUCTURE_SERIALIZATION_PASSES".into(),
            config.serialization_passes.to_string(),
        ),
    ]
}

fn rust_row(config: &Config, export: Option<&Path>) -> Result<Row, Box<dyn std::error::Error>> {
    let mut environment = env_pairs(config).to_vec();
    if let Some(path) = export {
        environment.push((
            "YAXML_STRUCTURE_EXPORT".into(),
            path.to_str()
                .ok_or("matched benchmark request path is not UTF-8")?
                .into(),
        ));
    }
    let output = process::run_with_environment(
        &config.yaxml_helper,
        &[],
        b"",
        process::ProcessLimits {
            timeout: Duration::from_secs(60),
            max_output_bytes: 1024 * 1024,
        },
        &environment,
        &[
            "YAXML_STRUCTURE_EXPORT",
            "YAXML_STRUCTURE_CASE",
            "YAXML_STRUCTURE_ENCODING",
            "YAXML_STRUCTURE_WIDTH",
            "YAXML_STRUCTURE_DEPTH",
            "YAXML_STRUCTURE_PAYLOAD_BYTES",
            "YAXML_STRUCTURE_ITERATIONS",
            "YAXML_STRUCTURE_OPERATIONS",
            "YAXML_STRUCTURE_TRAVERSAL_PASSES",
            "YAXML_STRUCTURE_SERIALIZATION_PASSES",
            "YAXML_STRUCTURE_MAX_VALUE_BYTES",
            "YAXML_STRUCTURE_MAX_OWNED_BYTES",
            "YAXML_STRUCTURE_MAX_CREATED_NODES",
        ],
        Some(Path::new(env!("CARGO_MANIFEST_DIR"))),
    )?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(format!(
            "yaxml benchmark failed: exit={:?} stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Row::parse(&output.stdout).map_err(Into::into)
}

fn helper_output(helper: &Path, args: &[&str]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let output = process::run(
        helper,
        args,
        b"",
        process::ProcessLimits {
            timeout: Duration::from_secs(60),
            max_output_bytes: 1024 * 1024,
        },
    )?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(format!(
            "libxml2 benchmark failed: exit={:?} stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(output.stdout)
}

fn c_row(config: &Config, request: &Path) -> Result<Row, Box<dyn std::error::Error>> {
    let request = request
        .to_str()
        .ok_or("matched benchmark request path is not UTF-8")?;
    Row::parse(&helper_output(&config.helper, &[request])?).map_err(Into::into)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = structure_benchmark::parse_args(std::env::args_os().skip(1))?;
    structure_benchmark::verify_version(&helper_output(&config.helper, &["--version"])?)?;
    let directory = RequestDirectory::new()?;
    let request = directory.request();
    // Fixture export and allocator/cache warm-up are excluded from retained samples.
    let preflight = rust_row(&config, Some(&request))?;
    let preflight_c = c_row(&config, &request)?;
    structure_benchmark::validate_config(&config, &preflight)?;
    structure_benchmark::validate_config(&config, &preflight_c)?;
    structure_benchmark::validate_pair(&preflight, &preflight_c)?;
    println!(
        "matched_benchmark=owned_structure schema=1 samples={} case={} encoding={} width={} depth={} payload_bytes={} iterations={} operations={} traversal_passes={} serialization_passes={} libxml_version=21504 status=preflight_complete",
        config.samples,
        config.case,
        config.encoding,
        config.width,
        config.depth,
        config.payload_bytes,
        config.iterations,
        config.operations,
        config.traversal_passes,
        config.serialization_passes
    );
    for sample in 1..=config.samples {
        let yaxml_first = sample % 2 == 1;
        let (rust, c) = if yaxml_first {
            (rust_row(&config, None)?, c_row(&config, &request)?)
        } else {
            let c = c_row(&config, &request)?;
            (rust_row(&config, None)?, c)
        };
        structure_benchmark::validate_config(&config, &rust)?;
        structure_benchmark::validate_config(&config, &c)?;
        let summary = structure_benchmark::validate_pair(&rust, &c)?;
        let order = if yaxml_first {
            "yaxml_first"
        } else {
            "libxml2_first"
        };
        if yaxml_first {
            println!("sample={sample} order={order} {}", rust.text);
            println!("sample={sample} order={order} {}", c.text);
        } else {
            println!("sample={sample} order={order} {}", c.text);
            println!("sample={sample} order={order} {}", rust.text);
        }
        println!(
            "sample={sample} order={order} comparison=exact_final_utf8 text_live_node_delta_per_iteration={} status=matched",
            summary.text_live_node_delta_per_iteration
        );
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
