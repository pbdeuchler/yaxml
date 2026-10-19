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
    include_bytes!("../fuzz/corpus/dtd-entities.xml"),
    include_bytes!("../fuzz/corpus/dtd-unread.xml"),
    include_bytes!("../fuzz/corpus/declaration.xml"),
    include_bytes!("../fuzz/corpus/encoding-name.xml"),
    include_bytes!("../fuzz/corpus/infoset.xml"),
    include_bytes!("../fuzz/corpus/utf16le-bom.xml"),
    include_bytes!("../fuzz/corpus/utf16be-bom.xml"),
    include_bytes!("../fuzz/corpus/utf16le-declaration.xml"),
    include_bytes!("../fuzz/corpus/utf16be-declaration.xml"),
    include_bytes!("../fuzz/corpus/utf16-malformed.xml"),
    include_bytes!("../fuzz/corpus/charset-alias.xml"),
    include_bytes!("../fuzz/corpus/charset-unmapped.xml"),
    include_bytes!("../fuzz/corpus/pull-lookahead.xml"),
    include_bytes!("../fuzz/corpus/serialize.xml"),
    include_bytes!("../fuzz/corpus/mutation-operations.bin"),
    include_bytes!("../fuzz/corpus/mutation-quotas.bin"),
    include_bytes!("../fuzz/corpus/structure-operations.bin"),
    include_bytes!("../fuzz/corpus/construction-controls.bin"),
    include_bytes!("../fuzz/corpus/construction-binding-limit.bin"),
    include_bytes!("../fuzz/corpus/construction-format.bin"),
    include_bytes!("../fuzz/corpus/construction-long-prefix.bin"),
    include_bytes!("../fuzz/corpus/name-owned-views.txt"),
    include_bytes!("../fuzz/corpus/uri.txt"),
    b"",
    b"\xff\xfe\x00\xd8",
];

fn mutation_input(header: [u8; 7], operations: &[[u8; 8]], payload: &[u8]) -> Vec<u8> {
    assert!(operations.len() <= 31);
    let mut input = header.to_vec();
    input.push(operations.len() as u8);
    for operation in operations {
        input.extend_from_slice(operation);
    }
    input.extend_from_slice(payload);
    input
}

#[test]
fn construction_model_covers_prefix_catalogs_and_encoded_parents() {
    for root_case in 0..6 {
        for encoding in 0..3 {
            driver::exercise_construction(&[
                root_case,
                encoding,
                (root_case + encoding) % 4,
                (root_case * 3 + encoding) % 8,
                0x04,
                0x7f,
                0x80,
                0xff,
            ]);
        }
    }
}

#[test]
fn construction_model_checks_exact_rejection_catalog() {
    for rejection in 0..8 {
        driver::exercise_construction(&[0, rejection % 3, rejection % 4, rejection]);
    }
}

#[test]
fn construction_model_derives_bounded_names_values_and_operations() {
    for input in [
        b"".as_slice(),
        b"\x00",
        b"<root attr='value'/>",
        b"\xff\xfe\x00\xd8",
        b"repeated-repeated-repeated",
    ] {
        driver::exercise_construction(input);
    }
}

#[test]
fn construction_model_reuses_long_inherited_prefixes_with_exact_bounds() {
    for encoding in 0..3 {
        for length in [0, 255] {
            driver::exercise_construction(&[0x2b, length, encoding, encoding]);
        }
    }
}

