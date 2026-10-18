//! Bounded Cargo preparation and evidence ownership for the test-only integration path.
//!
//! Preparation freezes every caller-owned input before launching tools, retains only completed
//! bounded captures, and publishes its owned evidence directory only after artifact selection and
//! resolution succeed. `PreparedCargoTarget` keeps the original selection and expectations; no
//! lifecycle check can replace or advance that baseline.
#![forbid(unsafe_code)]

use crate::cargo_artifact::{
    ArtifactError, CanonicalArtifactRoots, CargoBuildStatus, ExpectedArtifact, SelectedArtifact,
    SelectedArtifactCheck, establish_artifact_roots, recheck_selected_artifact,
    resolve_selected_artifact, select_artifact_record,
};
use crate::file_identity::{
    ContentIdentity, FileIdentityError, FileStamp, file_identity, file_identity_from_file_bounded,
    file_stamp,
};
use crate::process::{self, ProcessLimits, ProcessOutput};
use crate::source_inventory::{InventoryError, SourceInventory};
use std::ffi::OsStr;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) const MAX_BUILD_STDOUT_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_BUILD_STDERR_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_BUILD_OUTPUT_BYTES: usize = 9 * 1024 * 1024;
pub(crate) const MAX_PREPARATION_SECONDS: u64 = 600;
const MAX_VERSION_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_PKGID_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_PKGID_BYTES: usize = 16 * 1024;
const MAX_HOST_BYTES: usize = 256;
const MAX_PATH_BYTES: usize = 32 * 1024;
const MAX_SCRATCH_ATTEMPTS: usize = 128;

const REMOVED_ENVIRONMENT: [&str; 6] = [
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_LOG",
    "CARGO_TERM_VERBOSE",
    "CARGO_TERM_QUIET",
    "LLVM_PROFILE_FILE",
    "LLVM_PROFILE_PREFIX",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CargoProfile {
    Debug,
    Release,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CargoPreparationLimits {
    pub(crate) timeout: Duration,
    pub(crate) max_build_output_bytes: usize,
}

impl Default for CargoPreparationLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(300),
            max_build_output_bytes: MAX_BUILD_OUTPUT_BYTES,
        }
    }
}

#[derive(Debug)]
pub(crate) struct CargoBootstrapRequest<'a> {
    pub(crate) source_root: &'a Path,
    pub(crate) target_dir: &'a Path,
    pub(crate) output_dir: &'a Path,
    pub(crate) compiler_version_label: &'a str,
    pub(crate) limits: CargoPreparationLimits,
}

#[derive(Debug)]
pub(crate) struct CargoPreparationBootstrap {
    roots: CanonicalArtifactRoots,
    output_dir: PathBuf,
    manifest: FrozenFile,
    lockfile: FrozenFile,
    compiler_version_label: String,
    limits: CargoPreparationLimits,
}

#[derive(Debug)]
pub(crate) struct CargoPreparationRequest<'a> {
    pub(crate) bootstrap: CargoPreparationBootstrap,
    pub(crate) source_inventory: SourceInventory,
    pub(crate) cargo: &'a Path,
    pub(crate) rustc: &'a Path,
    pub(crate) llvm_profdata: &'a Path,
    pub(crate) llvm_cov: &'a Path,
    pub(crate) profile: CargoProfile,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreparationStage {
    Preflight,
    LlvmProfdataVersion,
    LlvmCovVersion,
    CargoVersion,
    RustcVersion,
    Pkgid,
    Build,
    Jsonl,
    Resolve,
    Publish,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EvidenceSubject {
    Manifest,
    Lockfile,
    SourceInventory,
    Cargo,
    Rustc,
    LlvmProfdata,
    LlvmCov,
    Jsonl,
    BuildStderr,
    CommandOutput,
    OwnerMarker,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolMetadataField {
    Manifest,
    Lockfile,
    Source,
    Cargo,
    Rustc,
    LlvmProfdata,
    LlvmCov,
    CommandOutput,
    OwnerMarker,
    RustcHost,
    LlvmVersion,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum CargoPreparationErrorKind {
    Configuration,
    ProcessTimeout,
    ProcessOutputLimit,
    ProcessIo(io::ErrorKind),
    ProcessExit,
    Stderr,
    SourceInventory(InventoryError),
    ToolMetadata(ToolMetadataField),
    PkgidShape,
    PkgidEncoding,
    PkgidLimit,
    StdoutLimit,
    StderrLimit,
    IdentityChanged(EvidenceSubject),
    EvidenceIo(io::ErrorKind),
    Artifact(ArtifactError),
}

#[derive(Debug)]
pub(crate) struct CargoPreparationError {
    pub(crate) stage: PreparationStage,
    pub(crate) kind: CargoPreparationErrorKind,
    pub(crate) evidence_dir: Option<PathBuf>,
    pub(crate) capture_complete: bool,
    pub(crate) retention_complete: bool,
    pub(crate) accepted: bool,
    pub(crate) secondary: Vec<CargoPreparationDiagnostic>,
    pub(crate) attempt: Option<CommandAttempt>,
    pub(crate) receipts: Vec<CommandReceipt>,
    pub(crate) inventory_error: Option<InventoryError>,
    io_error: Option<Arc<io::Error>>,
}

#[derive(Debug)]
pub(crate) struct CargoPreparationDiagnostic {
    kind: CargoPreparationErrorKind,
    inventory_error: Option<InventoryError>,
    io_error: Option<Arc<io::Error>>,
}

impl CargoPreparationDiagnostic {
    pub(crate) fn kind(&self) -> &CargoPreparationErrorKind {
        &self.kind
    }

    pub(crate) fn io_error(&self) -> Option<&io::Error> {
        self.io_error.as_deref()
    }

    pub(crate) fn inventory_error(&self) -> Option<&InventoryError> {
        self.inventory_error.as_ref()
    }
}

impl std::fmt::Display for CargoPreparationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let CargoPreparationErrorKind::SourceInventory(error) = &self.kind {
            write!(formatter, "preflight/source_inventory: {}", error.reason())?;
            if let Some(path) = error.path() {
                write!(formatter, ": {path}")?;
            }
            return Ok(());
        }
        write!(
            formatter,
            "Cargo preparation {:?}: {:?}",
            self.stage, self.kind
        )
    }
}

impl std::error::Error for CargoPreparationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.io_error
            .as_deref()
            .map(|error| error as &(dyn std::error::Error + 'static))
    }
}

impl CargoPreparationError {
    pub(crate) fn inventory_error(&self) -> Option<&InventoryError> {
        match &self.kind {
            CargoPreparationErrorKind::SourceInventory(error) => Some(error),
            _ => self.inventory_error.as_ref(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FrozenFile {
    supplied_path: PathBuf,
    canonical_path: PathBuf,
    stamp: FileStamp,
    identity: ContentIdentity,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FrozenFileView<'a> {
    pub(crate) supplied_path: &'a Path,
    pub(crate) canonical_path: &'a Path,
    pub(crate) stamp: &'a FileStamp,
    pub(crate) identity: ContentIdentity,
}

impl FrozenFile {
    pub(crate) fn view(&self) -> FrozenFileView<'_> {
        FrozenFileView {
            supplied_path: &self.supplied_path,
            canonical_path: &self.canonical_path,
            stamp: &self.stamp,
            identity: self.identity,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CommandAttempt {
    pub(crate) stage: PreparationStage,
    pub(crate) program: PathBuf,
    pub(crate) canonical_program: PathBuf,
    pub(crate) argv: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) environment: Vec<(String, String)>,
    pub(crate) removed_environment: Vec<&'static str>,
    pub(crate) configuration_locations: Vec<(String, Option<PathBuf>)>,
    pub(crate) timeout: Duration,
    pub(crate) max_output_bytes: usize,
    pub(crate) elapsed: Option<Duration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CommandReceipt {
    stage: PreparationStage,
    program: PathBuf,
    canonical_program: PathBuf,
    argv: Vec<String>,
    cwd: PathBuf,
    environment: Vec<(String, String)>,
    removed_environment: Vec<&'static str>,
    configuration_locations: Vec<(String, Option<PathBuf>)>,
    timeout: Duration,
    max_output_bytes: usize,
    elapsed: Duration,
    status: ExitStatus,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    stdout_len: usize,
    stderr_len: usize,
    stdout_identity: ContentIdentity,
    stderr_identity: ContentIdentity,
    stdout_stamp: Option<FileStamp>,
    stderr_stamp: Option<FileStamp>,
    capture_complete: bool,
    retention_complete: bool,
    accepted: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CommandReceiptView<'a> {
    pub(crate) stage: PreparationStage,
    pub(crate) program: &'a Path,
    pub(crate) canonical_program: &'a Path,
    pub(crate) argv: &'a [String],
    pub(crate) cwd: &'a Path,
    pub(crate) environment: &'a [(String, String)],
    pub(crate) removed_environment: &'a [&'static str],
    pub(crate) configuration_locations: &'a [(String, Option<PathBuf>)],
    pub(crate) timeout: Duration,
    pub(crate) max_output_bytes: usize,
    pub(crate) elapsed: Duration,
    pub(crate) status: &'a ExitStatus,
    pub(crate) stdout_len: usize,
    pub(crate) stderr_len: usize,
    pub(crate) stdout: Option<FrozenFileView<'a>>,
    pub(crate) stderr: Option<FrozenFileView<'a>>,
    pub(crate) capture_complete: bool,
    pub(crate) retention_complete: bool,
    pub(crate) accepted: bool,
}

impl CommandReceipt {
    pub(crate) fn view(&self) -> CommandReceiptView<'_> {
        CommandReceiptView {
            stage: self.stage,
            program: &self.program,
            canonical_program: &self.canonical_program,
            argv: &self.argv,
            cwd: &self.cwd,
            environment: &self.environment,
            removed_environment: &self.removed_environment,
            configuration_locations: &self.configuration_locations,
            timeout: self.timeout,
            max_output_bytes: self.max_output_bytes,
            elapsed: self.elapsed,
            status: &self.status,
            stdout_len: self.stdout_len,
            stderr_len: self.stderr_len,
            stdout: self.stdout_stamp.as_ref().map(|stamp| FrozenFileView {
                supplied_path: &self.stdout_path,
                canonical_path: &self.stdout_path,
                stamp,
                identity: self.stdout_identity,
            }),
            stderr: self.stderr_stamp.as_ref().map(|stamp| FrozenFileView {
                supplied_path: &self.stderr_path,
                canonical_path: &self.stderr_path,
                stamp,
                identity: self.stderr_identity,
            }),
            capture_complete: self.capture_complete,
            retention_complete: self.retention_complete,
            accepted: self.accepted,
        }
    }
}

#[derive(Debug)]
pub(crate) struct PreparedCargoTarget {
    selected: SelectedArtifact,
    expected: ExpectedArtifact,
    roots: CanonicalArtifactRoots,
    manifest: FrozenFile,
    lockfile: FrozenFile,
    source_inventory: SourceInventory,
    cargo: FrozenFile,
    rustc: FrozenFile,
    llvm_profdata: FrozenFile,
    llvm_cov: FrozenFile,
    package_id: String,
    rustc_host: String,
    profile: CargoProfile,
    preparation_elapsed: Duration,
    evidence_dir: PathBuf,
    jsonl: FrozenFile,
    build_stderr: FrozenFile,
    receipts: Vec<CommandReceipt>,
}

impl PreparedCargoTarget {
    pub(crate) fn selected(&self) -> &SelectedArtifact {
        &self.selected
    }

    pub(crate) fn expected(&self) -> &ExpectedArtifact {
        &self.expected
    }

    pub(crate) fn evidence_dir(&self) -> &Path {
        &self.evidence_dir
    }

    pub(crate) fn package_id(&self) -> &str {
        &self.package_id
    }

    pub(crate) fn rustc_host(&self) -> &str {
        &self.rustc_host
    }

    pub(crate) fn receipts(&self) -> &[CommandReceipt] {
        &self.receipts
    }

    pub(crate) fn roots(&self) -> &CanonicalArtifactRoots {
        &self.roots
    }

    pub(crate) fn profile(&self) -> CargoProfile {
        self.profile
    }

    pub(crate) fn preparation_elapsed(&self) -> Duration {
        self.preparation_elapsed
    }

    pub(crate) fn jsonl(&self) -> FrozenFileView<'_> {
        self.jsonl.view()
    }

    pub(crate) fn build_stderr(&self) -> FrozenFileView<'_> {
        self.build_stderr.view()
    }

    pub(crate) fn manifest(&self) -> FrozenFileView<'_> {
        self.manifest.view()
    }

    pub(crate) fn lockfile(&self) -> FrozenFileView<'_> {
        self.lockfile.view()
    }

    pub(crate) fn cargo(&self) -> FrozenFileView<'_> {
        self.cargo.view()
    }

    pub(crate) fn rustc(&self) -> FrozenFileView<'_> {
        self.rustc.view()
    }

    pub(crate) fn llvm_profdata(&self) -> FrozenFileView<'_> {
        self.llvm_profdata.view()
    }

    pub(crate) fn llvm_cov(&self) -> FrozenFileView<'_> {
        self.llvm_cov.view()
    }

    pub(crate) fn source_inventory(&self) -> &SourceInventory {
        &self.source_inventory
    }

    pub(crate) fn recheck(
        &self,
        check: SelectedArtifactCheck,
    ) -> Result<(), CargoPreparationError> {
        recheck_prepared(self, check).map_err(|failure| {
            failure.into_error(Some(self.evidence_dir.clone()), None, Vec::new())
        })
    }
}

#[derive(Debug)]
struct PreparationFailure {
    stage: PreparationStage,
    kind: CargoPreparationErrorKind,
    capture_complete: bool,
    retention_complete: bool,
    secondary: Vec<CargoPreparationDiagnostic>,
    inventory_error: Option<InventoryError>,
    io_error: Option<Arc<io::Error>>,
}

impl PreparationFailure {
    fn new(stage: PreparationStage, kind: CargoPreparationErrorKind) -> Self {
        Self {
            stage,
            kind,
            capture_complete: true,
            retention_complete: true,
            secondary: Vec::new(),
            inventory_error: None,
            io_error: None,
        }
    }

    fn with_secondary(
        mut self,
        kind: CargoPreparationErrorKind,
        io_error: Option<io::Error>,
    ) -> Self {
        self.secondary.push(CargoPreparationDiagnostic {
            kind,
            inventory_error: None,
            io_error: io_error.map(Arc::new),
        });
        self
    }

    fn into_error(
        self,
        evidence_dir: Option<PathBuf>,
        attempt: Option<CommandAttempt>,
        receipts: Vec<CommandReceipt>,
    ) -> CargoPreparationError {
        CargoPreparationError {
            stage: self.stage,
            kind: self.kind,
            evidence_dir,
            capture_complete: self.capture_complete,
            retention_complete: self.retention_complete,
            accepted: false,
            secondary: self.secondary,
            attempt,
            receipts,
            inventory_error: self.inventory_error,
            io_error: self.io_error,
        }
    }
}

struct OwnedPreparationDirectory {
    path: PathBuf,
    owner_marker: FrozenFile,
    process_attempted: bool,
    transferred: bool,
    receipts: Vec<CommandReceipt>,
    last_attempt: Option<CommandAttempt>,
    selection: Option<(SelectedArtifact, ExpectedArtifact)>,
}

impl OwnedPreparationDirectory {
    fn new(output: &Path) -> Result<Self, PreparationFailure> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = output.parent().ok_or_else(|| {
            PreparationFailure::new(
                PreparationStage::Preflight,
                CargoPreparationErrorKind::Configuration,
            )
        })?;
        let name = output.file_name().and_then(OsStr::to_str).ok_or_else(|| {
            PreparationFailure::new(
                PreparationStage::Preflight,
                CargoPreparationErrorKind::Configuration,
            )
        })?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| evidence_io(PreparationStage::Preflight, io::Error::other(error)))?
            .as_nanos();
        for _ in 0..MAX_SCRATCH_ATTEMPTS {
            let serial = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(
                ".{name}.cargo-preparation-{}-{timestamp}-{serial}",
                std::process::id()
            ));
            let mut builder = DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => {
                    let marker_path = path.join(".owner");
                    let token = format!("{}:{timestamp}:{serial}\n", std::process::id());
                    if let Err(error) = write_new(&marker_path, token.as_bytes()) {
                        let _ = fs::remove_file(&marker_path);
                        let _ = fs::remove_dir(&path);
                        return Err(evidence_io(PreparationStage::Preflight, error));
                    }
                    let owner_marker =
                        match freeze_strict_file(&marker_path, EvidenceSubject::OwnerMarker) {
                            Ok(marker) => marker,
                            Err(error) => {
                                let _ = fs::remove_file(&marker_path);
                                let _ = fs::remove_dir(&path);
                                return Err(error);
                            }
                        };
                    return Ok(Self {
                        path,
                        owner_marker,
                        process_attempted: false,
                        transferred: false,
                        receipts: Vec::new(),
                        last_attempt: None,
                        selection: None,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(evidence_io(PreparationStage::Preflight, error));
                }
            }
        }
        Err(evidence_io(
            PreparationStage::Preflight,
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "could not create unique Cargo preparation directory",
            ),
        ))
    }

    fn failure(&self, failure: PreparationFailure) -> CargoPreparationError {
        let PreparationFailure {
            stage,
            kind,
            capture_complete,
            retention_complete,
            secondary: intrinsic_secondary,
            inventory_error,
            io_error,
        } = failure;
        CargoPreparationError {
            stage,
            kind,
            evidence_dir: self.process_attempted.then(|| self.path.clone()),
            capture_complete,
            retention_complete,
            accepted: false,
            secondary: intrinsic_secondary,
            attempt: self.last_attempt.clone(),
            receipts: self.receipts.clone(),
            inventory_error,
            io_error,
        }
    }

    fn verify_owner(&self) -> Result<(), PreparationFailure> {
        if fs::canonicalize(&self.path)
            .map(|canonical| canonical != self.path)
            .unwrap_or(true)
        {
            return Err(identity_changed(EvidenceSubject::OwnerMarker));
        }
        recheck_frozen_file(&self.owner_marker, EvidenceSubject::OwnerMarker)
    }

    fn record_relocation(&mut self, published: &Path) {
        self.path = published.to_path_buf();
        self.owner_marker.supplied_path = published.join(".owner");
        self.owner_marker.canonical_path = self.owner_marker.supplied_path.clone();
    }
}

