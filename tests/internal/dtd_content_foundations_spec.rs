use crate::dtd_reference::{
    ReferenceError, ReferenceErrorKind, ReferenceKind, ReferenceLimit, ReferenceLimits,
    ReferenceScratch, ReferenceUsage,
};
use crate::tokenizer::TokenKind;
use crate::tokenizer::local::{
    LocalError, LocalErrorKind, LocalLimits, LocalTokenizer, LocalUsage,
};
use crate::{ErrorKind, Limit};
use std::ops::Range;

#[derive(Debug, Eq, PartialEq)]
struct LocalToken {
    kind: TokenKind,
    raw: Vec<u8>,
    range: Range<usize>,
}

fn generous_local_limits(input: &[u8]) -> LocalLimits {
    LocalLimits {
        max_token_bytes: input.len().max(1),
        max_scratch_bytes: input.len().max(1),
        max_work: input.len().saturating_mul(2),
    }
}

fn collect_ready(scanner: &mut LocalTokenizer, tokens: &mut Vec<LocalToken>) -> bool {
    let Some((kind, raw, range)) = scanner.current() else {
        return false;
    };
    tokens.push(LocalToken {
        kind,
        raw: raw.to_vec(),
        range,
    });
    scanner.release();
    true
}

fn scan_local_at_split(input: &[u8], start: usize, split: usize) -> (Vec<LocalToken>, LocalUsage) {
    let mut scanner = LocalTokenizer::new(start, generous_local_limits(input));
    let mut tokens = Vec::new();
    for chunk in [&input[..split], &input[split..]] {
        let mut cursor = 0;
        while cursor < chunk.len() {
            let consumed = scanner.advance(&chunk[cursor..]).unwrap();
            cursor += consumed;
            let ready = collect_ready(&mut scanner, &mut tokens);
            assert!(consumed > 0 || ready, "local scanner made no progress");
        }
    }
    while !scanner.is_complete() {
        scanner.finish_step().unwrap();
        if !collect_ready(&mut scanner, &mut tokens) {
            assert!(scanner.is_complete(), "EOF made no progress");
        }
    }
    (tokens, scanner.usage())
}

fn generous_reference_limits(raw: &[u8]) -> ReferenceLimits {
    ReferenceLimits {
        max_bytes: raw.len().max(1),
        max_scratch_bytes: raw.len().max(1),
        max_work: raw.len().saturating_mul(3),
    }
}

fn parse_reference_at_split(
    raw: &[u8],
    start: usize,
    split: usize,
) -> (ReferenceKind, String, Range<usize>, ReferenceUsage) {
    let mut scratch = ReferenceScratch::new(start, generous_reference_limits(raw));
    for chunk in [&raw[..split], &raw[split..]] {
        let mut cursor = 0;
        while cursor < chunk.len() {
            let consumed = scratch.advance(&chunk[cursor..]).unwrap();
            assert!(consumed > 0, "incomplete reference made no progress");
            cursor += consumed;
        }
    }
    let (kind, spelling, range) = scratch.current().unwrap();
    (kind, spelling.to_owned(), range, scratch.usage())
}

#[test]
fn local_tokens_preserve_every_content_kind_range_and_literal_bytes_at_all_splits() {
    let expected = [
        (TokenKind::Text, "π\r"),
        (TokenKind::Start, "<r a='>'>"),
        (TokenKind::Text, "t\r"),
        (TokenKind::Cdata, "<![CDATA[c\r]]>"),
        (TokenKind::Comment, "<!--m\r-->"),
        (TokenKind::Instruction, "<?p d\r?>"),
        (TokenKind::End, "</r>"),
    ];
    let input = expected.iter().map(|(_, raw)| *raw).collect::<String>();
    let start = 17;
    for split in 0..=input.len() {
        let (actual, usage) = scan_local_at_split(input.as_bytes(), start, split);
        assert_eq!(actual.len(), expected.len(), "split {split}");
        let mut cursor = start;
        for (found, (kind, raw)) in actual.iter().zip(expected) {
            assert_eq!(found.kind, kind, "split {split}");
            assert_eq!(found.raw, raw.as_bytes(), "split {split}");
            assert_eq!(found.range, cursor..cursor + raw.len(), "split {split}");
            cursor += raw.len();
        }
        assert_eq!(cursor, start + input.len());
        let joined = actual
            .iter()
            .flat_map(|token| token.raw.iter().copied())
            .collect::<Vec<_>>();
        assert_eq!(joined, input.as_bytes());
        assert_eq!(
            usage,
            LocalUsage {
                scanned: input.len(),
                validated: input.len(),
                work: input.len() * 2,
            },
            "split {split}",
        );
    }
}

