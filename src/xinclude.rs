//! XInclude 1.0 as libxml2 2.15's `xinclude.c` processes it.
//!
//! [`process`] returns a new [`OwnedDocument`] and never changes its input.
//! It handles XML and text inclusion, fallbacks, `xml:base` fixup, XPointer
//! selection and loop detection; resources load only through the given
//! [`Resolver`], within cumulative budgets. See the
//! [XInclude contract](https://github.com/pbdeuchler/yaxml/blob/main/docs/xinclude.md).
//!
//! ```
//! use yaxml::{OwnedDocument, Request, Writer, xinclude};
//!
//! let main = OwnedDocument::parse(
//!     "<r xmlns:xi='http://www.w3.org/2001/XInclude'>\
//!      <xi:include href='p.xml'/><xi:include href='a.xml' xpointer='element(/1/2)'/></r>",
//! )?;
//! let resolver = |request: Request<'_>| {
//!     Ok(match request.uri {
//!         "p.xml" => Some(b"<p/>".to_vec()),
//!         "a.xml" => Some(b"<a><b/><c/></a>".to_vec()),
//!         _ => None,
//!     })
//! };
//! let result = xinclude::process(&main, "main.xml", Box::new(resolver), &Default::default())?;
//! let mut writer = Writer::new(Vec::new());
//! writer.write_owned_document(&result)?;
//! assert_eq!(writer.into_inner(), br#"<r xmlns:xi="http://www.w3.org/2001/XInclude"><p/><c/></r>"#);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use crate::encoding::{Charset, charset};
use crate::options::{DTDLOAD, DTDVALID, NOENT, XINCLUDE};
use crate::owned::TreeBuilder;
use crate::serialize::owned_leaf;
use crate::uri::{self, Uri};
use crate::{
    Attribute, Doctype, Element, Event, Limit, Limits, Name, NodeId, OwnedDocument, OwnedNode,
    OwnedNodeId, OwnedNodeKind, ParseOptions, Request, Resolver, ResourceKind, xpath, xpointer,
};

const NS: &str = "http://www.w3.org/2003/XInclude";
const OLD_NS: &str = "http://www.w3.org/2001/XInclude";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";
/// libxml2's `XML_MAX_URI_LENGTH`.
const MAX_URI: usize = 2000;

/// Processing options; the defaults match `XML_PARSE_XINCLUDE`.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Options {
    /// Set `xml:base` on included elements whose base URI differs from the
    /// inclusion point's. `false` is libxml2's `XML_PARSE_NOBASEFIX`.
    pub base_fixup: bool,
    /// Set `xml:lang` on included elements whose language differs from the
    /// inclusion point's (XInclude 1.0 section 4.5.6). libxml2 never does.
    pub lang_fixup: bool,
    /// Nested inclusions (libxml2's `XINCLUDE_MAX_DEPTH`, 40). Processing
    /// recurses this deep.
    pub max_depth: usize,
    /// Limits for parsing included documents and for the result.
    /// `max_external_loads` and `max_external_bytes` are cumulative over the
    /// whole call, including the included documents' DTDs; `max_events`
    /// bounds result nodes and, separately, nodes visited while expanding
    /// (document nodes aside, as in parsing);
    /// `max_owned_bytes` bounds the result's strings.
    pub limits: Limits,
    /// Ceilings for each XPointer evaluation.
    pub xpath: xpath::Limits,
    /// [`crate::options`] flags for parsing included documents; like
    /// libxml2, `DTDLOAD` is always added, and `NOENT` when an XPointer
    /// selects from the document. Default: [`crate::options::LEGACY`].
    pub parse_flags: u32,
}

setters!(const Options { base_fixup: bool, lang_fixup: bool, max_depth: usize, limits: Limits, xpath: xpath::Limits, parse_flags: u32 });

impl Options {
    /// libxml2's `XML_PARSE_XINCLUDE` behavior.
    pub const DEFAULT: Self = Self {
        base_fixup: true,
        lang_fixup: false,
        max_depth: 40,
        limits: Limits::DEFAULT,
        xpath: xpath::Limits::DEFAULT,
        parse_flags: crate::options::LEGACY,
    };
}

impl Default for Options {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// An XInclude failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    /// What went wrong.
    pub kind: ErrorKind,
    /// The document holding the failing element.
    pub uri: String,
    /// The failing element's source offset in that document, when it has one.
    pub offset: Option<usize>,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let at = (self.offset).map_or_else(String::new, |offset| format!(" at byte {offset}"));
        write!(f, "XInclude error {:?} in {:?}{at}", self.kind, self.uri)
    }
}

impl std::error::Error for Error {}

