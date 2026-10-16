//! Paired fresh-process measurements with a canonical logical-observation gate.
#![forbid(unsafe_code)]

#[path = "support/parse_benchmark.rs"]
mod benchmark;
#[path = "support/process.rs"]
mod process;

use std::fs::{self, DirBuilder, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use benchmark::{Config, Row};

struct InputDirectory(PathBuf);

impl InputDirectory {
    fn new() -> io::Result<Self> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        for attempt in 0..128 {
            let path = std::env::temp_dir().join(format!(
                "yaxml-parse-bench-{}-{timestamp}-{attempt}",
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
            "cannot create benchmark input directory",
        ))
    }
}

impl Drop for InputDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn read_input(path: &Path) -> io::Result<Vec<u8>> {
    if !fs::metadata(path)?.is_file() {
        return Err(io::Error::other("input is not a regular file"));
    }
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("opened input is not a regular file"));
    }
    let mut bytes = Vec::new();
    file.take((benchmark::MAX_INPUT + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() > benchmark::MAX_INPUT {
        return Err(io::Error::other("input is empty or exceeds 8 MiB"));
    }
    Ok(bytes)
}

fn helper(
    path: &Path,
    args: &[&str],
    timeout_ms: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let output = process::run(
        path,
        args,
        b"",
        process::ProcessLimits {
            timeout: Duration::from_millis(timeout_ms),
            max_output_bytes: 4096,
        },
    )?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(format!(
            "helper {} failed: exit={:?} stderr={}",
            path.display(),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(output.stdout)
}

fn sample(
    config: &Config,
    input: &Path,
    original: &[u8],
    engine: &str,
) -> Result<Row, Box<dyn std::error::Error>> {
    if read_input(input)? != original {
        return Err("benchmark input changed before helper execution".into());
    }
    let (path, mode) = if engine == "yaxml" {
        (&config.yaxml, config.mode.as_str())
    } else {
        (
            &config.libxml2,
            if config.mode == "owned" {
                "dom"
            } else {
                &config.mode
            },
        )
    };
    let iterations = config.iterations.to_string();
    let chunk = config.chunk.to_string();
    let path_string = input.to_str().ok_or("private input path is not UTF-8")?;
    let row = Row::parse(&helper(
        path,
        &[mode, path_string, &iterations, &chunk],
        config.timeout_ms,
    )?)?;
    row.validate(config, engine, original.len())?;
    if read_input(input)? != original {
        return Err("benchmark input changed during helper execution".into());
    }
    Ok(row)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = benchmark::config(std::env::args_os().skip(1))?;
    let bytes = match (&config.input, &config.fixture) {
        (Some(path), None) => read_input(path)?,
        (None, Some(name)) => benchmark::fixture(name)?,
        _ => return Err("select exactly one input source".into()),
    };
    if bytes
        .len()
        .checked_mul(config.iterations)
        .is_none_or(|work| work > benchmark::MAX_WORK)
    {
        return Err("batch exceeds 512 MiB of source work".into());
    }
    for (path, engine, version) in [
        (&config.yaxml, "yaxml", "0.1.0"),
        (&config.libxml2, "libxml2", "21504"),
    ] {
        let expected = format!("{}\tengine={engine}\tversion={version}\n", benchmark::MAGIC);
        if helper(path, &["--version"], config.timeout_ms)? != expected.as_bytes() {
            return Err("unexpected helper version or protocol".into());
        }
    }
    let directory = InputDirectory::new()?;
    let input = directory.0.join("input.xml");
    fs::write(&input, &bytes)?;
    // This discarded pair warms process-external caches and validates the work;
    // each retained sample still runs in a fresh process with fresh parsers.
    let reference = sample(&config, &input, &bytes, "yaxml")?;
    let c = sample(&config, &input, &bytes, "libxml2")?;
    reference.same_observation(&c)?;
    println!(
        "matched_parse=1 fixture={} mode={} bytes={} input_fnv={:016x} iterations={} chunk={} samples={} status=preflight_complete",
        config.fixture.as_deref().unwrap_or("external"),
        config.mode,
        bytes.len(),
        benchmark::input_hash(&bytes),
        config.iterations,
        config.chunk,
        config.samples
    );
    for index in 1..=config.samples {
        let order = if index % 2 == 1 {
            ["yaxml", "libxml2"]
        } else {
            ["libxml2", "yaxml"]
        };
        let first = sample(&config, &input, &bytes, order[0])?;
        let second = sample(&config, &input, &bytes, order[1])?;
        reference.same_observation(&first)?;
        reference.same_observation(&second)?;
        // Publish only complete, independently observed pairs. A failed later
        // pair leaves the command unsuccessful and earlier evidence intact.
        println!("sample={index}\torder={}_first\t{}", order[0], first.text);
        println!("sample={index}\torder={}_first\t{}", order[0], second.text);
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
