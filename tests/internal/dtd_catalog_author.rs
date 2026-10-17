use super::*;
use crate::dtd::DtdLimits;

fn context() -> DeclarationContext {
    DeclarationContext {
        internal_subset: true,
        literals: LiteralSource::Physical,
    }
}

fn declare(catalog: &mut DtdCatalog, raw: &str) -> Result<DeclarationEffect, CatalogError> {
    catalog.apply(
        &DtdUnit::parse(raw, DtdLimits::default()).unwrap(),
        context(),
    )
}

#[test]
fn initial_replacement_and_default_phases_are_distinct() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ENTITY e 'AT&amp;T; &#60;a/> &#13;'>").unwrap();
    let EntityLookup::Internal { id, replacement } = catalog.lookup_general("e") else {
        panic!("internal binding")
    };
    assert_eq!(replacement, "AT&amp;T; <a/> \r");
    assert_eq!(catalog.replacement(id), Some(replacement));
    assert_eq!(id.index(), 0);
    assert_eq!(catalog.declaration(id), Some(0));
    declare(&mut catalog, "<!ENTITY e 'ignored'>").unwrap();
    assert!(matches!(
        catalog.lookup_parameter("e"),
        EntityLookup::Undeclared
    ));
    assert_eq!(
        catalog
            .normalize_attribute_value("r", "a", "AT&amp;T;", LiteralSource::Physical)
            .unwrap(),
        NormalizedValue::Value("AT&T;".into())
    );
    declare(&mut catalog, "<!ATTLIST r a NMTOKENS '  a  &#9; b  '>").unwrap();
    let attribute = catalog.attributes("r").next().unwrap();
    assert_eq!(attribute.name(), "a");
    assert_eq!(attribute.kind(), AttributeKind::NmTokens);
    assert_eq!(
        attribute.default(),
        DefaultValue::Value {
            fixed: false,
            value: "a \t b"
        }
    );
    assert_eq!(attribute.declaration(), 2);
    assert_eq!(catalog.usage().bindings, 2);
    assert_eq!(catalog.attributes("missing").count(), 0);
}

#[test]
fn internal_pe_restriction_and_unread_barrier_are_explicit() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    catalog.note_unread_parameter();
    assert!(
        declare(&mut catalog, "<!ENTITY e 'ignored'>")
            .unwrap()
            .ignored_after_unread_parameter
    );
    assert!(matches!(
        catalog.lookup_general("e"),
        EntityLookup::UnreadEntity
    ));
    let error = declare(&mut catalog, "<!ENTITY e '%p;'>").unwrap_err();
    assert_eq!(error.kind, CatalogErrorKind::ParameterInInternalDeclaration);
    assert_eq!(
        error.location,
        ValueLocation::Declaration {
            declaration: 1,
            offset: 12
        }
    );
    assert_eq!(declare(&mut catalog, "<!ENTITY e 'x'>"), Err(error));
}

#[test]
fn replacement_literals_preserve_numeric_cr_and_predefined_errors_cannot_panic() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    let unit = DtdUnit::parse("<!ENTITY e 'a\r\nb&#13;'>", DtdLimits::default()).unwrap();
    catalog
        .apply(
            &unit,
            DeclarationContext {
                internal_subset: true,
                literals: LiteralSource::Replacement,
            },
        )
        .unwrap();
    assert!(matches!(
        catalog.lookup_general("e"),
        EntityLookup::Internal {
            replacement: "a\r\nb\r",
            ..
        }
    ));
    assert_eq!(catalog.lookup_general("amp"), EntityLookup::Predefined('&'));
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    assert_eq!(
        declare(&mut catalog, "<!ENTITY amp ';'>").unwrap_err().kind,
        CatalogErrorKind::InvalidPredefinedDeclaration
    );
}

fn value(catalog: &mut DtdCatalog, raw: &str) -> String {
    match catalog
        .normalize_attribute_value("r", "a", raw, LiteralSource::Physical)
        .unwrap()
    {
        NormalizedValue::Value(value) => value,
        NormalizedValue::Unavailable(dependency) => panic!("unexpected dependency {dependency:?}"),
    }
}

