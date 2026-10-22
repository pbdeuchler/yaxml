//! DTD validation after libxml2's `valid.c`.
//!
//! [`Document::validate`](crate::Document::validate) and
//! [`OwnedDocument::validate`](crate::OwnedDocument::validate) check a parsed
//! tree against its own DTD (`xmlValidateDocument`, `XML_PARSE_DTDVALID`);
//! [`validate`] checks any tree against a separately parsed [`Dtd`]
//! (`xmlValidateDtd`, `xmllint --dtdvalid`). Validity errors are never fatal:
//! each is a [`Diagnostic`] whose [`Code`] carries libxml2's `XML_DTD_*` number.
//! [`StreamValidator`] validates parser events without a tree.
//!
//! ```
//! use yaxml::valid::{self, Code};
//!
//! let xml = "<!DOCTYPE r [<!ELEMENT r (a)><!ATTLIST r id ID #REQUIRED>]><r/>";
//! let diagnostics = yaxml::Document::parse(xml)?.validate();
//! let codes: Vec<_> = diagnostics.iter().map(|d| d.code).collect();
//! assert_eq!(codes, [Code::ContentModel, Code::MissingAttribute]);
//! assert!(!valid::is_valid(&diagnostics));
//! # Ok::<(), yaxml::Error>(())
//! ```

mod engine;
mod model;
mod stream;

use std::collections::HashMap;

use crate::dtd::{ElementContentView, ElementDeclarationView, ModelTokenView};
use crate::dtd_catalog::{EntityBody, EntityDecl, EntityId, StoredDefault};
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
    /// An allocation the document sized failed; validation stopped (`XML_ERR_NO_MEMORY`).
    Allocation = 2,
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
    /// The tree has no document element (`xmlValidateRoot`); nothing else
    /// is checked.
    NoRoot = 525,
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
    /// What went wrong.
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

/// A DTD's element, attribute-list, entity and notation declarations
/// (libxml2's `xmlDtd`).
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
    pub(crate) notation_index: HashMap<String, usize>,
    pub(crate) notations: Vec<NotationDecl>,
    /// Entity declarations in declaration order, duplicates dropped.
    pub(crate) entities: Vec<EntityDecl>,
    /// General entities references find; a standalone document's external
    /// declarations are not bound.
    pub(crate) general: HashMap<String, EntityId>,
    pub(crate) parameter: HashMap<String, EntityId>,
    /// The first declaration of each general entity that is not bound (a
    /// standalone document's external one, declared before any bound one).
    pub(crate) unbound: HashMap<String, EntityId>,
    /// Diagnostics libxml2 reports while parsing, with their input offsets.
    pub(crate) issues: Vec<(Code, Option<usize>, String)>,
}

#[derive(Clone, Debug)]
pub(crate) struct ElementDecl {
    pub(crate) name: String,
    /// `None` until an `<!ELEMENT>` declaration; attribute lists may come first.
    pub(crate) content: Option<Content>,
    /// The content model as libxml2 prints it; empty unless mixed or children.
    pub(crate) model: String,
    pub(crate) internal: bool,
    pub(crate) attributes: Vec<AttributeDecl>,
    /// Attribute names to positions, built once the list outgrows a search.
    index: HashMap<String, usize>,
    /// The amplification charge of copying every default (`copy_cost`).
    pub(crate) copied: usize,
    /// Some default declares a namespace.
    pub(crate) namespace_defaults: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct NotationDecl {
    pub(crate) name: String,
    pub(crate) public: Option<String>,
    pub(crate) system: Option<String>,
    pub(crate) internal: bool,
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
    /// The amplification charge of copying its default into a tag, as
    /// libxml2 counts `expandedSize`; zero without one.
    pub(crate) fn copy_cost(&self) -> usize {
        self.default.value().map_or(0, |value| {
            self.name.len() + value.len() + crate::dtd_controller::ENTITY_FIXED_COST
        })
    }

    /// It is `xmlns` or `xmlns:*`.
    pub(crate) fn declares_namespace(&self) -> bool {
        self.name == "xmlns" || self.name.starts_with("xmlns:")
    }

    fn fixed(&self) -> Option<&str> {
        match &self.default {
            StoredDefault::Value {
                fixed: true, value, ..
            } => Some(value),
            _ => None,
        }
    }
}

impl Dtd {
    /// The first declaration of general entity `name`, bound or not, as
    /// ENTITY attribute validation finds it.
    pub(crate) fn declared(&self, name: &str) -> Option<EntityId> {
        (self.unbound.get(name).or_else(|| self.general.get(name))).copied()
    }

