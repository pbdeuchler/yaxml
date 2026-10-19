//! DTD validation after libxml2's `valid.c`.
//!
//! [`Document::validate`](crate::Document::validate) and
//! [`OwnedDocument::validate`](crate::OwnedDocument::validate) check a parsed
//! tree against its own DTD (`xmlValidateDocument`, `XML_PARSE_DTDVALID`);
//! [`validate`] checks any tree against a separately parsed [`Dtd`]
//! (`xmlValidateDtd`, `xmllint --dtdvalid`). Validity errors are never fatal:
//! each is a [`Diagnostic`] whose [`Code`] carries libxml2's `XML_DTD_*` number.

mod model;

use std::collections::{HashMap, HashSet};

use crate::dtd::{ElementContentView, ElementDeclarationView, ModelTokenView};
use crate::dtd_catalog::{AttributeKind, StoredDefault};
use crate::name::{name_char, name_start};
use crate::xpath::tree::{Item, Navigate, QName, Tree, attributes, preorder};
use crate::{Error, Limits, Resolver};

pub(crate) use model::{Model, TooComplex};

/// A validity diagnostic code; the discriminant is libxml2's `xmlParserErrors` value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u16)]
#[non_exhaustive]
pub enum Code {
    /// An entity reference that unread declarations could have declared.
    UndeclaredEntity = 26,
    /// A parameter entity splits a declaration, group or conditional section.
    EntityBoundary = 90,
    /// An invalid or mismatched default or `#FIXED` value.
    AttributeDefault = 500,
    /// A repeated attribute declaration (a warning).
    AttributeRedefined = 501,
    /// A value outside its type's syntax or enumeration, or not the `#FIXED` value.
    AttributeValue = 502,
    /// A name repeated in a mixed-content declaration.
    ContentError = 503,
    /// Element content does not match its declared model.
    ContentModel = 504,
    /// The content model is not deterministic (XML 1.0 Appendix E).
    ContentNotDeterministic = 505,
    /// A required prefixed attribute appears with another prefix (a warning).
    DifferentPrefix = 506,
    /// A namespace declaration differs from its `#FIXED` value.
    ElementNamespace = 508,
    /// A repeated element type declaration.
    ElementRedefined = 509,
    /// A `NOTATION` attribute declared for an `EMPTY` element.
    EmptyNotation = 510,
    /// An `ENTITY` value names a parsed entity.
    EntityType = 511,
    /// An `ID` attribute with a default value.
    IdFixed = 512,
    /// A repeated `ID` value.
    IdRedefined = 513,
    /// More than one `ID` attribute declared for an element type.
    IdSubset = 514,
    /// A child element not listed in a mixed-content declaration.
    InvalidChild = 515,
    /// A namespace declaration outside its declared type's syntax.
    InvalidDefault = 516,
    /// A `#REQUIRED` attribute is missing.
    MissingAttribute = 518,
    /// The document has no DTD.
    NoDtd = 522,
    /// A required prefixed attribute appears unprefixed (a warning).
    NoPrefix = 524,
    /// A repeated notation declaration.
    NotationRedefined = 526,
    /// A `NOTATION` value outside its enumeration.
    NotationValue = 527,
    /// Content in an element declared `EMPTY`.
    NotEmpty = 528,
    /// A child element in an element declared `(#PCDATA)`.
    NotPcdata = 529,
    /// The root element is not the type named by the DOCTYPE.
    RootName = 531,
    /// A standalone document's attribute value normalized by an external declaration.
    NotStandalone = 530,
    /// Whitespace in externally declared element content of a standalone document.
    StandaloneWhiteSpace = 532,
    /// An undeclared attribute.
    UnknownAttribute = 533,
    /// An undeclared element type.
    UnknownElement = 534,
    /// An `ENTITY` value names no declared entity.
    UnknownEntity = 535,
    /// An `IDREF` value names no `ID`.
    UnknownId = 536,
    /// An undeclared notation.
    UnknownNotation = 537,
    /// A standalone document's attribute defaulted from an external declaration.
    StandaloneDefaulted = 538,
    /// A token repeated in an enumerated attribute type.
    DuplicateToken = 541,
}

