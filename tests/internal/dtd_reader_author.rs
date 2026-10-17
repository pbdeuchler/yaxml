use super::*;
use crate::dtd_controller::ControllerLimits;
use crate::dtd_reference::ReferenceLimits;
use crate::tokenizer::local::LocalLimits;
use crate::tokenizer::text::{TextLimits, TextScratchLimits};

fn configured() -> SessionLimits {
    SessionLimits {
        declarations: ControllerLimits::default(),
        text: TextLimits {
            max_work: 64 * 1024 * 1024,
            reference: ReferenceLimits {
                max_bytes: 1024,
                max_scratch_bytes: 1024,
                max_work: 64 * 1024 * 1024,
            },
            scratch: TextScratchLimits {
                input_token_bytes: Limits::default().max_token_bytes,
                replacement_token_bytes: 4096,
                max_bytes: Limits::default().max_token_bytes + 4096 + 1024,
            },
        },
        local: LocalLimits {
            max_token_bytes: 4096,
            max_scratch_bytes: 4096,
            max_work: 64 * 1024 * 1024,
        },
    }
}
fn label(event: &ReaderEvent<'_>) -> String {
    match event {
        ReaderEvent::Xml(event) => match event {
            Event::Start(e) => format!("+{}", e.name.qualified()),
            Event::Empty(e) => format!("={}", e.name.qualified()),
            Event::End(n) => format!("-{}", n.qualified()),
            Event::Text(t) => format!("T{t}"),
            _ => format!("{event:?}"),
        },
        ReaderEvent::Dtd(TakenDtdEvent::Begin(_)) => "begin".into(),
        ReaderEvent::Dtd(TakenDtdEvent::Item { .. }) => "item".into(),
        ReaderEvent::Dtd(TakenDtdEvent::End) => "end".into(),
    }
}

#[test]
fn stable_values_survive_advance_and_reader_drop_with_literal_high_water_positions() {
    let input = String::from("<!DOCTYPE r [<!ENTITY e '<n/>x'><!ENTITY z ''>]><r>a&e;b&z;c</r>");
    let mut reader = DtdReader::new(input.as_bytes(), configured()).unwrap();
    let mut events = Vec::new();
    while let Some((event, origin)) = reader.read_event().unwrap() {
        assert_eq!(reader.event_origin(), Some(origin));
        assert_eq!(reader.event_range(), origin.input_range());
        events.push((event, origin, reader.position()));
    }
    assert_eq!(reader.position(), input.len());
    assert!(reader.read_event().unwrap().is_none());
    drop(reader);
    assert_eq!(
        events.iter().map(|e| label(&e.0)).collect::<Vec<_>>(),
        [
            "begin", "item", "item", "end", "+r", "Ta", "=n", "Tx", "Tb", "Tc", "-r"
        ]
    );
    assert_eq!(
        events.iter().map(|e| e.2).collect::<Vec<_>>(),
        [13, 32, 46, 48, 51, 55, 55, 55, 59, 61, 64]
    );
    let ReaderEvent::Dtd(TakenDtdEvent::Item { unit, .. }) = &events[1].0 else {
        panic!()
    };
    assert_eq!(unit.raw().as_ptr(), input[13..].as_ptr());
    let ReaderEvent::Xml(Event::Text(Cow::Borrowed(text))) = &events[5].0 else {
        panic!()
    };
    assert_eq!(text.as_ptr(), input[51..].as_ptr());
    assert!(matches!(&events[7].0, ReaderEvent::Xml(Event::Text(Cow::Owned(text))) if text == "x"));
}

