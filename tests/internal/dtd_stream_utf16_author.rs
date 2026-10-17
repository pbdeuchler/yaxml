use super::tests::{driver, limits, name, options};
use super::*;
use crate::dtd_controller::content::ContentErrorKind;
use crate::encoding::SourceEncoding;

fn encode(raw: &str, little: bool, bom: bool) -> Vec<u8> {
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
    bytes
}
fn site(raw: &str, offset: usize, bom: bool) -> usize {
    usize::from(bom) * 2 + raw[..offset].encode_utf16().count() * 2
}
fn run(bytes: &[u8], cut: usize) -> (Vec<(String, SourceOrigin)>, StreamingSession) {
    let mut session = driver(limits());
    let mut rows = Vec::new();
    let mut emit = |event: LocatedEvent<'_>, origin: SourceOrigin, _: &mut Budget| {
        rows.push((name(event), origin));
        Ok(())
    };
    for part in [&bytes[..cut], &bytes[cut..]] {
        assert_eq!(session.advance(part, &mut emit).unwrap(), part.len());
    }
    session.finish(&mut emit).unwrap();
    (rows, session)
}

#[test]
fn real_utf16_every_transport_split_preserves_dtd_body_and_causal_origins() {
    let raw = "<?xml version='1.0' encoding='UTF-16'?><!--π--><!DOCTYPE r [<!ENTITY % p \"<!ENTITY e '<a/>𝄞&#13;tail'>\">%p;<!ENTITY outer '&e;end'><!ATTLIST a x NMTOKENS ' one  two '>]><r>pre&outer;post&#13;&amp;π</r>";
    let expected = [
        "xml:None",
        "Cπ",
        "dtd:r",
        "entity:p",
        "entity:e",
        "entity:outer",
        "attlist",
        "dtd:end",
        "+r",
        "Tpre",
        "=a x=one two",
        "T𝄞\rtail",
        "Tend",
        "Tpost\r&π",
        "-r",
    ];
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
            let raw = raw.as_str();
            let bytes = encode(raw, little, bom);
            let (baseline, session) = run(&bytes, 0);
            assert_eq!(
                baseline
                    .iter()
                    .map(|row| row.0.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(session.position(), bytes.len());
            let Some(Phase::Content(content)) = &session.session[0].phase else {
                panic!("body owner")
            };
            assert_eq!(content.position(), (raw.len(), bytes.len()));
            assert_eq!(
                session.dtd_counters().unwrap().physical_bytes,
                bytes.len() - site(raw, raw.find("<!DOCTYPE").unwrap(), bom)
            );
            let range = |text: &str| {
                let start = raw.find(text).unwrap();
                site(raw, start, bom)..site(raw, start + text.len(), bom)
            };
            assert_eq!(
                baseline[0].1.input_range(),
                Some(range(&raw[..raw.find("?>").unwrap() + 2]))
            );
            assert_eq!(baseline[1].1.input_range(), Some(range("<!--π-->")));
            assert_eq!(
                baseline[3].1.input_range(),
                Some(range("<!ENTITY % p \"<!ENTITY e '<a/>𝄞&#13;tail'>\">"))
            );
            assert_eq!(baseline[4].1.input_range(), None);
            assert_eq!(baseline[4].1.physical_context(), range("%p;"));
            assert_eq!(baseline[7].1.input_range(), Some(range("]>")));
            assert_eq!(baseline[8].1.input_range(), Some(range("<r>")));
            assert_eq!(baseline[10].1.physical_context(), range("&outer;"));
            assert_eq!(baseline[10].1.replacement_range(), Some(0..4));
            assert_eq!(baseline[11].1.replacement_range(), Some(4..13));
            assert_eq!(baseline[12].1.replacement_range(), Some(3..6));
            assert_eq!(baseline[13].1.input_range(), Some(range("post&#13;&amp;π")));
            for cut in 0..=bytes.len() {
                let (rows, session) = run(&bytes, cut);
                assert_eq!(rows, baseline, "little={little} bom={bom} split={cut}");
                assert_eq!(session.position(), bytes.len());
            }
            let mut session = driver(limits());
            let mut rows = Vec::new();
            for byte in &bytes {
                assert_eq!(
                    session
                        .advance(std::slice::from_ref(byte), &mut |event, origin, _| {
                            rows.push((name(event), origin));
                            Ok(())
                        })
                        .unwrap(),
                    1
                );
            }
            session
                .finish(&mut |event, origin, _| {
                    rows.push((name(event), origin));
                    Ok(())
                })
                .unwrap();
            assert_eq!(rows, baseline);
        }
    }
}

#[test]
fn utf16_body_physical_quota_is_checked_before_each_scalar() {
    for little in [false, true] {
        for (raw, exact, first_disallowed) in [
            ("<!DOCTYPE r><r/>", 32, 32),
            ("<!DOCTYPE r><r>𝄞</r>", 42, 42),
        ] {
            let bytes = encode(raw, little, true);
            assert_eq!(bytes.len() - 2, exact);
            for limit in [exact, exact - 1] {
                let mut config = options();
                config.declarations.max_physical_bytes = limit;
                let mut session = StreamingSession::new(
                    limits(),
                    DecodeLimits {
                        max_decoded_bytes: 4096,
                        max_decoded_token_bytes: 4096,
                    },
                    config,
                );
                let mut rows = Vec::new();
                let result = session.advance(&bytes, &mut |event, _, _| {
                    rows.push(name(event));
                    Ok(())
                });
                if limit == exact {
                    assert_eq!(result.unwrap(), bytes.len());
                    session
                        .finish(&mut |event, _, _| {
                            rows.push(name(event));
                            Ok(())
                        })
                        .unwrap();
                    assert!(rows.last().unwrap() == "=r" || rows.last().unwrap() == "-r");
                } else {
                    let error = result.unwrap_err();
                    assert!(
                        matches!(error, SessionError::Content(ContentError { kind: ContentErrorKind::PhysicalBytes, physical_offset, .. }) if physical_offset == first_disallowed)
                    );
                    assert!(!rows.iter().any(|row| row == "=r" || row == "-r"));
                    assert_eq!(
                        session.finish(&mut |_, _, _| panic!("sticky quota")),
                        Err(error)
                    );
                    assert_eq!(session.dtd_counters().unwrap().physical_bytes, exact - 2);
                }
            }
        }
    }
}

