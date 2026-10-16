use super::*;

fn utf16(text: &str, little_endian: bool, bom: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    if bom {
        bytes.extend(if little_endian {
            [0xff, 0xfe]
        } else {
            [0xfe, 0xff]
        });
    }
    for unit in text.encode_utf16() {
        bytes.extend(if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}

#[test]
fn decoding_is_independent_of_every_byte_boundary() {
    for (input, expected_encoding, bom, expected) in [
        (
            b"<r>\xc3\xa9\xf0\x9f\x99\x82</r>".to_vec(),
            SourceEncoding::Utf8,
            0,
            "<r>é🙂</r>",
        ),
        (
            [b"\xef\xbb\xbf".as_slice(), "<r>é🙂</r>".as_bytes()].concat(),
            SourceEncoding::Utf8,
            3,
            "<r>é🙂</r>",
        ),
        (
            utf16("<r>é🙂</r>", true, true),
            SourceEncoding::Utf16 {
                little_endian: true,
                bom: true,
            },
            2,
            "<r>é🙂</r>",
        ),
        (
            utf16("<r>é🙂</r>", false, true),
            SourceEncoding::Utf16 {
                little_endian: false,
                bom: true,
            },
            2,
            "<r>é🙂</r>",
        ),
        (
            utf16("<?xml", true, false),
            SourceEncoding::Utf16 {
                little_endian: true,
                bom: false,
            },
            0,
            "<?xml",
        ),
        (
            utf16("<?xml", false, false),
            SourceEncoding::Utf16 {
                little_endian: false,
                bom: false,
            },
            0,
            "<?xml",
        ),
    ] {
        for split in 0..=input.len() {
            let mut decoder = Decoder::new(expected.len());
            let mut found = Vec::new();
            let mut starts = 0;
            let mut emit = |event: Decoded<'_>| {
                match event {
                    Decoded::Start {
                        encoding,
                        bom: actual_bom,
                    } => {
                        starts += 1;
                        assert_eq!(encoding, expected_encoding);
                        assert_eq!(actual_bom, bom);
                    }
                    Decoded::Bytes(bytes) => found.extend_from_slice(bytes),
                }
                Ok(())
            };
            decoder.feed(&input[..split], &mut emit).unwrap();
            decoder.feed(&input[split..], &mut emit).unwrap();
            decoder.finish(&mut emit).unwrap();
            assert_eq!(starts, 1);
            assert_eq!(found, expected.as_bytes());
            assert_eq!(decoder.position(), input.len());
        }
    }
}

#[test]
fn signatures_wait_until_ambiguity_is_resolved() {
    for signature in UTF32 {
        for split in 0..=4 {
            let mut decoder = Decoder::new(1024);
            let mut starts = 0;
            let mut emit = |event: Decoded<'_>| {
                if matches!(event, Decoded::Start { .. }) {
                    starts += 1;
                }
                Ok(())
            };
            let error = decoder
                .feed(&signature[..split], &mut emit)
                .and_then(|()| decoder.feed(&signature[split..], &mut emit))
                .unwrap_err();
            assert_eq!(error, Error::new(ErrorKind::UnsupportedEncoding, 0));
            assert_eq!(decoder.position(), 4);
            assert_eq!(starts, 0);
        }
    }
    for prefix in [
        b"".as_slice(),
        b"\xef",
        b"\xef\xbb",
        b"\xff",
        b"\xfe",
        b"\0",
        b"<",
        b"<\0",
        b"<\0?",
        b"\0<",
        b"\0<\0",
    ] {
        let mut decoder = Decoder::new(1024);
        let mut found = Vec::new();
        let mut emit = |event: Decoded<'_>| {
            if let Decoded::Bytes(bytes) = event {
                found.extend_from_slice(bytes);
            }
            Ok(())
        };
        decoder.feed(prefix, &mut emit).unwrap();
        decoder.finish(&mut emit).unwrap();
        assert_eq!(found, prefix);
    }
    for (bytes, little_endian) in [([0xff, 0xfe], true), ([0xfe, 0xff], false)] {
        let mut decoder = Decoder::new(0);
        decoder.feed(&bytes, |_| Ok(())).unwrap();
        decoder.finish(|event| {
            assert!(matches!(event, Decoded::Start { encoding: SourceEncoding::Utf16 { little_endian: actual, bom: true }, bom: 2 } if actual == little_endian));
            Ok(())
        }).unwrap();
        assert_eq!(decoder.position(), 2);
    }
}

