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
    assert_eq!(reader.event_range(), None);
    let mut ranges = Vec::new();
    let mut events = Vec::new();
    while let Some(event) = reader.read_event().unwrap() {
        ranges.push(reader.event_range().expect("direct input event"));
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
            {
                let mut cfg = Limits::default();
                cfg.max_input_bytes = 3;
                cfg
            },
            Limit::InputBytes,
        ),
        (
            b"<r/>",
            {
                let mut cfg = Limits::default();
                cfg.max_token_bytes = 3;
                cfg
            },
            Limit::TokenBytes,
        ),
        (
            b"<r>abcd</r>",
            {
                let mut cfg = Limits::default();
                cfg.max_token_bytes = 3;
                cfg
            },
            Limit::TokenBytes,
        ),
        (
            b"<!--x--><r/>",
            {
                let mut cfg = Limits::default();
                cfg.max_token_bytes = 7;
                cfg
            },
            Limit::TokenBytes,
        ),
        (
            b"<r/>",
            {
                let mut cfg = Limits::default();
                cfg.max_depth = 0;
                cfg
            },
            Limit::Depth,
        ),
        (
            b"<r><c/></r>",
            {
                let mut cfg = Limits::default();
                cfg.max_depth = 1;
                cfg
            },
            Limit::Depth,
        ),
        (
            b"<r a='v'/>",
            {
                let mut cfg = Limits::default();
                cfg.max_attributes = 0;
                cfg
            },
            Limit::Attributes,
        ),
        (
            b"<r/>",
            {
                let mut cfg = Limits::default();
                cfg.max_events = 0;
                cfg
            },
            Limit::Events,
        ),
        (
            b"<r></r>",
            {
                let mut cfg = Limits::default();
                cfg.max_events = 1;
                cfg
            },
            Limit::Events,
        ),
        (
            b"<r>&amp;</r>",
            {
                let mut cfg = Limits::default();
                cfg.max_owned_bytes = 4;
                cfg
            },
            Limit::OwnedBytes,
        ),
        (
            b"<r>\r</r>",
            {
                let mut cfg = Limits::default();
                cfg.max_owned_bytes = 0;
                cfg
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
        (b"<r/>".as_slice(), {
            let mut cfg = Limits::default();
            cfg.max_input_bytes = 4;
            cfg.max_token_bytes = 4;
            cfg.max_depth = 1;
            cfg.max_attributes = 0;
            cfg.max_events = 1;
            cfg.max_owned_bytes = 0;
            cfg
        }),
        (b"<r a='v'/>", {
            let mut cfg = Limits::default();
            cfg.max_attributes = 1;
            cfg
        }),
        (b"<r>&amp;</r>", {
            let mut cfg = Limits::default();
            cfg.max_owned_bytes = 5;
            cfg
        }),
        (b"<r></r>", {
            let mut cfg = Limits::default();
            cfg.max_events = 2;
            cfg
        }),
        (b"<r>plain</r>", {
            let mut cfg = Limits::default();
            cfg.max_owned_bytes = 0;
            cfg
        }),
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
        let range = reader.event_range().expect("direct input event");
        for _ in 0..3 {
            assert_eq!(reader.read_event().unwrap_err(), error);
            assert_eq!(reader.position(), position);
            assert_eq!(reader.event_range().expect("direct input event"), range);
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
    let mut reader = Reader::with_limits(input, {
        let mut cfg = Limits::default();
        cfg.max_owned_bytes = 20;
        cfg
    })
    .unwrap();
    assert!(reader.read_event().unwrap().is_some());
    assert!(reader.read_event().unwrap().is_some());
    assert!(reader.read_event().unwrap().is_some());
    assert!(reader.read_event().unwrap().is_some());
    assert!(reader.read_event().unwrap().is_none());
    let mut reader = Reader::with_limits(input, {
        let mut cfg = Limits::default();
        cfg.max_owned_bytes = 19;
        cfg
    })
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
    let mut reader = Reader::with_limits(input, {
        let mut cfg = Limits::default();
        cfg.max_owned_bytes = 17;
        cfg
    })
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
    let mut reader = Reader::with_limits(input.as_bytes(), {
        let mut cfg = Limits::default();
        cfg.max_owned_bytes = 0;
        cfg
    })
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

#[test]
fn expanded_attribute_uniqueness_and_end_tags_on_both_paths() {
    let many = |extra: &str| {
        let attributes: String = (0..12).map(|n| format!(" a{n}='v'")).collect();
        format!("<r xmlns:p='u' xmlns:q='u'{attributes}{extra}/>")
    };
    let cases: Vec<(String, Option<ErrorKind>)> = vec![
        ("<r a='1' b='2'/>".into(), None),
        (
            "<r a='1' a='2'/>".into(),
            Some(ErrorKind::DuplicateAttribute),
        ),
        (
            "<r xmlns:p='u' xmlns:q='u' p:a='1' q:a='2'/>".into(),
            Some(ErrorKind::DuplicateAttribute),
        ),
        ("<r xmlns:p='u' xmlns:q='w' p:a='1' q:a='2'/>".into(), None),
        ("<r xmlns:p='u' p:a='1' a='2'/>".into(), None),
        (
            "<r a='1' a='2' z:b='3'/>".into(),
            Some(ErrorKind::DuplicateAttribute),
        ),
        (many(""), None),
        (
            many(" p:x='1' q:x='2'"),
            Some(ErrorKind::DuplicateAttribute),
        ),
        (many(" a3='1'"), Some(ErrorKind::DuplicateAttribute)),
        (many(" a3='1' z:b='2'"), Some(ErrorKind::DuplicateAttribute)),
        (many(" z:b='2' a3='1'"), Some(ErrorKind::Namespace)),
        (many(" z:b='2'"), Some(ErrorKind::Namespace)),
        (many(" prefixed_1='a' prefixed_2='b'"), None),
        (
            many(" prefixed_name='a' prefixed_name='b'"),
            Some(ErrorKind::DuplicateAttribute),
        ),
        (format!("<r a='{}&amp;'/>", "x".repeat(40)), None),
        (
            format!("<r a='{}<'/>", "x".repeat(40)),
            Some(ErrorKind::Syntax),
        ),
        ("<r a='x<y'/>".into(), Some(ErrorKind::Syntax)),
        ("<r a='x&bad;'/>".into(), Some(ErrorKind::UnknownEntity)),
        ("<p:r xmlns:p='u'></p:r \t\r\n>".into(), None),
        ("<r></r >".into(), None),
        ("<r></r x>".into(), Some(ErrorKind::Syntax)),
        ("<r></rr>".into(), Some(ErrorKind::MismatchedEndTag)),
        ("<r></1>".into(), Some(ErrorKind::InvalidName)),
    ];
    for (xml, expected) in cases {
        let mut reader = Reader::new(xml.as_bytes()).unwrap();
        let result = loop {
            match reader.read_event() {
                Ok(Some(_)) => {}
                Ok(None) => break None,
                Err(error) => break Some(error.kind),
            }
        };
        assert_eq!(result, expected, "{xml}");
    }
}

/// Scopes cross the prefix-index threshold in both directions: deep nesting
/// rebinding prefixes, then unwinding back to a few bindings, twice.
#[test]
fn namespace_scopes_resolve_across_index_threshold() {
    for root in [2, 20] {
        let mut xml = String::from("<r");
        for n in 0..root {
            xml.push_str(&format!(" xmlns:p{n}='urn:{n}'"));
        }
        xml.push('>');
        let mut expected = vec![None];
        for _ in 0..2 {
            for depth in 0..20 {
                xml.push_str(&format!(
                    "<p{depth}:e xmlns:p{depth}='urn:in{depth}' xmlns:q='urn:q{depth}'>"
                ));
                expected.push(Some(format!("urn:in{depth}")));
                xml.push_str("<q:x/>");
                expected.push(Some(format!("urn:q{depth}")));
            }
            for depth in (0..20).rev() {
                xml.push_str(&format!("</p{depth}:e>"));
                if depth < root {
                    xml.push_str(&format!("<p{depth}:after/>"));
                    expected.push(Some(format!("urn:{depth}")));
                }
            }
        }
        xml.push_str("<q:bad/></r>");
        for push in [false, true] {
            let mut seen = Vec::new();
            let mut record = |event: &Event<'_>| {
                if let Event::Start(element) | Event::Empty(element) = event {
                    seen.push(element.name.namespace().map(str::to_owned));
                }
            };
            let error = if push {
                let mut parser = yaxml::PushParser::new();
                parser
                    .feed(xml.as_bytes(), |event, _| {
                        record(event);
                        std::ops::ControlFlow::Continue(())
                    })
                    .unwrap_err()
            } else {
                let mut reader = Reader::new(xml.as_bytes()).unwrap();
                loop {
                    match reader.read_event() {
                        Ok(Some(event)) => record(&event),
                        Ok(None) => panic!("unbound q accepted"),
                        Err(error) => break error,
                    }
                }
            };
            assert_eq!(seen, expected, "root {root} push {push}");
            assert_eq!(error.kind, ErrorKind::Namespace);
        }
    }
}

#[test]
fn attribute_lists_compare_and_convert_between_owned_and_lent() {
    use yaxml::{Attribute, Attributes, Name};
    let attribute = |value: &'static str| Attribute {
        name: Name::new("a", None).unwrap(),
        value: Cow::Borrowed(value),
    };
    let storage = vec![attribute("1"), attribute("2")];
    let lent = Attributes::Borrowed(&storage);
    let owned: Attributes<'_> = storage.iter().cloned().collect();
    assert_eq!(lent, owned);
    assert_eq!(lent.len(), 2);
    assert_eq!((&lent).into_iter().count(), 2);
    assert_eq!(lent.clone().into_owned(), storage);
    let mut edited = lent.clone();
    edited.to_mut()[0].value = Cow::Borrowed("changed");
    assert!(matches!(edited, Attributes::Owned(_)));
    assert_ne!(edited, lent);
    assert_eq!(Attributes::default(), Attributes::from(Vec::new()));
    assert_eq!(
        owned.into_iter().map(|a| a.value).collect::<Vec<_>>(),
        ["1", "2"]
    );
}
