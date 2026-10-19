//! Caller-controlled loading of external DTD subsets and parsed entities.

use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use crate::dtd::DtdLimits;
use crate::encoding::{DecodeProgress, Decoded, Decoder, SourceEncoding, charset, declared_label};
use crate::escape::xml_char;
use crate::{ErrorKind, Limit, Limits, Uri};

/// What an external identifier names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceKind {
    /// The external subset named by the document type declaration.
    DtdSubset,
    /// An external parameter entity referenced in the DTD.
    ParameterEntity,
    /// An external parsed general entity referenced in content.
    GeneralEntity,
}

/// One load request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request<'a> {
    pub kind: ResourceKind,
    /// The system identifier resolved against the base of the resource that
    /// declared it (the document's base for the subset and internal declarations).
    pub uri: &'a str,
    pub public_id: Option<&'a str>,
    /// Bytes left under `Limits::max_external_bytes`; a longer response fails
    /// with `LimitExceeded(ExternalBytes)`, so read at most one byte more.
    pub max_bytes: usize,
}

/// Supplies external resources. Without one, nothing is ever loaded.
///
/// `Ok(None)` declines, like libxml2's missing-file warning: the subset is
/// skipped and an entity reference expands to nothing. `Err` aborts parsing
/// with [`ErrorKind::ExternalResource`]. Closures implement this trait.
pub trait Resolver: Send {
    fn resolve(&mut self, request: Request<'_>) -> io::Result<Option<Vec<u8>>>;
}

impl<F: FnMut(Request<'_>) -> io::Result<Option<Vec<u8>>> + Send> Resolver for F {
    fn resolve(&mut self, request: Request<'_>) -> io::Result<Option<Vec<u8>>> {
        self(request)
    }
}

/// Serves paths and `file:` URIs inside one directory; never the network.
///
/// Relative paths resolve against the root. A path that leaves the root,
/// lexically or through a symbolic link, or a non-`file` URI is an error; a
/// missing file declines. Checks precede opening but are not atomic with it.
#[derive(Clone, Debug)]
pub struct FileResolver {
    root: PathBuf,
}

impl FileResolver {
    pub fn new(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = root.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "root is not a directory",
            ));
        }
        Ok(Self { root })
    }
}

impl Resolver for FileResolver {
    fn resolve(&mut self, request: Request<'_>) -> io::Result<Option<Vec<u8>>> {
        let denied = |why| io::Error::new(io::ErrorKind::PermissionDenied, why);
        let parsed = Uri::parse(request.uri).ok();
        let path = match parsed.as_ref().and_then(|uri| uri.scheme.as_deref()) {
            None => request.uri,
            Some("file") => match parsed.as_ref().expect("parsed URI") {
                Uri {
                    host: None,
                    path: Some(path),
                    ..
                } => path,
                _ => return Err(denied("file URI with a host")),
            },
            Some(_) => return Err(denied("not a file URI")),
        };
        let lexical =
            self.root
                .join(path)
                .components()
                .fold(PathBuf::new(), |mut path, component| {
                    match component {
                        Component::ParentDir => {
                            path.pop();
                        }
                        Component::CurDir => {}
                        other => path.push(other),
                    }
                    path
                });
        if !lexical.starts_with(&self.root) {
            return Err(denied("path leaves the resolver root"));
        }
        let resolved = match lexical.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !resolved.starts_with(&self.root) {
            return Err(denied("link leaves the resolver root"));
        }
        let file = std::fs::File::open(&resolved)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        let mut bytes = Vec::new();
        file.take(request.max_bytes as u64 + 1)
            .read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }
}

/// The resolver with its document base and cumulative budgets.
pub(crate) struct Loader {
    resolver: Box<dyn Resolver>,
    /// Base of the resource whose declarations are being processed.
    pub(crate) base: String,
    pub(crate) unit: DtdLimits,
    loads_left: usize,
    bytes_left: usize,
}

