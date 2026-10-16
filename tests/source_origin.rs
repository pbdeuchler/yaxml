#![forbid(unsafe_code)]

use std::io::{BufReader, Cursor};
use std::ops::{ControlFlow, Range};

use yaxml::{
    BufferedReader, EditLimits, ElementSpec, ErrorKind, Event, NameSpec, OwnedDocument,
    PrefixPolicy, PushParser, Reader, SourceOrigin,
};

const SOURCE: &str = "<π>𝄞\r\n&#xD;</π>";

#[derive(Clone, Copy, Debug)]
enum Encoding {
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Observed {
    Start(String),
    Text(String),
    End(String),
    Empty(String),
}

impl Encoding {
    fn encode(self, source: &str) -> Vec<u8> {
        match self {
            Self::Utf8Bom => [b"\xef\xbb\xbf".as_slice(), source.as_bytes()].concat(),
            Self::Utf16Le | Self::Utf16Be => std::iter::once(0xfeff)
                .chain(source.encode_utf16())
                .flat_map(|unit| {
                    if matches!(self, Self::Utf16Le) {
                        unit.to_le_bytes()
                    } else {
                        unit.to_be_bytes()
                    }
                })
                .collect(),
        }
    }

    fn event_ranges(self) -> [Range<usize>; 3] {
        match self {
            Self::Utf8Bom => [3..7, 7..18, 18..23],
            Self::Utf16Le | Self::Utf16Be => [2..8, 8..26, 26..34],
        }
    }

    fn root_range(self) -> Range<usize> {
        match self {
            Self::Utf8Bom => 3..23,
            Self::Utf16Le | Self::Utf16Be => 2..34,
        }
    }

    fn text_lookahead(self) -> usize {
        match self {
            Self::Utf8Bom => 1,
            Self::Utf16Le | Self::Utf16Be => 2,
        }
    }
}

const ENCODINGS: [Encoding; 3] = [Encoding::Utf8Bom, Encoding::Utf16Le, Encoding::Utf16Be];

fn observed(event: Event<'_>) -> Observed {
    match event {
        Event::Start(element) => Observed::Start(element.name.qualified().to_owned()),
        Event::Text(value) => Observed::Text(value.into_owned()),
        Event::End(name) => Observed::End(name.qualified().to_owned()),
        Event::Empty(element) => Observed::Empty(element.name.qualified().to_owned()),
        other => panic!("unexpected event in literal fixture: {other:?}"),
    }
}

fn assert_origin(origin: SourceOrigin, expected: Range<usize>) {
    assert_eq!(origin.input_range(), Some(expected));
}

fn exact(local: &str) -> NameSpec<'_> {
    NameSpec {
        local,
        namespace: None,
        prefix: PrefixPolicy::Exact(None),
    }
}

fn empty(local: &str) -> ElementSpec<'_> {
    ElementSpec {
        name: exact(local),
        items: &[],
    }
}

#[test]
fn source_origin_is_an_owned_copyable_boundary_value() {
    fn assert_traits<T: Copy + Send + Sync>() {}
    assert_traits::<SourceOrigin>();
    assert_eq!(SOURCE.len(), 20);
    assert_eq!(Encoding::Utf8Bom.encode(SOURCE).len(), 23);
    assert_eq!(Encoding::Utf16Le.encode(SOURCE).len(), 34);
    assert_eq!(Encoding::Utf16Be.encode(SOURCE).len(), 34);

    let retained = {
        let input = Encoding::Utf8Bom.encode(SOURCE);
        let mut reader = Reader::new(&input).unwrap();
        assert!(reader.read_event().unwrap().is_some());
        reader.event_origin().unwrap()
    };
    assert_origin(retained, 3..7);
    assert_eq!(retained, retained);
    drop(format!("{retained:?}"));
}

#[test]
fn reader_reports_literal_origins_and_retains_the_last_delivery() {
    for encoding in [Encoding::Utf8Bom] {
        let input = encoding.encode(SOURCE);
        let expected_ranges = encoding.event_ranges();
        let expected_events = [
            Observed::Start("π".into()),
            Observed::Text("𝄞\n\r".into()),
            Observed::End("π".into()),
        ];
        let mut reader = Reader::new(&input).unwrap();
        assert_eq!(reader.event_origin(), None, "{encoding:?}");
        assert_eq!(reader.event_range(), None, "{encoding:?}");

        for (expected_event, expected_range) in
            expected_events.iter().zip(expected_ranges.iter().cloned())
        {
            let event = reader.read_event().unwrap().unwrap();
            assert_eq!(&observed(event), expected_event, "{encoding:?}");
            let origin = reader
                .event_origin()
                .expect("delivered event has an origin");
            assert_origin(origin, expected_range.clone());
            assert_eq!(reader.event_range(), Some(expected_range), "{encoding:?}");
        }

        let retained = reader.event_origin();
        for _ in 0..3 {
            assert!(reader.read_event().unwrap().is_none());
            assert_eq!(reader.event_origin(), retained, "{encoding:?}");
            assert_eq!(reader.event_range(), Some(expected_ranges[2].clone()));
        }
    }
}

