//! Private effective DTD bindings and value processing. No document acceptance.

use std::{borrow::Cow, collections::HashMap};

use crate::dtd::{
    AttributeTypeView, DefaultDeclarationView, DtdUnit, DtdUnitView, EntityDefinitionView,
    ReferenceKind,
};
use crate::escape::xml_char;
use crate::name::validate_name;
use crate::resolve::Loader;

mod external;

pub(crate) use crate::dtd::LiteralSource;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EntityId(usize);

impl EntityId {
    pub(crate) fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DeclarationContext {
    pub(crate) internal_subset: bool,
    pub(crate) literals: LiteralSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CatalogLimits {
    pub(crate) max_declarations: usize,
    pub(crate) max_bindings: usize,
    pub(crate) max_storage_bytes: usize,
    pub(crate) max_value_bytes: usize,
    pub(crate) max_expanded_bytes: usize,
    pub(crate) max_work: usize,
    pub(crate) max_references: usize,
    pub(crate) max_depth: usize,
}

impl Default for CatalogLimits {
    fn default() -> Self {
        Self {
            max_declarations: 16_384,
            max_bindings: 16_384,
            max_storage_bytes: 8 * 1024 * 1024,
            max_value_bytes: 1024 * 1024,
            max_expanded_bytes: 16 * 1024 * 1024,
            max_work: 64 * 1024 * 1024,
            max_references: 1024 * 1024,
            max_depth: 64,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CatalogUsage {
    pub(crate) declarations: usize,
    pub(crate) bindings: usize,
    pub(crate) storage_bytes: usize,
    pub(crate) expanded_bytes: usize,
    pub(crate) work: usize,
    pub(crate) references: usize,
    /// Replacement text entered and defaults copied, with a fixed cost each.
    pub(crate) amplification: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CatalogLimit {
    Declarations,
    Bindings,
    StorageBytes,
    ValueBytes,
    ExpandedBytes,
    Work,
    References,
    Depth,
}

/// Input offsets belong to the current operation, never implicitly physical input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ValueLocation {
    Input(usize),
    Declaration { declaration: usize, offset: usize },
    Replacement { entity: EntityId, offset: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CatalogErrorKind {
    EntityDeclared,
    ExternalInAttribute,
    UnparsedEntity,
    LessThanInAttribute,
    Recursion,
    ParameterInInternalDeclaration,
    InvalidReference,
    InvalidCharacter,
    InvalidPredefinedDeclaration,
    /// Namespaces 1.0 forbids colons in entity and notation names.
    Namespace,
    CallerValueBytes,
    Limit(CatalogLimit),
    Allocation,
    /// Malformed external DTD text.
    Grammar(crate::dtd::DtdErrorKind),
    /// A failed load or decode of an external resource.
    Xml(crate::ErrorKind),
    /// Internal: the value processor needs this external PE loaded first.
    Load(EntityId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CatalogError {
    pub(crate) kind: CatalogErrorKind,
    pub(crate) location: ValueLocation,
}

/// Unquoted identifier contents with literal EOL processing already applied.
/// Public-ID whitespace matching normalization and URI resolution are deferred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExternalIdentifier<'a> {
    pub(crate) public: Option<&'a str>,
    pub(crate) system: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EntityLookup<'a> {
    Predefined(char),
    Internal {
        id: EntityId,
        replacement: &'a str,
    },
    ExternalUnavailable {
        id: EntityId,
        identifier: ExternalIdentifier<'a>,
    },
    Unparsed {
        id: EntityId,
        identifier: ExternalIdentifier<'a>,
        notation: &'a str,
    },
    UnreadEntity,
    Undeclared,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DeclarationEffect {
    pub(crate) bindings_added: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AttributeKind {
    Cdata,
    Id,
    IdRef,
    IdRefs,
    Entity,
    Entities,
    NmToken,
    NmTokens,
    Notation,
    Enumeration,
}

impl AttributeKind {
    fn from_view(view: AttributeTypeView<'_>) -> Self {
        match view {
            AttributeTypeView::Cdata => Self::Cdata,
            AttributeTypeView::Id => Self::Id,
            AttributeTypeView::IdRef => Self::IdRef,
            AttributeTypeView::IdRefs => Self::IdRefs,
            AttributeTypeView::Entity => Self::Entity,
            AttributeTypeView::Entities => Self::Entities,
            AttributeTypeView::NmToken => Self::NmToken,
            AttributeTypeView::NmTokens => Self::NmTokens,
            AttributeTypeView::Notation(_) => Self::Notation,
            AttributeTypeView::Enumeration(_) => Self::Enumeration,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DefaultValue<'a> {
    Required,
    Implied,
    Value { fixed: bool, value: &'a str },
}

#[derive(Debug)]
enum StoredDefault {
    Required,
    Implied,
    Value { fixed: bool, value: String },
}

impl StoredDefault {
    fn view(&self) -> DefaultValue<'_> {
        match self {
            Self::Required => DefaultValue::Required,
            Self::Implied => DefaultValue::Implied,
            Self::Value { fixed, value } => DefaultValue::Value {
                fixed: *fixed,
                value,
            },
        }
    }

    fn bytes(&self) -> usize {
        match self {
            Self::Value { value, .. } => value.len(),
            _ => 0,
        }
    }
}

#[derive(Debug)]
struct Attribute {
    name: String,
    name_offset: usize,
    kind: AttributeKind,
    default: StoredDefault,
    declaration: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct AttributeBinding<'a>(&'a Attribute);

impl<'a> AttributeBinding<'a> {
    pub(crate) fn name(self) -> &'a str {
        &self.0.name
    }
    pub(crate) fn name_offset(self) -> usize {
        self.0.name_offset
    }
    #[cfg(test)]
    pub(crate) fn kind(self) -> AttributeKind {
        self.0.kind
    }
    pub(crate) fn default(self) -> DefaultValue<'a> {
        self.0.default.view()
    }
    pub(crate) fn declaration(self) -> usize {
        self.0.declaration
    }
}

#[derive(Default)]
struct ElementAttributes {
    ordered: Vec<Attribute>,
    index: HashMap<String, usize>,
}

struct ExternalEntity {
    public: Option<String>,
    system: Option<String>,
    /// The system identifier resolved against its declaration's base.
    uri: Option<String>,
    notation: Option<String>,
    /// Loaded replacement text; `loaded` records any attempt.
    text: Option<String>,
    loaded: bool,
}

impl ExternalEntity {
    fn identifier(&self) -> ExternalIdentifier<'_> {
        ExternalIdentifier {
            public: self.public.as_deref(),
            system: self.system.as_deref(),
        }
    }
}

enum EntityBody {
    Internal(String),
    External(ExternalEntity),
}

impl EntityBody {
    fn text(&self) -> Option<&str> {
        match self {
            Self::Internal(text) => Some(text),
            Self::External(external) => external.text.as_deref(),
        }
    }
}

struct Entity {
    body: EntityBody,
    parameter: bool,
    #[cfg_attr(not(test), expect(dead_code, reason = "inspected only by unit tests"))]
    declaration: usize,
}

#[derive(Clone, Copy)]
struct ValueFrame {
    entity: Option<EntityId>,
    cursor: usize,
}

pub(crate) struct DtdCatalog {
    general: HashMap<String, EntityId>,
    parameter: HashMap<String, EntityId>,
    entities: Vec<Entity>,
    element_index: HashMap<String, usize>,
    elements: Vec<ElementAttributes>,
    frames: Vec<ValueFrame>,
    limits: CatalogLimits,
    usage: CatalogUsage,
    standalone: bool,
    external_subset: bool,
    parameter_reference: bool,
    failure: Option<CatalogError>,
    amplification_ceiling: usize,
    loader: Option<Loader>,
    /// The DOCTYPE's external identifier, kept only for loading.
    subset: Option<ExternalEntity>,
}

impl DtdCatalog {
    pub(crate) fn new(limits: CatalogLimits, standalone: bool) -> Self {
        Self {
            general: HashMap::new(),
            parameter: HashMap::new(),
            entities: Vec::new(),
            element_index: HashMap::new(),
            elements: Vec::new(),
            frames: Vec::new(),
            limits,
            usage: CatalogUsage::default(),
            standalone,
            external_subset: false,
            parameter_reference: false,
            failure: None,
            amplification_ceiling: usize::MAX,
            loader: None,
            subset: None,
        }
    }

    pub(crate) fn cap_work(&mut self, ceiling: usize) {
        self.limits.max_work = self.limits.max_work.min(ceiling);
    }

    pub(crate) fn set_amplification_ceiling(&mut self, ceiling: usize) {
        self.amplification_ceiling = ceiling;
    }

    /// Bytes returned by the resolver; libxml2 counts them as consumed input.
    pub(crate) fn external_bytes(&self) -> usize {
        self.loader.as_ref().map_or(0, Loader::loaded)
    }

    /// Charges copied default-attribute text as libxml2 charges `expandedSize`.
    pub(crate) fn charge_default(&mut self, bytes: usize) -> Result<(), CatalogError> {
        increment(
            &mut self.usage.amplification,
            bytes,
            self.amplification_ceiling,
            CatalogLimit::ExpandedBytes,
            ValueLocation::Input(0),
        )
    }

    pub(crate) fn usage(&self) -> CatalogUsage {
        self.usage
    }

    /// Any parameter reference relaxes Entity Declared, as in libxml2's hasPErefs.
    pub(crate) fn note_parameter_reference(&mut self) {
        self.parameter_reference = true;
    }

    fn may_be_unread(&self) -> bool {
        !self.standalone && (self.external_subset || self.parameter_reference)
    }

    /// Loaded external general entities resolve like internal ones; external
    /// parameter entities are always included by `include_parameter`.
    fn lookup(
        &self,
        table: &HashMap<String, EntityId>,
        name: &str,
        parameter: bool,
    ) -> EntityLookup<'_> {
        let Some(&id) = table.get(name) else {
            return if self.may_be_unread() {
                EntityLookup::UnreadEntity
            } else {
                EntityLookup::Undeclared
            };
        };
        match &self.entities[id.0].body {
            EntityBody::Internal(replacement) => EntityLookup::Internal { id, replacement },
            EntityBody::External(entity) => match (entity.notation.as_deref(), &entity.text) {
                (Some(notation), _) => EntityLookup::Unparsed {
                    id,
                    identifier: entity.identifier(),
                    notation,
                },
                (None, Some(replacement)) if !parameter => {
                    EntityLookup::Internal { id, replacement }
                }
                (None, _) => EntityLookup::ExternalUnavailable {
                    id,
                    identifier: entity.identifier(),
                },
            },
        }
    }

    pub(crate) fn lookup_general(&self, name: &str) -> EntityLookup<'_> {
        match predefined(name) {
            Some(character) => EntityLookup::Predefined(character),
            None => self.lookup(&self.general, name, false),
        }
    }
    pub(crate) fn lookup_parameter(&self, name: &str) -> EntityLookup<'_> {
        self.lookup(&self.parameter, name, true)
    }

    #[cfg(test)]
    pub(crate) fn declaration(&self, id: EntityId) -> Option<usize> {
        self.entities.get(id.0).map(|entity| entity.declaration)
    }

    pub(crate) fn replacement(&self, id: EntityId) -> Option<&str> {
        self.entities.get(id.0)?.body.text()
    }

    /// `(element, attribute)` qualified names declared with type `ID`.
    pub(crate) fn id_attributes(&self) -> impl Iterator<Item = (&str, &str)> {
        self.element_index.iter().flat_map(|(element, index)| {
            self.elements[*index]
                .ordered
                .iter()
                .filter(|attribute| attribute.kind == AttributeKind::Id)
                .map(move |attribute| (element.as_str(), attribute.name.as_str()))
        })
    }

    pub(crate) fn attributes(&self, element: &str) -> impl Iterator<Item = AttributeBinding<'_>> {
        self.element_index
            .get(element)
            .map(|index| self.elements[*index].ordered.as_slice())
            .unwrap_or(&[])
            .iter()
            .map(AttributeBinding)
    }

    pub(crate) fn apply(
        &mut self,
        unit: &DtdUnit<'_>,
        context: DeclarationContext,
    ) -> Result<DeclarationEffect, CatalogError> {
        self.remember(|catalog| catalog.apply_inner(unit, context))
    }

    fn apply_inner(
        &mut self,
        unit: &DtdUnit<'_>,
        context: DeclarationContext,
    ) -> Result<DeclarationEffect, CatalogError> {
        let declaration = self.usage.declarations;
        let location = ValueLocation::Declaration {
            declaration,
            offset: 0,
        };
        increment(
            &mut self.usage.declarations,
            1,
            self.limits.max_declarations,
            CatalogLimit::Declarations,
            location,
        )?;
        increment(
            &mut self.usage.work,
            unit.raw().len(),
            self.limits.max_work,
            CatalogLimit::Work,
            location,
        )?;
        if context.internal_subset
            && let Some((offset, _)) = unit
                .reference_positions()
                .find(|(_, reference)| reference.kind == ReferenceKind::Parameter)
        {
            return Err(error(
                CatalogErrorKind::ParameterInInternalDeclaration,
                ValueLocation::Declaration {
                    declaration,
                    offset,
                },
            ));
        }
        let mut effect = DeclarationEffect::default();
        let colon = match unit.view() {
            DtdUnitView::Entity(entity) => entity.name().contains(':'),
            DtdUnitView::Notation(notation) => notation.name().contains(':'),
            _ => false,
        };
        if colon {
            return Err(error(CatalogErrorKind::Namespace, location));
        }
        match unit.view() {
            DtdUnitView::Header(header) => {
                self.external_subset |= header.external_id().is_some();
                if let Some(identifier) = header.external_id() {
                    self.subset = Some(self.external_entity(identifier, None, context, location)?);
                }
            }
            DtdUnitView::Entity(entity) => {
                let parameter = entity.is_parameter();
                // WFC Entity Declared: a standalone document cannot use general
                // entities from external markup, so they are never bound.
                let duplicate = if parameter {
                    self.parameter.contains_key(entity.name())
                } else {
                    self.general.contains_key(entity.name())
                        || (self.standalone && !context.internal_subset)
                };
                let body = match entity.definition() {
                    EntityDefinitionView::Internal { quoted_literal } => {
                        let offset = entity
                            .quoted_literal_offset()
                            .expect("internal entity literal has a token")
                            + 1;
                        let mut ignored = 0;
                        let result = self.process_value(
                            &quoted_literal[1..quoted_literal.len() - 1],
                            offset,
                            context.literals,
                            ValueMode::Entity,
                            Some(declaration),
                            ValueRequest::unbounded(&mut ignored),
                        )?;
                        if !parameter
                            && let Some(character) = predefined(entity.name())
                            && !valid_predefined(entity.name(), character, &result)
                        {
                            return Err(error(
                                CatalogErrorKind::InvalidPredefinedDeclaration,
                                ValueLocation::Declaration {
                                    declaration,
                                    offset,
                                },
                            ));
                        }
                        EntityBody::Internal(result)
                    }
                    EntityDefinitionView::External {
                        identifier,
                        notation,
                    } => {
                        if !parameter && predefined(entity.name()).is_some() {
                            return Err(error(
                                CatalogErrorKind::InvalidPredefinedDeclaration,
                                location,
                            ));
                        }
                        if duplicate {
                            return Ok(effect);
                        }
                        let bytes = checked_sum(
                            &[
                                entity.name().len(),
                                identifier
                                    .public
                                    .map_or(0, |raw| identifier_length(raw, context.literals)),
                                identifier
                                    .system
                                    .map_or(0, |raw| identifier_length(raw, context.literals)),
                                notation.map_or(0, str::len),
                            ],
                            location,
                        )?;
                        self.store_bytes(bytes, location)?;
                        self.add_binding(location)?;
                        let external =
                            self.external_entity(identifier, notation, context, location)?;
                        self.insert_entity(
                            entity.name(),
                            parameter,
                            EntityBody::External(external),
                            location,
                        )?;
                        effect.bindings_added = 1;
                        return Ok(effect);
                    }
                };
                if !duplicate {
                    let bytes = match &body {
                        EntityBody::Internal(value) => value.len(),
                        _ => 0,
                    };
                    self.store_bytes(
                        checked_sum(&[entity.name().len(), bytes], location)?,
                        location,
                    )?;
                    self.add_binding(location)?;
                    self.insert_entity(entity.name(), parameter, body, location)?;
                    effect.bindings_added = 1;
                }
            }
            DtdUnitView::AttributeList(list) => {
                for definition in list.definitions() {
                    let kind = AttributeKind::from_view(definition.attribute_type());
                    let default = match definition.default() {
                        DefaultDeclarationView::Required => StoredDefault::Required,
                        DefaultDeclarationView::Implied => StoredDefault::Implied,
                        DefaultDeclarationView::Value {
                            fixed,
                            quoted_literal,
                        } => {
                            let offset = definition
                                .default_literal_offset()
                                .expect("default literal has a token")
                                + 1;
                            let mut ignored = 0;
                            let value = self.process_value(
                                &quoted_literal[1..quoted_literal.len() - 1],
                                offset,
                                context.literals,
                                ValueMode::Attribute {
                                    tokenized: kind != AttributeKind::Cdata,
                                },
                                Some(declaration),
                                ValueRequest::unbounded(&mut ignored),
                            )?;
                            StoredDefault::Value { fixed, value }
                        }
                    };
                    let existing = self.element_index.get(list.element_name()).copied();
                    if existing.is_some_and(|index| {
                        self.elements[index].index.contains_key(definition.name())
                    }) {
                        continue;
                    }
                    self.add_binding(location)?;
                    let element_bytes = if existing.is_none() {
                        list.element_name().len()
                    } else {
                        0
                    };
                    self.store_bytes(
                        checked_sum(
                            &[
                                element_bytes,
                                definition.name().len(),
                                definition.name().len(),
                                default.bytes(),
                            ],
                            location,
                        )?,
                        location,
                    )?;
                    let element = match existing {
                        Some(index) => index,
                        None => {
                            self.element_index
                                .try_reserve(1)
                                .map_err(|_| error(CatalogErrorKind::Allocation, location))?;
                            self.elements
                                .try_reserve(1)
                                .map_err(|_| error(CatalogErrorKind::Allocation, location))?;
                            let name = copy_string(list.element_name(), location)?;
                            let index = self.elements.len();
                            self.elements.push(ElementAttributes::default());
                            self.element_index.insert(name, index);
                            index
                        }
                    };
                    let attributes = &mut self.elements[element];
                    attributes
                        .index
                        .try_reserve(1)
                        .map_err(|_| error(CatalogErrorKind::Allocation, location))?;
                    attributes
                        .ordered
                        .try_reserve(1)
                        .map_err(|_| error(CatalogErrorKind::Allocation, location))?;
                    let key = copy_string(definition.name(), location)?;
                    let name = copy_string(definition.name(), location)?;
                    let index = attributes.ordered.len();
                    attributes.ordered.push(Attribute {
                        name,
                        name_offset: definition.name_offset(),
                        kind,
                        default,
                        declaration: self.usage.declarations - 1,
                    });
                    attributes.index.insert(key, index);
                    effect.bindings_added += 1;
                }
            }
            _ => {}
        }
        Ok(effect)
    }

    fn insert_entity(
        &mut self,
        name: &str,
        parameter: bool,
        body: EntityBody,
        location: ValueLocation,
    ) -> Result<(), CatalogError> {
        let table = if parameter {
            &mut self.parameter
        } else {
            &mut self.general
        };
        table
            .try_reserve(1)
            .map_err(|_| error(CatalogErrorKind::Allocation, location))?;
        self.entities
            .try_reserve(1)
            .map_err(|_| error(CatalogErrorKind::Allocation, location))?;
        let name = copy_string(name, location)?;
        let id = EntityId(self.entities.len());
        self.entities.push(Entity {
            body,
            parameter,
            declaration: self.usage.declarations - 1,
        });
        table.insert(name, id);
        Ok(())
    }

    fn external_entity(
        &self,
        identifier: crate::dtd::ExternalIdView<'_>,
        notation: Option<&str>,
        context: DeclarationContext,
        location: ValueLocation,
    ) -> Result<ExternalEntity, CatalogError> {
        let system = copy_identifier(identifier.system, context.literals, location)?;
        Ok(ExternalEntity {
            public: copy_identifier(identifier.public, context.literals, location)?,
            uri: self
                .loader
                .as_ref()
                .zip(system.as_deref())
                .map(|(loader, system)| loader.uri(system)),
            system,
            notation: copy_optional(notation, location)?,
            text: None,
            loaded: false,
        })
    }

    fn store_bytes(&mut self, bytes: usize, location: ValueLocation) -> Result<(), CatalogError> {
        increment(
            &mut self.usage.storage_bytes,
            bytes,
            self.limits.max_storage_bytes,
            CatalogLimit::StorageBytes,
            location,
        )
    }

    fn add_binding(&mut self, location: ValueLocation) -> Result<(), CatalogError> {
        increment(
            &mut self.usage.bindings,
            1,
            self.limits.max_bindings,
            CatalogLimit::Bindings,
            location,
        )
    }

    #[cfg(test)]
    pub(crate) fn normalize_attribute_value(
        &mut self,
        element: &str,
        name: &str,
        literal_content: &str,
        source: LiteralSource,
    ) -> Result<String, CatalogError> {
        self.remember(|catalog| {
            let mut ignored = 0;
            catalog.normalize_attribute_value_inner(
                element,
                name,
                literal_content,
                source,
                None,
                &mut ignored,
            )
        })
    }

    pub(crate) fn normalize_attribute_value_bounded(
        &mut self,
        element: &str,
        name: &str,
        literal_content: &str,
        source: LiteralSource,
        caller_value_bytes: usize,
        requested_value_bytes: &mut usize,
    ) -> Result<String, CatalogError> {
        *requested_value_bytes = 0;
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = self.normalize_attribute_value_inner(
            element,
            name,
            literal_content,
            source,
            Some(caller_value_bytes),
            requested_value_bytes,
        );
        if let Err(error) = result
            && error.kind != CatalogErrorKind::CallerValueBytes
        {
            self.failure = Some(error);
        }
        result
    }

    fn normalize_attribute_value_inner(
        &mut self,
        element: &str,
        name: &str,
        literal_content: &str,
        source: LiteralSource,
        caller_value_bytes: Option<usize>,
        requested_value_bytes: &mut usize,
    ) -> Result<String, CatalogError> {
        let tokenized = self.tokenized_attribute(element, name);
        self.process_value(
            literal_content,
            0,
            source,
            ValueMode::Attribute { tokenized },
            None,
            ValueRequest {
                ceiling: caller_value_bytes,
                requested: requested_value_bytes,
                borrow_physical: false,
            },
        )
    }

    fn tokenized_attribute(&self, element: &str, name: &str) -> bool {
        self.element_index
            .get(element)
            .and_then(|index| {
                let attributes = &self.elements[*index];
                attributes
                    .index
                    .get(name)
                    .map(|index| attributes.ordered[*index].kind != AttributeKind::Cdata)
            })
            .unwrap_or(false)
    }

    /// Only the caller's physical literal can back the returned borrow.
    pub(crate) fn normalize_attribute_borrowed_bounded<'raw>(
        &mut self,
        element: &str,
        name: &str,
        literal_content: &'raw str,
        caller_value_bytes: usize,
        requested_value_bytes: &mut usize,
    ) -> Result<Cow<'raw, str>, CatalogError> {
        *requested_value_bytes = 0;
        if let Some(error) = self.failure {
            return Err(error);
        }
        let tokenized = self.tokenized_attribute(element, name);
        let result = self.process_value_cow(
            literal_content,
            0,
            LiteralSource::Physical,
            ValueMode::Attribute { tokenized },
            None,
            ValueRequest {
                ceiling: Some(caller_value_bytes),
                requested: requested_value_bytes,
                borrow_physical: true,
            },
        );
        if let Err(error) = result
            && error.kind != CatalogErrorKind::CallerValueBytes
        {
            self.failure = Some(error);
        }
        result
    }

    fn process_value(
        &mut self,
        raw: &str,
        offset: usize,
        source: LiteralSource,
        mode: ValueMode,
        declaration: Option<usize>,
        request: ValueRequest<'_>,
    ) -> Result<String, CatalogError> {
        match self.process_value_cow(raw, offset, source, mode, declaration, request)? {
            Cow::Owned(value) => Ok(value),
            Cow::Borrowed(_) => unreachable!("owned value policy cannot borrow"),
        }
    }

    fn process_value_cow<'raw>(
        &mut self,
        raw: &'raw str,
        offset: usize,
        source: LiteralSource,
        mode: ValueMode,
        declaration: Option<usize>,
        request: ValueRequest<'_>,
    ) -> Result<Cow<'raw, str>, CatalogError> {
        // EntityValue admits `%` only to start a PE reference; it sets hasPErefs first.
        if matches!(mode, ValueMode::Entity) && raw.contains('%') {
            self.parameter_reference = true;
        }
        let may_be_unread = self.may_be_unread();
        let (usage, requested) = (self.usage, *request.requested);
        loop {
            let mut processor = ValueProcessor {
                general: &self.general,
                parameter: &self.parameter,
                entities: &self.entities,
                frames: &mut self.frames,
                limits: self.limits,
                amplification_ceiling: self.amplification_ceiling,
                request: ValueRequest {
                    requested: &mut *request.requested,
                    ..request
                },
                usage: &mut self.usage,
                may_be_unread,
                loadable: self.loader.is_some(),
                raw,
                offset,
                declaration,
                source,
                mode,
            };
            let result = processor.process();
            processor.frames.clear();
            // An external PE inside an entity value is loaded, then the value restarts.
            let Err(CatalogError {
                kind: CatalogErrorKind::Load(id),
                ..
            }) = result
            else {
                return result;
            };
            (self.usage, *request.requested) = (usage, requested);
            self.load(id)?;
        }
    }

    fn remember<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, CatalogError>,
    ) -> Result<T, CatalogError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = operation(self);
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }
}

