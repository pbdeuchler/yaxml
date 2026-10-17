use super::*;
use crate::Limit;
use crate::dtd::DtdUnitView;
use crate::dtd_controller::ControllerErrorKind;
use crate::dtd_reference::ReferenceLimits;
use crate::dtd_scan::ScanErrorKind;
use crate::tokenizer::text::TextScratchLimits;

fn options() -> SessionLimits {
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
                input_token_bytes: 4096,
                replacement_token_bytes: 4096,
                max_bytes: 9216,
            },
        },
        local: LocalLimits {
            max_token_bytes: 4096,
            max_scratch_bytes: 4096,
            max_work: 64 * 1024 * 1024,
        },
    }
}
fn driver(limits: Limits) -> StreamingSession {
    StreamingSession::new(
        limits,
        DecodeLimits {
            max_decoded_bytes: 1024 * 1024,
            max_decoded_token_bytes: 4096,
        },
        options(),
    )
}
fn limits() -> Limits {
    Limits {
        max_token_bytes: 4096,
        ..Limits::default()
    }
}
fn name(event: LocatedEvent<'_>) -> String {
    match event {
        LocatedEvent::Xml(event) => match event {
            Event::Declaration(declaration) => format!("xml:{:?}", declaration.standalone),
            Event::Start(element) => format!("+{}", element.name.qualified()),
            Event::End(name) => format!("-{}", name.qualified()),
            Event::Empty(element) => format!(
                "={}{}",
                element.name.qualified(),
                element
                    .attributes
                    .iter()
                    .map(|a| format!(" {}={}", a.name.qualified(), a.value))
                    .collect::<String>()
            ),
            Event::Text(value) => format!("T{value}"),
            Event::Comment(value) => format!("C{value}"),
            Event::Cdata(value) => format!("D{value}"),
            Event::ProcessingInstruction { target, data } => format!("P{target}:{data}"),
        },
        LocatedEvent::Dtd(event) => match event {
            DtdEvent::Begin(header) => format!("dtd:{}", header.root_name()),
            DtdEvent::End => "dtd:end".into(),
            DtdEvent::Item { unit, .. } => match unit.view() {
                DtdUnitView::Entity(entity) => format!("entity:{}", entity.name()),
                DtdUnitView::AttributeList(_) => "attlist".into(),
                _ => "item".into(),
            },
        },
    }
}

#[test]
fn real_utf8_transport_keeps_preamble_budget_dtd_order_and_generated_tail() {
    let raw = "\u{feff}<?xml version='1.0' standalone='yes'?><!--pre--><!DOCTYPE r [<!ENTITY % p \"<!ENTITY e '<a/>tail'>\">%p;<!ATTLIST a x NMTOKENS ' one  two '>]><r>before&e;after&e;</r>";
    let mut stream = driver(limits());
    assert!(stream.event_origin().is_none());
    assert!(stream.dtd_counters().is_none());
    let mut rows = Vec::new();
    let mut owned = Vec::new();
    let mut remaining_after_copy = None;
    let mut emit = |event: LocatedEvent<'_>, origin: SourceOrigin, budget: &mut Budget| {
        if matches!(event, LocatedEvent::Xml(Event::Declaration(_))) {
            let before = budget.remaining_owned();
            owned.push(budget.copy("retained", origin.error_offset())?);
            assert_eq!(budget.remaining_owned(), before - 8);
            remaining_after_copy = Some(budget.remaining_owned());
        }
        if matches!(event, LocatedEvent::Dtd(DtdEvent::Begin(_))) {
            assert!(budget.remaining_owned() <= remaining_after_copy.unwrap());
        }
        rows.push((name(event), origin));
        Ok(())
    };
    assert_eq!(
        stream.advance(raw.as_bytes(), &mut emit).unwrap(),
        raw.len()
    );
    stream.finish(&mut emit).unwrap();
    stream.finish(&mut emit).unwrap();
    assert_eq!(
        rows.iter().map(|row| row.0.as_str()).collect::<Vec<_>>(),
        [
            "xml:Some(true)",
            "Cpre",
            "dtd:r",
            "entity:p",
            "entity:e",
            "attlist",
            "dtd:end",
            "+r",
            "Tbefore",
            "=a x=one two",
            "Ttail",
            "Tafter",
            "=a x=one two",
            "Ttail",
            "-r"
        ]
    );
    assert_eq!(stream.position(), raw.len());
    assert_eq!(stream.dtd_counters().unwrap().reference_attempts, 3);
    assert_eq!(
        stream.dtd_counters().unwrap().physical_bytes,
        raw.len() - raw.find("<!DOCTYPE").unwrap()
    );
    assert_eq!(stream.event_origin(), Some(rows.last().unwrap().1));
    assert_eq!(&*owned[0], "retained");
}