#[test]
fn mutation_operations_predict_results_and_preserve_complete_trees() {
    for encoding in 0..4 {
        for family in 0..2 {
            for variant in 0..16 {
                for derived in [0, 1] {
                    let mut operations = Vec::new();
                    for node in [0, 1, 2, 3, 4, 5, 6, 8, 9] {
                        operations.push([0, 0, node, 0, 0, 255, variant, 31]);
                    }
                    for selector in [0, 1, 2, 6, 7] {
                        operations.push([1, 0, 1, selector, 0, 255, variant, 31]);
                    }
                    operations.extend([
                        [0, 1, 12, 0, 0, 255, 12, 0],
                        [1, 1, 11, 2, 0, 255, 2, 0],
                        [6, 0, 2, 0, 0, 255, variant, 0],
                        [7, 0, 1, 0, 0, 255, variant, 0],
                        [5, 0, 0, 0, 0, 0, 0, 0],
                        [2, 1, 2, 0, 0, 0, 1, 19],
                        [0, 1, 19, 0, 0, 255, 12, 0],
                        [4, 1, 0, 0, 0, 0, 0, 0],
                        [3, 1, 0, 0, 0, 0, 0, 0],
                    ]);
                    driver::exercise_mutation(&mutation_input(
                        [family, encoding, derived, variant, variant, derived, 1],
                        &operations,
                        b"logical payload <>&\"\r\n\t",
                    ));
                }
            }
            for selector in 0..12 {
                let operations: Vec<_> = [0, 1, 2, 6, 7]
                    .map(|node| [1, 0, node, selector, 0, 255, 12, 0])
                    .into();
                driver::exercise_mutation(&mutation_input(
                    [family, encoding, 0, 0, 6, selector, 1],
                    &operations,
                    b"value",
                ));
            }
        }
    }
}

#[test]
fn mutation_value_boundaries_and_error_indices_are_independent() {
    for payload in [
        "",
        "é",
        "🙂",
        "é\0",
        "🙂]]>",
        "é--",
        "é-",
        "🙂?>",
        "é\r",
        "\tdata",
        "\ndata",
        " data",
        "]]>\0",
        " \u{ffff}",
        "é\u{fffe}",
        "\r--?>",
        "\u{1}",
        "\u{b}",
        "\u{c}",
        "\u{d7ff}",
        "\u{e000}",
        "\u{fffd}",
        "\u{10000}",
        "\u{10ffff}",
        "\u{85}",
        "\u{a0}",
        "\u{2003}",
        "\u{feff}",
        "&<>\"\r\n\t",
    ] {
        for encoding in 0..4 {
            for maximum in [
                payload.len().saturating_sub(1),
                payload.len(),
                payload.len() + 1,
            ] {
                let operations: Vec<_> = [2, 3, 4, 5]
                    .map(|node| [0, 0, node, 0, 0, 255, 0, 0])
                    .into_iter()
                    .chain(std::iter::once([1, 0, 1, 0, 0, 255, 0, 0]))
                    .collect();
                driver::exercise_mutation(&mutation_input(
                    [0, encoding, 1, maximum as u8, 5, 0, 1],
                    &operations,
                    payload.as_bytes(),
                ));
            }
        }
    }
}

#[test]
fn mutation_literal_quota_sequences_cover_reopened_and_encoded_trees() {
    for encoding in 0..4 {
        for case in 0..8 {
            for boundary in 0..3 {
                for lifecycle in [0, 2] {
                    driver::exercise_mutation(&mutation_input(
                        [0, encoding, 0, 0, case, boundary, lifecycle],
                        &[],
                        b"",
                    ));
                }
            }
        }
        driver::exercise_mutation(&mutation_input([0, encoding, 0, 0, b'*', 0, 0], &[], b""));
    }
}

