//! libxml2's debugging dumps (`debugXML.h`): `xmlDebugDumpDocument` and
//! the other `xmlDebugDump*` functions, which `xmllint --debug` prints, and
//! `xmlDebugCheckDocument`, and with feature `xpath` `xmlXPathDebugDumpObject`
//! ([`dump_xpath_object`]). Every function returns the text libxml2 writes to
//! its `FILE *`.
//!
//! The tree is an [`OwnedDocument`]'s; `xmllint --debug` output matches for
//! 1,991 of the 1,999 XMLTS documents both parse. Differences from libxml2
//! (docs/compatibility.md#debugging-dumps):
//!
//! - text nodes never carry libxml2's storage notes (` compact`, ` interned`);
//! - an entity declaration's children are the content of a kept reference to
//!   it (or of one in an attribute value), so under `NOENT` it has none, and
//!   attribute defaults always show their references (`"a&a;"`);
//! - entity values hold normalized line ends (libxml2 keeps the CR of CRLF);
//! - a PI without data never shows `content=` (libxml2 does after `<?pi ?>`);
//! - the internal subset is read again as [`save`](crate::save) reads it;
//! - [`dump_entities`] lists entities in declaration order (libxml2: hash
//!   order) without `orig`;
//! - [`check_document`] reports detached roots and names outside the Name
//!   production; libxml2's link, dictionary and namespace-scope checks test
//!   invariants the tree keeps by construction.
//!
//! ```
//! use yaxml::{OwnedDocument, debug};
//! let document = OwnedDocument::parse("<r a='1'>t<!--c--></r>")?;
//! assert_eq!(
//!     debug::dump_document(&document),
//!     "DOCUMENT\nversion=1.0\nstandalone=true\n  ELEMENT r\n    ATTRIBUTE a\n      \
//!      TEXT\n        content=1\n    TEXT\n      content=t\n    COMMENT\n      content=c\n"
//! );
//! assert_eq!(debug::check_document(&document), (0, String::new()));
//! # Ok::<(), yaxml::Error>(())
//! ```

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

#[cfg(feature = "xpath")]
use crate::OwnedNodeId;
use crate::allocator::Allocator;
use crate::dtd::{AttributeKind, DtdUnitView, ElementContentView};
use crate::save::dtd::{content_model, stored_default, units};
use crate::valid::{Entity, EntityKind};
use crate::xpath::tree::{QName, XML_NAMESPACE};
#[cfg(feature = "xpath")]
use crate::xpath::{Node, Value, printf_g, tree::namespaces};
use crate::{OwnedDocument, OwnedNode, OwnedNodeKind};

const NO_PARENT: u32 = 5013;
const NOT_NCNAME: u32 = 5034;

/// `xmlDebugDumpString`: the first 40 bytes, blanks as spaces and bytes from
/// 0x80 as `#XX`, then `...` when there are 40 or more.
pub fn dump_string(text: &str) -> String {
    let mut out = String::new();
    for &byte in text.as_bytes().iter().take(40) {
        match byte {
            b' ' | b'\t' | b'\n' | b'\r' => out.push(' '),
            0x80.. => _ = write!(out, "#{byte:X}"),
            _ => out.push(char::from(byte)),
        }
    }
    if text.len() >= 40 {
        out.push_str("...");
    }
    out
}

/// `xmlDebugDumpDocument`: the document's head, then its tree.
pub fn dump_document<A: Allocator>(document: &OwnedDocument<A>) -> String {
    Dump::new(document, 0, false).document().out
}

/// `xmlDebugDumpDocumentHead`: the document node and its properties.
pub fn dump_document_head<A: Allocator>(document: &OwnedDocument<A>) -> String {
    let mut dump = Dump::new(document, 0, false);
    dump.head();
    dump.out
}

/// `xmlDebugDumpDTD` of the internal subset (`doc->intSubset`).
pub fn dump_dtd<A: Allocator>(document: &OwnedDocument<A>) -> String {
    let doctype = document
        .root()
        .children()
        .find(|node| node.kind() == OwnedNodeKind::DocumentType);
    let Some(doctype) = doctype else {
        return "DTD is NULL\n".into();
    };
    let mut dump = Dump::new(document, 0, false);
    dump.one_node(doctype);
    let before = dump.out.len();
    dump.depth = 1;
    dump.declarations(doctype);
    if dump.out.len() == before {
        dump.out.push_str("    DTD is empty\n");
    }
    dump.out
}

