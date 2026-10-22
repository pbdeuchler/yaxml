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
    for signature in UNSUPPORTED {
        // The `encodings` feature decodes the UCS-4 and EBCDIC `<` signatures.
        if select(&signature, true).is_some_and(|selection| selection.is_ok()) {
            continue;
        }
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
    assert!(needs_decoding(&utf16("<r/>", true, true), false));
    assert!(needs_decoding(&utf16("<?xml", false, false), false));
    assert!(!needs_decoding(b"\xef\xbb\xbf<r/>", false));
    assert!(!needs_decoding(b"<r/>", true));
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

/// libxml2 2.15.4 xmllint output per byte, recorded by tooling/charset_oracle.py.
fn oracle() -> impl Iterator<Item = (&'static str, Vec<u32>)> {
    include_str!("../fixtures/charsets.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let (label, values) = line.split_once('\t').unwrap();
            let values = values
                .split(' ')
                .map(|value| u32::from_str_radix(value, 16).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(values.len(), 128);
            (label, values)
        })
}

fn byte_encoding(label: &str) -> SourceEncoding {
    match charset(label) {
        Some(Charset::Byte(table)) => SourceEncoding::Byte(table),
        other => panic!("{label} is not a byte charset: {other:?}"),
    }
}

#[test]
fn every_byte_charset_decodes_all_bytes_like_libxml2() {
    let mut labels = 0;
    for (label, high) in oracle() {
        labels += 1;
        for byte in 0..=u8::MAX {
            let mut decoder = Decoder::new(1024);
            decoder.switch(byte_encoding(label));
            let mut found = Vec::new();
            let result = decoder.feed(&[byte], |decoded| {
                if let Decoded::Bytes(bytes) = decoded {
                    found.extend_from_slice(bytes);
                }
                Ok(())
            });
            let expected = match byte {
                0..0x80 => Some(char::from(byte)),
                _ => char::from_u32(high[usize::from(byte - 0x80)]).filter(|c| *c != '\0'),
            };
            match expected {
                Some(character) => {
                    result.unwrap();
                    assert_eq!(found, character.to_string().as_bytes(), "{label} {byte:#x}");
                }
                None => {
                    assert_eq!(result, Err(Error::new(ErrorKind::UnmappedByte, 0)));
                    assert!(found.is_empty());
                }
            }
            assert_eq!(decoder.position(), 1);
        }
    }
    assert_eq!(labels, 17);
}

#[test]
fn byte_charsets_carry_scalars_across_suspension_and_limits() {
    let input = b"a\xe9\xa4b";
    for (label, limit, expected) in [
        ("ISO-8859-1", 64, Ok("a\u{e9}\u{a4}b")),
        ("ISO-8859-15", 64, Ok("a\u{e9}\u{20ac}b")),
        (
            "ISO-8859-15",
            4,
            Err(Error::new(ErrorKind::LimitExceeded(Limit::DecodedBytes), 0)),
        ),
        ("US-ASCII", 64, Err(Error::new(ErrorKind::UnmappedByte, 1))),
    ] {
        let mut decoder = Decoder::new(limit);
        decoder.switch(byte_encoding(label));
        let mut found = Vec::new();
        // Consume one decoded byte per callback so each scalar's carry resumes.
        let mut result = Ok(());
        let mut cursor = 0;
        while cursor < input.len() && result.is_ok() {
            result = decoder
                .advance(&input[cursor..], |decoded| {
                    let Decoded::Bytes(bytes) = decoded else {
                        unreachable!()
                    };
                    found.push(bytes[0]);
                    Ok(DecodeProgress {
                        consumed: 1,
                        suspended: true,
                    })
                })
                .map(|progress| cursor += progress.consumed);
        }
        match expected {
            Ok(text) => {
                result.unwrap();
                decoder
                    .finish(|decoded| {
                        if let Decoded::Bytes(bytes) = decoded {
                            found.extend_from_slice(bytes);
                        }
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(found, text.as_bytes(), "{label}");
                assert_eq!(decoder.position(), input.len());
            }
            Err(error) => assert_eq!(result, Err(error), "{label}"),
        }
    }
}

#[test]
fn labels_resolve_like_libxml2_without_iconv() {
    for (alias, canonical) in [
        ("latin1", "ISO-8859-1"),
        ("ISO_8859-1:1987", "ISO-8859-1"),
        ("CP819", "ISO-8859-1"),
        ("l2", "ISO-8859-2"),
        ("Cyrillic", "ISO-8859-5"),
        ("asmo-708", "ISO-8859-6"),
        ("greek8", "ISO-8859-7"),
        ("visual", "ISO-8859-8"),
        ("latin5", "ISO-8859-9"),
        ("L6", "ISO-8859-10"),
        ("iso885913", "ISO-8859-13"),
        ("csisolatin9", "ISO-8859-15"),
        // libxml2's table resolves these spellings to Latin-1.
        ("iso8859-15", "ISO-8859-1"),
        ("ISO8859-10", "ISO-8859-1"),
        ("ascii", "US-ASCII"),
        ("ANSI_X3.4-1968", "US-ASCII"),
        ("x-cp1252", "windows-1252"),
    ] {
        assert_eq!(charset(alias), charset(canonical), "{alias}");
        assert!(matches!(charset(alias), Some(Charset::Byte(_))), "{alias}");
    }
    for (label, expected) in [
        ("utf8", Some(Charset::Utf8)),
        ("UNICODE-1-1-UTF-8", Some(Charset::Utf8)),
        ("csUnicode", Some(Charset::Utf16(None))),
        // Out of order in libxml2's table: its binary search misses it.
        ("unicodefeff", None),
        ("UTF-16BE", Some(Charset::Utf16(Some(false)))),
        // Recognized by libxml2 but only decodable through iconv/ICU.
        ("Shift_JIS", None),
        ("EUC-JP", None),
        ("ISO-2022-JP", None),
        ("UCS-2", None),
        ("HTML", None),
        ("ISO-8859-12", None),
        ("iso885911", None),
        ("latin-1", None),
        (" UTF-8", None),
        ("", None),
    ] {
        assert_eq!(charset(label), expected, "{label}");
    }
    let iconv = cfg!(feature = "encodings");
    assert_eq!(
        charset("UCS-4"),
        iconv.then_some(Charset::Ucs4(Some(false)))
    );
    assert_eq!(charset("utf-32"), iconv.then_some(Charset::Ucs4(None)));
    assert_eq!(
        matches!(charset("ebcdic-cp-us"), Some(Charset::Ebcdic(_))),
        iconv
    );
}

#[test]
fn owned_routing_peeks_only_at_a_byte_charset_label() {
    for (input, expected) in [
        (
            &b"<?xml version='1.0' encoding='latin1'?><r/>"[..],
            (true, true),
        ),
        (
            b"<?xml version=\"1.0\" encoding = \"ISO-8859-2\" ?><r/>",
            (true, true),
        ),
        (
            b"<?xml version='1.0' encoding='UTF-8'?><r/>",
            (false, false),
        ),
        (
            b"<?xml version='1.0' encoding='Shift_JIS'?><r/>",
            (false, true),
        ),
        (
            b"<?xml version='1.0'?><r a='encoding=\"latin1\"'/>",
            (false, false),
        ),
        (b"<?xml version='1.0' encoding='latin1", (false, false)),
        (b"<?xml version='1.0' encoding latin1?>", (false, false)),
        (b"<?xml version='1.0' encoding=`latin1`?>", (false, false)),
        (
            b"<?xml\x0cversion='1.0' encoding='latin1'?>",
            (false, false),
        ),
        (b"<?xml version='1.0' encoding='latin1'", (false, false)),
        (b"<r/>", (false, false)),
        (b"Lo\xa7\x94", (true, true)),
    ] {
        let found = (needs_decoding(input, false), needs_decoding(input, true));
        assert_eq!(found, expected, "{input:?}");
    }
}
