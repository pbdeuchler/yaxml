use super::*;

use crate::dtd_catalog::{
    CatalogLimits, DeclarationContext, DefaultValue, DtdCatalog, EntityLookup,
};

const PHYSICAL: DeclarationContext = DeclarationContext {
    internal_subset: true,
    literals: LiteralSource::Physical,
};
const REPLACEMENT: DeclarationContext = DeclarationContext {
    internal_subset: true,
    literals: LiteralSource::Replacement,
};

fn unit(raw: &str) -> DtdUnit<'_> {
    DtdUnit::parse(raw, DtdLimits::default()).expect("literal test unit")
}

fn output(
    raw: &str,
    source: LiteralSource,
) -> Result<(Vec<String>, String), UnrepresentableIdentifier> {
    let parsed = unit(raw);
    let mut fragments = parsed.physical_fragments(source);
    let mut pieces = Vec::new();
    let mut joined = String::new();
    for fragment in fragments.by_ref() {
        let fragment = fragment?;
        assert!(!fragment.is_empty(), "iterator must omit empty fragments");
        pieces.push(fragment.to_owned());
        joined.push_str(fragment);
    }
    assert_eq!(fragments.next(), None, "iterator must be fused");
    Ok((pieces, joined))
}

fn first_error(raw: &str, source: LiteralSource) -> (Vec<String>, UnrepresentableIdentifier) {
    let parsed = unit(raw);
    let mut fragments = parsed.physical_fragments(source);
    let mut prefix = Vec::new();
    let error = loop {
        match fragments.next().expect("identifier CR must fail") {
            Ok(fragment) => {
                assert!(!fragment.is_empty());
                prefix.push(fragment.to_owned());
            }
            Err(error) => break error,
        }
    };
    assert_eq!(fragments.next(), None, "error must terminate the iterator");
    assert_eq!(fragments.next(), None, "error termination must be fused");
    (prefix, error)
}

fn loose_catalog() -> DtdCatalog {
    DtdCatalog::new(CatalogLimits::default(), false)
}

fn apply(catalog: &mut DtdCatalog, raw: &str, context: DeclarationContext) {
    catalog
        .apply(&unit(raw), context)
        .expect("literal declaration");
}

fn general_replacement<'a>(catalog: &'a DtdCatalog, name: &str) -> &'a str {
    let EntityLookup::Internal { replacement, .. } = catalog.lookup_general(name) else {
        panic!("expected internal general entity")
    };
    replacement
}

#[test]
fn physical_phase_is_one_exact_raw_fragment_for_every_unit_family() {
    let cases = [
        "<!DOCTYPE r SYSTEM 's\r\nu' [",
        "<!ELEMENT r (a|β)*>",
        "<!ATTLIST r a CDATA 'x\r\ny'>",
        "<!ENTITY e 'x\r&#13;&amp;π'>",
        "<!ENTITY e SYSTEM 's\r\nu'>",
        "<!NOTATION n PUBLIC 'p\r\nq'>",
        "<!--a\rb-->",
        "<?p a\rb?>",
    ];
    for raw in cases {
        let (fragments, actual) = output(raw, LiteralSource::Physical).unwrap();
        assert_eq!(fragments, [raw]);
        assert_eq!(actual.as_bytes(), raw.as_bytes());
    }
}

#[test]
fn replacement_entity_values_escape_only_literal_carriage_returns() {
    let raw = "<!ENTITY e \"A\r&#13;&#xD;&amp;π\r\">";
    let expected = "<!ENTITY e \"A&#13;&#13;&#xD;&amp;π&#13;\">";
    let (fragments, actual) = output(raw, LiteralSource::Replacement).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        fragments,
        [
            "<!ENTITY e \"A",
            "&#13;",
            "&#13;&#xD;&amp;π",
            "&#13;",
            "\">"
        ]
    );

    let quoted = "<!ENTITY q '\r&#13;&amp;\"π\r'>";
    let expected = "<!ENTITY q '&#13;&#13;&amp;\"π&#13;'>";
    assert_eq!(
        output(quoted, LiteralSource::Replacement).unwrap().1,
        expected
    );
}