#[test]
fn forward_entity_values_bind_at_use_but_defaults_do_not_rebind() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ENTITY e '&later;'>").unwrap();
    assert!(matches!(
        catalog.lookup_general("e"),
        EntityLookup::Internal {
            replacement: "&later;",
            ..
        }
    ));
    let effect = declare(&mut catalog, "<!ATTLIST r a CDATA '&e;'>").unwrap();
    assert_eq!(effect.unavailable_defaults, 1);
    let old = catalog.attributes("r").next().unwrap().default();
    assert!(matches!(
        old,
        DefaultValue::Unavailable {
            dependency: Dependency {
                kind: DependencyKind::MissingReadDeclaration,
                location: ValueLocation::Replacement { offset: 0, .. }
            },
            ..
        }
    ));
    declare(&mut catalog, "<!ENTITY later 'v'>").unwrap();
    assert_eq!(value(&mut catalog, "&e;"), "v");
    assert!(matches!(
        catalog.attributes("r").next().unwrap().default(),
        DefaultValue::Unavailable { .. }
    ));

    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    assert_eq!(
        declare(&mut catalog, "<!ATTLIST r a CDATA '&late;'>")
            .unwrap_err()
            .kind,
        CatalogErrorKind::EntityDeclared
    );
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ENTITY later 'v'>").unwrap();
    declare(&mut catalog, "<!ENTITY e '&later;'>").unwrap();
    declare(&mut catalog, "<!ATTLIST r a CDATA '&e;'>").unwrap();
    assert_eq!(
        catalog.attributes("r").next().unwrap().default(),
        DefaultValue::Value {
            fixed: false,
            value: "v"
        }
    );
}

#[test]
fn first_binding_is_per_attribute_and_lexical_element_and_entity_class() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ENTITY x 'general'>").unwrap();
    declare(&mut catalog, "<!ENTITY % x 'parameter'>").unwrap();
    declare(&mut catalog, "<!ENTITY x 'duplicate'>").unwrap();
    assert!(matches!(
        catalog.lookup_general("x"),
        EntityLookup::Internal {
            replacement: "general",
            ..
        }
    ));
    assert!(matches!(
        catalog.lookup_parameter("x"),
        EntityLookup::Internal {
            replacement: "parameter",
            ..
        }
    ));
    declare(
        &mut catalog,
        "<!ATTLIST p:r a CDATA 'first' b NMTOKENS '  a  b  '>",
    )
    .unwrap();
    declare(
        &mut catalog,
        "<!ATTLIST p:r a NMTOKENS 'ignored' c CDATA #FIXED 'last'>",
    )
    .unwrap();
    declare(&mut catalog, "<!ATTLIST r a CDATA 'other'>").unwrap();
    let attributes: Vec<_> = catalog
        .attributes("p:r")
        .map(|a| (a.name(), a.kind(), a.default()))
        .collect();
    assert_eq!(
        attributes,
        vec![
            (
                "a",
                AttributeKind::Cdata,
                DefaultValue::Value {
                    fixed: false,
                    value: "first"
                }
            ),
            (
                "b",
                AttributeKind::NmTokens,
                DefaultValue::Value {
                    fixed: false,
                    value: "a b"
                }
            ),
            (
                "c",
                AttributeKind::Cdata,
                DefaultValue::Value {
                    fixed: true,
                    value: "last"
                }
            ),
        ]
    );
    assert_eq!(
        catalog.attributes("r").next().unwrap().default(),
        DefaultValue::Value {
            fixed: false,
            value: "other"
        }
    );
    assert_eq!(value(&mut catalog, "&x;"), "general");
    assert_eq!(
        catalog
            .normalize_attribute_value("p:r", "a", "  a  b  ", LiteralSource::Physical)
            .unwrap(),
        NormalizedValue::Value("  a  b  ".into())
    );
    assert_eq!(
        catalog
            .normalize_attribute_value("p:r", "b", "  a  b  ", LiteralSource::Physical)
            .unwrap(),
        NormalizedValue::Value("a b".into())
    );
}

#[test]
fn duplicate_defaults_still_receive_required_reference_checks() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ATTLIST r a CDATA 'first'>").unwrap();
    assert_eq!(
        declare(&mut catalog, "<!ATTLIST r a CDATA '&missing;'>")
            .unwrap_err()
            .kind,
        CatalogErrorKind::EntityDeclared
    );
}

