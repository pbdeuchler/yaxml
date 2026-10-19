use super::*;

use crate::dtd::{DtdLimits, DtdUnit};

const INTERNAL: DeclarationContext = DeclarationContext {
    internal_subset: true,
    literals: LiteralSource::Physical,
};
const EXTERNAL: DeclarationContext = DeclarationContext {
    internal_subset: false,
    literals: LiteralSource::Physical,
};

fn unit(raw: &str) -> DtdUnit<'_> {
    DtdUnit::parse(raw, DtdLimits::default()).expect(raw)
}

fn loose() -> DtdCatalog {
    DtdCatalog::new(CatalogLimits::default(), false)
}

fn apply(catalog: &mut DtdCatalog, raw: &str) -> DeclarationEffect {
    catalog.apply(&unit(raw), INTERNAL).expect(raw)
}

fn value(catalog: &mut DtdCatalog, element: &str, name: &str, raw: &str) -> String {
    catalog
        .normalize_attribute_value(element, name, raw, LiteralSource::Physical)
        .expect(raw)
}

fn internal_replacement<'a>(catalog: &'a DtdCatalog, name: &str) -> &'a str {
    let EntityLookup::Internal { id, replacement } = catalog.lookup_general(name) else {
        panic!("expected internal general entity {name}")
    };
    assert_eq!(catalog.replacement(id), Some(replacement));
    replacement
}

fn marked(marked: &str) -> (String, usize) {
    let offset = marked.find('^').expect("fixture has one marker");
    assert_eq!(marked[offset + 1..].find('^'), None);
    let mut raw = marked.to_owned();
    raw.remove(offset);
    (raw, offset)
}

#[test]
fn entity_first_bindings_are_separate_and_predefined_values_are_fixed() {
    let mut catalog = loose();
    assert_eq!(apply(&mut catalog, "<!ENTITY x 'first'>").bindings_added, 1);
    assert_eq!(
        apply(&mut catalog, "<!ENTITY x 'second'>").bindings_added,
        0
    );
    assert_eq!(
        apply(&mut catalog, "<!ENTITY % x 'parameter'>").bindings_added,
        1
    );
    assert_eq!(internal_replacement(&catalog, "x"), "first");
    let EntityLookup::Internal { replacement, .. } = catalog.lookup_parameter("x") else {
        panic!("expected independent parameter binding")
    };
    assert_eq!(replacement, "parameter");

    let expected = [
        ("amp", '&'),
        ("lt", '<'),
        ("gt", '>'),
        ("apos", '\''),
        ("quot", '"'),
    ];
    for (name, character) in expected {
        assert_eq!(
            catalog.lookup_general(name),
            EntityLookup::Predefined(character)
        );
    }

    assert_eq!(
        apply(&mut catalog, "<!ENTITY amp '&#38;#38;'>").bindings_added,
        1
    );
    assert_eq!(
        apply(&mut catalog, "<!ENTITY lt '&#38;#60;'>").bindings_added,
        1
    );

    let before = catalog.usage();
    let error = catalog
        .apply(&unit("<!ENTITY amp 'WRONG'>"), INTERNAL)
        .unwrap_err();
    assert_eq!(
        error,
        CatalogError {
            kind: CatalogErrorKind::InvalidPredefinedDeclaration,
            location: ValueLocation::Declaration {
                declaration: before.declarations,
                offset: 14,
            },
        }
    );
    assert_eq!(catalog.lookup_general("amp"), EntityLookup::Predefined('&'));
}

#[test]
fn external_identifiers_are_unquoted_eol_processed_and_never_resolved() {
    let mut catalog = loose();
    apply(
        &mut catalog,
        "<!ENTITY ext PUBLIC 'A\r\n B' \"relative\rpath\">",
    );
    apply(&mut catalog, "<!ENTITY img SYSTEM 'image.bin' NDATA png>");
    assert_eq!(
        catalog.lookup_general("ext"),
        EntityLookup::ExternalUnavailable {
            id: EntityId(0),
            identifier: ExternalIdentifier {
                public: Some("A\n B"),
                system: Some("relative\npath"),
            },
        }
    );
    assert_eq!(
        catalog.lookup_general("img"),
        EntityLookup::Unparsed {
            id: EntityId(1),
            identifier: ExternalIdentifier {
                public: None,
                system: Some("image.bin"),
            },
            notation: "png",
        }
    );
}

