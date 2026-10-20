//! Build capabilities and versions: libxml2's `xmlHasFeature`,
//! `LIBXML_VERSION` and `xmlCheckVersion`.
//!
//! [`Feature`] keeps libxml2's `xmlFeature` numbers; unknown numbers and
//! `XML_WITH_NONE` are unsupported, as in libxml2.
//!
//! ```
//! use yaxml::features::{self, Feature};
//!
//! assert!(features::has(Feature::Xpath));
//! assert!(!features::has(Feature::Iconv)); // decoding is built in
//! assert!(features::has_code(16)); // XML_WITH_XPATH
//! assert_eq!(features::LIBXML2_COMPAT_VERSION_NUMBER, 21504);
//! ```

/// libxml2 2.15.4's `xmlFeature`, with its numeric values.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
#[repr(u32)]
pub enum Feature {
    Thread = 1,
    Tree = 2,
    Output = 3,
    Push = 4,
    Reader = 5,
    Pattern = 6,
    Writer = 7,
    Sax1 = 8,
    Ftp = 9,
    Http = 10,
    Valid = 11,
    Html = 12,
    Legacy = 13,
    C14n = 14,
    Catalog = 15,
    Xpath = 16,
    Xptr = 17,
    Xinclude = 18,
    Iconv = 19,
    Iso8859x = 20,
    Unicode = 21,
    Regexp = 22,
    Automata = 23,
    Expr = 24,
    Schemas = 25,
    Schematron = 26,
    Modules = 27,
    Debug = 28,
    DebugMem = 29,
    DebugRun = 30,
    Zlib = 31,
    Icu = 32,
    Lzma = 33,
    Relaxng = 34,
}

use Feature::*;

/// Every feature with its answer. `Thread`: all types are `Send` and there
/// is no global state. `Iso8859x`: own decoding tables, so no iconv is
/// needed. `Sax1`: only `SAX1` the option bit is accepted, no SAX1 handlers.
/// `Legacy` (deprecated APIs), `Automata` (`xmlAutomata*`; the XSD content
/// model automaton is internal) and `Expr` (`xmlexp`) have no equivalent.
const TABLE: [(Feature, bool); 34] = [
    (Thread, true),
    (Tree, true),
    (Output, true),
    (Push, true),
    (Reader, true),
    (Pattern, true),
    (Writer, true),
    (Sax1, false),
    (Ftp, false),
    (Http, false),
    (Valid, true),
    (Html, true),
    (Legacy, false),
    (C14n, true),
    (Catalog, true),
    (Xpath, true),
    (Xptr, true),
    (Xinclude, true),
    (Iconv, false),
    (Iso8859x, true),
    (Unicode, true),
    (Regexp, true),
    (Automata, false),
    (Expr, false),
    (Schemas, true),
    (Schematron, true),
    (Modules, false),
    (Debug, false),
    (DebugMem, false),
    (DebugRun, false),
    (Zlib, false),
    (Icu, false),
    (Lzma, false),
    (Relaxng, true),
];

/// This crate's version (`CARGO_PKG_VERSION`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The libxml2 release whose behavior is matched.
pub const LIBXML2_COMPAT_VERSION: &str = "2.15.4";
/// `LIBXML_VERSION` of that release: `major * 10000 + minor * 100 + patch`.
pub const LIBXML2_COMPAT_VERSION_NUMBER: u32 = 21504;
/// Whether the `allocator_api` cargo feature (`OwnedDocument::parse_in`) is on.
pub const ALLOCATOR_API: bool = cfg!(feature = "allocator_api");

impl Feature {
    /// The feature for an `XML_WITH_*` number; `None` for `XML_WITH_NONE`
    /// (99999) and anything unknown.
    pub fn from_code(code: u32) -> Option<Self> {
        TABLE
            .iter()
            .map(|&(feature, _)| feature)
            .find(|&feature| feature as u32 == code)
    }
}

/// `xmlHasFeature`: whether this build provides `feature`.
pub fn has(feature: Feature) -> bool {
    TABLE.iter().any(|&(listed, on)| listed == feature && on)
}

/// `xmlHasFeature` for a raw `XML_WITH_*` number; unknown numbers are false.
pub fn has_code(code: u32) -> bool {
    Feature::from_code(code).is_some_and(has)
}

/// Every feature in `xmlFeature` order with its answer.
pub fn all() -> impl ExactSizeIterator<Item = (Feature, bool)> {
    TABLE.into_iter()
}

/// `xmlCheckVersion` without the printing: false when the major version of
/// `version` (`LIBXML_VERSION` form) differs from [`LIBXML2_COMPAT_VERSION_NUMBER`]'s
/// or its minor version is newer.
pub fn check_version(version: u32) -> bool {
    version / 10000 == LIBXML2_COMPAT_VERSION_NUMBER / 10000
        && version / 100 <= LIBXML2_COMPAT_VERSION_NUMBER / 100
}
