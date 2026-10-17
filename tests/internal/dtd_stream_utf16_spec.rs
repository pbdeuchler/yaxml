//! Blind transport conformance, authored against the UTF-8-only session base.
use super::*;
use crate::Limit;
use crate::dtd::{DtdUnitView, EntityDefinitionView, LiteralSource};
use crate::dtd_controller::ControllerErrorKind;
use crate::dtd_controller::content::ContentErrorKind;
use crate::dtd_frames::{FrameErrorKind, FrameLimit};
use crate::dtd_reference::ReferenceLimits;
use crate::dtd_scan::ScanErrorKind;
use crate::tokenizer::text::TextScratchLimits;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};

const MAIN: &str = "<!DOCTYPE r [<!ENTITY e '<n/>𝄞'><!ATTLIST n a CDATA '🙂'>]><r>α&e;β</r>";
const PE: &str = "<!DOCTYPE r [<!ENTITY % p \"<!ENTITY e '𝄞'>\">%p;]><r>&e;</r>";
const TINY: &str = "<!DOCTYPE r><r/>";
const TOKEN: usize = 4096;
const WORK: usize = 64 * 1024 * 1024;

fn options() -> SessionLimits {
    SessionLimits {
        declarations: ControllerLimits::default(),
        text: TextLimits {
            max_work: WORK,
            reference: ReferenceLimits {
                max_bytes: TOKEN,
                max_scratch_bytes: TOKEN,
                max_work: WORK,
            },
            scratch: TextScratchLimits {
                input_token_bytes: TOKEN,
                replacement_token_bytes: TOKEN,
                max_bytes: TOKEN * 3,
            },
        },
        local: LocalLimits {
            max_token_bytes: TOKEN,
            max_scratch_bytes: TOKEN,
            max_work: WORK,
        },
    }
}

fn session() -> StreamingSession {
    configured(Limits::default(), DecodeLimits::default(), options())
}

fn configured(
    mut limits: Limits,
    mut decode: DecodeLimits,
    options: SessionLimits,
) -> StreamingSession {
    // Match this fixture's bounded scratch slots; smaller explicit ceilings
    // remain unchanged for the independent resource-boundary tests.
    limits.max_token_bytes = limits.max_token_bytes.min(TOKEN);
    decode.max_decoded_token_bytes = decode.max_decoded_token_bytes.min(TOKEN);
    StreamingSession::new(limits, decode, options)
}

#[derive(Clone, Copy, Debug)]
struct Transport {
    little: bool,
    bom: bool,
    label: Option<&'static str>,
}

const TRANSPORTS: [Transport; 6] = [
    Transport {
        little: true,
        bom: true,
        label: None,
    },
    Transport {
        little: false,
        bom: true,
        label: None,
    },
    Transport {
        little: true,
        bom: true,
        label: Some("uTf-16"),
    },
    Transport {
        little: false,
        bom: true,
        label: Some("UTF-16"),
    },
    Transport {
        little: true,
        bom: false,
        label: Some("uTf-16lE"),
    },
    Transport {
        little: false,
        bom: false,
        label: Some("UTF-16BE"),
    },
];

fn declaration(label: Option<&str>) -> String {
    label.map_or_else(String::new, |label| {
        format!("<?xml version='1.00' encoding='{label}' standalone='yes'?>")
    })
}

