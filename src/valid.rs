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
use crate::dtd_catalog::{EntityBody, EntityDecl, EntityId, StoredDefault};
use crate::name::{name_char, name_start};
use crate::xpath::tree::{self, Item, Navigate, QName, Tree, attributes, preorder};
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
    pub(crate) notation_index: HashMap<String, usize>,
    pub(crate) notations: Vec<NotationDecl>,
    /// Entity declarations in declaration order, duplicates dropped.
    pub(crate) entities: Vec<EntityDecl>,
    /// General entities references find; a standalone document's external
    /// declarations are not bound.
    pub(crate) general: HashMap<String, EntityId>,
    pub(crate) parameter: HashMap<String, EntityId>,
    /// The first declaration of each general entity, bound or not, as
    /// ENTITY attribute validation finds it.
    pub(crate) declared: HashMap<String, EntityId>,
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
    pub(crate) index: HashMap<String, usize>,
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
            model: String::new(),
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
    fn attribute_for(&self, element: QName<'_>, attribute: &str) -> Option<&AttributeDecl> {
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
    pub name: &'a str,
    pub kind: EntityKind,
    /// The replacement text (libxml2's `content`): an internal entity's
    /// literal with character and parameter-entity references expanded and
    /// general references kept; an external parsed entity's text once a
    /// resolver has loaded it; an unparsed entity's notation, as libxml2
    /// stores it; otherwise `None`.
    pub content: Option<&'a str>,
    pub public_id: Option<&'a str>,
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
    pub name: &'a str,
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
    pub kind: AttributeKind,
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
    pub name: &'a str,
    pub public_id: Option<&'a str>,
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
            &decl.attributes[*decl.index.get(name)?],
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

/// Children for content models: entity references are transparent, as
/// libxml2 validates through `XML_ENTITY_REF_NODE`s.
fn children<T: Navigate>(tree: &T, id: T::Id) -> impl Iterator<Item = T::Id> + '_ {
    tree::flattened_children(tree, id)
}

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
        let dtd = self.dtd;
        let mut missing: Vec<_> = (dtd.declared.values())
            .filter_map(|id| match &dtd.entities[id.0].body {
                EntityBody::External(external) => external.notation.as_deref(),
                EntityBody::Internal(_) => None,
            })
            .filter(|notation| !dtd.notation_index.contains_key(*notation))
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
        match dtd.element_for(name) {
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
        let Some(decl) = dtd.attribute_for(element, attribute.qualified) else {
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
        if !self.dtd.notation_index.contains_key(value) {
            self.report(Code::UnknownNotation, Some(id), message);
        }
    }

    fn entity(&mut self, id: T::Id, attribute: &str, value: &str) {
        let entity = self
            .dtd
            .declared
            .get(value)
            .map(|id| &self.dtd.entities[id.0]);
        let code = match entity.map(|entity| &entity.body) {
            Some(EntityBody::External(external)) if external.notation.is_some() => return,
            Some(_) => Code::EntityType,
            None if predefined_entity(value).is_some() => Code::EntityType,
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
