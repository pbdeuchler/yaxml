//! Reading schema documents into [`El`] trees: RELAX NG section 4.1 to 4.11
//! (annotations, whitespace, `datatypeLibrary`, `href`, `externalRef`,
//! `include` and `ns` inheritance), loading through the caller's resolver.

use std::collections::HashSet;
use std::rc::Rc;

use super::pattern::is_whitespace;
use super::{Limits, RNG_NS, SchemaCode, SchemaError};
use crate::fallible::TryPush;
use crate::xpath::tree::{Item, Navigate, QName, XML_NAMESPACE, attributes, namespaces};
use crate::{OwnedDocument, Request, Resolver, ResourceKind, Uri};

/// The RELAX NG elements and the attributes each allows besides `ns` and
/// `datatypeLibrary`.
const VOCABULARY: [(&str, &[&str]); 28] = [
    ("element", &["name"]),
    ("attribute", &["name"]),
    ("group", &[]),
    ("interleave", &[]),
    ("choice", &[]),
    ("optional", &[]),
    ("zeroOrMore", &[]),
    ("oneOrMore", &[]),
    ("list", &[]),
    ("mixed", &[]),
    ("ref", &["name"]),
    ("parentRef", &["name"]),
    ("empty", &[]),
    ("text", &[]),
    ("value", &["type"]),
    ("data", &["type"]),
    ("notAllowed", &[]),
    ("externalRef", &["href"]),
    ("grammar", &[]),
    ("param", &["name"]),
    ("except", &[]),
    ("div", &[]),
    ("include", &["href"]),
    ("start", &["combine"]),
    ("define", &["name", "combine"]),
    ("name", &[]),
    ("anyName", &[]),
    ("nsName", &[]),
];

/// Where a schema element came from: an index into the loaded resources.
#[derive(Clone, Copy, Debug)]
pub(super) struct Loc {
    pub(super) resource: usize,
    pub(super) offset: Option<usize>,
}

/// A RELAX NG element after the section 4.1 to 4.11 rules.
#[derive(Debug)]
pub(super) struct El {
    pub(super) name: String,
    /// Unqualified attributes; `name`, `type` and `combine` are trimmed.
    pub(super) attrs: Vec<(String, String)>,
    pub(super) children: Vec<El>,
    pub(super) text: String,
    /// The `ns` in scope (section 4.9).
    pub(super) ns: Rc<str>,
    /// Whether the element has its own `ns` attribute.
    pub(super) own_ns: bool,
    /// The `datatypeLibrary` in scope (section 4.3).
    pub(super) library: Rc<str>,
    /// In-scope namespace declarations as (prefix, URI).
    pub(super) scope: Rc<[(String, String)]>,
    pub(super) at: Loc,
}

impl El {
    pub(super) fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

impl crate::schema_types::Context for El {
    fn namespace(&self, prefix: &str) -> Option<&str> {
        match prefix {
            "" => Some(&*self.ns).filter(|ns| !ns.is_empty()),
            _ => self
                .scope
                .iter()
                .find(|(bound, _)| bound == prefix)
                .map(|(_, uri)| uri.as_str()),
        }
    }
}

/// What an element inherits from its ancestors.
struct Inherited {
    ns: Rc<str>,
    library: Rc<str>,
    base: Rc<str>,
    scope: Rc<[(String, String)]>,
    resource: usize,
}

/// The resolver, its budgets and the resources loaded so far.
pub(super) struct Loader<'l> {
    resolver: Box<dyn Resolver>,
    limits: &'l Limits,
    loads: usize,
    bytes: usize,
    /// URIs by resource index; the first is the schema itself.
    pub(super) resources: Vec<String>,
    /// Resources being loaded, innermost last, to detect recursion.
    stack: Vec<String>,
}

impl<'l> Loader<'l> {
    pub(super) fn new(base: &str, resolver: Box<dyn Resolver>, limits: &'l Limits) -> Self {
        Self {
            resolver,
            limits,
            loads: 0,
            bytes: 0,
            resources: vec![base.to_owned()],
            stack: vec![base.to_owned()],
        }
    }

    fn refused(&self, at: Loc) -> SchemaError {
        self.error(
            SchemaCode::Allocation,
            at,
            "Memory allocation failed".into(),
        )
    }

