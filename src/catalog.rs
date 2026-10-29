//! OASIS XML and SGML catalogs, libxml2's `catalog.c`, without global state.
//!
//! A [`Catalog`] is built from bytes or loaded through a [`Resolver`], which
//! also fetches `nextCatalog`, delegate and SGML `CATALOG` files lazily,
//! within [`Limits`]. [`CatalogResolver`] maps identifiers before another
//! resolver fetches them, so any parser gains catalog support. Details:
//! <https://github.com/pbdeuchler/yaxml/blob/main/docs/catalog.md>.
//!
//! ```
//! use yaxml::catalog::{Catalog, CatalogResolver};
//!
//! let xml = br#"<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog">
//!   <public publicId="-//EX//DTD R//EN" uri="local/r.dtd"/>
//!   <rewriteSystem systemIdStartString="http://example.org/" rewritePrefix="mirror/"/>
//! </catalog>"#;
//! let none = |_: yaxml::Request<'_>| Ok(None);
//! let mut catalog = Catalog::parse(xml, Default::default(), "cat.xml", Box::new(none))?;
//! assert_eq!(catalog.resolve(Some("-//EX//DTD  R//EN"), None).as_deref(), Some("local/r.dtd"));
//! let system = catalog.resolve(None, Some("http://example.org/s.dtd"));
//! assert_eq!(system.as_deref(), Some("mirror/s.dtd"));
//!
//! let dtd = |request: yaxml::Request<'_>| {
//!     Ok((request.uri == "local/r.dtd").then(|| b"<!ENTITY e 'mapped'>".to_vec()))
//! };
//! let resolver = CatalogResolver::new(catalog, Box::new(dtd));
//! let input = b"<!DOCTYPE r PUBLIC '-//EX//DTD R//EN' 'http://example.com/r.dtd'><r>&e;</r>";
//! use yaxml::options::{DTDLOAD, NOENT};
//! let options = yaxml::ParseOptions::new(DTDLOAD | NOENT).with_resolver("doc.xml", Box::new(resolver));
//! let document = yaxml::Document::parse_with(input, options)?;
//! assert_eq!(document.root_element().text_chunks().collect::<String>(), "mapped");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet, TryReserveError};
use std::{fmt, io};

use crate::diagnostic::{Diagnostic, Domain, LineIndex, code};
use crate::fallible::TryPush;
use crate::save::{ATTRIBUTE, escape_into};
use crate::uri::{canonic_path, resolve as build};
use crate::xpath::tree::XML_NAMESPACE;
use crate::{OwnedDocument, OwnedNode, Request, Resolver, ResourceKind};

const NAMESPACE: &str = "urn:oasis:names:tc:entity:xmlns:xml:catalog";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";
/// The prolog `xmlACatalogDump` writes before the `catalog` element.
const DOCTYPE: &str = "<?xml version=\"1.0\"?>\n<!DOCTYPE catalog PUBLIC \"-//OASIS//DTD Entity Resolution XML Catalog V1.0//EN\" \"http://www.oasis-open.org/committees/entity/release/1.0/catalog.dtd\">\n";
const URN: &str = "urn:publicid:";
/// libxml2's `MAX_DELEGATE`: distinct delegate catalogs remembered per step.
const MAX_DELEGATE: usize = 50;
/// Diagnostics a catalog keeps (libxml2 has no bound), as the parser's.
const MAX_DIAGNOSTICS: usize = 100;

/// Budgets for catalog files fetched through the resolver. libxml2 has none.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Limits {
    /// Catalog files requested, including the one [`Catalog::load`] fetches.
    pub max_loads: usize,
    /// Bytes across all of those files.
    pub max_bytes: usize,
    /// Nesting of catalogs while loading SGML catalogs or resolving
    /// (libxml2's `MAX_CATAL_DEPTH`, 50); deeper catalogs are skipped.
    pub max_depth: usize,
    /// Catalogs a document adds with `<?oasis-xml-catalog?>` (see
    /// [`CatalogResolver`]); later ones are ignored. libxml2 has no bound.
    pub max_local: usize,
}

setters!(const Limits { max_loads: usize, max_bytes: usize, max_depth: usize, max_local: usize });

impl Limits {
    /// The defaults: 64 loads, 16 MiB, depth 50 and 64 local catalogs.
    pub const DEFAULT: Self = Self {
        max_loads: 64,
        max_bytes: 1 << 24,
        max_depth: 50,
        max_local: 64,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Why a catalog could not be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// The resolver declined the catalog file.
    Missing,
    /// The resolver failed.
    Io(io::ErrorKind),
    /// [`Limits::max_loads`] or [`Limits::max_bytes`] was reached.
    LimitExceeded,
    /// Not a well-formed XML catalog, or an SGML catalog libxml2 rejects.
    Malformed,
    /// A well-formed XML document whose root is not an OASIS `catalog`
    /// (`XML_CATALOG_NOT_CATALOG`).
    NotCatalog,
    /// [`Catalog::add`] refused the entry: an unknown type, an SGML entry
    /// without a name or whose name is taken.
    Rejected,
    /// An allocation the catalog sized failed.
    Allocation,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("catalog not found"),
            Self::Io(kind) => write!(f, "catalog could not be read: {kind}"),
            Self::LimitExceeded => f.write_str("catalog load limit exceeded"),
            Self::Malformed => f.write_str("malformed catalog"),
            Self::NotCatalog => f.write_str("not an XML catalog"),
            Self::Rejected => f.write_str("catalog entry rejected"),
            Self::Allocation => f.write_str("catalog allocation failed"),
        }
    }
}