    /// `self`, a freshly read internal subset, followed by `old`'s
    /// declarations from the external subset: a name `self` declares keeps
    /// its own, as libxml2 looks in `intSubset` before `extSubset`. Fails
    /// without memory.
    pub(crate) fn with_external(mut self, old: &Dtd) -> Result<Self, ()> {
        let entities = old.entities.iter().filter(|entity| entity.external_subset);
        (self.entities.try_reserve(entities.clone().count())).map_err(drop)?;
        (self.elements.try_reserve(old.elements.len())).map_err(drop)?;
        (self.element_index.try_reserve(old.elements.len())).map_err(drop)?;
        for entity in entities {
            let (name, parameter) = (&entity.name, entity.parameter);
            let table = match (parameter, parameter || !self.standalone) {
                (true, _) if self.parameter.contains_key(name) => continue,
                (false, _) if self.declared(name).is_some() => continue,
                (true, _) => &mut self.parameter,
                (false, true) => &mut self.general,
                (false, false) => &mut self.unbound,
            };
            table.insert(name.clone(), EntityId(self.entities.len()));
            let input = None;
            self.entities.push(EntityDecl {
                input,
                ..entity.clone()
            });
        }
        for notation in old.notations.iter().filter(|notation| !notation.internal) {
            self.declare_notation(notation.clone());
        }
        for old in &old.elements {
            let content = old.content.as_ref().filter(|_| !old.internal);
            let mut attributes = old.attributes.iter().filter(|a| !a.internal).peekable();
            if content.is_none() && attributes.peek().is_none() {
                continue;
            }
            let index = *(self.element_index)
                .entry(old.name.clone())
                .or_insert(self.elements.len());
            if index == self.elements.len() {
                self.elements.push(ElementDecl::new(old.name.clone()));
            }
            let element = &mut self.elements[index];
            if element.content.is_none()
                && let Some(content) = content
            {
                element.content = Some(content.clone());
                element.model = old.model.clone();
            }
            for attribute in attributes {
                if element.attribute(&attribute.name).is_none() {
                    element.push_attribute(attribute.clone())?;
                }
            }
        }
        Ok(self)
    }

    /// The first declaration of each general entity (see `declared`).
    pub(crate) fn declared_ids(&self) -> impl Iterator<Item = EntityId> {
        let bound = self.general.iter();
        let bound = bound.filter(|(name, _)| !self.unbound.contains_key(*name));
        self.unbound
            .values()
            .chain(bound.map(|(_, id)| id))
            .copied()
    }
}

impl ElementDecl {
    /// [`Dtd::has_element_content`] of this declaration.
    pub(crate) fn element_content(&self) -> Option<bool> {
        match self.content.as_ref()? {
            Content::Children(_) => Some(true),
            Content::Any | Content::Mixed(_) => Some(false),
            Content::Empty => None,
        }
    }

    pub(crate) fn new(name: String) -> Self {
        Self {
            name,
            content: None,
            model: String::new(),
            internal: false,
            attributes: Vec::new(),
            index: HashMap::new(),
            copied: 0,
            namespace_defaults: false,
        }
    }

    /// The position of attribute `name`: a short list is searched, a longer
    /// one indexed.
    pub(crate) fn attribute(&self, name: &str) -> Option<usize> {
        match self.index.is_empty() {
            true => self.attributes.iter().position(|decl| decl.name == name),
            false => self.index.get(name).copied(),
        }
    }