#[test]
fn no_subset_end_is_returned_before_body_and_failure_is_sticky() {
    let input = b"<!DOCTYPE r><r/>";
    let mut reader = DtdReader::new(input, configured()).unwrap();
    let first = reader.read_event().unwrap().unwrap();
    assert_eq!(reader.position(), 12);
    assert_eq!(first.1.input_range(), Some(0..11));
    let second = reader.read_event().unwrap().unwrap();
    assert_eq!(reader.position(), 12);
    assert_eq!(second.1.input_range(), Some(11..12));
    let third = reader.read_event().unwrap().unwrap();
    assert_eq!(reader.position(), 16);
    assert_eq!(label(&third.0), "=r");
    assert_eq!(label(&first.0), "begin");
    assert_eq!(label(&second.0), "end");
    assert!(reader.read_event().unwrap().is_none());
    assert_eq!(reader.into_inner(), input);

    let mut reader = DtdReader::new(b"<!DOCTYPE r><r>pre&missing;</r>", configured()).unwrap();
    for expected in ["begin", "end", "+r", "Tpre"] {
        assert_eq!(label(&reader.read_event().unwrap().unwrap().0), expected);
    }
    let Phase::Content(controller) = &reader.session[0] else {
        panic!()
    };
    assert_eq!(controller.frame_counters().reference_attempts, 0);
    let origin = reader.event_origin();
    let error = reader.read_event().unwrap_err();
    assert!(matches!(error, SessionError::Content(_)));
    let Phase::Content(controller) = &reader.session[0] else {
        panic!()
    };
    assert_eq!(controller.frame_counters().reference_attempts, 1);
    let position = reader.position();
    assert_eq!(reader.read_event().unwrap_err(), error);
    assert_eq!(reader.event_origin(), origin);
    assert_eq!(reader.position(), position);
}

#[test]
fn constructors_and_ordinary_error_positions_match_the_existing_reader() {
    for input in [
        b"\xff\xfe<\0".as_slice(),
        b"\0<\0?",
        b"<\0?\0",
        b"<r/>\0",
        b"<r/>\xff",
    ] {
        let expected = Reader::new(input).err().unwrap();
        assert!(
            matches!(DtdReader::new(input, configured()), Err(SessionError::Xml(e)) if e == expected)
        );
    }
    for input in [
        "<r></s>",
        "<r>&bad;</r>",
        "<?xml?><r/>",
        "<r><!--a--b--></r>",
    ] {
        let mut expected = Reader::new(input.as_bytes()).unwrap();
        let mut reader = DtdReader::new(input.as_bytes(), configured()).unwrap();
        loop {
            match expected.read_event() {
                Ok(Some(event)) => {
                    let (ReaderEvent::Xml(actual), origin) = reader.read_event().unwrap().unwrap()
                    else {
                        panic!()
                    };
                    assert_eq!(actual, event);
                    assert_eq!(Some(origin), expected.event_origin());
                    assert_eq!(reader.position(), expected.position());
                }
                Err(error) => {
                    assert_eq!(reader.read_event().unwrap_err(), SessionError::Xml(error));
                    assert_eq!(reader.position(), expected.position());
                    break;
                }
                Ok(None) => panic!("invalid fixture accepted"),
            }
        }
    }
}

#[test]
fn physical_quota_can_stop_within_a_utf8_scalar_without_losing_failure_progress() {
    let mut options = configured();
    options.declarations.max_physical_bytes = 16;
    let mut reader = DtdReader::new("<!DOCTYPE r><r>π</r>".as_bytes(), options).unwrap();
    for expected in ["begin", "end", "+r"] {
        assert_eq!(label(&reader.read_event().unwrap().unwrap().0), expected);
    }
    let last = reader.event_origin();
    let error = reader.read_event().unwrap_err();
    assert!(matches!(
        error,
        SessionError::Content(crate::dtd_controller::content::ContentError {
            kind: crate::dtd_controller::content::ContentErrorKind::PhysicalBytes,
            physical_offset: 16,
            ..
        })
    ));
    assert_eq!(reader.position(), 16);
    assert_eq!(reader.event_origin(), last);
    assert_eq!(reader.read_event().unwrap_err(), error);
}