impl From<TryReserveError> for Error {
    fn from(_: TryReserveError) -> Self {
        Self::Allocation
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Public,
    System,
    RewriteSystem,
    DelegatePublic,
    DelegateSystem,
    Uri,
    RewriteUri,
    DelegateUri,
    NextCatalog,
    /// A `group`; its members carry `id` as their `group`. `prefer` is the
    /// attribute when the group has one (even an invalid value).
    Group {
        id: usize,
        prefer: Option<bool>,
    },
    /// libxml2's `catalog` type, which [`Catalog::add`] accepts and no lookup
    /// consults.
    Catalog,
    /// A delegate or `nextCatalog` whose file failed to load, or a removed
    /// entry; never matches and is not dumped.
    Inert,
}

/// Element, match attribute and URI attribute of each XML catalog entry.
const ENTRIES: [(&str, Kind, Option<&str>, &str); 9] = [
    ("public", Kind::Public, Some("publicId"), "uri"),
    ("system", Kind::System, Some("systemId"), "uri"),
    (
        "rewriteSystem",
        Kind::RewriteSystem,
        Some("systemIdStartString"),
        "rewritePrefix",
    ),
    (
        "delegatePublic",
        Kind::DelegatePublic,
        Some("publicIdStartString"),
        "catalog",
    ),
    (
        "delegateSystem",
        Kind::DelegateSystem,
        Some("systemIdStartString"),
        "catalog",
    ),
    ("uri", Kind::Uri, Some("name"), "uri"),
    (
        "rewriteURI",
        Kind::RewriteUri,
        Some("uriStartString"),
        "rewritePrefix",
    ),
    (
        "delegateURI",
        Kind::DelegateUri,
        Some("uriStartString"),
        "catalog",
    ),
    ("nextCatalog", Kind::NextCatalog, None, "catalog"),
];

#[derive(Debug)]
struct Entry {
    kind: Kind,
    /// The identifier or prefix matched; `None` for `nextCatalog` and for a
    /// public identifier that normalizes to nothing (a prefix of everything).
    name: Option<String>,
    /// The target as written (a group's `xml:base`), which [`Catalog::dump`]
    /// prints.
    value: Option<String>,
    /// The target, rewrite prefix or catalog, resolved against `xml:base`.
    url: String,
    prefer_public: bool,
    /// Enclosing `group` (0: none); distinguishes repeated `nextCatalog`s.
    group: usize,
    /// Index of the loaded catalog for delegates and `nextCatalog`.
    file: Option<usize>,
}

impl Entry {
    fn prefix(&self) -> &str {
        self.name.as_deref().unwrap_or("")
    }
}

/// libxml2's `XML_CATAL_BREAK`: a delegate matched, so resolution stops.
enum Found {
    Uri(String),
    Cut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Sgml {
    Public,
    System,
    Catalog,
    Delegate,
    Entity,
    Doctype,
    Linktype,
    Notation,
    Sgmldecl,
    Document,
    Base,
}

/// The SGML entry keywords [`Catalog::add`] takes and [`Catalog::dump`] prints.
const KEYWORDS: [(&str, Sgml); 11] = [
    ("SYSTEM", Sgml::System),
    ("PUBLIC", Sgml::Public),
    ("DELEGATE", Sgml::Delegate),
    ("ENTITY", Sgml::Entity),
    ("DOCTYPE", Sgml::Doctype),
    ("LINKTYPE", Sgml::Linktype),
    ("NOTATION", Sgml::Notation),
    ("SGMLDECL", Sgml::Sgmldecl),
    ("DOCUMENT", Sgml::Document),
    ("CATALOG", Sgml::Catalog),
    ("BASE", Sgml::Base),
];

/// An SGML entry; the hash key is its name, or the loaded URL of a `CATALOG`.
#[derive(Debug)]
struct Named {
    kind: Sgml,
    name: String,
    value: Option<String>,
}

#[derive(Debug)]
enum Body {
    /// Files by load order (0 is the root) and loaded files by URL, like
    /// libxml2's `xmlCatalogXMLFiles`.
    Xml {
        root: String,
        files: Vec<Vec<Entry>>,
        cache: HashMap<String, usize>,
    },
    /// libxml2's single SGML hash: the first entry for a key wins.
    Sgml(HashMap<String, Named>),
}

/// The resolver with its remaining budgets.
struct Fetcher {
    resolver: Box<dyn Resolver>,
    limits: Limits,
    loads: usize,
    bytes: usize,
    /// An SGML catalog's entries outgrew what the allocator gave.
    refused: bool,
    /// What libxml2's `xmlCatalogErr` raised while parsing loaded XML
    /// catalogs, with the URL of each file.
    diagnostics: Vec<(String, Diagnostic)>,
}

impl Fetcher {
    fn new(resolver: Box<dyn Resolver>, limits: Limits) -> Self {
        Self {
            resolver,
            limits,
            loads: 0,
            bytes: 0,
            refused: false,
            diagnostics: Vec::new(),
        }
    }

    /// Keeps `found`, within [`MAX_DIAGNOSTICS`].
    fn note(&mut self, url: &str, found: Vec<Diagnostic>) {
        let room = MAX_DIAGNOSTICS - self.diagnostics.len();
        let found = found.into_iter().take(room);
        self.diagnostics
            .extend(found.map(|diagnostic| (url.to_owned(), diagnostic)));
    }

    fn get(&mut self, uri: &str) -> Result<Vec<u8>, Error> {
        if self.loads >= self.limits.max_loads {
            return Err(Error::LimitExceeded);
        }
        self.loads += 1;
        let max_bytes = self.limits.max_bytes - self.bytes;
        let request = Request {
            kind: ResourceKind::Catalog,
            uri,
            public_id: None,
            max_bytes,
        };
        let bytes = (self.resolver.resolve(request))
            .map_err(|error| Error::Io(error.kind()))?
            .ok_or(Error::Missing)?;
        if bytes.len() > max_bytes {
            return Err(Error::LimitExceeded);
        }
        self.bytes += bytes.len();
        Ok(bytes)
    }
}

/// An XML or SGML catalog and the resolver that loads the catalogs it names.
///
/// Resolution takes `&mut self`: referenced catalogs load on first use, and
/// one that fails to load is ignored from then on, as in libxml2.
pub struct Catalog {
    body: Body,
    fetch: Fetcher,
    /// Catalogs a document named (see [`Catalog::add_local`]): the list as
    /// file 0, `nextCatalog` entries, and the files loaded from it.
    local: Local,
}

#[derive(Debug)]
struct Local {
    files: Vec<Vec<Entry>>,
    cache: HashMap<String, usize>,
}

impl fmt::Debug for Catalog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (f.debug_struct("Catalog").field("body", &self.body)).finish()
    }
}

impl Catalog {
    /// Builds a catalog from the bytes of the file at `url`, which bases
    /// relative references. Like `xmlLoadACatalog`, content whose first
    /// letter, `-` or `<` is `<` is an XML catalog; anything else is SGML,
    /// whose `CATALOG` entries load now.
    pub fn parse(
        input: &[u8],
        limits: Limits,
        url: &str,
        resolver: Box<dyn Resolver>,
    ) -> Result<Self, Error> {
        Self::build(input, url, Fetcher::new(resolver, limits), false)
    }