#[test]
fn local_empty_start_and_xml_declaration_remain_lexical_units() {
    let input = b"<?xml version='1.0'?><e/></e>";
    let (tokens, usage) = scan_local_at_split(input, 0, 7);
    assert_eq!(
        tokens,
        [
            LocalToken {
                kind: TokenKind::Instruction,
                raw: b"<?xml version='1.0'?>".to_vec(),
                range: 0..21,
            },
            LocalToken {
                kind: TokenKind::Start,
                raw: b"<e/>".to_vec(),
                range: 21..25,
            },
            LocalToken {
                kind: TokenKind::End,
                raw: b"</e>".to_vec(),
                range: 25..29,
            },
        ]
    );
    assert_eq!(usage.work, input.len() * 2);
}

#[test]
fn local_finish_release_and_restart_preserve_aggregate_work_and_monotone_caps() {
    let limits = LocalLimits {
        max_token_bytes: 8,
        max_scratch_bytes: 8,
        max_work: 8,
    };
    let mut scanner = LocalTokenizer::new(4, limits);
    assert_eq!(scanner.advance(b"a").unwrap(), 1);
    scanner.finish_step().unwrap();
    assert_eq!(scanner.current().unwrap().2, 4..5);
    assert_eq!(scanner.usage().work, 2);
    scanner.release();
    assert!(scanner.is_complete());
    let completed = scanner.usage();
    scanner.finish_step().unwrap();
    scanner.release();
    assert_eq!(scanner.usage(), completed);

    scanner.restart(10).unwrap();
    scanner.cap_work(3);
    scanner.cap_work(8);
    assert_eq!(scanner.advance(b"b").unwrap(), 1);
    let error = scanner.finish_step().unwrap_err();
    assert_eq!(
        error,
        LocalError {
            kind: LocalErrorKind::Work,
            offset: 10
        }
    );
    assert_eq!(
        scanner.usage(),
        LocalUsage {
            scanned: 2,
            validated: 1,
            work: 3
        }
    );
    assert_eq!(scanner.finish_step().unwrap_err(), error);
}

#[test]
fn local_partial_forbidden_utf8_and_xml_character_errors_are_exact_and_sticky() {
    for (raw, expected, position) in [
        (
            &b"<!DOCTYPE r>tail"[..],
            LocalError {
                kind: LocalErrorKind::ForbiddenMarkup,
                offset: 30,
            },
            39,
        ),
        (
            &b"<!BOGUS tail"[..],
            LocalError {
                kind: LocalErrorKind::ForbiddenMarkup,
                offset: 30,
            },
            33,
        ),
        (
            &b"a\xC3<"[..],
            LocalError {
                kind: LocalErrorKind::Xml(ErrorKind::InvalidUtf8),
                offset: 31,
            },
            33,
        ),
        (
            &b"a\0<"[..],
            LocalError {
                kind: LocalErrorKind::Xml(ErrorKind::InvalidCharacter),
                offset: 31,
            },
            33,
        ),
    ] {
        let mut scanner = LocalTokenizer::new(30, generous_local_limits(raw));
        let error = scanner.advance(raw).unwrap_err();
        assert_eq!(error, expected);
        assert_eq!(scanner.position(), position);
        assert_eq!(scanner.current(), None);
        assert_eq!(scanner.advance(b"more").unwrap_err(), expected);
        assert_eq!(scanner.finish_step().unwrap_err(), expected);
    }

    for raw in [b"<".as_slice(), b"<r", b"<!--x", b"<![CDATA[x", b"<?p"] {
        let mut scanner = LocalTokenizer::new(7, generous_local_limits(raw));
        assert_eq!(scanner.advance(raw).unwrap(), raw.len());
        let error = LocalError {
            kind: LocalErrorKind::Xml(ErrorKind::UnexpectedEof),
            offset: 7,
        };
        assert_eq!(scanner.finish_step().unwrap_err(), error, "{raw:?}");
        assert_eq!(scanner.finish_step().unwrap_err(), error, "{raw:?}");
    }
}