#[test]
fn shared_event_and_copy_counters_continue_through_taken_dtd_end() {
    let input = b"<!DOCTYPE r [<!ENTITY unused 'v'>]><r/>";
    let mut options = configured();
    options.declarations.max_owned_bytes = 0;
    let mut reader = DtdReader::with_limits(
        input,
        Limits {
            max_owned_bytes: 0,
            ..Limits::default()
        },
        options,
    )
    .unwrap();
    for expected in ["begin", "item", "end"] {
        assert_eq!(label(&reader.read_event().unwrap().unwrap().0), expected);
        let Phase::Declarations { controller, .. } = &reader.session[0] else {
            panic!()
        };
        assert_eq!(controller.counters().owned_bytes, 0);
    }
    assert_eq!(label(&reader.read_event().unwrap().unwrap().0), "=r");
    assert!(reader.read_event().unwrap().is_none());

    let mut reader = DtdReader::with_limits(
        b"<!DOCTYPE r><r/>",
        Limits {
            max_events: 2,
            ..Limits::default()
        },
        configured(),
    )
    .unwrap();
    assert_eq!(label(&reader.read_event().unwrap().unwrap().0), "begin");
    assert_eq!(label(&reader.read_event().unwrap().unwrap().0), "end");
    let origin = reader.event_origin();
    assert!(matches!(
        reader.read_event(),
        Err(SessionError::Content(
            crate::dtd_controller::content::ContentError {
                kind: crate::dtd_controller::content::ContentErrorKind::Tag(
                    crate::dtd_tag::DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(
                        crate::Limit::Events
                    ))
                ),
                physical_offset: 12,
                ..
            }
        ))
    ));
    assert_eq!(reader.position(), 16);
    assert_eq!(reader.event_origin(), origin);
}

fn apply_declaration(catalog: &mut crate::dtd_catalog::DtdCatalog, raw: &str) {
    let unit = crate::dtd::DtdUnit::parse(raw, crate::dtd::DtdLimits::default()).unwrap();
    catalog
        .apply(
            &unit,
            crate::dtd_catalog::DeclarationContext {
                internal_subset: true,
                literals: crate::dtd::LiteralSource::Physical,
            },
        )
        .unwrap();
}

#[test]
fn stable_explicit_values_borrow_input_while_defaults_and_generated_values_own() {
    let input = String::from(
        "<!DOCTYPE r [<!ENTITY e \"<n a='v'/>\"><!ATTLIST r b CDATA 'd'>]><r a='v'>&e;</r>",
    );
    let mut reader = DtdReader::new(input.as_bytes(), configured()).unwrap();
    let mut held = Vec::new();
    while let Some((event, _)) = reader.read_event().unwrap() {
        held.push(event);
    }
    drop(reader);
    let root = held
        .iter()
        .find_map(|e| match e {
            ReaderEvent::Xml(Event::Start(e)) => Some(e),
            _ => None,
        })
        .unwrap();
    let specified = root
        .attributes
        .iter()
        .find(|a| a.name.qualified() == "a")
        .unwrap();
    let Cow::Borrowed(value) = specified.value else {
        panic!("physical unchanged value was copied")
    };
    assert_eq!(
        value.as_ptr(),
        input[input.rfind("a='v'").unwrap() + 3..].as_ptr()
    );
    assert!(
        matches!(&root.attributes.iter().find(|a| a.name.qualified() == "b").unwrap().value, Cow::Owned(value) if value == "d")
    );
    let generated = held
        .iter()
        .find_map(|e| match e {
            ReaderEvent::Xml(Event::Empty(e)) => Some(e),
            _ => None,
        })
        .unwrap();
    assert!(matches!(&generated.attributes[0].value, Cow::Owned(value) if value == "v"));

    let mut reader = DtdReader::with_limits(
        b"<!DOCTYPE r><r a='v'/>",
        Limits {
            max_owned_bytes: 0,
            ..Limits::default()
        },
        configured(),
    )
    .unwrap();
    assert!(reader.read_event().unwrap().is_some());
    assert!(reader.read_event().unwrap().is_some());
    assert!(
        matches!(reader.read_event().unwrap().unwrap().0, ReaderEvent::Xml(Event::Empty(e)) if matches!(e.attributes[0].value, Cow::Borrowed("v")))
    );
    assert!(reader.read_event().unwrap().is_none());
}