impl Drop for OwnedPreparationDirectory {
    fn drop(&mut self) {
        if self.transferred || self.process_attempted {
            return;
        }
        if self.verify_owner().is_err() {
            return;
        }
        if fs::remove_file(&self.owner_marker.supplied_path).is_ok() {
            let _ = fs::remove_dir(&self.path);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreparationCheckpoint {
    BeforeFirstGate,
    BeforeCommand(PreparationStage),
    AfterCommand(PreparationStage),
    AfterBuild,
    BeforeArtifactSelection,
    AfterArtifactSelection,
    AfterResolution,
    BeforePublication,
    AfterRelocation,
}

thread_local! {
    static PREPARATION_CHECKPOINT_HOOK: std::cell::RefCell<
        Option<Box<dyn FnMut(PreparationCheckpoint, &OwnedPreparationDirectory, &Preflight)>>
    > = std::cell::RefCell::new(None);
    static JSONL_CHECKPOINT_HOOK: std::cell::RefCell<
        Option<Box<dyn FnMut(&FrozenFile)>>
    > = std::cell::RefCell::new(None);
}

fn jsonl_post_read_checkpoint(frozen: &FrozenFile) {
    JSONL_CHECKPOINT_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(frozen);
        }
    });
}

fn test_checkpoint(
    checkpoint: PreparationCheckpoint,
    owned: &OwnedPreparationDirectory,
    preflight: &Preflight,
) -> Result<(), PreparationFailure> {
    PREPARATION_CHECKPOINT_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(checkpoint, owned, preflight);
        }
    });
    Ok(())
}

pub(crate) fn begin_cargo_preparation(
    request: &CargoBootstrapRequest<'_>,
) -> Result<CargoPreparationBootstrap, CargoPreparationError> {
    begin_cargo_preparation_inner(request)
        .map_err(|failure| failure.into_error(None, None, Vec::new()))
}

pub(crate) fn capture_preparation_inventory(
    bootstrap: &CargoPreparationBootstrap,
) -> Result<SourceInventory, CargoPreparationError> {
    SourceInventory::capture(&bootstrap.roots.canonical_source_root).map_err(|error| {
        PreparationFailure::new(
            PreparationStage::Preflight,
            CargoPreparationErrorKind::SourceInventory(error),
        )
        .into_error(None, None, Vec::new())
    })
}

pub(crate) fn prepare_cargo_target(
    request: CargoPreparationRequest<'_>,
) -> Result<PreparedCargoTarget, CargoPreparationError> {
    let mut preflight =
        preflight(request).map_err(|failure| failure.into_error(None, None, Vec::new()))?;
    let mut owned = OwnedPreparationDirectory::new(&preflight.output_dir)
        .map_err(|failure| failure.into_error(None, None, Vec::new()))?;
    match prepare_in_owned(&mut preflight, &mut owned) {
        Ok(prepared) => Ok(prepared),
        Err(failure) => {
            let mut aggregate = Some(failure);
            if owned.process_attempted {
                aggregate_check(
                    &mut aggregate,
                    recheck_preflight(&preflight, SelectedArtifactCheck::Content),
                );
                for receipt in &owned.receipts {
                    if receipt.retention_complete {
                        aggregate_check(
                            &mut aggregate,
                            recheck_receipt(receipt, SelectedArtifactCheck::Content),
                        );
                    }
                }
                if let Some((selected, expected)) = &owned.selection {
                    aggregate_check(
                        &mut aggregate,
                        recheck_selected_artifact(
                            selected,
                            expected,
                            SelectedArtifactCheck::Content,
                        )
                        .map_err(|error| {
                            PreparationFailure::new(
                                PreparationStage::Resolve,
                                CargoPreparationErrorKind::Artifact(error),
                            )
                        }),
                    );
                }
            }
            Err(owned.failure(aggregate.expect("the primary preparation failure is retained")))
        }
    }
}

struct Preflight {
    roots: CanonicalArtifactRoots,
    output_dir: PathBuf,
    manifest: FrozenFile,
    lockfile: FrozenFile,
    source_inventory: Option<SourceInventory>,
    cargo: FrozenFile,
    rustc: FrozenFile,
    llvm_profdata: FrozenFile,
    llvm_cov: FrozenFile,
    compiler_version_label: String,
    profile: CargoProfile,
    limits: CargoPreparationLimits,
    configuration_locations: Vec<(String, Option<PathBuf>)>,
}

impl Preflight {
    fn source_inventory(&self) -> &SourceInventory {
        self.source_inventory
            .as_ref()
            .expect("source inventory transfers only after the final gate")
    }
}

fn begin_cargo_preparation_inner(
    request: &CargoBootstrapRequest<'_>,
) -> Result<CargoPreparationBootstrap, PreparationFailure> {
    validate_fresh_output(request.output_dir)?;
    validate_limits(request.compiler_version_label, request.limits)?;
    validate_root_length(
        request.source_root,
        crate::cargo_artifact::ArtifactPathField::SourceRoot,
    )?;
    validate_root_length(
        request.target_dir,
        crate::cargo_artifact::ArtifactPathField::TargetDir,
    )?;
    let roots =
        establish_artifact_roots(request.source_root, request.target_dir).map_err(|error| {
            PreparationFailure::new(
                PreparationStage::Preflight,
                CargoPreparationErrorKind::Artifact(error),
            )
        })?;
    let manifest = freeze_strict_file(
        &roots.canonical_source_root.join("Cargo.toml"),
        EvidenceSubject::Manifest,
    )?;
    let lockfile = freeze_strict_file(
        &roots.canonical_source_root.join("Cargo.lock"),
        EvidenceSubject::Lockfile,
    )?;
    if roots
        .canonical_source_root
        .starts_with(&roots.canonical_target_dir)
        || roots.canonical_target_dir == roots.canonical_source_root
        || roots
            .canonical_target_dir
            .starts_with(roots.canonical_source_root.join("src"))
    {
        return Err(PreparationFailure::new(
            PreparationStage::Preflight,
            CargoPreparationErrorKind::Configuration,
        ));
    }
    Ok(CargoPreparationBootstrap {
        roots,
        output_dir: request.output_dir.to_path_buf(),
        manifest,
        lockfile,
        compiler_version_label: request.compiler_version_label.to_owned(),
        limits: request.limits,
    })
}

fn preflight(request: CargoPreparationRequest<'_>) -> Result<Preflight, PreparationFailure> {
    let CargoPreparationRequest {
        bootstrap,
        source_inventory,
        cargo,
        rustc,
        llvm_profdata,
        llvm_cov,
        profile,
    } = request;
    source_inventory
        .require_roots(
            &bootstrap.roots.canonical_source_root,
            bootstrap
                .roots
                .canonical_src_path
                .parent()
                .expect("src/lib.rs has a parent"),
        )
        .map_err(initial_inventory_failure)?;
    recheck_bootstrap(&bootstrap)?;
    source_inventory
        .recheck()
        .map_err(initial_inventory_failure)?;
    let configuration_locations = configuration_locations(&bootstrap.roots.canonical_source_root)?;
    Ok(Preflight {
        roots: bootstrap.roots,
        output_dir: bootstrap.output_dir,
        manifest: bootstrap.manifest,
        lockfile: bootstrap.lockfile,
        source_inventory: Some(source_inventory),
        cargo: freeze_tool_file(cargo, EvidenceSubject::Cargo)?,
        rustc: freeze_tool_file(rustc, EvidenceSubject::Rustc)?,
        llvm_profdata: freeze_tool_file(llvm_profdata, EvidenceSubject::LlvmProfdata)?,
        llvm_cov: freeze_tool_file(llvm_cov, EvidenceSubject::LlvmCov)?,
        compiler_version_label: bootstrap.compiler_version_label,
        profile,
        limits: bootstrap.limits,
        configuration_locations,
    })
}

fn validate_limits(
    compiler_version_label: &str,
    limits: CargoPreparationLimits,
) -> Result<(), PreparationFailure> {
    if limits.timeout.is_zero()
        || limits.timeout > Duration::from_secs(MAX_PREPARATION_SECONDS)
        || limits.max_build_output_bytes == 0
        || limits.max_build_output_bytes > MAX_BUILD_OUTPUT_BYTES
        || compiler_version_label.is_empty()
        || compiler_version_label.len() > 4096
        || compiler_version_label
            .bytes()
            .any(|byte| byte.is_ascii_control())
    {
        return Err(PreparationFailure::new(
            PreparationStage::Preflight,
            CargoPreparationErrorKind::Configuration,
        ));
    }
    Ok(())
}

fn validate_root_length(
    path: &Path,
    field: crate::cargo_artifact::ArtifactPathField,
) -> Result<(), PreparationFailure> {
    if path
        .to_str()
        .is_some_and(|text| text.len() > MAX_PATH_BYTES)
    {
        return Err(PreparationFailure::new(
            PreparationStage::Preflight,
            CargoPreparationErrorKind::Artifact(ArtifactError::Path {
                field,
                kind: crate::cargo_artifact::ArtifactPathError::LengthOverflow,
            }),
        ));
    }
    Ok(())
}

fn validate_fresh_output(output: &Path) -> Result<(), PreparationFailure> {
    validate_lexical(output)?;
    match fs::symlink_metadata(output) {
        Ok(_) => {
            return Err(PreparationFailure::new(
                PreparationStage::Preflight,
                CargoPreparationErrorKind::Configuration,
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(evidence_io(PreparationStage::Preflight, error)),
    }
    let parent = output.parent().ok_or_else(|| {
        PreparationFailure::new(
            PreparationStage::Preflight,
            CargoPreparationErrorKind::Configuration,
        )
    })?;
    walk_nonsymlink_directory(parent)?;
    if fs::canonicalize(parent)
        .map(|canonical| canonical != parent)
        .unwrap_or(true)
    {
        return Err(PreparationFailure::new(
            PreparationStage::Preflight,
            CargoPreparationErrorKind::Configuration,
        ));
    }
    Ok(())
}

fn walk_nonsymlink_directory(path: &Path) -> Result<(), PreparationFailure> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                current.push(component.as_os_str());
            }
            Component::CurDir | Component::ParentDir => {
                return Err(PreparationFailure::new(
                    PreparationStage::Preflight,
                    CargoPreparationErrorKind::Configuration,
                ));
            }
        }
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| evidence_io(PreparationStage::Preflight, error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(PreparationFailure::new(
                PreparationStage::Preflight,
                CargoPreparationErrorKind::Configuration,
            ));
        }
    }
    Ok(())
}

fn prepare_in_owned(
    preflight: &mut Preflight,
    owned: &mut OwnedPreparationDirectory,
) -> Result<PreparedCargoTarget, PreparationFailure> {
    let started = Instant::now();
    let environment = preparation_environment(&preflight)?;
    test_checkpoint(PreparationCheckpoint::BeforeFirstGate, owned, preflight)?;
    recheck_preflight(preflight, SelectedArtifactCheck::Content)?;
    let profdata = run_stage(
        owned,
        preflight,
        &preflight.llvm_profdata,
        EvidenceSubject::LlvmProfdata,
        &["--version"],
        PreparationStage::LlvmProfdataVersion,
        MAX_VERSION_OUTPUT_BYTES,
        Duration::from_secs(5),
        &environment,
        &preflight.configuration_locations,
        &preflight.roots.canonical_source_root,
        started,
    )?;
    require_success_without_stderr(PreparationStage::LlvmProfdataVersion, &profdata)?;
    recheck_preflight(preflight, SelectedArtifactCheck::Content)?;
    require_matching_llvm(
        PreparationStage::LlvmProfdataVersion,
        &profdata.stdout,
        "LLVM version ",
        &preflight.compiler_version_label,
    )?;
    accept_last(&mut owned.receipts);

    let cov = run_stage(
        owned,
        preflight,
        &preflight.llvm_cov,
        EvidenceSubject::LlvmCov,
        &["--version"],
        PreparationStage::LlvmCovVersion,
        MAX_VERSION_OUTPUT_BYTES,
        Duration::from_secs(5),
        &environment,
        &preflight.configuration_locations,
        &preflight.roots.canonical_source_root,
        started,
    )?;
    require_success_without_stderr(PreparationStage::LlvmCovVersion, &cov)?;
    recheck_preflight(preflight, SelectedArtifactCheck::Content)?;
    require_matching_llvm(
        PreparationStage::LlvmCovVersion,
        &cov.stdout,
        "LLVM version ",
        &preflight.compiler_version_label,
    )?;
    accept_last(&mut owned.receipts);

    let cargo_version = run_stage(
        owned,
        preflight,
        &preflight.cargo,
        EvidenceSubject::Cargo,
        &["-vV"],
        PreparationStage::CargoVersion,
        MAX_VERSION_OUTPUT_BYTES,
        Duration::from_secs(5),
        &environment,
        &preflight.configuration_locations,
        &preflight.roots.canonical_source_root,
        started,
    )?;
    require_success_without_stderr(PreparationStage::CargoVersion, &cargo_version)?;
    recheck_preflight(preflight, SelectedArtifactCheck::Content)?;
    accept_last(&mut owned.receipts);

    let rustc_version = run_stage(
        owned,
        preflight,
        &preflight.rustc,
        EvidenceSubject::Rustc,
        &["-vV"],
        PreparationStage::RustcVersion,
        MAX_VERSION_OUTPUT_BYTES,
        Duration::from_secs(5),
        &environment,
        &preflight.configuration_locations,
        &preflight.roots.canonical_source_root,
        started,
    )?;
    require_success_without_stderr(PreparationStage::RustcVersion, &rustc_version)?;
    recheck_preflight(preflight, SelectedArtifactCheck::Content)?;
    let host = parse_rustc_host(&rustc_version.stdout)?;
    require_matching_llvm(
        PreparationStage::RustcVersion,
        &rustc_version.stdout,
        "LLVM version: ",
        &preflight.compiler_version_label,
    )?;
    accept_last(&mut owned.receipts);

    let manifest = path_text(&preflight.manifest.supplied_path)?;
    let pkgid_args = [
        "pkgid",
        "--manifest-path",
        manifest,
        "--offline",
        "--locked",
        "--color",
        "never",
    ];
    let pkgid = run_stage(
        owned,
        preflight,
        &preflight.cargo,
        EvidenceSubject::Cargo,
        &pkgid_args,
        PreparationStage::Pkgid,
        MAX_PKGID_OUTPUT_BYTES,
        Duration::from_secs(10),
        &environment,
        &preflight.configuration_locations,
        &preflight.roots.canonical_source_root,
        started,
    )?;
    require_success_without_stderr(PreparationStage::Pkgid, &pkgid)?;
    recheck_preflight(preflight, SelectedArtifactCheck::Content)?;
    let package_id = parse_pkgid(&pkgid.stdout)?;
    accept_last(&mut owned.receipts);

    let target = path_text(&preflight.roots.canonical_target_dir)?;
    let mut build_args = vec![
        "test",
        "--lib",
        "--no-run",
        "--message-format=json-render-diagnostics",
        "--manifest-path",
        manifest,
        "--target-dir",
        target,
        "--target",
        host.as_str(),
        "--offline",
        "--locked",
        "--color",
        "never",
    ];
    if preflight.profile == CargoProfile::Release {
        build_args.push("--release");
    }
    let build = run_stage(
        owned,
        preflight,
        &preflight.cargo,
        EvidenceSubject::Cargo,
        &build_args,
        PreparationStage::Build,
        preflight.limits.max_build_output_bytes,
        preflight.limits.timeout,
        &environment,
        &preflight.configuration_locations,
        &preflight.roots.canonical_source_root,
        started,
    )?;
    if !build.status.success() {
        return Err(PreparationFailure::new(
            PreparationStage::Build,
            CargoPreparationErrorKind::ProcessExit,
        ));
    }
    validate_build_stream_lengths(build.stdout.len(), build.stderr.len())?;
    accept_last(&mut owned.receipts);
    test_checkpoint(PreparationCheckpoint::AfterBuild, owned, preflight)?;
    recheck_preflight(preflight, SelectedArtifactCheck::Content)?;
    let jsonl_path = owned.path.join("build.stdout");
    let stderr_path = owned.path.join("build.stderr");
    let build_receipt = owned
        .receipts
        .last()
        .expect("accepted build has a retained receipt");
    let jsonl = frozen_receipt_file(
        &jsonl_path,
        build_receipt.stdout_stamp.as_ref(),
        build_receipt.stdout_identity,
    )?;
    let build_stderr = frozen_receipt_file(
        &stderr_path,
        build_receipt.stderr_stamp.as_ref(),
        build_receipt.stderr_identity,
    )?;
    recheck_jsonl_inner(&jsonl)?;
    if crate::file_identity::content_identity(&build.stdout) != jsonl.identity
        || crate::file_identity::content_identity(&build.stderr) != build_stderr.identity
    {
        return Err(identity_changed(EvidenceSubject::Jsonl));
    }

    let expected = ExpectedArtifact {
        package_id: package_id.clone(),
        target_name: "yaxml".to_owned(),
        target_kind: vec!["lib".to_owned()],
        crate_types: vec!["lib".to_owned()],
        edition: "2024".to_owned(),
        doc: true,
        doctest: true,
        test: true,
        canonical_src_path: preflight.roots.canonical_src_path.clone(),
        canonical_target_dir: preflight.roots.canonical_target_dir.clone(),
    };
    test_checkpoint(
        PreparationCheckpoint::BeforeArtifactSelection,
        owned,
        preflight,
    )?;
    let record = select_artifact_record(&build.stdout, CargoBuildStatus::Success, &expected)
        .map_err(|error| {
            PreparationFailure::new(
                PreparationStage::Jsonl,
                CargoPreparationErrorKind::Artifact(error),
            )
        })?;
    test_checkpoint(
        PreparationCheckpoint::AfterArtifactSelection,
        owned,
        preflight,
    )?;
    recheck_jsonl_inner(&jsonl)?;
    let selected = resolve_selected_artifact(record, &expected).map_err(|error| {
        PreparationFailure::new(
            PreparationStage::Resolve,
            CargoPreparationErrorKind::Artifact(error),
        )
    })?;
    test_checkpoint(PreparationCheckpoint::AfterResolution, owned, preflight)?;
    recheck_selected_artifact(&selected, &expected, SelectedArtifactCheck::Content).map_err(
        |error| {
            PreparationFailure::new(
                PreparationStage::Resolve,
                CargoPreparationErrorKind::Artifact(error),
            )
        },
    )?;
    owned.selection = Some((selected, expected));
    recheck_receipts(&owned.receipts, SelectedArtifactCheck::Content)?;
    recheck_preflight(preflight, SelectedArtifactCheck::Content)?;

    test_checkpoint(PreparationCheckpoint::BeforePublication, owned, preflight)?;
    owned.verify_owner()?;
    fs::create_dir(&preflight.output_dir)
        .map_err(|error| evidence_io(PreparationStage::Publish, error))?;
    let published = preflight.output_dir.join("build");
    fs::rename(&owned.path, &published)
        .map_err(|error| evidence_io(PreparationStage::Publish, error))?;
    owned.record_relocation(&published);
    let mut jsonl = jsonl;
    jsonl.supplied_path = published.join("build.stdout");
    jsonl.canonical_path = jsonl.supplied_path.clone();
    let mut build_stderr = build_stderr;
    build_stderr.supplied_path = published.join("build.stderr");
    build_stderr.canonical_path = build_stderr.supplied_path.clone();
    for receipt in &mut owned.receipts {
        receipt.stdout_path = published.join(
            receipt
                .stdout_path
                .file_name()
                .expect("receipt path has a file name"),
        );
        receipt.stderr_path = published.join(
            receipt
                .stderr_path
                .file_name()
                .expect("receipt path has a file name"),
        );
    }
    test_checkpoint(PreparationCheckpoint::AfterRelocation, owned, preflight)?;
    recheck_receipts(&owned.receipts, SelectedArtifactCheck::Content)?;
    recheck_preflight(preflight, SelectedArtifactCheck::Content)?;
    owned.verify_owner()?;
    let (selected, expected) = owned
        .selection
        .as_ref()
        .expect("resolved selection is retained by the evidence owner");
    recheck_selected_artifact(selected, expected, SelectedArtifactCheck::Content).map_err(
        |error| {
            PreparationFailure::new(
                PreparationStage::Resolve,
                CargoPreparationErrorKind::Artifact(error),
            )
        },
    )?;
    let (selected, expected) = owned
        .selection
        .take()
        .expect("validated selection is transferred exactly once");
    owned.transferred = true;

    Ok(PreparedCargoTarget {
        selected,
        expected,
        roots: preflight.roots.clone(),
        manifest: preflight.manifest.clone(),
        lockfile: preflight.lockfile.clone(),
        source_inventory: preflight
            .source_inventory
            .take()
            .expect("validated source inventory is transferred exactly once"),
        cargo: preflight.cargo.clone(),
        rustc: preflight.rustc.clone(),
        llvm_profdata: preflight.llvm_profdata.clone(),
        llvm_cov: preflight.llvm_cov.clone(),
        package_id,
        rustc_host: host,
        profile: preflight.profile,
        preparation_elapsed: started.elapsed(),
        evidence_dir: published,
        jsonl,
        build_stderr,
        receipts: std::mem::take(&mut owned.receipts),
    })
}