    /// Fetches `url` through `resolver`, then [`Catalog::parse`]s it.
    pub fn load(url: &str, limits: Limits, resolver: Box<dyn Resolver>) -> Result<Self, Error> {
        let mut fetch = Fetcher::new(resolver, limits);
        Self::build(&fetch.get(url)?, url, fetch, false)
    }

    /// Fetches the SGML super catalog at `url` (`xmlLoadSGMLSuperCatalog`):
    /// its `CATALOG` entries are kept by the name as written and never
    /// loaded, and the content is SGML whatever it looks like.
    pub fn load_super(
        url: &str,
        limits: Limits,
        resolver: Box<dyn Resolver>,
    ) -> Result<Self, Error> {
        let mut fetch = Fetcher::new(resolver, limits);
        Self::build(&fetch.get(url)?, url, fetch, true)
    }

    fn build(input: &[u8], url: &str, mut fetch: Fetcher, super_: bool) -> Result<Self, Error> {
        let first = (input.iter())
            .find(|&&byte| matches!(byte, 0 | b'-' | b'<') || byte.is_ascii_alphabetic());
        let body = match first {
            Some(b'<') if !super_ => {
                let (entries, found) = parse_xml(input, url, true)?;
                fetch.note(url, found);
                Body::Xml {
                    root: url.to_owned(),
                    files: vec![entries],
                    cache: HashMap::from([(url.to_owned(), 0)]),
                }
            }
            _ => {
                let mut map = HashMap::new();
                let parsed = parse_sgml(&mut map, &mut fetch, input, url, (!super_).then_some(0));
                // A refusal in a nested catalog fails the whole.
                match (fetch.refused, parsed) {
                    (true, _) => return Err(Error::Allocation),
                    (false, None) => return Err(Error::Malformed),
                    (false, Some(())) => {}
                }
                Body::Sgml(map)
            }
        };
        let local = Local {
            files: vec![Vec::new()],
            cache: HashMap::new(),
        };
        Ok(Self { body, fetch, local })
    }

    /// What libxml2 reports while reading XML catalogs (`xmlCatalogErr`),
    /// with the URL of the file, in load order: entries lacking an attribute
    /// or whose URI does not resolve, bad `prefer` values (all skipped or
    /// ignored, as in libxml2), and loaded files that are not catalogs. At
    /// most 100 are kept.
    pub fn diagnostics(&self) -> &[(String, Diagnostic)] {
        &self.fetch.diagnostics
    }

    /// The URI for an external identifier (`xmlACatalogResolve`; with one
    /// identifier, `xmlACatalogResolvePublic` or `xmlACatalogResolveSystem`).
    /// Public identifiers are normalized and `urn:publicid:` URNs unwrapped.
    pub fn resolve(&mut self, public: Option<&str>, system: Option<&str>) -> Option<String> {
        match &mut self.body {
            Body::Xml { root, files, cache } => {
                uri(Query::new(files, cache, &mut self.fetch).list(root, 0, public, system))
            }
            Body::Sgml(map) => (public.map(normalize))
                .filter(|id| !id.is_empty())
                .and_then(|id| sgml(map, &id, Sgml::Public))
                .or_else(|| system.and_then(|id| sgml(map, id, Sgml::System))),
        }
    }

    /// Adds an entry (`xmlACatalogAdd`). An XML catalog takes the element
    /// names of its entries (`"public"`, `"rewriteSystem"`, ...) and
    /// `"catalog"`; `orig` is the identifier or prefix matched (not needed for
    /// `"nextCatalog"`), `replace` the target. An entry of the same type and
    /// name changes in place; any other is appended to the root catalog's
    /// entries, and `replace` is used as given, without resolving it. An SGML
    /// catalog takes its keywords (`"PUBLIC"`, `"SYSTEM"`, ...), which need an
    /// `orig` no entry has.
    pub fn add(&mut self, kind: &str, orig: Option<&str>, replace: &str) -> Result<(), Error> {
        match &mut self.body {
            Body::Xml { files, .. } => {
                let kind = match kind {
                    "catalog" => Kind::Catalog,
                    _ => {
                        ENTRIES
                            .iter()
                            .find(|entry| entry.0 == kind)
                            .ok_or(Error::Rejected)?
                            .1
                    }
                };
                let entries = &mut files[0];
                let same = |entry: &&mut Entry| {
                    entry.kind == kind && orig.is_some() && entry.name.as_deref() == orig
                };
                match entries.iter_mut().find(same) {
                    // A delegate keeps the file it already loaded, as in C.
                    Some(entry) => {
                        (entry.value, entry.url) = (Some(replace.into()), replace.into())
                    }
                    None => entries.push(Entry {
                        kind,
                        name: identifier(kind, orig),
                        value: Some(replace.into()),
                        url: replace.into(),
                        prefer_public: true,
                        group: 0,
                        file: None,
                    }),
                }
                Ok(())
            }
            Body::Sgml(map) => {
                let (_, kind) = KEYWORDS
                    .iter()
                    .find(|entry| entry.0 == kind)
                    .ok_or(Error::Rejected)?;
                let name = orig
                    .filter(|name| !map.contains_key(*name))
                    .ok_or(Error::Rejected)?;
                let value = Some(replace.into());
                map.insert(
                    name.into(),
                    Named {
                        kind: *kind,
                        name: name.into(),
                        value,
                    },
                );
                Ok(())
            }
        }
    }

    /// Removes the entries whose name or target is `value` from an XML
    /// catalog's root file, or the SGML entry with that name
    /// (`xmlACatalogRemove`), and returns how many. libxml2 returns 0 for XML.
    pub fn remove(&mut self, value: &str) -> usize {
        match &mut self.body {
            Body::Xml { files, .. } => (files[0].iter_mut())
                .filter(|entry| {
                    entry.name.as_deref() == Some(value) || entry.value.as_deref() == Some(value)
                })
                .map(|entry| std::mem::replace(&mut entry.kind, Kind::Inert))
                .filter(|kind| *kind != Kind::Inert)
                .count(),
            Body::Sgml(map) => usize::from(map.remove(value).is_some()),
        }
    }

