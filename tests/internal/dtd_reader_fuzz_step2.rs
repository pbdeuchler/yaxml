//! Frozen step-2 selector probes for truncation, quotas, and lifecycle.

use super::*;
use crate::DecodeLimits;
use crate::dtd::{DtdErrorKind, DtdLimit};
use crate::dtd_catalog::DependencyKind;
use crate::dtd_catalog::{CatalogErrorKind, CatalogLimit, ValueLocation};
use crate::dtd_controller::content::ContentErrorKind;
use crate::dtd_controller::{ControllerErrorKind, ControllerLimit};
use crate::dtd_frames::{FrameErrorKind, FrameLimit};
use crate::dtd_reference::{ReferenceErrorKind, ReferenceLimit};
use crate::dtd_scan::{ScanErrorKind, ScanLimit};
use crate::dtd_tag::DtdTagErrorKind;
use crate::tokenizer::local::LocalErrorKind;
use crate::tokenizer::text::TextErrorKind;
use std::cell::Cell;

const T: &[u8] = b"<!DOCTYPE r><r/>";
const B: &[u8] = b"\xef\xbb\xbf<!DOCTYPE r><r/>";
const U: &[u8] = "<!DOCTYPE r><r a='éé'/>".as_bytes();
const M: &[u8] = b"<!DOCTYPE r [<!ELEMENT r (a)>]><r/>";
const A: &[u8] = b"<!DOCTYPE r [<!ATTLIST r a CDATA #IMPLIED>]><r/>";
const V: &[u8] = "<!DOCTYPE r [<!ENTITY e 'é'>]><r a='&e;'/>".as_bytes();
const F: &[u8] = b"<!DOCTYPE r [<!ENTITY e 'x'>]><r>&e;</r>";
const G: &[u8] = b"<!DOCTYPE r [<!ENTITY e '<n/>'>]><r>&e;</r>";
const D: &[u8] = b"<!DOCTYPE r [<!ATTLIST r a CDATA 'v'>]><r/>";
const NS: &[u8] = b"<!DOCTYPE r [<!ATTLIST r xmlns CDATA 'urn:d' a CDATA 'v'>]><r/>";
const H: &[u8] = b"<!DOCTYPE r [<!ENTITY % p \"<!ENTITY e 'x'>\">%p;]><r>&e;</r>";
const EMPTY: &[u8] = b"<!DOCTYPE r [<!ENTITY e '&z;'><!ENTITY z ''>]><r>&e;</r>";
const VISIBLE_EMPTY: &[u8] = b"<!DOCTYPE r [<!ENTITY e 'x'><!ENTITY z ''>]><r>&e;&z;&e;</r>";
const REPEATED: &[u8] = "<!DOCTYPE r><r><n a='é'/>x<n a='é'/>x</r>".as_bytes();
const RP: &[u8] = b"<!DOCTYPE r [<!ENTITY % p '<!--x-->'>%p;%p;]><r/>";
const E: &[u8] = b"<!DOCTYPE r><r a='v'/>";
const AMP: &[u8] = b"<!DOCTYPE r><r a='&amp;'/>";
const RL: &[u8] = b"<!DOCTYPE r [<!ENTITY e '<n/>'>]><r>&e;&e;</r>";
const DOC: &[u8] = b"<!DOCTYPE r [<!ENTITY e '<n/>x'><!ENTITY z ''>]><r>a&e;b&z;c</r>";
const S: &[u8] = b"<?p a\r\n?><!DOCTYPE r [<!ENTITY % p '<!--a&#13;b-->'>%p;]><r/>";
const PE_BAD: &[u8] = b"<!DOCTYPE r [<!ENTITY % p '<!ELEMENT r>'>%p;]><r/>";
const PE_MODEL: &[u8] = b"<!DOCTYPE r [<!ENTITY % p '<!ELEMENT r (a)>'>%p;]><r/>";
const UNKNOWN: &[u8] = b"<!DOCTYPE r><r a='v&z;'/>";
const UNAVAILABLE: &[u8] = b"<!DOCTYPE r SYSTEM 'x'><r a='v&z;'/>";
pub(super) const MAX_RETAINED_OWNED: usize = 16 * 1024;
pub(super) const MAX_RETAINED_OBJECTS: usize = 128;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct RoutingReceipt {
    pub(super) witness_lifecycle: Option<u8>,
    pub(super) truncation: Option<(u8, usize, usize, u8)>,
    pub(super) quota: Option<(u8, usize, usize, usize, u8)>,
    pub(super) configured_early_stop: usize,
}

thread_local! {
    static ROUTING_RECEIPT: Cell<RoutingReceipt> = const { Cell::new(RoutingReceipt {
        witness_lifecycle: None,
        truncation: None,
        quota: None,
        configured_early_stop: 0,
    }) };
}

pub(super) fn reset_routing_receipt() {
    ROUTING_RECEIPT.set(RoutingReceipt::default());
}

pub(super) fn routing_receipt() -> RoutingReceipt {
    ROUTING_RECEIPT.get()
}

fn update_receipt(update: impl FnOnce(&mut RoutingReceipt)) {
    ROUTING_RECEIPT.set({
        let mut receipt = ROUTING_RECEIPT.get();
        update(&mut receipt);
        receipt
    });
}

const UTF8_TEXT: &[u8] = "<!DOCTYPE r><r>é</r>".as_bytes();
const EMOJI_TEXT: &[u8] = "<!DOCTYPE r><r>🙂</r>".as_bytes();
const CHAR_REF: &[u8] = b"<!DOCTYPE r><r>&#65;</r>";
const LEAVES: &[u8] = b"<!DOCTYPE r><r><!--x--><?p x?><![CDATA[x]]><n/></r>";

