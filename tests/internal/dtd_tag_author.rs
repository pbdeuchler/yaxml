use super::*;
use crate::dtd::{DtdLimits, DtdUnit};
use crate::dtd_catalog::{CatalogLimits, DeclarationContext, EntityLookup};
use crate::parser::ParserState;
use crate::tokenizer::TokenKind;
use crate::{ErrorKind, Event, Limit, Limits};
use std::ops::Range;

fn declare(catalog: &mut DtdCatalog, raw: &str) {
    let unit = DtdUnit::parse(raw, DtdLimits::default()).unwrap();
    catalog
        .apply(
            &unit,
            DeclarationContext {
                internal_subset: true,
                literals: LiteralSource::Physical,
            },
        )
        .unwrap();
}

fn prepare<'a>(
    state: &mut ParserState<'a>,
    catalog: &mut DtdCatalog,
    raw: &'a str,
    source: LiteralSource,
) -> Result<PreparedTag<'a>, DtdTagError> {
    let tag = state.collect_raw_tag(raw, 0).unwrap();
    prepare_dtd_tag(catalog, state.owned_budget(), tag, source)
}

#[test]
fn prepared_defaults_precede_namespace_resolution_without_reparsing() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(
        &mut catalog,
        "<!ATTLIST p:r a NMTOKENS #IMPLIED xmlns:p CDATA 'urn:default'>",
    );
    let mut state = ParserState::new(Limits::default(), 0);
    let raw = state.collect_raw_tag("<p:r a='  x  '/>", 0).unwrap();
    let prepared = prepare_dtd_tag(
        &mut catalog,
        state.owned_budget(),
        raw,
        LiteralSource::Physical,
    )
    .unwrap();
    assert_eq!(prepared.attributes.len(), 2);
    assert_eq!(prepared.attributes[0].value, "x");
    assert!(matches!(
        prepared.attributes[0].provenance,
        AttributeProvenance::Specified {
            name_offset: 5,
            value_offset: 8
        }
    ));
    assert!(matches!(
        prepared.attributes[1].provenance,
        AttributeProvenance::Defaulted { declaration: 0 }
    ));

    let event = state
        .publish_prepared_tag(prepared, &mut |value, _, _| Ok(Cow::Borrowed(value)))
        .unwrap();
    let Event::Empty(element) = event else {
        panic!("empty element")
    };
    assert_eq!(element.name.qualified(), "p:r");
    assert_eq!(element.name.namespace(), Some("urn:default"));
    assert_eq!(element.attributes[0].name.qualified(), "a");
    assert_eq!(element.attributes[0].value, "x");
    assert_eq!(element.attributes[1].name.qualified(), "xmlns:p");
    assert_eq!(
        element.attributes[1].name.namespace(),
        Some("http://www.w3.org/2000/xmlns/")
    );
}

#[test]
fn literal_source_controls_one_eol_phase_and_replacement_names_are_owned() {
    for (source, expected, owned) in [
        (LiteralSource::Physical, "a b c", false),
        (LiteralSource::Replacement, "a  b c", true),
    ] {
        let raw = "<p:r p:a='a\r\nb\rc'/>";
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        let mut state = ParserState::new(Limits::default(), 0);
        let prepared = prepare(&mut state, &mut catalog, raw, source).unwrap();
        assert_eq!(prepared.attributes[0].value, expected);
        assert_eq!(
            matches!(prepared.name.qualified_source(), Cow::Owned(_)),
            owned
        );
        assert_eq!(
            matches!(
                prepared.attributes[0].name.qualified_source(),
                Cow::Owned(_)
            ),
            owned
        );
    }
}

#[test]
fn invalid_unused_dtd_names_are_deferred_until_a_default_is_applied() {
    let mut unused = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut unused, "<!ATTLIST r a:b:c CDATA #IMPLIED>");
    let mut state = ParserState::new(Limits::default(), 0);
    assert!(prepare(&mut state, &mut unused, "<r/>", LiteralSource::Physical).is_ok());

    let mut applied = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut applied, "<!ATTLIST r a:b:c CDATA 'v'>");
    let mut state = ParserState::new(Limits::default(), 0);
    assert_eq!(
        prepare(&mut state, &mut applied, "<r/>", LiteralSource::Physical).unwrap_err(),
        DtdTagError {
            kind: DtdTagErrorKind::Parser(ErrorKind::InvalidName),
            location: DtdTagLocation::Declaration {
                declaration: 0,
                offset: 15
            }
        }
    );

    let mut unicode = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut unicode, "<!ATTLIST 根 π:a:b CDATA 'v'>");
    let mut state = ParserState::new(Limits::default(), 0);
    assert_eq!(
        prepare(&mut state, &mut unicode, "<根/>", LiteralSource::Physical).unwrap_err(),
        DtdTagError {
            kind: DtdTagErrorKind::Parser(ErrorKind::InvalidName),
            location: DtdTagLocation::Declaration {
                declaration: 0,
                offset: 18
            }
        }
    );
}

