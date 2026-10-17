//! Private effective DTD bindings and value processing. No document acceptance.

use std::collections::HashMap;

use crate::dtd::{
    AttributeTypeView, DefaultDeclarationView, DtdUnit, DtdUnitView, EntityDefinitionView,
    ReferenceKind,
};
use crate::escape::xml_char;
use crate::name::validate_name;

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
    CallerValueBytes,
    Limit(CatalogLimit),
    Allocation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CatalogError {
    pub(crate) kind: CatalogErrorKind,
    pub(crate) location: ValueLocation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DependencyKind {
    UnreadEntity,
    MissingReadDeclaration,
    ExternalEntity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Dependency {
    pub(crate) kind: DependencyKind,
    pub(crate) location: ValueLocation,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum NormalizedValue {
    Value(String),
    Unavailable(Dependency),
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
    Unavailable(Dependency),
    UnreadEntity,
    Undeclared,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DeclarationEffect {
    pub(crate) bindings_added: usize,
    pub(crate) unavailable_defaults: usize,
    pub(crate) unavailable: Option<Dependency>,
    pub(crate) ignored_after_unread_parameter: bool,
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
    Unavailable { fixed: bool, dependency: Dependency },
}

#[derive(Debug)]
enum StoredDefault {
    Required,
    Implied,
    Value { fixed: bool, value: NormalizedValue },
}

impl StoredDefault {
    fn view(&self) -> DefaultValue<'_> {
        match self {
            Self::Required => DefaultValue::Required,
            Self::Implied => DefaultValue::Implied,
            Self::Value {
                fixed,
                value: NormalizedValue::Value(value),
            } => DefaultValue::Value {
                fixed: *fixed,
                value,
            },
            Self::Value {
                fixed,
                value: NormalizedValue::Unavailable(dependency),
            } => DefaultValue::Unavailable {
                fixed: *fixed,
                dependency: *dependency,
            },
        }
    }

    fn bytes(&self) -> usize {
        match self {
            Self::Value {
                value: NormalizedValue::Value(value),
                ..
            } => value.len(),
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
    notation: Option<String>,
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
    Unavailable(Dependency),
}

struct Entity {
    body: EntityBody,
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
    unread_parameter: bool,
    failure: Option<CatalogError>,
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
            unread_parameter: false,
            failure: None,
        }
    }

    pub(crate) fn cap_work(&mut self, ceiling: usize) {
        self.limits.max_work = self.limits.max_work.min(ceiling);
    }

    pub(crate) fn usage(&self) -> CatalogUsage {
        self.usage
    }

    pub(crate) fn note_unread_parameter(&mut self) {
        self.unread_parameter = true;
    }

    fn may_be_unread(&self) -> bool {
        !self.standalone && (self.external_subset || self.unread_parameter)
    }

    fn lookup(&self, table: &HashMap<String, EntityId>, name: &str) -> EntityLookup<'_> {
        let Some(&id) = table.get(name) else {
            return if self.may_be_unread() {
                EntityLookup::UnreadEntity
            } else {
                EntityLookup::Undeclared
            };
        };
        match &self.entities[id.0].body {
            EntityBody::Internal(replacement) => EntityLookup::Internal { id, replacement },
            EntityBody::External(entity) => match entity.notation.as_deref() {
                Some(notation) => EntityLookup::Unparsed {
                    id,
                    identifier: entity.identifier(),
                    notation,
                },
                None => EntityLookup::ExternalUnavailable {
                    id,
                    identifier: entity.identifier(),
                },
            },
            EntityBody::Unavailable(dependency) => EntityLookup::Unavailable(*dependency),
        }
    }

    pub(crate) fn lookup_general(&self, name: &str) -> EntityLookup<'_> {
        match predefined(name) {
            Some(character) => EntityLookup::Predefined(character),
            None => self.lookup(&self.general, name),
        }
    }
    pub(crate) fn lookup_parameter(&self, name: &str) -> EntityLookup<'_> {
        self.lookup(&self.parameter, name)
    }

    pub(crate) fn declaration(&self, id: EntityId) -> Option<usize> {
        self.entities.get(id.0).map(|entity| entity.declaration)
    }

    pub(crate) fn replacement(&self, id: EntityId) -> Option<&str> {
        match &self.entities.get(id.0)?.body {
            EntityBody::Internal(value) => Some(value),
            _ => None,
        }
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
        match unit.view() {
            DtdUnitView::Header(header) => {
                self.external_subset |= header.external_id().is_some();
            }
            DtdUnitView::Entity(entity) => {
                if self.unread_parameter && !self.standalone {
                    effect.ignored_after_unread_parameter = true;
                    return Ok(effect);
                }
                let parameter = entity.is_parameter();
                let duplicate = if parameter {
                    self.parameter.contains_key(entity.name())
                } else {
                    self.general.contains_key(entity.name())
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
                        if let NormalizedValue::Unavailable(dependency) = &result {
                            effect.unavailable.get_or_insert(*dependency);
                        }
                        if !parameter && let Some(character) = predefined(entity.name()) {
                            let valid = match &result {
                                NormalizedValue::Value(value) => {
                                    valid_predefined(entity.name(), character, value)
                                }
                                _ => false,
                            };
                            if !valid {
                                return Err(error(
                                    CatalogErrorKind::InvalidPredefinedDeclaration,
                                    ValueLocation::Declaration {
                                        declaration,
                                        offset,
                                    },
                                ));
                            }
                        }
                        match result {
                            NormalizedValue::Value(value) => EntityBody::Internal(value),
                            NormalizedValue::Unavailable(dependency) => {
                                EntityBody::Unavailable(dependency)
                            }
                        }
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
                        let external = ExternalEntity {
                            public: copy_identifier(identifier.public, context.literals, location)?,
                            system: copy_identifier(identifier.system, context.literals, location)?,
                            notation: copy_optional(notation, location)?,
                        };
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
                if self.unread_parameter && !self.standalone {
                    effect.ignored_after_unread_parameter = true;
                    return Ok(effect);
                }
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
                    if let StoredDefault::Value {
                        value: NormalizedValue::Unavailable(dependency),
                        ..
                    } = &default
                    {
                        effect.unavailable_defaults += 1;
                        effect.unavailable.get_or_insert(*dependency);
                    }
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
            declaration: self.usage.declarations - 1,
        });
        table.insert(name, id);
        Ok(())
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

    pub(crate) fn normalize_attribute_value(
        &mut self,
        element: &str,
        name: &str,
        literal_content: &str,
        source: LiteralSource,
    ) -> Result<NormalizedValue, CatalogError> {
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
    ) -> Result<NormalizedValue, CatalogError> {
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
    ) -> Result<NormalizedValue, CatalogError> {
        let tokenized = self
            .element_index
            .get(element)
            .and_then(|index| {
                let attributes = &self.elements[*index];
                attributes
                    .index
                    .get(name)
                    .map(|index| attributes.ordered[*index].kind != AttributeKind::Cdata)
            })
            .unwrap_or(false);
        self.process_value(
            literal_content,
            0,
            source,
            ValueMode::Attribute { tokenized },
            None,
            ValueRequest {
                ceiling: caller_value_bytes,
                requested: requested_value_bytes,
            },
        )
    }

    fn process_value(
        &mut self,
        raw: &str,
        offset: usize,
        source: LiteralSource,
        mode: ValueMode,
        declaration: Option<usize>,
        request: ValueRequest<'_>,
    ) -> Result<NormalizedValue, CatalogError> {
        let may_be_unread = self.may_be_unread();
        let mut processor = ValueProcessor {
            general: &self.general,
            parameter: &self.parameter,
            entities: &self.entities,
            frames: &mut self.frames,
            limits: self.limits,
            request,
            usage: &mut self.usage,
            may_be_unread,
            raw,
            offset,
            declaration,
            source,
            mode,
        };
        let result = processor.process();
        processor.frames.clear();
        if matches!(mode, ValueMode::Entity)
            && matches!(&result, Ok(NormalizedValue::Unavailable(_)))
        {
            self.unread_parameter = true;
        }
        result
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
}

impl<'a> ValueRequest<'a> {
    fn unbounded(requested: &'a mut usize) -> Self {
        Self {
            ceiling: None,
            requested,
        }
    }
}

struct ValueProcessor<'a> {
    general: &'a HashMap<String, EntityId>,
    parameter: &'a HashMap<String, EntityId>,
    entities: &'a [Entity],
    frames: &'a mut Vec<ValueFrame>,
    limits: CatalogLimits,
    request: ValueRequest<'a>,
    usage: &'a mut CatalogUsage,
    may_be_unread: bool,
    raw: &'a str,
    offset: usize,
    declaration: Option<usize>,
    source: LiteralSource,
    mode: ValueMode,
}

impl ValueProcessor<'_> {
    fn input_location(&self, offset: usize) -> ValueLocation {
        match self.declaration {
            Some(declaration) => ValueLocation::Declaration {
                declaration,
                offset,
            },
            None => ValueLocation::Input(offset),
        }
    }

    fn process(&mut self) -> Result<NormalizedValue, CatalogError> {
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
        let mut output = String::new();
        let mut pending_space = false;
        while let Some(frame) = self.frames.last().copied() {
            let text = match frame.entity {
                None => self.raw,
                Some(id) => match &self.entities[id.0].body {
                    EntityBody::Internal(text) => text,
                    _ => unreachable!("only internal replacements become value frames"),
                },
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
                        let Some(&id) = table.get(name) else {
                            if is_entity || self.may_be_unread || frame.entity.is_some() {
                                return Ok(NormalizedValue::Unavailable(Dependency {
                                    kind: if self.may_be_unread {
                                        DependencyKind::UnreadEntity
                                    } else {
                                        DependencyKind::MissingReadDeclaration
                                    },
                                    location,
                                }));
                            }
                            return Err(error(CatalogErrorKind::EntityDeclared, location));
                        };
                        match &self.entities[id.0].body {
                            EntityBody::Unavailable(dependency) => {
                                return Ok(NormalizedValue::Unavailable(*dependency));
                            }
                            EntityBody::External(external) => {
                                if !is_entity {
                                    return Err(error(
                                        if external.notation.is_some() {
                                            CatalogErrorKind::UnparsedEntity
                                        } else {
                                            CatalogErrorKind::ExternalInAttribute
                                        },
                                        location,
                                    ));
                                }
                                return Ok(NormalizedValue::Unavailable(Dependency {
                                    kind: DependencyKind::ExternalEntity,
                                    location,
                                }));
                            }
                            EntityBody::Internal(replacement) => {
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
        Ok(NormalizedValue::Value(output))
    }

    fn emit(
        &mut self,
        output: &mut String,
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
                *pending_space = !output.is_empty();
                return Ok(());
            }
            if *pending_space {
                append(
                    output,
                    ' ',
                    self.limits.max_value_bytes,
                    self.request.ceiling,
                    self.request.requested,
                    location,
                )?;
                *pending_space = false;
            }
        }
        append(
            output,
            character,
            self.limits.max_value_bytes,
            self.request.ceiling,
            self.request.requested,
            location,
        )
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