    /// Whether the root file (XML) or the table (SGML) has no entries
    /// (`xmlCatalogIsEmpty`). Removed XML entries still count, as in libxml2;
    /// unlike libxml2, a loaded XML catalog is not empty before its first lookup.
    pub fn is_empty(&self) -> bool {
        match &self.body {
            Body::Xml { files, .. } => files[0].is_empty(),
            Body::Sgml(map) => map.is_empty(),
        }
    }

    /// The catalog as `xmlACatalogDump` writes it: an XML catalog document
    /// of the root file's entries (indented, with the catalog DTD's document
    /// type declaration), or one SGML entry per line in the order of their
    /// keys (libxml2 uses its hash order). Entries that never match (removed,
    /// or whose file failed to load) are left out, groups are kept.
    pub fn dump(&self) -> String {
        match &self.body {
            Body::Xml { files, .. } => {
                let members = dump_group(&files[0], 0, 1);
                let (open, close) = match members.is_empty() {
                    true => ("/>\n", ""),
                    false => (">\n", "</catalog>\n"),
                };
                format!("{DOCTYPE}<catalog xmlns=\"{NAMESPACE}\"{open}{members}{close}")
            }
            Body::Sgml(map) => {
                let mut named: Vec<_> = map.iter().collect();
                named.sort_unstable_by_key(|(key, _)| *key);
                (named.into_iter())
                    .map(|(_, entry)| dump_sgml(entry))
                    .collect()
            }
        }
    }

    /// Adds a catalog for lookups before the main one (`xmlCatalogAddLocal`),
    /// loaded through this catalog's resolver on first use. `url` is not
    /// resolved against anything. Past [`Limits::max_local`] it is ignored.
    pub(crate) fn add_local(&mut self, url: &str) {
        let list = &mut self.local.files[0];
        if list.len() < self.fetch.limits.max_local {
            list.push(Entry {
                kind: Kind::NextCatalog,
                name: None,
                value: Some(url.into()),
                url: url.into(),
                prefer_public: true,
                group: 0,
                file: None,
            });
        }
    }

    /// `xmlResolveFromCatalog`: the document's catalogs, then this one, for
    /// the external identifiers and then for the URI.
    pub(crate) fn resolve_all(&mut self, public: Option<&str>, system: &str) -> Option<String> {
        self.resolve_local(|query, url, child| query.list(url, child, public, Some(system)))
            .or_else(|| self.resolve(public, Some(system)))
            .or_else(|| self.resolve_local(|query, url, child| query.list_uri(url, child, system)))
            .or_else(|| self.resolve_uri(system))
    }

    fn resolve_local(
        &mut self,
        search: impl FnMut(&mut Query<'_>, &str, usize) -> Option<Found>,
    ) -> Option<String> {
        let Local { files, cache } = &mut self.local;
        let mut query = Query::new(files, cache, &mut self.fetch);
        uri(query.follow(0, |entry| entry.kind == Kind::NextCatalog, false, search))
    }

    /// The URI a catalog maps `uri` to (`xmlACatalogResolveURI`). An SGML
    /// catalog consults its `SYSTEM` entries.
    pub fn resolve_uri(&mut self, uri_reference: &str) -> Option<String> {
        match &mut self.body {
            Body::Xml { root, files, cache } => {
                uri(Query::new(files, cache, &mut self.fetch).list_uri(root, 0, uri_reference))
            }
            Body::Sgml(map) => sgml(map, uri_reference, Sgml::System),
        }
    }
}

/// An answer, without libxml2's `XML_CATAL_BREAK`.
fn uri(found: Option<Found>) -> Option<String> {
    match found? {
        Found::Uri(uri) => Some(uri),
        Found::Cut => None,
    }
}

fn sgml(map: &HashMap<String, Named>, key: &str, kind: Sgml) -> Option<String> {
    (map.get(key)).and_then(|found| found.value.clone().filter(|_| found.kind == kind))
}

/// One top-level resolution: libxml2's per-call `xmlCatalogResolveCache`
/// cuts cycles by never visiting a catalog twice with the same identifiers.
struct Query<'a> {
    files: &'a mut Vec<Vec<Entry>>,
    cache: &'a mut HashMap<String, usize>,
    fetch: &'a mut Fetcher,
    seen: HashSet<(String, Option<String>, Option<String>)>,
    depth: usize,
}

/// The entry kinds one lookup step consults: an exact match, the longest
/// rewrite prefix, then delegates (`trigger` starts the step, `follow` lists
/// the delegates it tries).
struct Step {
    exact: Kind,
    rewrite: Option<Kind>,
    trigger: Kind,
    follow: &'static [Kind],
}

const SYSTEM: Step = Step {
    exact: Kind::System,
    rewrite: Some(Kind::RewriteSystem),
    trigger: Kind::DelegateSystem,
    follow: &[Kind::DelegateSystem],
};

const PUBLIC: Step = Step {
    exact: Kind::Public,
    rewrite: None,
    trigger: Kind::DelegatePublic,
    follow: &[Kind::DelegatePublic],
};

/// libxml2's URI step also follows `delegateSystem`, though only a matching
/// `delegateURI` starts it.
const URI: Step = Step {
    exact: Kind::Uri,
    rewrite: Some(Kind::RewriteUri),
    trigger: Kind::DelegateUri,
    follow: &[Kind::DelegateSystem, Kind::DelegateUri],
};

impl<'a> Query<'a> {
    fn new(
        files: &'a mut Vec<Vec<Entry>>,
        cache: &'a mut HashMap<String, usize>,
        fetch: &'a mut Fetcher,
    ) -> Self {
        Self {
            files,
            cache,
            fetch,
            seen: HashSet::new(),
            depth: 0,
        }
    }

    /// Searches a catalog unless it was already visited with these
    /// identifiers or nesting is too deep.
    fn visit(
        &mut self,
        key: (&str, Option<&str>, Option<&str>),
        search: impl FnOnce(&mut Self) -> Option<Found>,
    ) -> Option<Found> {
        let (url, public, system) = key;
        let key = (
            url.to_owned(),
            public.map(str::to_owned),
            system.map(str::to_owned),
        );
        if self.depth > self.fetch.limits.max_depth || !self.seen.insert(key) {
            return None;
        }
        self.depth += 1;
        let found = search(self);
        self.depth -= 1;
        found
    }

