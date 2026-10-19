//! OASIS XML Catalogs and SGML catalogs as libxml2 2.15.4 resolves them
//! (`catalog.c`), without global state: a [`Catalog`] is built from bytes or
//! loaded through a caller's [`Resolver`], which also fetches `nextCatalog`,
//! delegate and SGML `CATALOG` files, lazily and within [`Limits`].
//! [`CatalogResolver`] maps identifiers before another resolver fetches them.
//!
//! ```
//! use yaxml::catalog::{Catalog, CatalogResolver};
//!
//! let xml = br#"<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog">
//!   <public publicId="-//EX//DTD R//EN" uri="local/r.dtd"/>
//! </catalog>"#;
//! let none = |_: yaxml::Request<'_>| Ok(None);
//! let mut catalog = Catalog::parse(xml, Default::default(), "cat.xml", Box::new(none))?;
//! assert_eq!(catalog.resolve(Some("-//EX//DTD  R//EN"), None).as_deref(), Some("local/r.dtd"));
//!
//! let dtd = |request: yaxml::Request<'_>| {
//!     Ok((request.uri == "local/r.dtd").then(|| b"<!ENTITY e 'mapped'>".to_vec()))
//! };
//! let resolver = CatalogResolver::new(catalog, Box::new(dtd));
//! let input = b"<!DOCTYPE r PUBLIC '-//EX//DTD R//EN' 'http://example.com/r.dtd'><r>&e;</r>";
//! let document = yaxml::Document::with_resolver(input, Default::default(), "doc.xml", Box::new(resolver))?;
//! assert_eq!(document.root_element().text_chunks().collect::<String>(), "mapped");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::collections::{HashMap, HashSet};
use std::{fmt, io};

use crate::uri::{canonic_path, resolve as build};
use crate::{OwnedDocument, OwnedNode, Request, Resolver, ResourceKind};

const NAMESPACE: &str = "urn:oasis:names:tc:entity:xmlns:xml:catalog";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";
const URN: &str = "urn:publicid:";
/// libxml2's `MAX_DELEGATE`: distinct delegate catalogs remembered per step.
const MAX_DELEGATE: usize = 50;

/// Budgets for catalog files fetched through the resolver. libxml2 has none.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Catalog files requested, including the one [`Catalog::load`] fetches.
    pub max_loads: usize,
    /// Bytes across all of those files.
    pub max_bytes: usize,
    /// Nesting of catalogs while loading SGML catalogs or resolving
    /// (libxml2's `MAX_CATAL_DEPTH`, 50); deeper catalogs are skipped.
    pub max_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_loads: 64,
            max_bytes: 1 << 24,
            max_depth: 50,
        }
    }
}

/// Why a catalog could not be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// The resolver declined the catalog file.
    Missing,
    /// The resolver failed.
    Io(io::ErrorKind),
    /// [`Limits::max_loads`] or [`Limits::max_bytes`] was reached.
    LimitExceeded,
    /// Not a well-formed XML catalog, or an SGML catalog libxml2 rejects.
    Malformed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("catalog not found"),
            Self::Io(kind) => write!(f, "catalog could not be read: {kind}"),
            Self::LimitExceeded => f.write_str("catalog load limit exceeded"),
            Self::Malformed => f.write_str("malformed catalog"),
        }
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
    /// A delegate or `nextCatalog` whose file failed to load; never matches.
    Broken,
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
    Sgml(HashMap<String, (Sgml, String)>),
}

/// The resolver with its remaining budgets.
struct Fetcher {
    resolver: Box<dyn Resolver>,
    limits: Limits,
    loads: usize,
    bytes: usize,
}