/// libxml2 `XML_XINCLUDE_*` categories.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// `NO_HREF`: neither `href` nor `xpointer`.
    NoHref,
    /// `PARSE_VALUE`: `parse` is neither `xml` nor `text`.
    InvalidParse,
    /// `HREF_URI`: an unusable `href`, or no relative `xml:base` exists.
    InvalidHref,
    /// `FRAGMENT_ID`: a fragment in `href` (allowed in the 2001 namespace).
    FragmentIdentifier,
    /// `RECURSION`: a local `parse="xml"` inclusion without an XPointer, or
    /// an inclusion loop.
    Recursion,
    /// `RECURSION`: more than [`Options::max_depth`] nested inclusions.
    RecursionDepth,
    /// `TEXT_DOCUMENT`: `parse="text"` of the including document itself.
    TextDocument,
    /// `UNKNOWN_ENCODING`
    UnknownEncoding,
    /// `INVALID_CHAR`: included text that does not decode to XML characters.
    InvalidCharacter,
    /// `XPTR_FAILED`
    XPointer(xpointer::Error),
    /// `XPTR_RESULT`: the XPointer selects an attribute or namespace node.
    XPointerResult,
    /// `NO_FALLBACK`: the resource could not be loaded (the resolver declined
    /// or failed, or the document is not well-formed: the parse error).
    NoFallback(Option<crate::Error>),
    /// `MULTIPLE_ROOT`: a top-level inclusion yields other than one element.
    MultipleRoots,
    /// `INCLUDE_IN_INCLUDE`
    IncludeInInclude,
    /// `FALLBACKS_IN_INCLUDE`
    MultipleFallbacks,
    /// `FALLBACK_NOT_IN_INCLUDE`
    FallbackNotInInclude,
    /// A limit or allocation failure while loading or building.
    Xml(crate::Error),
}

/// Replaces every `xi:include` in `document`, whose URI is `base`, and
/// returns the result (`xmlXIncludeProcess`).
///
/// A resource that cannot be loaded uses its `xi:fallback`; any other
/// libxml2 error fails the call. Each URI is loaded once per call. The
/// result has no `XINCLUDE_START`/`END` nodes (as with
/// `XML_PARSE_NOXINCNODE`), and adjacent Text nodes are not merged.
pub fn process(
    document: &OwnedDocument,
    base: &str,
    resolver: Box<dyn Resolver>,
    options: &Options,
) -> Result<OwnedDocument, Error> {
    let budget = Budget {
        resolver,
        loads: options.limits.max_external_loads,
        bytes: options.limits.max_external_bytes,
        exceeded: None,
    };
    Processor {
        options,
        loader: Loader(Arc::new(Mutex::new(budget))),
        documents: Vec::new(),
        texts: HashMap::new(),
        legacy: false,
        visited: 0,
    }
    .document(document, base, 0)
}

/// The caller's resolver with the call's cumulative budgets, shared with the
/// parsers of included documents.
#[derive(Clone)]
struct Loader(Arc<Mutex<Budget>>);

struct Budget {
    resolver: Box<dyn Resolver>,
    loads: usize,
    bytes: usize,
    exceeded: Option<Limit>,
}

impl Loader {
    fn exceeded(&self) -> Option<Limit> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .exceeded
    }
}

impl Resolver for Loader {
    fn resolve(&mut self, request: Request<'_>) -> io::Result<Option<Vec<u8>>> {
        let mut budget = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let exceed = |budget: &mut Budget, limit| {
            budget.exceeded = Some(limit);
            Err(io::Error::other("XInclude resource budget exhausted"))
        };
        let Some(loads) = budget.loads.checked_sub(1) else {
            return exceed(&mut budget, Limit::ExternalLoads);
        };
        budget.loads = loads;
        let max_bytes = request.max_bytes.min(budget.bytes);
        let response = budget.resolver.resolve(Request {
            max_bytes,
            ..request
        })?;
        match response.as_ref().map_or(Some(budget.bytes), |bytes| {
            budget.bytes.checked_sub(bytes.len())
        }) {
            Some(left) => {
                budget.bytes = left;
                Ok(response)
            }
            None => exceed(&mut budget, Limit::ExternalBytes),
        }
    }
}

/// One item an inclusion contributes: text, or a copy of a node subtree.
#[derive(Clone, Debug)]
enum Piece {
    Text(Rc<str>),
    Node {
        source: Source,
        node: OwnedNodeId,
        root: Root,
    },
}

/// The document a piece copies from: the one being processed, whose nested
/// inclusions are expanded while copying, or a processed cached document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Source {
    Local,
    Cached(usize),
}

/// `xml:base` and `xml:lang` changes on a copied top-level element.
#[derive(Clone, Debug, Default)]
struct Root {
    base: Fix,
    lang: Fix,
}

#[derive(Clone, Debug, Default)]
enum Fix {
    #[default]
    Keep,
    Remove,
    Set(String),
}

/// A parsed `xi:include` element.
struct Reference {
    url: String,
    fragment: Option<String>,
    xml: bool,
    /// The inclusion point's base and language, when they are fixed up.
    base: Option<String>,
    lang: Option<String>,
}

struct Cached {
    url: String,
    document: Option<OwnedDocument>,
    expanding: bool,
}