#[test]
fn unread_dependency_and_standalone_barrier_policies_are_separate() {
    for standalone in [false, true] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), standalone);
        declare(&mut catalog, "<!DOCTYPE r SYSTEM 'never-open' [").unwrap();
        let result =
            catalog.normalize_attribute_value("r", "a", "&unknown;", LiteralSource::Physical);
        if standalone {
            assert_eq!(result.unwrap_err().kind, CatalogErrorKind::EntityDeclared);
        } else {
            assert_eq!(
                result.unwrap(),
                NormalizedValue::Unavailable(Dependency {
                    kind: DependencyKind::UnreadEntity,
                    location: ValueLocation::Input(0)
                })
            );
        }
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), standalone);
        declare(&mut catalog, "<!ENTITY first 'retained'>").unwrap();
        catalog.note_unread_parameter();
        let entity = declare(&mut catalog, "<!ENTITY second 'new'>").unwrap();
        let attrs = declare(&mut catalog, "<!ATTLIST r a CDATA 'default'>").unwrap();
        assert_eq!(entity.ignored_after_unread_parameter, !standalone);
        assert_eq!(attrs.ignored_after_unread_parameter, !standalone);
        assert!(matches!(
            catalog.lookup_general("first"),
            EntityLookup::Internal {
                replacement: "retained",
                ..
            }
        ));
        assert_eq!(catalog.attributes("r").count(), usize::from(standalone));
        if standalone {
            assert!(matches!(
                catalog.lookup_general("second"),
                EntityLookup::Internal {
                    replacement: "new",
                    ..
                }
            ));
        } else {
            assert_eq!(catalog.lookup_general("second"), EntityLookup::UnreadEntity);
        }
    }
}

#[test]
fn external_entities_are_explicit_and_never_attribute_replacements() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ENTITY e PUBLIC 'pub' 'relative'>").unwrap();
    let EntityLookup::ExternalUnavailable { id, identifier } = catalog.lookup_general("e") else {
        panic!("external")
    };
    assert_eq!(
        identifier,
        ExternalIdentifier {
            public: Some("pub"),
            system: Some("relative")
        }
    );
    assert_eq!(catalog.replacement(id), None);
    assert_eq!(catalog.declaration(id), Some(0));
    assert_eq!(
        catalog
            .normalize_attribute_value("r", "a", "&e;", LiteralSource::Physical)
            .unwrap_err()
            .kind,
        CatalogErrorKind::ExternalInAttribute
    );
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(
        &mut catalog,
        "<!ENTITY e SYSTEM 'never-open' NDATA missing>",
    )
    .unwrap();
    assert!(matches!(
        catalog.lookup_general("e"),
        EntityLookup::Unparsed {
            notation: "missing",
            ..
        }
    ));
    assert_eq!(
        catalog
            .normalize_attribute_value("r", "a", "&e;", LiteralSource::Physical)
            .unwrap_err()
            .kind,
        CatalogErrorKind::UnparsedEntity
    );
}

#[test]
fn external_entity_value_context_includes_parameter_text_without_padding_or_quote_termination() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ENTITY % p '&#34;yes&#34;'>").unwrap();
    let unit = DtdUnit::parse("<!ENTITY e 'before%p;after'>", DtdLimits::default()).unwrap();
    catalog
        .apply(
            &unit,
            DeclarationContext {
                internal_subset: false,
                literals: LiteralSource::Physical,
            },
        )
        .unwrap();
    assert!(matches!(
        catalog.lookup_general("e"),
        EntityLookup::Internal {
            replacement: "before\"yes\"after",
            ..
        }
    ));
    assert_eq!(value(&mut catalog, "&e;"), "before\"yes\"after");
    let unit = DtdUnit::parse("<!ENTITY e 'ignored%p;'>", DtdLimits::default()).unwrap();
    assert_eq!(
        catalog.apply(&unit, context()).unwrap_err().kind,
        CatalogErrorKind::ParameterInInternalDeclaration
    );
}

#[test]
fn unavailable_parameter_construction_is_first_bound_without_retroactive_repair() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    let external = DeclarationContext {
        internal_subset: false,
        literals: LiteralSource::Physical,
    };
    let unit = DtdUnit::parse("<!ENTITY e '%later;'>", DtdLimits::default()).unwrap();
    catalog.apply(&unit, external).unwrap();
    assert_eq!(
        catalog.lookup_general("e"),
        EntityLookup::Unavailable(Dependency {
            kind: DependencyKind::MissingReadDeclaration,
            location: ValueLocation::Declaration {
                declaration: 0,
                offset: 12
            }
        })
    );
    declare(&mut catalog, "<!ENTITY % later 'now'>").unwrap();
    assert!(matches!(
        catalog.lookup_general("e"),
        EntityLookup::Unavailable(_)
    ));
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ENTITY % p SYSTEM 'not-opened'>").unwrap();
    let unit = DtdUnit::parse("<!ENTITY e '%p;'>", DtdLimits::default()).unwrap();
    catalog.apply(&unit, external).unwrap();
    assert!(matches!(
        catalog.lookup_general("e"),
        EntityLookup::Unavailable(Dependency {
            kind: DependencyKind::ExternalEntity,
            ..
        })
    ));
    assert!(matches!(
        catalog
            .normalize_attribute_value("r", "a", "&e;", LiteralSource::Physical)
            .unwrap(),
        NormalizedValue::Unavailable(Dependency {
            kind: DependencyKind::ExternalEntity,
            ..
        })
    ));
}