#[test]
fn final_attribute_limit_names_the_first_unretained_default() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ATTLIST r a CDATA 'one'>");
    declare(&mut catalog, "<!ATTLIST r b CDATA 'two'>");
    let mut state = ParserState::new(
        Limits {
            max_attributes: 1,
            ..Limits::default()
        },
        0,
    );
    assert_eq!(
        prepare(&mut state, &mut catalog, "<r/>", LiteralSource::Physical).unwrap_err(),
        DtdTagError {
            kind: DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(Limit::Attributes)),
            location: DtdTagLocation::Declaration {
                declaration: 1,
                offset: 0
            }
        }
    );
}

#[test]
fn explicit_value_locations_are_rebased_without_touching_entity_locations() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    let mut state = ParserState::new(Limits::default(), 0);
    let tag = state.collect_raw_tag("<r a='x&missing;'/>", 100).unwrap();
    assert_eq!(
        prepare_dtd_tag(
            &mut catalog,
            state.owned_budget(),
            tag,
            LiteralSource::Physical
        )
        .unwrap_err(),
        DtdTagError {
            kind: DtdTagErrorKind::Catalog(CatalogErrorKind::EntityDeclared),
            location: DtdTagLocation::Value(ValueLocation::Input(107))
        }
    );

    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!DOCTYPE r SYSTEM 'never' [");
    let mut state = ParserState::new(Limits::default(), 0);
    let tag = state.collect_raw_tag("<r z='xx&unknown;'/>", 40).unwrap();
    let prepared = prepare_dtd_tag(
        &mut catalog,
        state.owned_budget(),
        tag,
        LiteralSource::Physical,
    )
    .unwrap();
    assert_eq!(prepared.attributes[0].value, "xx");

    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ENTITY e '&later;'>");
    let mut state = ParserState::new(Limits::default(), 0);
    let tag = state.collect_raw_tag("<r a='&e;'/>", 80).unwrap();
    assert!(matches!(
        prepare_dtd_tag(
            &mut catalog,
            state.owned_budget(),
            tag,
            LiteralSource::Physical
        )
        .unwrap_err(),
        DtdTagError {
            kind: DtdTagErrorKind::Catalog(CatalogErrorKind::EntityDeclared),
            location: DtdTagLocation::Value(ValueLocation::Replacement { offset: 0, .. })
        }
    ));
}

#[test]
fn default_namespace_constraints_keep_declaration_locations() {
    for declaration in [
        "<!ATTLIST r xmlns:xmlns CDATA 'urn:x'>",
        "<!ATTLIST r xmlns:p CDATA 'http://www.w3.org/2000/xmlns/'>",
        "<!ATTLIST r xmlns:xml CDATA 'urn:not-xml'>",
        "<!ATTLIST r xmlns:p CDATA 'http://www.w3.org/XML/1998/namespace'>",
        "<!ATTLIST r xmlns:p CDATA ''>",
    ] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        declare(&mut catalog, declaration);
        let mut state = ParserState::new(Limits::default(), 0);
        let prepared = prepare(&mut state, &mut catalog, "<r/>", LiteralSource::Physical).unwrap();
        assert_eq!(
            state
                .publish_prepared_tag(prepared, &mut |value, _, _| Ok(Cow::Borrowed(value)))
                .unwrap_err(),
            crate::parser::PreparedTagError {
                kind: ErrorKind::Namespace,
                location: crate::parser::PreparedTagErrorLocation::Declaration {
                    declaration: 0,
                    offset: 0
                }
            }
        );
    }
}

