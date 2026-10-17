//! Remaining explicit Q1–Q8 discriminators, frozen before facade execution.
use super::*;
use crate::dtd_tag::DtdTagErrorKind as Tag;

interaction!(repeated_scanner_work, REPEATED_PE, 58 => 61,
    |p,n| p.declarations.scan.max_work=n, |_,_,_| {},
    dtd(D::Scan(ScanErrorKind::Limit(ScanLimit::Work)),40,replacement(0,2,0..7,40..43)).local(7),43,
    replacement(0,1,0..8,37..40).map(SourceOrigin::from_repr),3);
interaction!(generated_model_depth, PE_MODEL, 0 => 1,
    |p,n| p.declarations.unit.max_model_depth=n, |_,_,_| {},
    dtd(D::Grammar(DtdErrorKind::LimitExceeded(DtdLimit::ModelDepth)),45,replacement(0,1,0..16,45..48)).local(12),48,last(13..45),2);
interaction!(stable_value_bytes, EXPLICIT, 0 => 1,
    |p,n| p.declarations.catalog.max_value_bytes=n, |_,_,_| {},
    content(C::Tag(Tag::Catalog(CatalogErrorKind::Limit(CatalogLimit::ValueBytes))),18,input(12..22)).value(Site::Input(18)),22,last(11..12),2);
interaction!(stable_expanded_bytes, EXPLICIT, 0 => 1,
    |p,n| p.declarations.catalog.max_expanded_bytes=n, |_,_,_| {},
    content(C::Tag(Tag::Catalog(CatalogErrorKind::Limit(CatalogLimit::ExpandedBytes))),18,input(12..22)).value(Site::Input(18)),22,last(11..12),2);
interaction!(stable_native_work, EXPLICIT, 13 => 14,
    |p,n| p.declarations.catalog.max_work=n, |_,_,_| {},
    content(C::Tag(Tag::Catalog(CatalogErrorKind::Limit(CatalogLimit::Work))),18,input(12..22)).value(Site::Input(18)),22,last(11..12),2);
interaction!(read_default_native_value, READ_DEFAULT, 0 => 1,
    |p,n| p.declarations.catalog.max_value_bytes=n, |_,_,_| {},
    dtd(D::Catalog(CatalogErrorKind::Limit(CatalogLimit::ValueBytes)),34,input(13..37)).value(Site::Declaration(1,21)),37,last(0..13),1);
interaction!(tokenized_expanded_before_collapse, TOKENIZED, 3 => 4,
    |p,n| {p.declarations.catalog.max_expanded_bytes=n; p.declarations.catalog.max_value_bytes=1;}, |_,_,_| {},
    content(C::Tag(Tag::Catalog(CatalogErrorKind::Limit(CatalogLimit::ExpandedBytes))),56,input(47..60)).value(Site::Input(56)),60,last(45..47),3);
interaction!(indirect_value_context, INDIRECT, 6 => 7,
    |p,n| p.declarations.catalog.max_expanded_bytes=n, |_,_,_| {},
    content(C::Tag(Tag::Catalog(CatalogErrorKind::Limit(CatalogLimit::ExpandedBytes))),48,input(48..60)).value(Site::Replacement(1,1)),60,last(46..48),4);
interaction!(nested_empty_attempts, EMPTY_NESTED, 1 => 2,
    |p,n| p.declarations.frames.max_reference_attempts=n, |_,_,_| {},
    content(C::Frame(FrameErrorKind::LimitExceeded(FrameLimit::ReferenceAttempts)),49,replacement(0,1,0..3,49..52)).local(0),52,last(46..49),5);
interaction!(nested_empty_work, EMPTY_NESTED, 5 => 6,
    |p,n| p.declarations.frames.max_work=n, |_,_,_| {},
    content(C::Frame(FrameErrorKind::LimitExceeded(FrameLimit::Work)),49,replacement(0,1,0..3,49..52)).local(0),52,last(46..49),5);
interaction!(nested_empty_spelling, EMPTY_NESTED, 2 => 3,
    |p,n| p.declarations.frames.max_expanded_bytes=n, |_,_,_| {},
    content(C::Frame(FrameErrorKind::LimitExceeded(FrameLimit::ExpandedBytes)),49,replacement(0,1,0..3,49..52)).local(0),52,last(46..49),5);
interaction!(repeated_local_work, REPEATED_LOCAL, 15 => 16,
    |p,n| p.local.max_work=n, |_,_,_| {},
    content(C::Text(T::Local(LocalErrorKind::Work)),39,replacement(0,2,0..0,39..42)).local(0),42,
    replacement(0,1,0..4,36..39).map(SourceOrigin::from_repr),5);
