use super::*;
use crate::Limit;
fn limits() -> LocalLimits {
    LocalLimits {
        max_token_bytes: 128,
        max_scratch_bytes: 128,
        max_work: 4096,
    }
}
#[test]
fn local_tokens_have_exact_ranges_and_replay_markup_once() {
    let mut scanner = LocalTokenizer::new(50, limits());
    assert_eq!(scanner.advance("é<r/>".as_bytes()).unwrap(), 3);
    let (kind, raw, range) = scanner.current().unwrap();
    assert_eq!(kind, TokenKind::Text);
    assert_eq!(raw, "é".as_bytes());
    assert_eq!(range, 50..52);
    assert_eq!(scanner.position(), 53);
    assert_eq!(scanner.usage().scanned, 3);
    assert_eq!(scanner.advance(b"r/>").unwrap(), 0);
    scanner.release();
    assert_eq!(scanner.advance(&[]).unwrap(), 0);
    assert_eq!(scanner.usage().scanned, 3);
    assert_eq!(scanner.advance(b"r/>").unwrap(), 3);
    let (kind, raw, range) = scanner.current().unwrap();
    assert_eq!(kind, TokenKind::Start);
    assert_eq!(raw, b"<r/>");
    assert_eq!(range, 52..56);
    scanner.release();
    scanner.finish_step().unwrap();
    assert!(scanner.is_complete());
    assert_eq!(
        scanner.usage(),
        LocalUsage {
            scanned: 6,
            validated: 6,
            reference_copied: 0,
            work: 12
        }
    );
    scanner.restart(100).unwrap();
    assert_eq!(scanner.position(), 100);
    assert_eq!(scanner.usage().work, 12);
}
#[test]
fn local_eof_limits_invalid_utf8_and_seal_are_sticky() {
    for (input, kind) in [
        (&b"<!--"[..], LocalErrorKind::Xml(ErrorKind::UnexpectedEof)),
        (&b"\xff"[..], LocalErrorKind::Xml(ErrorKind::InvalidUtf8)),
    ] {
        let mut s = LocalTokenizer::new(7, limits());
        s.advance(input).unwrap();
        let error = s.finish_step().unwrap_err();
        assert_eq!(error.kind, kind);
        assert_eq!(error.offset, 7);
        assert_eq!(s.advance(b"x"), Err(error));
        assert_eq!(s.restart(0), Err(error));
    }
    let mut s = LocalTokenizer::new(
        0,
        LocalLimits {
            max_work: 3,
            ..limits()
        },
    );
    s.advance(b"abc").unwrap();
    assert_eq!(s.finish_step().unwrap_err().kind, LocalErrorKind::Work);
    let mut s = LocalTokenizer::new(0, limits());
    s.finish_step().unwrap();
    assert!(s.is_complete());
    assert_eq!(s.advance(b"x").unwrap_err().kind, LocalErrorKind::Sealed);
    let mut s = LocalTokenizer::new(usize::MAX, limits());
    assert_eq!(
        s.advance(b"x").unwrap_err().kind,
        LocalErrorKind::PositionOverflow
    );
    let mut s = LocalTokenizer::new(0, limits());
    s.advance(b"<").unwrap();
    assert_eq!(s.restart(4).unwrap_err().kind, LocalErrorKind::InvalidState);
}
#[test]
fn local_token_and_scratch_limits_remain_distinct() {
    for (limits, expected) in [
        (
            LocalLimits {
                max_token_bytes: 2,
                ..limits()
            },
            Limit::TokenBytes,
        ),
        (
            LocalLimits {
                max_scratch_bytes: 2,
                ..limits()
            },
            Limit::DecodedTokenBytes,
        ),
    ] {
        let mut s = LocalTokenizer::new(10, limits);
        let error = s.advance(b"abc").unwrap_err();
        assert_eq!(
            error.kind,
            LocalErrorKind::Xml(ErrorKind::LimitExceeded(expected))
        );
        assert_eq!(error.offset, 10);
    }
}

#[test]
fn local_markup_sentinels_are_failures_and_never_content_events() {
    for raw in [&b"<!DOCTYPE r>"[..], &b"<!BAD>"[..]] {
        let mut s = LocalTokenizer::new(20, limits());
        let error = s.advance(raw).unwrap_err();
        assert_eq!(error.kind, LocalErrorKind::ForbiddenMarkup);
        assert_eq!(error.offset, 20);
        assert!(s.current().is_none());
    }
}

#[test]
fn local_work_caps_only_decrease_and_survive_restart() {
    let mut s = LocalTokenizer::new(0, limits());
    s.advance(b"a").unwrap();
    s.finish_step().unwrap();
    s.release();
    assert!(s.is_complete());
    assert_eq!(s.usage().work, 2);
    s.cap_work(3);
    s.cap_work(usize::MAX);
    s.restart(10).unwrap();
    s.advance(b"a").unwrap();
    assert_eq!(s.usage().work, 3);
    assert_eq!(s.finish_step().unwrap_err().kind, LocalErrorKind::Work);
    let mut s = LocalTokenizer::new(0, limits());
    s.advance(b"a").unwrap();
    s.cap_work(0);
    assert_eq!(s.advance(b"b").unwrap_err().kind, LocalErrorKind::Work);
}

#[test]
fn local_complete_units_are_invariant_at_every_utf8_byte_split() {
    let text = "pre&#60;\r\n<根 a='>'><!--é\r--><![CDATA[🦀\r]]><?p a\r?><子/></根>tail";
    let expected = [
        (TokenKind::Text, "pre&#60;\r\n"),
        (TokenKind::Start, "<根 a='>'>"),
        (TokenKind::Comment, "<!--é\r-->"),
        (TokenKind::Cdata, "<![CDATA[🦀\r]]>"),
        (TokenKind::Instruction, "<?p a\r?>"),
        (TokenKind::Start, "<子/>"),
        (TokenKind::End, "</根>"),
        (TokenKind::Text, "tail"),
    ];
    for split in 0..=text.len() {
        let mut s = LocalTokenizer::new(19, limits());
        let mut cursor = 0;
        let mut found = Vec::new();
        for end in [split, text.len()] {
            while cursor < end {
                let consumed = s.advance(&text.as_bytes()[cursor..end]).unwrap();
                cursor += consumed;
                if let Some((kind, bytes, range)) = s.current() {
                    assert_eq!(&text.as_bytes()[range.start - 19..range.end - 19], bytes);
                    found.push((kind, std::str::from_utf8(bytes).unwrap().to_owned()));
                    s.release()
                } else {
                    assert!(consumed > 0)
                }
            }
        }
        while !s.is_complete() {
            s.finish_step().unwrap();
            if let Some((kind, bytes, _)) = s.current() {
                found.push((kind, std::str::from_utf8(bytes).unwrap().to_owned()));
                s.release()
            }
        }
        assert_eq!(
            found
                .iter()
                .map(|(kind, raw)| (*kind, raw.as_str()))
                .collect::<Vec<_>>(),
            expected,
            "split {split}"
        );
        assert_eq!(s.usage().scanned, text.len());
        assert_eq!(s.usage().validated, text.len());
        assert_eq!(s.usage().work, text.len() * 2);
    }
}