#[test]
fn structural_operations_cover_live_stale_foreign_and_protected_handles() {
    for encoding in 0..4 {
        for family in 0..2 {
            for variant in 0..16 {
                let operations = [
                    [8, 0, 1, 0, 0, 255, variant, 0],
                    [9, 0, 7, 0, 0, 255, variant, 0],
                    [10, 0, 0, 0, 0, 255, variant, 0],
                    [11, 0, 0, 12, 0, 255, 1, 0],
                    [11, 0, 1, variant, 0, 255, 1, 0],
                    [12, 0, 0, 0, 0, 0, 0, 0],
                    [12, 0, 1, 0, 0, 0, 0, 0],
                    [12, 0, 6, 0, 0, 0, 0, 0],
                    [12, 0, 6, 0, 0, 0, 0, 0],
                    [0, 0, 7, 0, 0, 255, 12, 0],
                    [1, 0, 6, 0, 0, 255, 12, 0],
                    [8, 0, 6, 0, 0, 255, 12, 0],
                    [2, 0, 7, 0, 0, 0, 0, 0],
                    [8, 0, 0, 0, 0, 255, 12, 0],
                    [9, 0, 5, 0, 0, 255, 12, 0],
                    [15, 0, 1, variant, variant, 255, 12, 0],
                    [13, 0, 12, 0, 0, 0, 0, 0],
                    [12, 0, 22, 0, 0, 0, 0, 0], // Mandatory-created stale Comment.
                    [5, 0, 0, 0, 0, 0, 0, 0],
                    [8, 1, 1, 0, 0, 255, 12, 0],
                    [14, 1, 0, 0, 0, 0, variant, 0],
                    [4, 1, 0, 0, 0, 0, 0, 0],
                    [3, 1, 0, 0, 0, 0, 0, 0],
                ];
                driver::exercise_mutation(&mutation_input(
                    [family, encoding, 0, 0, variant * 8, variant, 1],
                    &operations,
                    b"payload <>&\r\n\t",
                ));
            }
        }
    }
}

#[test]
fn structural_text_projection_and_independent_quota_profiles() {
    for encoding in 0..4 {
        for case in 0..8 {
            for boundary in 0..3 {
                driver::exercise_mutation(&mutation_input(
                    [0, encoding, 0, 0, case * 8, boundary, 0],
                    &[
                        [12, 0, 3, 0, 0, 0, 0, 0],
                        [12, 0, 4, 0, 0, 0, 0, 0],
                        [12, 0, 5, 0, 0, 0, 0, 0],
                        [12, 0, 6, 0, 0, 0, 0, 0],
                        [12, 0, 21, 0, 0, 0, 0, 0], // Mandatory-created CDATA.
                        [12, 0, 23, 0, 0, 0, 0, 0], // Mandatory-created PI.
                        [8, 0, 1, 0, 0, 255, 1, 0], // Empty Text between Text nodes.
                        [8, 0, 1, 0, 0, 255, 12, 0],
                        [9, 0, 1, 0, 0, 255, 1, 0], // Empty CDATA stays distinct.
                        [9, 0, 1, 0, 0, 255, 12, 0],
                        [3, 0, 0, 0, 0, 0, 0, 0],
                    ],
                    b"",
                ));
            }
        }
        for maximum in [0, 4, 5, 6, 7] {
            driver::exercise_mutation(&mutation_input(
                [0, encoding, 2, 0, 0, maximum, 0],
                &[
                    [8, 0, 1, 0, 0, 255, 12, 0],
                    [12, 0, 20, 0, 0, 0, 0, 0],
                    [8, 0, 1, 0, 0, 255, 1, 0],
                    [14, 0, 0, 0, 0, 0, 2, 0],
                    [0, 0, 2, 0, 0, 255, 12, 0],
                ],
                b"",
            ));
        }
    }
}

#[test]
fn structural_pi_name_boundaries_and_each_input_length_are_independent() {
    for target in [
        "",
        "1p",
        "p:x",
        "a b",
        "é🙂!",
        "xml",
        "XmL",
        "é\0",
        "1\0",
        "é🙂",
        "xml-stylesheet",
        "a\u{b7}",
        "\u{300}",
        "a\u{300}",
        "\u{37d}",
        "\u{37e}",
        "\u{37f}",
        "\u{effff}",
        "\u{f0000}",
    ] {
        for encoding in 0..4 {
            for maximum in [
                target.len().saturating_sub(1),
                target.len(),
                target.len() + 1,
            ] {
                driver::exercise_mutation(&mutation_input(
                    [0, encoding, 1, maximum as u8, 56, 0, 0],
                    &[[11, 0, 1, 0, 0, 255, 1, 0]],
                    target.as_bytes(),
                ));
            }
        }
    }
}