#[test]
fn utf16_decoder_carry_errors_retain_real_transport_offsets_after_handoff() {
    for little in [false, true] {
        for prefix in ["<!DOCTYPE r [", "<!DOCTYPE r><r>", "<!DOCTYPE r><r>𝄞"] {
            for tail in [
                vec![0x41],
                if little {
                    vec![0x00, 0xd8]
                } else {
                    vec![0xd8, 0x00]
                },
                if little {
                    vec![0x00, 0xdc]
                } else {
                    vec![0xdc, 0x00]
                },
            ] {
                let mut bytes = encode(prefix, little, true);
                let anchor = bytes.len();
                bytes.extend(tail);
                for chunk in [1, bytes.len()] {
                    let mut session = driver(limits());
                    let mut failure = None;
                    for part in bytes.chunks(chunk) {
                        match session.advance(part, &mut |_, _, _| Ok(())) {
                            Ok(consumed) => assert_eq!(consumed, part.len()),
                            Err(error) => {
                                failure = Some(error);
                                break;
                            }
                        }
                    }
                    let error = failure
                        .unwrap_or_else(|| session.finish(&mut |_, _, _| Ok(())).unwrap_err());
                    assert_eq!(
                        error,
                        SessionError::Xml(Error::new(ErrorKind::InvalidUtf16, anchor))
                    );
                    assert_eq!(session.position(), bytes.len());
                    assert_eq!(
                        session.finish(&mut |_, _, _| panic!("sticky carry")),
                        Err(error)
                    );
                }
            }
        }
    }
}

#[test]
fn utf16_body_grammar_errors_map_after_non_bmp_and_keep_reference_origins() {
    let raw = "<?xml version='1.0' encoding='UTF-16'?><!DOCTYPE r><r>𝄞&#0;</r>";
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
            let raw = raw.as_str();
            let bytes = encode(raw, little, bom);
            let anchor = site(raw, raw.find("&#0;").unwrap(), bom);
            let mut session = driver(limits());
            let error = session.advance(&bytes, &mut |_, _, _| Ok(())).unwrap_err();
            assert!(
                matches!(error, SessionError::Content(ContentError { kind: ContentErrorKind::Text(_), physical_offset, .. }) if physical_offset == anchor)
            );
            assert_eq!(session.finish(&mut |_, _, _| Ok(())), Err(error));
        }
    }
}

#[test]
fn encoding_resume_validates_geometry_and_accounts_for_utf16_scratch() {
    use crate::tokenizer::text::{TextCoordinator, TextErrorKind, TextShared, TextSourceLease};
    for little_endian in [false, true] {
        let encoding = SourceEncoding::Utf16 {
            little_endian,
            bom: true,
        };
        let lease = TextSourceLease::resume(Tokenizer::new(17, 100), encoding, 11, 20).unwrap();
        assert_eq!(lease.input_position(), Some((11, 20)));
        assert_eq!(lease.input_encoding(), Some(encoding));
        for slot in [23, 24] {
            let mut text = options().text;
            text.scratch.input_token_bytes = slot;
            text.scratch.max_bytes = slot + text.scratch.replacement_token_bytes + 1024;
            let lease = TextSourceLease::resume(Tokenizer::new(17, 100), encoding, 11, 20).unwrap();
            let result = TextCoordinator::new(TextShared::new(text, 0).unwrap(), lease);
            if slot == 24 {
                assert!(result.is_ok());
            } else {
                assert_eq!(result.err().unwrap().kind, TextErrorKind::Scratch);
            }
        }
        for physical in [0, 1, 19] {
            let error = TextSourceLease::resume(Tokenizer::new(17, 100), encoding, 11, physical)
                .err()
                .unwrap();
            assert_eq!(error.kind, TextErrorKind::InvalidState);
            assert_eq!(error.offset, physical);
        }
    }
}

#[test]
fn utf16_non_bmp_transport_quota_does_not_reject_already_charged_continuations() {
    let raw = "<!DOCTYPE r><r>𝄞</r>";
    for little in [false, true] {
        for (limit, offset, spent) in [(33, 32, 30), (34, 36, 34)] {
            let bytes = encode(raw, little, true);
            let mut config = options();
            config.declarations.max_physical_bytes = limit;
            let mut session = StreamingSession::new(
                limits(),
                DecodeLimits {
                    max_decoded_bytes: 4096,
                    max_decoded_token_bytes: 4096,
                },
                config,
            );
            let mut events = Vec::new();
            let error = session
                .advance(&bytes, &mut |event, _, _| {
                    events.push(name(event));
                    Ok(())
                })
                .unwrap_err();
            assert!(
                matches!(error, SessionError::Content(ContentError {kind: ContentErrorKind::PhysicalBytes, physical_offset, ..}) if physical_offset == offset),
                "{error:?}"
            );
            assert_eq!(events, ["dtd:r", "dtd:end", "+r"]);
            assert_eq!(session.dtd_counters().unwrap().physical_bytes, spent);
        }
    }
}