    pub(super) fn error(&self, code: SchemaCode, at: Loc, message: String) -> SchemaError {
        SchemaError {
            code,
            uri: self.resources[at.resource].clone(),
            offset: at.offset,
            message,
        }
    }

    /// The schema's document element, inheriting nothing.
    pub(super) fn document<T: Navigate>(&mut self, tree: &T) -> Result<El, SchemaError> {
        let resource = self.resources.len() - 1;
        let inherited = Inherited {
            ns: Rc::from(""),
            library: Rc::from(""),
            base: Rc::from(self.resources[resource].as_str()),
            scope: Rc::new([]),
            resource,
        };
        let root =
            std::iter::successors(tree.first_child(tree.root()), |&id| tree.next_sibling(id))
                .find(|&id| matches!(tree.item(id), Item::Element(_)));
        let at = Loc {
            resource,
            offset: None,
        };
        match root.and_then(|id| Some((id, rng(tree.item(id))?))) {
            Some((id, qname)) => self.convert(tree, id, qname, &inherited),
            None => Err(self.error(
                SchemaCode::Empty,
                at,
                format!("xmlRelaxNGParse: {} is empty", self.resources[resource]),
            )),
        }
    }

    fn convert<T: Navigate>(
        &mut self,
        tree: &T,
        id: T::Id,
        qname: QName<'_>,
        up: &Inherited,
    ) -> Result<El, SchemaError> {
        let at = Loc {
            resource: up.resource,
            offset: tree.offset(id),
        };
        let name = qname.local;
        let Some(&(_, allowed)) = VOCABULARY.iter().find(|(known, _)| *known == name) else {
            return Err(self.error(
                SchemaCode::UnknownConstruct,
                at,
                format!("Unexpected node {name} is not a pattern"),
            ));
        };
        let mut attrs = Vec::new();
        let mut declares = false;
        let mut base = up.base.clone();
        for (_, attribute, value) in attributes(tree, id) {
            match attribute.namespace {
                _ if attribute.is_namespace_declaration() => declares = true,
                Some(XML_NAMESPACE) if attribute.local == "base" => {
                    base = Rc::from(crate::uri::resolve(value, &base).unwrap_or(value.to_owned()))
                }
                Some(RNG_NS) => {
                    return Err(self.error(
                        SchemaCode::ForbiddenAttribute,
                        at,
                        format!("Attribute {} is not allowed on {name}", attribute.qualified),
                    ));
                }
                Some(_) => {}
                None if matches!(attribute.local, "ns" | "datatypeLibrary")
                    || allowed.contains(&attribute.local) =>
                {
                    let value = match attribute.local {
                        "name" | "type" | "combine" => value.trim_matches(is_space),
                        _ => value,
                    };
                    attrs.push((attribute.local.to_owned(), value.to_owned()));
                }
                None => {
                    let local = attribute.local;
                    return Err(
                        match matches!(local, "name" | "type" | "href" | "combine") {
                            true => self.error(
                                SchemaCode::ForbiddenAttribute,
                                at,
                                format!("Attribute {local} is not allowed on {name}"),
                            ),
                            false => self.error(
                                SchemaCode::UnknownAttribute,
                                at,
                                format!("Unknown attribute {local} on {name}"),
                            ),
                        },
                    );
                }
            }
        }
        let own = |key| {
            attrs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        let library = match own("datatypeLibrary") {
            None => up.library.clone(),
            Some(uri) => {
                self.check_library(uri, at)?;
                Rc::from(uri)
            }
        };
        let own_ns = own("ns");
        let here = Inherited {
            ns: own_ns.map_or_else(|| up.ns.clone(), Rc::from),
            library,
            base,
            scope: match declares {
                true => namespaces(tree, id)
                    .into_iter()
                    .map(|(prefix, uri)| (prefix.to_owned(), uri.to_owned()))
                    .collect(),
                false => up.scope.clone(),
            },
            resource: up.resource,
        };
        let mut children = Vec::new();
        let mut text = String::new();
        let first = tree.first_child(id);
        for child in std::iter::successors(first, |&child| tree.next_sibling(child)) {
            match tree.item(child) {
                Item::Element(_) if matches!(name, "value" | "param" | "name") => {
                    return Err(self.error(
                        SchemaCode::TextExpected,
                        at,
                        format!("Element {name} has element content"),
                    ));
                }
                Item::Element(qname) if qname.namespace == Some(RNG_NS) => {
                    let el = self.convert(tree, child, qname, &here)?;
                    children.try_push(el).map_err(|_| self.refused(at))?;
                }
                Item::Text(value) => text.try_push(value).map_err(|_| self.refused(at))?,
                _ => {}
            }
        }
        if !matches!(name, "value" | "param" | "name") && !is_whitespace(&text) {
            return Err(self.error(
                SchemaCode::TextExpected,
                at,
                format!("Element {name} has unexpected text content"),
            ));
        }
        let el = El {
            name: name.to_owned(),
            own_ns: own_ns.is_some(),
            attrs,
            children,
            text,
            ns: here.ns.clone(),
            library: here.library.clone(),
            scope: here.scope.clone(),
            at,
        };
        match name {
            "externalRef" => self.load(&el, &here, false),
            "include" => self.include(el, &here),
            _ => Ok(el),
        }
    }

    /// Section 4.3: an absolute URI without fragment, or empty; libxml2's
    /// checks in its order.
    fn check_library(&self, uri: &str, at: Loc) -> Result<(), SchemaError> {
        if uri.is_empty() {
            return Ok(());
        }
        let (code, message) = match Uri::parse(uri) {
            Err(_) => (
                SchemaCode::InvalidUri,
                format!("contains invalid URI {uri}"),
            ),
            // RFC 2396: an absolute URI has a scheme and a non-empty part after it.
            Ok(Uri { ref scheme, .. })
                if scheme.as_ref().is_none_or(|s| uri.len() <= s.len() + 1) =>
            {
                (
                    SchemaCode::UriNotAbsolute,
                    format!("URI {uri} is not absolute"),
                )
            }
            Ok(Uri {
                fragment: Some(_), ..
            }) => (
                SchemaCode::UriFragment,
                format!("URI {uri} has a fragment ID"),
            ),
            Ok(_) => return Ok(()),
        };
        Err(self.error(code, at, format!("Attribute datatypeLibrary {message}")))
    }

    /// Loads the document an `externalRef` or `include` names (sections 4.5
    /// to 4.7); its element inherits `ns` but not `datatypeLibrary`.
    fn load(&mut self, el: &El, here: &Inherited, include: bool) -> Result<El, SchemaError> {
        let (failure, recurse, what) = match include {
            true => (
                SchemaCode::IncludeFailure,
                SchemaCode::IncludeRecurse,
                "include",
            ),
            false => (
                SchemaCode::ExternalRefFailure,
                SchemaCode::ExternalRefRecurse,
                "externalRef",
            ),
        };
        let href = el.attr("href").ok_or_else(|| {
            self.error(
                SchemaCode::MissingHref,
                el.at,
                format!("xmlRelaxNGParse: {what} has no href attribute"),
            )
        })?;
        if href.contains('#') {
            return Err(self.error(
                SchemaCode::UriFragment,
                el.at,
                format!("Fragment forbidden in URI for {what} {href}"),
            ));
        }
        let uri = crate::uri::resolve(href, &here.base).map_err(|_| {
            self.error(
                SchemaCode::HrefError,
                el.at,
                format!("Incorrect URI for {what} {href}"),
            )
        })?;
        if self.stack.contains(&uri) {
            return Err(self.error(
                recurse,
                el.at,
                format!("Detected an {what} recursion for {uri}"),
            ));
        }
        if self.stack.len() > self.limits.max_include_depth {
            return Err(self.error(
                SchemaCode::ParseError,
                el.at,
                "xmlRelaxNG: inclusion recursion limit reached".into(),
            ));
        }
        let document = self.fetch(&uri, el.at, failure)?;
        self.resources.push(uri.clone());
        self.stack.push(uri.clone());
        let inherited = Inherited {
            ns: here.ns.clone(),
            library: Rc::from(""),
            base: Rc::from(uri.as_str()),
            scope: Rc::new([]),
            resource: self.resources.len() - 1,
        };
        let root = self.document_element(&document, &inherited);
        self.stack.pop();
        root
    }

    fn document_element(
        &mut self,
        document: &OwnedDocument,
        inherited: &Inherited,
    ) -> Result<El, SchemaError> {
        let root = document.root_element();
        let at = Loc {
            resource: inherited.resource,
            offset: root.range().map(|range| range.start),
        };
        match rng(Navigate::item(document, root.id())) {
            Some(qname) => self.convert(document, root.id(), qname, inherited),
            None => Err(self.error(
                SchemaCode::UnknownConstruct,
                at,
                "document element is not in the RELAX NG namespace".into(),
            )),
        }
    }

    fn fetch(
        &mut self,
        uri: &str,
        at: Loc,
        failure: SchemaCode,
    ) -> Result<OwnedDocument, SchemaError> {
        let xml = &self.limits.xml;
        let max_bytes = xml.max_external_bytes - self.bytes;
        if self.loads >= xml.max_external_loads {
            return Err(self.error(
                SchemaCode::ResourceLimit,
                at,
                "too many schema documents loaded".into(),
            ));
        }
        self.loads += 1;
        let request = Request {
            kind: ResourceKind::RelaxNg,
            uri,
            public_id: None,
            max_bytes,
        };
        let bytes = match self.resolver.resolve(request) {
            Ok(Some(bytes)) => bytes,
            Ok(None) | Err(_) => {
                return Err(self.error(failure, at, format!("Failed to load {uri}")));
            }
        };
        if bytes.len() > max_bytes {
            return Err(self.error(
                SchemaCode::ResourceLimit,
                at,
                "schema documents exceed the byte limit".into(),
            ));
        }
        self.bytes += bytes.len();
        OwnedDocument::parse_legacy(&bytes, *xml).map_err(|error| {
            self.error(
                SchemaCode::loading(error),
                at,
                format!("xmlRelaxNG: could not load {uri}: {error}"),
            )
        })
    }

    /// Section 4.7: the included grammar, minus the components the
    /// `include` overrides, followed by the overrides, as a `div`.
    fn include(&mut self, el: El, here: &Inherited) -> Result<El, SchemaError> {
        let mut grammar = self.load(&el, here, true)?;
        if grammar.name != "grammar" {
            return Err(self.error(
                SchemaCode::GrammarMissing,
                el.at,
                format!(
                    "xmlRelaxNG: included document {} root is not a grammar",
                    self.resources[grammar.at.resource]
                ),
            ));
        }
        let url = &self.resources[grammar.at.resource];
        let mut start = false;
        let mut defines = HashSet::new();
        overrides(&el.children, &mut start, &mut defines);
        if start && !remove(&mut grammar.children, &|c| c.name == "start") {
            return Err(self.error(
                SchemaCode::StartMissing,
                el.at,
                format!("xmlRelaxNG: include {url} has a start but not the included grammar"),
            ));
        }
        for name in defines {
            let named = |c: &El| c.name == "define" && c.attr("name") == Some(name);
            if !remove(&mut grammar.children, &named) {
                return Err(self.error(
                    SchemaCode::DefineMissing,
                    el.at,
                    format!(
                        "xmlRelaxNG: include {url} has a define {name} but not the included grammar"
                    ),
                ));
            }
        }
        grammar.children.extend(el.children);
        Ok(El {
            name: "div".into(),
            attrs: Vec::new(),
            ..grammar
        })
    }
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn rng(item: Item<'_>) -> Option<QName<'_>> {
    match item {
        Item::Element(name) if name.namespace == Some(RNG_NS) => Some(name),
        _ => None,
    }
}

/// Whether the overrides of an `include` contain a start, and their define names.
fn overrides<'e>(children: &'e [El], start: &mut bool, defines: &mut HashSet<&'e str>) {
    for child in children {
        match child.name.as_str() {
            "start" => *start = true,
            "define" => defines.extend(child.attr("name")),
            "div" => overrides(&child.children, start, defines),
            _ => {}
        }
    }
}

/// Removes matching components, descending into `div`s; whether any matched.
fn remove(children: &mut Vec<El>, matches: &dyn Fn(&El) -> bool) -> bool {
    let before = children.len();
    children.retain(|child| !matches(child));
    let mut found = children.len() != before;
    for child in children.iter_mut().filter(|child| child.name == "div") {
        found |= remove(&mut child.children, matches);
    }
    found
}