#[test]
fn literal_eol_and_reference_whitespace_phases_have_independent_expected_results() {
    for (source, replacement) in [
        (LiteralSource::Physical, "a\nb\nc\r\n\t"),
        (LiteralSource::Replacement, "a\r\nb\rc\r\n\t"),
    ] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        let unit = DtdUnit::parse(
            "<!ENTITY e 'a\r\nb\rc&#13;&#10;&#9;'>",
            DtdLimits::default(),
        )
        .unwrap();
        catalog
            .apply(
                &unit,
                DeclarationContext {
                    internal_subset: true,
                    literals: source,
                },
            )
            .unwrap();
        assert!(
            matches!(catalog.lookup_general("e"),EntityLookup::Internal {replacement:actual,..} if actual==replacement)
        );
        assert_eq!(
            catalog
                .normalize_attribute_value("r", "a", "&e;", source)
                .unwrap(),
            NormalizedValue::Value(
                replacement
                    .chars()
                    .map(|c| if matches!(c, '\r' | '\n' | '\t') {
                        ' '
                    } else {
                        c
                    })
                    .collect()
            )
        );
    }
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    assert_eq!(
        value(&mut catalog, "a\r\nb\rc&#13;&#10;&#9;"),
        "a b c\r\n\t"
    );
    assert_eq!(
        catalog
            .normalize_attribute_value("r", "a", "a\r\nb\rc", LiteralSource::Replacement)
            .unwrap(),
        NormalizedValue::Value("a  b c".into())
    );
}

#[test]
fn numeric_markup_and_predefined_spellings_are_reparsed_only_at_entity_use() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ENTITY e 'AT&amp;T;'>").unwrap();
    declare(&mut catalog, "<!ENTITY ltvalue '&lt;'>").unwrap();
    declare(&mut catalog, "<!ENTITY numeric '&#60;'>").unwrap();
    assert_eq!(value(&mut catalog, "&e;"), "AT&T;");
    assert_eq!(value(&mut catalog, "&ltvalue;"), "<");
    assert_eq!(value(&mut catalog, "&#60;a/>"), "<a/>");
    assert_eq!(
        catalog
            .normalize_attribute_value("r", "a", "&numeric;", LiteralSource::Physical)
            .unwrap_err()
            .kind,
        CatalogErrorKind::LessThanInAttribute
    );
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ENTITY e '&#38;#65;'>").unwrap();
    assert!(matches!(
        catalog.lookup_general("e"),
        EntityLookup::Internal {
            replacement: "&#65;",
            ..
        }
    ));
    assert_eq!(value(&mut catalog, "&e;"), "A");
}

#[test]
fn unused_replacement_errors_are_not_eagerly_rejected() {
    for raw in [
        "<!ENTITY e '&never;'>",
        "<!ENTITY e '<a'>",
        "<!ENTITY e '&e;'>",
    ] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        declare(&mut catalog, raw).unwrap();
        assert!(matches!(
            catalog.lookup_general("e"),
            EntityLookup::Internal { .. }
        ));
    }
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ENTITY u SYSTEM 'none' NDATA n>").unwrap();
    declare(&mut catalog, "<!ENTITY e '&u;'>").unwrap();
    assert!(matches!(
        catalog.lookup_general("e"),
        EntityLookup::Internal {
            replacement: "&u;",
            ..
        }
    ));
    assert_eq!(
        catalog
            .normalize_attribute_value("r", "a", "&e;", LiteralSource::Physical)
            .unwrap_err()
            .kind,
        CatalogErrorKind::UnparsedEntity
    );
}

#[test]
fn general_and_parameter_cycles_fail_only_when_included() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ENTITY a '&b;'>").unwrap();
    declare(&mut catalog, "<!ENTITY b '&a;'>").unwrap();
    assert_eq!(
        catalog
            .normalize_attribute_value("r", "a", "&a;", LiteralSource::Physical)
            .unwrap_err()
            .kind,
        CatalogErrorKind::Recursion
    );
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ENTITY % p '&#37;p;'>").unwrap();
    let unit = DtdUnit::parse("<!ENTITY e '%p;'>", DtdLimits::default()).unwrap();
    assert_eq!(
        catalog
            .apply(
                &unit,
                DeclarationContext {
                    internal_subset: false,
                    literals: LiteralSource::Physical
                }
            )
            .unwrap_err()
            .kind,
        CatalogErrorKind::Recursion
    );
}

