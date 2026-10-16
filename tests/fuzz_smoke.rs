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
    include_bytes!("../fuzz/corpus/declaration.xml"),
    include_bytes!("../fuzz/corpus/encoding-name.xml"),
    include_bytes!("../fuzz/corpus/infoset.xml"),
    include_bytes!("../fuzz/corpus/utf16le-bom.xml"),
    include_bytes!("../fuzz/corpus/utf16be-bom.xml"),
    include_bytes!("../fuzz/corpus/utf16le-declaration.xml"),
    include_bytes!("../fuzz/corpus/utf16be-declaration.xml"),
    include_bytes!("../fuzz/corpus/utf16-malformed.xml"),
    b"",
    b"\xff\xfe\x00\xd8",
];

#[test]
fn streaming_lifecycle_operation_selectors() {
    // Middle-byte parity selects finished versus newly poisoned parser states;
    // the declaration also selects the invalid Read byte-count action.
    for (input, accepted) in [
        (b"<r/>".as_slice(), true),
        (b"<r>x</r>", true),
        (b"<r>xy</r>", true),
        (b"<r>xyz</r>", true),
        (b"<?xml version='1.0'?><r/>", true),
        (b"<r></s>", false),
        (b"<r>\xff</r>", false),
    ] {
        assert_eq!(driver::exercise(input).error.is_none(), accepted);
    }
}

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
fn writer_operation_and_output_selectors() {
    for control in 0..=255 {
        for next in [0, 1, 2, 3, 4, 5, 6, 255] {
            driver::exercise(&[control, next, 255, 7, 3, 255, 255, 1, control]);
        }
    }
    for payload in [
        b"".as_slice(),
        b"<>&\"\r\n\t".as_slice(),
        b"?>",
        b"]]>",
        b"--",
        b"\0",
        b"plain",
        b"r a='injected'",
        b"a='injected' b",
        b"p data",
        b"1.0\" standalone=\"yes",
        b"a-",
    ] {
        for operations in [
            &[1][..],
            &[0, 3, 2],
            &[0, 4, 2],
            &[0, 5, 2],
            &[0, 6, 2],
            &[7, 1],
            &[15, 1],
            &[14, 1],
            &[1, 1],
            &[0, 0, 2, 2],
            &[0x41],
            &[0x80, 2],
            &[0x40, 0x42],
        ] {
            for policy in [0, 1] {
                let mut input = vec![policy, 255, 255, 7, 3, 255, 255, operations.len() as u8];
                input.extend_from_slice(operations);
                input.extend_from_slice(payload);
                driver::exercise(&input);
            }
        }
    }
    // Isolate each writer policy byte with enough operations to reach it.
    for field in 1..=6 {
        for value in 0..=8 {
            let mut input = vec![0, 255, 255, 7, 3, 255, 255, 3, 0, 3, 2];
            input[field] = value;
            input.extend_from_slice(b"<&>\"\r\n\t");
            driver::exercise(&input);
        }
    }
}

#[test]
fn shared_namespace_values_compare_without_owned_snapshots() {
    // A small source can expose the same URI through many borrowed names.
    // Lockstep comparison avoids cloning that URI once per attribute/event.
    let mut input = format!("<r xmlns:p='{}'>", "u".repeat(16 * 1024));
    for _ in 0..4 {
        input.push_str("<p:n");
        for index in 0..128 {
            input.push_str(&format!(" p:a{index}='v'"));
        }
        input.push_str("/>");
    }
    input.push_str("</r>");
    assert!(driver::exercise(input.as_bytes()).error.is_none());
}

