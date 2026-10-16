#![forbid(unsafe_code)]

#[path = "../fuzz/driver.rs"]
mod driver;

const SEEDS: &[&[u8]] = &[
    include_bytes!("../fuzz/corpus/minimal.xml"),
    include_bytes!("../fuzz/corpus/namespaces.xml"),
    include_bytes!("../fuzz/corpus/lexical.xml"),
    include_bytes!("../fuzz/corpus/references.xml"),
    include_bytes!("../fuzz/corpus/malformed.xml"),
    include_bytes!("../fuzz/corpus/dtd.xml"),
    b"",
    b"\xff\xfe\x00\xd8",
];

#[test]
fn replay_corpus_and_every_truncation() {
    for seed in SEEDS {
        for end in 0..=seed.len() {
            driver::exercise(&seed[..end]);
        }
    }
}

#[test]
fn mutate_each_seed_byte() {
    for seed in SEEDS {
        for offset in 0..seed.len() {
            for byte in [0, 0xff, b'<', b'>', b'&', b';', b'\'', b'"', b':', b'\r'] {
                let mut mutated = seed.to_vec();
                mutated[offset] = byte;
                driver::exercise(&mutated);
            }
            let mut deleted = seed.to_vec();
            deleted.remove(offset);
            driver::exercise(&deleted);
        }
    }
}

#[test]
fn deterministic_arbitrary_bytes() {
    let mut state = 0x4d59_5df4_d0f3_3173_u64;
    for len in 0..=256 {
        let mut input = Vec::with_capacity(len);
        for _ in 0..len {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            input.push(state as u8);
        }
        driver::exercise(&input);
    }
}

#[test]
fn generated_valid_documents() {
    for depth in 1..=64 {
        let mut input = String::from("<?xml version='1.0'?><r xmlns='urn:r' xmlns:p='urn:p'>");
        for index in 0..depth {
            input.push_str(&format!("<p:n a='{index}' p:a='&#xA;'>é&amp;中"));
        }
        for _ in 0..depth {
            input.push_str("</p:n>");
        }
        input.push_str("</r>");
        let outcome = driver::exercise(input.as_bytes());
        assert!(
            outcome.error.is_none(),
            "generated document rejected: {:?}",
            outcome.error
        );
    }
}

#[test]
fn known_acceptance() {
    for (input, accepted) in [
        (b"<r/>".as_slice(), true),
        (b"<r>text</r>", true),
        (b"<r>", false),
        (b"<r a='1' a='2'/>", false),
        (b"<r>&missing;</r>", false),
        (b"<!DOCTYPE r><r/>", false),
        (b"<p:r/>", false),
        (b"\xff", false),
    ] {
        assert_eq!(
            driver::exercise(input).error.is_none(),
            accepted,
            "input: {input:?}"
        );
    }
}

#[test]
fn each_resource_budget() {
    use yaxml::{ErrorKind, Limit, Limits};
    let default = Limits::default();
    for (input, limits, expected) in [
        (
            b"<r/>".as_slice(),
            Limits {
                max_input_bytes: 3,
                ..default
            },
            Limit::InputBytes,
        ),
        (
            b"<r/>",
            Limits {
                max_token_bytes: 3,
                ..default
            },
            Limit::TokenBytes,
        ),
        (
            b"<r><c/></r>",
            Limits {
                max_depth: 1,
                ..default
            },
            Limit::Depth,
        ),
        (
            b"<r a='1'/>",
            Limits {
                max_attributes: 0,
                ..default
            },
            Limit::Attributes,
        ),
        (
            b"<r>x</r>",
            Limits {
                max_events: 1,
                ..default
            },
            Limit::Events,
        ),
        (
            b"<r>&amp;</r>",
            Limits {
                max_owned_bytes: 0,
                ..default
            },
            Limit::OwnedBytes,
        ),
    ] {
        assert_eq!(
            driver::exercise_with_limits(input, limits)
                .error
                .unwrap()
                .kind,
            ErrorKind::LimitExceeded(expected)
        );
        assert!(driver::exercise_with_limits(input, default).error.is_none());
    }
}

#[test]
fn generated_name_boundaries() {
    for scalar in [
        0xc0, 0xd6, 0xd8, 0xf6, 0xf8, 0x2ff, 0x370, 0x37d, 0x37f, 0x1fff, 0x200c, 0x200d, 0x2070,
        0x218f, 0x2c00, 0x2fef, 0x3001, 0xd7ff, 0xf900, 0xfdcf, 0xfdf0, 0xfffd, 0x10000, 0xeffff,
    ] {
        let name = char::from_u32(scalar).unwrap();
        let input = format!("<{name} a{name}='v'/>");
        assert!(
            driver::exercise(input.as_bytes()).error.is_none(),
            "valid name U+{scalar:X} rejected"
        );
    }
}

#[test]
fn generated_reference_semantics() {
    use yaxml::Document;
    for scalar in [
        9, 10, 13, 0x20, 0x7f, 0xd7ff, 0xe000, 0xfffd, 0x10000, 0x10ffff,
    ] {
        let input = format!("<r a='&#{scalar};'>&#x{scalar:X};</r>");
        assert!(driver::exercise(input.as_bytes()).error.is_none());
        let document = Document::parse(&input).unwrap();
        let expected = char::from_u32(scalar).unwrap().to_string();
        assert_eq!(document.root_element().attributes()[0].value, expected);
        assert_eq!(
            document.root_element().first_child().unwrap().text(),
            Some(expected.as_str())
        );
    }
    for scalar in [0, 1, 8, 11, 12, 0xd800, 0xdfff, 0xfffe, 0xffff, 0x110000] {
        let input = format!("<r>&#x{scalar:X};</r>");
        assert!(
            driver::exercise(input.as_bytes()).error.is_some(),
            "illegal reference U+{scalar:X} accepted"
        );
    }
}