    /// Appends an attribute that is not declared yet; fails without memory.
    pub(crate) fn push_attribute(&mut self, attribute: AttributeDecl) -> Result<(), ()> {
        const SEARCHED: usize = 8;
        self.attributes.try_reserve(1).map_err(drop)?;
        self.copied = self.copied.saturating_add(attribute.copy_cost());
        self.namespace_defaults |=
            attribute.default.value().is_some() && attribute.declares_namespace();
        self.attributes.push(attribute);
        if self.attributes.len() <= SEARCHED {
            return Ok(());
        }
        let indexed = self.index.len();
        let fresh = &self.attributes[indexed..];
        self.index.try_reserve(fresh.len()).map_err(drop)?;
        for (position, attribute) in (indexed..).zip(fresh) {
            let mut key = String::new();
            key.try_reserve_exact(attribute.name.len()).map_err(drop)?;
            key.push_str(&attribute.name);
            self.index.insert(key, position);
        }
        Ok(())
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
        (self.elements.get(*self.element_index.get(name)?)?).element_content()
    }

    fn element_for(&self, name: QName<'_>) -> Option<(usize, &ElementDecl)> {
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
            (element.attribute(attribute))
                .is_some_and(|a| element.attributes[a].kind == AttributeKind::Id)
        })
    }

    /// The default or fixed value declared for `attribute` of `element`
    /// (names as written), as `xmlGetDtdQAttrDesc` finds it.
    pub(crate) fn default_value(&self, element: &str, attribute: &str) -> Option<&str> {
        let element = &self.elements[*self.element_index.get(element)?];
        element.attributes[element.attribute(attribute)?]
            .default
            .value()
    }

    /// The declaration of attribute `attribute` on `element`, qualified name first.
    fn attribute_for(
        &self,
        element: QName<'_>,
        attribute: &str,
    ) -> Option<((usize, usize), &AttributeDecl)> {
        let declared = |name: &str| {
            let owner = *self.element_index.get(name)?;
            let slot = self.elements[owner].attribute(attribute)?;
            Some(((owner, slot), &self.elements[owner].attributes[slot]))
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
        let model = match view.content() {
            ElementContentView::Mixed(tokens) | ElementContentView::Children(tokens) => {
                crate::save::dtd::content_model(tokens)
            }
            ElementContentView::Empty | ElementContentView::Any => String::new(),
        };
        let (content, bytes) = match view.content() {
            ElementContentView::Empty => (Content::Empty, 0),
            ElementContentView::Any => (Content::Any, 0),
            ElementContentView::Mixed(tokens) => {
                let mut names: Vec<String> = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for token in tokens {
                    let ModelTokenView::Name(name) = token else {
                        continue;
                    };
                    if !seen.insert(name) {
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
        let bytes = bytes + model.len();
        let element = &mut self.elements[index];
        element.content = Some(content);
        element.model = model;
        element.internal = internal;
        Ok(bytes)
    }

    /// Records a notation declaration.
    pub(crate) fn declare_notation(&mut self, notation: NotationDecl) {
        let name = &notation.name;
        match self.notation_index.get(name) {
            Some(&seen) if self.notations[seen].internal == notation.internal => {
                let message = format!("xmlAddNotationDecl: {name} already defined");
                self.issues.push((Code::NotationRedefined, None, message));
            }
            Some(_) => {}
            None => {
                self.notation_index
                    .insert(name.clone(), self.notations.len());
                self.notations.push(notation);
            }
        }
    }

    /// The declaration-time checks of `xmlAddAttributeDecl`,
    /// `xmlValidateAttributeDecl` and the ATTLIST parser for attribute
    /// `attribute` of element `element`, before it is added.
    pub(crate) fn check_attribute(&mut self, element: usize, attribute: &AttributeDecl) {
        let decl = &self.elements[element];
        let (name, owner) = (&attribute.name, &decl.name);
        let mut seen = std::collections::HashSet::new();
        for value in &attribute.values {
            if !seen.insert(value) {
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
        let lexical = default.is_none_or(|value| value_matches(attribute.kind, value));
        if !lexical {
            let message = format!("Attribute {owner} of {name}: invalid default value");
            self.issues.push((Code::AttributeDefault, None, message));
        }
        match decl.attribute(name).map(|index| &decl.attributes[index]) {
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

/// A declared attribute type; the discriminant is libxml2's `xmlAttributeType`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum AttributeKind {
    /// `CDATA`
    Cdata = 1,
    /// `ID`
    Id = 2,
    /// `IDREF`
    IdRef = 3,
    /// `IDREFS`
    IdRefs = 4,
    /// `ENTITY`
    Entity = 5,
    /// `ENTITIES`
    Entities = 6,
    /// `NMTOKEN`
    NmToken = 7,
    /// `NMTOKENS`
    NmTokens = 8,
    /// An enumeration, `(a | b)`.
    Enumeration = 9,
    /// `NOTATION (a | b)`
    Notation = 10,
}

/// What an entity declares; the discriminant is libxml2's `xmlEntityType`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum EntityKind {
    /// `<!ENTITY name "value">`
    InternalGeneral = 1,
    /// `<!ENTITY name SYSTEM "uri">`
    ExternalParsedGeneral = 2,
    /// `<!ENTITY name SYSTEM "uri" NDATA notation>`
    ExternalUnparsedGeneral = 3,
    /// `<!ENTITY % name "value">`
    InternalParameter = 4,
    /// `<!ENTITY % name SYSTEM "uri">`
    ExternalParameter = 5,
    /// `lt`, `gt`, `amp`, `apos` or `quot`, unless the DTD redeclares it.
    Predefined = 6,
}

/// An entity declaration (libxml2's `xmlEntity`), borrowed from its [`Dtd`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Entity<'a> {
    /// The entity's name.
    pub name: &'a str,
    /// What the entity declares.
    pub kind: EntityKind,
    /// The replacement text (libxml2's `content`): an internal entity's
    /// literal with character and parameter-entity references expanded and
    /// general references kept; an external parsed entity's text once a
    /// resolver has loaded it; an unparsed entity's notation, as libxml2
    /// stores it; otherwise `None`.
    pub content: Option<&'a str>,
    /// The public identifier.
    pub public_id: Option<&'a str>,
    /// The system identifier as written.
    pub system_id: Option<&'a str>,
    /// The system identifier resolved against the base of the resource that
    /// declares it (libxml2's `URI`); without a resolver, the system
    /// identifier as written.
    pub uri: Option<&'a str>,
    /// An unparsed entity's `NDATA` notation.
    pub notation: Option<&'a str>,
    /// Declared in the external subset (libxml2's `doc->extSubset`), not in
    /// the internal subset or a parameter entity it references.
    pub external_subset: bool,
}

/// An element's declared content; the discriminant is libxml2's `xmlElementTypeVal`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum ContentKind {
    /// Only attribute lists name the element; no `<!ELEMENT>` declaration.
    Undefined = 0,
    /// `EMPTY`
    Empty = 1,
    /// `ANY`
    Any = 2,
    /// `(#PCDATA | ...)*`
    Mixed = 3,
    /// Element content, `(a , b)`.
    Element = 4,
}

/// An element type (libxml2's `xmlElement`), borrowed from its [`Dtd`].
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct Element<'a> {
    /// The element type's name.
    pub name: &'a str,
    /// The declared content.
    pub kind: ContentKind,
    /// Mixed or element content as libxml2 prints it, e.g. `(a , (b | c)*)`.
    pub model: Option<&'a str>,
    decl: &'a ElementDecl,
}

impl<'a> Element<'a> {
    /// The element's attribute declarations, in declaration order.
    pub fn attributes(self) -> impl Iterator<Item = Attribute<'a>> {
        let element = self.decl.name.as_str();
        (self.decl.attributes.iter()).map(move |decl| Attribute::new(element, decl))
    }

    /// The names in the content model, each once in model order, `#PCDATA`
    /// for mixed content (`xmlValidGetPotentialChildren`); none for `EMPTY`
    /// and `ANY`.
    pub fn potential_children(self) -> impl Iterator<Item = &'a str> {
        let mut seen = std::collections::HashSet::new();
        (self.model.unwrap_or_default())
            .split(['(', ')', '|', ',', '?', '*', '+', ' '])
            .filter(move |name| !name.is_empty() && seen.insert(*name))
    }

    /// The mixed or element content model as a tree (libxml2's
    /// `xmlElementContent`); `None` for `EMPTY`, `ANY` and undeclared.
    ///
    /// ```
    /// use yaxml::Document;
    /// use yaxml::valid::{Occurrence, ParticleKind};
    /// let document = Document::parse("<!DOCTYPE r [<!ELEMENT r (a, (b | c)*)>]><r/>")?;
    /// let model = document.dtd().unwrap().element("r").unwrap().content().unwrap();
    /// assert_eq!((model.kind(), model.occurrence()), (ParticleKind::Sequence, Occurrence::Once));
    /// let [a, choice] = model.children().collect::<Vec<_>>()[..] else { panic!() };
    /// assert_eq!((a.kind(), a.name()), (ParticleKind::Element, Some("a")));
    /// assert_eq!((choice.kind(), choice.occurrence()), (ParticleKind::Choice, Occurrence::ZeroOrMore));
    /// assert_eq!(choice.children().filter_map(|c| c.name()).collect::<Vec<_>>(), ["b", "c"]);
    /// # Ok::<(), yaxml::Error>(())
    /// ```
    pub fn content(self) -> Option<Particle<'a>> {
        let (group, occurrence) = Particle::split(self.model?);
        let body = &group[1..group.len() - 1];
        // A one-item model is that item, with the model's occurrence.
        let text = match separator(body) {
            Some(_) => group,
            None => body,
        };
        Some(Particle { text, occurrence })
    }
}

/// How often a particle occurs; the discriminant is libxml2's
/// `xmlElementContentOccur`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum Occurrence {
    /// No indicator.
    Once = 1,
    /// `?`
    Optional = 2,
    /// `*`
    ZeroOrMore = 3,
    /// `+`
    OneOrMore = 4,
}