#[test]
fn predefined_declarations_obey_required_forms_and_cannot_replace_builtins() {
    for raw in [
        "<!ENTITY amp '&#38;#38;'>",
        "<!ENTITY lt '&#38;#x3c;'>",
        "<!ENTITY gt '&#62;'>",
        "<!ENTITY apos '&#39;'>",
        "<!ENTITY quot '&#34;'>",
    ] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        declare(&mut catalog, raw).unwrap();
        assert_eq!(value(&mut catalog, "&amp;&lt;&gt;&apos;&quot;"), "&<>'\"");
    }
    for raw in [
        "<!ENTITY amp '&#38;'>",
        "<!ENTITY lt '&#60;'>",
        "<!ENTITY gt 'x'>",
        "<!ENTITY amp SYSTEM 'none'>",
        "<!ENTITY amp ';'>",
        "<!ENTITY amp ''>",
    ] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        assert_eq!(
            declare(&mut catalog, raw).unwrap_err().kind,
            CatalogErrorKind::InvalidPredefinedDeclaration,
            "{raw}"
        );
        assert_eq!(catalog.lookup_general("amp"), EntityLookup::Predefined('&'));
    }
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ENTITY % amp 'ordinary PE'>").unwrap();
    assert!(matches!(
        catalog.lookup_parameter("amp"),
        EntityLookup::Internal {
            replacement: "ordinary PE",
            ..
        }
    ));
}

#[test]
fn all_attribute_types_normalize_without_enforcing_validity() {
    for (kind, expected) in [
        ("CDATA", AttributeKind::Cdata),
        ("ID", AttributeKind::Id),
        ("IDREF", AttributeKind::IdRef),
        ("IDREFS", AttributeKind::IdRefs),
        ("ENTITY", AttributeKind::Entity),
        ("ENTITIES", AttributeKind::Entities),
        ("NMTOKEN", AttributeKind::NmToken),
        ("NMTOKENS", AttributeKind::NmTokens),
        ("NOTATION (n|n)", AttributeKind::Notation),
        ("(a|a)", AttributeKind::Enumeration),
    ] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        declare(
            &mut catalog,
            &format!("<!ATTLIST r a {kind} '  invalid?  value  '>"),
        )
        .unwrap();
        let attribute = catalog.attributes("r").next().unwrap();
        assert_eq!(attribute.kind(), expected);
        assert_eq!(
            attribute.default(),
            DefaultValue::Value {
                fixed: false,
                value: if expected == AttributeKind::Cdata {
                    "  invalid?  value  "
                } else {
                    "invalid? value"
                }
            }
        );
    }
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(
        &mut catalog,
        "<!ATTLIST r a ID #REQUIRED b ID #IMPLIED c ID #FIXED 'not an ID'>",
    )
    .unwrap();
    let defaults: Vec<_> = catalog.attributes("r").map(|a| a.default()).collect();
    assert_eq!(
        defaults,
        vec![
            DefaultValue::Required,
            DefaultValue::Implied,
            DefaultValue::Value {
                fixed: true,
                value: "not an ID"
            }
        ]
    );
    assert_eq!(
        catalog
            .normalize_attribute_value("r", "c", "different", LiteralSource::Physical)
            .unwrap(),
        NormalizedValue::Value("different".into())
    );
}

#[test]
fn direct_value_input_checks_reference_and_character_grammar_and_keeps_failures_sticky() {
    for (raw, kind) in [
        ("&", CatalogErrorKind::InvalidReference),
        ("&;", CatalogErrorKind::InvalidReference),
        ("&#;", CatalogErrorKind::InvalidReference),
        ("&#x;", CatalogErrorKind::InvalidReference),
        ("&#x110000;", CatalogErrorKind::InvalidReference),
        ("&#0;", CatalogErrorKind::InvalidReference),
        (
            "&#999999999999999999999;",
            CatalogErrorKind::InvalidReference,
        ),
        ("&bad name;", CatalogErrorKind::InvalidReference),
        ("\0", CatalogErrorKind::InvalidCharacter),
        ("<", CatalogErrorKind::LessThanInAttribute),
        ("&unknown;", CatalogErrorKind::EntityDeclared),
    ] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        let error = catalog
            .normalize_attribute_value("r", "a", raw, LiteralSource::Physical)
            .unwrap_err();
        assert_eq!(error.kind, kind, "{raw:?}");
        assert_eq!(error.location, ValueLocation::Input(0));
        assert_eq!(
            catalog.normalize_attribute_value("r", "a", "valid", LiteralSource::Physical),
            Err(error)
        );
        assert_eq!(declare(&mut catalog, "<!ENTITY e 'v'>"), Err(error));
    }
}

