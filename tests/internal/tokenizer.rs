use super::*;

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
        let mut tokenizer = Tokenizer::new(1024);
        let mut found = Vec::new();
        let mut emit = |kind, raw: &[u8], range: Range<usize>| {
            assert_eq!(&input[range], raw);
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
        assert_eq!(tokenizer.position(), input.len());
    }
}

#[test]
fn encoding_preambles_and_partial_prefixes() {
    for (input, expected) in [
        (&b"\xff\xfe"[..], Some(ErrorKind::UnsupportedEncoding)),
        (&b"\xfe\xff"[..], Some(ErrorKind::UnsupportedEncoding)),
        (&b"\0\0\xfe\xff"[..], Some(ErrorKind::UnsupportedEncoding)),
        (&b"\0<\0?"[..], Some(ErrorKind::UnsupportedEncoding)),
        (&b"\xef\xbb\xbf<r/>"[..], None),
        (&b"\xef\xbb"[..], None),
    ] {
        let mut tokenizer = Tokenizer::new(1024);
        let result = input
            .iter()
            .try_for_each(|byte| tokenizer.feed(std::slice::from_ref(byte), |_, _, _| Ok(())))
            .and_then(|()| tokenizer.finish(|_, _, _| Ok(())));
        assert_eq!(result.err().map(|e| e.kind), expected);
    }
    for input in [b"<".as_slice(), b"<?", b"<!--", b"<![CDATA[", b"<r a='"] {
        let mut tokenizer = Tokenizer::new(1024);
        tokenizer.feed(input, |_, _, _| Ok(())).unwrap();
        assert_eq!(
            tokenizer.finish(|_, _, _| Ok(())).unwrap_err().kind,
            ErrorKind::UnexpectedEof
        );
    }
}

#[test]
fn token_limit_and_capacity_do_not_depend_on_feed_size() {
    for limit in [0, 1, 3, 4, 63, 64, 65, 1024] {
        let mut tokenizer = Tokenizer::new(limit);
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
fn callback_errors_stop_at_the_triggering_token() {
    let mut tokenizer = Tokenizer::new(1024);
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
    assert_eq!(tokenizer.position(), 4);
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
        let mut tokenizer = Tokenizer::new(128 * 1024);
        let mut found = Vec::new();
        let mut emit = |kind, raw: &[u8], range: Range<usize>| {
            assert_eq!(raw, &input.as_bytes()[range]);
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
        let mut tokenizer = Tokenizer::new(3);
        tokenizer.feed(first, |_, _, _| Ok(())).unwrap();
        let error = tokenizer.feed(b"xxxxxxxx", |_, _, _| Ok(())).unwrap_err();
        assert_eq!(
            error,
            Error::new(ErrorKind::LimitExceeded(Limit::TokenBytes), 0)
        );
        assert_eq!(tokenizer.position(), 4);
        assert!(tokenizer.buffer.capacity() <= 3);
    }
}