/// A particle's kind; the discriminant is libxml2's `xmlElementContentType`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum ParticleKind {
    /// `#PCDATA`, the first child of a mixed model's choice (or the whole
    /// `(#PCDATA)` model).
    Pcdata = 1,
    /// An element name.
    Element = 2,
    /// `(a , b)`
    Sequence = 3,
    /// `(a | b)`
    Choice = 4,
}

/// One particle of a content model, borrowed from its [`Element`]. libxml2
/// chains a group's items through binary `c1`/`c2` nodes; here a group lists
/// them, read from [`Element::model`] (`xmlBufDumpElementContent`'s form), so
/// a group of the same connector without an indicator merges into its parent:
/// `(a , (b , c))`, `((a , b) , c)` and `(a , b , c)` are one three-item
/// sequence (libxml2 keeps `((a , b) , c)` left-nested), while
/// `((a , b)* , c)` keeps its inner group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Particle<'a> {
    /// A name, `#PCDATA` or a parenthesized group, without its indicator.
    text: &'a str,
    occurrence: Occurrence,
}

/// The connector of a group's body: the first one outside nested groups.
fn separator(body: &str) -> Option<u8> {
    let mut depth = 0usize;
    body.bytes().find(|&byte| {
        match byte {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => return depth == 0 && matches!(byte, b',' | b'|'),
        }
        false
    })
}

