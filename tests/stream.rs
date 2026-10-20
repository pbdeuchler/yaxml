#![forbid(unsafe_code)]

#[path = "support/fuzz/driver.rs"]
mod driver;

#[path = "support/capture.rs"]
mod capture;

use std::ops::ControlFlow;
use yaxml::{ErrorKind, Limit, Limits, PushParser};

const VALID: &[&[u8]] = &[
    b"<r/>",
    b"\xef\xbb\xbf<?xml version='1.0' encoding='UTF-8' standalone='yes'?><r/>",
    b"<!--before--><?before data?><r a='&quot;&apos;&#10;&#13;&#9;\r\n\t'>&lt;&gt;&amp;&#x10FFFF;\r\nx\ry<![CDATA[\r\na<&]]><!--x\r\ny--><?pi x\r\ny?><c/></r><!--after-->",
    "<é a='中'>🙂é中</é>".as_bytes(),
    b"<r xmlns='u' xmlns:p='p' a='0' p:a='1'><p:c xmlns:p='q'><p:d/></p:c><p:e/><c xmlns=''/><c/></r>",
    b"<r xmlns:p='one'><a xmlns:p='two'/><p:b/><a xmlns='three'/><b/></r>",
    b" \r\n<r a='>' b='&gt;'/>",
    // Lent namespaces: equal local names under distinct URIs, and end tags
    // whose own scope closes with them.
    b"<r xmlns:a='u1' xmlns:b='u2' a:x='1' b:x='2'><a:e xmlns:a='u3' a:x='3'/><b:e/></r>",
    b"<a:r xmlns:a='u1'><a:r xmlns:a='u2'>t</a:r><a:r/></a:r>",
    // Lent normalized values: several per tag around plain values and a
    // normalized (unlent) declaration, then one per token in a row.
    b"<r a='1&amp;2' b='plain' xmlns:p='u&amp;v' p:c='x&#10;y' d='&lt;'>t&amp;u<?pi a\r\nb?><!--c\r\nd--><![CDATA[e\r\nf]]></r>",
    b"<r>a&amp;b<e f='&gt;'/>c&amp;d<e/>\r\n</r>",
];

#[test]
fn every_split_and_single_bytes() {
    for input in VALID {
        let expected = capture::slice_events(input).unwrap();
        for split in 0..=input.len() {
            let actual = capture::push_events(
                input,
                &[0, split, 0, input.len() - split, 0],
                Limits::default(),
            )
            .unwrap();
            assert_eq!(actual, expected, "split {split} for {input:?}");
        }
        assert_eq!(
            capture::push_events(input, &vec![1; input.len()], Limits::default()).unwrap(),
            expected
        );
    }
    assert!(driver::exercise(VALID[0]).error.is_none());
}

#[test]
fn deterministic_random_chunk_schedules() {
    let input = format!(
        "<r xmlns:p='one'>{}</r>",
        "<p:n a='中&amp;'>\r\n🙂<![CDATA[x]]></p:n>".repeat(32)
    );
    let expected = capture::slice_events(input.as_bytes()).unwrap();
    let mut state = 0x4d59_5df4_d0f3_3173_u64;
    for _ in 0..64 {
        let mut schedule = Vec::new();
        let mut remaining = input.len();
        while remaining > 0 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            if state & 1 == 0 {
                schedule.push(0);
            }
            let length = (1 + (state as usize % 37)).min(remaining);
            schedule.push(length);
            remaining -= length;
        }
        assert_eq!(
            capture::push_events(input.as_bytes(), &schedule, Limits::default()).unwrap(),
            expected
        );
    }
}

#[test]
fn every_malformed_split_and_prefix() {
    for input in [
        b"<r>".as_slice(),
        b"<r></s>",
        b"<r/><s/>",
        b"<r/>tail",
        b"<r a='1' a='2'/>",
        b"<r>&unknown;</r>",
        b"<r>&#0;</r>",
        b"<r><!--a--b--></r>",
        b"<r>]]></r>",
        b"<r>\x00</r>",
        b"<r>\xf0\x9f</r>",
        b"<?xml version='1.x'?><r/>",
        b"<?xml version='1.0' encoding='UTF-16'?><r/>",
        b"<!DOCTYPE r [<!ELEMENT>]><r/>",
        b"<r/ >",
        b"<r></s>\xff",
        b"<?pi",
        b"<!--",
        b"<r><![CDATA[",
    ] {
        assert!(capture::slice_events(input).is_err());
        for split in 0..=input.len() {
            assert!(
                capture::push_events(input, &[split, input.len() - split], Limits::default())
                    .is_err(),
                "split {split} for {input:?}"
            );
        }
    }
    for input in VALID {
        for end in 0..=input.len() {
            let prefix = &input[..end];
            assert_eq!(
                capture::slice_events(prefix).is_ok(),
                capture::push_events(prefix, &vec![1; end], Limits::default()).is_ok(),
                "prefix {end} for {input:?}"
            );
        }
    }
}

