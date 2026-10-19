use super::tests::{driver, limits, name};
use super::*;

fn lending_rows(bytes: &[u8], session: &mut StreamingSession) -> Vec<(String, SourceOrigin)> {
    let mut consumed = 0;
    let mut rows = Vec::new();
    for _ in 0..512 {
        let step = if consumed < bytes.len() {
            session.prepare(&bytes[consumed..]).unwrap()
        } else {
            session.finish_step().unwrap()
        };
        consumed += step.consumed;
        match step.outcome {
            SessionOutcome::Ready => {
                let position = session.position();
                let counters = session.dtd_counters();
                assert_eq!(
                    session.prepare(&[]).unwrap(),
                    SessionStep {
                        consumed: 0,
                        outcome: SessionOutcome::Ready
                    }
                );
                assert_eq!(session.position(), position);
                assert_eq!(session.dtd_counters(), counters);
                let (event, origin) = session.lend().unwrap();
                rows.push((name(event), origin));
                session.acknowledge().unwrap();
            }
            SessionOutcome::NeedInput => assert_eq!(consumed, bytes.len()),
            SessionOutcome::Complete => {
                assert_eq!(consumed, bytes.len());
                return rows;
            }
        }
    }
    panic!("finite lending transitions exhausted");
}

#[test]
fn lending_and_callbacks_share_one_preparation_and_exact_event_budget() {
    let bytes = b"<?xml version='1.0'?> \n<!DOCTYPE r [<!ENTITY e 'x'>]>\n<r>pre&e;post</r> \n";
    let ceiling = Limits {
        max_events: 9,
        ..limits()
    };
    let mut session = driver(ceiling);
    let rows = lending_rows(bytes, &mut session);
    assert_eq!(
        rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        [
            "xml:None", "dtd:r", "entity:e", "dtd:end", "+r", "Tpre", "Tx", "Tpost", "-r"
        ]
    );
    let mut callback = driver(ceiling);
    let mut expected = Vec::new();
    callback
        .advance(bytes, &mut |event, origin, _| {
            expected.push((name(event), origin));
            Ok(())
        })
        .unwrap();
    callback
        .finish(&mut |event, origin, _| {
            expected.push((name(event), origin));
            Ok(())
        })
        .unwrap();
    assert_eq!(rows, expected);
    assert_eq!(session.position(), bytes.len());
    assert_eq!(session.dtd_counters(), callback.dtd_counters());
    assert_eq!(
        session.finish_step().unwrap().outcome,
        SessionOutcome::Complete
    );
}

#[test]
fn lent_delivery_rejects_reentry_without_reparse_or_source_release() {
    for misuse in 0..5 {
        let mut session = driver(limits());
        assert_eq!(
            session.prepare(b"<r/>").unwrap().outcome,
            SessionOutcome::Ready
        );
        let (event, origin) = session.lend().unwrap();
        assert_eq!(name(event), "=r");
        assert_eq!(origin.input_range(), Some(0..4));
        let before = session.position();
        let error = match misuse {
            0 => session.lend().unwrap_err(),
            1 => session.prepare(b"<later/>").unwrap_err(),
            2 => session.finish_step().unwrap_err(),
            3 => session
                .advance(b"<later/>", &mut |_, _, _| panic!("must remain lent"))
                .unwrap_err(),
            _ => session
                .finish(&mut |_, _, _| panic!("must remain lent"))
                .unwrap_err(),
        };
        assert_eq!(
            error,
            SessionError::Xml(Error::new(ErrorKind::InvalidState, before))
        );
        assert_eq!(session.position(), before);
        assert_eq!(session.acknowledge(), Err(error));
        assert_eq!(session.finish_step(), Err(error));
    }
    for scan_first in [false, true] {
        let mut session = driver(limits());
        if scan_first {
            session.prepare(b"<r/>").unwrap();
        }
        let before = session.position();
        let error = session.acknowledge().unwrap_err();
        assert_eq!(
            error,
            SessionError::Xml(Error::new(ErrorKind::InvalidState, before))
        );
        assert_eq!(session.prepare(b"<r/>"), Err(error));
    }
}

