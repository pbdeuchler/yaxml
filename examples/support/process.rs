//! Bounded capture for a caller-selected, trusted oracle executable.
//!
//! Regular files avoid pipe deadlocks and inherited-pipe joins. Output is polled,
//! so a child can write past the ceiling between polls: this is not a disk quota.
//! Cancellation kills and reaps the direct child, not an arbitrary process tree.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug)]
pub struct ProcessLimits {
    pub timeout: Duration,
    /// Combined stdout and stderr bytes. Must be positive.
    pub max_output_bytes: usize,
}

#[derive(Debug)]
pub struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Supplies input through a closed regular file and captures both output streams.
/// Nonzero exit status is an ordinary result. The timeout bounds child execution,
/// not filesystem operations. Temporary files have best-effort RAII cleanup.
pub fn run(
    program: &Path,
    args: &[&str],
    input: &[u8],
    limits: ProcessLimits,
) -> io::Result<ProcessOutput> {
    if limits.timeout.is_zero()
        || Instant::now().checked_add(limits.timeout).is_none()
        || limits.max_output_bytes == 0
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "oracle timeout and output ceiling must be positive and representable",
        ));
    }
    let output_limit = u64::try_from(limits.max_output_bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "output ceiling is too large"))?;
    let directory = TemporaryDirectory::new()?;
    let mut stdin = capture_file(&directory.0.join("stdin"))?;
    stdin.write_all(input)?;
    stdin.seek(SeekFrom::Start(0))?;
    let mut stdout = capture_file(&directory.0.join("stdout"))?;
    let mut stderr = capture_file(&directory.0.join("stderr"))?;
    let start = Instant::now();
    let mut child = RunningChild(Some(
        Command::new(program)
            .args(args)
            .env_remove("XML_CATALOG_FILES")
            .env_remove("SGML_CATALOG_FILES")
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout.try_clone()?))
            .stderr(Stdio::from(stderr.try_clone()?))
            .spawn()?,
    ));
    let status = loop {
        if stdout
            .metadata()?
            .len()
            .checked_add(stderr.metadata()?.len())
            .is_none_or(|length| length > output_limit)
        {
            child.terminate()?;
            return Err(output_exceeded());
        }
        if let Some(status) = child.0.as_mut().expect("running child").try_wait()? {
            child.0 = None;
            break status;
        }
        let elapsed = start.elapsed();
        if elapsed >= limits.timeout {
            child.terminate()?;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "oracle execution timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(5).min(limits.timeout - elapsed));
    };
    // A final bounded read catches output written after the last metadata poll.
    // Descendants retaining these handles cannot force an unbounded pipe read.
    let mut remaining = limits.max_output_bytes;
    let stdout = read_capture(&mut stdout, &mut remaining)?;
    let stderr = read_capture(&mut stderr, &mut remaining)?;
    Ok(ProcessOutput {
        status,
        stdout,
        stderr,
    })
}

fn capture_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
}

fn output_exceeded() -> io::Error {
    io::Error::new(
        io::ErrorKind::FileTooLarge,
        "oracle exceeded the combined output ceiling",
    )
}

fn read_capture(file: &mut File, remaining: &mut usize) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let length = match file.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if length == 0 {
            return Ok(output);
        }
        if length > *remaining {
            return Err(output_exceeded());
        }
        output.try_reserve(length).map_err(io::Error::other)?;
        output.extend_from_slice(&buffer[..length]);
        *remaining -= length;
    }
}

struct RunningChild(Option<Child>);

impl RunningChild {
    fn terminate(&mut self) -> io::Result<()> {
        let Some(child) = &mut self.0 else {
            return Ok(());
        };
        let killed = child.kill();
        let waited = child.wait();
        if waited.is_ok() {
            self.0 = None;
        }
        waited?;
        match killed {
            // The child can exit between the last poll and kill.
            Err(error) if error.kind() != io::ErrorKind::InvalidInput => Err(error),
            _ => Ok(()),
        }
    }
}

impl Drop for RunningChild {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        for _ in 0..128 {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "yaxml-oracle-{}-{timestamp}-{id}",
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
            "could not create a unique oracle temporary directory",
        ))
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
