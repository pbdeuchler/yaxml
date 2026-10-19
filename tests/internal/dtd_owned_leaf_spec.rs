//! Literal, ownership and quota oracles for generated body event preparation.
use super::*;
use crate::dtd_catalog::{CatalogLimits, DtdCatalog};

fn physical<'a>(state: &mut ParserState<'a>, raw: &'a str) {
    state
        .process_token(TokenKind::Start, raw, 0..raw.len(), |raw, _, _| {
            Ok(Cow::Borrowed(raw))
        })
        .unwrap();
}

fn body(limits: Limits) -> ParserState<'static> {
    let mut state = ParserState::new(limits, 0);
    physical(&mut state, "<r>");
    state
}

fn owned_value(value: Cow<'static, str>, expected: &str) {
    assert!(matches!(value, Cow::Owned(_)));
    assert_eq!(value, expected);
}

#[test]
fn generated_text_is_body_only_including_empty_and_whitespace_values() {
    for epilog in [false, true] {
        for raw in ["", " \r\n", "text", "é"] {
            let mut state = ParserState::new(Limits::default(), 0);
            if epilog {
                physical(&mut state, "<r/>");
            }
            let events = state.events;
            let owned = state.budget.owned_bytes;
            let error = state
                .process_dtd_leaf_owned(DtdLeafKind::Text, raw, 7..7 + raw.len())
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::TextOutsideRoot, "{raw:?} {epilog}");
            assert_eq!(error.local_offset, 7);
            assert_eq!(state.events, events);
            assert_eq!(state.budget.owned_bytes, owned);
            assert_eq!(state.element_depth(), 0);
        }
    }
}

#[test]
fn generated_leaf_values_own_final_data_and_preserve_literal_replacement_cr() {
    let cases = [
        (
            DtdLeafKind::Text,
            "é\r\n&amp;missing;&#13;",
            "é\r\n&missing;\r",
        ),
        (DtdLeafKind::Cdata, "<![CDATA[é\r\n&amp;]]>", "é\r\n&amp;"),
        (DtdLeafKind::Comment, "<!--é\r\n&amp;-->", "é\r\n&amp;"),
    ];
    for (kind, raw, expected) in cases {
        let event: Event<'static> = {
            let mut state = body(Limits::default());
            let raw = raw.to_owned();
            state
                .process_dtd_leaf_owned(kind, &raw, 11..11 + raw.len())
                .unwrap()
        };
        match event {
            Event::Text(value) | Event::Cdata(value) | Event::Comment(value) => {
                owned_value(value, expected)
            }
            other => panic!("unexpected leaf: {other:?}"),
        }
    }
    let event: Event<'static> = {
        let mut state = body(Limits::default());
        let raw = String::from("<?π \té\r\n?>");
        state
            .process_dtd_leaf_owned(DtdLeafKind::ProcessingInstruction, &raw, 23..23 + raw.len())
            .unwrap()
    };
    let Event::ProcessingInstruction { target, data } = event else {
        panic!("expected PI");
    };
    owned_value(target, "π");
    owned_value(data, "é\r\n");
}

#[test]
fn generated_leaf_copy_ceilings_count_each_retained_destination() {
    for (kind, raw, exact) in [
        (DtdLeafKind::Text, "&amp;", 5),
        (DtdLeafKind::Cdata, "<![CDATA[é\r\n]]>", 4),
        (DtdLeafKind::Comment, "<!--é\r-->", 3),
        (DtdLeafKind::ProcessingInstruction, "<?π \té\r\n?>", 6),
    ] {
        for short in [false, true] {
            let mut state = body(Limits {
                max_owned_bytes: exact - usize::from(short),
                ..Limits::default()
            });
            let events = state.events;
            let result = state.process_dtd_leaf_owned(kind, raw, 19..19 + raw.len());
            if short {
                assert_eq!(
                    result.unwrap_err().kind,
                    ErrorKind::LimitExceeded(Limit::OwnedBytes)
                );
                assert_eq!(state.events, events);
            } else {
                result.unwrap();
                assert_eq!(state.events, events + 1);
                assert_eq!(state.budget.owned_bytes, exact);
            }
            assert_eq!(state.element_depth(), 1);
        }
    }
}