impl Fetcher {
    fn new(resolver: Box<dyn Resolver>, limits: Limits) -> Self {
        Self {
            resolver,
            limits,
            loads: 0,
            bytes: 0,
        }
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
}

impl fmt::Debug for Catalog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Catalog").field("body", &self.body).finish()
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
        Self::build(input, url, Fetcher::new(resolver, limits))
    }

    /// Fetches `url` through `resolver`, then [`Catalog::parse`]s it.
    pub fn load(url: &str, limits: Limits, resolver: Box<dyn Resolver>) -> Result<Self, Error> {
        let mut fetch = Fetcher::new(resolver, limits);
        Self::build(&fetch.get(url)?, url, fetch)
    }

    fn build(input: &[u8], url: &str, mut fetch: Fetcher) -> Result<Self, Error> {
        let first = (input.iter())
            .find(|&&byte| matches!(byte, 0 | b'-' | b'<') || byte.is_ascii_alphabetic());
        let body = match first {
            Some(b'<') => Body::Xml {
                root: url.to_owned(),
                files: vec![parse_xml(input, url, true).ok_or(Error::Malformed)?],
                cache: HashMap::from([(url.to_owned(), 0)]),
            },
            _ => {
                let mut map = HashMap::new();
                parse_sgml(&mut map, &mut fetch, input, url, 0).ok_or(Error::Malformed)?;
                Body::Sgml(map)
            }
        };
        Ok(Self { body, fetch })
    }

    /// The URI for an external identifier (`xmlACatalogResolve`; with one
    /// identifier, `xmlACatalogResolvePublic` or `xmlACatalogResolveSystem`).
    /// Public identifiers are normalized and `urn:publicid:` URNs unwrapped.
    pub fn resolve(&mut self, public: Option<&str>, system: Option<&str>) -> Option<String> {
        match &mut self.body {
            Body::Xml { root, files, cache } => {
                uri(Query::new(files, cache, &mut self.fetch).list(root, 0, public, system))
            }
            Body::Sgml(map) => (public.and_then(|id| sgml(map, &normalize(id), Sgml::Public)))
                .or_else(|| system.and_then(|id| sgml(map, id, Sgml::System))),
        }
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

fn sgml(map: &HashMap<String, (Sgml, String)>, key: &str, kind: Sgml) -> Option<String> {
    (map.get(key)).and_then(|(found, uri)| (*found == kind).then(|| uri.clone()))
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
            let next = |entry: &Entry| entry.kind == Kind::NextCatalog;
            query.follow(file, next, false, |query, url, child| {
                query.list(url, child, public, system)
            })
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
            let entries = parse_xml(&self.fetch.get(&url).ok()?, &url, prefer_public)?;
            self.files.push(entries);
            self.cache.insert(url, self.files.len() - 1);
            Some(self.files.len() - 1)
        });
        let entry = &mut self.files[file][index];
        match child {
            Some(_) => entry.file = child,
            None => entry.kind = Kind::Broken,
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
    let mut base: Option<String> = None;
    for node in std::iter::successors(Some(node), |node| node.parent()) {
        let outer = (node.attributes()).find(|attribute| {
            attribute.name.local() == "base" && attribute.name.namespace() == Some(XML_NS)
        });
        if let Some(outer) = outer.map(|attribute| attribute.value.into_owned()) {
            let combined = match base {
                Some(inner) => build(&inner, &outer).ok()?,
                None => outer,
            };
            if ["http://", "ftp://", "urn:"]
                .iter()
                .any(|scheme| combined.starts_with(scheme))
            {
                return Some(combined);
            }
            base = Some(combined);
        }
    }
    match base {
        Some(inner) => build(&inner, document_url).ok(),
        None => Some(document_url.to_owned()),
    }
}

fn prefer(node: OwnedNode<'_>, inherited: bool) -> bool {
    match property(node, "prefer").as_deref() {
        Some("public") => true,
        Some("system") => false,
        _ => inherited,
    }
}

