use std::borrow::Cow;

use yaxml::{Error, ErrorKind, Event, Limit, Limits, Reader};

fn read(input: &[u8]) -> Result<Vec<Event<'_>>, Error> {
    let mut reader = Reader::new(input)?;
    let mut events = Vec::new();
    while let Some(event) = reader.read_event()? {
        events.push(event);
    }
    assert!(reader.read_event()?.is_none());
    assert_eq!(reader.position(), input.len());
    Ok(events)
}

#[test]
fn accepted_document_forms() {
    for input in [
        "<r/>",
        "<r></r>",
        "\u{feff}<r/>",
        " \r\n<r/> \t",
        "<!--before--><?pi?><r/><!--after--><?pi data ?>",
        "<?xml version='1.0'?><r/>",
        "<?xml version = \"1.0\" encoding='utf-8' standalone='no' ?><r/>",
        "<?xml version='1.0' standalone='yes'?><r/>",
        "<r a='>' b=\"&lt;&amp;\"> > <![CDATA[<&]]><!--ok--><?pi x?></r>",
        "<r xmlns='u' xmlns:p='p'><p:a a='v'/><a xmlns=''/></r>",
        "<r xml:lang='en' xmlns:xml='http://www.w3.org/XML/1998/namespace'/>",
        "<r xmlns:p='u' p:a='1' a='2'/>",
    ] {
        read(input.as_bytes()).unwrap_or_else(|e| panic!("{input:?}: {e}"));
    }
}

#[test]
fn malformed_documents_are_rejected() {
    use ErrorKind::*;
    for (input, expected) in [
        ("", MissingRoot),
        (" \n", MissingRoot),
        ("<!--only-->", MissingRoot),
        ("<r>", UnexpectedEof),
        ("<r", UnexpectedEof),
        ("<r a='x>", UnexpectedEof),
        ("<r><!--", UnexpectedEof),
        ("<r><![CDATA[x", UnexpectedEof),
        ("<?pi", UnexpectedEof),
        ("<r></s>", MismatchedEndTag),
        ("</r>", MismatchedEndTag),
        ("<r/><s/>", MultipleRoots),
        ("x<r/>", TextOutsideRoot),
        ("<r/>x", TextOutsideRoot),
        ("&#32;<r/>", TextOutsideRoot),
        ("<![CDATA[ ]]><r/>", TextOutsideRoot),
        ("<r>]]></r>", Syntax),
        ("<r><!--a--b--></r>", Syntax),
        ("<r><!--a---></r>", Syntax),
        ("<!other><r/>", Syntax),
        ("<r a=x/>", Syntax),
        ("<r a 'v'/>", Syntax),
        ("<r a='<'/>", Syntax),
        ("<r a='v'b='v'/>", Syntax),
        ("<r / >", InvalidName),
        ("<r></r a>", Syntax),
        ("<r></ r>", InvalidName),
        ("< r/>", InvalidName),
        ("<1/>", InvalidName),
        ("<a:b:c/>", InvalidName),
        ("<:a/>", InvalidName),
        ("<a:/>", InvalidName),
        ("<r a::b='v'/>", InvalidName),
        ("<?a:b?><r/>", InvalidName),
        ("<r a='1' a='2'/>", DuplicateAttribute),
        ("<p:r/>", Namespace),
        ("<r p:a='v'/>", Namespace),
        ("<xmlns:r/>", Namespace),
        ("<r xmlns:p=''/>", Namespace),
        ("<r xmlns:xmlns='u'/>", Namespace),
        ("<r xmlns:xml='u'/>", Namespace),
        (
            "<r xmlns='http://www.w3.org/XML/1998/namespace'/>",
            Namespace,
        ),
        (
            "<r xmlns:p='http://www.w3.org/XML/1998/namespace'/>",
            Namespace,
        ),
        ("<r xmlns='http://www.w3.org/2000/xmlns/'/>", Namespace),
        ("<r xmlns:p='http://www.w3.org/2000/xmlns/'/>", Namespace),
        (
            "<r xmlns:p='u' xmlns:q='u' p:a='1' q:a='2'/>",
            DuplicateAttribute,
        ),
        ("<r><s xmlns:p='u'/><p:s/></r>", Namespace),
        ("<r>&unknown;</r>", UnknownEntity),
        ("<r>&amp</r>", InvalidReference),
        ("<r>&#;</r>", InvalidReference),
        ("<r>&#x;</r>", InvalidReference),
        ("<r>&#X41;</r>", InvalidReference),
        ("<r>&#+1;</r>", InvalidReference),
        ("<r>&#-1;</r>", InvalidReference),
        ("<r>&#x+41;</r>", InvalidReference),
        ("<r>&#xGG;</r>", InvalidReference),
        ("<r>&#99999999999999999;</r>", InvalidReference),
        ("<r>&#0;</r>", InvalidReference),
        ("<r>&#xD800;</r>", InvalidReference),
        ("<r>&#xFFFE;</r>", InvalidReference),
        ("<r>&#x110000;</r>", InvalidReference),
        ("<r>\u{0}</r>", InvalidCharacter),
        ("<r>\u{b}</r>", InvalidCharacter),
        ("<r>\u{fffe}</r>", InvalidCharacter),
        ("<?xml?><r/>", InvalidDeclaration),
        ("<?XML version='1.0'?><r/>", InvalidDeclaration),
        (" <?xml version='1.0'?><r/>", InvalidDeclaration),
        ("<r><?xml version='1.0'?></r>", InvalidDeclaration),
        (
            "<?xml version='1.0'?><?xml version='1.0'?><r/>",
            InvalidDeclaration,
        ),
        ("<?xml version='1.x'?><r/>", InvalidDeclaration),
        ("<?xml standalone='yes'?><r/>", InvalidDeclaration),
        (
            "<?xml version='1.0' version='1.0'?><r/>",
            InvalidDeclaration,
        ),
        (
            "<?xml version='1.0' standalone='maybe'?><r/>",
            InvalidDeclaration,
        ),
        (
            "<?xml version='1.0' standalone='yes' encoding='UTF-8'?><r/>",
            InvalidDeclaration,
        ),
        (
            "<?xml version='1.0' standalone='yes' standalone='no'?><r/>",
            InvalidDeclaration,
        ),
        ("<?xml version=1.0?><r/>", InvalidDeclaration),
        (
            "<?xml version='1.0' encoding='ISO-8859-1'?><r/>",
            UnsupportedEncoding,
        ),
        ("<!DOCTYPE r><r/>", UnsupportedDtd),
    ] {
        assert_eq!(
            read(input.as_bytes()).unwrap_err().kind,
            expected,
            "{input:?}"
        );
    }
}