impl<'a> Particle<'a> {
    /// Splits a printed item into its text and occurrence indicator.
    fn split(item: &'a str) -> (&'a str, Occurrence) {
        let occurrence = match item.as_bytes().last() {
            Some(b'?') => Occurrence::Optional,
            Some(b'*') => Occurrence::ZeroOrMore,
            Some(b'+') => Occurrence::OneOrMore,
            _ => return (item, Occurrence::Once),
        };
        (&item[..item.len() - 1], occurrence)
    }

    /// The particle's kind.
    pub fn kind(self) -> ParticleKind {
        match self.text.as_bytes()[0] {
            b'#' => ParticleKind::Pcdata,
            b'(' if separator(&self.text[1..self.text.len() - 1]) == Some(b',') => {
                ParticleKind::Sequence
            }
            b'(' => ParticleKind::Choice,
            _ => ParticleKind::Element,
        }
    }

    /// Its occurrence indicator.
    pub fn occurrence(self) -> Occurrence {
        self.occurrence
    }

    /// The qualified element name of an [`ParticleKind::Element`] particle
    /// (libxml2 splits it into `prefix` and `name`).
    pub fn name(self) -> Option<&'a str> {
        (self.kind() == ParticleKind::Element).then_some(self.text)
    }

    /// A group's items in order; none for a name or `#PCDATA`.
    pub fn children(self) -> impl Iterator<Item = Particle<'a>> + 'a {
        let body = match self.text.strip_prefix('(') {
            Some(body) => &body[..body.len() - 1],
            None => "",
        };
        let mut rest = (!body.is_empty()).then_some(body);
        std::iter::from_fn(move || {
            let item = rest?;
            let mut depth = 0usize;
            let end = item.bytes().position(|byte| {
                match byte {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    b' ' if depth == 0 => return true,
                    _ => {}
                }
                false
            });
            // Items are separated by " , " or " | ".
            rest = end.map(|end| &item[end + 3..]);
            let (text, occurrence) = Particle::split(&item[..end.unwrap_or(item.len())]);
            Some(Particle { text, occurrence })
        })
    }
}