impl Loader {
    pub(crate) fn new(
        base: &str,
        resolver: Box<dyn Resolver>,
        limits: &Limits,
        unit: DtdLimits,
    ) -> Self {
        Self {
            resolver,
            base: base.to_owned(),
            unit,
            loads_left: limits.max_external_loads,
            bytes_left: limits.max_external_bytes,
        }
    }

    /// Resolves `system` against the current base, as libxml2's `xmlBuildURI`.
    pub(crate) fn uri(&self, system: &str) -> String {
        crate::uri::resolve(system, &self.base).unwrap_or_else(|_| system.to_owned())
    }

    /// Requests and decodes one resource; `None` when the resolver declines.
    pub(crate) fn load(
        &mut self,
        kind: ResourceKind,
        uri: &str,
        public_id: Option<&str>,
    ) -> Result<Option<String>, ErrorKind> {
        self.loads_left = self
            .loads_left
            .checked_sub(1)
            .ok_or(ErrorKind::LimitExceeded(Limit::ExternalLoads))?;
        let request = Request {
            kind,
            uri,
            public_id,
            max_bytes: self.bytes_left,
        };
        let Some(bytes) = self
            .resolver
            .resolve(request)
            .map_err(|_| ErrorKind::ExternalResource)?
        else {
            return Ok(None);
        };
        self.bytes_left = self
            .bytes_left
            .checked_sub(bytes.len())
            .ok_or(ErrorKind::LimitExceeded(Limit::ExternalBytes))?;
        decode(&bytes).map(Some)
    }
}

/// Decodes an external parsed entity: BOM or declared encoding, character
/// validity, line ends and the leading text declaration, which is removed.
pub(crate) fn decode(bytes: &[u8]) -> Result<String, ErrorKind> {
    let mut decoder = Decoder::new(usize::MAX);
    let mut output = Vec::new();
    let mut sink = |decoded: Decoded<'_>| {
        let consumed = match decoded {
            Decoded::Start { .. } => 0,
            Decoded::Bytes(bytes) => {
                output
                    .try_reserve(bytes.len())
                    .map_err(|_| crate::Error::new(ErrorKind::Allocation, 0))?;
                output.extend_from_slice(bytes);
                bytes.len()
            }
        };
        Ok(DecodeProgress {
            consumed,
            suspended: false,
        })
    };
    let byte_charset = declared_label(bytes)
        .and_then(charset)
        .and_then(|charset| match charset {
            crate::encoding::Charset::Byte(table) => Some(table),
            _ => None,
        });
    let split = byte_charset.map_or(0, |_| {
        memchr::memmem::find(bytes, b"?>").expect("declared label is terminated") + 2
    });
    let decoded = (|| {
        decoder.advance(&bytes[..split], &mut sink)?;
        if let Some(table) = byte_charset {
            decoder.switch(SourceEncoding::Byte(table));
        }
        decoder.advance(&bytes[split..], &mut sink)?;
        decoder.finish_step(&mut sink)
    })();
    decoded.map_err(|error| error.kind)?;
    let text = String::from_utf8(output).map_err(|_| ErrorKind::InvalidUtf8)?;
    if !text.chars().all(xml_char) {
        return Err(ErrorKind::InvalidCharacter);
    }
    let text = match text.contains('\r') {
        true => text.replace("\r\n", "\n").replace('\r', "\n"),
        false => text,
    };
    let Some(rest) = text
        .strip_prefix("<?xml")
        .filter(|rest| rest.starts_with([' ', '\t', '\n']))
    else {
        return Ok(text);
    };
    let end = rest.find("?>").ok_or(ErrorKind::InvalidDeclaration)?;
    let (declaration, _) =
        crate::parser::parse_declaration(&rest[..end], 0, true).map_err(|error| error.kind)?;
    declaration
        .encoding
        .and_then(charset)
        .ok_or(ErrorKind::UnsupportedEncoding)?;
    Ok(rest[end + 2..].to_owned())
}

#[cfg(test)]
#[path = "../tests/internal/resolve.rs"]
mod tests;