#[test]
fn attribute_bindings_use_lexical_names_first_definition_and_source_order() {
    let mut catalog = loose();
    apply(
        &mut catalog,
        "<!ATTLIST p:r a CDATA 'plain' p:a NMTOKENS ' x  y '>",
    );
    apply(
        &mut catalog,
        "<!ATTLIST p:r a CDATA #IMPLIED b CDATA 'later'>",
    );

    let expected = [
        (
            "a",
            AttributeKind::Cdata,
            DefaultValue::Value {
                fixed: false,
                value: "plain",
            },
            0,
        ),
        (
            "p:a",
            AttributeKind::NmTokens,
            DefaultValue::Value {
                fixed: false,
                value: "x y",
            },
            0,
        ),
        (
            "b",
            AttributeKind::Cdata,
            DefaultValue::Value {
                fixed: false,
                value: "later",
            },
            1,
        ),
    ];
    let actual: Vec<_> = catalog
        .attributes("p:r")
        .map(|binding| {
            (
                binding.name(),
                binding.kind(),
                binding.default(),
                binding.declaration(),
            )
        })
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(catalog.attributes("q:r").count(), 0);
}

#[test]
fn entity_value_and_attribute_use_are_distinct_non_rescanning_phases() {
    let mut catalog = loose();
    for declaration in [
        "<!ENTITY numericMarkup '&#60;a/>'>",
        "<!ENTITY generalLt '&lt;a/>'>",
        "<!ENTITY numericAmp '&#38;lt;'>",
        "<!ENTITY generalAmp '&amp;lt;'>",
        "<!ENTITY phrase 'AT&amp;T;'>",
    ] {
        apply(&mut catalog, declaration);
    }
    assert_eq!(internal_replacement(&catalog, "numericMarkup"), "<a/>");
    assert_eq!(internal_replacement(&catalog, "generalLt"), "&lt;a/>");
    assert_eq!(internal_replacement(&catalog, "numericAmp"), "&lt;");
    assert_eq!(internal_replacement(&catalog, "generalAmp"), "&amp;lt;");
    assert_eq!(internal_replacement(&catalog, "phrase"), "AT&amp;T;");

    assert_eq!(value(&mut catalog, "r", "x", "&generalLt;"), "<a/>");
    assert_eq!(value(&mut catalog, "r", "x", "&numericAmp;"), "<");
    assert_eq!(value(&mut catalog, "r", "x", "&generalAmp;"), "&lt;");
    assert_eq!(value(&mut catalog, "r", "x", "&phrase;"), "AT&T;");
}

#[test]
fn direct_character_less_than_is_data_but_replacement_less_than_is_rejected() {
    let mut direct = loose();
    assert_eq!(value(&mut direct, "r", "x", "&#60;a/>"), "<a/>");

    let mut replacement = loose();
    apply(&mut replacement, "<!ENTITY x '&#60;'>");
    assert_eq!(
        replacement
            .normalize_attribute_value("r", "x", "&x;", LiteralSource::Physical)
            .unwrap_err(),
        CatalogError {
            kind: CatalogErrorKind::LessThanInAttribute,
            location: ValueLocation::Replacement {
                entity: EntityId(0),
                offset: 0,
            },
        }
    );
}