interaction!(scanner_global_competitor, TINY, 11 => 48,
    |p,n| {p.declarations.max_work=n; p.declarations.scan.max_work=if n==11 {11} else {12};}, |_,_,_| {},
    dtd(D::Limit(ControllerLimit::Work),11,None),11,None,0);
interaction!(reference_global_competitor, ONE_GE, 104 => 122,
    |p,n| {p.declarations.max_work=n; p.text.reference.max_work=if n==104 {8} else {9};}, |_,_,_| {},
    content(C::Work,33,None),36,last(30..33),4);
interaction!(reference_native_competitor, ONE_GE, 105 => 122,
    |p,n| {p.declarations.max_work=n; p.text.reference.max_work=if n==105 {8} else {9};}, |_,_,_| {},
    content(C::Text(T::Reference(ReferenceErrorKind::Limit(ReferenceLimit::Work))),33,None),36,last(30..33),4);
interaction!(frame_global_competitor, ONE_GE, 105 => 122,
    |p,n| {p.declarations.max_work=n; p.declarations.frames.max_work=if n==105 {0} else {2};}, |_,_,_| {},
    content(C::Work,33,input(33..36)),36,last(30..33),4);
interaction!(frame_native_competitor, ONE_GE, 106 => 122,
    |p,n| {p.declarations.max_work=n; p.declarations.frames.max_work=if n==106 {0} else {2};}, |_,_,_| {},
    content(C::Frame(FrameErrorKind::LimitExceeded(FrameLimit::Work)),33,input(33..36)),36,last(30..33),4);
interaction!(local_global_competitor, ONE_GE, 107 => 122,
    |p,n| {p.declarations.max_work=n; p.local.max_work=if n==107 {1} else {2};}, |_,_,_| {},
    content(C::Work,33,replacement(0,1,0..0,33..36)).local(0),36,last(30..33),4);
interaction!(local_native_competitor, ONE_GE, 108 => 122,
    |p,n| {p.declarations.max_work=n; p.local.max_work=if n==108 {1} else {2};}, |_,_,_| {},
    content(C::Text(T::Local(LocalErrorKind::Work)),33,replacement(0,1,0..0,33..36)).local(0),36,last(30..33),4);
interaction!(body_constructor_inherited_work, TINY, 35 => 48,
    |p,n| p.text.max_work=n, |_,_,_| {},
    content(C::Text(T::Work),12,None),12,last(11..12),2);
interaction!(body_active_inherited_work, TINY, 36 => 48,
    |p,n| p.text.max_work=n, |_,_,_| {},
    content(C::Work,12,None),12,last(11..12),2);
interaction!(scanner_work_before_growth, TINY, 0 => 12,
    |p,n| {p.declarations.scan.max_work=n; p.declarations.scan.max_unit_bytes=n; p.declarations.scan.max_scratch_bytes=n;}, |_,_,_| {},
    dtd(D::Scan(ScanErrorKind::Limit(ScanLimit::Work)),0,None),0,None,0);
interaction!(unit_bytes_before_tokens, TINY, 11 => 12,
    |p,n| {p.declarations.unit.max_unit_bytes=n; p.declarations.unit.max_tokens=if n==11 {0} else {1};}, |_,_,_| {},
    dtd(D::Grammar(DtdErrorKind::LimitExceeded(DtdLimit::UnitBytes)),11,input(0..12)),12,None,0);
interaction!(reference_work_before_growth, ONE_GE, 0 => 9,
    |p,n| {p.text.reference.max_work=n; p.text.reference.max_bytes=if n==0 {0} else {3}; p.text.reference.max_scratch_bytes=if n==0 {0} else {3};}, |_,_,_| {},
    content(C::Text(T::Reference(ReferenceErrorKind::Limit(ReferenceLimit::Work))),33,None),33,last(30..33),4);
interaction!(expanded_before_value, EXPLICIT, 0 => 1,
    |p,n| {p.declarations.catalog.max_expanded_bytes=n; p.declarations.catalog.max_value_bytes=n;}, |_,_,_| {},
    content(C::Tag(Tag::Catalog(CatalogErrorKind::Limit(CatalogLimit::ExpandedBytes))),18,input(12..22)).value(Site::Input(18)),22,last(11..12),2);