#[test]
fn expanded_duplicate_from_a_default_reports_its_declaration() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(
        &mut catalog,
        "<!ATTLIST r xmlns:p CDATA 'urn:u' xmlns:q CDATA 'urn:u'>",
    );
    declare(&mut catalog, "<!ATTLIST r q:a CDATA 'default'>");
    let mut state = ParserState::new(Limits::default(), 0);
    let prepared = prepare(
        &mut state,
        &mut catalog,
        "<r p:a='explicit'/>",
        LiteralSource::Physical,
    )
    .unwrap();
    let error = state
        .publish_prepared_tag(prepared, &mut |value, _, _| Ok(Cow::Borrowed(value)))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::DuplicateAttribute);
    assert_eq!(
        error.location,
        crate::parser::PreparedTagErrorLocation::Declaration {
            declaration: 1,
            offset: 0
        }
    );
}

#[test]
fn failed_publication_does_not_leave_a_namespace_binding() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(
        &mut catalog,
        "<!ATTLIST r xmlns:p CDATA 'urn:u' xmlns:q CDATA ''>",
    );
    let mut state = ParserState::new(Limits::default(), 0);
    let prepared = prepare(&mut state, &mut catalog, "<r/>", LiteralSource::Physical).unwrap();
    assert_eq!(
        state
            .publish_prepared_tag(prepared, &mut |value, _, _| Ok(Cow::Borrowed(value)))
            .unwrap_err()
            .kind,
        ErrorKind::Namespace
    );

    let raw = "<p:r/>";
    assert_eq!(
        state
            .process_token(
                TokenKind::Start,
                raw,
                Range {
                    start: 0,
                    end: raw.len()
                },
                |value, _, _| { Ok(Cow::Borrowed(value)) }
            )
            .unwrap_err()
            .kind,
        ErrorKind::Namespace
    );
}

#[test]
fn lexical_collection_finishes_before_catalog_normalization() {
    let catalog = DtdCatalog::new(CatalogLimits::default(), false);
    let before = catalog.usage();
    let state = ParserState::new(Limits::default(), 0);
    let error = state.collect_raw_tag("<r a='&missing;' b>", 0).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(catalog.usage(), before);
}

#[test]
fn explicit_order_suppression_and_first_defaults_are_literal() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ATTLIST r a CDATA 'first'>");
    declare(&mut catalog, "<!ATTLIST r b CDATA 'default-b'>");
    declare(&mut catalog, "<!ATTLIST r a CDATA 'ignored'>");
    let mut state = ParserState::new(Limits::default(), 0);
    let prepared = prepare(
        &mut state,
        &mut catalog,
        "<r b='explicit-b'/>",
        LiteralSource::Physical,
    )
    .unwrap();
    let actual: Vec<_> = prepared
        .attributes
        .iter()
        .map(|attribute| {
            (
                attribute.name.qualified(),
                attribute.value.as_ref(),
                attribute.provenance,
            )
        })
        .collect();
    assert_eq!(
        actual,
        vec![
            (
                "b",
                "explicit-b",
                AttributeProvenance::Specified {
                    name_offset: 3,
                    value_offset: 6
                }
            ),
            (
                "a",
                "first",
                AttributeProvenance::Defaulted { declaration: 0 }
            )
        ]
    );
}

#[test]
fn lexical_duplicate_and_staged_error_precedence_are_explicit() {
    let mut state = ParserState::new(Limits::default(), 0);
    assert_eq!(
        state
            .collect_raw_tag("<r a='ok' a='&missing;'/>", 10)
            .unwrap_err(),
        crate::Error {
            kind: ErrorKind::DuplicateAttribute,
            offset: 20
        }
    );

    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    assert_eq!(
        state
            .collect_raw_tag("<r a='&missing;' b>", 20)
            .unwrap_err()
            .kind,
        ErrorKind::Syntax
    );
    let tag = state
        .collect_raw_tag("<r a='&missing;' xmlns:p=''>", 30)
        .unwrap();
    assert_eq!(
        prepare_dtd_tag(
            &mut catalog,
            state.owned_budget(),
            tag,
            LiteralSource::Physical
        )
        .unwrap_err()
        .kind,
        DtdTagErrorKind::Catalog(CatalogErrorKind::EntityDeclared)
    );
}

