//! Canonical XML 1.0 and 1.1 and Exclusive XML Canonicalization 1.0 over
//! either DOM, with libxml2 2.15.4's `c14n.c` bytes.
//!
//! Where libxml2 departs from the recommendations, this module follows
//! libxml2. A tree with kept entity references fails with
//! [`Error::EntityReference`]; parse with `NOENT`. See the
//! [C14N contract](https://github.com/pbdeuchler/yaxml/blob/main/docs/c14n.md).
//!
//! ```
//! use yaxml::OwnedDocument;
//! use yaxml::c14n::{self, Mode, Options};
//! use yaxml::xpath::{Context, XPath};
//!
//! let document = OwnedDocument::parse("<r xmlns:p='urn:p'><p:e b='2' a='1'/></r>")?;
//! let whole = c14n::to_string(&document, &Options::new(Mode::Exclusive10))?;
//! assert_eq!(whole, r#"<r><p:e xmlns:p="urn:p" a="1" b="2"></p:e></r>"#);
//!
//! let context = Context::new().with_namespace("p", "urn:p");
//! let nodes = XPath::compile("(//. | //@* | //namespace::*)[ancestor-or-self::p:e]")?
//!     .select_nodes(&document, document.root().id(), &context)?;
//! let options = Options::new(Mode::Exclusive10).nodes(Some(&nodes));
//! assert_eq!(c14n::to_string(&document, &options)?, r#"<p:e xmlns:p="urn:p" a="1" b="2"></p:e>"#);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::io::{self, BufWriter, Write};
use std::iter::successors;

use crate::uri::{self, Uri};
use crate::xpath::tree::{self, Item, Navigate, QName, XML_NAMESPACE};
use crate::xpath::{Node, Tree};

/// libxml2 `xmlC14NMode`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    /// `XML_C14N_1_0`: Canonical XML 1.0.
    C14n10,
    /// `XML_C14N_EXCLUSIVE_1_0`: Exclusive XML Canonicalization 1.0.
    Exclusive10,
    /// `XML_C14N_1_1`: Canonical XML 1.1.
    C14n11,
}

/// What to canonicalize and how; the arguments of `xmlC14NDocSaveTo`.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Options<'a, Id> {
    /// The canonicalization algorithm.
    pub mode: Mode,
    /// Whether comments are kept.
    pub with_comments: bool,
    /// The XPath node-set to render; `None` renders the whole document.
    pub nodes: Option<&'a [Node<Id>]>,
    /// Exclusive mode's InclusiveNamespaces PrefixList; `#default` or `""`
    /// names the default namespace. The inclusive modes ignore it.
    pub inclusive_prefixes: &'a [&'a str],
}

setters!(const Options['a, Id] { mode: Mode, with_comments: bool, nodes: Option<&'a [Node<Id>]>, inclusive_prefixes: &'a [&'a str] });

impl<Id> Options<'_, Id> {
    /// The whole document, without comments.
    pub const fn new(mode: Mode) -> Self {
        Self {
            mode,
            with_comments: false,
            nodes: None,
            inclusive_prefixes: &[],
        }
    }
}