/// The document being processed and its expanded inclusions.
struct Local<'d> {
    document: &'d OwnedDocument,
    url: &'d str,
    /// `None` while expanding, to detect loops.
    memo: HashMap<OwnedNodeId, Option<Rc<[Piece]>>>,
    /// A newly loaded document had a DTD; libxml2 then creates an empty
    /// internal subset in a document without one.
    doctype: bool,
}

/// A load that failed without an error, so the fallback applies.
type Loaded = Result<Vec<Piece>, Option<crate::Error>>;

struct Processor<'o> {
    options: &'o Options,
    loader: Loader,
    documents: Vec<Cached>,
    texts: HashMap<String, Rc<str>>,
    /// An element in the 2001 namespace was seen, allowing `href` fragments.
    legacy: bool,
    visited: usize,
}

impl Processor<'_> {
    /// `xmlXIncludeDoProcess`: expands the direct inclusions, then copies.
    fn document(
        &mut self,
        document: &OwnedDocument,
        url: &str,
        depth: usize,
    ) -> Result<OwnedDocument, Error> {
        let mut local = Local {
            document,
            url,
            memo: HashMap::new(),
            doctype: false,
        };
        self.walk(&mut local, document.root(), depth, true)?;
        self.emit(&local)
    }

    /// Expands the inclusions in `top`'s subtree. `strict` adds libxml2's
    /// `xmlXIncludeTestNode` checks of the document's own tree.
    fn walk<'d>(
        &mut self,
        local: &mut Local<'d>,
        top: OwnedNode<'d>,
        depth: usize,
        strict: bool,
    ) -> Result<(), Error> {
        let mut next = Some(top);
        while let Some(node) = next {
            // A document node is no event, as in the parse's node ceiling.
            self.visited += usize::from(node.kind() != OwnedNodeKind::Document);
            if self.visited > self.options.limits.max_events {
                let error = crate::Error::new(crate::ErrorKind::LimitExceeded(Limit::Events), 0);
                return Err(fail(local, None, ErrorKind::Xml(error)));
            }
            let include = match strict {
                true => self.test(local, node)?,
                false => is_include(node),
            };
            if include {
                self.expand(local, node, depth)?;
            }
            // libxml2 does not look for inclusions inside entity references.
            let opaque = include || node.kind() == OwnedNodeKind::EntityRef;
            next = (!opaque)
                .then(|| node.first_child())
                .flatten()
                .or_else(|| following(node, top));
        }
        Ok(())
    }

    fn test(&mut self, local: &Local<'_>, node: OwnedNode<'_>) -> Result<bool, Error> {
        let Some((name, namespace)) = xi_name(node) else {
            return Ok(false);
        };
        self.legacy |= namespace == OLD_NS;
        let error = |kind| Err(fail(local, Some(node), kind));
        match name {
            "include" => {
                let children = node.children().filter_map(xi_name);
                let mut fallbacks = 0;
                for (child, _) in children {
                    match child {
                        "include" => return error(ErrorKind::IncludeInInclude),
                        "fallback" => fallbacks += 1,
                        _ => {}
                    }
                }
                match fallbacks > 1 {
                    true => error(ErrorKind::MultipleFallbacks),
                    false => Ok(true),
                }
            }
            "fallback" if !node.parent().is_some_and(is_include) => {
                error(ErrorKind::FallbackNotInInclude)
            }
            _ => Ok(false),
        }
    }

    /// `xmlXIncludeExpandNode`: the memoized pieces of one inclusion.
    fn expand<'d>(
        &mut self,
        local: &mut Local<'d>,
        node: OwnedNode<'d>,
        depth: usize,
    ) -> Result<Rc<[Piece]>, Error> {
        if depth >= self.options.max_depth {
            return Err(fail(local, Some(node), ErrorKind::RecursionDepth));
        }
        match local.memo.get(&node.id()) {
            Some(Some(pieces)) => return Ok(pieces.clone()),
            Some(None) => return Err(fail(local, Some(node), ErrorKind::Recursion)),
            None => {}
        }
        local.memo.insert(node.id(), None);
        let pieces: Rc<[Piece]> = self.load(local, node, depth + 1)?.into();
        local.memo.insert(node.id(), Some(pieces.clone()));
        Ok(pieces)
    }

    /// `xmlXIncludeLoadNode`: the resource, or else the fallback's children.
    fn load<'d>(
        &mut self,
        local: &mut Local<'d>,
        node: OwnedNode<'d>,
        depth: usize,
    ) -> Result<Vec<Piece>, Error> {
        let reference = self.reference(local, node)?;
        let loaded = match reference.xml {
            true => self.load_xml(local, node, &reference, depth)?,
            false => self.load_text(local, node, &reference)?,
        };
        let cause = match loaded {
            Ok(pieces) => return Ok(pieces),
            Err(cause) => cause,
        };
        let fallback = node
            .children()
            .find(|child| xi_name(*child).is_some_and(|(name, _)| name == "fallback"));
        let Some(fallback) = fallback else {
            return Err(fail(local, Some(node), ErrorKind::NoFallback(cause)));
        };
        let mut pieces = Vec::new();
        for child in fallback.children() {
            pieces.extend(self.copy_local(local, child, &reference, depth)?);
        }
        Ok(pieces)
    }

    /// `xmlXIncludeCopyNode`: a local node whose nested inclusions expand.
    fn copy_local<'d>(
        &mut self,
        local: &mut Local<'d>,
        node: OwnedNode<'d>,
        reference: &Reference,
        depth: usize,
    ) -> Result<Vec<Piece>, Error> {
        if is_include(node) {
            return Ok(self.expand(local, node, depth)?.to_vec());
        }
        self.walk(local, node, depth, false)?;
        let root =
            root(node, local.url, reference).map_err(|kind| fail(local, Some(node), kind))?;
        Ok(vec![Piece::Node {
            source: Source::Local,
            node: node.id(),
            root,
        }])
    }

    /// `xmlXIncludeAddNode`: reads and resolves the element's attributes.
    fn reference(&self, local: &Local<'_>, node: OwnedNode<'_>) -> Result<Reference, Error> {
        let error = |kind| fail(local, Some(node), kind);
        let mut fragment = self.property(node, "xpointer").map(str::to_owned);
        let href = match self.property(node, "href") {
            None if fragment.is_none() => return Err(error(ErrorKind::NoHref)),
            None => "",
            Some(href) if href.len() > MAX_URI => return Err(error(ErrorKind::InvalidHref)),
            Some(href) => href,
        };
        let xml = match self.property(node, "parse") {
            None | Some("xml") => true,
            Some("text") => false,
            Some(_) => return Err(error(ErrorKind::InvalidParse)),
        };
        let mut parsed = Uri::parse(href).map_err(|_| error(ErrorKind::InvalidHref))?;
        if let Some(identifier) = parsed.fragment.take() {
            if !self.legacy {
                return Err(error(ErrorKind::FragmentIdentifier));
            }
            fragment.get_or_insert(identifier);
        }
        let href = parsed.to_string();
        let base = base_of(node, local.url);
        let url = match href.is_empty() {
            true => href,
            false => uri::resolve(&href, base.as_deref().unwrap_or(""))
                .map_err(|_| error(ErrorKind::InvalidHref))?,
        };
        if (url.is_empty() || url == local.url)
            && xml
            && fragment.as_deref().is_none_or(str::is_empty)
        {
            return Err(error(ErrorKind::Recursion));
        }
        Ok(Reference {
            url,
            fragment,
            xml,
            base: self.options.base_fixup.then(|| base.unwrap_or_default()),
            lang: (self.options.lang_fixup).then(|| lang_of(node).unwrap_or_default().to_owned()),
        })
    }

    /// `xmlXIncludeGetProp`: the XInclude-namespaced attribute, then the plain one.
    fn property<'d>(&self, node: OwnedNode<'d>, name: &str) -> Option<&'d str> {
        [Some(NS), self.legacy.then_some(OLD_NS)]
            .into_iter()
            .flatten()
            .map(Some)
            .chain([None])
            .find_map(|namespace| attribute(node, namespace, name))
    }

    /// `xmlXIncludeLoadDoc`.
    fn load_xml<'d>(
        &mut self,
        local: &mut Local<'d>,
        node: OwnedNode<'d>,
        reference: &Reference,
        depth: usize,
    ) -> Result<Loaded, Error> {
        let source = if reference.url.is_empty() || reference.url == local.url {
            Source::Local
        } else {
            match self
                .documents
                .iter()
                .position(|cached| cached.url == reference.url)
            {
                Some(index) if self.documents[index].expanding => {
                    return Err(fail(local, Some(node), ErrorKind::Recursion));
                }
                Some(index) if self.documents[index].document.is_none() => return Ok(Err(None)),
                Some(index) => Source::Cached(index),
                None => {
                    let index = self.documents.len();
                    self.documents.push(Cached {
                        url: reference.url.clone(),
                        document: None,
                        expanding: true,
                    });
                    let xpointer = reference.fragment.is_some();
                    let parsed = self.parse(local, &reference.url, depth, xpointer);
                    self.documents[index].expanding = false;
                    match parsed? {
                        Ok(document) => self.documents[index].document = Some(document),
                        Err(cause) => return Ok(Err(cause)),
                    }
                    Source::Cached(index)
                }
            }
        };
        let fragment = reference.fragment.as_deref();
        let error = |kind| fail(local, Some(node), kind);
        let Source::Cached(index) = source else {
            let mut pieces = Vec::new();
            for found in select(local.document, fragment, &self.options.xpath).map_err(error)? {
                pieces.extend(self.copy_local(local, found, reference, depth)?);
            }
            return Ok(Ok(pieces));
        };
        let cached = &self.documents[index];
        let document = cached.document.as_ref().expect("cached document loaded");
        select(document, fragment, &self.options.xpath)
            .and_then(|nodes| {
                nodes
                    .into_iter()
                    .map(|found| {
                        Ok(Piece::Node {
                            source,
                            node: found.id(),
                            root: root(found, &cached.url, reference)?,
                        })
                    })
                    .collect()
            })
            .map(Ok)
            .map_err(error)
    }

    /// Loads, parses and processes an included document.
    fn parse(
        &mut self,
        local: &mut Local<'_>,
        url: &str,
        depth: usize,
        xpointer: bool,
    ) -> Result<Result<OwnedDocument, Option<crate::Error>>, Error> {
        let Some(bytes) = self.fetch(local, ResourceKind::XInclude, url)? else {
            return Ok(Err(None));
        };
        let loader = Box::new(self.loader.clone());
        let flags = (self.options.parse_flags | DTDLOAD | if xpointer { NOENT } else { 0 })
            & !(XINCLUDE | DTDVALID);
        let options = ParseOptions::new(flags).with_limits(self.options.limits);
        let parsed = OwnedDocument::parse_with(&bytes, options.with_resolver(url, loader));
        self.check_budget(local)?;
        let document = match parsed {
            Ok(document) => document,
            Err(error) if fatal(error) => return Err(fail(local, None, ErrorKind::Xml(error))),
            Err(error) => return Ok(Err(Some(error))),
        };
        local.doctype |= document.doctype().is_some();
        self.document(&document, url, depth).map(Ok)
    }

    /// `xmlXIncludeLoadTxt`.
    fn load_text(
        &mut self,
        local: &Local<'_>,
        node: OwnedNode<'_>,
        reference: &Reference,
    ) -> Result<Loaded, Error> {
        let error = |kind| fail(local, Some(node), kind);
        if reference.url.is_empty() {
            return Err(error(ErrorKind::TextDocument));
        }
        if let Some(text) = self.texts.get(&reference.url) {
            return Ok(Ok(vec![Piece::Text(text.clone())]));
        }
        let encoding = self
            .property(node, "encoding")
            .map(|label| charset(label).ok_or_else(|| error(ErrorKind::UnknownEncoding)))
            .transpose()?;
        let Some(bytes) = self.fetch(local, ResourceKind::XIncludeText, &reference.url)? else {
            return Ok(Err(None));
        };
        let text: Rc<str> = decode_text(&bytes, encoding.unwrap_or(Charset::Utf8))
            .filter(|text| text.chars().all(crate::escape::xml_char))
            .ok_or_else(|| error(ErrorKind::InvalidCharacter))?
            .into();
        self.texts.insert(reference.url.clone(), text.clone());
        Ok(Ok(vec![Piece::Text(text)]))
    }

    /// Requests a resource; a declined or failed request is `None`.
    fn fetch(
        &mut self,
        local: &Local<'_>,
        kind: ResourceKind,
        url: &str,
    ) -> Result<Option<Vec<u8>>, Error> {
        let request = Request {
            kind,
            uri: url,
            public_id: None,
            max_bytes: usize::MAX,
        };
        let response = self.loader.resolve(request);
        self.check_budget(local)?;
        Ok(response.ok().flatten())
    }

    fn check_budget(&self, local: &Local<'_>) -> Result<(), Error> {
        match self.loader.exceeded() {
            Some(limit) => {
                let error = crate::Error::new(crate::ErrorKind::LimitExceeded(limit), 0);
                Err(fail(local, None, ErrorKind::Xml(error)))
            }
            None => Ok(()),
        }
    }

    fn source<'a>(&'a self, local: &Local<'a>, source: Source) -> &'a OwnedDocument {
        match source {
            Source::Local => local.document,
            Source::Cached(index) => self.documents[index]
                .document
                .as_ref()
                .expect("cached document loaded"),
        }
    }

    /// Copies the processed document, substituting each direct inclusion.
    fn emit(&self, local: &Local<'_>) -> Result<OwnedDocument, Error> {
        let document = local.document;
        let xml = |error| fail(local, None, ErrorKind::Xml(error));
        let mut builder =
            TreeBuilder::new(self.options.limits, document.declaration()).map_err(xml)?;
        let mut stack = vec![Work::Nodes {
            source: Source::Local,
            next: document.root().first_child(),
            parent: TreeBuilder::ROOT,
            default: "",
        }];
        let mut doctype = local.doctype && document.doctype().is_none();
        while let Some(work) = stack.pop() {
            match work {
                Work::Close(id) => builder.close(id),
                Work::Pieces {
                    pieces,
                    parent,
                    default,
                } => {
                    let Some((piece, rest)) = pieces.split_first() else {
                        continue;
                    };
                    stack.push(Work::Pieces {
                        pieces: rest,
                        parent,
                        default,
                    });
                    match piece {
                        Piece::Text(text) => {
                            let event = Event::Text(Cow::Borrowed(text));
                            builder.append(parent, event, &[]).map_err(xml)?;
                        }
                        Piece::Node { source, node, root } => {
                            let node = self.source(local, *source).node(*node);
                            let node = node.expect("piece nodes are live");
                            let copy = Copy {
                                source: *source,
                                node,
                                root: Some(root),
                                parent,
                                default,
                            };
                            self.copy(local, &mut builder, &mut stack, copy)
                                .map_err(xml)?;
                        }
                    }
                }
                Work::Nodes {
                    source,
                    next: Some(node),
                    parent,
                    default,
                } => {
                    stack.push(Work::Nodes {
                        source,
                        next: node.next_sibling(),
                        parent,
                        default,
                    });
                    if doctype
                        && parent == TreeBuilder::ROOT
                        && node.kind() == OwnedNodeKind::Element
                    {
                        doctype = false;
                        let name = document.root_element().qualified_name().unwrap_or_default();
                        let doctype = Doctype {
                            name: name.rsplit(':').next().unwrap_or(name),
                            public_id: None,
                            system_id: None,
                            internal_subset: None,
                        };
                        builder
                            .append(parent, Event::Doctype(doctype), &[])
                            .map_err(xml)?;
                    }
                    let memo = (source == Source::Local && is_include(node))
                        .then(|| local.memo.get(&node.id()))
                        .flatten();
                    if let Some(pieces) = memo {
                        let pieces = pieces.as_deref();
                        let pieces = pieces.expect("direct inclusions are expanded");
                        if parent == TreeBuilder::ROOT && !self.one_root(local, pieces) {
                            return Err(fail(local, Some(node), ErrorKind::MultipleRoots));
                        }
                        stack.push(Work::Pieces {
                            pieces,
                            parent,
                            default,
                        });
                        continue;
                    }
                    let copy = Copy {
                        source,
                        node,
                        root: None,
                        parent,
                        default,
                    };
                    self.copy(local, &mut builder, &mut stack, copy)
                        .map_err(xml)?;
                }
                Work::Nodes { next: None, .. } => {}
            }
        }
        builder.finish().map_err(xml)
    }

    /// `xmlXIncludeIncludeNode`'s check for a top-level inclusion.
    fn one_root(&self, local: &Local<'_>, pieces: &[Piece]) -> bool {
        let kinds = pieces.iter().map(|piece| match piece {
            Piece::Text(_) => OwnedNodeKind::Text,
            Piece::Node { source, node, .. } => self
                .source(local, *source)
                .node(*node)
                .map_or(OwnedNodeKind::Text, OwnedNode::kind),
        });
        let mut elements = 0;
        for kind in kinds {
            match kind {
                OwnedNodeKind::Element => elements += 1,
                OwnedNodeKind::Text | OwnedNodeKind::Cdata => return false,
                _ => {}
            }
        }
        elements == 1
    }

    /// Appends one node; an element's children are queued after it.
    fn copy<'a>(
        &'a self,
        local: &'a Local<'a>,
        builder: &mut TreeBuilder,
        stack: &mut Vec<Work<'a>>,
        copy: Copy<'a>,
    ) -> Result<(), crate::Error> {
        let Copy {
            source,
            node,
            root,
            parent,
            default,
        } = copy;
        if node.kind() != OwnedNodeKind::Element {
            let event = owned_leaf(node).expect("non-element nodes are leaves");
            let id = builder.append(parent, event, &[])?;
            // An entity reference keeps its replacement nodes, unprocessed.
            if node.kind() == OwnedNodeKind::EntityRef {
                stack.push(Work::Close(id));
                stack.push(Work::Nodes {
                    source,
                    next: node.first_child(),
                    parent: id,
                    default,
                });
            }
            return Ok(());
        }
        let added = match root {
            Some(_) => declarations(node, source == Source::Local, default),
            None => Vec::new(),
        };
        let ids = self.source(local, source).ids();
        let mut attributes = Vec::new();
        let mut positions = Vec::new();
        let (declared, own): (Vec<_>, Vec<_>) = node
            .attributes()
            .enumerate()
            .partition(|(_, attribute)| attribute.name.namespace() == Some(XMLNS_NS));
        attributes.extend(declared.into_iter().map(|(_, attribute)| attribute));
        attributes.extend(added.iter().map(|&(prefix, uri)| declaration(prefix, uri)));
        let mut fixes = root.map_or(Vec::new(), |root| {
            vec![("base", root.base.clone()), ("lang", root.lang.clone())]
        });
        for (index, attribute) in own {
            let fix = fixes.iter().position(|(name, _)| {
                attribute.name.namespace() == Some(XML_NS) && attribute.name.local() == *name
            });
            match fix.map(|fix| fixes.remove(fix).1) {
                Some(Fix::Remove) => continue,
                Some(Fix::Set(value)) => attributes.push(Attribute {
                    value: Cow::Owned(value),
                    ..attribute
                }),
                Some(Fix::Keep) | None => {
                    if ids.binary_search(&(node.id(), index)).is_ok() {
                        positions.push(attributes.len());
                    }
                    attributes.push(attribute);
                }
            }
        }
        for (name, fix) in fixes {
            if let Fix::Set(value) = fix {
                let qualified = Cow::Owned(format!("xml:{name}"));
                attributes.push(Attribute {
                    name: Name::from_validated_parts(
                        qualified,
                        NonZeroUsize::new(3),
                        Some(Cow::Borrowed(XML_NS)),
                    ),
                    value: Cow::Owned(value),
                });
            }
        }
        let default = (0..)
            .map_while(|index| node.attribute_parts(index))
            .find(|((qualified, _), _)| *qualified == "xmlns")
            .map(|(_, value)| value)
            .or_else(|| {
                added
                    .iter()
                    .find(|(prefix, _)| prefix.is_empty())
                    .map(|(_, uri)| *uri)
            })
            .unwrap_or(default);
        let name = node.name().expect("element has a name");
        let event = Event::Start(Element {
            name,
            attributes: attributes.into(),
        });
        let id = builder.append(parent, event, &positions)?;
        stack.push(Work::Close(id));
        stack.push(Work::Nodes {
            source,
            next: node.first_child(),
            parent: id,
            default,
        });
        Ok(())
    }
}