#[test]
fn byte_validation_and_offsets() {
    for (input, kind, offset) in [
        (b"<r>\xff</r>".as_slice(), ErrorKind::InvalidUtf8, 3),
        (b"\xff\xfe<\0r\0/\0>\0", ErrorKind::UnsupportedEncoding, 0),
        (b"\xfe\xff\0<\0r", ErrorKind::UnsupportedEncoding, 0),
        (b"\0\0\xfe\xff", ErrorKind::UnsupportedEncoding, 0),
        (b"\0<\0?", ErrorKind::UnsupportedEncoding, 0),
        (b"<r>\0</r>", ErrorKind::InvalidCharacter, 3),
        (b"<r a='x&bad;' />", ErrorKind::UnknownEntity, 7),
        (b"<r>x&#0;</r>", ErrorKind::InvalidReference, 4),
    ] {
        let error = read(input).unwrap_err();
        assert_eq!((error.kind, error.offset), (kind, offset), "{input:?}");
        assert_eq!(
            format!("{error}"),
            format!("XML error {kind:?} at byte {offset}")
        );
        let _: &dyn std::error::Error = &error;
    }
}

#[test]
fn logical_values_and_source_ranges() {
    let input = "\u{feff}<?xml version='1.0'?>\r\n<r a='\t\n\r\n&#xA;&#13;&lt;&gt;&amp;&apos;&quot;'>a\r\nb\rc&#xD;&#10;<![CDATA[x\r\ny&z]]><!--a\rb--><?p \r\nvalue\r?></r> ";
    let mut reader = Reader::new(input.as_bytes()).unwrap();
    assert_eq!(reader.position(), 3);
    assert_eq!(reader.event_range(), 3..3);
    let mut ranges = Vec::new();
    let mut events = Vec::new();
    while let Some(event) = reader.read_event().unwrap() {
        ranges.push(reader.event_range());
        events.push(event);
    }
    let Event::Declaration(d) = &events[0] else {
        panic!()
    };
    assert_eq!((d.version, d.encoding, d.standalone), ("1.0", None, None));
    let Event::Start(element) = &events[1] else {
        panic!()
    };
    assert_eq!(element.attributes[0].value, "   \n\r<>&'\"");
    let Event::Text(text) = &events[2] else {
        panic!()
    };
    assert_eq!(text, "a\nb\nc\r\n");
    assert!(matches!(&events[3], Event::Cdata(v) if v == "x\ny&z"));
    assert!(matches!(&events[4], Event::Comment(v) if v == "a\nb"));
    assert!(
        matches!(&events[5], Event::ProcessingInstruction {target: Cow::Borrowed("p"), data} if data == "value\n")
    );
    assert!(matches!(&events[6], Event::End(n) if n.qualified() == "r"));
    for (range, expected) in ranges.iter().zip([
        "<?xml version='1.0'?>",
        "<r a='\t\n\r\n&#xA;&#13;&lt;&gt;&amp;&apos;&quot;'>",
        "a\r\nb\rc&#xD;&#10;",
        "<![CDATA[x\r\ny&z]]>",
        "<!--a\rb-->",
        "<?p \r\nvalue\r?>",
        "</r>",
    ]) {
        assert_eq!(&input[range.clone()], expected);
    }
}