impl Code {
    /// libxml2 reports these as validity warnings; they leave a document valid.
    pub fn is_warning(self) -> bool {
        matches!(
            self,
            Self::AttributeRedefined | Self::DifferentPrefix | Self::NoPrefix
        )
    }
}

/// One validity error or warning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic<Id> {
    pub code: Code,
    /// The element concerned; `None` for declaration-level diagnostics.
    pub node: Option<Id>,
    /// The start of `node`'s source range, when it has one.
    pub offset: Option<usize>,
    /// A libxml2-style description.
    pub message: String,
}

/// True when no diagnostic is an error.
pub fn is_valid<Id>(diagnostics: &[Diagnostic<Id>]) -> bool {
    diagnostics
        .iter()
        .all(|diagnostic| diagnostic.code.is_warning())
}

/// Element and attribute-list declarations, notations and entity kinds.
///
/// A parsed document keeps the DTD its internal and external subsets
/// declared; [`Dtd::load`] and [`Dtd::parse`] read a standalone external DTD.
#[derive(Clone, Debug, Default)]
pub struct Dtd {
    /// The DOCTYPE name, checked against the root element.
    pub(crate) name: Option<String>,
    pub(crate) standalone: bool,
    pub(crate) element_index: HashMap<String, usize>,
    pub(crate) elements: Vec<ElementDecl>,
    /// Notation name and whether it was declared in the internal subset.
    pub(crate) notations: HashMap<String, bool>,
    /// General entities: the notation of an unparsed entity, `None` if parsed.
    pub(crate) entities: HashMap<String, Option<String>>,
    /// Diagnostics libxml2 reports while parsing, with their input offsets.
    pub(crate) issues: Vec<(Code, Option<usize>, String)>,
}

#[derive(Clone, Debug)]
pub(crate) struct ElementDecl {
    pub(crate) name: String,
    /// `None` until an `<!ELEMENT>` declaration; attribute lists may come first.
    pub(crate) content: Option<Content>,
    pub(crate) internal: bool,
    pub(crate) attributes: Vec<AttributeDecl>,
    pub(crate) index: HashMap<String, usize>,
}

#[derive(Clone, Debug)]
pub(crate) enum Content {
    Empty,
    Any,
    /// Allowed child names; empty for `(#PCDATA)`.
    Mixed(Vec<String>),
    Children(Model),
}

#[derive(Clone, Debug)]
pub(crate) struct AttributeDecl {
    pub(crate) name: String,
    pub(crate) name_offset: usize,
    pub(crate) kind: AttributeKind,
    /// Enumerated or notation tokens.
    pub(crate) values: Vec<String>,
    pub(crate) default: StoredDefault,
    pub(crate) declaration: usize,
    pub(crate) internal: bool,
}

impl AttributeDecl {
    fn fixed(&self) -> Option<&str> {
        match &self.default {
            StoredDefault::Value { fixed: true, value } => Some(value),
            _ => None,
        }
    }
}

impl ElementDecl {
    pub(crate) fn new(name: String) -> Self {
        Self {
            name,
            content: None,
            internal: false,
            attributes: Vec::new(),
            index: HashMap::new(),
        }
    }
}

impl Dtd {
    /// Reads the external DTD `system` through `resolver`, as libxml2's
    /// `xmlParseDTD(NULL, system)`; external parameter entities load the same
    /// way. A declined subset is [`ErrorKind::ExternalResource`](crate::ErrorKind).
    pub fn load(system: &str, limits: Limits, resolver: Box<dyn Resolver>) -> Result<Self, Error> {
        crate::dtd_catalog::external_dtd(system, limits, resolver)
    }