#[test]
fn replacement_attribute_values_preserve_reference_syntax_and_crlf_semantics() {
    let raw = concat!(
        "<!ATTLIST r ",
        "a CDATA \"A\r\n&#13;&#xD;&amp;\tπ\" ",
        "b NMTOKENS '  y\rz  '>"
    );
    let expected = concat!(
        "<!ATTLIST r ",
        "a CDATA \"A \n&#13;&#xD;&amp;\tπ\" ",
        "b NMTOKENS '  y z  '>"
    );
    let (_, actual) = output(raw, LiteralSource::Replacement).unwrap();
    assert_eq!(actual.as_bytes(), expected.as_bytes());

    let mut original = loose_catalog();
    apply(&mut original, raw, REPLACEMENT);
    let mut reparsed = loose_catalog();
    apply(&mut reparsed, &actual, PHYSICAL);

    let original_defaults: Vec<_> = original
        .attributes("r")
        .map(|attribute| (attribute.name().to_owned(), attribute.default()))
        .collect();
    let reparsed_defaults: Vec<_> = reparsed
        .attributes("r")
        .map(|attribute| (attribute.name().to_owned(), attribute.default()))
        .collect();
    let expected_defaults = [
        (
            "a".to_owned(),
            DefaultValue::Value {
                fixed: false,
                value: "A  \r\r& π",
            },
        ),
        (
            "b".to_owned(),
            DefaultValue::Value {
                fixed: false,
                value: "y z",
            },
        ),
    ];
    assert_eq!(original_defaults, expected_defaults);
    assert_eq!(reparsed_defaults, expected_defaults);
}

#[test]
fn encoded_entity_declaration_reconstructs_the_same_catalog_value() {
    let raw = "<!ENTITY e \"A\r&#13;&#xD;&amp;π\">";
    let encoded = output(raw, LiteralSource::Replacement).unwrap().1;
    let expected = "A\r\r\r&amp;π";

    let mut original = loose_catalog();
    apply(&mut original, raw, REPLACEMENT);
    let mut reparsed = loose_catalog();
    apply(&mut reparsed, &encoded, PHYSICAL);

    assert_eq!(general_replacement(&original, "e"), expected);
    assert_eq!(general_replacement(&reparsed, "e"), expected);
}

#[test]
fn comments_processing_instructions_and_nonliteral_declarations_keep_raw_cr() {
    for raw in [
        "<!--a\rb\r-->",
        "<?p a\rb\r?>",
        "<!ELEMENT r (a|β)*>",
        "<!ATTLIST r a CDATA #IMPLIED>",
    ] {
        assert_eq!(
            output(raw, LiteralSource::Replacement).unwrap().1,
            raw,
            "{raw:?}"
        );
    }
}

#[test]
fn replacement_identifier_cr_is_typed_at_the_first_literal_byte() {
    let cases = [
        ("<!ENTITY e SYSTEM \"a\rb\">", 20),
        ("<!ENTITY e PUBLIC 'pub' \"a\rb\">", 26),
        ("<!NOTATION n PUBLIC \"a\rb\">", 22),
        ("<!ENTITY e PUBLIC \"p\rq\" \"s\rt\">", 20),
        ("<!DOCTYPE r PUBLIC \"p\rq\" \"s\rt\" [", 21),
    ];
    for (raw, expected_offset) in cases {
        let (prefix, error) = first_error(raw, LiteralSource::Replacement);
        assert_eq!(
            error,
            UnrepresentableIdentifier {
                offset: expected_offset
            }
        );
        assert_eq!(prefix.concat(), raw[..expected_offset]);
    }
}

#[test]
fn adjacent_transformations_never_rescan_or_drop_the_raw_suffix() {
    let raw = "<!ATTLIST r a CDATA '\r\r&#13;\r' b CDATA '\r'>";
    let expected = "<!ATTLIST r a CDATA '  &#13; ' b CDATA ' '>";
    let (fragments, actual) = output(raw, LiteralSource::Replacement).unwrap();
    assert_eq!(actual, expected);
    assert!(fragments.iter().all(|fragment| !fragment.is_empty()));
    assert_eq!(fragments.concat(), expected);

    let entity = "<!ENTITY e '\r&#13;\r'>";
    let expected = "<!ENTITY e '&#13;&#13;&#13;'>";
    assert_eq!(
        output(entity, LiteralSource::Replacement).unwrap().1,
        expected
    );
}