#[test]
fn malformed_utf16_reports_the_original_unit() {
    for little_endian in [false, true] {
        for (units, odd, offset) in [
            (vec![0xdc00], false, 2),
            (vec![0xd800, b'x' as u16], false, 2),
            (vec![0xd800, 0xd800], false, 2),
            (vec![b'x' as u16, 0xdbff], false, 4),
            (vec![b'x' as u16], true, 4),
            (vec![0xd800], true, 2),
        ] {
            let mut input = if little_endian {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in units {
                input.extend(if little_endian {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            if odd {
                input.push(0);
            }
            for size in 1..=input.len() {
                let mut decoder = Decoder::new(1024);
                let error = input
                    .chunks(size)
                    .try_for_each(|chunk| decoder.feed(chunk, |_| Ok(())))
                    .and_then(|()| decoder.finish(|_| Ok(())))
                    .unwrap_err();
                assert_eq!(error, Error::new(ErrorKind::InvalidUtf16, offset));
            }
        }
    }
}

#[test]
fn decoded_total_is_checked_before_emitting_excess() {
    for input in [
        b"ab".to_vec(),
        utf16("ab", true, true),
        utf16("ab", false, true),
    ] {
        let mut decoder = Decoder::new(1);
        let mut found = Vec::new();
        let error = decoder
            .feed(&input, |event| {
                if let Decoded::Bytes(bytes) = event {
                    found.extend_from_slice(bytes);
                }
                Ok(())
            })
            .unwrap_err();
        assert_eq!(
            error,
            Error::new(ErrorKind::LimitExceeded(Limit::DecodedBytes), 0)
        );
        assert_eq!(found, b"a");
    }
    for text in ["é", "中", "🙂"] {
        let input = utf16(text, true, true);
        let mut decoder = Decoder::new(text.len() - 1);
        let error = decoder
            .feed(&input, |event| {
                assert!(!matches!(event, Decoded::Bytes(_)));
                Ok(())
            })
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::LimitExceeded(Limit::DecodedBytes));
    }
    let mut decoder = Decoder::new(0);
    decoder.feed(b"\xef\xbb\xbf", |_| Ok(())).unwrap();
    decoder.finish(|_| Ok(())).unwrap();
}

#[test]
fn consumer_failure_is_preserved_and_stops_conversion() {
    let expected = Error::new(ErrorKind::Allocation, 19);
    for input in [b"<r/>".to_vec(), utf16("<r/>", true, true)] {
        let mut decoder = Decoder::new(1024);
        let error = decoder
            .feed(&input, |event| match event {
                Decoded::Start { .. } => Ok(()),
                Decoded::Bytes(_) => Err(expected),
            })
            .unwrap_err();
        assert_eq!(error, expected);
    }
    let mut decoder = Decoder::new(1024);
    decoder.feed(b"\xef", |_| Ok(())).unwrap();
    assert_eq!(decoder.finish(|_| Err(expected)), Err(expected));
    let mut decoder = Decoder::new(1024);
    assert_eq!(decoder.feed(b"x", |_| Err(expected)), Err(expected));
}

#[test]
fn semantic_offsets_use_unnormalized_scalar_prefixes() {
    let raw = "<r>é🙂&#xG;</r>";
    let span = TokenSpan {
        decoded: 100..100 + raw.len(),
        source: 12..12 + raw.encode_utf16().count() * 2,
        encoding: SourceEncoding::Utf16 {
            little_endian: true,
            bom: true,
        },
    };
    for (relative, expected) in [
        (0, 12),
        (3, 18),
        (5, 20),
        (9, 24),
        (10, 26),
        (raw.len(), span.source.end),
    ] {
        assert_eq!(
            span.map_error(raw, Error::new(ErrorKind::InvalidReference, 100 + relative)),
            Error::new(ErrorKind::InvalidReference, expected)
        );
    }
    assert_eq!(
        span.map_error(raw, Error::new(ErrorKind::Syntax, 106))
            .offset,
        20
    );
    let utf8 = TokenSpan {
        decoded: 100..200,
        source: 100..200,
        encoding: SourceEncoding::Utf8,
    };
    assert_eq!(
        utf8.map_error(raw, Error::new(ErrorKind::Syntax, 104))
            .offset,
        104
    );
    assert!(needs_decoding(&utf16("<r/>", true, true)));
    assert!(needs_decoding(&utf16("<?xml", false, false)));
    assert!(!needs_decoding(b"\xef\xbb\xbf<r/>"));
    assert!(!needs_decoding(b"<r/>"));
}

#[test]
fn utf16_scalar_is_charged_once_before_suspension_and_carry_survives() {
    for little_endian in [false, true] {
        for text in ["é", "中", "🙂"] {
            let input = utf16(text, little_endian, true);
            let mut decoder = Decoder::new(text.len());
            let mut found = Vec::new();
            let progress = decoder
                .advance(&input, |event| {
                    let Decoded::Bytes(bytes) = event else {
                        return Ok(DecodeProgress {
                            consumed: 0,
                            suspended: false,
                        });
                    };
                    found.push(bytes[0]);
                    Ok(DecodeProgress {
                        consumed: 1,
                        suspended: true,
                    })
                })
                .unwrap();
            assert!(progress.suspended);
            assert_eq!(progress.consumed, input.len());
            assert_eq!(decoder.position(), input.len());
            assert_eq!(decoder.decoded, text.len());
            assert_eq!(decoder.pending_len - decoder.pending_cursor, text.len() - 1);
            while found.len() < text.len() {
                let progress = decoder
                    .advance(&[], |event| {
                        let Decoded::Bytes(bytes) = event else {
                            panic!("encoding started twice")
                        };
                        found.push(bytes[0]);
                        Ok(DecodeProgress {
                            consumed: 1,
                            suspended: true,
                        })
                    })
                    .unwrap();
                assert_eq!(progress.consumed, 0);
                assert!(progress.suspended);
                assert_eq!(decoder.decoded, text.len());
            }
            assert_eq!(found, text.as_bytes());
            assert!(
                !decoder
                    .finish_step(|_| panic!("carry already delivered"))
                    .unwrap()
            );
            let next = utf16("x", little_endian, false);
            assert_eq!(
                decoder
                    .advance(&next, |_| panic!("total quota was exhausted"))
                    .unwrap_err(),
                Error::new(ErrorKind::LimitExceeded(Limit::DecodedBytes), 0)
            );
            assert_eq!(decoder.position(), input.len() + 2);
            let mut decoder = Decoder::new(text.len() - 1);
            assert_eq!(
                decoder
                    .advance(&input, |event| {
                        assert!(matches!(event, Decoded::Start { .. }));
                        Ok(DecodeProgress {
                            consumed: 0,
                            suspended: false,
                        })
                    })
                    .unwrap_err(),
                Error::new(ErrorKind::LimitExceeded(Limit::DecodedBytes), 0)
            );
        }
    }
}

#[test]
fn utf8_and_final_preamble_suspension_charge_only_delivered_bytes() {
    let mut decoder = Decoder::new(4);
    let mut collected = Vec::new();
    let mut offset = 0;
    while offset < 4 {
        let progress = decoder
            .advance(&b"abcd"[offset..], |event| {
                let Decoded::Bytes(bytes) = event else {
                    return Ok(DecodeProgress {
                        consumed: 0,
                        suspended: false,
                    });
                };
                collected.push(bytes[0]);
                Ok(DecodeProgress {
                    consumed: 1,
                    suspended: true,
                })
            })
            .unwrap();
        offset += progress.consumed;
        assert_eq!(decoder.decoded, collected.len());
        assert_eq!(decoder.position(), collected.len());
    }
    assert_eq!(collected, b"abcd");
    decoder.finish(|_| panic!("no bytes remain")).unwrap();
    let mut decoder = Decoder::new(2);
    decoder
        .feed(b"\xef\xbb", |_| panic!("preamble still ambiguous"))
        .unwrap();
    let mut found = Vec::new();
    assert!(
        decoder
            .finish_step(|event| {
                match event {
                    Decoded::Start { .. } => Ok(DecodeProgress {
                        consumed: 0,
                        suspended: false,
                    }),
                    Decoded::Bytes(bytes) => {
                        found.push(bytes[0]);
                        Ok(DecodeProgress {
                            consumed: 1,
                            suspended: true,
                        })
                    }
                }
            })
            .unwrap()
    );
    assert_eq!(decoder.decoded, 1);
    assert!(
        !decoder
            .finish_step(|event| {
                let Decoded::Bytes(bytes) = event else {
                    panic!("encoding started twice")
                };
                found.extend(bytes);
                Ok(DecodeProgress {
                    consumed: bytes.len(),
                    suspended: false,
                })
            })
            .unwrap()
    );
    assert_eq!(found, b"\xef\xbb");
    assert_eq!(decoder.position(), 2);
    assert_eq!(decoder.decoded, 2);
}

#[test]
fn decoded_consumer_progress_is_checked() {
    for (consumed, suspended, available, accepted) in [
        (0, false, 0, true),
        (1, false, 1, true),
        (0, true, 1, true),
        (1, true, 2, true),
        (2, true, 1, false),
        (0, false, 1, false),
    ] {
        let result = check_progress(
            DecodeProgress {
                consumed,
                suspended,
            },
            available,
            7,
        );
        assert_eq!(result.is_ok(), accepted);
        if !accepted {
            assert_eq!(result, Err(Error::new(ErrorKind::InvalidState, 7)));
        }
    }
}