#[test]
fn unread_pe_inside_entity_value_sets_the_later_declaration_barrier() {
    for standalone in [false, true] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), standalone);
        declare(&mut catalog, "<!ENTITY % p SYSTEM 'never-open'>").unwrap();
        let unit = DtdUnit::parse("<!ENTITY e '%p;'>", DtdLimits::default()).unwrap();
        catalog
            .apply(
                &unit,
                DeclarationContext {
                    internal_subset: false,
                    literals: LiteralSource::Physical,
                },
            )
            .unwrap();
        assert!(matches!(
            catalog.lookup_general("e"),
            EntityLookup::Unavailable(_)
        ));
        assert_eq!(
            declare(&mut catalog, "<!ENTITY later 'v'>")
                .unwrap()
                .ignored_after_unread_parameter,
            !standalone
        );
        assert_eq!(
            declare(&mut catalog, "<!ATTLIST r a CDATA 'v'>")
                .unwrap()
                .ignored_after_unread_parameter,
            !standalone
        );
        assert_eq!(catalog.attributes("r").count(), usize::from(standalone));
    }
}

#[test]
fn semantic_external_ids_are_unquoted_eol_processed_but_not_public_match_normalized() {
    for (source, public, system) in [
        (LiteralSource::Physical, "  a\n b  ", "a\nb\nc&literal;"),
        (
            LiteralSource::Replacement,
            "  a\r\n b  ",
            "a\r\nb\rc&literal;",
        ),
    ] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        let unit = DtdUnit::parse(
            "<!ENTITY e PUBLIC '  a\r\n b  ' 'a\r\nb\rc&literal;'>",
            DtdLimits::default(),
        )
        .unwrap();
        catalog
            .apply(
                &unit,
                DeclarationContext {
                    internal_subset: true,
                    literals: source,
                },
            )
            .unwrap();
        assert!(
            matches!(catalog.lookup_general("e"),EntityLookup::ExternalUnavailable {identifier:ExternalIdentifier {public:Some(actual_public),system:Some(actual_system)},..} if actual_public==public && actual_system==system)
        );
        assert_eq!(
            catalog.usage().storage_bytes,
            1 + public.len() + system.len()
        );
    }
}

#[test]
fn declaration_binding_and_retained_payload_ceilings_have_exact_boundaries() {
    let entity = "<!ENTITY e 'x'>";
    for (limit, ceiling) in [
        (CatalogLimit::Declarations, 1),
        (CatalogLimit::Bindings, 1),
        (CatalogLimit::StorageBytes, 2),
    ] {
        for allowed in [ceiling - 1, ceiling] {
            let mut limits = CatalogLimits::default();
            match limit {
                CatalogLimit::Declarations => limits.max_declarations = allowed,
                CatalogLimit::Bindings => limits.max_bindings = allowed,
                CatalogLimit::StorageBytes => limits.max_storage_bytes = allowed,
                _ => unreachable!(),
            }
            let mut catalog = DtdCatalog::new(limits, false);
            let result = declare(&mut catalog, entity);
            if allowed == ceiling {
                assert_eq!(result.unwrap().bindings_added, 1);
            } else {
                assert_eq!(result.unwrap_err().kind, CatalogErrorKind::Limit(limit));
            }
        }
    }
    let mut limits = CatalogLimits::default();
    limits.max_storage_bytes = 4;
    let mut catalog = DtdCatalog::new(limits, false);
    declare(&mut catalog, "<!ATTLIST r a CDATA 'x'>").unwrap();
    assert_eq!(catalog.usage().storage_bytes, 4); // r + indexed a + ordered a + x.
    declare(&mut catalog, "<!ATTLIST r a CDATA 'ignored'>").unwrap();
    assert_eq!(catalog.usage().storage_bytes, 4);
    assert_eq!(
        declare(&mut catalog, "<!ATTLIST r b CDATA #IMPLIED>")
            .unwrap_err()
            .kind,
        CatalogErrorKind::Limit(CatalogLimit::StorageBytes)
    );
    let mut limits = CatalogLimits::default();
    limits.max_storage_bytes = 2;
    let mut catalog = DtdCatalog::new(limits, false);
    declare(&mut catalog, entity).unwrap();
    declare(&mut catalog, "<!ENTITY e 'duplicate payload'>").unwrap();
    assert_eq!(catalog.usage().storage_bytes, 2);
}