    /// `xmlCatalogListXMLResolve` for the catalog `file` loaded from `url`:
    /// normalizes and unwraps the identifiers, then `xmlCatalogXMLResolve`
    /// (system step, public step, `nextCatalog`s).
    fn list(
        &mut self,
        url: &str,
        file: usize,
        public: Option<&str>,
        system: Option<&str>,
    ) -> Option<Found> {
        let (mut public, mut system) = (public.map(str::to_owned), system.map(str::to_owned));
        loop {
            if public.is_none() && system.is_none() {
                return None;
            }
            public = (public.map(|public| normalize(&public))).filter(|public| !public.is_empty());
            if let Some(urn) = public.as_deref().filter(|id| id.starts_with(URN)) {
                public = Some(unwrap_urn(urn));
            } else if let Some(urn) = system.as_deref().filter(|id| id.starts_with(URN)) {
                let urn = unwrap_urn(urn);
                (public, system) = match public {
                    Some(public) if public != urn => (Some(public), Some(urn)),
                    _ => (Some(urn), None),
                };
            } else {
                break;
            }
        }
        let (public, system) = (public.as_deref(), system.as_deref());
        self.visit((url, public, system), |query| {
            if let Some(system) = system {
                let search =
                    |query: &mut Self, url: &str, child| query.list(url, child, None, Some(system));
                if let Some(found) = query.step(file, system, &SYSTEM, search) {
                    return found;
                }
            }
            if let Some(public) = public {
                let search =
                    |query: &mut Self, url: &str, child| query.list(url, child, Some(public), None);
                if let Some(found) = query.step(file, public, &PUBLIC, search) {
                    return found;
                }
            }
            // libxml2 counts `nextCatalog`s only while looking for an identifier.
            let next = |entry: &Entry| entry.kind == Kind::NextCatalog;
            (public.is_some() || system.is_some())
                .then(|| {
                    query.follow(file, next, false, |query, url, child| {
                        query.list(url, child, public, system)
                    })
                })
                .flatten()
        })
    }

    /// `xmlCatalogListXMLResolveURI` and `xmlCatalogXMLResolveURI`.
    fn list_uri(&mut self, url: &str, file: usize, uri: &str) -> Option<Found> {
        if uri.starts_with(URN) {
            return self.list(url, file, Some(&unwrap_urn(uri)), None);
        }
        let search = |query: &mut Self, url: &str, child| query.list_uri(url, child, uri);
        self.visit((url, Some(uri), None), |query| {
            let next = |entry: &Entry| entry.kind == Kind::NextCatalog;
            (query.step(file, uri, &URI, search))
                .unwrap_or_else(|| query.follow(file, next, false, search))
        })
    }

    /// One step for `id`; `None` when it does not apply. A delegate that
    /// matches decides the lookup even without an answer (the
    /// specification's "cut").
    fn step(
        &mut self,
        file: usize,
        id: &str,
        step: &Step,
        search: impl FnMut(&mut Self, &str, usize) -> Option<Found>,
    ) -> Option<Option<Found>> {
        let delegates = |entry: &Entry| {
            id.starts_with(entry.prefix())
                && (entry.kind != Kind::DelegatePublic || entry.prefer_public)
        };
        let (mut rewrite, mut delegate) = (None::<&Entry>, false);
        for entry in &self.files[file] {
            if entry.kind == step.exact && entry.name.as_deref() == Some(id) {
                return Some(Some(Found::Uri(entry.url.clone())));
            }
            let longer = entry.prefix().len() > rewrite.map_or(0, |best| best.prefix().len());
            if Some(entry.kind) == step.rewrite && longer && id.starts_with(entry.prefix()) {
                rewrite = Some(entry);
            }
            delegate |= entry.kind == step.trigger && delegates(entry);
        }
        if let Some(entry) = rewrite {
            return Some(Some(Found::Uri(
                entry.url.clone() + &id[entry.prefix().len()..],
            )));
        }
        let follow = |entry: &Entry| step.follow.contains(&entry.kind) && delegates(entry);
        delegate.then(|| self.follow(file, follow, true, search).or(Some(Found::Cut)))
    }

    /// Searches the catalogs named by matching entries in order, each URL
    /// once if `distinct` (libxml2 remembers `MAX_DELEGATE` of them).
    fn follow(
        &mut self,
        file: usize,
        matches: impl Fn(&Entry) -> bool,
        distinct: bool,
        mut search: impl FnMut(&mut Self, &str, usize) -> Option<Found>,
    ) -> Option<Found> {
        let mut tried: Vec<String> = Vec::new();
        for index in 0..self.files[file].len() {
            let entry = &self.files[file][index];
            if !matches(entry) || (distinct && tried.contains(&entry.url)) {
                continue;
            }
            let url = entry.url.clone();
            if distinct && tried.len() < MAX_DELEGATE {
                tried.push(url.clone());
            }
            if let Some(found) = self
                .load(file, index)
                .and_then(|child| search(self, &url, child))
            {
                return Some(found);
            }
        }
        None
    }

    /// `xmlFetchXMLCatalogFile`: the catalog an entry names, loaded once per
    /// URL; a failure marks the entry broken.
    fn load(&mut self, file: usize, index: usize) -> Option<usize> {
        let entry = &self.files[file][index];
        if entry.file.is_some() {
            return entry.file;
        }
        let (url, prefer_public) = (entry.url.clone(), entry.prefer_public);
        let child = self.cache.get(&url).copied().or_else(|| {
            let parsed = parse_xml(&self.fetch.get(&url).ok()?, &url, prefer_public);
            let (entries, found) = match parsed {
                Ok(parsed) => parsed,
                Err(Error::NotCatalog) => {
                    let message = format!("File {url} is not an XML Catalog");
                    let diagnostic = catalog_error(code::CATALOG_NOT_CATALOG, message, &url, None);
                    self.fetch.note(&url, vec![diagnostic]);
                    return None;
                }
                Err(_) => return None,
            };
            self.fetch.note(&url, found);
            self.files.push(entries);
            self.cache.insert(url, self.files.len() - 1);
            Some(self.files.len() - 1)
        });
        let entry = &mut self.files[file][index];
        match child {
            Some(_) => entry.file = child,
            None => entry.kind = Kind::Inert,
        }
        child
    }
}

/// XML Catalogs section 6.2: blank runs become one space, ends are trimmed.
fn normalize(id: &str) -> String {
    let words: Vec<&str> = id
        .split([' ', '\t', '\n', '\r'])
        .filter(|word| !word.is_empty())
        .collect();
    words.join(" ")
}