#[test]
fn shared_owned_budget_counts_names_values_and_defaults_once() {
    for (source, ceiling, result, remaining) in [
        (LiteralSource::Physical, 63, Err(6), 63),
        (LiteralSource::Physical, 64, Ok(()), 0),
        (LiteralSource::Replacement, 65, Err(6), 63),
        (LiteralSource::Replacement, 66, Ok(()), 0),
    ] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        let mut state = ParserState::new(
            Limits {
                max_owned_bytes: ceiling,
                ..Limits::default()
            },
            0,
        );
        let actual = prepare(&mut state, &mut catalog, "<r a='v'/>", source)
            .map(|_| ())
            .map_err(|error| match error.location {
                DtdTagLocation::Input(offset) => offset,
                DtdTagLocation::Value(ValueLocation::Input(offset)) => offset,
                _ => usize::MAX,
            });
        assert_eq!(actual, result);
        assert_eq!(state.owned_budget().remaining_owned(), remaining);
    }

    for ceiling in [1, 2] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        declare(&mut catalog, "<!ATTLIST r a CDATA 'v'>");
        let mut state = ParserState::new(
            Limits {
                max_owned_bytes: ceiling,
                ..Limits::default()
            },
            0,
        );
        let result = prepare(&mut state, &mut catalog, "<r/>", LiteralSource::Physical);
        assert_eq!(result.is_ok(), ceiling == 2);
        if ceiling == 1 {
            assert_eq!(
                result.unwrap_err(),
                DtdTagError {
                    kind: DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(Limit::OwnedBytes)),
                    location: DtdTagLocation::Declaration {
                        declaration: 0,
                        offset: 0
                    }
                }
            );
        }
    }
}

#[test]
fn normalization_obeys_the_smaller_catalog_or_parser_output_ceiling() {
    let raw = "<r a='&e;'/>";

    for ceiling in [63, 64] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        declare(&mut catalog, "<!ENTITY e 'abcd'>");
        let EntityLookup::Internal { id: entity, .. } = catalog.lookup_general("e") else {
            panic!("internal entity")
        };
        let mut state = ParserState::new(
            Limits {
                max_owned_bytes: ceiling,
                ..Limits::default()
            },
            0,
        );
        let result = prepare(&mut state, &mut catalog, raw, LiteralSource::Physical);
        if ceiling == 63 {
            assert_eq!(
                result.unwrap_err(),
                DtdTagError {
                    kind: DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(Limit::OwnedBytes)),
                    location: DtdTagLocation::Value(ValueLocation::Replacement {
                        entity,
                        offset: 0,
                    }),
                }
            );
            assert_eq!(
                catalog
                    .normalize_attribute_value("r", "a", "x", LiteralSource::Physical)
                    .unwrap(),
                String::from("x"),
            );
        } else {
            assert_eq!(result.unwrap().attributes[0].value, "abcd");
            assert_eq!(state.owned_budget().remaining_owned(), 0);
        }
    }

    let catalog_limits = CatalogLimits {
        max_value_bytes: 3,
        ..CatalogLimits::default()
    };
    let mut catalog = DtdCatalog::new(catalog_limits, false);
    let mut state = ParserState::new(Limits::default(), 0);
    assert_eq!(
        prepare(
            &mut state,
            &mut catalog,
            "<r a='abcd'/>",
            LiteralSource::Physical,
        )
        .unwrap_err(),
        DtdTagError {
            kind: DtdTagErrorKind::Catalog(CatalogErrorKind::Limit(
                crate::dtd_catalog::CatalogLimit::ValueBytes,
            )),
            location: DtdTagLocation::Value(ValueLocation::Input(9)),
        }
    );
    assert_eq!(
        state.owned_budget().remaining_owned(),
        Limits::default().max_owned_bytes - 3,
    );
}

#[test]
fn normalization_charges_cumulative_full_capacity_requests() {
    for (length, requested, failure_at, prior_requested) in
        [(1, 64, 0, 0), (65, 192, 64, 64), (129, 448, 128, 192)]
    {
        let value = "x".repeat(length);
        let raw = format!("<r a='{value}'/>");

        let mut below_catalog = DtdCatalog::new(CatalogLimits::default(), false);
        let mut below = ParserState::new(
            Limits {
                max_owned_bytes: requested - 1,
                ..Limits::default()
            },
            0,
        );
        assert_eq!(
            prepare(
                &mut below,
                &mut below_catalog,
                &raw,
                LiteralSource::Physical,
            )
            .unwrap_err(),
            DtdTagError {
                kind: DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(Limit::OwnedBytes)),
                location: DtdTagLocation::Value(ValueLocation::Input(6 + failure_at)),
            },
        );
        assert_eq!(
            below.owned_budget().remaining_owned(),
            requested - 1 - prior_requested,
        );

        let mut exact_catalog = DtdCatalog::new(CatalogLimits::default(), false);
        let mut exact = ParserState::new(
            Limits {
                max_owned_bytes: requested,
                ..Limits::default()
            },
            0,
        );
        let prepared = prepare(
            &mut exact,
            &mut exact_catalog,
            &raw,
            LiteralSource::Physical,
        )
        .unwrap();
        assert_eq!(prepared.attributes[0].value, value);
        assert_eq!(exact.owned_budget().remaining_owned(), 0);
    }
}