/// `xmlDebugDumpEntities`: the general entities of each subset.
pub fn dump_entities<A: Allocator>(document: &OwnedDocument<A>) -> String {
    let mut out = String::from(if document.is_html() {
        "HTML DOCUMENT\n"
    } else {
        "DOCUMENT\n"
    });
    let entities: Vec<_> = document
        .dtd()
        .into_iter()
        .flat_map(|dtd| dtd.entities())
        .filter(|entity| !parameter(entity.kind))
        .collect();
    for (external, name) in [(false, "internal"), (true, "external")] {
        let mut subset = entities
            .iter()
            .filter(|entity| entity.external_subset == external)
            .peekable();
        if subset.peek().is_none() {
            _ = writeln!(out, "No entities in {name} subset");
            continue;
        }
        _ = writeln!(out, "Entities in {name} subset");
        for entity in subset {
            _ = write!(out, "{} : ", entity.name);
            out.push_str(match entity.kind {
                EntityKind::InternalGeneral => "INTERNAL GENERAL, ",
                EntityKind::ExternalParsedGeneral => "EXTERNAL PARSED, ",
                _ => "EXTERNAL UNPARSED, ",
            });
            if let Some(public) = entity.public_id {
                _ = write!(out, "ID \"{public}\"");
            }
            if let Some(system) = entity.system_id {
                _ = write!(out, "SYSTEM \"{system}\"");
            }
            if let Some(content) = content(entity) {
                _ = write!(out, "\n content \"{content}\"");
            }
            out.push('\n');
        }
    }
    out
}

/// `xmlDebugDumpNode`: `node` and its subtree, indented `depth` levels.
pub fn dump_node<A: Allocator>(node: OwnedNode<'_, A>, depth: usize) -> String {
    let mut dump = Dump::new(node.document(), depth, false);
    dump.node(node);
    dump.out
}

/// `xmlDebugDumpOneNode`: `node` with its attributes, not its children.
pub fn dump_one_node<A: Allocator>(node: OwnedNode<'_, A>, depth: usize) -> String {
    let mut dump = Dump::new(node.document(), depth, false);
    dump.one_node(node);
    dump.out
}

/// `xmlDebugDumpNodeList`: `node` and its following siblings, recursively.
pub fn dump_node_list<A: Allocator>(node: OwnedNode<'_, A>, depth: usize) -> String {
    let mut dump = Dump::new(node.document(), depth, false);
    std::iter::successors(Some(node), |node| node.next_sibling()).for_each(|n| dump.node(n));
    dump.out
}

/// `xmlDebugDumpAttr`: the attribute at source `position` of `element`
/// (namespace declarations count, as in [`OwnedNode::attributes`]).
pub fn dump_attribute<A: Allocator>(
    element: OwnedNode<'_, A>,
    position: usize,
    depth: usize,
) -> String {
    let mut dump = Dump::new(element.document(), depth, false);
    match element.attributes().nth(position) {
        Some(_) => dump.attribute(element, position),
        None => dump.out = "  ".repeat(depth.min(50)) + "Attr is NULL",
    }
    dump.out
}

/// `xmlDebugDumpAttrList`: every attribute of `element` (`node->properties`,
/// not its namespace declarations).
pub fn dump_attributes<A: Allocator>(element: OwnedNode<'_, A>, depth: usize) -> String {
    let mut dump = Dump::new(element.document(), depth, false);
    dump.attributes(element);
    dump.out
}

