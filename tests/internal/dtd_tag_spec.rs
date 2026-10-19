//! Independent tag/default oracle. Expected strings are literal XML phase rules.
use super::*;
use crate::dtd::{DtdLimits, DtdUnit, LiteralSource};
use crate::dtd_catalog::{
    CatalogErrorKind, CatalogLimit, CatalogLimits, DeclarationContext, DtdCatalog, ValueLocation,
};
use crate::dtd_tag::{DtdTagErrorKind, DtdTagLocation, prepare_dtd_tag};

fn catalog(declarations: &[&str]) -> DtdCatalog {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    for declaration in declarations {
        let unit = DtdUnit::parse(*declaration, DtdLimits::default()).unwrap();
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
    catalog
}

fn prepare<'event>(
    state: &mut ParserState<'_>,
    catalog: &mut DtdCatalog,
    raw: &'event str,
    start: usize,
    source: LiteralSource,
) -> Result<PreparedTag<'event>, crate::dtd_tag::DtdTagError> {
    let collected = state.collect_raw_tag(raw, start)?;
    prepare_dtd_tag(catalog, state.owned_budget(), collected, source)
}

fn publish<'event>(
    state: &mut ParserState<'static>,
    prepared: PreparedTag<'event>,
) -> Event<'event> {
    state
        .publish_prepared_tag(prepared, &mut |raw, offset, budget| {
            budget.copy(raw, offset)
        })
        .unwrap()
}

fn values<'a>(prepared: &'a PreparedTag<'_>) -> Vec<(&'a str, &'a str)> {
    prepared
        .attributes
        .iter()
        .map(|attribute| (attribute.name.qualified(), attribute.value.as_ref()))
        .collect()
}

#[test]
fn explicit_order_then_first_binding_defaults_and_nonvalidating_fixed_policy() {
    let mut catalog = catalog(&[
        "<!ATTLIST r a CDATA 'first' b CDATA #FIXED 'fixed' c CDATA #REQUIRED d CDATA #IMPLIED>",
        "<!ATTLIST r a CDATA 'later' e CDATA 'end'>",
    ]);
    let mut state = ParserState::new(Limits::default(), 0);
    let prepared = prepare(
        &mut state,
        &mut catalog,
        "<r z='specified' b='override'/>",
        0,
        LiteralSource::Physical,
    )
    .unwrap();
    assert_eq!(
        values(&prepared),
        [
            ("z", "specified"),
            ("b", "override"),
            ("a", "first"),
            ("e", "end")
        ]
    );
    assert!(matches!(
        prepared.attributes[0].provenance,
        AttributeProvenance::Specified { .. }
    ));
    assert_eq!(
        prepared.attributes[2].provenance,
        AttributeProvenance::Defaulted { declaration: 0 }
    );
    assert_eq!(
        prepared.attributes[3].provenance,
        AttributeProvenance::Defaulted { declaration: 1 }
    );
}

#[test]
fn explicit_whitespace_is_normalized_once_with_numeric_and_general_distinction() {
    for (source, expected) in [
        (LiteralSource::Physical, " x y \t\n\r &#x9; "),
        (LiteralSource::Replacement, " x  y \t\n\r &#x9; "),
    ] {
        let mut catalog = catalog(&["<!ENTITY escaped '&amp;#x9;'>"]);
        let mut state = ParserState::new(Limits::default(), 0);
        let prepared = prepare(
            &mut state,
            &mut catalog,
            "<r a=' x\r\ny &#x9;&#xA;&#xD; &escaped; '/>",
            0,
            source,
        )
        .unwrap();
        assert_eq!(prepared.attributes[0].value, expected, "{source:?}");
    }
}

#[test]
fn tokenized_explicit_and_default_values_collapse_only_spaces() {
    let mut catalog = catalog(&["<!ATTLIST r a NMTOKENS #IMPLIED b NMTOKENS '  &#x9;  x \t y  '>"]);
    let mut state = ParserState::new(Limits::default(), 0);
    let prepared = prepare(
        &mut state,
        &mut catalog,
        "<r a='  &#x9;  x \t y  '/>",
        0,
        LiteralSource::Physical,
    )
    .unwrap();
    assert_eq!(values(&prepared), [("a", "\t x y"), ("b", "\t x y")]);
}