#[test]
fn catalog_owned_policy_keeps_its_literal_requests_and_counters() {
    use crate::dtd_catalog::{CatalogLimits, DtdCatalog, LiteralSource, NormalizedValue};
    for (raw, expected, work, expanded, references, requested) in [
        ("", "", 0, 0, 0, 0),
        ("v", "v", 1, 1, 0, 64),
        ("π", "π", 2, 2, 0, 64),
        ("a\tb", "a b", 3, 3, 0, 64),
        ("&amp;", "&", 5, 1, 1, 64),
    ] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        let mut bytes = 999;
        assert_eq!(
            catalog
                .normalize_attribute_value_bounded(
                    "r",
                    "a",
                    raw,
                    LiteralSource::Physical,
                    64,
                    &mut bytes
                )
                .unwrap(),
            NormalizedValue::Value(expected.into())
        );
        assert_eq!(bytes, requested);
        assert_eq!(catalog.usage().work, work);
        assert_eq!(catalog.usage().expanded_bytes, expanded);
        assert_eq!(catalog.usage().references, references);
    }
}

#[test]
fn lazy_catalog_proves_identity_in_one_pass_including_declared_types_and_references() {
    use crate::dtd_catalog::{CatalogLimits, DtdCatalog, NormalizedAttribute};
    for raw in [
        "",
        "v",
        "π",
        "a b",
        "ninety byte unchanged value that crosses the old sixty four byte initial capacity floor with zero copies",
    ] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        let mut requested = 999;
        let value = catalog
            .normalize_attribute_borrowed_bounded("r", "a", raw, 0, &mut requested)
            .unwrap();
        let NormalizedAttribute::Value(Cow::Borrowed(value)) = value else {
            panic!("unchanged input copied")
        };
        assert_eq!(value.as_ptr(), raw.as_ptr());
        assert_eq!(requested, 0);
        assert_eq!(catalog.usage().work, raw.len() * 2);
        assert_eq!(catalog.usage().expanded_bytes, raw.len());
    }
    for (raw, expected, borrowed) in [
        ("a b", "a b", true),
        (" a b", "a b", false),
        ("a b ", "a b", false),
        ("a  b", "a b", false),
        ("  ", "", false),
    ] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        apply_declaration(&mut catalog, "<!ATTLIST r a NMTOKENS #IMPLIED>");
        let mut requested = 0;
        let NormalizedAttribute::Value(value) = catalog
            .normalize_attribute_borrowed_bounded("r", "a", raw, 64, &mut requested)
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(value, expected);
        assert_eq!(matches!(value, Cow::Borrowed(_)), borrowed);
        assert_eq!(
            requested,
            if borrowed || expected.is_empty() {
                0
            } else {
                64
            }
        );
    }
    for (raw, expected) in [
        ("&#65;", "A"),
        ("AT&amp;T;", "AT&T;"),
        ("v\t", "v "),
        ("π\r\nx", "π x"),
    ] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        let mut requested = 0;
        assert_eq!(
            catalog
                .normalize_attribute_borrowed_bounded("r", "a", raw, 64, &mut requested)
                .unwrap(),
            NormalizedAttribute::Value(Cow::Owned(expected.into()))
        );
        assert_eq!(requested, 64);
    }
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    apply_declaration(&mut catalog, "<!ENTITY e 'AT&amp;T;'>");
    let mut requested = 0;
    assert_eq!(
        catalog
            .normalize_attribute_borrowed_bounded("r", "a", "&e;", 64, &mut requested)
            .unwrap(),
        NormalizedAttribute::Value(Cow::Owned("AT&T;".into()))
    );
}

