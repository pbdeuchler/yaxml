//! Private DTD-aware start-tag preparation. No public parser enables it yet.

use std::borrow::Cow;
use std::collections::HashSet;

use crate::dtd_catalog::{
    CatalogError, CatalogErrorKind, DefaultValue, DtdCatalog, LiteralSource, ValueLocation,
};
use crate::name::qname;
use crate::parser::{
    AttributeProvenance, Budget, PreparedAttribute, PreparedTag, PreparedTagError,
    PreparedTagErrorLocation, RawTag,
};
use crate::valid::Code;
use crate::{Error, ErrorKind, Limit, Name};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DtdTagErrorKind {
    Parser(ErrorKind),
    Catalog(CatalogErrorKind),
    PositionOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DtdTagLocation {
    Input(usize),
    Declaration { declaration: usize, offset: usize },
    Value(ValueLocation),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DtdTagError {
    pub(crate) kind: DtdTagErrorKind,
    pub(crate) location: DtdTagLocation,
}

impl From<Error> for DtdTagError {
    fn from(error: Error) -> Self {
        Self {
            kind: DtdTagErrorKind::Parser(error.kind),
            location: DtdTagLocation::Input(error.offset),
        }
    }
}

impl From<PreparedTagError> for DtdTagError {
    fn from(error: PreparedTagError) -> Self {
        let location = match error.location {
            PreparedTagErrorLocation::Input(offset) => DtdTagLocation::Input(offset),
            PreparedTagErrorLocation::Declaration {
                declaration,
                offset,
            } => DtdTagLocation::Declaration {
                declaration,
                offset,
            },
        };
        Self {
            kind: DtdTagErrorKind::Parser(error.kind),
            location,
        }
    }
}

pub(crate) fn prepare_dtd_tag<'event>(
    catalog: &mut DtdCatalog,
    budget: &mut Budget,
    raw: RawTag<'event>,
    source: LiteralSource,
) -> Result<PreparedTag<'event>, DtdTagError> {
    prepare_tag(catalog, budget, raw, source, false)
}

pub(crate) fn prepare_stable_dtd_tag<'event>(
    catalog: &mut DtdCatalog,
    budget: &mut Budget,
    raw: RawTag<'event>,
) -> Result<PreparedTag<'event>, DtdTagError> {
    prepare_tag(catalog, budget, raw, LiteralSource::Physical, true)
}

fn prepare_tag<'event>(
    catalog: &mut DtdCatalog,
    budget: &mut Budget,
    raw: RawTag<'event>,
    source: LiteralSource,
    borrow_physical: bool,
) -> Result<PreparedTag<'event>, DtdTagError> {
    let mut explicit = HashSet::new();
    explicit
        .try_reserve(raw.attributes.len())
        .map_err(|_| input_error(ErrorKind::Allocation, raw.start))?;
    for attribute in &raw.attributes {
        explicit.insert(attribute.qualified);
    }

    let name = retained_name(
        raw.qualified,
        raw.prefix_length,
        source,
        raw.start + 1,
        budget,
    )?;
    let mut names = Vec::new();
    names
        .try_reserve(raw.attributes.len())
        .map_err(|_| input_error(ErrorKind::Allocation, raw.start))?;
    for attribute in &raw.attributes {
        debug_assert_eq!(
            attribute.value_end.checked_sub(attribute.value_offset),
            Some(attribute.literal.len())
        );
        names.push(retained_name(
            attribute.qualified,
            attribute.prefix_length,
            source,
            attribute.name_offset,
            budget,
        )?);
    }

    // Offsets are physical only outside entity replacement text.
    let physical = |offset| (source == LiteralSource::Physical).then_some(offset);
    let standalone = catalog.standalone();
    let mut attributes = Vec::new();
    attributes
        .try_reserve(raw.attributes.len())
        .map_err(|_| input_error(ErrorKind::Allocation, raw.start))?;
    for (attribute, name) in raw.attributes.into_iter().zip(names) {
        // libxml2's XML_ATTVAL_NORM_CHANGE, judged on the literal's whitespace.
        if standalone
            && catalog.attributes(raw.qualified).any(|binding| {
                binding.name() == attribute.qualified && !binding.internal() && binding.tokenized()
            })
            && tokenizing_changes(attribute.literal)
        {
            let message = format!(
                "standalone: normalization of attribute {} on {} by external subset declaration",
                attribute.qualified, raw.qualified
            );
            catalog.note(
                Code::NotStandalone,
                physical(attribute.name_offset),
                message,
            );
        }
        let mut requested_value_bytes = 0;
        let normalized = if borrow_physical {
            catalog.normalize_attribute_borrowed_bounded(
                raw.qualified,
                attribute.qualified,
                attribute.literal,
                budget.remaining_owned(),
                &mut requested_value_bytes,
            )
        } else {
            catalog
                .normalize_attribute_value_bounded(
                    raw.qualified,
                    attribute.qualified,
                    attribute.literal,
                    source,
                    budget.remaining_owned(),
                    &mut requested_value_bytes,
                )
                .map(Cow::Owned)
        };
        budget
            .charge_owned(requested_value_bytes, attribute.value_offset)
            .map_err(DtdTagError::from)?;
        let value =
            normalized.map_err(|error| rebase_catalog_error(error, attribute.value_offset))?;
        attributes.push(PreparedAttribute {
            name,
            value,
            provenance: AttributeProvenance::Specified {
                name_offset: attribute.name_offset,
                value_offset: attribute.value_offset,
            },
        });
    }

    // Copied defaults amplify output like entities do (libxml2 `expandedSize`).
    let copied = catalog
        .attributes(raw.qualified)
        .filter(|binding| !explicit.contains(binding.name()))
        .filter_map(|binding| match binding.default() {
            DefaultValue::Value { value, .. } => {
                Some(binding.name().len() + value.len() + crate::dtd_controller::ENTITY_FIXED_COST)
            }
            _ => None,
        })
        .fold(0_usize, usize::saturating_add);
    catalog
        .charge_default(copied)
        .map_err(|error| rebase_catalog_error(error, raw.start))?;
    let mut defaulted = Vec::new();
    for binding in catalog.attributes(raw.qualified) {
        if explicit.contains(binding.name()) {
            continue;
        }
        let value = match binding.default() {
            DefaultValue::Required | DefaultValue::Implied => continue,
            DefaultValue::Value { value, .. } => value,
        };
        if standalone && !binding.internal() {
            defaulted.push(format!(
                "standalone: attribute {} on {} defaulted from external subset",
                binding.name(),
                raw.qualified
            ));
        }
        let name_offset = binding.name_offset();
        let (prefix, _) = qname(binding.name(), name_offset).map_err(|error| DtdTagError {
            kind: DtdTagErrorKind::Parser(error.kind),
            location: DtdTagLocation::Declaration {
                declaration: binding.declaration(),
                offset: error.offset,
            },
        })?;
        if attributes.len() == budget.max_attributes() {
            return Err(DtdTagError {
                kind: DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(Limit::Attributes)),
                location: DtdTagLocation::Declaration {
                    declaration: binding.declaration(),
                    offset: 0,
                },
            });
        }
        attributes
            .try_reserve(1)
            .map_err(|_| declaration_error(ErrorKind::Allocation, binding.declaration(), 0))?;
        let qualified = budget
            .copy(binding.name(), 0)
            .map_err(|error| declaration_error(error.kind, binding.declaration(), 0))?;
        let value = budget
            .copy(value, 0)
            .map_err(|error| declaration_error(error.kind, binding.declaration(), 0))?;
        attributes.push(PreparedAttribute {
            name: Name::from_validated_parts(
                qualified,
                prefix.and_then(|value| std::num::NonZeroUsize::new(value.len())),
                None,
            ),
            value,
            provenance: AttributeProvenance::Defaulted {
                declaration: binding.declaration(),
            },
        });
    }

    for message in defaulted {
        catalog.note(Code::StandaloneDefaulted, physical(raw.start), message);
    }
    Ok(PreparedTag {
        name,
        attributes,
        empty: raw.empty,
        start: raw.start,
    })
}