/// Copy work, popped in document order. `default` is the parent's default
/// namespace in the result.
enum Work<'a> {
    Nodes {
        source: Source,
        next: Option<OwnedNode<'a>>,
        parent: NodeId,
        default: &'a str,
    },
    Pieces {
        pieces: &'a [Piece],
        parent: NodeId,
        default: &'a str,
    },
    Close(NodeId),
}

struct Copy<'a> {
    source: Source,
    node: OwnedNode<'a>,
    root: Option<&'a Root>,
    parent: NodeId,
    default: &'a str,
}

fn fail(local: &Local<'_>, node: Option<OwnedNode<'_>>, kind: ErrorKind) -> Error {
    Error {
        kind,
        uri: local.url.to_owned(),
        offset: node.and_then(OwnedNode::range).map(|range| range.start),
    }
}

/// Limits and allocation failures stop processing; other parse errors make
/// the resource unavailable, as libxml2 only reports them.
fn fatal(error: crate::Error) -> bool {
    matches!(
        error.kind,
        crate::ErrorKind::LimitExceeded(_)
            | crate::ErrorKind::Allocation
            | crate::ErrorKind::IdentifierExhausted
    )
}

/// The local name and namespace of an element in either XInclude namespace.
fn xi_name(node: OwnedNode<'_>) -> Option<(&str, &str)> {
    let (qualified, namespace) = node.name_parts()?;
    let namespace = namespace.filter(|namespace| [NS, OLD_NS].contains(namespace))?;
    Some((qualified.rsplit(':').next().unwrap_or(qualified), namespace))
}