#[test]
fn lazy_catalog_native_and_caller_quota_edges_are_checked_before_publication() {
    use crate::dtd_catalog::{
        CatalogErrorKind as K, CatalogLimit as L, CatalogLimits, DtdCatalog, NormalizedAttribute,
        ValueLocation,
    };
    for (raw, limits, caller, expected, work) in [
        (
            "v",
            CatalogLimits {
                max_value_bytes: 0,
                ..CatalogLimits::default()
            },
            0,
            Some(K::Limit(L::ValueBytes)),
            1,
        ),
        (
            "v",
            CatalogLimits {
                max_value_bytes: 1,
                max_work: 2,
                max_expanded_bytes: 1,
                ..CatalogLimits::default()
            },
            0,
            None,
            2,
        ),
        (
            "v",
            CatalogLimits {
                max_work: 1,
                ..CatalogLimits::default()
            },
            0,
            Some(K::Limit(L::Work)),
            1,
        ),
        (
            "v",
            CatalogLimits {
                max_expanded_bytes: 0,
                ..CatalogLimits::default()
            },
            0,
            Some(K::Limit(L::ExpandedBytes)),
            1,
        ),
        (
            "v\t",
            CatalogLimits {
                max_work: 4,
                ..CatalogLimits::default()
            },
            64,
            Some(K::Limit(L::Work)),
            4,
        ),
        (
            "v\t",
            CatalogLimits {
                max_work: 5,
                max_value_bytes: 2,
                max_expanded_bytes: 2,
                ..CatalogLimits::default()
            },
            2,
            None,
            5,
        ),
        (
            "v\t",
            CatalogLimits {
                max_value_bytes: 2,
                ..CatalogLimits::default()
            },
            1,
            Some(K::CallerValueBytes),
            5,
        ),
        (
            "v\t",
            CatalogLimits::default(),
            63,
            Some(K::CallerValueBytes),
            5,
        ),
        ("v\t", CatalogLimits::default(), 64, None, 5),
    ] {
        let mut catalog = DtdCatalog::new(limits, false);
        let mut requested = 0;
        let result =
            catalog.normalize_attribute_borrowed_bounded("r", "a", raw, caller, &mut requested);
        if let Some(kind) = expected {
            let error = result.unwrap_err();
            assert_eq!(error.kind, kind);
            assert_eq!(error.location, ValueLocation::Input(0));
            if kind != K::CallerValueBytes {
                assert_eq!(
                    catalog
                        .normalize_attribute_borrowed_bounded("r", "a", "ok", 64, &mut requested)
                        .unwrap_err(),
                    error
                );
            }
        } else {
            let NormalizedAttribute::Value(value) = result.unwrap() else {
                panic!()
            };
            assert_eq!(value, if raw == "v" { "v" } else { "v " });
            assert_eq!(requested, if raw == "v" { 0 } else { caller.min(64) });
        }
        assert_eq!(catalog.usage().work, work);
    }
}

#[test]
fn lazy_catalog_unavailable_and_error_forms_never_skip_reference_validation() {
    use crate::dtd_catalog::{
        CatalogErrorKind, CatalogLimits, Dependency, DependencyKind, DtdCatalog,
        NormalizedAttribute, ValueLocation,
    };
    for (raw, kind, offset) in [
        ("v&missing;", CatalogErrorKind::EntityDeclared, 1),
        ("v<", CatalogErrorKind::LessThanInAttribute, 1),
        ("&#0;", CatalogErrorKind::InvalidReference, 0),
        ("\0", CatalogErrorKind::InvalidCharacter, 0),
        ("v&bad", CatalogErrorKind::InvalidReference, 1),
    ] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        let mut requested = 0;
        let error = catalog
            .normalize_attribute_borrowed_bounded("r", "a", raw, 0, &mut requested)
            .unwrap_err();
        assert_eq!(error.kind, kind);
        assert_eq!(error.location, ValueLocation::Input(offset));
        assert_eq!(requested, 0);
    }
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    catalog.note_unread_parameter();
    let mut requested = 999;
    assert_eq!(
        catalog
            .normalize_attribute_borrowed_bounded("r", "a", "v&missing;", 0, &mut requested)
            .unwrap(),
        NormalizedAttribute::Unavailable(Dependency {
            kind: DependencyKind::UnreadEntity,
            location: ValueLocation::Input(1)
        })
    );
    assert_eq!(requested, 0);
}

