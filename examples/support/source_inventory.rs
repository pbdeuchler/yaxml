//! Frozen source inventory and LLVM source-row namespace policy.
#![forbid(unsafe_code)]

use crate::file_identity::{
    ContentIdentity, FileIdentityError, FileStamp, file_identity_from_file_bounded,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, Metadata};
use std::io;
use std::path::{Component, Path, PathBuf};

pub(crate) const MAX_ENTRIES: usize = 64;
pub(crate) const MAX_DIRECTORIES: usize = 8;
pub(crate) const MAX_DEPTH: usize = 4;
pub(crate) const MAX_NAME_BYTES: usize = 32 * 1024;
pub(crate) const MAX_SOURCE_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_TOTAL_SOURCE_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) const MASTER_SOURCE_PATHS: [&str; 32] = [
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

const PUBLIC_REPLAY_PATHS: [&str; 17] = [
    "src/buffered.rs",
    "src/dom.rs",
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
    "src/serialize.rs",
    "src/stream.rs",
    "src/tokenizer.rs",
    "src/writer.rs",
];

const STABLE_READER_PATHS: [&str; 21] = [
    "src/dtd.rs",
    "src/dtd_catalog.rs",
    "src/dtd_controller.rs",
    "src/dtd_controller/content.rs",
    "src/dtd_frames.rs",
    "src/dtd_reference.rs",
    "src/dtd_scan.rs",
    "src/dtd_tag.rs",
    "src/encoding.rs",
    "src/error.rs",
    "src/escape.rs",
    "src/name.rs",
    "src/origin.rs",
    "src/parser.rs",
    "src/reader.rs",
    "src/reader/dtd.rs",
    "src/stream/dtd.rs",
    "src/tokenizer.rs",
    "src/tokenizer/local.rs",
    "src/tokenizer/reference.rs",
    "src/tokenizer/text.rs",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourceProfile {
    PublicReplayV2,
    StableReaderV1,
    MasterLibtestMapV1,
}

impl SourceProfile {
    pub(crate) fn manifest(self) -> &'static [&'static str] {
        match self {
            Self::PublicReplayV2 => &PUBLIC_REPLAY_PATHS,
            Self::StableReaderV1 => &STABLE_READER_PATHS,
            Self::MasterLibtestMapV1 => &MASTER_SOURCE_PATHS,
        }
    }

    pub(crate) fn expected_keys(self) -> Vec<String> {
        self.manifest()
            .iter()
            .filter(|path| {
                !matches!(self, Self::PublicReplayV2 | Self::MasterLibtestMapV1)
                    || **path != "src/lib.rs"
            })
            .map(|path| {
                path.strip_prefix("src/")
                    .expect("literal src path")
                    .to_owned()
            })
            .collect()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FrozenSource {
    pub(crate) relative: &'static str,
    pub(crate) key: &'static str,
    pub(crate) canonical: PathBuf,
    pub(crate) stamp: FileStamp,
    pub(crate) identity: ContentIdentity,
    file_key: FileKey,
}

#[derive(Clone, Debug)]
pub(crate) struct SourceInventory {
    source_root: PathBuf,
    src_root: PathBuf,
    sources: Vec<FrozenSource>,
}

impl SourceInventory {
    pub(crate) fn capture(canonical_source_root: &Path) -> Result<Self, InventoryError> {
        validate_profile_literals()?;
        require_path_text(canonical_source_root, "source root")?;
        if fs::canonicalize(canonical_source_root).map_err(|_| err("source root I/O"))?
            != canonical_source_root
        {
            return Err(err("source root is not canonical"));
        }
        let src_root = canonical_source_root.join("src");
        let src_meta = fs::symlink_metadata(&src_root).map_err(|_| err("src metadata I/O"))?;
        if src_meta.file_type().is_symlink() || !src_meta.is_dir() {
            return Err(err("src is not a real directory"));
        }
        let canonical_src =
            fs::canonicalize(&src_root).map_err(|_| err("src canonicalization I/O"))?;
        if canonical_src != src_root {
            return Err(err("src namespace changed"));
        }
        let records = collect_tree(&src_root)?;
        validate_records(&records)?;

        let found: BTreeSet<_> = records
            .iter()
            .filter(|record| record.metadata.as_ref().is_ok_and(Metadata::is_file))
            .filter(|record| record.relative.extension().is_some_and(|ext| ext == "rs"))
            .map(|record| format!("src/{}", slash_path(&record.relative)))
            .collect();
        let expected: BTreeSet<_> = MASTER_SOURCE_PATHS
            .iter()
            .map(|path| (*path).to_owned())
            .collect();
        let missing = expected.difference(&found).next().cloned();
        let extra = found.difference(&expected).next().cloned();

        let mut lengths = Vec::with_capacity(MASTER_SOURCE_PATHS.len());
        let mut total = 0_u64;
        for relative in MASTER_SOURCE_PATHS {
            let path = canonical_source_root.join(relative);
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => {
                    return Err(InventoryError::with_path(
                        "reviewed source metadata I/O",
                        relative,
                    ));
                }
            };
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(InventoryError::with_path(
                    "reviewed source is not a real file",
                    relative,
                ));
            }
            let canonical = fs::canonicalize(&path).map_err(|_| {
                InventoryError::with_path("reviewed source canonicalization I/O", relative)
            })?;
            if canonical != path || !canonical.starts_with(&src_root) {
                return Err(InventoryError::with_path(
                    "reviewed source escaped canonical namespace",
                    relative,
                ));
            }
            require_path_text(&canonical, "canonical source path")?;
            if metadata.len() > MAX_SOURCE_BYTES {
                return Err(InventoryError::with_path(
                    "reviewed source exceeds 1 MiB",
                    relative,
                ));
            }
            total = total
                .checked_add(metadata.len())
                .ok_or_else(|| err("source aggregate length overflow"))?;
            if total > MAX_TOTAL_SOURCE_BYTES {
                return Err(err("reviewed sources exceed 8 MiB aggregate"));
            }
            lengths.push((relative, canonical, metadata));
        }

        let mut sources = Vec::with_capacity(lengths.len());
        for (relative, canonical, metadata) in lengths {
            let stamp = FileStamp::from_metadata(&metadata)
                .map_err(|_| InventoryError::with_path("reviewed source stamp I/O", relative))?;
            let frozen_file_key = file_key(&metadata);
            let mut file = File::open(&canonical)
                .map_err(|_| InventoryError::with_path("reviewed source open I/O", relative))?;
            let opened = file.metadata().map_err(|_| {
                InventoryError::with_path("reviewed source handle metadata I/O", relative)
            })?;
            if !opened.is_file()
                || FileStamp::from_metadata(&opened).ok().as_ref() != Some(&stamp)
                || file_key(&opened) != frozen_file_key
            {
                return Err(InventoryError::with_path(
                    "reviewed source changed before read",
                    relative,
                ));
            }
            let frozen_length = usize::try_from(stamp.len).map_err(|_| {
                InventoryError::with_path("reviewed source length overflow", relative)
            })?;
            let identity = file_identity_from_file_bounded(&mut file, frozen_length)
                .map_err(|error| identity_error(relative, error))?;
            if identity.1 < frozen_length {
                return Err(InventoryError::with_path(
                    "reviewed source truncated during read",
                    relative,
                ));
            }
            if identity.1 > frozen_length {
                return Err(InventoryError::with_path(
                    "reviewed source grew during read",
                    relative,
                ));
            }
            let post_handle = file.metadata().map_err(|_| {
                InventoryError::with_path("reviewed source post-read handle I/O", relative)
            })?;
            let post_path = fs::symlink_metadata(&canonical).map_err(|_| {
                InventoryError::with_path("reviewed source post-read path I/O", relative)
            })?;
            if post_path.file_type().is_symlink()
                || !post_path.is_file()
                || FileStamp::from_metadata(&post_handle).ok().as_ref() != Some(&stamp)
                || FileStamp::from_metadata(&post_path).ok().as_ref() != Some(&stamp)
                || file_key(&post_handle) != frozen_file_key
                || file_key(&post_path) != frozen_file_key
                || fs::canonicalize(&canonical).ok().as_ref() != Some(&canonical)
            {
                return Err(InventoryError::with_path(
                    "reviewed source changed during read",
                    relative,
                ));
            }
            let key = relative.strip_prefix("src/").expect("literal src path");
            sources.push(FrozenSource {
                relative,
                key,
                canonical,
                stamp,
                identity,
                file_key: frozen_file_key,
            });
        }
        if let Some(missing) = missing {
            return Err(InventoryError::with_path(
                "missing reviewed source",
                missing,
            ));
        }
        if let Some(extra) = extra {
            return Err(InventoryError::with_path("unexpected Rust source", extra));
        }
        Ok(Self {
            source_root: canonical_source_root.to_path_buf(),
            src_root,
            sources,
        })
    }

    pub(crate) fn sources(&self) -> &[FrozenSource] {
        &self.sources
    }

    pub(crate) fn profile_paths(&self, profile: SourceProfile) -> Vec<PathBuf> {
        profile
            .manifest()
            .iter()
            .map(|relative| self.source_root.join(relative))
            .collect()
    }

    pub(crate) fn expected_keys(&self, profile: SourceProfile) -> Vec<String> {
        profile.expected_keys()
    }

    pub(crate) fn recheck(&self) -> Result<(), InventoryError> {
        let current = Self::capture(&self.source_root)?;
        if current.src_root != self.src_root || current.sources.len() != self.sources.len() {
            return Err(err("source inventory changed"));
        }
        for (current, frozen) in current.sources.iter().zip(&self.sources) {
            if current.relative != frozen.relative
                || current.canonical != frozen.canonical
                || current.stamp != frozen.stamp
                || current.identity != frozen.identity
                || current.file_key != frozen.file_key
            {
                return Err(InventoryError::with_path(
                    "source identity changed",
                    frozen.relative,
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn validate_keys(
        &self,
        profile: SourceProfile,
        keys: &[String],
    ) -> Result<(), InventoryError> {
        let expected = profile.expected_keys();
        if keys != expected {
            return Err(err("LLVM source rows differ from frozen profile"));
        }
        let expected_set: BTreeSet<_> = expected.iter().map(String::as_str).collect();
        let master: BTreeMap<_, _> = self
            .sources
            .iter()
            .map(|source| (source.key, &source.canonical))
            .collect();
        let mut canonical_seen = BTreeSet::new();
        for key in keys {
            if !expected_set.contains(key.as_str()) {
                return Err(InventoryError::with_path(
                    "LLVM key is outside selected profile",
                    key,
                ));
            }
            let canonical = master.get(key.as_str()).ok_or_else(|| {
                InventoryError::with_path("LLVM key is outside master inventory", key)
            })?;
            let joined = self.src_root.join(key);
            require_path_text(&joined, "joined source path")?;
            let now = fs::canonicalize(&joined).map_err(|_| {
                InventoryError::with_path("LLVM row source canonicalization I/O", key)
            })?;
            if &now != *canonical || !canonical_seen.insert(now) {
                return Err(InventoryError::with_path(
                    "LLVM row source namespace mismatch",
                    key,
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
struct Record {
    relative: PathBuf,
    metadata: Result<Metadata, io::ErrorKind>,
}

fn collect_tree(src_root: &Path) -> Result<Vec<Record>, InventoryError> {
    let mut queue = BTreeSet::from([(PathBuf::new(), 0_usize)]);
    let mut directory_count = 1_usize;
    let mut records = Vec::new();
    while let Some((directory, depth)) = queue.pop_first() {
        let absolute = src_root.join(&directory);
        let mut names = Vec::new();
        let entries =
            fs::read_dir(&absolute).map_err(|_| err("source directory enumeration I/O"))?;
        for entry in entries {
            let entry = entry.map_err(|_| err("source directory entry I/O"))?;
            names.push(entry.file_name());
            if records
                .len()
                .checked_add(names.len())
                .is_none_or(|count| count > MAX_ENTRIES)
            {
                return Err(err("source entry enumeration exceeds 64"));
            }
        }
        names.sort();
        let mut children = Vec::new();
        for name in names {
            let relative = directory.join(name);
            let metadata =
                fs::symlink_metadata(src_root.join(&relative)).map_err(|error| error.kind());
            if metadata.as_ref().is_ok_and(Metadata::is_dir) {
                children.push(relative.clone());
            }
            records.push(Record { relative, metadata });
        }
        children.sort();
        for child in children {
            directory_count = directory_count
                .checked_add(1)
                .ok_or_else(|| err("source directory count overflow"))?;
            if directory_count > MAX_DIRECTORIES {
                return Err(err("source directory enumeration exceeds 8"));
            }
            let child_depth = depth
                .checked_add(1)
                .ok_or_else(|| err("source directory depth overflow"))?;
            if child_depth > MAX_DEPTH {
                return Err(err("source directory depth exceeds 4"));
            }
            queue.insert((child, child_depth));
        }
    }
    records.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(records)
}

fn validate_profile_literals() -> Result<(), InventoryError> {
    let master: BTreeSet<_> = MASTER_SOURCE_PATHS.into_iter().collect();
    for profile in [
        SourceProfile::PublicReplayV2,
        SourceProfile::StableReaderV1,
        SourceProfile::MasterLibtestMapV1,
    ] {
        let manifest = profile.manifest();
        if manifest.is_empty()
            || manifest.windows(2).any(|pair| pair[0] >= pair[1])
            || manifest.iter().any(|path| !master.contains(path))
        {
            return Err(err("frozen source profile is invalid"));
        }
        let keys = profile.expected_keys();
        if keys.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(err("frozen executable-row profile is invalid"));
        }
    }
    Ok(())
}

fn validate_records(records: &[Record]) -> Result<(), InventoryError> {
    let mut normalized = BTreeSet::new();
    for record in records {
        let normalized_name = normalize_inventory_relative(&record.relative)?;
        if !normalized.insert(normalized_name.clone()) {
            return Err(InventoryError::with_path(
                "duplicate normalized source identity",
                &normalized_name,
            ));
        }
    }
    if let Some(record) = records.iter().find(|record| record.metadata.is_err()) {
        return Err(InventoryError::with_path(
            "source metadata I/O",
            slash_path(&record.relative),
        ));
    }
    if let Some(record) = records.iter().find(|record| {
        record.metadata.as_ref().is_ok_and(|metadata| {
            let kind = metadata.file_type();
            !kind.is_file() && !kind.is_dir() && !kind.is_symlink()
        })
    }) {
        return Err(InventoryError::with_path(
            "unsupported source entry type",
            slash_path(&record.relative),
        ));
    }
    if let Some(record) = records.iter().find(|record| {
        record
            .metadata
            .as_ref()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
    }) {
        return Err(InventoryError::with_path(
            "symlink in source tree",
            slash_path(&record.relative),
        ));
    }
    Ok(())
}

fn normalize_inventory_relative(path: &Path) -> Result<String, InventoryError> {
    let name = path
        .to_str()
        .ok_or_else(|| err("source relative path is not UTF-8"))?;
    if name.is_empty() || name.len() > MAX_NAME_BYTES || name.chars().any(char::is_control) {
        return Err(err("source relative path is invalid"));
    }
    let mut normalized = String::with_capacity(name.len());
    for component in path.components() {
        let Component::Normal(component) = component else {
            return Err(InventoryError::with_path(
                "source relative path is invalid",
                name,
            ));
        };
        let component = component
            .to_str()
            .ok_or_else(|| err("source relative path is not UTF-8"))?;
        if !normalized.is_empty() {
            normalized.push('/');
        }
        normalized.push_str(component);
    }
    if normalized.is_empty() || normalized.len() > MAX_NAME_BYTES {
        return Err(err("source relative path is invalid"));
    }
    Ok(normalized)
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn require_path_text(path: &Path, label: &'static str) -> Result<(), InventoryError> {
    let text = path.to_str().ok_or_else(|| err("path is not UTF-8"))?;
    if text.len() > MAX_NAME_BYTES || text.chars().any(char::is_control) {
        return Err(InventoryError::with_path(label, "path rejected"));
    }
    Ok(())
}

fn identity_error(path: &str, error: FileIdentityError) -> InventoryError {
    match error {
        FileIdentityError::Io(_) => InventoryError::with_path("reviewed source read I/O", path),
        FileIdentityError::LengthOverflow => {
            InventoryError::with_path("reviewed source identity length overflow", path)
        }
    }
}

#[cfg(unix)]
type FileKey = (u64, u64);
#[cfg(not(unix))]
type FileKey = ();

#[cfg(unix)]
fn file_key(metadata: &Metadata) -> FileKey {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn file_key(_: &Metadata) -> FileKey {}

fn err(reason: &'static str) -> InventoryError {
    InventoryError { reason, path: None }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InventoryError {
    reason: &'static str,
    path: Option<String>,
}

impl InventoryError {
    fn with_path(reason: &'static str, path: impl Into<String>) -> Self {
        Self {
            reason,
            path: Some(path.into()),
        }
    }
}

impl fmt::Display for InventoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.reason)?;
        if let Some(path) = &self.path {
            write!(formatter, ": {path}")?;
        }
        Ok(())
    }
}

impl std::error::Error for InventoryError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

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
    const TEST_PUBLIC: [&str; 17] = [
        "src/buffered.rs",
        "src/dom.rs",
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
        "src/serialize.rs",
        "src/stream.rs",
        "src/tokenizer.rs",
        "src/writer.rs",
    ];
    const TEST_STABLE: [&str; 21] = [
        "src/dtd.rs",
        "src/dtd_catalog.rs",
        "src/dtd_controller.rs",
        "src/dtd_controller/content.rs",
        "src/dtd_frames.rs",
        "src/dtd_reference.rs",
        "src/dtd_scan.rs",
        "src/dtd_tag.rs",
        "src/encoding.rs",
        "src/error.rs",
        "src/escape.rs",
        "src/name.rs",
        "src/origin.rs",
        "src/parser.rs",
        "src/reader.rs",
        "src/reader/dtd.rs",
        "src/stream/dtd.rs",
        "src/tokenizer.rs",
        "src/tokenizer/local.rs",
        "src/tokenizer/reference.rs",
        "src/tokenizer/text.rs",
    ];

    fn fixture_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "yaxml-source-inventory-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("source/src")).unwrap();
        for relative in TEST_MASTER {
            let path = root.join("source").join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"// fixture\n").unwrap();
        }
        root
    }

    fn capture(root: &Path) -> Result<SourceInventory, InventoryError> {
        SourceInventory::capture(&fs::canonicalize(root.join("source")).unwrap())
    }

    #[test]
    fn literal_profiles_and_collision_keys_are_frozen() {
        assert_eq!(MASTER_SOURCE_PATHS, TEST_MASTER);
        assert_eq!(SourceProfile::PublicReplayV2.manifest(), TEST_PUBLIC);
        assert_eq!(SourceProfile::StableReaderV1.manifest(), TEST_STABLE);
        assert_eq!(MASTER_SOURCE_PATHS.len(), 32);
        assert_eq!(SourceProfile::PublicReplayV2.manifest().len(), 17);
        assert_eq!(SourceProfile::PublicReplayV2.expected_keys().len(), 16);
        assert_eq!(SourceProfile::StableReaderV1.manifest().len(), 21);
        assert_eq!(SourceProfile::StableReaderV1.expected_keys().len(), 21);
        assert_eq!(SourceProfile::MasterLibtestMapV1.manifest().len(), 32);
        assert_eq!(SourceProfile::MasterLibtestMapV1.expected_keys().len(), 31);
        let master = SourceProfile::MasterLibtestMapV1.expected_keys();
        assert!(master.contains(&"dtd.rs".to_owned()));
        assert!(master.contains(&"reader/dtd.rs".to_owned()));
        assert!(master.contains(&"stream/dtd.rs".to_owned()));
        assert!(master.contains(&"dtd_controller/content.rs".to_owned()));
        assert!(!master.contains(&"lib.rs".to_owned()));
        assert!(
            !SourceProfile::StableReaderV1
                .expected_keys()
                .contains(&"stream.rs".to_owned())
        );
    }

    #[test]
    fn exact_inventory_catches_missing_extra_symlink_and_mixed_precedence() {
        let root = fixture_root("membership");
        assert_eq!(capture(&root).unwrap().sources().len(), 32);
        let missing_path = root.join("source/src/writer.rs");
        fs::remove_file(&missing_path).unwrap();
        fs::write(root.join("source/src/extra.rs"), b"extra").unwrap();
        assert!(
            capture(&root)
                .unwrap_err()
                .to_string()
                .contains("missing reviewed source: src/writer.rs")
        );
        fs::write(&missing_path, b"restored").unwrap();
        assert!(
            capture(&root)
                .unwrap_err()
                .to_string()
                .contains("unexpected Rust source: src/extra.rs")
        );
        fs::remove_file(root.join("source/src/extra.rs")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("writer.rs", root.join("source/src/link.txt")).unwrap();
            assert!(
                capture(&root)
                    .unwrap_err()
                    .to_string()
                    .contains("symlink in source tree: link.txt")
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn semantic_precedence_is_independent_of_fault_creation_order() {
        for reverse in [false, true] {
            let root = fixture_root(if reverse {
                "precedence-b"
            } else {
                "precedence-a"
            });
            let missing = root.join("source/src/buffered.rs");
            let extra = root.join("source/src/extra.rs");
            if reverse {
                fs::write(&extra, b"extra").unwrap();
                fs::remove_file(&missing).unwrap();
            } else {
                fs::remove_file(&missing).unwrap();
                fs::write(&extra, b"extra").unwrap();
            }
            assert_eq!(
                capture(&root).unwrap_err().to_string(),
                "missing reviewed source: src/buffered.rs"
            );

            fs::remove_file(&extra).unwrap();
            let bad_expected = root.join("source/src/writer.rs");
            fs::remove_file(&bad_expected).unwrap();
            fs::create_dir(&bad_expected).unwrap();
            assert_eq!(
                capture(&root).unwrap_err().to_string(),
                "reviewed source is not a real file: src/writer.rs"
            );
            fs::remove_dir_all(root).unwrap();
        }

        #[cfg(unix)]
        for reverse in [false, true] {
            use std::os::unix::fs::symlink;

            let root = fixture_root(if reverse { "lexical-b" } else { "lexical-a" });
            let invalid = root.join("source/src/bad-\n.rs");
            let link = root.join("source/src/link.txt");
            if reverse {
                symlink("writer.rs", &link).unwrap();
                fs::write(&invalid, b"invalid name").unwrap();
            } else {
                fs::write(&invalid, b"invalid name").unwrap();
                symlink("writer.rs", &link).unwrap();
            }
            assert_eq!(
                capture(&root).unwrap_err().to_string(),
                "source relative path is invalid"
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn traversal_limits_count_ignored_entries_and_directories() {
        let root = fixture_root("limits");
        for index in 0..27 {
            fs::write(root.join(format!("source/src/filler-{index:02}.txt")), b"").unwrap();
        }
        assert!(
            capture(&root).is_ok(),
            "37 base entries plus 27 ignored entries is the exact cap"
        );
        fs::write(root.join("source/src/filler-over.txt"), b"").unwrap();
        assert_eq!(
            capture(&root).unwrap_err().to_string(),
            "source entry enumeration exceeds 64"
        );
        fs::remove_file(root.join("source/src/filler-over.txt")).unwrap();
        for index in 0..27 {
            fs::remove_file(root.join(format!("source/src/filler-{index:02}.txt"))).unwrap();
        }
        for index in 0..2 {
            fs::create_dir(root.join(format!("source/src/dir-{index}"))).unwrap();
        }
        assert!(
            capture(&root).is_ok(),
            "src plus five reviewed and two empty directories is the cap"
        );
        fs::create_dir(root.join("source/src/dir-over")).unwrap();
        assert_eq!(
            capture(&root).unwrap_err().to_string(),
            "source directory enumeration exceeds 8"
        );
        fs::remove_dir_all(root).unwrap();

        let root = std::env::temp_dir().join(format!("yaxml-source-depth-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut path = root.join("src");
        fs::create_dir_all(&path).unwrap();
        for index in 0..4 {
            path.push(format!("d{index}"));
            fs::create_dir(&path).unwrap();
        }
        assert!(collect_tree(&root.join("src")).is_ok());
        path.push("over");
        fs::create_dir(&path).unwrap();
        assert_eq!(
            collect_tree(&root.join("src")).unwrap_err().to_string(),
            "source directory depth exceeds 4"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_content_caps_apply_before_reads_and_rechecks_do_not_rebaseline() {
        let root = fixture_root("content");
        let first = root.join("source/src/buffered.rs");
        fs::write(&first, vec![b'a'; MAX_SOURCE_BYTES as usize]).unwrap();
        let inventory = capture(&root).unwrap();
        fs::write(&first, vec![b'a'; MAX_SOURCE_BYTES as usize + 1]).unwrap();
        assert!(
            capture(&root)
                .unwrap_err()
                .to_string()
                .contains("exceeds 1 MiB")
        );
        assert!(inventory.recheck().is_err());
        fs::write(&first, vec![b'b'; MAX_SOURCE_BYTES as usize]).unwrap();
        assert!(
            inventory.recheck().is_err(),
            "restoring bytes must not refresh the frozen stamp"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn aggregate_content_cap_accepts_exactly_eight_mib_and_rejects_one_more() {
        let root = fixture_root("aggregate");
        for relative in MASTER_SOURCE_PATHS {
            fs::write(root.join("source").join(relative), b"").unwrap();
        }
        for relative in MASTER_SOURCE_PATHS.iter().take(8) {
            fs::write(
                root.join("source").join(relative),
                vec![b'x'; MAX_SOURCE_BYTES as usize],
            )
            .unwrap();
        }
        assert!(capture(&root).is_ok());
        fs::write(root.join("source").join(MASTER_SOURCE_PATHS[8]), b"x").unwrap();
        assert_eq!(
            capture(&root).unwrap_err().to_string(),
            "reviewed sources exceed 8 MiB aggregate"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn src_relative_mapping_is_exact_and_revalidates_namespace() {
        let root = fixture_root("space ☃");
        let inventory = capture(&root).unwrap();
        for profile in [
            SourceProfile::PublicReplayV2,
            SourceProfile::StableReaderV1,
            SourceProfile::MasterLibtestMapV1,
        ] {
            let keys = profile.expected_keys();
            inventory.validate_keys(profile, &keys).unwrap();
            let mut basename_alias = keys.clone();
            if let Some(key) = basename_alias
                .iter_mut()
                .find(|key| key.as_str() == "reader/dtd.rs")
            {
                *key = "dtd.rs".to_owned();
                basename_alias.sort();
                assert!(inventory.validate_keys(profile, &basename_alias).is_err());
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inventory_relative_names_enforce_utf8_control_and_byte_caps() {
        assert_eq!(
            normalize_inventory_relative(Path::new("reader/dtd.rs")).unwrap(),
            "reader/dtd.rs"
        );
        assert_eq!(
            normalize_inventory_relative(Path::new(&"a".repeat(MAX_NAME_BYTES)))
                .unwrap()
                .len(),
            MAX_NAME_BYTES
        );
        assert_eq!(
            normalize_inventory_relative(Path::new(&"a".repeat(MAX_NAME_BYTES + 1)))
                .unwrap_err()
                .to_string(),
            "source relative path is invalid"
        );
        assert!(normalize_inventory_relative(Path::new("bad\nname.rs")).is_err());
        assert!(normalize_inventory_relative(Path::new("../escape.rs")).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let invalid = Path::new(std::ffi::OsStr::from_bytes(b"bad-\xff.rs"));
            assert_eq!(
                normalize_inventory_relative(invalid)
                    .unwrap_err()
                    .to_string(),
                "source relative path is not UTF-8"
            );
        }
    }
}
