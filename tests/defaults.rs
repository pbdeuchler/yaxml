//! Pins every default of the public configuration structs. The defaults are
//! the hostile-input policy; changing one is a security decision. The Debug
//! text lists every field, so adding a field fails here until it is pinned.

use yaxml::{DecodeLimits, EditLimits, Limits, ParseOptions, WriterLimits};
use yaxml::{catalog, html, pattern, relaxng, save, schematron, xinclude, xpath, xsd};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;

fn xml() -> String {
    format!(
        "Limits {{ max_input_bytes: {}, max_token_bytes: {}, max_depth: 256, max_attributes: 1024, max_total_attributes: 4000000, \
         max_events: 1000000, max_owned_bytes: {}, max_dtd_bytes: {}, max_entity_expansion_bytes: 1000000, \
         max_entity_amplification: 5, max_entity_references: {}, max_entity_depth: 20, \
         max_external_loads: 256, max_external_bytes: {} }}",
        64 * MIB,
        MIB,
        64 * MIB,
        16 * MIB,
        MIB,
        16 * MIB
    )
}

fn xpath_limits() -> String {
    format!(
        "Limits {{ max_expression_bytes: {}, max_depth: 200, max_operations: 50000000, max_nodes: 10000000 }}",
        64 * KIB
    )
}

/// (name, `Default::default()` Debug, `DEFAULT` Debug, expected Debug).
fn cases() -> Vec<(&'static str, String, String, String)> {
    macro_rules! case {
        ($name:literal, $ty:ty, $want:expr) => {
            (
                $name,
                format!("{:?}", <$ty>::default()),
                format!("{:?}", <$ty>::DEFAULT),
                $want,
            )
        };
    }
    vec![
        case!("Limits", Limits, xml()),
        case!(
            "DecodeLimits",
            DecodeLimits,
            format!(
                "DecodeLimits {{ max_decoded_bytes: {}, max_decoded_token_bytes: {} }}",
                64 * MIB,
                MIB
            )
        ),
        case!(
            "WriterLimits",
            WriterLimits,
            format!(
                "WriterLimits {{ max_output_bytes: {}, max_token_bytes: {}, max_depth: 256, \
                 max_attributes: 1024, max_events: 1000000, max_owned_bytes: {} }}",
                64 * MIB,
                MIB,
                64 * MIB
            )
        ),
        case!(
            "EditLimits",
            EditLimits,
            format!(
                "EditLimits {{ max_value_bytes: {}, max_owned_bytes: {}, max_created_nodes: 1000000, \
                 max_name_bytes: {}, max_supplied_bytes: {}, max_attributes: 1024, max_depth: 256, \
                 max_created_attributes: 1000000, max_namespace_work: {} }}",
                MIB,
                64 * MIB,
                MIB,
                8 * MIB,
                64 * MIB
            )
        ),
        case!(
            "ParseOptions",
            ParseOptions,
            format!(
                "ParseOptions {{ flags: 0x0, limits: {}, decode_limits: DecodeLimits {{ \
                 max_decoded_bytes: {}, max_decoded_token_bytes: {} }}, base: None, resolver: None, converters: None }}",
                xml(),
                64 * MIB,
                MIB
            )
        ),
        case!(
            "html::Options",
            html::Options,
            format!("Options {{ flags: 0, limits: {} }}", xml())
        ),
        case!(
            "save::Options",
            save::Options,
            "Options { flags: 0, encoding: None, indent: \"  \", level: 0 }".into()
        ),
        case!(
            "pattern::Limits",
            pattern::Limits,
            format!(
                "Limits {{ max_bytes: {}, max_steps: 256, max_depth: 1024 }}",
                64 * KIB
            )
        ),
        case!("xpath::Limits", xpath::Limits, xpath_limits()),
        case!(
            "catalog::Limits",
            catalog::Limits,
            format!(
                "Limits {{ max_loads: 64, max_bytes: {}, max_depth: 50, max_local: 64 }}",
                16 * MIB
            )
        ),
        case!(
            "xsd::Limits",
            xsd::Limits,
            format!(
                "Limits {{ xml: {}, max_documents: 256, max_bytes: {}, max_depth: 256, \
                 max_model_work: {}, max_model_bytes: {}, max_counter_vectors: 1024, \
                 max_idc_keys: {} }}",
                xml(),
                64 * MIB,
                16 * MIB,
                64 * MIB,
                MIB
            )
        ),
        case!(
            "relaxng::Limits",
            relaxng::Limits,
            format!(
                "Limits {{ xml: {}, max_include_depth: 64, max_work: 100000000 }}",
                xml()
            )
        ),
        case!(
            "schematron::Limits",
            schematron::Limits,
            format!(
                "Limits {{ xml: {}, xpath: {}, max_operations: 500000000 }}",
                xml(),
                xpath_limits()
            )
        ),
        case!(
            "xinclude::Options",
            xinclude::Options,
            format!(
                "Options {{ base_fixup: true, lang_fixup: false, max_depth: 40, limits: {}, xpath: {}, \
                 parse_flags: {} }}",
                xml(),
                xpath_limits(),
                yaxml::options::LEGACY
            )
        ),
    ]
}

#[test]
fn every_default_is_pinned_and_const_matches_default() {
    for (name, default, constant, want) in cases() {
        assert_eq!(default, want, "{name}::default()");
        assert_eq!(constant, want, "{name}::DEFAULT");
    }
}

#[test]
fn a_changed_default_is_detected() {
    let mut limits = Limits::default();
    limits.max_depth += 1;
    assert_ne!(format!("{limits:?}"), xml());
}

#[test]
fn setters_are_named_after_fields_and_chain() {
    // A field missing from its `setters!` list fails to compile in the crate.
    let limits = Limits::DEFAULT.max_depth(3).max_attributes(10);
    assert_eq!((limits.max_depth, limits.max_attributes), (3, 10));
    assert_eq!(limits.max_events, Limits::DEFAULT.max_events);
    const STATIC: Limits = Limits::DEFAULT.max_depth(7);
    assert_eq!(STATIC.max_depth, 7);

    let options = ParseOptions::DEFAULT
        .flags(8)
        .limits(limits)
        .base(Some("b".into()));
    assert_eq!(
        (options.flags, options.limits, options.base.as_deref()),
        (8, limits, Some("b"))
    );

    let save = save::Options::DEFAULT.encoding(Some("UTF-8")).level(2);
    assert_eq!(
        (save.encoding, save.level, save.indent),
        (Some("UTF-8"), 2, "  ")
    );
}