#[test]
fn local_token_scratch_work_and_position_limits_have_exact_boundaries() {
    let raw = b"<a/>";
    for (limits, expected) in [
        (
            LocalLimits {
                max_token_bytes: 4,
                max_scratch_bytes: 4,
                max_work: 8,
            },
            None,
        ),
        (
            LocalLimits {
                max_token_bytes: 3,
                max_scratch_bytes: 4,
                max_work: 8,
            },
            Some(LocalError {
                kind: LocalErrorKind::Xml(ErrorKind::LimitExceeded(Limit::TokenBytes)),
                offset: 11,
            }),
        ),
        (
            LocalLimits {
                max_token_bytes: 4,
                max_scratch_bytes: 3,
                max_work: 8,
            },
            Some(LocalError {
                kind: LocalErrorKind::Xml(ErrorKind::LimitExceeded(Limit::DecodedTokenBytes)),
                offset: 11,
            }),
        ),
        (
            LocalLimits {
                max_token_bytes: 4,
                max_scratch_bytes: 4,
                max_work: 7,
            },
            Some(LocalError {
                kind: LocalErrorKind::Work,
                offset: 11,
            }),
        ),
    ] {
        let mut scanner = LocalTokenizer::new(11, limits);
        let result = scanner.advance(raw);
        match expected {
            None => {
                assert_eq!(result.unwrap(), raw.len());
                assert_eq!(scanner.current().unwrap().1, raw);
                assert_eq!(scanner.usage().work, 8);
            }
            Some(error) => assert_eq!(result.unwrap_err(), error),
        }
    }

    let mut overflow = LocalTokenizer::new(
        usize::MAX,
        LocalLimits {
            max_token_bytes: 1,
            max_scratch_bytes: 1,
            max_work: 2,
        },
    );
    assert_eq!(
        overflow.advance(b"x").unwrap_err(),
        LocalError {
            kind: LocalErrorKind::PositionOverflow,
            offset: usize::MAX
        },
    );
}

#[test]
fn references_classify_literal_spellings_without_lookup_at_all_splits() {
    let cases = [
        ("&#65;", ReferenceKind::Numeric('A')),
        ("&#00065;", ReferenceKind::Numeric('A')),
        ("&#x41;", ReferenceKind::Numeric('A')),
        ("&#x0001D11E;", ReferenceKind::Numeric('𝄞')),
        ("&lt;", ReferenceKind::Predefined('<')),
        ("&gt;", ReferenceKind::Predefined('>')),
        ("&amp;", ReferenceKind::Predefined('&')),
        ("&apos;", ReferenceKind::Predefined('\'')),
        ("&quot;", ReferenceKind::Predefined('"')),
        ("&unknown;", ReferenceKind::General),
        ("&π:δ;", ReferenceKind::General),
        ("&:local;", ReferenceKind::General),
    ];
    for (raw, expected) in cases {
        let start = 23;
        for split in 0..=raw.len() {
            let (kind, spelling, range, usage) =
                parse_reference_at_split(raw.as_bytes(), start, split);
            assert_eq!(kind, expected, "{raw:?} split {split}");
            assert_eq!(spelling, raw, "{raw:?} split {split}");
            assert_eq!(range, start..start + raw.len());
            assert_eq!(
                usage,
                ReferenceUsage {
                    scanned: raw.len(),
                    copied: raw.len(),
                    classified: raw.len(),
                    work: raw.len() * 3,
                },
                "{raw:?} split {split}",
            );
        }
    }
}

