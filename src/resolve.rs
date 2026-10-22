//! Caller-controlled loading of external resources.

use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use crate::encoding::charset;
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
    /// A document named by an `xi:include` with `parse="xml"`.
    XInclude,
    /// A resource named by an `xi:include` with `parse="text"`.
    XIncludeText,
    /// An XML or SGML catalog file loaded by [`crate::catalog::Catalog`].
    Catalog,
    /// An XML Schema document included, imported or redefined by
    /// [`crate::xsd::Schema`], or named by `xsi:schemaLocation`.
    Schema,
    /// A schema document named by a RELAX NG `include` or `externalRef`.
    RelaxNg,
}

/// One load request passed to a [`Resolver`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request<'a> {
    /// What the identifier names.
    pub kind: ResourceKind,
    /// The system identifier resolved against the base of the resource that
    /// declared it (the document's base for the subset and internal declarations).
    pub uri: &'a str,
    /// The public identifier, if declared.
    pub public_id: Option<&'a str>,
    /// Bytes left under [`Limits::max_external_bytes`]; a longer response
    /// fails with [`Limit::ExternalBytes`], so read at most one byte more.
    pub max_bytes: usize,
}

/// Supplies external resources; without one, nothing is ever loaded.
///
/// libxml2's custom resource loader. It is passed in
/// [`ParseOptions`](crate::ParseOptions), whose flags decide what is
/// requested: `DTDLOAD` the external subset, `NOENT` or `DTDVALID` external
/// entities, never anything with `NO_XXE`. Loads are bounded by
/// [`Limits::max_external_loads`] and [`Limits::max_external_bytes`].
/// Closures implement this trait.
///
/// ```
/// use yaxml::{Document, ParseOptions, Request, options};
///
/// let dtd = "<!ATTLIST r id ID #IMPLIED><!ENTITY e 'loaded'>";
/// let resolver = move |request: Request<'_>| {
///     Ok((request.uri == "dtd/r.dtd").then(|| dtd.as_bytes().to_vec()))
/// };
/// let options = ParseOptions::new(options::DTDLOAD | options::NOENT)
///     .with_resolver("dtd/doc.xml", Box::new(resolver));
/// let document = Document::parse_with(b"<!DOCTYPE r SYSTEM 'r.dtd'><r id='a'>&e;</r>", options)?;
/// let element = document.element_by_id("a").unwrap();
/// assert_eq!(element.text_chunks().collect::<String>(), "loaded");
/// # Ok::<(), yaxml::Error>(())
/// ```
pub trait Resolver: Send {
    /// Loads one resource.
    ///
    /// # Errors
    ///
    /// `Ok(None)` declines, like libxml2's missing-file warning: the subset
    /// is skipped and an entity reference expands to nothing. `Err` aborts
    /// parsing with [`ErrorKind::ExternalResource`].
    fn resolve(&mut self, request: Request<'_>) -> io::Result<Option<Vec<u8>>>;

    /// Receives the `catalog` of each `<?oasis-xml-catalog?>` instruction
    /// outside the DTD when [`CATALOG_PI`](crate::options::CATALOG_PI) is
    /// set (libxml2's `xmlCatalogAddLocal`).
    ///
    /// The default ignores it; [`CatalogResolver`](crate::catalog::CatalogResolver)
    /// uses it for the rest of the parse.
    fn catalog_pi(&mut self, catalog: &str) {
        let _ = catalog;
    }
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
    /// A resolver confined to the directory `root`.
    ///
    /// # Errors
    ///
    /// `root` cannot be canonicalized or is not a directory.
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
        let path = match parsed
            .as_ref()
            .map(|u| (u.scheme.as_deref(), &u.host, &u.path))
        {
            Some((Some("file"), None, Some(path))) => path.as_str(),
            Some((Some("file"), ..)) => return Err(denied("file URI with a host")),
            Some((Some(_), ..)) => return Err(denied("not a file URI")),
            _ => request.uri,
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

/// Handles to one resolver for several consumers in turn (parsing, then
/// XInclude); calls never overlap.
pub(crate) fn share(resolver: Box<dyn Resolver>) -> impl Fn() -> Box<dyn Resolver> {
    struct Shared(std::sync::Arc<std::sync::Mutex<Box<dyn Resolver>>>);
    impl Resolver for Shared {
        fn resolve(&mut self, request: Request<'_>) -> io::Result<Option<Vec<u8>>> {
            let mut resolver =
                (self.0.lock()).map_err(|_| io::Error::other("resolver panicked"))?;
            resolver.resolve(request)
        }

        fn catalog_pi(&mut self, catalog: &str) {
            if let Ok(mut resolver) = self.0.lock() {
                resolver.catalog_pi(catalog);
            }
        }
    }
    let shared = std::sync::Arc::new(std::sync::Mutex::new(resolver));
    move || Box::new(Shared(shared.clone()))
}

/// The resolver with its document base and cumulative budgets.
pub(crate) struct Loader {
    resolver: Box<dyn Resolver>,
    /// What the parser options allow to load; other requests are declined.
    flags: crate::options::Flags,
    /// Base of the resource whose declarations are being processed.
    pub(crate) base: String,
    loads_left: usize,
    bytes_left: usize,
    max_bytes: usize,
}

impl Loader {
    pub(crate) fn new(base: &str, resolver: Box<dyn Resolver>, limits: &Limits) -> Self {
        Self {
            resolver,
            base: base.to_owned(),
            flags: crate::options::Flags::LOAD_ALL,
            loads_left: limits.max_external_loads,
            bytes_left: limits.max_external_bytes,
            max_bytes: limits.max_external_bytes,
        }
    }

    /// Declines what `flags` do not load, without consulting the resolver.
    pub(crate) fn with_flags(self, flags: crate::options::Flags) -> Self {
        Self { flags, ..self }
    }

    /// Passes a document's `oasis-xml-catalog` instruction to the resolver.
    pub(crate) fn catalog_pi(&mut self, catalog: &str) {
        self.resolver.catalog_pi(catalog);
    }

    /// Bytes the resolver has returned so far.
    pub(crate) fn loaded(&self) -> usize {
        self.max_bytes - self.bytes_left
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
        if !self.flags.loads(kind) {
            return Ok(None);
        }
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
    let mut output = Vec::new();
    let switch = crate::encoding::declared(bytes);
    let (_, decoded) = crate::encoding::transcode(bytes, switch, None, usize::MAX, &mut output);
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
    let (declaration, _) = crate::parser::parse_declaration(&rest[..end], 0, true, &mut Err)
        .map_err(|error| error.kind)?;
    // libxml2: "Version mismatch between document and entity".
    if declaration.version != "1.0" {
        return Err(ErrorKind::InvalidDeclaration);
    }
    declaration
        .encoding
        .and_then(charset)
        .ok_or(ErrorKind::UnsupportedEncoding)?;
    Ok(rest[end + 2..].to_owned())
}

#[cfg(test)]
#[path = "../tests/internal/resolve.rs"]
mod tests;