fn run_stage(
    owned: &mut OwnedPreparationDirectory,
    preflight: &Preflight,
    tool: &FrozenFile,
    tool_subject: EvidenceSubject,
    args: &[&str],
    stage: PreparationStage,
    max_output_bytes: usize,
    stage_timeout: Duration,
    environment: &[(String, String)],
    configuration_locations: &[(String, Option<PathBuf>)],
    canonical_cwd: &Path,
    started: Instant,
) -> Result<ProcessOutput, PreparationFailure> {
    test_checkpoint(
        PreparationCheckpoint::BeforeCommand(stage),
        owned,
        preflight,
    )?;
    recheck_preflight(preflight, SelectedArtifactCheck::Content)?;
    let remaining = preflight
        .limits
        .timeout
        .checked_sub(started.elapsed())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| PreparationFailure::new(stage, CargoPreparationErrorKind::ProcessTimeout))?;
    let timeout = remaining.min(stage_timeout);
    let canonical_program = fs::canonicalize(&tool.supplied_path)
        .map_err(|error| evidence_io(PreparationStage::Preflight, error))?;
    if canonical_program != tool.canonical_path {
        return Err(identity_changed(tool_subject));
    }
    owned.process_attempted = true;
    let attempt = CommandAttempt {
        stage,
        program: tool.supplied_path.clone(),
        canonical_program: tool.canonical_path.clone(),
        argv: args.iter().map(|arg| (*arg).to_owned()).collect(),
        cwd: canonical_cwd.to_path_buf(),
        environment: environment.to_vec(),
        removed_environment: REMOVED_ENVIRONMENT.to_vec(),
        configuration_locations: configuration_locations.to_vec(),
        timeout,
        max_output_bytes,
        elapsed: None,
    };
    owned.last_attempt = Some(attempt.clone());
    let command_started = Instant::now();
    let result = process::run_with_environment(
        &tool.supplied_path,
        args,
        b"",
        ProcessLimits {
            timeout,
            max_output_bytes,
        },
        environment,
        &REMOVED_ENVIRONMENT,
        Some(canonical_cwd),
    );
    let elapsed = command_started.elapsed();
    owned
        .last_attempt
        .as_mut()
        .expect("attempt was recorded before invoking the runner")
        .elapsed = Some(elapsed);
    let output = result.map_err(|error| {
        let kind = match error.kind() {
            io::ErrorKind::TimedOut => CargoPreparationErrorKind::ProcessTimeout,
            io::ErrorKind::FileTooLarge => CargoPreparationErrorKind::ProcessOutputLimit,
            kind => CargoPreparationErrorKind::ProcessIo(kind),
        };
        PreparationFailure {
            stage,
            kind,
            capture_complete: false,
            retention_complete: false,
            secondary: Vec::new(),
            inventory_error: None,
            io_error: Some(Arc::new(error)),
        }
    })?;
    test_checkpoint(PreparationCheckpoint::AfterCommand(stage), owned, preflight)?;

    let prefix = stage_name(stage);
    let stdout_path = owned.path.join(format!("{prefix}.stdout"));
    let stderr_path = owned.path.join(format!("{prefix}.stderr"));
    let persistence = (|| -> io::Result<(FileStamp, FileStamp)> {
        let (stdout_stamp, stdout_identity) = persist_new(&stdout_path, &output.stdout)?;
        if stdout_identity != crate::file_identity::content_identity(&output.stdout) {
            return Err(io::Error::other(
                "persisted stdout differs from process capture",
            ));
        }
        let (stderr_stamp, stderr_identity) = persist_new(&stderr_path, &output.stderr)?;
        if stderr_identity != crate::file_identity::content_identity(&output.stderr) {
            return Err(io::Error::other(
                "persisted stderr differs from process capture",
            ));
        }
        Ok((stdout_stamp, stderr_stamp))
    })();
    let (stdout_stamp, stderr_stamp, retention_complete, persistence_error) = match persistence {
        Ok((stdout_stamp, stderr_stamp)) => (Some(stdout_stamp), Some(stderr_stamp), true, None),
        Err(error) => (None, None, false, Some(error)),
    };
    owned.receipts.push(CommandReceipt {
        stage: attempt.stage,
        program: attempt.program,
        canonical_program: attempt.canonical_program,
        argv: attempt.argv,
        cwd: attempt.cwd,
        environment: attempt.environment,
        removed_environment: attempt.removed_environment,
        configuration_locations: attempt.configuration_locations,
        timeout: attempt.timeout,
        max_output_bytes: attempt.max_output_bytes,
        elapsed,
        status: output.status,
        stdout_len: output.stdout.len(),
        stderr_len: output.stderr.len(),
        stdout_identity: crate::file_identity::content_identity(&output.stdout),
        stderr_identity: crate::file_identity::content_identity(&output.stderr),
        stdout_stamp,
        stderr_stamp,
        stdout_path,
        stderr_path,
        capture_complete: true,
        retention_complete,
        accepted: false,
    });
    if let Some(error) = persistence_error {
        let kind = error.kind();
        let evidence = CargoPreparationErrorKind::EvidenceIo(kind);
        if output.status.success() {
            return Err(retention_io(stage, error));
        }
        let mut failure = PreparationFailure::new(stage, CargoPreparationErrorKind::ProcessExit)
            .with_secondary(evidence, Some(error));
        failure.retention_complete = false;
        return Err(failure);
    }
    Ok(output)
}

fn preparation_environment(
    preflight: &Preflight,
) -> Result<Vec<(String, String)>, PreparationFailure> {
    Ok(vec![
        (
            "RUSTC".to_owned(),
            path_text(&preflight.rustc.supplied_path)?.to_owned(),
        ),
        ("RUSTC_WRAPPER".to_owned(), String::new()),
        ("RUSTC_WORKSPACE_WRAPPER".to_owned(), String::new()),
        (
            "CARGO_TARGET_DIR".to_owned(),
            path_text(&preflight.roots.canonical_target_dir)?.to_owned(),
        ),
        ("CARGO_INCREMENTAL".to_owned(), "0".to_owned()),
        (
            "RUSTFLAGS".to_owned(),
            "-C instrument-coverage -Z coverage-options=branch".to_owned(),
        ),
        ("CARGO_PROFILE_RELEASE_LTO".to_owned(), "false".to_owned()),
        (
            "CARGO_PROFILE_RELEASE_CODEGEN_UNITS".to_owned(),
            "16".to_owned(),
        ),
        ("CARGO_TERM_COLOR".to_owned(), "never".to_owned()),
        ("CARGO_TERM_PROGRESS_WHEN".to_owned(), "never".to_owned()),
        ("TERM".to_owned(), "dumb".to_owned()),
        ("XML_CATALOG_FILES".to_owned(), String::new()),
        ("SGML_CATALOG_FILES".to_owned(), String::new()),
    ])
}

fn configuration_locations(
    source_root: &Path,
) -> Result<Vec<(String, Option<PathBuf>)>, PreparationFailure> {
    let locations = vec![
        ("source .cargo".to_owned(), Some(source_root.join(".cargo"))),
        (
            "CARGO_HOME".to_owned(),
            std::env::var_os("CARGO_HOME").map(PathBuf::from),
        ),
        (
            "RUSTUP_HOME".to_owned(),
            std::env::var_os("RUSTUP_HOME").map(PathBuf::from),
        ),
        (
            "HOME/.cargo".to_owned(),
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")),
        ),
    ];
    for (_, path) in &locations {
        if let Some(path) = path
            && ambient_path_bytes(path) > MAX_PATH_BYTES
        {
            return Err(PreparationFailure::new(
                PreparationStage::Preflight,
                CargoPreparationErrorKind::Configuration,
            ));
        }
    }
    Ok(locations)
}

#[cfg(unix)]
fn ambient_path_bytes(path: &Path) -> usize {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().len()
}

#[cfg(windows)]
fn ambient_path_bytes(path: &Path) -> usize {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().count().saturating_mul(2)
}

fn require_success_without_stderr(
    stage: PreparationStage,
    output: &ProcessOutput,
) -> Result<(), PreparationFailure> {
    if !output.status.success() {
        return Err(PreparationFailure::new(
            stage,
            CargoPreparationErrorKind::ProcessExit,
        ));
    }
    if !output.stderr.is_empty() {
        return Err(PreparationFailure::new(
            stage,
            CargoPreparationErrorKind::Stderr,
        ));
    }
    Ok(())
}

fn validate_build_stream_lengths(
    stdout_len: usize,
    stderr_len: usize,
) -> Result<(), PreparationFailure> {
    if stdout_len > MAX_BUILD_STDOUT_BYTES {
        return Err(PreparationFailure::new(
            PreparationStage::Build,
            CargoPreparationErrorKind::StdoutLimit,
        ));
    }
    if stderr_len > MAX_BUILD_STDERR_BYTES {
        return Err(PreparationFailure::new(
            PreparationStage::Build,
            CargoPreparationErrorKind::StderrLimit,
        ));
    }
    Ok(())
}

fn accept_last(receipts: &mut [CommandReceipt]) {
    receipts
        .last_mut()
        .expect("a completed command has a retained receipt")
        .accepted = true;
}

fn parse_pkgid(stdout: &[u8]) -> Result<String, PreparationFailure> {
    let text = std::str::from_utf8(stdout).map_err(|_| {
        PreparationFailure::new(
            PreparationStage::Pkgid,
            CargoPreparationErrorKind::PkgidEncoding,
        )
    })?;
    let body = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .ok_or_else(|| {
            PreparationFailure::new(
                PreparationStage::Pkgid,
                CargoPreparationErrorKind::PkgidShape,
            )
        })?;
    if body.is_empty()
        || body.contains(['\r', '\n'])
        || body.starts_with(' ')
        || body.ends_with(' ')
        || body.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(PreparationFailure::new(
            PreparationStage::Pkgid,
            CargoPreparationErrorKind::PkgidShape,
        ));
    }
    if body.len() > MAX_PKGID_BYTES {
        return Err(PreparationFailure::new(
            PreparationStage::Pkgid,
            CargoPreparationErrorKind::PkgidLimit,
        ));
    }
    Ok(body.to_owned())
}

fn parse_rustc_host(stdout: &[u8]) -> Result<String, PreparationFailure> {
    let text = std::str::from_utf8(stdout).map_err(|_| {
        PreparationFailure::new(
            PreparationStage::RustcVersion,
            CargoPreparationErrorKind::ToolMetadata(ToolMetadataField::RustcHost),
        )
    })?;
    let candidates: Vec<_> = text
        .lines()
        .filter(|line| line.starts_with("host"))
        .collect();
    let host = if candidates.len() == 1 {
        candidates[0].strip_prefix("host: ")
    } else {
        None
    };
    if host.is_none()
        || host.is_some_and(str::is_empty)
        || host.is_some_and(|host| host.len() > MAX_HOST_BYTES)
        || !host.is_some_and(|host| {
            host.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
    {
        return Err(PreparationFailure::new(
            PreparationStage::RustcVersion,
            CargoPreparationErrorKind::ToolMetadata(ToolMetadataField::RustcHost),
        ));
    }
    Ok(host.expect("validated host field").to_owned())
}

fn require_matching_llvm(
    stage: PreparationStage,
    observed: &[u8],
    marker: &str,
    label: &str,
) -> Result<(), PreparationFailure> {
    let expected = exact_version_after(label, "LLVM version: ").filter(|v| v.starts_with("22."));
    let actual = std::str::from_utf8(observed)
        .ok()
        .and_then(|text| exact_version_after(text, marker));
    if expected.is_none() || actual != expected {
        return Err(PreparationFailure::new(
            stage,
            CargoPreparationErrorKind::ToolMetadata(ToolMetadataField::LlvmVersion),
        ));
    }
    Ok(())
}

fn exact_version_after(text: &str, marker: &str) -> Option<String> {
    let mut matches = text.match_indices(marker);
    let (_, tail) = matches
        .next()
        .map(|(index, _)| (index, &text[index + marker.len()..]))?;
    if matches.next().is_some() {
        return None;
    }
    let version: String = tail
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    (version.starts_with("22.")
        && version.split('.').count() == 3
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())))
    .then_some(version)
}

fn recheck_preflight(
    preflight: &Preflight,
    check: SelectedArtifactCheck,
) -> Result<(), PreparationFailure> {
    if check == SelectedArtifactCheck::Metadata {
        recheck_roots(&preflight.roots)?;
        for (file, subject) in [
            (&preflight.manifest, EvidenceSubject::Manifest),
            (&preflight.lockfile, EvidenceSubject::Lockfile),
        ] {
            recheck_frozen(file, subject, check)?;
        }
        preflight
            .source_inventory()
            .recheck()
            .map_err(inventory_drift_failure)?;
        for (file, subject) in [
            (&preflight.cargo, EvidenceSubject::Cargo),
            (&preflight.rustc, EvidenceSubject::Rustc),
            (&preflight.llvm_profdata, EvidenceSubject::LlvmProfdata),
            (&preflight.llvm_cov, EvidenceSubject::LlvmCov),
        ] {
            recheck_tool(file, subject, check)?;
        }
        return Ok(());
    }

    let mut primary = None;
    aggregate_check(&mut primary, recheck_roots(&preflight.roots));
    for (file, subject) in [
        (&preflight.manifest, EvidenceSubject::Manifest),
        (&preflight.lockfile, EvidenceSubject::Lockfile),
    ] {
        aggregate_check(&mut primary, recheck_frozen(file, subject, check));
    }
    aggregate_check(
        &mut primary,
        preflight
            .source_inventory()
            .recheck()
            .map_err(inventory_drift_failure),
    );
    for (file, subject) in [
        (&preflight.cargo, EvidenceSubject::Cargo),
        (&preflight.rustc, EvidenceSubject::Rustc),
        (&preflight.llvm_profdata, EvidenceSubject::LlvmProfdata),
        (&preflight.llvm_cov, EvidenceSubject::LlvmCov),
    ] {
        aggregate_check(&mut primary, recheck_tool(file, subject, check));
    }
    primary.map_or(Ok(()), Err)
}

fn recheck_bootstrap(bootstrap: &CargoPreparationBootstrap) -> Result<(), PreparationFailure> {
    recheck_roots(&bootstrap.roots)?;
    recheck_frozen_file(&bootstrap.manifest, EvidenceSubject::Manifest)?;
    recheck_frozen_file(&bootstrap.lockfile, EvidenceSubject::Lockfile)
}

fn initial_inventory_failure(error: InventoryError) -> PreparationFailure {
    PreparationFailure::new(
        PreparationStage::Preflight,
        CargoPreparationErrorKind::SourceInventory(error),
    )
}

fn inventory_drift_failure(error: InventoryError) -> PreparationFailure {
    let mut failure = PreparationFailure::new(
        PreparationStage::Preflight,
        CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::SourceInventory),
    );
    failure.inventory_error = Some(error);
    failure
}

fn recheck_roots(roots: &CanonicalArtifactRoots) -> Result<(), PreparationFailure> {
    let observed =
        establish_artifact_roots(&roots.canonical_source_root, &roots.canonical_target_dir)
            .map_err(|error| {
                PreparationFailure::new(
                    PreparationStage::Preflight,
                    CargoPreparationErrorKind::Artifact(error),
                )
            })?;
    if observed.canonical_source_root != roots.canonical_source_root
        || observed.canonical_src_path != roots.canonical_src_path
    {
        return Err(PreparationFailure::new(
            PreparationStage::Preflight,
            CargoPreparationErrorKind::Artifact(ArtifactError::Path {
                field: crate::cargo_artifact::ArtifactPathField::SourceRoot,
                kind: crate::cargo_artifact::ArtifactPathError::CanonicalMismatch,
            }),
        ));
    }
    if observed.canonical_target_dir != roots.canonical_target_dir {
        return Err(PreparationFailure::new(
            PreparationStage::Preflight,
            CargoPreparationErrorKind::Artifact(ArtifactError::Path {
                field: crate::cargo_artifact::ArtifactPathField::TargetDir,
                kind: crate::cargo_artifact::ArtifactPathError::CanonicalMismatch,
            }),
        ));
    }
    Ok(())
}