/// `%XX` escapes `xmlCatalogUnWrapURN` decodes (uppercase only).
const ESCAPES: [(&[u8], &[u8]); 8] = [
    (b"2B", b"+"),
    (b"3A", b":"),
    (b"2F", b"/"),
    (b"3B", b";"),
    (b"27", b"'"),
    (b"3F", b"?"),
    (b"23", b"#"),
    (b"25", b"%"),
];

/// `xmlCatalogUnWrapURN` (RFC 3151), including its 2000-byte buffer.
fn unwrap_urn(urn: &str) -> String {
    let mut out = Vec::new();
    let mut rest = &urn.as_bytes()[URN.len()..];
    while let Some(&byte) = rest.first() {
        if out.len() > 1996 {
            break;
        }
        let (expansion, used): (&[u8], usize) = match byte {
            b'+' => (b" ", 1),
            b':' => (b"//", 1),
            b';' => (b"::", 1),
            b'%' => (ESCAPES.iter())
                .find(|(code, _)| rest.get(1..3) == Some(*code))
                .map_or((&rest[..1], 1), |(_, decoded)| (*decoded, 3)),
            _ => (&rest[..1], 1),
        };
        out.extend_from_slice(expansion);
        rest = &rest[used..];
    }
    // Truncation can split a character; keep the complete prefix.
    let valid = std::str::from_utf8(&out).map_or_else(|error| error.valid_up_to(), str::len);
    out.truncate(valid);
    String::from_utf8(out).unwrap_or_default()
}

/// `xmlGetProp`: the first attribute with this local name, in any namespace.
fn property(node: OwnedNode<'_>, local: &str) -> Option<String> {
    (node.attributes())
        .find(|attribute| {
            attribute.name.local() == local
                && attribute.name.namespace() != Some(XMLNS_NS)
                && attribute.name.qualified() != "xmlns"
        })
        .map(|attribute| attribute.value.into_owned())
}

/// `xmlNodeGetBase`: `xml:base` values from `node` outwards, then the
/// document URL; `None` when a combination fails to resolve.
fn base(node: OwnedNode<'_>, document_url: &str) -> Option<String> {
    crate::xpath::tree::base(node.document(), node.id(), Some(document_url))
}

/// The preference `node` sets, else `inherited`, which a value other than
/// `public` or `system` keeps with libxml2's report.
fn prefer(node: OwnedNode<'_>, inherited: bool) -> (bool, Option<Diagnostic>) {
    match property(node, "prefer").as_deref() {
        Some("public") => (true, None),
        Some("system") => (false, None),
        Some(value) => {
            let message = format!("Invalid value for prefer: '{value}'");
            let diagnostic = catalog_error(code::CATALOG_PREFER_VALUE, message, value, None);
            (inherited, Some(diagnostic))
        }
        None => (inherited, None),
    }
}

/// An `xmlCatalogErr` report: domain catalog, level error.
fn catalog_error(code: u16, message: String, str1: &str, str2: Option<&str>) -> Diagnostic {
    Diagnostic {
        domain: Domain::Catalog,
        code,
        message,
        str1: Some(str1.to_owned()),
        str2: str2.map(str::to_owned),
        ..Diagnostic::default()
    }
}

/// `xmlParseXMLCatalogFile`: the entries of an XML catalog, `group`
/// members flattened in document order, and the problems libxml2 reports
/// while reading them (at most [`MAX_DIAGNOSTICS`]).
fn parse_xml(
    input: &[u8],
    url: &str,
    prefer_public: bool,
) -> Result<(Vec<Entry>, Vec<Diagnostic>), Error> {
    let document = OwnedDocument::parse_legacy(input, crate::Limits::default()).map_err(
        |error| match error.kind {
            crate::ErrorKind::Allocation => Error::Allocation,
            _ => Error::Malformed,
        },
    )?;
    let local = |node: OwnedNode<'_>| {
        (node.name())
            .filter(|name| name.namespace() == Some(NAMESPACE))
            .map(|name| name.local().to_owned())
    };
    let root = document.root_element();
    if local(root).as_deref() != Some("catalog") {
        return Err(Error::NotCatalog);
    }
    let document_url = canonic_path(url);
    let lines = OnceCell::new();
    let mut found = Vec::new();
    let mut report = |node: OwnedNode<'_>, diagnostic: Diagnostic| {
        if found.len() < MAX_DIAGNOSTICS {
            let lines = lines.get_or_init(|| LineIndex::new(input));
            found.push(Diagnostic {
                line: node.line(lines).unwrap_or(0),
                element: node.name().map(|name| name.local().to_owned()),
                ..diagnostic
            });
        }
    };
    let (now, problem) = prefer(root, prefer_public);
    problem
        .into_iter()
        .for_each(|diagnostic| report(root, diagnostic));
    // Elements whose children are entries (the root and nested groups):
    // group number and preference.
    let mut scopes = HashMap::from([(root.id(), (0, now))]);
    let mut entries: Vec<Entry> = Vec::new();
    for node in root.descendants().skip(1) {
        let scope = node.parent().and_then(|parent| scopes.get(&parent.id()));
        let (Some(&(group, prefer_public)), Some(element)) = (scope, local(node)) else {
            continue;
        };
        if element == "group" {
            let ((now, problem), id) = (prefer(node, prefer_public), scopes.len());
            problem
                .into_iter()
                .for_each(|diagnostic| report(node, diagnostic));
            scopes.try_reserve(1)?;
            scopes.insert(node.id(), (id, now));
            let base = (node.attributes())
                .find(|attribute| {
                    attribute.name.local() == "base"
                        && attribute.name.namespace() == Some(XML_NAMESPACE)
                })
                .map(|attribute| attribute.value.into_owned());
            entries.try_push(Entry {
                kind: Kind::Group {
                    id,
                    prefer: property(node, "prefer").map(|_| now),
                },
                name: property(node, "id"),
                value: base,
                url: String::new(),
                prefer_public,
                group,
                file: None,
            })?;
            continue;
        }
        let Some(&(_, kind, name, target)) = ENTRIES.iter().find(|entry| entry.0 == element) else {
            continue;
        };
        let name = name.map(|attribute| (attribute, property(node, attribute)));
        let uri = (target, property(node, target));
        for &(attribute, _) in name
            .iter()
            .chain([&uri])
            .filter(|(_, value)| value.is_none())
        {
            let message = format!("{element} entry lacks '{attribute}'");
            let diagnostic = catalog_error(
                code::CATALOG_MISSING_ATTR,
                message,
                &element,
                Some(attribute),
            );
            report(node, diagnostic);
        }
        let (Some(name), Some(target)) =
            (name.map_or(Some(None), |(_, value)| value.map(Some)), uri.1)
        else {
            continue;
        };
        let url = match base(node, &document_url) {
            Some(base) => build(&target, &base),
            None => Ok(target.clone()),
        };
        let Ok(url) = url else {
            let message = format!("{element} entry '{}' broken ?: {target}", uri.0);
            report(
                node,
                catalog_error(code::CATALOG_ENTRY_BROKEN, message, &element, Some(uri.0)),
            );
            continue;
        };
        let name = identifier(kind, name.as_deref());
        let repeated = |other: &Entry| {
            other.kind == Kind::NextCatalog
                && other.url == url
                && other.prefer_public == prefer_public
                && other.group == group
        };
        if kind != Kind::NextCatalog || !entries.iter().any(repeated) {
            entries.try_push(Entry {
                kind,
                name,
                value: Some(target),
                url,
                prefer_public,
                group,
                file: None,
            })?;
        }
    }
    Ok((entries, found))
}