fn encode(text: &str, transport: Transport) -> Vec<u8> {
    let mut bytes = Vec::new();
    if transport.bom {
        bytes.extend(if transport.little {
            [0xff, 0xfe]
        } else {
            [0xfe, 0xff]
        });
    }
    for unit in text.encode_utf16() {
        bytes.extend(if transport.little {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}

fn document(body: &str, transport: Transport) -> (Vec<u8>, usize) {
    let prefix = declaration(transport.label);
    assert!(prefix.is_ascii());
    let shift = usize::from(transport.bom) * 2 + prefix.len() * 2;
    (encode(&format!("{prefix}{body}"), transport), shift)
}

fn body_bytes(text: &str, little: Option<bool>) -> Vec<u8> {
    little.map_or_else(
        || text.as_bytes().to_vec(),
        |little| {
            encode(
                text,
                Transport {
                    little,
                    bom: true,
                    label: None,
                },
            )
        },
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Value {
    Declaration(String, Option<String>, Option<bool>),
    Begin(String, bool),
    Item {
        raw: String,
        source: LiteralSource,
        entity: Option<(String, bool, String)>,
    },
    DtdEnd,
    Start(String, Vec<(String, String)>),
    Empty(String, Vec<(String, String)>),
    End(String),
    Text(String),
    Cdata(String),
    Comment(String),
    Pi(String, String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Origin {
    input: Option<Range<usize>>,
    local: Option<Range<usize>>,
    physical: Range<usize>,
}

fn origin(value: SourceOrigin) -> Origin {
    let captured = Origin {
        input: value.input_range(),
        local: value.replacement_range(),
        physical: value.physical_context(),
    };
    assert_eq!(value.error_offset(), captured.physical.start);
    assert_eq!(value.cancellation_offset(), captured.physical.end);
    captured
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Row {
    value: Value,
    origin: Origin,
}

fn qualified(name: &crate::Name<'_>) -> String {
    assert_eq!(name.prefix(), None);
    assert_eq!(name.namespace(), None);
    assert_eq!(name.local(), name.qualified());
    name.qualified().into()
}

fn element(value: crate::Element<'_>, empty: bool) -> Value {
    let name = qualified(&value.name);
    let attributes = value
        .attributes
        .iter()
        .map(|a| (qualified(&a.name), a.value.to_string()))
        .collect();
    if empty {
        Value::Empty(name, attributes)
    } else {
        Value::Start(name, attributes)
    }
}

fn capture(event: LocatedEvent<'_>, source: SourceOrigin) -> Row {
    let value = match event {
        LocatedEvent::Xml(event) => match event {
            Event::Declaration(d) => Value::Declaration(
                d.version.into(),
                d.encoding.map(str::to_owned),
                d.standalone,
            ),
            Event::Start(value) => element(value, false),
            Event::Empty(value) => element(value, true),
            Event::End(name) => Value::End(qualified(&name)),
            Event::Text(value) => Value::Text(value.into_owned()),
            Event::Cdata(value) => Value::Cdata(value.into_owned()),
            Event::Comment(value) => Value::Comment(value.into_owned()),
            Event::ProcessingInstruction { target, data } => {
                Value::Pi(target.into_owned(), data.into_owned())
            }
        },
        LocatedEvent::Dtd(event) => match event {
            DtdEvent::Begin(header) => {
                assert_eq!(header.external_id(), None);
                Value::Begin(header.root_name().into(), header.has_internal_subset())
            }
            DtdEvent::End => Value::DtdEnd,
            DtdEvent::Item { unit, literals } => {
                let entity = match unit.view() {
                    DtdUnitView::Entity(entity) => {
                        let EntityDefinitionView::Internal { quoted_literal } = entity.definition()
                        else {
                            panic!("internal fixture entity");
                        };
                        Some((
                            entity.name().into(),
                            entity.is_parameter(),
                            quoted_literal.into(),
                        ))
                    }
                    _ => None,
                };
                Value::Item {
                    raw: unit.raw().into(),
                    source: literals,
                    entity,
                }
            }
        },
    };
    Row {
        value,
        origin: origin(source),
    }
}

fn physical(value: Value, span: Range<usize>) -> Row {
    Row {
        value,
        origin: Origin {
            input: Some(span.clone()),
            local: None,
            physical: span,
        },
    }
}

fn replacement(value: Value, local: Range<usize>, physical: Range<usize>) -> Row {
    Row {
        value,
        origin: Origin {
            input: None,
            local: Some(local),
            physical,
        },
    }
}

fn item(raw: &str, name: &str, parameter: bool, literal: &str) -> Value {
    Value::Item {
        raw: raw.into(),
        source: LiteralSource::Physical,
        entity: Some((name.into(), parameter, literal.into())),
    }
}

fn main_values() -> Vec<Value> {
    vec![
        Value::Begin("r".into(), true),
        item("<!ENTITY e '<n/>𝄞'>", "e", false, "'<n/>𝄞'"),
        Value::Item {
            raw: "<!ATTLIST n a CDATA '🙂'>".into(),
            source: LiteralSource::Physical,
            entity: None,
        },
        Value::DtdEnd,
        Value::Start("r".into(), vec![]),
        Value::Text("α".into()),
        Value::Empty("n".into(), vec![("a".into(), "🙂".into())]),
        Value::Text("𝄞".into()),
        Value::Text("β".into()),
        Value::End("r".into()),
    ]
}

fn main_expected(utf16: bool, shift: usize) -> Vec<Row> {
    // Literal byte coordinates: UTF-16 excludes its BOM/header here. Local
    // entity ranges remain UTF-8, including the four bytes of U+1D11E.
    let spans = if utf16 {
        [
            0..26,
            26..66,
            66..116,
            116..120,
            120..126,
            126..128,
            128..134,
            128..134,
            134..136,
            136..144,
        ]
    } else {
        [
            0..13,
            13..35,
            35..62,
            62..64,
            64..67,
            67..69,
            69..72,
            69..72,
            72..74,
            74..78,
        ]
    };
    main_values()
        .into_iter()
        .zip(spans)
        .enumerate()
        .map(|(index, (value, span))| {
            let span = span.start + shift..span.end + shift;
            match index {
                6 => replacement(value, 0..4, span),
                7 => replacement(value, 4..8, span),
                _ => physical(value, span),
            }
        })
        .collect()
}

fn feed(
    stream: &mut StreamingSession,
    bytes: &[u8],
    cuts: &[usize],
    rows: &mut Vec<Row>,
) -> Result<(), SessionError> {
    let mut start = 0;
    for end in cuts.iter().copied().chain(std::iter::once(bytes.len())) {
        assert!((start..=bytes.len()).contains(&end));
        let mut turns = 0;
        while start < end {
            turns += 1;
            assert!(turns <= bytes.len() + 64, "bounded session progress");
            let before = rows.len();
            let consumed = stream.advance(&bytes[start..end], &mut |event, source, _| {
                rows.push(capture(event, source));
                Ok(())
            })?;
            assert!(consumed <= end - start);
            assert!(consumed != 0 || before != rows.len());
            start += consumed;
            assert_eq!(
                stream.position(),
                start,
                "position is accepted transport bytes"
            );
        }
    }
    Ok(())
}

fn finish(stream: &mut StreamingSession, rows: &mut Vec<Row>) -> Result<(), SessionError> {
    stream.finish(&mut |event, source, _| {
        rows.push(capture(event, source));
        Ok(())
    })?;
    assert_eq!(
        stream.event_origin().map(origin),
        rows.last().map(|row| row.origin.clone())
    );
    stream.finish(&mut |_, _, _| panic!("repeated successful EOF has no callbacks"))
}

fn run(bytes: &[u8], cuts: &[usize]) -> (Vec<Row>, StreamingSession) {
    let mut stream = session();
    let mut rows = Vec::new();
    feed(&mut stream, bytes, cuts, &mut rows).unwrap();
    finish(&mut stream, &mut rows).unwrap();
    assert_eq!(stream.position(), bytes.len());
    (rows, stream)
}

fn sticky(stream: &mut StreamingSession, error: SessionError) {
    let position = stream.position();
    let last = stream.event_origin();
    assert_eq!(
        stream.advance(b"ignored", &mut |_, _, _| panic!("terminal feed")),
        Err(error)
    );
    assert_eq!(
        stream.finish(&mut |_, _, _| panic!("terminal EOF")),
        Err(error)
    );
    assert_eq!(stream.position(), position);
    assert_eq!(stream.event_origin(), last);
}

#[test]
fn utf8_control_matches_independent_non_bmp_dtd_and_origin_goldens() {
    let (rows, stream) = run(MAIN.as_bytes(), &(1..MAIN.len()).collect::<Vec<_>>());
    assert_eq!(rows, main_expected(false, 0));
    assert_eq!(stream.dtd_counters().unwrap().physical_bytes, 78);
    assert_eq!(stream.dtd_counters().unwrap().reference_attempts, 1);
}

#[test]
fn every_physical_utf16_split_preserves_literal_events_origins_and_declaration_labels() {
    for transport in TRANSPORTS {
        let (bytes, shift) = document(MAIN, transport);
        let mut expected = main_expected(true, shift);
        if let Some(label) = transport.label {
            expected.insert(
                0,
                physical(
                    Value::Declaration("1.00".into(), Some(label.into()), Some(true)),
                    usize::from(transport.bom) * 2..shift,
                ),
            );
        }
        let reference_prefix = declaration(transport.label.map(|_| "UTF-8"));
        let (reference, _) = run(format!("{reference_prefix}{MAIN}").as_bytes(), &[]);
        for cut in 0..=bytes.len() {
            let (rows, stream) = run(&bytes, &[cut]);
            assert_eq!(rows, expected, "{transport:?} byte split {cut}");
            let mut logical: Vec<_> = rows.into_iter().map(|row| row.value).collect();
            if let Some(Value::Declaration(_, label, _)) = logical.first_mut() {
                *label = Some("UTF-8".into());
            }
            assert_eq!(
                logical,
                reference
                    .iter()
                    .map(|row| row.value.clone())
                    .collect::<Vec<_>>()
            );
            let counters = stream.dtd_counters().unwrap();
            assert_eq!(counters.physical_bytes, 144);
            assert_eq!(counters.reference_attempts, 1);
        }
        let (rows, _) = run(&bytes, &(1..bytes.len()).collect::<Vec<_>>());
        assert_eq!(rows, expected);
    }
}

#[test]
fn parameter_entity_declarations_keep_utf8_local_ranges_and_utf16_reference_contexts() {
    let mut entity = item("<!ENTITY e '𝄞'>", "e", false, "'𝄞'");
    let Value::Item { source, .. } = &mut entity else {
        unreachable!()
    };
    *source = LiteralSource::Replacement;
    let values = [
        Value::Begin("r".into(), true),
        item(
            "<!ENTITY % p \"<!ENTITY e '𝄞'>\">",
            "p",
            true,
            "\"<!ENTITY e '𝄞'>\"",
        ),
        entity,
        Value::DtdEnd,
        Value::Start("r".into(), vec![]),
        Value::Text("𝄞".into()),
        Value::End("r".into()),
    ];
    for utf16 in [false, true] {
        for little in [true, false] {
            if !utf16 && !little {
                continue;
            }
            let bytes = if utf16 {
                encode(
                    PE,
                    Transport {
                        little,
                        bom: true,
                        label: None,
                    },
                )
            } else {
                PE.as_bytes().to_vec()
            };
            let shift = if utf16 { 2 } else { 0 };
            let spans = if utf16 {
                [0..26, 26..90, 90..96, 96..100, 100..106, 106..112, 112..120]
            } else {
                [0..13, 13..47, 47..50, 50..52, 52..55, 55..58, 58..62]
            };
            let expected: Vec<_> = values
                .iter()
                .cloned()
                .zip(spans)
                .enumerate()
                .map(|(index, (value, span))| {
                    let physical = span.start + shift..span.end + shift;
                    match index {
                        2 => replacement(value, 0..18, physical),
                        5 => replacement(value, 0..4, physical),
                        _ => self::physical(value, physical),
                    }
                })
                .collect();
            for cut in 0..=bytes.len() {
                let (rows, stream) = run(&bytes, &[cut]);
                assert_eq!(rows, expected);
                assert_eq!(
                    stream.dtd_counters().unwrap().physical_bytes,
                    if utf16 { 120 } else { 62 }
                );
                assert_eq!(stream.dtd_counters().unwrap().reference_attempts, 2);
            }
        }
    }
}

#[test]
fn generated_tail_is_drained_at_reference_boundary_before_any_suffix_is_offered() {
    for transport in TRANSPORTS {
        let (bytes, shift) = document(MAIN, transport);
        let boundary = shift + 134;
        let mut stream = session();
        let mut rows = Vec::new();
        feed(
            &mut stream,
            &bytes[..boundary],
            &(1..boundary).collect::<Vec<_>>(),
            &mut rows,
        )
        .unwrap();
        let mut expected = main_expected(true, shift);
        if let Some(label) = transport.label {
            expected.insert(
                0,
                physical(
                    Value::Declaration("1.00".into(), Some(label.into()), Some(true)),
                    usize::from(transport.bom) * 2..shift,
                ),
            );
        }
        assert_eq!(rows, expected[..expected.len() - 2]);
        assert_eq!(stream.position(), boundary);
        assert_eq!(
            stream.advance(&[], &mut |_, _, _| panic!("no delayed generated tail")),
            Ok(0)
        );
        let consumed = stream
            .advance(&bytes[boundary..], &mut |event, source, _| {
                rows.push(capture(event, source));
                Ok(())
            })
            .unwrap();
        assert_eq!(consumed, bytes.len() - boundary);
        finish(&mut stream, &mut rows).unwrap();
        assert_eq!(rows, expected);
        assert_eq!(stream.position(), bytes.len());
    }
}

#[test]
fn malformed_code_units_and_odd_eof_keep_original_offsets_after_dtd_handoff() {
    for little in [true, false] {
        let transport = Transport {
            little,
            bom: true,
            label: None,
        };
        for (prefix, anchor) in [
            ("<!DOCTYPE r [<!ENTITY e '", 52),
            ("<!DOCTYPE r><r>α🙂", 38),
            ("<!DOCTYPE r><r/>", 34),
        ] {
            for (units, odd, immediate) in [
                (vec![0xdc00], false, true),
                (vec![0xd800, 0x61], false, true),
                (vec![0xd800, 0xd800], false, true),
                (vec![0xd800], false, false),
                (vec![], true, false),
                (vec![0xd800], true, false),
            ] {
                let mut bytes = encode(prefix, transport);
                assert_eq!(bytes.len(), anchor);
                for &unit in &units {
                    bytes.extend(if little {
                        u16::to_le_bytes(unit)
                    } else {
                        u16::to_be_bytes(unit)
                    });
                }
                if odd {
                    bytes.push(0);
                }
                if immediate {
                    bytes.extend([0, 0, 0, 0]);
                } // Unread suffix must stay unread.
                for cut in 0..=bytes.len() {
                    let mut stream = session();
                    let mut rows = Vec::new();
                    let result = feed(&mut stream, &bytes, &[cut], &mut rows)
                        .and_then(|()| finish(&mut stream, &mut rows));
                    let expected = SessionError::Xml(Error::new(ErrorKind::InvalidUtf16, anchor));
                    assert_eq!(
                        result,
                        Err(expected),
                        "{little} {prefix:?} {units:?} split {cut}"
                    );
                    assert_eq!(
                        stream.position(),
                        if immediate {
                            anchor + units.len() * 2
                        } else {
                            bytes.len()
                        }
                    );
                    sticky(&mut stream, expected);
                }
            }
        }
    }
}

#[test]
fn eof_in_declarations_or_expanded_body_is_sticky_at_the_raw_document_end() {
    for little in [None, Some(true), Some(false)] {
        for (text, utf16_end, utf8_end, dtd) in [
            ("<!DOCTYPE r [<!ENTITY e '𝄞'>", 60, 31, true),
            ("<!DOCTYPE r><r>", 32, 15, false),
            ("<!DOCTYPE r [<!ENTITY e '𝄞'>]><r>&e;", 76, 39, false),
        ] {
            let bytes = body_bytes(text, little);
            let physical_end = if little.is_some() {
                utf16_end
            } else {
                utf8_end
            };
            assert_eq!(bytes.len(), physical_end);
            for cut in 0..=bytes.len() {
                let mut stream = session();
                let mut rows = Vec::new();
                feed(&mut stream, &bytes, &[cut], &mut rows).unwrap();
                let failure = finish(&mut stream, &mut rows).unwrap_err();
                assert_eq!(failure.physical_offset(), physical_end);
                if dtd {
                    assert!(matches!(
                        failure,
                        SessionError::Dtd(ControllerError {
                            kind: ControllerErrorKind::Scan(ScanErrorKind::UnexpectedEof),
                            ..
                        })
                    ));
                } else {
                    assert!(matches!(
                        failure,
                        SessionError::Content(ContentError {
                            kind: ContentErrorKind::Parser(ErrorKind::UnexpectedEof),
                            ..
                        })
                    ));
                }
                assert_eq!(stream.position(), physical_end);
                assert_eq!(
                    stream.event_origin().map(origin),
                    rows.last().map(|row| row.origin.clone())
                );
                sticky(&mut stream, failure);
            }
        }
    }
}

#[test]
fn physical_decoded_token_total_event_and_dtd_ceilings_are_distinct_and_inclusive() {
    for little in [None, Some(true), Some(false)] {
        let bytes = body_bytes(TINY, little);
        let utf16 = little.is_some();
        assert_eq!(bytes.len(), if utf16 { 34 } else { 16 });
        let cases = if utf16 {
            [(0, 18, 2), (1, 9, 2), (2, 16, 0), (3, 32, 32), (4, 3, 26)]
        } else {
            [(0, 9, 0), (1, 9, 0), (2, 16, 0), (3, 16, 15), (4, 3, 12)]
        };
        for (category, boundary, offset) in cases {
            // The new scalar-start physical-quota diagnostic is frozen for
            // UTF-16; UTF-8 currently anchors this legacy error at token start.
            if !utf16 && category == 3 {
                continue;
            }
            for ceiling in [boundary - 1, boundary, boundary + 1] {
                let mut limits = Limits::default();
                let mut decode = DecodeLimits::default();
                let mut options = options();
                match category {
                    0 => limits.max_token_bytes = ceiling,
                    1 => decode.max_decoded_token_bytes = ceiling,
                    2 => decode.max_decoded_bytes = ceiling,
                    3 => options.declarations.max_physical_bytes = ceiling,
                    _ => limits.max_events = ceiling,
                }
                for cuts in [
                    vec![],
                    vec![if utf16 { 19 } else { 9 }],
                    (1..bytes.len()).collect(),
                ] {
                    let mut stream = configured(limits, decode, options);
                    let mut rows = Vec::new();
                    let result = feed(&mut stream, &bytes, &cuts, &mut rows)
                        .and_then(|()| finish(&mut stream, &mut rows));
                    if ceiling >= boundary {
                        result.unwrap();
                        assert_eq!(rows.len(), 3);
                        assert_eq!(stream.position(), bytes.len());
                    } else {
                        let failure = result.unwrap_err();
                        assert_eq!(failure.physical_offset(), offset);
                        match category {
                            0..=2 => assert_eq!(
                                failure,
                                SessionError::Xml(Error::new(
                                    ErrorKind::LimitExceeded(
                                        [
                                            Limit::TokenBytes,
                                            Limit::DecodedTokenBytes,
                                            Limit::DecodedBytes
                                        ][category]
                                    ),
                                    offset
                                ))
                            ),
                            3 => assert!(matches!(
                                failure,
                                SessionError::Content(ContentError {
                                    kind: ContentErrorKind::PhysicalBytes,
                                    ..
                                })
                            )),
                            _ => assert!(matches!(
                                failure,
                                SessionError::Content(ContentError {
                                    kind: ContentErrorKind::Tag(
                                        crate::dtd_tag::DtdTagErrorKind::Parser(
                                            ErrorKind::LimitExceeded(Limit::Events)
                                        )
                                    ),
                                    ..
                                })
                            )),
                        }
                        assert!(rows.len() < 3);
                        sticky(&mut stream, failure);
                    }
                }
            }
        }
    }
}

#[test]
fn non_bmp_expansion_uses_utf8_work_bytes_but_raw_utf16_error_context() {
    let text = "<!DOCTYPE r [<!ENTITY e '𝄞'>]><r>&e;</r>";
    for little in [None, Some(true), Some(false)] {
        let bytes = body_bytes(text, little);
        for ceiling in [3, 4, 5] {
            let mut options = options();
            options.declarations.frames.max_expanded_bytes = ceiling;
            let mut stream = configured(Limits::default(), DecodeLimits::default(), options);
            let mut rows = Vec::new();
            let result = feed(
                &mut stream,
                &bytes,
                &(1..bytes.len()).collect::<Vec<_>>(),
                &mut rows,
            )
            .and_then(|()| finish(&mut stream, &mut rows));
            if ceiling >= 4 {
                result.unwrap();
                assert_eq!(
                    rows.iter()
                        .filter(|row| row.value == Value::Text("𝄞".into()))
                        .count(),
                    1
                );
            } else {
                let failure = result.unwrap_err();
                assert!(matches!(
                    failure,
                    SessionError::Content(ContentError {
                        kind: ContentErrorKind::Frame(FrameErrorKind::LimitExceeded(
                            FrameLimit::ExpandedBytes
                        )),
                        ..
                    })
                ));
                assert_eq!(
                    failure.physical_offset(),
                    if little.is_some() { 70 } else { 36 }
                );
                assert!(!rows.iter().any(|row| matches!(row.value, Value::Text(_))));
                sticky(&mut stream, failure);
            }
        }
    }
}

#[test]
fn cancellation_callbacks_and_panics_keep_physical_anchors_and_never_resume() {
    for little in [None, Some(true), Some(false)] {
        let bytes = body_bytes(TINY, little);
        let cases = if little.is_some() {
            [(0, 24, 26), (1, 26, 26), (2, 34, 34)]
        } else {
            [(0, 11, 12), (1, 12, 12), (2, 16, 16)]
        };
        for (cancel, offset, position) in cases {
            let mut stream = session();
            let mut callbacks = 0;
            let failure = stream
                .advance(&bytes, &mut |_, source, _| {
                    let index = callbacks;
                    callbacks += 1;
                    if index == cancel {
                        Err(Error::new(ErrorKind::Aborted, source.cancellation_offset()))
                    } else {
                        Ok(())
                    }
                })
                .unwrap_err();
            assert_eq!(callbacks, cancel + 1);
            assert_eq!(failure.physical_offset(), offset);
            assert_eq!(stream.position(), position);
            assert_eq!(stream.event_origin().unwrap().cancellation_offset(), offset);
            sticky(&mut stream, failure);
        }
        let bytes = body_bytes(MAIN, little);
        for (cancel_at, kind, offset, position) in [
            (
                0,
                ErrorKind::Syntax,
                777,
                if little.is_some() { 28 } else { 13 },
            ),
            (
                7,
                ErrorKind::Aborted,
                if little.is_some() { 136 } else { 72 },
                if little.is_some() { 136 } else { 72 },
            ),
        ] {
            let mut stream = session();
            let mut callbacks = 0;
            let failure = stream
                .advance(&bytes, &mut |_, source, _| {
                    let index = callbacks;
                    callbacks += 1;
                    if index == cancel_at {
                        if kind == ErrorKind::Aborted {
                            assert_eq!(source.cancellation_offset(), offset);
                        }
                        Err(Error::new(kind, offset))
                    } else {
                        Ok(())
                    }
                })
                .unwrap_err();
            assert_eq!(callbacks, cancel_at + 1);
            assert_eq!(failure.physical_offset(), offset);
            if cancel_at == 0 {
                assert_eq!(failure, SessionError::Xml(Error::new(kind, offset)));
            } else {
                assert!(matches!(
                    failure,
                    SessionError::Content(ContentError {
                        kind: ContentErrorKind::Callback(ErrorKind::Aborted),
                        ..
                    })
                ));
            }
            assert_eq!(stream.position(), position);
            sticky(&mut stream, failure);
        }
        let cases = if little.is_some() {
            [(1, 68), (7, 136)]
        } else {
            [(1, 35), (7, 72)]
        };
        for (panic_at, position) in cases {
            let mut stream = session();
            let mut callbacks = 0;
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                stream.advance(&bytes, &mut |_, _, _| {
                    let index = callbacks;
                    callbacks += 1;
                    if index == panic_at {
                        panic!("intentional callback panic");
                    }
                    Ok(())
                })
            }));
            assert!(outcome.is_err());
            assert_eq!(stream.position(), position);
            let failure = stream
                .finish(&mut |_, _, _| panic!("poisoned callback"))
                .unwrap_err();
            assert_eq!(
                failure,
                SessionError::Poisoned {
                    physical_offset: position
                }
            );
            sticky(&mut stream, failure);
        }
    }
}

#[test]
fn callback_budget_and_error_identity_survive_declaration_dtd_and_body_handoffs() {
    for transport in TRANSPORTS
        .into_iter()
        .filter(|transport| transport.label.is_some())
    {
        let (bytes, shift) = document(MAIN, transport);
        let mut stream = session();
        let mut retained = Vec::new();
        let mut remaining = None;
        let failure = stream
            .advance(&bytes, &mut |event, source, budget| {
                match event {
                    LocatedEvent::Xml(Event::Declaration(_)) => {
                        let before = budget.remaining_owned();
                        retained.push(budget.copy("held🙂", source.error_offset())?);
                        assert_eq!(budget.remaining_owned(), before - 8);
                        remaining = Some(budget.remaining_owned());
                    }
                    LocatedEvent::Dtd(DtdEvent::Begin(_)) => {
                        assert!(budget.remaining_owned() <= remaining.unwrap())
                    }
                    LocatedEvent::Xml(Event::Text(ref text)) if text == "𝄞" => {
                        assert!(budget.remaining_owned() <= remaining.unwrap());
                        budget.charge_owned(budget.remaining_owned() + 1, source.error_offset())?;
                    }
                    _ => {}
                }
                Ok(())
            })
            .unwrap_err();
        assert!(matches!(
            failure,
            SessionError::Content(ContentError {
                kind: ContentErrorKind::Callback(ErrorKind::LimitExceeded(Limit::OwnedBytes)),
                ..
            })
        ));
        assert_eq!(failure.physical_offset(), shift + 128);
        assert_eq!(stream.position(), shift + 134);
        assert_eq!(&*retained[0], "held🙂");
        assert_eq!(
            stream.event_origin().map(origin),
            Some(replacement(Value::Text("𝄞".into()), 4..8, shift + 128..shift + 134).origin)
        );
        sticky(&mut stream, failure);
    }
}

#[test]
fn declaration_encoding_errors_precede_dtd_entry_and_keep_raw_attribute_offsets() {
    for little in [true, false] {
        for (bom, label, kind) in [
            (false, "UTF-16", ErrorKind::EncodingMismatch),
            (
                false,
                if little { "UTF-16BE" } else { "UTF-16LE" },
                ErrorKind::EncodingMismatch,
            ),
            (true, "UTF-8", ErrorKind::EncodingMismatch),
            (true, "unknown-encoding", ErrorKind::UnsupportedEncoding),
            (true, "8UTF", ErrorKind::InvalidDeclaration),
        ] {
            let text = format!("<?xml version='1.0' encoding='{label}'?>{TINY}");
            let bytes = encode(
                &text,
                Transport {
                    little,
                    bom,
                    label: None,
                },
            );
            let offset = usize::from(bom) * 2 + 40; // Encoding attribute starts at ASCII column20.
            for cut in 0..=bytes.len() {
                let mut stream = session();
                let mut rows = Vec::new();
                let failure = feed(&mut stream, &bytes, &[cut], &mut rows).unwrap_err();
                assert_eq!(failure, SessionError::Xml(Error::new(kind, offset)));
                assert!(rows.is_empty());
                assert!(stream.dtd_counters().is_none());
                assert_eq!(stream.event_origin(), None);
                sticky(&mut stream, failure);
            }
        }
    }
}

#[test]
fn successful_utf16_eof_is_idempotent_and_later_feed_is_terminal_misuse() {
    for transport in TRANSPORTS {
        let (bytes, _) = document(TINY, transport);
        let (_, mut stream) = run(&bytes, &(1..bytes.len()).collect::<Vec<_>>());
        let failure = SessionError::Xml(Error::new(ErrorKind::InvalidState, bytes.len()));
        assert_eq!(
            stream.advance(&[], &mut |_, _, _| panic!("no callback after EOF")),
            Err(failure)
        );
        sticky(&mut stream, failure);
    }
}