fn recheck_receipt(
    receipt: &CommandReceipt,
    check: SelectedArtifactCheck,
) -> Result<(), PreparationFailure> {
    for (path, stamp, identity, subject) in [
        (
            &receipt.stdout_path,
            receipt
                .stdout_stamp
                .as_ref()
                .expect("retained receipt has a stdout stamp"),
            receipt.stdout_identity,
            if receipt.stage == PreparationStage::Build && receipt.accepted {
                EvidenceSubject::Jsonl
            } else {
                EvidenceSubject::CommandOutput
            },
        ),
        (
            &receipt.stderr_path,
            receipt
                .stderr_stamp
                .as_ref()
                .expect("retained receipt has a stderr stamp"),
            receipt.stderr_identity,
            if receipt.stage == PreparationStage::Build {
                EvidenceSubject::BuildStderr
            } else {
                EvidenceSubject::CommandOutput
            },
        ),
    ] {
        let frozen = FrozenFile {
            supplied_path: path.clone(),
            canonical_path: path.clone(),
            stamp: stamp.clone(),
            identity,
        };
        if subject == EvidenceSubject::Jsonl {
            recheck_jsonl_inner(&frozen)?;
        } else if check == SelectedArtifactCheck::Metadata {
            recheck_frozen_metadata(&frozen, subject)?;
        } else {
            recheck_frozen_file(&frozen, subject)?;
        }
    }
    Ok(())
}

fn recheck_receipts(
    receipts: &[CommandReceipt],
    check: SelectedArtifactCheck,
) -> Result<(), PreparationFailure> {
    for receipt in receipts {
        recheck_receipt(receipt, check)?;
    }
    Ok(())
}

fn recheck_prepared(
    prepared: &PreparedCargoTarget,
    check: SelectedArtifactCheck,
) -> Result<(), PreparationFailure> {
    let mut primary = None;
    aggregate_check(&mut primary, recheck_roots(&prepared.roots));
    for (file, subject) in [
        (&prepared.manifest, EvidenceSubject::Manifest),
        (&prepared.lockfile, EvidenceSubject::Lockfile),
    ] {
        aggregate_check(&mut primary, recheck_frozen(file, subject, check));
    }
    aggregate_check(
        &mut primary,
        prepared
            .source_inventory
            .recheck()
            .map_err(inventory_drift_failure),
    );
    for (file, subject) in [
        (&prepared.cargo, EvidenceSubject::Cargo),
        (&prepared.rustc, EvidenceSubject::Rustc),
        (&prepared.llvm_profdata, EvidenceSubject::LlvmProfdata),
        (&prepared.llvm_cov, EvidenceSubject::LlvmCov),
    ] {
        aggregate_check(&mut primary, recheck_tool(file, subject, check));
    }
    for receipt in &prepared.receipts {
        aggregate_check(&mut primary, recheck_receipt(receipt, check));
    }
    aggregate_check(
        &mut primary,
        recheck_selected_artifact(&prepared.selected, &prepared.expected, check).map_err(|error| {
            PreparationFailure::new(
                PreparationStage::Resolve,
                CargoPreparationErrorKind::Artifact(error),
            )
        }),
    );
    primary.map_or(Ok(()), Err)
}

fn aggregate_check(
    primary: &mut Option<PreparationFailure>,
    result: Result<(), PreparationFailure>,
) {
    let Err(mut failure) = result else {
        return;
    };
    if let Some(primary) = primary {
        primary.secondary.push(CargoPreparationDiagnostic {
            kind: failure.kind,
            inventory_error: failure.inventory_error,
            io_error: failure.io_error,
        });
        primary.secondary.append(&mut failure.secondary);
    } else {
        *primary = Some(failure);
    }
}

fn freeze_strict_file(
    path: &Path,
    subject: EvidenceSubject,
) -> Result<FrozenFile, PreparationFailure> {
    validate_lexical(path)?;
    walk_nonsymlink_file(path, subject)?;
    let canonical_path = fs::canonicalize(path).map_err(|_| metadata_failure(subject))?;
    if canonical_path != path {
        return Err(metadata_failure(subject));
    }
    Ok(FrozenFile {
        supplied_path: path.to_path_buf(),
        canonical_path,
        stamp: file_stamp(path).map_err(|error| evidence_io(PreparationStage::Preflight, error))?,
        identity: file_identity(path)
            .map_err(|error| evidence_io(PreparationStage::Preflight, error))?,
    })
}

fn freeze_tool_file(
    path: &Path,
    subject: EvidenceSubject,
) -> Result<FrozenFile, PreparationFailure> {
    validate_lexical(path)?;
    let canonical_path = fs::canonicalize(path).map_err(|_| metadata_failure(subject))?;
    validate_lexical(&canonical_path)?;
    walk_nonsymlink_file(&canonical_path, subject)?;
    Ok(FrozenFile {
        supplied_path: path.to_path_buf(),
        stamp: file_stamp(&canonical_path)
            .map_err(|error| evidence_io(PreparationStage::Preflight, error))?,
        identity: file_identity(&canonical_path)
            .map_err(|error| evidence_io(PreparationStage::Preflight, error))?,
        canonical_path,
    })
}

fn frozen_receipt_file(
    path: &Path,
    stamp: Option<&FileStamp>,
    identity: ContentIdentity,
) -> Result<FrozenFile, PreparationFailure> {
    let stamp = stamp.cloned().ok_or_else(|| {
        PreparationFailure::new(
            PreparationStage::Jsonl,
            CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::Jsonl),
        )
    })?;
    validate_lexical(path)?;
    Ok(FrozenFile {
        supplied_path: path.to_path_buf(),
        canonical_path: path.to_path_buf(),
        stamp,
        identity,
    })
}

fn recheck_frozen(
    frozen: &FrozenFile,
    subject: EvidenceSubject,
    check: SelectedArtifactCheck,
) -> Result<(), PreparationFailure> {
    match check {
        SelectedArtifactCheck::Metadata => recheck_frozen_metadata(frozen, subject),
        SelectedArtifactCheck::Content => recheck_frozen_file(frozen, subject),
    }
}

fn recheck_tool(
    frozen: &FrozenFile,
    subject: EvidenceSubject,
    check: SelectedArtifactCheck,
) -> Result<(), PreparationFailure> {
    match check {
        SelectedArtifactCheck::Metadata => recheck_tool_metadata(frozen, subject),
        SelectedArtifactCheck::Content => recheck_frozen_tool(frozen, subject),
    }
}

fn recheck_frozen_metadata(
    frozen: &FrozenFile,
    subject: EvidenceSubject,
) -> Result<(), PreparationFailure> {
    walk_nonsymlink_file(&frozen.supplied_path, subject).map_err(|failure| {
        if matches!(failure.kind, CargoPreparationErrorKind::EvidenceIo(_)) {
            failure
        } else {
            identity_changed(subject)
        }
    })?;
    let canonical = fs::canonicalize(&frozen.supplied_path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            identity_changed(subject)
        } else {
            evidence_io(PreparationStage::Preflight, error)
        }
    })?;
    let stamp = file_stamp(&frozen.supplied_path)
        .map_err(|error| evidence_io(PreparationStage::Preflight, error))?;
    if canonical != frozen.canonical_path || stamp != frozen.stamp {
        return Err(identity_changed(subject));
    }
    Ok(())
}

fn recheck_tool_metadata(
    frozen: &FrozenFile,
    subject: EvidenceSubject,
) -> Result<(), PreparationFailure> {
    let canonical = fs::canonicalize(&frozen.supplied_path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            identity_changed(subject)
        } else {
            evidence_io(PreparationStage::Preflight, error)
        }
    })?;
    if canonical != frozen.canonical_path {
        return Err(identity_changed(subject));
    }
    walk_nonsymlink_file(&canonical, subject).map_err(|failure| {
        if matches!(failure.kind, CargoPreparationErrorKind::EvidenceIo(_)) {
            failure
        } else {
            identity_changed(subject)
        }
    })?;
    if file_stamp(&canonical).map_err(|error| evidence_io(PreparationStage::Preflight, error))?
        != frozen.stamp
    {
        return Err(identity_changed(subject));
    }
    Ok(())
}

fn recheck_frozen_file(
    frozen: &FrozenFile,
    subject: EvidenceSubject,
) -> Result<(), PreparationFailure> {
    walk_nonsymlink_file(&frozen.supplied_path, subject).map_err(|failure| {
        if matches!(failure.kind, CargoPreparationErrorKind::EvidenceIo(_)) {
            failure
        } else {
            identity_changed(subject)
        }
    })?;
    let canonical = fs::canonicalize(&frozen.supplied_path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            identity_changed(subject)
        } else {
            evidence_io(PreparationStage::Preflight, error)
        }
    })?;
    if canonical != frozen.canonical_path
        || file_stamp(&frozen.supplied_path)
            .map_err(|error| evidence_io(PreparationStage::Preflight, error))?
            != frozen.stamp
        || file_identity(&frozen.supplied_path)
            .map_err(|error| evidence_io(PreparationStage::Preflight, error))?
            != frozen.identity
    {
        return Err(identity_changed(subject));
    }
    Ok(())
}

fn recheck_frozen_tool(
    frozen: &FrozenFile,
    subject: EvidenceSubject,
) -> Result<(), PreparationFailure> {
    let canonical = fs::canonicalize(&frozen.supplied_path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            identity_changed(subject)
        } else {
            evidence_io(PreparationStage::Preflight, error)
        }
    })?;
    if canonical != frozen.canonical_path {
        return Err(identity_changed(subject));
    }
    walk_nonsymlink_file(&canonical, subject).map_err(|failure| {
        if matches!(failure.kind, CargoPreparationErrorKind::EvidenceIo(_)) {
            failure
        } else {
            identity_changed(subject)
        }
    })?;
    if file_stamp(&canonical).map_err(|error| evidence_io(PreparationStage::Preflight, error))?
        != frozen.stamp
        || file_identity(&canonical)
            .map_err(|error| evidence_io(PreparationStage::Preflight, error))?
            != frozen.identity
    {
        return Err(identity_changed(subject));
    }
    Ok(())
}

fn recheck_jsonl_inner(frozen: &FrozenFile) -> Result<(), PreparationFailure> {
    walk_nonsymlink_file(&frozen.supplied_path, EvidenceSubject::Jsonl).map_err(|failure| {
        if matches!(failure.kind, CargoPreparationErrorKind::EvidenceIo(_)) {
            failure
        } else {
            identity_changed(EvidenceSubject::Jsonl)
        }
    })?;
    let canonical = fs::canonicalize(&frozen.supplied_path)
        .map_err(|_| identity_changed(EvidenceSubject::Jsonl))?;
    if canonical != frozen.canonical_path {
        return Err(identity_changed(EvidenceSubject::Jsonl));
    }
    let pre = file_stamp(&frozen.supplied_path)
        .map_err(|error| evidence_io(PreparationStage::Jsonl, error))?;
    if pre != frozen.stamp {
        return Err(identity_changed(EvidenceSubject::Jsonl));
    }
    let mut file = File::open(&frozen.supplied_path)
        .map_err(|error| evidence_io(PreparationStage::Jsonl, error))?;
    let handle_pre = FileStamp::from_metadata(
        &file
            .metadata()
            .map_err(|error| evidence_io(PreparationStage::Jsonl, error))?,
    )
    .map_err(|error| evidence_io(PreparationStage::Jsonl, error))?;
    if handle_pre != frozen.stamp {
        return Err(identity_changed(EvidenceSubject::Jsonl));
    }
    let identity = file_identity_from_file_bounded(&mut file, frozen.identity.1).map_err(
        |error| match error {
            FileIdentityError::Io(error) => evidence_io(PreparationStage::Jsonl, error),
            FileIdentityError::LengthOverflow => identity_changed(EvidenceSubject::Jsonl),
        },
    )?;
    let handle_post = FileStamp::from_metadata(
        &file
            .metadata()
            .map_err(|error| evidence_io(PreparationStage::Jsonl, error))?,
    )
    .map_err(|error| evidence_io(PreparationStage::Jsonl, error))?;
    if handle_post != frozen.stamp || identity != frozen.identity {
        return Err(identity_changed(EvidenceSubject::Jsonl));
    }
    jsonl_post_read_checkpoint(frozen);
    walk_nonsymlink_file(&frozen.supplied_path, EvidenceSubject::Jsonl).map_err(|failure| {
        if matches!(failure.kind, CargoPreparationErrorKind::EvidenceIo(_)) {
            failure
        } else {
            identity_changed(EvidenceSubject::Jsonl)
        }
    })?;
    let post = file_stamp(&frozen.supplied_path)
        .map_err(|error| evidence_io(PreparationStage::Jsonl, error))?;
    let post_canonical = fs::canonicalize(&frozen.supplied_path)
        .map_err(|_| identity_changed(EvidenceSubject::Jsonl))?;
    if post != frozen.stamp || post_canonical != frozen.canonical_path {
        return Err(identity_changed(EvidenceSubject::Jsonl));
    }
    Ok(())
}

fn walk_nonsymlink_file(path: &Path, subject: EvidenceSubject) -> Result<(), PreparationFailure> {
    let mut current = PathBuf::new();
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                current.push(component.as_os_str());
            }
            Component::CurDir | Component::ParentDir => return Err(metadata_failure(subject)),
        }
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                metadata_failure(subject)
            } else {
                evidence_io(PreparationStage::Preflight, error)
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(metadata_failure(subject));
        }
        if components.peek().is_some() {
            if !metadata.is_dir() {
                return Err(metadata_failure(subject));
            }
        } else if !metadata.is_file() {
            return Err(metadata_failure(subject));
        }
    }
    Ok(())
}

fn validate_lexical(path: &Path) -> Result<(), PreparationFailure> {
    let text = path.to_str().ok_or_else(|| {
        PreparationFailure::new(
            PreparationStage::Preflight,
            CargoPreparationErrorKind::Configuration,
        )
    })?;
    if text.is_empty()
        || text.len() > MAX_PATH_BYTES
        || !path.is_absolute()
        || text.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        || lexical_component(text, ".")
        || lexical_component(text, "..")
    {
        return Err(PreparationFailure::new(
            PreparationStage::Preflight,
            CargoPreparationErrorKind::Configuration,
        ));
    }
    Ok(())
}

fn lexical_component(path: &str, component: &str) -> bool {
    #[cfg(windows)]
    {
        path.split(['/', '\\']).any(|part| part == component)
    }
    #[cfg(not(windows))]
    {
        path.split('/').any(|part| part == component)
    }
}

fn path_text(path: &Path) -> Result<&str, PreparationFailure> {
    path.to_str().ok_or_else(|| {
        PreparationFailure::new(
            PreparationStage::Preflight,
            CargoPreparationErrorKind::Configuration,
        )
    })
}

fn metadata_failure(subject: EvidenceSubject) -> PreparationFailure {
    let field = match subject {
        EvidenceSubject::Manifest => ToolMetadataField::Manifest,
        EvidenceSubject::Lockfile => ToolMetadataField::Lockfile,
        EvidenceSubject::SourceInventory => ToolMetadataField::Source,
        EvidenceSubject::Cargo => ToolMetadataField::Cargo,
        EvidenceSubject::Rustc => ToolMetadataField::Rustc,
        EvidenceSubject::LlvmProfdata => ToolMetadataField::LlvmProfdata,
        EvidenceSubject::LlvmCov => ToolMetadataField::LlvmCov,
        EvidenceSubject::OwnerMarker => ToolMetadataField::OwnerMarker,
        EvidenceSubject::Jsonl | EvidenceSubject::BuildStderr | EvidenceSubject::CommandOutput => {
            ToolMetadataField::CommandOutput
        }
    };
    PreparationFailure::new(
        PreparationStage::Preflight,
        CargoPreparationErrorKind::ToolMetadata(field),
    )
}

fn identity_changed(subject: EvidenceSubject) -> PreparationFailure {
    PreparationFailure::new(
        if subject == EvidenceSubject::Jsonl {
            PreparationStage::Jsonl
        } else {
            PreparationStage::Preflight
        },
        CargoPreparationErrorKind::IdentityChanged(subject),
    )
}

fn evidence_io(stage: PreparationStage, error: io::Error) -> PreparationFailure {
    let kind = error.kind();
    let mut failure = PreparationFailure::new(stage, CargoPreparationErrorKind::EvidenceIo(kind));
    failure.io_error = Some(Arc::new(error));
    failure
}