#[test]
fn physical_and_referenced_whitespace_follow_two_distinct_normalization_phases() {
    let mut cdata = loose();
    assert_eq!(
        value(&mut cdata, "r", "a", "  x\t\r\ny\r z  "),
        "  x  y  z  "
    );
    assert_eq!(
        value(&mut cdata, "r", "a", "&#x9;&#xA;&#xD;&#x20;"),
        "\t\n\r "
    );

    let mut tokenized = loose();
    apply(&mut tokenized, "<!ATTLIST r a NMTOKENS #IMPLIED>");
    assert_eq!(value(&mut tokenized, "r", "a", "  x\t y  "), "x y");
    assert_eq!(
        value(&mut tokenized, "r", "a", "  &#x9;  x \t y  "),
        "\t x y"
    );
    assert_eq!(
        value(&mut tokenized, "r", "a", "&#x9;&#xA;&#xD;&#x20;"),
        "\t\n\r"
    );
}

#[test]
fn default_normalization_uses_only_already_read_declarations() {
    let mut available = loose();
    apply(&mut available, "<!ENTITY a '&future;'>");
    apply(&mut available, "<!ENTITY future 'ok'>");
    apply(&mut available, "<!ATTLIST r x CDATA '&a;'>");
    assert_eq!(
        available.attributes("r").next().unwrap().default(),
        DefaultValue::Value {
            fixed: false,
            value: "ok",
        }
    );

    let mut indirect = loose();
    apply(&mut indirect, "<!ENTITY a '&future;'>");
    assert_eq!(
        indirect.apply(&unit("<!ATTLIST r x CDATA '&a;'>"), INTERNAL),
        Err(CatalogError {
            kind: CatalogErrorKind::EntityDeclared,
            location: ValueLocation::Replacement {
                entity: EntityId(0),
                offset: 0,
            },
        })
    );
}

#[test]
fn discarded_duplicate_attribute_still_checks_its_default() {
    let mut catalog = loose();
    apply(&mut catalog, "<!ENTITY indirect '&missing;'>");
    apply(&mut catalog, "<!ATTLIST r x CDATA 'first'>");
    assert_eq!(
        catalog.apply(&unit("<!ATTLIST r x CDATA '&indirect;'>"), INTERNAL),
        Err(CatalogError {
            kind: CatalogErrorKind::EntityDeclared,
            location: ValueLocation::Replacement {
                entity: EntityId(0),
                offset: 0,
            },
        })
    );
}

#[test]
fn discarded_duplicate_entity_with_unloaded_parameter_keeps_first_binding() {
    let mut catalog = loose();
    apply(&mut catalog, "<!ENTITY % unread SYSTEM 'external.ent'>");
    apply(&mut catalog, "<!ENTITY % p 'first'>");
    let effect = catalog
        .apply(&unit("<!ENTITY % p '%unread;'>"), EXTERNAL)
        .unwrap();
    assert_eq!(effect, DeclarationEffect { bindings_added: 0 });
    let EntityLookup::Internal { replacement, .. } = catalog.lookup_parameter("p") else {
        panic!("first parameter binding must remain effective")
    };
    assert_eq!(replacement, "first");
    apply(&mut catalog, "<!ENTITY later 'kept'>");
    assert_eq!(internal_replacement(&catalog, "later"), "kept");
}

#[test]
fn first_missing_default_reference_fails_in_lexical_order() {
    let mut catalog = loose();
    apply(&mut catalog, "<!ENTITY a '&missingA;'>");
    apply(&mut catalog, "<!ENTITY b '&missingB;'>");
    assert_eq!(
        catalog.apply(&unit("<!ATTLIST r x CDATA '&a;' y CDATA '&b;'>"), INTERNAL),
        Err(CatalogError {
            kind: CatalogErrorKind::EntityDeclared,
            location: ValueLocation::Replacement {
                entity: EntityId(0),
                offset: 0,
            },
        })
    );
    assert_eq!(catalog.attributes("r").count(), 0);
}

#[test]
fn direct_missing_default_is_typed_at_the_reference_and_sticky() {
    let (raw, offset) = marked("<!ATTLIST r x CDATA '^&missing;'>");
    let expected = CatalogError {
        kind: CatalogErrorKind::EntityDeclared,
        location: ValueLocation::Declaration {
            declaration: 0,
            offset,
        },
    };
    let mut catalog = loose();
    assert_eq!(catalog.apply(&unit(&raw), INTERNAL), Err(expected));
    let after_failure = catalog.usage();
    assert_eq!(
        catalog.apply(&unit("<!ENTITY missing 'late'>"), INTERNAL),
        Err(expected)
    );
    assert_eq!(catalog.usage(), after_failure);
    assert_eq!(catalog.lookup_general("missing"), EntityLookup::Undeclared);
    assert_eq!(catalog.attributes("r").count(), 0);
}