fn is_include(node: OwnedNode<'_>) -> bool {
    xi_name(node).is_some_and(|(name, _)| name == "include")
}

/// The next preorder node after `node`'s subtree, inside `top`'s.
fn following<'d>(node: OwnedNode<'d>, top: OwnedNode<'d>) -> Option<OwnedNode<'d>> {
    std::iter::successors(Some(node), |node| node.parent())
        .take_while(|node| node.id() != top.id())
        .find_map(OwnedNode::next_sibling)
}

/// An attribute value by namespace and local name.
fn attribute<'d>(node: OwnedNode<'d>, namespace: Option<&str>, local: &str) -> Option<&'d str> {
    (0..)
        .map_while(|index| node.attribute_parts(index))
        .find(|((qualified, found), _)| {
            *found == namespace && qualified.rsplit(':').next() == Some(local)
        })
        .map(|(_, value)| value)
}

/// `xmlNodeGetBaseSafe`: `xml:base` attributes resolved outward, then against
/// the document URI; `None` without either.
fn base_of(node: OwnedNode<'_>, url: &str) -> Option<String> {
    let url = (!url.is_empty()).then_some(url);
    crate::xpath::tree::base(node.document(), node.id(), url)
}

fn lang_of<'d>(node: OwnedNode<'d>) -> Option<&'d str> {
    node.lang()
}

