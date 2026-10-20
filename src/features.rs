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
    /// `XML_WITH_THREAD`: thread safety.
    Thread = 1,
    /// `XML_WITH_TREE`: the tree API.
    Tree = 2,
    /// `XML_WITH_OUTPUT`: serialization.
    Output = 3,
    /// `XML_WITH_PUSH`: the push parser.
    Push = 4,
    /// `XML_WITH_READER`: the `xmlTextReader` API.
    Reader = 5,
    /// `XML_WITH_PATTERN`: `xmlPattern` matching.
    Pattern = 6,
    /// `XML_WITH_WRITER`: the `xmlTextWriter` API.
    Writer = 7,
    /// `XML_WITH_SAX1`: the SAX1 interface.
    Sax1 = 8,
    /// `XML_WITH_FTP`: the FTP client.
    Ftp = 9,
    /// `XML_WITH_HTTP`: the HTTP client.
    Http = 10,
    /// `XML_WITH_VALID`: DTD validation.
    Valid = 11,
    /// `XML_WITH_HTML`: the HTML parser.
    Html = 12,
    /// `XML_WITH_LEGACY`: deprecated legacy APIs.
    Legacy = 13,
    /// `XML_WITH_C14N`: canonicalization.
    C14n = 14,
    /// `XML_WITH_CATALOG`: XML catalogs.
    Catalog = 15,
    /// `XML_WITH_XPATH`: XPath.
    Xpath = 16,
    /// `XML_WITH_XPTR`: XPointer.
    Xptr = 17,
    /// `XML_WITH_XINCLUDE`: XInclude.
    Xinclude = 18,
    /// `XML_WITH_ICONV`: iconv-based decoding.
    Iconv = 19,
    /// `XML_WITH_ISO8859X`: ISO-8859-x decoding.
    Iso8859x = 20,
    /// `XML_WITH_UNICODE`: Unicode character classes.
    Unicode = 21,
    /// `XML_WITH_REGEXP`: XML Schema regular expressions.
    Regexp = 22,
    /// `XML_WITH_AUTOMATA`: the `xmlAutomata` API.
    Automata = 23,
    /// `XML_WITH_EXPR`: the `xmlExp` API.
    Expr = 24,
    /// `XML_WITH_SCHEMAS`: XML Schema and RELAX NG.
    Schemas = 25,
    /// `XML_WITH_SCHEMATRON`: Schematron.
    Schematron = 26,
    /// `XML_WITH_MODULES`: dynamic module loading.
    Modules = 27,
    /// `XML_WITH_DEBUG`: debugging output.
    Debug = 28,
    /// `XML_WITH_DEBUG_MEM`: memory debugging.
    DebugMem = 29,
    /// `XML_WITH_DEBUG_RUN`: runtime debugging.
    DebugRun = 30,
    /// `XML_WITH_ZLIB`: zlib decompression.
    Zlib = 31,
    /// `XML_WITH_ICU`: ICU-based decoding.
    Icu = 32,
    /// `XML_WITH_LZMA`: LZMA decompression.
    Lzma = 33,
    /// `XML_WITH_RELAXNG`: RELAX NG.
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
