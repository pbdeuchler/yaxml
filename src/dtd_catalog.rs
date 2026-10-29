//! Effective DTD bindings and value processing. No document acceptance.

use std::{borrow::Cow, collections::HashMap};

use crate::dtd::{
    DefaultDeclarationView, DtdLimits, DtdUnit, DtdUnitView, EntityDefinitionView, ReferenceKind,
};
use crate::dtd_reference::{ReferenceKind as Spelled, classify};
use crate::escape::predefined;
use crate::resolve::Loader;
use crate::valid::{AttributeDecl, Code, Dtd, ElementDecl, NotationDecl};
use crate::{DtdError, ErrorKind, Limit};

mod external;

pub(crate) use external::external_dtd;

pub(crate) use crate::dtd::{AttributeKind, LiteralSource};
pub(crate) use crate::origin::EntityId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DeclarationContext {
    pub(crate) internal_subset: bool,
    pub(crate) literals: LiteralSource,
    /// Where the declaration starts in complete input that content is read
    /// from in place, when it was read from that input.
    pub(crate) input: Option<usize>,
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
    /// `kept` is the value with its general references kept, when it has any.
    Value {
        fixed: bool,
        value: String,
        kept: Option<String>,
    },
}

impl StoredDefault {
    pub(crate) fn value(&self) -> Option<&str> {
        match self {
            Self::Value { value, .. } => Some(value),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ExternalEntity {
    pub(crate) public: Option<String>,
    /// The system literal as declared.
    pub(crate) system: Option<String>,
    /// The system identifier resolved against its declaration's base; only
    /// with a loader.
    pub(crate) uri: Option<String>,
    pub(crate) notation: Option<String>,
    /// Loaded replacement text; `loaded` records any attempt.
    pub(crate) text: Option<String>,
    loaded: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum EntityBody {
    Internal(String),
    External(ExternalEntity),
}

impl EntityBody {
    pub(crate) fn text(&self) -> Option<&str> {
        match self {
            Self::Internal(text) => Some(text),
            Self::External(external) => external.text.as_deref(),
        }
    }
}

/// One stored entity declaration (libxml2's `xmlEntity`).
#[derive(Clone, Debug)]
pub(crate) struct EntityDecl {
    pub(crate) name: String,
    pub(crate) body: EntityBody,
    pub(crate) parameter: bool,
    /// Declared in markup read from the external subset (libxml2's
    /// `extSubset`), not in the internal subset or a parameter entity it
    /// references.
    pub(crate) external_subset: bool,
    /// Where an internal entity's replacement text appears verbatim in the
    /// complete input, as its declaration's literal.
    pub(crate) input: Option<usize>,
    /// An internal entity's literal as written (libxml2's `orig`), when it
    /// differs from the replacement text.
    pub(crate) literal: Option<Box<str>>,
}

#[derive(Clone, Copy)]
struct ValueFrame {
    entity: Option<EntityId>,
    cursor: usize,
}

/// Recent lookups of short names, in front of a SipHash table: direct-mapped
/// slots chosen by an unkeyed hash of the name, which they hold inline
/// (length first; zero is empty). Content names repeat, so most lookups
/// hit; crafted names can only collide and evict, which costs no more than
/// the keyed table lookup they then fall back to.
struct Recent<T> {
    slots: [([u8; 16], T); 64],
}

impl<T: Copy> Recent<T> {
    fn new(empty: T) -> Self {
        Self {
            slots: [([0; 16], empty); 64],
        }
    }

    /// The slot and inline key of a name of 1 to 15 bytes.
    fn key(name: &str) -> Option<(usize, [u8; 16])> {
        let bytes = name.as_bytes();
        let (&first, &last) = bytes.first().zip(bytes.last())?;
        let mut key = [0; 16];
        key[0] = u8::try_from(bytes.len())
            .ok()
            .filter(|length| *length < 16)?;
        key[1..=bytes.len()].copy_from_slice(bytes);
        let slot = bytes.len() ^ usize::from(first) << 1 ^ usize::from(last) << 2;
        Some((slot & 63, key))
    }

    fn get(&self, name: &str) -> Option<T> {
        let (slot, key) = Self::key(name)?;
        let (held, value) = &self.slots[slot];
        (*held == key).then_some(*value)
    }

    fn put(&mut self, name: &str, value: T) {
        if let Some((slot, key)) = Self::key(name) {
            self.slots[slot] = (key, value);
        }
    }
}

pub(crate) struct DtdCatalog {
    dtd: Dtd,
    frames: Vec<ValueFrame>,
    limits: DtdLimits,
    usage: CatalogUsage,
    standalone: bool,
    external_subset: bool,
    /// Declarations come from the external subset (libxml2's `inSubset == 2`).
    reading_external_subset: bool,
    parameter_reference: bool,
    /// Whether any element has an attribute declaration.
    attribute_lists: bool,
    /// Recently referenced general entities, and whether recently started
    /// elements declare attributes.
    recent_entities: Recent<EntityId>,
    recent_elements: Recent<Option<usize>>,
    amplification_ceiling: usize,
    loader: Option<Loader>,
    /// The DOCTYPE's external identifier, kept only for loading.
    subset: Option<ExternalEntity>,
    /// Offsets in the last processed value of the references to undeclared
    /// entities it expanded to nothing (outermost references for nested ones).
    undeclared: Vec<usize>,
}

impl DtdCatalog {
    pub(crate) fn new(limits: DtdLimits, standalone: bool) -> Self {
        Self {
            dtd: Dtd {
                standalone,
                ..Dtd::default()
            },
            frames: Vec::new(),
            limits,
            usage: CatalogUsage::default(),
            standalone,
            external_subset: false,
            reading_external_subset: false,
            parameter_reference: false,
            attribute_lists: false,
            recent_entities: Recent::new(EntityId(0)),
            recent_elements: Recent::new(None),
            amplification_ceiling: usize::MAX,
            loader: None,
            subset: None,
            undeclared: Vec::new(),
        }
    }

    /// Takes all of `dtd`'s declarations as already applied, so more can
    /// follow them (`Editor::add_declarations`).
    pub(crate) fn resume(&mut self, dtd: &Dtd) {
        self.seed(dtd);
        let entities = std::mem::take(&mut self.dtd.entities);
        self.dtd = Dtd {
            entities,
            ..dtd.clone()
        };
    }

    /// Takes `dtd`'s entity and notation declarations as already applied.
    pub(crate) fn seed(&mut self, dtd: &Dtd) {
        // Replacement texts are no longer spelled in the input.
        let entities = dtd.entities.iter().map(|entity| EntityDecl {
            input: None,
            ..entity.clone()
        });
        self.dtd = Dtd {
            entities: entities.collect(),
            general: dtd.general.clone(),
            parameter: dtd.parameter.clone(),
            notations: dtd.notations.clone(),
            notation_index: dtd.notation_index.clone(),
            ..Dtd::default()
        };
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

    /// The expansion usage its ceilings still allow.
    pub(crate) fn headroom(&self) -> CatalogUsage {
        let (limits, usage) = (&self.limits, self.usage);
        CatalogUsage {
            expanded_bytes: limits.expansion.saturating_sub(usage.expanded_bytes),
            work: limits.work.saturating_sub(usage.work),
            references: limits.references.saturating_sub(usage.references),
            amplification: self
                .amplification_ceiling
                .saturating_sub(usage.amplification),
            ..CatalogUsage::default()
        }
    }

    /// Charges a replayed expansion's usage, within `headroom`.
    pub(crate) fn replayed(&mut self, charged: CatalogUsage) {
        let usage = &mut self.usage;
        usage.expanded_bytes += charged.expanded_bytes;
        usage.work += charged.work;
        usage.references += charged.references;
        usage.amplification += charged.amplification;
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
            &self.dtd.parameter
        } else {
            &self.dtd.general
        };
        match table.get(name) {
            Some(&id) => self.classify(parameter, id),
            None if self.may_be_unread() => EntityLookup::UnreadEntity,
            None => EntityLookup::Undeclared,
        }
    }

    /// A general entity's lookup by its id.
    pub(crate) fn entity_lookup(&self, id: EntityId) -> EntityLookup<'_> {
        self.classify(false, id)
    }

    fn classify(&self, parameter: bool, id: EntityId) -> EntityLookup<'_> {
        match &self.dtd.entities[id.0].body {
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

    /// A general entity; predefined names are character references, never
    /// looked up here.
    pub(crate) fn lookup_general(&self, name: &str) -> EntityLookup<'_> {
        self.lookup(false, name)
    }
    /// A general entity referenced in content, loading external text first.
    pub(crate) fn resolve_general(&mut self, name: &str) -> Result<EntityLookup<'_>, DtdError> {
        let id = self.recent_entities.get(name).or_else(|| {
            let id = self.dtd.general.get(name).copied();
            if let Some(id) = id {
                self.recent_entities.put(name, id);
            }
            id
        });
        match id {
            Some(id) => {
                self.load(id)?;
                Ok(self.classify(false, id))
            }
            None if self.may_be_unread() => Ok(EntityLookup::UnreadEntity),
            None => Ok(EntityLookup::Undeclared),
        }
    }
    pub(crate) fn lookup_parameter(&self, name: &str) -> EntityLookup<'_> {
        self.lookup(true, name)
    }

    pub(crate) fn replacement(&self, id: EntityId) -> Option<&str> {
        self.dtd.entities.get(id.0)?.body.text()
    }

    /// Where `id`'s replacement text appears verbatim in complete input.
    pub(crate) fn replacement_input(&self, id: EntityId) -> Option<usize> {
        self.dtd.entities[id.0].input
    }

    pub(crate) fn standalone(&self) -> bool {
        self.standalone
    }

    /// Whether `attribute`'s value keeps its general references
    /// ([`kept_form`](crate::dtd_tag::kept_form)): without `NOENT`, except
    /// namespace declarations, which libxml2 always expands.
    pub(crate) fn keeps_references(&self, name: &str, literal: &str) -> bool {
        !self.limits.flags.substitutes()
            && literal.contains('&')
            && name != "xmlns"
            && !name.starts_with("xmlns:")
    }

    /// Whether a reference to general entity `name` is kept: it is declared
    /// and expandable.
    pub(crate) fn keeps(&self, name: &str) -> bool {
        predefined(name).is_none()
            && matches!(self.lookup_general(name), EntityLookup::Internal { .. })
    }

    /// Whether non-namespace default attributes are added (`DTDATTR`).
    pub(crate) fn default_attributes(&self) -> bool {
        self.limits.flags.default_attributes()
    }

    /// Records a validity error found while parsing.
    /// The warning a reference to an undeclared entity that unread markup
    /// could declare raises besides its validity note.
    pub(crate) fn undeclared_warning(&self) -> Option<crate::Warning> {
        self.limits.flags.undeclared_entity()
    }

    /// Where the last processed value referenced undeclared entities.
    pub(crate) fn undeclared_references(&self) -> &[usize] {
        &self.undeclared
    }

    pub(crate) fn note(&mut self, code: Code, offset: Option<usize>, message: String) {
        self.dtd.issues.push((code, offset, message));
    }

    /// The validation view of every declaration applied so far.
    pub(crate) fn dtd(&self) -> &Dtd {
        &self.dtd
    }

    /// Moves the declarations out for a finished document: cloning them
    /// copied every entity, name and value (DTD-heavy DOM builds +10%).
    pub(crate) fn take_dtd(&mut self) -> Dtd {
        std::mem::take(&mut self.dtd)
    }

    /// Whether a start tag needs nothing from the declarations: no
    /// attribute value has a reference, and its element declares no
    /// attribute (so nothing is normalized, defaulted or noted).
    pub(crate) fn plain_tag(&mut self, raw: &str) -> bool {
        if memchr::memchr(b'&', raw.as_bytes()).is_some() {
            return false;
        }
        if !self.attribute_lists {
            return true;
        }
        let name = raw.get(1..).unwrap_or_default();
        let end = (name.bytes())
            .position(|byte| {
                !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'.' | b'-'))
            })
            .unwrap_or(name.len());
        // The parser names an element by its longest Name (`<r'x'>` is `r`),
        // so a name that does not end at a delimiter, or is not ASCII, takes
        // the declarations path.
        if !matches!(
            name.as_bytes().get(end),
            None | Some(b' ' | b'\t' | b'\r' | b'\n' | b'/' | b'>')
        ) {
            return false;
        }
        self.element_decl(&name[..end])
            .is_none_or(|element| self.dtd.elements[element].attributes.is_empty())
    }

    /// The declaration of element `name`, from its attribute list or
    /// element declaration.
    pub(crate) fn element_decl(&mut self, name: &str) -> Option<usize> {
        self.recent_elements.get(name).unwrap_or_else(|| {
            let element = self.dtd.element_index.get(name).copied();
            self.recent_elements.put(name, element);
            element
        })
    }

    /// The attributes `element` declares.
    pub(crate) fn element_attributes(&self, element: usize) -> &[AttributeDecl] {
        &self.dtd.elements[element].attributes
    }

    /// The position of `element`'s declaration of attribute `name`.
    pub(crate) fn attribute_position(&self, element: usize, name: &str) -> Option<usize> {
        self.dtd.elements[element].attribute(name)
    }

    pub(crate) fn attributes(&self, element: &str) -> impl Iterator<Item = &AttributeDecl> {
        self.dtd
            .element_index
            .get(element)
            .map(|index| self.dtd.elements[*index].attributes.as_slice())
            .unwrap_or(&[])
            .iter()
    }

    /// Applies one declaration, returning the warning libxml2 raises for it
    /// and where in `unit`: an invalid redeclaration of a predefined entity
    /// (dropped), a redefined entity under `PEDANTIC` or an `xml:id`
    /// attribute declared other than ID. Failure
    /// offsets are relative to `unit`, or absent inside replacement text.
    pub(crate) fn apply(
        &mut self,
        unit: &DtdUnit<'_>,
        context: DeclarationContext,
    ) -> Result<Option<(crate::Warning, usize)>, DtdError> {
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
                let (mut input, mut written) = (None, None);
                // WFC Entity Declared: a standalone document cannot use general
                // entities from external markup, so they are stored (libxml2
                // still finds them for ENTITY attributes) but never bound.
                let bound = parameter || !self.standalone || context.internal_subset;
                let existing = match (parameter, bound) {
                    (true, _) => self.dtd.parameter.get(entity.name()).copied(),
                    (false, true) => self.dtd.general.get(entity.name()).copied(),
                    (false, false) => self.dtd.declared(entity.name()),
                };
                let duplicate = existing.is_some();
                // libxml2 keeps one table per subset and warns under PEDANTIC
                // about a second declaration in the same one.
                let redefined = existing
                    .filter(|id| {
                        self.limits.flags.pedantic()
                            && self.dtd.entities[id.0].external_subset
                                == self.reading_external_subset
                    })
                    .map(|_| (crate::Warning::EntityRedefined, 0));
                let (body, bytes) = match entity.definition() {
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
                            return Ok(Some((crate::Warning::PredefinedEntity, 0)));
                        }
                        if duplicate {
                            return Ok(redefined);
                        }
                        let bytes = result.len();
                        match result == content {
                            true => input = context.input.map(|unit| unit + offset),
                            false => written = Some(copy_string(content)?.into_boxed_str()),
                        }
                        let bytes = bytes + written.as_ref().map_or(0, |text| text.len());
                        (EntityBody::Internal(result.into_owned()), bytes)
                    }
                    EntityDefinitionView::External {
                        identifier,
                        notation,
                    } => {
                        // A recovered "Fragment not allowed" (src/dtd.rs).
                        if parameter && identifier.system.is_some_and(|s| s.contains('#')) {
                            return Ok(None);
                        }
                        if !parameter && predefined(entity.name()).is_some() {
                            return Ok(Some((crate::Warning::PredefinedEntity, 0)));
                        }
                        if duplicate {
                            return Ok(redefined);
                        }
                        let bytes = checked_sum(&[
                            identifier
                                .public
                                .map_or(0, |raw| identifier_length(raw, context.literals)),
                            identifier
                                .system
                                .map_or(0, |raw| identifier_length(raw, context.literals)),
                            notation.map_or(0, str::len),
                        ])?;
                        let external = self.external_entity(identifier, notation, context)?;
                        (EntityBody::External(external), bytes)
                    }
                };
                self.store_bytes(checked_sum(&[entity.name().len(), bytes])?)?;
                self.add_binding()?;
                let stored = (parameter, bound, input, written);
                self.insert_entity(entity.name(), stored, body)?;
            }
            DtdUnitView::AttributeList(list) => {
                let mut warning = None;
                for definition in list.definitions() {
                    let kind = definition.kind();
                    if definition.name() == "xml:id" && kind != AttributeKind::Id {
                        warning.get_or_insert((crate::Warning::XmlIdType, definition.end()));
                    }
                    let default = match definition.default() {
                        DefaultDeclarationView::Required => StoredDefault::Required,
                        DefaultDeclarationView::Implied => StoredDefault::Implied,
                        DefaultDeclarationView::Value { fixed, literal } => {
                            let (content, offset) = literal.content();
                            let tokenized = kind != AttributeKind::Cdata;
                            let value = self.process_value(
                                content,
                                offset,
                                context.literals,
                                ValueMode::Attribute { tokenized },
                                &mut ValueRequest::default(),
                            )?;
                            let name = definition.name();
                            let declaration = name == "xmlns" || name.starts_with("xmlns:");
                            let kept = match self.limits.flags.substitutes() || declaration {
                                true => None,
                                false => crate::dtd_tag::kept_form(
                                    content,
                                    tokenized,
                                    context.literals == LiteralSource::Physical,
                                    |name| self.keeps(name),
                                    |_| Ok(()),
                                )?,
                            };
                            self.store_bytes(kept.as_ref().map_or(0, String::len))?;
                            StoredDefault::Value {
                                fixed,
                                value: value.into_owned(),
                                kept,
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
                        .attribute(definition.name())
                        .is_some()
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
                    self.dtd.elements[element]
                        .push_attribute(attribute)
                        .map_err(|()| allocation())?;
                    self.attribute_lists = true;
                }
                return Ok(warning);
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
                let identifier = notation.identifier();
                let length = |raw: Option<&str>| {
                    raw.map_or(0, |raw| identifier_length(raw, context.literals))
                };
                self.store_bytes(checked_sum(&[
                    notation.name().len(),
                    length(identifier.public),
                    length(identifier.system),
                ])?)?;
                self.dtd.declare_notation(NotationDecl {
                    name: copy_string(notation.name())?,
                    public: copy_identifier(identifier.public, context.literals)?,
                    system: copy_identifier(identifier.system, context.literals)?,
                    internal: context.internal_subset,
                });
            }
            _ => {}
        }
        Ok(None)
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

    /// Stores a declaration that is not a duplicate; a `bound` one is also
    /// found by references.
    fn insert_entity(
        &mut self,
        name: &str,
        (parameter, bound, input, literal): (bool, bool, Option<usize>, Option<Box<str>>),
        body: EntityBody,
    ) -> Result<(), DtdError> {
        let dtd = &mut self.dtd;
        let id = EntityId(dtd.entities.len());
        dtd.entities.try_reserve(1).map_err(|_| allocation())?;
        dtd.entities.push(EntityDecl {
            name: copy_string(name)?,
            body,
            parameter,
            external_subset: self.reading_external_subset,
            input,
            literal,
        });
        // `apply` returned for duplicates: the name is new to its table.
        let table = match (bound, parameter) {
            (false, true) => return Ok(()),
            (false, false) => &mut dtd.unbound,
            (true, true) => &mut dtd.parameter,
            (true, false) => &mut dtd.general,
        };
        table.try_reserve(1).map_err(|_| allocation())?;
        table.insert(copy_string(name)?, id);
        Ok(())
    }

    fn external_entity(
        &self,
        identifier: crate::dtd::ExternalIdView<'_>,
        notation: Option<&str>,
        context: DeclarationContext,
    ) -> Result<ExternalEntity, DtdError> {
        let system = copy_identifier(identifier.system, context.literals)?;
        let uri = (self.loader.as_ref().zip(system.as_deref()))
            .map(|(loader, system)| loader.uri(system));
        Ok(ExternalEntity {
            public: copy_identifier(identifier.public, context.literals)?,
            system,
            uri,
            notation: notation.map(copy_string).transpose()?,
            text: None,
            loaded: false,
        })
    }

    /// Retains `bytes` more for the declarations: the stored text, and the
    /// scanned replacement text content keeps.
    pub(crate) fn store_bytes(&mut self, bytes: usize) -> Result<(), DtdError> {
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

    /// Normalizes an attribute value (`tokenized` unless CDATA), spending
    /// at most `caller_value_bytes` of new capacity, recorded in
    /// `requested`. With `borrow`, an unchanged physical literal is
    /// returned as is.
    pub(crate) fn normalize_value<'raw>(
        &mut self,
        literal_content: &'raw str,
        (source, borrow, tokenized): (LiteralSource, bool, bool),
        caller_value_bytes: usize,
        requested: &mut usize,
    ) -> Result<Cow<'raw, str>, DtdError> {
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
        // Callers read `undeclared_references` after every value, unchanged ones too.
        self.undeclared.clear();
        if let Some(value) = self.unchanged(raw, mode, request.borrow_physical) {
            return Ok(value);
        }
        let may_be_unread = self.may_be_unread();
        let (usage, requested) = (self.usage, request.requested);
        let noted = self.dtd.issues.len();
        loop {
            let mut processor = ValueProcessor {
                general: &self.dtd.general,
                parameter: &self.dtd.parameter,
                entities: &self.dtd.entities,
                frames: &mut self.frames,
                limits: self.limits,
                amplification_ceiling: self.amplification_ceiling,
                request: &mut *request,
                usage: &mut self.usage,
                issues: &mut self.dtd.issues,
                undeclared: &mut self.undeclared,
                reference: offset,
                may_be_unread,
                // xmlExpandPEsInEntityValue loads external PEs only to
                // substitute or validate; otherwise they expand to nothing.
                loadable: self.loader.is_some()
                    && (self.limits.flags.substitutes() || self.limits.flags.validates()),
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
                    self.undeclared.clear();
                    self.load(id)?;
                }
            }
        }
    }
}