/// Whether discarding leading, trailing and repeated spaces changes a
/// literal after whitespace normalization; references are not expanded.
fn tokenizing_changes(literal: &str) -> bool {
    let mut previous = true;
    for c in literal.chars().filter(|&c| c != '\r') {
        let space = matches!(c, ' ' | '\t' | '\n');
        if space && previous {
            return true;
        }
        previous = space;
    }
    previous && !literal.is_empty()
}

fn retained_name<'event>(
    raw: &'event str,
    prefix_length: Option<std::num::NonZeroUsize>,
    source: LiteralSource,
    offset: usize,
    budget: &mut Budget,
) -> Result<Name<'event>, DtdTagError> {
    let qualified = match source {
        LiteralSource::Physical => Cow::Borrowed(raw),
        LiteralSource::Replacement => budget.copy(raw, offset).map_err(DtdTagError::from)?,
    };
    Ok(Name::from_validated_parts(qualified, prefix_length, None))
}

fn input_error(kind: ErrorKind, offset: usize) -> DtdTagError {
    DtdTagError {
        kind: DtdTagErrorKind::Parser(kind),
        location: DtdTagLocation::Input(offset),
    }
}

fn declaration_error(kind: ErrorKind, declaration: usize, offset: usize) -> DtdTagError {
    DtdTagError {
        kind: DtdTagErrorKind::Parser(kind),
        location: DtdTagLocation::Declaration {
            declaration,
            offset,
        },
    }
}

fn rebase_catalog_error(error: CatalogError, base: usize) -> DtdTagError {
    match rebase_value_location(error.location, base) {
        Ok(location) => DtdTagError {
            kind: match error.kind {
                CatalogErrorKind::CallerValueBytes => {
                    DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(Limit::OwnedBytes))
                }
                kind => DtdTagErrorKind::Catalog(kind),
            },
            location: DtdTagLocation::Value(location),
        },
        Err(error) => error,
    }
}

fn rebase_value_location(
    location: ValueLocation,
    base: usize,
) -> Result<ValueLocation, DtdTagError> {
    match location {
        ValueLocation::Input(relative) => base
            .checked_add(relative)
            .map(ValueLocation::Input)
            .ok_or(DtdTagError {
                kind: DtdTagErrorKind::PositionOverflow,
                location: DtdTagLocation::Input(base),
            }),
        ValueLocation::Declaration { .. } | ValueLocation::Replacement { .. } => Ok(location),
    }
}

#[cfg(test)]
#[path = "../tests/internal/dtd_tag_author.rs"]
mod tests;
