#![forbid(unsafe_code)]
//! `yaxml::chars` at the class boundaries (the full differential against
//! libxml2's `chvalid.h` is tests/dtd_oracle.rs).

use yaxml::chars;

type Class = fn(u32) -> bool;

const CLASSES: [(&str, Class, &[u32], &[u32]); 8] = [
    (
        "Char",
        chars::is_char,
        &[
            0x9, 0xa, 0xd, 0x20, 0xd7ff, 0xe000, 0xfffd, 0x10000, 0x10ffff,
        ],
        &[
            0x0,
            0x8,
            0xb,
            0x1f,
            0xd800,
            0xdfff,
            0xfffe,
            0xffff,
            0x110000,
            u32::MAX,
        ],
    ),
    (
        "Blank",
        chars::is_blank,
        &[0x9, 0xa, 0xd, 0x20],
        &[0x0, 0xb, 0x85, 0xa0, 0x3000],
    ),
    (
        "PubidChar",
        chars::is_pubid_char,
        &[0x20, 0xa, 0xd, 0x27, 0x25, 0x5f, 0x30, 0x7a],
        &[0x9, 0x22, 0x26, 0x3c, 0x7f, 0xe9],
    ),
    (
        "BaseChar",
        chars::is_base_char,
        &[0x41, 0x7a, 0xc0, 0xf8, 0xff, 0x100, 0xac00, 0xd7a3],
        &[0x40, 0x5b, 0xd7, 0xf7, 0x132, 0x4e00, 0xd7a4, 0x10000],
    ),
    (
        "Ideographic",
        chars::is_ideographic,
        &[0x3007, 0x3021, 0x3029, 0x4e00, 0x9fa5],
        &[0x41, 0x3008, 0x302a, 0x9fa6, 0x20000],
    ),
    (
        "Combining",
        chars::is_combining,
        &[0x300, 0x345, 0x360, 0x3099, 0x309a],
        &[0x2ff, 0x346, 0x41, 0x309b],
    ),
    (
        "Digit",
        chars::is_digit,
        &[0x30, 0x39, 0x660, 0xf29],
        &[0x2f, 0x3a, 0xb2, 0x10000],
    ),
    (
        "Extender",
        chars::is_extender,
        &[0xb7, 0x2d0, 0x2d1, 0x3005, 0x30fe],
        &[0xb6, 0x2d2, 0x3006, 0x30ff],
    ),
];

#[test]
fn classes_accept_members_and_reject_neighbours() {
    for (name, is, members, others) in CLASSES {
        for &c in members {
            assert!(is(c), "{name} {c:#x}");
        }
        for &c in others {
            assert!(!is(c), "{name} {c:#x}");
        }
    }
}

#[test]
fn bytes_chars_and_code_points_agree() {
    for byte in 0..=u8::MAX {
        let c = char::from(byte);
        let all = [
            (
                chars::is_char(byte),
                chars::is_char(c),
                chars::is_char(u32::from(byte)),
            ),
            (
                chars::is_blank(byte),
                chars::is_blank(c),
                chars::is_blank(u32::from(byte)),
            ),
            (
                chars::is_pubid_char(byte),
                chars::is_pubid_char(c),
                chars::is_pubid_char(u32::from(byte)),
            ),
            (
                chars::is_base_char(byte),
                chars::is_base_char(c),
                chars::is_base_char(u32::from(byte)),
            ),
            (
                chars::is_digit(byte),
                chars::is_digit(c),
                chars::is_digit(u32::from(byte)),
            ),
            (
                chars::is_extender(byte),
                chars::is_extender(c),
                chars::is_extender(u32::from(byte)),
            ),
        ];
        for (index, (a, b, c)) in all.into_iter().enumerate() {
            assert!(a == b && b == c, "class {index} byte {byte:#x}");
        }
        assert!(!chars::is_combining(byte) && !chars::is_ideographic(byte));
    }
}