/// `xmlXPathDebugDumpObject`: an XPath result over `document`; each node of
/// a node-set follows its 1-based position, dumped one level deeper.
#[cfg(feature = "xpath")]
pub fn dump_xpath_object<A: Allocator>(
    document: &OwnedDocument<A>,
    value: &Value<OwnedNodeId>,
    depth: usize,
) -> String {
    let shift = "  ".repeat(depth.min(25));
    let nodes = match value {
        Value::Boolean(b) => return format!("{shift}Object is a Boolean : {b}\n"),
        Value::String(s) => return format!("{shift}Object is a string : {}\n", dump_string(s)),
        Value::Number(n) => {
            let number = match *n {
                n if n.is_nan() => "NaN".into(),
                f64::INFINITY => "Infinity".into(),
                f64::NEG_INFINITY => "-Infinity".into(),
                0.0 => "0".into(), // also -0.0
                n => printf_g(n),
            };
            return format!("{shift}Object is a number : {number}\n");
        }
        Value::NodeSet(nodes) => nodes,
    };
    let mut dump = Dump::new(document, depth + 1, false);
    dump.out = format!(
        "{shift}Object is a Node Set :\nSet contains {} nodes:\n",
        nodes.len()
    );
    for (position, &node) in nodes.iter().enumerate() {
        _ = write!(dump.out, "{shift}{}", position + 1);
        let (Node::Tree(id) | Node::Attribute(id, _) | Node::Namespace(id, _)) = node;
        let tree = document.node(id);
        match node {
            Node::Tree(_) if tree.is_some_and(|n| n.kind() == OwnedNodeKind::Document) => {
                dump.spaces();
                dump.put(" /\n");
            }
            Node::Tree(_) => tree.into_iter().for_each(|n| dump.one_node(n)),
            Node::Attribute(_, index) => tree.into_iter().for_each(|n| dump.attribute(n, index)),
            Node::Namespace(_, index) => {
                let Some(&(prefix, uri)) = namespaces(document, id).get(index) else {
                    continue;
                };
                dump.spaces();
                match prefix {
                    "" => dump.put("default namespace href="),
                    prefix => dump.put(&format!("namespace {prefix} href=")),
                }
                dump.string(uri);
                dump.put("\n");
            }
        }
    }
    dump.out
}

/// `xmlDebugCheckDocument`: the number of problems found and their reports.
pub fn check_document<A: Allocator>(document: &OwnedDocument<A>) -> (usize, String) {
    let dump = Dump::new(document, 0, true).document();
    (dump.errors, dump.out)
}

fn parameter(kind: EntityKind) -> bool {
    matches!(
        kind,
        EntityKind::InternalParameter | EntityKind::ExternalParameter
    )
}

/// The `content` libxml2 keeps: none for external parsed entities.
fn content<'a>(entity: &Entity<'a>) -> Option<&'a str> {
    let external = matches!(
        entity.kind,
        EntityKind::ExternalParsedGeneral | EntityKind::ExternalParameter
    );
    entity.content.filter(|_| !external)
}

/// An attribute value's children (`xmlNodeParseAttValue`): text runs with
/// character and predefined references resolved, and the other references.
fn pieces(value: &str) -> Vec<(bool, String)> {
    let mut pieces: Vec<(bool, String)> = Vec::new();
    let mut push = |reference: bool, text: &str| match pieces.last_mut() {
        Some((false, run)) if !reference => run.push_str(text),
        _ => pieces.push((reference, text.into())),
    };
    let mut rest = value;
    while let Some(at) = rest.find('&') {
        push(false, &rest[..at]);
        let Some(end) = rest[at..].find(';').map(|end| at + end) else {
            break;
        };
        let name = &rest[at + 1..end];
        let number = match name.strip_prefix("#x") {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => name
                .strip_prefix('#')
                .and_then(|decimal| decimal.parse().ok()),
        };
        match (
            number.and_then(char::from_u32),
            crate::valid::predefined_entity(name),
        ) {
            (Some(c), _) => push(false, c.encode_utf8(&mut [0; 4])),
            (None, Some(entity)) => push(false, entity.content.unwrap_or_default()),
            (None, None) => push(true, name),
        }
        rest = &rest[end + 1..];
    }
    push(false, rest);
    pieces.retain(|(reference, text)| *reference || !text.is_empty());
    pieces
}

/// Where an entity declaration's children come from.
enum Children<'d, A: Allocator> {
    /// A kept reference whose children are the content.
    Tree(OwnedNode<'d, A>),
    /// Referenced only from attribute values: the replacement text's pieces.
    Value(&'d str),
}

// Manual: a derive would require `A: Copy`.
impl<A: Allocator> Clone for Children<'_, A> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A: Allocator> Copy for Children<'_, A> {}

struct Dump<'d, A: Allocator> {
    document: &'d OwnedDocument<A>,
    out: String,
    depth: usize,
    /// `xmlDebugCheckDocument`: only error reports are written.
    check: bool,
    errors: usize,
}