#[derive(Clone, Copy)]
enum ValueMode {
    Entity,
    Attribute { tokenized: bool },
}

struct ValueRequest<'a> {
    ceiling: Option<usize>,
    requested: &'a mut usize,
    borrow_physical: bool,
}

impl<'a> ValueRequest<'a> {
    fn unbounded(requested: &'a mut usize) -> Self {
        Self {
            ceiling: None,
            requested,
            borrow_physical: false,
        }
    }
}

struct ValueProcessor<'a, 'raw> {
    general: &'a HashMap<String, EntityId>,
    parameter: &'a HashMap<String, EntityId>,
    entities: &'a [Entity],
    frames: &'a mut Vec<ValueFrame>,
    limits: CatalogLimits,
    amplification_ceiling: usize,
    request: ValueRequest<'a>,
    usage: &'a mut CatalogUsage,
    may_be_unread: bool,
    loadable: bool,
    raw: &'raw str,
    offset: usize,
    declaration: Option<usize>,
    source: LiteralSource,
    mode: ValueMode,
}

impl<'raw> ValueProcessor<'_, 'raw> {
    fn input_location(&self, offset: usize) -> ValueLocation {
        match self.declaration {
            Some(declaration) => ValueLocation::Declaration {
                declaration,
                offset,
            },
            None => ValueLocation::Input(offset),
        }
    }

    fn process(&mut self) -> Result<Cow<'raw, str>, CatalogError> {
        let root = self.input_location(self.offset);
        increment(
            &mut self.usage.work,
            self.raw.len(),
            self.limits.max_work,
            CatalogLimit::Work,
            root,
        )?;
        self.frames
            .try_reserve(1)
            .map_err(|_| error(CatalogErrorKind::Allocation, root))?;
        self.frames.push(ValueFrame {
            entity: None,
            cursor: 0,
        });
        let mut output = if self.request.borrow_physical {
            ValueOutput::Matching {
                raw: self.raw,
                matched: 0,
            }
        } else {
            ValueOutput::Owned(String::new())
        };
        let mut pending_space = false;
        while let Some(frame) = self.frames.last().copied() {
            let text = match frame.entity {
                None => self.raw,
                Some(id) => self.entities[id.0]
                    .body
                    .text()
                    .expect("only replacement texts become value frames"),
            };
            if frame.cursor == text.len() {
                self.frames.pop();
                continue;
            }
            let location = match frame.entity {
                Some(entity) => ValueLocation::Replacement {
                    entity,
                    offset: frame.cursor,
                },
                None => self.input_location(self.offset + frame.cursor),
            };
            let remaining = &text[frame.cursor..];
            let character = remaining.chars().next().expect("nonempty UTF-8 frame");
            let mut consumed = character.len_utf8();
            if !xml_char(character) {
                return Err(error(CatalogErrorKind::InvalidCharacter, location));
            }
            let is_entity = matches!(self.mode, ValueMode::Entity);
            if character == '&' || (is_entity && character == '%') {
                let reference = parse_reference(remaining, character, location)?;
                consumed = reference.spelling.len();
                increment(
                    &mut self.usage.references,
                    1,
                    self.limits.max_references,
                    CatalogLimit::References,
                    location,
                )?;
                self.frames.last_mut().expect("active value frame").cursor += consumed;
                match reference.value {
                    Reference::Character(character) => {
                        self.emit(&mut output, &mut pending_space, character, location)?
                    }
                    Reference::Named(_) if is_entity && character == '&' => {
                        // Every GE, including predefined names, is bypassed in EntityValue.
                        for character in reference.spelling.chars() {
                            self.emit(&mut output, &mut pending_space, character, location)?;
                        }
                    }
                    Reference::Named(name) => {
                        if !is_entity && let Some(character) = predefined(name) {
                            self.emit(&mut output, &mut pending_space, character, location)?;
                            continue;
                        }
                        let table = if is_entity {
                            self.parameter
                        } else {
                            self.general
                        };
                        // As libxml2 does, an undeclared entity that unread markup
                        // could declare, or an unloaded external PE, expands empty.
                        let Some(&id) = table.get(name) else {
                            if self.may_be_unread {
                                continue;
                            }
                            return Err(error(CatalogErrorKind::EntityDeclared, location));
                        };
                        match &self.entities[id.0].body {
                            EntityBody::External(external) if !is_entity => {
                                return Err(error(
                                    if external.notation.is_some() {
                                        CatalogErrorKind::UnparsedEntity
                                    } else {
                                        CatalogErrorKind::ExternalInAttribute
                                    },
                                    location,
                                ));
                            }
                            EntityBody::External(external) if external.text.is_none() => {
                                if self.loadable && !external.loaded {
                                    return Err(error(CatalogErrorKind::Load(id), location));
                                }
                                continue;
                            }
                            body => {
                                let replacement = body.text().expect("checked replacement");
                                increment(
                                    &mut self.usage.work,
                                    self.frames.len(),
                                    self.limits.max_work,
                                    CatalogLimit::Work,
                                    location,
                                )?;
                                if self.frames.iter().any(|frame| frame.entity == Some(id)) {
                                    return Err(error(CatalogErrorKind::Recursion, location));
                                }
                                if self.frames.len() > self.limits.max_depth {
                                    return Err(error(
                                        CatalogErrorKind::Limit(CatalogLimit::Depth),
                                        location,
                                    ));
                                }
                                increment(
                                    &mut self.usage.work,
                                    replacement.len(),
                                    self.limits.max_work,
                                    CatalogLimit::Work,
                                    location,
                                )?;
                                // libxml2 charges each entered replacement plus a fixed cost.
                                increment(
                                    &mut self.usage.amplification,
                                    replacement
                                        .len()
                                        .saturating_add(crate::dtd_controller::ENTITY_FIXED_COST),
                                    self.amplification_ceiling,
                                    CatalogLimit::ExpandedBytes,
                                    location,
                                )?;
                                self.frames
                                    .try_reserve(1)
                                    .map_err(|_| error(CatalogErrorKind::Allocation, location))?;
                                self.frames.push(ValueFrame {
                                    entity: Some(id),
                                    cursor: 0,
                                });
                            }
                        }
                    }
                }
                continue;
            }
            let physical = frame.entity.is_none() && self.source == LiteralSource::Physical;
            let character = if character == '\r' && physical {
                if remaining.as_bytes().get(1) == Some(&b'\n') {
                    consumed += 1;
                }
                '\n'
            } else {
                character
            };
            if !is_entity && character == '<' {
                return Err(error(CatalogErrorKind::LessThanInAttribute, location));
            }
            let character = if !is_entity && matches!(character, '\r' | '\n' | '\t') {
                ' '
            } else {
                character
            };
            self.frames.last_mut().expect("active value frame").cursor += consumed;
            self.emit(&mut output, &mut pending_space, character, location)?;
        }
        if matches!(&output, ValueOutput::Matching { raw, matched } if *matched != raw.len()) {
            self.materialize(&mut output, root)?;
        }
        Ok(match output {
            ValueOutput::Owned(value) => Cow::Owned(value),
            ValueOutput::Matching { raw, .. } => Cow::Borrowed(raw),
        })
    }

    fn emit(
        &mut self,
        output: &mut ValueOutput<'raw>,
        pending_space: &mut bool,
        character: char,
        location: ValueLocation,
    ) -> Result<(), CatalogError> {
        increment(
            &mut self.usage.expanded_bytes,
            character.len_utf8(),
            self.limits.max_expanded_bytes,
            CatalogLimit::ExpandedBytes,
            location,
        )?;
        if matches!(self.mode, ValueMode::Attribute { tokenized: true }) {
            if character == ' ' {
                *pending_space = output.len() != 0;
                return Ok(());
            }
            if *pending_space {
                self.append_output(output, ' ', location)?;
                *pending_space = false;
            }
        }
        self.append_output(output, character, location)
    }

    fn append_output(
        &mut self,
        output: &mut ValueOutput<'raw>,
        character: char,
        location: ValueLocation,
    ) -> Result<(), CatalogError> {
        if let ValueOutput::Matching { raw, matched } = output {
            let needed = matched
                .checked_add(character.len_utf8())
                .filter(|length| *length <= self.limits.max_value_bytes)
                .ok_or_else(|| {
                    error(CatalogErrorKind::Limit(CatalogLimit::ValueBytes), location)
                })?;
            increment(
                &mut self.usage.work,
                character.len_utf8(),
                self.limits.max_work,
                CatalogLimit::Work,
                location,
            )?;
            let mut encoded = [0; 4];
            if raw.as_bytes().get(*matched..needed)
                == Some(character.encode_utf8(&mut encoded).as_bytes())
            {
                *matched = needed;
                return Ok(());
            }
            self.materialize(output, self.input_location(self.offset))?;
        }
        let ValueOutput::Owned(output) = output else {
            unreachable!("materialized output")
        };
        append(
            output,
            character,
            self.limits.max_value_bytes,
            self.request.ceiling,
            self.request.requested,
            location,
        )
    }

    fn materialize(
        &mut self,
        output: &mut ValueOutput<'raw>,
        location: ValueLocation,
    ) -> Result<(), CatalogError> {
        let ValueOutput::Matching { raw, matched } = output else {
            return Ok(());
        };
        increment(
            &mut self.usage.work,
            *matched,
            self.limits.max_work,
            CatalogLimit::Work,
            location,
        )?;
        let mut owned = String::new();
        if *matched != 0 {
            let target = (*matched).max(64).min(self.limits.max_value_bytes);
            if let Some(ceiling) = self.request.ceiling {
                *self.request.requested = self
                    .request
                    .requested
                    .checked_add(target)
                    .filter(|total| *total <= ceiling)
                    .ok_or_else(|| error(CatalogErrorKind::CallerValueBytes, location))?;
            }
            owned
                .try_reserve_exact(target)
                .map_err(|_| error(CatalogErrorKind::Allocation, location))?;
            owned.push_str(&raw[..*matched]);
        }
        *output = ValueOutput::Owned(owned);
        Ok(())
    }
}