#[test]
fn retention_failure_after_one_planned_binding_publishes_nothing() {
    let mut state = ParserState::new(
        Limits {
            max_owned_bytes: 5,
            ..Limits::default()
        },
        0,
    );
    let prepared = PreparedTag {
        name: Name::new(String::from("r"), None).unwrap(),
        attributes: vec![
            PreparedAttribute {
                name: Name::new(String::from("xmlns:p"), None).unwrap(),
                value: Cow::Owned(String::from("u")),
                provenance: AttributeProvenance::Specified {
                    name_offset: 3,
                    value_offset: 12,
                },
            },
            PreparedAttribute {
                name: Name::new(String::from("xmlns:q"), None).unwrap(),
                value: Cow::Owned(String::from("v")),
                provenance: AttributeProvenance::Specified {
                    name_offset: 15,
                    value_offset: 24,
                },
            },
        ],
        empty: true,
        start: 0,
    };
    let error = state
        .publish_prepared_tag(prepared, &mut |value, _, _| Ok(Cow::Borrowed(value)))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::LimitExceeded(Limit::OwnedBytes));
    assert_eq!(
        error.location,
        crate::parser::PreparedTagErrorLocation::Input(15)
    );

    let raw = "<p:r/>";
    assert_eq!(
        state
            .process_token(TokenKind::Start, raw, 0..raw.len(), |value, _, _| {
                Ok(Cow::Borrowed(value))
            })
            .unwrap_err()
            .kind,
        ErrorKind::Namespace
    );
}

#[test]
fn many_unique_prefixes_and_uri_aliases_resolve_from_pending_indexes() {
    const PREFIXES: usize = 64;
    let mut attributes = Vec::new();
    attributes.try_reserve(PREFIXES * 2).unwrap();
    for index in 0..PREFIXES {
        attributes.push(PreparedAttribute {
            name: Name::new(format!("xmlns:p{index}"), None).unwrap(),
            value: Cow::Owned(format!("urn:{}", index % 4)),
            provenance: AttributeProvenance::Specified {
                name_offset: index,
                value_offset: index,
            },
        });
    }
    for index in 0..PREFIXES {
        attributes.push(PreparedAttribute {
            name: Name::new(format!("p{index}:a{index}"), None).unwrap(),
            value: Cow::Owned(String::from("v")),
            provenance: AttributeProvenance::Specified {
                name_offset: PREFIXES + index,
                value_offset: PREFIXES + index,
            },
        });
    }
    let prepared = PreparedTag {
        name: Name::new(String::from("r"), None).unwrap(),
        attributes,
        empty: true,
        start: 0,
    };
    let mut state = ParserState::new(Limits::default(), 0);
    let Event::Empty(element) = state
        .publish_prepared_tag(prepared, &mut |value, _, _| Ok(Cow::Borrowed(value)))
        .unwrap()
    else {
        panic!("empty element")
    };
    assert_eq!(element.attributes.len(), PREFIXES * 2);
    for index in 0..PREFIXES {
        let attribute = &element.attributes[PREFIXES + index];
        assert_eq!(attribute.name.qualified(), format!("p{index}:a{index}"));
        let namespace = format!("urn:{}", index % 4);
        assert_eq!(attribute.name.namespace(), Some(namespace.as_str()));
    }
}

