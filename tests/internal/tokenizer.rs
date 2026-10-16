use super::*;

impl Tokenizer {
    pub(crate) fn retained_snapshot(&self) -> (usize, usize) {
        (self.buffer.len(), self.buffer.capacity())
    }
}

#[test]
fn suspension_retains_completed_tokens_and_replays_lookahead_once() {
    let mut tokenizer = Tokenizer::new(3, 3);
    assert_eq!(tokenizer.advance(b"abc<r/>").unwrap(), 4);
    let (kind, bytes, span) = tokenizer.current().unwrap();
    assert_eq!(kind, TokenKind::Text);
    assert_eq!(bytes, b"abc");
    assert_eq!(span.source, 0..3);
    assert_eq!(tokenizer.advance(b"r/>").unwrap(), 0);
    tokenizer.release();
    assert_eq!(tokenizer.advance(&[]).unwrap(), 0);
    assert_eq!(tokenizer.source_position(), 4);
    assert_eq!(tokenizer.buffer, b"<");
    assert_eq!(
        tokenizer.advance(b"r/>").unwrap_err(),
        Error::new(ErrorKind::LimitExceeded(Limit::TokenBytes), 3)
    );
    assert_eq!(tokenizer.source_position(), 7);
}

#[test]
fn tokens_and_spans_are_chunk_invariant() {
    let input = b"<?pi x?><r a='>'>text<!--c--><![CDATA[x]]><c/></r>";
    let expected = [
        (TokenKind::Instruction, &b"<?pi x?>"[..]),
        (TokenKind::Start, &b"<r a='>'>"[..]),
        (TokenKind::Text, &b"text"[..]),
        (TokenKind::Comment, &b"<!--c-->"[..]),
        (TokenKind::Cdata, &b"<![CDATA[x]]>"[..]),
        (TokenKind::Start, &b"<c/>"[..]),
        (TokenKind::End, &b"</r>"[..]),
    ];
    for size in 1..=input.len() {
        let mut tokenizer = Tokenizer::new(1024, 1024);
        let mut found = Vec::new();
        let mut emit = |kind, raw: &[u8], span: TokenSpan| {
            assert_eq!(&input[span.source], raw);
            found.push((kind, raw.to_vec()));
            Ok(())
        };
        for chunk in input.chunks(size) {
            tokenizer.feed(chunk, &mut emit).unwrap();
        }
        tokenizer.finish(emit).unwrap();
        assert_eq!(
            found
                .iter()
                .map(|(k, v)| (*k, v.as_slice()))
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(tokenizer.source_position(), input.len());
    }
}

#[test]
fn partial_delimiters_report_eof() {
    for input in [b"<".as_slice(), b"<?", b"<!--", b"<![CDATA[", b"<r a='"] {
        let mut tokenizer = Tokenizer::new(1024, 1024);
        tokenizer.feed(input, |_, _, _| Ok(())).unwrap();
        assert_eq!(
            tokenizer.finish(|_, _, _| Ok(())).unwrap_err().kind,
            ErrorKind::UnexpectedEof
        );
    }
}

#[test]
fn utf16_token_spans_and_both_token_ceilings_exclude_lookahead() {
    for little_endian in [false, true] {
        let encoding = SourceEncoding::Utf16 {
            little_endian,
            bom: true,
        };
        let mut tokenizer = Tokenizer::new(6, 6);
        tokenizer.configure(encoding, 2);
        let mut found = Vec::new();
        for scalar in ["é", "🙂", "<"] {
            tokenizer
                .feed(scalar.as_bytes(), |kind, bytes, span| {
                    found.push((kind, bytes.to_vec(), span.decoded, span.source));
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(
            found,
            [(TokenKind::Text, "é🙂".as_bytes().to_vec(), 0..6, 2..8)]
        );
        assert_eq!(tokenizer.source_position(), 10);
        assert_eq!(
            tokenizer.finish(|_, _, _| Ok(())).unwrap_err(),
            Error::new(ErrorKind::UnexpectedEof, 8)
        );
        for (raw, decoded, expected) in
            [(5, 6, Limit::TokenBytes), (6, 5, Limit::DecodedTokenBytes)]
        {
            let mut tokenizer = Tokenizer::new(raw, decoded);
            tokenizer.configure(encoding, 2);
            tokenizer.feed("é".as_bytes(), |_, _, _| Ok(())).unwrap();
            let error = tokenizer
                .feed("🙂".as_bytes(), |_, _, _| {
                    panic!("unfinished text must not be emitted")
                })
                .unwrap_err();
            assert_eq!(error, Error::new(ErrorKind::LimitExceeded(expected), 2));
            assert!(tokenizer.buffer.len() <= decoded);
            assert!(tokenizer.buffer.capacity() <= decoded);
        }
    }
}

#[test]
fn token_limit_and_capacity_do_not_depend_on_feed_size() {
    for limit in [0, 1, 3, 4, 63, 64, 65, 1024] {
        let mut tokenizer = Tokenizer::new(limit, limit);
        let input = vec![b'x'; limit];
        for byte in &input {
            tokenizer
                .feed(std::slice::from_ref(byte), |_, _, _| Ok(()))
                .unwrap();
        }
        assert_eq!(tokenizer.buffer.len(), limit);
        assert!(tokenizer.buffer.capacity() <= limit);
        assert_eq!(
            tokenizer.feed(b"x", |_, _, _| Ok(())).unwrap_err().kind,
            ErrorKind::LimitExceeded(Limit::TokenBytes)
        );
    }
}

#[test]
fn scratch_capacity_obeys_small_source_and_decoded_quotas() {
    for (raw, decoded) in [(1, 1024), (3, 1024), (1024, 1), (1024, 3)] {
        let mut tokenizer = Tokenizer::new(raw, decoded);
        tokenizer.feed(b"x", |_, _, _| Ok(())).unwrap();
        assert!(tokenizer.buffer.capacity() <= raw.min(decoded));
    }
    for raw in [2, 3, 4, 5, 8] {
        let mut tokenizer = Tokenizer::new(raw, 1024);
        tokenizer.configure(
            SourceEncoding::Utf16 {
                little_endian: true,
                bom: true,
            },
            2,
        );
        tokenizer.feed("中".as_bytes(), |_, _, _| Ok(())).unwrap();
        assert!(tokenizer.buffer.capacity() <= (raw / 2) * 3);
    }
}

#[test]
fn callback_errors_stop_at_the_triggering_token() {
    let mut tokenizer = Tokenizer::new(1024, 1024);
    let expected = Error::new(ErrorKind::Syntax, 0);
    let mut calls = 0;
    assert_eq!(
        tokenizer.feed(b"<a/><b/>", |_, _, _| {
            calls += 1;
            Err(expected)
        }),
        Err(expected)
    );
    assert_eq!(calls, 1);
    assert_eq!(tokenizer.source_position(), 4);
}

#[test]
fn long_runs_and_delimiter_prefixes_match_scalar_chunks() {
    let body = "x> y<&".repeat(10_000);
    let parts = [
        (TokenKind::Start, format!("<r a='{body}'>")),
        (TokenKind::Text, "plain ".repeat(10_000)),
        (TokenKind::Comment, format!("<!--{body}--->")),
        (TokenKind::Cdata, format!("<![CDATA[{body}]]]>")),
        (TokenKind::Instruction, format!("<?pi {body}??>")),
        (TokenKind::End, "</r>".to_owned()),
    ];
    let input = parts
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<String>();
    for size in [1, 7, 8192, input.len()] {
        let mut tokenizer = Tokenizer::new(128 * 1024, 128 * 1024);
        let mut found = Vec::new();
        let mut emit = |kind, raw: &[u8], span: TokenSpan| {
            assert_eq!(raw, &input.as_bytes()[span.source]);
            found.push((kind, raw.to_vec()));
            Ok(())
        };
        for chunk in input.as_bytes().chunks(size) {
            tokenizer.feed(chunk, &mut emit).unwrap();
        }
        tokenizer.finish(emit).unwrap();
        assert_eq!(
            found
                .iter()
                .map(|(kind, bytes)| (*kind, bytes.as_slice()))
                .collect::<Vec<_>>(),
            parts
                .iter()
                .map(|(kind, text)| (*kind, text.as_bytes()))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn oversized_bulk_runs_fail_at_the_first_offending_byte() {
    for first in [b"".as_slice(), b"x", b"xx", b"xxx"] {
        let mut tokenizer = Tokenizer::new(3, 3);
        tokenizer.feed(first, |_, _, _| Ok(())).unwrap();
        let error = tokenizer.feed(b"xxxxxxxx", |_, _, _| Ok(())).unwrap_err();
        assert_eq!(
            error,
            Error::new(ErrorKind::LimitExceeded(Limit::TokenBytes), 0)
        );
        assert_eq!(tokenizer.source_position(), 4);
        assert!(tokenizer.buffer.capacity() <= 3);
    }
}