#[test]
fn repeated_promotions_are_cumulative_when_prior_items_are_retained_or_dropped() {
    for (parser, name) in [
        (false, "raw_promotion_dropped"),
        (false, "raw_promotion_retained"),
        (true, "parser_promotion_dropped"),
        (true, "parser_promotion_retained"),
    ] {
        verify(Case {
            name,
            source: REPEATED_PE,
            bounds: (15, 16),
            prefix: Some(3),
            configure: if parser {
                |_, _| {}
            } else {
                |p, n| p.declarations.max_owned_bytes = n
            },
            public: if parser {
                |l, _, n| l.max_owned_bytes = n
            } else {
                |_, _, _| {}
            },
            error: dtd(
                if parser {
                    D::Ownership(ErrorKind::LimitExceeded(Limit::OwnedBytes))
                } else {
                    D::Limit(ControllerLimit::OwnedBytes)
                },
                40,
                replacement(0, 2, 0..8, 40..43),
            )
            .local(0),
            position: 43,
            last: replacement(0, 1, 0..8, 37..40).map(SourceOrigin::from_repr),
        });
    }
}

fn assert_sticky(
    reader: &mut DtdReader<'_>,
    expected: Diagnostic,
    position: usize,
    origin: Option<SourceOrigin>,
) {
    let error = reader.read_event().unwrap_err();
    assert_eq!(Diagnostic::from_error(error), expected);
    let state = counters(reader);
    for _ in 0..3 {
        assert_eq!(reader.position(), position);
        assert_eq!(reader.event_origin(), origin);
        assert_eq!(
            reader.event_range(),
            origin.and_then(SourceOrigin::input_range)
        );
        assert_eq!(reader.read_event().unwrap_err(), error);
        assert_eq!(counters(reader), state);
    }
}

#[test]
fn malformed_generated_grammar_precedes_both_raw_promotion_budgets() {
    for (native, parser) in [
        (0, usize::MAX),
        (usize::MAX, 0),
        (0, 0),
        (usize::MAX, usize::MAX),
    ] {
        let mut p = policy();
        p.declarations.max_owned_bytes = native;
        let mut reader = DtdReader::with_limits(
            PE_BAD.as_bytes(),
            Limits {
                max_token_bytes: 16,
                max_owned_bytes: parser,
                ..Limits::default()
            },
            p,
        )
        .unwrap();
        for expected in ["begin", "item"] {
            assert_eq!(label(&reader.read_event().unwrap().unwrap().0), expected);
        }
        assert_sticky(
            &mut reader,
            dtd(
                D::Grammar(DtdErrorKind::Syntax),
                41,
                replacement(0, 1, 0..12, 41..44),
            )
            .local(11),
            44,
            last(13..41),
        );
        assert_eq!(reader.into_inner(), PE_BAD.as_bytes());
    }
}

#[test]
fn matching_attribute_prefix_still_validates_missing_and_unavailable_references_before_copy() {
    use crate::dtd_catalog::DependencyKind;
    for (source, anchor, span, latest, kind) in [
        (
            "<!DOCTYPE r><r a='v&z;'/>",
            19,
            12..25,
            11..12,
            Tag::Catalog(CatalogErrorKind::EntityDeclared),
        ),
        (
            "<!DOCTYPE r SYSTEM 'x'><r a='v&z;'/>",
            30,
            23..36,
            22..23,
            Tag::Unavailable(DependencyKind::UnreadEntity),
        ),
    ] {
        let mut reader = DtdReader::with_limits(
            source.as_bytes(),
            Limits {
                max_token_bytes: 16,
                max_owned_bytes: 0,
                ..Limits::default()
            },
            policy(),
        )
        .unwrap();
        for expected in ["begin", "end"] {
            assert_eq!(label(&reader.read_event().unwrap().unwrap().0), expected);
        }
        assert_sticky(
            &mut reader,
            content(C::Tag(kind), anchor, input(span.clone())).value(Site::Input(anchor)),
            span.end,
            last(latest),
        );
        assert_eq!(reader.into_inner(), source.as_bytes());
    }
}