/// A canonicalization failure. Output written before it is not retracted.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// `XML_ERR_INVALID_URI`: a namespace URI does not parse, or `xml:base`
    /// fixup cannot resolve a reference.
    InvalidUri(String),
    /// `XML_C14N_RELATIVE_NAMESPACE`: a namespace URI without a scheme.
    RelativeNamespace(String),
    /// The sink failed.
    Io(io::Error),
    /// An HTML document holds names or text that XML cannot represent.
    NotXml(String),
    /// `XML_C14N_INVALID_NODE`: the tree keeps an entity reference, which
    /// canonical XML cannot represent; parse with `NOENT`.
    EntityReference(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUri(uri) => write!(f, "C14N error: invalid URI {uri:?}"),
            Self::RelativeNamespace(uri) => write!(f, "C14N error: relative namespace {uri:?}"),
            Self::Io(error) => write!(f, "C14N output error: {error}"),
            Self::NotXml(problem) => write!(f, "C14N error: {problem}"),
            Self::EntityReference(name) => {
                write!(
                    f,
                    "C14N error: entity reference &{name}; (parse with NOENT)"
                )
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

/// Writes the canonical form to `sink` and flushes it (`xmlC14NDocSaveTo`).
/// Output written before an error stays in the sink.
pub fn write<T: Tree>(
    tree: &T,
    options: &Options<'_, T::Id>,
    sink: impl Write,
) -> Result<(), Error> {
    let (mut sink, mut failed) = (BufWriter::new(sink), None);
    // The first sink error ends output; it outranks a later canonicalization error.
    let result = canonicalize(tree, options, |text| {
        failed = failed
            .take()
            .or_else(|| sink.write_all(text.as_bytes()).err());
    });
    failed.map_or(result, |error| Err(Error::Io(error)))?;
    sink.flush().map_err(Error::Io)
}

/// The canonical form as a string (`xmlC14NDocDumpMemory`).
pub fn to_string<T: Tree>(tree: &T, options: &Options<'_, T::Id>) -> Result<String, Error> {
    let mut out = String::new();
    canonicalize(tree, options, |text| out.push_str(text))?;
    Ok(out)
}

/// `xmlC14NPosition`: where top-level comments and PIs get their newline.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Position {
    Before,
    Inside,
    After,
}

/// `xmlC14NNormalizationMode`.
#[derive(Clone, Copy)]
enum Escape {
    Attribute,
    Text,
    Other,
}

fn replacement(byte: u8, escape: Escape) -> Option<&'static str> {
    use Escape::{Attribute, Text};
    match (byte, escape) {
        (b'\r', _) => Some("&#xD;"),
        (b'<', Attribute | Text) => Some("&lt;"),
        (b'&', Attribute | Text) => Some("&amp;"),
        (b'>', Text) => Some("&gt;"),
        (b'"', Attribute) => Some("&quot;"),
        (b'\t', Attribute) => Some("&#x9;"),
        (b'\n', Attribute) => Some("&#xA;"),
        _ => None,
    }
}

fn prefix(qualified: &str) -> &str {
    qualified.split_once(':').map_or("", |(prefix, _)| prefix)
}

fn xml_attribute<'t, T: Navigate>(
    tree: &'t T,
    id: T::Id,
    local: &str,
) -> Option<(QName<'t>, &'t str)> {
    tree::attributes(tree, id)
        .find(|(_, name, _)| name.namespace == Some(XML_NAMESPACE) && name.local == local)
        .map(|(_, name, value)| (name, value))
}

/// The explicit node-set; namespace nodes become (owner, prefix) as libxml2 matches them.
struct Subset<'t, Id> {
    nodes: HashSet<Node<Id>>,
    namespaces: HashSet<(Id, &'t str)>,
}

fn subset<'t, T: Navigate>(tree: &'t T, nodes: &[Node<T::Id>]) -> Subset<'t, T::Id> {
    let mut namespaces = HashSet::new();
    // Sorted node-sets list an element's namespace nodes together: compute its scope once.
    let (mut element, mut scope) = (None, Vec::new());
    for &node in nodes {
        if let Node::Namespace(owner, index) = node {
            if element != Some(owner) {
                (element, scope) = (Some(owner), tree::namespaces(tree, owner));
            }
            if let Some(&(prefix, _)) = scope.get(index) {
                namespaces.insert((owner, prefix));
            }
        }
    }
    Subset {
        nodes: nodes.iter().copied().collect(),
        namespaces,
    }
}

/// An open element and the state its end tag restores.
struct Frame<Id> {
    id: Id,
    visible: bool,
    rendered: usize,
    previous: (usize, usize),
    scope: usize,
    document_child: bool,
}

/// Traversal state: libxml2's `xmlC14NCtx` plus the in-scope declarations.
struct C14n<'t, 'o, T: Navigate, F> {
    tree: &'t T,
    options: &'o Options<'o, T::Id>,
    subset: Option<Subset<'t, T::Id>>,
    emit: F,
    /// Declarations in scope as (prefix, URI), innermost last.
    scope: Vec<(&'t str, &'t str)>,
    /// `ns_rendered`: (prefix, URI, element) entries.
    rendered: Vec<(&'t str, &'t str, T::Id)>,
    /// `nsPrevStart` and `nsPrevEnd`.
    previous: (usize, usize),
    position: Position,
    parent_is_doc: bool,
}