#[test]
fn isolated_failures_keep_global_offsets() {
    for input in [
        b"<r></s>".as_slice(),
        b"<r>&bad;</r>",
        b"<r>&#0;</r>",
        b"<r a='1' a='2'/>",
        b"<r>\xff</r>",
        b"<r>\x00</r>",
    ] {
        let expected = capture::slice_events(input).unwrap_err();
        for split in 0..=input.len() {
            assert_eq!(
                capture::push_events(input, &[split], Limits::default()).unwrap_err(),
                expected
            );
        }
    }
}

#[test]
fn normalization_has_independent_expected_values() {
    let input = b"<r a='\r\n\t&#xA;&#xD;&#9;'>x\r\ny\rz&#13;<![CDATA[a\r\nb]]></r>";
    let events = capture::push_events(input, &vec![1; input.len()], Limits::default()).unwrap();
    assert_eq!(events[1].0, capture::Captured::Text("x\ny\nz\r".into()));
    assert_eq!(events[2].0, capture::Captured::Cdata("a\nb".into()));
    match &events[0].0 {
        capture::Captured::Element(false, _, attributes) => assert_eq!(attributes[0].1, "  \n\r\t"),
        event => panic!("expected start event, got {event:?}"),
    }
}

#[test]
fn finish_and_error_states_are_terminal() {
    let mut parser = PushParser::default();
    assert_eq!(parser.position(), 0);
    assert_eq!(parser.event_range(), None);
    parser
        .feed(&[], |_, _| panic!("empty feed emitted"))
        .unwrap();
    parser
        .feed(b"<r/>", |_, _| ControlFlow::Continue(()))
        .unwrap();
    parser
        .finish(|_, _| panic!("unexpected EOF callback"))
        .unwrap();
    parser
        .finish(|_, _| panic!("repeated EOF callback"))
        .unwrap();
    let error = parser
        .feed(&[], |_, _| panic!("callback after finish"))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidState);
    assert_eq!(error.offset, 4);
    assert_eq!(parser.feed(b"ignored", |_, _| panic!()).unwrap_err(), error);
    assert_eq!(parser.finish(|_, _| panic!()).unwrap_err(), error);
    for input in [
        b"<r>\xf0\x9f".as_slice(),
        b"<r>&amp",
        b"<r><!--",
        b"<r><![CDATA[",
    ] {
        let mut parser = PushParser::new();
        parser
            .feed(input, |_, _| ControlFlow::Continue(()))
            .unwrap();
        let error = parser.finish(|_, _| ControlFlow::Continue(())).unwrap_err();
        assert_eq!(parser.finish(|_, _| panic!()).unwrap_err(), error);
        assert_eq!(parser.feed(b"suffix", |_, _| panic!()).unwrap_err(), error);
    }
}