const TRUNCATION_CUTS: [&[usize]; 8] = [
    &[16, 0],
    &[11, 12, 15, 16],
    &[15, 16, 17, 21],
    &[15, 16, 17, 18, 19, 23],
    &[33, 34, 35, 36, 40],
    &[13, 25, 26, 27, 28, 29, 30, 40],
    &[22, 23, 29, 30, 42, 43, 47, 51],
    &[16, 17, 18, 19, 20, 24],
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalClass {
    Complete,
    ConstructorXml,
    Xml,
    Dtd,
    Content,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TruncationCause {
    MissingRoot,
    ParserEof,
    ScanEof,
    TextEof,
    ReferenceEof,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TruncationExpected {
    class: TerminalClass,
    cause: Option<TruncationCause>,
    diagnostic: Option<QuotaDiagnostic>,
    delivered: usize,
    error_offset: usize,
    position: usize,
    last: Option<SourceOrigin>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TruncationObservation {
    class: TerminalClass,
    diagnostic: Option<QuotaDiagnostic>,
    delivered: usize,
    position: usize,
    last: Option<SourceOrigin>,
}

fn check_truncation_observation(
    actual: TruncationObservation,
    expected: TruncationExpected,
) -> Result<(), &'static str> {
    if actual.class != expected.class {
        return Err("truncation terminal class");
    }
    if actual.diagnostic != expected.diagnostic {
        return Err("truncation diagnostic");
    }
    if actual.delivered != expected.delivered {
        return Err("truncation delivered prefix");
    }
    if actual.position != expected.position {
        return Err("truncation terminal position");
    }
    if actual.last != expected.last {
        return Err("truncation last origin");
    }
    Ok(())
}

fn input_origin(range: Range<usize>) -> Option<SourceOrigin> {
    Some(SourceOrigin::input(range))
}

fn replacement_origin(
    declaration: usize,
    invocation: u64,
    local: Range<usize>,
    physical: Range<usize>,
) -> SourceOrigin {
    use crate::origin::{ByteSpan, EntityDeclarationId, EntityInvocationId, OriginRepr};
    use std::num::NonZeroU64;

    SourceOrigin::from_repr(OriginRepr::Replacement {
        declaration: EntityDeclarationId::from_index(declaration),
        invocation: EntityInvocationId(NonZeroU64::new(invocation).unwrap()),
        local: ByteSpan::new(local).unwrap(),
        physical_reference: ByteSpan::new(physical).unwrap(),
    })
}

fn truncation_fixture(family: usize) -> &'static [u8] {
    match family {
        0 | 1 => T,
        2 => UTF8_TEXT,
        3 => EMOJI_TEXT,
        4 | 5 => F,
        6 => LEAVES,
        7 => CHAR_REF,
        _ => unreachable!(),
    }
}

fn truncation_selection(selector: u8) -> (usize, usize, usize) {
    let family = usize::from(selector & 7);
    let cuts = TRUNCATION_CUTS[family];
    let point = usize::from(selector >> 3) % cuts.len();
    (family, point, cuts[point])
}

fn quota_selection(axis: u8, mode: u8, shape: u8) -> (usize, usize, usize) {
    (
        usize::from(axis % 48),
        usize::from(mode & 3),
        usize::from(shape % 8),
    )
}

fn expected_truncation(family: usize, cut: usize, full: usize) -> TruncationExpected {
    if cut == full {
        return TruncationExpected {
            class: TerminalClass::Complete,
            cause: None,
            diagnostic: None,
            delivered: canonical_expected(truncation_fixture(family)).len(),
            error_offset: full,
            position: full,
            last: canonical_expected(truncation_fixture(family))
                .last()
                .map(|event| event.origin.full),
        };
    }
    if matches!((family, cut), (2, 16) | (3, 16..=18)) {
        return TruncationExpected {
            class: TerminalClass::ConstructorXml,
            cause: None,
            diagnostic: None,
            delivered: 0,
            error_offset: 15,
            position: 15,
            last: None,
        };
    }
    let (class, cause, error_offset, position, last) = match (family, cut) {
        (0, 0) => (TerminalClass::Xml, TruncationCause::MissingRoot, 0, 0, None),
        (1, 11) => (TerminalClass::Dtd, TruncationCause::ScanEof, 11, 11, None),
        (1, 12) => (
            TerminalClass::Content,
            TruncationCause::MissingRoot,
            12,
            12,
            input_origin(11..12),
        ),
        (1, 15) => (
            TerminalClass::Content,
            TruncationCause::TextEof,
            12,
            15,
            input_origin(11..12),
        ),
        (2 | 3, 15) => (
            TerminalClass::Content,
            TruncationCause::ParserEof,
            15,
            15,
            input_origin(12..15),
        ),
        (2, 17) => (
            TerminalClass::Content,
            TruncationCause::ParserEof,
            17,
            17,
            input_origin(15..17),
        ),
        (3, 19) => (
            TerminalClass::Content,
            TruncationCause::ParserEof,
            19,
            19,
            input_origin(15..19),
        ),
        (4, 33) => (
            TerminalClass::Content,
            TruncationCause::ParserEof,
            33,
            33,
            input_origin(30..33),
        ),
        (4, 34 | 35) => (
            TerminalClass::Content,
            TruncationCause::ReferenceEof,
            cut,
            cut,
            input_origin(30..33),
        ),
        (4, 36) => (
            TerminalClass::Content,
            TruncationCause::ParserEof,
            36,
            36,
            Some(replacement_origin(0, 1, 0..1, 33..36)),
        ),
        (5, 13) => (
            TerminalClass::Dtd,
            TruncationCause::ScanEof,
            13,
            13,
            input_origin(0..13),
        ),
        (5, 25..=27) => (
            TerminalClass::Dtd,
            TruncationCause::ScanEof,
            cut,
            cut,
            input_origin(0..13),
        ),
        (5, 28 | 29) => (
            TerminalClass::Dtd,
            TruncationCause::ScanEof,
            cut,
            cut,
            input_origin(13..28),
        ),
        (5, 30) => (
            TerminalClass::Content,
            TruncationCause::MissingRoot,
            30,
            30,
            input_origin(28..30),
        ),
        (6, 22) => (
            TerminalClass::Content,
            TruncationCause::TextEof,
            15,
            22,
            input_origin(12..15),
        ),
        (6, 23) => (
            TerminalClass::Content,
            TruncationCause::ParserEof,
            23,
            23,
            input_origin(15..23),
        ),
        (6, 29) => (
            TerminalClass::Content,
            TruncationCause::TextEof,
            23,
            29,
            input_origin(15..23),
        ),
        (6, 30) => (
            TerminalClass::Content,
            TruncationCause::ParserEof,
            30,
            30,
            input_origin(23..30),
        ),
        (6, 42) => (
            TerminalClass::Content,
            TruncationCause::TextEof,
            30,
            42,
            input_origin(23..30),
        ),
        (6, 43) => (
            TerminalClass::Content,
            TruncationCause::ParserEof,
            43,
            43,
            input_origin(30..43),
        ),
        (6, 47) => (
            TerminalClass::Content,
            TruncationCause::ParserEof,
            47,
            47,
            input_origin(43..47),
        ),
        (7, 16..=19) => (
            TerminalClass::Content,
            TruncationCause::ReferenceEof,
            cut,
            cut,
            input_origin(12..15),
        ),
        (7, 20) => (
            TerminalClass::Content,
            TruncationCause::ParserEof,
            20,
            20,
            input_origin(15..20),
        ),
        _ => unreachable!(),
    };
    let diagnostic = Some(match (class, cause) {
        (TerminalClass::Xml, TruncationCause::MissingRoot) => {
            qdiag(QuotaKind::Xml(ErrorKind::MissingRoot), error_offset)
        }
        (TerminalClass::Dtd, TruncationCause::ScanEof) => {
            let origin = match (family, cut) {
                (1, 11) => input_fragment(0..11),
                (5, 25..=27) => input_fragment(13..cut),
                (5, 29) => input_fragment(28..29),
                _ => None,
            };
            qdiag(
                QuotaKind::Dtd(ControllerErrorKind::Scan(ScanErrorKind::UnexpectedEof)),
                error_offset,
            )
            .origin(origin)
        }
        (TerminalClass::Content, TruncationCause::MissingRoot) => qdiag(
            QuotaKind::Content(ContentErrorKind::Parser(ErrorKind::MissingRoot)),
            error_offset,
        ),
        (TerminalClass::Content, TruncationCause::ParserEof) => qdiag(
            QuotaKind::Content(ContentErrorKind::Parser(ErrorKind::UnexpectedEof)),
            error_offset,
        ),
        (TerminalClass::Content, TruncationCause::TextEof) => qdiag(
            QuotaKind::Content(ContentErrorKind::Text(TextErrorKind::Xml(
                ErrorKind::UnexpectedEof,
            ))),
            error_offset,
        ),
        (TerminalClass::Content, TruncationCause::ReferenceEof) => qdiag(
            QuotaKind::Content(ContentErrorKind::Text(TextErrorKind::Reference(
                ReferenceErrorKind::UnexpectedEof,
            ))),
            error_offset,
        ),
        _ => unreachable!(),
    });
    let delivered = match (family, cut) {
        (0, 0) | (1, 11) => 0,
        (1, 12 | 15) => 2,
        (2 | 3, 15) => 3,
        (2, 17) | (3, 19) => 4,
        (4, 33..=35) => 4,
        (4, 36) => 5,
        (5, 13 | 25..=27) => 1,
        (5, 28 | 29) => 2,
        (5, 30) => 3,
        (6, 22) => 3,
        (6, 23 | 29) => 4,
        (6, 30 | 42) => 5,
        (6, 43) => 6,
        (6, 47) => 7,
        (7, 16..=19) => 3,
        (7, 20) => 4,
        _ => unreachable!(),
    };
    TruncationExpected {
        class,
        cause: Some(cause),
        diagnostic,
        delivered,
        error_offset,
        position,
        last,
    }
}

fn truncation_cause(error: SessionError) -> TruncationCause {
    match error {
        SessionError::Xml(Error {
            kind: ErrorKind::MissingRoot,
            ..
        })
        | SessionError::Content(crate::dtd_controller::content::ContentError {
            kind: ContentErrorKind::Parser(ErrorKind::MissingRoot),
            ..
        }) => TruncationCause::MissingRoot,
        SessionError::Content(crate::dtd_controller::content::ContentError {
            kind: ContentErrorKind::Parser(ErrorKind::UnexpectedEof),
            ..
        }) => TruncationCause::ParserEof,
        SessionError::Dtd(crate::dtd_controller::ControllerError {
            kind: ControllerErrorKind::Scan(ScanErrorKind::UnexpectedEof),
            ..
        }) => TruncationCause::ScanEof,
        SessionError::Content(crate::dtd_controller::content::ContentError {
            kind: ContentErrorKind::Text(TextErrorKind::Xml(ErrorKind::UnexpectedEof)),
            ..
        }) => TruncationCause::TextEof,
        SessionError::Content(crate::dtd_controller::content::ContentError {
            kind:
                ContentErrorKind::Text(TextErrorKind::Reference(ReferenceErrorKind::UnexpectedEof)),
            ..
        }) => TruncationCause::ReferenceEof,
        other => panic!("unexpected truncation cause: {other:?}"),
    }
}

fn header_event(
    raw: &str,
    origin: Range<usize>,
    field: Range<usize>,
    subset: bool,
) -> ExpectedEvent {
    physical(
        ExpectedKind::DtdBegin(raw.to_owned(), RawSource::Borrowed),
        origin,
    )
    .with_fields(vec![borrowed_field(FieldRole::DtdRaw, raw, field)])
    .with_dtd(DtdSemantic::Header {
        root: "r".to_owned(),
        subset,
    })
}

fn entity_semantic(parameter: bool, name: &str, quoted: &str) -> DtdSemantic {
    DtdSemantic::Entity {
        parameter,
        name: name.to_owned(),
        quoted: quoted.to_owned(),
        quoted_offset: Some(if parameter { 13 } else { 11 }),
        references: quoted
            .contains("&z;")
            .then(|| "&z;".to_owned())
            .into_iter()
            .collect(),
    }
}

fn canonical_expected(source: &[u8]) -> Vec<ExpectedEvent> {
    if source == T
        || source == UTF8_TEXT
        || source == EMOJI_TEXT
        || source == CHAR_REF
        || source == LEAVES
        || source == REPEATED
    {
        let mut events = vec![
            header_event("<!DOCTYPE r>", 0..11, 0..12, false),
            physical(ExpectedKind::DtdEnd, 11..12),
        ];
        if source == T {
            events.push(
                physical(ExpectedKind::Empty("r".to_owned(), None, vec![]), 12..16)
                    .with_fields(vec![borrowed_field(FieldRole::ElementName, "r", 13..14)]),
            );
        } else if source == UTF8_TEXT {
            events.extend([
                physical(ExpectedKind::Start("r".to_owned(), None, vec![]), 12..15)
                    .with_fields(vec![borrowed_field(FieldRole::ElementName, "r", 13..14)]),
                physical(ExpectedKind::Text("é".to_owned()), 15..17)
                    .with_fields(vec![borrowed_field(FieldRole::Leaf, "é", 15..17)]),
                physical(ExpectedKind::End("r".to_owned(), None), 17..21)
                    .with_fields(vec![borrowed_field(FieldRole::EndName, "r", 19..20)]),
            ]);
        } else if source == EMOJI_TEXT {
            events.extend([
                physical(ExpectedKind::Start("r".to_owned(), None, vec![]), 12..15)
                    .with_fields(vec![borrowed_field(FieldRole::ElementName, "r", 13..14)]),
                physical(ExpectedKind::Text("🙂".to_owned()), 15..19)
                    .with_fields(vec![borrowed_field(FieldRole::Leaf, "🙂", 15..19)]),
                physical(ExpectedKind::End("r".to_owned(), None), 19..23)
                    .with_fields(vec![borrowed_field(FieldRole::EndName, "r", 21..22)]),
            ]);
        } else if source == CHAR_REF {
            events.extend([
                physical(ExpectedKind::Start("r".to_owned(), None, vec![]), 12..15)
                    .with_fields(vec![borrowed_field(FieldRole::ElementName, "r", 13..14)]),
                physical(ExpectedKind::Text("A".to_owned()), 15..20)
                    .with_fields(vec![owned_field(FieldRole::Leaf, "A")]),
                physical(ExpectedKind::End("r".to_owned(), None), 20..24)
                    .with_fields(vec![borrowed_field(FieldRole::EndName, "r", 22..23)]),
            ]);
        } else if source == LEAVES {
            events.extend([
                physical(ExpectedKind::Start("r".to_owned(), None, vec![]), 12..15)
                    .with_fields(vec![borrowed_field(FieldRole::ElementName, "r", 13..14)]),
                physical(ExpectedKind::Comment("x".to_owned()), 15..23)
                    .with_fields(vec![borrowed_field(FieldRole::Leaf, "x", 19..20)]),
                physical(ExpectedKind::Pi("p".to_owned(), "x".to_owned()), 23..30).with_fields(
                    vec![
                        borrowed_field(FieldRole::PiTarget, "p", 25..26),
                        borrowed_field(FieldRole::PiData, "x", 27..28),
                    ],
                ),
                physical(ExpectedKind::Cdata("x".to_owned()), 30..43)
                    .with_fields(vec![borrowed_field(FieldRole::Leaf, "x", 39..40)]),
                physical(ExpectedKind::Empty("n".to_owned(), None, vec![]), 43..47)
                    .with_fields(vec![borrowed_field(FieldRole::ElementName, "n", 44..45)]),
                physical(ExpectedKind::End("r".to_owned(), None), 47..51)
                    .with_fields(vec![borrowed_field(FieldRole::EndName, "r", 49..50)]),
            ]);
        } else {
            events.extend([
                physical(ExpectedKind::Start("r".to_owned(), None, vec![]), 12..15)
                    .with_fields(vec![borrowed_field(FieldRole::ElementName, "r", 13..14)]),
                physical(
                    ExpectedKind::Empty(
                        "n".to_owned(),
                        None,
                        vec![ExpectedAttribute {
                            name: "a".to_owned(),
                            namespace: None,
                            value: "é".to_owned(),
                        }],
                    ),
                    15..26,
                )
                .with_fields(vec![
                    borrowed_field(FieldRole::ElementName, "n", 16..17),
                    borrowed_field(FieldRole::AttributeName(0), "a", 18..19),
                    borrowed_field(FieldRole::AttributeValue(0), "é", 21..23),
                ]),
                physical(ExpectedKind::Text("x".to_owned()), 26..27)
                    .with_fields(vec![borrowed_field(FieldRole::Leaf, "x", 26..27)]),
                physical(
                    ExpectedKind::Empty(
                        "n".to_owned(),
                        None,
                        vec![ExpectedAttribute {
                            name: "a".to_owned(),
                            namespace: None,
                            value: "é".to_owned(),
                        }],
                    ),
                    27..38,
                )
                .with_fields(vec![
                    borrowed_field(FieldRole::ElementName, "n", 28..29),
                    borrowed_field(FieldRole::AttributeName(0), "a", 30..31),
                    borrowed_field(FieldRole::AttributeValue(0), "é", 33..35),
                ]),
                physical(ExpectedKind::Text("x".to_owned()), 38..39)
                    .with_fields(vec![borrowed_field(FieldRole::Leaf, "x", 38..39)]),
                physical(ExpectedKind::End("r".to_owned(), None), 39..43)
                    .with_fields(vec![borrowed_field(FieldRole::EndName, "r", 41..42)]),
            ]);
        }
        return events;
    }

    let mut events = vec![header_event("<!DOCTYPE r [", 0..13, 0..13, true)];
    if source == F {
        events.extend([
            physical(
                ExpectedKind::DtdItem(
                    "<!ENTITY e 'x'>".to_owned(),
                    LiteralSource::Physical,
                    RawSource::Borrowed,
                ),
                13..28,
            )
            .with_fields(vec![borrowed_field(
                FieldRole::DtdRaw,
                "<!ENTITY e 'x'>",
                13..28,
            )])
            .with_dtd(entity_semantic(false, "e", "'x'")),
            physical(ExpectedKind::DtdEnd, 28..30),
            physical(ExpectedKind::Start("r".to_owned(), None, vec![]), 30..33)
                .with_fields(vec![borrowed_field(FieldRole::ElementName, "r", 31..32)]),
            replacement(ExpectedKind::Text("x".to_owned()), 0, 1, 0..1, 33..36)
                .with_fields(vec![owned_field(FieldRole::Leaf, "x")]),
            physical(ExpectedKind::End("r".to_owned(), None), 36..40)
                .with_fields(vec![borrowed_field(FieldRole::EndName, "r", 38..39)]),
        ]);
    } else if source == D {
        events.extend([
            physical(
                ExpectedKind::DtdItem(
                    "<!ATTLIST r a CDATA 'v'>".to_owned(),
                    LiteralSource::Physical,
                    RawSource::Borrowed,
                ),
                13..37,
            )
            .with_fields(vec![borrowed_field(
                FieldRole::DtdRaw,
                "<!ATTLIST r a CDATA 'v'>",
                13..37,
            )])
            .with_dtd(DtdSemantic::AttributeList {
                element: "r".to_owned(),
                definitions: vec![ExpectedAttributeDefinition {
                    name: "a".to_owned(),
                    name_offset: 12,
                    default: "'v'".to_owned(),
                    default_offset: Some(20),
                }],
            }),
            physical(ExpectedKind::DtdEnd, 37..39),
            physical(
                ExpectedKind::Empty(
                    "r".to_owned(),
                    None,
                    vec![ExpectedAttribute {
                        name: "a".to_owned(),
                        namespace: None,
                        value: "v".to_owned(),
                    }],
                ),
                39..43,
            )
            .with_fields(vec![
                borrowed_field(FieldRole::ElementName, "r", 40..41),
                owned_field(FieldRole::AttributeName(0), "a"),
                owned_field(FieldRole::AttributeValue(0), "v"),
            ]),
        ]);
    } else if source == NS {
        events.extend([
            physical(
                ExpectedKind::DtdItem(
                    "<!ATTLIST r xmlns CDATA 'urn:d' a CDATA 'v'>".to_owned(),
                    LiteralSource::Physical,
                    RawSource::Borrowed,
                ),
                13..57,
            )
            .with_fields(vec![borrowed_field(
                FieldRole::DtdRaw,
                "<!ATTLIST r xmlns CDATA 'urn:d' a CDATA 'v'>",
                13..57,
            )])
            .with_dtd(DtdSemantic::AttributeList {
                element: "r".to_owned(),
                definitions: vec![
                    ExpectedAttributeDefinition {
                        name: "xmlns".to_owned(),
                        name_offset: 12,
                        default: "'urn:d'".to_owned(),
                        default_offset: Some(24),
                    },
                    ExpectedAttributeDefinition {
                        name: "a".to_owned(),
                        name_offset: 32,
                        default: "'v'".to_owned(),
                        default_offset: Some(40),
                    },
                ],
            }),
            physical(ExpectedKind::DtdEnd, 57..59),
            physical(
                ExpectedKind::Empty(
                    "r".to_owned(),
                    Some("urn:d".to_owned()),
                    vec![
                        ExpectedAttribute {
                            name: "xmlns".to_owned(),
                            namespace: Some("http://www.w3.org/2000/xmlns/".to_owned()),
                            value: "urn:d".to_owned(),
                        },
                        ExpectedAttribute {
                            name: "a".to_owned(),
                            namespace: None,
                            value: "v".to_owned(),
                        },
                    ],
                ),
                59..63,
            )
            .with_fields(vec![
                borrowed_field(FieldRole::ElementName, "r", 60..61),
                owned_field(FieldRole::ElementNamespace, "urn:d"),
                owned_field(FieldRole::AttributeName(0), "xmlns"),
                static_field(
                    FieldRole::AttributeNamespace(0),
                    "http://www.w3.org/2000/xmlns/",
                ),
                owned_field(FieldRole::AttributeValue(0), "urn:d"),
                owned_field(FieldRole::AttributeName(1), "a"),
                owned_field(FieldRole::AttributeValue(1), "v"),
            ]),
        ]);
    } else if source == H {
        events.extend([
            physical(
                ExpectedKind::DtdItem(
                    "<!ENTITY % p \"<!ENTITY e 'x'>\">".to_owned(),
                    LiteralSource::Physical,
                    RawSource::Borrowed,
                ),
                13..44,
            )
            .with_fields(vec![borrowed_field(
                FieldRole::DtdRaw,
                "<!ENTITY % p \"<!ENTITY e 'x'>\">",
                13..44,
            )])
            .with_dtd(entity_semantic(true, "p", "\"<!ENTITY e 'x'>\"")),
            replacement(
                ExpectedKind::DtdItem(
                    "<!ENTITY e 'x'>".to_owned(),
                    LiteralSource::Replacement,
                    RawSource::Owned,
                ),
                0,
                1,
                0..15,
                44..47,
            )
            .with_fields(vec![owned_field(FieldRole::DtdRaw, "<!ENTITY e 'x'>")])
            .with_dtd(entity_semantic(false, "e", "'x'")),
            physical(ExpectedKind::DtdEnd, 47..49),
            physical(ExpectedKind::Start("r".to_owned(), None, vec![]), 49..52)
                .with_fields(vec![borrowed_field(FieldRole::ElementName, "r", 50..51)]),
            replacement(ExpectedKind::Text("x".to_owned()), 1, 2, 0..1, 52..55)
                .with_fields(vec![owned_field(FieldRole::Leaf, "x")]),
            physical(ExpectedKind::End("r".to_owned(), None), 55..59)
                .with_fields(vec![borrowed_field(FieldRole::EndName, "r", 57..58)]),
        ]);
    } else if source == EMPTY {
        events.extend([
            physical(
                ExpectedKind::DtdItem(
                    "<!ENTITY e '&z;'>".to_owned(),
                    LiteralSource::Physical,
                    RawSource::Borrowed,
                ),
                13..30,
            )
            .with_fields(vec![borrowed_field(
                FieldRole::DtdRaw,
                "<!ENTITY e '&z;'>",
                13..30,
            )])
            .with_dtd(entity_semantic(false, "e", "'&z;'")),
            physical(
                ExpectedKind::DtdItem(
                    "<!ENTITY z ''>".to_owned(),
                    LiteralSource::Physical,
                    RawSource::Borrowed,
                ),
                30..44,
            )
            .with_fields(vec![borrowed_field(
                FieldRole::DtdRaw,
                "<!ENTITY z ''>",
                30..44,
            )])
            .with_dtd(entity_semantic(false, "z", "''")),
            physical(ExpectedKind::DtdEnd, 44..46),
            physical(ExpectedKind::Start("r".to_owned(), None, vec![]), 46..49)
                .with_fields(vec![borrowed_field(FieldRole::ElementName, "r", 47..48)]),
            physical(ExpectedKind::End("r".to_owned(), None), 52..56)
                .with_fields(vec![borrowed_field(FieldRole::EndName, "r", 54..55)]),
        ]);
    } else if source == VISIBLE_EMPTY {
        events.extend([
            physical(
                ExpectedKind::DtdItem(
                    "<!ENTITY e 'x'>".to_owned(),
                    LiteralSource::Physical,
                    RawSource::Borrowed,
                ),
                13..28,
            )
            .with_fields(vec![borrowed_field(
                FieldRole::DtdRaw,
                "<!ENTITY e 'x'>",
                13..28,
            )])
            .with_dtd(entity_semantic(false, "e", "'x'")),
            physical(
                ExpectedKind::DtdItem(
                    "<!ENTITY z ''>".to_owned(),
                    LiteralSource::Physical,
                    RawSource::Borrowed,
                ),
                28..42,
            )
            .with_fields(vec![borrowed_field(
                FieldRole::DtdRaw,
                "<!ENTITY z ''>",
                28..42,
            )])
            .with_dtd(entity_semantic(false, "z", "''")),
            physical(ExpectedKind::DtdEnd, 42..44),
            physical(ExpectedKind::Start("r".to_owned(), None, vec![]), 44..47)
                .with_fields(vec![borrowed_field(FieldRole::ElementName, "r", 45..46)]),
            replacement(ExpectedKind::Text("x".to_owned()), 0, 1, 0..1, 47..50)
                .with_fields(vec![owned_field(FieldRole::Leaf, "x")]),
            replacement(ExpectedKind::Text("x".to_owned()), 0, 3, 0..1, 53..56)
                .with_fields(vec![owned_field(FieldRole::Leaf, "x")]),
            physical(ExpectedKind::End("r".to_owned(), None), 56..60)
                .with_fields(vec![borrowed_field(FieldRole::EndName, "r", 58..59)]),
        ]);
    } else {
        panic!(
            "missing exact canonical oracle for {}",
            String::from_utf8_lossy(source)
        );
    }
    events
}

fn terminal_class(error: SessionError) -> TerminalClass {
    match error {
        SessionError::Xml(_) => TerminalClass::Xml,
        SessionError::Dtd(_) => TerminalClass::Dtd,
        SessionError::Content(_) => TerminalClass::Content,
        SessionError::Allocation { .. } | SessionError::Poisoned { .. } => {
            panic!("unexpected infrastructure failure: {error:?}")
        }
    }
}

fn drain_with_lifecycle(source: &[u8], lifecycle: u8) -> Result<(), SessionError> {
    let mut reader = DtdReader::new(source, options())?;
    let source_text = std::str::from_utf8(source).expect("reader accepted UTF-8 source");
    let expected = canonical_expected(source);
    let mut retained = Vec::new();
    let mut retained_bytes = 0_usize;
    let mut owned = Vec::new();
    let mut expected_owned_values = Vec::new();
    let mut owned_bytes = 0_usize;
    let stop = usize::from(lifecycle >> 2).min(expected.len().saturating_sub(1));
    let mut delivered = 0_usize;
    loop {
        match reader.read_event() {
            Ok(Some((event, origin))) => {
                let expected_event = &expected[delivered];
                check_expected_event(&event, origin, expected_event, source_text).unwrap();
                validate_origin(origin, source.len());
                match lifecycle & 3 {
                    0 => {}
                    1 => {
                        let bytes = owned_event_bytes(&event);
                        if retained.len() < MAX_RETAINED_OBJECTS
                            && retained_bytes + bytes <= MAX_RETAINED_OWNED
                        {
                            retained_bytes += bytes;
                            retained.push((event, origin));
                        }
                    }
                    2 => {
                        for value in expected_owned(expected_event) {
                            if expected_owned_values.len() < MAX_RETAINED_OBJECTS
                                && expected_owned_values
                                    .iter()
                                    .map(|value: &OwnedExtraction| value.value.len())
                                    .sum::<usize>()
                                    + value.value.len()
                                    <= MAX_RETAINED_OWNED
                            {
                                expected_owned_values.push(value);
                            }
                        }
                        collect_owned_bounded(event, &mut owned, &mut owned_bytes);
                    }
                    3 if delivered == stop => {
                        let pointer = source.as_ptr();
                        let length = source.len();
                        let inner = reader.into_inner();
                        assert_eq!(inner.as_ptr(), pointer);
                        assert_eq!(inner.len(), length);
                        check_expected_event(&event, origin, expected_event, source_text).unwrap();
                        drain_with_lifecycle(source, 0)?;
                        return Ok(());
                    }
                    3 => {}
                    _ => unreachable!(),
                }
                for (index, (held, held_origin)) in retained.iter().enumerate() {
                    check_expected_event(held, *held_origin, &expected[index], source_text)
                        .unwrap();
                }
                delivered += 1;
            }
            Ok(None) => {
                assert_eq!(delivered, expected.len());
                assert_eq!(reader.position(), source.len());
                assert_eq!(
                    reader.event_origin(),
                    expected.last().map(|event| event.origin.full)
                );
                assert_terminal_sticky_with(&mut reader, Ok(()), || {
                    for (index, (event, origin)) in retained.iter().enumerate() {
                        check_expected_event(event, *origin, &expected[index], source_text)
                            .unwrap();
                    }
                });
                drop(reader);
                for (index, (event, origin)) in retained.iter().enumerate() {
                    check_expected_event(event, *origin, &expected[index], source_text).unwrap();
                }
                assert!(owned_bytes <= MAX_RETAINED_OWNED);
                assert!(owned.len() <= MAX_RETAINED_OBJECTS);
                assert_eq!(
                    owned.iter().map(|value| value.value.len()).sum::<usize>(),
                    owned_bytes
                );
                assert_eq!(owned, expected_owned_values);
                return Ok(());
            }
            Err(error) => {
                assert_terminal_sticky(&mut reader, Err(error));
                drop(reader);
                for (event, origin) in &retained {
                    check_event_storage(event, *origin, source_text).unwrap();
                }
                assert!(owned_bytes <= MAX_RETAINED_OWNED);
                return Err(error);
            }
        }
    }
}

pub(super) fn owned_event_bytes(event: &ReaderEvent<'_>) -> usize {
    observed_fields(event)
        .into_iter()
        .map(|field| match (field.role, field.storage) {
            (
                FieldRole::ElementNamespace
                | FieldRole::AttributeNamespace(_)
                | FieldRole::EndNamespace,
                ObservedStorage::Owned(value) | ObservedStorage::Borrowed(value),
            ) => value.len(),
            (_, ObservedStorage::Owned(value)) => value.len(),
            (_, ObservedStorage::Borrowed(_)) => 0,
        })
        .sum()
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct OwnedExtraction {
    pub(super) role: FieldRole,
    pub(super) value: String,
}

pub(super) fn expected_owned(event: &ExpectedEvent) -> Vec<OwnedExtraction> {
    event
        .fields
        .iter()
        .filter(|field| matches!(field.storage, ExpectedStorage::Owned))
        .map(|field| OwnedExtraction {
            role: field.role.clone(),
            value: field.value.clone(),
        })
        .collect()
}

pub(super) fn observed_owned_before_move(event: &ReaderEvent<'_>) -> Vec<OwnedExtraction> {
    observed_fields(event)
        .into_iter()
        .filter_map(|field| match field.storage {
            ObservedStorage::Owned(value) => Some(OwnedExtraction {
                role: field.role,
                value: value.to_owned(),
            }),
            ObservedStorage::Borrowed(_) => None,
        })
        .collect()
}

pub(super) fn collect_owned_bounded(
    event: ReaderEvent<'_>,
    values: &mut Vec<OwnedExtraction>,
    used: &mut usize,
) {
    let mut push = |role: FieldRole, value: String| {
        if values.len() < MAX_RETAINED_OBJECTS && *used + value.len() <= MAX_RETAINED_OWNED {
            *used += value.len();
            values.push(OwnedExtraction { role, value });
        }
    };
    match event {
        ReaderEvent::Dtd(TakenDtdEvent::Begin(unit))
        | ReaderEvent::Dtd(TakenDtdEvent::Item { unit, .. }) => {
            if let Cow::Owned(raw) = unit.into_fuzz_raw() {
                push(FieldRole::DtdRaw, raw);
            }
        }
        ReaderEvent::Dtd(TakenDtdEvent::End) => {}
        ReaderEvent::Xml(Event::Start(element) | Event::Empty(element)) => {
            let (qualified, namespace) = element.name.into_fuzz_parts();
            if let Cow::Owned(value) = qualified {
                push(FieldRole::ElementName, value);
            }
            if let Some(Cow::Owned(value)) = namespace {
                push(FieldRole::ElementNamespace, value);
            }
            for (index, attribute) in element.attributes.into_iter().enumerate() {
                let (qualified, namespace) = attribute.name.into_fuzz_parts();
                if let Cow::Owned(value) = qualified {
                    push(FieldRole::AttributeName(index), value);
                }
                if let Some(Cow::Owned(value)) = namespace {
                    push(FieldRole::AttributeNamespace(index), value);
                }
                if let Cow::Owned(value) = attribute.value {
                    push(FieldRole::AttributeValue(index), value);
                }
            }
        }
        ReaderEvent::Xml(Event::End(name)) => {
            let (qualified, namespace) = name.into_fuzz_parts();
            if let Cow::Owned(value) = qualified {
                push(FieldRole::EndName, value);
            }
            if let Some(Cow::Owned(value)) = namespace {
                push(FieldRole::EndNamespace, value);
            }
        }
        ReaderEvent::Xml(Event::Text(Cow::Owned(value)))
        | ReaderEvent::Xml(Event::Cdata(Cow::Owned(value)))
        | ReaderEvent::Xml(Event::Comment(Cow::Owned(value))) => push(FieldRole::Leaf, value),
        ReaderEvent::Xml(Event::ProcessingInstruction { target, data }) => {
            if let Cow::Owned(value) = target {
                push(FieldRole::PiTarget, value);
            }
            if let Cow::Owned(value) = data {
                push(FieldRole::PiData, value);
            }
        }
        _ => {}
    }
}

pub(super) fn exercise_truncation(selector: u8, lifecycle: u8) {
    let (family, _, cut) = truncation_selection(selector);
    let fixture = truncation_fixture(family);
    update_receipt(|receipt| {
        receipt.truncation = Some((selector, family, cut, lifecycle));
    });

    drain_with_lifecycle(fixture, lifecycle).expect("full truncation control");
    let expected = expected_truncation(family, cut, fixture.len());
    let prefix = &fixture[..cut];
    check_truncation_prefix(family, prefix, expected, lifecycle);
}

fn check_truncation_prefix(
    family: usize,
    prefix: &[u8],
    expected: TruncationExpected,
    lifecycle: u8,
) {
    let expected_events = canonical_expected(truncation_fixture(family));
    match DtdReader::new(prefix, options()) {
        Err(SessionError::Xml(error)) => {
            assert_eq!(expected.class, TerminalClass::ConstructorXml);
            assert_eq!(
                error,
                Error::new(ErrorKind::InvalidUtf8, expected.error_offset)
            );
        }
        Err(other) => panic!("unexpected constructor result: {other:?}"),
        Ok(mut reader) => {
            let source_text = std::str::from_utf8(prefix).unwrap();
            let mut delivered = 0_usize;
            let mut retained = Vec::new();
            let mut retained_bytes = 0_usize;
            let mut owned = Vec::new();
            let mut expected_owned_values = Vec::new();
            let mut owned_bytes = 0_usize;
            let checkpoint = usize::from(lifecycle >> 2).min(expected.delivered.saturating_sub(1));
            loop {
                match reader.read_event() {
                    Ok(Some((event, origin))) => {
                        assert!(delivered < expected.delivered, "unexpected extra event");
                        let expected_event = &expected_events[delivered];
                        check_expected_event(&event, origin, expected_event, source_text).unwrap();
                        match lifecycle & 3 {
                            0 => {}
                            1 => {
                                let bytes = owned_event_bytes(&event);
                                if retained.len() < MAX_RETAINED_OBJECTS
                                    && retained_bytes + bytes <= MAX_RETAINED_OWNED
                                {
                                    retained_bytes += bytes;
                                    retained.push((event, origin));
                                }
                            }
                            2 => {
                                for value in expected_owned(expected_event) {
                                    if expected_owned_values.len() < MAX_RETAINED_OBJECTS
                                        && expected_owned_values
                                            .iter()
                                            .map(|value: &OwnedExtraction| value.value.len())
                                            .sum::<usize>()
                                            + value.value.len()
                                            <= MAX_RETAINED_OWNED
                                    {
                                        expected_owned_values.push(value);
                                    }
                                }
                                collect_owned_bounded(event, &mut owned, &mut owned_bytes);
                            }
                            3 if delivered == checkpoint => {
                                let pointer = prefix.as_ptr();
                                let length = prefix.len();
                                let inner = reader.into_inner();
                                assert_eq!(inner.as_ptr(), pointer);
                                assert_eq!(inner.len(), length);
                                check_expected_event(&event, origin, expected_event, source_text)
                                    .unwrap();
                                check_truncation_prefix(family, prefix, expected, 0);
                                return;
                            }
                            3 => {}
                            _ => unreachable!(),
                        }
                        for (index, (held, held_origin)) in retained.iter().enumerate() {
                            check_expected_event(
                                held,
                                *held_origin,
                                &expected_events[index],
                                source_text,
                            )
                            .unwrap();
                        }
                        delivered += 1;
                    }
                    Ok(None) => {
                        check_truncation_observation(
                            TruncationObservation {
                                class: TerminalClass::Complete,
                                diagnostic: None,
                                delivered,
                                position: reader.position(),
                                last: reader.event_origin(),
                            },
                            expected,
                        )
                        .unwrap();
                        assert_terminal_sticky_with(&mut reader, Ok(()), || {
                            for (index, (held, held_origin)) in retained.iter().enumerate() {
                                check_expected_event(
                                    held,
                                    *held_origin,
                                    &expected_events[index],
                                    source_text,
                                )
                                .unwrap();
                            }
                        });
                        drop(reader);
                        for (index, (held, held_origin)) in retained.iter().enumerate() {
                            check_expected_event(
                                held,
                                *held_origin,
                                &expected_events[index],
                                source_text,
                            )
                            .unwrap();
                        }
                        assert_eq!(owned, expected_owned_values);
                        assert!(owned_bytes <= MAX_RETAINED_OWNED);
                        return;
                    }
                    Err(error) => {
                        let class = terminal_class(error);
                        assert_eq!(truncation_cause(error), expected.cause.unwrap());
                        check_truncation_observation(
                            TruncationObservation {
                                class,
                                diagnostic: Some(quota_diagnostic(error)),
                                delivered,
                                position: reader.position(),
                                last: reader.event_origin(),
                            },
                            expected,
                        )
                        .unwrap();
                        assert_terminal_sticky_with(&mut reader, Err(error), || {
                            for (index, (held, held_origin)) in retained.iter().enumerate() {
                                check_expected_event(
                                    held,
                                    *held_origin,
                                    &expected_events[index],
                                    source_text,
                                )
                                .unwrap();
                            }
                        });
                        drop(reader);
                        for (index, (held, held_origin)) in retained.iter().enumerate() {
                            check_expected_event(
                                held,
                                *held_origin,
                                &expected_events[index],
                                source_text,
                            )
                            .unwrap();
                        }
                        assert_eq!(owned, expected_owned_values);
                        assert!(owned_bytes <= MAX_RETAINED_OWNED);
                        return;
                    }
                }
            }
        }
    }
}

pub(super) fn exercise_witness_bank(lifecycle: u8) {
    update_receipt(|receipt| receipt.witness_lifecycle = Some(lifecycle));
    for fixture in [T, F, D, NS, H, EMPTY, LEAVES, REPEATED, VISIBLE_EMPTY] {
        drain_with_lifecycle(fixture, lifecycle).expect("canonical semantic witness");
    }
}

fn quota_profile() -> SessionLimits {
    let mut profile = options();
    profile.text.reference = ReferenceLimits {
        max_bytes: 16,
        max_scratch_bytes: 16,
        max_work: WORK,
    };
    profile.text.scratch = TextScratchLimits {
        input_token_bytes: 16,
        replacement_token_bytes: 16,
        max_bytes: 48,
    };
    profile.local = LocalLimits {
        max_token_bytes: 16,
        max_scratch_bytes: 16,
        max_work: WORK,
    };
    profile
}

fn scalar_fixture(axis: usize) -> (&'static [u8], usize) {
    match axis {
        0 => (B, 19),
        1 => (B, 16),
        2 | 3 => (U, 13),
        4 => (T, 1),
        5 => (b"<!DOCTYPE r [<!ATTLIST r a CDATA 'v'>]><r/>", 1),
        6 => (DOC, 11),
        7 | 10 => (RP, 16),
        8 => (T, 16),
        9 | 30 => (T, 48),
        11..=14 | 23 => (T, 12),
        15 => (T, 1),
        16 => (M, 1),
        17 => (A, 1),
        18 => (T, 1),
        19 => (V, 1),
        20 => (V, 3),
        21 | 22 => (E, 1),
        24 => (AMP, 1),
        25 => (V, 1),
        26 | 27 | 29 => (F, 1),
        28 => (F, 2),
        31..=33 => (F, if axis == 33 { 9 } else { 3 }),
        34 => (T, 16),
        35 => (T, 8),
        36 => (T, 28),
        37 | 38 => (G, 4),
        39 => (RL, 16),
        _ => unreachable!(),
    }
}

fn apply_scalar_axis(
    axis: usize,
    ceiling: usize,
    limits: &mut Limits,
    decode: &mut DecodeLimits,
    profile: &mut SessionLimits,
) {
    match axis {
        0 => limits.max_input_bytes = ceiling,
        1 => decode.max_decoded_bytes = ceiling,
        2 => limits.max_token_bytes = ceiling,
        3 => decode.max_decoded_token_bytes = ceiling,
        4 => limits.max_depth = ceiling,
        5 => limits.max_attributes = ceiling,
        6 => limits.max_events = ceiling,
        7 => limits.max_owned_bytes = ceiling,
        8 => profile.declarations.max_physical_bytes = ceiling,
        9 => profile.declarations.max_work = ceiling,
        10 => profile.declarations.max_owned_bytes = ceiling,
        11 => profile.declarations.scan.max_unit_bytes = ceiling,
        12 => profile.declarations.scan.max_scratch_bytes = ceiling,
        13 => profile.declarations.scan.max_work = ceiling,
        14 => profile.declarations.unit.max_unit_bytes = ceiling,
        15 => profile.declarations.unit.max_tokens = ceiling,
        16 => profile.declarations.unit.max_model_depth = ceiling,
        17 => profile.declarations.unit.max_attribute_definitions = ceiling,
        18 => profile.declarations.catalog.max_declarations = ceiling,
        19 => profile.declarations.catalog.max_bindings = ceiling,
        20 => profile.declarations.catalog.max_storage_bytes = ceiling,
        21 => profile.declarations.catalog.max_value_bytes = ceiling,
        22 => profile.declarations.catalog.max_expanded_bytes = ceiling,
        23 => profile.declarations.catalog.max_work = ceiling,
        24 => profile.declarations.catalog.max_references = ceiling,
        25 => profile.declarations.catalog.max_depth = ceiling,
        26 => profile.declarations.frames.max_active_depth = ceiling,
        27 => profile.declarations.frames.max_reference_attempts = ceiling,
        28 => profile.declarations.frames.max_work = ceiling,
        29 => profile.declarations.frames.max_expanded_bytes = ceiling,
        30 => profile.text.max_work = ceiling,
        31 => profile.text.reference.max_bytes = ceiling,
        32 => profile.text.reference.max_scratch_bytes = ceiling,
        33 => profile.text.reference.max_work = ceiling,
        34 => {
            q6_scratch(profile);
            profile.text.scratch.input_token_bytes = ceiling;
        }
        35 => {
            q6_scratch(profile);
            profile.text.scratch.replacement_token_bytes = ceiling;
        }
        36 => {
            q6_scratch(profile);
            profile.text.scratch.max_bytes = ceiling;
        }
        37 => profile.local.max_token_bytes = ceiling,
        38 => profile.local.max_scratch_bytes = ceiling,
        39 => profile.local.max_work = ceiling,
        _ => unreachable!(),
    }
}

fn q6_scratch(profile: &mut SessionLimits) {
    profile.local.max_token_bytes = 8;
    profile.local.max_scratch_bytes = 8;
    profile.text.reference.max_bytes = 4;
    profile.text.reference.max_scratch_bytes = 4;
    profile.text.scratch = TextScratchLimits {
        input_token_bytes: 16,
        replacement_token_bytes: 8,
        max_bytes: 29,
    };
}

fn expected_quota_phase(axis: usize, mode: usize) -> TerminalClass {
    match axis {
        0..=1 => TerminalClass::Xml,
        2..=3 if mode == 0 => TerminalClass::Xml,
        6 if mode == 0 => TerminalClass::Xml,
        8..=9 if mode == 0 => TerminalClass::Dtd,
        7 | 10..=20 | 23 => TerminalClass::Dtd,
        _ => TerminalClass::Content,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QuotaKind {
    Xml(ErrorKind),
    Dtd(ControllerErrorKind),
    Content(ContentErrorKind),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct QuotaDiagnostic {
    kind: QuotaKind,
    physical: usize,
    origin: Option<crate::origin::OriginRepr>,
    local: Option<usize>,
    value: Option<ValueLocation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct QuotaFailure {
    diagnostic: QuotaDiagnostic,
    position: usize,
    last: Option<SourceOrigin>,
    prefix: usize,
}

fn check_quota_failure(actual: QuotaFailure, expected: QuotaFailure) -> Result<(), &'static str> {
    if actual.diagnostic.kind != expected.diagnostic.kind {
        return Err("quota kind");
    }
    if actual.diagnostic.physical != expected.diagnostic.physical {
        return Err("quota physical anchor");
    }
    if actual.diagnostic.origin != expected.diagnostic.origin {
        return Err("quota origin");
    }
    if actual.diagnostic.local != expected.diagnostic.local {
        return Err("quota local offset");
    }
    if actual.diagnostic.value != expected.diagnostic.value {
        return Err("quota value location");
    }
    if actual.position != expected.position {
        return Err("quota accepted position");
    }
    if actual.last != expected.last {
        return Err("quota last event origin");
    }
    if actual.prefix != expected.prefix {
        return Err("quota delivered prefix length");
    }
    Ok(())
}

fn input_fragment(range: Range<usize>) -> Option<crate::origin::OriginRepr> {
    crate::origin::OriginRepr::input(range)
}

fn replacement_fragment(
    declaration: usize,
    invocation: u64,
    local: Range<usize>,
    physical: Range<usize>,
) -> Option<crate::origin::OriginRepr> {
    use crate::origin::{ByteSpan, EntityDeclarationId, EntityInvocationId, OriginRepr};
    use std::num::NonZeroU64;
    Some(OriginRepr::Replacement {
        declaration: EntityDeclarationId::from_index(declaration),
        invocation: EntityInvocationId(NonZeroU64::new(invocation).unwrap()),
        local: ByteSpan::new(local).unwrap(),
        physical_reference: ByteSpan::new(physical).unwrap(),
    })
}

fn quota_diagnostic(error: SessionError) -> QuotaDiagnostic {
    match error {
        SessionError::Xml(error) => QuotaDiagnostic {
            kind: QuotaKind::Xml(error.kind),
            physical: error.offset,
            origin: None,
            local: None,
            value: None,
        },
        SessionError::Dtd(error) => QuotaDiagnostic {
            kind: QuotaKind::Dtd(error.kind),
            physical: error.physical_offset,
            origin: error.origin,
            local: error.local_offset,
            value: error.value_location,
        },
        SessionError::Content(error) => QuotaDiagnostic {
            kind: QuotaKind::Content(error.kind),
            physical: error.physical_offset,
            origin: error.origin,
            local: error.local_offset,
            value: error.value_location,
        },
        SessionError::Allocation { physical_offset } => QuotaDiagnostic {
            kind: QuotaKind::Content(ContentErrorKind::Allocation),
            physical: physical_offset,
            origin: None,
            local: None,
            value: None,
        },
        SessionError::Poisoned { physical_offset } => QuotaDiagnostic {
            kind: QuotaKind::Content(ContentErrorKind::Poisoned),
            physical: physical_offset,
            origin: None,
            local: None,
            value: None,
        },
    }
}

fn qdiag(kind: QuotaKind, physical: usize) -> QuotaDiagnostic {
    QuotaDiagnostic {
        kind,
        physical,
        origin: None,
        local: None,
        value: None,
    }
}

impl QuotaDiagnostic {
    fn origin(mut self, origin: Option<crate::origin::OriginRepr>) -> Self {
        self.origin = origin;
        self
    }

    fn local(mut self, local: usize) -> Self {
        self.local = Some(local);
        self
    }

    fn value(mut self, value: ValueLocation) -> Self {
        self.value = Some(value);
        self
    }
}

fn expected_scalar_failure(axis: usize, mode: usize) -> QuotaFailure {
    let zero = mode == 0;
    let input = |range| input_origin(range);
    let replacement = |d, i, local, physical| Some(replacement_origin(d, i, local, physical));
    let (diagnostic, position, last) = match axis {
        0 => (
            qdiag(
                QuotaKind::Xml(ErrorKind::LimitExceeded(Limit::InputBytes)),
                0,
            ),
            0,
            None,
        ),
        1 => (
            qdiag(
                QuotaKind::Xml(ErrorKind::LimitExceeded(Limit::DecodedBytes)),
                0,
            ),
            0,
            None,
        ),
        2 | 3 if zero => (
            qdiag(
                QuotaKind::Xml(ErrorKind::LimitExceeded(if axis == 2 {
                    Limit::TokenBytes
                } else {
                    Limit::DecodedTokenBytes
                })),
                0,
            ),
            0,
            None,
        ),
        2 | 3 => (
            qdiag(
                QuotaKind::Content(ContentErrorKind::Text(TextErrorKind::Xml(
                    ErrorKind::LimitExceeded(if axis == 2 {
                        Limit::TokenBytes
                    } else {
                        Limit::DecodedTokenBytes
                    }),
                ))),
                12,
            ),
            25,
            input(11..12),
        ),
        4 => (
            qdiag(
                QuotaKind::Content(ContentErrorKind::Tag(DtdTagErrorKind::Parser(
                    ErrorKind::LimitExceeded(Limit::Depth),
                ))),
                12,
            )
            .origin(input_fragment(12..16)),
            16,
            input(11..12),
        ),
        5 => (
            qdiag(
                QuotaKind::Content(ContentErrorKind::Tag(DtdTagErrorKind::Parser(
                    ErrorKind::LimitExceeded(Limit::Attributes),
                ))),
                39,
            )
            .origin(input_fragment(39..43))
            .value(ValueLocation::Declaration {
                declaration: 1,
                offset: 0,
            }),
            43,
            input(37..39),
        ),
        6 if zero => (
            qdiag(QuotaKind::Xml(ErrorKind::LimitExceeded(Limit::Events)), 0),
            0,
            None,
        ),
        6 => (
            qdiag(
                QuotaKind::Content(ContentErrorKind::Parser(ErrorKind::LimitExceeded(
                    Limit::Events,
                ))),
                60,
            )
            .origin(input_fragment(60..64)),
            64,
            input(59..60),
        ),
        7 => (
            qdiag(
                QuotaKind::Dtd(ControllerErrorKind::Ownership(ErrorKind::LimitExceeded(
                    Limit::OwnedBytes,
                ))),
                if zero { 37 } else { 40 },
            )
            .origin(replacement_fragment(
                0,
                if zero { 1 } else { 2 },
                0..8,
                if zero { 37..40 } else { 40..43 },
            ))
            .local(0),
            if zero { 40 } else { 43 },
            if zero {
                input(13..37)
            } else {
                replacement(0, 1, 0..8, 37..40)
            },
        ),
        8 => (
            qdiag(
                if zero {
                    QuotaKind::Dtd(ControllerErrorKind::Limit(ControllerLimit::PhysicalBytes))
                } else {
                    QuotaKind::Content(ContentErrorKind::PhysicalBytes)
                },
                if zero { 0 } else { 15 },
            ),
            if zero { 0 } else { 15 },
            if zero { None } else { input(11..12) },
        ),
        9 => (
            qdiag(
                if zero {
                    QuotaKind::Dtd(ControllerErrorKind::Limit(ControllerLimit::Work))
                } else {
                    QuotaKind::Content(ContentErrorKind::Work)
                },
                if zero { 0 } else { 12 },
            ),
            if zero { 0 } else { 16 },
            if zero { None } else { input(11..12) },
        ),
        10 => (
            qdiag(
                QuotaKind::Dtd(ControllerErrorKind::Limit(ControllerLimit::OwnedBytes)),
                if zero { 37 } else { 40 },
            )
            .origin(replacement_fragment(
                0,
                if zero { 1 } else { 2 },
                0..8,
                if zero { 37..40 } else { 40..43 },
            ))
            .local(0),
            if zero { 40 } else { 43 },
            if zero {
                input(13..37)
            } else {
                replacement(0, 1, 0..8, 37..40)
            },
        ),
        11..=13 => {
            let limit = [
                ScanLimit::UnitBytes,
                ScanLimit::ScratchBytes,
                ScanLimit::Work,
            ][axis - 11];
            (
                qdiag(
                    QuotaKind::Dtd(ControllerErrorKind::Scan(ScanErrorKind::Limit(limit))),
                    if zero { 0 } else { 11 },
                )
                .origin((!zero).then(|| input_fragment(0..11).unwrap())),
                if zero { 0 } else { 11 },
                None,
            )
        }
        14..=17 => {
            let (limit, physical, range, position, last) = match axis {
                14 => (
                    DtdLimit::UnitBytes,
                    if zero { 0 } else { 11 },
                    0..12,
                    12,
                    None,
                ),
                15 => (DtdLimit::Tokens, 10, 0..12, 12, None),
                16 => (DtdLimit::ModelDepth, 25, 13..29, 29, input(0..13)),
                17 => (DtdLimit::AttributeDefinitions, 25, 13..42, 42, input(0..13)),
                _ => unreachable!(),
            };
            (
                qdiag(
                    QuotaKind::Dtd(ControllerErrorKind::Grammar(DtdErrorKind::LimitExceeded(
                        limit,
                    ))),
                    physical,
                )
                .origin(input_fragment(range)),
                position,
                last,
            )
        }
        18..=20 | 23 => {
            let (limit, physical, range, position, last, declaration) = match axis {
                18 => (CatalogLimit::Declarations, 0, 0..12, 12, None, 0),
                19 => (CatalogLimit::Bindings, 13, 13..29, 29, input(0..13), 1),
                20 => (CatalogLimit::StorageBytes, 13, 13..29, 29, input(0..13), 1),
                23 => (CatalogLimit::Work, 0, 0..12, 12, None, 0),
                _ => unreachable!(),
            };
            (
                qdiag(
                    QuotaKind::Dtd(ControllerErrorKind::Catalog(CatalogErrorKind::Limit(limit))),
                    physical,
                )
                .origin(input_fragment(range))
                .value(ValueLocation::Declaration {
                    declaration,
                    offset: 0,
                }),
                position,
                last,
            )
        }
        21 | 22 | 24 | 25 => {
            let (limit, physical, range, position, last, value) = match axis {
                21 => (CatalogLimit::ValueBytes, 18, 12..22, 22, input(11..12), 18),
                22 => (
                    CatalogLimit::ExpandedBytes,
                    18,
                    12..22,
                    22,
                    input(11..12),
                    18,
                ),
                24 => (CatalogLimit::References, 18, 12..26, 26, input(11..12), 18),
                25 => (CatalogLimit::Depth, 37, 31..43, 43, input(29..31), 37),
                _ => unreachable!(),
            };
            (
                qdiag(
                    QuotaKind::Content(ContentErrorKind::Tag(DtdTagErrorKind::Catalog(
                        CatalogErrorKind::Limit(limit),
                    ))),
                    physical,
                )
                .origin(input_fragment(range))
                .value(ValueLocation::Input(value)),
                position,
                last,
            )
        }
        26 | 27 => {
            let limit = if axis == 26 {
                FrameLimit::ActiveDepth
            } else {
                FrameLimit::ReferenceAttempts
            };
            (
                qdiag(
                    QuotaKind::Content(ContentErrorKind::Frame(FrameErrorKind::LimitExceeded(
                        limit,
                    ))),
                    33,
                )
                .origin(input_fragment(33..36)),
                36,
                input(30..33),
            )
        }
        28 => (
            {
                let diagnostic = qdiag(
                    QuotaKind::Content(ContentErrorKind::Frame(FrameErrorKind::LimitExceeded(
                        FrameLimit::Work,
                    ))),
                    33,
                )
                .origin(if zero {
                    input_fragment(33..36)
                } else {
                    replacement_fragment(0, 1, 0..1, 33..36)
                });
                if zero {
                    diagnostic
                } else {
                    diagnostic.local(0)
                }
            },
            36,
            input(30..33),
        ),
        29 => (
            qdiag(
                QuotaKind::Content(ContentErrorKind::Frame(FrameErrorKind::LimitExceeded(
                    FrameLimit::ExpandedBytes,
                ))),
                33,
            )
            .origin(replacement_fragment(0, 1, 0..1, 33..36))
            .local(0),
            36,
            input(30..33),
        ),
        30 => (
            qdiag(
                QuotaKind::Content(if zero {
                    ContentErrorKind::Text(TextErrorKind::Work)
                } else {
                    ContentErrorKind::Work
                }),
                12,
            ),
            if zero { 12 } else { 16 },
            input(11..12),
        ),
        31..=33 => {
            let kind = match axis {
                31 => ReferenceErrorKind::Limit(ReferenceLimit::Bytes),
                32 => ReferenceErrorKind::Limit(ReferenceLimit::ScratchBytes),
                33 => ReferenceErrorKind::Limit(ReferenceLimit::Work),
                _ => unreachable!(),
            };
            let physical = if axis == 33 || zero { 33 } else { 35 };
            (
                qdiag(
                    QuotaKind::Content(ContentErrorKind::Text(TextErrorKind::Reference(kind))),
                    physical,
                ),
                if axis == 33 && !zero { 36 } else { physical },
                input(30..33),
            )
        }
        34..=36 => (
            qdiag(
                QuotaKind::Content(ContentErrorKind::Text(TextErrorKind::Scratch)),
                12,
            ),
            12,
            input(11..12),
        ),
        37 | 38 => (
            qdiag(
                QuotaKind::Content(ContentErrorKind::Text(TextErrorKind::Local(
                    LocalErrorKind::Xml(ErrorKind::LimitExceeded(if axis == 37 {
                        Limit::TokenBytes
                    } else {
                        Limit::DecodedTokenBytes
                    })),
                ))),
                36,
            )
            .origin(replacement_fragment(0, 1, 0..0, 36..39))
            .local(0),
            39,
            input(33..36),
        ),
        39 => (
            qdiag(
                QuotaKind::Content(ContentErrorKind::Text(TextErrorKind::Local(
                    LocalErrorKind::Work,
                ))),
                if zero { 36 } else { 39 },
            )
            .origin(replacement_fragment(
                0,
                if zero { 1 } else { 2 },
                0..0,
                if zero { 36..39 } else { 39..42 },
            ))
            .local(0),
            if zero { 39 } else { 42 },
            if zero {
                input(33..36)
            } else {
                replacement(0, 1, 0..4, 36..39)
            },
        ),
        _ => unreachable!(),
    };
    QuotaFailure {
        diagnostic,
        position,
        last,
        prefix: match axis {
            0 | 1 => 0,
            2 | 3 => {
                if zero {
                    0
                } else {
                    2
                }
            }
            4 => 2,
            5 => 3,
            6 => {
                if zero {
                    0
                } else {
                    10
                }
            }
            7 | 10 => {
                if zero {
                    2
                } else {
                    3
                }
            }
            8 | 9 => {
                if zero {
                    0
                } else {
                    2
                }
            }
            11..=15 | 18 | 23 => 0,
            16 | 17 | 19 | 20 => 1,
            21 | 22 | 24 | 30 | 34..=36 => 2,
            25 => 3,
            26..=29 | 31..=33 | 37 | 38 => 4,
            39 => {
                if zero {
                    4
                } else {
                    5
                }
            }
            _ => unreachable!(),
        },
    }
}

fn run_configured(
    source: &'static [u8],
    limits: Limits,
    decode: DecodeLimits,
    profile: SessionLimits,
    lifecycle: u8,
) -> Result<(), SessionError> {
    if lifecycle & 3 == 3 {
        let mut early = DtdReader::with_decode_limits(source, limits, decode, profile)?;
        let expected = literal_transcript(source);
        let checkpoint = usize::from(lifecycle >> 2).min(expected.len().saturating_sub(1));
        for (index, expected_signature) in expected.iter().enumerate().take(checkpoint + 1) {
            match early.read_event()? {
                Some((event, origin)) if index < checkpoint => {
                    assert_eq!(&event_signature(&event), expected_signature);
                    check_event_storage(&event, origin, std::str::from_utf8(source).unwrap())
                        .unwrap();
                }
                Some((event, origin)) => {
                    assert_eq!(&event_signature(&event), expected_signature);
                    check_event_storage(&event, origin, std::str::from_utf8(source).unwrap())
                        .unwrap();
                    let inner = early.into_inner();
                    assert_eq!(inner.as_ptr(), source.as_ptr());
                    assert_eq!(inner.len(), source.len());
                    assert_eq!(&event_signature(&event), expected_signature);
                    check_event_storage(&event, origin, std::str::from_utf8(source).unwrap())
                        .unwrap();
                    update_receipt(|receipt| {
                        receipt.configured_early_stop += 1;
                    });
                    return run_configured(source, limits, decode, profile, 0);
                }
                None => break,
            }
        }
        panic!("configured early-stop checkpoint was not delivered");
    }
    let mut reader = DtdReader::with_decode_limits(source, limits, decode, profile)?;
    let source_text = std::str::from_utf8(source).unwrap();
    let mut retained = Vec::new();
    let mut transcript = Vec::new();
    let mut owned = Vec::new();
    let mut expected_owned_values = Vec::new();
    let mut owned_bytes = 0_usize;
    loop {
        match reader.read_event() {
            Ok(Some((event, origin))) => {
                transcript.push(event_signature(&event));
                if lifecycle & 3 == 1
                    && retained.len() < MAX_RETAINED_OBJECTS
                    && retained
                        .iter()
                        .map(|(held, _)| owned_event_bytes(held))
                        .sum::<usize>()
                        + owned_event_bytes(&event)
                        <= MAX_RETAINED_OWNED
                {
                    retained.push((event, origin));
                } else if lifecycle & 3 == 2 {
                    for value in observed_owned_before_move(&event) {
                        if expected_owned_values.len() < MAX_RETAINED_OBJECTS
                            && expected_owned_values
                                .iter()
                                .map(|value: &OwnedExtraction| value.value.len())
                                .sum::<usize>()
                                + value.value.len()
                                <= MAX_RETAINED_OWNED
                        {
                            expected_owned_values.push(value);
                        }
                    }
                    collect_owned_bounded(event, &mut owned, &mut owned_bytes);
                }
                for (held, held_origin) in &retained {
                    check_event_storage(held, *held_origin, source_text).unwrap();
                }
            }
            Ok(None) => {
                assert_eq!(transcript, literal_transcript(source));
                assert_eq!(reader.position(), source.len());
                assert_eq!(reader.event_origin(), Some(literal_last_origin(source)));
                assert_terminal_sticky_with(&mut reader, Ok(()), || {
                    for (held, held_origin) in &retained {
                        check_event_storage(held, *held_origin, source_text).unwrap();
                    }
                });
                drop(reader);
                for (held, held_origin) in &retained {
                    check_event_storage(held, *held_origin, source_text).unwrap();
                }
                assert!(retained.len() <= MAX_EVENTS);
                assert!(owned_bytes <= MAX_RETAINED_OWNED);
                assert_eq!(
                    owned.iter().map(|value| value.value.len()).sum::<usize>(),
                    owned_bytes
                );
                assert_eq!(owned, expected_owned_values);
                return Ok(());
            }
            Err(error) => {
                assert_terminal_sticky_with(&mut reader, Err(error), || {
                    for (held, held_origin) in &retained {
                        check_event_storage(held, *held_origin, source_text).unwrap();
                    }
                });
                drop(reader);
                for (held, held_origin) in &retained {
                    check_event_storage(held, *held_origin, source_text).unwrap();
                }
                assert!(retained.len() <= MAX_EVENTS);
                return Err(error);
            }
        }
    }
}

fn event_signature(event: &ReaderEvent<'_>) -> String {
    match event {
        ReaderEvent::Dtd(TakenDtdEvent::Begin(_)) => "B".to_owned(),
        ReaderEvent::Dtd(TakenDtdEvent::Item { .. }) => "I".to_owned(),
        ReaderEvent::Dtd(TakenDtdEvent::End) => "D".to_owned(),
        ReaderEvent::Xml(Event::Declaration(_)) => "XML".to_owned(),
        ReaderEvent::Xml(Event::Start(element)) => format!("S:{}", element.name.qualified()),
        ReaderEvent::Xml(Event::Empty(element)) => {
            let attributes = element
                .attributes
                .iter()
                .map(|attribute| {
                    let namespace = attribute
                        .name
                        .namespace()
                        .map(|value| format!("@{value}"))
                        .unwrap_or_default();
                    format!(
                        "{}{namespace}={}",
                        attribute.name.qualified(),
                        attribute.value
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            let namespace = element
                .name
                .namespace()
                .map(|value| format!("@{value}"))
                .unwrap_or_default();
            format!("E:{}{namespace}[{attributes}]", element.name.qualified())
        }
        ReaderEvent::Xml(Event::End(name)) => format!("X:{}", name.qualified()),
        ReaderEvent::Xml(Event::Text(value)) => format!("T:{value}"),
        ReaderEvent::Xml(Event::Cdata(value)) => format!("C:{value}"),
        ReaderEvent::Xml(Event::Comment(value)) => format!("M:{value}"),
        ReaderEvent::Xml(Event::ProcessingInstruction { target, data }) => {
            format!("P:{target}:{data}")
        }
    }
}

fn literal_transcript(source: &[u8]) -> Vec<String> {
    let labels: &[&str] = if source == T || source == B {
        &["B", "D", "E:r[]"]
    } else if source == U {
        &["B", "D", "E:r[a=éé]"]
    } else if source == M || source == A {
        &["B", "I", "D", "E:r[]"]
    } else if source == V {
        &["B", "I", "D", "E:r[a=é]"]
    } else if source == F {
        &["B", "I", "D", "S:r", "T:x", "X:r"]
    } else if source == G {
        &["B", "I", "D", "S:r", "E:n[]", "X:r"]
    } else if source == RP {
        &["B", "I", "I", "I", "D", "E:r[]"]
    } else if source == E {
        &["B", "D", "E:r[a=v]"]
    } else if source == AMP {
        &["B", "D", "E:r[a=&]"]
    } else if source == RL {
        &["B", "I", "D", "S:r", "E:n[]", "E:n[]", "X:r"]
    } else if source == DOC {
        &[
            "B", "I", "I", "D", "S:r", "T:a", "E:n[]", "T:x", "T:b", "T:c", "X:r",
        ]
    } else if source == S {
        &["P:p:a\n", "B", "I", "I", "D", "E:r[]"]
    } else if source == UTF8_TEXT {
        &["B", "D", "S:r", "T:é", "X:r"]
    } else if source == EMOJI_TEXT {
        &["B", "D", "S:r", "T:🙂", "X:r"]
    } else if source == CHAR_REF {
        &["B", "D", "S:r", "T:A", "X:r"]
    } else if source == LEAVES {
        &["B", "D", "S:r", "M:x", "P:p:x", "C:x", "E:n[]", "X:r"]
    } else if source == PE_MODEL {
        &["B", "I", "I", "D", "E:r[]"]
    } else if source == PE_BAD {
        &["B", "I"]
    } else if source == UNKNOWN || source == UNAVAILABLE {
        &["B", "D"]
    } else if source == D {
        &["B", "I", "D", "E:r[a=v]"]
    } else if source == NS {
        &[
            "B",
            "I",
            "D",
            "E:r@urn:d[xmlns@http://www.w3.org/2000/xmlns/=urn:d,a=v]",
        ]
    } else if source == H {
        &["B", "I", "I", "D", "S:r", "T:x", "X:r"]
    } else if source == EMPTY {
        &["B", "I", "I", "D", "S:r", "X:r"]
    } else if source == REPEATED {
        &["B", "D", "S:r", "E:n[a=é]", "T:x", "E:n[a=é]", "T:x", "X:r"]
    } else {
        panic!(
            "missing canonical transcript for {}",
            String::from_utf8_lossy(source)
        );
    };
    labels.iter().map(|label| (*label).to_owned()).collect()
}

fn literal_last_origin(source: &[u8]) -> SourceOrigin {
    let range = if source == B {
        15..19
    } else if source == U {
        12..25
    } else if source == M {
        31..35
    } else if source == A {
        44..48
    } else if source == V {
        31..43
    } else if source == F {
        36..40
    } else if source == G {
        39..43
    } else if source == RP {
        45..49
    } else if source == E {
        12..22
    } else if source == AMP {
        12..26
    } else if source == RL {
        42..46
    } else if source == DOC {
        60..64
    } else if source == S {
        57..61
    } else if source == UTF8_TEXT {
        17..21
    } else if source == EMOJI_TEXT {
        19..23
    } else if source == CHAR_REF {
        20..24
    } else if source == LEAVES {
        47..51
    } else if source == PE_MODEL {
        source.len() - 4..source.len()
    } else if source == D {
        39..43
    } else if source == NS {
        59..63
    } else if source == H {
        55..59
    } else if source == EMPTY {
        52..56
    } else if source == REPEATED {
        39..43
    } else if source == T {
        12..16
    } else {
        panic!("missing literal final origin")
    };
    SourceOrigin::input(range)
}

fn run_expected_failure(
    source: &'static [u8],
    limits: Limits,
    decode: DecodeLimits,
    profile: SessionLimits,
    expected: QuotaFailure,
    lifecycle: u8,
) {
    let mut reader = match DtdReader::with_decode_limits(source, limits, decode, profile) {
        Ok(reader) => reader,
        Err(error) => {
            assert_eq!(quota_diagnostic(error), expected.diagnostic);
            assert_eq!(expected.position, 0);
            assert_eq!(expected.last, None);
            return;
        }
    };
    let source_text = std::str::from_utf8(source).unwrap();
    if lifecycle & 3 == 3 && expected.prefix > 0 {
        let checkpoint = usize::from(lifecycle >> 2).min(expected.prefix - 1);
        for index in 0..=checkpoint {
            let (event, origin) = reader
                .read_event()
                .expect("failure early-stop checkpoint precedes rejection")
                .expect("failure early-stop checkpoint exists");
            assert_eq!(event_signature(&event), literal_transcript(source)[index]);
            check_event_storage(&event, origin, source_text).unwrap();
            if index == checkpoint {
                let inner = reader.into_inner();
                assert_eq!(inner.as_ptr(), source.as_ptr());
                assert_eq!(inner.len(), source.len());
                assert_eq!(event_signature(&event), literal_transcript(source)[index]);
                check_event_storage(&event, origin, source_text).unwrap();
                update_receipt(|receipt| receipt.configured_early_stop += 1);
                run_expected_failure(source, limits, decode, profile, expected, 0);
                return;
            }
        }
    }
    let mut transcript = Vec::new();
    let mut retained = Vec::new();
    let mut owned = Vec::new();
    let mut expected_owned_values = Vec::new();
    let mut owned_bytes = 0_usize;
    loop {
        match reader.read_event() {
            Ok(Some((event, origin))) => {
                transcript.push(event_signature(&event));
                match lifecycle & 3 {
                    1 => {
                        if retained.len() < MAX_EVENTS
                            && retained
                                .iter()
                                .map(|(held, _)| owned_event_bytes(held))
                                .sum::<usize>()
                                + owned_event_bytes(&event)
                                <= MAX_RETAINED_OWNED
                        {
                            retained.push((event, origin));
                        }
                        for (held, held_origin) in &retained {
                            check_event_storage(held, *held_origin, source_text).unwrap();
                        }
                    }
                    2 => {
                        for value in observed_owned_before_move(&event) {
                            if expected_owned_values.len() < MAX_RETAINED_OBJECTS
                                && expected_owned_values
                                    .iter()
                                    .map(|value: &OwnedExtraction| value.value.len())
                                    .sum::<usize>()
                                    + value.value.len()
                                    <= MAX_RETAINED_OWNED
                            {
                                expected_owned_values.push(value);
                            }
                        }
                        collect_owned_bounded(event, &mut owned, &mut owned_bytes);
                    }
                    _ => {}
                }
            }
            Ok(None) => panic!("constrained quota unexpectedly completed"),
            Err(error) => {
                check_quota_failure(
                    QuotaFailure {
                        diagnostic: quota_diagnostic(error),
                        position: reader.position(),
                        last: reader.event_origin(),
                        prefix: transcript.len(),
                    },
                    expected,
                )
                .unwrap();
                assert_eq!(
                    transcript,
                    literal_transcript(source)[..expected.prefix],
                    "rejected transcript prefix"
                );
                assert_terminal_sticky_with(&mut reader, Err(error), || {
                    for (held, held_origin) in &retained {
                        check_event_storage(held, *held_origin, source_text).unwrap();
                    }
                });
                drop(reader);
                for (held, held_origin) in &retained {
                    check_event_storage(held, *held_origin, source_text).unwrap();
                }
                assert!(owned_bytes <= MAX_RETAINED_OWNED);
                assert_eq!(owned, expected_owned_values);
                return;
            }
        }
    }
}

fn exercise_scalar(axis: usize, mode: usize, lifecycle: u8) {
    let (source, boundary) = scalar_fixture(axis);
    run_configured(
        source,
        Limits {
            max_token_bytes: 16,
            ..Limits::default()
        },
        DecodeLimits {
            max_decoded_token_bytes: 16,
            ..DecodeLimits::default()
        },
        quota_profile(),
        lifecycle,
    )
    .expect("unconstrained quota control");

    let ceiling = match mode {
        0 => 0,
        1 => boundary - 1,
        2 => boundary,
        3 => boundary + 1,
        _ => unreachable!(),
    };
    let mut limits = Limits {
        max_token_bytes: 16,
        ..Limits::default()
    };
    let mut decode = DecodeLimits {
        max_decoded_token_bytes: 16,
        ..DecodeLimits::default()
    };
    let mut profile = quota_profile();
    apply_scalar_axis(axis, ceiling, &mut limits, &mut decode, &mut profile);
    if mode >= 2 {
        let result = run_configured(source, limits, decode, profile, lifecycle);
        if let Err(error) = result {
            panic!("scalar quota axis {axis} mode {mode} must accept: {error:?}");
        }
    } else {
        let expected = expected_scalar_failure(axis, mode);
        assert_eq!(
            expected_quota_phase(axis, mode),
            match expected.diagnostic.kind {
                QuotaKind::Xml(_) => TerminalClass::Xml,
                QuotaKind::Dtd(_) => TerminalClass::Dtd,
                QuotaKind::Content(_) => TerminalClass::Content,
            }
        );
        run_expected_failure(source, limits, decode, profile, expected, lifecycle);
    }
}

fn precedence_configuration(
    axis: usize,
    mode: usize,
    bank: usize,
) -> (&'static [u8], Limits, SessionLimits, bool) {
    let mut source = match axis {
        40 | 44 | 46 => T,
        45 => S,
        _ => F,
    };
    let mut limits = Limits {
        max_token_bytes: 16,
        ..Limits::default()
    };
    let mut profile = quota_profile();
    let succeeds = match axis {
        40 => {
            let values = [(35, 11), (36, 11), (48, 12), (49, 12)][mode];
            profile.declarations.max_work = values.0;
            profile.declarations.catalog.max_work = values.1;
            mode >= 2
        }
        41 => {
            let values = [(105, 0), (106, 0), (122, 2), (123, 2)][mode];
            profile.declarations.max_work = values.0;
            profile.declarations.frames.max_work = values.1;
            mode >= 2
        }
        42 => {
            let values = [(104, 8), (105, 8), (122, 9), (123, 9)][mode];
            profile.declarations.max_work = values.0;
            profile.text.reference.max_work = values.1;
            mode >= 2
        }
        43 => {
            let values = [(107, 1), (108, 1), (122, 2), (123, 2)][mode];
            profile.declarations.max_work = values.0;
            profile.local.max_work = values.1;
            mode >= 2
        }
        44 => {
            let values = [(usize::MAX, 0), (11, 11), (48, 12), (49, 12)][mode];
            profile.declarations.max_work = values.0;
            profile.declarations.scan.max_work = values.1;
            mode >= 2
        }
        45 => {
            limits.max_owned_bytes = [0, 12, 13, 14][mode];
            mode >= 2
        }
        46 => {
            profile.text.scratch.max_bytes = usize::MAX;
            match mode {
                0 => profile.text.scratch.input_token_bytes = usize::MAX,
                1 => profile.text.scratch.replacement_token_bytes = usize::MAX,
                2 => {
                    profile.text.reference.max_bytes = usize::MAX;
                    profile.text.reference.max_scratch_bytes = usize::MAX;
                }
                3 => {}
                _ => unreachable!(),
            }
            mode == 3
        }
        47 => configure_bank(bank, mode, &mut source, &mut limits, &mut profile),
        _ => unreachable!(),
    };
    (source, limits, profile, succeeds)
}

fn configure_bank(
    bank: usize,
    mode: usize,
    source: &mut &'static [u8],
    limits: &mut Limits,
    profile: &mut SessionLimits,
) -> bool {
    match bank {
        0 => {
            *source = T;
            let values = [(0, 0, 0), (0, 12, 12), (12, 0, 0), (12, 12, 12)][mode];
            profile.declarations.scan.max_work = values.0;
            profile.declarations.scan.max_unit_bytes = values.1;
            profile.declarations.scan.max_scratch_bytes = values.2;
            mode == 3
        }
        1 => {
            *source = T;
            let values = [(11, 0), (11, 1), (12, 0), (12, 1)][mode];
            profile.declarations.unit.max_unit_bytes = values.0;
            profile.declarations.unit.max_tokens = values.1;
            mode == 3
        }
        2 => {
            *source = if mode == 3 { PE_MODEL } else { PE_BAD };
            let values = [
                (0, 0),
                (0, usize::MAX),
                (usize::MAX, 0),
                (usize::MAX, usize::MAX),
            ][mode];
            profile.declarations.max_owned_bytes = values.0;
            limits.max_owned_bytes = values.1;
            mode == 3
        }
        3 => {
            let values = [(0, 0, 0), (0, 3, 3), (9, 0, 0), (9, 3, 3)][mode];
            profile.text.reference.max_work = values.0;
            profile.text.reference.max_bytes = values.1;
            profile.text.reference.max_scratch_bytes = values.2;
            mode == 3
        }
        4 => {
            *source = E;
            let values = [(0, 0), (0, 1), (1, 0), (1, 1)][mode];
            profile.declarations.catalog.max_expanded_bytes = values.0;
            profile.declarations.catalog.max_value_bytes = values.1;
            mode == 3
        }
        5 => {
            *source = if mode == 3 { E } else { UNKNOWN };
            limits.max_owned_bytes = [0, 1, 64, 0][mode];
            mode == 3
        }
        6 => {
            *source = if mode == 3 { T } else { UNAVAILABLE };
            limits.max_owned_bytes = [0, 1, 64, 64][mode];
            mode == 3
        }
        7 => {
            *source = RP;
            if mode & 1 == 0 {
                profile.declarations.max_owned_bytes = if mode == 0 { 15 } else { 16 };
            } else {
                limits.max_owned_bytes = if mode == 1 { 15 } else { 16 };
            }
            mode >= 2
        }
        _ => unreachable!(),
    }
}

fn expected_precedence_failure(axis: usize, mode: usize, bank: usize) -> QuotaFailure {
    let input = |range| input_origin(range);
    let replacement = |d, i, local, physical| Some(replacement_origin(d, i, local, physical));
    let (diagnostic, position, last) = match axis {
        40 => (
            qdiag(
                QuotaKind::Dtd(if mode == 0 {
                    ControllerErrorKind::Limit(ControllerLimit::Work)
                } else {
                    ControllerErrorKind::Catalog(CatalogErrorKind::Limit(CatalogLimit::Work))
                }),
                0,
            )
            .origin(input_fragment(0..12))
            .value(ValueLocation::Declaration {
                declaration: 0,
                offset: 0,
            }),
            12,
            None,
        ),
        41 => (
            qdiag(
                QuotaKind::Content(if mode == 0 {
                    ContentErrorKind::Work
                } else {
                    ContentErrorKind::Frame(FrameErrorKind::LimitExceeded(FrameLimit::Work))
                }),
                33,
            )
            .origin(input_fragment(33..36)),
            36,
            input(30..33),
        ),
        42 => (
            qdiag(
                QuotaKind::Content(if mode == 0 {
                    ContentErrorKind::Work
                } else {
                    ContentErrorKind::Text(TextErrorKind::Reference(ReferenceErrorKind::Limit(
                        ReferenceLimit::Work,
                    )))
                }),
                33,
            ),
            36,
            input(30..33),
        ),
        43 => (
            qdiag(
                QuotaKind::Content(if mode == 0 {
                    ContentErrorKind::Work
                } else {
                    ContentErrorKind::Text(TextErrorKind::Local(LocalErrorKind::Work))
                }),
                33,
            )
            .origin(replacement_fragment(0, 1, 0..0, 33..36))
            .local(0),
            36,
            input(30..33),
        ),
        44 if mode == 0 => (
            qdiag(
                QuotaKind::Dtd(ControllerErrorKind::Scan(ScanErrorKind::Limit(
                    ScanLimit::Work,
                ))),
                0,
            ),
            0,
            None,
        ),
        44 => (
            qdiag(
                QuotaKind::Dtd(ControllerErrorKind::Limit(ControllerLimit::Work)),
                11,
            ),
            11,
            None,
        ),
        45 if mode == 0 => (
            qdiag(
                QuotaKind::Xml(ErrorKind::LimitExceeded(Limit::OwnedBytes)),
                4,
            ),
            9,
            None,
        ),
        45 => (
            qdiag(
                QuotaKind::Dtd(ControllerErrorKind::Ownership(ErrorKind::LimitExceeded(
                    Limit::OwnedBytes,
                ))),
                52,
            )
            .origin(replacement_fragment(0, 1, 0..10, 52..55))
            .local(0),
            55,
            input(22..52),
        ),
        46 => (
            qdiag(
                QuotaKind::Content(ContentErrorKind::Text(TextErrorKind::ScratchOverflow)),
                12,
            ),
            12,
            input(11..12),
        ),
        47 => match bank {
            0 => {
                let (limit, physical) = if mode == 2 {
                    (ScanLimit::UnitBytes, 0)
                } else {
                    (ScanLimit::Work, 0)
                };
                (
                    qdiag(
                        QuotaKind::Dtd(ControllerErrorKind::Scan(ScanErrorKind::Limit(limit))),
                        physical,
                    ),
                    0,
                    None,
                )
            }
            1 => {
                let limit = if mode == 2 {
                    DtdLimit::Tokens
                } else {
                    DtdLimit::UnitBytes
                };
                (
                    qdiag(
                        QuotaKind::Dtd(ControllerErrorKind::Grammar(DtdErrorKind::LimitExceeded(
                            limit,
                        ))),
                        if mode == 2 { 10 } else { 11 },
                    )
                    .origin(input_fragment(0..12)),
                    12,
                    None,
                )
            }
            2 => (
                qdiag(
                    QuotaKind::Dtd(ControllerErrorKind::Grammar(DtdErrorKind::Syntax)),
                    41,
                )
                .origin(replacement_fragment(0, 1, 0..12, 41..44))
                .local(11),
                44,
                input(13..41),
            ),
            3 => {
                let kind = if mode == 2 {
                    ReferenceErrorKind::Limit(ReferenceLimit::Bytes)
                } else {
                    ReferenceErrorKind::Limit(ReferenceLimit::Work)
                };
                (
                    qdiag(
                        QuotaKind::Content(ContentErrorKind::Text(TextErrorKind::Reference(kind))),
                        33,
                    ),
                    33,
                    input(30..33),
                )
            }
            4 => {
                let limit = if mode == 2 {
                    CatalogLimit::ValueBytes
                } else {
                    CatalogLimit::ExpandedBytes
                };
                (
                    qdiag(
                        QuotaKind::Content(ContentErrorKind::Tag(DtdTagErrorKind::Catalog(
                            CatalogErrorKind::Limit(limit),
                        ))),
                        18,
                    )
                    .origin(input_fragment(12..22))
                    .value(ValueLocation::Input(18)),
                    22,
                    input(11..12),
                )
            }
            5 => (
                qdiag(
                    QuotaKind::Content(ContentErrorKind::Tag(DtdTagErrorKind::Catalog(
                        CatalogErrorKind::EntityDeclared,
                    ))),
                    19,
                )
                .origin(input_fragment(12..25))
                .value(ValueLocation::Input(19)),
                25,
                input(11..12),
            ),
            6 => (
                qdiag(
                    QuotaKind::Content(ContentErrorKind::Tag(DtdTagErrorKind::Unavailable(
                        DependencyKind::UnreadEntity,
                    ))),
                    30,
                )
                .origin(input_fragment(23..36))
                .value(ValueLocation::Input(30)),
                36,
                input(22..23),
            ),
            7 => (
                qdiag(
                    QuotaKind::Dtd(if mode == 0 {
                        ControllerErrorKind::Limit(ControllerLimit::OwnedBytes)
                    } else {
                        ControllerErrorKind::Ownership(ErrorKind::LimitExceeded(Limit::OwnedBytes))
                    }),
                    40,
                )
                .origin(replacement_fragment(0, 2, 0..8, 40..43))
                .local(0),
                43,
                replacement(0, 1, 0..8, 37..40),
            ),
            _ => unreachable!(),
        },
        _ => unreachable!(),
    };
    QuotaFailure {
        diagnostic,
        position,
        last,
        prefix: match axis {
            40 | 44 => 0,
            41..=43 => 4,
            45 => {
                if mode == 0 {
                    0
                } else {
                    3
                }
            }
            46 => 2,
            47 => match bank {
                0 | 1 => 0,
                2 => 2,
                3 => 4,
                4..=6 => 2,
                7 => 3,
                _ => unreachable!(),
            },
            _ => unreachable!(),
        },
    }
}

fn exercise_precedence(axis: usize, mode: usize, bank: usize, lifecycle: u8) {
    let positive = match (axis, bank) {
        (47, 0 | 1) => T,
        (47, 2) => PE_MODEL,
        (47, 5) => E,
        (47, 6) => T,
        (45, _) => S,
        (47, 4) => E,
        (47, 7) => RP,
        (40 | 44 | 46, _) => T,
        _ => F,
    };
    run_configured(
        positive,
        Limits {
            max_token_bytes: 16,
            ..Limits::default()
        },
        DecodeLimits::default(),
        quota_profile(),
        lifecycle,
    )
    .expect("unconstrained precedence control");

    let (source, limits, profile, succeeds) = precedence_configuration(axis, mode, bank);
    if succeeds {
        let result = run_configured(source, limits, DecodeLimits::default(), profile, lifecycle);
        if let Err(error) = result {
            panic!("precedence axis {axis} bank {bank} mode {mode} must accept: {error:?}");
        }
    } else {
        run_expected_failure(
            source,
            limits,
            DecodeLimits::default(),
            profile,
            expected_precedence_failure(axis, mode, bank),
            lifecycle,
        );
    }
}

pub(super) fn exercise_quota(axis_selector: u8, mode_selector: u8, shape: u8, lifecycle: u8) {
    let (axis, mode, bank) = quota_selection(axis_selector, mode_selector, shape);
    update_receipt(|receipt| {
        receipt.quota = Some((axis_selector, axis, mode, bank, lifecycle));
    });
    if axis < 40 {
        exercise_scalar(axis, mode, lifecycle);
    } else {
        exercise_precedence(axis, mode, bank, lifecycle);
    }
}

#[test]
fn truncation_selector_covers_every_point_and_alias() {
    for family in 0_u8..8 {
        for point in 0..TRUNCATION_CUTS[usize::from(family)].len() {
            for lifecycle in 0_u8..4 {
                exercise_truncation(family | ((point as u8) << 3), lifecycle);
            }
        }
    }
    exercise_truncation(u8::MAX, u8::MAX);
}

#[test]
fn every_selector_alias_decodes_inside_frozen_bounds() {
    for byte in 0_u8..=u8::MAX {
        let (family, point, cut) = truncation_selection(byte);
        assert!(family < TRUNCATION_CUTS.len());
        assert!(point < TRUNCATION_CUTS[family].len());
        assert_eq!(cut, TRUNCATION_CUTS[family][point]);

        let (axis, mode, bank) = quota_selection(byte, byte, byte);
        assert!(axis < 48);
        assert!(mode < 4);
        assert!(bank < 8);
        assert!(usize::from(byte >> 2) <= 63);
    }
}

#[test]
fn rejecting_profiles_cross_retained_extracted_and_early_stop_lifecycles() {
    for axis in 0..40 {
        for mode in 0..2 {
            for lifecycle in 0..4 {
                exercise_scalar(axis, mode, lifecycle);
            }
        }
    }
    for axis in 40..48 {
        let banks = if axis == 47 { 0..8 } else { 0..1 };
        for bank in banks {
            for mode in 0..4 {
                for lifecycle in 0..4 {
                    exercise_precedence(axis, mode, bank, lifecycle);
                }
            }
        }
    }
}

#[test]
fn quota_selector_covers_every_axis_mode_bank_and_alias() {
    for axis in 0_u8..48 {
        for mode in 0_u8..4 {
            if axis == 47 {
                for bank in 0_u8..8 {
                    exercise_quota(axis, mode, bank, mode);
                }
            } else {
                exercise_quota(axis, mode, 0, mode);
            }
        }
    }
    exercise_quota(u8::MAX, u8::MAX, u8::MAX, u8::MAX);
}

#[test]
fn selector_zero_and_success_cells_are_distinguished() {
    for axis in 0_u8..40 {
        exercise_quota(axis, 0, 0, 0);
        exercise_quota(axis, 2, 0, 0);
    }
}

#[test]
fn owned_retention_cap_is_literal_and_finite() {
    assert_eq!(16 * 1024, MAX_LOGICAL_EXPANSION);
    drain_with_lifecycle(F, 2).unwrap();
}

#[test]
fn retained_namespace_accounting_bounds_eighty_large_generated_names() {
    let uri = "u".repeat(1024);
    let source = format!(
        "<!DOCTYPE p:r [<!ENTITY e '<p:n/>'>]><p:r xmlns:p='{uri}'>{}</p:r>",
        "&e;".repeat(80)
    );
    let mut reader = DtdReader::new(source.as_bytes(), options()).unwrap();
    let mut retained = Vec::new();
    let mut retained_bytes = 0_usize;
    let mut generated = 0_usize;
    while let Some((event, origin)) = reader.read_event().unwrap() {
        if matches!(&event, ReaderEvent::Xml(Event::Empty(element)) if element.name.qualified() == "p:n")
        {
            generated += 1;
            let bytes = owned_event_bytes(&event);
            assert_eq!(bytes, uri.len() + "p:n".len());
            if retained.len() < MAX_RETAINED_OBJECTS && retained_bytes + bytes <= MAX_RETAINED_OWNED
            {
                retained_bytes += bytes;
                retained.push((event, origin));
            }
        }
    }
    assert_eq!(generated, 80);
    assert_eq!(retained.len(), MAX_RETAINED_OWNED / (uri.len() + 3));
    assert!(retained_bytes <= MAX_RETAINED_OWNED);
    assert!(retained_bytes + uri.len() + 3 > MAX_RETAINED_OWNED);
    let names = retained
        .into_iter()
        .map(|(event, _)| match event {
            ReaderEvent::Xml(Event::Empty(element)) => element.name.try_into_static(0).unwrap(),
            _ => unreachable!(),
        })
        .collect::<Vec<_>>();
    drop(reader);
    drop(source);
    assert!(
        names
            .iter()
            .all(|name| name.qualified() == "p:n" && name.namespace() == Some(uri.as_str()))
    );
}

#[test]
fn extraction_moves_normalized_pi_and_generated_dtd_raw_before_source_drop() {
    let source = String::from_utf8(S.to_vec()).unwrap();
    let mut reader = DtdReader::new(source.as_bytes(), options()).unwrap();
    let mut extracted = Vec::new();
    let mut bytes = 0_usize;
    while let Some((event, _)) = reader.read_event().unwrap() {
        collect_owned_bounded(event, &mut extracted, &mut bytes);
    }
    drop(reader);
    drop(source);
    assert_eq!(
        extracted,
        [
            OwnedExtraction {
                role: FieldRole::PiData,
                value: "a\n".to_owned(),
            },
            OwnedExtraction {
                role: FieldRole::DtdRaw,
                value: "<!--a\rb-->".to_owned(),
            },
        ]
    );
    assert_eq!(bytes, "a\n".len() + "<!--a\rb-->".len());
}

#[test]
fn full_origin_oracle_rejects_reused_declaration_and_invocation_ids() {
    let physical = 33..36;
    let expected = replacement(
        ExpectedKind::Text("x".to_owned()),
        1,
        2,
        0..1,
        physical.clone(),
    )
    .origin;
    let wrong_declaration = replacement(
        ExpectedKind::Text("x".to_owned()),
        0,
        2,
        0..1,
        physical.clone(),
    )
    .origin
    .full;
    let wrong_invocation = replacement(ExpectedKind::Text("x".to_owned()), 1, 1, 0..1, physical)
        .origin
        .full;

    assert_eq!(check_expected_origin(expected.full, &expected), Ok(()));
    assert!(check_expected_origin(wrong_declaration, &expected).is_err());
    assert!(check_expected_origin(wrong_invocation, &expected).is_err());
}

#[test]
fn canonical_dtd_view_oracle_rejects_same_raw_semantic_mutations() {
    let source = std::str::from_utf8(F).unwrap();
    let expected = canonical_expected(F);
    let mut reader = DtdReader::new(F, options()).unwrap();
    reader.read_event().unwrap().unwrap();
    let (entity, origin) = reader.read_event().unwrap().unwrap();
    check_expected_event(&entity, origin, &expected[1], source).unwrap();
    for wrong in [
        entity_semantic(true, "e", "'x'"),
        entity_semantic(false, "z", "'x'"),
        entity_semantic(false, "e", "'y'"),
    ] {
        let mut mutated = expected[1].clone();
        mutated.dtd = Some(wrong);
        assert!(check_expected_event(&entity, origin, &mutated, source).is_err());
    }
    let mut wrong_offset = expected[1].clone();
    if let Some(DtdSemantic::Entity { quoted_offset, .. }) = &mut wrong_offset.dtd {
        *quoted_offset = Some(10);
    }
    assert!(check_expected_event(&entity, origin, &wrong_offset, source).is_err());

    let source = std::str::from_utf8(D).unwrap();
    let expected = canonical_expected(D);
    let mut reader = DtdReader::new(D, options()).unwrap();
    reader.read_event().unwrap().unwrap();
    let (attlist, origin) = reader.read_event().unwrap().unwrap();
    check_expected_event(&attlist, origin, &expected[1], source).unwrap();
    for definitions in [
        Vec::new(),
        vec![ExpectedAttributeDefinition {
            name: "z".to_owned(),
            name_offset: 12,
            default: "'v'".to_owned(),
            default_offset: Some(20),
        }],
        vec![ExpectedAttributeDefinition {
            name: "a".to_owned(),
            name_offset: 12,
            default: "'x'".to_owned(),
            default_offset: Some(20),
        }],
    ] {
        let mut mutated = expected[1].clone();
        mutated.dtd = Some(DtdSemantic::AttributeList {
            element: "r".to_owned(),
            definitions,
        });
        assert!(check_expected_event(&attlist, origin, &mutated, source).is_err());
    }
    let mut wrong_offset = expected[1].clone();
    if let Some(DtdSemantic::AttributeList { definitions, .. }) = &mut wrong_offset.dtd {
        definitions[0].name_offset = 13;
    }
    assert!(check_expected_event(&attlist, origin, &wrong_offset, source).is_err());

    let source = std::str::from_utf8(EMPTY).unwrap();
    let expected = canonical_expected(EMPTY);
    let mut reader = DtdReader::new(EMPTY, options()).unwrap();
    reader.read_event().unwrap().unwrap();
    let (entity, origin) = reader.read_event().unwrap().unwrap();
    check_expected_event(&entity, origin, &expected[1], source).unwrap();
    let mut missing_reference = expected[1].clone();
    missing_reference.dtd = Some(DtdSemantic::Entity {
        parameter: false,
        name: "e".to_owned(),
        quoted: "'&z;'".to_owned(),
        quoted_offset: Some(11),
        references: Vec::new(),
    });
    assert!(check_expected_event(&entity, origin, &missing_reference, source).is_err());
}

#[test]
fn empty_invocation_advances_the_next_visible_occurrence_id() {
    let source = std::str::from_utf8(VISIBLE_EMPTY).unwrap();
    let expected = canonical_expected(VISIBLE_EMPTY);
    let mut reader = DtdReader::new(VISIBLE_EMPTY, options()).unwrap();
    for (index, expected_event) in expected.iter().enumerate() {
        let (event, origin) = reader.read_event().unwrap().unwrap();
        check_expected_event(&event, origin, expected_event, source).unwrap();
        if index == 6 {
            let mut skipped_empty = expected_event.clone();
            skipped_empty.origin =
                replacement(ExpectedKind::Text("x".to_owned()), 0, 2, 0..1, 53..56).origin;
            assert!(check_expected_event(&event, origin, &skipped_empty, source).is_err());
        }
    }
    assert!(reader.read_event().unwrap().is_none());
}

#[test]
fn exact_diagnostic_oracle_rejects_every_envelope_mutation() {
    let baseline = expected_scalar_failure(39, 1);
    let variants = [
        QuotaFailure {
            diagnostic: QuotaDiagnostic {
                kind: QuotaKind::Content(ContentErrorKind::Text(TextErrorKind::Work)),
                ..baseline.diagnostic
            },
            ..baseline
        },
        QuotaFailure {
            diagnostic: QuotaDiagnostic {
                physical: baseline.diagnostic.physical + 1,
                ..baseline.diagnostic
            },
            ..baseline
        },
        QuotaFailure {
            diagnostic: QuotaDiagnostic {
                origin: None,
                ..baseline.diagnostic
            },
            ..baseline
        },
        QuotaFailure {
            diagnostic: QuotaDiagnostic {
                local: None,
                ..baseline.diagnostic
            },
            ..baseline
        },
        QuotaFailure {
            diagnostic: QuotaDiagnostic {
                value: Some(ValueLocation::Input(0)),
                ..baseline.diagnostic
            },
            ..baseline
        },
        QuotaFailure {
            position: baseline.position + 1,
            ..baseline
        },
        QuotaFailure {
            last: None,
            ..baseline
        },
        QuotaFailure {
            prefix: baseline.prefix - 1,
            ..baseline
        },
    ];
    assert_eq!(check_quota_failure(baseline, baseline), Ok(()));
    for variant in variants {
        assert!(check_quota_failure(variant, baseline).is_err());
    }
}

#[test]
fn literal_transcript_oracle_rejects_value_order_and_success_mutations() {
    let expected = canonical_expected(D);
    let source = std::str::from_utf8(D).unwrap();
    let mut reader = DtdReader::new(D, options()).unwrap();
    for expected_event in &expected[..3] {
        let (event, origin) = reader.read_event().unwrap().unwrap();
        check_expected_event(&event, origin, expected_event, source).unwrap();
    }
    let (event, origin) = reader.read_event().unwrap().unwrap();
    check_expected_event(&event, origin, &expected[3], source).unwrap();
    let mut wrong_value = expected[3].clone();
    wrong_value.kind = ExpectedKind::Empty(
        "r".to_owned(),
        None,
        vec![ExpectedAttribute {
            name: "a".to_owned(),
            namespace: None,
            value: "x".to_owned(),
        }],
    );
    assert!(check_expected_event(&event, origin, &wrong_value, source).is_err());

    let failure = expected_truncation(4, 35, F.len());
    let actual = TruncationObservation {
        class: failure.class,
        diagnostic: failure.diagnostic,
        delivered: failure.delivered,
        position: failure.position,
        last: failure.last,
    };
    assert_eq!(check_truncation_observation(actual, failure), Ok(()));
    assert!(
        check_truncation_observation(
            TruncationObservation {
                diagnostic: None,
                ..actual
            },
            failure,
        )
        .is_err()
    );
    assert!(
        check_truncation_observation(
            TruncationObservation {
                class: TerminalClass::Complete,
                ..actual
            },
            failure,
        )
        .is_err()
    );
}

#[test]
fn lifecycle_modes_retain_extract_and_early_stop_all_witnesses() {
    for lifecycle in 0_u8..4 {
        exercise_witness_bank(lifecycle);
        let mut controls = [0_u8; 64];
        controls[0] = lifecycle;
        controls[1] = 4;
        controls[2] = 8;
        controls[3] = 3;
        controls[7] = lifecycle | (2 << 2);
        run_generated_lifecycle(generated_case(&controls), controls[7]);
    }
}

#[test]
fn equal_physical_tokens_keep_distinct_literal_pointers() {
    let source = std::str::from_utf8(REPEATED).unwrap();
    let mut reader = DtdReader::new(REPEATED, options()).unwrap();
    let expected = canonical_expected(REPEATED);
    let mut actual = Vec::new();
    for expected_event in &expected {
        let (event, origin) = reader.read_event().unwrap().unwrap();
        check_expected_event(&event, origin, expected_event, source).unwrap();
        actual.push((event, origin));
    }
    assert!(reader.read_event().unwrap().is_none());
    drop(reader);
    for ((event, origin), expected_event) in actual.iter().zip(&expected) {
        check_expected_event(event, *origin, expected_event, source).unwrap();
    }

    let mut wrong_first = expected[3].clone();
    wrong_first.fields[0].storage = ExpectedStorage::Borrowed(28..29);
    assert!(check_expected_event(&actual[3].0, actual[3].1, &wrong_first, source).is_err());
    let mut wrong_value = expected[3].clone();
    wrong_value.fields[2].storage = ExpectedStorage::Borrowed(33..35);
    assert!(check_expected_event(&actual[3].0, actual[3].1, &wrong_value, source).is_err());
    let mut wrong_text = expected[4].clone();
    wrong_text.fields[0].storage = ExpectedStorage::Borrowed(38..39);
    assert!(check_expected_event(&actual[4].0, actual[4].1, &wrong_text, source).is_err());
}