#[test]
fn dtd_matching_is_lexical_and_unused_default_qname_is_not_eagerly_rejected() {
    let mut catalog = catalog(&["<!ATTLIST p:r a:b:c CDATA 'unused'>"]);
    let mut state = ParserState::new(Limits::default(), 0);
    let prepared = prepare(
        &mut state,
        &mut catalog,
        "<q:r xmlns:q='urn:r'/>",
        0,
        LiteralSource::Physical,
    )
    .unwrap();
    assert_eq!(values(&prepared), [("xmlns:q", "urn:r")]);
    let error = prepare(
        &mut state,
        &mut catalog,
        "<p:r xmlns:p='urn:r'/>",
        40,
        LiteralSource::Physical,
    )
    .err()
    .unwrap();
    assert_eq!(error.kind, DtdTagErrorKind::Parser(ErrorKind::InvalidName));
    assert!(matches!(
        error.location,
        DtdTagLocation::Declaration { declaration: 0, .. }
    ));
}

#[test]
fn default_namespaces_resolve_element_and_attributes_after_all_defaults() {
    let mut catalog = catalog(&[
        "<!ATTLIST p:r p:a CDATA 'value' xmlns:p CDATA 'urn:p' xmlns CDATA 'urn:default' plain CDATA 'v'>",
    ]);
    let mut state = ParserState::new(Limits::default(), 0);
    let prepared = prepare(
        &mut state,
        &mut catalog,
        "<p:r/>",
        0,
        LiteralSource::Physical,
    )
    .unwrap();
    let Event::Empty(element) = publish(&mut state, prepared) else {
        panic!("expected empty element")
    };
    assert_eq!(element.name.namespace(), Some("urn:p"));
    assert_eq!(element.attributes[0].name.namespace(), Some("urn:p"));
    assert_eq!(element.attributes[1].name.namespace(), Some(XMLNS));
    assert_eq!(element.attributes[2].name.namespace(), Some(XMLNS));
    assert_eq!(element.attributes[3].name.namespace(), None);
    assert!(state.bindings.is_empty());
    assert!(state.namespace_ids.is_empty());
    assert_eq!(state.element_depth(), 0);
}

#[test]
fn explicit_namespace_override_controls_defaulted_names() {
    let mut catalog = catalog(&["<!ATTLIST p:r xmlns:p CDATA 'urn:default' p:a CDATA 'value'>"]);
    let mut state = ParserState::new(Limits::default(), 0);
    let prepared = prepare(
        &mut state,
        &mut catalog,
        "<p:r xmlns:p='urn:explicit'/>",
        0,
        LiteralSource::Physical,
    )
    .unwrap();
    assert_eq!(
        values(&prepared),
        [("xmlns:p", "urn:explicit"), ("p:a", "value")]
    );
    let Event::Empty(element) = publish(&mut state, prepared) else {
        panic!("expected empty element")
    };
    assert_eq!(element.name.namespace(), Some("urn:explicit"));
    assert_eq!(element.attributes[1].name.namespace(), Some("urn:explicit"));
}

#[test]
fn applied_defaults_count_toward_attribute_limit_including_xmlns() {
    for ceiling in [2, 3] {
        let mut catalog = catalog(&["<!ATTLIST r a CDATA 'a' xmlns:p CDATA 'urn:p'>"]);
        let mut state = ParserState::new(
            Limits {
                max_attributes: ceiling,
                ..Limits::default()
            },
            0,
        );
        let result = prepare(
            &mut state,
            &mut catalog,
            "<r z='z'/>",
            0,
            LiteralSource::Physical,
        );
        if ceiling == 3 {
            assert_eq!(values(&result.unwrap()).len(), 3);
        } else {
            let error = result.err().unwrap();
            assert_eq!(
                error.kind,
                DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(Limit::Attributes))
            );
            assert_eq!(
                error.location,
                DtdTagLocation::Declaration {
                    declaration: 0,
                    offset: 0
                }
            );
        }
    }
}

#[test]
fn generated_name_and_value_copies_spend_same_cumulative_budget() {
    for (source, required) in [
        (LiteralSource::Physical, 64),
        (LiteralSource::Replacement, 66),
    ] {
        for allowance in [required - 1, required] {
            let mut catalog = catalog(&[]);
            let mut state = ParserState::new(
                Limits {
                    max_owned_bytes: allowance + 2,
                    ..Limits::default()
                },
                0,
            );
            state.owned_budget().copy("zz", 0).unwrap();
            let result = prepare(&mut state, &mut catalog, "<r a='x'/>", 0, source);
            if allowance < required {
                assert_eq!(
                    result.err().unwrap().kind,
                    DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(Limit::OwnedBytes))
                );
            } else {
                let prepared = result.unwrap();
                assert_eq!(values(&prepared), [("a", "x")]);
                assert_eq!(state.owned_budget().remaining_owned(), 0);
                let owned = source == LiteralSource::Replacement;
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
    }
}

#[test]
fn normalization_capacity_requests_are_cumulative_and_checked_before_growth() {
    // Initial 64, then full 128 and 256 requests, including released buffers.
    // These are capacity budgets, not normalized value-length budgets.
    for (characters, allowance, requested, success) in [
        (1, 1024, 64, true),
        (65, 192, 192, true),
        (129, 448, 448, true),
        (129, 447, 192, false),
    ] {
        let mut catalog = catalog(&[]);
        let mut state = ParserState::new(
            Limits {
                max_owned_bytes: allowance,
                ..Limits::default()
            },
            0,
        );
        let value = "x".repeat(characters);
        let raw = format!("<r a='{value}'/>");
        let result = prepare(&mut state, &mut catalog, &raw, 0, LiteralSource::Physical);
        if success {
            assert_eq!(values(&result.unwrap()), [("a", value.as_str())]);
        } else {
            assert_eq!(
                result.err().unwrap().kind,
                DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(Limit::OwnedBytes))
            );
        }
        assert_eq!(
            state.owned_budget().remaining_owned(),
            allowance - requested
        );
    }
}