    /// Parses external DTD text; parameter entities it references externally
    /// expand to nothing.
    pub fn parse(text: &[u8]) -> Result<Self, Error> {
        let text = text.to_vec();
        let resolver = move |request: crate::Request<'_>| {
            Ok((request.kind == crate::ResourceKind::DtdSubset).then(|| text.clone()))
        };
        Self::load("", Limits::default(), Box::new(resolver))
    }

    /// `(element, attribute)` qualified names declared with type `ID`.
    pub(crate) fn id_attributes(&self) -> impl Iterator<Item = (&str, &str)> {
        self.elements.iter().flat_map(|element| {
            element
                .attributes
                .iter()
                .filter(|attribute| attribute.kind == AttributeKind::Id)
                .map(move |attribute| (element.name.as_str(), attribute.name.as_str()))
        })
    }

    /// The DOCTYPE name of the document this DTD came from.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    fn element(&self, name: QName<'_>) -> Option<(usize, &ElementDecl)> {
        let index = self.element_index.get(name.qualified).or_else(|| {
            (name.qualified != name.local)
                .then(|| self.element_index.get(name.local))
                .flatten()
        })?;
        Some((*index, &self.elements[*index]))
    }

    /// The declaration of attribute `attribute` on `element`, qualified name first.
    fn attribute(&self, element: QName<'_>, attribute: &str) -> Option<&AttributeDecl> {
        let declared = |name: &str| {
            let element = &self.elements[*self.element_index.get(name)?];
            Some(&element.attributes[*element.index.get(attribute)?])
        };
        declared(element.qualified).or_else(|| {
            (element.qualified != element.local)
                .then(|| declared(element.local))
                .flatten()
        })
    }

    /// Records an `<!ELEMENT>` declaration; returns the bytes retained.
    pub(crate) fn declare_element(
        &mut self,
        index: usize,
        view: ElementDeclarationView<'_>,
        internal: bool,
        budget: &mut usize,
    ) -> Result<usize, TooComplex> {
        let element = &self.elements[index];
        if element.content.is_some() {
            let message = format!("Redefinition of element {}", element.name);
            self.issues.push((Code::ElementRedefined, None, message));
            return Ok(0);
        }
        let (content, bytes) = match view.content() {
            ElementContentView::Empty => (Content::Empty, 0),
            ElementContentView::Any => (Content::Any, 0),
            ElementContentView::Mixed(tokens) => {
                let mut names: Vec<String> = Vec::new();
                for token in tokens {
                    let ModelTokenView::Name(name) = token else {
                        continue;
                    };
                    if names.iter().any(|seen| seen == name) {
                        let message = format!(
                            "Definition of {} has duplicate references of {name}",
                            view.name()
                        );
                        self.issues.push((Code::ContentError, None, message));
                    } else {
                        names.push(name.to_owned());
                    }
                }
                let bytes = names.iter().map(String::len).sum();
                (Content::Mixed(names), bytes)
            }
            ElementContentView::Children(tokens) => {
                let model = Model::compile(tokens, budget)?;
                let bytes = model.bytes();
                (Content::Children(model), bytes)
            }
        };
        let element = &mut self.elements[index];
        element.content = Some(content);
        element.internal = internal;
        Ok(bytes)
    }

    /// Records a notation declaration.
    pub(crate) fn declare_notation(&mut self, name: &str, internal: bool) {
        match self.notations.get(name) {
            Some(&seen) if seen == internal => {
                let message = format!("xmlAddNotationDecl: {name} already defined");
                self.issues.push((Code::NotationRedefined, None, message));
            }
            Some(_) => {}
            None => {
                self.notations.insert(name.to_owned(), internal);
            }
        }
    }

    /// The declaration-time checks of `xmlAddAttributeDecl`,
    /// `xmlValidateAttributeDecl` and the ATTLIST parser for attribute
    /// `attribute` of element `element`, before it is added.
    pub(crate) fn check_attribute(&mut self, element: usize, attribute: &AttributeDecl) {
        let decl = &self.elements[element];
        let (name, owner) = (&attribute.name, &decl.name);
        for (index, value) in attribute.values.iter().enumerate() {
            if attribute.values[..index].contains(value) {
                let message = format!("attribute value token {value} duplicated");
                self.issues.push((Code::DuplicateToken, None, message));
            }
        }
        let default = match &attribute.default {
            StoredDefault::Value { value, .. } => Some(value.as_str()),
            _ => None,
        };
        let lexical = default.is_none_or(|value| lexical(attribute.kind, value));
        if !lexical {
            let message = format!("Attribute {owner} of {name}: invalid default value");
            self.issues.push((Code::AttributeDefault, None, message));
        }
        match decl.index.get(name).map(|&index| &decl.attributes[index]) {
            Some(previous) => {
                if previous.internal == attribute.internal {
                    let message = format!("Attribute {name} of element {owner}: already defined");
                    self.issues.push((Code::AttributeRedefined, None, message));
                }
            }
            None => {
                if attribute.kind == AttributeKind::Id {
                    if default.is_some() {
                        let message = format!(
                            "ID attribute {name} of {owner} is not valid must be #IMPLIED or #REQUIRED"
                        );
                        self.issues.push((Code::IdFixed, None, message));
                    }
                    if decl.attributes.iter().any(|a| a.kind == AttributeKind::Id) {
                        let message = format!("Element {owner} has too many ID attributes: {name}");
                        self.issues.push((Code::IdSubset, None, message));
                    }
                }
                if let Some(value) = default.filter(|_| lexical)
                    && matches!(
                        attribute.kind,
                        AttributeKind::Enumeration | AttributeKind::Notation
                    )
                    && !attribute.values.iter().any(|token| token == value)
                {
                    let message = format!(
                        "Default value \"{value}\" for attribute {name} of {owner} is not among the enumerated set"
                    );
                    self.issues.push((Code::AttributeValue, None, message));
                }
            }
        }
    }
}