#[test]
fn internal_parameter_reference_is_semantic_but_raw_markup_use_is_lexical() {
    let (raw, offset) = marked("<!ENTITY e '^%p;'>");
    let expected = CatalogError {
        kind: CatalogErrorKind::ParameterInInternalDeclaration,
        location: ValueLocation::Declaration {
            declaration: 0,
            offset,
        },
    };
    let mut internal = loose();
    assert_eq!(internal.apply(&unit(&raw), INTERNAL), Err(expected));

    let mut external = loose();
    apply(&mut external, "<!ENTITY % p 'included'>");
    assert_eq!(
        external.apply(&unit("<!ENTITY e '%p;'>"), EXTERNAL),
        Ok(DeclarationEffect { bindings_added: 1 })
    );
    assert_eq!(internal_replacement(&external, "e"), "included");

    assert!(DtdUnit::parse("<!ATTLIST r %p;>", DtdLimits::default()).is_err());
}

#[test]
fn unread_sources_never_trigger_io_and_barrier_effect_depends_on_standalone() {
    let header = unit("<!DOCTYPE r SYSTEM 'external.dtd'>");
    let default = unit("<!ATTLIST r x CDATA '&maybe;'>");

    let mut open = loose();
    open.apply(&header, INTERNAL).unwrap();
    open.apply(&default, INTERNAL).unwrap();
    assert_eq!(
        open.attributes("r").next().unwrap().default(),
        DefaultValue::Value {
            fixed: false,
            value: "",
        }
    );

    let mut referenced = loose();
    referenced.note_parameter_reference();
    assert_eq!(
        apply(&mut referenced, "<!ENTITY later 'kept'>").bindings_added,
        1
    );
    assert_eq!(internal_replacement(&referenced, "later"), "kept");

    let mut standalone = DtdCatalog::new(CatalogLimits::default(), true);
    standalone.note_parameter_reference();
    assert_eq!(
        apply(&mut standalone, "<!ENTITY later 'kept'>").bindings_added,
        1
    );
    assert_eq!(internal_replacement(&standalone, "later"), "kept");

    let expected = CatalogErrorKind::EntityDeclared;
    let error = standalone
        .apply(&default, INTERNAL)
        .expect_err("standalone cannot defer a missing declaration");
    assert_eq!(error.kind, expected);
}

#[test]
fn external_and_unparsed_entities_fail_only_when_attribute_use_reaches_them() {
    let mut external = loose();
    apply(&mut external, "<!ENTITY ext SYSTEM 'x'>");
    apply(&mut external, "<!ENTITY outer '&ext;'>");
    assert_eq!(
        external
            .normalize_attribute_value("r", "a", "&outer;", LiteralSource::Physical)
            .unwrap_err(),
        CatalogError {
            kind: CatalogErrorKind::ExternalInAttribute,
            location: ValueLocation::Replacement {
                entity: EntityId(1),
                offset: 0,
            },
        }
    );

    let mut unparsed = loose();
    apply(&mut unparsed, "<!ENTITY img SYSTEM 'x' NDATA n>");
    apply(&mut unparsed, "<!ENTITY unused '&img;'>");
    assert_eq!(internal_replacement(&unparsed, "unused"), "&img;");
    assert_eq!(
        unparsed
            .normalize_attribute_value("r", "a", "&unused;", LiteralSource::Physical)
            .unwrap_err()
            .kind,
        CatalogErrorKind::UnparsedEntity
    );
}