impl std::fmt::Debug for Element<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Element")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

/// An attribute's default declaration; libxml2's `xmlAttributeDefault` and
/// `defaultValue`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttributeDefault<'a> {
    /// A default value, normalized, as libxml2's `defaultValue`: general
    /// entity references expanded under `NOENT`, otherwise kept (`&name;`,
    /// with a literal `&` as `&#38;`).
    Value(&'a str),
    /// `#REQUIRED`
    Required,
    /// `#IMPLIED`
    Implied,
    /// `#FIXED "value"`, spelled as [`AttributeDefault::Value`].
    Fixed(&'a str),
}

/// An attribute declaration (libxml2's `xmlAttribute`), borrowed from its [`Dtd`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Attribute<'a> {
    /// The declaring element type's name.
    pub element: &'a str,
    /// The qualified name as declared.
    pub name: &'a str,
    /// The declared type.
    pub kind: AttributeKind,
    /// The default declaration.
    pub default: AttributeDefault<'a>,
    /// An enumeration's or `NOTATION` type's tokens, in declaration order.
    pub values: &'a [String],
}

impl<'a> Attribute<'a> {
    fn new(element: &'a str, decl: &'a AttributeDecl) -> Self {
        let default = match &decl.default {
            StoredDefault::Required => AttributeDefault::Required,
            StoredDefault::Implied => AttributeDefault::Implied,
            StoredDefault::Value { fixed, value, kept } => {
                let value = kept.as_deref().unwrap_or(value);
                match fixed {
                    true => AttributeDefault::Fixed(value),
                    false => AttributeDefault::Value(value),
                }
            }
        };
        Self {
            element,
            name: &decl.name,
            kind: decl.kind,
            default,
            values: &decl.values,
        }
    }
}

/// A notation declaration (libxml2's `xmlNotation`), borrowed from its [`Dtd`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Notation<'a> {
    /// The notation's name.
    pub name: &'a str,
    /// The public identifier.
    pub public_id: Option<&'a str>,
    /// The system identifier.
    pub system_id: Option<&'a str>,
}

/// The five predefined entities and their replacement text.
const PREDEFINED: [(&str, &str); 5] = [
    ("lt", "<"),
    ("gt", ">"),
    ("amp", "&"),
    ("apos", "'"),
    ("quot", "\""),
];