#[test]
fn native_value_and_caller_capacity_limits_keep_distinct_causes_and_stickiness() {
    for caller_limit in [31, 64] {
        let mut catalog = DtdCatalog::new(
            CatalogLimits {
                max_value_bytes: 32,
                ..CatalogLimits::default()
            },
            false,
        );
        let mut state = ParserState::new(
            Limits {
                max_owned_bytes: caller_limit,
                ..Limits::default()
            },
            0,
        );
        let raw = format!("<r a='{}'/>", "x".repeat(33));
        let error = prepare(&mut state, &mut catalog, &raw, 0, LiteralSource::Physical)
            .err()
            .unwrap();
        if caller_limit == 31 {
            assert_eq!(
                error.kind,
                DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(Limit::OwnedBytes))
            );
            assert_eq!(state.owned_budget().remaining_owned(), 31);
            assert_eq!(
                catalog.normalize_attribute_value("r", "a", "ok", LiteralSource::Physical),
                Ok(String::from("ok"))
            );
        } else {
            assert_eq!(
                error.kind,
                DtdTagErrorKind::Catalog(CatalogErrorKind::Limit(CatalogLimit::ValueBytes))
            );
            assert_eq!(state.owned_budget().remaining_owned(), 32);
            assert_eq!(
                catalog
                    .normalize_attribute_value("r", "a", "ok", LiteralSource::Physical)
                    .unwrap_err()
                    .kind,
                CatalogErrorKind::Limit(CatalogLimit::ValueBytes)
            );
        }
    }
}

#[test]
fn unknown_explicit_reference_has_real_tag_local_offset() {
    let mut catalog = catalog(&[]);
    let mut state = ParserState::new(Limits::default(), 0);
    let raw = "<r a='before' b='xx&missing;'/>";
    let start = 113;
    let error = prepare(
        &mut state,
        &mut catalog,
        raw,
        start,
        LiteralSource::Physical,
    )
    .err()
    .unwrap();
    assert_eq!(
        error.kind,
        DtdTagErrorKind::Catalog(CatalogErrorKind::EntityDeclared)
    );
    let expected = start + raw.find('&').unwrap();
    assert!(
        matches!(error.location, DtdTagLocation::Input(offset) if offset == expected)
            || matches!(error.location, DtdTagLocation::Value(ValueLocation::Input(offset)) if offset == expected),
        "expected current-token anchor {expected}, got {:?}",
        error.location
    );
}

#[test]
fn replacement_error_retains_entity_local_location_without_tag_rebasing() {
    let mut catalog = catalog(&["<!ENTITY bad 'xx&#60;'>"]);
    let mut state = ParserState::new(Limits::default(), 0);
    let error = prepare(
        &mut state,
        &mut catalog,
        "<r a='ok' b='&bad;'/>",
        113,
        LiteralSource::Physical,
    )
    .err()
    .unwrap();
    assert_eq!(
        error.kind,
        DtdTagErrorKind::Catalog(CatalogErrorKind::LessThanInAttribute)
    );
    assert!(matches!(
        error.location,
        DtdTagLocation::Value(ValueLocation::Replacement { offset: 2, .. })
    ));
}

#[test]
fn raw_lexical_duplicate_precedes_value_and_namespace_processing() {
    let mut state = ParserState::new(Limits::default(), 0);
    let raw = "<r a='&missing;' a='second' xmlns:p=''/>";
    let error = state.collect_raw_tag(raw, 70).err().unwrap();
    assert_eq!(
        error,
        Error::new(
            ErrorKind::DuplicateAttribute,
            70 + raw.find("a='second'").unwrap()
        )
    );
    assert_eq!(
        state.owned_budget().remaining_owned(),
        Limits::default().max_owned_bytes
    );
}

