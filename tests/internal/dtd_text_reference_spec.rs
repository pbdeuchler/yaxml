//! Independent literal oracles for `docs/plans/dtd-text-reference-adapter.md`.

use crate::dtd_reference::{ReferenceKind, ReferenceLimits, ReferenceScratch};
use crate::encoding::{DecodeProgress, Decoded, Decoder, SourceEncoding, TokenSpan};
use crate::tokenizer::local::{LocalError, LocalErrorKind, LocalLimits, LocalTokenizer};
use crate::tokenizer::reference::{
    ContentStop, ReferenceCursor, ReferenceCursorError, ReferenceCursorErrorKind, ReferenceExtent,
};
use crate::tokenizer::{TokenKind, Tokenizer};
use crate::{Error, ErrorKind, Limit};
use std::ops::Range;

#[derive(Debug, Eq, PartialEq)]
struct InputToken {
    kind: TokenKind,
    raw: Vec<u8>,
    decoded: Range<usize>,
    source: Range<usize>,
    encoding: SourceEncoding,
}

#[derive(Debug, Eq, PartialEq)]
struct GeneralBoundary {
    raw: String,
    decoded: Range<usize>,
    source: Range<usize>,
}

#[derive(Debug, Eq, PartialEq)]
struct InputTrace {
    tokens: Vec<InputToken>,
    generals: Vec<GeneralBoundary>,
}

fn reference_limits(length: usize) -> ReferenceLimits {
    ReferenceLimits {
        max_bytes: length,
        max_scratch_bytes: length,
        max_work: length * 3,
    }
}