/// The name an entry of `kind` stores for `name`: public identifiers are
/// normalized, and one that normalizes to nothing has none.
fn identifier(kind: Kind, name: Option<&str>) -> Option<String> {
    match kind {
        Kind::Public | Kind::DelegatePublic => name.map(normalize).filter(|name| !name.is_empty()),
        _ => name.map(str::to_owned),
    }
}

/// `xmlDumpXMLCatalogNode`: the entries of `group` as indented elements,
/// each group followed by its members.
fn dump_group(entries: &[Entry], group: usize, depth: usize) -> String {
    let mut out = String::new();
    let indent = "  ".repeat(depth);
    for entry in entries.iter().filter(|entry| entry.group == group) {
        let name = entry.name.as_deref().unwrap_or_default();
        let (element, attributes, members): (&str, Vec<(&str, Option<&str>)>, String) =
            match entry.kind {
                Kind::Group { id, prefer } => {
                    let prefer = prefer.map(|public| ["system", "public"][usize::from(public)]);
                    let attributes = [("id", Some(name)), ("xml:base", entry.value.as_deref())]
                        .into_iter()
                        .chain([("prefer", prefer)]);
                    (
                        "group",
                        attributes.collect(),
                        dump_group(entries, id, depth + 1),
                    )
                }
                kind => {
                    let Some(&(element, _, attribute, target)) =
                        ENTRIES.iter().find(|candidate| candidate.1 == kind)
                    else {
                        continue;
                    };
                    let attributes = [attribute.map(|attribute| (attribute, Some(name)))]
                        .into_iter()
                        .flatten()
                        .chain([(target, entry.value.as_deref())]);
                    (element, attributes.collect(), String::new())
                }
            };
        out.push_str(&format!("{indent}<{element}"));
        for (attribute, value) in attributes.into_iter().filter_map(|(a, v)| Some((a, v?))) {
            out.push_str(&format!(" {attribute}=\""));
            escape_into(value, &ATTRIBUTE, true, |piece| out.push_str(piece));
            out.push('"');
        }
        match members.is_empty() {
            true => out.push_str("/>\n"),
            false => out.push_str(&format!(">\n{members}{indent}</{element}>\n")),
        }
    }
    out
}

/// `xmlCatalogDumpEntry`: one SGML entry as a line.
fn dump_sgml(entry: &Named) -> String {
    let keyword = KEYWORDS.iter().find(|candidate| candidate.1 == entry.kind);
    let keyword = keyword.map_or("", |keyword| keyword.0);
    let bare = matches!(
        entry.kind,
        Sgml::Entity | Sgml::Doctype | Sgml::Linktype | Sgml::Notation
    );
    let name = match bare {
        true => entry.name.clone(),
        false => format!("\"{}\"", entry.name),
    };
    let value = match entry.kind {
        Sgml::Catalog | Sgml::Sgmldecl | Sgml::Document | Sgml::Base => String::new(),
        _ => format!(" \"{}\"", entry.value.as_deref().unwrap_or("(null)")),
    };
    format!("{keyword} {name}{value}\n")
}

/// `xmlParseCatalogPI`: the `catalog` pseudo-attribute of an
/// `oasis-xml-catalog` processing instruction's data (`catalog="URL"`, either
/// quote); `Ok(None)` without `=` (ignored), `Err` for libxml2's syntax
/// warning.
pub(crate) fn pi_catalog(data: &str) -> Result<Option<&str>, ()> {
    fn after<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
        let rest = text.trim_start_matches(blank_char).strip_prefix(prefix)?;
        Some(rest.trim_start_matches(blank_char))
    }
    let rest = after(data, "catalog").ok_or(())?;
    // libxml2 ignores `catalog` without `=` silently.
    let Some(rest) = after(rest, "=") else {
        return Ok(None);
    };
    let quote = (rest.chars().next())
        .filter(|quote| matches!(quote, '"' | '\''))
        .ok_or(())?;
    let (url, tail) = rest[1..].split_once(quote).ok_or(())?;
    match tail.trim_start_matches(blank_char).is_empty() {
        true => Ok(Some(url)),
        false => Err(()),
    }
}

fn blank_char(c: char) -> bool {
    c.is_ascii() && blank(&(c as u8))
}