#[test]
fn cancellation_at_every_event_and_pending_text() {
    let input = b"<?xml version='1.0'?><!--a--><r>x<c/><?pi data?><![CDATA[y]]></r>";
    let expected = capture::slice_events(input).unwrap();
    for stop in 0..expected.len() {
        let mut parser = PushParser::new();
        let mut events = Vec::new();
        let error = parser
            .feed(input, |event, origin| {
                let range = origin.input_range().expect("direct input event");
                events.push((capture::Captured::from(event), range));
                if events.len() == stop + 1 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            })
            .unwrap_err();
        assert_eq!(events, expected[..=stop]);
        assert_eq!(error.kind, ErrorKind::Aborted);
        assert_eq!(error.offset, expected[stop].1.end);
        assert_eq!(
            parser.event_range().expect("direct input event"),
            expected[stop].1
        );
        assert_eq!(parser.feed(b"", |_, _| panic!()).unwrap_err(), error);
        assert_eq!(parser.finish(|_, _| panic!()).unwrap_err(), error);
    }
    let mut parser = PushParser::new();
    parser
        .feed(b"<r>text", |_, _| ControlFlow::Continue(()))
        .unwrap();
    let error = parser
        .finish(|event, origin| {
            let range = origin.input_range().expect("direct input event");
            assert_eq!(
                capture::Captured::from(event),
                capture::Captured::Text("text".into())
            );
            assert_eq!(range, 3..7);
            ControlFlow::Break(())
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Aborted);
    assert_eq!(error.offset, 7);
    assert_eq!(parser.finish(|_, _| panic!()).unwrap_err(), error);
    assert_eq!(parser.feed(b"</r>", |_, _| panic!()).unwrap_err(), error);
}

#[test]
fn resource_boundaries_are_chunk_independent() {
    let defaults = Limits::default();
    for (input, limit, values) in [
        (b"<r/>".as_slice(), Limit::InputBytes, [3, 4, 5]),
        (b"<r/>", Limit::TokenBytes, [3, 4, 5]),
        (b"<r><c/></r>", Limit::Depth, [1, 2, 3]),
        (b"<r a='1'/>", Limit::Attributes, [0, 1, 2]),
        (b"<r>x</r>", Limit::Events, [2, 3, 4]),
        (b"<r></r>", Limit::OwnedBytes, [0, 1, 2]),
    ] {
        for (index, value) in values.into_iter().enumerate() {
            let mut limits = defaults;
            match limit {
                Limit::InputBytes => limits.max_input_bytes = value,
                Limit::TokenBytes => limits.max_token_bytes = value,
                Limit::Depth => limits.max_depth = value,
                Limit::Attributes => limits.max_attributes = value,
                Limit::Events => limits.max_events = value,
                Limit::OwnedBytes => limits.max_owned_bytes = value,
                _ => unreachable!("the table contains only parser limits"),
            }
            for schedule in [vec![input.len()], vec![1; input.len()]] {
                let result = capture::push_events(input, &schedule, limits);
                if index == 0 {
                    assert_eq!(result.unwrap_err().kind, ErrorKind::LimitExceeded(limit));
                } else {
                    assert!(result.is_ok(), "{limit:?} ceiling {value}: {result:?}");
                }
            }
        }
    }
}

#[test]
fn large_input_small_tokens_and_long_single_token() {
    let input = format!("<r>{}</r>", "<c/>".repeat(8192));
    let limits = Limits::default().max_token_bytes(8);
    assert!(capture::push_events(input.as_bytes(), &[input.len()], limits).is_ok());
    let input = format!("<r>{}</r>", "]-?".repeat(4096));
    let expected = capture::slice_events(input.as_bytes()).unwrap();
    assert_eq!(
        capture::push_events(input.as_bytes(), &vec![1; input.len()], Limits::default()).unwrap(),
        expected
    );
    let limits = Limits::default().max_token_bytes(32);
    assert_eq!(
        capture::push_events(input.as_bytes(), &vec![1; input.len()], limits)
            .unwrap_err()
            .kind,
        ErrorKind::LimitExceeded(Limit::TokenBytes)
    );
}

#[test]
fn input_ceiling_is_cumulative_whole_chunk_and_sticky() {
    let limits = Limits::default().max_input_bytes(6);
    // (chunks, failing chunk index): each chunk is checked whole before any byte.
    for (chunks, failing) in [
        (&[b"<r>".as_slice(), b"</r>"][..], Some(1)),
        (&[b"<r".as_slice(), b"/>", b""][..], None),
        (&[b"<r/>   ".as_slice()][..], Some(0)),
    ] {
        let mut parser = PushParser::with_limits(limits);
        let mut events = 0;
        let mut failed = None;
        for (index, chunk) in chunks.iter().enumerate() {
            if let Err(error) = parser.feed(chunk, |_, _| {
                events += 1;
                ControlFlow::Continue(())
            }) {
                assert_eq!(error.kind, ErrorKind::LimitExceeded(Limit::InputBytes));
                failed = Some((index, error));
                break;
            }
        }
        assert_eq!(failed.map(|(index, _)| index), failing, "{chunks:?}");
        match failed {
            Some((index, error)) => {
                // Nothing of the rejected chunk was processed.
                assert_eq!(events, index, "{chunks:?}");
                assert_eq!(parser.feed(b"", |_, _| panic!()).unwrap_err(), error);
                assert_eq!(parser.finish(|_, _| panic!()).unwrap_err(), error);
            }
            None => parser.finish(|_, _| ControlFlow::Continue(())).unwrap(),
        }
    }
}

/// `NOBLANKS` on push and pull as on the text reader (whose expectations
/// are `xmllint --stream --debug --noblanks` output): the text events that
/// stay, for every chunk split and read window, and the DOM's text nodes.
#[test]
fn noblanks_drops_ignorable_blank_events_whatever_the_chunks() {
    use yaxml::options::{NOBLANKS, NOENT, RECOVER};
    use yaxml::{BufferedReader, Document, Event, NodeKind, ParseOptions};
    // (flags, document, text events kept)
    let cases: &[(u32, &str, &[&str])] = &[
        (0, "<r>\n <a>\n  <b/>\n </a>\n <c> </c>\n</r>", &[" "]),
        // `xml:space="preserve"` keeps blanks below it, `default` does not.
        (
            0,
            "<r xml:space='preserve'> <a> </a><b xml:space='default'> <c/> </b></r>",
            &[" ", " "],
        ),
        // Element content declared by the DTD is dropped, #PCDATA is kept.
        (
            0,
            "<!DOCTYPE r [<!ELEMENT r (a)*><!ELEMENT a (#PCDATA)>]><r> <a> </a> </r>",
            &[" "],
        ),
        // Mixed content keeps every blank, whatever the tags declare.
        (
            0,
            "<!DOCTYPE r [<!ELEMENT r (#PCDATA|a)*><!ELEMENT a EMPTY>]><r> <a/> <a/> </r>",
            &[" ", " ", " "],
        ),
        // Text that the document spoke makes later blanks significant.
        (0, "<r><a/>x<b/> <c/></r>", &["x", " "]),
        // So does a blank before a reference, whose content passes.
        (
            0,
            "<!DOCTYPE r [<!ENTITY e ' x '>]><r><a/> &e;<a/></r>",
            &[" ", " x "],
        ),
        (
            NOENT,
            "<!DOCTYPE r [<!ENTITY e ' '>]><r><a/> &e;<a/></r>",
            &[" ", " "],
        ),
        (0, "<r> </r>", &[" "]),
        (0, "<r>\n<a/>\n<a/>\n</r>", &[]),
        // Recovery closes elements as events, which decide blanks too: an only
        // child is kept.
        (RECOVER, "<r><a/>\n<b>\n", &["\n"]),
    ];
    for &(flags, xml, kept) in cases {
        let options = || ParseOptions::new(flags | NOBLANKS);
        let input = xml.as_bytes();
        let splits = (0..=input.len()).map(|at| vec![&input[..at], &input[at..]]);
        for chunks in splits.chain([input.chunks(1).collect()]) {
            let mut parser = PushParser::with_options(options()).unwrap();
            let mut seen = Vec::new();
            let mut emit = |event: &Event<'_>, _| {
                if let Event::Text(text) = event {
                    seen.push(text.to_string());
                }
                ControlFlow::Continue(())
            };
            for chunk in chunks {
                parser.feed(chunk, &mut emit).unwrap();
            }
            parser.finish(&mut emit).unwrap();
            assert_eq!(seen, kept, "{xml}");
        }
        for window in [1, 2, 5, 4096] {
            let source = std::io::BufReader::with_capacity(window, input);
            let mut reader = BufferedReader::with_options(source, options()).unwrap();
            let mut seen = Vec::new();
            while let Some((event, _)) = reader.read_event().unwrap() {
                if let Event::Text(text) = event {
                    seen.push(text.to_string());
                }
            }
            assert_eq!(seen, kept, "{xml} by {window}");
        }
        // So does the text reader.
        let mut reader = yaxml::TextReader::with_options(input, options()).unwrap();
        let mut seen = Vec::new();
        while reader.read().unwrap() {
            if matches!(reader.node_type() as i32, 3 | 14) && reader.depth() > 0 {
                seen.push(reader.value().unwrap_or_default().to_owned());
            }
        }
        if !xml.contains('&') {
            assert_eq!(seen, kept, "text reader: {xml}");
        }
        // The DOM drops the same blanks, references aside.
        if !xml.contains('&') {
            let document = Document::parse_with(input, options()).unwrap();
            let nodes = document.nodes().filter_map(|node| match node.kind() {
                NodeKind::Text(text) => Some(text.to_string()),
                _ => None,
            });
            assert_eq!(nodes.collect::<Vec<_>>(), kept, "{xml}");
        }
    }
}