#[test]
fn all_work_at_transport_chunk_end_is_drained_without_physical_suffix() {
    let raw = "<!DOCTYPE r [<!ENTITY e '<a/>tail'><!ENTITY empty ''>]><r>&e;&empty;";
    let mut stream = driver(limits());
    let mut rows = Vec::new();
    assert_eq!(
        stream
            .advance(raw.as_bytes(), &mut |event, _, _| {
                rows.push(name(event));
                Ok(())
            })
            .unwrap(),
        raw.len()
    );
    assert!(rows.ends_with(&["+r".into(), "=a".into(), "Ttail".into()]));
    assert_eq!(stream.dtd_counters().unwrap().reference_attempts, 2);
    assert_eq!(stream.position(), raw.len());
    let count = rows.len();
    assert_eq!(
        stream
            .advance(&[], &mut |event, _, _| {
                rows.push(name(event));
                Ok(())
            })
            .unwrap(),
        0
    );
    assert_eq!(rows.len(), count);
    assert_eq!(
        stream
            .advance(b"</r>", &mut |event, _, _| {
                rows.push(name(event));
                Ok(())
            })
            .unwrap(),
        4
    );
    stream
        .finish(&mut |event, _, _| {
            rows.push(name(event));
            Ok(())
        })
        .unwrap();
    assert_eq!(rows.last().unwrap(), "-r");
}

#[test]
fn dtd_session_slot_reservation_is_atomic_and_terminal_on_failure() {
    struct RejectSessionSlot;
    impl SessionSlotReservation for RejectSessionSlot {
        fn reserve_one<T>(&self, _: &mut Vec<T>) -> bool {
            false
        }
    }

    let mut stream = StreamingSession::with_reservation(
        limits(),
        DecodeLimits {
            max_decoded_bytes: 1024 * 1024,
            max_decoded_token_bytes: 4096,
        },
        options(),
        RejectSessionSlot,
    );
    let preamble = b"<!--pre-->";
    let mut preamble_events = 0;
    assert_eq!(
        stream
            .advance(preamble, &mut |event, _, _| {
                assert!(
                    matches!(event, LocatedEvent::Xml(Event::Comment(ref value)) if value == "pre")
                );
                preamble_events += 1;
                Ok(())
            })
            .unwrap(),
        preamble.len()
    );
    assert_eq!(preamble_events, 1);

    let failure = SessionError::Allocation {
        physical_offset: preamble.len(),
    };
    assert_eq!(
        stream
            .advance(b"<!DOCTYPE r><r/>", &mut |_, _, _| {
                panic!("no DTD callback before reservation")
            })
            .unwrap_err(),
        failure
    );
    assert!(stream.ordinary.is_some());
    assert!(stream.session.is_empty());
    assert_eq!(stream.physical, 0);
    assert_eq!(stream.position(), preamble.len() + 9);
    assert_eq!(
        stream.event_origin().and_then(SourceOrigin::input_range),
        Some(0..preamble.len())
    );
    assert_eq!(stream.dtd_counters(), None);
    assert_eq!(
        stream
            .advance(b"ignored", &mut |_, _, _| panic!(
                "terminal allocation failure"
            ))
            .unwrap_err(),
        failure
    );
    assert_eq!(
        stream
            .finish(&mut |_, _, _| panic!("terminal allocation failure"))
            .unwrap_err(),
        failure
    );
    assert_eq!(stream.position(), preamble.len() + 9);
}