#[test]
fn value_and_expansion_limits_count_utf8_bytes_and_pre_collapse_output() {
    for allowed in [1, 2] {
        let mut limits = CatalogLimits::default();
        limits.max_value_bytes = allowed;
        let mut catalog = DtdCatalog::new(limits, false);
        let result = declare(&mut catalog, "<!ENTITY e 'é'>");
        if allowed == 2 {
            result.unwrap();
        } else {
            assert_eq!(
                result.unwrap_err().kind,
                CatalogErrorKind::Limit(CatalogLimit::ValueBytes)
            );
        }
    }
    for allowed in [3, 4] {
        let mut limits = CatalogLimits::default();
        limits.max_expanded_bytes = allowed;
        let mut catalog = DtdCatalog::new(limits, false);
        declare(&mut catalog, "<!ENTITY e 'é'>").unwrap();
        let result = catalog.normalize_attribute_value("r", "a", "&e;", LiteralSource::Physical);
        if allowed == 4 {
            assert_eq!(result.unwrap(), NormalizedValue::Value("é".into()));
        } else {
            assert_eq!(
                result.unwrap_err().kind,
                CatalogErrorKind::Limit(CatalogLimit::ExpandedBytes)
            );
        }
    }
    let mut limits = CatalogLimits::default();
    limits.max_value_bytes = 0;
    limits.max_expanded_bytes = 2;
    let mut catalog = DtdCatalog::new(limits, false);
    declare(&mut catalog, "<!ATTLIST r a NMTOKENS #IMPLIED>").unwrap();
    assert_eq!(value(&mut catalog, "  "), "");
    assert_eq!(catalog.usage().expanded_bytes, 2);
    assert_eq!(
        catalog
            .normalize_attribute_value("r", "a", " ", LiteralSource::Physical)
            .unwrap_err()
            .kind,
        CatalogErrorKind::Limit(CatalogLimit::ExpandedBytes)
    );
}

#[test]
fn empty_entities_consume_reference_and_depth_budgets_without_output() {
    for allowed in 0..=2 {
        let mut limits = CatalogLimits::default();
        limits.max_references = allowed;
        limits.max_value_bytes = 0;
        limits.max_expanded_bytes = 0;
        let mut catalog = DtdCatalog::new(limits, false);
        declare(&mut catalog, "<!ENTITY e ''>").unwrap();
        let result = catalog.normalize_attribute_value("r", "a", "&e;&e;", LiteralSource::Physical);
        if allowed == 2 {
            assert_eq!(result.unwrap(), NormalizedValue::Value(String::new()));
        } else {
            let error = result.unwrap_err();
            assert_eq!(
                error.kind,
                CatalogErrorKind::Limit(CatalogLimit::References)
            );
            assert_eq!(error.location, ValueLocation::Input(allowed * 3));
        }
        assert_eq!(catalog.usage().expanded_bytes, 0);
    }
    for allowed in [0, 1] {
        let mut limits = CatalogLimits::default();
        limits.max_depth = allowed;
        let mut catalog = DtdCatalog::new(limits, false);
        declare(&mut catalog, "<!ENTITY e ''>").unwrap();
        let result = catalog.normalize_attribute_value("r", "a", "&e;", LiteralSource::Physical);
        if allowed == 1 {
            assert_eq!(result.unwrap(), NormalizedValue::Value(String::new()));
        } else {
            assert_eq!(
                result.unwrap_err().kind,
                CatalogErrorKind::Limit(CatalogLimit::Depth)
            );
        }
    }
}

#[test]
fn work_charges_entered_text_and_cycle_comparisons_before_consuming_replacements() {
    for allowed in [2, 3] {
        let mut limits = CatalogLimits::default();
        limits.max_work = allowed;
        let mut catalog = DtdCatalog::new(limits, false);
        let result = catalog.normalize_attribute_value("r", "a", "abc", LiteralSource::Physical);
        if allowed == 3 {
            assert_eq!(result.unwrap(), NormalizedValue::Value("abc".into()));
            assert_eq!(catalog.usage().work, 3);
        } else {
            assert_eq!(
                result.unwrap_err(),
                CatalogError {
                    kind: CatalogErrorKind::Limit(CatalogLimit::Work),
                    location: ValueLocation::Input(0)
                }
            );
        }
    }
    let unit = "<!ENTITY e ''>";
    for allowed in [unit.len() + 3, unit.len() + 4] {
        let mut limits = CatalogLimits::default();
        limits.max_work = allowed;
        let mut catalog = DtdCatalog::new(limits, false);
        declare(&mut catalog, unit).unwrap();
        let result = catalog.normalize_attribute_value("r", "a", "&e;", LiteralSource::Physical);
        if allowed == unit.len() + 4 {
            assert_eq!(result.unwrap(), NormalizedValue::Value(String::new()));
            assert_eq!(catalog.usage().work, allowed);
        } else {
            assert_eq!(
                result.unwrap_err().kind,
                CatalogErrorKind::Limit(CatalogLimit::Work)
            );
        }
    }
}