#[test]
fn reader_and_push_start_without_an_origin_after_bom_and_whitespace() {
    for encoding in ENCODINGS {
        let input = encoding.encode(" \r\n<r/>");
        let expected = match encoding {
            Encoding::Utf8Bom => 6..10,
            Encoding::Utf16Le | Encoding::Utf16Be => 8..16,
        };

        if matches!(encoding, Encoding::Utf8Bom) {
            let mut reader = Reader::new(&input).unwrap();
            assert_eq!(reader.event_origin(), None);
            assert_eq!(reader.event_range(), None);
            assert_eq!(
                observed(reader.read_event().unwrap().unwrap()),
                Observed::Empty("r".into())
            );
            assert_origin(reader.event_origin().unwrap(), expected.clone());
        }

        let mut push = PushParser::new();
        assert_eq!(push.event_origin(), None);
        assert_eq!(push.event_range(), None);
        let mut delivered = Vec::new();
        push.feed(&input, |event, origin| {
            delivered.push((observed(event), origin));
            ControlFlow::Continue(())
        })
        .unwrap();
        push.finish(|_, _| panic!("empty element was already delivered"))
            .unwrap();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].0, Observed::Empty("r".into()));
        assert_origin(delivered[0].1, expected.clone());
        assert_eq!(push.event_origin(), Some(delivered[0].1));
        assert_eq!(push.event_range(), Some(expected));
    }
}

#[test]
fn push_origins_are_split_independent_at_every_byte_boundary() {
    for encoding in ENCODINGS {
        let input = encoding.encode(SOURCE);
        let ranges = encoding.event_ranges();
        let expected = vec![
            (Observed::Start("π".into()), ranges[0].clone()),
            (Observed::Text("𝄞\n\r".into()), ranges[1].clone()),
            (Observed::End("π".into()), ranges[2].clone()),
        ];

        for split in 0..=input.len() {
            let mut parser = PushParser::new();
            assert_eq!(parser.event_origin(), None);
            assert_eq!(parser.event_range(), None);
            let mut actual = Vec::new();
            parser
                .feed(&input[..split], |event, origin| {
                    actual.push((observed(event), origin.input_range().unwrap()));
                    ControlFlow::Continue(())
                })
                .unwrap();
            parser
                .feed(&input[split..], |event, origin| {
                    actual.push((observed(event), origin.input_range().unwrap()));
                    ControlFlow::Continue(())
                })
                .unwrap();
            parser
                .finish(|event, origin| {
                    actual.push((observed(event), origin.input_range().unwrap()));
                    ControlFlow::Continue(())
                })
                .unwrap();
            assert_eq!(actual, expected, "{encoding:?} split {split}");
            assert_eq!(parser.event_range(), Some(ranges[2].clone()));
            let retained = parser.event_origin();
            for _ in 0..2 {
                parser
                    .finish(|_, _| panic!("repeated finish emitted"))
                    .unwrap();
                assert_eq!(parser.event_origin(), retained);
            }
        }
    }
}

#[test]
fn buffered_origin_excludes_physical_text_lookahead() {
    for encoding in ENCODINGS {
        let input = encoding.encode(SOURCE);
        let ranges = encoding.event_ranges();
        let source = BufReader::with_capacity(1, Cursor::new(input));
        let mut reader = BufferedReader::new(source);

        for (index, expected_event) in [
            Observed::Start("π".into()),
            Observed::Text("𝄞\n\r".into()),
            Observed::End("π".into()),
        ]
        .into_iter()
        .enumerate()
        {
            let (event, origin) = reader.read_event().unwrap().unwrap();
            assert_eq!(observed(event), expected_event, "{encoding:?}");
            assert_origin(origin, ranges[index].clone());
            if index == 1 {
                let physical = ranges[1].end + encoding.text_lookahead();
                assert_eq!(reader.position(), physical, "{encoding:?}");
                assert_eq!(reader.get_ref().get_ref().position() as usize, physical);
            }
        }
        assert!(reader.read_event().unwrap().is_none());
        assert!(reader.read_event().unwrap().is_none());
    }
}