impl<'d, A: Allocator> Dump<'d, A> {
    fn new(document: &'d OwnedDocument<A>, depth: usize, check: bool) -> Self {
        Self {
            document,
            out: String::new(),
            depth,
            check,
            errors: 0,
        }
    }

    fn spaces(&mut self) {
        if !self.check {
            (0..2 * self.depth.min(50)).for_each(|_| self.out.push(' '));
        }
    }

    /// Text a dump writes and a check does not.
    fn put(&mut self, text: &str) {
        if !self.check {
            self.out.push_str(text);
        }
    }

    fn string(&mut self, text: &str) {
        if !self.check {
            self.out.push_str(&dump_string(text));
        }
    }

    /// An indented `label=` line with the dumped string.
    fn field(&mut self, label: &str, text: &str) {
        self.spaces();
        self.put(label);
        self.string(text);
        self.put("\n");
    }

    fn error(&mut self, code: u32, message: &str) {
        self.errors += 1;
        _ = write!(self.out, "ERROR {code}: {message}");
    }

    fn document(mut self) -> Self {
        self.head();
        self.depth += 1;
        self.document
            .root()
            .children()
            .for_each(|child| self.node(child));
        self.depth -= 1;
        self
    }

    fn head(&mut self) {
        let document = self.document;
        let html = document.is_html();
        self.put(if html {
            "HTML DOCUMENT\n"
        } else {
            "DOCUMENT\n"
        });
        let declaration = document.declaration();
        let version = declaration.map_or("1.0", |declaration| declaration.version);
        let encoding = match html {
            true => document.html_encoding(),
            false => declaration.and_then(|declaration| declaration.encoding),
        };
        let fields = [
            ("version=", (!html).then_some(version)),
            ("encoding=", encoding),
            ("URL=", document.url()),
        ];
        for (label, value) in fields {
            if let Some(value) = value {
                self.field(label, value);
            }
        }
        if html || declaration.and_then(|declaration| declaration.standalone) != Some(false) {
            self.put("standalone=true\n");
        }
        // `doc->oldNs`, created by the first name in the xml namespace.
        let xml = document.nodes().any(|node| {
            let element = node
                .name()
                .is_some_and(|name| name.namespace() == Some(XML_NAMESPACE));
            element
                || node
                    .attributes()
                    .any(|attribute| attribute.name.namespace() == Some(XML_NAMESPACE))
        });
        if xml {
            self.spaces();
            self.put("namespace xml href=");
            self.string(XML_NAMESPACE);
            self.put("\n");
        }
    }

    fn node(&mut self, node: OwnedNode<'d, A>) {
        self.one_node(node);
        self.depth += 1;
        match node.kind() {
            OwnedNodeKind::EntityRef => {}
            OwnedNodeKind::DocumentType => self.declarations(node),
            _ => node.children().for_each(|child| self.node(child)),
        }
        self.depth -= 1;
    }