#[test]
fn namespaces_shadow_and_unwind() {
    let events = read(b"<r xmlns='urn:d' xmlns:p='urn:p'><p:c xmlns:p='urn:q' p:a='1' a='2'/><p:c/><c xmlns=''/><c/></r>").unwrap();
    let mut names = Vec::new();
    for event in &events {
        let element = match event {
            Event::Start(e) | Event::Empty(e) => e,
            _ => continue,
        };
        names.push((element.name.qualified(), element.name.namespace()));
    }
    assert_eq!(
        names,
        [
            ("r", Some("urn:d")),
            ("p:c", Some("urn:q")),
            ("p:c", Some("urn:p")),
            ("c", None),
            ("c", Some("urn:d"))
        ]
    );
    let Event::Empty(element) = &events[1] else {
        panic!()
    };
    assert_eq!(
        element.attributes[0].name.namespace(),
        Some("http://www.w3.org/2000/xmlns/")
    );
    assert_eq!(element.attributes[1].name.namespace(), Some("urn:q"));
    assert_eq!(element.attributes[2].name.namespace(), None);

    let events = read(b"<p:r xmlns:p='urn:&#x61;'><p:c/></p:r>").unwrap();
    for event in events {
        let name = match event {
            Event::Start(e) | Event::Empty(e) => e.name,
            Event::End(n) => n,
            _ => continue,
        };
        assert_eq!(name.namespace(), Some("urn:a"));
    }
}

#[test]
fn resource_limits_are_inclusive() {
    for (input, limits, expected) in [
        (
            b"<r/>".as_slice(),
            Limits {
                max_input_bytes: 3,
                ..Limits::default()
            },
            Limit::InputBytes,
        ),
        (
            b"<r/>",
            Limits {
                max_token_bytes: 3,
                ..Limits::default()
            },
            Limit::TokenBytes,
        ),
        (
            b"<r>abcd</r>",
            Limits {
                max_token_bytes: 3,
                ..Limits::default()
            },
            Limit::TokenBytes,
        ),
        (
            b"<!--x--><r/>",
            Limits {
                max_token_bytes: 7,
                ..Limits::default()
            },
            Limit::TokenBytes,
        ),
        (
            b"<r/>",
            Limits {
                max_depth: 0,
                ..Limits::default()
            },
            Limit::Depth,
        ),
        (
            b"<r><c/></r>",
            Limits {
                max_depth: 1,
                ..Limits::default()
            },
            Limit::Depth,
        ),
        (
            b"<r a='v'/>",
            Limits {
                max_attributes: 0,
                ..Limits::default()
            },
            Limit::Attributes,
        ),
        (
            b"<r/>",
            Limits {
                max_events: 0,
                ..Limits::default()
            },
            Limit::Events,
        ),
        (
            b"<r></r>",
            Limits {
                max_events: 1,
                ..Limits::default()
            },
            Limit::Events,
        ),
        (
            b"<r>&amp;</r>",
            Limits {
                max_owned_bytes: 4,
                ..Limits::default()
            },
            Limit::OwnedBytes,
        ),
        (
            b"<r>\r</r>",
            Limits {
                max_owned_bytes: 0,
                ..Limits::default()
            },
            Limit::OwnedBytes,
        ),
    ] {
        let result = Reader::with_limits(input, limits).and_then(|mut r| {
            while r.read_event()?.is_some() {}
            Ok(())
        });
        assert_eq!(
            result.unwrap_err().kind,
            ErrorKind::LimitExceeded(expected),
            "{input:?}"
        );
    }
    for (input, limits) in [
        (
            b"<r/>".as_slice(),
            Limits {
                max_input_bytes: 4,
                max_token_bytes: 4,
                max_depth: 1,
                max_attributes: 0,
                max_events: 1,
                max_owned_bytes: 0,
            },
        ),
        (
            b"<r a='v'/>",
            Limits {
                max_attributes: 1,
                ..Limits::default()
            },
        ),
        (
            b"<r>&amp;</r>",
            Limits {
                max_owned_bytes: 5,
                ..Limits::default()
            },
        ),
        (
            b"<r></r>",
            Limits {
                max_events: 2,
                ..Limits::default()
            },
        ),
        (
            b"<r>plain</r>",
            Limits {
                max_owned_bytes: 0,
                ..Limits::default()
            },
        ),
    ] {
        let mut reader = Reader::with_limits(input, limits).unwrap();
        while reader.read_event().unwrap().is_some() {}
        assert!(reader.read_event().unwrap().is_none());
    }
}