#[test]
fn generated_leaf_errors_keep_local_utf8_coordinates_and_do_not_commit_events() {
    for (kind, raw, expected, offset) in [
        (DtdLeafKind::Text, "é]]>", ErrorKind::Syntax, 42),
        (
            DtdLeafKind::Text,
            "é&unknown;",
            ErrorKind::UnknownEntity,
            42,
        ),
        (DtdLeafKind::Text, "é&#0;", ErrorKind::InvalidReference, 42),
        (DtdLeafKind::Text, "é&amp", ErrorKind::InvalidReference, 42),
        (DtdLeafKind::Comment, "<!--a--b-->", ErrorKind::Syntax, 40),
        (DtdLeafKind::Comment, "<!--a--->", ErrorKind::Syntax, 40),
        (
            DtdLeafKind::ProcessingInstruction,
            "<?XmL?>",
            ErrorKind::InvalidDeclaration,
            40,
        ),
        (
            DtdLeafKind::ProcessingInstruction,
            "<?1bad?>",
            ErrorKind::InvalidName,
            42,
        ),
    ] {
        let mut state = body(Limits::default());
        let events = state.events;
        let error = state
            .process_dtd_leaf_owned(kind, raw, 40..40 + raw.len())
            .unwrap_err();
        assert_eq!(error.kind, expected, "{raw:?}");
        assert_eq!(error.local_offset, offset, "{raw:?}");
        assert_eq!(state.events, events);
        assert_eq!(state.element_depth(), 1);
    }
    let mut state = body(Limits::default());
    assert_eq!(
        state
            .process_dtd_leaf_owned(DtdLeafKind::Text, "é", 40..41)
            .unwrap_err(),
        ReplacementError {
            kind: ErrorKind::InvalidState,
            local_offset: 40
        },
    );
    assert_eq!(state.events, 1);
}

#[test]
fn generated_leaf_parser_ceilings_precede_copying() {
    let mut state = body(Limits {
        max_events: 1,
        max_owned_bytes: 0,
        ..Limits::default()
    });
    let error = state
        .process_dtd_leaf_owned(DtdLeafKind::Text, "x", 9..10)
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::LimitExceeded(Limit::Events));
    assert_eq!(state.budget.owned_bytes, 0);
    assert_eq!(state.events, 1);
    let mut state = body(Limits {
        max_token_bytes: 3,
        max_owned_bytes: 0,
        ..Limits::default()
    });
    let error = state
        .process_dtd_leaf_owned(DtdLeafKind::Text, "long", 9..13)
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::LimitExceeded(Limit::TokenBytes));
    assert_eq!(state.budget.owned_bytes, 0);
    assert_eq!(state.events, 1);
}

#[test]
fn owned_generated_start_copies_inherited_short_source_namespace_at_exact_ceiling() {
    fn generated(
        source: &str,
        ceiling: usize,
    ) -> Result<Event<'static>, crate::dtd_tag::DtdTagError> {
        let mut state = ParserState::new(
            Limits {
                max_owned_bytes: ceiling,
                ..Limits::default()
            },
            0,
        );
        physical(&mut state, source);
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        let raw = String::from("<p:c/>");
        let event = state.process_dtd_start_owned(&raw, 0, &mut catalog);
        assert_eq!(state.element_depth(), 1);
        assert_eq!(state.events, if event.is_ok() { 2 } else { 1 });
        event
    }
    let source = String::from("<p:r xmlns:p='urn'>");
    let error = generated(&source, 5).unwrap_err();
    assert_eq!(
        error.kind,
        crate::dtd_tag::DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(Limit::OwnedBytes))
    );
    let event = generated(&source, 6).unwrap();
    drop(source);
    let Event::Empty(element) = event else {
        panic!("expected empty");
    };
    assert_eq!(element.name.qualified(), "p:c");
    assert_eq!(element.name.namespace(), Some("urn"));
}