/// A predefined entity, `lt`, `gt`, `amp`, `apos` or `quot`
/// (`xmlGetPredefinedEntity`).
pub fn predefined_entity(name: &str) -> Option<Entity<'static>> {
    let &(name, content) = PREDEFINED.iter().find(|(entity, _)| *entity == name)?;
    Some(Entity {
        name,
        kind: EntityKind::Predefined,
        content: Some(content),
        public_id: None,
        system_id: None,
        uri: None,
        notation: None,
        external_subset: false,
    })
}

impl Dtd {
    fn entity_view(&self, id: EntityId) -> Entity<'_> {
        let decl = &self.entities[id.0];
        let (kind, external) = match (&decl.body, decl.parameter) {
            (EntityBody::Internal(_), false) => (EntityKind::InternalGeneral, None),
            (EntityBody::Internal(_), true) => (EntityKind::InternalParameter, None),
            (EntityBody::External(external), true) => {
                (EntityKind::ExternalParameter, Some(external))
            }
            (EntityBody::External(external), false) => match external.notation {
                Some(_) => (EntityKind::ExternalUnparsedGeneral, Some(external)),
                None => (EntityKind::ExternalParsedGeneral, Some(external)),
            },
        };
        Entity {
            name: &decl.name,
            kind,
            content: decl
                .body
                .text()
                .or(external.and_then(|e| e.notation.as_deref())),
            public_id: external.and_then(|external| external.public.as_deref()),
            system_id: external.and_then(|external| external.system.as_deref()),
            uri: external
                .and_then(|external| external.uri.as_deref().or(external.system.as_deref())),
            notation: external.and_then(|external| external.notation.as_deref()),
            external_subset: decl.external_subset,
        }
    }

    /// The general entity `name` as references find it, else a predefined
    /// entity (`xmlGetDocEntity`). A standalone document's externally
    /// declared entities are not found; [`Dtd::entities`] lists them.
    pub fn entity(&self, name: &str) -> Option<Entity<'_>> {
        match self.general.get(name) {
            Some(&id) => Some(self.entity_view(id)),
            None => predefined_entity(name),
        }
    }

    /// The parameter entity `name` (`xmlGetParameterEntity`).
    pub fn parameter_entity(&self, name: &str) -> Option<Entity<'_>> {
        Some(self.entity_view(*self.parameter.get(name)?))
    }

    /// Every general and parameter entity declaration in declaration order,
    /// a repeated name keeping its first (libxml2's entity tables); filter
    /// on [`Entity::external_subset`] for `xmlGetDtdEntity`.
    pub fn entities(&self) -> impl Iterator<Item = Entity<'_>> {
        (0..self.entities.len()).map(|index| self.entity_view(EntityId(index)))
    }

    fn element_view<'a>(&'a self, decl: &'a ElementDecl) -> Element<'a> {
        let kind = match decl.content {
            None => ContentKind::Undefined,
            Some(Content::Empty) => ContentKind::Empty,
            Some(Content::Any) => ContentKind::Any,
            Some(Content::Mixed(_)) => ContentKind::Mixed,
            Some(Content::Children(_)) => ContentKind::Element,
        };
        Element {
            name: &decl.name,
            kind,
            model: (!decl.model.is_empty()).then_some(decl.model.as_str()),
            decl,
        }
    }

    /// The element type `name`, qualified as declared (`xmlGetDtdElementDesc`).
    pub fn element(&self, name: &str) -> Option<Element<'_>> {
        Some(self.element_view(&self.elements[*self.element_index.get(name)?]))
    }

    /// Every element type in order of first declaration or attribute list.
    pub fn elements(&self) -> impl Iterator<Item = Element<'_>> {
        self.elements.iter().map(|decl| self.element_view(decl))
    }

    /// Attribute `name` of element type `element`, both qualified as
    /// declared (`xmlGetDtdAttrDesc`, `xmlGetDtdQAttrDesc`).
    pub fn attribute(&self, element: &str, name: &str) -> Option<Attribute<'_>> {
        let decl = &self.elements[*self.element_index.get(element)?];
        Some(Attribute::new(
            &decl.name,
            &decl.attributes[decl.attribute(name)?],
        ))
    }

    /// The notation `name` (`xmlGetDtdNotationDesc`).
    pub fn notation(&self, name: &str) -> Option<Notation<'_>> {
        Some(notation_view(
            &self.notations[*self.notation_index.get(name)?],
        ))
    }

    /// Every notation in declaration order.
    pub fn notations(&self) -> impl Iterator<Item = Notation<'_>> {
        self.notations.iter().map(notation_view)
    }
}

