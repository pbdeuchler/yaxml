//! Run a finite process-based, LLVM coverage-guided mutation campaign.
#![forbid(unsafe_code)]

#[path = "support/fuzz_campaign.rs"]
mod campaign;
#[path = "support/process.rs"]
mod process;

use campaign::{CampaignError, Counts, Limits, MutationRecord, Prng, Summary};
use process::ProcessLimits;
use std::error::Error;
use std::ffi::OsString;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailureKind {
    TargetCrash,
    Timeout,
    OutputLimit,
    TargetDiagnostics,
    Capacity,
    Infrastructure,
}

impl FailureKind {
    fn label(self) -> &'static str {
        match self {
            Self::TargetCrash => "target_crash",
            Self::Timeout => "timeout",
            Self::OutputLimit => "output_limit",
            Self::TargetDiagnostics => "target_diagnostics",
            Self::Capacity => "capacity",
            Self::Infrastructure => "infrastructure",
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse(std::env::args_os().skip(1))?;
    if args.help {
        println!("{}", Args::usage());
        return Ok(());
    }
    Campaign::new(args)?.execute()
}

#[derive(Debug)]
struct Args {
    target: PathBuf,
    profdata: PathBuf,
    cov: PathBuf,
    source_root: PathBuf,
    seeds: PathBuf,
    output: PathBuf,
    revision: String,
    build_flags: String,
    compiler_version: String,
    prng_seed: u64,
    limits: Limits,
    help: bool,
}

impl Args {
    fn usage() -> &'static str {
        "usage: fuzz_campaign --target BIN --llvm-profdata BIN --llvm-cov BIN --source-root DIR --seeds DIR --output DIR --source-revision LABEL --build-flags LABEL --compiler-version VERSION [--prng-seed N] [--max-cases N] [--max-seconds N] [--target-timeout-seconds N] [--max-input-bytes N] [--max-output-bytes N] [--max-profile-bytes N] [--max-seeds N] [--max-seed-bytes N] [--max-corpus-files N] [--max-corpus-bytes N] [--max-pending-bytes N]"
    }

    fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, Box<dyn Error>> {
        let mut result = Self {
            target: PathBuf::new(),
            profdata: PathBuf::new(),
            cov: PathBuf::new(),
            source_root: PathBuf::new(),
            seeds: PathBuf::new(),
            output: PathBuf::new(),
            revision: String::new(),
            build_flags: String::new(),
            compiler_version: String::new(),
            prng_seed: 1,
            limits: Limits::default(),
            help: false,
        };
        let mut values = arguments.into_iter();
        while let Some(argument) = values.next() {
            let flag = argument
                .into_string()
                .map_err(|_| "option name is not UTF-8")?;
            if flag == "--help" || flag == "-h" {
                result.help = true;
                continue;
            }
            let value = values
                .next()
                .ok_or_else(|| format!("{flag} requires a value"))?;
            let value_text = value.to_str().ok_or("argument value is not UTF-8")?;
            match flag.as_ref() {
                "--target" => result.target = PathBuf::from(value),
                "--llvm-profdata" => result.profdata = PathBuf::from(value),
                "--llvm-cov" => result.cov = PathBuf::from(value),
                "--source-root" => result.source_root = PathBuf::from(value),
                "--seeds" => result.seeds = PathBuf::from(value),
                "--output" => result.output = PathBuf::from(value),
                "--source-revision" => result.revision = value_text.to_owned(),
                "--build-flags" => result.build_flags = value_text.to_owned(),
                "--compiler-version" => result.compiler_version = value_text.to_owned(),
                "--prng-seed" => result.prng_seed = value_text.parse()?,
                "--max-cases" => result.limits.cases = value_text.parse()?,
                "--max-seconds" => result.limits.wall_seconds = value_text.parse()?,
                "--target-timeout-seconds" => result.limits.target_seconds = value_text.parse()?,
                "--max-input-bytes" => result.limits.input_bytes = value_text.parse()?,
                "--max-output-bytes" => result.limits.output_bytes = value_text.parse()?,
                "--max-profile-bytes" => result.limits.profile_bytes = value_text.parse()?,
                "--max-seeds" => result.limits.seed_files = value_text.parse()?,
                "--max-seed-bytes" => result.limits.seed_bytes = value_text.parse()?,
                "--max-corpus-files" => result.limits.corpus_files = value_text.parse()?,
                "--max-corpus-bytes" => result.limits.corpus_bytes = value_text.parse()?,
                "--max-pending-bytes" => result.limits.pending_bytes = value_text.parse()?,
                _ => return Err(format!("unknown option: {flag}").into()),
            }
        }
        if result.help {
            return Ok(result);
        }
        result.limits.validate()?;
        for (name, path) in [
            ("--target", &result.target),
            ("--llvm-profdata", &result.profdata),
            ("--llvm-cov", &result.cov),
            ("--source-root", &result.source_root),
            ("--seeds", &result.seeds),
            ("--output", &result.output),
        ] {
            if path.as_os_str().is_empty() {
                return Err(format!("missing required option {name}\n{}", Self::usage()).into());
            }
        }
        if result.revision.is_empty()
            || result.build_flags.is_empty()
            || result.compiler_version.is_empty()
        {
            return Err("--source-revision, --build-flags, and --compiler-version are required labels; they do not prove provenance".into());
        }
        Ok(result)
    }
}