enum ValueOutput<'raw> {
    Owned(String),
    Matching { raw: &'raw str, matched: usize },
}
impl ValueOutput<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Owned(value) => value.len(),
            Self::Matching { matched, .. } => *matched,
        }
    }
}

struct ParsedReference<'a> {
    spelling: &'a str,
    value: Reference<'a>,
}
enum Reference<'a> {
    Character(char),
    Named(&'a str),
}

fn parse_reference(
    raw: &str,
    delimiter: char,
    location: ValueLocation,
) -> Result<ParsedReference<'_>, CatalogError> {
    let invalid = || error(CatalogErrorKind::InvalidReference, location);
    if !raw.starts_with(delimiter) {
        return Err(invalid());
    }
    let end = raw.find(';').ok_or_else(invalid)?;
    let body = &raw[1..end];
    let value = if delimiter == '&' && body.starts_with('#') {
        let (digits, radix) = match body.strip_prefix("#x") {
            Some(digits) => (digits, 16),
            None => (&body[1..], 10),
        };
        if digits.is_empty()
            || !digits.bytes().all(|byte| {
                if radix == 16 {
                    byte.is_ascii_hexdigit()
                } else {
                    byte.is_ascii_digit()
                }
            })
        {
            return Err(invalid());
        }
        let character = u32::from_str_radix(digits, radix)
            .ok()
            .and_then(char::from_u32)
            .filter(|character| xml_char(*character))
            .ok_or_else(invalid)?;
        Reference::Character(character)
    } else {
        validate_name(body, 0, true).map_err(|_| invalid())?;
        Reference::Named(body)
    };
    Ok(ParsedReference {
        spelling: &raw[..=end],
        value,
    })
}

