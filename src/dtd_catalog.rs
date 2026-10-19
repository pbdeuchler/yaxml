//! Effective DTD bindings and value processing. No document acceptance.

use std::{borrow::Cow, collections::HashMap};

use crate::dtd::{
    DefaultDeclarationView, DtdLimits, DtdUnit, DtdUnitView, EntityDefinitionView, ReferenceKind,
};
use crate::dtd_reference::{ReferenceKind as Spelled, classify};
use crate::escape::{predefined, xml_char};
use crate::resolve::Loader;
use crate::valid::{AttributeDecl, Code, Dtd, ElementDecl};
use crate::{DtdError, ErrorKind, Limit};

mod external;

pub(crate) use external::external_dtd;

pub(crate) use crate::dtd::{AttributeKind, LiteralSource};
pub(crate) use crate::origin::EntityId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DeclarationContext {
    pub(crate) internal_subset: bool,
    pub(crate) literals: LiteralSource,
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

const fn limit(limit: Limit) -> ErrorKind {
    ErrorKind::LimitExceeded(limit)
}
const DTD_BYTES: ErrorKind = limit(Limit::DtdBytes);
const OWNED: ErrorKind = limit(Limit::OwnedBytes);
const EXPANSION: ErrorKind = limit(Limit::EntityExpansion);
const VALUE_BYTES: ErrorKind = limit(Limit::TokenBytes);
const ALLOCATION: ErrorKind = ErrorKind::Allocation;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EntityLookup<'a> {
    Predefined,
    Internal { id: EntityId, replacement: &'a str },
    ExternalUnavailable { id: EntityId },
    Unparsed,
    UnreadEntity,
    Undeclared,
}

#[derive(Clone, Debug)]
pub(crate) enum StoredDefault {
    Required,
    Implied,
    Value { fixed: bool, value: String },
}

impl StoredDefault {
    pub(crate) fn value(&self) -> Option<&str> {
        match self {
            Self::Value { value, .. } => Some(value),
            _ => None,
        }
    }
}

struct ExternalEntity {
    public: Option<String>,
    /// The system identifier resolved against its declaration's base.
    uri: Option<String>,
    notation: Option<String>,
    /// Loaded replacement text; `loaded` records any attempt.
    text: Option<String>,
    loaded: bool,
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
    dtd: Dtd,
    frames: Vec<ValueFrame>,
    limits: DtdLimits,
    usage: CatalogUsage,
    standalone: bool,
    external_subset: bool,
    parameter_reference: bool,
    amplification_ceiling: usize,
    loader: Option<Loader>,
    /// The DOCTYPE's external identifier, kept only for loading.
    subset: Option<ExternalEntity>,
}

impl DtdCatalog {
    pub(crate) fn new(limits: DtdLimits, standalone: bool) -> Self {
        Self {
            general: HashMap::new(),
            parameter: HashMap::new(),
            entities: Vec::new(),
            dtd: Dtd {
                standalone,
                ..Dtd::default()
            },
            frames: Vec::new(),
            limits,
            usage: CatalogUsage::default(),
            standalone,
            external_subset: false,
            parameter_reference: false,
            amplification_ceiling: usize::MAX,
            loader: None,
            subset: None,
        }
    }

    pub(crate) fn cap_work(&mut self, ceiling: usize) {
        self.limits.work = self.limits.work.min(ceiling);
    }

    pub(crate) fn set_amplification_ceiling(&mut self, ceiling: usize) {
        self.amplification_ceiling = ceiling;
    }

    /// Bytes returned by the resolver; libxml2 counts them as consumed input.
    pub(crate) fn external_bytes(&self) -> usize {
        self.loader.as_ref().map_or(0, Loader::loaded)
    }

    /// Charges copied default-attribute text as libxml2 charges `expandedSize`.
    pub(crate) fn charge_default(&mut self, bytes: usize) -> Result<(), DtdError> {
        let ceiling = self.amplification_ceiling;
        increment(
            &mut self.usage.amplification,
            bytes,
            ceiling,
            EXPANSION,
            Some(0),
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
    fn lookup(&self, parameter: bool, name: &str) -> EntityLookup<'_> {
        let table = if parameter {
            &self.parameter
        } else {
            &self.general
        };
        let Some(&id) = table.get(name) else {
            return match self.may_be_unread() {
                true => EntityLookup::UnreadEntity,
                false => EntityLookup::Undeclared,
            };
        };
        match &self.entities[id.0].body {
            EntityBody::Internal(replacement) => EntityLookup::Internal { id, replacement },
            EntityBody::External(entity) => match (&entity.notation, &entity.text) {
                (Some(_), _) => EntityLookup::Unparsed,
                (None, Some(replacement)) if !parameter => {
                    EntityLookup::Internal { id, replacement }
                }
                (None, _) => EntityLookup::ExternalUnavailable { id },
            },
        }
    }

    pub(crate) fn lookup_general(&self, name: &str) -> EntityLookup<'_> {
        match predefined(name) {
            Some(_) => EntityLookup::Predefined,
            None => self.lookup(false, name),
        }
    }
    pub(crate) fn lookup_parameter(&self, name: &str) -> EntityLookup<'_> {
        self.lookup(true, name)
    }

    pub(crate) fn replacement(&self, id: EntityId) -> Option<&str> {
        self.entities.get(id.0)?.body.text()
    }

    pub(crate) fn standalone(&self) -> bool {
        self.standalone
    }

    /// Records a validity error found while parsing.
    pub(crate) fn note(&mut self, code: Code, offset: Option<usize>, message: String) {
        self.dtd.issues.push((code, offset, message));
    }

    /// The validation view of every declaration applied so far.
    pub(crate) fn dtd(&self) -> &Dtd {
        &self.dtd
    }

    pub(crate) fn attributes(&self, element: &str) -> impl Iterator<Item = &AttributeDecl> {
        self.dtd
            .element_index
            .get(element)
            .map(|index| self.dtd.elements[*index].attributes.as_slice())
            .unwrap_or(&[])
            .iter()
    }

    /// Applies one declaration. Failure offsets are relative to `unit`, or
    /// absent inside replacement text.
    pub(crate) fn apply(
        &mut self,
        unit: &DtdUnit<'_>,
        context: DeclarationContext,
    ) -> Result<(), DtdError> {
        let at = Some(0);
        let limits = self.limits;
        increment(
            &mut self.usage.declarations,
            1,
            limits.declarations,
            DTD_BYTES,
            at,
        )?;
        increment(
            &mut self.usage.work,
            unit.raw().len(),
            limits.work,
            EXPANSION,
            at,
        )?;
        if context.internal_subset
            && let Some((offset, _)) = unit
                .reference_positions()
                .find(|(_, reference)| reference.kind == ReferenceKind::Parameter)
        {
            return Err(DtdError::new(ErrorKind::InvalidDtd, Some(offset)));
        }
        let colon = match unit.view() {
            DtdUnitView::Entity(entity) => entity.name().contains(':'),
            DtdUnitView::Notation(notation) => notation.name().contains(':'),
            _ => false,
        };
        if colon {
            return Err(DtdError::new(ErrorKind::Namespace, at));
        }
        match unit.view() {
            DtdUnitView::Header(header) => {
                self.dtd.name = Some(copy_string(header.root_name())?);
                self.external_subset |= header.external_id().is_some();
                if let Some(identifier) = header.external_id() {
                    self.subset = Some(self.external_entity(identifier, None, context)?);
                }
            }
            DtdUnitView::Entity(entity) => {
                let parameter = entity.is_parameter();
                if !parameter && self.standalone && !context.internal_subset {
                    // Never bound, but libxml2 still finds it for ENTITY attributes.
                    let notation = match entity.definition() {
                        EntityDefinitionView::External { notation, .. } => notation,
                        EntityDefinitionView::Internal(_) => None,
                    };
                    self.store_bytes(entity.name().len() + notation.map_or(0, str::len))?;
                    self.record_entity(entity.name(), notation)?;
                }
                // WFC Entity Declared: a standalone document cannot use general
                // entities from external markup, so they are never bound.
                let duplicate = if parameter {
                    self.parameter.contains_key(entity.name())
                } else {
                    self.general.contains_key(entity.name())
                        || (self.standalone && !context.internal_subset)
                };
                let body = match entity.definition() {
                    EntityDefinitionView::Internal(literal) => {
                        let (content, offset) = literal.content();
                        let result = self.process_value(
                            content,
                            offset,
                            context.literals,
                            ValueMode::Entity,
                            &mut ValueRequest::default(),
                        )?;
                        if !parameter
                            && let Some(character) = predefined(entity.name())
                            && !valid_predefined(entity.name(), character, &result)
                        {
                            return Err(DtdError::new(ErrorKind::InvalidDtd, Some(offset)));
                        }
                        EntityBody::Internal(result.into_owned())
                    }
                    EntityDefinitionView::External {
                        identifier,
                        notation,
                    } => {
                        if !parameter && predefined(entity.name()).is_some() {
                            return Err(DtdError::new(ErrorKind::InvalidDtd, at));
                        }
                        if duplicate {
                            return Ok(());
                        }
                        let bytes = checked_sum(&[
                            entity.name().len(),
                            identifier
                                .public
                                .map_or(0, |raw| identifier_length(raw, context.literals)),
                            identifier
                                .system
                                .map_or(0, |raw| identifier_length(raw, context.literals)),
                            notation.map_or(0, str::len),
                        ])?;
                        self.store_bytes(bytes)?;
                        self.add_binding()?;
                        let external = self.external_entity(identifier, notation, context)?;
                        return self.insert_entity(
                            entity.name(),
                            parameter,
                            EntityBody::External(external),
                        );
                    }
                };
                if !duplicate {
                    let bytes = match &body {
                        EntityBody::Internal(value) => value.len(),
                        _ => 0,
                    };
                    self.store_bytes(checked_sum(&[entity.name().len(), bytes])?)?;
                    self.add_binding()?;
                    self.insert_entity(entity.name(), parameter, body)?;
                }
            }
            DtdUnitView::AttributeList(list) => {
                for definition in list.definitions() {
                    let kind = definition.kind();
                    let default = match definition.default() {
                        DefaultDeclarationView::Required => StoredDefault::Required,
                        DefaultDeclarationView::Implied => StoredDefault::Implied,
                        DefaultDeclarationView::Value { fixed, literal } => {
                            let (content, offset) = literal.content();
                            let value = self.process_value(
                                content,
                                offset,
                                context.literals,
                                ValueMode::Attribute {
                                    tokenized: kind != AttributeKind::Cdata,
                                },
                                &mut ValueRequest::default(),
                            )?;
                            StoredDefault::Value {
                                fixed,
                                value: value.into_owned(),
                            }
                        }
                    };
                    let values = definition.values().map(str::to_owned).collect();
                    let element = self.element_slot(list.element_name())?;
                    let attribute = AttributeDecl {
                        name: copy_string(definition.name())?,
                        kind,
                        values,
                        default,
                        internal: context.internal_subset,
                    };
                    self.dtd.check_attribute(element, &attribute);
                    if self.dtd.elements[element]
                        .index
                        .contains_key(definition.name())
                    {
                        continue;
                    }
                    self.add_binding()?;
                    let values: usize = attribute.values.iter().map(String::len).sum();
                    self.store_bytes(checked_sum(&[
                        definition.name().len(),
                        definition.name().len(),
                        attribute.default.value().map_or(0, str::len),
                        values,
                    ])?)?;
                    let attributes = &mut self.dtd.elements[element];
                    attributes.index.try_reserve(1).map_err(|_| allocation())?;
                    attributes
                        .attributes
                        .try_reserve(1)
                        .map_err(|_| allocation())?;
                    let key = copy_string(definition.name())?;
                    attributes.index.insert(key, attributes.attributes.len());
                    attributes.attributes.push(attribute);
                }
            }
            DtdUnitView::Element(view) => {
                let element = self.element_slot(view.name())?;
                // Follow-set entries are four bytes: bound them by storage too.
                let work = self.limits.work - self.usage.work;
                let storage = self.limits.storage - self.usage.storage_bytes;
                let (kind, budget) = match work <= storage / 4 {
                    true => (EXPANSION, work),
                    false => (OWNED, storage / 4),
                };
                let mut remaining = budget;
                let bytes = self
                    .dtd
                    .declare_element(element, view, context.internal_subset, &mut remaining)
                    .map_err(|_| DtdError::new(kind, at))?;
                self.usage.work += budget - remaining;
                self.store_bytes(bytes)?;
            }
            DtdUnitView::Notation(notation) => {
                self.store_bytes(notation.name().len())?;
                self.dtd
                    .declare_notation(notation.name(), context.internal_subset);
            }
            _ => {}
        }
        Ok(())
    }

    /// Records general entity `name` for ENTITY attribute validation.
    fn record_entity(&mut self, name: &str, notation: Option<&str>) -> Result<(), DtdError> {
        if !self.dtd.entities.contains_key(name) {
            self.dtd.entities.try_reserve(1).map_err(|_| allocation())?;
            let notation = notation.map(copy_string).transpose()?;
            self.dtd.entities.insert(copy_string(name)?, notation);
        }
        Ok(())
    }

    /// The index of `name`'s element declaration, created on first use.
    fn element_slot(&mut self, name: &str) -> Result<usize, DtdError> {
        if let Some(&index) = self.dtd.element_index.get(name) {
            return Ok(index);
        }
        self.store_bytes(name.len())?;
        self.dtd
            .element_index
            .try_reserve(1)
            .map_err(|_| allocation())?;
        self.dtd.elements.try_reserve(1).map_err(|_| allocation())?;
        let index = self.dtd.elements.len();
        let key = copy_string(name)?;
        self.dtd.elements.push(ElementDecl::new(copy_string(name)?));
        self.dtd.element_index.insert(key, index);
        Ok(index)
    }

    fn insert_entity(
        &mut self,
        name: &str,
        parameter: bool,
        body: EntityBody,
    ) -> Result<(), DtdError> {
        if !parameter {
            // Validation's copy is bounded by the binding's charged bytes.
            let notation = match &body {
                EntityBody::External(external) => external.notation.as_deref(),
                EntityBody::Internal(_) => None,
            };
            self.record_entity(name, notation)?;
        }
        let table = if parameter {
            &mut self.parameter
        } else {
            &mut self.general
        };
        table.try_reserve(1).map_err(|_| allocation())?;
        self.entities.try_reserve(1).map_err(|_| allocation())?;
        let name = copy_string(name)?;
        let id = EntityId(self.entities.len());
        self.entities.push(Entity { body, parameter });
        table.insert(name, id);
        Ok(())
    }

    fn external_entity(
        &self,
        identifier: crate::dtd::ExternalIdView<'_>,
        notation: Option<&str>,
        context: DeclarationContext,
    ) -> Result<ExternalEntity, DtdError> {
        let system = copy_identifier(identifier.system, context.literals)?;
        Ok(ExternalEntity {
            public: copy_identifier(identifier.public, context.literals)?,
            uri: self
                .loader
                .as_ref()
                .zip(system.as_deref())
                .map(|(loader, system)| loader.uri(system)),
            notation: notation.map(copy_string).transpose()?,
            text: None,
            loaded: false,
        })
    }

    fn store_bytes(&mut self, bytes: usize) -> Result<(), DtdError> {
        let ceiling = self.limits.storage;
        increment(
            &mut self.usage.storage_bytes,
            bytes,
            ceiling,
            OWNED,
            Some(0),
        )
    }

    fn add_binding(&mut self) -> Result<(), DtdError> {
        let ceiling = self.limits.declarations;
        increment(&mut self.usage.bindings, 1, ceiling, DTD_BYTES, Some(0))
    }

    fn tokenized_attribute(&self, element: &str, name: &str) -> bool {
        self.dtd
            .element_index
            .get(element)
            .and_then(|index| {
                let attributes = &self.dtd.elements[*index];
                attributes
                    .index
                    .get(name)
                    .map(|index| attributes.attributes[*index].kind != AttributeKind::Cdata)
            })
            .unwrap_or(false)
    }

    /// Normalizes attribute `name` of `element`, spending at most
    /// `caller_value_bytes` of new capacity, recorded in `requested`. With
    /// `borrow`, an unchanged physical literal is returned as is.
    #[expect(clippy::too_many_arguments, reason = "one normalization request")]
    pub(crate) fn normalize_attribute<'raw>(
        &mut self,
        element: &str,
        name: &str,
        literal_content: &'raw str,
        source: LiteralSource,
        borrow: bool,
        caller_value_bytes: usize,
        requested: &mut usize,
    ) -> Result<Cow<'raw, str>, DtdError> {
        let tokenized = self.tokenized_attribute(element, name);
        let mut request = ValueRequest {
            ceiling: Some(caller_value_bytes),
            requested: 0,
            borrow_physical: borrow,
        };
        let result = self.process_value(
            literal_content,
            0,
            source,
            ValueMode::Attribute { tokenized },
            &mut request,
        );
        *requested = request.requested;
        result
    }

    fn process_value<'raw>(
        &mut self,
        raw: &'raw str,
        offset: usize,
        source: LiteralSource,
        mode: ValueMode,
        request: &mut ValueRequest,
    ) -> Result<Cow<'raw, str>, DtdError> {
        // EntityValue admits `%` only to start a PE reference; it sets hasPErefs first.
        if matches!(mode, ValueMode::Entity) && raw.contains('%') {
            self.parameter_reference = true;
        }
        let may_be_unread = self.may_be_unread();
        let (usage, requested) = (self.usage, request.requested);
        let noted = self.dtd.issues.len();
        loop {
            let mut processor = ValueProcessor {
                general: &self.general,
                parameter: &self.parameter,
                entities: &self.entities,
                frames: &mut self.frames,
                limits: self.limits,
                amplification_ceiling: self.amplification_ceiling,
                request: &mut *request,
                usage: &mut self.usage,
                issues: &mut self.dtd.issues,
                may_be_unread,
                loadable: self.loader.is_some(),
                raw,
                offset,
                source,
                mode,
            };
            let result = processor.process();
            processor.frames.clear();
            // An external PE inside an entity value is loaded, then the value restarts.
            match result {
                Ok(value) => return Ok(value),
                Err(Stop::Fail(error)) => return Err(error),
                Err(Stop::Load(id)) => {
                    (self.usage, request.requested) = (usage, requested);
                    self.dtd.issues.truncate(noted);
                    self.load(id)?;
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ValueMode {
    Entity,
    Attribute { tokenized: bool },
}

#[derive(Default)]
struct ValueRequest {
    /// The caller's remaining capacity for new value storage.
    ceiling: Option<usize>,
    requested: usize,
    borrow_physical: bool,
}

/// Why value processing stopped: a failure, or an external PE to load first.
enum Stop {
    Fail(DtdError),
    Load(EntityId),
}

impl From<DtdError> for Stop {
    fn from(error: DtdError) -> Self {
        Self::Fail(error)
    }
}

struct ValueProcessor<'a, 'raw> {
    general: &'a HashMap<String, EntityId>,
    parameter: &'a HashMap<String, EntityId>,
    entities: &'a [Entity],
    frames: &'a mut Vec<ValueFrame>,
    limits: DtdLimits,
    amplification_ceiling: usize,
    request: &'a mut ValueRequest,
    usage: &'a mut CatalogUsage,
    issues: &'a mut Vec<(Code, Option<usize>, String)>,
    may_be_unread: bool,
    loadable: bool,
    raw: &'raw str,
    offset: usize,
    source: LiteralSource,
    mode: ValueMode,
}

impl<'raw> ValueProcessor<'_, 'raw> {
    fn process(&mut self) -> Result<Cow<'raw, str>, Stop> {
        let root = Some(self.offset);
        increment(
            &mut self.usage.work,
            self.raw.len(),
            self.limits.work,
            EXPANSION,
            root,
        )?;
        self.frames
            .try_reserve(1)
            .map_err(|_| DtdError::new(ALLOCATION, root))?;
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
            // Offsets exist only in the caller's own text.
            let at = frame.entity.is_none().then_some(self.offset + frame.cursor);
            let fail = |kind| Stop::Fail(DtdError::new(kind, at));
            let remaining = &text[frame.cursor..];
            let character = remaining.chars().next().expect("nonempty UTF-8 frame");
            let mut consumed = character.len_utf8();
            if !xml_char(character) {
                return Err(fail(ErrorKind::InvalidCharacter));
            }
            let is_entity = matches!(self.mode, ValueMode::Entity);
            if character == '&' || (is_entity && character == '%') {
                let (spelling, reference) =
                    parse_reference(remaining).ok_or(fail(ErrorKind::InvalidReference))?;
                consumed = spelling.len();
                increment(
                    &mut self.usage.references,
                    1,
                    self.limits.references,
                    limit(Limit::EntityReferences),
                    at,
                )?;
                self.frames.last_mut().expect("active value frame").cursor += consumed;
                match reference {
                    Reference::Character(character) => {
                        self.emit(&mut output, &mut pending_space, character, at)?
                    }
                    Reference::Named(_) if is_entity && character == '&' => {
                        // Every GE, including predefined names, is bypassed in EntityValue.
                        for character in spelling.chars() {
                            self.emit(&mut output, &mut pending_space, character, at)?;
                        }
                    }
                    Reference::Named(name) => {
                        if !is_entity && let Some(character) = predefined(name) {
                            self.emit(&mut output, &mut pending_space, character, at)?;
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
                                let message = format!("Entity '{name}' not defined");
                                self.issues.push((Code::UndeclaredEntity, None, message));
                                continue;
                            }
                            return Err(fail(ErrorKind::UnknownEntity));
                        };
                        match &self.entities[id.0].body {
                            EntityBody::External(external) if !is_entity => {
                                return Err(fail(match external.notation {
                                    Some(_) => ErrorKind::UnparsedEntity,
                                    None => ErrorKind::ExternalEntityInAttribute,
                                }));
                            }
                            EntityBody::External(external) if external.text.is_none() => {
                                if self.loadable && !external.loaded {
                                    return Err(Stop::Load(id));
                                }
                                continue;
                            }
                            body => {
                                let replacement = body.text().expect("checked replacement");
                                let work = self.limits.work;
                                increment(
                                    &mut self.usage.work,
                                    self.frames.len(),
                                    work,
                                    EXPANSION,
                                    at,
                                )?;
                                if self.frames.iter().any(|frame| frame.entity == Some(id)) {
                                    return Err(fail(ErrorKind::EntityLoop));
                                }
                                if self.frames.len() > self.limits.depth {
                                    return Err(fail(limit(Limit::EntityDepth)));
                                }
                                increment(
                                    &mut self.usage.work,
                                    replacement.len(),
                                    work,
                                    EXPANSION,
                                    at,
                                )?;
                                // libxml2 charges each entered replacement plus a fixed cost.
                                increment(
                                    &mut self.usage.amplification,
                                    replacement
                                        .len()
                                        .saturating_add(crate::dtd_controller::ENTITY_FIXED_COST),
                                    self.amplification_ceiling,
                                    EXPANSION,
                                    at,
                                )?;
                                self.frames.try_reserve(1).map_err(|_| fail(ALLOCATION))?;
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
                return Err(fail(ErrorKind::Syntax));
            }
            let character = if !is_entity && matches!(character, '\r' | '\n' | '\t') {
                ' '
            } else {
                character
            };
            self.frames.last_mut().expect("active value frame").cursor += consumed;
            self.emit(&mut output, &mut pending_space, character, at)?;
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
        at: Option<usize>,
    ) -> Result<(), DtdError> {
        increment(
            &mut self.usage.expanded_bytes,
            character.len_utf8(),
            self.limits.expansion,
            EXPANSION,
            at,
        )?;
        if matches!(self.mode, ValueMode::Attribute { tokenized: true }) {
            if character == ' ' {
                *pending_space = output.len() != 0;
                return Ok(());
            }
            if *pending_space {
                self.append_output(output, ' ', at)?;
                *pending_space = false;
            }
        }
        self.append_output(output, character, at)
    }

    fn append_output(
        &mut self,
        output: &mut ValueOutput<'raw>,
        character: char,
        at: Option<usize>,
    ) -> Result<(), DtdError> {
        if let ValueOutput::Matching { raw, matched } = output {
            let needed = matched
                .checked_add(character.len_utf8())
                .filter(|length| *length <= self.limits.token)
                .ok_or(DtdError::new(VALUE_BYTES, at))?;
            increment(
                &mut self.usage.work,
                character.len_utf8(),
                self.limits.work,
                EXPANSION,
                at,
            )?;
            let mut encoded = [0; 4];
            if raw.as_bytes().get(*matched..needed)
                == Some(character.encode_utf8(&mut encoded).as_bytes())
            {
                *matched = needed;
                return Ok(());
            }
            self.materialize(output, Some(self.offset))?;
        }
        let ValueOutput::Owned(output) = output else {
            unreachable!("materialized output")
        };
        let needed = output
            .len()
            .checked_add(character.len_utf8())
            .filter(|length| *length <= self.limits.token)
            .ok_or(DtdError::new(VALUE_BYTES, at))?;
        if needed > output.capacity() {
            let target = output
                .capacity()
                .saturating_mul(2)
                .max(64)
                .max(needed)
                .min(self.limits.token);
            self.request_capacity(target, at)?;
            output
                .try_reserve_exact(target - output.len())
                .map_err(|_| DtdError::new(ALLOCATION, at))?;
        }
        output.push(character);
        Ok(())
    }

    fn materialize(
        &mut self,
        output: &mut ValueOutput<'raw>,
        at: Option<usize>,
    ) -> Result<(), DtdError> {
        let ValueOutput::Matching { raw, matched } = output else {
            return Ok(());
        };
        increment(
            &mut self.usage.work,
            *matched,
            self.limits.work,
            EXPANSION,
            at,
        )?;
        let mut owned = String::new();
        if *matched != 0 {
            let target = (*matched).max(64).min(self.limits.token);
            self.request_capacity(target, at)?;
            owned
                .try_reserve_exact(target)
                .map_err(|_| DtdError::new(ALLOCATION, at))?;
            owned.push_str(&raw[..*matched]);
        }
        *output = ValueOutput::Owned(owned);
        Ok(())
    }

    /// Spends `bytes` of the caller's value capacity, when it set one.
    fn request_capacity(&mut self, bytes: usize, at: Option<usize>) -> Result<(), DtdError> {
        if let Some(ceiling) = self.request.ceiling {
            increment(&mut self.request.requested, bytes, ceiling, OWNED, at)?;
        }
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

enum Reference<'a> {
    Character(char),
    Named(&'a str),
}

/// The reference spelled at the start of `raw` through its `;`, and its value.
/// A parameter reference names an entity; it is never a character.
fn parse_reference(raw: &str) -> Option<(&str, Reference<'_>)> {
    let spelling = &raw[..=raw.find(';')?];
    let name = &spelling[1..spelling.len() - 1];
    if raw.starts_with('%') && name.starts_with('#') {
        return None;
    }
    Some(match classify(spelling, 0, true).ok()? {
        Spelled::Numeric(character) => (spelling, Reference::Character(character)),
        _ => (spelling, Reference::Named(name)),
    })
}

fn valid_predefined(name: &str, character: char, replacement: &str) -> bool {
    if !matches!(name, "amp" | "lt")
        && replacement.len() == character.len_utf8()
        && replacement.starts_with(character)
    {
        return true;
    }
    replacement.starts_with('&')
        && matches!(parse_reference(replacement),
            Some((spelling, Reference::Character(found)))
                if spelling.len() == replacement.len() && found == character)
}

fn allocation() -> DtdError {
    DtdError::new(ALLOCATION, Some(0))
}

fn increment(
    value: &mut usize,
    amount: usize,
    ceiling: usize,
    kind: ErrorKind,
    at: Option<usize>,
) -> Result<(), DtdError> {
    *value = value
        .checked_add(amount)
        .filter(|total| *total <= ceiling)
        .ok_or(DtdError::new(kind, at))?;
    Ok(())
}

fn checked_sum(values: &[usize]) -> Result<usize, DtdError> {
    values.iter().try_fold(0usize, |sum, value| {
        sum.checked_add(*value).ok_or(DtdError::new(OWNED, Some(0)))
    })
}

fn copy_string(raw: &str) -> Result<String, DtdError> {
    let mut value = String::new();
    value
        .try_reserve_exact(raw.len())
        .map_err(|_| allocation())?;
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
) -> Result<Option<String>, DtdError> {
    let Some(quoted) = quoted else {
        return Ok(None);
    };
    let mut output = String::new();
    output
        .try_reserve_exact(identifier_length(quoted, source))
        .map_err(|_| allocation())?;
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