#[test]
fn owned_storage_growth_and_aggregate_limits() {
    assert!(
        driver::exercise(
            "<é:r xmlns:é='urn:ü' é:a=''><![CDATA[]]><!----><?空?><é:c/></é:r>".as_bytes()
        )
        .error
        .is_none()
    );
    let mut input = String::from("<?xml version='1.0' encoding='UTF-8'?><r xmlns:p='urn:é'>");
    for index in 0..128 {
        input.push_str(&format!(
            "<p:n a='{index}&#9;'>é中&amp;<![CDATA[🙂]]><?pi x\ry?></p:n>"
        ));
    }
    input.push_str("</r>");
    assert!(driver::exercise(input.as_bytes()).error.is_none());
    for input in [
        b"<r/>".as_slice(),
        b"<r>a</r>",
        b"<r>&amp;</r>",
        b"<r a='&#9;'/>",
    ] {
        for maximum in 0..=80 {
            driver::exercise_with_limits(
                input,
                yaxml::Limits {
                    max_owned_bytes: maximum,
                    ..yaxml::Limits::default()
                },
            );
        }
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
        ("<?xml version='1.1'?><r/>", None),
        ("<?xml version='1.234'?><r/>", None),
        ("<?xml version='1.00'?><r/>", None),
        ("<?xml version='1.'?><r/>", Some(InvalidDeclaration)),
        ("<?xml version='1.١'?><r/>", Some(InvalidDeclaration)),
        ("<?xml version='1.1'?><r>&#x1;</r>", Some(InvalidReference)),
        (
            "<?xml version='1.0' encoding=''?><r/>",
            Some(InvalidDeclaration),
        ),
        (
            "<?xml version='1.0' encoding='8UTF'?><r/>",
            Some(InvalidDeclaration),
        ),
        (
            "<?xml version='1.0' encoding='UTF 8'?><r/>",
            Some(InvalidDeclaration),
        ),
        (
            "<?xml version='1.0' encoding='UTé'?><r/>",
            Some(InvalidDeclaration),
        ),
        (
            "<?xml version='1.0' encoding='x.Custom_1-2'?><r/>",
            Some(UnsupportedEncoding),
        ),
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
    assert_eq!(
        driver::exercise(b"\0\0\xfe\xff").error.unwrap().kind,
        UnsupportedEncoding
    );
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

fn utf16(input: &str, little: bool, bom: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    if bom {
        bytes.extend_from_slice(if little { &[0xff, 0xfe] } else { &[0xfe, 0xff] });
    }
    for unit in input.encode_utf16() {
        bytes.extend_from_slice(&if little {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}

#[test]
fn encoded_equivalents_and_every_truncation() {
    for little in [false, true] {
        for bom in [false, true] {
            let label = if little { "uTf-16LE" } else { "UTF-16BE" };
            for body in [
                "<r/>",
                "<!--é--><?空 x\ry?><é:r xmlns:é='urn:中' é:a='&#9;'>é中🙂\r\n&amp;<![CDATA[x\ry]]><é:c/></é:r><!--z-->",
                "<r><![CDATA[]]><!----><?p?><a a=''/></r>",
            ] {
                let source =
                    format!("<?xml version='1.0' encoding='{label}' standalone='no'?>{body}");
                let bytes = utf16(&source, little, bom);
                assert!(driver::exercise(&bytes).error.is_none());
                for end in 0..bytes.len() {
                    driver::exercise(&bytes[..end]);
                }
            }
        }
        let source = utf16("<r>é中🙂</r>", little, true);
        for offset in 0..source.len() {
            for replacement in [0, 0xff, 0xd8, 0xdc, b'<', b'&'] {
                let mut mutated = source.clone();
                mutated[offset] = replacement;
                driver::exercise(&mutated);
            }
        }
        for input in [
            "<?xml version='1.0' encoding='UTF-8'?><r/>",
            "<?xml version='1.0' encoding='unknown'?><r/>",
            "<?xml version='1.0' encoding='UTF-16'?><r/>",
            "<?xml version='1.0' encoding='UTF-16' encoding='UTF-16'?><r/>",
            "\u{feff}<r/>",
            "<r>🙂&unknown;</r>",
            "<r>é中</wrong>",
            "<r a='x' a='y'/>",
        ] {
            driver::exercise(&utf16(input, little, true));
            // A missing-BOM UTF-16 declaration is not the endian label this
            // library requires, and signatures only select a leading <?xml.
            if input.starts_with("<?xml") {
                driver::exercise(&utf16(input, little, false));
            }
        }
        for maximum in 0..=80 {
            driver::exercise_with_limits(
                &utf16("<r>&amp;</r>", little, true),
                yaxml::Limits {
                    max_owned_bytes: maximum,
                    ..yaxml::Limits::default()
                },
            );
        }
    }
}
