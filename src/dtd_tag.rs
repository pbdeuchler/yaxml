//! DTD-aware start-tag preparation: attribute normalization and defaults.

use std::borrow::Cow;
use std::collections::HashSet;

use crate::dtd_catalog::{AttributeKind, DtdCatalog, LiteralSource};
use crate::name::qname;
use crate::parser::{Budget, PreparedAttribute, PreparedTag, RawTag};
use crate::valid::Code;
use crate::{DtdError, ErrorKind, Limit, Name};

/// Normalizes `raw`'s attributes and adds its defaults. Failure offsets are
/// input offsets, or absent for a defaulted attribute. With `borrow`, an
/// unchanged physical literal is not copied.
pub(crate) fn prepare_dtd_tag<'event>(
    catalog: &mut DtdCatalog,
    budget: &mut Budget,
    raw: RawTag<'event>,
    source: LiteralSource,
    borrow: bool,
) -> Result<PreparedTag<'event>, DtdError> {
    let input = |kind, offset| DtdError::new(kind, Some(offset));
    let declared = |kind| DtdError::new(kind, None);
    let mut explicit = HashSet::new();
    explicit
        .try_reserve(raw.attributes.len())
        .map_err(|_| input(ErrorKind::Allocation, raw.start))?;
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
        .map_err(|_| input(ErrorKind::Allocation, raw.start))?;
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
        .map_err(|_| input(ErrorKind::Allocation, raw.start))?;
    for (attribute, name) in raw.attributes.into_iter().zip(names) {
        // libxml2's XML_ATTVAL_NORM_CHANGE, judged on the literal's whitespace.
        if standalone
            && catalog.attributes(raw.qualified).any(|binding| {
                binding.name == attribute.qualified
                    && !binding.internal
                    && binding.kind != AttributeKind::Cdata
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
        let mut requested = 0;
        let normalized = catalog.normalize_attribute(
            raw.qualified,
            attribute.qualified,
            attribute.literal,
            source,
            borrow,
            budget.remaining_owned(),
            &mut requested,
        );
        budget.charge_owned(requested, attribute.value_offset)?;
        let value = normalized.map_err(|error| DtdError {
            offset: error.offset.map(|offset| attribute.value_offset + offset),
            ..error
        })?;
        attributes.push(PreparedAttribute {
            name,
            value,
            name_offset: Some(attribute.name_offset),
        });
    }

    // Copied defaults amplify output like entities do (libxml2 `expandedSize`).
    let copied = catalog
        .attributes(raw.qualified)
        .filter(|binding| !explicit.contains(binding.name.as_str()))
        .filter_map(|binding| {
            let value = binding.default.value()?;
            Some(binding.name.len() + value.len() + crate::dtd_controller::ENTITY_FIXED_COST)
        })
        .fold(0_usize, usize::saturating_add);
    catalog.charge_default(copied).map_err(|error| DtdError {
        offset: error.offset.map(|offset| raw.start + offset),
        ..error
    })?;
    let mut defaulted = Vec::new();
    // Without `DTDATTR` libxml2 applies only defaulted namespace declarations.
    let complete = catalog.default_attributes();
    for binding in catalog.attributes(raw.qualified) {
        if explicit.contains(binding.name.as_str()) {
            continue;
        }
        let Some(value) = binding.default.value() else {
            continue;
        };
        if standalone && !binding.internal {
            defaulted.push(format!(
                "standalone: attribute {} on {} defaulted from external subset",
                binding.name, raw.qualified
            ));
        }
        if !complete && binding.name != "xmlns" && !binding.name.starts_with("xmlns:") {
            continue;
        }
        let (prefix, _) = qname(&binding.name, 0).map_err(|error| declared(error.kind))?;
        if attributes.len() == budget.max_attributes() {
            return Err(declared(ErrorKind::LimitExceeded(Limit::Attributes)));
        }
        attributes
            .try_reserve(1)
            .map_err(|_| declared(ErrorKind::Allocation))?;
        let mut copy = |text| budget.copy(text, 0).map_err(|error| declared(error.kind));
        let qualified = copy(&binding.name)?;
        let value = copy(value)?;
        attributes.push(PreparedAttribute {
            name: Name::from_validated_parts(
                qualified,
                prefix.and_then(|value| std::num::NonZeroUsize::new(value.len())),
                None,
            ),
            value,
            name_offset: None,
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
) -> Result<Name<'event>, DtdError> {
    let qualified = match source {
        LiteralSource::Physical => Cow::Borrowed(raw),
        LiteralSource::Replacement => budget.copy(raw, offset)?,
    };
    Ok(Name::from_validated_parts(qualified, prefix_length, None))
}