#[test]
fn generated_end_copy_failure_after_namespace_copy_keeps_the_exact_scope_live() {
    for short in [false, true] {
        let mut state = ParserState::new(
            Limits {
                max_owned_bytes: 4 - usize::from(short),
                ..Limits::default()
            },
            0,
        );
        physical(&mut state, "<r xmlns='urn'>");
        let event = state.process_dtd_end_owned("</r>", 10);
        if short {
            let error = event.unwrap_err();
            assert_eq!(error.kind, ErrorKind::LimitExceeded(Limit::OwnedBytes));
            assert_eq!(error.local_offset, 12);
            assert_eq!(state.budget.owned_bytes, 3);
            assert_eq!(state.element_depth(), 1);
            assert_eq!(state.events, 1);
            assert!(state.in_content());
            assert_eq!(
                state.resolve_name("r", None, 10).unwrap().namespace(),
                Some("urn")
            );
        } else {
            let Event::End(name) = event.unwrap() else {
                panic!("expected End");
            };
            assert_eq!(name.namespace(), Some("urn"));
            assert_eq!(name.qualified(), "r");
            assert_eq!(state.budget.owned_bytes, 4);
            assert_eq!(state.element_depth(), 0);
            assert_eq!(state.events, 2);
            state.validate_finish(14).unwrap();
        }
    }
}

#[test]
fn generated_end_mismatch_leaves_shadowed_namespace_and_frame_identity_intact() {
    let mut state = ParserState::new(Limits::default(), 0);
    physical(&mut state, "<p:r xmlns:p='outer'>");
    state
        .process_token(
            TokenKind::Start,
            "<p:c xmlns:p='inner'>",
            22..42,
            |raw, _, _| Ok(Cow::Borrowed(raw)),
        )
        .unwrap();
    let error = state.process_dtd_end_owned("</p:r>", 0).unwrap_err();
    assert_eq!(error.kind, ErrorKind::MismatchedEndTag);
    assert_eq!(state.element_depth(), 2);
    assert_eq!(state.events, 2);
    let Event::End(child) = state.process_dtd_end_owned("</p:c>", 0).unwrap() else {
        panic!("expected child End");
    };
    assert_eq!(child.namespace(), Some("inner"));
    let Event::End(parent) = state.process_dtd_end_owned("</p:r>", 0).unwrap() else {
        panic!("expected parent End");
    };
    assert_eq!(parent.namespace(), Some("outer"));
    assert_eq!(state.events, 4);
    state.validate_finish(0).unwrap();
}