#[test]
fn ordinary_start_tags_keep_the_shipped_one_pass_error_order() {
    let raw = "<r a='ok' a='&missing;'/>";
    let mut state = ParserState::new(Limits::default(), 0);
    let error = state
        .process_token(TokenKind::Start, raw, 0..raw.len(), |value, _, _| {
            Ok(Cow::Borrowed(value))
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::UnknownEntity);
    assert_eq!(error.offset, 13);

    let state = ParserState::new(Limits::default(), 0);
    let error = state.collect_raw_tag(raw, 0).unwrap_err();
    assert_eq!(error.kind, ErrorKind::DuplicateAttribute);
    assert_eq!(error.offset, 10);
}

#[test]
fn dtd_start_orchestration_applies_defaults_and_commits_one_event() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(
        &mut catalog,
        "<!ATTLIST p:r xmlns:p CDATA 'urn:dtd' a CDATA 'v'>",
    );
    let mut state = ParserState::new(
        Limits {
            max_events: 1,
            ..Limits::default()
        },
        0,
    );
    let raw = "<p:r/>";
    let Event::Empty(element) = state
        .process_dtd_start(
            raw,
            0,
            &mut catalog,
            LiteralSource::Physical,
            &mut |value, _, _| Ok(Cow::Borrowed(value)),
        )
        .unwrap()
    else {
        panic!("empty element")
    };
    assert_eq!(element.name.namespace(), Some("urn:dtd"));
    assert_eq!(element.attributes.len(), 2);
    assert_eq!(element.attributes[0].name.qualified(), "xmlns:p");
    assert_eq!(element.attributes[1].name.qualified(), "a");
    assert_eq!(element.attributes[1].value, "v");

    assert_eq!(
        state
            .process_dtd_start(
                "<r/>",
                raw.len(),
                &mut catalog,
                LiteralSource::Physical,
                &mut |value, _, _| Ok(Cow::Borrowed(value)),
            )
            .unwrap_err()
            .kind,
        DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(Limit::Events))
    );
}

#[test]
fn failed_dtd_start_does_not_commit_event_or_namespace_state() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    let mut state = ParserState::new(
        Limits {
            max_events: 1,
            ..Limits::default()
        },
        0,
    );
    assert_eq!(
        state
            .process_dtd_start(
                "<p:r/>",
                0,
                &mut catalog,
                LiteralSource::Physical,
                &mut |value, _, _| Ok(Cow::Borrowed(value)),
            )
            .unwrap_err()
            .kind,
        DtdTagErrorKind::Parser(ErrorKind::Namespace)
    );

    let Event::Empty(element) = state
        .process_dtd_start(
            "<r/>",
            6,
            &mut catalog,
            LiteralSource::Physical,
            &mut |value, _, _| Ok(Cow::Borrowed(value)),
        )
        .unwrap()
    else {
        panic!("empty element")
    };
    assert_eq!(element.name.qualified(), "r");
}

#[test]
fn dtd_start_checks_token_encoding_event_and_depth_before_collection() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    for (limits, expected) in [
        (
            Limits {
                max_token_bytes: 3,
                ..Limits::default()
            },
            ErrorKind::LimitExceeded(Limit::TokenBytes),
        ),
        (
            Limits {
                max_events: 0,
                ..Limits::default()
            },
            ErrorKind::LimitExceeded(Limit::Events),
        ),
        (
            Limits {
                max_depth: 0,
                ..Limits::default()
            },
            ErrorKind::LimitExceeded(Limit::Depth),
        ),
    ] {
        let mut state = ParserState::new(limits, 0);
        let error = state
            .process_dtd_start(
                "<r/>",
                0,
                &mut catalog,
                LiteralSource::Physical,
                &mut |value, _, _| Ok(Cow::Borrowed(value)),
            )
            .unwrap_err();
        assert_eq!(error.kind, DtdTagErrorKind::Parser(expected));
        assert_eq!(error.location, DtdTagLocation::Input(0));
    }

    let mut state = ParserState::new(Limits::default(), 0);
    state.set_encoding(
        crate::encoding::SourceEncoding::Utf16 {
            little_endian: true,
            bom: false,
        },
        0,
        Limits::default().max_token_bytes,
    );
    let error = state
        .process_dtd_start(
            "<r/>",
            0,
            &mut catalog,
            LiteralSource::Physical,
            &mut |value, _, _| Ok(Cow::Borrowed(value)),
        )
        .unwrap_err();
    assert_eq!(
        error.kind,
        DtdTagErrorKind::Parser(ErrorKind::EncodingMismatch)
    );
    assert_eq!(error.location, DtdTagLocation::Input(0));
}
