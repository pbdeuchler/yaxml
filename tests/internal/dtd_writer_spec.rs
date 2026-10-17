//! Independent literal oracles: /private/tmp/yaxml-dtd-writer-spec-catalog.md.
use super::*;
use crate::dtd::{DtdErrorKind, DtdLimit, DtdLimits};
use crate::dtd_catalog::{
    CatalogErrorKind, CatalogLimit, DefaultValue, DependencyKind, EntityLookup, ValueLocation,
};
use crate::dtd_writer::{DtdWriteLimit, DtdWriteLimits};
use crate::{Attribute, Declaration};
use std::borrow::Cow;
use std::panic::{AssertUnwindSafe, catch_unwind};

fn unit(raw: &'static str) -> DtdUnit<'static> {
    DtdUnit::parse(raw, DtdLimits::default()).unwrap()
}
fn begin(raw: &'static str) -> TakenDtdEvent<'static> {
    TakenDtdEvent::Begin(unit(raw))
}
fn item(raw: &'static str, literals: LiteralSource) -> TakenDtdEvent<'static> {
    TakenDtdEvent::Item {
        unit: unit(raw),
        literals,
    }
}
fn nm(q: &'static str, ns: Option<&'static str>) -> Name<'static> {
    Name::new(q, ns.map(Cow::Borrowed)).unwrap()
}
fn attr(q: &'static str, ns: Option<&'static str>, value: &'static str) -> Attribute<'static> {
    Attribute {
        name: nm(q, ns),
        value: Cow::Borrowed(value),
    }
}
fn el(
    q: &'static str,
    ns: Option<&'static str>,
    attributes: Vec<Attribute<'static>>,
) -> Element<'static> {
    Element {
        name: nm(q, ns),
        attributes,
    }
}
fn empty() -> Event<'static> {
    Event::Empty(el("r", None, vec![]))
}
fn subset<W: Write>(w: &mut Writer<W>, items: &[&'static str]) {
    w.write_dtd_event(&begin("<!DOCTYPE r [")).unwrap();
    for raw in items {
        w.write_dtd_event(&item(raw, LiteralSource::Physical))
            .unwrap();
    }
    w.write_dtd_event(&TakenDtdEvent::End).unwrap();
}
fn xml(error: WriteError) -> Error {
    match error {
        WriteError::Xml(e) => e,
        e => panic!("expected XML error: {e:?}"),
    }
}
fn dtd(error: DtdEventWriteError) -> DtdWriteError {
    match error {
        DtdEventWriteError::Dtd(e) => e,
        e => panic!("expected DTD error: {e:?}"),
    }
}
fn terminal<W: Write>(w: &mut Writer<W>) {
    let position = w.position();
    assert_eq!(
        xml(w.write_event(&empty()).unwrap_err()),
        Error::new(ErrorKind::InvalidState, position)
    );
    assert_eq!(
        xml(w.finish().unwrap_err()),
        Error::new(ErrorKind::InvalidState, position)
    );
    assert!(
        matches!(w.write_dtd_event(&TakenDtdEvent::End), Err(DtdEventWriteError::Writer(WriteError::Xml(Error { kind: ErrorKind::InvalidState, offset }))) if offset == position)
    );
}

#[test]
fn delimiters_and_six_item_families_have_literal_ordered_bytes() {
    for (header, prefix, ending) in [
        ("<!DOCTYPE r>", "<!DOCTYPE r", ">"),
        (
            "<!DOCTYPE r SYSTEM 'urn:never-opened'>",
            "<!DOCTYPE r SYSTEM 'urn:never-opened'",
            ">",
        ),
        ("<!DOCTYPE r [", "<!DOCTYPE r [", "]>"),
    ] {
        let mut w = Writer::new(Vec::new());
        w.write_dtd_event(&begin(header)).unwrap();
        assert_eq!(w.sink, prefix.as_bytes());
        w.write_dtd_event(&TakenDtdEvent::End).unwrap();
        w.write_event(&empty()).unwrap();
        w.finish().unwrap();
        assert_eq!(w.position(), prefix.len() + ending.len() + 4);
        assert_eq!(w.into_inner(), format!("{prefix}{ending}<r/>").as_bytes());
    }
    let declarations = [
        "<!ELEMENT r ANY>",
        "<!ATTLIST r a CDATA #IMPLIED>",
        "<!ENTITY e 'v'>",
        "<!NOTATION n SYSTEM 'urn:n'>",
        "<!--note-->",
        "<?work data?>",
    ];
    let mut w = Writer::new(Vec::new());
    subset(&mut w, &declarations);
    w.write_event(&empty()).unwrap();
    w.finish().unwrap();
    assert_eq!(w.into_inner(), b"<!DOCTYPE r [<!ELEMENT r ANY><!ATTLIST r a CDATA #IMPLIED><!ENTITY e 'v'><!NOTATION n SYSTEM 'urn:n'><!--note--><?work data?>]><r/>");
}

#[test]
fn literal_phase_pairs_preserve_reference_spelling_and_replacement_cr_meaning() {
    for (raw, physical, replacement) in [
        (
            "<!ENTITY e 'a\rb&amp;&#13;'>",
            "<!ENTITY e 'a\rb&amp;&#13;'>",
            "<!ENTITY e 'a&#13;b&amp;&#13;'>",
        ),
        (
            "<!ATTLIST r a CDATA 'x\ry'>",
            "<!ATTLIST r a CDATA 'x\ry'>",
            "<!ATTLIST r a CDATA 'x y'>",
        ),
        ("<!--a\rb-->", "<!--a\rb-->", "<!--a\rb-->"),
        ("<?p a\rb?>", "<?p a\rb?>", "<?p a\rb?>"),
    ] {
        for (source, expected) in [
            (LiteralSource::Physical, physical),
            (LiteralSource::Replacement, replacement),
        ] {
            let mut w = Writer::new(Vec::new());
            w.write_dtd_event(&begin("<!DOCTYPE r [")).unwrap();
            w.write_dtd_event(&item(raw, source)).unwrap();
            assert_eq!(
                &w.sink[b"<!DOCTYPE r [".len()..],
                expected.as_bytes(),
                "{raw:?} {source:?}"
            );
            w.write_dtd_event(&TakenDtdEvent::End).unwrap();
            w.write_event(&empty()).unwrap();
            w.finish().unwrap();
        }
    }
}

#[test]
fn identifier_rejection_keeps_raw_local_and_encoded_output_coordinates_separate() {
    for (raw, offset) in [
        ("<!ENTITY e SYSTEM 'ab\rcd'>", 21),
        ("<!NOTATION n PUBLIC 'a\rb'>", 22),
    ] {
        let mut w = Writer::new(Vec::new());
        w.write_dtd_event(&begin("<!DOCTYPE r [")).unwrap();
        let position = w.position();
        let before = w.sink.clone();
        let error = dtd(w
            .write_dtd_event(&item(raw, LiteralSource::Replacement))
            .unwrap_err());
        assert_eq!(error.kind, DtdWriteErrorKind::UnrepresentableIdentifier);
        assert_eq!(error.unit_offset, Some(offset));
        assert_eq!(error.output_offset, position + offset);
        assert_eq!(error.value_location, None);
        assert_eq!(w.position(), position);
        assert_eq!(w.sink, before);
        terminal(&mut w);
    }
}

#[test]
fn invalid_sequence_family_and_placement_fail_before_event_io() {
    for case in 0..9 {
        let mut w = Writer::new(Vec::new());
        match case {
            2 | 3 | 6 | 8 => w.write_dtd_event(&begin("<!DOCTYPE r [")).unwrap(),
            4 => w.write_dtd_event(&begin("<!DOCTYPE r>")).unwrap(),
            5 => w.write_event(&empty()).unwrap(),
            _ => {}
        }
        if case == 3 {
            w.write_dtd_event(&TakenDtdEvent::End).unwrap();
        }
        let position = w.position();
        let before = w.sink.clone();
        let result = match case {
            0 => w.write_dtd_event(&TakenDtdEvent::End),
            1 | 4 => w.write_dtd_event(&item("<!--x-->", LiteralSource::Physical)),
            2 | 5 => w.write_dtd_event(&begin("<!DOCTYPE r>")),
            3 => w.write_dtd_event(&TakenDtdEvent::End),
            6 => w.write_dtd_event(&TakenDtdEvent::Item {
                unit: unit("<!DOCTYPE r>"),
                literals: LiteralSource::Physical,
            }),
            7 => w.write_dtd_event(&TakenDtdEvent::Begin(unit("<!--x-->"))),
            8 => w
                .write_event(&Event::Comment(Cow::Borrowed("x")))
                .map_err(DtdEventWriteError::Writer),
            _ => unreachable!(),
        };
        let error = result.unwrap_err();
        match error {
            DtdEventWriteError::Dtd(e) => {
                assert_eq!(e.output_offset, position);
                assert_eq!(
                    e.kind,
                    if matches!(case, 6 | 7) {
                        DtdWriteErrorKind::InvalidUnitFamily
                    } else {
                        DtdWriteErrorKind::InvalidSequence
                    }
                );
            }
            DtdEventWriteError::Writer(e) => {
                assert_eq!(xml(e), Error::new(ErrorKind::Syntax, position))
            }
        }
        assert_eq!(w.position(), position);
        assert_eq!(w.sink, before);
        terminal(&mut w);
    }
    let mut w = Writer::new(Vec::new());
    w.write_dtd_event(&begin("<!DOCTYPE r [")).unwrap();
    assert_eq!(
        xml(w.finish().unwrap_err()),
        Error::new(ErrorKind::UnexpectedEof, 13)
    );
    terminal(&mut w);
}

#[test]
fn declaration_misc_and_nonvalidating_constraints_remain_allowed() {
    let mut w = Writer::new(Vec::new());
    w.write_event(&Event::Declaration(Declaration {
        version: "1.0",
        encoding: None,
        standalone: Some(true),
    }))
    .unwrap();
    w.write_event(&Event::Comment(Cow::Borrowed("before")))
        .unwrap();
    w.write_dtd_event(&begin("<!DOCTYPE other [")).unwrap();
    w.write_dtd_event(&item(
        "<!ATTLIST r a CDATA #FIXED 'fixed' b ID #REQUIRED c (x|y) 'x'>",
        LiteralSource::Physical,
    ))
    .unwrap();
    w.write_dtd_event(&TakenDtdEvent::End).unwrap();
    w.write_event(&Event::Empty(el(
        "r",
        None,
        vec![attr("a", None, "different"), attr("c", None, "outside")],
    )))
    .unwrap();
    w.finish().unwrap();
    let position = w.position();
    w.finish().unwrap();
    assert_eq!(w.position(), position);
    assert_eq!(w.into_inner(), b"<?xml version=\"1.0\" standalone=\"yes\"?><!--before--><!DOCTYPE other [<!ATTLIST r a CDATA #FIXED 'fixed' b ID #REQUIRED c (x|y) 'x'>]><r a=\"different\" c=\"outside\"/>");
}

#[test]
fn body_uses_actual_escaped_literals_and_tokenized_defaults_at_every_depth() {
    let mut w = Writer::new(Vec::new());
    subset(
        &mut w,
        &[
            "<!ATTLIST r words NMTOKENS '  a   b  '>",
            "<!ATTLIST c words NMTOKENS #IMPLIED>",
        ],
    );
    w.write_event(&Event::Start(el("r", None, vec![]))).unwrap();
    w.write_event(&Event::Empty(el(
        "c",
        None,
        vec![attr("words", None, "a b")],
    )))
    .unwrap();
    w.write_event(&Event::End(nm("r", None))).unwrap();
    w.finish().unwrap();
    assert_eq!(w.into_inner(), b"<!DOCTYPE r [<!ATTLIST r words NMTOKENS '  a   b  '><!ATTLIST c words NMTOKENS #IMPLIED>]><r><c words=\"a b\"/></r>");
    for (value, expected) in [
        ("AT&T", "AT&amp;T"),
        ("<", "&lt;"),
        ("\r\n\t", "&#13;&#10;&#9;"),
    ] {
        let mut w = Writer::new(Vec::new());
        subset(&mut w, &["<!ATTLIST r a CDATA #IMPLIED>"]);
        w.write_event(&Event::Empty(el("r", None, vec![attr("a", None, value)])))
            .unwrap();
        w.finish().unwrap();
        assert!(
            String::from_utf8(w.into_inner())
                .unwrap()
                .ends_with(&format!("<r a=\"{expected}\"/>"))
        );
    }
    let mut w = Writer::new(Vec::new());
    subset(&mut w, &["<!ATTLIST r a NMTOKENS #IMPLIED>"]);
    let position = w.position();
    let before = w.sink.clone();
    assert_eq!(
        xml(w
            .write_event(&Event::Empty(el(
                "r",
                None,
                vec![attr("a", None, " a  b ")]
            )))
            .unwrap_err()),
        Error::new(ErrorKind::Syntax, position)
    );
    assert_eq!(w.sink, before);
    terminal(&mut w);
}

#[test]
fn sparse_and_expanded_default_namespaces_agree_and_bad_metadata_fails() {
    const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
    for expanded in [false, true] {
        let mut w = Writer::new(Vec::new());
        subset(
            &mut w,
            &["<!ATTLIST p:r xmlns:p CDATA 'urn:p' a CDATA 'default'>"],
        );
        let mut attrs = vec![attr("p:x", Some("urn:p"), "v")];
        if expanded {
            attrs.extend([
                attr("xmlns:p", Some(XMLNS), "urn:p"),
                attr("a", None, "default"),
            ]);
        }
        w.write_event(&Event::Empty(el("p:r", Some("urn:p"), attrs)))
            .unwrap();
        w.finish().unwrap();
        let output = String::from_utf8(w.into_inner()).unwrap();
        assert!(output.ends_with(if expanded {
            "<p:r p:x=\"v\" xmlns:p=\"urn:p\" a=\"default\"/>"
        } else {
            "<p:r p:x=\"v\"/>"
        }));
    }
    let mut w = Writer::new(Vec::new());
    subset(&mut w, &["<!ATTLIST p:r xmlns:p CDATA 'urn:p'>"]);
    let before = w.sink.clone();
    assert_eq!(
        xml(w
            .write_event(&Event::Empty(el("p:r", Some("wrong"), vec![])))
            .unwrap_err())
        .kind,
        ErrorKind::Namespace
    );
    assert_eq!(w.sink, before);
    terminal(&mut w);
}

#[test]
fn first_bindings_survive_end_and_explicit_attributes_suppress_defaults() {
    let mut w = Writer::new(Vec::new());
    subset(
        &mut w,
        &[
            "<!ENTITY e 'first'>",
            "<!ENTITY e 'second'>",
            "<!ATTLIST r a CDATA '&e;'>",
            "<!ATTLIST r a CDATA 'later'>",
        ],
    );
    let catalog = w.dtd[0].catalog().unwrap();
    assert!(matches!(
        catalog.lookup_general("e"),
        EntityLookup::Internal {
            replacement: "first",
            ..
        }
    ));
    assert_eq!(
        catalog.attributes("r").next().unwrap().default(),
        DefaultValue::Value {
            fixed: false,
            value: "first"
        }
    );
    w.write_event(&Event::Empty(el(
        "r",
        None,
        vec![attr("a", None, "explicit")],
    )))
    .unwrap();
    w.finish().unwrap();
    assert!(w.into_inner().ends_with(b"]><r a=\"explicit\"/>"));
}

#[test]
fn unavailable_duplicate_default_is_not_silent_and_typed_origin_is_retained() {
    let mut w = Writer::new(Vec::new());
    w.write_dtd_event(&begin("<!DOCTYPE r [")).unwrap();
    w.write_dtd_event(&item("<!ENTITY a '&later;'>", LiteralSource::Physical))
        .unwrap();
    w.write_dtd_event(&item(
        "<!ATTLIST r x CDATA 'first'>",
        LiteralSource::Physical,
    ))
    .unwrap();
    let position = w.position();
    let before = w.sink.clone();
    let error = dtd(w
        .write_dtd_event(&item("<!ATTLIST r x CDATA '&a;'>", LiteralSource::Physical))
        .unwrap_err());
    assert_eq!(
        error.kind,
        DtdWriteErrorKind::Unavailable(DependencyKind::MissingReadDeclaration)
    );
    assert_eq!(error.output_offset, position);
    assert!(matches!(
        error.value_location,
        Some(ValueLocation::Replacement { offset: 0, .. })
    ));
    assert_eq!(error.unit_offset, None);
    assert_eq!(w.sink, before);
    assert_eq!(w.dtd[0].failure(), Some(error));
    terminal(&mut w);
}

#[test]
fn output_and_token_ceilings_use_encoded_event_bytes_before_io() {
    for (stage, header, raw, encoded) in [
        (0, "<!DOCTYPE r>", "", 11),
        (1, "<!DOCTYPE r [", "<!--longer than header-->", 25),
        (2, "<!DOCTYPE r [", "", 2),
    ] {
        for short in [false, true] {
            let previous = if stage == 0 { 0 } else { 13 };
            let mut w = Writer::with_limits(
                Vec::new(),
                WriterLimits {
                    max_output_bytes: previous + encoded - usize::from(short),
                    ..WriterLimits::default()
                },
            );
            if stage != 0 {
                w.write_dtd_event(&begin(header)).unwrap();
            }
            let bytes = w.sink.clone();
            let event = match stage {
                0 => begin(header),
                1 => item(raw, LiteralSource::Physical),
                _ => TakenDtdEvent::End,
            };
            let result = w.write_dtd_event(&event);
            if short {
                assert!(
                    matches!(result, Err(DtdEventWriteError::Writer(WriteError::Xml(Error { kind: ErrorKind::LimitExceeded(Limit::OutputBytes), offset }))) if offset == previous)
                );
                assert_eq!(w.sink, bytes);
                assert_eq!(w.position(), previous);
                terminal(&mut w);
            } else {
                result.unwrap();
                assert_eq!(w.position(), previous + encoded);
            }
        }
    }
    for ceiling in [24, 25] {
        let mut w = Writer::with_limits(
            Vec::new(),
            WriterLimits {
                max_token_bytes: ceiling,
                ..WriterLimits::default()
            },
        );
        w.write_dtd_event(&begin("<!DOCTYPE r [")).unwrap();
        let result = w.write_dtd_event(&item("<!--longer than header-->", LiteralSource::Physical));
        if ceiling == 25 {
            result.unwrap();
        } else {
            assert!(matches!(
                result,
                Err(DtdEventWriteError::Writer(WriteError::Xml(Error {
                    kind: ErrorKind::LimitExceeded(Limit::TokenBytes),
                    offset: 13
                })))
            ));
            assert_eq!(w.sink, b"<!DOCTYPE r [");
        }
    }
    // Replacement encoding grows this entity declaration by four bytes.
    for ceiling in [18, 19] {
        let mut w = Writer::with_limits(
            Vec::new(),
            WriterLimits {
                max_token_bytes: ceiling,
                ..WriterLimits::default()
            },
        );
        w.write_dtd_event(&begin("<!DOCTYPE r [")).unwrap();
        let result = w.write_dtd_event(&item("<!ENTITY e '\r'>", LiteralSource::Replacement));
        if ceiling == 19 {
            result.unwrap();
        } else {
            assert!(matches!(
                result,
                Err(DtdEventWriteError::Writer(WriteError::Xml(Error {
                    kind: ErrorKind::LimitExceeded(Limit::TokenBytes),
                    ..
                })))
            ));
            assert_eq!(w.sink, b"<!DOCTYPE r [");
        }
    }
}

#[test]
fn shared_and_dtd_event_item_byte_ceilings_are_cumulative() {
    for ceiling in [2, 3] {
        let mut w = Writer::with_limits(
            Vec::new(),
            WriterLimits {
                max_events: ceiling,
                ..WriterLimits::default()
            },
        );
        w.write_dtd_event(&begin("<!DOCTYPE r>")).unwrap();
        w.write_dtd_event(&TakenDtdEvent::End).unwrap();
        let result = w.write_event(&empty());
        if ceiling == 3 {
            result.unwrap();
            w.finish().unwrap();
        } else {
            assert_eq!(
                xml(result.unwrap_err()),
                Error::new(ErrorKind::LimitExceeded(Limit::Events), 12)
            );
            assert_eq!(w.sink, b"<!DOCTYPE r>");
        }
    }
    for (kind, exact) in [
        (DtdWriteLimit::Events, 3),
        (DtdWriteLimit::Items, 1),
        (DtdWriteLimit::EncodedBytes, 23),
    ] {
        for short in [false, true] {
            let mut limits = DtdWriteLimits::default();
            let value = exact - usize::from(short);
            match kind {
                DtdWriteLimit::Events => limits.max_events = value,
                DtdWriteLimit::Items => limits.max_items = value,
                _ => limits.max_encoded_bytes = value,
            }
            let mut w = Writer::new(Vec::new());
            w.write_dtd_event_with_limits(&begin("<!DOCTYPE r ["), limits)
                .unwrap();
            let result = w.write_dtd_event(&item("<!--x-->", LiteralSource::Physical));
            let result = result.and_then(|()| w.write_dtd_event(&TakenDtdEvent::End));
            if short {
                let error = dtd(result.unwrap_err());
                assert_eq!(error.kind, DtdWriteErrorKind::Limit(kind));
                let boundary = if kind == DtdWriteLimit::Items { 13 } else { 21 };
                assert_eq!(w.position(), boundary);
                assert_eq!(error.output_offset, boundary);
                terminal(&mut w);
            } else {
                result.unwrap();
                assert_eq!(w.dtd[0].counters().events, 3);
                assert_eq!(w.dtd[0].counters().items, 1);
                assert_eq!(w.dtd[0].counters().encoded_bytes, 23);
                w.write_event(&empty()).unwrap();
                w.finish().unwrap();
            }
        }
    }
}

#[test]
fn tighter_consumer_unit_metadata_limits_are_checked_without_event_io() {
    for (limit, raw) in [
        (DtdLimit::UnitBytes, "<!DOCTYPE r ["),
        (DtdLimit::Tokens, "<!DOCTYPE r ["),
        (DtdLimit::ModelDepth, "<!ELEMENT r (a)>"),
        (DtdLimit::AttributeDefinitions, "<!ATTLIST r a CDATA 'v'>"),
    ] {
        for short in [false, true] {
            let mut limits = DtdWriteLimits::default();
            match limit {
                DtdLimit::UnitBytes => limits.unit.max_unit_bytes = 13 - usize::from(short),
                DtdLimit::Tokens => limits.unit.max_tokens = 1 - usize::from(short),
                DtdLimit::ModelDepth => limits.unit.max_model_depth = 1 - usize::from(short),
                DtdLimit::AttributeDefinitions => {
                    limits.unit.max_attribute_definitions = 1 - usize::from(short)
                }
            }
            let mut w = Writer::new(Vec::new());
            let is_header = matches!(limit, DtdLimit::UnitBytes | DtdLimit::Tokens);
            let result = if is_header {
                w.write_dtd_event_with_limits(&begin(raw), limits)
            } else {
                w.write_dtd_event_with_limits(&begin("<!DOCTYPE r ["), limits)
                    .unwrap();
                w.write_dtd_event(&item(raw, LiteralSource::Physical))
            };
            if short {
                let error = dtd(result.unwrap_err());
                assert_eq!(
                    error.kind,
                    DtdWriteErrorKind::Unit(DtdErrorKind::LimitExceeded(limit))
                );
                assert_eq!(w.position(), if is_header { 0 } else { 13 });
                assert_eq!(
                    w.sink,
                    if is_header {
                        b"".as_slice()
                    } else {
                        b"<!DOCTYPE r ["
                    }
                );
                terminal(&mut w);
            } else {
                result.unwrap();
            }
        }
    }
}

#[test]
fn catalog_declaration_binding_storage_value_and_expansion_ceilings_remain_live() {
    for limit in [
        CatalogLimit::Declarations,
        CatalogLimit::Bindings,
        CatalogLimit::StorageBytes,
        CatalogLimit::ValueBytes,
        CatalogLimit::ExpandedBytes,
    ] {
        for short in [false, true] {
            let mut limits = DtdWriteLimits::default();
            let exact = match limit {
                CatalogLimit::Declarations | CatalogLimit::StorageBytes => 2,
                _ => 1,
            };
            let value = exact - usize::from(short);
            match limit {
                CatalogLimit::Declarations => limits.catalog.max_declarations = value,
                CatalogLimit::Bindings => limits.catalog.max_bindings = value,
                CatalogLimit::StorageBytes => limits.catalog.max_storage_bytes = value,
                CatalogLimit::ValueBytes => limits.catalog.max_value_bytes = value,
                CatalogLimit::ExpandedBytes => limits.catalog.max_expanded_bytes = value,
                _ => unreachable!(),
            }
            let mut w = Writer::new(Vec::new());
            w.write_dtd_event_with_limits(&begin("<!DOCTYPE r ["), limits)
                .unwrap();
            let result = w.write_dtd_event(&item("<!ENTITY e 'v'>", LiteralSource::Physical));
            if short {
                let error = dtd(result.unwrap_err());
                assert_eq!(
                    error.kind,
                    DtdWriteErrorKind::Catalog(CatalogErrorKind::Limit(limit))
                );
                assert_eq!(error.output_offset, 13);
                assert!(matches!(
                    error.value_location,
                    Some(ValueLocation::Declaration { declaration: 1, .. })
                ));
                assert_eq!(w.sink, b"<!DOCTYPE r [");
                assert_eq!(w.dtd[0].failure(), Some(error));
                terminal(&mut w);
            } else {
                result.unwrap();
            }
        }
    }
    for limit in [CatalogLimit::References, CatalogLimit::Depth] {
        for short in [false, true] {
            let mut limits = DtdWriteLimits::default();
            match limit {
                CatalogLimit::References => limits.catalog.max_references = 1 - usize::from(short),
                _ => limits.catalog.max_depth = 1 - usize::from(short),
            }
            let mut w = Writer::new(Vec::new());
            w.write_dtd_event_with_limits(&begin("<!DOCTYPE r ["), limits)
                .unwrap();
            w.write_dtd_event(&item("<!ENTITY e 'v'>", LiteralSource::Physical))
                .unwrap();
            let position = w.position();
            let result =
                w.write_dtd_event(&item("<!ATTLIST r a CDATA '&e;'>", LiteralSource::Physical));
            if short {
                assert_eq!(
                    dtd(result.unwrap_err()).kind,
                    DtdWriteErrorKind::Catalog(CatalogErrorKind::Limit(limit))
                );
                assert_eq!(w.position(), position);
            } else {
                result.unwrap();
            }
        }
    }
}

#[test]
fn owned_budget_and_default_attribute_limits_apply_to_ordinary_body_calls() {
    for ceiling in [1, 2] {
        let mut w = Writer::with_limits(
            Vec::new(),
            WriterLimits {
                max_owned_bytes: ceiling,
                ..WriterLimits::default()
            },
        );
        subset(&mut w, &["<!ATTLIST r a CDATA 'v'>"]);
        let before = w.sink.clone();
        let result = w.write_event(&empty());
        if ceiling == 2 {
            result.unwrap();
            w.finish().unwrap();
        } else {
            assert_eq!(
                xml(result.unwrap_err()).kind,
                ErrorKind::LimitExceeded(Limit::OwnedBytes)
            );
            assert_eq!(w.sink, before);
            terminal(&mut w);
        }
    }
    let mut w = Writer::with_limits(
        Vec::new(),
        WriterLimits {
            max_attributes: 0,
            ..WriterLimits::default()
        },
    );
    subset(&mut w, &["<!ATTLIST r a CDATA 'v'>"]);
    let before = w.sink.clone();
    assert_eq!(
        xml(w.write_event(&empty()).unwrap_err()).kind,
        ErrorKind::LimitExceeded(Limit::Attributes)
    );
    assert_eq!(w.sink, before);
    terminal(&mut w);
}

#[test]
fn native_and_aggregate_work_limits_have_distinct_attribution_and_sticky_core_error() {
    // Header: one metadata token + 13 catalog bytes. Comment: one + 8.
    for (global, native, expected) in [
        (23, 21, None),
        (22, 21, Some(DtdWriteErrorKind::Limit(DtdWriteLimit::Work))),
        (
            24,
            20,
            Some(DtdWriteErrorKind::Catalog(CatalogErrorKind::Limit(
                CatalogLimit::Work,
            ))),
        ),
        (22, 20, Some(DtdWriteErrorKind::Limit(DtdWriteLimit::Work))),
    ] {
        let mut limits = DtdWriteLimits {
            max_work: global,
            ..DtdWriteLimits::default()
        };
        limits.catalog.max_work = native;
        let mut core = DtdWriterState::begin(&unit("<!DOCTYPE r ["), 13, 0, false, limits).unwrap();
        let result = core.item(&unit("<!--x-->"), LiteralSource::Physical, 8, 13);
        if let Some(expected) = expected {
            let error = result.unwrap_err();
            assert_eq!(error.kind, expected);
            assert_eq!(error.output_offset, 13);
            assert_eq!(core.failure(), Some(error));
            assert_eq!(core.end(2, 999).unwrap_err(), error);
            assert_eq!(
                core.item(&unit("<!--y-->"), LiteralSource::Physical, 8, 999)
                    .unwrap_err(),
                error
            );
            assert_eq!(core.check_body_ready(999).unwrap_err(), error);
        } else {
            result.unwrap();
            assert_eq!(core.counters().work, 23);
            core.end(2, 21).unwrap();
            assert!(core.catalog().is_some());
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Fault {
    Interrupted,
    Short,
    Zero,
    Oversized,
    Error,
    Panic,
}
#[derive(Default)]
struct Sink {
    bytes: Vec<u8>,
    script: std::collections::VecDeque<Fault>,
    calls: usize,
    flushes: usize,
    flush_fault: Option<Fault>,
}
impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        match self.script.pop_front() {
            Some(Fault::Interrupted) => Err(io::Error::from(io::ErrorKind::Interrupted)),
            Some(Fault::Zero) => Ok(0),
            Some(Fault::Oversized) => Ok(bytes.len() + 1),
            Some(Fault::Error) => Err(io::Error::other("independent sink error")),
            Some(Fault::Panic) => panic!("independent sink panic"),
            Some(Fault::Short) => {
                self.bytes.push(bytes[0]);
                Ok(1)
            }
            None => {
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        match self.flush_fault.take() {
            Some(Fault::Interrupted) => Err(io::Error::from(io::ErrorKind::Interrupted)),
            Some(Fault::Error) => Err(io::Error::other("independent flush error")),
            Some(Fault::Panic) => panic!("independent flush panic"),
            _ => Ok(()),
        }
    }
}
fn stage(stage: usize) -> (Writer<Sink>, TakenDtdEvent<'static>, &'static [u8]) {
    let mut w = Writer::new(Sink::default());
    if stage != 0 {
        w.write_dtd_event(&begin("<!DOCTYPE r [")).unwrap();
    }
    let (event, bytes) = match stage {
        0 => (begin("<!DOCTYPE r>"), b"<!DOCTYPE r".as_slice()),
        1 => (
            item("<!--x-->", LiteralSource::Physical),
            b"<!--x-->".as_slice(),
        ),
        _ => (TakenDtdEvent::End, b"]>".as_slice()),
    };
    (w, event, bytes)
}

#[test]
fn dtd_events_share_interrupted_and_short_write_semantics() {
    for event_stage in 0..3 {
        let (mut w, event, bytes) = stage(event_stage);
        let previous = w.position();
        let before = w.sink.bytes.clone();
        w.sink.script.extend([
            Fault::Interrupted,
            Fault::Short,
            Fault::Interrupted,
            Fault::Short,
        ]);
        w.write_dtd_event(&event).unwrap();
        assert_eq!(w.position(), previous + bytes.len());
        assert_eq!(w.sink.bytes, [before.as_slice(), bytes].concat());
        assert!(w.sink.script.is_empty());
    }
}

#[test]
fn every_dtd_event_is_terminal_after_zero_oversized_error_or_panic() {
    for event_stage in 0..3 {
        for fault in [Fault::Zero, Fault::Oversized, Fault::Error, Fault::Panic] {
            let (mut w, event, bytes) = stage(event_stage);
            let previous = w.position();
            let before = w.sink.bytes.clone();
            let partial = matches!(fault, Fault::Error | Fault::Panic);
            if partial {
                w.sink.script.push_back(Fault::Short);
            }
            w.sink.script.push_back(fault);
            let result = catch_unwind(AssertUnwindSafe(|| w.write_dtd_event(&event)));
            if matches!(fault, Fault::Panic) {
                assert!(result.is_err());
            } else {
                let error = result.unwrap().unwrap_err();
                assert!(
                    matches!(error, DtdEventWriteError::Writer(WriteError::Io(ref e)) if e.kind() == match fault { Fault::Zero => io::ErrorKind::WriteZero, Fault::Oversized => io::ErrorKind::InvalidData, _ => io::ErrorKind::Other })
                );
            }
            assert_eq!(w.position(), previous + usize::from(partial));
            assert_eq!(
                w.sink.bytes,
                [before.as_slice(), if partial { &bytes[..1] } else { b"" }].concat()
            );
            let calls = w.sink.calls;
            terminal(&mut w);
            assert_eq!(w.sink.calls, calls);
        }
    }
}

#[test]
fn finishing_dtd_document_retries_interrupt_and_poison_survives_flush_error_or_panic() {
    for fault in [Fault::Interrupted, Fault::Error, Fault::Panic] {
        let mut w = Writer::new(Sink::default());
        subset(&mut w, &[]);
        w.write_event(&empty()).unwrap();
        let bytes = w.sink.bytes.clone();
        let position = w.position();
        w.sink.flush_fault = Some(fault);
        let result = catch_unwind(AssertUnwindSafe(|| w.finish()));
        match fault {
            Fault::Interrupted => {
                result.unwrap().unwrap();
                assert_eq!(w.sink.flushes, 2);
                w.finish().unwrap();
                assert_eq!(w.sink.flushes, 2);
            }
            Fault::Panic => {
                assert!(result.is_err());
                terminal(&mut w);
            }
            _ => {
                assert!(matches!(result.unwrap(), Err(WriteError::Io(_))));
                terminal(&mut w);
            }
        }
        assert_eq!(w.position(), position);
        assert_eq!(w.sink.bytes, bytes);
    }
}

#[test]
fn default_name_and_duplicate_expanded_attribute_failures_retain_declaration_context() {
    for (decl, q, ns, attributes, kind, name_offset) in [
        (
            "<!ATTLIST r a:b:c CDATA 'v'>",
            "r",
            None,
            vec![],
            ErrorKind::InvalidName,
            15,
        ),
        (
            "<!ATTLIST r é:x:y CDATA 'v'>",
            "r",
            None,
            vec![],
            ErrorKind::InvalidName,
            16,
        ),
        (
            "<!ATTLIST r xmlns:a CDATA 'urn:x' xmlns:b CDATA 'urn:x' a:x CDATA 'first' b:x CDATA 'second'>",
            "r",
            None,
            vec![],
            ErrorKind::DuplicateAttribute,
            0,
        ),
        (
            "<!ATTLIST r xmlns:p CDATA 'urn:x'>",
            "r",
            None,
            vec![attr("p:x", Some("wrong"), "v")],
            ErrorKind::Namespace,
            0,
        ),
    ] {
        let mut w = Writer::new(Sink::default());
        subset(&mut w, &[decl]);
        let position = w.position();
        let before = w.sink.bytes.clone();
        let calls = w.sink.calls;
        let error = xml(w
            .write_event(&Event::Empty(el(q, ns, attributes)))
            .unwrap_err());
        assert_eq!(error.kind, kind);
        let typed = w.dtd[0].failure().unwrap();
        assert_eq!(typed.kind, DtdWriteErrorKind::Parser(kind));
        if kind == ErrorKind::InvalidName {
            assert_eq!(typed.output_offset, position);
            assert_eq!(
                typed.value_location,
                Some(ValueLocation::Declaration {
                    declaration: 1,
                    offset: name_offset
                })
            );
        }
        assert_eq!(w.position(), position);
        assert_eq!(w.sink.bytes, before);
        assert_eq!(w.sink.calls, calls);
        terminal(&mut w);
    }
}

#[test]
fn replacement_growth_and_late_identifier_errors_never_write_rejected_prefix() {
    for short in [false, true] {
        let limits = DtdWriteLimits {
            max_encoded_bytes: 32 - usize::from(short),
            ..DtdWriteLimits::default()
        };
        let mut w = Writer::new(Sink::default());
        w.write_dtd_event_with_limits(&begin("<!DOCTYPE r ["), limits)
            .unwrap();
        let calls = w.sink.calls;
        let result = w.write_dtd_event(&item("<!ENTITY e '\r'>", LiteralSource::Replacement));
        if short {
            assert_eq!(
                dtd(result.unwrap_err()).kind,
                DtdWriteErrorKind::Limit(DtdWriteLimit::EncodedBytes)
            );
            assert_eq!(w.sink.calls, calls);
            assert_eq!(w.sink.bytes, b"<!DOCTYPE r [");
        } else {
            result.unwrap();
            assert_eq!(w.dtd[0].counters().encoded_bytes, 32);
            assert_eq!(w.sink.bytes, b"<!DOCTYPE r [<!ENTITY e '&#13;'>");
        }
    }
    let mut w = Writer::new(Sink::default());
    w.write_dtd_event(&begin("<!DOCTYPE r [")).unwrap();
    let calls = w.sink.calls;
    let error = dtd(w
        .write_dtd_event(&item(
            "<!ENTITY e PUBLIC 'okay' 'long-prefix\rnot-physical'>",
            LiteralSource::Replacement,
        ))
        .unwrap_err());
    assert_eq!(error.kind, DtdWriteErrorKind::UnrepresentableIdentifier);
    assert_eq!(w.sink.calls, calls);
    assert_eq!(w.sink.bytes, b"<!DOCTYPE r [");
    assert_eq!(w.position(), 13);
    terminal(&mut w);
}

#[test]
fn exhausted_dtd_counts_precede_encoding_and_preserve_the_prepared_buffer() {
    let mut w = Writer::with_limits(
        Sink::default(),
        WriterLimits {
            max_output_bytes: 0,
            ..WriterLimits::default()
        },
    );
    let error = dtd(w
        .write_dtd_event_with_limits(
            &begin("<!DOCTYPE r ["),
            DtdWriteLimits {
                max_events: 0,
                ..DtdWriteLimits::default()
            },
        )
        .unwrap_err());
    assert_eq!(error.kind, DtdWriteErrorKind::Limit(DtdWriteLimit::Events));
    assert!(w.buffer.text.is_empty());
    assert!(w.dtd.is_empty());
    assert_eq!(w.sink.calls, 0);
    assert_eq!(w.position(), 0);
    terminal(&mut w);

    for limit in [DtdWriteLimit::Events, DtdWriteLimit::Items] {
        let mut limits = DtdWriteLimits::default();
        match limit {
            DtdWriteLimit::Events => limits.max_events = 1,
            _ => limits.max_items = 0,
        }
        let mut w = Writer::new(Sink::default());
        w.write_dtd_event_with_limits(&begin("<!DOCTYPE r ["), limits)
            .unwrap();
        let buffer = w.buffer.text.clone();
        let bytes = w.sink.bytes.clone();
        let calls = w.sink.calls;
        let counts = w.dtd[0].counters();
        let error = dtd(w
            .write_dtd_event(&item(
                "<!ENTITY e SYSTEM 'late\ridentifier'>",
                LiteralSource::Replacement,
            ))
            .unwrap_err());
        assert_eq!(error.kind, DtdWriteErrorKind::Limit(limit));
        assert_eq!(error.output_offset, 13);
        assert_eq!(w.buffer.text, buffer);
        assert_eq!(w.sink.bytes, bytes);
        assert_eq!(w.sink.calls, calls);
        assert_eq!(w.dtd[0].counters(), counts);
        assert_eq!(w.position(), 13);
        terminal(&mut w);
    }

    let mut w = Writer::new(Sink::default());
    w.write_dtd_event_with_limits(
        &begin("<!DOCTYPE r ["),
        DtdWriteLimits {
            max_events: 1,
            ..DtdWriteLimits::default()
        },
    )
    .unwrap();
    let buffer = w.buffer.text.clone();
    let calls = w.sink.calls;
    let counts = w.dtd[0].counters();
    let error = dtd(w.write_dtd_event(&TakenDtdEvent::End).unwrap_err());
    assert_eq!(error.kind, DtdWriteErrorKind::Limit(DtdWriteLimit::Events));
    assert_eq!(w.buffer.text, buffer);
    assert_eq!(w.sink.calls, calls);
    assert_eq!(w.dtd[0].counters(), counts);
    assert_eq!(w.position(), 13);
    terminal(&mut w);
}