#[test]
fn validity_only_attribute_declarations_remain_effective() {
    let mut catalog = loose();
    for raw in [
        "<!ATTLIST r a ID 'literal'>",
        "<!ATTLIST r b ID #IMPLIED>",
        "<!ATTLIST r c (x|x) 'outside'>",
        "<!ATTLIST r d NMTOKENS '&#x9;'>",
    ] {
        apply(&mut catalog, raw);
    }
    let actual: Vec<_> = catalog
        .attributes("r")
        .map(|binding| (binding.name(), binding.kind()))
        .collect();
    assert_eq!(
        actual,
        [
            ("a", AttributeKind::Id),
            ("b", AttributeKind::Id),
            ("c", AttributeKind::Enumeration),
            ("d", AttributeKind::NmTokens),
        ]
    );
    assert_eq!(value(&mut catalog, "r", "d", "&#x9;"), "\t");
}

#[test]
fn unused_replacement_dependencies_cycles_and_bad_markup_are_not_swept() {
    let mut catalog = loose();
    for raw in [
        "<!ENTITY missing '&notRead;'>",
        "<!ENTITY cycle '&cycle;'>",
        "<!ENTITY markup '<a'>",
        "<!ENTITY img SYSTEM 'x' NDATA n>",
        "<!ENTITY imageRef '&img;'>",
    ] {
        apply(&mut catalog, raw);
    }
    assert_eq!(internal_replacement(&catalog, "missing"), "&notRead;");
    assert_eq!(internal_replacement(&catalog, "cycle"), "&cycle;");
    assert_eq!(internal_replacement(&catalog, "markup"), "<a");
    assert_eq!(internal_replacement(&catalog, "imageRef"), "&img;");
}

#[test]
fn entity_literal_source_controls_exactly_one_eol_processing_phase() {
    let raw = "<!ENTITY physical 'a\r\nb\rc'>";
    let mut physical = loose();
    physical.apply(&unit(raw), INTERNAL).unwrap();
    assert_eq!(internal_replacement(&physical, "physical"), "a\nb\nc");

    let mut replacement = loose();
    replacement
        .apply(
            &unit(raw),
            DeclarationContext {
                internal_subset: true,
                literals: LiteralSource::Replacement,
            },
        )
        .unwrap();
    assert_eq!(internal_replacement(&replacement, "physical"), "a\r\nb\rc");
}

#[test]
fn empty_references_still_cost_and_cycles_fail_only_on_actual_use() {
    let mut empty = loose();
    apply(&mut empty, "<!ENTITY e ''>");
    let before = empty.usage();
    assert_eq!(value(&mut empty, "r", "a", "&e;&e;"), "");
    let after = empty.usage();
    assert_eq!(after.references - before.references, 2);
    assert_eq!(after.work - before.work, 8);
    assert_eq!(after.expanded_bytes - before.expanded_bytes, 0);

    let mut cycle = loose();
    apply(&mut cycle, "<!ENTITY a '&b;'>");
    apply(&mut cycle, "<!ENTITY b '&a;'>");
    assert_eq!(internal_replacement(&cycle, "a"), "&b;");
    assert_eq!(internal_replacement(&cycle, "b"), "&a;");
    assert_eq!(
        cycle
            .normalize_attribute_value("r", "x", "&a;", LiteralSource::Physical)
            .unwrap_err(),
        CatalogError {
            kind: CatalogErrorKind::Recursion,
            location: ValueLocation::Replacement {
                entity: EntityId(1),
                offset: 0,
            },
        }
    );
}

#[test]
fn missing_parameter_reference_in_a_value_expands_empty_and_keeps_later_declarations() {
    let mut catalog = loose();
    let effect = catalog
        .apply(&unit("<!ENTITY % p 'a%missing;b'>"), EXTERNAL)
        .unwrap();
    assert_eq!(effect.bindings_added, 1);
    let EntityLookup::Internal { replacement, .. } = catalog.lookup_parameter("p") else {
        panic!("parameter binding")
    };
    assert_eq!(replacement, "ab");
    apply(&mut catalog, "<!ENTITY later 'kept'>");
    assert_eq!(internal_replacement(&catalog, "later"), "kept");
}