fn is_name(value: &str) -> bool {
    let mut characters = value.chars();
    characters.next().is_some_and(|c| name_start(c, true)) && characters.all(|c| name_char(c, true))
}

fn is_nmtoken(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| name_char(c, true))
}

fn blank(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// The lexical check of `xmlValidateAttributeValueInternal`: names are
/// separated by runs of U+0020, and NMTOKENS (and enumerations) may also have
/// leading blanks and trailing spaces.
fn lexical(kind: AttributeKind, value: &str) -> bool {
    match kind {
        AttributeKind::Cdata => true,
        AttributeKind::Id
        | AttributeKind::IdRef
        | AttributeKind::Entity
        | AttributeKind::Notation => is_name(value),
        AttributeKind::IdRefs | AttributeKind::Entities => {
            value.split(' ').filter(|s| !s.is_empty()).all(is_name)
                && !value.starts_with(' ')
                && !value.ends_with(' ')
                && !value.is_empty()
        }
        AttributeKind::NmToken => is_nmtoken(value),
        AttributeKind::NmTokens | AttributeKind::Enumeration => {
            let value = value.trim_start_matches(blank).trim_end_matches(' ');
            !value.is_empty() && value.split(' ').filter(|s| !s.is_empty()).all(is_nmtoken)
        }
    }
}

fn children<T: Navigate>(tree: &T, id: T::Id) -> impl Iterator<Item = T::Id> + '_ {
    std::iter::successors(tree.first_child(id), move |&child| tree.next_sibling(child))
}

const PREDEFINED: [&str; 5] = ["lt", "gt", "amp", "apos", "quot"];

/// Checks `tree` against `dtd` as libxml2's `xmlValidateDtd`: the root,
/// every element and attribute, then IDREFs. Declaration-time checks of
/// `dtd` and the DOCTYPE name are not repeated.
pub fn validate<T: Tree>(tree: &T, dtd: &Dtd) -> Vec<Diagnostic<T::Id>> {
    run(tree, dtd, false)
}

/// `xmlValidateDocument` with the checks libxml2 makes while parsing.
pub(crate) fn validate_document<T: Navigate>(
    tree: &T,
    dtd: Option<&Dtd>,
) -> Vec<Diagnostic<T::Id>> {
    match dtd {
        Some(dtd) => run(tree, dtd, true),
        None => vec![Diagnostic {
            code: Code::NoDtd,
            node: None,
            offset: None,
            message: "no DTD found!".into(),
        }],
    }
}

struct Run<'a, T: Navigate> {
    tree: &'a T,
    dtd: &'a Dtd,
    out: Vec<Diagnostic<T::Id>>,
    ids: HashSet<&'a str>,
    /// `(element, attribute, value, IDREFS)` in document order.
    references: Vec<(T::Id, &'a str, &'a str, bool)>,
    nondeterministic: HashSet<usize>,
}

