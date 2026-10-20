//! DTD validation after libxml2's `valid.c`.
//!
//! [`Document::validate`](crate::Document::validate) and
//! [`OwnedDocument::validate`](crate::OwnedDocument::validate) check a parsed
//! tree against its own DTD (`xmlValidateDocument`, `XML_PARSE_DTDVALID`);
//! [`validate`] checks any tree against a separately parsed [`Dtd`]
//! (`xmlValidateDtd`, `xmllint --dtdvalid`). Validity errors are never fatal:
//! each is a [`Diagnostic`] whose [`Code`] carries libxml2's `XML_DTD_*` number.

mod engine;
mod model;
mod stream;

use std::collections::HashMap;

use crate::dtd::{ElementContentView, ElementDeclarationView, ModelTokenView};
use crate::dtd_catalog::{AttributeKind, StoredDefault};
use crate::name::{name_char, name_start};
use crate::xpath::tree::{Navigate, QName, Tree};
use crate::{Error, Limits, Resolver};

pub(crate) use model::{Model, TooComplex};
pub use stream::StreamValidator;

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
    pub(crate) kind: AttributeKind,
    /// Enumerated or notation tokens.
    pub(crate) values: Vec<String>,
    pub(crate) default: StoredDefault,
    pub(crate) internal: bool,
}

impl AttributeDecl {
    fn fixed(&self) -> Option<&str> {
        match &self.default {
            StoredDefault::Value {
                fixed: true, value, ..
            } => Some(value),
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

    /// Whether `name` is declared with element-only content (`Some(true)`),
    /// ANY or mixed content (`Some(false)`), or neither (`None`): the cases
    /// libxml2's `XML_PARSE_NOBLANKS` decides by declaration.
    pub fn has_element_content(&self, name: &str) -> Option<bool> {
        match self
            .elements
            .get(*self.element_index.get(name)?)?
            .content
            .as_ref()?
        {
            Content::Children(_) => Some(true),
            Content::Any | Content::Mixed(_) => Some(false),
            Content::Empty => None,
        }
    }

    fn element(&self, name: QName<'_>) -> Option<(usize, &ElementDecl)> {
        let index = self.element_index.get(name.qualified).or_else(|| {
            (name.qualified != name.local)
                .then(|| self.element_index.get(name.local))
                .flatten()
        })?;
        Some((*index, &self.elements[*index]))
    }

    /// Whether the DTD declares `attribute` of `element` (qualified names)
    /// with type `ID`.
    pub(crate) fn declares_id(&self, element: &str, attribute: &str) -> bool {
        self.element_index.get(element).is_some_and(|&index| {
            let element = &self.elements[index];
            (element.index.get(attribute))
                .is_some_and(|&a| element.attributes[a].kind == AttributeKind::Id)
        })
    }

    /// The default or fixed value declared for `attribute` of `element`
    /// (names as written), as `xmlGetDtdQAttrDesc` finds it.
    pub(crate) fn default_value(&self, element: &str, attribute: &str) -> Option<&str> {
        let element = &self.elements[*self.element_index.get(element)?];
        element.attributes[*element.index.get(attribute)?]
            .default
            .value()
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
                let kind = match attribute.kind {
                    AttributeKind::Notation => "notation",
                    _ => "enumeration",
                };
                let message =
                    format!("standalone: attribute {kind} value token {value} duplicated");
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
                    // xmlValidateAttributeDecl: the element's internal IDs, then all.
                    let ids = decl
                        .attributes
                        .iter()
                        .filter(|a| a.kind == AttributeKind::Id);
                    let internal = ids.clone().filter(|a| a.internal).count();
                    let (count, subset) = match attribute.internal {
                        true => (internal + 1, "internal"),
                        false if internal > 1 => (internal, "internal"),
                        false => (ids.count() + 1, "external"),
                    };
                    if count > 1 {
                        let message = format!(
                            "Element {owner} has {count} ID attribute defined in the {subset} subset : {name}"
                        );
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

const PREDEFINED: [&str; 5] = ["lt", "gt", "amp", "apos", "quot"];

/// Checks `tree` against `dtd` as libxml2's `xmlValidateDtd`: the root,
/// every element and attribute, then IDREFs. Declaration-time checks of
/// `dtd` and the DOCTYPE name are not repeated.
pub fn validate<T: Tree>(tree: &T, dtd: &Dtd) -> Vec<Diagnostic<T::Id>> {
    engine::run(tree, dtd, false)
}

/// `xmlValidateDocument` with the checks libxml2 makes while parsing.
pub(crate) fn validate_document<T: Navigate>(
    tree: &T,
    dtd: Option<&Dtd>,
) -> Vec<Diagnostic<T::Id>> {
    match dtd {
        Some(dtd) => engine::run(tree, dtd, true),
        None => vec![Diagnostic {
            code: Code::NoDtd,
            node: None,
            offset: None,
            message: "no DTD found!".into(),
        }],
    }
}

#[cfg(test)]
#[path = "../tests/internal/valid.rs"]
mod tests;