struct Seed {
    source: PathBuf,
    name: String,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileStamp {
    len: u64,
    modified: SystemTime,
}

#[derive(Clone)]
struct Retained {
    name: String,
    path: PathBuf,
    identity: (u64, usize),
    origin: String,
    origin_identity: (u64, usize),
}

struct Campaign {
    args: Args,
    root: PathBuf,
    source_paths: Vec<PathBuf>,
    source_identities: Vec<(PathBuf, (u64, usize))>,
    source_stamps: Vec<(PathBuf, FileStamp)>,
    target_id: (u64, usize),
    target_stamp: FileStamp,
    profdata_id: (u64, usize),
    profdata_stamp: FileStamp,
    cov_id: (u64, usize),
    cov_stamp: FileStamp,
    tool_versions: Vec<String>,
    denominator: Option<Counts>,
    last_summary: Option<Summary>,
    observed_summary: Option<Summary>,
    retained: Vec<Retained>,
    executed: usize,
    seed_cases: usize,
    mutation_cases: usize,
    first_seed_no_coverage: bool,
    novel: usize,
    duplicates: usize,
    termination: String,
    started: Instant,
    rng: Prng,
}

impl Campaign {
    fn new(args: Args) -> Result<Self, Box<dyn Error>> {
        args.limits.validate()?;
        if args.output.exists() {
            return Err("output path already exists; campaigns are not resumable".into());
        }
        let source_root = fs::canonicalize(&args.source_root)?;
        require_utf8_path(&source_root)?;
        let seeds_path = fs::symlink_metadata(&args.seeds)?;
        if seeds_path.file_type().is_symlink() || !seeds_path.is_dir() {
            return Err("seed path must be a real directory".into());
        }
        let seeds_root = fs::canonicalize(&args.seeds)?;
        require_utf8_path(&seeds_root)?;
        let source_paths = production_sources(&source_root)?;
        let source_identities = source_paths
            .iter()
            .map(|path| Ok((path.clone(), file_identity(path)?)))
            .collect::<Result<_, Box<dyn Error>>>()?;
        let source_stamps = source_paths
            .iter()
            .map(|path| Ok((path.clone(), file_stamp(path)?)))
            .collect::<Result<_, Box<dyn Error>>>()?;
        let target = fs::canonicalize(&args.target)?;
        let profdata = fs::canonicalize(&args.profdata)?;
        let cov = fs::canonicalize(&args.cov)?;
        require_utf8_path(&target)?;
        require_utf8_path(&profdata)?;
        require_utf8_path(&cov)?;
        require_utf8_path(&args.output)?;
        let target_id = file_identity(&target)?;
        let profdata_id = file_identity(&profdata)?;
        let cov_id = file_identity(&cov)?;
        let target_stamp = file_stamp(&target)?;
        let profdata_stamp = file_stamp(&profdata)?;
        let cov_stamp = file_stamp(&cov)?;
        let tool_versions = vec![version(&profdata)?, version(&cov)?];
        let expected_llvm = llvm_version(&args.compiler_version, "LLVM version: ")
            .filter(|version| version.starts_with("22."))
            .ok_or("compiler version label must identify an exact LLVM 22.x.y version")?;
        for output in &tool_versions {
            if llvm_version(output, "LLVM version ").as_deref() != Some(&expected_llvm) {
                return Err(format!(
                    "coverage tool LLVM version does not match compiler label {expected_llvm}: {output}"
                )
                .into());
            }
        }
        if args.revision.len() > 4096
            || args.build_flags.len() > 4096
            || args.compiler_version.len() > 4096
            || [
                args.revision.as_str(),
                args.build_flags.as_str(),
                args.compiler_version.as_str(),
            ]
            .iter()
            .any(|value| value.bytes().any(|byte| byte.is_ascii_control()))
        {
            return Err("revision, build flags and compiler-version labels must be single-line values up to 4096 bytes".into());
        }
        let parent = args
            .output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if !parent.is_dir() {
            return Err("output parent directory must already exist".into());
        }
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&args.output)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&args.output, fs::Permissions::from_mode(0o700))?;
        }
        let root = fs::canonicalize(&args.output)?;
        require_utf8_path(&root)?;
        fs::create_dir(root.join("pending"))?;
        fs::create_dir(root.join("profiles"))?;
        fs::create_dir(root.join("corpus"))?;
        fs::create_dir(root.join("failures"))?;
        Ok(Self {
            args: Args {
                target,
                profdata,
                cov,
                source_root,
                seeds: seeds_root,
                ..args
            },
            root,
            source_paths,
            source_identities,
            source_stamps,
            target_id,
            target_stamp,
            profdata_id,
            profdata_stamp,
            cov_id,
            cov_stamp,
            tool_versions,
            denominator: None,
            last_summary: None,
            observed_summary: None,
            retained: Vec::new(),
            executed: 0,
            seed_cases: 0,
            mutation_cases: 0,
            first_seed_no_coverage: false,
            novel: 0,
            duplicates: 0,
            termination: String::new(),
            started: Instant::now(),
            rng: Prng::new(1),
        })
    }

    fn execute(&mut self) -> Result<(), Box<dyn Error>> {
        let result = self.execute_inner();
        if let Err(error) = result {
            let classification = self.root.join("failures").join("classification.txt");
            if classification.exists() {
                return Err(error);
            }
            let message = error.to_string();
            self.termination = "failure:infrastructure".to_owned();
            let message = match self.verify_identities() {
                Ok(()) => message,
                Err(identity_error) => {
                    format!("{message}; end-of-run identity verification failed: {identity_error}")
                }
            };
            fs::write(&classification, format!("infrastructure\n{message}\n"))?;
            self.write_report(Some(("infrastructure", &message)))?;
            return Err(message.into());
        }
        Ok(())
    }

    fn execute_inner(&mut self) -> Result<(), Box<dyn Error>> {
        let seeds = self.read_seeds()?;
        if seeds.len() > self.args.limits.cases {
            return self.fail(
                FailureKind::Infrastructure,
                "case limit is lower than baseline seed count",
                None,
            );
        }
        self.rng = Prng::new(self.args.prng_seed);
        for (index, seed) in seeds.iter().enumerate() {
            let candidate = self.persist_candidate(CandidateSpec {
                index,
                parent: &seed.name,
                parent_identity: None,
                origin: path_text(&seed.source),
                origin_identity: campaign::identity(&seed.bytes),
                bytes: &seed.bytes,
                mutation: None,
            })?;
            let result = self.execute_candidate(index, &candidate, &seed.bytes);
            match result {
                Ok(profile) => {
                    let summary = &profile.summary;
                    let retain = index == 0
                        || summary.counts.has_novelty_over(
                            self.last_summary.as_ref().expect("baseline summary").counts,
                        );
                    self.accept_result(index, candidate, &seed.bytes, profile, retain)?;
                }
                Err((kind, error, output)) => {
                    return self.fail(kind, &error, Some((candidate, output)));
                }
            }
        }
        while self.executed < self.args.limits.cases
            && self.started.elapsed() < Duration::from_secs(self.args.limits.wall_seconds)
        {
            let parent_index =
                usize::try_from(self.rng.next() % self.retained.len() as u64).unwrap_or(0);
            let parent = self.retained[parent_index].clone();
            let parent_bytes = bounded_read(&parent.path, self.args.limits.input_bytes)?;
            if campaign::identity(&parent_bytes) != parent.identity {
                return Err("retained corpus parent identity changed".into());
            }
            let (bytes, mut record) =
                campaign::mutate(&parent_bytes, &mut self.rng, self.args.limits.input_bytes);
            record.parent_index = parent_index;
            let case_index = self.executed;
            let candidate = self.persist_candidate(CandidateSpec {
                index: case_index,
                parent: &parent.name,
                parent_identity: Some(parent.identity),
                origin: &parent.origin,
                origin_identity: parent.origin_identity,
                bytes: &bytes,
                mutation: Some(record),
            })?;
            match self.execute_candidate(case_index, &candidate, &bytes) {
                Ok(profile) => {
                    let summary = &profile.summary;
                    let retain = summary.counts.has_novelty_over(
                        self.last_summary.as_ref().expect("baseline summary").counts,
                    );
                    self.accept_result(case_index, candidate, &bytes, profile, retain)?;
                }
                Err((kind, error, output)) => {
                    return self.fail(kind, &error, Some((candidate, output)));
                }
            }
        }
        self.verify_identities()?;
        self.termination = if self.executed >= self.args.limits.cases {
            "case_limit".to_owned()
        } else {
            "wall_time_limit".to_owned()
        };
        self.write_report(None)?;
        Ok(())
    }

    fn read_seeds(&self) -> Result<Vec<Seed>, Box<dyn Error>> {
        let mut paths = Vec::new();
        for entry in fs::read_dir(&self.args.seeds)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() || !kind.is_file() {
                return Err(format!(
                    "seed entry is not a regular file: {}",
                    entry.path().display()
                )
                .into());
            }
            paths.push(entry.path());
        }
        paths.sort();
        if paths.is_empty() || paths.len() > self.args.limits.seed_files {
            return Err("seed set is empty or exceeds the seed-file limit".into());
        }
        let mut total = 0usize;
        let mut seeds = Vec::new();
        for source in paths {
            let metadata = fs::symlink_metadata(&source)?;
            if !metadata.file_type().is_file()
                || metadata.len() > self.args.limits.input_bytes as u64
            {
                return Err(format!(
                    "seed is not regular or exceeds input limit: {}",
                    source.display()
                )
                .into());
            }
            total = total
                .checked_add(metadata.len() as usize)
                .ok_or("seed byte count overflow")?;
            if total > self.args.limits.seed_bytes {
                return Err("seed bytes exceed configured limit".into());
            }
            let name = source
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("seed filename is not UTF-8")?
                .to_owned();
            seeds.push(Seed {
                source,
                name,
                bytes: Vec::new(),
            });
        }
        // Read after preflight to ensure a changed seed cannot exceed its bound.
        let mut actual_total = 0usize;
        for seed in &mut seeds {
            seed.bytes = bounded_read(&seed.source, self.args.limits.input_bytes)?;
            actual_total = actual_total
                .checked_add(seed.bytes.len())
                .ok_or("seed byte count overflow")?;
            if actual_total > self.args.limits.seed_bytes {
                return Err("actual seed bytes exceed configured limit".into());
            }
        }
        let mut manifest = String::new();
        for seed in &seeds {
            let identity = campaign::identity(&seed.bytes);
            manifest.push_str(&format!(
                "{}\t{:016x}:{}\t{}\n",
                escape_field(&seed.name),
                identity.0,
                identity.1,
                escape_field(path_text(&seed.source.canonicalize()?))
            ));
        }
        if manifest.len() > 512 * 1024 {
            return Err("seed manifest exceeds 512 KiB cap".into());
        }
        fs::write(self.root.join("seeds.tsv"), manifest)?;
        Ok(seeds)
    }

    fn persist_candidate(&self, spec: CandidateSpec<'_>) -> Result<Candidate, Box<dyn Error>> {
        let CandidateSpec {
            index,
            parent,
            parent_identity,
            origin,
            origin_identity,
            bytes,
            mutation,
        } = spec;
        let metadata = format!(
            "case={index}\nparent={}\nparent_identity_fnv64_len={}\norigin={}\norigin_identity_fnv64_len={:016x}:{}\ncandidate_identity_fnv64_len={:016x}:{}\nmutation={mutation:?}\n",
            escape_field(parent),
            parent_identity.map_or_else(
                || "-".to_owned(),
                |(hash, len)| format!("{hash:016x}:{len}")
            ),
            escape_field(origin),
            origin_identity.0,
            origin_identity.1,
            campaign::identity(bytes).0,
            bytes.len()
        );
        if bytes
            .len()
            .checked_add(metadata.len())
            .is_none_or(|size| size > self.args.limits.pending_bytes)
        {
            return Err("candidate plus metadata exceeds pending-byte limit".into());
        }
        let input = self
            .root
            .join("pending")
            .join(format!("candidate-{index:06}.bin"));
        let meta = self
            .root
            .join("pending")
            .join(format!("candidate-{index:06}.meta"));
        write_new(&input, bytes)?;
        if let Err(error) = write_new(&meta, metadata.as_bytes()) {
            let _ = fs::remove_file(&input);
            return Err(error.into());
        }
        Ok(Candidate {
            input,
            meta,
            index,
            parent: parent.to_owned(),
            parent_identity,
            origin: origin.to_owned(),
            origin_identity,
            mutation,
            is_seed: mutation.is_none(),
            preserve_on_drop: false,
        })
    }

    fn execute_candidate(
        &mut self,
        index: usize,
        candidate: &Candidate,
        bytes: &[u8],
    ) -> Result<ValidatedProfile, (FailureKind, String, Option<process::ProcessOutput>)> {
        if self.started.elapsed() >= Duration::from_secs(self.args.limits.wall_seconds) {
            return Err((
                FailureKind::Timeout,
                "campaign wall-time limit exceeded before target start".into(),
                None,
            ));
        }
        if let Err(error) = self.check_metadata_identities() {
            return Err((FailureKind::Infrastructure, error.to_string(), None));
        }
        self.executed += 1;
        if candidate.is_seed {
            self.seed_cases += 1;
        } else {
            self.mutation_cases += 1;
        }
        let raw = self
            .root
            .join("profiles")
            .join(format!("case-{index:06}.profraw"));
        let _ = fs::remove_file(&raw);
        let pending = self.root.join("profiles").join("pending.profdata");
        let _ = fs::remove_file(&pending);
        let mut profile_cleanup = ProfileCleanup {
            pending: pending.clone(),
            armed: true,
        };
        let remaining = Duration::from_secs(self.args.limits.wall_seconds)
            .saturating_sub(self.started.elapsed());
        let timeout = Duration::from_secs(self.args.limits.target_seconds).min(remaining);
        let profile_env = vec![("LLVM_PROFILE_FILE".to_owned(), path_text(&raw).to_owned())];
        let process_output = match process::run_with_environment(
            &self.args.target,
            &[],
            bytes,
            ProcessLimits {
                timeout,
                max_output_bytes: self.args.limits.output_bytes,
            },
            &profile_env,
            &["LLVM_PROFILE_FILE", "LLVM_PROFILE_PREFIX"],
            Some(&self.root),
        ) {
            Ok(output) => output,
            Err(error) => {
                let kind = match error.kind() {
                    io::ErrorKind::TimedOut => FailureKind::Timeout,
                    io::ErrorKind::FileTooLarge => FailureKind::OutputLimit,
                    _ => FailureKind::Infrastructure,
                };
                return Err((kind, error.to_string(), None));
            }
        };
        if !process_output.status.success() {
            return Err((
                FailureKind::TargetCrash,
                format!("target exited with {}", process_output.status),
                Some(process_output),
            ));
        }
        if !process_output.stdout.is_empty() || !process_output.stderr.is_empty() {
            return Err((
                FailureKind::TargetDiagnostics,
                "target wrote to stdout or stderr".into(),
                Some(process_output),
            ));
        }
        let profile_size = fs::metadata(&raw)
            .map_err(|error| {
                (
                    FailureKind::Infrastructure,
                    format!("target produced no profile: {error}"),
                    None,
                )
            })?
            .len();
        if profile_size == 0 || profile_size > self.args.limits.profile_bytes as u64 {
            return Err((
                FailureKind::Infrastructure,
                format!("raw profile size {profile_size} is empty or over limit"),
                None,
            ));
        }
        let cumulative = self.root.join("profiles").join("cumulative.profdata");
        let mut merge_args = vec![
            "merge".to_owned(),
            "-sparse".to_owned(),
            "-o".to_owned(),
            path_text(&pending).to_owned(),
        ];
        if cumulative.exists() {
            merge_args.push(path_text(&cumulative).to_owned());
        }
        merge_args.push(path_text(&raw).to_owned());
        let merge = self.run_tool(
            &self.args.profdata,
            &merge_args,
            self.args.limits.output_bytes,
        )?;
        if !merge.status.success() || !merge.stdout.is_empty() || !merge.stderr.is_empty() {
            return Err((
                FailureKind::Infrastructure,
                "llvm-profdata merge failed or emitted diagnostics".into(),
                Some(merge),
            ));
        }
        let pending_size = fs::metadata(&pending)
            .map_err(|error| (FailureKind::Infrastructure, error.to_string(), None))?
            .len();
        if pending_size == 0 || pending_size > self.args.limits.profile_bytes as u64 {
            return Err((
                FailureKind::Infrastructure,
                format!("cumulative profile size {pending_size} is empty or over limit"),
                None,
            ));
        }
        let mut report_args = vec![
            "report".to_owned(),
            "--color=never".to_owned(),
            format!("--instr-profile={}", path_text(&pending)),
            path_text(&self.args.target).to_owned(),
            "--sources".to_owned(),
        ];
        report_args.extend(
            self.coverage_sources()
                .iter()
                .map(|path| path_text(path).to_owned()),
        );
        let report = self.run_tool(&self.args.cov, &report_args, self.args.limits.output_bytes)?;
        if !report.status.success() || !report.stderr.is_empty() {
            return Err((
                FailureKind::Infrastructure,
                "llvm-cov report failed or emitted diagnostics".into(),
                Some(report),
            ));
        }
        let source_names = self
            .coverage_source_names()
            .map_err(|error| (FailureKind::Infrastructure, error.to_string(), None))?;
        let report_text = match std::str::from_utf8(&report.stdout) {
            Ok(text) => text,
            Err(_) => {
                return Err((
                    FailureKind::Infrastructure,
                    "llvm-cov output is not UTF-8".into(),
                    Some(report),
                ));
            }
        };
        let summary = match campaign::parse_summary(report_text, &source_names) {
            Ok(summary) => summary,
            Err(error) => {
                return Err((FailureKind::Infrastructure, error.to_string(), Some(report)));
            }
        };
        if let Some(old) = self.denominator
            && !summary.counts.denominators_match(old)
        {
            return Err((
                FailureKind::Infrastructure,
                "coverage denominator changed".into(),
                Some(report),
            ));
        }
        let previous = self.last_summary.as_ref().map(|old| old.counts);
        if let Some(old) = previous {
            if !summary.counts.covered_never_decreases(old) {
                self.observed_summary = Some(summary.clone());
                return Err((
                    FailureKind::Infrastructure,
                    "cumulative covered count decreased".into(),
                    Some(report),
                ));
            }
        } else {
            self.denominator = Some(summary.counts);
        }
        if index == 0 {
            self.first_seed_no_coverage = summary.counts.regions_covered == 0
                && summary.counts.functions_covered == 0
                && summary.counts.branches_covered == 0;
        }
        let novelty = previous.is_none_or(|old| summary.counts.has_novelty_over(old));
        if index == 0 || novelty {
            let retained_bytes = self
                .retained
                .iter()
                .map(|entry| entry.identity.1)
                .sum::<usize>();
            if self.retained.len() >= self.args.limits.corpus_files
                || retained_bytes.saturating_add(bytes.len()) > self.args.limits.corpus_bytes
            {
                self.observed_summary = Some(summary.clone());
                return Err((
                    FailureKind::Capacity,
                    "novel candidate exceeds retained corpus limits".into(),
                    Some(report),
                ));
            }
        }
        let journal_len = file_len_or_zero(&self.root.join("cases.tsv"))
            .map_err(|error| (FailureKind::Infrastructure, error.to_string(), None))?;
        let candidate_meta_len = file_len_or_zero(&candidate.meta)
            .map_err(|error| (FailureKind::Infrastructure, error.to_string(), None))?;
        if !journal_fits(journal_len, candidate_meta_len.saturating_add(512)) {
            self.observed_summary = Some(summary.clone());
            return Err((
                FailureKind::Capacity,
                "case journal would exceed 4 MiB cap".into(),
                Some(report),
            ));
        }
        profile_cleanup.disarm();
        Ok(ValidatedProfile {
            summary,
            raw,
            pending,
        })
    }

    fn accept_result(
        &mut self,
        index: usize,
        mut candidate: Candidate,
        bytes: &[u8],
        profile: ValidatedProfile,
        retain: bool,
    ) -> Result<(), Box<dyn Error>> {
        let summary = profile.summary;
        let previous = self.last_summary.as_ref().map(|summary| summary.counts);
        let novel = previous.is_some_and(|old| summary.counts.has_novelty_over(old));
        let identity = campaign::identity(bytes);
        let origin = candidate.origin.clone();
        let origin_identity = candidate.origin_identity;
        if retain {
            let corpus_bytes = self
                .retained
                .iter()
                .map(|item| item.identity.1)
                .sum::<usize>();
            if self.retained.len() >= self.args.limits.corpus_files
                || corpus_bytes.saturating_add(bytes.len()) > self.args.limits.corpus_bytes
            {
                return self.fail(
                    FailureKind::Capacity,
                    "novel candidate exceeds retained corpus limits",
                    Some((candidate, None)),
                );
            }
            let filename = format!("case-{index:06}-{:016x}.bin", campaign::identity(bytes).0);
            let path = self.root.join("corpus").join(filename.clone());
            if let Err(error) = fs::rename(&candidate.input, &path) {
                return self.fail(
                    FailureKind::Infrastructure,
                    &format!("could not stage retained candidate: {error}"),
                    Some((candidate, None)),
                );
            }
            candidate.input = path;
            candidate.preserve_on_drop = true;
        }
        let decision = if retain { "retained" } else { "duplicate" };
        let counts = summary.counts;
        let parent_identity = candidate.parent_identity.map_or_else(
            || "-".to_owned(),
            |(hash, len)| format!("{hash:016x}:{len}"),
        );
        let mutation = candidate
            .mutation
            .map_or_else(|| "baseline".to_owned(), |record| format!("{record:?}"));
        let record = format!(
            "{index}\t{:016x}:{}\tparent={}\tparent_id={}\torigin={}\torigin_id={:016x}:{}\t{decision}\t{}/{}/{}\t{}\n",
            identity.0,
            identity.1,
            escape_field(&candidate.parent),
            parent_identity,
            escape_field(&origin),
            origin_identity.0,
            origin_identity.1,
            counts.regions_covered,
            counts.functions_covered,
            counts.branches_covered,
            mutation
        );
        let journal = self.root.join("cases.tsv");
        let journal_len = match file_len_or_zero(&journal) {
            Ok(length) => length,
            Err(error) => {
                return self.fail(
                    FailureKind::Infrastructure,
                    &format!("could not inspect case journal: {error}"),
                    Some((candidate, None)),
                );
            }
        };
        if !journal_fits(journal_len, record.len() as u64) {
            return self.fail(
                FailureKind::Capacity,
                "case journal exceeds 4 MiB cap",
                Some((candidate, None)),
            );
        }
        let journal_error = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&journal)
            .and_then(|mut cases| cases.write_all(record.as_bytes()));
        if let Err(error) = journal_error {
            let mut rollback = Vec::new();
            if let Err(error) = OpenOptions::new()
                .write(true)
                .open(&journal)
                .and_then(|file| file.set_len(journal_len))
            {
                rollback.push(format!("journal rollback failed: {error}"));
            }
            if retain {
                let pending_input = self
                    .root
                    .join("pending")
                    .join(format!("candidate-{index:06}.bin"));
                if let Err(error) = fs::rename(&candidate.input, &pending_input) {
                    rollback.push(format!("candidate rollback failed: {error}"));
                } else {
                    candidate.input = pending_input;
                    candidate.preserve_on_drop = false;
                }
            }
            let message = if rollback.is_empty() {
                format!("case journal write failed: {error}")
            } else {
                format!(
                    "case journal write failed: {error}; {}",
                    rollback.join("; ")
                )
            };
            return self.fail(
                FailureKind::Infrastructure,
                &message,
                Some((candidate, None)),
            );
        }
        let cumulative = self.root.join("profiles/cumulative.profdata");
        if let Err(error) = fs::rename(&profile.pending, &cumulative) {
            let mut rollback = Vec::new();
            if let Err(error) = OpenOptions::new()
                .write(true)
                .open(&journal)
                .and_then(|file| file.set_len(journal_len))
            {
                rollback.push(format!("journal rollback failed: {error}"));
            }
            if retain {
                let pending_input = self
                    .root
                    .join("pending")
                    .join(format!("candidate-{index:06}.bin"));
                if let Err(error) = fs::rename(&candidate.input, &pending_input) {
                    rollback.push(format!("candidate rollback failed: {error}"));
                } else {
                    candidate.input = pending_input;
                    candidate.preserve_on_drop = false;
                }
            }
            let message = if rollback.is_empty() {
                format!("cumulative profile commit failed: {error}")
            } else {
                format!(
                    "cumulative profile commit failed: {error}; {}",
                    rollback.join("; ")
                )
            };
            return self.fail(
                FailureKind::Infrastructure,
                &message,
                Some((candidate, None)),
            );
        }

        // Renaming pending cumulative to its final path is the commit point.
        self.last_summary = Some(summary.clone());
        if retain {
            let filename = candidate
                .input
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("retained candidate filename is not UTF-8")?
                .to_owned();
            self.retained.push(Retained {
                name: filename,
                path: candidate.input.clone(),
                identity,
                origin: origin.clone(),
                origin_identity,
            });
            self.novel += usize::from(novel);
        } else {
            self.duplicates += 1;
        }
        let final_raw = self.root.join("profiles/final.profraw");
        if let Err(error) = fs::rename(&profile.raw, &final_raw) {
            return self.post_commit_failure(
                &mut candidate,
                &profile.raw,
                &format!("raw profile publication failed after cumulative commit: {error}"),
            );
        }
        if let Err(error) = self.write_report(None) {
            return self.post_commit_failure(
                &mut candidate,
                &profile.raw,
                &format!("campaign report write failed after commit: {error}"),
            );
        }
        if retain && let Err(error) = fs::remove_file(&candidate.meta) {
            return self.post_commit_failure(
                &mut candidate,
                &profile.raw,
                &format!("candidate metadata cleanup failed after commit: {error}"),
            );
        }
        Ok(())
    }

    fn fail<T>(
        &mut self,
        kind: FailureKind,
        message: &str,
        evidence: Option<(Candidate, Option<process::ProcessOutput>)>,
    ) -> Result<T, Box<dyn Error>> {
        let kind = kind.label();
        let identity_message = self
            .verify_identities()
            .err()
            .map(|error| format!("{message}; end-of-run identity verification failed: {error}"));
        let mut failure_message = identity_message.unwrap_or_else(|| message.to_owned());
        self.termination = format!("failure:{kind}");
        let mut failed_profile = None;
        let mut preservation_errors = Vec::new();
        if let Some((mut candidate, output)) = evidence {
            failed_profile = Some(
                self.root
                    .join("profiles")
                    .join(format!("case-{:06}.profraw", candidate.index)),
            );
            let failure = self.root.join("failures").join("first-failure.input");
            if candidate.input.exists() {
                candidate.preserve_on_drop = true;
                if let Err(error) = fs::rename(&candidate.input, &failure) {
                    preservation_errors.push(format!("could not preserve candidate: {error}"));
                }
            }
            let metadata = self.root.join("failures").join("first-failure.meta");
            if candidate.meta.exists()
                && let Err(error) = fs::rename(&candidate.meta, &metadata)
            {
                preservation_errors.push(format!("could not preserve candidate metadata: {error}"));
            }
            if let Some(output) = output {
                if let Err(error) = write_new(
                    &self.root.join("failures").join("stdout.log"),
                    &output.stdout,
                ) {
                    preservation_errors.push(format!("could not preserve stdout: {error}"));
                }
                if let Err(error) = write_new(
                    &self.root.join("failures").join("stderr.log"),
                    &output.stderr,
                ) {
                    preservation_errors.push(format!("could not preserve stderr: {error}"));
                }
            }
        }
        if let Err(error) = fs::remove_file(self.root.join("profiles").join("pending.profdata"))
            && error.kind() != io::ErrorKind::NotFound
        {
            preservation_errors.push(format!("could not remove pending profile: {error}"));
        }
        if let Some(path) = failed_profile
            && path.exists()
        {
            let failure_raw = self.root.join("failures/first-failure.profraw");
            match fs::metadata(&path) {
                Ok(metadata) if metadata.len() <= self.args.limits.profile_bytes as u64 => {
                    if let Err(error) = fs::rename(&path, failure_raw) {
                        preservation_errors
                            .push(format!("could not preserve failed raw profile: {error}"));
                    }
                }
                Ok(metadata) => {
                    preservation_errors.push(format!(
                        "failed raw profile omitted at {} bytes (profile limit {} bytes)",
                        metadata.len(),
                        self.args.limits.profile_bytes
                    ));
                    if let Err(error) = fs::remove_file(&path) {
                        preservation_errors
                            .push(format!("could not remove over-limit raw profile: {error}"));
                    }
                }
                Err(error) => preservation_errors
                    .push(format!("could not inspect failed raw profile: {error}")),
            }
        }
        if !preservation_errors.is_empty() {
            failure_message.push_str("; evidence preservation: ");
            failure_message.push_str(&preservation_errors.join("; "));
        }
        fs::write(
            self.root.join("failures").join("classification.txt"),
            format!("{kind}\n{failure_message}\n"),
        )?;
        self.write_report(Some((kind, &failure_message)))?;
        if !preservation_errors.is_empty() {
            return Err(format!("campaign {kind}: {failure_message}").into());
        }
        Err(format!(
            "campaign {kind}: {failure_message}; evidence in {}",
            self.root.display()
        )
        .into())
    }

    fn post_commit_failure<T>(
        &mut self,
        candidate: &mut Candidate,
        raw: &Path,
        message: &str,
    ) -> Result<T, Box<dyn Error>> {
        // Candidate::drop owns pending input and metadata. Preserve that
        // ownership before attempting copies, so a failed evidence write can
        // never turn into evidence deletion when this function returns.
        candidate.preserve_on_drop = true;
        self.termination = "post_commit_failure".to_owned();
        let mut message = match self.verify_identities() {
            Ok(()) => message.to_owned(),
            Err(error) => format!("{message}; end-of-run identity verification failed: {error}"),
        };
        let failure_dir = self.root.join("failures");
        let mut errors = Vec::new();
        let mut copies_succeeded = true;
        for (source, name) in [
            (&candidate.input, "first-failure.input"),
            (&candidate.meta, "first-failure.meta"),
        ] {
            if source.exists()
                && let Err(error) = fs::copy(source, failure_dir.join(name))
            {
                copies_succeeded = false;
                errors.push(format!("could not preserve {name}: {error}"));
            }
        }
        if copies_succeeded {
            let corpus = self.root.join("corpus");
            if !candidate.input.starts_with(&corpus)
                && candidate.input.exists()
                && let Err(error) = fs::remove_file(&candidate.input)
            {
                errors.push(format!("could not remove copied candidate: {error}"));
            }
            if candidate.meta.exists()
                && let Err(error) = fs::remove_file(&candidate.meta)
            {
                errors.push(format!(
                    "could not remove copied candidate metadata: {error}"
                ));
            }
        }
        if raw.exists()
            && let Err(error) = fs::rename(raw, failure_dir.join("first-failure.profraw"))
        {
            errors.push(format!("could not preserve failed raw profile: {error}"));
        }
        if !errors.is_empty() {
            message.push_str("; evidence preservation: ");
            message.push_str(&errors.join("; "));
        }
        fs::write(
            failure_dir.join("classification.txt"),
            format!("infrastructure\npost-commit: {message}\n"),
        )?;
        self.write_report(Some(("infrastructure", &format!("post-commit: {message}"))))?;
        if errors.is_empty() {
            Err(format!(
                "post-commit infrastructure failure: {message}; accepted case and cumulative profile remain committed"
            )
            .into())
        } else {
            Err(format!(
                "post-commit infrastructure failure: {message}; accepted case and cumulative profile remain committed; {}",
                errors.join("; ")
            )
            .into())
        }
    }

    fn run_tool(
        &self,
        program: &Path,
        args: &[String],
        output_limit: usize,
    ) -> Result<process::ProcessOutput, (FailureKind, String, Option<process::ProcessOutput>)> {
        let args: Vec<_> = args.iter().map(String::as_str).collect();
        let remaining = Duration::from_secs(self.args.limits.wall_seconds)
            .saturating_sub(self.started.elapsed());
        let timeout = Duration::from_secs(30).min(remaining);
        process::run_with_environment(
            program,
            &args,
            b"",
            ProcessLimits {
                timeout,
                max_output_bytes: output_limit,
            },
            &[],
            &["LLVM_PROFILE_FILE", "LLVM_PROFILE_PREFIX"],
            Some(&self.root),
        )
        .map_err(|error| (FailureKind::Infrastructure, error.to_string(), None))
    }

    fn coverage_sources(&self) -> Vec<PathBuf> {
        self.source_paths.clone()
    }

    fn coverage_source_names(&self) -> Result<Vec<String>, CampaignError> {
        let sources: Vec<_> = self
            .source_paths
            .iter()
            .filter(|path| path.file_name().is_some_and(|name| name != "lib.rs"))
            .cloned()
            .collect();
        campaign::source_names(&sources)
    }

    fn check_metadata_identities(&self) -> Result<(), Box<dyn Error>> {
        if file_stamp(&self.args.target)? != self.target_stamp
            || file_stamp(&self.args.profdata)? != self.profdata_stamp
            || file_stamp(&self.args.cov)? != self.cov_stamp
        {
            return Err("target or coverage tool metadata changed".into());
        }
        for (path, expected) in &self.source_stamps {
            if file_stamp(path)? != *expected {
                return Err(format!("source metadata changed: {}", path.display()).into());
            }
        }
        Ok(())
    }

    fn verify_identities(&self) -> Result<(), Box<dyn Error>> {
        if file_identity(&self.args.target)? != self.target_id
            || file_identity(&self.args.profdata)? != self.profdata_id
            || file_identity(&self.args.cov)? != self.cov_id
        {
            return Err("target or LLVM tool identity changed during campaign".into());
        }
        for (path, expected) in &self.source_identities {
            if file_identity(path)? != *expected {
                return Err(format!("source identity changed: {}", path.display()).into());
            }
        }
        Ok(())
    }

    fn write_report(&self, failure: Option<(&str, &str)>) -> Result<(), Box<dyn Error>> {
        let mut text = format!(
            "revision={}\nsource_root={}\nbuild_flags={}\ncompiler_version_label={}\ntarget_args=stdin, no arguments\nmerge_options=-sparse\nreport_options=--color=never --instr-profile --sources all 14 src/*.rs files; report expects 13 rows because lib.rs has no executable mapping\nprofile_format=LLVM raw plus indexed profdata\nidentity_hash=FNV-1a 64-bit plus length; non-cryptographic accidental-change check only\nprofile_storage_max_files=4 (previous successful raw, current raw, cumulative profile, pending merge)\nprofile_storage_max_bytes={}\nprng_seed={}\nlimits={:?}\ntermination={}\ntarget={}\ntarget_identity_fnv64_len={:016x}:{}\nllvm_profdata={}\nllvm_profdata_identity_fnv64_len={:016x}:{}\nllvm_cov={}\nllvm_cov_identity_fnv64_len={:016x}:{}\ntool_versions={:?}\ncases_attempted={}\nseed_cases_attempted={}\nmutation_cases_attempted={}\nfirst_seed_no_coverage={}\nretained={}\nduplicates={}\nnovel={}\nelapsed_seconds={}\n",
            self.args.revision,
            escape_field(path_text(&self.args.source_root)),
            escape_field(&self.args.build_flags),
            escape_field(&self.args.compiler_version),
            self.args.limits.profile_bytes.saturating_mul(4),
            self.args.prng_seed,
            self.args.limits,
            if self.termination.is_empty() {
                "running"
            } else {
                &self.termination
            },
            escape_field(path_text(&self.args.target)),
            self.target_id.0,
            self.target_id.1,
            escape_field(path_text(&self.args.profdata)),
            self.profdata_id.0,
            self.profdata_id.1,
            escape_field(path_text(&self.args.cov)),
            self.cov_id.0,
            self.cov_id.1,
            self.tool_versions,
            self.executed,
            self.seed_cases,
            self.mutation_cases,
            self.first_seed_no_coverage,
            self.retained.len(),
            self.duplicates,
            self.novel,
            self.started.elapsed().as_secs()
        );
        for (path, identity) in &self.source_identities {
            text.push_str(&format!(
                "source={} fnv64_len={:016x}:{}\n",
                escape_field(path_text(path)),
                identity.0,
                identity.1
            ));
        }
        if let Some(summary) = &self.last_summary {
            let counts = summary.counts;
            text.push_str(&format!("regions={}/{}\nfunctions={}/{}\nlines={}/{}\nbranches={}/{}\nregion_gaps={}\nfunction_gaps={}\nline_gaps={}\nbranch_gaps={}\n", counts.regions_covered, counts.regions_total, counts.functions_covered, counts.functions_total, counts.lines_covered, counts.lines_total, counts.branches_covered, counts.branches_total, counts.regions_total-counts.regions_covered, counts.functions_total-counts.functions_covered, counts.lines_total-counts.lines_covered, counts.branches_total-counts.branches_covered));
        }
        if let Some(summary) = &self.observed_summary {
            let counts = summary.counts;
            text.push_str(&format!("observed_failure_regions={}/{}\nobserved_failure_functions={}/{}\nobserved_failure_lines={}/{}\nobserved_failure_branches={}/{}\n", counts.regions_covered, counts.regions_total, counts.functions_covered, counts.functions_total, counts.lines_covered, counts.lines_total, counts.branches_covered, counts.branches_total));
        }
        if let Some((kind, message)) = failure {
            text.push_str(&format!(
                "failure_kind={}\nfailure={}\n",
                escape_field(kind),
                escape_field(message)
            ));
        }
        fs::write(self.root.join("campaign-report.txt"), text)?;
        Ok(())
    }
}