    fn one_node(&mut self, node: OwnedNode<'d, A>) {
        self.spaces();
        let kind = node.kind();
        let name = node.name().filter(|_| kind == OwnedNodeKind::Element);
        let name = name.as_ref().map(QName::of);
        let target = node.processing_instruction_target();
        self.depth += 1;
        match kind {
            OwnedNodeKind::Document => self.put("Error, DOCUMENT found here\n"),
            OwnedNodeKind::DocumentType => {
                if let Some(doctype) = node.doctype() {
                    self.put(&format!("DTD({})", doctype.name));
                    if let Some(public) = doctype.public_id {
                        self.put(&format!(", PUBLIC {public}"));
                    }
                    if let Some(system) = doctype.system_id {
                        self.put(&format!(", SYSTEM {system}"));
                    }
                }
                self.put("\n");
            }
            OwnedNodeKind::Element => {
                self.put("ELEMENT ");
                if let Some(name) = name {
                    if let Some(prefix) = prefix(name) {
                        self.string(prefix);
                        self.put(":");
                    }
                    self.string(name.local);
                }
                self.put("\n");
                for attribute in node.attributes() {
                    let Some(prefix) = QName::of(&attribute.name)
                        .declared_prefix()
                        .filter(|p| *p != "xml")
                    else {
                        continue;
                    };
                    self.spaces();
                    match prefix {
                        "" => self.put("default namespace href="),
                        prefix => self.put(&format!("namespace {prefix} href=")),
                    }
                    self.string(&attribute.value);
                    self.put("\n");
                }
                self.attributes(node);
            }
            OwnedNodeKind::EntityRef => {
                let entity = node.entity_name().unwrap_or_default();
                self.put(&format!("ENTITY_REF({entity})\n"));
                if let Some(entity) = self.document.dtd().and_then(|dtd| dtd.entity(entity)) {
                    self.entity(&entity);
                }
            }
            _ => {
                self.put(
                    match (kind, target) {
                        (OwnedNodeKind::Text, _) => "TEXT\n".into(),
                        (OwnedNodeKind::Cdata, _) => "CDATA_SECTION\n".into(),
                        (OwnedNodeKind::Comment, _) => "COMMENT\n".into(),
                        (_, target) => format!("PI {}\n", target.unwrap_or_default()),
                    }
                    .as_str(),
                );
                if let Some(content) = node
                    .text()
                    .filter(|text| target.is_none() || !text.is_empty())
                {
                    self.field("content=", content);
                }
            }
        }
        self.depth -= 1;
        self.check(node, name.map(|name| name.local).or(target));
    }

    fn attributes(&mut self, element: OwnedNode<'d, A>) {
        for (position, attribute) in element.attributes().enumerate() {
            if !QName::of(&attribute.name).is_namespace_declaration() {
                self.attribute(element, position);
            }
        }
    }

    fn attribute(&mut self, element: OwnedNode<'d, A>, position: usize) {
        let Some(attribute) = element.attributes().nth(position) else {
            return;
        };
        let name = QName::of(&attribute.name);
        self.spaces();
        self.put("ATTRIBUTE ");
        self.string(name.local);
        self.put("\n");
        if !self.check {
            self.depth += 1;
            match element.unexpanded_attribute(position) {
                Some(form) => self.value(form),
                None => self.text(&attribute.value),
            }
            self.depth -= 1;
        }
        self.name(name.local);
    }

    /// Children parsed from a value: text runs and entity references.
    fn value(&mut self, value: &str) {
        for (reference, text) in pieces(value) {
            match reference {
                true => {
                    self.spaces();
                    self.put(&format!("ENTITY_REF({text})\n"));
                    let entity = self.document.dtd().and_then(|dtd| dtd.entity(&text));
                    if let Some(entity) = entity {
                        self.depth += 1;
                        self.entity(&entity);
                        self.depth -= 1;
                    }
                }
                false => self.text(&text),
            }
        }
    }

    fn text(&mut self, text: &str) {
        self.spaces();
        self.put("TEXT\n");
        self.depth += 1;
        self.field("content=", text);
        self.depth -= 1;
    }

    /// `xmlCtxtDumpEntity`: the entity a reference names.
    fn entity(&mut self, entity: &Entity<'_>) {
        self.spaces();
        let kind = match entity.kind {
            EntityKind::InternalGeneral => "INTERNAL_GENERAL_ENTITY",
            EntityKind::ExternalParsedGeneral => "EXTERNAL_GENERAL_PARSED_ENTITY",
            EntityKind::ExternalUnparsedGeneral => "EXTERNAL_GENERAL_UNPARSED_ENTITY",
            EntityKind::InternalParameter => "INTERNAL_PARAMETER_ENTITY",
            EntityKind::ExternalParameter => "EXTERNAL_PARAMETER_ENTITY",
            EntityKind::Predefined => "ENTITY_6 !",
        };
        self.put(&format!("{kind} {}\n", entity.name));
        self.identifiers(entity, "");
        if let Some(content) = content(entity) {
            self.field("content=", content);
        }
    }