#[test]
fn acknowledgment_precedes_general_entity_entry_and_dtd_end_handoff() {
    let bytes = b"<!DOCTYPE r [<!ENTITY e 'x'>]><r>pre&e;post</r>";
    let mut session = driver(limits());
    let mut consumed = 0;
    for expected in ["dtd:r", "entity:e", "dtd:end", "+r", "Tpre"] {
        let step = session.prepare(&bytes[consumed..]).unwrap();
        consumed += step.consumed;
        assert_eq!(step.outcome, SessionOutcome::Ready);
        let (event, _) = session.lend().unwrap();
        assert_eq!(name(event), expected);
        if expected == "dtd:end" {
            assert!(matches!(
                session.session[0].phase,
                Some(Phase::Declarations { .. })
            ));
        }
        if expected == "Tpre" {
            assert_eq!(session.dtd_counters().unwrap().reference_attempts, 0);
            let Some(Phase::Content(content)) = &session.session[0].phase else {
                panic!("content")
            };
            assert_eq!(content.frame_counters().reference_attempts, 0);
        }
        session.acknowledge().unwrap();
        if expected == "Tpre" {
            assert_eq!(session.dtd_counters().unwrap().reference_attempts, 0);
        }
    }
    let step = session.prepare(&bytes[consumed..]).unwrap();
    assert_eq!(step.outcome, SessionOutcome::Ready);
    assert_eq!(step.consumed, 0);
    let (event, origin) = session.lend().unwrap();
    assert_eq!(name(event), "Tx");
    assert!(origin.input_range().is_none());
    session.acknowledge().unwrap();
    assert_eq!(session.dtd_counters().unwrap().reference_attempts, 1);
}

fn rows_in_parts(
    bytes: &[u8],
    boundaries: &[usize],
) -> (Vec<(String, SourceOrigin)>, StreamingSession) {
    let mut session = driver(limits());
    let mut rows = Vec::new();
    let mut start = 0;
    for end in boundaries.iter().copied().chain([bytes.len()]) {
        let mut consumed = start;
        for _ in 0..512 {
            let step = session.prepare(&bytes[consumed..end]).unwrap();
            consumed += step.consumed;
            match step.outcome {
                SessionOutcome::Ready => {
                    let (event, origin) = session.lend().unwrap();
                    rows.push((name(event), origin));
                    session.acknowledge().unwrap();
                }
                SessionOutcome::NeedInput => {
                    assert_eq!(consumed, end);
                    break;
                }
                SessionOutcome::Complete => panic!("EOF not supplied"),
            }
        }
        assert_eq!(consumed, end);
        start = end;
    }
    for _ in 0..512 {
        match session.finish_step().unwrap().outcome {
            SessionOutcome::Ready => {
                let (event, origin) = session.lend().unwrap();
                rows.push((name(event), origin));
                session.acknowledge().unwrap();
            }
            SessionOutcome::Complete => return (rows, session),
            SessionOutcome::NeedInput => panic!("EOF cannot need transport"),
        }
    }
    panic!("finite EOF transitions exhausted");
}