#[test]
fn fatal_errors_are_sticky() {
    for input in ["<r></x>", "<r><p:x/></r>", "<r>&bad;</r>", "<r>"] {
        let mut reader = Reader::new(input.as_bytes()).unwrap();
        let error = loop {
            match reader.read_event() {
                Err(error) => break error,
                Ok(Some(_)) => {}
                Ok(None) => panic!("invalid document reached EOF: {input:?}"),
            }
        };
        let position = reader.position();
        let range = reader.event_range();
        for _ in 0..3 {
            assert_eq!(reader.read_event().unwrap_err(), error);
            assert_eq!(reader.position(), position);
            assert_eq!(reader.event_range(), range);
        }
    }
}

#[test]
fn name_range_boundaries_and_references() {
    for c in [
        'A',
        '_',
        'é',
        '\u{c0}',
        '\u{d6}',
        '\u{d8}',
        '\u{f6}',
        '\u{f8}',
        '\u{2ff}',
        '\u{370}',
        '\u{37d}',
        '\u{37f}',
        '\u{1fff}',
        '\u{200c}',
        '\u{200d}',
        '\u{2070}',
        '\u{218f}',
        '\u{2c00}',
        '\u{2fef}',
        '\u{3001}',
        '\u{d7ff}',
        '\u{f900}',
        '\u{fdcf}',
        '\u{fdf0}',
        '\u{fffd}',
        '\u{10000}',
        '\u{effff}',
    ] {
        let xml = format!("<{c} a{c}='v'></{c}>");
        assert!(read(xml.as_bytes()).is_ok(), "U+{:X}", c as u32);
    }
    for c in [
        '0', '-', '.', '\u{b7}', '\u{300}', '\u{36f}', '\u{203f}', '\u{2040}',
    ] {
        assert!(read(format!("<a{c}/>").as_bytes()).is_ok());
        assert_eq!(
            read(format!("<{c}/>").as_bytes()).unwrap_err().kind,
            ErrorKind::InvalidName
        );
    }
    for c in [
        '\u{d7}',
        '\u{f7}',
        '\u{37e}',
        '\u{200b}',
        '\u{200e}',
        '\u{206f}',
        '\u{2190}',
        '\u{2bff}',
        '\u{2ff0}',
        '\u{3000}',
        '\u{f8ff}',
        '\u{fdd0}',
        '\u{f0000}',
    ] {
        assert_eq!(
            read(format!("<{c}/>").as_bytes()).unwrap_err().kind,
            ErrorKind::InvalidName
        );
    }
    for c in [
        '\t',
        '\n',
        '\r',
        ' ',
        '<',
        '&',
        '\u{7f}',
        '\u{d7ff}',
        '\u{e000}',
        '\u{fffd}',
        '\u{10000}',
        '\u{10ffff}',
    ] {
        for reference in [format!("&#{};", c as u32), format!("&#x{:X};", c as u32)] {
            let xml = format!("<r>{reference}</r>");
            let events = read(xml.as_bytes()).unwrap();
            assert!(matches!(&events[1],Event::Text(v) if v == &c.to_string()));
        }
    }
}

