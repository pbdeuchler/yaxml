//! Independent facade quotas frozen in docs/plans/dtd-reader-quota-conformance.md.
use super::*;
use crate::dtd::{DtdErrorKind, DtdLimit};
use crate::dtd_catalog::{CatalogLimit, CatalogUsage, ValueLocation};
use crate::dtd_controller::content::ContentErrorKind as C;
use crate::dtd_controller::{ControllerCounters, ControllerErrorKind as D, ControllerLimit};
use crate::dtd_frames::{EntityDeclarationId, EntityInvocationId, FragmentOrigin, FrameCounters};
use crate::dtd_reference::{ReferenceErrorKind, ReferenceLimit};
use crate::dtd_scan::{ScanErrorKind, ScanLimit};
use crate::tokenizer::local::LocalErrorKind;
use crate::tokenizer::text::{TextErrorKind as T, TextUsage};
use std::num::NonZeroU64;

const TINY: &str = "<!DOCTYPE r><r/>";
const MODEL: &str = "<!DOCTYPE r [<!ELEMENT r (a)>]><r/>";
const ATTRS: &str = "<!DOCTYPE r [<!ATTLIST r a CDATA #IMPLIED>]><r/>";
const VALUE: &str = "<!DOCTYPE r [<!ENTITY e 'é'>]><r a='&e;'/>";
const GENERATED: &str = "<!DOCTYPE r [<!ENTITY e '<n/>'>]><r>&e;</r>";
const REFERENCES: &str = "<!DOCTYPE r [<!ENTITY e 'x'>]><r>&e;&e;</r>";
const NESTED: &str = "<!DOCTYPE r [<!ENTITY a '&b;'><!ENTITY b 'B'>]><r>&a;</r>";
const PARAMETER: &str = "<!DOCTYPE r [<!ENTITY % p '<!--a&#13;b-->'>%p;]><r/>";
const HANDOFF: &str = "<!DOCTYPE r [<!ENTITY % p \"<!ENTITY e 'x'>\">%p;]><r>&e;</r>";
const BODY_UNICODE: &str = "<!DOCTYPE r><r a='éé'/>";
const DEPTH: &str = "<!DOCTYPE r [<!ENTITY e '<n><m/></n>'>]><r>&e;</r>";
const LOCAL_NAMED: &str = "<!DOCTYPE r [<!ENTITY e '&#38;inner;'><!ENTITY inner 'I'>]><r>&e;</r>";
const LOCAL_NUMERIC: &str = "<!DOCTYPE r [<!ENTITY e '&#38;#65;'>]><r>&e;</r>";
const GENERAL_TEXT: &str = "<!DOCTYPE r [<!ENTITY e 'x'>]><r>1234567890123456&e;</r>";
const NUMERIC_TEXT: &str = "<!DOCTYPE r [<!ENTITY e 'x'>]><r>1234567890123456&#65;</r>";
const SHARED: &str = "<?p a\r\n?><!DOCTYPE r [<!ENTITY % p '<!--a&#13;b-->'>%p;]><r/>";

fn policy() -> SessionLimits {
    let mut p = configured();
    p.text.reference.max_bytes = 16;
    p.text.reference.max_scratch_bytes = 16;
    p.text.scratch = TextScratchLimits {
        input_token_bytes: 16,
        replacement_token_bytes: 16,
        max_bytes: 48,
    };
    p.local.max_token_bytes = 16;
    p.local.max_scratch_bytes = 16;
    p
}

