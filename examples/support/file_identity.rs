//! Shared accidental-change identities for campaign inputs.
#![forbid(unsafe_code)]

use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::path::Path;
use std::time::SystemTime;

pub(crate) type ContentIdentity = (u64, usize);

#[derive(Debug)]
pub(crate) enum FileIdentityError {
    Io(io::Error),
    LengthOverflow,
}

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FileStamp {
    pub(crate) len: u64,
    pub(crate) modified: SystemTime,
}

impl FileStamp {
    pub(crate) fn from_metadata(metadata: &Metadata) -> io::Result<Self> {
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified()?,
        })
    }
}

struct FnvIdentity {
    hash: u64,
    length: usize,
}

impl FnvIdentity {
    fn new() -> Self {
        Self {
            hash: FNV_OFFSET,
            length: 0,
        }
    }

    fn update(&mut self, bytes: &[u8]) -> Result<(), FileIdentityError> {
        self.length = self
            .length
            .checked_add(bytes.len())
            .ok_or(FileIdentityError::LengthOverflow)?;
        for byte in bytes {
            self.hash ^= u64::from(*byte);
            self.hash = self.hash.wrapping_mul(FNV_PRIME);
        }
        Ok(())
    }

    fn finish(self) -> ContentIdentity {
        (self.hash, self.length)
    }
}

pub(crate) fn content_identity(bytes: &[u8]) -> ContentIdentity {
    let mut identity = FnvIdentity::new();
    identity
        .update(bytes)
        .expect("a byte slice length always fits usize");
    identity.finish()
}

pub(crate) fn file_identity(path: &Path) -> io::Result<ContentIdentity> {
    let mut file = File::open(path)?;
    file_identity_from_file(&mut file).map_err(|error| match error {
        FileIdentityError::Io(error) => error,
        FileIdentityError::LengthOverflow => io::Error::other("file length overflow"),
    })
}

pub(crate) fn file_identity_from_file(
    file: &mut File,
) -> Result<ContentIdentity, FileIdentityError> {
    let mut identity = FnvIdentity::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(FileIdentityError::Io)?;
        if read == 0 {
            return Ok(identity.finish());
        }
        identity.update(&buffer[..read])?;
    }
}

pub(crate) fn file_stamp(path: &Path) -> io::Result<FileStamp> {
    FileStamp::from_metadata(&fs::metadata(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};

    static NEXT_IDENTITY_FILE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    #[test]
    fn slice_and_file_identities_share_one_incremental_implementation() {
        let serial = NEXT_IDENTITY_FILE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yaxml-file-identity-{}-{serial}",
            std::process::id()
        ));
        let bytes = vec![0xa5; 64 * 1024 + 17];
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.write_all(&bytes).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();

        assert_eq!(
            file_identity_from_file(&mut file).unwrap(),
            content_identity(&bytes)
        );
        assert_eq!(file_identity(&path).unwrap(), content_identity(&bytes));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn preserves_the_existing_fnv_identity_values() {
        assert_eq!(content_identity(b""), (0xcbf29ce484222325, 0));
        assert_eq!(content_identity(b"a"), (0xaf63dc4c8601ec8c, 1));
    }
}