#[test]
fn unchanged_values_borrow_input() {
    let events = read(b"<r a='text'>plain<![CDATA[x&y]]><!--c--><?pi d?></r>").unwrap();
    assert!(
        matches!(&events[0],Event::Start(e) if matches!(e.attributes[0].value,Cow::Borrowed("text")))
    );
    assert!(matches!(&events[1], Event::Text(Cow::Borrowed("plain"))));
    assert!(matches!(&events[2], Event::Cdata(Cow::Borrowed("x&y"))));
    assert!(matches!(&events[3], Event::Comment(Cow::Borrowed("c"))));
    assert!(matches!(
        &events[4],
        Event::ProcessingInstruction {
            data: Cow::Borrowed("d"),
            ..
        }
    ));
}

#[test]
fn normalized_namespace_copies_share_one_cumulative_budget() {
    let input = b"<r xmlns:p='u&#x72;n'><p:c/><p:c/></r>";
    // 8 bytes for normalization, 3 each for the interner and binding, then 3 per name.
    let mut reader = Reader::with_limits(
        input,
        Limits {
            max_owned_bytes: 20,
            ..Limits::default()
        },
    )
    .unwrap();
    assert!(reader.read_event().unwrap().is_some());
    assert!(reader.read_event().unwrap().is_some());
    assert!(reader.read_event().unwrap().is_some());
    assert!(reader.read_event().unwrap().is_some());
    assert!(reader.read_event().unwrap().is_none());
    let mut reader = Reader::with_limits(
        input,
        Limits {
            max_owned_bytes: 19,
            ..Limits::default()
        },
    )
    .unwrap();
    reader.read_event().unwrap();
    reader.read_event().unwrap();
    assert_eq!(
        reader.read_event().unwrap_err().kind,
        ErrorKind::LimitExceeded(Limit::OwnedBytes)
    );
}

#[test]
fn namespace_interning_preserves_aliases_and_scope() {
    for input in [
        "<r xmlns:p='urn:a' xmlns:q='urn:a' p:a='1' q:a='2'/>",
        "<r xmlns:p='urn:&#x61;' xmlns:q='urn:a' p:a='1' q:a='2'/>",
        "<r xmlns:p='urn:a' xmlns:q='urn:&#x61;' p:a='1' q:a='2'/>",
        "<r xmlns:p='urn:a'><c xmlns:p='urn:b'/><c xmlns:q='urn:a' p:a='1' q:a='2'/></r>",
    ] {
        assert_eq!(
            read(input.as_bytes()).unwrap_err().kind,
            ErrorKind::DuplicateAttribute
        );
    }
    assert!(read(b"<r xmlns:p='urn:a' xmlns:q='urn:b' p:a='1' q:a='2'/>").is_ok());
    let input = b"<r xmlns:p='u&#x72;n' xmlns:q='urn'><p:a q:b=''/><q:a/></r>";
    let mut reader = Reader::with_limits(
        input,
        Limits {
            max_owned_bytes: 17,
            ..Limits::default()
        },
    )
    .unwrap();
    while reader.read_event().unwrap().is_some() {}
}

#[test]
fn long_namespace_uri_with_many_qualified_attributes() {
    let uri = format!("urn:{}", "a".repeat(128 * 1024));
    let mut input = format!("<r xmlns:p='{uri}' xmlns:q='{uri}'>");
    let mut tag = String::from("<c");
    for i in 0..256 {
        let prefix = if i % 2 == 0 { 'p' } else { 'q' };
        tag.push_str(&format!(" {prefix}:a{i}=''"));
    }
    tag.push_str("/>");
    input.push_str(&tag.repeat(32));
    input.push_str("</r>");
    let mut reader = Reader::with_limits(
        input.as_bytes(),
        Limits {
            max_owned_bytes: 0,
            ..Limits::default()
        },
    )
    .unwrap();
    let mut children = 0;
    while let Some(event) = reader.read_event().unwrap() {
        if let Event::Empty(element) = event {
            children += 1;
            assert_eq!(element.attributes.len(), 256);
            for attribute in element.attributes {
                assert_eq!(attribute.name.namespace(), Some(uri.as_str()));
            }
        }
    }
    assert_eq!(children, 32);
}
