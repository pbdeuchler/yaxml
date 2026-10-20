//! The hand-written header and the Rust definitions agree: the same exported
//! functions, and the same struct layouts (tests/c/abi_test.c asserts the
//! same numbers on the C side with `_Static_assert`).

use std::collections::BTreeSet;
use std::mem::{offset_of, size_of};

use yaxml_capi::types::*;

fn names(text: &str, marker: &str) -> BTreeSet<String> {
    text.match_indices(marker)
        .map(|(at, _)| {
            let rest = &text[at + marker.len()..];
            let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'));
            rest[..end.unwrap_or(rest.len())].to_owned()
        })
        .collect()
}

#[test]
fn header_declares_exactly_the_exported_functions() {
    let dir = env!("CARGO_MANIFEST_DIR");
    let header = std::fs::read_to_string(format!("{dir}/include/yaxml.h")).unwrap();
    let declared: BTreeSet<String> = header
        .lines()
        .filter(|line| !line.starts_with(' ') && !line.starts_with('#') && line.contains('('))
        .filter(|line| !line.starts_with("typedef") && !line.starts_with("/*"))
        .flat_map(|line| {
            names(line, " yaxml_")
                .into_iter()
                .chain(names(line, "*yaxml_"))
        })
        .filter(|name| !name.is_empty() && !name.ends_with("_t"))
        .map(|name| format!("yaxml_{name}"))
        .filter(|name| header.contains(&format!("{name}(")))
        .collect();
    let mut exported = BTreeSet::new();
    for file in std::fs::read_dir(format!("{dir}/src")).unwrap() {
        let source = std::fs::read_to_string(file.unwrap().path()).unwrap();
        exported.extend(
            names(&source, "extern \"C\" fn yaxml_")
                .into_iter()
                .map(|n| format!("yaxml_{n}")),
        );
    }
    assert_eq!(declared, exported);
    assert_eq!(exported.len(), 56);
}

#[test]
fn struct_layouts_match_the_header() {
    #[cfg(target_pointer_width = "64")]
    let expected = [
        ("yaxml_str", size_of::<yaxml_str>(), 16),
        ("yaxml_buffer", size_of::<yaxml_buffer>(), 16),
        ("yaxml_node", size_of::<yaxml_node>(), 16),
        ("yaxml_name", size_of::<yaxml_name>(), 64),
        ("yaxml_attribute", size_of::<yaxml_attribute>(), 80),
        ("yaxml_diagnostic", size_of::<yaxml_diagnostic>(), 48),
        ("yaxml_error", size_of::<yaxml_error>(), 64),
        ("yaxml_diagnostics", size_of::<yaxml_diagnostics>(), 24),
        ("yaxml_parse_options", size_of::<yaxml_parse_options>(), 144),
        ("yaxml_edit_limits", size_of::<yaxml_edit_limits>(), 80),
        ("yaxml_name_spec", size_of::<yaxml_name_spec>(), 56),
        ("yaxml_ns", size_of::<yaxml_ns>(), 32),
        ("yaxml_xpath_item", size_of::<yaxml_xpath_item>(), 24),
        ("yaxml_xpath_value", size_of::<yaxml_xpath_value>(), 48),
        ("yaxml_event", size_of::<yaxml_event>(), 168),
        ("error.diagnostic", offset_of!(yaxml_error, diagnostic), 16),
        (
            "diagnostic.message",
            offset_of!(yaxml_diagnostic, message),
            32,
        ),
        ("options.base", offset_of!(yaxml_parse_options, base), 8),
        (
            "options.max_decoded_token_bytes",
            offset_of!(yaxml_parse_options, max_decoded_token_bytes),
            136,
        ),
        (
            "limits.max_namespace_work",
            offset_of!(yaxml_edit_limits, max_namespace_work),
            72,
        ),
        ("spec.automatic", offset_of!(yaxml_name_spec, automatic), 48),
        ("value.string", offset_of!(yaxml_xpath_value, string), 16),
        ("event.name", offset_of!(yaxml_event, name), 8),
        ("event.attributes", offset_of!(yaxml_event, attributes), 136),
        ("event.end", offset_of!(yaxml_event, end), 160),
    ];
    #[cfg(target_pointer_width = "64")]
    for (what, actual, expected) in expected {
        assert_eq!(actual, expected, "{what}");
    }
}