struct Candidate {
    input: PathBuf,
    meta: PathBuf,
    index: usize,
    parent: String,
    parent_identity: Option<(u64, usize)>,
    origin: String,
    origin_identity: (u64, usize),
    mutation: Option<MutationRecord>,
    is_seed: bool,
    preserve_on_drop: bool,
}

struct CandidateSpec<'a> {
    index: usize,
    parent: &'a str,
    parent_identity: Option<(u64, usize)>,
    origin: &'a str,
    origin_identity: (u64, usize),
    bytes: &'a [u8],
    mutation: Option<MutationRecord>,
}

struct ValidatedProfile {
    summary: Summary,
    raw: PathBuf,
    pending: PathBuf,
}

struct ProfileCleanup {
    pending: PathBuf,
    armed: bool,
}

impl ProfileCleanup {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ProfileCleanup {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.pending);
        }
    }
}

const MAX_JOURNAL_BYTES: u64 = 4 * 1024 * 1024;

fn journal_fits(current: u64, additional: u64) -> bool {
    current.saturating_add(additional) <= MAX_JOURNAL_BYTES
}

fn escape_field(value: &str) -> String {
    let mut escaped = String::new();
    for byte in value.bytes() {
        if !byte.is_ascii_graphic() || byte == b'%' {
            escaped.push_str(&format!("%{byte:02X}"));
        } else {
            escaped.push(byte as char);
        }
    }
    escaped
}