/// The base and language fixes for a copied top-level node
/// (`xmlXIncludeBaseFixup`, and XInclude's language fixup).
fn root(node: OwnedNode<'_>, url: &str, reference: &Reference) -> Result<Root, ErrorKind> {
    if node.kind() != OwnedNodeKind::Element {
        return Ok(Root::default());
    }
    let base = match (&reference.base, base_of(node, url)) {
        (None, _) => Fix::Keep,
        (Some(target), Some(base)) if base != *target => {
            let relative = match base.len() > MAX_URI || target.len() > MAX_URI {
                true => base,
                false => uri::relative(&base, target).map_err(|_| ErrorKind::InvalidHref)?,
            };
            // A base without a slash can be omitted.
            match relative.contains('/') {
                true => Fix::Set(uri::canonic_path(&relative)),
                false => Fix::Remove,
            }
        }
        (Some(_), _) => Fix::Remove,
    };
    let lang = match &reference.lang {
        Some(target) if lang_of(node).unwrap_or_default() != target => {
            Fix::Set(lang_of(node).unwrap_or_default().to_owned())
        }
        _ => Fix::Keep,
    };
    Ok(Root { base, lang })
}

/// The nodes an XPointer (or, without one, the document element) selects;
/// the document node stands for its element.
fn select<'d>(
    document: &'d OwnedDocument,
    fragment: Option<&str>,
    limits: &xpath::Limits,
) -> Result<Vec<OwnedNode<'d>>, ErrorKind> {
    // A recovered document without an element selects nothing.
    let Some(fragment) = fragment else {
        return Ok(document.document_element().into_iter().collect());
    };
    xpointer::evaluate(document, fragment, limits)
        .map_err(ErrorKind::XPointer)?
        .into_iter()
        .filter_map(|found| match found {
            xpath::Node::Tree(id) => {
                let node = document.node(id).expect("XPath selects live nodes");
                match node.kind() {
                    OwnedNodeKind::Document => document.document_element(),
                    _ => Some(node),
                }
                .map(Ok)
            }
            _ => Some(Err(ErrorKind::XPointerResult)),
        })
        .collect()
}