#[test]
fn lending_all_transport_splits_match_callback_origins_and_work() {
    let raw = "<?xml version='1.0' encoding='UTF-16'?><!--π--><!DOCTYPE 根 [<!ENTITY e '<a/>𝄞tail'>]><根>π&e;post&#13;&amp;</根> \r\n";
    for little in [false, true] {
        for bom in [false, true] {
            let raw = raw.replace(
                "UTF-16",
                if bom {
                    "UTF-16"
                } else if little {
                    "UTF-16LE"
                } else {
                    "UTF-16BE"
                },
            );
            let mut bytes = Vec::new();
            if bom {
                bytes.extend(if little { [0xff, 0xfe] } else { [0xfe, 0xff] });
            }
            for unit in raw.encode_utf16() {
                bytes.extend(if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            let mut callback = driver(limits());
            let mut expected = Vec::new();
            callback
                .advance(&bytes, &mut |event, origin, _| {
                    expected.push((name(event), origin));
                    Ok(())
                })
                .unwrap();
            callback
                .finish(&mut |event, origin, _| {
                    expected.push((name(event), origin));
                    Ok(())
                })
                .unwrap();
            assert_eq!(
                expected.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
                [
                    "xml:None",
                    "Cπ",
                    "dtd:根",
                    "entity:e",
                    "dtd:end",
                    "+根",
                    "Tπ",
                    "=a",
                    "T𝄞tail",
                    "Tpost\r&",
                    "-根"
                ]
            );
            for cut in 0..=bytes.len() {
                let (rows, session) = rows_in_parts(&bytes, &[cut]);
                assert_eq!(rows, expected, "little={little} bom={bom} cut={cut}");
                assert_eq!(session.position(), bytes.len());
                assert_eq!(session.dtd_counters(), callback.dtd_counters());
            }
            let (rows, _) = rows_in_parts(&bytes, &(0..bytes.len()).collect::<Vec<_>>());
            assert_eq!(rows, expected);
        }
    }
    let bytes = b"<!DOCTYPE r [<!ENTITY e ''>]><r>a&e;b</r>";
    let (expected, _) = rows_in_parts(bytes, &[]);
    for cut in 0..=bytes.len() {
        assert_eq!(rows_in_parts(bytes, &[cut]).0, expected);
    }
}

#[test]
fn semantic_publication_errors_are_sticky_without_an_extra_event() {
    use crate::Limit;
    use crate::dtd_controller::content::ContentErrorKind;
    use crate::dtd_frames::FrameErrorKind;
    use crate::dtd_tag::DtdTagErrorKind;

    for (raw, ceiling, expected_kind, expected_rows) in [
        (
            "<!DOCTYPE r><r/>",
            2,
            ContentErrorKind::Tag(DtdTagErrorKind::Parser(ErrorKind::LimitExceeded(
                Limit::Events,
            ))),
            vec!["dtd:r", "dtd:end"],
        ),
        (
            "<!DOCTYPE r [<!ENTITY e '</r>'>]><r>&e;</r>",
            usize::MAX,
            ContentErrorKind::Frame(FrameErrorKind::ElementBoundaryCrossing),
            vec!["dtd:r", "entity:e", "dtd:end", "+r"],
        ),
    ] {
        let mut session = driver(Limits {
            max_events: ceiling,
            ..limits()
        });
        let mut consumed = 0;
        let mut rows = Vec::new();
        let error = loop {
            let step = session.prepare(&raw.as_bytes()[consumed..]).unwrap();
            consumed += step.consumed;
            assert_eq!(step.outcome, SessionOutcome::Ready);
            match session.lend() {
                Ok((event, _)) => rows.push(name(event)),
                Err(error) => break error,
            }
            session.acknowledge().unwrap();
        };
        assert_eq!(rows, expected_rows);
        assert!(
            matches!(error, SessionError::Content(ContentError { kind, .. }) if kind == expected_kind),
            "{error:?}"
        );
        let position = session.position();
        assert_eq!(session.prepare(b"ignored"), Err(error));
        assert_eq!(session.lend().unwrap_err(), error);
        assert_eq!(session.acknowledge(), Err(error));
        assert_eq!(session.finish_step(), Err(error));
        assert_eq!(session.position(), position);
    }
}

#[test]
fn eof_text_is_lent_once_before_acknowledgment_and_final_validation() {
    let mut session = driver(limits());
    assert_eq!(
        session.prepare(b"<r>tail").unwrap(),
        SessionStep {
            consumed: 3,
            outcome: SessionOutcome::Ready
        }
    );
    assert_eq!(name(session.lend().unwrap().0), "+r");
    session.acknowledge().unwrap();
    assert_eq!(
        session.prepare(b"tail").unwrap(),
        SessionStep {
            consumed: 4,
            outcome: SessionOutcome::NeedInput
        }
    );
    assert_eq!(
        session.finish_step().unwrap().outcome,
        SessionOutcome::Ready
    );
    assert_eq!(
        session.prepare(&[]).unwrap(),
        SessionStep {
            consumed: 0,
            outcome: SessionOutcome::Ready
        }
    );
    let (event, origin) = session.lend().unwrap();
    assert!(matches!(
        event,
        LocatedEvent::Xml(Event::Text(std::borrow::Cow::Borrowed("tail")))
    ));
    assert_eq!(origin.input_range(), Some(3..7));
    session.acknowledge().unwrap();
    let error = session.finish_step().unwrap_err();
    assert_eq!(
        error,
        SessionError::Xml(Error::new(ErrorKind::UnexpectedEof, 7))
    );
    assert_eq!(session.finish_step(), Err(error));
}

#[test]
fn sealed_eof_only_allows_empty_prepare_while_ready() {
    for keep_ready in [false, true] {
        let mut session = driver(limits());
        session.prepare(b"<r>").unwrap();
        assert_eq!(name(session.lend().unwrap().0), "+r");
        session.acknowledge().unwrap();
        session.prepare(b"tail").unwrap();
        assert_eq!(
            session.finish_step().unwrap().outcome,
            SessionOutcome::Ready
        );
        if !keep_ready {
            assert_eq!(name(session.lend().unwrap().0), "Ttail");
            session.acknowledge().unwrap();
        }
        let before = session.position();
        let extra: &[u8] = if keep_ready { b"extra" } else { b"" };
        let error = session.prepare(extra).unwrap_err();
        assert_eq!(
            error,
            SessionError::Xml(Error::new(ErrorKind::InvalidState, 7))
        );
        assert_eq!(session.position(), before);
        assert_eq!(session.finish_step(), Err(error));
    }
}

#[test]
fn callback_rejection_and_unwind_leave_physical_and_generated_prefixes_lent() {
    for generated in [false, true] {
        let raw = if generated {
            "<!DOCTYPE r [<!ENTITY e 'pre&later;'><!ENTITY later 'tail'>]><r>&e;after</r>"
        } else {
            "<!DOCTYPE r [<!ENTITY later 'tail'>]><r>pre&later;after</r>"
        };
        for panics in [false, true] {
            let mut session = driver(limits());
            let mut target_origin = None;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                session.advance(raw.as_bytes(), &mut |event, origin, _| {
                    if matches!(event, LocatedEvent::Xml(Event::Text(ref value)) if value == "pre")
                    {
                        assert!(target_origin.replace(origin).is_none());
                        if panics {
                            panic!("prefix callback stopped");
                        }
                        return Err(Error::new(ErrorKind::Aborted, origin.cancellation_offset()));
                    }
                    Ok(())
                })
            }));
            let origin = target_origin.expect("prefix delivered");
            if panics {
                assert!(result.is_err());
            } else {
                let error = result.unwrap().unwrap_err();
                assert_eq!(error.physical_offset(), origin.cancellation_offset());
                assert!(matches!(
                    error,
                    SessionError::Content(ContentError {
                        kind: crate::dtd_controller::content::ContentErrorKind::Callback(
                            ErrorKind::Aborted
                        ),
                        ..
                    })
                ));
            }
            // Sticky failure alone cannot prove release was withheld: inspect
            // the delivery boundary before invoking any poisoned API again.
            assert!(matches!(session.delivery, Delivery::Lent));
            assert_eq!(origin.input_range().is_none(), generated);
            assert_eq!(
                session.dtd_counters().unwrap().reference_attempts,
                usize::from(generated)
            );
            let error = session
                .finish(&mut |_, _, _| panic!("terminal callback"))
                .unwrap_err();
            assert_eq!(session.acknowledge(), Err(error));
            assert!(matches!(session.delivery, Delivery::Lent));
            assert_eq!(
                session.dtd_counters().unwrap().reference_attempts,
                usize::from(generated)
            );
        }
    }
}