fn notation_view(decl: &NotationDecl) -> Notation<'_> {
    Notation {
        name: &decl.name,
        public_id: decl.public.as_deref(),
        system_id: decl.system.as_deref(),
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

/// Whether `value` is lexically valid for an attribute of type `kind`
/// (`xmlValidateAttributeValue`; `Id` = `xmlValidateNameValue`, `IdRefs` =
/// `xmlValidateNamesValue`, `NmToken`/`NmTokens` = `xmlValidateNmtoken(s)Value`).
/// Names are separated by runs of U+0020, and NMTOKENS (and enumerations) may
/// also have leading blanks and trailing spaces. XML 1.0 fifth-edition names.
///
/// ```
/// use yaxml::valid::{AttributeKind, value_matches};
///
/// assert!(value_matches(AttributeKind::IdRefs, "a b  c"));
/// assert!(!value_matches(AttributeKind::IdRefs, "a b "));
/// assert!(value_matches(AttributeKind::NmTokens, "\t1 2 "));
/// assert!(!value_matches(AttributeKind::Id, "1a"));
/// ```
pub fn value_matches(kind: AttributeKind, value: &str) -> bool {
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

/// Checks `tree` against `dtd` as libxml2's `xmlValidateDtd`: the root,
/// every element and attribute, then IDREFs. Declaration-time checks of
/// `dtd` and the DOCTYPE name are not repeated.
pub fn validate<T: Tree>(tree: &T, dtd: &Dtd) -> Vec<Diagnostic<T::Id>> {
    engine::run(tree, dtd, false)
}

/// The element names that can be inserted between siblings `prev` and `next`
/// keeping their parent valid (`xmlValidGetValidElements`): each of the
/// parent declaration's [potential children](Element::potential_children),
/// at most 256, for which the parent, with a childless element of that name
/// in place of the nodes between `prev` and `next`, passes
/// `xmlValidateOneElement` (its attributes and content; descendants and
/// IDREFs are not checked). `prev` alone appends after it, `next` alone
/// prepends before it. `None` (libxml2's -1) when both are `None` or the
/// parent is not an element type `dtd` names.
///
/// ```
/// use yaxml::{OwnedDocument, ParseOptions, valid};
///
/// let input = "<!DOCTYPE r [<!ELEMENT r (a, b?, c)> <!ELEMENT a EMPTY> \
///     <!ELEMENT b EMPTY> <!ELEMENT c EMPTY>]><r><a/><c/></r>";
/// let document = OwnedDocument::parse(input)?;
/// let dtd = document.dtd().unwrap();
/// let a = document.document_element().unwrap().first_child().unwrap();
/// let c = a.next_sibling().unwrap();
/// let between = valid::valid_elements(&document, dtd, Some(a.id()), Some(c.id()));
/// assert_eq!(between, Some(vec!["b"]));
/// assert_eq!(valid::valid_elements(&document, dtd, Some(c.id()), None), Some(vec![]));
/// # Ok::<(), yaxml::Error>(())
/// ```
pub fn valid_elements<'d, T: Tree>(
    tree: &T,
    dtd: &'d Dtd,
    prev: Option<T::Id>,
    next: Option<T::Id>,
) -> Option<Vec<&'d str>> {
    engine::insertable(tree, dtd, prev, next)
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