/// Namespace declarations a copied subtree needs on its top element: the
/// prefixes its names use without declaring them inside the copy, as
/// libxml2's `xmlStaticCopyNode` adds them. An unprefixed element without a
/// namespace below a result default namespace also gets `xmlns=""`, which
/// libxml2 omits. Nested local inclusions are copied separately.
fn declarations<'d>(top: OwnedNode<'d>, local: bool, default: &str) -> Vec<(&'d str, &'d str)> {
    let mut added: Vec<(&str, &str)> = Vec::new();
    let mut next = Some(top);
    while let Some(node) = next {
        let descend = node.kind() == OwnedNodeKind::Element && !(local && is_include(node));
        if descend {
            let element = node.name_parts().into_iter();
            let attributes = (0..)
                .map_while(|index| node.attribute_parts(index))
                .map(|(name, _)| name)
                .filter(|(qualified, namespace)| {
                    qualified.contains(':') && *namespace != Some(XMLNS_NS)
                });
            for (qualified, namespace) in element.chain(attributes) {
                let prefix = qualified.split_once(':').map_or("", |(prefix, _)| prefix);
                let namespace = namespace.filter(|namespace| !namespace.is_empty());
                if prefix == "xml"
                    || added.iter().any(|(seen, _)| *seen == prefix)
                    || declared(node, top, prefix)
                {
                    continue;
                }
                match namespace {
                    Some(namespace) => added.push((prefix, namespace)),
                    None if !default.is_empty() => added.push(("", "")),
                    None => {}
                }
            }
        }
        next = descend
            .then(|| node.first_child())
            .flatten()
            .or_else(|| following(node, top));
    }
    added
}

