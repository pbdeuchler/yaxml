//! Versioned option structs to core configuration. A field of 0 keeps the
//! core default, so a zeroed struct is the hostile-input policy.

use yaxml::{DecodeLimits, EditLimits, Limits, ParseOptions};

use crate::error::Fail;
use crate::ffi::{text, versioned};
use crate::types::{yaxml_edit_limits, yaxml_parse_options};

/// `value`, or `default` for 0.
fn pick(value: u64, default: usize) -> usize {
    match value {
        0 => default,
        v => usize::try_from(v).unwrap_or(usize::MAX),
    }
}

/// # Safety
/// `options` is NULL or a `yaxml_parse_options` per yaxml.h.
pub(crate) unsafe fn parse_options(
    options: *const yaxml_parse_options,
) -> Result<ParseOptions, Fail> {
    // SAFETY: forwarded contract.
    let Some(o) = (unsafe { versioned(options) })? else {
        return Ok(ParseOptions::default());
    };
    // SAFETY: yaxml.h: `base` is a valid string or NULL.
    let base = unsafe { text(o.base) }?;
    let d = Limits::DEFAULT;
    let limits = Limits::DEFAULT
        .max_input_bytes(pick(o.max_input_bytes, d.max_input_bytes))
        .max_token_bytes(pick(o.max_token_bytes, d.max_token_bytes))
        .max_depth(pick(o.max_depth, d.max_depth))
        .max_attributes(pick(o.max_attributes, d.max_attributes))
        .max_events(pick(o.max_events, d.max_events))
        .max_owned_bytes(pick(o.max_owned_bytes, d.max_owned_bytes))
        .max_dtd_bytes(pick(o.max_dtd_bytes, d.max_dtd_bytes))
        .max_entity_expansion_bytes(pick(
            o.max_entity_expansion_bytes,
            d.max_entity_expansion_bytes,
        ))
        .max_entity_amplification(pick(o.max_entity_amplification, d.max_entity_amplification))
        .max_entity_references(pick(o.max_entity_references, d.max_entity_references))
        .max_entity_depth(pick(o.max_entity_depth, d.max_entity_depth))
        .max_external_loads(pick(o.max_external_loads, d.max_external_loads))
        .max_external_bytes(pick(o.max_external_bytes, d.max_external_bytes));
    let dd = DecodeLimits::DEFAULT;
    let decode = DecodeLimits::DEFAULT
        .max_decoded_bytes(pick(o.max_decoded_bytes, dd.max_decoded_bytes))
        .max_decoded_token_bytes(pick(o.max_decoded_token_bytes, dd.max_decoded_token_bytes));
    Ok(ParseOptions::new(o.flags)
        .limits(limits)
        .decode_limits(decode)
        .base(base.map(str::to_owned)))
}

/// # Safety
/// `limits` is NULL or a `yaxml_edit_limits` per yaxml.h.
pub(crate) unsafe fn edit_limits(limits: *const yaxml_edit_limits) -> Result<EditLimits, Fail> {
    // SAFETY: forwarded contract.
    let Some(l) = (unsafe { versioned(limits) })? else {
        return Ok(EditLimits::DEFAULT);
    };
    let d = EditLimits::DEFAULT;
    Ok(EditLimits::DEFAULT
        .max_value_bytes(pick(l.max_value_bytes, d.max_value_bytes))
        .max_owned_bytes(pick(l.max_owned_bytes, d.max_owned_bytes))
        .max_created_nodes(pick(l.max_created_nodes, d.max_created_nodes))
        .max_name_bytes(pick(l.max_name_bytes, d.max_name_bytes))
        .max_supplied_bytes(pick(l.max_supplied_bytes, d.max_supplied_bytes))
        .max_attributes(pick(l.max_attributes, d.max_attributes))
        .max_depth(pick(l.max_depth, d.max_depth))
        .max_created_attributes(pick(l.max_created_attributes, d.max_created_attributes))
        .max_namespace_work(pick(l.max_namespace_work, d.max_namespace_work)))
}