    /// The `ExternalID=`, `SystemID=` and `URI=` lines, after `lead`.
    fn identifiers(&mut self, entity: &Entity<'_>, lead: &str) {
        // `xmlBuildURI(SystemID, base)`: a resolver's URI, else the system
        // identifier against the document URL; none when it is no URI.
        let uri = entity
            .system_id
            .and_then(|system| match (entity.uri, self.document.url()) {
                (Some(uri), _) if uri != system => Some(uri.to_owned()),
                (_, Some(base)) => crate::uri::resolve(system, base).ok(),
                _ => crate::uri::Uri::parse(system)
                    .ok()
                    .map(|_| system.to_owned()),
            });
        let fields = [
            ("ExternalID=", entity.public_id),
            ("SystemID=", entity.system_id),
            ("URI=", uri.as_deref()),
        ];
        for (label, value) in fields {
            if let Some(value) = value {
                self.spaces();
                self.put(&format!("{lead}{label}{value}\n"));
            }
        }
    }

    /// The internal subset's declarations, comments and PIs, as libxml2
    /// links them under `intSubset`.
    fn declarations(&mut self, doctype: OwnedNode<'d, A>) {
        let Some(declared) = doctype.doctype() else {
            return;
        };
        let Some(subset) = declared.internal_subset else {
            return;
        };
        let children = self.entity_children();
        let mut seen = HashSet::new();
        let walked = units(declared.name, subset, None, |view, catalog| {
            let mut first = |kind: &str, name: &str| seen.insert(format!("{kind} {name}"));
            match view {
                DtdUnitView::Element(element) if first("element", element.name()) => {
                    self.spaces();
                    self.put("ELEMDECL(");
                    self.string(local(element.name()));
                    self.put(")");
                    let model = match element.content() {
                        ElementContentView::Empty => ", EMPTY".into(),
                        ElementContentView::Any => ", ANY".into(),
                        ElementContentView::Mixed(tokens)
                        | ElementContentView::Children(tokens) => {
                            format!(", MIXED {}", content_model(tokens))
                        }
                    };
                    self.put(&model);
                    self.put("\n");
                }
                DtdUnitView::AttributeList(list) => {
                    let element = list.element_name();
                    for definition in list.definitions() {
                        if !first("attribute", &format!("{element} {}", definition.name())) {
                            continue;
                        }
                        let mut line =
                            format!("ATTRDECL({}) for {element}", local(definition.name()));
                        line.push_str(match definition.kind() {
                            AttributeKind::Cdata => " CDATA",
                            AttributeKind::Id => " ID",
                            AttributeKind::IdRef => " IDREF",
                            AttributeKind::IdRefs => " IDREFS",
                            AttributeKind::Entity => " ENTITY",
                            AttributeKind::Entities => " ENTITIES",
                            AttributeKind::NmToken => " NMTOKEN",
                            AttributeKind::NmTokens => " NMTOKENS",
                            AttributeKind::Enumeration => " ENUMERATION",
                            AttributeKind::Notation => " NOTATION ",
                        });
                        let values: Vec<_> = definition.values().collect();
                        if !values.is_empty() {
                            let shown = values[..values.len().min(5)].join("|");
                            let more = if values.len() > 5 { "..." } else { "" };
                            _ = write!(line, " ({shown}{more})");
                        }
                        line.push_str(match definition.default() {
                            crate::dtd::DefaultDeclarationView::Required => " REQUIRED",
                            crate::dtd::DefaultDeclarationView::Implied => " IMPLIED",
                            crate::dtd::DefaultDeclarationView::Value { fixed: true, .. } => {
                                " FIXED"
                            }
                            crate::dtd::DefaultDeclarationView::Value { .. } => "",
                        });
                        if let Some(value) = stored_default(catalog, element, &definition, true) {
                            _ = write!(line, "\"{}\"", dump_string(value));
                        }
                        self.spaces();
                        self.put(&line);
                        self.put("\n");
                    }
                }
                DtdUnitView::Entity(declaration) => {
                    let (name, parameter) = (declaration.name(), declaration.is_parameter());
                    if !first(if parameter { "%" } else { "&" }, name) {
                        return;
                    }
                    let dtd = self.document.dtd();
                    let entity = dtd.and_then(|dtd| match parameter {
                        true => dtd.parameter_entity(name),
                        false => dtd.entity(name),
                    });
                    let Some(entity) = entity else {
                        return;
                    };
                    self.entity_declaration(&entity, children.get(name).copied());
                }
                DtdUnitView::Comment(text) => {
                    self.spaces();
                    self.put("COMMENT\n");
                    self.depth += 1;
                    self.field("content=", text);
                    self.depth -= 1;
                }
                DtdUnitView::ProcessingInstruction(instruction) => {
                    self.spaces();
                    self.put(&format!("PI {}\n", instruction.target));
                    self.depth += 1;
                    if let Some(data) = instruction.data {
                        self.field("content=", data);
                    }
                    self.depth -= 1;
                }
                _ => {}
            }
        });
        // The document parsed this subset, so reading it again succeeds.
        debug_assert!(walked.is_ok());
    }