fn run<T: Navigate>(tree: &T, dtd: &Dtd, document: bool) -> Vec<Diagnostic<T::Id>> {
    let mut run = Run {
        tree,
        dtd,
        out: Vec::new(),
        ids: HashSet::new(),
        references: Vec::new(),
        nondeterministic: HashSet::new(),
    };
    if document {
        for (code, offset, message) in &dtd.issues {
            run.out.push(Diagnostic {
                code: *code,
                node: None,
                offset: *offset,
                message: message.clone(),
            });
        }
        run.declarations();
    }
    let root = std::iter::successors(tree.first_child(tree.root()), |&id| tree.next_sibling(id))
        .find(|&id| matches!(tree.item(id), Item::Element(_)));
    if let (Some(root), Item::Element(name)) = (root, root.map_or(Item::Root, |id| tree.item(id))) {
        if let Some(expected) = dtd.name.as_deref().filter(|_| document)
            && expected != name.local
            && expected != name.qualified
            && !(expected == "HTML" && name.local == "html")
        {
            let message = format!(
                "root and DTD name do not match '{}' and '{expected}'",
                name.local
            );
            run.report(Code::RootName, Some(root), message);
        }
        for id in preorder(tree, Some(root), Some(root)) {
            if let Item::Element(name) = tree.item(id) {
                run.element(id, name);
            }
        }
    }
    run.references();
    run.out
}

impl<'a, T: Navigate> Run<'a, T> {
    fn report(&mut self, code: Code, node: Option<T::Id>, message: String) {
        let offset = node.and_then(|id| self.tree.offset(id));
        self.out.push(Diagnostic {
            code,
            node,
            offset,
            message,
        });
    }

    /// `xmlValidateDtdFinal`: notation attributes and unparsed entities.
    fn declarations(&mut self) {
        for decl in &self.dtd.elements {
            for attribute in &decl.attributes {
                if attribute.kind == AttributeKind::Notation
                    && matches!(decl.content, Some(Content::Empty))
                {
                    let message = format!(
                        "NOTATION attribute {} declared for EMPTY element {}",
                        attribute.name, decl.name
                    );
                    self.report(Code::EmptyNotation, None, message);
                }
            }
        }
        let mut missing: Vec<_> = (self.dtd.entities.values().flatten())
            .filter(|notation| !self.dtd.notations.contains_key(*notation))
            .collect();
        missing.sort_unstable();
        for notation in missing {
            self.report(
                Code::UnknownNotation,
                None,
                format!("NOTATION {notation} is not declared"),
            );
        }
    }

    /// `xmlValidateOneElement` and `xmlValidateOneAttribute` for every attribute.
    fn element(&mut self, id: T::Id, name: QName<'a>) {
        let (tree, dtd) = (self.tree, self.dtd);
        let attributes: Vec<_> = attributes(tree, id).collect();
        match dtd.element(name) {
            Some((index, decl)) if decl.content.is_some() => {
                self.content(id, name, index, decl);
                self.required(id, name, decl, &attributes);
            }
            _ => {
                let message = format!("No declaration for element {}", name.local);
                self.report(Code::UnknownElement, Some(id), message);
            }
        }
        // libxml2 keeps no namespace node for the predefined `xml` binding.
        let (namespaces, ordinary): (Vec<_>, Vec<_>) = attributes
            .into_iter()
            .filter(|(_, attribute, _)| attribute.qualified != "xmlns:xml")
            .partition(|(_, attribute, _)| attribute.is_namespace_declaration());
        for (_, attribute, value) in ordinary.into_iter().chain(namespaces) {
            self.attribute(id, name, attribute, value);
        }
    }

