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
    identity_from_reader(file, None)
}

#[cfg(test)]
pub(crate) fn file_identity_from_file_bounded(
    file: &mut File,
    frozen_length: usize,
) -> Result<ContentIdentity, FileIdentityError> {
    content_identity_from_reader_bounded(file, frozen_length)
}

#[cfg(test)]
pub(crate) fn content_identity_from_reader_bounded(
    reader: &mut impl Read,
    frozen_length: usize,
) -> Result<ContentIdentity, FileIdentityError> {
    let maximum_bytes = frozen_length
        .checked_add(1)
        .ok_or(FileIdentityError::LengthOverflow)?;
    identity_from_reader(reader, Some(maximum_bytes))
}

fn identity_from_reader(
    reader: &mut impl Read,
    maximum_bytes: Option<usize>,
) -> Result<ContentIdentity, FileIdentityError> {
    let mut identity = FnvIdentity::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut remaining = maximum_bytes;
    loop {
        let available = remaining
            .map(|remaining| remaining.min(buffer.len()))
            .unwrap_or(buffer.len());
        if available == 0 {
            return Ok(identity.finish());
        }
        let read = reader
            .read(&mut buffer[..available])
            .map_err(FileIdentityError::Io)?;
        if read == 0 {
            return Ok(identity.finish());
        }
        identity.update(&buffer[..read])?;
        if let Some(remaining) = &mut remaining {
            *remaining -= read;
        }
    }
}

pub(crate) fn file_stamp(path: &Path) -> io::Result<FileStamp> {
    FileStamp::from_metadata(&fs::metadata(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::{Cursor, Error, ErrorKind, Seek, SeekFrom, Write};

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

    #[test]
    fn bounded_reader_hashes_the_exact_bound_and_one_growth_byte() {
        let exact = b"exact";
        let mut exact_reader = Cursor::new(exact);
        assert_eq!(
            content_identity_from_reader_bounded(&mut exact_reader, exact.len()).unwrap(),
            content_identity(exact)
        );

        let one_more = b"exact+";
        let mut one_more_reader = Cursor::new(one_more);
        assert_eq!(
            content_identity_from_reader_bounded(&mut one_more_reader, exact.len()).unwrap(),
            content_identity(one_more)
        );

        let two_more = b"exact++";
        let mut two_more_reader = Cursor::new(two_more);
        assert_eq!(
            content_identity_from_reader_bounded(&mut two_more_reader, exact.len()).unwrap(),
            content_identity(one_more)
        );
    }

    #[test]
    fn bounded_reader_preserves_read_errors() {
        struct FailsAfterOneByte(bool);

        impl Read for FailsAfterOneByte {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if self.0 {
                    return Err(Error::new(ErrorKind::PermissionDenied, "fixture failure"));
                }
                self.0 = true;
                buffer[0] = b'x';
                Ok(1)
            }
        }

        let error =
            content_identity_from_reader_bounded(&mut FailsAfterOneByte(false), 1).unwrap_err();
        assert!(matches!(
            error,
            FileIdentityError::Io(error) if error.kind() == ErrorKind::PermissionDenied
        ));
    }

    #[test]
    fn bounded_reader_stops_after_the_growth_probe_and_checks_overflow_first() {
        struct Endless {
            bytes: usize,
            largest_request: usize,
        }

        impl Read for Endless {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                assert!(self.bytes < 2, "bounded reader requested a third byte");
                self.largest_request = self.largest_request.max(buffer.len());
                buffer[0] = b'a';
                self.bytes += 1;
                Ok(1)
            }
        }

        let mut one = Endless {
            bytes: 0,
            largest_request: 0,
        };
        assert_eq!(
            content_identity_from_reader_bounded(&mut one, 1).unwrap(),
            content_identity(b"aa")
        );
        assert_eq!(one.bytes, 2);
        assert!(one.largest_request <= 2);

        let mut zero = Endless {
            bytes: 0,
            largest_request: 0,
        };
        assert_eq!(
            content_identity_from_reader_bounded(&mut zero, 0).unwrap(),
            content_identity(b"a")
        );
        assert_eq!(zero.bytes, 1);
        assert_eq!(zero.largest_request, 1);

        let mut unread = Endless {
            bytes: 0,
            largest_request: 0,
        };
        assert!(matches!(
            content_identity_from_reader_bounded(&mut unread, usize::MAX),
            Err(FileIdentityError::LengthOverflow)
        ));
        assert_eq!(unread.bytes, 0);
        assert_eq!(unread.largest_request, 0);
    }

    #[test]
    fn bounded_reader_preserves_immediate_prefix_and_probe_errors() {
        struct FailsAt {
            bytes_before_error: usize,
            bytes: usize,
            kind: ErrorKind,
        }

        impl Read for FailsAt {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if self.bytes == self.bytes_before_error {
                    return Err(Error::new(self.kind, "bounded fixture error"));
                }
                buffer[0] = b'a';
                self.bytes += 1;
                Ok(1)
            }
        }

        for (bytes_before_error, kind) in [
            (0, ErrorKind::PermissionDenied),
            (1, ErrorKind::BrokenPipe),
            (2, ErrorKind::UnexpectedEof),
        ] {
            let mut reader = FailsAt {
                bytes_before_error,
                bytes: 0,
                kind,
            };
            let error = content_identity_from_reader_bounded(&mut reader, 2).unwrap_err();
            assert!(matches!(error, FileIdentityError::Io(error) if error.kind() == kind));
        }
    }

    #[test]
    fn shared_identity_engines_agree_across_the_buffer_boundary() {
        for length in [65_535usize, 65_536, 65_537] {
            let bytes: Vec<u8> = (0..length).map(|offset| (offset % 251) as u8).collect();
            let expected = content_identity(&bytes);
            let mut cursor = Cursor::new(&bytes);
            assert_eq!(
                content_identity_from_reader_bounded(&mut cursor, length).unwrap(),
                expected
            );

            let serial = NEXT_IDENTITY_FILE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "yaxml-file-identity-boundary-{}-{serial}",
                std::process::id()
            ));
            std::fs::write(&path, &bytes).unwrap();
            assert_eq!(file_identity(&path).unwrap(), expected);
            let mut file = File::open(&path).unwrap();
            assert_eq!(
                file_identity_from_file_bounded(&mut file, length).unwrap(),
                expected
            );
            std::fs::remove_file(path).unwrap();
        }
        assert_eq!(content_identity(b""), (0xcbf29ce484222325, 0));
        assert_eq!(content_identity(b"a"), (0xaf63dc4c8601ec8c, 1));
    }
}