fn canonicalize<T: Navigate>(
    tree: &T,
    options: &Options<'_, T::Id>,
    emit: impl FnMut(&str),
) -> Result<(), Error> {
    if let Some(problem) = tree.xml_violation() {
        return Err(Error::NotXml(problem));
    }
    let mut c14n = C14n {
        tree,
        options,
        subset: options.nodes.map(|nodes| subset(tree, nodes)),
        emit,
        scope: Vec::new(),
        rendered: Vec::new(),
        previous: (0, 0),
        position: Position::Before,
        parent_is_doc: true,
    };
    let mut frames = Vec::new();
    let mut next = tree.first_child(tree.root());
    loop {
        let Some(id) = next else {
            let Some(frame) = frames.pop() else {
                return Ok(());
            };
            c14n.close(&frame);
            next = tree.next_sibling(frame.id);
            continue;
        };
        match tree.item(id) {
            Item::Element(name) => {
                frames.push(c14n.open(id, name)?);
                next = tree.first_child(id);
            }
            item => {
                c14n.leaf(id, item)?;
                next = tree.next_sibling(id);
            }
        }
    }
}

impl<'t, T: Navigate, F: FnMut(&str)> C14n<'t, '_, T, F> {
    fn escaped(&mut self, text: &str, escape: Escape) {
        let mut start = 0;
        for (index, byte) in text.bytes().enumerate() {
            if let Some(replacement) = replacement(byte, escape) {
                (self.emit)(&text[start..index]);
                (self.emit)(replacement);
                start = index + 1;
            }
        }
        (self.emit)(&text[start..]);
    }

    fn visible(&self, node: Node<T::Id>) -> bool {
        self.subset
            .as_ref()
            .is_none_or(|subset| subset.nodes.contains(&node))
    }

    fn visible_namespace(&self, owner: T::Id, prefix: &str) -> bool {
        self.subset
            .as_ref()
            .is_none_or(|subset| subset.namespaces.contains(&(owner, prefix)))
    }

    /// `xmlSearchNs` from the current element.
    fn search(&self, prefix: &str) -> Option<(&'t str, &'t str)> {
        self.scope
            .iter()
            .rev()
            .find(|binding| binding.0 == prefix)
            .copied()
    }

    /// `xmlC14NVisibleNsStackFind`.
    fn rendered_inclusive(&self, prefix: &str, uri: &str) -> bool {
        let empty = prefix.is_empty() && uri.is_empty();
        let start = if empty { 0 } else { self.previous.0 };
        self.rendered[start..]
            .iter()
            .rev()
            .find(|entry| entry.0 == prefix)
            .map_or(empty, |entry| entry.1 == uri)
    }

    /// `xmlExcC14NVisibleNsStackFind`.
    fn rendered_exclusive(&self, prefix: &str, uri: &str) -> bool {
        match self.rendered.iter().rev().find(|entry| entry.0 == prefix) {
            Some(&(_, found, owner)) => found == uri && self.visible_namespace(owner, prefix),
            None => prefix.is_empty() && uri.is_empty(),
        }
    }

    fn declaration(&mut self, prefix: &str, uri: &str) {
        (self.emit)(if prefix.is_empty() {
            " xmlns"
        } else {
            " xmlns:"
        });
        (self.emit)(prefix);
        (self.emit)("=\"");
        (self.emit)(uri);
        (self.emit)("\"");
    }

    fn declarations(&mut self, mut list: Vec<(&str, &str)>) {
        list.sort_by_key(|&(prefix, _)| prefix);
        (list.into_iter()).for_each(|(prefix, uri)| self.declaration(prefix, uri));
    }

    /// `xmlC14NProcessElementNode` up to the children.
    fn open(&mut self, id: T::Id, name: QName<'t>) -> Result<Frame<T::Id>, Error> {
        let mut frame = Frame {
            id,
            visible: self.visible(Node::Tree(id)),
            rendered: self.rendered.len(),
            previous: self.previous,
            scope: self.scope.len(),
            document_child: false,
        };
        for (_, attribute, uri) in tree::attributes(self.tree, id) {
            let Some(prefix) = attribute.declared_prefix() else {
                continue;
            };
            if !uri.is_empty() {
                let parsed = Uri::parse(uri).map_err(|_| Error::InvalidUri(uri.into()))?;
                if parsed.scheme.is_none() {
                    return Err(Error::RelativeNamespace(uri.into()));
                }
            }
            self.scope.push((prefix, uri));
        }
        if frame.visible {
            if self.parent_is_doc {
                frame.document_child = true;
                self.parent_is_doc = false;
                self.position = Position::Inside;
            }
            (self.emit)("<");
            (self.emit)(name.qualified);
        }
        match self.options.mode {
            Mode::Exclusive10 => self.exclusive_namespaces(id, name, frame.visible),
            Mode::C14n10 | Mode::C14n11 => self.namespaces(id, frame.visible),
        }
        if frame.visible {
            self.previous = (self.previous.1, self.rendered.len());
        }
        self.attributes(id, frame.visible)?;
        if frame.visible {
            (self.emit)(">");
        }
        Ok(frame)
    }

    fn close(&mut self, frame: &Frame<T::Id>) {
        if frame.visible {
            if let Item::Element(name) = self.tree.item(frame.id) {
                (self.emit)("</");
                (self.emit)(name.qualified);
                (self.emit)(">");
            }
            if frame.document_child {
                self.parent_is_doc = true;
                self.position = Position::After;
            }
        }
        self.rendered.truncate(frame.rendered);
        self.previous = frame.previous;
        self.scope.truncate(frame.scope);
    }

    /// `xmlC14NProcessNamespacesAxis`.
    fn namespaces(&mut self, id: T::Id, visible: bool) {
        let mut seen = HashSet::new();
        let bindings: Vec<_> = (self.scope.iter().rev().copied())
            .filter(|&(prefix, _)| seen.insert(prefix))
            .collect();
        let mut list = Vec::new();
        let mut has_empty = false;
        for (prefix, uri) in bindings {
            if prefix == "xml" || !self.visible_namespace(id, prefix) {
                continue;
            }
            if !self.rendered_inclusive(prefix, uri) {
                list.push((prefix, uri));
            }
            if visible {
                self.rendered.push((prefix, uri, id));
            }
            has_empty |= prefix.is_empty();
        }
        if visible && !has_empty && !self.rendered_inclusive("", "") {
            self.declaration("", "");
        }
        self.declarations(list);
    }

    /// `xmlExcC14NProcessNamespacesAxis`.
    fn exclusive_namespaces(&mut self, id: T::Id, name: QName<'t>, visible: bool) {
        let mut list = Vec::new();
        let (mut has_empty, mut empty_listed) = (false, false);
        for &listed in self.options.inclusive_prefixes {
            let listed = if listed == "#default" { "" } else { listed };
            empty_listed |= listed.is_empty();
            let Some((prefix, uri)) = self.search(listed) else {
                continue;
            };
            if prefix == "xml" || !self.visible_namespace(id, prefix) {
                continue;
            }
            if !self.rendered_inclusive(prefix, uri) {
                list.push((prefix, uri));
            }
            if visible {
                self.rendered.push((prefix, uri, id));
            }
            has_empty |= prefix.is_empty();
        }
        let element = match name.namespace {
            Some(uri) => Some((prefix(name.qualified), uri)),
            None => self.search(""),
        };
        if let Some((prefix, uri)) = element.filter(|&(prefix, _)| prefix != "xml") {
            if visible
                && self.visible_namespace(id, prefix)
                && !self.rendered_exclusive(prefix, uri)
            {
                list.push((prefix, uri));
            }
            if visible {
                self.rendered.push((prefix, uri, id));
            }
            has_empty |= prefix.is_empty();
        }
        for (index, attribute, _) in tree::attributes(self.tree, id) {
            let Some(uri) = attribute.namespace else {
                continue;
            };
            let prefix = prefix(attribute.qualified);
            if attribute.is_namespace_declaration()
                || prefix == "xml"
                || !self.visible(Node::Attribute(id, index))
            {
                continue;
            }
            if !self.rendered_exclusive(prefix, uri) && visible {
                list.push((prefix, uri));
            }
            self.rendered.push((prefix, uri, id));
        }
        // libxml2's other xmlns="" case, for an unqualified element, needs a
        // rendered default namespace that is not in scope: it cannot happen.
        if visible && !has_empty && empty_listed && !self.rendered_inclusive("", "") {
            self.declaration("", "");
        }
        self.declarations(list);
    }

    /// `xmlC14NFindHiddenParentAttr`: `xml:{local}` on the nearest invisible ancestors.
    fn hidden_attribute(
        &self,
        mut node: Option<T::Id>,
        local: &str,
    ) -> Option<(T::Id, QName<'t>, &'t str)> {
        while let Some(id) = node.filter(|&id| !self.visible(Node::Tree(id))) {
            if let Some((name, value)) = xml_attribute(self.tree, id, local) {
                return Some((id, name, value));
            }
            node = self.tree.parent(id);
        }
        None
    }

    /// `xmlC14NFixupBaseAttr`: resolves `value`, owned by `owner`, against the
    /// `xml:base` of invisible ancestors. An empty result drops the attribute.
    fn fixup_base(&self, owner: T::Id, value: &str) -> Result<Option<String>, Error> {
        let mut base = value.to_owned();
        let mut node = self.tree.parent(owner);
        while let Some(id) = node.filter(|&id| !self.visible(Node::Tree(id))) {
            if let Some((_, outer)) = xml_attribute(self.tree, id, "base") {
                let mut outer = Cow::Borrowed(outer);
                if outer.len() > 1 && outer.as_bytes()[outer.len() - 2] == b'.' {
                    outer.to_mut().push('/');
                }
                base = uri::resolve(&base, &outer).map_err(|_| Error::InvalidUri(base))?;
            }
            node = self.tree.parent(id);
        }
        Ok((!base.is_empty()).then_some(base))
    }

    /// `xmlC14NProcessAttrsAxis`.
    fn attributes(&mut self, id: T::Id, visible: bool) -> Result<(), Error> {
        const INHERITED: [&str; 3] = ["lang", "space", "base"];
        let tree = self.tree;
        let mode = self.options.mode;
        let is_xml = |name: &QName| name.namespace == Some(XML_NAMESPACE);
        let mut list: Vec<(QName<'t>, Cow<'t, str>)> = Vec::new();
        let mut own = [None; 3];
        for (index, name, value) in tree::attributes(tree, id) {
            let slot = INHERITED.iter().position(|&local| local == name.local);
            if name.is_namespace_declaration() {
                continue;
            } else if let Some(slot) =
                slot.filter(|_| mode == Mode::C14n11 && visible && is_xml(&name))
            {
                own[slot] = Some((id, name, value));
            } else if self.visible(Node::Attribute(id, index)) {
                list.push((name, Cow::Borrowed(value)));
            }
        }
        let parent = tree.parent(id);
        match mode {
            Mode::C14n10 if visible && parent.is_some_and(|id| !self.visible(Node::Tree(id))) => {
                for ancestor in successors(parent, |&id| tree.parent(id)) {
                    for (_, name, value) in tree::attributes(tree, ancestor) {
                        let same = |(other, _): &(QName, _)| {
                            other.namespace == name.namespace && other.local == name.local
                        };
                        if is_xml(&name) && !list.iter().any(same) {
                            list.push((name, Cow::Borrowed(value)));
                        }
                    }
                }
            }
            Mode::C14n11 if visible => {
                for (local, own) in INHERITED.into_iter().zip(own) {
                    let Some((owner, name, value)) =
                        own.or_else(|| self.hidden_attribute(parent, local))
                    else {
                        continue;
                    };
                    let value = match local {
                        "base" => match self.fixup_base(owner, value)? {
                            Some(base) => Cow::Owned(base),
                            None => continue,
                        },
                        _ => Cow::Borrowed(value),
                    };
                    list.push((name, value));
                }
            }
            _ => {}
        }
        list.sort_by(|(left, _), (right, _)| {
            (left.namespace.unwrap_or(""), left.local)
                .cmp(&(right.namespace.unwrap_or(""), right.local))
        });
        for (name, value) in list {
            (self.emit)(" ");
            (self.emit)(name.qualified);
            (self.emit)("=\"");
            self.escaped(&value, Escape::Attribute);
            (self.emit)("\"");
        }
        Ok(())
    }

    /// Text, comment and PI nodes; top-level markup gets libxml2's newlines.
    fn leaf(&mut self, id: T::Id, item: Item<'t>) -> Result<(), Error> {
        if let Item::EntityRef(name) = item {
            return Err(Error::EntityReference(name.to_owned()));
        }
        if !self.visible(Node::Tree(id)) {
            return Ok(());
        }
        match item {
            Item::Text(text) => self.escaped(text, Escape::Text),
            Item::Pi(target, data) => {
                let separator = if data.is_empty() { "" } else { " " };
                self.markup(&["<?", target, separator], data, "?>")
            }
            Item::Comment(text) if self.options.with_comments => {
                self.markup(&["<!--"], text, "-->")
            }
            _ => {}
        }
        Ok(())
    }

    fn markup(&mut self, head: &[&str], body: &str, tail: &str) {
        if self.position == Position::After {
            (self.emit)("\n");
        }
        head.iter().for_each(|part| (self.emit)(part));
        self.escaped(body, Escape::Other);
        (self.emit)(tail);
        if self.position == Position::Before {
            (self.emit)("\n");
        }
    }
}