#[test]
fn normalized_identity_still_expands_and_charges_each_reference() {
    use crate::dtd_catalog::{
        CatalogErrorKind, CatalogLimit, CatalogLimits, DtdCatalog, EntityLookup,
        NormalizedAttribute, ValueLocation,
    };
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    // Construction leaves "&#38;e;"; inclusion produces literal "&e;", without rescan.
    apply_declaration(&mut catalog, "<!ENTITY e '&#38;#38;e;'>");
    let before = catalog.usage();
    let raw = String::from("&e;");
    let mut requested = 999;
    let NormalizedAttribute::Value(Cow::Borrowed(value)) = catalog
        .normalize_attribute_borrowed_bounded("r", "a", &raw, 0, &mut requested)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(value.as_ptr(), raw.as_ptr());
    assert_eq!(catalog.usage().references - before.references, 2);
    assert_eq!(requested, 0);

    let mut catalog = DtdCatalog::new(
        CatalogLimits {
            max_references: 2,
            ..CatalogLimits::default()
        },
        false,
    );
    apply_declaration(&mut catalog, "<!ENTITY e '&#38;#38;e;'>");
    let EntityLookup::Internal { id, .. } = catalog.lookup_general("e") else {
        panic!()
    };
    let error = catalog
        .normalize_attribute_borrowed_bounded("r", "a", "&e;", 0, &mut requested)
        .unwrap_err();
    assert_eq!(
        error.kind,
        CatalogErrorKind::Limit(CatalogLimit::References)
    );
    assert_eq!(
        error.location,
        ValueLocation::Replacement {
            entity: id,
            offset: 0
        }
    );
    assert_eq!(requested, 0);
}

#[test]
fn changed_value_owned_quota_failure_keeps_the_prior_reader_origin_and_exact_anchor() {
    for owned in [63, 64] {
        let mut reader = DtdReader::with_limits(
            b"<!DOCTYPE r><r a='v\t'/>",
            Limits {
                max_owned_bytes: owned,
                ..Limits::default()
            },
            configured(),
        )
        .unwrap();
        assert_eq!(label(&reader.read_event().unwrap().unwrap().0), "begin");
        assert_eq!(label(&reader.read_event().unwrap().unwrap().0), "end");
        let last = reader.event_origin();
        if owned == 63 {
            let error = reader.read_event().unwrap_err();
            assert!(matches!(
                error,
                SessionError::Content(crate::dtd_controller::content::ContentError {
                    kind: crate::dtd_controller::content::ContentErrorKind::Tag(
                        crate::dtd_tag::DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(
                            crate::Limit::OwnedBytes
                        ))
                    ),
                    physical_offset: 18,
                    ..
                })
            ));
            assert_eq!(reader.position(), 23);
            assert_eq!(reader.event_origin(), last);
            assert_eq!(reader.read_event().unwrap_err(), error);
        } else {
            let ReaderEvent::Xml(Event::Empty(element)) = reader.read_event().unwrap().unwrap().0
            else {
                panic!()
            };
            assert!(matches!(&element.attributes[0].value, Cow::Owned(value) if value == "v "));
            assert!(reader.read_event().unwrap().is_none());
        }
    }
}