#[test]
fn deep_general_entity_expansion_uses_frames_and_preserves_unicode_names_and_values() {
    let mut limits = CatalogLimits::default();
    limits.max_depth = 1024;
    let mut catalog = DtdCatalog::new(limits, false);
    for index in 0..1023 {
        declare(
            &mut catalog,
            &format!("<!ENTITY e{index} '&e{};'>", index + 1),
        )
        .unwrap();
    }
    declare(&mut catalog, "<!ENTITY e1023 '&é;'>").unwrap();
    declare(&mut catalog, "<!ENTITY é '🙂'>").unwrap();
    assert_eq!(
        catalog
            .normalize_attribute_value("r", "a", "&e0;", LiteralSource::Physical)
            .unwrap_err()
            .kind,
        CatalogErrorKind::Limit(CatalogLimit::Depth)
    );
    let mut limits = CatalogLimits::default();
    limits.max_depth = 1025;
    let mut catalog = DtdCatalog::new(limits, false);
    for index in 0..1023 {
        declare(
            &mut catalog,
            &format!("<!ENTITY e{index} '&e{};'>", index + 1),
        )
        .unwrap();
    }
    declare(&mut catalog, "<!ENTITY e1023 '&é;'>").unwrap();
    declare(&mut catalog, "<!ENTITY é '🙂'>").unwrap();
    assert_eq!(value(&mut catalog, "&e0;"), "🙂");
}

#[test]
fn exact_unit_offsets_are_exposed_without_changing_lexical_views() {
    let unit = DtdUnit::parse("<!ENTITY e 'a&#65;&later;%p;'>", DtdLimits::default()).unwrap();
    let DtdUnitView::Entity(entity) = unit.view() else {
        panic!("entity")
    };
    assert_eq!(entity.quoted_literal_offset(), Some(11));
    let positions: Vec<_> = unit
        .reference_positions()
        .map(|(offset, reference)| (offset, reference.kind, reference.spelling))
        .collect();
    assert_eq!(
        positions,
        vec![
            (13, ReferenceKind::DecimalCharacter, "&#65;"),
            (18, ReferenceKind::General, "&later;"),
            (25, ReferenceKind::Parameter, "%p;")
        ]
    );
    let unit = DtdUnit::parse("<!ENTITY e SYSTEM 'x'>", DtdLimits::default()).unwrap();
    let DtdUnitView::Entity(entity) = unit.view() else {
        panic!("entity")
    };
    assert_eq!(entity.quoted_literal_offset(), None);
    let unit = DtdUnit::parse(
        "<!ATTLIST r a CDATA #IMPLIED b CDATA #REQUIRED c CDATA 'v'>",
        DtdLimits::default(),
    )
    .unwrap();
    let DtdUnitView::AttributeList(list) = unit.view() else {
        panic!("list")
    };
    assert_eq!(
        list.definitions()
            .map(|d| d.default_literal_offset())
            .collect::<Vec<_>>(),
        vec![None, None, Some(55)]
    );
}

#[test]
fn fatal_errors_preserve_only_a_read_only_prefix_and_never_resume_catalog_mutation() {
    let mut limits = CatalogLimits::default();
    limits.max_bindings = 1;
    let mut catalog = DtdCatalog::new(limits, false);
    let error = declare(&mut catalog, "<!ATTLIST r a CDATA 'v' b CDATA 'w'>").unwrap_err();
    assert_eq!(error.kind, CatalogErrorKind::Limit(CatalogLimit::Bindings));
    assert_eq!(
        catalog
            .attributes("r")
            .map(|attribute| attribute.name())
            .collect::<Vec<_>>(),
        vec!["a"]
    );
    let usage = catalog.usage();
    assert_eq!(declare(&mut catalog, "<!ENTITY e 'x'>"), Err(error));
    assert_eq!(
        catalog.normalize_attribute_value("r", "a", "valid", LiteralSource::Physical),
        Err(error)
    );
    assert_eq!(catalog.usage(), usage);
}