#[test]
fn failures_and_cancellation_retain_the_last_delivered_origin() {
    let input = b"<r>x</s>";
    let mut reader = Reader::new(input).unwrap();
    assert_eq!(
        observed(reader.read_event().unwrap().unwrap()),
        Observed::Start("r".into())
    );
    assert_eq!(
        observed(reader.read_event().unwrap().unwrap()),
        Observed::Text("x".into())
    );
    let retained = reader.event_origin().unwrap();
    assert_origin(retained, 3..4);
    let error = reader.read_event().unwrap_err();
    assert_eq!(error.kind, ErrorKind::MismatchedEndTag);
    assert_eq!(error.offset, 4);
    assert_eq!(reader.event_origin(), Some(retained));
    assert_eq!(reader.event_range(), Some(3..4));
    assert_eq!(reader.read_event().unwrap_err(), error);

    let mut push = PushParser::new();
    let mut delivered = Vec::new();
    let error = push
        .feed(input, |event, origin| {
            delivered.push((observed(event), origin));
            ControlFlow::Continue(())
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::MismatchedEndTag);
    assert_eq!(error.offset, 4);
    assert_eq!(delivered.len(), 2);
    assert_origin(delivered[1].1, 3..4);
    assert_eq!(push.event_origin(), Some(delivered[1].1));
    assert_eq!(push.event_range(), Some(3..4));
    assert_eq!(push.feed(b"ignored", |_, _| panic!()).unwrap_err(), error);

    let input = Encoding::Utf8Bom.encode(SOURCE);
    let mut push = PushParser::new();
    let mut text_origin = None;
    let error = push
        .feed(&input, |event, origin| {
            if matches!(event, Event::Text(_)) {
                text_origin = Some(origin);
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Aborted);
    assert_eq!(error.offset, 18);
    assert_eq!(push.position(), 19, "text terminator is lookahead");
    let text_origin = text_origin.unwrap();
    assert_origin(text_origin, 7..18);
    assert_eq!(push.event_origin(), Some(text_origin));
    assert_eq!(push.event_range(), Some(7..18));
    assert_eq!(push.finish(|_, _| panic!()).unwrap_err(), error);
}

#[test]
fn borrowed_dom_nodes_expose_literal_nested_origins() {
    let input = b"<r><a>x</a><b/></r>";
    let document = yaxml::Document::parse_bytes(input).unwrap();
    let root = document.root_element();
    let a = root.first_child().unwrap();
    let text = a.first_child().unwrap();
    let b = a.next_sibling().unwrap();
    for (node, expected) in [(root, 0..19), (a, 3..11), (text, 6..7), (b, 11..15)] {
        assert_eq!(node.range(), Some(expected.clone()));
        assert_origin(node.origin(), expected);
    }
}

#[test]
fn owned_parsed_origins_survive_input_drop_and_content_edits() {
    for encoding in ENCODINGS {
        let mut input = encoding.encode(SOURCE);
        let document = OwnedDocument::parse_bytes(&input).unwrap();
        input.fill(0);
        drop(input);

        let root = document.root_element();
        let text = root.first_child().unwrap();
        assert_eq!(root.range(), Some(encoding.root_range()));
        assert_eq!(root.origin().unwrap().input_range(), root.range());
        assert_eq!(text.range(), Some(encoding.event_ranges()[1].clone()));
        assert_eq!(text.origin().unwrap().input_range(), text.range());
        assert_eq!(text.text(), Some("𝄞\n\r"));
        assert!(document.root().origin().is_some());
    }

    let mut document = OwnedDocument::parse("<r a='old'>text<c/></r>").unwrap();
    let root = document.root_element().id();
    let text = document.root_element().first_child().unwrap().id();
    let parsed_child = document.root_element().last_child().unwrap().id();
    let root_origin = document.node(root).unwrap().origin();
    let text_origin = document.node(text).unwrap().origin();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    editor.set_node_value(text, "changed").unwrap();
    editor.set_attribute_value(root, None, "a", "new").unwrap();
    let constructed = editor.append_element(root, empty("new")).unwrap();
    let constructed_text = editor.append_text(constructed, "literal").unwrap();
    editor.remove_subtree(parsed_child).unwrap();
    assert_eq!(editor.document().node(root).unwrap().origin(), root_origin);
    assert_eq!(editor.document().node(text).unwrap().origin(), text_origin);
    assert_eq!(editor.document().node(constructed).unwrap().origin(), None);
    assert_eq!(
        editor.document().node(constructed_text).unwrap().origin(),
        None
    );
    assert!(editor.document().node(parsed_child).is_none());
    let fresh = editor.append_element(root, empty("fresh")).unwrap();
    assert_ne!(fresh, parsed_child);
    assert_eq!(editor.document().node(fresh).unwrap().origin(), None);
}

#[test]
fn wholly_constructed_documents_have_no_input_origin() {
    let mut document = OwnedDocument::create(empty("root"), EditLimits::default()).unwrap();
    assert_eq!(document.root().origin(), None);
    assert_eq!(document.root_element().origin(), None);
    assert_eq!(document.root().range(), None);
    assert_eq!(document.root_element().range(), None);

    let root = document.root_element().id();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let child = editor.append_element(root, empty("child")).unwrap();
    let text = editor.append_text(child, "text").unwrap();
    assert_eq!(editor.document().node(child).unwrap().origin(), None);
    assert_eq!(editor.document().node(text).unwrap().origin(), None);
}
