//! Blind stable-reader specification: no future implementation was inspected.
use super::dtd::{DtdReader, ReaderEvent};
use super::*;
use crate::DecodeLimits;
use crate::dtd::{DtdUnitView, LiteralSource};
use crate::dtd_controller::content::ContentErrorKind;
use crate::dtd_controller::{ControllerErrorKind, ControllerLimits, TakenDtdEvent};
use crate::dtd_frames::{FrameErrorKind, FrameLimit, LookupFailure};
use crate::dtd_reference::ReferenceLimits;
use crate::dtd_scan::ScanErrorKind;
use crate::stream::dtd::{SessionError, SessionLimits};
use crate::tokenizer::local::LocalLimits;
use crate::tokenizer::text::{TextLimits, TextScratchLimits};

const DOC: &str = "<!DOCTYPE r [<!ENTITY e '<n/>x'><!ENTITY z ''>]><r>a&e;b&z;c</r>";
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
                input_token_bytes: 2 * 1024 * 1024,
                replacement_token_bytes: TOKEN,
                max_bytes: 2 * 1024 * 1024 + TOKEN * 2,
            },
        },
        local: LocalLimits {
            max_token_bytes: TOKEN,
            max_scratch_bytes: TOKEN,
            max_work: WORK,
        },
    }
}
fn limits() -> Limits {
    Limits {
        max_token_bytes: TOKEN,
        ..Limits::default()
    }
}
fn decode() -> DecodeLimits {
    DecodeLimits {
        max_decoded_token_bytes: TOKEN,
        ..DecodeLimits::default()
    }
}
fn reader(input: &[u8]) -> DtdReader<'_> {
    DtdReader::with_decode_limits(input, limits(), decode(), options()).unwrap()
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Value {
    Declaration(String, Option<String>, Option<bool>),
    Begin(String, bool, String),
    Item(String, LiteralSource),
    DtdEnd,
    Tag(
        bool,
        String,
        Option<String>,
        Vec<(String, Option<String>, String)>,
    ),
    End(String, Option<String>),
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
#[derive(Clone, Debug, Eq, PartialEq)]
struct Row {
    value: Value,
    origin: Origin,
}
fn origin(value: SourceOrigin) -> Origin {
    Origin {
        input: value.input_range(),
        local: value.replacement_range(),
        physical: value.physical_context(),
    }
}
fn name(value: &Name<'_>) -> (String, Option<String>) {
    let (prefix, local) = value
        .qualified()
        .split_once(':')
        .map_or((None, value.qualified()), |(p, l)| (Some(p), l));
    assert_eq!(value.prefix(), prefix);
    assert_eq!(value.local(), local);
    (
        value.qualified().into(),
        value.namespace().map(str::to_owned),
    )
}
fn xml(event: &Event<'_>) -> Value {
    match event {
        Event::Declaration(d) => Value::Declaration(
            d.version.into(),
            d.encoding.map(str::to_owned),
            d.standalone,
        ),
        Event::Start(e) | Event::Empty(e) => {
            let (q, ns) = name(&e.name);
            Value::Tag(
                matches!(event, Event::Empty(_)),
                q,
                ns,
                e.attributes
                    .iter()
                    .map(|a| {
                        let (q, ns) = name(&a.name);
                        (q, ns, a.value.to_string())
                    })
                    .collect(),
            )
        }
        Event::End(n) => {
            let (q, ns) = name(n);
            Value::End(q, ns)
        }
        Event::Text(v) => Value::Text(v.to_string()),
        Event::Cdata(v) => Value::Cdata(v.to_string()),
        Event::Comment(v) => Value::Comment(v.to_string()),
        Event::ProcessingInstruction { target, data } => {
            Value::Pi(target.to_string(), data.to_string())
        }
    }
}
fn capture(event: &ReaderEvent<'_>, source: SourceOrigin) -> Row {
    let value = match event {
        ReaderEvent::Xml(event) => xml(event),
        ReaderEvent::Dtd(TakenDtdEvent::Begin(unit)) => {
            let DtdUnitView::Header(header) = unit.view() else {
                panic!("Begin must own header metadata");
            };
            Value::Begin(
                header.root_name().into(),
                header.has_internal_subset(),
                unit.raw().into(),
            )
        }
        ReaderEvent::Dtd(TakenDtdEvent::Item { unit, literals }) => {
            Value::Item(unit.raw().into(), *literals)
        }
        ReaderEvent::Dtd(TakenDtdEvent::End) => Value::DtdEnd,
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
fn tag(empty: bool, q: &str) -> Value {
    Value::Tag(empty, q.into(), None, vec![])
}
fn expected(shift: usize) -> Vec<Row> {
    let values = [
        Value::Begin("r".into(), true, "<!DOCTYPE r [".into()),
        Value::Item("<!ENTITY e '<n/>x'>".into(), LiteralSource::Physical),
        Value::Item("<!ENTITY z ''>".into(), LiteralSource::Physical),
        Value::DtdEnd,
        tag(false, "r"),
        Value::Text("a".into()),
        tag(true, "n"),
        Value::Text("x".into()),
        Value::Text("b".into()),
        Value::Text("c".into()),
        Value::End("r".into(), None),
    ];
    let spans = [
        0..13,
        13..32,
        32..46,
        46..48,
        48..51,
        51..52,
        52..55,
        52..55,
        55..56,
        59..60,
        60..64,
    ];
    values
        .into_iter()
        .zip(spans)
        .enumerate()
        .map(|(i, (value, span))| {
            let span = span.start + shift..span.end + shift;
            let local = match i {
                6 => Some(0..4),
                7 => Some(4..5),
                _ => None,
            };
            Row {
                value,
                origin: Origin {
                    input: local.is_none().then(|| span.clone()),
                    local,
                    physical: span,
                },
            }
        })
        .collect()
}

// Returning these events must not retain a mutable loan of a reader that drops.
fn collect(input: &[u8]) -> Result<Vec<(ReaderEvent<'_>, SourceOrigin)>, SessionError> {
    let mut reader = reader(input);
    let mut events = Vec::new();
    for _ in 0..1024 {
        match reader.read_event()? {
            Some(event) => events.push(event),
            None => return Ok(events),
        }
    }
    panic!("unbounded or duplicate event publication")
}
fn sticky(reader: &mut DtdReader<'_>, error: SessionError) {
    let position = reader.position();
    let source = reader.event_origin();
    let range = reader.event_range();
    for _ in 0..3 {
        assert_eq!(reader.read_event().unwrap_err(), error);
    }
    assert_eq!(reader.position(), position);
    assert_eq!(reader.event_origin(), source);
    assert_eq!(reader.event_range(), range);
}
fn failure(reader: &mut DtdReader<'_>, rows: &mut Vec<Row>) -> SessionError {
    for _ in 0..256 {
        match reader.read_event() {
            Ok(Some((e, o))) => rows.push(capture(&e, o)),
            Ok(None) => panic!("invalid fixture accepted"),
            Err(error) => return error,
        }
    }
    panic!("unbounded error drain")
}

#[test]
fn public_control_pins_stable_lifetime_cursor_and_owned_normalization_minimum() {
    let source = String::from("<r>x<n/>x</r>");
    let mut public = Reader::new(source.as_bytes()).unwrap();
    let first = public.read_event().unwrap().unwrap();
    for (position, range) in [(4, 3..4), (8, 4..8), (9, 8..9), (13, 9..13)] {
        let event = public.read_event().unwrap().unwrap();
        if let Event::Text(Cow::Borrowed(s)) = event {
            assert_eq!(s.as_ptr(), source[range.clone()].as_ptr());
        }
        assert_eq!(public.position(), position);
        assert_eq!(public.event_range(), Some(range));
    }
    drop(public);
    assert!(matches!(first, Event::Start(_)));
    for ceiling in [4, 5, 6] {
        let mut public = Reader::with_limits(
            b"<r>&amp;</r>",
            Limits {
                max_owned_bytes: ceiling,
                ..limits()
            },
        )
        .unwrap();
        public.read_event().unwrap();
        if ceiling < 5 {
            assert_eq!(
                public.read_event().unwrap_err(),
                Error::new(ErrorKind::LimitExceeded(Limit::OwnedBytes), 3)
            );
        } else {
            assert!(
                matches!(public.read_event().unwrap(), Some(Event::Text(Cow::Owned(s))) if s == "&")
            );
        }
    }
}

#[test]
fn ordinary_documents_match_public_reader_positions_values_origins_and_lifetimes() {
    for source in [
        "<r>x<n/>x</r>",
        "\u{feff}<?xml version='1.00' encoding='uTf-8'?>\r\n<r a='&amp;'>a\r\nb<![CDATA[x&y]]><!--c--><?p \r\nz?></r> ",
        "<r xmlns:p='u&#x72;n'><p:a/><p:b/></r>",
        "<é:r xmlns:é='urn:u' é:中='v'/>",
    ] {
        let mut public = Reader::new(source.as_bytes()).unwrap();
        let mut stable = reader(source.as_bytes());
        let mut held = Vec::new();
        let mut expected_held = Vec::new();
        assert_eq!(stable.position(), public.position());
        assert_eq!(stable.event_origin(), None);
        loop {
            let expected = public.read_event().unwrap();
            let actual = stable.read_event().unwrap();
            match (expected, actual) {
                (Some(a), Some((ReaderEvent::Xml(b), source))) => {
                    assert_eq!(a, b);
                    assert_eq!(Some(source), public.event_origin());
                    expected_held.push(a);
                    held.push(b);
                }
                (None, None) => break,
                other => panic!("ordinary output differs: {other:?}"),
            }
            assert_eq!(stable.position(), public.position());
            assert_eq!(stable.event_range(), public.event_range());
        }
        assert_eq!(stable.position(), source.len());
        drop(stable);
        assert_eq!(held, expected_held);
    }
}

#[test]
fn dtd_position_table_and_origins_are_literal_and_all_events_outlive_reader() {
    for bom in [false, true] {
        let source = format!("{}{DOC}", if bom { "\u{feff}" } else { "" });
        let shift = usize::from(bom) * 3;
        let mut stable = reader(source.as_bytes());
        let mut held = Vec::new();
        let mut rows = Vec::new();
        for position in [13, 32, 46, 48, 51, 55, 55, 55, 59, 61, 64] {
            let event = stable.read_event().unwrap().unwrap();
            rows.push(capture(&event.0, event.1));
            held.push(event);
            assert_eq!(stable.position(), position + shift);
            assert_eq!(
                stable.event_origin().map(origin),
                rows.last().map(|r| r.origin.clone())
            );
            assert_eq!(stable.event_range(), rows.last().unwrap().origin.input);
        }
        assert_eq!(rows, expected(shift));
        for _ in 0..3 {
            assert!(stable.read_event().unwrap().is_none());
            assert_eq!(stable.position(), source.len());
        }
        drop(stable);
        assert_eq!(
            held.iter().map(|(e, o)| capture(e, *o)).collect::<Vec<_>>(),
            expected(shift)
        );
        let returned = collect(source.as_bytes()).unwrap();
        assert_eq!(
            returned
                .iter()
                .map(|(e, o)| capture(e, *o))
                .collect::<Vec<_>>(),
            expected(shift)
        );
    }
}

#[test]
fn no_subset_header_owns_metadata_borrows_full_raw_and_preserves_held_end() {
    let source = String::from(TINY);
    let mut stable = reader(source.as_bytes());
    let begin = stable.read_event().unwrap().unwrap();
    assert_eq!(stable.position(), 12);
    assert_eq!(stable.event_range(), Some(0..11));
    let end = stable.read_event().unwrap().unwrap();
    assert_eq!(stable.position(), 12);
    assert_eq!(capture(&end.0, end.1), physical(Value::DtdEnd, 11..12));
    let root = stable.read_event().unwrap().unwrap();
    assert_eq!(stable.position(), 16);
    assert_eq!(capture(&root.0, root.1), physical(tag(true, "r"), 12..16));
    drop(stable);
    let ReaderEvent::Dtd(TakenDtdEvent::Begin(unit)) = begin.0 else {
        panic!("moved header");
    };
    assert_eq!(unit.raw(), "<!DOCTYPE r>");
    assert_eq!(unit.raw().as_ptr(), source.as_ptr());
    assert_eq!(begin.1.input_range(), Some(0..11));
    assert!(
        matches!(unit.view(), DtdUnitView::Header(h) if h.root_name() == "r" && !h.has_internal_subset())
    );
}

#[test]
fn physical_items_and_payloads_borrow_exact_occurrences_without_reader_loans() {
    let source = String::from(
        "<!DOCTYPE r [<!--x--><!--x--><?p z?>]><r a='v'>x<![CDATA[y]]><!--z--><?p d?></r>",
    );
    let held = collect(source.as_bytes()).unwrap();
    let mut comment_starts = Vec::new();
    for (event, o) in &held {
        match event {
            ReaderEvent::Dtd(TakenDtdEvent::Item { unit, literals }) => {
                assert!(matches!(literals, LiteralSource::Physical));
                let range = o.input_range().unwrap();
                assert_eq!(unit.raw(), &source[range.clone()]);
                assert_eq!(unit.raw().as_ptr(), source[range.clone()].as_ptr());
                if unit.raw() == "<!--x-->" {
                    comment_starts.push(range.start);
                }
            }
            ReaderEvent::Xml(Event::Start(e)) => {
                assert!(matches!(e.name.qualified_source(), Cow::Borrowed("r")));
                assert!(matches!(e.attributes[0].value, Cow::Borrowed("v")));
            }
            ReaderEvent::Xml(Event::Text(v) | Event::Cdata(v) | Event::Comment(v)) => {
                assert!(matches!(v, Cow::Borrowed(_)))
            }
            ReaderEvent::Xml(Event::ProcessingInstruction { target, data }) => {
                assert!(matches!(target, Cow::Borrowed("p")));
                assert!(matches!(data, Cow::Borrowed("d")));
            }
            _ => {}
        }
    }
    assert_eq!(comment_starts, [13, 21]);
}

#[test]
fn generated_fields_are_owned_and_survive_namespace_unwind_input_and_reader_drop_when_extracted() {
    let retained = {
        let source = String::from(
            "<!DOCTYPE r [<!ENTITY e '<p:n a=\"v\">x</p:n><![CDATA[y]]><!--z--><?p d?>'>]><r xmlns:p='urn:p'>&e;</r>",
        );
        let held = collect(source.as_bytes()).unwrap();
        let mut text = Vec::new();
        let mut names = Vec::new();
        for (event, o) in held {
            if o.input_range().is_some() {
                continue;
            }
            match event {
                ReaderEvent::Xml(Event::Start(e)) => {
                    assert_eq!(e.name.namespace(), Some("urn:p"));
                    names.push(e.name.try_into_static(0).unwrap());
                    for a in e.attributes {
                        names.push(a.name.try_into_static(0).unwrap());
                        let Cow::Owned(v) = a.value else {
                            panic!("generated attribute value must own");
                        };
                        text.push(v);
                    }
                }
                ReaderEvent::Xml(Event::End(n)) => names.push(n.try_into_static(0).unwrap()),
                ReaderEvent::Xml(Event::Text(v) | Event::Cdata(v) | Event::Comment(v)) => {
                    let Cow::Owned(v) = v else {
                        panic!("generated leaf must own");
                    };
                    text.push(v);
                }
                ReaderEvent::Xml(Event::ProcessingInstruction { target, data }) => {
                    let (Cow::Owned(target), Cow::Owned(data)) = (target, data) else {
                        panic!("generated PI fields must own");
                    };
                    text.extend([target, data]);
                }
                _ => panic!("unexpected generated event"),
            }
        }
        (text, names)
    };
    assert_eq!(retained.0, ["v", "x", "y", "z", "p", "d"]);
    assert_eq!(
        retained.1.iter().map(Name::qualified).collect::<Vec<_>>(),
        ["p:n", "a", "p:n"]
    );
    assert_eq!(retained.1[2].namespace(), Some("urn:p"));
}

#[test]
fn parameter_generated_dtd_raw_retains_replacement_phase_after_reader_drop() {
    let source = String::from("<!DOCTYPE r [<!ENTITY % p '<!--a&#13;b-->'>%p;]><r/>");
    let held = collect(source.as_bytes()).unwrap();
    let (event, o) = held
        .into_iter()
        .find(|(event, _)| {
            matches!(
                event,
                ReaderEvent::Dtd(TakenDtdEvent::Item {
                    literals: LiteralSource::Replacement,
                    ..
                })
            )
        })
        .unwrap();
    let ReaderEvent::Dtd(TakenDtdEvent::Item { unit, .. }) = event else {
        unreachable!()
    };
    assert_eq!(unit.raw(), "<!--a\rb-->");
    assert_eq!(o.input_range(), None);
    assert_eq!(o.replacement_range(), Some(0..10));
    let at = source.find("%p;").unwrap();
    assert_eq!(o.physical_context(), at..at + 3);
    let address = unit.raw().as_ptr() as usize;
    assert!(
        !(source.as_ptr() as usize..source.as_ptr() as usize + source.len()).contains(&address)
    );
    // The reader has dropped, but this unit remains typed to the input lifetime.
}

#[test]
fn defaults_namespace_order_and_nonvalidating_rules_survive_later_reader_mutation() {
    let source = String::from(
        "<!DOCTYPE p:r [<!ATTLIST p:r p:a CDATA 'v' xmlns:p CDATA 'urn:p' b CDATA #FIXED 'fixed' c CDATA #REQUIRED><!ATTLIST p:r p:a CDATA 'later' d CDATA 'last'>]><p:r z='specified' b='override'/>",
    );
    let held = collect(source.as_bytes()).unwrap();
    let (event, o) = held.last().unwrap();
    let ReaderEvent::Xml(Event::Empty(e)) = event else {
        panic!("empty root");
    };
    assert_eq!(e.name.namespace(), Some("urn:p"));
    assert_eq!(
        e.attributes
            .iter()
            .map(|a| (a.name.qualified(), a.value.as_ref()))
            .collect::<Vec<_>>(),
        [
            ("z", "specified"),
            ("b", "override"),
            ("p:a", "v"),
            ("xmlns:p", "urn:p"),
            ("d", "last")
        ]
    );
    assert_eq!(e.attributes[2].name.namespace(), Some("urn:p"));
    for a in &e.attributes[2..] {
        assert!(matches!(a.name.qualified_source(), Cow::Owned(_)));
        assert!(matches!(a.value, Cow::Owned(_)));
    }
    assert_eq!(
        o.input_range(),
        Some(source.find("<p:r z=").unwrap()..source.len())
    );
}

#[test]
fn expanded_duplicate_from_defaults_rejects_the_tag_without_publishing_it() {
    let raw =
        "<!DOCTYPE r [<!ATTLIST r xmlns:p CDATA 'u' xmlns:q CDATA 'u' q:a CDATA 'v'>]><r p:a='x'/>";
    let mut stable = reader(raw.as_bytes());
    let mut rows = Vec::new();
    let error = failure(&mut stable, &mut rows);
    assert!(matches!(
        error,
        SessionError::Content(crate::dtd_controller::content::ContentError {
            kind: ContentErrorKind::Tag(crate::dtd_tag::DtdTagErrorKind::Parser(
                ErrorKind::DuplicateAttribute
            )),
            ..
        })
    ));
    assert_eq!(rows.len(), 3);
    assert_eq!(rows.last().unwrap().value, Value::DtdEnd);
    assert_eq!(
        stable.event_origin().map(origin),
        rows.last().map(|r| r.origin.clone())
    );
    sticky(&mut stable, error);
}

#[test]
fn prefix_delivery_precedes_unknown_or_quota_blocked_entity_inclusion() {
    for limited in [false, true] {
        let raw = if limited {
            DOC
        } else {
            "<!DOCTYPE r><r>prefix&missing;</r>"
        };
        let mut policy = options();
        if limited {
            policy.declarations.frames.max_reference_attempts = 0;
        }
        let mut stable = DtdReader::with_limits(raw.as_bytes(), limits(), policy).unwrap();
        let mut rows = Vec::new();
        let error = failure(&mut stable, &mut rows);
        let expected = if limited {
            FrameErrorKind::LimitExceeded(FrameLimit::ReferenceAttempts)
        } else {
            FrameErrorKind::Lookup(LookupFailure::Undeclared)
        };
        assert!(
            matches!(error, SessionError::Content(crate::dtd_controller::content::ContentError { kind: ContentErrorKind::Frame(kind), .. }) if kind == expected)
        );
        assert_eq!(
            rows.last().unwrap().value,
            Value::Text(if limited { "a" } else { "prefix" }.into())
        );
        assert_eq!(
            error.physical_offset(),
            raw.find(if limited { "&e;" } else { "&missing;" }).unwrap()
        );
        sticky(&mut stable, error);
    }
}

#[test]
fn constructors_reject_utf16_and_invalid_whole_input_exactly_like_public_reader() {
    for bytes in [
        b"\xff\xfe<\0r\0/\0>\0".as_slice(),
        b"\xfe\xff\0<\0r",
        b"\0<\0?\0x",
        b"<\0?\0x\0",
        b"\0<\0r",
        b"<r/>\xff",
        b"<r/>\0",
    ] {
        let expected = Reader::new(bytes).err().unwrap();
        assert_eq!(
            DtdReader::new(bytes, options()).err().unwrap(),
            SessionError::Xml(expected)
        );
        assert_eq!(
            DtdReader::with_limits(bytes, limits(), options())
                .err()
                .unwrap(),
            SessionError::Xml(expected)
        );
        assert_eq!(
            DtdReader::with_decode_limits(bytes, limits(), decode(), options())
                .err()
                .unwrap(),
            SessionError::Xml(expected)
        );
        assert_eq!(
            DtdReader::with_limits(
                bytes,
                Limits {
                    max_input_bytes: 0,
                    ..limits()
                },
                options()
            )
            .err()
            .unwrap(),
            SessionError::Xml(Error::new(ErrorKind::LimitExceeded(Limit::InputBytes), 0))
        );
    }
}

#[test]
fn ordinary_errors_keep_exact_public_cursor_and_origin_and_first_failure() {
    for raw in [
        "<r></s>",
        "<r>&bad;</r>",
        "<r>&#0;</r>",
        "<r a='&bad;'/>",
        "<r><!--a--b--></r>",
        "<?xml?><r/>",
        "<?xml version='1.x'?><r/>",
        "<r>tail",
    ] {
        let mut public = Reader::new(raw.as_bytes()).unwrap();
        let mut stable = reader(raw.as_bytes());
        loop {
            match public.read_event() {
                Ok(Some(event)) => {
                    let (actual, o) = stable.read_event().unwrap().unwrap();
                    assert_eq!(capture(&actual, o).value, xml(&event));
                }
                Err(error) => {
                    assert_eq!(stable.read_event().unwrap_err(), SessionError::Xml(error));
                    assert_eq!(stable.position(), public.position());
                    assert_eq!(stable.event_origin(), public.event_origin());
                    sticky(&mut stable, SessionError::Xml(error));
                    break;
                }
                Ok(None) => panic!("invalid public fixture"),
            }
        }
    }
}

#[test]
fn eof_missing_root_partial_dtd_and_missing_end_have_typed_sticky_failures() {
    for raw in [
        "",
        " \r\n",
        "<!DOCTYPE r",
        "<!DOCTYPE r [<!ENTITY e 'x'>",
        "<!DOCTYPE r><r>tail",
    ] {
        let mut stable = reader(raw.as_bytes());
        let mut rows = Vec::new();
        let error = failure(&mut stable, &mut rows);
        if raw.starts_with("<!DOCTYPE") && !raw.contains("><r>") {
            assert!(matches!(
                error,
                SessionError::Dtd(crate::dtd_controller::ControllerError {
                    kind: ControllerErrorKind::Scan(ScanErrorKind::UnexpectedEof),
                    ..
                })
            ));
        } else if raw.ends_with("tail") {
            assert!(matches!(
                error,
                SessionError::Content(crate::dtd_controller::content::ContentError {
                    kind: ContentErrorKind::Parser(ErrorKind::UnexpectedEof),
                    ..
                })
            ));
            assert_eq!(rows.last().unwrap().value, Value::Text("tail".into()));
        } else {
            assert_eq!(
                error,
                SessionError::Xml(Error::new(ErrorKind::MissingRoot, raw.len()))
            );
        }
        assert_eq!(error.physical_offset(), raw.len());
        assert_eq!(
            stable.event_origin().map(origin),
            rows.last().map(|r| r.origin.clone())
        );
        sticky(&mut stable, error);
    }
}

#[test]
fn exact_borrowed_owned_and_event_budgets_do_not_inherit_streaming_copy_costs() {
    for raw in [TINY, "<!DOCTYPE r><r>a</r>"] {
        let mut policy = options();
        policy.declarations.max_owned_bytes = 0;
        let mut stable = DtdReader::with_limits(
            raw.as_bytes(),
            Limits {
                max_owned_bytes: 0,
                ..limits()
            },
            policy,
        )
        .unwrap();
        for _ in 0..32 {
            if stable.read_event().unwrap().is_none() {
                break;
            }
        }
        assert!(stable.read_event().unwrap().is_none());
    }
    for ceiling in [4, 5, 6] {
        let raw = b"<!DOCTYPE r><r>&amp;</r>";
        let mut stable = DtdReader::with_limits(
            raw,
            Limits {
                max_owned_bytes: ceiling,
                ..limits()
            },
            options(),
        )
        .unwrap();
        for _ in 0..3 {
            stable.read_event().unwrap().unwrap();
        }
        if ceiling < 5 {
            let error = stable.read_event().unwrap_err();
            assert!(matches!(
                error,
                SessionError::Content(crate::dtd_controller::content::ContentError {
                    kind: ContentErrorKind::Parser(ErrorKind::LimitExceeded(Limit::OwnedBytes)),
                    ..
                })
            ));
            sticky(&mut stable, error);
        } else {
            let (event, _) = stable.read_event().unwrap().unwrap();
            assert!(matches!(event, ReaderEvent::Xml(Event::Text(Cow::Owned(s))) if s == "&"));
            assert!(stable.read_event().unwrap().is_some());
            assert!(stable.read_event().unwrap().is_none());
        }
    }
    for ceiling in [10, 11, 12] {
        let mut stable = DtdReader::with_limits(
            DOC.as_bytes(),
            Limits {
                max_events: ceiling,
                ..limits()
            },
            options(),
        )
        .unwrap();
        let mut rows = Vec::new();
        for _ in 0..ceiling.min(11) {
            let (e, o) = stable.read_event().unwrap().unwrap();
            rows.push(capture(&e, o));
        }
        if ceiling < 11 {
            let error = stable.read_event().unwrap_err();
            assert!(matches!(
                error,
                SessionError::Content(crate::dtd_controller::content::ContentError {
                    kind: ContentErrorKind::Parser(ErrorKind::LimitExceeded(Limit::Events)),
                    ..
                })
            ));
            assert_eq!(error.physical_offset(), 60);
            assert_eq!(stable.position(), 64);
            sticky(&mut stable, error);
        } else {
            assert!(stable.read_event().unwrap().is_none());
        }
        assert_eq!(rows, expected(0)[..rows.len()]);
    }
}

#[test]
fn attribute_depth_input_and_decoded_ceilings_are_inclusive() {
    let raw = "<!DOCTYPE r [<!ATTLIST r a CDATA 'v'>]><r z='x'/>";
    for ceiling in [1, 2, 3] {
        let mut stable = DtdReader::with_limits(
            raw.as_bytes(),
            Limits {
                max_attributes: ceiling,
                ..limits()
            },
            options(),
        )
        .unwrap();
        if ceiling < 2 {
            let error = failure(&mut stable, &mut Vec::new());
            assert!(matches!(
                error,
                SessionError::Content(crate::dtd_controller::content::ContentError {
                    kind: ContentErrorKind::Tag(crate::dtd_tag::DtdTagErrorKind::Parser(
                        ErrorKind::LimitExceeded(Limit::Attributes)
                    )),
                    ..
                })
            ));
            sticky(&mut stable, error);
        } else {
            while stable.read_event().unwrap().is_some() {}
        }
    }
    for ceiling in [0, 1, 2] {
        let mut stable = DtdReader::with_limits(
            TINY.as_bytes(),
            Limits {
                max_depth: ceiling,
                ..limits()
            },
            options(),
        )
        .unwrap();
        if ceiling == 0 {
            let error = failure(&mut stable, &mut Vec::new());
            assert!(matches!(
                error,
                SessionError::Content(crate::dtd_controller::content::ContentError {
                    kind: ContentErrorKind::Tag(crate::dtd_tag::DtdTagErrorKind::Parser(
                        ErrorKind::LimitExceeded(Limit::Depth)
                    )),
                    ..
                })
            ));
            sticky(&mut stable, error);
        } else {
            while stable.read_event().unwrap().is_some() {}
        }
    }
    for ceiling in [15, 16, 17] {
        let result = DtdReader::with_limits(
            TINY.as_bytes(),
            Limits {
                max_input_bytes: ceiling,
                ..limits()
            },
            options(),
        );
        if ceiling < 16 {
            assert_eq!(
                result.err().unwrap(),
                SessionError::Xml(Error::new(ErrorKind::LimitExceeded(Limit::InputBytes), 0))
            );
        } else {
            let mut stable = result.unwrap();
            while stable.read_event().unwrap().is_some() {}
        }
        let result = DtdReader::with_decode_limits(
            TINY.as_bytes(),
            limits(),
            DecodeLimits {
                max_decoded_bytes: ceiling,
                ..decode()
            },
            options(),
        );
        if ceiling < 16 {
            assert_eq!(
                result.err().unwrap(),
                SessionError::Xml(Error::new(ErrorKind::LimitExceeded(Limit::DecodedBytes), 0))
            );
        } else {
            let mut stable = result.unwrap();
            while stable.read_event().unwrap().is_some() {}
        }
    }
}

#[test]
fn early_drop_or_into_inner_preserves_original_bom_bytes_and_retained_events() {
    let raw = String::from("\u{feff}<!DOCTYPE r><r>unclosed");
    for count in [0, 1, 2, 3, 4] {
        let mut stable = reader(raw.as_bytes());
        let mut retained = Vec::new();
        for _ in 0..count {
            retained.push(stable.read_event().unwrap().unwrap());
        }
        let returned = stable.into_inner();
        assert_eq!(returned.as_ptr(), raw.as_ptr());
        assert_eq!(returned, raw.as_bytes());
        for (event, o) in retained {
            let _ = capture(&event, o);
        }
    }
    let mut stable = reader(TINY.as_bytes());
    while stable.read_event().unwrap().is_some() {}
    assert_eq!(stable.into_inner(), TINY.as_bytes());
    let mut stable = reader(b"<r></s>");
    let _ = failure(&mut stable, &mut Vec::new());
    assert_eq!(stable.into_inner(), b"<r></s>");
}

#[test]
fn each_constructor_is_lazy_beyond_preflight_and_returns_original_bytes() {
    let raw = String::from("\u{feff}<!DOCTYPE r><r/>unread-invalid-syntax");
    for constructor in 0..3 {
        let mut stable = match constructor {
            0 => DtdReader::new(raw.as_bytes(), options()),
            1 => DtdReader::with_limits(raw.as_bytes(), limits(), options()),
            _ => DtdReader::with_decode_limits(raw.as_bytes(), limits(), decode(), options()),
        }
        .unwrap();
        assert_eq!(stable.position(), 3);
        assert_eq!(stable.event_origin(), None);
        assert_eq!(stable.event_range(), None);
        let retained = stable.read_event().unwrap().unwrap();
        assert_eq!(stable.position(), 15);
        assert_eq!(stable.into_inner(), raw.as_bytes());
        assert_eq!(
            capture(&retained.0, retained.1),
            physical(
                Value::Begin("r".into(), false, "<!DOCTYPE r>".into()),
                3..14
            )
        );
    }
}

#[test]
fn raw_decoded_token_and_physical_dtd_ceilings_have_exact_boundaries() {
    for category in 0..3 {
        let boundary = if category == 2 { 16 } else { 9 };
        for ceiling in [boundary - 1, boundary, boundary + 1] {
            let mut raw_limits = limits();
            let mut decoded = decode();
            let mut policy = options();
            match category {
                0 => raw_limits.max_token_bytes = ceiling,
                1 => decoded.max_decoded_token_bytes = ceiling,
                _ => policy.declarations.max_physical_bytes = ceiling,
            }
            let mut stable =
                DtdReader::with_decode_limits(TINY.as_bytes(), raw_limits, decoded, policy)
                    .unwrap();
            if ceiling < boundary {
                let error = failure(&mut stable, &mut Vec::new());
                if category < 2 {
                    assert_eq!(
                        error,
                        SessionError::Xml(Error::new(
                            ErrorKind::LimitExceeded(if category == 0 {
                                Limit::TokenBytes
                            } else {
                                Limit::DecodedTokenBytes
                            }),
                            0
                        ))
                    );
                } else {
                    assert!(matches!(
                        error,
                        SessionError::Content(crate::dtd_controller::content::ContentError {
                            kind: ContentErrorKind::PhysicalBytes,
                            physical_offset: 15,
                            ..
                        })
                    ));
                    assert_eq!(stable.position(), 15);
                }
                sticky(&mut stable, error);
            } else {
                let mut count = 0;
                while stable.read_event().unwrap().is_some() {
                    count += 1;
                }
                assert_eq!(count, 3);
            }
        }
    }
}

#[test]
fn non_bmp_replacement_work_counts_utf8_bytes_instead_of_scalars() {
    let raw = "<!DOCTYPE r [<!ENTITY e '𝄞'>]><r>&e;</r>";
    for ceiling in [3, 4, 5] {
        let mut policy = options();
        policy.declarations.frames.max_expanded_bytes = ceiling;
        let mut stable = DtdReader::with_limits(raw.as_bytes(), limits(), policy).unwrap();
        let mut held = Vec::new();
        for _ in 0..4 {
            held.push(stable.read_event().unwrap().unwrap());
        }
        if ceiling < 4 {
            let error = stable.read_event().unwrap_err();
            assert!(matches!(
                error,
                SessionError::Content(crate::dtd_controller::content::ContentError {
                    kind: ContentErrorKind::Frame(FrameErrorKind::LimitExceeded(
                        FrameLimit::ExpandedBytes
                    )),
                    ..
                })
            ));
            assert_eq!(error.physical_offset(), 36);
            sticky(&mut stable, error);
        } else {
            held.push(stable.read_event().unwrap().unwrap());
            assert!(matches!(&held[4].0, ReaderEvent::Xml(Event::Text(Cow::Owned(s))) if s == "𝄞"));
            assert!(stable.read_event().unwrap().is_some());
            assert!(stable.read_event().unwrap().is_none());
        }
    }
}

#[test]
fn declared_types_and_lexical_matching_preserve_the_nonvalidating_profile() {
    let raw = "<!DOCTYPE r [<!ATTLIST r a NMTOKENS #IMPLIED b NMTOKENS '  &#x9;  x \t y  '>]><r a='  &#x9;  x \t y  '/>";
    let held = collect(raw.as_bytes()).unwrap();
    let ReaderEvent::Xml(Event::Empty(e)) = &held.last().unwrap().0 else {
        panic!("root");
    };
    assert_eq!(
        e.attributes
            .iter()
            .map(|a| a.value.as_ref())
            .collect::<Vec<_>>(),
        ["\t x y", "\t x y"]
    );
    let raw = "<!DOCTYPE q:r [<!ATTLIST p:r a:b:c CDATA 'unused'>]><q:r xmlns:q='urn:r'/>";
    let held = collect(raw.as_bytes()).unwrap();
    let ReaderEvent::Xml(Event::Empty(e)) = &held.last().unwrap().0 else {
        panic!("root");
    };
    assert_eq!(e.attributes.len(), 1);
    assert_eq!(e.name.namespace(), Some("urn:r"));
}

#[test]
fn many_retained_events_survive_repeated_catalog_use_and_owned_default_copies() {
    let source = format!(
        "<!DOCTYPE r [<!ENTITY e 'x'><!ATTLIST n a CDATA '🙂'>]><r>{}</r>",
        "&e;<n/>".repeat(128)
    );
    let held = collect(source.as_bytes()).unwrap();
    assert_eq!(held.len(), 262);
    for pair in held[5..261].chunks_exact(2) {
        assert!(matches!(&pair[0].0, ReaderEvent::Xml(Event::Text(Cow::Owned(s))) if s == "x"));
        let ReaderEvent::Xml(Event::Empty(e)) = &pair[1].0 else {
            panic!("physical n");
        };
        assert_eq!(e.attributes[0].value, "🙂");
        assert!(matches!(e.attributes[0].value, Cow::Owned(_)));
        assert!(pair[0].1.input_range().is_none());
        assert!(pair[1].1.input_range().is_some());
    }
}

#[test]
fn non_bmp_generated_values_and_default_attributes_keep_literal_utf8_origins() {
    let raw =
        String::from("<!DOCTYPE r [<!ENTITY e '<n/>𝄞'><!ATTLIST n a CDATA '🙂'>]><r>α&e;β</r>");
    let held = collect(raw.as_bytes()).unwrap();
    assert_eq!(held.len(), 10);
    let (generated, o) = &held[6];
    let ReaderEvent::Xml(Event::Empty(e)) = generated else {
        panic!("generated n");
    };
    assert_eq!(e.attributes[0].value, "🙂");
    assert_eq!(o.input_range(), None);
    assert_eq!(o.replacement_range(), Some(0..4));
    assert_eq!(o.physical_context(), 69..72);
    assert_eq!(held[7].1.replacement_range(), Some(4..8));
    assert_eq!(held[7].1.physical_context(), 69..72);
    assert!(matches!(&held[7].0, ReaderEvent::Xml(Event::Text(Cow::Owned(s))) if s == "𝄞"));
    assert_eq!(raw.len(), 78);
}