fn encode_utf16(text: &str, little_endian: bool) -> Vec<u8> {
    let mut output = if little_endian {
        vec![0xff, 0xfe]
    } else {
        vec![0xfe, 0xff]
    };
    for unit in text.encode_utf16() {
        output.extend(if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    output
}

fn decode_at_split(source: &[u8], split: usize) -> (SourceEncoding, usize, Vec<Vec<u8>>) {
    let mut decoder = Decoder::new(1024 * 1024);
    let mut selection = None;
    let mut chunks = Vec::new();
    let mut accept = |decoded: Decoded<'_>| {
        let consumed = match decoded {
            Decoded::Start { encoding, bom } => {
                selection = Some((encoding, bom));
                0
            }
            Decoded::Bytes(bytes) => {
                let consumed = bytes.len();
                chunks.push(bytes.to_vec());
                consumed
            }
        };
        Ok(DecodeProgress {
            consumed,
            suspended: false,
        })
    };
    for input in [&source[..split], &source[split..]] {
        let mut cursor = 0;
        while cursor < input.len() {
            let progress = decoder.advance(&input[cursor..], &mut accept).unwrap();
            assert!(!progress.suspended);
            assert!(progress.consumed > 0);
            cursor += progress.consumed;
        }
    }
    assert!(!decoder.finish_step(&mut accept).unwrap());
    let (encoding, bom) = selection.expect("decoder selected an encoding");
    (encoding, bom, chunks)
}

fn take_input_token(tokenizer: &mut Tokenizer, tokens: &mut Vec<InputToken>) -> bool {
    let Some((kind, raw, span)) = tokenizer.current() else {
        return false;
    };
    tokens.push(InputToken {
        kind,
        raw: raw.to_vec(),
        decoded: span.decoded.clone(),
        source: span.source.clone(),
        encoding: span.encoding,
    });
    tokenizer.release();
    true
}

fn drive_input(
    encoding: SourceEncoding,
    bom: usize,
    chunks: &[Vec<u8>],
    raw_limit: usize,
    decoded_limit: usize,
) -> InputTrace {
    let mut tokenizer = Tokenizer::new(raw_limit, decoded_limit);
    tokenizer.configure(encoding, bom);
    let mut tokens = Vec::new();
    let mut generals = Vec::new();
    let mut pending: Option<(ReferenceScratch, ReferenceCursor)> = None;

    for chunk in chunks {
        let mut cursor = 0;
        while cursor < chunk.len() {
            if let Some((scratch, extent_cursor)) = pending.as_mut() {
                let byte = &chunk[cursor..cursor + 1];
                let advanced = extent_cursor.checked_advance(byte).unwrap();
                assert_eq!(scratch.advance(byte).unwrap(), 1);
                *extent_cursor = advanced;
                cursor += 1;
                if let Some(reference) = scratch.validated() {
                    let extent = extent_cursor.finish(reference).unwrap();
                    match reference.kind() {
                        ReferenceKind::General => {
                            if tokenizer.flush_text_before_reference().unwrap() {
                                assert!(take_input_token(&mut tokenizer, &mut tokens));
                            }
                            let span = extent.input_span().expect("input reference span");
                            generals.push(GeneralBoundary {
                                raw: reference.raw().to_owned(),
                                decoded: span.decoded.clone(),
                                source: span.source.clone(),
                            });
                            tokenizer
                                .skip_general_reference(reference, &extent)
                                .unwrap();
                        }
                        ReferenceKind::Numeric(_) | ReferenceKind::Predefined(_) => tokenizer
                            .append_character_reference(reference, &extent)
                            .unwrap(),
                    }
                    scratch.release();
                    pending = None;
                }
                continue;
            }

            let step = tokenizer.advance_content(&chunk[cursor..]).unwrap();
            cursor += step.consumed;
            match step.stop {
                ContentStop::ReferenceStart => {
                    let extent_cursor = tokenizer.reference_cursor().unwrap();
                    pending = Some((
                        ReferenceScratch::new(
                            extent_cursor.decoded_position(),
                            reference_limits(128),
                        ),
                        extent_cursor,
                    ));
                }
                ContentStop::TokenReady => {
                    assert!(take_input_token(&mut tokenizer, &mut tokens));
                }
                ContentStop::Exhausted => {}
            }
        }
    }
    assert!(pending.is_none(), "fixture ended inside a reference");
    tokenizer.finish_step().unwrap();
    take_input_token(&mut tokenizer, &mut tokens);
    InputTrace { tokens, generals }
}

fn collect_local_reference(
    tokenizer: &mut LocalTokenizer,
    raw: &'static str,
) -> (ReferenceScratch, ReferenceExtent) {
    let cursor = tokenizer.reference_cursor().unwrap();
    let mut scratch = ReferenceScratch::new(cursor.decoded_position(), reference_limits(raw.len()));
    let mut extent_cursor = cursor;
    for byte in raw.as_bytes() {
        let advanced = extent_cursor
            .checked_advance(std::slice::from_ref(byte))
            .unwrap();
        assert_eq!(scratch.advance(std::slice::from_ref(byte)).unwrap(), 1);
        extent_cursor = advanced;
    }
    let extent = extent_cursor.finish(scratch.validated().unwrap()).unwrap();
    (scratch, extent)
}

#[test]
fn local_numeric_and_predefined_spellings_join_text_once_without_rescan() {
    for (raw, kind) in [
        ("&#65;", ReferenceKind::Numeric('A')),
        ("&#x41;", ReferenceKind::Numeric('A')),
        ("&amp;", ReferenceKind::Predefined('&')),
    ] {
        let mut tokenizer = LocalTokenizer::new(
            0,
            LocalLimits {
                max_token_bytes: 64,
                max_scratch_bytes: 64,
                max_work: 256,
            },
        );
        let step = tokenizer.advance_content(b"p&").unwrap();
        assert_eq!(step.consumed, 1);
        assert_eq!(step.stop, ContentStop::ReferenceStart);
        let (mut scratch, extent) = collect_local_reference(&mut tokenizer, raw);
        let reference = scratch.validated().unwrap();
        assert_eq!(reference.kind(), kind);
        assert_eq!(reference.raw(), raw);
        tokenizer
            .append_character_reference(reference, &extent)
            .unwrap();
        scratch.release();
        assert_eq!(tokenizer.usage().reference_copied, raw.len());

        let suffix = b"q<x/>";
        let step = tokenizer.advance_content(suffix).unwrap();
        assert_eq!(step.consumed, 2);
        assert_eq!(step.stop, ContentStop::TokenReady);
        let (_, text, range) = tokenizer.current().unwrap();
        assert_eq!(text, format!("p{raw}q").as_bytes());
        assert_eq!(range, 0..raw.len() + 2);
        tokenizer.release();
        let step = tokenizer.advance_content(&suffix[2..]).unwrap();
        assert_eq!(step.stop, ContentStop::TokenReady);
        assert_eq!(tokenizer.current().unwrap().1, b"<x/>");
        tokenizer.release();
        tokenizer.finish_step().unwrap();
        assert!(tokenizer.is_complete());
        assert_eq!(tokenizer.usage().reference_copied, raw.len());
    }
}

#[test]
fn general_reference_flushes_prefix_then_skips_exactly_once() {
    let mut tokenizer = LocalTokenizer::new(
        11,
        LocalLimits {
            max_token_bytes: 64,
            max_scratch_bytes: 64,
            max_work: 256,
        },
    );
    let step = tokenizer.advance_content("pre&π;tail".as_bytes()).unwrap();
    assert_eq!(step.consumed, 3);
    assert_eq!(step.stop, ContentStop::ReferenceStart);
    let (mut scratch, extent) = collect_local_reference(&mut tokenizer, "&π;");
    let reference = scratch.validated().unwrap();
    assert_eq!(reference.kind(), ReferenceKind::General);
    assert_eq!(extent.local_range(), Some(14..18));

    assert!(tokenizer.flush_text_before_reference().unwrap());
    assert_eq!(tokenizer.current().unwrap().1, b"pre");
    assert_eq!(tokenizer.current().unwrap().2, 11..14);
    tokenizer.release();
    tokenizer
        .skip_general_reference(reference, &extent)
        .unwrap();
    scratch.release();
    assert_eq!(tokenizer.position(), 18);
    assert_eq!(tokenizer.usage().reference_copied, 0);

    let step = tokenizer.advance_content(b"tail<r/>").unwrap();
    assert_eq!(step.consumed, 5);
    assert_eq!(step.stop, ContentStop::TokenReady);
    assert_eq!(tokenizer.current().unwrap().1, b"tail");
    assert_eq!(tokenizer.current().unwrap().2, 18..22);
    tokenizer.release();
    assert_eq!(
        tokenizer.advance_content(b"r/>").unwrap().stop,
        ContentStop::TokenReady
    );
    assert_eq!(tokenizer.current().unwrap().1, b"<r/>");
}

#[test]
fn references_only_split_text_and_saved_less_than_replays_once() {
    let input = b"<e a='&a;'><!--&b;--><![CDATA[&c;]]><?p &d;?>text&g;";
    let mut tokenizer = LocalTokenizer::new(
        0,
        LocalLimits {
            max_token_bytes: 128,
            max_scratch_bytes: 128,
            max_work: 512,
        },
    );
    let expected = [
        (TokenKind::Start, b"<e a='&a;'>".as_slice()),
        (TokenKind::Comment, b"<!--&b;-->".as_slice()),
        (TokenKind::Cdata, b"<![CDATA[&c;]]>".as_slice()),
        (TokenKind::Instruction, b"<?p &d;?>".as_slice()),
    ];
    let mut cursor = 0;
    for (kind, raw) in expected {
        let step = tokenizer.advance_content(&input[cursor..]).unwrap();
        cursor += step.consumed;
        assert_eq!(step.stop, ContentStop::TokenReady);
        let current = tokenizer.current().unwrap();
        assert_eq!(current.0, kind);
        assert_eq!(current.1, raw);
        tokenizer.release();
    }
    let step = tokenizer.advance_content(&input[cursor..]).unwrap();
    assert_eq!(step.consumed, 4);
    assert_eq!(step.stop, ContentStop::ReferenceStart);

    let mut replay = LocalTokenizer::new(
        30,
        LocalLimits {
            max_token_bytes: 32,
            max_scratch_bytes: 32,
            max_work: 64,
        },
    );
    let first = replay.advance_content(b"pre<x/>").unwrap();
    assert_eq!(first.consumed, 4);
    assert_eq!(first.stop, ContentStop::TokenReady);
    assert_eq!(replay.current().unwrap().2, 30..33);
    replay.release();
    let second = replay.advance_content(b"x/>").unwrap();
    assert_eq!(second.consumed, 3);
    assert_eq!(replay.current().unwrap().1, b"<x/>");
    assert_eq!(replay.current().unwrap().2, 33..37);
}

#[test]
fn utf16_general_boundaries_keep_exact_decoded_and_physical_spans_at_every_split() {
    let text = "𝄞pre&π;post<e/>";
    for little_endian in [true, false] {
        let source = encode_utf16(text, little_endian);
        for split in 0..=source.len() {
            let (encoding, bom, chunks) = decode_at_split(&source, split);
            let trace = drive_input(encoding, bom, &chunks, 128, 128);
            assert_eq!(
                trace,
                InputTrace {
                    tokens: vec![
                        InputToken {
                            kind: TokenKind::Text,
                            raw: "𝄞pre".as_bytes().to_vec(),
                            decoded: 0..7,
                            source: 2..12,
                            encoding,
                        },
                        InputToken {
                            kind: TokenKind::Text,
                            raw: b"post".to_vec(),
                            decoded: 11..15,
                            source: 18..26,
                            encoding,
                        },
                        InputToken {
                            kind: TokenKind::Start,
                            raw: b"<e/>".to_vec(),
                            decoded: 15..19,
                            source: 26..34,
                            encoding,
                        },
                    ],
                    generals: vec![GeneralBoundary {
                        raw: "&π;".to_owned(),
                        decoded: 7..11,
                        source: 12..18,
                    }],
                },
                "endian={little_endian} split={split}",
            );
        }
    }
}

fn append_utf16_character_reference(
    little_endian: bool,
    raw_limit: usize,
    decoded_limit: usize,
) -> Result<(Vec<u8>, TokenSpan), Error> {
    let source = encode_utf16("𝄞&amp;<", little_endian);
    let (encoding, bom, chunks) = decode_at_split(&source, 5);
    let decoded = chunks.concat();
    assert_eq!(decoded, "𝄞&amp;<".as_bytes());
    let mut tokenizer = Tokenizer::new(raw_limit, decoded_limit);
    tokenizer.configure(encoding, bom);
    let step = tokenizer.advance_content("𝄞&amp;<".as_bytes()).unwrap();
    assert_eq!(step.consumed, 4);
    assert_eq!(step.stop, ContentStop::ReferenceStart);
    let mut cursor = tokenizer.reference_cursor().unwrap();
    let mut scratch = ReferenceScratch::new(4, reference_limits(5));
    for byte in b"&amp;" {
        cursor = cursor
            .checked_advance(std::slice::from_ref(byte))
            .map_err(|error| {
                Error::new(
                    match error.kind {
                        ReferenceCursorErrorKind::Limit(limit) => ErrorKind::LimitExceeded(limit),
                        ReferenceCursorErrorKind::PositionOverflow => ErrorKind::Allocation,
                        ReferenceCursorErrorKind::InvalidState => ErrorKind::InvalidState,
                    },
                    error.offset,
                )
            })?;
        scratch.advance(std::slice::from_ref(byte)).unwrap();
    }
    let reference = scratch.validated().unwrap();
    let extent = cursor.finish(reference).unwrap();
    tokenizer.append_character_reference(reference, &extent)?;
    let step = tokenizer.advance_content(b"<").unwrap();
    assert_eq!(step.stop, ContentStop::TokenReady);
    let (_, raw, span) = tokenizer.current().unwrap();
    Ok((raw.to_vec(), span.clone()))
}

#[test]
fn utf16_combined_text_limits_use_physical_width_and_decoded_utf8_length() {
    for little_endian in [true, false] {
        let (raw, span) = append_utf16_character_reference(little_endian, 14, 9).unwrap();
        assert_eq!(raw, "𝄞&amp;".as_bytes());
        assert_eq!(span.decoded, 0..9);
        assert_eq!(span.source, 2..16);

        assert_eq!(
            append_utf16_character_reference(little_endian, 13, 9).unwrap_err(),
            Error::new(ErrorKind::LimitExceeded(Limit::TokenBytes), 2)
        );
        assert_eq!(
            append_utf16_character_reference(little_endian, 14, 8).unwrap_err(),
            Error::new(ErrorKind::LimitExceeded(Limit::DecodedTokenBytes), 2)
        );
    }
}

#[test]
fn stale_range_wrong_mode_and_wrong_kind_are_rejected_without_cursor_change() {
    let mut local = LocalTokenizer::new(
        4,
        LocalLimits {
            max_token_bytes: 32,
            max_scratch_bytes: 32,
            max_work: 128,
        },
    );
    assert_eq!(
        local.advance_content(b"&").unwrap().stop,
        ContentStop::ReferenceStart
    );
    let stale_cursor = local.reference_cursor().unwrap();
    let mut wrong_start = ReferenceScratch::new(5, reference_limits(3));
    wrong_start.advance(b"&x;").unwrap();
    assert_eq!(
        stale_cursor
            .finish(wrong_start.validated().unwrap())
            .unwrap_err(),
        ReferenceCursorError {
            kind: ReferenceCursorErrorKind::InvalidState,
            offset: 4,
        }
    );

    let (general_scratch, local_extent) = collect_local_reference(&mut local, "&x;");
    let general = general_scratch.validated().unwrap();
    assert_eq!(
        local
            .append_character_reference(general, &local_extent)
            .unwrap_err(),
        LocalError {
            kind: LocalErrorKind::Xml(ErrorKind::InvalidState),
            offset: 4,
        }
    );

    let mut local = LocalTokenizer::new(
        4,
        LocalLimits {
            max_token_bytes: 32,
            max_scratch_bytes: 32,
            max_work: 128,
        },
    );
    local.advance_content(b"&").unwrap();
    let (numeric_scratch, numeric_extent) = collect_local_reference(&mut local, "&#65;");
    let numeric = numeric_scratch.validated().unwrap();
    assert_eq!(
        local
            .skip_general_reference(numeric, &numeric_extent)
            .unwrap_err(),
        LocalError {
            kind: LocalErrorKind::Xml(ErrorKind::InvalidState),
            offset: 4,
        }
    );

    let source = encode_utf16("&x;", true);
    let (encoding, bom, _) = decode_at_split(&source, 1);
    let mut input = Tokenizer::new(16, 16);
    input.configure(encoding, bom);
    assert_eq!(
        input.advance_content(b"&").unwrap().stop,
        ContentStop::ReferenceStart
    );
    let mut input_cursor = input.reference_cursor().unwrap();
    let mut input_scratch = ReferenceScratch::new(0, reference_limits(3));
    for byte in b"&x;" {
        input_cursor = input_cursor
            .checked_advance(std::slice::from_ref(byte))
            .unwrap();
        input_scratch.advance(std::slice::from_ref(byte)).unwrap();
    }
    let input_reference = input_scratch.validated().unwrap();
    let input_extent = input_cursor.finish(input_reference).unwrap();
    let mut local = LocalTokenizer::new(
        0,
        LocalLimits {
            max_token_bytes: 16,
            max_scratch_bytes: 16,
            max_work: 64,
        },
    );
    local.advance_content(b"&").unwrap();
    assert_eq!(
        local
            .skip_general_reference(input_reference, &input_extent)
            .unwrap_err(),
        LocalError {
            kind: LocalErrorKind::Xml(ErrorKind::InvalidState),
            offset: 0,
        }
    );
}

#[test]
fn local_boundary_failures_are_sticky_and_successful_restart_keeps_spent_work() {
    let limits = LocalLimits {
        max_token_bytes: 32,
        max_scratch_bytes: 32,
        max_work: 128,
    };
    let mut failed = LocalTokenizer::new(0, limits);
    failed.advance_content(b"pre&").unwrap();
    let (scratch, extent) = collect_local_reference(&mut failed, "&x;");
    let error = failed
        .skip_general_reference(scratch.validated().unwrap(), &extent)
        .unwrap_err();
    assert_eq!(
        error,
        LocalError {
            kind: LocalErrorKind::Xml(ErrorKind::InvalidState),
            offset: 0,
        }
    );
    assert_eq!(failed.advance_content(b"more").unwrap_err(), error);
    assert_eq!(failed.finish_step().unwrap_err(), error);

    let mut restarted = LocalTokenizer::new(0, limits);
    restarted.advance_content(b"a").unwrap();
    restarted.finish_step().unwrap();
    assert_eq!(restarted.current().unwrap().1, b"a");
    restarted.release();
    let first = restarted.usage();
    restarted.restart(20).unwrap();
    assert_eq!(
        restarted.advance_content(b"&").unwrap().stop,
        ContentStop::ReferenceStart
    );
    let (mut scratch, extent) = collect_local_reference(&mut restarted, "&amp;");
    restarted
        .append_character_reference(scratch.validated().unwrap(), &extent)
        .unwrap();
    scratch.release();
    restarted.finish_step().unwrap();
    assert_eq!(restarted.current().unwrap().1, b"&amp;");
    assert!(restarted.usage().work > first.work);
    let completed = restarted.usage();
    restarted.release();
    restarted.finish_step().unwrap();
    restarted.release();
    assert_eq!(restarted.usage(), completed);

    let sealed = restarted.reference_cursor().unwrap_err();
    assert_eq!(sealed.kind, LocalErrorKind::Sealed);
    assert_eq!(restarted.advance_content(b"x").unwrap_err(), sealed);
}

#[test]
fn cursor_limits_and_overflow_fail_before_reference_copy_or_skip() {
    for (raw_limit, decoded_limit, expected) in [
        (5, 4, ReferenceCursorErrorKind::Limit(Limit::TokenBytes)),
        (
            6,
            3,
            ReferenceCursorErrorKind::Limit(Limit::DecodedTokenBytes),
        ),
    ] {
        let source = encode_utf16("&π;", false);
        let (encoding, bom, chunks) = decode_at_split(&source, 3);
        assert_eq!(chunks.concat(), "&π;".as_bytes());
        let mut tokenizer = Tokenizer::new(raw_limit, decoded_limit);
        tokenizer.configure(encoding, bom);
        assert_eq!(
            tokenizer.advance_content(b"&").unwrap().stop,
            ContentStop::ReferenceStart
        );
        let mut cursor = tokenizer.reference_cursor().unwrap();
        let mut scratch = ReferenceScratch::new(0, reference_limits(4));
        let mut failure = None;
        for byte in "&π;".as_bytes() {
            match cursor.checked_advance(std::slice::from_ref(byte)) {
                Ok(next) => {
                    scratch.advance(std::slice::from_ref(byte)).unwrap();
                    cursor = next;
                }
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        assert_eq!(
            failure.unwrap(),
            ReferenceCursorError {
                kind: expected,
                offset: 2,
            }
        );
        assert!(scratch.validated().is_none());
    }

    let mut tokenizer = LocalTokenizer::new(
        usize::MAX - 1,
        LocalLimits {
            max_token_bytes: 8,
            max_scratch_bytes: 8,
            max_work: 8,
        },
    );
    assert_eq!(
        tokenizer.advance_content(b"&").unwrap().stop,
        ContentStop::ReferenceStart
    );
    assert_eq!(
        tokenizer
            .reference_cursor()
            .unwrap()
            .checked_advance(b"&&")
            .unwrap_err(),
        ReferenceCursorError {
            kind: ReferenceCursorErrorKind::PositionOverflow,
            offset: usize::MAX - 1,
        }
    );
}