/// Whether `prefix` is declared on `node` or an ancestor up to `top`.
fn declared(node: OwnedNode<'_>, top: OwnedNode<'_>, prefix: &str) -> bool {
    for element in std::iter::successors(Some(node), |node| node.parent()) {
        let declares = (0..).map_while(|index| element.attribute_parts(index)).any(
            |((qualified, namespace), _)| {
                namespace == Some(XMLNS_NS)
                    && qualified.split_once(':').map_or("", |(_, local)| local) == prefix
            },
        );
        if declares || element.id() == top.id() {
            return declares;
        }
    }
    false
}

fn declaration<'d>(prefix: &str, uri: &'d str) -> Attribute<'d> {
    let (qualified, prefix_length) = match prefix {
        "" => (Cow::Borrowed("xmlns"), None),
        prefix => (Cow::Owned(format!("xmlns:{prefix}")), NonZeroUsize::new(5)),
    };
    Attribute {
        name: Name::from_validated_parts(qualified, prefix_length, Some(Cow::Borrowed(XMLNS_NS))),
        value: Cow::Borrowed(uri),
    }
}

/// Decodes included text: UTF-8 by default, otherwise the named charset.
fn decode_text(bytes: &[u8], charset: Charset) -> Option<String> {
    match charset {
        Charset::Utf8 => String::from_utf8(bytes.to_vec()).ok(),
        // libxml2 reads "UTF-16" as little-endian and keeps any BOM as U+FEFF.
        Charset::Utf16(order) => {
            let little_endian = order.unwrap_or(true);
            let units = bytes.chunks(2).map(|pair| match (pair, little_endian) {
                ([low, high], true) => Some(u16::from_le_bytes([*low, *high])),
                ([high, low], false) => Some(u16::from_be_bytes([*high, *low])),
                _ => None,
            });
            let units: Option<Vec<u16>> = units.collect();
            char::decode_utf16(units?).collect::<Result<_, _>>().ok()
        }
        Charset::Byte(table) => bytes
            .iter()
            .map(|&byte| match byte {
                0..0x80 => Some(char::from(byte)),
                _ => Some(table[usize::from(byte - 0x80)])
                    .filter(|&code| code != 0)
                    .and_then(|code| char::from_u32(code.into())),
            })
            .collect(),
    }
}