fn retention_io(stage: PreparationStage, error: io::Error) -> PreparationFailure {
    PreparationFailure {
        stage,
        kind: CargoPreparationErrorKind::EvidenceIo(error.kind()),
        capture_complete: true,
        retention_complete: false,
        secondary: Vec::new(),
        inventory_error: None,
        io_error: Some(Arc::new(error)),
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn persist_new(path: &Path, bytes: &[u8]) -> io::Result<(FileStamp, ContentIdentity)> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    let before = FileStamp::from_metadata(&file.metadata()?)?;
    file.seek(SeekFrom::Start(0))?;
    let identity =
        file_identity_from_file_bounded(&mut file, bytes.len()).map_err(|error| match error {
            FileIdentityError::Io(error) => error,
            FileIdentityError::LengthOverflow => io::Error::other("persisted file grew"),
        })?;
    let after = FileStamp::from_metadata(&file.metadata()?)?;
    if before != after || before.len != bytes.len() as u64 {
        return Err(io::Error::other(
            "persisted file changed during verification",
        ));
    }
    Ok((after, identity))
}

fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>, PreparationFailure> {
    let limit = maximum.checked_add(1).ok_or_else(|| {
        PreparationFailure::new(
            PreparationStage::Jsonl,
            CargoPreparationErrorKind::StdoutLimit,
        )
    })?;
    let mut file = File::open(path).map_err(|error| evidence_io(PreparationStage::Jsonl, error))?;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    while bytes.len() < limit {
        let available = (limit - bytes.len()).min(buffer.len());
        let read = file
            .read(&mut buffer[..available])
            .map_err(|error| evidence_io(PreparationStage::Jsonl, error))?;
        if read == 0 {
            return Ok(bytes);
        }
        bytes
            .try_reserve(read)
            .map_err(|error| evidence_io(PreparationStage::Jsonl, io::Error::other(error)))?;
        bytes.extend_from_slice(&buffer[..read]);
    }
    Err(PreparationFailure::new(
        PreparationStage::Jsonl,
        CargoPreparationErrorKind::StdoutLimit,
    ))
}

fn stage_name(stage: PreparationStage) -> &'static str {
    match stage {
        PreparationStage::Preflight => "preflight",
        PreparationStage::LlvmProfdataVersion => "llvm-profdata-version",
        PreparationStage::LlvmCovVersion => "llvm-cov-version",
        PreparationStage::CargoVersion => "cargo-version",
        PreparationStage::RustcVersion => "rustc-version",
        PreparationStage::Pkgid => "pkgid",
        PreparationStage::Build => "build",
        PreparationStage::Jsonl => "jsonl",
        PreparationStage::Resolve => "resolve",
        PreparationStage::Publish => "publish",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{LIBTEST_EXACT_V1_ARGS, parse_libtest_exact_v1_stdout};
    use std::ffi::OsString;
    use std::io::Read;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        source: PathBuf,
        target: PathBuf,
        output: PathBuf,
        cargo: PathBuf,
        rustc: PathBuf,
        profdata: PathBuf,
        cov: PathBuf,
    }

    impl Fixture {
        fn new(cargo_mode: &str, rustc_mode: &str) -> Self {
            let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("yaxml-cargo-build-{}-{serial}", std::process::id()));
            fs::create_dir(&root).unwrap();
            let root = fs::canonicalize(root).unwrap();
            let source = root.join("source with space é 🙂");
            let target = root.join("target with space é 🙂");
            let output = root.join("output");
            fs::create_dir(&source).unwrap();
            fs::create_dir(source.join("src")).unwrap();
            fs::create_dir(&target).unwrap();
            write_package(&source, fixture_library());

            let helper_source = root.join("fake_command.rs");
            fs::write(&helper_source, fake_command_source()).unwrap();
            let helper = root.join("fake-command");
            let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
            let compilation = std::process::Command::new(compiler)
                .arg("--edition=2024")
                .arg(&helper_source)
                .arg("-o")
                .arg(&helper)
                .output()
                .unwrap();
            assert!(
                compilation.status.success(),
                "fake helper compilation failed: {}",
                String::from_utf8_lossy(&compilation.stderr)
            );
            let install = |name: &str| {
                let path = root.join(name);
                fs::copy(&helper, &path).unwrap();
                path
            };
            let cargo = install(cargo_mode);
            let rustc = install(rustc_mode);
            let profdata = install("profdata-ok");
            let cov = install("cov-ok");
            Self {
                root,
                source,
                target,
                output,
                cargo,
                rustc,
                profdata,
                cov,
            }
        }

        fn bootstrap_request(&self) -> CargoBootstrapRequest<'_> {
            CargoBootstrapRequest {
                source_root: &self.source,
                target_dir: &self.target,
                output_dir: &self.output,
                compiler_version_label: "rustc fixture LLVM version: 22.1.6",
                limits: CargoPreparationLimits {
                    timeout: Duration::from_secs(5),
                    max_build_output_bytes: MAX_BUILD_OUTPUT_BYTES,
                },
            }
        }

        fn request(&self) -> CargoPreparationRequest<'_> {
            let bootstrap = begin_cargo_preparation(&self.bootstrap_request()).unwrap();
            let source_inventory = capture_preparation_inventory(&bootstrap).unwrap();
            CargoPreparationRequest {
                bootstrap,
                source_inventory,
                cargo: &self.cargo,
                rustc: &self.rustc,
                llvm_profdata: &self.profdata,
                llvm_cov: &self.cov,
                profile: CargoProfile::Debug,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let metadata = fs::symlink_metadata(&self.root).unwrap();
            assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
            fs::remove_dir_all(&self.root).unwrap();
        }
    }

    fn fixture_library() -> &'static str {
        "pub fn fixture() -> bool { true }\n"
    }

    fn actual_fixture_library() -> &'static str {
        r#"pub mod reader {
    #[cfg(test)]
    pub mod dtd_fuzz_tests {
        #[test]
        #[ignore = "private stdin replay entry"]
        fn replay_stdin() {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::io::stdin().read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, b"known fixture payload\n");
        }
    }
}
"#
    }

    fn write_package(source: &Path, library: &str) {
        const TEST_MASTER: [&str; 32] = [
            "src/buffered.rs",
            "src/dom.rs",
            "src/dtd.rs",
            "src/dtd_catalog.rs",
            "src/dtd_controller.rs",
            "src/dtd_controller/content.rs",
            "src/dtd_frames.rs",
            "src/dtd_reference.rs",
            "src/dtd_scan.rs",
            "src/dtd_tag.rs",
            "src/dtd_writer.rs",
            "src/encoding.rs",
            "src/error.rs",
            "src/escape.rs",
            "src/lib.rs",
            "src/name.rs",
            "src/origin.rs",
            "src/owned.rs",
            "src/owned/construction.rs",
            "src/parser.rs",
            "src/push.rs",
            "src/reader.rs",
            "src/reader/dtd.rs",
            "src/serialize.rs",
            "src/stream.rs",
            "src/stream/dtd.rs",
            "src/stream/dtd_buffered.rs",
            "src/tokenizer.rs",
            "src/tokenizer/local.rs",
            "src/tokenizer/reference.rs",
            "src/tokenizer/text.rs",
            "src/writer.rs",
        ];
        fs::write(
            source.join("Cargo.toml"),
            "[package]\nname = \"yaxml\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
        )
        .unwrap();
        fs::write(
            source.join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"yaxml\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        for relative in TEST_MASTER {
            let path = source.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                &path,
                if relative == "src/lib.rs" {
                    library.as_bytes()
                } else {
                    b"// independent fixture source\n"
                },
            )
            .unwrap();
        }
    }

    struct CheckpointHookReset;

    impl Drop for CheckpointHookReset {
        fn drop(&mut self) {
            PREPARATION_CHECKPOINT_HOOK.with(|hook| *hook.borrow_mut() = None);
            JSONL_CHECKPOINT_HOOK.with(|hook| *hook.borrow_mut() = None);
        }
    }

    fn with_jsonl_checkpoint_hook<T>(
        hook: impl FnMut(&FrozenFile) + 'static,
        action: impl FnOnce() -> T,
    ) -> T {
        JSONL_CHECKPOINT_HOOK.with(|slot| {
            assert!(slot.borrow().is_none());
            *slot.borrow_mut() = Some(Box::new(hook));
        });
        let reset = CheckpointHookReset;
        let result = action();
        drop(reset);
        result
    }

    fn with_checkpoint_hook<T>(
        hook: impl FnMut(PreparationCheckpoint, &OwnedPreparationDirectory, &Preflight) + 'static,
        action: impl FnOnce() -> T,
    ) -> T {
        PREPARATION_CHECKPOINT_HOOK.with(|slot| {
            assert!(slot.borrow().is_none());
            *slot.borrow_mut() = Some(Box::new(hook));
        });
        let reset = CheckpointHookReset;
        let result = action();
        drop(reset);
        result
    }

    fn rewrite_preserving_stamp(path: &Path, bytes: &[u8]) {
        let stamp = file_stamp(path).unwrap();
        fs::write(path, bytes).unwrap();
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(stamp.modified))
            .unwrap();
    }

    fn fake_command_source() -> &'static str {
        r#"use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::thread;
use std::time::Duration;

fn main() {
    let mode = env::args_os().next().and_then(|path| Path::new(&path).file_name().map(|v| v.to_string_lossy().into_owned())).unwrap();
    let args: Vec<String> = env::args().skip(1).collect();
    let cwd = env::current_dir().unwrap();
    let mut stdin = Vec::new();
    std::io::stdin().read_to_end(&mut stdin).unwrap();
    assert!(Path::new(&env::var("RUSTC").unwrap()).is_absolute());
    assert!(Path::new(&env::var("CARGO_TARGET_DIR").unwrap()).is_absolute());
    for name in ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "XML_CATALOG_FILES", "SGML_CATALOG_FILES"] {
        assert_eq!(env::var(name).unwrap(), "");
    }
    for name in ["CARGO_ENCODED_RUSTFLAGS", "CARGO_LOG", "CARGO_TERM_VERBOSE", "CARGO_TERM_QUIET", "LLVM_PROFILE_FILE", "LLVM_PROFILE_PREFIX"] {
        assert!(env::var_os(name).is_none());
    }
    assert_eq!(env::var("CARGO_INCREMENTAL").unwrap(), "0");
    assert_eq!(env::var("RUSTFLAGS").unwrap(), "-C instrument-coverage -Z coverage-options=branch");
    assert_eq!(env::var("CARGO_PROFILE_RELEASE_LTO").unwrap(), "false");
    assert_eq!(env::var("CARGO_PROFILE_RELEASE_CODEGEN_UNITS").unwrap(), "16");
    assert_eq!(env::var("CARGO_TERM_COLOR").unwrap(), "never");
    assert_eq!(env::var("CARGO_TERM_PROGRESS_WHEN").unwrap(), "never");
    assert_eq!(env::var("TERM").unwrap(), "dumb");
    let mut log = OpenOptions::new().create(true).append(true).open(cwd.join(".invocations")).unwrap();
    writeln!(log, "{}|{}|{}|{}|{}|{}", mode, args.join(" "), cwd.display(), env::var("RUSTFLAGS").unwrap_or_default(), env::var("CARGO_TARGET_DIR").unwrap_or_default(), stdin.len()).unwrap();
    if mode.starts_with("profdata") || mode.starts_with("cov") {
        println!("LLVM version 22.1.6-rust-fixture");
        return;
    }
    if mode.starts_with("rustc") {
        if mode == "rustc-bad-host" {
            print!("rustc fixture\nhost: bad host\nLLVM version: 22.1.6\n");
        } else {
            print!("rustc fixture\nhost: fixture-host\nLLVM version: 22.1.6\n");
        }
        return;
    }
    if args == ["-vV"] {
        if matches!(mode.as_str(), "cargo-receipt-collision" | "cargo-nonzero-receipt-collision") {
            let scratch = fs::read_dir(cwd.parent().unwrap()).unwrap().filter_map(Result::ok).find(|entry| entry.file_name().to_string_lossy().contains("cargo-preparation")).unwrap().path();
            fs::write(scratch.join("cargo-version.stdout"), b"collision").unwrap();
        }
        println!("cargo fixture");
        if mode == "cargo-nonzero-receipt-collision" { process::exit(7); }
        if mode == "cargo-version-mutate" { fs::write(cwd.join("src/lib.rs"), b"changed after cargo version\n").unwrap(); }
        #[cfg(unix)]
        if mode == "cargo-target-swap" {
            use std::os::unix::fs::symlink;
            let target = PathBuf::from(env::var_os("CARGO_TARGET_DIR").unwrap());
            let parked = target.with_extension("parked");
            fs::rename(&target, &parked).unwrap();
            symlink(&parked, &target).unwrap();
        }
        return;
    }
    if args.first().map(String::as_str) == Some("pkgid") {
        match mode.as_str() {
            "cargo-pkgid-multiline" => print!("path+file:///opaque%20source#yaxml@7.3.1\nextra\n"),
            "cargo-pkgid-no-lf" => print!("path+file:///opaque%20source#yaxml@7.3.1"),
            "cargo-pkgid-nonutf8" => std::io::stdout().write_all(&[0xff, b'\n']).unwrap(),
            "cargo-pkgid-stderr" => { println!("path+file:///opaque%20source#yaxml@7.3.1"); eprintln!("diagnostic"); },
            "cargo-pkgid-stderr-mutate" => { fs::write(cwd.join("src/lib.rs"), b"changed during pkgid\n").unwrap(); println!("path+file:///opaque%20source#yaxml@7.3.1"); eprintln!("diagnostic"); },
            _ => println!("path+file:///opaque%20source#yaxml@7.3.1"),
        }
        return;
    }
    if args.first().map(String::as_str) != Some("test") { process::exit(91); }
    match mode.as_str() {
        "cargo-timeout" => thread::sleep(Duration::from_secs(2)),
        "cargo-timeout-mutate" => { fs::write(cwd.join("src/lib.rs"), b"changed before timeout\n").unwrap(); thread::sleep(Duration::from_secs(2)); },
        "cargo-output" => { print!("{}", "x".repeat(200_000)); return; },
        "cargo-stdout-exact" => { padded_json(&cwd, &args, 8 * 1024 * 1024); return; },
        "cargo-stdout-limit" => { padded_json(&cwd, &args, 8 * 1024 * 1024 + 1); return; },
        "cargo-combined-exact" => { padded_json(&cwd, &args, 8 * 1024 * 1024); std::io::stderr().write_all(&vec![b'e'; 1024 * 1024]).unwrap(); return; },
        "cargo-combined-limit" => { padded_json(&cwd, &args, 8 * 1024 * 1024); std::io::stderr().write_all(&vec![b'e'; 1024 * 1024 + 1]).unwrap(); return; },
        "cargo-nonzero" => { perfect_json(&cwd, &args); process::exit(7); },
        "cargo-nonzero-stderr-limit" => { perfect_json(&cwd, &args); std::io::stderr().write_all(&vec![b'e'; 1024 * 1024 + 1]).unwrap(); process::exit(7); },
        "cargo-nonzero-mutate" => { fs::write(cwd.join("src/lib.rs"), b"changed source\n").unwrap(); perfect_json(&cwd, &args); process::exit(7); },
        "cargo-bad-json" => { print!("{{bad json}}\n"); return; },
        "cargo-bad-json-mutate" => { fs::write(cwd.join("src/lib.rs"), b"changed before bad json\n").unwrap(); print!("{{bad json}}\n"); return; },
        "cargo-false" => { print!("{{\"reason\":\"build-finished\",\"success\":false}}\n"); return; },
        "cargo-stderr-limit" => { perfect_json(&cwd, &args); std::io::stderr().write_all(&vec![b'e'; 1024 * 1024 + 1]).unwrap(); return; },
        _ => {}
    }
    perfect_json(&cwd, &args);
    if mode == "cargo-build-stderr" { eprintln!("permitted Cargo diagnostic"); }
    if mode == "cargo-publish-block" { fs::create_dir(cwd.parent().unwrap().join("output")).unwrap(); }
}

fn perfect_json(cwd: &Path, args: &[String]) {
    print!("{}", perfect_json_text(cwd, args));
}

fn perfect_json_text(cwd: &Path, args: &[String]) -> String {
    let target = PathBuf::from(args.iter().position(|arg| arg == "--target-dir").and_then(|i| args.get(i + 1)).unwrap());
    let executable = target.join("fixture-host/debug/deps/opaque-harness-481");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, b"fixture executable").unwrap();
    let source = cwd.join("src/lib.rs");
    format!("{{\"reason\":\"compiler-artifact\",\"package_id\":\"registry+https://example.invalid/index#dependency@9.8.7\",\"target\":{{\"kind\":[\"lib\"],\"crate_types\":[\"lib\"],\"name\":\"dependency\",\"src_path\":\"{}\",\"edition\":\"2024\",\"doc\":true,\"doctest\":true,\"test\":true}},\"profile\":{{\"test\":true}},\"executable\":null}}\n{{\"reason\":\"compiler-artifact\",\"package_id\":\"path+file:///opaque%20source#yaxml@7.3.1\",\"target\":{{\"kind\":[\"lib\"],\"crate_types\":[\"lib\"],\"name\":\"yaxml\",\"src_path\":\"{}\",\"edition\":\"2024\",\"doc\":true,\"doctest\":true,\"test\":true}},\"profile\":{{\"test\":true}},\"executable\":\"{}\"}}\n{{\"reason\":\"build-finished\",\"success\":true}}\n", escape(&source), escape(&source), escape(&executable))
}

fn padded_json(cwd: &Path, args: &[String], total: usize) {
    let suffix = perfect_json_text(cwd, args);
    let mut remaining = total.checked_sub(suffix.len()).unwrap();
    let mut stdout = std::io::stdout().lock();
    while remaining > 200_028 {
        write!(
            stdout,
            "{{\"reason\":\"other\",\"pad\":\"{}\"}}\n",
            "p".repeat(200_000)
        )
        .unwrap();
        remaining -= 200_028;
    }
    assert!(remaining >= 28);
    write!(
        stdout,
        "{{\"reason\":\"other\",\"pad\":\"{}\"}}\n",
        "p".repeat(remaining - 28)
    )
    .unwrap();
    stdout.write_all(suffix.as_bytes()).unwrap();
}