    fn content(&mut self, id: T::Id, name: QName<'a>, index: usize, decl: &ElementDecl) {
        let tree = self.tree;
        let children = || children(tree, id).map(|child| (child, tree.item(child)));
        let element = |item: &Item<'_>| matches!(item, Item::Element(_));
        match decl.content.as_ref().expect("declared") {
            Content::Empty if children().next().is_some() => {
                let message = format!(
                    "Element {} was declared EMPTY this one has content",
                    name.local
                );
                self.report(Code::NotEmpty, Some(id), message);
            }
            Content::Empty | Content::Any => {}
            Content::Mixed(names) if names.is_empty() => {
                if children().any(|(_, item)| element(&item)) {
                    let message = format!(
                        "Element {} was declared #PCDATA but contains non text nodes",
                        name.local
                    );
                    self.report(Code::NotPcdata, Some(id), message);
                }
            }
            Content::Mixed(names) => {
                let allowed = |child: &str| {
                    names
                        .iter()
                        .any(|name| name.rsplit(':').next() == Some(child))
                };
                let invalid: Vec<_> = children()
                    .filter_map(|(_, item)| match item {
                        Item::Element(child) if !allowed(child.local) => Some(child.local),
                        _ => None,
                    })
                    .collect();
                for child in invalid {
                    let message = format!(
                        "Element {child} is not declared in {} list of possible children",
                        name.local
                    );
                    self.report(Code::InvalidChild, Some(id), message);
                }
            }
            Content::Children(model) => {
                let whitespace = |(child, item): (T::Id, Item<'_>)| matches!(item, Item::Text(text) if !tree.is_cdata(child) && text.chars().all(blank));
                if self.dtd.standalone && !decl.internal && children().any(whitespace) {
                    let message = format!(
                        "standalone: {} declared in the external subset contains white spaces nodes",
                        name.local
                    );
                    self.report(Code::StandaloneWhiteSpace, Some(id), message);
                }
                if !model.deterministic() {
                    if self.nondeterministic.insert(index) {
                        let message =
                            format!("Content model of {} is not deterministic", name.local);
                        self.report(Code::ContentNotDeterministic, None, message);
                    }
                    return;
                }
                let end = children().try_fold(0, |state, (child, item)| match item {
                    Item::Element(child) => model.step(state, child.qualified),
                    Item::Text(text) if tree.is_cdata(child) || !text.chars().all(blank) => None,
                    _ => Some(state),
                });
                if !end.is_some_and(|state| model.accepts(state)) {
                    let message = format!("Element {} content does not follow the DTD", name.local);
                    self.report(Code::ContentModel, Some(id), message);
                }
            }
        }
    }

    /// VC: Required Attribute, with libxml2's prefix leniency.
    fn required(
        &mut self,
        id: T::Id,
        name: QName<'a>,
        decl: &ElementDecl,
        present: &[(usize, QName<'a>, &'a str)],
    ) {
        for attribute in &decl.attributes {
            if !matches!(attribute.default, StoredDefault::Required) {
                continue;
            }
            let wanted = attribute.name.as_str();
            let namespace = wanted == "xmlns" || wanted.starts_with("xmlns:");
            let candidates = present
                .iter()
                .map(|(_, name, _)| name)
                .filter(|name| name.is_namespace_declaration() == namespace);
            let code = match wanted.split_once(':') {
                _ if namespace => candidates
                    .clone()
                    .all(|name| name.qualified != wanted)
                    .then_some(Code::MissingAttribute),
                None => candidates
                    .clone()
                    .all(|name| name.local != wanted)
                    .then_some(Code::MissingAttribute),
                Some((prefix, local)) => {
                    let prefixes: Vec<_> = candidates
                        .filter(|name| name.local == local)
                        .map(|name| name.qualified.split_once(':').map(|(prefix, _)| prefix))
                        .collect();
                    if prefixes.contains(&Some(prefix)) {
                        None
                    } else if prefixes.iter().any(Option::is_some) {
                        Some(Code::DifferentPrefix)
                    } else if prefixes.is_empty() {
                        Some(Code::MissingAttribute)
                    } else {
                        Some(Code::NoPrefix)
                    }
                }
            };
            let message = match code {
                None => continue,
                Some(Code::MissingAttribute) => {
                    format!("Element {} does not carry attribute {wanted}", name.local)
                }
                Some(Code::NoPrefix) => {
                    format!(
                        "Element {} required attribute {wanted} has no prefix",
                        name.local
                    )
                }
                Some(_) => format!(
                    "Element {} required attribute {wanted} has different prefix",
                    name.local
                ),
            };
            self.report(code.expect("reported"), Some(id), message);
        }
    }

    /// `xmlValidateOneAttribute`, or `xmlValidateOneNamespace` for declarations.
    fn attribute(&mut self, id: T::Id, element: QName<'a>, attribute: QName<'a>, value: &'a str) {
        let namespace = attribute.is_namespace_declaration();
        let shown = match namespace {
            true => attribute.qualified,
            false => attribute.local,
        };
        let owner = element.local;
        let dtd = self.dtd;
        let Some(decl) = dtd.attribute(element, attribute.qualified) else {
            let message = format!("No declaration for attribute {shown} of element {owner}");
            return self.report(Code::UnknownAttribute, Some(id), message);
        };
        if !lexical(decl.kind, value) {
            let code = match namespace {
                true => Code::InvalidDefault,
                false => Code::AttributeValue,
            };
            let message = format!("Syntax of value for attribute {shown} of {owner} is not valid");
            self.report(code, Some(id), message);
        }
        let fixed = decl.fixed().filter(|&fixed| fixed != value);
        if let Some(fixed) = fixed {
            let message = format!(
                "Value for attribute {shown} of {owner} is different from default \"{fixed}\""
            );
            self.report(Code::AttributeDefault, Some(id), message);
        }
        match decl.kind {
            AttributeKind::Id if !namespace => {
                // libxml2's xmlAddID refuses an empty value as a duplicate.
                let fresh = !value.is_empty() && self.ids.insert(value);
                if !fresh {
                    let message = format!("ID {value} already defined");
                    self.report(Code::IdRedefined, Some(id), message);
                }
            }
            AttributeKind::IdRef | AttributeKind::IdRefs if !namespace => {
                let multiple = decl.kind == AttributeKind::IdRefs;
                self.references.push((id, attribute.local, value, multiple));
            }
            AttributeKind::Notation => {
                self.notation(id, format!("Value \"{value}\" for attribute {shown} of {owner} is not a declared Notation"), value);
                if !decl.values.iter().any(|token| token == value) {
                    let message = format!(
                        "Value \"{value}\" for attribute {shown} of {owner} is not among the enumerated notations"
                    );
                    self.report(Code::NotationValue, Some(id), message);
                }
            }
            AttributeKind::Enumeration if !decl.values.iter().any(|token| token == value) => {
                let message = format!(
                    "Value \"{value}\" for attribute {shown} of {owner} is not among the enumerated set"
                );
                self.report(Code::AttributeValue, Some(id), message);
            }
            _ => {}
        }
        if let Some(fixed) = fixed {
            let code = match namespace {
                true => Code::ElementNamespace,
                false => Code::AttributeValue,
            };
            let message = format!("Value for attribute {shown} of {owner} must be \"{fixed}\"");
            self.report(code, Some(id), message);
        }
        // xmlValidateAttributeValue2
        match decl.kind {
            AttributeKind::Entity => self.entity(id, shown, value),
            AttributeKind::Entities => {
                for name in value.split(blank).filter(|name| !name.is_empty()) {
                    self.entity(id, shown, name);
                }
            }
            AttributeKind::Notation => self.notation(
                id,
                format!("NOTATION attribute {shown} reference an unknown notation \"{value}\""),
                value,
            ),
            _ => {}
        }
    }

    fn notation(&mut self, id: T::Id, message: String, value: &str) {
        if !self.dtd.notations.contains_key(value) {
            self.report(Code::UnknownNotation, Some(id), message);
        }
    }

    fn entity(&mut self, id: T::Id, attribute: &str, value: &str) {
        let code = match self.dtd.entities.get(value) {
            Some(Some(_)) => return,
            Some(None) => Code::EntityType,
            None if PREDEFINED.contains(&value) => Code::EntityType,
            None => Code::UnknownEntity,
        };
        let message = match code {
            Code::EntityType => {
                format!(
                    "ENTITY attribute {attribute} reference an entity \"{value}\" of wrong type"
                )
            }
            _ => format!("ENTITY attribute {attribute} reference an unknown entity \"{value}\""),
        };
        self.report(code, Some(id), message);
    }

    /// `xmlValidateDocumentFinal`: every IDREF names an ID.
    fn references(&mut self) {
        for (id, attribute, value, multiple) in std::mem::take(&mut self.references) {
            let names = match multiple {
                true => value.split(blank).filter(|name| !name.is_empty()).collect(),
                false => vec![value],
            };
            let missing: Vec<_> = names
                .into_iter()
                .filter(|name| !self.ids.contains(name))
                .collect();
            for name in missing {
                let kind = if multiple { "IDREFS" } else { "IDREF" };
                let message =
                    format!("{kind} attribute {attribute} references an unknown ID \"{name}\"");
                self.report(Code::UnknownId, Some(id), message);
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests/internal/valid.rs"]
mod tests;