fn blank(byte: &u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

fn skip_blanks(text: &[u8]) -> &[u8] {
    &text[text
        .iter()
        .position(|byte| !blank(byte))
        .unwrap_or(text.len())..]
}

/// `xmlParseSGMLCatalogName`: a bytewise Latin-1 name shorter than 100 bytes.
fn sgml_name(text: &[u8]) -> Option<&[u8]> {
    let letter =
        |byte: u8| byte.is_ascii_alphabetic() || (byte >= 0xC0 && byte != 0xD7 && byte != 0xF7);
    if !text
        .first()
        .is_some_and(|&byte| letter(byte) || byte == b'_' || byte == b':')
    {
        return None;
    }
    let name = |&byte: &u8| letter(byte) || byte.is_ascii_digit() || b".-_:".contains(&byte);
    let length = text
        .iter()
        .position(|byte| !name(byte))
        .unwrap_or(text.len());
    (length < 100).then(|| &text[..length])
}

/// `xmlParseSGMLCatalogPubid`: a quoted or blank-terminated literal of
/// public identifier characters, and the text after it.
fn sgml_literal(text: &[u8]) -> Option<(String, &[u8])> {
    let pubid =
        |byte: u8| byte.is_ascii_alphanumeric() || b" \r\n-'()+,./:=?;!*#@$_%".contains(&byte);
    let (quote, body) = match text.first() {
        Some(&quote @ (b'"' | b'\'')) => (Some(quote), &text[1..]),
        _ => (None, text),
    };
    let end = |&byte: &u8| !pubid(byte) || Some(byte) == quote || (quote.is_none() && blank(&byte));
    let (value, rest) = body.split_at(body.iter().position(end).unwrap_or(body.len()));
    let rest = match quote {
        Some(quote) => rest.strip_prefix(&[quote])?,
        None => rest.first().is_some_and(blank).then_some(rest)?,
    };
    Some((value.iter().copied().map(char::from).collect(), rest))
}

/// `xmlParseSGMLCatalog`, loading `CATALOG` entries at `depth` as
/// `xmlExpandCatalog` does, or keeping them unloaded by their written name
/// when `depth` is `None` (a super catalog). `None` is libxml2's -1; entries
/// before the error are kept.
fn parse_sgml(
    map: &mut HashMap<String, Named>,
    fetch: &mut Fetcher,
    input: &[u8],
    file: &str,
    depth: Option<usize>,
) -> Option<()> {
    if depth.is_some_and(|depth| depth > fetch.limits.max_depth) {
        return None;
    }
    let mut text = &input[..memchr::memchr(0, input).unwrap_or(input.len())];
    let mut base = file.to_owned();
    let followed = |text: &[u8]| text.first().is_some_and(blank);
    loop {
        text = skip_blanks(text);
        if text.is_empty() {
            return Some(());
        }
        if let Some(comment) = text.strip_prefix(b"--") {
            text = &comment[memchr::memmem::find(comment, b"--")? + 2..];
            continue;
        }
        let keyword = sgml_name(text)?;
        if !followed(&text[keyword.len()..]) {
            return Some(());
        }
        text = skip_blanks(&text[keyword.len()..]);
        match keyword {
            b"OVERRIDE" => text = &text[sgml_name(text)?.len()..],
            // `ENTITY %name` fails here, as in libxml2.
            b"ENTITY" | b"DOCTYPE" | b"LINKTYPE" | b"NOTATION" => {
                text = &text[sgml_name(text)?.len()..];
                if followed(text) {
                    text = sgml_literal(skip_blanks(text))?.1;
                }
            }
            b"PUBLIC" | b"SYSTEM" | b"DELEGATE" => {
                let (name, rest) = sgml_literal(text)?;
                text = rest;
                let mut system = None;
                if followed(text) {
                    let (literal, rest) = sgml_literal(skip_blanks(text))?;
                    (system, text) = (Some(literal), rest);
                }
                let (kind, name) = match keyword {
                    b"SYSTEM" => (Sgml::System, name),
                    _ => (Sgml::Public, normalize(&name)),
                };
                let url = system.and_then(|system| build(&system, &base).ok());
                // libxml2 parses DELEGATE but keeps no entry for it.
                let keep = keyword != b"DELEGATE" && !(kind == Sgml::Public && name.is_empty());
                if let Some(url) = url.filter(|_| keep) {
                    if map.try_reserve(1).is_err() {
                        fetch.refused = true;
                        return None;
                    }
                    let value = Some(url);
                    map.entry(name.clone())
                        .or_insert(Named { kind, name, value });
                }
            }
            b"BASE" | b"CATALOG" | b"DOCUMENT" | b"SGMLDECL" => {
                let (literal, rest) = sgml_literal(text)?;
                text = rest;
                if keyword == b"BASE" {
                    base = literal;
                } else if keyword == b"CATALOG" {
                    let key = match depth {
                        Some(_) => build(&literal, &base).ok(),
                        None => Some(literal.clone()),
                    };
                    let Some(url) = key.filter(|url| !map.contains_key(url)) else {
                        continue;
                    };
                    let named = Named {
                        kind: Sgml::Catalog,
                        name: literal,
                        value: None,
                    };
                    if map.try_reserve(1).is_err() {
                        fetch.refused = true;
                        return None;
                    }
                    map.insert(url.clone(), named);
                    // Nested failures are ignored, as by xmlExpandCatalog.
                    if let Some(depth) = depth
                        && let Ok(nested) = fetch.get(&url)
                    {
                        parse_sgml(map, fetch, &nested, &url, Some(depth + 1));
                    }
                }
            }
            _ => {}
        }
    }
}

/// A [`Resolver`] that maps each request through a [`Catalog`] as libxml2's
/// default entity loader does (`xmlCatalogResolve` with the public and system
/// identifiers, then `xmlCatalogResolveURI`), then lets `inner` fetch the
/// mapped URI or, without a mapping, the original one.
pub struct CatalogResolver {
    catalog: Catalog,
    inner: Box<dyn Resolver>,
}

impl CatalogResolver {
    /// Maps requests through `catalog`, then passes them to `inner`.
    pub fn new(catalog: Catalog, inner: Box<dyn Resolver>) -> Self {
        Self { catalog, inner }
    }
}

impl Resolver for CatalogResolver {
    fn resolve(&mut self, request: Request<'_>) -> io::Result<Option<Vec<u8>>> {
        let mapped = self.catalog.resolve_all(request.public_id, request.uri);
        self.inner.resolve(Request {
            uri: mapped.as_deref().unwrap_or(request.uri),
            ..request
        })
    }

    /// With `CATALOG_PI`, a document's `<?oasis-xml-catalog catalog="URL"?>`
    /// adds that catalog before the main one, for the rest of this parse (the
    /// resolver is consumed by it). The catalog is fetched through the
    /// catalog's own resolver, so nothing it forbids is loaded.
    fn catalog_pi(&mut self, catalog: &str) {
        self.catalog.add_local(catalog);
    }
}

#[cfg(test)]
#[path = "../tests/internal/catalog.rs"]
mod tests;