#[test]
fn dtd_free_constructors_ignore_invalid_and_overflowing_session_configuration() {
    for overflow in [false, true] {
        let mut p = policy();
        p.declarations.scan = crate::dtd_scan::ScanLimits {
            max_unit_bytes: 0,
            max_scratch_bytes: 0,
            max_work: 0,
        };
        p.declarations.max_work = 0;
        p.declarations.max_owned_bytes = 0;
        p.declarations.max_physical_bytes = 0;
        p.declarations.unit = crate::dtd::DtdLimits {
            max_unit_bytes: 0,
            max_tokens: 0,
            max_model_depth: 0,
            max_attribute_definitions: 0,
        };
        p.declarations.catalog = crate::dtd_catalog::CatalogLimits {
            max_declarations: 0,
            max_bindings: 0,
            max_storage_bytes: 0,
            max_value_bytes: 0,
            max_expanded_bytes: 0,
            max_work: 0,
            max_references: 0,
            max_depth: 0,
        };
        p.declarations.frames = crate::dtd_frames::FrameLimits {
            max_active_depth: 0,
            max_reference_attempts: 0,
            max_work: 0,
            max_expanded_bytes: 0,
        };
        p.text.max_work = 0;
        p.text.reference = crate::dtd_reference::ReferenceLimits {
            max_bytes: 0,
            max_scratch_bytes: 0,
            max_work: 0,
        };
        p.text.scratch = TextScratchLimits {
            input_token_bytes: if overflow { usize::MAX } else { 0 },
            replacement_token_bytes: if overflow { usize::MAX } else { 0 },
            max_bytes: 0,
        };
        p.local = LocalLimits {
            max_token_bytes: 0,
            max_scratch_bytes: 0,
            max_work: 0,
        };
        for constructor in 0..3 {
            let mut reader = match constructor {
                0 => DtdReader::new(b"<r/>", p),
                1 => DtdReader::with_limits(b"<r/>", Limits::default(), p),
                _ => DtdReader::with_decode_limits(
                    b"<r/>",
                    Limits::default(),
                    DecodeLimits::default(),
                    p,
                ),
            }
            .unwrap();
            let (event, origin) = reader.read_event().unwrap().unwrap();
            assert_eq!(label(&event), "=r");
            assert_eq!(origin, SourceOrigin::input(0..4));
            assert_eq!(reader.position(), 4);
            assert!(reader.read_event().unwrap().is_none());
            assert!(reader.read_event().unwrap().is_none());
            assert_eq!(reader.into_inner(), b"<r/>");
        }
        assert!(
            matches!(DtdReader::with_limits(b"<r/>",Limits {max_input_bytes:3,..Limits::default()},p),Err(SessionError::Xml(e)) if e==Error::new(ErrorKind::LimitExceeded(Limit::InputBytes),0))
        );
    }
}

#[test]
fn exact_shared_work_and_one_above_do_not_reopen_or_add_work() {
    for declaration in [48, 49] {
        for text in [48, 49, usize::MAX] {
            let mut p = policy();
            p.declarations.max_work = declaration;
            p.text.max_work = text;
            let mut reader = DtdReader::with_limits(
                TINY.as_bytes(),
                Limits {
                    max_token_bytes: 16,
                    ..Limits::default()
                },
                p,
            )
            .unwrap();
            let mut labels = Vec::new();
            while let Some((event, _)) = reader.read_event().unwrap() {
                labels.push(label(&event));
            }
            assert_eq!(labels, golden(TINY));
            assert_eq!(counters(&reader).unwrap().controller.work, 48);
            assert!(reader.read_event().unwrap().is_none());
        }
    }
}

#[test]
fn canonical_scratch_slots_have_independent_below_exact_above_controls() {
    for family in 0..5 {
        let boundary = match family {
            0 | 3 | 4 => 28,
            1 => 16,
            _ => 8,
        };
        for ceiling in [boundary - 1, boundary, boundary + 1] {
            let mut p = policy();
            p.local.max_token_bytes = 8;
            p.local.max_scratch_bytes = 8;
            p.text.reference.max_bytes = if family == 3 { 10 } else { 4 };
            p.text.reference.max_scratch_bytes = if family == 4 { 10 } else { 4 };
            p.text.scratch = TextScratchLimits {
                input_token_bytes: 16,
                replacement_token_bytes: 8,
                max_bytes: 29,
            };
            match family {
                1 => p.text.scratch.input_token_bytes = ceiling,
                2 => p.text.scratch.replacement_token_bytes = ceiling,
                _ => p.text.scratch.max_bytes = ceiling,
            }
            let mut reader = DtdReader::with_limits(
                TINY.as_bytes(),
                Limits {
                    max_token_bytes: 16,
                    ..Limits::default()
                },
                p,
            )
            .unwrap();
            for expected in ["begin", "end"] {
                assert_eq!(label(&reader.read_event().unwrap().unwrap().0), expected);
            }
            if ceiling < boundary {
                assert_sticky(
                    &mut reader,
                    content(C::Text(T::Scratch), 12, None),
                    12,
                    last(11..12),
                );
            } else {
                let (event, origin) = reader.read_event().unwrap().unwrap();
                assert_eq!(label(&event), "=r");
                assert_eq!(origin, SourceOrigin::input(12..16));
                assert_eq!(reader.position(), 16);
                assert!(reader.read_event().unwrap().is_none());
                assert!(reader.read_event().unwrap().is_none());
            }
            assert_eq!(reader.into_inner(), TINY.as_bytes());
        }
    }
}