#[test]
fn document_serialization_semantics_and_independent_quotas() {
    for body in [
        "<r/>",
        "<r></r>",
        "<r>a</r>",
        "<r>&amp;</r>",
        "<r a='&#9;'/>",
        "<r><![CDATA[]]></r>",
        "<r><a/><b/></r>",
        "<r><a>x</a><a>x</a></r>",
        "<!--before--><?p?><r xmlns:p='urn:p' xmlns:q='urn:unused' a='q:T'><p:c/>x<![CDATA[]]><!----><c xmlns='urn:d'><c xmlns=''/></c></r><!--after-->",
        "<é:r xmlns:é='urn:p' é:a='&#9;&#10;&#13;&quot;&amp;&lt;'>🙂&#13;<![CDATA[中]]><?空 data?></é:r>",
    ] {
        assert!(driver::exercise(body.as_bytes()).error.is_none());
        for little in [false, true] {
            assert!(driver::exercise(&utf16(body, little, true)).error.is_none());
            let declared =
                format!("<?xml version='1.00' encoding='UTF-16' standalone='no'?>{body}");
            assert!(
                driver::exercise(&utf16(&declared, little, true))
                    .error
                    .is_none()
            );
        }
    }
    for source in [
        "<?xml version='1.0' encoding='uTf-8' standalone='yes'?><r/>",
        "\u{feff} \n<r><![CDATA[]]><!--x--><?p?></r>\t",
    ] {
        assert!(driver::exercise(source.as_bytes()).error.is_none());
    }
}

#[test]
fn pull_schedules_and_independent_policy_boundaries() {
    for control in 0..=255 {
        for input in [
            b"<r/>".as_slice(),
            b"<r>a</r>",
            b"<r>&amp;</r>",
            b"<r a='&#9;'/>",
            b"<r>a<c/>b</r> ",
            b"<r xmlns:p='u'><p:c/><c xmlns='v'/></r>",
            b"<r>unterminated",
            b"<r a='unterminated",
            b"<r></wrong>",
        ] {
            driver::exercise_pull(input, control);
        }
    }
    for little in [false, true] {
        for input in [
            "<r/>",
            "<r>a</r>",
            "<r>&amp;</r>",
            "<r a='&#9;'/>",
            "<r>🙂<c/>é</r>",
        ] {
            let input = utf16(input, little, true);
            for control in [0, 1, 2, 3, 7, 64, 128, 254, 255] {
                driver::exercise_pull(&input, control);
            }
        }
    }
}

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
        (b"<!DOCTYPE r><r/>", true),
        (b"<!DOCTYPE r [<!ENTITY e '&lt;n/>'>]><r>&e;</r>", true),
        (b"<!DOCTYPE r [<!ENTITY e '&e;'>]><r>&e;</r>", false),
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
fn bomless_utf16_requires_an_encoding_declaration() {
    for little_endian in [false, true] {
        let input: Vec<_> = "<?xml version='1.0'?><r/>"
            .encode_utf16()
            .flat_map(|unit| {
                if little_endian {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                }
            })
            .collect();
        assert_eq!(
            driver::exercise(&input).error,
            Some(yaxml::Error {
                kind: yaxml::ErrorKind::EncodingMismatch,
                offset: 0,
            })
        );
    }
}

