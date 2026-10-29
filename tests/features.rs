#![forbid(unsafe_code)]
//! Capability and version reporting against libxml2 2.15.4's `xmlFeature`.

use yaxml::features::{self, Feature};

/// (`XML_WITH_*` number, name, expected answer).
const EXPECTED: [(u32, &str, bool); 34] = [
    (1, "THREAD", true),
    (2, "TREE", true),
    (3, "OUTPUT", true),
    (4, "PUSH", true),
    (5, "READER", cfg!(feature = "textreader")),
    (6, "PATTERN", cfg!(feature = "xpath")),
    (7, "WRITER", cfg!(feature = "textwriter")),
    (8, "SAX1", false),
    (9, "FTP", false),
    (10, "HTTP", false),
    (11, "VALID", true),
    (12, "HTML", cfg!(feature = "html")),
    (13, "LEGACY", false),
    (14, "C14N", cfg!(feature = "c14n")),
    (15, "CATALOG", cfg!(feature = "catalog")),
    (16, "XPATH", cfg!(feature = "xpath")),
    (17, "XPTR", cfg!(feature = "xpath")),
    (18, "XINCLUDE", cfg!(feature = "xinclude")),
    (19, "ICONV", false),
    (20, "ISO8859X", true),
    (21, "UNICODE", cfg!(feature = "schemas")),
    (22, "REGEXP", cfg!(feature = "schemas")),
    (23, "AUTOMATA", false),
    (24, "EXPR", false),
    (25, "SCHEMAS", cfg!(feature = "schemas")),
    (26, "SCHEMATRON", cfg!(feature = "schemas")),
    (27, "MODULES", false),
    (28, "DEBUG", cfg!(feature = "debug")),
    (29, "DEBUG_MEM", false),
    (30, "DEBUG_RUN", false),
    (31, "ZLIB", false),
    (32, "ICU", false),
    (33, "LZMA", false),
    (34, "RELAXNG", cfg!(feature = "schemas")),
];

#[test]
fn every_libxml2_feature_has_its_number_and_answer() {
    for (code, name, expected) in EXPECTED {
        let feature = Feature::from_code(code).unwrap_or_else(|| panic!("{name} missing"));
        assert_eq!(feature as u32, code, "{name}");
        assert_eq!(features::has(feature), expected, "{name}");
        assert_eq!(features::has_code(code), expected, "{name} by number");
    }
    let listed: Vec<_> = features::all().map(|(f, on)| (f as u32, on)).collect();
    let expected: Vec<_> = EXPECTED.iter().map(|&(c, _, on)| (c, on)).collect();
    assert_eq!(listed, expected);
}

#[test]
fn unknown_numbers_are_unsupported() {
    // 0, one past the last, XML_WITH_NONE and the largest value.
    for code in [0, 35, 99999, u32::MAX] {
        assert_eq!(Feature::from_code(code), None, "{code}");
        assert!(!features::has_code(code), "{code}");
    }
}

#[test]
fn version_constants_agree() {
    let (major, minor, patch) = (21504 / 10000, 21504 / 100 % 100, 21504 % 100);
    assert_eq!(
        features::LIBXML2_COMPAT_VERSION,
        format!("{major}.{minor}.{patch}")
    );
    assert_eq!(features::LIBXML2_COMPAT_VERSION_NUMBER, 21504);
    assert_eq!(yaxml::LIBXML2_COMPAT_VERSION_NUMBER, 21504);
    assert_eq!(yaxml::VERSION, env!("CARGO_PKG_VERSION"));
    assert_eq!(features::ALLOCATOR_API, cfg!(feature = "allocator_api"));
}

#[test]
fn check_version_follows_xml_check_version() {
    // (program compiled against, compatible): major must match and the
    // library may not be older in minor version; patch is ignored.
    for (version, compatible) in [
        (21504, true),
        (21500, true),
        (21599, true),
        (21400, true),
        (20000, true),
        (19999, false),
        (21600, false),
        (30000, false),
        (11504, false),
        (0, false),
    ] {
        assert_eq!(features::check_version(version), compatible, "{version}");
    }
}