#[test]
fn same_budget_sink_failure_and_cancellation_keep_distinct_physical_anchors() {
    let raw = "<!DOCTYPE r [<!ENTITY e 'text'>]><r>&e;</r>";
    let mut stream = driver(Limits {
        max_owned_bytes: 4096,
        ..limits()
    });
    let mut generated_origin = None;
    let failure = stream
        .advance(raw.as_bytes(), &mut |event, origin, budget| {
            if matches!(event, LocatedEvent::Xml(Event::Text(_))) {
                generated_origin = Some(origin);
                let extra = "x".repeat(budget.remaining_owned() + 1);
                budget.copy(&extra, origin.error_offset())?;
            }
            Ok(())
        })
        .unwrap_err();
    let SessionError::Content(error) = failure else {
        panic!("typed body callback failure")
    };
    assert_eq!(
        error.kind,
        crate::dtd_controller::content::ContentErrorKind::Callback(ErrorKind::LimitExceeded(
            Limit::OwnedBytes
        ))
    );
    assert_eq!(error.physical_offset, raw.find("&e;").unwrap());
    assert_eq!(error.local_offset, None);
    assert_eq!(error.origin.map(SourceOrigin::from_repr), generated_origin);
    assert_eq!(
        stream
            .finish(&mut |_, _, _| panic!("must not resume"))
            .unwrap_err(),
        failure
    );
    assert_eq!(
        stream
            .advance(b"ignored", &mut |_, _, _| panic!("must not resume"))
            .unwrap_err(),
        failure
    );

    let raw = "<!DOCTYPE r><r/>";
    let mut stream = driver(limits());
    let mut delivered = Vec::new();
    let failure = stream
        .advance(raw.as_bytes(), &mut |event, origin, _| {
            delivered.push(name(event));
            Err(Error::new(ErrorKind::Aborted, origin.cancellation_offset()))
        })
        .unwrap_err();
    assert_eq!(delivered, ["dtd:r"]);
    assert_eq!(
        failure,
        SessionError::Xml(Error::new(ErrorKind::Aborted, raw.find('>').unwrap()))
    );
    assert_eq!(stream.position(), "<!DOCTYPE r>".len());
    assert_eq!(stream.event_origin().unwrap().input_range(), Some(0..11));
    assert_eq!(
        stream
            .advance(b"<r/>", &mut |_, _, _| panic!("must not deliver End"))
            .unwrap_err(),
        failure
    );
}

#[test]
fn real_utf16_ordinary_documents_work_but_dtd_entry_is_explicitly_unavailable() {
    for little in [false, true] {
        fn encoded(raw: &str, little: bool) -> Vec<u8> {
            let mut bytes = if little {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in raw.encode_utf16() {
                bytes.extend_from_slice(&if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            bytes
        }
        let bytes = encoded("<r>𝄞</r>", little);
        let mut stream = driver(limits());
        let mut rows = Vec::new();
        for byte in &bytes {
            assert_eq!(
                stream
                    .advance(std::slice::from_ref(byte), &mut |event, origin, _| {
                        rows.push((name(event), origin));
                        Ok(())
                    })
                    .unwrap(),
                1
            );
        }
        stream
            .finish(&mut |event, origin, _| {
                rows.push((name(event), origin));
                Ok(())
            })
            .unwrap();
        assert_eq!(
            rows.iter().map(|row| row.0.as_str()).collect::<Vec<_>>(),
            ["+r", "T𝄞", "-r"]
        );
        assert_eq!(stream.position(), bytes.len());
        assert_eq!(rows[1].1.input_range(), Some(8..12));
        let bytes = encoded("<!DOCTYPE r><r/>", little);
        let mut stream = driver(limits());
        let failure = stream
            .advance(&bytes, &mut |_, _, _| {
                panic!("no DTD event before encoding rejection")
            })
            .unwrap_err();
        assert_eq!(
            failure,
            SessionError::Xml(Error::new(ErrorKind::UnsupportedEncoding, 2))
        );
        assert_eq!(stream.position(), 20);
        assert_eq!(
            stream
                .finish(&mut |_, _, _| panic!("terminal"))
                .unwrap_err(),
            failure
        );
    }
}

#[test]
fn dtd_events_spend_existing_event_budget_and_partial_eof_is_sticky() {
    let raw = "<?xml version='1.0'?><!--pre--><!DOCTYPE r><r/>";
    let mut stream = driver(Limits {
        max_events: 3,
        ..limits()
    });
    let mut rows = Vec::new();
    let failure = stream
        .advance(raw.as_bytes(), &mut |event, _, _| {
            rows.push(name(event));
            Ok(())
        })
        .unwrap_err();
    assert_eq!(rows, ["xml:None", "Cpre", "dtd:r"]);
    assert_eq!(
        failure,
        SessionError::Xml(Error::new(
            ErrorKind::LimitExceeded(Limit::Events),
            raw.find("<!DOCTYPE").unwrap() + 11
        ))
    );
    assert_eq!(
        stream
            .finish(&mut |_, _, _| panic!("terminal"))
            .unwrap_err(),
        failure
    );
    let mut stream = driver(limits());
    let raw = "<!DOCTYPE r [<!ENTITY e 'value'>";
    assert_eq!(
        stream
            .advance(raw.as_bytes(), &mut |_, _, _| Ok(()))
            .unwrap(),
        raw.len()
    );
    let failure = stream.finish(&mut |_, _, _| Ok(())).unwrap_err();
    let SessionError::Dtd(error) = failure else {
        panic!("typed DTD EOF failure")
    };
    assert_eq!(
        error.kind,
        ControllerErrorKind::Scan(ScanErrorKind::UnexpectedEof)
    );
    assert_eq!(error.physical_offset, raw.len());
    assert_eq!(
        stream
            .finish(&mut |_, _, _| panic!("terminal"))
            .unwrap_err(),
        failure
    );
}

#[test]
fn callback_panic_and_post_finish_calls_never_resume_semantic_publication() {
    for (raw, at_dtd, generated, expected_position) in [
        ("<r>text</r>", false, false, 8),
        ("<!DOCTYPE r><r/>", true, false, 12),
        (
            "<!DOCTYPE r [<!ENTITY e 'text'>]><r>&e;</r>",
            false,
            true,
            39,
        ),
    ] {
        let mut stream = driver(limits());
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            stream.advance(raw.as_bytes(), &mut |event, _, _| {
                if (at_dtd && matches!(event, LocatedEvent::Dtd(DtdEvent::Begin(_))))
                    || (!at_dtd && matches!(event, LocatedEvent::Xml(Event::Text(_))))
                {
                    panic!("callback stopped");
                }
                Ok(())
            })
        }));
        assert!(outcome.is_err());
        assert_eq!(stream.position(), expected_position);
        let failure = if at_dtd {
            stream
                .advance(&[], &mut |_, _, _| panic!("must remain poisoned"))
                .unwrap_err()
        } else {
            stream
                .finish(&mut |_, _, _| panic!("must remain poisoned"))
                .unwrap_err()
        };
        assert_eq!(
            failure,
            SessionError::Poisoned {
                physical_offset: expected_position
            }
        );
        assert_eq!(
            stream
                .advance(b"ignored", &mut |_, _, _| panic!("must remain poisoned"))
                .unwrap_err(),
            failure
        );
        assert_eq!(
            stream
                .finish(&mut |_, _, _| panic!("must remain poisoned"))
                .unwrap_err(),
            failure
        );
        assert_eq!(stream.position(), expected_position);
        assert_eq!(
            stream.event_origin().unwrap().input_range().is_none(),
            generated
        );
    }
    let mut stream = driver(limits());
    stream
        .advance(b"<!DOCTYPE r><r/>", &mut |_, _, _| Ok(()))
        .unwrap();
    stream.finish(&mut |_, _, _| Ok(())).unwrap();
    let failure = stream
        .advance(&[], &mut |_, _, _| panic!("no events after EOF"))
        .unwrap_err();
    assert_eq!(
        failure,
        SessionError::Xml(Error::new(ErrorKind::InvalidState, 16))
    );
    assert_eq!(
        stream
            .finish(&mut |_, _, _| panic!("post-EOF misuse sticky"))
            .unwrap_err(),
        failure
    );
}

