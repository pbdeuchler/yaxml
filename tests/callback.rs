use std::ops::ControlFlow;
use std::panic::{AssertUnwindSafe, catch_unwind};

use yaxml::{ErrorKind, PushParser};

#[test]
fn recovered_callback_panics_poison_the_parser() {
    for during_finish in [false, true] {
        let mut parser = PushParser::new();
        if during_finish {
            parser
                .feed(b"<r>text", |_, _| ControlFlow::Continue(()))
                .unwrap();
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            if during_finish {
                parser.finish(|_, _| panic!("injected callback panic"))
            } else {
                parser.feed(b"<r/>", |_, _| panic!("injected callback panic"))
            }
        }));
        assert!(result.is_err());
        let error = parser
            .feed(b"", |_, _| panic!("callback after panic"))
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidState);
        assert_eq!(
            parser
                .finish(|_, _| panic!("callback after panic"))
                .unwrap_err(),
            error
        );
    }
}

#[test]
fn successful_feeds_clear_the_in_flight_panic_guard() {
    let mut parser = PushParser::new();
    for chunk in [b"<r>".as_slice(), b"text", b"</r>"] {
        parser
            .feed(chunk, |_, _| ControlFlow::Continue(()))
            .unwrap();
    }
    parser.finish(|_, _| ControlFlow::Continue(())).unwrap();
    parser.finish(|_, _| panic!("events after finish")).unwrap();
}

/// Whole-input feeds take complete tokens directly from the chunk; one-byte
/// feeds always buffer. Both must agree on events, origins, errors and
/// positions, including at limits and around every delimiter.
#[test]
fn borrowed_and_buffered_tokens_are_indistinguishable() {
    use yaxml::Limits;
    fn transcript(input: &[u8], chunk: usize, limits: Limits) -> Vec<String> {
        let mut parser = PushParser::with_limits(limits);
        let mut log = Vec::new();
        let mut record = |event: &yaxml::Event<'_>, origin: yaxml::SourceOrigin| {
            log.push(format!("{event:?} {origin:?}"));
            ControlFlow::Continue(())
        };
        let mut result = Ok(());
        for bytes in input.chunks(chunk) {
            result = parser.feed(bytes, &mut record);
            if result.is_err() {
                break;
            }
        }
        if result.is_ok() {
            result = parser.finish(&mut record);
        }
        log.push(format!("{result:?} at {}", parser.position()));
        log
    }
    fn lent(input: &[u8], capacity: usize, limits: Limits) -> Vec<String> {
        let source = std::io::BufReader::with_capacity(capacity, input);
        let mut reader = yaxml::BufferedReader::with_limits(source, limits);
        let mut log = Vec::new();
        loop {
            match reader.read_event() {
                Ok(Some((event, origin))) => log.push(format!("{event:?} {origin:?}")),
                result => {
                    log.push(format!("{:?} at {}", result.map(|_| ()), reader.position()));
                    return log;
                }
            }
        }
    }
    let tight = Limits {
        max_token_bytes: 12,
        ..Limits::default()
    };
    let cases: &[&[u8]] = &[
        b"<r a='>' b=\"'\">t&amp;x\r\n</r >",
        b"<?xml version='1.0'?><!--c-->\n<?p d?><r><![CDATA[x]]>y<e/><!---->z</r>",
        b"<r><!-->--></r>",
        b"<r><??></r>",
        b"<r><?>?></r>",
        b"<>",
        b"<r>]]></r>",
        b"<r>a\x01b</r>",
        b"<r>\xef\xbf\xbe</r>",
        b"<r>\xff</r>",
        b"<r>text</x>",
        b"<r><!DOCTYPE r></r>",
        b"<!DOCTYPE r [<!ENTITY e 'v'>]><r>&e;</r>",
        b"<r>&bogus;</r>",
        b"<r><!-x--></r>",
        b"\xef\xbb\xbf<r>bom</r>",
        b"<?xml version='1.0' encoding='ISO-8859-1'?><r>\xe9</r>",
        b"<r xmlns:p='u'><p:e p:a='1'/></r>",
        b"<r>0123456789ab</r>",
        b"<r>0123456789abc</r>",
        b"<r a='12345'/>",
        b"<r a='123456'/>",
        b"<r>tail",
        b"<r></r>trailing",
    ];
    // Longer documents give the in-place chunk scanner whole tokens to take.
    let long: Vec<Vec<u8>> = vec![
        format!("<r>{}</r>", "<i a='v'>t&amp;x\r\n</i>".repeat(20)).into_bytes(),
        [
            &b"<?xml version='1.0' encoding='ISO-8859-1'?><r>"[..],
            &b"<e>\xe9</e>".repeat(10),
            b"</r>",
        ]
        .concat(),
        format!(
            "<!--{}--><!DOCTYPE r [<!ENTITY e 'v'>]><r>&e;{}</r>",
            "c".repeat(40),
            "<x/>".repeat(10)
        )
        .into_bytes(),
        [&b"<r>"[..], &b"<i/>".repeat(20), b"\x01</r>"].concat(),
        format!("<r>{}<toolongname/></r>", "<i/>".repeat(10)).into_bytes(),
        format!(
            "<根>{}</根>",
            "<前:項 xmlns:前='u' 前:属='é😀'>か</前:項>".repeat(5)
        )
        .into_bytes(),
        format!("{}<r/>{}", " \n".repeat(30), "<!--c-->".repeat(5)).into_bytes(),
        format!("<r/>{}tail", "<!--c-->".repeat(5)).into_bytes(),
    ];
    for input in cases.iter().copied().chain(long.iter().map(Vec::as_slice)) {
        for limits in [Limits::default(), tight] {
            let buffered = transcript(input, 1, limits);
            let pulled = lent(input, 1, limits);
            for chunk in [2, 3, 7, 33, 64, input.len()] {
                assert_eq!(
                    transcript(input, chunk, limits),
                    buffered,
                    "{} chunk {chunk}",
                    String::from_utf8_lossy(input)
                );
                assert_eq!(
                    lent(input, chunk, limits),
                    pulled,
                    "{} capacity {chunk}",
                    String::from_utf8_lossy(input)
                );
            }
        }
    }
}