fn predefined(name: &str) -> Option<char> {
    match name {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "amp" => Some('&'),
        "apos" => Some('\''),
        "quot" => Some('"'),
        _ => None,
    }
}

fn valid_predefined(name: &str, character: char, replacement: &str) -> bool {
    if !matches!(name, "amp" | "lt")
        && replacement.len() == character.len_utf8()
        && replacement.starts_with(character)
    {
        return true;
    }
    let Ok(reference) = parse_reference(replacement, '&', ValueLocation::Input(0)) else {
        return false;
    };
    reference.spelling.len() == replacement.len()
        && matches!(reference.value,Reference::Character(found) if found==character)
}

fn append(
    output: &mut String,
    character: char,
    ceiling: usize,
    caller_ceiling: Option<usize>,
    requested: &mut usize,
    location: ValueLocation,
) -> Result<(), CatalogError> {
    let needed = output
        .len()
        .checked_add(character.len_utf8())
        .filter(|length| *length <= ceiling)
        .ok_or_else(|| error(CatalogErrorKind::Limit(CatalogLimit::ValueBytes), location))?;
    if needed > output.capacity() {
        let target = output
            .capacity()
            .saturating_mul(2)
            .max(64)
            .max(needed)
            .min(ceiling);
        if let Some(caller_ceiling) = caller_ceiling {
            *requested = requested
                .checked_add(target)
                .filter(|total| *total <= caller_ceiling)
                .ok_or_else(|| error(CatalogErrorKind::CallerValueBytes, location))?;
        }
        output
            .try_reserve_exact(target - output.len())
            .map_err(|_| error(CatalogErrorKind::Allocation, location))?;
    }
    output.push(character);
    Ok(())
}