#[test]
fn declaration_standalone_controls_unread_external_subset_failures() {
    for (declaration, expected_label, expected_kind, expected_error_delta) in [
        (
            "<?xml version='1.0'?>",
            "xml:None",
            ControllerErrorKind::ValueUnavailable(crate::dtd_catalog::DependencyKind::UnreadEntity),
            0,
        ),
        (
            "<?xml version='1.0' standalone='yes'?>",
            "xml:Some(true)",
            ControllerErrorKind::Catalog(crate::dtd_catalog::CatalogErrorKind::EntityDeclared),
            21,
        ),
    ] {
        let raw = format!(
            "{declaration}<!DOCTYPE r SYSTEM 'external.dtd' [<!ATTLIST r x CDATA '&unknown;'>]><r/>"
        );
        let begin = declaration.len()..declaration.len() + 35;
        let item = begin.end..begin.end + 32;
        let mut stream = driver(limits());
        let mut rows = Vec::new();
        let failure = stream
            .advance(raw.as_bytes(), &mut |event, origin, _| {
                rows.push((name(event), origin));
                Ok(())
            })
            .unwrap_err();
        let SessionError::Dtd(error) = failure else {
            panic!("standalone policy must remain a typed DTD failure")
        };
        assert_eq!(error.kind, expected_kind);
        assert_eq!(error.physical_offset, item.start + expected_error_delta);
        assert_eq!(error.origin.unwrap().input_range(), Some(item.clone()));
        assert_eq!(stream.position(), item.end);
        assert_eq!(
            rows.iter().map(|row| row.0.as_str()).collect::<Vec<_>>(),
            [expected_label, "dtd:r"]
        );
        assert_eq!(rows[0].1.input_range(), Some(0..declaration.len()));
        assert_eq!(rows[1].1.input_range(), Some(begin.clone()));
        assert_eq!(stream.event_origin(), Some(rows[1].1));
        assert_eq!(
            stream
                .finish(&mut |_, _, _| panic!("failure must remain terminal"))
                .unwrap_err(),
            failure
        );
        assert_eq!(
            stream
                .advance(b"ignored", &mut |_, _, _| panic!("must not read suffix"))
                .unwrap_err(),
            failure
        );
    }
}