#[test]
fn generated_defaults_and_pending_namespaces_survive_all_source_storage() {
    use crate::dtd::{DtdLimits, DtdUnit, LiteralSource};
    use crate::dtd_catalog::DeclarationContext;

    let event: Event<'static> = {
        let mut state = body(Limits::default());
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        let declaration = String::from(
            "<!ATTLIST p:c xmlns:p CDATA 'urn' p:a CDATA 'default' words NMTOKENS '  x   y  '>",
        );
        catalog
            .apply(
                &DtdUnit::parse(&declaration, DtdLimits::default()).unwrap(),
                DeclarationContext {
                    internal_subset: true,
                    literals: LiteralSource::Physical,
                },
            )
            .unwrap();
        drop(declaration);
        let raw = String::from("<p:c p:a='explicit'/>");
        let event = state
            .process_dtd_start_owned(&raw, 0, &mut catalog)
            .unwrap();
        assert_eq!(state.element_depth(), 1);
        assert_eq!(state.events, 2);
        let error = state
            .process_dtd_start_owned("<p:d/>", 0, &mut catalog)
            .unwrap_err();
        assert_eq!(
            error.kind,
            crate::dtd_tag::DtdTagErrorKind::Parser(ErrorKind::Namespace)
        );
        assert_eq!(state.element_depth(), 1);
        assert_eq!(state.events, 2);
        event
    };
    let Event::Empty(element) = event else {
        panic!("expected empty");
    };
    assert_eq!(element.name.qualified(), "p:c");
    assert_eq!(element.name.namespace(), Some("urn"));
    let attributes = element
        .attributes
        .iter()
        .map(|attribute| {
            (
                attribute.name.qualified(),
                attribute.name.namespace(),
                attribute.value.as_ref(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        attributes,
        [
            ("p:a", Some("urn"), "explicit"),
            ("xmlns:p", Some("http://www.w3.org/2000/xmlns/"), "urn"),
            ("words", None, "x y"),
        ]
    );
}

fn namespace_defaults() -> DtdCatalog {
    use crate::dtd::{DtdLimits, DtdUnit, LiteralSource};
    use crate::dtd_catalog::DeclarationContext;
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    catalog
        .apply(
            &DtdUnit::parse(
                "<!ATTLIST p:c xmlns:p CDATA 'urn' a CDATA 'v'>",
                DtdLimits::default(),
            )
            .unwrap(),
            DeclarationContext {
                internal_subset: true,
                literals: LiteralSource::Physical,
            },
        )
        .unwrap();
    catalog
}

#[test]
fn nonempty_generated_start_retains_default_scope_after_raw_and_catalog_drop() {
    let (parent, child, end): (Event<'static>, Event<'static>, Event<'static>) = {
        let mut state = body(Limits::default());
        let raw = String::from("<p:c>");
        let mut catalog = namespace_defaults();
        let parent = state
            .process_dtd_start_owned(&raw, 0, &mut catalog)
            .unwrap();
        drop(raw);
        drop(catalog);
        assert_eq!(state.element_depth(), 2);
        assert_eq!(state.events, 2);
        let child = state
            .process_dtd_start_owned(
                "<p:x/>",
                0,
                &mut DtdCatalog::new(CatalogLimits::default(), false),
            )
            .unwrap();
        assert_eq!(state.element_depth(), 2);
        let end = state.process_dtd_end_owned("</p:c>", 0).unwrap();
        assert_eq!(state.element_depth(), 1);
        assert_eq!(
            state
                .resolve_name("p:x", std::num::NonZeroUsize::new(1), 0)
                .err()
                .unwrap()
                .kind,
            ErrorKind::Namespace
        );
        state.process_dtd_end_owned("</r>", 0).unwrap();
        assert_eq!(state.events, 5);
        state.validate_finish(0).unwrap();
        (parent, child, end)
    };
    let Event::Start(parent) = parent else {
        panic!("expected nonempty Start");
    };
    assert_eq!(parent.name.qualified(), "p:c");
    assert_eq!(parent.name.namespace(), Some("urn"));
    assert_eq!(parent.attributes.len(), 2);
    assert_eq!(parent.attributes[0].name.qualified(), "xmlns:p");
    assert_eq!(parent.attributes[0].value, "urn");
    assert_eq!(parent.attributes[1].name.qualified(), "a");
    assert_eq!(parent.attributes[1].value, "v");
    assert!(
        matches!(child, Event::Empty(element) if element.name.qualified() == "p:x" && element.name.namespace() == Some("urn"))
    );
    assert!(
        matches!(end, Event::End(name) if name.qualified() == "p:c" && name.namespace() == Some("urn"))
    );
}

#[test]
fn nonempty_generated_start_frame_copy_ceiling_keeps_pending_scope_unpublished() {
    // Event names/values, interner/scope strings and the frame QName total 29.
    // The final three bytes retain p:c in the open-element frame.
    for short in [false, true] {
        let mut state = body(Limits {
            max_owned_bytes: 29 - usize::from(short),
            ..Limits::default()
        });
        let mut catalog = namespace_defaults();
        let result = state.process_dtd_start_owned("<p:c>", 10, &mut catalog);
        if short {
            let error = result.unwrap_err();
            assert_eq!(
                error.kind,
                crate::dtd_tag::DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(
                    Limit::OwnedBytes
                ))
            );
            assert_eq!(error.location, crate::dtd_tag::DtdTagLocation::Input(11));
            assert_eq!(state.budget.owned_bytes, 26);
            assert_eq!(state.element_depth(), 1);
            assert_eq!(state.events, 1);
            assert_eq!(
                state
                    .resolve_name("p:x", std::num::NonZeroUsize::new(1), 0)
                    .err()
                    .unwrap()
                    .kind,
                ErrorKind::Namespace
            );
        } else {
            assert!(matches!(result.unwrap(), Event::Start(_)));
            assert_eq!(state.budget.owned_bytes, 29);
            assert_eq!(state.element_depth(), 2);
            assert_eq!(state.events, 2);
        }
    }
}