fn escape(path: &Path) -> String {
    path.to_str().unwrap().replace('\\', "\\\\").replace('"', "\\\"")
}
"#
    }

    #[test]
    fn pkgid_and_host_parsers_are_closed_and_bounded() {
        assert_eq!(
            parse_pkgid(b"opaque unicode \xe2\x98\x83\n").unwrap(),
            "opaque unicode ☃"
        );
        assert_eq!(parse_pkgid(b"opaque\r\n").unwrap(), "opaque");
        assert_eq!(
            parse_pkgid("opaque \u{1f642}\n".as_bytes()).unwrap(),
            "opaque \u{1f642}"
        );
        let mut exact = vec![b'x'; MAX_PKGID_BYTES];
        exact.push(b'\n');
        assert_eq!(parse_pkgid(&exact).unwrap().len(), MAX_PKGID_BYTES);
        for bytes in [
            &b""[..],
            &b"opaque"[..],
            &b" opaque\n"[..],
            &b"opaque \n"[..],
            &b"opaque\nextra\n"[..],
            &b"opaque\0\n"[..],
        ] {
            assert_eq!(
                parse_pkgid(bytes).unwrap_err().kind,
                CargoPreparationErrorKind::PkgidShape
            );
        }
        assert_eq!(
            parse_pkgid(&[0xff, b'\n']).unwrap_err().kind,
            CargoPreparationErrorKind::PkgidEncoding
        );
        let mut oversized = vec![b'x'; MAX_PKGID_BYTES + 1];
        oversized.push(b'\n');
        assert_eq!(
            parse_pkgid(&oversized).unwrap_err().kind,
            CargoPreparationErrorKind::PkgidLimit
        );
        assert_eq!(
            parse_pkgid(&vec![b'x'; MAX_PKGID_BYTES + 3])
                .unwrap_err()
                .kind,
            CargoPreparationErrorKind::PkgidShape
        );
        let mut oversized_invalid = vec![b'x'; MAX_PKGID_BYTES + 3];
        oversized_invalid[0] = 0xff;
        assert_eq!(
            parse_pkgid(&oversized_invalid).unwrap_err().kind,
            CargoPreparationErrorKind::PkgidEncoding
        );

        assert_eq!(
            parse_rustc_host(b"rustc\nhost: aarch64-apple-darwin\nLLVM version: 22.1.6\n").unwrap(),
            "aarch64-apple-darwin"
        );
        assert_eq!(
            parse_rustc_host(b"rustc\r\nhost: x86_64-pc-windows-msvc\r\n").unwrap(),
            "x86_64-pc-windows-msvc"
        );
        let maximum_host = format!("host: {}\n", "a".repeat(MAX_HOST_BYTES));
        assert_eq!(
            parse_rustc_host(maximum_host.as_bytes()).unwrap().len(),
            MAX_HOST_BYTES
        );
        let oversized_host = format!("host: {}\n", "a".repeat(MAX_HOST_BYTES + 1));
        assert_eq!(
            parse_rustc_host(oversized_host.as_bytes())
                .unwrap_err()
                .kind,
            CargoPreparationErrorKind::ToolMetadata(ToolMetadataField::RustcHost)
        );
        for bytes in [
            &b"rustc\n"[..],
            &b"host: one\nhost: two\n"[..],
            &b"host: bad host\n"[..],
            &b"host: bad/host\n"[..],
            &b"host: nonascii-\xc3\xa9\n"[..],
            &b"host: \n"[..],
            &b"host: valid\nhost:bad\n"[..],
            &b"host: valid\nhost:\tbad\n"[..],
            &b"host : invalid\n"[..],
        ] {
            assert_eq!(
                parse_rustc_host(bytes).unwrap_err().kind,
                CargoPreparationErrorKind::ToolMetadata(ToolMetadataField::RustcHost)
            );
        }
        assert!(
            require_matching_llvm(
                PreparationStage::RustcVersion,
                b"LLVM version: 22.1.6\nLLVM version: 22.1.6\n",
                "LLVM version: ",
                "label LLVM version: 22.1.6",
            )
            .is_err()
        );
        assert!(
            require_matching_llvm(
                PreparationStage::LlvmCovVersion,
                b"LLVM version 21.1.6\n",
                "LLVM version ",
                "label LLVM version: 22.1.6",
            )
            .is_err()
        );
    }

    #[test]
    fn prepares_one_artifact_with_exact_commands_and_owned_receipts() {
        let fixture = Fixture::new("cargo-ok", "rustc-ok");
        let prepared = prepare_cargo_target(fixture.request()).unwrap();
        assert_eq!(prepared.package_id(), prepared.package_id);
        assert_eq!(prepared.rustc_host(), prepared.rustc_host);
        assert_eq!(prepared.evidence_dir(), prepared.evidence_dir);
        assert_eq!(prepared.receipts(), prepared.receipts);
        assert_eq!(prepared.selected(), &prepared.selected);
        assert_eq!(prepared.expected(), &prepared.expected);
        assert_eq!(prepared.roots(), &prepared.roots);
        assert_eq!(prepared.profile(), CargoProfile::Debug);
        assert!(prepared.preparation_elapsed() <= Duration::from_secs(5));
        let jsonl_view = prepared.jsonl();
        assert_eq!(jsonl_view.supplied_path, prepared.jsonl.supplied_path);
        assert_eq!(jsonl_view.canonical_path, prepared.jsonl.canonical_path);
        assert_eq!(jsonl_view.stamp, &prepared.jsonl.stamp);
        assert_eq!(jsonl_view.identity, prepared.jsonl.identity);
        assert_eq!(
            prepared.build_stderr().identity,
            prepared.build_stderr.identity
        );
        assert_eq!(prepared.manifest().identity, prepared.manifest.identity);
        assert_eq!(prepared.lockfile().identity, prepared.lockfile.identity);
        assert_eq!(prepared.cargo().identity, prepared.cargo.identity);
        assert_eq!(prepared.rustc().identity, prepared.rustc.identity);
        assert_eq!(
            prepared.llvm_profdata().identity,
            prepared.llvm_profdata.identity
        );
        assert_eq!(prepared.llvm_cov().identity, prepared.llvm_cov.identity);
        assert_eq!(prepared.source_inventory().sources().len(), 32);
        assert_eq!(
            prepared.package_id,
            "path+file:///opaque%20source#yaxml@7.3.1"
        );
        assert_eq!(prepared.rustc_host, "fixture-host");
        assert_eq!(prepared.profile, CargoProfile::Debug);
        assert_eq!(prepared.receipts.len(), 6);
        assert_eq!(
            prepared
                .receipts
                .iter()
                .map(|receipt| receipt.stage)
                .collect::<Vec<_>>(),
            vec![
                PreparationStage::LlvmProfdataVersion,
                PreparationStage::LlvmCovVersion,
                PreparationStage::CargoVersion,
                PreparationStage::RustcVersion,
                PreparationStage::Pkgid,
                PreparationStage::Build,
            ]
        );
        assert_eq!(prepared.receipts[0].argv, ["--version"]);
        assert_eq!(prepared.receipts[1].argv, ["--version"]);
        assert_eq!(prepared.receipts[2].argv, ["-vV"]);
        assert_eq!(prepared.receipts[3].argv, ["-vV"]);
        assert_eq!(
            prepared.receipts[4].argv,
            [
                "pkgid",
                "--manifest-path",
                fixture.source.join("Cargo.toml").to_str().unwrap(),
                "--offline",
                "--locked",
                "--color",
                "never",
            ]
        );
        assert!(prepared.receipts.iter().all(|receipt| {
            receipt.capture_complete && receipt.retention_complete && receipt.accepted
        }));
        let receipt_view = prepared.receipts()[0].view();
        assert_eq!(receipt_view.stage, PreparationStage::LlvmProfdataVersion);
        assert_eq!(receipt_view.program, prepared.receipts[0].program);
        assert_eq!(
            receipt_view.canonical_program,
            prepared.receipts[0].canonical_program
        );
        assert_eq!(receipt_view.argv, prepared.receipts[0].argv);
        assert_eq!(receipt_view.cwd, prepared.receipts[0].cwd);
        assert_eq!(receipt_view.environment, prepared.receipts[0].environment);
        assert_eq!(
            receipt_view.removed_environment,
            prepared.receipts[0].removed_environment
        );
        assert_eq!(
            receipt_view.configuration_locations,
            prepared.receipts[0].configuration_locations
        );
        assert_eq!(receipt_view.timeout, prepared.receipts[0].timeout);
        assert_eq!(
            receipt_view.max_output_bytes,
            prepared.receipts[0].max_output_bytes
        );
        assert_eq!(receipt_view.elapsed, prepared.receipts[0].elapsed);
        assert_eq!(receipt_view.status, &prepared.receipts[0].status);
        assert_eq!(receipt_view.stdout_len, prepared.receipts[0].stdout_len);
        assert_eq!(receipt_view.stderr_len, prepared.receipts[0].stderr_len);
        assert!(receipt_view.stdout.is_some() && receipt_view.stderr.is_some());
        assert!(receipt_view.capture_complete && receipt_view.retention_complete);
        assert!(receipt_view.accepted);
        assert_eq!(
            prepared
                .receipts
                .iter()
                .map(|receipt| receipt.max_output_bytes)
                .collect::<Vec<_>>(),
            [
                MAX_VERSION_OUTPUT_BYTES,
                MAX_VERSION_OUTPUT_BYTES,
                MAX_VERSION_OUTPUT_BYTES,
                MAX_VERSION_OUTPUT_BYTES,
                MAX_PKGID_OUTPUT_BYTES,
                MAX_BUILD_OUTPUT_BYTES,
            ]
        );
        for receipt in &prepared.receipts {
            assert!(!receipt.timeout.is_zero());
            assert!(receipt.timeout <= Duration::from_secs(5));
            assert!(receipt.elapsed <= prepared.preparation_elapsed);
            assert!(receipt.status.success());
            assert_eq!(
                receipt.stdout_len as u64,
                fs::metadata(&receipt.stdout_path).unwrap().len()
            );
            assert_eq!(
                receipt.stdout_identity,
                file_identity(&receipt.stdout_path).unwrap()
            );
            assert_eq!(
                receipt.stderr_identity,
                file_identity(&receipt.stderr_path).unwrap()
            );
            assert!(receipt.program.is_file());
        }
        let build = prepared.receipts.last().unwrap();
        assert_eq!(
            &build.argv[..4],
            [
                "test",
                "--lib",
                "--no-run",
                "--message-format=json-render-diagnostics"
            ]
        );
        assert!(
            build
                .argv
                .windows(2)
                .any(|pair| pair == ["--target", "fixture-host"])
        );
        assert_eq!(
            build.argv,
            [
                "test",
                "--lib",
                "--no-run",
                "--message-format=json-render-diagnostics",
                "--manifest-path",
                fixture.source.join("Cargo.toml").to_str().unwrap(),
                "--target-dir",
                fixture.target.to_str().unwrap(),
                "--target",
                "fixture-host",
                "--offline",
                "--locked",
                "--color",
                "never",
            ]
        );
        assert_eq!(build.cwd, fixture.source);
        assert_eq!(
            build
                .environment
                .iter()
                .find(|(name, _)| name == "RUSTFLAGS")
                .unwrap()
                .1,
            "-C instrument-coverage -Z coverage-options=branch"
        );
        assert_eq!(build.removed_environment, REMOVED_ENVIRONMENT);
        assert_eq!(
            build
                .configuration_locations
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["source .cargo", "CARGO_HOME", "RUSTUP_HOME", "HOME/.cargo"]
        );
        assert!(prepared.evidence_dir.ends_with("output/build"));
        assert!(prepared.jsonl.supplied_path.exists());
        assert!(prepared.build_stderr.supplied_path.exists());
        assert_eq!(prepared.selected.package_id, prepared.expected.package_id);
        let selected = prepared.selected.clone();
        let expected = prepared.expected.clone();
        prepared.recheck(SelectedArtifactCheck::Metadata).unwrap();
        prepared.recheck(SelectedArtifactCheck::Content).unwrap();
        assert_eq!(prepared.selected, selected);
        assert_eq!(prepared.expected, expected);
        assert!(prepared.preparation_elapsed <= Duration::from_secs(5));
        assert_eq!(prepared.roots.canonical_source_root, fixture.source);
        assert_eq!(prepared.source_inventory().source_root(), fixture.source);
        assert_eq!(
            prepared.source_inventory().src_root(),
            fixture.source.join("src")
        );
        let audit = fs::read_to_string(fixture.source.join(".invocations")).unwrap();
        assert_eq!(audit.lines().count(), 6);
        assert!(audit.lines().all(|line| line.ends_with("|0")));
    }

    #[test]
    fn preflight_rejects_manifest_metadata_before_process_or_scratch() {
        let fixture = Fixture::new("cargo-ok", "rustc-ok");
        fs::remove_file(fixture.source.join("Cargo.toml")).unwrap();
        let error = begin_cargo_preparation(&fixture.bootstrap_request()).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Preflight);
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::ToolMetadata(ToolMetadataField::Manifest)
        );
        assert_eq!(error.evidence_dir, None);
        assert_eq!(error.attempt, None);
        assert!(error.receipts.is_empty());
        assert!(!fixture.source.join(".invocations").exists());
        assert!(!fixture.output.exists());
        assert_eq!(
            fs::read_dir(&fixture.root)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .contains("cargo-preparation"))
                .count(),
            0
        );

        let deadline_fixture = Fixture::new("cargo-ok", "rustc-ok");
        let mut request = deadline_fixture.request();
        request.bootstrap.limits.timeout = Duration::from_nanos(1);
        let error = prepare_cargo_target(request).unwrap_err();
        assert_eq!(error.stage, PreparationStage::LlvmProfdataVersion);
        assert_eq!(error.kind, CargoPreparationErrorKind::ProcessTimeout);
        assert_eq!(error.evidence_dir, None);
        assert_eq!(error.attempt, None);
        assert!(!deadline_fixture.source.join(".invocations").exists());
        assert_eq!(
            fs::read_dir(&deadline_fixture.root)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .contains("cargo-preparation"))
                .count(),
            0
        );
    }

    #[test]
    fn inventory_capture_and_handoff_are_structured_and_never_rebased() {
        let capture = Fixture::new("cargo-ok", "rustc-ok");
        let bootstrap = begin_cargo_preparation(&capture.bootstrap_request()).unwrap();
        fs::remove_file(capture.source.join("src/dtd.rs")).unwrap();
        let error = capture_preparation_inventory(&bootstrap).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Preflight);
        assert!(matches!(
            error.kind,
            CargoPreparationErrorKind::SourceInventory(_)
        ));
        let inventory_error = error.inventory_error().unwrap();
        assert_eq!(inventory_error.reason(), "missing reviewed source");
        assert_eq!(inventory_error.path(), Some("src/dtd.rs"));
        assert_eq!(
            error.to_string(),
            "preflight/source_inventory: missing reviewed source: src/dtd.rs"
        );
        assert!(!capture.output.exists());
        assert!(!capture.source.join(".invocations").exists());

        let handoff = Fixture::new("cargo-ok", "rustc-ok");
        let bootstrap = begin_cargo_preparation(&handoff.bootstrap_request()).unwrap();
        let inventory = capture_preparation_inventory(&bootstrap).unwrap();
        let path = handoff.source.join("src/dtd.rs");
        fs::write(&path, b"// changed after the sole capture\n").unwrap();
        let error = prepare_cargo_target(CargoPreparationRequest {
            bootstrap,
            source_inventory: inventory,
            cargo: &handoff.cargo,
            rustc: &handoff.rustc,
            llvm_profdata: &handoff.profdata,
            llvm_cov: &handoff.cov,
            profile: CargoProfile::Debug,
        })
        .unwrap_err();
        assert!(matches!(
            error.kind,
            CargoPreparationErrorKind::SourceInventory(_)
        ));
        assert_eq!(
            error.inventory_error().unwrap().reason(),
            "source identity changed"
        );
        assert_eq!(error.inventory_error().unwrap().path(), Some("src/dtd.rs"));
        assert!(!handoff.output.exists());
        assert!(!handoff.source.join(".invocations").exists());

        let manifest_handoff = Fixture::new("cargo-ok", "rustc-ok");
        let bootstrap = begin_cargo_preparation(&manifest_handoff.bootstrap_request()).unwrap();
        let inventory = capture_preparation_inventory(&bootstrap).unwrap();
        fs::write(
            manifest_handoff.source.join("Cargo.lock"),
            b"changed after bootstrap\n",
        )
        .unwrap();
        let error = prepare_cargo_target(CargoPreparationRequest {
            bootstrap,
            source_inventory: inventory,
            cargo: &manifest_handoff.cargo,
            rustc: &manifest_handoff.rustc,
            llvm_profdata: &manifest_handoff.profdata,
            llvm_cov: &manifest_handoff.cov,
            profile: CargoProfile::Debug,
        })
        .unwrap_err();
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::Lockfile)
        );
        assert!(!manifest_handoff.output.exists());
        assert!(!manifest_handoff.source.join(".invocations").exists());

        let roots = Fixture::new("cargo-ok", "rustc-ok");
        let unrelated = Fixture::new("cargo-ok", "rustc-ok");
        let bootstrap = begin_cargo_preparation(&roots.bootstrap_request()).unwrap();
        let unrelated_bootstrap = begin_cargo_preparation(&unrelated.bootstrap_request()).unwrap();
        let unrelated_inventory = capture_preparation_inventory(&unrelated_bootstrap).unwrap();
        let error = prepare_cargo_target(CargoPreparationRequest {
            bootstrap,
            source_inventory: unrelated_inventory,
            cargo: &roots.cargo,
            rustc: &roots.rustc,
            llvm_profdata: &roots.profdata,
            llvm_cov: &roots.cov,
            profile: CargoProfile::Debug,
        })
        .unwrap_err();
        assert!(matches!(
            error.kind,
            CargoPreparationErrorKind::SourceInventory(_)
        ));
        assert_eq!(
            error.inventory_error().unwrap().reason(),
            "source inventory root handoff mismatch"
        );
        assert!(!roots.output.exists());
        assert!(!roots.source.join(".invocations").exists());
    }

    #[test]
    fn every_command_has_immediate_pre_and_post_inventory_gates() {
        let stages = [
            PreparationStage::LlvmProfdataVersion,
            PreparationStage::LlvmCovVersion,
            PreparationStage::CargoVersion,
            PreparationStage::RustcVersion,
            PreparationStage::Pkgid,
            PreparationStage::Build,
        ];
        for (index, stage) in stages.into_iter().enumerate() {
            for after in [false, true] {
                let fixture = Fixture::new("cargo-ok", "rustc-ok");
                let error = with_checkpoint_hook(
                    move |checkpoint, _, preflight| {
                        let selected = if after {
                            PreparationCheckpoint::AfterCommand(stage)
                        } else {
                            PreparationCheckpoint::BeforeCommand(stage)
                        };
                        if checkpoint == selected {
                            let path = &preflight.source_inventory().sources()[0].canonical;
                            fs::write(path, b"// command-boundary mutation\n").unwrap();
                        }
                    },
                    || prepare_cargo_target(fixture.request()).unwrap_err(),
                );
                assert_eq!(
                    error.kind,
                    CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::SourceInventory),
                    "{stage:?} after={after}"
                );
                assert_eq!(
                    error.inventory_error().unwrap().reason(),
                    "source identity changed"
                );
                assert_eq!(
                    fs::read_to_string(fixture.source.join(".invocations"))
                        .map(|audit| audit.lines().count())
                        .unwrap_or(0),
                    index + usize::from(after),
                    "{stage:?} after={after}"
                );
            }
        }
    }

    #[test]
    fn preflight_rejects_tool_types_and_lexical_output_without_launching() {
        let fixture = Fixture::new("cargo-ok", "rustc-ok");
        let mut request = fixture.request();
        request.cargo = &fixture.target;
        let error = prepare_cargo_target(request).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Preflight);
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::ToolMetadata(ToolMetadataField::Cargo)
        );
        assert_eq!(error.evidence_dir, None);
        assert!(!fixture.source.join(".invocations").exists());

        let relative_output = PathBuf::from("relative-output");
        let error = begin_cargo_preparation(&CargoBootstrapRequest {
            output_dir: &relative_output,
            ..fixture.bootstrap_request()
        })
        .unwrap_err();
        assert_eq!(error.stage, PreparationStage::Preflight);
        assert_eq!(error.kind, CargoPreparationErrorKind::Configuration);
        assert_eq!(error.evidence_dir, None);
        assert!(!fixture.source.join(".invocations").exists());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("missing-output-target", &fixture.output).unwrap();
            let error = begin_cargo_preparation(&fixture.bootstrap_request()).unwrap_err();
            assert_eq!(error.stage, PreparationStage::Preflight);
            assert_eq!(error.kind, CargoPreparationErrorKind::Configuration);
            assert_eq!(error.evidence_dir, None);
            assert!(!fixture.source.join(".invocations").exists());
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let alias_fixture = Fixture::new("cargo-ok", "rustc-ok");
            let alias = alias_fixture.root.join("cargo-proxy-alias");
            symlink(&alias_fixture.cargo, &alias).unwrap();
            let mut request = alias_fixture.request();
            request.cargo = &alias;
            let prepared = prepare_cargo_target(request).unwrap();
            assert_eq!(prepared.cargo.supplied_path, alias);
            assert_eq!(prepared.cargo.canonical_path, alias_fixture.cargo);
            assert_eq!(prepared.receipts[2].program, prepared.cargo.supplied_path);
            let audit = fs::read_to_string(alias_fixture.source.join(".invocations")).unwrap();
            assert_eq!(
                audit
                    .lines()
                    .filter(|line| line.starts_with("cargo-proxy-alias|"))
                    .count(),
                3
            );
            assert_eq!(
                audit
                    .lines()
                    .filter(|line| line.starts_with("cargo-ok|"))
                    .count(),
                0
            );
            fs::remove_file(&prepared.cargo.supplied_path).unwrap();
            symlink(&alias_fixture.rustc, &prepared.cargo.supplied_path).unwrap();
            let error = prepared
                .recheck(SelectedArtifactCheck::Metadata)
                .unwrap_err();
            assert_eq!(error.stage, PreparationStage::Preflight);
            assert_eq!(
                error.kind,
                CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::Cargo)
            );
        }
    }

    #[test]
    fn receipt_and_publication_failures_preserve_only_owned_evidence() {
        let receipt_fixture = Fixture::new("cargo-receipt-collision", "rustc-ok");
        let error = prepare_cargo_target(receipt_fixture.request()).unwrap_err();
        assert_eq!(error.stage, PreparationStage::CargoVersion);
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::EvidenceIo(io::ErrorKind::AlreadyExists)
        );
        assert!(error.capture_complete);
        assert!(!error.retention_complete);
        assert_eq!(error.receipts.len(), 3);
        let failed_receipt = error.receipts.last().unwrap();
        assert!(failed_receipt.status.success());
        assert!(failed_receipt.capture_complete);
        assert!(!failed_receipt.retention_complete);
        assert_eq!(failed_receipt.stdout_identity.1, b"cargo fixture\n".len());
        assert_eq!(failed_receipt.stdout_stamp, None);
        assert_eq!(failed_receipt.stderr_stamp, None);
        let scratch = error.evidence_dir.unwrap();
        assert!(scratch.is_dir());
        assert_eq!(
            fs::read(scratch.join("cargo-version.stdout")).unwrap(),
            b"collision"
        );
        assert!(receipt_fixture.source.is_dir());
        assert!(receipt_fixture.target.is_dir());

        let combined_fixture = Fixture::new("cargo-nonzero-receipt-collision", "rustc-ok");
        let error = prepare_cargo_target(combined_fixture.request()).unwrap_err();
        assert_eq!(error.stage, PreparationStage::CargoVersion);
        assert_eq!(error.kind, CargoPreparationErrorKind::ProcessExit);
        assert_eq!(
            error.secondary[0].kind(),
            &CargoPreparationErrorKind::EvidenceIo(io::ErrorKind::AlreadyExists)
        );
        assert_eq!(
            error.secondary[0].io_error().unwrap().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(error.capture_complete);
        assert!(!error.retention_complete);
        let failed_receipt = error.receipts.last().unwrap();
        assert!(!failed_receipt.status.success());
        assert!(failed_receipt.capture_complete);
        assert!(!failed_receipt.retention_complete);
        assert!(!failed_receipt.accepted);

        let publish_fixture = Fixture::new("cargo-publish-block", "rustc-ok");
        let error = prepare_cargo_target(publish_fixture.request()).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Publish);
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::EvidenceIo(io::ErrorKind::AlreadyExists)
        );
        assert!(error.capture_complete && error.retention_complete);
        assert_eq!(error.receipts.len(), 6);
        let scratch = error.evidence_dir.unwrap();
        assert!(scratch.is_dir());
        assert!(scratch.join("build.stdout").is_file());
        assert!(publish_fixture.output.is_dir());
        assert!(!publish_fixture.output.join("build").exists());
        assert!(publish_fixture.source.is_dir());
        assert!(publish_fixture.target.is_dir());
    }

    #[test]
    fn process_status_timeout_output_and_pkgid_failures_keep_exact_evidence_state() {
        for (mode, expected_stage, expected_kind, complete) in [
            (
                "cargo-nonzero",
                PreparationStage::Build,
                CargoPreparationErrorKind::ProcessExit,
                true,
            ),
            (
                "cargo-pkgid-multiline",
                PreparationStage::Pkgid,
                CargoPreparationErrorKind::PkgidShape,
                true,
            ),
            (
                "cargo-pkgid-stderr",
                PreparationStage::Pkgid,
                CargoPreparationErrorKind::Stderr,
                true,
            ),
            (
                "cargo-bad-json",
                PreparationStage::Jsonl,
                CargoPreparationErrorKind::Artifact(ArtifactError::Json {
                    line: 1,
                    offset: 1,
                    kind: crate::cargo_artifact::JsonErrorKind::UnexpectedByte,
                }),
                true,
            ),
        ] {
            let fixture = Fixture::new(mode, "rustc-ok");
            let error = prepare_cargo_target(fixture.request()).unwrap_err();
            assert_eq!(error.stage, expected_stage, "{mode}");
            assert_eq!(error.kind, expected_kind, "{mode}");
            assert_eq!(error.capture_complete, complete, "{mode}");
            assert_eq!(error.retention_complete, complete, "{mode}");
            assert!(!error.accepted);
            assert_eq!(
                error.attempt.as_ref().unwrap().stage,
                if expected_stage == PreparationStage::Jsonl {
                    PreparationStage::Build
                } else {
                    expected_stage
                }
            );
            let evidence = error
                .evidence_dir
                .expect("attempted process preserves evidence");
            assert!(evidence.exists());
            if expected_stage == PreparationStage::Build
                || expected_stage == PreparationStage::Jsonl
            {
                assert!(evidence.join("build.stdout").exists());
                assert!(evidence.join("build.stderr").exists());
            }
        }

        let timeout_fixture = Fixture::new("cargo-timeout", "rustc-ok");
        let mut request = timeout_fixture.request();
        request.bootstrap.limits.timeout = Duration::from_secs(1);
        let error = prepare_cargo_target(request).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Build);
        assert_eq!(error.kind, CargoPreparationErrorKind::ProcessTimeout);
        assert!(!error.capture_complete);
        assert!(!error.retention_complete);
        assert_eq!(
            error.attempt.as_ref().unwrap().stage,
            PreparationStage::Build
        );
        assert!(error.attempt.as_ref().unwrap().elapsed.is_some());
        assert_eq!(error.receipts.len(), 5);

        let output_fixture = Fixture::new("cargo-output", "rustc-ok");
        let mut request = output_fixture.request();
        request.bootstrap.limits.max_build_output_bytes = 1024;
        let error = prepare_cargo_target(request).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Build);
        assert_eq!(error.kind, CargoPreparationErrorKind::ProcessOutputLimit);
        assert!(!error.capture_complete);
        assert!(!error.retention_complete);
        assert_eq!(
            error.attempt.as_ref().unwrap().stage,
            PreparationStage::Build
        );
        assert!(error.attempt.as_ref().unwrap().elapsed.is_some());
        assert_eq!(error.receipts.len(), 5);

        let stream_fixture = Fixture::new("cargo-stderr-limit", "rustc-ok");
        let error = prepare_cargo_target(stream_fixture.request()).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Build);
        assert_eq!(error.kind, CargoPreparationErrorKind::StderrLimit);
        assert!(error.capture_complete);
        assert!(error.retention_complete);
        assert!(!error.accepted);
        assert_eq!(error.receipts.len(), 6);
        assert!(!error.receipts.last().unwrap().accepted);
        let evidence = error.evidence_dir.unwrap();
        assert_eq!(
            fs::metadata(evidence.join("build.stderr")).unwrap().len(),
            (MAX_BUILD_STDERR_BYTES + 1) as u64
        );

        let nonzero_stream_fixture = Fixture::new("cargo-nonzero-stderr-limit", "rustc-ok");
        let error = prepare_cargo_target(nonzero_stream_fixture.request()).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Build);
        assert_eq!(error.kind, CargoPreparationErrorKind::ProcessExit);
        assert!(error.capture_complete && error.retention_complete);
        assert!(!error.accepted);
        assert_eq!(error.receipts.len(), 6);
        let build = error.receipts.last().unwrap();
        assert!(!build.status.success());
        assert_eq!(build.stderr_len, MAX_BUILD_STDERR_BYTES + 1);
        assert!(!build.accepted);

        let stdout_exact_fixture = Fixture::new("cargo-stdout-exact", "rustc-ok");
        let prepared = prepare_cargo_target(stdout_exact_fixture.request()).unwrap();
        let build = prepared.receipts.last().unwrap();
        assert_eq!(build.stdout_len, MAX_BUILD_STDOUT_BYTES);
        assert_eq!(
            fs::metadata(&build.stdout_path).unwrap().len(),
            MAX_BUILD_STDOUT_BYTES as u64
        );
        assert!(build.capture_complete && build.retention_complete && build.accepted);

        let stdout_limit_fixture = Fixture::new("cargo-stdout-limit", "rustc-ok");
        let error = prepare_cargo_target(stdout_limit_fixture.request()).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Build);
        assert_eq!(error.kind, CargoPreparationErrorKind::StdoutLimit);
        assert!(error.capture_complete && error.retention_complete);
        assert!(!error.accepted);
        let build = error.receipts.last().unwrap();
        assert_eq!(build.stdout_len, MAX_BUILD_STDOUT_BYTES + 1);
        assert_eq!(
            fs::metadata(&build.stdout_path).unwrap().len(),
            (MAX_BUILD_STDOUT_BYTES + 1) as u64
        );
        assert!(!build.accepted);

        let combined_exact_fixture = Fixture::new("cargo-combined-exact", "rustc-ok");
        let prepared = prepare_cargo_target(combined_exact_fixture.request()).unwrap();
        let build = prepared.receipts.last().unwrap();
        assert_eq!(build.stdout_len, MAX_BUILD_STDOUT_BYTES);
        assert_eq!(build.stderr_len, MAX_BUILD_STDERR_BYTES);
        assert_eq!(
            build.stdout_len.checked_add(build.stderr_len),
            Some(MAX_BUILD_OUTPUT_BYTES)
        );
        assert!(build.capture_complete && build.retention_complete && build.accepted);

        let combined_limit_fixture = Fixture::new("cargo-combined-limit", "rustc-ok");
        let error = prepare_cargo_target(combined_limit_fixture.request()).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Build);
        assert_eq!(error.kind, CargoPreparationErrorKind::ProcessOutputLimit);
        assert!(!error.capture_complete);
        assert!(!error.retention_complete);
        assert_eq!(error.receipts.len(), 5);
        let evidence = error.evidence_dir.unwrap();
        assert!(!evidence.join("build.stdout").exists());
        assert!(!evidence.join("build.stderr").exists());
    }

    #[test]
    fn primary_process_failure_retains_secondary_input_drift() {
        let fixture = Fixture::new("cargo-nonzero-mutate", "rustc-ok");
        let error = prepare_cargo_target(fixture.request()).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Build);
        assert_eq!(error.kind, CargoPreparationErrorKind::ProcessExit);
        assert_eq!(
            error.secondary[0].kind(),
            &CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::SourceInventory)
        );
        assert_eq!(
            error.secondary[0].inventory_error().unwrap().reason(),
            "source identity changed"
        );

        let stderr_fixture = Fixture::new("cargo-pkgid-stderr-mutate", "rustc-ok");
        let error = prepare_cargo_target(stderr_fixture.request()).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Pkgid);
        assert_eq!(error.kind, CargoPreparationErrorKind::Stderr);
        assert_eq!(
            error.secondary[0].kind(),
            &CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::SourceInventory)
        );

        let timeout_fixture = Fixture::new("cargo-timeout-mutate", "rustc-ok");
        let mut request = timeout_fixture.request();
        request.bootstrap.limits.timeout = Duration::from_secs(1);
        let error = prepare_cargo_target(request).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Build);
        assert_eq!(error.kind, CargoPreparationErrorKind::ProcessTimeout);
        assert_eq!(
            error.secondary[0].kind(),
            &CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::SourceInventory)
        );

        let between_fixture = Fixture::new("cargo-version-mutate", "rustc-ok");
        let error = prepare_cargo_target(between_fixture.request()).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Preflight);
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::SourceInventory)
        );
        assert_eq!(error.receipts.len(), 3);
        assert_eq!(
            fs::read_to_string(between_fixture.source.join(".invocations"))
                .unwrap()
                .lines()
                .count(),
            3
        );

        let json_fixture = Fixture::new("cargo-bad-json-mutate", "rustc-ok");
        let error = prepare_cargo_target(json_fixture.request()).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Preflight);
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::SourceInventory)
        );
        assert_eq!(error.receipts.len(), 6);
        assert!(error.receipts.last().unwrap().accepted);
    }

    #[test]
    fn host_schema_build_status_and_permitted_build_stderr_are_ordered() {
        let llvm_fixture = Fixture::new("cargo-ok", "rustc-ok");
        let mut request = llvm_fixture.request();
        request.bootstrap.compiler_version_label = "rustc fixture LLVM version: 22.1.7".to_owned();
        let error = prepare_cargo_target(request).unwrap_err();
        assert_eq!(error.stage, PreparationStage::LlvmProfdataVersion);
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::ToolMetadata(ToolMetadataField::LlvmVersion)
        );
        assert_eq!(error.receipts.len(), 1);
        assert!(!error.receipts[0].accepted);

        let host_fixture = Fixture::new("cargo-ok", "rustc-bad-host");
        let error = prepare_cargo_target(host_fixture.request()).unwrap_err();
        assert_eq!(error.stage, PreparationStage::RustcVersion);
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::ToolMetadata(ToolMetadataField::RustcHost)
        );

        let false_fixture = Fixture::new("cargo-false", "rustc-ok");
        let error = prepare_cargo_target(false_fixture.request()).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Jsonl);
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::Artifact(ArtifactError::BuildFinished(
                crate::cargo_artifact::BuildFinishedError::False,
            ))
        );

        let stderr_fixture = Fixture::new("cargo-build-stderr", "rustc-ok");
        let prepared = prepare_cargo_target(stderr_fixture.request()).unwrap();
        assert!(prepared.build_stderr.stamp.len > 0);
        assert!(prepared.receipts.last().unwrap().stderr_len > 0);
    }

    #[test]
    fn jsonl_rechecks_reject_equal_stamp_unequal_content_and_path_substitution() {
        let fixture = Fixture::new("cargo-ok", "rustc-ok");
        let prepared = prepare_cargo_target(fixture.request()).unwrap();
        let original = fs::read(&prepared.jsonl.supplied_path).unwrap();
        let mut changed = original.clone();
        let index = changed.iter().position(|byte| *byte == b'y').unwrap();
        changed[index] = b'z';
        fs::write(&prepared.jsonl.supplied_path, &changed).unwrap();
        File::options()
            .write(true)
            .open(&prepared.jsonl.supplied_path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(prepared.jsonl.stamp.modified))
            .unwrap();
        assert_eq!(
            file_stamp(&prepared.jsonl.supplied_path).unwrap(),
            prepared.jsonl.stamp
        );
        let error = prepared
            .recheck(SelectedArtifactCheck::Metadata)
            .unwrap_err();
        assert_eq!(error.stage, PreparationStage::Jsonl);
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::Jsonl)
        );
        fs::write(&prepared.jsonl.supplied_path, original).unwrap();
        File::options()
            .write(true)
            .open(&prepared.jsonl.supplied_path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(prepared.jsonl.stamp.modified))
            .unwrap();
        prepared.recheck(SelectedArtifactCheck::Content).unwrap();

        let original = fs::read(&prepared.jsonl.supplied_path).unwrap();
        fs::write(
            &prepared.jsonl.supplied_path,
            &original[..original.len() - 1],
        )
        .unwrap();
        File::options()
            .write(true)
            .open(&prepared.jsonl.supplied_path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(prepared.jsonl.stamp.modified))
            .unwrap();
        let error = prepared
            .recheck(SelectedArtifactCheck::Metadata)
            .unwrap_err();
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::Jsonl)
        );

        let mut grown = original.clone();
        grown.push(b' ');
        fs::write(&prepared.jsonl.supplied_path, &grown).unwrap();
        File::options()
            .write(true)
            .open(&prepared.jsonl.supplied_path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(prepared.jsonl.stamp.modified))
            .unwrap();
        let error = prepared
            .recheck(SelectedArtifactCheck::Metadata)
            .unwrap_err();
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::Jsonl)
        );
        fs::write(&prepared.jsonl.supplied_path, &original).unwrap();
        File::options()
            .write(true)
            .open(&prepared.jsonl.supplied_path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(prepared.jsonl.stamp.modified))
            .unwrap();
        prepared.recheck(SelectedArtifactCheck::Content).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let parked = prepared.evidence_dir.join("parked-jsonl");
            fs::rename(&prepared.jsonl.supplied_path, &parked).unwrap();
            symlink(&parked, &prepared.jsonl.supplied_path).unwrap();
            let error = prepared
                .recheck(SelectedArtifactCheck::Metadata)
                .unwrap_err();
            assert!(matches!(
                error.kind,
                CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::Jsonl)
            ));
        }
    }

    #[test]
    fn preflight_root_bounds_and_overlap_precedence_are_frozen() {
        let exact = PathBuf::from(format!("/{}", "a".repeat(MAX_PATH_BYTES - 1)));
        validate_root_length(&exact, crate::cargo_artifact::ArtifactPathField::SourceRoot).unwrap();
        let over = PathBuf::from(format!("/{}", "a".repeat(MAX_PATH_BYTES)));
        let error =
            validate_root_length(&over, crate::cargo_artifact::ArtifactPathField::SourceRoot)
                .unwrap_err();
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::Artifact(ArtifactError::Path {
                field: crate::cargo_artifact::ArtifactPathField::SourceRoot,
                kind: crate::cargo_artifact::ArtifactPathError::LengthOverflow,
            })
        );

        let fixture = Fixture::new("cargo-ok", "rustc-ok");
        fs::remove_file(fixture.source.join("Cargo.toml")).unwrap();
        let error = begin_cargo_preparation(&CargoBootstrapRequest {
            target_dir: &fixture.source,
            ..fixture.bootstrap_request()
        })
        .unwrap_err();
        assert_eq!(error.stage, PreparationStage::Preflight);
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::ToolMetadata(ToolMetadataField::Manifest)
        );
        assert!(error.receipts.is_empty());

        write_package(&fixture.source, fixture_library());
        let error = begin_cargo_preparation(&CargoBootstrapRequest {
            target_dir: &fixture.source,
            ..fixture.bootstrap_request()
        })
        .unwrap_err();
        assert_eq!(error.stage, PreparationStage::Preflight);
        assert_eq!(error.kind, CargoPreparationErrorKind::Configuration);
        assert!(error.receipts.is_empty());

        let contains = Fixture::new("cargo-ok", "rustc-ok");
        let error = begin_cargo_preparation(&CargoBootstrapRequest {
            target_dir: &contains.root,
            ..contains.bootstrap_request()
        })
        .unwrap_err();
        assert_eq!(error.stage, PreparationStage::Preflight);
        assert_eq!(error.kind, CargoPreparationErrorKind::Configuration);
        assert!(error.receipts.is_empty());
    }

    #[test]
    fn full_identity_gates_precede_first_process_and_json_selection() {
        let first = Fixture::new("cargo-ok", "rustc-ok");
        let error = with_checkpoint_hook(
            |checkpoint, _, preflight| {
                if checkpoint == PreparationCheckpoint::BeforeFirstGate {
                    let source = &preflight.source_inventory().sources()[0];
                    let mut bytes = fs::read(&source.canonical).unwrap();
                    bytes[0] = b'P';
                    rewrite_preserving_stamp(&source.canonical, &bytes);
                }
            },
            || prepare_cargo_target(first.request()).unwrap_err(),
        );
        assert_eq!(error.stage, PreparationStage::Preflight);
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::SourceInventory)
        );
        assert!(error.receipts.is_empty());
        assert!(!first.source.join(".invocations").exists());

        let post_build = Fixture::new("cargo-bad-json", "rustc-ok");
        let error = with_checkpoint_hook(
            |checkpoint, _, preflight| {
                if checkpoint == PreparationCheckpoint::AfterBuild {
                    let source = &preflight.source_inventory().sources()[0];
                    let mut bytes = fs::read(&source.canonical).unwrap();
                    bytes[0] = b'P';
                    rewrite_preserving_stamp(&source.canonical, &bytes);
                }
            },
            || prepare_cargo_target(post_build.request()).unwrap_err(),
        );
        assert_eq!(error.stage, PreparationStage::Preflight);
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::SourceInventory)
        );
        assert_eq!(error.receipts.len(), 6);
        assert!(error.receipts.last().unwrap().accepted);
    }

    #[test]
    fn content_recheck_aggregates_every_frozen_input_failure() {
        let fixture = Fixture::new("cargo-ok", "rustc-ok");
        let prepared = prepare_cargo_target(fixture.request()).unwrap();
        for frozen in [&prepared.manifest, &prepared.lockfile, &prepared.llvm_cov] {
            let mut bytes = fs::read(&frozen.supplied_path).unwrap();
            bytes[0] ^= 1;
            rewrite_preserving_stamp(&frozen.supplied_path, &bytes);
        }
        let error = prepared
            .recheck(SelectedArtifactCheck::Content)
            .unwrap_err();
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::Manifest)
        );
        assert!(error.secondary.iter().any(|diagnostic| {
            diagnostic.kind()
                == &CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::Lockfile)
        }));
        assert!(error.secondary.iter().any(|diagnostic| {
            diagnostic.kind()
                == &CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::LlvmCov)
        }));
    }

    #[cfg(unix)]
    #[test]
    fn metadata_canonicalize_preserves_permission_denied_source() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let fixture = Fixture::new("cargo-ok", "rustc-ok");
        let restricted = fixture.root.join("restricted-tool-parent");
        fs::create_dir(&restricted).unwrap();
        let canonical = restricted.join("cov-restricted");
        fs::copy(&fixture.cov, &canonical).unwrap();
        let alias = fixture.root.join("cov-proxy");
        symlink(&canonical, &alias).unwrap();
        let mut request = fixture.request();
        request.llvm_cov = &alias;
        let prepared = prepare_cargo_target(request).unwrap();
        fs::set_permissions(&restricted, fs::Permissions::from_mode(0o600)).unwrap();
        let result = prepared.recheck(SelectedArtifactCheck::Metadata);
        fs::set_permissions(&restricted, fs::Permissions::from_mode(0o700)).unwrap();
        let error = result.unwrap_err();
        assert_eq!(error.stage, PreparationStage::Preflight);
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::EvidenceIo(io::ErrorKind::PermissionDenied)
        );
        assert_eq!(
            std::error::Error::source(&error)
                .and_then(|source| source.downcast_ref::<io::Error>())
                .map(io::Error::kind),
            Some(io::ErrorKind::PermissionDenied)
        );
    }

    #[cfg(unix)]
    #[test]
    fn target_root_drift_and_first_attempt_io_preserve_evidence() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = Fixture::new("cargo-target-swap", "rustc-ok");
        let error = prepare_cargo_target(fixture.request()).unwrap_err();
        assert_eq!(error.stage, PreparationStage::Preflight);
        assert_eq!(error.receipts.len(), 3);
        assert_eq!(
            fs::read_to_string(fixture.source.join(".invocations"))
                .unwrap()
                .lines()
                .count(),
            3
        );

        let io_fixture = Fixture::new("cargo-ok", "rustc-ok");
        fs::set_permissions(&io_fixture.profdata, fs::Permissions::from_mode(0o600)).unwrap();
        let error = prepare_cargo_target(io_fixture.request()).unwrap_err();
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::ProcessIo(io::ErrorKind::PermissionDenied)
        );
        assert!(std::error::Error::source(&error).is_some());
        assert!(
            error
                .evidence_dir
                .as_ref()
                .is_some_and(|path| path.is_dir())
        );
        assert!(error.receipts.is_empty());
    }

    #[test]
    fn selection_uses_captured_stdout_during_restored_path_swap() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let fixture = Fixture::new("cargo-ok", "rustc-ok");
        let parked = Rc::new(RefCell::new(None::<PathBuf>));
        let parked_for_hook = Rc::clone(&parked);
        let prepared = with_checkpoint_hook(
            move |checkpoint, owned, preflight| {
                let jsonl = owned.path.join("build.stdout");
                match checkpoint {
                    PreparationCheckpoint::BeforeArtifactSelection => {
                        let saved = owned.path.join("build.stdout.original");
                        fs::rename(&jsonl, &saved).unwrap();
                        let bytes = fs::read(&saved).unwrap();
                        let altered = String::from_utf8(bytes)
                            .unwrap()
                            .replace("opaque-harness-481", "opaque-harness-482");
                        fs::write(&jsonl, altered).unwrap();
                        let original = preflight
                            .roots
                            .canonical_target_dir
                            .join("fixture-host/debug/deps/opaque-harness-481");
                        let alternate = preflight
                            .roots
                            .canonical_target_dir
                            .join("fixture-host/debug/deps/opaque-harness-482");
                        fs::copy(original, alternate).unwrap();
                        *parked_for_hook.borrow_mut() = Some(saved);
                    }
                    PreparationCheckpoint::AfterArtifactSelection => {
                        fs::remove_file(&jsonl).unwrap();
                        fs::rename(parked_for_hook.borrow_mut().take().unwrap(), &jsonl).unwrap();
                    }
                    _ => {}
                }
            },
            || prepare_cargo_target(fixture.request()).unwrap(),
        );
        assert!(
            prepared
                .selected()
                .reported_executable
                .ends_with("opaque-harness-481")
        );
    }

    #[cfg(unix)]
    #[test]
    fn lifecycle_handoffs_detect_selected_jsonl_and_source_faults() {
        use std::os::unix::fs::symlink;

        let post_read = Fixture::new("cargo-ok", "rustc-ok");
        let error = with_checkpoint_hook(
            |checkpoint, owned, _| {
                if checkpoint == PreparationCheckpoint::AfterArtifactSelection {
                    let jsonl = owned.path.join("build.stdout");
                    let parked = owned.path.join("parked-build.stdout");
                    fs::rename(&jsonl, &parked).unwrap();
                    symlink(&parked, &jsonl).unwrap();
                }
            },
            || prepare_cargo_target(post_read.request()).unwrap_err(),
        );
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::Jsonl)
        );

        let initial_selected = Fixture::new("cargo-ok", "rustc-ok");
        let error = with_checkpoint_hook(
            |checkpoint, _, preflight| {
                if checkpoint == PreparationCheckpoint::AfterResolution {
                    let executable = preflight
                        .roots
                        .canonical_target_dir
                        .join("fixture-host/debug/deps/opaque-harness-481");
                    rewrite_preserving_stamp(&executable, b"fixture executablE");
                }
            },
            || prepare_cargo_target(initial_selected.request()).unwrap_err(),
        );
        assert!(matches!(
            error.kind,
            CargoPreparationErrorKind::Artifact(ArtifactError::Path {
                field: crate::cargo_artifact::ArtifactPathField::Executable,
                kind: crate::cargo_artifact::ArtifactPathError::ContentIdentityMismatch,
            })
        ));
        assert!(!initial_selected.output.exists());

        let publication = Fixture::new("cargo-ok", "rustc-ok");
        let error = with_checkpoint_hook(
            |checkpoint, owned, _| {
                if checkpoint == PreparationCheckpoint::BeforePublication {
                    let selected = &owned.selection.as_ref().unwrap().0;
                    rewrite_preserving_stamp(&selected.canonical_executable, b"fixture executablE");
                    fs::create_dir(owned.path.parent().unwrap().join("output")).unwrap();
                }
            },
            || prepare_cargo_target(publication.request()).unwrap_err(),
        );
        assert_eq!(error.stage, PreparationStage::Publish);
        assert!(error.secondary.iter().any(|diagnostic| matches!(
            diagnostic.kind(),
            CargoPreparationErrorKind::Artifact(ArtifactError::Path {
                field: crate::cargo_artifact::ArtifactPathField::Executable,
                kind: crate::cargo_artifact::ArtifactPathError::ContentIdentityMismatch,
            })
        )));

        let final_jsonl = Fixture::new("cargo-ok", "rustc-ok");
        let error = with_checkpoint_hook(
            |checkpoint, owned, _| {
                if checkpoint == PreparationCheckpoint::AfterRelocation {
                    let path = owned.path.join("build.stdout");
                    let mut bytes = fs::read(&path).unwrap();
                    let index = bytes.iter().position(|byte| *byte == b'd').unwrap();
                    bytes[index] = b'e';
                    rewrite_preserving_stamp(&path, &bytes);
                }
            },
            || prepare_cargo_target(final_jsonl.request()).unwrap_err(),
        );
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::Jsonl)
        );

        let final_walk = Fixture::new("cargo-ok", "rustc-ok");
        let prepared = prepare_cargo_target(final_walk.request()).unwrap();
        let changed = std::cell::Cell::new(false);
        let error = with_jsonl_checkpoint_hook(
            move |frozen| {
                if !changed.replace(true) {
                    let parked = frozen.supplied_path.with_extension("post-read-original");
                    fs::rename(&frozen.supplied_path, &parked).unwrap();
                    symlink(&parked, &frozen.supplied_path).unwrap();
                }
            },
            || {
                prepared
                    .recheck(SelectedArtifactCheck::Content)
                    .unwrap_err()
            },
        );
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::Jsonl)
        );

        let final_selected = Fixture::new("cargo-ok", "rustc-ok");
        let error = with_checkpoint_hook(
            |checkpoint, owned, _| {
                if checkpoint == PreparationCheckpoint::AfterRelocation {
                    let selected = &owned.selection.as_ref().unwrap().0;
                    rewrite_preserving_stamp(&selected.canonical_executable, b"fixture executablE");
                }
            },
            || prepare_cargo_target(final_selected.request()).unwrap_err(),
        );
        assert_eq!(error.stage, PreparationStage::Resolve);

        let source = Fixture::new("cargo-ok", "rustc-ok");
        let prepared = prepare_cargo_target(source.request()).unwrap();
        assert_eq!(prepared.source_inventory().sources().len(), 32);
        let path = source.source.join("src/lib.rs");
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] = b'P';
        rewrite_preserving_stamp(&path, &bytes);
        let error = prepared
            .recheck(SelectedArtifactCheck::Metadata)
            .unwrap_err();
        assert_eq!(
            error.kind,
            CargoPreparationErrorKind::IdentityChanged(EvidenceSubject::SourceInventory)
        );
    }

    #[test]
    #[ignore = "requires explicit pinned nightly Cargo/rustc and LLVM tool paths"]
    fn actual_pinned_cargo_libtest_preparation_and_protocol() {
        let cargo = required_path("YAXML_REAL_CARGO");
        let rustc = required_path("YAXML_REAL_RUSTC");
        let profdata = required_path("YAXML_REAL_LLVM_PROFDATA");
        let cov = required_path("YAXML_REAL_LLVM_COV");
        let llvm = std::env::var("YAXML_REAL_LLVM_VERSION").expect("LLVM version is explicit");

        let fixture = Fixture::new("cargo-ok", "rustc-ok");
        write_package(&fixture.source, actual_fixture_library());
        let label = format!("pinned nightly LLVM version: {llvm}");
        let bootstrap = begin_cargo_preparation(&CargoBootstrapRequest {
            source_root: &fixture.source,
            target_dir: &fixture.target,
            output_dir: &fixture.output,
            compiler_version_label: &label,
            limits: CargoPreparationLimits::default(),
        })
        .unwrap();
        let source_inventory = capture_preparation_inventory(&bootstrap).unwrap();
        let request = CargoPreparationRequest {
            bootstrap,
            source_inventory,
            cargo: &cargo,
            rustc: &rustc,
            llvm_profdata: &profdata,
            llvm_cov: &cov,
            profile: CargoProfile::Debug,
        };
        let prepared = prepare_cargo_target(request).unwrap();
        assert_eq!(prepared.selected.target.name, "yaxml");
        assert_eq!(
            prepared.selected.target.src_path,
            fixture.source.join("src/lib.rs").to_str().unwrap()
        );
        prepared.recheck(SelectedArtifactCheck::Content).unwrap();

        let profile = fixture.root.join("actual.profraw");
        let environment = vec![
            (
                "LLVM_PROFILE_FILE".to_owned(),
                profile.to_str().unwrap().to_owned(),
            ),
            ("TERM".to_owned(), "dumb".to_owned()),
            ("NO_COLOR".to_owned(), "1".to_owned()),
        ];
        let removed = [
            "RUST_TEST_THREADS",
            "RUST_TEST_NOCAPTURE",
            "RUST_TEST_SHUFFLE",
            "RUST_TEST_SHUFFLE_SEED",
            "RUST_TEST_INVOKE_HELPER",
            "RUST_TEST_TIME_UNIT",
            "RUST_TEST_TIME_INTEGRATION",
            "RUST_TEST_TIME_DOCTEST",
        ];
        let output = process::run_with_environment(
            &prepared.selected.canonical_executable,
            &LIBTEST_EXACT_V1_ARGS,
            b"known fixture payload\n",
            ProcessLimits {
                timeout: Duration::from_secs(30),
                max_output_bytes: crate::protocol::MAX_LIBTEST_PARSE_BYTES,
            },
            &environment,
            &removed,
            Some(&fixture.root),
        )
        .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        parse_libtest_exact_v1_stdout(&output.stdout).unwrap();
        let metadata = fs::symlink_metadata(&profile).unwrap();
        assert!(metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() > 0);
        prepared.recheck(SelectedArtifactCheck::Content).unwrap();
        let build = prepared.receipts.last().unwrap();
        eprintln!(
            "actual-cargo-evidence package_id={:?} host={:?} cargo={:?} rustc={:?} llvm_profdata={:?} llvm_cov={:?} build_status={:?} jsonl_len={} jsonl_identity={:?} stderr_len={} stderr_identity={:?} executable_identity={:?} profile_len={}",
            prepared.package_id,
            prepared.rustc_host,
            prepared.cargo.identity,
            prepared.rustc.identity,
            prepared.llvm_profdata.identity,
            prepared.llvm_cov.identity,
            build.status,
            prepared.jsonl.stamp.len,
            prepared.jsonl.identity,
            prepared.build_stderr.stamp.len,
            prepared.build_stderr.identity,
            prepared.selected.executable_identity,
            metadata.len(),
        );
    }

    fn required_path(name: &str) -> PathBuf {
        PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} is required")))
    }

    #[test]
    fn frozen_constants_and_release_argv_are_exact() {
        assert_eq!(MAX_BUILD_STDOUT_BYTES, 8 * 1024 * 1024);
        assert_eq!(MAX_BUILD_STDERR_BYTES, 1024 * 1024);
        assert_eq!(MAX_BUILD_OUTPUT_BYTES, 9 * 1024 * 1024);
        assert_eq!(MAX_PREPARATION_SECONDS, 600);
        assert!(
            validate_build_stream_lengths(MAX_BUILD_STDOUT_BYTES, MAX_BUILD_STDERR_BYTES).is_ok()
        );
        assert_eq!(
            validate_build_stream_lengths(MAX_BUILD_STDOUT_BYTES + 1, 0)
                .unwrap_err()
                .kind,
            CargoPreparationErrorKind::StdoutLimit
        );
        assert_eq!(
            validate_build_stream_lengths(0, MAX_BUILD_STDERR_BYTES + 1)
                .unwrap_err()
                .kind,
            CargoPreparationErrorKind::StderrLimit
        );

        let bounded = fixture_root("bounded-read");
        let file = bounded.join("bytes");
        fs::write(&file, b"abc").unwrap();
        assert_eq!(read_bounded(&file, 3).unwrap(), b"abc");
        fs::write(&file, b"abcd").unwrap();
        assert_eq!(
            read_bounded(&file, 3).unwrap_err().kind,
            CargoPreparationErrorKind::StdoutLimit
        );
        fs::remove_dir_all(&bounded).unwrap();
        let fixture = Fixture::new("cargo-ok", "rustc-ok");
        let mut request = fixture.request();
        request.profile = CargoProfile::Release;
        let prepared = prepare_cargo_target(request).unwrap();
        assert_eq!(prepared.profile, CargoProfile::Release);
        assert_eq!(
            prepared.receipts.last().unwrap().argv.last().unwrap(),
            "--release"
        );
        let mut stderr = String::new();
        File::open(&prepared.build_stderr.supplied_path)
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert!(stderr.is_empty());
    }

    fn fixture_root(label: &str) -> PathBuf {
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yaxml-cargo-build-{label}-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        fs::canonicalize(path).unwrap()
    }
}