#[test]
fn expanded_duplicate_from_default_is_rejected_before_semantic_publication() {
    let mut catalog = catalog(&["<!ATTLIST r xmlns:q CDATA 'urn:same' q:a CDATA 'default'>"]);
    let mut state = ParserState::new(Limits::default(), 0);
    let prepared = prepare(
        &mut state,
        &mut catalog,
        "<r xmlns:p='urn:same' p:a='specified'/>",
        0,
        LiteralSource::Physical,
    )
    .unwrap();
    let error = state
        .publish_prepared_tag(prepared, &mut |raw, offset, budget| {
            budget.copy(raw, offset)
        })
        .err()
        .unwrap();
    assert_eq!(error.kind, ErrorKind::DuplicateAttribute);
    assert_eq!(state.element_depth(), 0);
    assert!(
        state.bindings.is_empty(),
        "rejected namespaces were published"
    );
    assert!(
        state.namespace_ids.is_empty(),
        "rejected interner entries were published"
    );
    assert!(state.namespace_index.is_empty());
    assert_eq!(state.next_namespace, 0);
}

#[test]
fn invalid_default_namespace_is_rejected_before_any_bindings_are_published() {
    let mut catalog = catalog(&["<!ATTLIST r xmlns:p CDATA 'urn:ok' xmlns:xml CDATA 'urn:wrong'>"]);
    let mut state = ParserState::new(Limits::default(), 0);
    let prepared = prepare(&mut state, &mut catalog, "<r/>", 0, LiteralSource::Physical).unwrap();
    let error = state
        .publish_prepared_tag(prepared, &mut |raw, offset, budget| {
            budget.copy(raw, offset)
        })
        .err()
        .unwrap();
    assert_eq!(error.kind, ErrorKind::Namespace);
    assert!(state.bindings.is_empty());
    assert!(state.namespace_ids.is_empty());
    assert!(state.namespace_index.is_empty());
    assert_eq!(state.next_namespace, 0);
}

#[test]
fn rejected_retention_leaves_existing_scope_and_end_tag_usable() {
    let mut probe_state = ParserState::new(Limits::default(), 0);
    probe_state
        .process_token(
            TokenKind::Start,
            "<r xmlns:p='urn:parent'>",
            0..24,
            |raw, offset, budget| budget.copy(raw, offset),
        )
        .unwrap();
    let probe_tag = prepare(
        &mut probe_state,
        &mut catalog(&[]),
        "<p:c xmlns:p='urn:child' xmlns:q='urn:other'>",
        24,
        LiteralSource::Physical,
    )
    .unwrap();
    let mut retention_calls = 0;
    probe_state
        .publish_prepared_tag(probe_tag, &mut |raw, offset, budget| {
            retention_calls += 1;
            budget.copy(raw, offset)
        })
        .unwrap();
    assert!(retention_calls > 0);

    for failed_call in 0..retention_calls {
        let mut catalog = catalog(&[]);
        let mut state = ParserState::new(Limits::default(), 0);
        state
            .process_token(
                TokenKind::Start,
                "<r xmlns:p='urn:parent'>",
                0..24,
                |raw, offset, budget| budget.copy(raw, offset),
            )
            .unwrap();
        let before = (
            state.stack.len(),
            state.bindings.len(),
            state.namespace_ids.len(),
            state.namespace_index.len(),
            state.next_namespace,
        );
        let prepared = prepare(
            &mut state,
            &mut catalog,
            "<p:c xmlns:p='urn:child' xmlns:q='urn:other'>",
            24,
            LiteralSource::Physical,
        )
        .unwrap();
        let mut calls = 0;
        let result = state.publish_prepared_tag(prepared, &mut |raw, offset, budget| {
            let current = calls;
            calls += 1;
            if current == failed_call {
                Err(Error::new(ErrorKind::Allocation, offset))
            } else {
                budget.copy(raw, offset)
            }
        });
        assert!(
            result.is_err(),
            "fault callback {failed_call} was never reached"
        );
        assert_eq!(
            before,
            (
                state.stack.len(),
                state.bindings.len(),
                state.namespace_ids.len(),
                state.namespace_index.len(),
                state.next_namespace
            ),
            "fault callback {failed_call}"
        );
        let end = state
            .process_token(TokenKind::End, "</r>", 100..104, |raw, offset, budget| {
                budget.copy(raw, offset)
            })
            .unwrap();
        assert!(matches!(end, Some(Event::End(name)) if name.qualified() == "r"));
        assert!(state.bindings.is_empty());
        assert!(state.namespace_index.is_empty());
        assert!(state.namespace_ids.is_empty());
    }
}