fn error(kind: CatalogErrorKind, location: ValueLocation) -> CatalogError {
    CatalogError { kind, location }
}

fn increment(
    value: &mut usize,
    amount: usize,
    ceiling: usize,
    limit: CatalogLimit,
    location: ValueLocation,
) -> Result<(), CatalogError> {
    *value = value
        .checked_add(amount)
        .filter(|total| *total <= ceiling)
        .ok_or_else(|| error(CatalogErrorKind::Limit(limit), location))?;
    Ok(())
}

fn checked_sum(values: &[usize], location: ValueLocation) -> Result<usize, CatalogError> {
    values.iter().try_fold(0usize, |sum, value| {
        sum.checked_add(*value).ok_or_else(|| {
            error(
                CatalogErrorKind::Limit(CatalogLimit::StorageBytes),
                location,
            )
        })
    })
}

fn copy_string(raw: &str, location: ValueLocation) -> Result<String, CatalogError> {
    let mut value = String::new();
    value
        .try_reserve_exact(raw.len())
        .map_err(|_| error(CatalogErrorKind::Allocation, location))?;
    value.push_str(raw);
    Ok(value)
}

/// The lexical unit validates the quote delimiters. No reference decoding occurs.
fn identifier_length(quoted: &str, source: LiteralSource) -> usize {
    let contents = &quoted.as_bytes()[1..quoted.len() - 1];
    contents.len()
        - if source == LiteralSource::Physical {
            contents
                .windows(2)
                .filter(|bytes| *bytes == b"\r\n")
                .count()
        } else {
            0
        }
}

fn copy_identifier(
    quoted: Option<&str>,
    source: LiteralSource,
    location: ValueLocation,
) -> Result<Option<String>, CatalogError> {
    let Some(quoted) = quoted else {
        return Ok(None);
    };
    let mut output = String::new();
    output
        .try_reserve_exact(identifier_length(quoted, source))
        .map_err(|_| error(CatalogErrorKind::Allocation, location))?;
    let mut characters = quoted[1..quoted.len() - 1].chars().peekable();
    while let Some(character) = characters.next() {
        if source == LiteralSource::Physical && character == '\r' {
            if characters.peek() == Some(&'\n') {
                characters.next();
            }
            output.push('\n');
        } else {
            output.push(character);
        }
    }
    Ok(Some(output))
}

fn copy_optional(
    raw: Option<&str>,
    location: ValueLocation,
) -> Result<Option<String>, CatalogError> {
    raw.map(|value| copy_string(value, location)).transpose()
}

#[cfg(test)]
#[path = "../tests/internal/dtd_catalog_author.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/internal/dtd_catalog_spec.rs"]
mod spec_tests;
