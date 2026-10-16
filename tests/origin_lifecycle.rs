use std::{io::Cursor, ops::ControlFlow};

use yaxml::{BufferedReader, Document, ErrorKind, Event, OwnedDocument, PushParser, Reader};

#[test]
fn utf8_bom_ranges_survive_normalization_and_eof() {
    let input = b"\xef\xbb\xbf<r>x\r\n&#13;</r>";
    let mut reader = Reader::new(input).unwrap();
    assert_eq!(reader.event_origin(), None);
    assert_eq!(reader.event_range(), None);
    assert_eq!(reader.position(), 3);
    let expected = [3..6, 6..14, 14..18];
    for range in expected {
        let event = reader.read_event().unwrap().unwrap();
        if let Event::Text(value) = event {
            assert_eq!(value, "x\n\r");
        }
        let origin = reader.event_origin().unwrap();
        assert_eq!(origin.input_range(), Some(range.clone()));
        assert_eq!(reader.event_range(), Some(range));
    }
    let last = reader.event_origin();
    for _ in 0..2 {
        assert!(reader.read_event().unwrap().is_none());
        assert_eq!(reader.event_origin(), last);
    }
    let borrowed = Document::parse_bytes(input).unwrap();
    let owned = OwnedDocument::parse_bytes(input).unwrap();
    for (a, b) in borrowed.nodes().zip(owned.nodes()) {
        assert_eq!(Some(a.origin()), b.origin());
        assert_eq!(a.range(), b.range());
    }
    assert_eq!(borrowed.root().range(), Some(0..18));
    assert_eq!(borrowed.root_element().range(), Some(3..18));
    assert_eq!(
        borrowed.root_element().first_child().unwrap().range(),
        Some(6..14)
    );
}

#[test]
fn original_span_does_not_control_buffered_consumption() {
    let mut reader = BufferedReader::new(Cursor::new(b"<r>text</r>suffix"));
    let (_, start) = reader.read_event().unwrap().unwrap();
    assert_eq!(start.input_range(), Some(0..3));
    let (event, text) = reader.read_event().unwrap().unwrap();
    assert!(matches!(event, Event::Text(value) if value == "text"));
    assert_eq!(text.input_range(), Some(3..7));
    assert_eq!(reader.position(), 8);
    assert_eq!(reader.get_ref().position(), 8);
    let input = reader.into_inner();
    assert_eq!(&input.get_ref()[input.position() as usize..], b"/r>suffix");
}

#[test]
fn utf16_cancellation_keeps_the_delivered_origin() {
    for prefix in [[0xff, 0xfe], [0xfe, 0xff]] {
        let mut input = prefix.to_vec();
        for unit in "<r>🙂</r>".encode_utf16() {
            input.extend_from_slice(&if prefix[0] == 0xff {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        let mut parser = PushParser::new();
        assert_eq!(parser.event_origin(), None);
        let mut latest = None;
        let error = parser
            .feed(&input, |event, origin| {
                latest = Some(origin);
                if matches!(event, Event::Text(_)) {
                    assert_eq!(origin.input_range(), Some(8..12));
                    ControlFlow::Break(())
                } else {
                    assert_eq!(origin.input_range(), Some(2..8));
                    ControlFlow::Continue(())
                }
            })
            .unwrap_err();
        assert_eq!((error.kind, error.offset), (ErrorKind::Aborted, 12));
        assert_eq!(parser.position(), 14); // the physical '<' lookahead
        assert_eq!(parser.event_origin(), latest);
        assert_eq!(parser.event_range(), Some(8..12));
        assert_eq!(
            parser.finish(|_, _| panic!("callback after cancellation")),
            Err(error)
        );
        assert_eq!(parser.event_origin(), latest);
    }
}