#[test]
fn expanded_unicode_bytes_have_an_exact_independent_boundary() {
    let mut short = DtdCatalog::new(
        CatalogLimits {
            max_expanded_bytes: 1,
            ..CatalogLimits::default()
        },
        false,
    );
    assert_eq!(
        short
            .normalize_attribute_value("r", "a", "π", LiteralSource::Physical)
            .unwrap_err(),
        CatalogError {
            kind: CatalogErrorKind::Limit(CatalogLimit::ExpandedBytes),
            location: ValueLocation::Input(0),
        }
    );
    assert_eq!(short.usage().expanded_bytes, 0);

    let mut exact = DtdCatalog::new(
        CatalogLimits {
            max_expanded_bytes: 2,
            max_value_bytes: 2,
            ..CatalogLimits::default()
        },
        false,
    );
    assert_eq!(value(&mut exact, "r", "a", "π"), "π");
    assert_eq!(exact.usage().expanded_bytes, 2);
}

#[test]
fn reference_depth_and_unicode_byte_ceilings_are_independent_and_atomic() {
    let mut zero_references = DtdCatalog::new(
        CatalogLimits {
            max_references: 0,
            ..CatalogLimits::default()
        },
        false,
    );
    let expected_reference = CatalogError {
        kind: CatalogErrorKind::Limit(CatalogLimit::References),
        location: ValueLocation::Input(0),
    };
    assert_eq!(
        zero_references.normalize_attribute_value("r", "a", "&amp;", LiteralSource::Physical),
        Err(expected_reference)
    );
    assert_eq!(zero_references.usage().references, 0);

    let mut short_bytes = DtdCatalog::new(
        CatalogLimits {
            max_value_bytes: 1,
            ..CatalogLimits::default()
        },
        false,
    );
    assert_eq!(
        short_bytes
            .normalize_attribute_value("r", "a", "π", LiteralSource::Physical)
            .unwrap_err(),
        CatalogError {
            kind: CatalogErrorKind::Limit(CatalogLimit::ValueBytes),
            location: ValueLocation::Input(0),
        }
    );
    assert_eq!(short_bytes.usage().expanded_bytes, 2);

    let mut shallow = DtdCatalog::new(
        CatalogLimits {
            max_depth: 1,
            ..CatalogLimits::default()
        },
        false,
    );
    apply(&mut shallow, "<!ENTITY b 'v'>");
    apply(&mut shallow, "<!ENTITY a '&b;'>");
    assert_eq!(
        shallow
            .normalize_attribute_value("r", "a", "&a;", LiteralSource::Physical)
            .unwrap_err(),
        CatalogError {
            kind: CatalogErrorKind::Limit(CatalogLimit::Depth),
            location: ValueLocation::Replacement {
                entity: EntityId(1),
                offset: 0,
            },
        }
    );
}

#[test]
fn declaration_binding_and_storage_limits_fail_before_publication() {
    let declaration_error = CatalogError {
        kind: CatalogErrorKind::Limit(CatalogLimit::Declarations),
        location: ValueLocation::Declaration {
            declaration: 0,
            offset: 0,
        },
    };
    let mut none = DtdCatalog::new(
        CatalogLimits {
            max_declarations: 0,
            ..CatalogLimits::default()
        },
        false,
    );
    assert_eq!(
        none.apply(&unit("<!ENTITY x 'v'>"), INTERNAL),
        Err(declaration_error)
    );
    assert_eq!(none.usage(), CatalogUsage::default());

    let mut one_byte = DtdCatalog::new(
        CatalogLimits {
            max_storage_bytes: 1,
            ..CatalogLimits::default()
        },
        false,
    );
    let error = one_byte
        .apply(&unit("<!ENTITY x 'v'>"), INTERNAL)
        .unwrap_err();
    assert_eq!(
        error.kind,
        CatalogErrorKind::Limit(CatalogLimit::StorageBytes)
    );
    assert_eq!(one_byte.lookup_general("x"), EntityLookup::Undeclared);
    assert_eq!(one_byte.usage().bindings, 0);
    assert_eq!(one_byte.usage().storage_bytes, 0);
}