/// `xmlParseXMLCatalogFile`: the entries of an XML catalog, `group`
/// members flattened in document order. `None` if malformed.
fn parse_xml(input: &[u8], url: &str, prefer_public: bool) -> Option<Vec<Entry>> {
    let document = OwnedDocument::with_limits(input, crate::Limits::default()).ok()?;
    let local = |node: OwnedNode<'_>| {
        (node.name())
            .filter(|name| name.namespace() == Some(NAMESPACE))
            .map(|name| name.local().to_owned())
    };
    let root = document.root_element();
    if local(root).as_deref() != Some("catalog") {
        return None;
    }
    let document_url = canonic_path(url);
    // Elements whose children are entries (the root and nested groups):
    // group number and preference.
    let mut scopes = HashMap::from([(root.id(), (0, prefer(root, prefer_public)))]);
    let mut entries: Vec<Entry> = Vec::new();
    for node in root.descendants().skip(1) {
        let scope = node.parent().and_then(|parent| scopes.get(&parent.id()));
        let (Some(&(group, prefer_public)), Some(element)) = (scope, local(node)) else {
            continue;
        };
        if element == "group" {
            scopes.insert(node.id(), (scopes.len(), prefer(node, prefer_public)));
            continue;
        }
        let Some(&(_, kind, name, target)) = ENTRIES.iter().find(|entry| entry.0 == element) else {
            continue;
        };
        let name = match name {
            Some(name) => match property(node, name) {
                Some(value) => Some(value),
                None => continue,
            },
            None => None,
        };
        let Some(target) = property(node, target) else {
            continue;
        };
        let url = match base(node, &document_url) {
            Some(base) => build(&target, &base),
            None => Ok(target),
        };
        let Ok(url) = url else {
            continue;
        };
        let name = match kind {
            Kind::Public | Kind::DelegatePublic => name
                .map(|name| normalize(&name))
                .filter(|name| !name.is_empty()),
            _ => name,
        };
        let repeated = |other: &Entry| {
            other.kind == Kind::NextCatalog
                && other.url == url
                && other.prefer_public == prefer_public
                && other.group == group
        };
        if kind != Kind::NextCatalog || !entries.iter().any(repeated) {
            entries.push(Entry {
                kind,
                name,
                url,
                prefer_public,
                group,
                file: None,
            });
        }
    }
    Some(entries)
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

/// `xmlParseSGMLCatalog`, loading `CATALOG` entries as `xmlExpandCatalog`
/// does. `None` is libxml2's -1; entries before the error are kept.
fn parse_sgml(
    map: &mut HashMap<String, (Sgml, String)>,
    fetch: &mut Fetcher,
    input: &[u8],
    file: &str,
    depth: usize,
) -> Option<()> {
    if depth > fetch.limits.max_depth {
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
                    map.entry(name).or_insert((kind, url));
                }
            }
            b"BASE" | b"CATALOG" | b"DOCUMENT" | b"SGMLDECL" => {
                let (literal, rest) = sgml_literal(text)?;
                text = rest;
                if keyword == b"BASE" {
                    base = literal;
                } else if keyword == b"CATALOG" {
                    let Some(url) = build(&literal, &base)
                        .ok()
                        .filter(|url| !map.contains_key(url))
                    else {
                        continue;
                    };
                    map.insert(url.clone(), (Sgml::Catalog, url.clone()));
                    // Nested failures are ignored, as by xmlExpandCatalog.
                    if let Ok(nested) = fetch.get(&url) {
                        parse_sgml(map, fetch, &nested, &url, depth + 1);
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
    pub fn new(catalog: Catalog, inner: Box<dyn Resolver>) -> Self {
        Self { catalog, inner }
    }
}

impl Resolver for CatalogResolver {
    fn resolve(&mut self, request: Request<'_>) -> io::Result<Option<Vec<u8>>> {
        let mapped = (self.catalog.resolve(request.public_id, Some(request.uri)))
            .or_else(|| self.catalog.resolve_uri(request.uri));
        self.inner.resolve(Request {
            uri: mapped.as_deref().unwrap_or(request.uri),
            ..request
        })
    }
}