impl Drop for Candidate {
    fn drop(&mut self) {
        if !self.preserve_on_drop {
            let _ = fs::remove_file(&self.input);
            let _ = fs::remove_file(&self.meta);
        }
    }
}

fn production_sources(source_root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(source_root.join("src"))? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err("symlink found in production source directory".into());
        }
        if kind.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "rs")
        {
            let path = fs::canonicalize(entry.path())?;
            require_utf8_path(&path)?;
            paths.push(path);
        }
    }
    paths.sort();
    if paths.len() != 14
        || !paths
            .iter()
            .any(|path| path.file_name().is_some_and(|name| name == "lib.rs"))
    {
        return Err("production source inventory differs from the reviewed 14-file mapping".into());
    }
    Ok(paths)
}

fn version(program: &Path) -> Result<String, Box<dyn Error>> {
    let output = process::run(
        program,
        &["--version"],
        b"",
        ProcessLimits {
            timeout: Duration::from_secs(5),
            max_output_bytes: 16 * 1024,
        },
    )?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err("coverage tool --version failed or emitted diagnostics".into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn llvm_version(output: &str, marker: &str) -> Option<String> {
    let suffix = output.split_once(marker)?.1;
    let version: String = suffix
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    (version.split('.').count() == 3
        && version.split('.').all(|component| {
            !component.is_empty() && component.bytes().all(|b| b.is_ascii_digit())
        }))
    .then_some(version)
}

fn file_identity(path: &Path) -> io::Result<(u64, usize)> {
    let mut file = File::open(path)?;
    let mut hash = 0xcbf29ce484222325u64;
    let mut length = 0usize;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        length = length
            .checked_add(read)
            .ok_or_else(|| io::Error::other("file length overflow"))?;
        for byte in &buffer[..read] {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    Ok((hash, length))
}

fn file_stamp(path: &Path) -> io::Result<FileStamp> {
    let metadata = fs::metadata(path)?;
    Ok(FileStamp {
        len: metadata.len(),
        modified: metadata.modified()?,
    })
}

fn file_len_or_zero(path: &Path) -> io::Result<u64> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

fn require_utf8_path(path: &Path) -> Result<(), Box<dyn Error>> {
    let text = path
        .to_str()
        .ok_or("paths must be valid UTF-8; lossy path conversion is refused")?;
    if text.bytes().any(|byte| byte.is_ascii_control()) {
        return Err("paths containing control characters are refused".into());
    }
    Ok(())
}

fn path_text(path: &Path) -> &str {
    path.to_str()
        .expect("all subprocess paths were validated as UTF-8")
}

fn bounded_read(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "file exceeds configured byte limit",
        ));
    }
    Ok(bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn required_args() -> Vec<OsString> {
        [
            "--target",
            "/tmp/target",
            "--llvm-profdata",
            "/tmp/profdata",
            "--llvm-cov",
            "/tmp/cov",
            "--source-root",
            "/tmp/source",
            "--seeds",
            "/tmp/seeds",
            "--output",
            "/tmp/output",
            "--source-revision",
            "abc123",
            "--build-flags",
            "-C instrument-coverage -C lto=no",
            "--compiler-version",
            "rustc nightly-2026-06-06 LLVM version: 22.1.6",
        ]
        .into_iter()
        .map(OsString::from)
        .collect()
    }

    #[test]
    fn arguments_accept_defaults_and_reject_limits_and_missing_fields() {
        let parsed = Args::parse(required_args()).unwrap();
        assert_eq!(parsed.limits, Limits::default());
        let mut invalid = required_args();
        invalid.extend([OsString::from("--max-cases"), OsString::from("10001")]);
        assert!(Args::parse(invalid).is_err());
        assert!(Args::parse([OsString::from("--target")]).is_err());
        assert!(Args::parse([OsString::from("--help")]).unwrap().help);
    }

    #[test]
    fn failure_classes_and_manifest_escaping_are_stable() {
        assert_eq!(FailureKind::Timeout.label(), "timeout");
        assert_eq!(FailureKind::TargetCrash.label(), "target_crash");
        assert_eq!(FailureKind::Infrastructure.label(), "infrastructure");
        assert_eq!(escape_field("seed\tname\n%"), "seed%09name%0A%25");
    }

    #[test]
    fn journal_budget_accepts_exact_bound_and_rejects_overflow() {
        assert!(journal_fits(MAX_JOURNAL_BYTES - 1, 1));
        assert!(journal_fits(MAX_JOURNAL_BYTES, 0));
        assert!(!journal_fits(MAX_JOURNAL_BYTES, 1));
        assert!(!journal_fits(u64::MAX, 1));
    }

    #[test]
    fn llvm_versions_are_exactly_parsed_for_matching() {
        let tool = "LLVM (http://llvm.org/): LLVM version 22.1.6-rust-1.98";
        assert_eq!(
            llvm_version(tool, "LLVM version ").as_deref(),
            Some("22.1.6")
        );
        assert_eq!(
            llvm_version("rustc LLVM version: 22.1.6", "LLVM version: ").as_deref(),
            Some("22.1.6")
        );
        assert_eq!(
            llvm_version("LLVM version 22.2.0", "LLVM version ").as_deref(),
            Some("22.2.0")
        );
        assert_ne!(
            llvm_version("LLVM version 22.2.0", "LLVM version "),
            llvm_version("rustc LLVM version: 22.1.6", "LLVM version: ")
        );
        assert_eq!(llvm_version("LLVM 22", "LLVM version "), None);
    }

    #[test]
    fn candidate_drop_cleans_pending_files_unless_evidence_must_be_kept() {
        let root = std::env::temp_dir().join(format!(
            "yaxml-candidate-drop-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let input = root.join("candidate");
        let meta = root.join("candidate.meta");
        fs::write(&input, b"input").unwrap();
        fs::write(&meta, b"metadata").unwrap();
        drop(Candidate {
            input: input.clone(),
            meta: meta.clone(),
            index: 0,
            parent: "seed".to_owned(),
            parent_identity: None,
            origin: "seed.xml".to_owned(),
            origin_identity: campaign::identity(b"input"),
            mutation: None,
            is_seed: true,
            preserve_on_drop: false,
        });
        assert!(!input.exists());
        assert!(!meta.exists());

        fs::write(&input, b"failure input").unwrap();
        fs::write(&meta, b"failure metadata").unwrap();
        drop(Candidate {
            input: input.clone(),
            meta: meta.clone(),
            index: 1,
            parent: "parent".to_owned(),
            parent_identity: Some(campaign::identity(b"parent")),
            origin: "seed.xml".to_owned(),
            origin_identity: campaign::identity(b"input"),
            mutation: None,
            is_seed: false,
            preserve_on_drop: true,
        });
        assert!(input.exists());
        assert!(meta.exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn fixture_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "yaxml-fuzz-cli-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("source/src")).unwrap();
        fs::create_dir(root.join("seeds")).unwrap();
        for index in 0..13 {
            fs::write(
                root.join(format!("source/src/m{index:02}.rs")),
                b"// fixture source\n",
            )
            .unwrap();
        }
        fs::write(
            root.join("source/src/lib.rs"),
            b"// no executable mapping\n",
        )
        .unwrap();
        fs::write(root.join("seeds/seed % name.xml"), b"x").unwrap();
        root
    }

    fn compile_fake_programs(root: &Path) -> (PathBuf, PathBuf) {
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
        let helper_source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_fuzz_campaign.rs");
        let helper = root.join("fake-helper");
        let compilation = std::process::Command::new(&rustc)
            .arg("--edition=2024")
            .arg(helper_source)
            .arg("-o")
            .arg(&helper)
            .output()
            .unwrap();
        assert!(
            compilation.status.success(),
            "fake helper compile failed: {}",
            String::from_utf8_lossy(&compilation.stderr)
        );

        let campaign_source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/fuzz_campaign.rs");
        let campaign = root.join("fuzz_campaign");
        let compilation = std::process::Command::new(&rustc)
            .arg("--edition=2024")
            .arg(campaign_source)
            .arg("-o")
            .arg(&campaign)
            .output()
            .unwrap();
        assert!(
            compilation.status.success(),
            "campaign CLI compile failed: {}",
            String::from_utf8_lossy(&compilation.stderr)
        );
        (helper, campaign)
    }

    fn install_mode(helper: &Path, root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        fs::copy(helper, &path).unwrap();
        path
    }

    fn cli_command(
        campaign: &Path,
        target: &Path,
        profdata: &Path,
        cov: &Path,
        root: &Path,
        output_name: &str,
        cases: usize,
        extra: &[&str],
    ) -> std::process::Command {
        let mut command = std::process::Command::new(campaign);
        command
            .arg("--target")
            .arg(target)
            .arg("--llvm-profdata")
            .arg(profdata)
            .arg("--llvm-cov")
            .arg(cov)
            .arg("--source-root")
            .arg(root.join("source"))
            .arg("--seeds")
            .arg(root.join("seeds"))
            .arg("--output")
            .arg(root.join(output_name))
            .arg("--source-revision")
            .arg("fixture-revision")
            .arg("--build-flags")
            .arg("coverage,branch,no-lto")
            .arg("--compiler-version")
            .arg("rustc fixture LLVM version: 22.1.6")
            .arg("--prng-seed")
            .arg("12345")
            .arg("--max-cases")
            .arg(cases.to_string())
            .arg("--max-seconds")
            .arg("60")
            .arg("--target-timeout-seconds")
            .arg("1")
            .args(extra);
        command
    }

    fn run_cli(
        campaign: &Path,
        target: &Path,
        profdata: &Path,
        cov: &Path,
        root: &Path,
        output_name: &str,
        cases: usize,
        extra: &[&str],
    ) -> std::process::Output {
        cli_command(
            campaign,
            target,
            profdata,
            cov,
            root,
            output_name,
            cases,
            extra,
        )
        .output()
        .unwrap()
    }

    #[test]
    fn actual_cli_commits_cumulative_feedback_and_preserves_first_failure() {
        let root = fixture_root();
        let (helper, campaign) = compile_fake_programs(&root);
        let target = install_mode(&helper, &root, "target_ok");
        let profdata = install_mode(&helper, &root, "profdata_ok");
        let cov = install_mode(&helper, &root, "cov_ok");

        let success = run_cli(
            &campaign,
            &target,
            &profdata,
            &cov,
            &root,
            "success",
            100,
            &[],
        );
        assert!(
            success.status.success(),
            "campaign failed: {}",
            String::from_utf8_lossy(&success.stderr)
        );
        let report = fs::read_to_string(root.join("success/campaign-report.txt")).unwrap();
        assert!(report.contains("retained=2\n"), "{report}");
        assert!(report.contains("cases_attempted=100\n"), "{report}");
        assert!(report.contains("seed_cases_attempted=1\n"), "{report}");
        assert!(report.contains("mutation_cases_attempted=99\n"), "{report}");
        assert!(report.contains("first_seed_no_coverage=true\n"), "{report}");
        assert!(report.contains("identity_hash=FNV-1a 64-bit plus length"));
        assert!(report.contains("duplicates="), "{report}");
        let journal = fs::read_to_string(root.join("success/cases.tsv")).unwrap();
        assert!(journal.contains("\tretained\t"), "{journal}");
        assert!(journal.contains("\tduplicate\t"), "{journal}");
        assert!(journal.contains("parent_id=-"), "{journal}");
        assert!(
            journal.contains("parent_id=af63f54c86021707:1"),
            "{journal}"
        );
        let expected_origin = escape_field(path_text(
            &fs::canonicalize(root.join("seeds/seed % name.xml")).unwrap(),
        ));
        assert!(
            journal
                .lines()
                .all(|line| line.contains(&format!("origin={expected_origin}\t")))
        );
        assert!(root.join("success/profiles/cumulative.profdata").exists());
        assert!(root.join("success/profiles/final.profraw").exists());
        assert!(
            fs::read_dir(root.join("success/profiles"))
                .unwrap()
                .all(|entry| {
                    entry
                        .unwrap()
                        .file_name()
                        .to_str()
                        .is_some_and(|name| !name.starts_with("case-"))
                })
        );

        let mut missing_lib = std::process::Command::new(&cov);
        missing_lib
            .args([
                "report",
                "--color=never",
                "--instr-profile=/unused",
                "/unused-target",
                "--sources",
            ])
            .args((0..13).map(|index| root.join(format!("source/src/m{index:02}.rs"))));
        let missing_lib = missing_lib.output().unwrap();
        assert!(!missing_lib.status.success());
        assert!(
            String::from_utf8_lossy(&missing_lib.stderr)
                .contains("expected all 14 absolute source paths")
        );

        let paused_cov = install_mode(&helper, &root, "cov_pause");
        let paused_output = root.join("four-profile-bound");
        let mut child = cli_command(
            &campaign,
            &target,
            &profdata,
            &paused_cov,
            &root,
            "four-profile-bound",
            2,
            &[],
        )
        .spawn()
        .unwrap();
        let profiles = paused_output.join("profiles");
        let mut observed_four = false;
        for _ in 0..300 {
            if profiles.join("final.profraw").exists()
                && profiles.join("cumulative.profdata").exists()
                && profiles.join("pending.profdata").exists()
                && profiles.join("case-000001.profraw").exists()
            {
                observed_four = true;
                break;
            }
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            observed_four,
            "four bounded profile files were not observed"
        );
        let paused_output = child.wait_with_output().unwrap();
        assert!(
            paused_output.status.success(),
            "four-profile campaign failed: {}",
            String::from_utf8_lossy(&paused_output.stderr)
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(root.join("success"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }

        let repeated = run_cli(
            &campaign,
            &target,
            &profdata,
            &cov,
            &root,
            "success",
            1,
            &[],
        );
        assert!(!repeated.status.success());

        let version_mismatch = install_mode(&helper, &root, "cov_version_mismatch");
        let mismatch = run_cli(
            &campaign,
            &target,
            &profdata,
            &version_mismatch,
            &root,
            "version-mismatch",
            1,
            &[],
        );
        assert!(!mismatch.status.success());
        assert!(!root.join("version-mismatch").exists());

        let failure_modes = [
            ("target_crash", "target_crash"),
            ("target_stderr", "target_diagnostics"),
            ("target_timeout", "timeout"),
            ("target_output", "output_limit"),
            ("target_missing", "infrastructure"),
            ("target_corrupt", "infrastructure"),
        ];
        for (mode, expected) in failure_modes {
            let failing_target = install_mode(&helper, &root, mode);
            let output_name = format!("failure-{mode}");
            let output = run_cli(
                &campaign,
                &failing_target,
                &profdata,
                &cov,
                &root,
                &output_name,
                1,
                if mode == "target_output" {
                    &["--max-output-bytes", "64"]
                } else {
                    &[]
                },
            );
            assert!(!output.status.success(), "{mode}");
            let failure_root = root.join(&output_name).join("failures");
            let classification =
                fs::read_to_string(failure_root.join("classification.txt")).unwrap();
            assert!(classification.starts_with(expected), "{classification}");
            assert_eq!(
                fs::read(failure_root.join("first-failure.input")).unwrap(),
                b"x"
            );
            let failed_metadata =
                fs::read_to_string(failure_root.join("first-failure.meta")).unwrap();
            assert!(failed_metadata.contains("parent_identity_fnv64_len=-"));
            assert!(failed_metadata.contains(&format!("origin={expected_origin}\n")));
            let has_failed_profile = matches!(mode, "target_corrupt" | "target_output");
            assert_eq!(
                failure_root.join("first-failure.profraw").exists(),
                has_failed_profile,
                "{mode}"
            );
            let profiles = root.join(&output_name).join("profiles");
            assert!(!profiles.join("pending.profdata").exists());
            assert!(!profiles.join("cumulative.profdata").exists());
            assert!(fs::read_dir(profiles).unwrap().all(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_str()
                    .is_some_and(|name| !name.starts_with("case-"))
            }));
        }

        let large_profile_target = install_mode(&helper, &root, "target_large_profile");
        let oversized = run_cli(
            &campaign,
            &large_profile_target,
            &profdata,
            &cov,
            &root,
            "oversized-profile",
            1,
            &["--max-profile-bytes", "64"],
        );
        assert!(!oversized.status.success());
        let oversized_root = root.join("oversized-profile");
        let oversized_classification =
            fs::read_to_string(oversized_root.join("failures/classification.txt")).unwrap();
        assert!(oversized_classification.contains("omitted at 1048576 bytes"));
        assert!(oversized_root.join("failures/first-failure.input").exists());
        assert!(
            !oversized_root
                .join("failures/first-failure.profraw")
                .exists()
        );

        for (mode, name) in [
            ("profdata_fail", "merge-failure"),
            ("profdata_diag", "merge-diagnostic"),
        ] {
            let failing_profdata = install_mode(&helper, &root, mode);
            let output = run_cli(
                &campaign,
                &target,
                &failing_profdata,
                &cov,
                &root,
                name,
                1,
                &[],
            );
            assert!(!output.status.success(), "{mode}");
            let failure_root = root.join(name);
            assert!(failure_root.join("failures/first-failure.input").exists());
            assert!(failure_root.join("failures/first-failure.profraw").exists());
            assert!(!failure_root.join("profiles/pending.profdata").exists());
            assert!(!failure_root.join("profiles/cumulative.profdata").exists());
        }

        for (mode, name) in [
            ("cov_malformed", "malformed-report"),
            ("cov_drift", "denominator-drift"),
        ] {
            let failing_cov = install_mode(&helper, &root, mode);
            let output = run_cli(
                &campaign,
                &target,
                &profdata,
                &failing_cov,
                &root,
                name,
                if mode == "cov_drift" { 100 } else { 1 },
                &[],
            );
            assert!(!output.status.success(), "{mode}");
            let failure_root = root.join(name);
            assert!(failure_root.join("failures/first-failure.input").exists());
            assert!(failure_root.join("failures/first-failure.profraw").exists());
            assert!(!failure_root.join("profiles/pending.profdata").exists());
            if mode == "cov_drift" {
                let classification =
                    fs::read_to_string(failure_root.join("failures/classification.txt")).unwrap();
                assert!(
                    classification.contains("denominator changed"),
                    "{classification}"
                );
                assert!(failure_root.join("profiles/cumulative.profdata").exists());
            } else {
                assert!(!failure_root.join("profiles/cumulative.profdata").exists());
            }
        }

        let pending_capacity = run_cli(
            &campaign,
            &target,
            &profdata,
            &cov,
            &root,
            "pending-capacity",
            1,
            &["--max-pending-bytes", "64"],
        );
        assert!(!pending_capacity.status.success());
        let pending_root = root.join("pending-capacity");
        let pending_classification =
            fs::read_to_string(pending_root.join("failures/classification.txt")).unwrap();
        assert!(pending_classification.contains("pending-byte limit"));
        assert!(
            fs::read_dir(pending_root.join("pending"))
                .unwrap()
                .next()
                .is_none()
        );
        assert!(!pending_root.join("profiles/final.profraw").exists());

        let commit_fail_cov = install_mode(&helper, &root, "cov_cumulative_commit_fail");
        let commit_failure = run_cli(
            &campaign,
            &target,
            &profdata,
            &commit_fail_cov,
            &root,
            "cumulative-commit-failure",
            100,
            &[],
        );
        assert!(!commit_failure.status.success());
        let commit_root = root.join("cumulative-commit-failure");
        let commit_classification =
            fs::read_to_string(commit_root.join("failures/classification.txt")).unwrap();
        assert!(commit_classification.contains("cumulative profile commit failed"));
        assert_eq!(
            fs::read(commit_root.join("cases.tsv")).unwrap(),
            fs::read(commit_root.join("pre-commit-journal.snapshot")).unwrap()
        );
        assert_eq!(
            fs::read(commit_root.join("profiles/cumulative.prior")).unwrap(),
            b"low"
        );
        assert!(commit_root.join("profiles/cumulative.profdata").is_dir());
        assert_eq!(
            fs::read(commit_root.join("profiles/final.profraw")).unwrap(),
            b"low"
        );
        assert!(!commit_root.join("profiles/pending.profdata").exists());
        assert_eq!(fs::read_dir(commit_root.join("corpus")).unwrap().count(), 1);
        let failed_input = fs::read(commit_root.join("failures/first-failure.input")).unwrap();
        let failed_meta =
            fs::read_to_string(commit_root.join("failures/first-failure.meta")).unwrap();
        let failed_identity = campaign::identity(&failed_input);
        assert!(failed_meta.contains(&format!(
            "candidate_identity_fnv64_len={:016x}:{}",
            failed_identity.0, failed_identity.1
        )));

        let capacity = run_cli(
            &campaign,
            &target,
            &profdata,
            &cov,
            &root,
            "corpus-capacity",
            100,
            &["--max-corpus-files", "1"],
        );
        assert!(!capacity.status.success());
        let failure_root = root.join("corpus-capacity");
        assert!(failure_root.join("failures/first-failure.input").exists());
        let failed_metadata =
            fs::read_to_string(failure_root.join("failures/first-failure.meta")).unwrap();
        assert!(!failed_metadata.contains("parent_identity_fnv64_len=-"));
        assert_eq!(
            fs::read(failure_root.join("failures/first-failure.profraw")).unwrap(),
            b"high"
        );
        assert_eq!(
            fs::read(failure_root.join("profiles/final.profraw")).unwrap(),
            b"low"
        );
        assert!(failure_root.join("profiles/cumulative.profdata").exists());
        assert!(!failure_root.join("profiles/pending.profdata").exists());

        // The first two seeds cover the low and high fixture states. The third
        // candidate reaches the post-commit path; the fake cov tool makes
        // evidence publication fail so Candidate::drop must leave its original
        // pending files intact.
        fs::write(root.join("seeds/seed-z.xml"), b"y").unwrap();
        let post_commit_cov = install_mode(&helper, &root, "cov_post_commit_fail");
        let post_commit = run_cli(
            &campaign,
            &target,
            &profdata,
            &post_commit_cov,
            &root,
            "post-commit-evidence",
            3,
            &[],
        );
        assert!(!post_commit.status.success());
        let post_root = root.join("post-commit-evidence");
        let classification =
            fs::read_to_string(post_root.join("failures/classification.txt")).unwrap();
        assert!(classification.contains("post-commit"), "{classification}");
        assert!(
            classification.contains("could not preserve first-failure.input"),
            "{classification}"
        );
        let pending_input = fs::read(post_root.join("pending/candidate-000002.bin")).unwrap();
        let pending_meta =
            fs::read_to_string(post_root.join("pending/candidate-000002.meta")).unwrap();
        let identity = campaign::identity(&pending_input);
        assert!(
            pending_meta.contains(&format!(
                "candidate_identity_fnv64_len={:016x}:{}",
                identity.0, identity.1
            )),
            "{pending_meta}"
        );
        assert!(post_root.join("failures/first-failure.input").is_dir());
        assert!(post_root.join("profiles/cumulative.profdata").exists());
        let post_journal = fs::read_to_string(post_root.join("cases.tsv")).unwrap();
        assert_eq!(post_journal.lines().count(), 3);
        assert!(
            post_journal
                .lines()
                .last()
                .unwrap()
                .contains("\tduplicate\t"),
            "{post_journal}"
        );

        fs::remove_dir_all(root).unwrap();

        let identity_root = fixture_root();
        let (identity_helper, identity_campaign) = compile_fake_programs(&identity_root);
        let identity_target = install_mode(&identity_helper, &identity_root, "target_ok");
        let identity_profdata = install_mode(&identity_helper, &identity_root, "profdata_ok");
        let changed_clean_cov =
            install_mode(&identity_helper, &identity_root, "cov_change_source_clean");
        let changed_clean = run_cli(
            &identity_campaign,
            &identity_target,
            &identity_profdata,
            &changed_clean_cov,
            &identity_root,
            "identity-clean",
            1,
            &[],
        );
        assert!(!changed_clean.status.success());
        let clean_classification =
            fs::read_to_string(identity_root.join("identity-clean/failures/classification.txt"))
                .unwrap();
        assert!(
            clean_classification.contains("source identity changed"),
            "{clean_classification}"
        );
        let changed_failure_cov =
            install_mode(&identity_helper, &identity_root, "cov_change_source_fail");
        let changed_failure = run_cli(
            &identity_campaign,
            &identity_target,
            &identity_profdata,
            &changed_failure_cov,
            &identity_root,
            "identity-failure",
            1,
            &[],
        );
        assert!(!changed_failure.status.success());
        let failure_classification =
            fs::read_to_string(identity_root.join("identity-failure/failures/classification.txt"))
                .unwrap();
        assert!(
            failure_classification.contains("source identity changed"),
            "{failure_classification}"
        );
        fs::remove_dir_all(identity_root).unwrap();
    }

    #[test]
    fn bounded_process_overlay_covers_success_nonzero_output_and_timeout() {
        let shell = Path::new("/bin/sh");
        let limits = ProcessLimits {
            timeout: Duration::from_secs(2),
            max_output_bytes: 64,
        };
        let success = process::run(shell, &["-c", "printf ok"], b"", limits).unwrap();
        assert!(success.status.success());
        assert_eq!(success.stdout, b"ok");

        let nonzero = process::run(shell, &["-c", "printf err >&2; exit 7"], b"", limits).unwrap();
        assert_eq!(nonzero.status.code(), Some(7));
        assert_eq!(nonzero.stderr, b"err");

        let output = process::run(
            shell,
            &["-c", "printf 123456789"],
            b"",
            ProcessLimits {
                max_output_bytes: 4,
                ..limits
            },
        )
        .unwrap_err();
        assert_eq!(output.kind(), io::ErrorKind::FileTooLarge);

        let timeout = process::run(
            shell,
            &["-c", "while :; do :; done"],
            b"",
            ProcessLimits {
                timeout: Duration::from_millis(50),
                ..limits
            },
        )
        .unwrap_err();
        assert_eq!(timeout.kind(), io::ErrorKind::TimedOut);
    }
}