    /// `xmlCtxtDumpEntityDecl`, then the parsed content of a referenced entity.
    fn entity_declaration(&mut self, entity: &Entity<'_>, children: Option<Children<'d, A>>) {
        self.spaces();
        self.put("ENTITYDECL(");
        self.string(entity.name);
        self.put(match entity.kind {
            EntityKind::InternalGeneral => "), internal\n",
            EntityKind::ExternalParsedGeneral => "), external parsed\n",
            EntityKind::ExternalUnparsedGeneral => "), unparsed\n",
            EntityKind::InternalParameter => "), parameter\n",
            EntityKind::ExternalParameter => "), external parameter\n",
            EntityKind::Predefined => "), predefined\n",
        });
        self.identifiers(entity, " ");
        if let Some(content) = content(entity) {
            self.field(" content=", content);
        }
        self.depth += 1;
        match children {
            Some(Children::Tree(reference)) => reference.children().for_each(|n| self.node(n)),
            Some(Children::Value(value)) => self.value(value),
            None => {}
        }
        self.depth -= 1;
    }

    /// Each referenced general entity's children: a kept reference node, else
    /// its replacement text when an attribute value (or such an entity)
    /// references it.
    fn entity_children(&self) -> HashMap<&'d str, Children<'d, A>> {
        let document = self.document;
        let mut children = HashMap::new();
        let mut values = Vec::new();
        for node in document.nodes() {
            if let Some(name) = node.entity_name() {
                children.entry(name).or_insert(Children::Tree(node));
            }
            let forms = (0..node.attributes().len()).filter_map(|i| node.unexpanded_attribute(i));
            values.extend(forms);
        }
        while let Some(value) = values.pop() {
            for (_, name) in pieces(value)
                .into_iter()
                .filter(|(reference, _)| *reference)
            {
                let entity = document.dtd().and_then(|dtd| dtd.entity(&name));
                let Some((entity, text)) = entity.and_then(|e| Some((e.name, e.content?))) else {
                    continue;
                };
                if !children.contains_key(entity) {
                    children.insert(entity, Children::Value(text));
                    values.push(text);
                }
            }
        }
        children
    }

    /// `xmlCtxtGenericNodeCheck` where a tree can differ from libxml2's
    /// invariants: a detached root, and names outside the Name production
    /// (HTML). Namespace scope needs no check: the tree keeps every prefix
    /// bound (libxml2 parses entity content without the reference's scope).
    fn check(&mut self, node: OwnedNode<'d, A>, name: Option<&str>) {
        if node.parent().is_none() {
            self.error(NO_PARENT, "Node has no parent\n");
        }
        if let Some(name) = name {
            self.name(name);
        }
    }

    fn name(&mut self, name: &str) {
        if self.check && crate::name::validate_name(name, 0, true).is_err() {
            self.error(NOT_NCNAME, &format!("Name is not an NCName '{name}'"));
        }
    }
}

/// The prefix of a name in a namespace.
fn prefix(name: QName<'_>) -> Option<&str> {
    let length = name.qualified.len() - name.local.len();
    (name.namespace.is_some() && length > 0).then(|| &name.qualified[..length - 1])
}

/// A declared name without its prefix (`xmlSplitQName2`: libxml2 keeps the
/// prefix of an element or attribute declaration apart; a colon at either end
/// splits nothing).
fn local(name: &str) -> &str {
    match name.split_once(':') {
        Some((prefix, local)) if !prefix.is_empty() && !local.is_empty() => local,
        _ => name,
    }
}