impl DtdCatalog {
    /// `raw` when processing in `mode` leaves it unchanged, its bytes
    /// charged as `ValueProcessor` charges them one by one: a unit of work
    /// and an expanded byte each, and a second unit matching a physical
    /// literal that is `borrow`ed. Anything else, including a charge that
    /// would fail at some byte, is left to `ValueProcessor`.
    fn unchanged<'raw>(
        &mut self,
        raw: &'raw str,
        mode: ValueMode,
        borrow: bool,
    ) -> Option<Cow<'raw, str>> {
        let bytes = raw.as_bytes();
        let plain = match mode {
            ValueMode::Entity => memchr::memchr3(b'&', b'%', b'\r', bytes).is_none(),
            ValueMode::Attribute { tokenized } => {
                memchr::memchr3(b'&', b'<', b'\r', bytes).is_none()
                    && memchr::memchr2(b'\t', b'\n', bytes).is_none()
                    && !(tokenized
                        && (raw.starts_with(' ') || raw.ends_with(' ') || raw.contains("  ")))
            }
        };
        let work = raw.len().checked_mul(1 + usize::from(borrow))?;
        let work = self.usage.work.checked_add(work)?;
        let expanded = self.usage.expanded_bytes.checked_add(raw.len())?;
        let within = raw.len() <= self.limits.token
            && work <= self.limits.work
            && expanded <= self.limits.expansion;
        (plain && within).then(|| {
            (self.usage.work, self.usage.expanded_bytes) = (work, expanded);
            Cow::Borrowed(raw)
        })
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
    entities: &'a [EntityDecl],
    frames: &'a mut Vec<ValueFrame>,
    limits: DtdLimits,
    amplification_ceiling: usize,
    request: &'a mut ValueRequest,
    usage: &'a mut CatalogUsage,
    issues: &'a mut Vec<(Code, Option<usize>, String)>,
    undeclared: &'a mut Vec<usize>,
    /// The offset of the outermost reference being expanded.
    reference: usize,
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
        let mut output = ValueOutput {
            owned: String::new(),
            matching: self.request.borrow_physical.then_some((self.raw, 0)),
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
            // Literals and replacement texts hold only XML characters,
            // checked when their declarations or tags were read.
            let mut consumed = character.len_utf8();
            let is_entity = matches!(self.mode, ValueMode::Entity);
            if character == '&' || (is_entity && character == '%') {
                if let Some(at) = at {
                    self.reference = at;
                }
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
                                self.undeclared
                                    .try_reserve(1)
                                    .map_err(|_| Stop::Fail(DtdError::new(ALLOCATION, at)))?;
                                self.undeclared.push(self.reference);
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
            // A literal's own `<` is its scanner's error (kept when tags
            // recover, as libxml2 keeps it); only replacement text's is ours.
            if !is_entity && character == '<' && frame.entity.is_some() {
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
        match output.matching {
            Some((raw, matched)) if matched == raw.len() => Ok(Cow::Borrowed(raw)),
            Some(matching) => Ok(Cow::Owned(self.materialize(matching, root)?)),
            None => Ok(Cow::Owned(output.owned)),
        }
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
        if let Some((raw, matched)) = &mut output.matching {
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
            let matching = (*raw, *matched);
            output.owned = self.materialize(matching, Some(self.offset))?;
            output.matching = None;
        }
        let output = &mut output.owned;
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

    /// An owned copy of the `matched` prefix of `raw`.
    fn materialize(
        &mut self,
        (raw, matched): (&str, usize),
        at: Option<usize>,
    ) -> Result<String, DtdError> {
        increment(
            &mut self.usage.work,
            matched,
            self.limits.work,
            EXPANSION,
            at,
        )?;
        let mut owned = String::new();
        if matched != 0 {
            let target = matched.max(64).min(self.limits.token);
            self.request_capacity(target, at)?;
            owned
                .try_reserve_exact(target)
                .map_err(|_| DtdError::new(ALLOCATION, at))?;
            owned.push_str(&raw[..matched]);
        }
        Ok(owned)
    }

    /// Spends `bytes` of the caller's value capacity, when it set one.
    fn request_capacity(&mut self, bytes: usize, at: Option<usize>) -> Result<(), DtdError> {
        if let Some(ceiling) = self.request.ceiling {
            increment(&mut self.request.requested, bytes, ceiling, OWNED, at)?;
        }
        Ok(())
    }
}

/// A value being built: while `matching`, the prefix of the physical
/// literal it equals; otherwise `owned`.
struct ValueOutput<'raw> {
    owned: String,
    matching: Option<(&'raw str, usize)>,
}
impl ValueOutput<'_> {
    fn len(&self) -> usize {
        self.matching
            .map_or(self.owned.len(), |(_, matched)| matched)
    }
}

enum Reference<'a> {
    Character(char),
    Named(&'a str),
}

/// The reference spelled at the start of `raw` through its `;`, and its value.
/// A parameter reference, validated with its declaration, names an entity.
fn parse_reference(raw: &str) -> Option<(&str, Reference<'_>)> {
    let spelling = &raw[..=raw.find(';')?];
    let name = &spelling[1..spelling.len() - 1];
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