fn input(range: Range<usize>) -> Option<FragmentOrigin> {
    FragmentOrigin::input(range)
}
fn last(range: Range<usize>) -> Option<SourceOrigin> {
    Some(SourceOrigin::input(range))
}
fn replacement(
    declaration: usize,
    invocation: u64,
    local: Range<usize>,
    physical: Range<usize>,
) -> Option<FragmentOrigin> {
    Some(FragmentOrigin::Replacement {
        declaration: EntityDeclarationId::from_index(declaration),
        invocation: EntityInvocationId(NonZeroU64::new(invocation).unwrap()),
        local: crate::origin::ByteSpan::new(local).unwrap(),
        physical_reference: crate::origin::ByteSpan::new(physical).unwrap(),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Dtd(D),
    Content(C),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Site {
    Input(usize),
    Declaration(usize, usize),
    Replacement(usize, usize),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Diagnostic {
    kind: Kind,
    physical: usize,
    origin: Option<FragmentOrigin>,
    local: Option<usize>,
    value: Option<Site>,
}
fn dtd(kind: D, physical: usize, origin: Option<FragmentOrigin>) -> Diagnostic {
    Diagnostic {
        kind: Kind::Dtd(kind),
        physical,
        origin,
        local: None,
        value: None,
    }
}
fn content(kind: C, physical: usize, origin: Option<FragmentOrigin>) -> Diagnostic {
    Diagnostic {
        kind: Kind::Content(kind),
        physical,
        origin,
        local: None,
        value: None,
    }
}
impl Diagnostic {
    fn local(mut self, offset: usize) -> Self {
        self.local = Some(offset);
        self
    }
    fn value(mut self, site: Site) -> Self {
        self.value = Some(site);
        self
    }
    fn from_error(error: SessionError) -> Self {
        let (kind, physical, origin, local, value) = match error {
            SessionError::Dtd(e) => (
                Kind::Dtd(e.kind),
                e.physical_offset,
                e.origin,
                e.local_offset,
                e.value_location,
            ),
            SessionError::Content(e) => (
                Kind::Content(e.kind),
                e.physical_offset,
                e.origin,
                e.local_offset,
                e.value_location,
            ),
            other => panic!("unexpected facade error: {other:?}"),
        };
        let value = value.map(|site| match site {
            ValueLocation::Input(offset) => Site::Input(offset),
            ValueLocation::Declaration {
                declaration,
                offset,
            } => Site::Declaration(declaration, offset),
            ValueLocation::Replacement { entity, offset } => {
                Site::Replacement(entity.index(), offset)
            }
        });
        Self {
            kind,
            physical,
            origin,
            local,
            value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Counters {
    controller: ControllerCounters,
    catalog: Option<CatalogUsage>,
    frames: Option<FrameCounters>,
    text: Option<TextUsage>,
}
fn counters(reader: &DtdReader<'_>) -> Option<Counters> {
    match reader.session.first()? {
        Phase::Declarations { controller, .. } => Some(Counters {
            controller: controller.counters(),
            catalog: Some(controller.catalog().usage()),
            frames: None,
            text: None,
        }),
        Phase::Content(controller) => Some(Counters {
            controller: controller.counters(),
            catalog: None,
            frames: Some(controller.frame_counters()),
            text: Some(controller.usage()),
        }),
    }
}

fn golden(source: &str) -> &'static [&'static str] {
    match source {
        TINY | BODY_UNICODE => &["begin", "end", "=r"],
        MODEL | ATTRS | VALUE => &["begin", "item", "end", "=r"],
        GENERATED => &["begin", "item", "end", "+r", "=n", "-r"],
        REFERENCES => &["begin", "item", "end", "+r", "Tx", "Tx", "-r"],
        NESTED => &["begin", "item", "item", "end", "+r", "TB", "-r"],
        PARAMETER => &["begin", "item", "item", "end", "=r"],
        HANDOFF => &["begin", "item", "item", "end", "+r", "Tx", "-r"],
        DEPTH => &["begin", "item", "end", "+r", "+n", "=m", "-n", "-r"],
        LOCAL_NAMED => &["begin", "item", "item", "end", "+r", "TI", "-r"],
        LOCAL_NUMERIC => &["begin", "item", "end", "+r", "TA", "-r"],
        GENERAL_TEXT => &[
            "begin",
            "item",
            "end",
            "+r",
            "T1234567890123456",
            "Tx",
            "-r",
        ],
        NUMERIC_TEXT => &["begin", "item", "end", "+r", "T1234567890123456A", "-r"],
        SHARED => &["pi", "begin", "item", "item", "end", "=r"],
        _ => panic!("unknown literal fixture"),
    }
}
struct Case {
    name: &'static str,
    source: &'static str,
    bounds: (usize, usize),
    configure: fn(&mut SessionLimits, usize),
    public: fn(&mut Limits, &mut DecodeLimits, usize),
    prefix: Option<usize>,
    error: Diagnostic,
    position: usize,
    last: Option<SourceOrigin>,
}
fn verify(case: Case) {
    let exact_shared_work = matches!(case.name, "declaration_work_exact" | "text_work_exact");
    for (ceiling, succeeds) in [(case.bounds.0, false), (case.bounds.1, true)] {
        let mut p = policy();
        (case.configure)(&mut p, ceiling);
        let mut limits = Limits {
            max_token_bytes: 16,
            ..Limits::default()
        };
        let mut decode = DecodeLimits::default();
        (case.public)(&mut limits, &mut decode, ceiling);
        let mut reader =
            DtdReader::with_decode_limits(case.source.as_bytes(), limits, decode, p).unwrap();
        let mut labels = Vec::new();
        let mut previous = None::<ControllerCounters>;
        let mut terminated = false;
        for _ in 0..32 {
            match reader.read_event() {
                Ok(Some((event, origin))) => {
                    assert_eq!(reader.event_origin(), Some(origin), "{}", case.name);
                    if let ReaderEvent::Xml(Event::ProcessingInstruction { target, data }) = &event
                    {
                        assert_eq!(*target, "p");
                        assert_eq!(data, "a\n");
                        assert!(matches!(data, std::borrow::Cow::Owned(_)));
                        labels.push("pi".to_owned());
                    } else {
                        labels.push(label(&event));
                    }
                    if let Some(now) = counters(&reader) {
                        if let Some(before) = previous {
                            assert!(
                                now.controller.work >= before.work,
                                "{} work reset",
                                case.name
                            );
                            assert!(
                                now.controller.scanned_bytes >= before.scanned_bytes,
                                "{} scan reset",
                                case.name
                            );
                            assert!(
                                now.controller.owned_bytes >= before.owned_bytes,
                                "{} owned reset",
                                case.name
                            );
                            assert!(
                                now.controller.reference_attempts >= before.reference_attempts,
                                "{} attempts reset",
                                case.name
                            );
                        }
                        previous = Some(now.controller);
                        if exact_shared_work
                            && matches!(event, ReaderEvent::Dtd(TakenDtdEvent::End))
                        {
                            assert_eq!(now.controller.work, 36, "{} DTD carry", case.name);
                        }
                    }
                    if case.source == VALUE && matches!(event, ReaderEvent::Xml(Event::Empty(_))) {
                        let ReaderEvent::Xml(Event::Empty(element)) = &event else {
                            unreachable!()
                        };
                        assert_eq!(element.attributes[0].value, "é");
                    }
                    if case.source == BODY_UNICODE
                        && let ReaderEvent::Xml(Event::Empty(element)) = &event
                    {
                        assert_eq!(element.attributes[0].value, "éé");
                    }
                    if case.source == SHARED && labels.len() == 4 {
                        let Phase::Declarations { controller, parser } =
                            reader.session.first_mut().unwrap()
                        else {
                            panic!("generated Item phase")
                        };
                        assert_eq!(controller.counters().owned_bytes, 10);
                        assert_eq!(parser.owned_budget().remaining_owned(), 0);
                    }
                }
                Ok(None) => {
                    assert!(
                        succeeds,
                        "{} unexpectedly accepted ceiling {ceiling}",
                        case.name
                    );
                    assert_eq!(labels, golden(case.source), "{}", case.name);
                    assert_eq!(reader.position(), case.source.len());
                    if case.source == HANDOFF {
                        assert_eq!(
                            counters(&reader).unwrap().frames.unwrap(),
                            FrameCounters {
                                reference_attempts: 2,
                                work: 18,
                                expanded_bytes: 16,
                            }
                        );
                    }
                    let state = counters(&reader);
                    if exact_shared_work {
                        assert_eq!(
                            state.unwrap().controller.work,
                            48,
                            "{} complete work",
                            case.name
                        );
                    }
                    let origin = reader.event_origin();
                    assert!(reader.read_event().unwrap().is_none());
                    assert_eq!(counters(&reader), state);
                    assert_eq!(reader.event_origin(), origin);
                    terminated = true;
                    break;
                }
                Err(error) => {
                    assert!(
                        !succeeds,
                        "{} control ceiling {ceiling}: {error:?}",
                        case.name
                    );
                    assert_eq!(Diagnostic::from_error(error), case.error, "{}", case.name);
                    assert_eq!(reader.position(), case.position, "{} high-water", case.name);
                    assert_eq!(
                        reader.event_origin(),
                        case.last,
                        "{} last origin",
                        case.name
                    );
                    assert_eq!(
                        reader.event_range(),
                        case.last.and_then(SourceOrigin::input_range)
                    );
                    let prefix = case.prefix.unwrap_or_else(|| match case.source {
                        TINY => {
                            if case.last.is_none() {
                                0
                            } else {
                                2
                            }
                        }
                        MODEL | ATTRS => 1,
                        VALUE => {
                            if case.last == last(0..13) {
                                1
                            } else {
                                3
                            }
                        }
                        GENERATED => 4,
                        REFERENCES | NESTED | HANDOFF => 5,
                        PARAMETER => 2,
                        _ => unreachable!(),
                    });
                    assert_eq!(
                        labels,
                        &golden(case.source)[..prefix],
                        "{} rejected-event prefix",
                        case.name
                    );
                    let state = counters(&reader);
                    if case.name == "scanner_work_generated" {
                        assert_eq!(state.unwrap().controller.scanned_bytes, 52);
                    }
                    if case.name == "scanner_work_after_generated" {
                        assert_eq!(state.unwrap().controller.scanned_bytes, 57);
                    }
                    if case.source == SHARED {
                        let Phase::Declarations { controller, parser } =
                            reader.session.first_mut().unwrap()
                        else {
                            panic!("rejected generated Item phase")
                        };
                        assert_eq!(controller.counters().owned_bytes, 0);
                        assert_eq!(parser.owned_budget().remaining_owned(), 9);
                    }
                    if exact_shared_work {
                        assert_eq!(
                            state.unwrap().controller.work,
                            44,
                            "{} rejected work was debited",
                            case.name
                        );
                    }
                    if case.name == "frame_work_handoff" {
                        assert_eq!(
                            state.unwrap().frames.unwrap(),
                            FrameCounters {
                                reference_attempts: 2,
                                work: 17,
                                expanded_bytes: 15,
                            }
                        );
                    }
                    if case.name == "frame_attempts_handoff" {
                        assert_eq!(
                            state.unwrap().frames.unwrap(),
                            FrameCounters {
                                reference_attempts: 1,
                                work: 16,
                                expanded_bytes: 15,
                            }
                        );
                    }
                    if case.source == REFERENCES {
                        let usage = state.unwrap().text.unwrap();
                        assert_eq!(
                            (
                                usage.reference_scanned,
                                usage.reference_copied,
                                usage.reference_classified
                            ),
                            (3, 3, 3)
                        );
                    }
                    for _ in 0..3 {
                        assert_eq!(reader.read_event().unwrap_err(), error);
                        assert_eq!(reader.position(), case.position);
                        assert_eq!(reader.event_origin(), case.last);
                        assert_eq!(
                            counters(&reader),
                            state,
                            "{} terminal work changed",
                            case.name
                        );
                    }
                    terminated = true;
                    break;
                }
            }
        }
        assert!(terminated, "{} failed to terminate", case.name);
        assert_eq!(reader.into_inner(), case.source.as_bytes());
    }
}

macro_rules! quota {
    ($name:ident, $source:expr, $low:expr => $high:expr, $configure:expr, $error:expr, $position:expr, $last:expr) => {
        #[test]
        fn $name() {
            verify(Case {
                name: stringify!($name),
                source: $source,
                bounds: ($low, $high),
                configure: $configure,
                public: |_, _, _| {},
                prefix: None,
                error: $error,
                position: $position,
                last: $last,
            });
        }
    };
}

quota!(scanner_unit, TINY, 11 => 12, |p,n| p.declarations.scan.max_unit_bytes=n,
    dtd(D::Scan(ScanErrorKind::Limit(ScanLimit::UnitBytes)),11,input(0..11)),11,None);
quota!(scanner_scratch, TINY, 11 => 12, |p,n| p.declarations.scan.max_scratch_bytes=n,
    dtd(D::Scan(ScanErrorKind::Limit(ScanLimit::ScratchBytes)),11,input(0..11)),11,None);
quota!(scanner_work, TINY, 11 => 12, |p,n| p.declarations.scan.max_work=n,
    dtd(D::Scan(ScanErrorKind::Limit(ScanLimit::Work)),11,input(0..11)),11,None);
quota!(unit_tokens, TINY, 0 => 1, |p,n| p.declarations.unit.max_tokens=n,
    dtd(D::Grammar(DtdErrorKind::LimitExceeded(DtdLimit::Tokens)),10,input(0..12)),12,None);
quota!(unit_model, MODEL, 0 => 1, |p,n| p.declarations.unit.max_model_depth=n,
    dtd(D::Grammar(DtdErrorKind::LimitExceeded(DtdLimit::ModelDepth)),25,input(13..29)),29,last(0..13));
quota!(unit_attributes, ATTRS, 0 => 1, |p,n| p.declarations.unit.max_attribute_definitions=n,
    dtd(D::Grammar(DtdErrorKind::LimitExceeded(DtdLimit::AttributeDefinitions)),25,input(13..42)),42,last(0..13));
quota!(catalog_storage, VALUE, 2 => 3, |p,n| p.declarations.catalog.max_storage_bytes=n,
    dtd(D::Catalog(CatalogErrorKind::Limit(CatalogLimit::StorageBytes)),13,input(13..29)).value(Site::Declaration(1,0)),29,last(0..13));
quota!(catalog_value, VALUE, 1 => 2, |p,n| p.declarations.catalog.max_value_bytes=n,
    dtd(D::Catalog(CatalogErrorKind::Limit(CatalogLimit::ValueBytes)),25,input(13..29)).value(Site::Declaration(1,12)),29,last(0..13));
quota!(catalog_expanded_handoff, VALUE, 3 => 4, |p,n| p.declarations.catalog.max_expanded_bytes=n,
    content(C::Tag(crate::dtd_tag::DtdTagErrorKind::Catalog(CatalogErrorKind::Limit(CatalogLimit::ExpandedBytes))),31,input(31..43)).value(Site::Replacement(0,0)),43,last(29..31));
quota!(catalog_work, TINY, 0 => 1000, |p,n| p.declarations.catalog.max_work=n,
    dtd(D::Catalog(CatalogErrorKind::Limit(CatalogLimit::Work)),0,input(0..12)).value(Site::Declaration(0,0)),12,None);
quota!(frame_depth, NESTED, 1 => 2, |p,n| p.declarations.frames.max_active_depth=n,
    content(C::Frame(FrameErrorKind::LimitExceeded(FrameLimit::ActiveDepth)),50,replacement(0,1,0..3,50..53)).local(0),53,last(47..50));
quota!(frame_work, GENERATED, 4 => 5, |p,n| p.declarations.frames.max_work=n,
    content(C::Frame(FrameErrorKind::LimitExceeded(FrameLimit::Work)),36,replacement(0,1,0..4,36..39)).local(0),39,last(33..36));
quota!(frame_work_handoff, HANDOFF, 17 => 18, |p,n| p.declarations.frames.max_work=n,
    content(C::Frame(FrameErrorKind::LimitExceeded(FrameLimit::Work)),52,replacement(1,2,0..1,52..55)).local(0),55,last(49..52));
quota!(text_global_work, TINY, 0 => 1000, |p,n| p.text.max_work=n,
    content(C::Text(T::Work),12,None),12,last(11..12));
quota!(reference_bytes, GENERATED, 2 => 3, |p,n| p.text.reference.max_bytes=n,
    content(C::Text(T::Reference(ReferenceErrorKind::Limit(ReferenceLimit::Bytes))),38,None),38,last(33..36));
quota!(reference_scratch, GENERATED, 2 => 3, |p,n| p.text.reference.max_scratch_bytes=n,
    content(C::Text(T::Reference(ReferenceErrorKind::Limit(ReferenceLimit::ScratchBytes))),38,None),38,last(33..36));
quota!(reference_work, GENERATED, 8 => 9, |p,n| p.text.reference.max_work=n,
    content(C::Text(T::Reference(ReferenceErrorKind::Limit(ReferenceLimit::Work))),36,None),39,last(33..36));
quota!(reference_work_cumulative, REFERENCES, 9 => 18, |p,n| p.text.reference.max_work=n,
    content(C::Text(T::Reference(ReferenceErrorKind::Limit(ReferenceLimit::Work))),36,None),36,replacement(0,1,0..1,33..36).map(SourceOrigin::from_repr));
quota!(aggregate_scratch, TINY, 34 => 35, |p,n| {
    p.text.reference.max_bytes=3; p.text.reference.max_scratch_bytes=3; p.text.scratch.max_bytes=n;
},content(C::Text(T::Scratch),12,None),12,last(11..12));
quota!(input_scratch_slot, TINY, 15 => 16, |p,n| p.text.scratch.input_token_bytes=n,
    content(C::Text(T::Scratch),12,None),12,last(11..12));
quota!(replacement_scratch_slot, TINY, 15 => 16, |p,n| p.text.scratch.replacement_token_bytes=n,
    content(C::Text(T::Scratch),12,None),12,last(11..12));
quota!(local_token, GENERATED, 3 => 4, |p,n| p.local.max_token_bytes=n,
    content(C::Text(T::Local(LocalErrorKind::Xml(ErrorKind::LimitExceeded(Limit::TokenBytes)))),36,replacement(0,1,0..0,36..39)).local(0),39,last(33..36));
quota!(local_scratch, GENERATED, 3 => 4, |p,n| p.local.max_scratch_bytes=n,
    content(C::Text(T::Local(LocalErrorKind::Xml(ErrorKind::LimitExceeded(Limit::DecodedTokenBytes)))),36,replacement(0,1,0..0,36..39)).local(0),39,last(33..36));
quota!(local_work, GENERATED, 7 => 8, |p,n| p.local.max_work=n,
    content(C::Text(T::Local(LocalErrorKind::Work)),36,replacement(0,1,0..0,36..39)).local(0),39,last(33..36));
quota!(declaration_owned, PARAMETER, 9 => 10, |p,n| p.declarations.max_owned_bytes=n,
    dtd(D::Limit(ControllerLimit::OwnedBytes),43,replacement(0,1,0..10,43..46)).local(0),46,last(13..43));
quota!(declaration_work, TINY, 0 => 1000, |p,n| p.declarations.max_work=n,
    dtd(D::Limit(ControllerLimit::Work),0,None),0,None);

quota!(unit_bytes, TINY, 11 => 12, |p,n| p.declarations.unit.max_unit_bytes=n,
    dtd(D::Grammar(DtdErrorKind::LimitExceeded(DtdLimit::UnitBytes)),11,input(0..12)),12,None);
quota!(catalog_declarations, TINY, 0 => 1, |p,n| p.declarations.catalog.max_declarations=n,
    dtd(D::Catalog(CatalogErrorKind::Limit(CatalogLimit::Declarations)),0,input(0..12)).value(Site::Declaration(0,0)),12,None);
quota!(catalog_bindings, VALUE, 0 => 1, |p,n| p.declarations.catalog.max_bindings=n,
    dtd(D::Catalog(CatalogErrorKind::Limit(CatalogLimit::Bindings)),13,input(13..29)).value(Site::Declaration(1,0)),29,last(0..13));
quota!(catalog_references, VALUE, 0 => 1, |p,n| p.declarations.catalog.max_references=n,
    content(C::Tag(crate::dtd_tag::DtdTagErrorKind::Catalog(CatalogErrorKind::Limit(CatalogLimit::References))),37,input(31..43)).value(Site::Input(37)),43,last(29..31));
quota!(catalog_depth, VALUE, 0 => 1, |p,n| p.declarations.catalog.max_depth=n,
    content(C::Tag(crate::dtd_tag::DtdTagErrorKind::Catalog(CatalogErrorKind::Limit(CatalogLimit::Depth))),37,input(31..43)).value(Site::Input(37)),43,last(29..31));
quota!(frame_attempts, GENERATED, 0 => 1, |p,n| p.declarations.frames.max_reference_attempts=n,
    content(C::Frame(FrameErrorKind::LimitExceeded(FrameLimit::ReferenceAttempts)),36,input(36..39)),39,last(33..36));
quota!(declaration_physical, TINY, 11 => 16, |p,n| p.declarations.max_physical_bytes=n,
    dtd(D::Limit(ControllerLimit::PhysicalBytes),11,None),11,None);
quota!(body_physical, TINY, 15 => 16, |p,n| p.declarations.max_physical_bytes=n,
    content(C::PhysicalBytes,15,None),15,last(11..12));
quota!(catalog_work_exact, TINY, 11 => 12, |p,n| p.declarations.catalog.max_work=n,
    dtd(D::Catalog(CatalogErrorKind::Limit(CatalogLimit::Work)),0,input(0..12)).value(Site::Declaration(0,0)),12,None);
quota!(declaration_work_exact, TINY, 47 => 48, |p,n| p.declarations.max_work=n,
    content(C::Work,12,None),16,last(11..12));
quota!(text_work_exact, TINY, 47 => 48, |p,n| p.text.max_work=n,
    content(C::Work,12,None),16,last(11..12));

// The 16-byte body/local token profiles are intentionally independent of the
// larger declaration scanner units.
macro_rules! interaction {
    ($name:ident, $source:expr, $low:expr => $high:expr, $private:expr, $public:expr,
     $error:expr, $position:expr, $last:expr, $prefix:expr) => {
        #[test]
        fn $name() {
            verify(Case {
                name: stringify!($name),
                source: $source,
                bounds: ($low, $high),
                configure: $private,
                public: $public,
                prefix: Some($prefix),
                error: $error,
                position: $position,
                last: $last,
            });
        }
    };
}

interaction!(body_raw_token, BODY_UNICODE, 12 => 13, |_,_| {},
    |l,_,n| l.max_token_bytes=n,
    content(C::Text(T::Xml(ErrorKind::LimitExceeded(Limit::TokenBytes))),12,None),25,last(11..12),2);
interaction!(body_decoded_token, BODY_UNICODE, 12 => 13, |_,_| {},
    |_,d,n| d.max_decoded_token_bytes=n,
    content(C::Text(T::Xml(ErrorKind::LimitExceeded(Limit::DecodedTokenBytes))),12,None),25,last(11..12),2);
interaction!(body_token_tie, BODY_UNICODE, 12 => 13, |_,_| {},
    |l,d,n| {l.max_token_bytes=n; d.max_decoded_token_bytes=n;},
    content(C::Text(T::Xml(ErrorKind::LimitExceeded(Limit::TokenBytes))),12,None),25,last(11..12),2);
interaction!(generated_parser_depth, DEPTH, 2 => 3, |_,_| {},
    |l,_,n| l.max_depth=n,
    content(C::Tag(crate::dtd_tag::DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(Limit::Depth))),43,replacement(0,1,3..7,43..46)).local(3),46,
    replacement(0,1,0..3,43..46).map(SourceOrigin::from_repr),5);
interaction!(scanner_work_generated, PARAMETER, 52 => 58,
    |p,n| p.declarations.scan.max_work=n, |_,_,_| {},
    dtd(D::Scan(ScanErrorKind::Limit(ScanLimit::Work)),43,replacement(0,1,0..6,43..46)).local(6),46,last(13..43),2);
interaction!(scanner_work_after_generated, PARAMETER, 57 => 58,
    |p,n| p.declarations.scan.max_work=n, |_,_,_| {},
    dtd(D::Scan(ScanErrorKind::Limit(ScanLimit::Work)),47,input(46..47)),47,
    replacement(0,1,0..10,43..46).map(SourceOrigin::from_repr),3);
interaction!(local_named_reference, LOCAL_NAMED, 6 => 7,
    |p,n| p.text.reference.max_bytes=n, |_,_,_| {},
    content(C::Text(T::Reference(ReferenceErrorKind::Limit(ReferenceLimit::Bytes))),62,replacement(0,1,6..6,62..65)).local(6),65,last(59..62),5);
interaction!(local_numeric_reference, LOCAL_NUMERIC, 4 => 5,
    |p,n| p.text.reference.max_bytes=n, |_,_,_| {},
    content(C::Text(T::Reference(ReferenceErrorKind::Limit(ReferenceLimit::Bytes))),41,replacement(0,1,4..4,41..44)).local(4),44,last(38..41),4);
interaction!(numeric_reference_keeps_text_token, NUMERIC_TEXT, 16 => 21,
    |p,_| {p.text.scratch.input_token_bytes=21; p.text.scratch.max_bytes=53;},
    |l,_,n| l.max_token_bytes=n,
    content(C::Text(T::Xml(ErrorKind::LimitExceeded(Limit::TokenBytes))),33,None),54,last(30..33),4);
interaction!(reference_min_bytes, TINY, 34 => 35,
    |p,n| {p.text.reference.max_bytes=7; p.text.reference.max_scratch_bytes=3; p.text.scratch.max_bytes=n;}, |_,_,_| {},
    content(C::Text(T::Scratch),12,None),12,last(11..12),2);
interaction!(reference_min_scratch, TINY, 34 => 35,
    |p,n| {p.text.reference.max_bytes=3; p.text.reference.max_scratch_bytes=7; p.text.scratch.max_bytes=n;}, |_,_,_| {},
    content(C::Text(T::Scratch),12,None),12,last(11..12),2);
interaction!(scratch_input_overflow, TINY, usize::MAX => 16,
    |p,n| {p.text.scratch.input_token_bytes=n; p.text.scratch.max_bytes=usize::MAX;}, |_,_,_| {},
    content(C::Text(T::ScratchOverflow),12,None),12,last(11..12),2);
interaction!(scratch_replacement_overflow, TINY, usize::MAX => 16,
    |p,n| {p.text.scratch.replacement_token_bytes=n; p.text.scratch.max_bytes=usize::MAX;}, |_,_,_| {},
    content(C::Text(T::ScratchOverflow),12,None),12,last(11..12),2);
interaction!(scratch_reference_overflow, TINY, usize::MAX => 16,
    |p,n| {p.text.reference.max_bytes=n; p.text.reference.max_scratch_bytes=n; p.text.scratch.max_bytes=usize::MAX;}, |_,_,_| {},
    content(C::Text(T::ScratchOverflow),12,None),12,last(11..12),2);
interaction!(text_work_precedes_overflow, TINY, 0 => 64*1024*1024,
    |p,n| {p.text.max_work=n; if n==0 {p.text.scratch.input_token_bytes=usize::MAX;}}, |_,_,_| {},
    content(C::Text(T::Work),12,None),12,last(11..12),2);
interaction!(catalog_global_tie, TINY, 35 => 48,
    |p,n| {p.declarations.max_work=n; p.declarations.catalog.max_work=if n==35 {11} else {12};}, |_,_,_| {},
    dtd(D::Limit(ControllerLimit::Work),0,input(0..12)).value(Site::Declaration(0,0)),12,None,0);
interaction!(catalog_native_collision, TINY, 11 => 12,
    |p,n| {p.declarations.catalog.max_work=n; p.declarations.max_work=if n==11 {36} else {48};}, |_,_,_| {},
    dtd(D::Catalog(CatalogErrorKind::Limit(CatalogLimit::Work)),0,input(0..12)).value(Site::Declaration(0,0)),12,None,0);
interaction!(shared_parser_promotion, SHARED, 12 => 13, |_,_| {},
    |l,_,n| l.max_owned_bytes=n,
    dtd(D::Ownership(ErrorKind::LimitExceeded(Limit::OwnedBytes)),52,replacement(0,1,0..10,52..55)).local(0),55,last(22..52),3);
interaction!(catalog_declarations_cumulative, VALUE, 1 => 2,
    |p,n| p.declarations.catalog.max_declarations=n, |_,_,_| {},
    dtd(D::Catalog(CatalogErrorKind::Limit(CatalogLimit::Declarations)),13,input(13..29)).value(Site::Declaration(1,0)),29,last(0..13),1);
interaction!(frame_attempts_handoff, HANDOFF, 1 => 2,
    |p,n| p.declarations.frames.max_reference_attempts=n, |_,_,_| {},
    content(C::Frame(FrameErrorKind::LimitExceeded(FrameLimit::ReferenceAttempts)),52,input(52..55)),55,last(49..52),5);

#[test]
fn general_reference_splits_a_full_physical_text_token() {
    let mut reader = DtdReader::with_decode_limits(
        GENERAL_TEXT.as_bytes(),
        Limits {
            max_token_bytes: 16,
            ..Limits::default()
        },
        DecodeLimits {
            max_decoded_token_bytes: 16,
            ..DecodeLimits::default()
        },
        policy(),
    )
    .unwrap();
    let mut labels = Vec::new();
    let mut text = Vec::new();
    while let Some((event, origin)) = reader.read_event().unwrap() {
        if let ReaderEvent::Xml(Event::Text(value)) = &event {
            text.push((value.to_string(), origin));
        }
        labels.push(label(&event));
    }
    assert_eq!(labels, golden(GENERAL_TEXT));
    assert_eq!(
        text,
        vec![
            ("1234567890123456".to_owned(), SourceOrigin::input(33..49)),
            (
                "x".to_owned(),
                SourceOrigin::from_repr(replacement(0, 1, 0..1, 49..52).unwrap())
            )
        ]
    );
    assert_eq!(reader.position(), 56);
    assert!(reader.read_event().unwrap().is_none());
}

#[test]
fn bom_constructor_bytes_precede_lazy_session_validation() {
    const BOM: &[u8] = b"\xef\xbb\xbf<!DOCTYPE r><r/>";
    for bad_session in [false, true] {
        for (raw, decoded, expected) in [
            (18, 15, Some(Limit::InputBytes)),
            (19, 15, Some(Limit::DecodedBytes)),
            (19, 16, None),
        ] {
            let mut p = policy();
            if bad_session {
                p.declarations.scan.max_work = 0;
            }
            let result = DtdReader::with_decode_limits(
                BOM,
                Limits {
                    max_input_bytes: raw,
                    max_token_bytes: 16,
                    ..Limits::default()
                },
                DecodeLimits {
                    max_decoded_bytes: decoded,
                    ..DecodeLimits::default()
                },
                p,
            );
            if let Some(limit) = expected {
                assert!(
                    matches!(result,Err(SessionError::Xml(e)) if e.kind==ErrorKind::LimitExceeded(limit) && e.offset==0)
                );
                continue;
            }
            let mut reader = result.unwrap();
            assert_eq!(reader.position(), 3);
            if bad_session {
                let error = reader.read_event().unwrap_err();
                assert_eq!(
                    Diagnostic::from_error(error),
                    dtd(D::Scan(ScanErrorKind::Limit(ScanLimit::Work)), 3, None)
                );
                assert_eq!(reader.position(), 3);
                assert_eq!(reader.event_origin(), None);
                for _ in 0..3 {
                    assert_eq!(reader.read_event().unwrap_err(), error);
                }
            } else {
                let mut observed = Vec::new();
                while let Some((event, origin)) = reader.read_event().unwrap() {
                    observed.push((label(&event), origin));
                }
                assert_eq!(
                    observed,
                    vec![
                        ("begin".to_owned(), SourceOrigin::input(3..14)),
                        ("end".to_owned(), SourceOrigin::input(14..15)),
                        ("=r".to_owned(), SourceOrigin::input(15..19))
                    ]
                );
                assert_eq!(reader.position(), 19);
                assert!(reader.read_event().unwrap().is_none());
            }
            assert_eq!(reader.into_inner(), BOM);
        }
    }
}

#[test]
fn independent_stable_text_token_limit_retains_the_complete_scanned_tag() {
    use crate::tokenizer::Tokenizer;
    use crate::tokenizer::text::{TextCoordinator, TextShared, TextSourceLease};
    for decoded in [false, true] {
        let tokenizer =
            Tokenizer::new(if decoded { 16 } else { 12 }, if decoded { 12 } else { 16 });
        let source = TextSourceLease::resume_stable_utf8(tokenizer, BODY_UNICODE, 12, 12).unwrap();
        let mut text =
            TextCoordinator::new(TextShared::new(policy().text, 0).unwrap(), source).unwrap();
        let error = text.advance_borrowed().unwrap_err();
        assert_eq!(
            error.kind,
            T::Xml(ErrorKind::LimitExceeded(if decoded {
                Limit::DecodedTokenBytes
            } else {
                Limit::TokenBytes
            }))
        );
        assert_eq!(error.offset, 12);
        assert_eq!(text.input_position(), Some((25, 25)));
        assert_eq!(text.advance_borrowed().unwrap_err(), error);
    }
}

#[test]
fn independent_scanner_header_keeps_its_physical_delimiter_before_publication() {
    use crate::dtd_scan::{DtdScanner, ScanKind, ScanMode};
    let mut scanner = DtdScanner::new(ScanMode::Doctype, policy().declarations.scan);
    assert_eq!(scanner.advance(TINY.as_bytes()).unwrap(), 12);
    let (kind, raw, range) = scanner.current().unwrap();
    assert_eq!(kind, ScanKind::Header);
    assert_eq!(raw, b"<!DOCTYPE r>");
    assert_eq!(range, 0..12);
    assert_eq!(scanner.position(), 12);

    let mut limits = policy().declarations.scan;
    limits.max_work = 0;
    let mut scanner = DtdScanner::new(ScanMode::Doctype, limits);
    let error = scanner.advance(TINY.as_bytes()).unwrap_err();
    assert_eq!(error.kind, ScanErrorKind::Limit(ScanLimit::Work));
    assert_eq!(error.offset, 0);
    assert_eq!(scanner.position(), 0);
    assert!(scanner.current().is_none());
}

#[test]
fn independent_content_work_rejection_has_an_anchor_before_tag_publication() {
    use crate::dtd_controller::DtdController;
    use crate::dtd_controller::content::{BorrowedContentController, ContentOutcome};
    use crate::tokenizer::Tokenizer;
    for text_ceiling in [false, true] {
        let mut p = policy();
        if text_ceiling {
            p.text.max_work = 47;
        } else {
            p.declarations.max_work = 47;
        }
        let mut parser = ParserState::new(
            Limits {
                max_token_bytes: 16,
                ..Limits::default()
            },
            0,
        );
        let mut declarations =
            DtdController::stable_utf8(TINY, 0, 0, p.declarations, false).unwrap();
        for _ in 0..2 {
            declarations
                .advance_borrowed(parser.owned_budget())
                .unwrap();
            assert!(declarations.take_current().is_some());
        }
        assert!(declarations.is_complete());
        assert_eq!(declarations.counters().work, 36);
        let mut body = BorrowedContentController::stable_utf8(
            declarations.into_body_state().unwrap(),
            parser,
            Tokenizer::new(16, 16),
            TINY,
            p.text,
            p.local,
        )
        .unwrap();
        assert_eq!(
            body.advance_borrowed().unwrap().outcome,
            ContentOutcome::EventReady
        );
        assert_eq!(body.counters().work, 44);
        let error = body.take_event().unwrap_err();
        assert_eq!(error.kind, C::Work);
        assert_eq!(error.physical_offset, 12);
        assert_eq!(error.origin, None);
        assert_eq!(error.local_offset, None);
        assert_eq!(error.value_location, None);
        assert_eq!(body.position(), (16, 16));
        assert_eq!(body.counters().work, 44);
        assert_eq!(body.advance_borrowed().unwrap_err(), error);
    }
}

#[test]
fn independent_reference_limits_stop_at_the_first_unaccepted_source_byte() {
    use crate::tokenizer::Tokenizer;
    use crate::tokenizer::text::{TextCoordinator, TextOutcome, TextShared, TextSourceLease};

    for scratch in [false, true] {
        let mut p = policy().text;
        if scratch {
            p.reference.max_scratch_bytes = 2;
        } else {
            p.reference.max_bytes = 2;
        }
        let mut text = TextCoordinator::new(
            TextShared::new(p, 0).unwrap(),
            TextSourceLease::streaming(Tokenizer::new(16, 16)),
        )
        .unwrap();
        let error = text.advance(b"&e;").unwrap_err();
        assert_eq!(
            error.kind,
            T::Reference(ReferenceErrorKind::Limit(if scratch {
                ReferenceLimit::ScratchBytes
            } else {
                ReferenceLimit::Bytes
            }))
        );
        assert_eq!(error.offset, 2);
        assert_eq!(text.input_position(), Some((2, 2)));
    }

    let mut p = policy().text;
    p.reference.max_work = 9;
    let mut text = TextCoordinator::new(
        TextShared::new(p, 0).unwrap(),
        TextSourceLease::streaming(Tokenizer::new(16, 16)),
    )
    .unwrap();
    let first = text.advance(b"&e;").unwrap();
    assert_eq!(first.outcome, TextOutcome::GeneralReferenceReady);
    assert_eq!(first.consumed, 3);
    let (shared, source) = text.acknowledge_general().unwrap().into_parts();
    let mut text = TextCoordinator::new(shared, source).unwrap();
    let error = text.advance(b"&e;").unwrap_err();
    assert_eq!(
        error.kind,
        T::Reference(ReferenceErrorKind::Limit(ReferenceLimit::Work))
    );
    assert_eq!(error.offset, 3);
    assert_eq!(text.input_position(), Some((3, 3)));
}