fn stream(input: &[u8]) -> Vec<yaxml::Event<'_>> {
    let mut reader = yaxml::Reader::new(input).unwrap();
    let mut events = Vec::new();
    while let Some(event) = reader.read_event().unwrap() {
        events.push(event);
    }
    events
}

#[test]
fn projection_preserves_all_payload_kinds() {
    let input = b"<?xml version='1.0' standalone='yes'?><!--a--><?pi a?><r xmlns:p='urn:p' p:a='&amp;'>a<![CDATA[b]]><p:c/>c</r><!--z-->";
    assert!(driver::exercise(input).error.is_none());
}

#[test]
#[should_panic(expected = "DOM payload differs from stream")]
fn projection_detects_changed_payload() {
    let document = yaxml::Document::parse("<r>right</r>").unwrap();
    driver::check_projection(&document, &stream(b"<r>wrong</r>"));
}

#[test]
#[should_panic(expected = "element names or attributes differ from stream")]
fn projection_detects_changed_expanded_name() {
    let document = yaxml::Document::parse("<r xmlns='urn:one'/>").unwrap();
    driver::check_projection(&document, &stream(b"<r xmlns='urn:two'/>"));
}

#[test]
#[should_panic(expected = "element names or attributes differ from stream")]
fn projection_detects_reordered_attributes() {
    let document = yaxml::Document::parse("<r a='1' b='2'/>").unwrap();
    driver::check_projection(&document, &stream(b"<r b='2' a='1'/>"));
}

#[test]
fn grammar_regression_corpus() {
    use yaxml::ErrorKind::*;
    for (input, expected) in [
        (
            "\u{feff}<?xml version='1.0' encoding='utf-8' standalone='no'?><r/>",
            None,
        ),
        ("<r a='\t\n\r\n&#xA;'><!--a\r\nb--><?pi a\rb?></r>", None),
        (
            "<r xmlns:xml='http://www.w3.org/XML/1998/namespace' xml:lang='en'/>",
            None,
        ),
        (
            "<r xmlns:p='urn:&#x61;' xmlns:q='urn:a'><p:c q:a='1'/></r>",
            None,
        ),
        ("<r xmlns='u'><c xmlns=''/></r>", None),
        ("<r/>x", Some(TextOutsideRoot)),
        ("<![CDATA[x]]><r/>", Some(TextOutsideRoot)),
        ("<r>]]></r>", Some(Syntax)),
        ("<r><!--a--b--></r>", Some(Syntax)),
        ("<r><!--a---></r>", Some(Syntax)),
        ("<r>\n&amp;\t</r>", None),
        ("<?xml?><r/>", Some(InvalidDeclaration)),
        ("<?XML version='1.0'?><r/>", Some(InvalidDeclaration)),
        (" <?xml version='1.0'?><r/>", Some(InvalidDeclaration)),
        (
            "<?xml version='1.0' version='1.0'?><r/>",
            Some(InvalidDeclaration),
        ),
        ("<?xml standalone='yes'?><r/>", Some(InvalidDeclaration)),
        (
            "<?xml version='1.0' standalone='yes' standalone='no'?><r/>",
            Some(InvalidDeclaration),
        ),
        (
            "<?xml version='1.0' standalone='other'?><r/>",
            Some(InvalidDeclaration),
        ),
        (
            "<?xml version='1.0' encoding='UTF-16'?><r/>",
            Some(UnsupportedEncoding),
        ),
        (
            "<?xml version='1.0' standalone='no' encoding='UTF-8'?><r/>",
            Some(InvalidDeclaration),
        ),
        (
            "<r xmlns:p='u' xmlns:q='u' p:a='' q:a=''/>",
            Some(DuplicateAttribute),
        ),
        ("<r xmlns:xml='u'/>", Some(Namespace)),
        ("<r xmlns:p=''/>", Some(Namespace)),
        (
            "<r xmlns='http://www.w3.org/XML/1998/namespace'/>",
            Some(Namespace),
        ),
        ("<r xmlns:xmlns='u'/>", Some(Namespace)),
        (
            "<r xmlns='http://www.w3.org/2000/xmlns/'/>",
            Some(Namespace),
        ),
        ("<xmlns:r/>", Some(Namespace)),
        ("<r a='v'b='v'/>", Some(Syntax)),
        ("<r></r x>", Some(Syntax)),
        ("<r a:b:c=''/>", Some(InvalidName)),
        ("<r a\u{2190}=''/>", Some(InvalidName)),
        ("<r>\u{fffe}</r>", Some(InvalidCharacter)),
    ] {
        assert_eq!(
            driver::exercise(input.as_bytes()).error.map(|e| e.kind),
            expected,
            "{input:?}"
        );
    }
    for input in [b"\xfe\xff\0<".as_slice(), b"\0\0\xfe\xff", b"\0<\0?"] {
        assert_eq!(
            driver::exercise(input).error.unwrap().kind,
            UnsupportedEncoding
        );
    }
    for (input, ceiling) in [
        (b"<!--x--><r/>".as_slice(), 3),
        (b"<r><!--abcdefgh--></r>", 8),
    ] {
        assert_eq!(
            driver::exercise_with_limits(
                input,
                yaxml::Limits {
                    max_token_bytes: ceiling,
                    ..yaxml::Limits::default()
                }
            )
            .error
            .unwrap()
            .kind,
            LimitExceeded(yaxml::Limit::TokenBytes)
        );
    }
}