#[test]
fn complete_reference_stops_before_suffix_and_requires_release_before_restart() {
    let raw = b"&amp;suffix";
    let mut scratch = ReferenceScratch::new(5, generous_reference_limits(b"&amp;"));
    assert_eq!(scratch.advance(raw).unwrap(), 5);
    assert_eq!(
        scratch.current(),
        Some((ReferenceKind::Predefined('&'), "&amp;", 5..10)),
    );
    assert_eq!(scratch.position(), 10);
    assert_eq!(scratch.advance(b"suffix").unwrap(), 0);
    assert_eq!(
        scratch.restart(30).unwrap_err(),
        ReferenceError {
            kind: ReferenceErrorKind::InvalidState,
            offset: 10
        },
    );

    // Use a fresh scanner because InvalidState is deliberately sticky.
    let mut reusable = ReferenceScratch::new(
        5,
        ReferenceLimits {
            max_bytes: 5,
            max_scratch_bytes: 5,
            max_work: 24,
        },
    );
    assert_eq!(reusable.advance(b"&amp;").unwrap(), 5);
    reusable.finish_step().unwrap();
    let completed = reusable.usage();
    reusable.release();
    reusable.release();
    assert_eq!(reusable.usage(), completed);
    reusable.restart(30).unwrap();
    assert_eq!(reusable.usage(), completed);
    assert_eq!(reusable.advance(b"&x;").unwrap(), 3);
    assert_eq!(reusable.current().unwrap().2, 30..33);
    assert_eq!(reusable.usage().work, completed.work + 9);
}

#[test]
fn reference_syntax_utf8_xml_and_numeric_errors_have_literal_offsets() {
    let cases: &[(&[u8], ReferenceErrorKind, usize)] = &[
        (b"x", ReferenceErrorKind::Syntax, 40),
        (b"&;", ReferenceErrorKind::Syntax, 41),
        (b"&#;", ReferenceErrorKind::Syntax, 42),
        (b"&#x;", ReferenceErrorKind::Syntax, 43),
        (b"&#1x;", ReferenceErrorKind::Syntax, 43),
        (b"&#x1g;", ReferenceErrorKind::Syntax, 44),
        (b"&1a;", ReferenceErrorKind::Syntax, 41),
        (b"&#0;", ReferenceErrorKind::InvalidCharacter, 40),
        (b"&#x110000;", ReferenceErrorKind::InvalidCharacter, 40),
        (b"&\xC3;", ReferenceErrorKind::InvalidUtf8, 41),
        (b"&\x01;", ReferenceErrorKind::InvalidCharacter, 41),
    ];
    for &(raw, kind, offset) in cases {
        let mut scratch = ReferenceScratch::new(40, generous_reference_limits(raw));
        let error = scratch.advance(raw).unwrap_err();
        assert_eq!(error, ReferenceError { kind, offset }, "{raw:?}");
        assert_eq!(scratch.current(), None);
        assert_eq!(scratch.advance(b"&ok;").unwrap_err(), error);
        assert_eq!(scratch.finish_step().unwrap_err(), error);
    }
}

#[test]
fn incomplete_references_classify_once_before_reporting_eof() {
    let cases: &[(&[u8], ReferenceErrorKind, usize)] = &[
        (b"", ReferenceErrorKind::UnexpectedEof, 70),
        (b"&", ReferenceErrorKind::UnexpectedEof, 71),
        (b"&name", ReferenceErrorKind::UnexpectedEof, 75),
        (b"&#12", ReferenceErrorKind::UnexpectedEof, 74),
        (b"&#xg", ReferenceErrorKind::Syntax, 73),
        (b"&1", ReferenceErrorKind::Syntax, 71),
        (b"&\xC3", ReferenceErrorKind::InvalidUtf8, 71),
    ];
    for &(raw, kind, offset) in cases {
        let mut scratch = ReferenceScratch::new(70, generous_reference_limits(raw));
        assert_eq!(scratch.advance(raw).unwrap(), raw.len());
        let error = scratch.finish_step().unwrap_err();
        assert_eq!(error, ReferenceError { kind, offset }, "{raw:?}");
        let usage = scratch.usage();
        assert_eq!(usage.scanned, raw.len());
        assert_eq!(usage.copied, raw.len());
        assert_eq!(usage.classified, raw.len());
        assert_eq!(usage.work, raw.len() * 3);
        assert_eq!(scratch.finish_step().unwrap_err(), error);
        assert_eq!(scratch.usage(), usage);
    }
}