#[test]
fn each_resource_budget() {
    use yaxml::{Error, ErrorKind, Limit, Limits};
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
            b"    <r/>",
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
    assert_eq!(
        driver::exercise_with_limits(
            b"    <r/>",
            Limits {
                max_token_bytes: 3,
                ..default
            },
        )
        .error,
        Some(Error {
            kind: ErrorKind::LimitExceeded(Limit::TokenBytes),
            offset: 0,
        }),
        "pre-root whitespace quota lost its source start"
    );
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
            "<!é",
            "<!🙂>",
            "<r><!é></r>",
        ] {
            driver::exercise(&utf16(input, little, true));
            // A missing-BOM UTF-16 declaration is not the endian label this
            // library requires, and signatures only select a leading <?xml.
            if input.starts_with("<?xml") {
                driver::exercise(&utf16(input, little, false));
            }
        }
        for body in ["<r/>", "<r>a</r>", "<r>&amp;</r>", "<r a='&#9;'/>"] {
            assert!(driver::exercise(&utf16(body, little, true)).error.is_none());
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

#[test]
fn xpath_entry_replays_expressions_and_documents() {
    let cases: &[&[u8]] = &[
        b"//*[@id]/@*",
        b"count(//node()) + sum(//text())",
        b"//p:c/namespace::* | //e/..",
        b"string(/r/a[1]/following::node()[last()])",
        b"id('i')/preceding::node()",
        b"concat(name(//*[lang('EN')][1]), substring('abc', 2), translate('x', 'x', 'y'))",
        b"//a[",
        b"((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((((1))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))))",
        b"//*[count(//*) > 0][1]\0<r><s><t/></s><s/></r>",
        b"//x\0<r",
        b"\xff\xfe//*",
        b"",
    ];
    for case in cases {
        driver::exercise_xpath(case);
    }
}

#[test]
fn c14n_entry_replays_modes_and_node_sets() {
    let document = b"<?p d?><!--c--><r xmlns='urn:d' xmlns:p='urn:p' xml:lang='en' xml:base='http://h/a/'><p:s p:a='1' b='&#9;&#13;'>t&amp;<e xmlns='' xml:base='b/'/><![CDATA[<]]></p:s></r><?q?>";
    let mut cases: Vec<Vec<u8>> = (0..=255u8)
        .map(|selector| [&[selector][..], document].concat())
        .collect();
    cases.extend([
        b"\x00<a xmlns:p='rel'/>".to_vec(),
        b"\x00<a xmlns:p='http://a/?x&amp;y'/>".to_vec(),
        b"\x0a<r".to_vec(),
        b"\x00".to_vec(),
        Vec::new(),
    ]);
    for case in &cases {
        driver::exercise_c14n(case);
    }
}

#[test]
fn dtd_wrappers_reach_every_construction() {
    for input in [
        b"\0<!ENTITY e 'x'>".as_slice(),
        b"\x01<n/>&amp;",
        b"\x02<!ENTITY e 'y'>",
        b"\x00<!BAD>",
        b"\x01&e;",
        b"<!DOCTYPE r><r/>",
        b"",
    ] {
        driver::exercise_dtd(input);
    }
    assert!(driver::exercise_dtd(b"\x01x").error.is_none());
    assert!(driver::exercise_dtd(b"\x01&e;").error.is_some());
}

/// Replays every minimized crash under `fuzz/regressions/<target>/` through
/// that target's entry point.
#[test]
fn fuzz_regressions_replay() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz/regressions");
    for target in std::fs::read_dir(root).unwrap() {
        let target = target.unwrap();
        let name = target.file_name().into_string().unwrap();
        for case in std::fs::read_dir(target.path()).unwrap() {
            let data = std::fs::read(case.unwrap().path()).unwrap();
            match name.as_str() {
                "buffered" => {
                    driver::exercise_pull(&data, data.get(data.len() / 2).copied().unwrap_or(0));
                }
                "dtd" => drop(driver::exercise_dtd(&data)),
                _ => drop(driver::exercise(&data)),
            }
        }
    }
}

#[test]
fn resolver_wrapper_serves_fuzz_resources_to_every_surface() {
    let resolved = [
        (b"\x03".as_slice(), true),
        (
            b"\x03\0<!ATTLIST r a CDATA 'd'>\0text<b/>\0<!ENTITY x 'y'>",
            true,
        ),
        (
            b"\x03\0<![INCLUDE[<!ENTITY q SYSTEM '0'>]]>\0&q;\0\0<i/>",
            true,
        ),
        (b"\x03\0\0<a>", false),
        (b"\x03\0\0\0%p;", false),
        (b"\x03\0<!BAD>", false),
    ];
    for (input, accepted) in resolved {
        assert_eq!(
            driver::exercise_dtd(input).error.is_none(),
            accepted,
            "{input:?}"
        );
    }
}