#[test]
fn reference_limits_cover_append_and_final_classification_boundaries() {
    let raw = b"&name;";
    for (limits, expected) in [
        (
            ReferenceLimits {
                max_bytes: 6,
                max_scratch_bytes: 6,
                max_work: 18,
            },
            None,
        ),
        (
            ReferenceLimits {
                max_bytes: 5,
                max_scratch_bytes: 6,
                max_work: 18,
            },
            Some((ReferenceErrorKind::Limit(ReferenceLimit::Bytes), 45)),
        ),
        (
            ReferenceLimits {
                max_bytes: 6,
                max_scratch_bytes: 5,
                max_work: 18,
            },
            Some((ReferenceErrorKind::Limit(ReferenceLimit::ScratchBytes), 45)),
        ),
        (
            ReferenceLimits {
                max_bytes: 6,
                max_scratch_bytes: 6,
                max_work: 17,
            },
            Some((ReferenceErrorKind::Limit(ReferenceLimit::Work), 40)),
        ),
    ] {
        let mut scratch = ReferenceScratch::new(40, limits);
        let result = scratch.advance(raw);
        match expected {
            None => {
                assert_eq!(result.unwrap(), raw.len());
                assert_eq!(scratch.usage().work, 18);
            }
            Some((kind, offset)) => {
                assert_eq!(result.unwrap_err(), ReferenceError { kind, offset });
                if kind == ReferenceErrorKind::Limit(ReferenceLimit::Work) {
                    assert_eq!(
                        scratch.usage(),
                        ReferenceUsage {
                            scanned: 6,
                            copied: 6,
                            classified: 0,
                            work: 12
                        },
                    );
                }
            }
        }
    }
}

#[test]
fn reference_restart_keeps_work_cap_monotone_and_sealing_is_sticky() {
    let limits = ReferenceLimits {
        max_bytes: 4,
        max_scratch_bytes: 4,
        max_work: 18,
    };
    let mut scratch = ReferenceScratch::new(0, limits);
    assert_eq!(scratch.advance(b"&a;").unwrap(), 3);
    scratch.release();
    scratch.restart(10).unwrap();
    scratch.cap_work(17);
    scratch.cap_work(18);
    let error = scratch.advance(b"&b;").unwrap_err();
    assert_eq!(
        error,
        ReferenceError {
            kind: ReferenceErrorKind::Limit(ReferenceLimit::Work),
            offset: 10,
        },
    );
    assert_eq!(scratch.usage().work, 15);
    assert_eq!(scratch.advance(b"&c;").unwrap_err(), error);

    let mut sealed = ReferenceScratch::new(0, limits);
    assert_eq!(sealed.advance(b"&a;").unwrap(), 3);
    sealed.finish_step().unwrap();
    sealed.release();
    let sealed_error = ReferenceError {
        kind: ReferenceErrorKind::Sealed,
        offset: 3,
    };
    assert_eq!(sealed.advance(b"&b;").unwrap_err(), sealed_error);
    assert_eq!(sealed.advance(b"").unwrap_err(), sealed_error);
}

#[test]
fn reference_position_overflow_precedes_collection_work() {
    let mut scratch = ReferenceScratch::new(
        usize::MAX,
        ReferenceLimits {
            max_bytes: 1,
            max_scratch_bytes: 1,
            max_work: 3,
        },
    );
    assert_eq!(
        scratch.advance(b"&").unwrap_err(),
        ReferenceError {
            kind: ReferenceErrorKind::PositionOverflow,
            offset: usize::MAX
        },
    );
    assert_eq!(scratch.usage(), ReferenceUsage::default());
}
