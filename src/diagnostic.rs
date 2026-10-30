//! libxml2's structured error record (`xmlError`), built on demand.
//!
//! [`Error`] is a cheap `{ kind, offset }` value; [`Error::describe`] or
//! [`LineIndex::describe_all`] re-reads the input around the offset to build a
//! [`Diagnostic`] with libxml2's domain, code, level, line, column and
//! message. The validators' diagnostics convert to the same shape. Codes and
//! domains are stable; see the
//! [error mapping](https://github.com/pbdeuchler/yaxml/blob/main/docs/bindings.md#error-mapping).
//!
//! ```
//! use yaxml::{Document, LineIndex, ParseOptions, options};
//!
//! let input = b"<r><a>text</b><p:c/>";
//! let document = Document::parse_with(input, ParseOptions::new(options::RECOVER))?;
//! let lines = LineIndex::new(input);
//! let messages: Vec<_> = lines.describe_all(document.errors()).into_iter().map(|d| d.message).collect();
//! assert_eq!(messages, [
//!     "Opening and ending tag mismatch: a line 1 and b",
//!     "Namespace prefix p on c is not defined",
//! ]);
//! assert_eq!(document.root_element().line(&lines), Some(1));
//! # Ok::<(), yaxml::Error>(())
//! ```

use std::borrow::Cow;

use crate::xpath::tree::{Item, Navigate};
use crate::{Error, ErrorKind, Warning};

mod reread;

use reread::{Cur, Found, Scan, Work, found, local, unquoted};

/// The subsystem that raised a [`Diagnostic`] (libxml2's `xmlErrorDomain`);
/// `as u8` is the C constant.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum Domain {
    /// `XML_FROM_PARSER`.
    #[default]
    Parser = 1,
    /// `XML_FROM_NAMESPACE`.
    Namespace = 3,
    /// `XML_FROM_DTD`.
    Dtd = 4,
    /// `XML_FROM_HTML`.
    Html = 5,
    /// `XML_FROM_OUTPUT`.
    Output = 7,
    /// `XML_FROM_IO`.
    Io = 8,
    /// `XML_FROM_XINCLUDE`.
    XInclude = 11,
    /// `XML_FROM_XPATH`.
    XPath = 12,
    /// `XML_FROM_XPOINTER`.
    XPointer = 13,
    /// `XML_FROM_REGEXP`.
    Regexp = 14,
    /// `XML_FROM_DATATYPE`.
    Datatype = 15,
    /// `XML_FROM_SCHEMASP`.
    SchemasParser = 16,
    /// `XML_FROM_SCHEMASV`.
    SchemasValidity = 17,
    /// `XML_FROM_RELAXNGP`.
    RelaxNgParser = 18,
    /// `XML_FROM_RELAXNGV`.
    RelaxNgValidity = 19,
    /// `XML_FROM_CATALOG`.
    Catalog = 20,
    /// `XML_FROM_C14N`.
    C14n = 21,
    /// `XML_FROM_VALID`.
    Valid = 23,
    /// `XML_FROM_WRITER`.
    Writer = 25,
    /// `XML_FROM_SCHEMATRONV`.
    SchematronValidity = 28,
}

/// A [`Diagnostic`]'s severity (libxml2's `xmlErrorLevel`); `as u8` is the
/// C constant.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum Level {
    /// `XML_ERR_WARNING`.
    Warning = 1,
    /// `XML_ERR_ERROR`.
    #[default]
    Error = 2,
    /// `XML_ERR_FATAL`.
    Fatal = 3,
}

/// One `xmlError`: what a libxml2 structured error handler receives.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Diagnostic {
    /// The subsystem that raised it.
    pub domain: Domain,
    /// The `xmlParserErrors` number (`xmlError.code`).
    pub code: u16,
    /// Severity.
    pub level: Level,
    /// 1-based line; 0 when unknown.
    pub line: usize,
    /// 1-based column in characters (`xmlError.int2`); 0 when unknown.
    pub column: usize,
    /// libxml2's text without its trailing newline (`xmlError.message` is
    /// this plus `"\n"`).
    pub message: String,
    /// libxml2's first string argument (`xmlError.str1`), such as a name.
    pub str1: Option<String>,
    /// libxml2's second string argument (`xmlError.str2`).
    pub str2: Option<String>,
    /// libxml2's third string argument (`xmlError.str3`): only the element
    /// of an undefined attribute prefix (`XML_NS_ERR_UNDEFINED_NAMESPACE`).
    pub str3: Option<String>,
    /// libxml2's integer argument (`xmlError.int1`), such as a line.
    pub int1: i64,
    /// Local name of the element `xmlError.node` names, which
    /// `xmlFormatError` prints as `element NAME: `.
    pub element: Option<String>,
    /// Raised in a nested entity's replacement text: `line` and `column`
    /// count in that text, `xmlError.file` is NULL and the report reads
    /// `Entity: line N`.
    pub entity: bool,
}

/// The libxml2 `xmlParserErrors` numbers that [`Diagnostic::code`] takes.
pub mod code {
    /// `XML_ERR_INTERNAL_ERROR`.
    pub const INTERNAL_ERROR: u16 = 1;
    /// `XML_ERR_NO_MEMORY`.
    pub const NO_MEMORY: u16 = 2;
    /// `XML_ERR_DOCUMENT_EMPTY`.
    pub const DOCUMENT_EMPTY: u16 = 4;
    /// `XML_ERR_DOCUMENT_END`.
    pub const DOCUMENT_END: u16 = 5;
    /// `XML_ERR_INVALID_HEX_CHARREF`.
    pub const INVALID_HEX_CHARREF: u16 = 6;
    /// `XML_ERR_INVALID_DEC_CHARREF`.
    pub const INVALID_DEC_CHARREF: u16 = 7;
    /// `XML_ERR_INVALID_CHARREF`.
    pub const INVALID_CHARREF: u16 = 8;
    /// `XML_ERR_INVALID_CHAR`.
    pub const INVALID_CHAR: u16 = 9;
    /// `XML_ERR_ENTITYREF_SEMICOL_MISSING`.
    pub const ENTITYREF_SEMICOL_MISSING: u16 = 23;
    /// `XML_ERR_PEREF_NO_NAME`.
    pub const PEREF_NO_NAME: u16 = 24;
    /// `XML_ERR_PEREF_SEMICOL_MISSING`.
    pub const PEREF_SEMICOL_MISSING: u16 = 25;
    /// `XML_ERR_UNDECLARED_ENTITY`.
    pub const UNDECLARED_ENTITY: u16 = 26;
    /// `XML_WAR_UNDECLARED_ENTITY`.
    pub const WAR_UNDECLARED_ENTITY: u16 = 27;
    /// `XML_ERR_UNPARSED_ENTITY`.
    pub const UNPARSED_ENTITY: u16 = 28;
    /// `XML_ERR_ENTITY_IS_EXTERNAL`.
    pub const ENTITY_IS_EXTERNAL: u16 = 29;
    /// `XML_ERR_UNSUPPORTED_ENCODING`.
    pub const UNSUPPORTED_ENCODING: u16 = 32;
    /// `XML_ERR_STRING_NOT_STARTED`.
    pub const STRING_NOT_STARTED: u16 = 33;
    /// `XML_ERR_STRING_NOT_CLOSED`.
    pub const STRING_NOT_CLOSED: u16 = 34;
    /// `XML_ERR_ENTITY_NOT_FINISHED`.
    pub const ENTITY_NOT_FINISHED: u16 = 37;
    /// `XML_ERR_LT_IN_ATTRIBUTE`.
    pub const LT_IN_ATTRIBUTE: u16 = 38;
    /// `XML_ERR_ATTRIBUTE_NOT_STARTED`.
    pub const ATTRIBUTE_NOT_STARTED: u16 = 39;
    /// `XML_ERR_ATTRIBUTE_NOT_FINISHED`.
    pub const ATTRIBUTE_NOT_FINISHED: u16 = 40;
    /// `XML_ERR_ATTRIBUTE_WITHOUT_VALUE`.
    pub const ATTRIBUTE_WITHOUT_VALUE: u16 = 41;
    /// `XML_ERR_ATTRIBUTE_REDEFINED`.
    pub const ATTRIBUTE_REDEFINED: u16 = 42;
    /// `XML_ERR_LITERAL_NOT_STARTED`.
    pub const LITERAL_NOT_STARTED: u16 = 43;
    /// `XML_ERR_LITERAL_NOT_FINISHED`.
    pub const LITERAL_NOT_FINISHED: u16 = 44;
    /// `XML_ERR_COMMENT_NOT_FINISHED`.
    pub const COMMENT_NOT_FINISHED: u16 = 45;
    /// `XML_ERR_PI_NOT_STARTED`.
    pub const PI_NOT_STARTED: u16 = 46;
    /// `XML_ERR_PI_NOT_FINISHED`.
    pub const PI_NOT_FINISHED: u16 = 47;
    /// `XML_ERR_NOTATION_NOT_STARTED`.
    pub const NOTATION_NOT_STARTED: u16 = 48;
    /// `XML_ERR_NOTATION_NOT_FINISHED`.
    pub const NOTATION_NOT_FINISHED: u16 = 49;
    /// `XML_ERR_ATTLIST_NOT_STARTED`.
    pub const ATTLIST_NOT_STARTED: u16 = 50;
    /// `XML_ERR_ATTLIST_NOT_FINISHED`.
    pub const ATTLIST_NOT_FINISHED: u16 = 51;
    /// `XML_ERR_MIXED_NOT_STARTED`.
    pub const MIXED_NOT_STARTED: u16 = 52;
    /// `XML_ERR_ELEMCONTENT_NOT_STARTED`.
    pub const ELEMCONTENT_NOT_STARTED: u16 = 54;
    /// `XML_ERR_ELEMCONTENT_NOT_FINISHED`.
    pub const ELEMCONTENT_NOT_FINISHED: u16 = 55;
    /// `XML_ERR_XMLDECL_NOT_FINISHED`.
    pub const XMLDECL_NOT_FINISHED: u16 = 57;
    /// `XML_ERR_DOCTYPE_NOT_FINISHED`.
    pub const DOCTYPE_NOT_FINISHED: u16 = 61;
    /// `XML_ERR_MISPLACED_CDATA_END`.
    pub const MISPLACED_CDATA_END: u16 = 62;
    /// `XML_ERR_CDATA_NOT_FINISHED`.
    pub const CDATA_NOT_FINISHED: u16 = 63;
    /// `XML_ERR_RESERVED_XML_NAME`.
    pub const RESERVED_XML_NAME: u16 = 64;
    /// `XML_ERR_REDECL_PREDEF_ENTITY`.
    pub const REDECL_PREDEF_ENTITY: u16 = 117;
    /// `XML_ERR_SPACE_REQUIRED`.
    pub const SPACE_REQUIRED: u16 = 65;
    /// `XML_ERR_SEPARATOR_REQUIRED`.
    pub const SEPARATOR_REQUIRED: u16 = 66;
    /// `XML_ERR_NMTOKEN_REQUIRED`.
    pub const NMTOKEN_REQUIRED: u16 = 67;
    /// `XML_ERR_NAME_REQUIRED`.
    pub const NAME_REQUIRED: u16 = 68;
    /// `XML_ERR_GT_REQUIRED`.
    pub const GT_REQUIRED: u16 = 73;
    /// `XML_ERR_EQUAL_REQUIRED`.
    pub const EQUAL_REQUIRED: u16 = 75;
    /// `XML_ERR_TAG_NAME_MISMATCH`.
    pub const TAG_NAME_MISMATCH: u16 = 76;
    /// `XML_ERR_TAG_NOT_FINISHED`.
    pub const TAG_NOT_FINISHED: u16 = 77;
    /// `XML_ERR_STANDALONE_VALUE`.
    pub const STANDALONE_VALUE: u16 = 78;
    /// `XML_ERR_ENCODING_NAME`.
    pub const ENCODING_NAME: u16 = 79;
    /// `XML_ERR_HYPHEN_IN_COMMENT`.
    pub const HYPHEN_IN_COMMENT: u16 = 80;
    /// `XML_ERR_INVALID_ENCODING`.
    pub const INVALID_ENCODING: u16 = 81;
    /// `XML_ERR_VALUE_REQUIRED`.
    pub const VALUE_REQUIRED: u16 = 84;
    /// `XML_ERR_NOT_WELL_BALANCED`.
    pub const NOT_WELL_BALANCED: u16 = 85;
    /// `XML_ERR_ENTITY_CHAR_ERROR`.
    pub const ENTITY_CHAR_ERROR: u16 = 87;
    /// `XML_ERR_ENTITY_PE_INTERNAL`.
    pub const ENTITY_PE_INTERNAL: u16 = 88;
    /// `XML_ERR_ENTITY_LOOP`.
    pub const ENTITY_LOOP: u16 = 89;
    /// `XML_WAR_CATALOG_PI`.
    pub const WAR_CATALOG_PI: u16 = 93;
    /// `XML_ERR_VERSION_MISSING`.
    pub const VERSION_MISSING: u16 = 96;
    /// `XML_WAR_UNKNOWN_VERSION`.
    pub const WAR_UNKNOWN_VERSION: u16 = 97;
    /// `XML_WAR_LANG_VALUE`.
    pub const WAR_LANG_VALUE: u16 = 98;
    /// `XML_CATALOG_MISSING_ATTR`: an entry lacks its identifier or URI.
    pub const CATALOG_MISSING_ATTR: u16 = 1650;
    /// `XML_CATALOG_ENTRY_BROKEN`: an entry's URI does not resolve.
    pub const CATALOG_ENTRY_BROKEN: u16 = 1651;
    /// `XML_CATALOG_PREFER_VALUE`: `prefer` is neither `public` nor `system`.
    pub const CATALOG_PREFER_VALUE: u16 = 1652;
    /// `XML_CATALOG_NOT_CATALOG`: the root is not an OASIS `catalog`.
    pub const CATALOG_NOT_CATALOG: u16 = 1653;
    /// `XML_WAR_NS_URI`.
    pub const WAR_NS_URI: u16 = 99;
    /// `XML_WAR_NS_URI_RELATIVE`.
    pub const WAR_NS_URI_RELATIVE: u16 = 100;
    /// `XML_WAR_SPACE_VALUE`.
    pub const WAR_SPACE_VALUE: u16 = 102;
    /// `XML_ERR_NOTATION_PROCESSING`.
    pub const NOTATION_PROCESSING: u16 = 105;
    /// `XML_WAR_ENTITY_REDEFINED`.
    pub const WAR_ENTITY_REDEFINED: u16 = 107;
    /// `XML_ERR_UNKNOWN_VERSION`.
    pub const UNKNOWN_VERSION: u16 = 108;
    /// `XML_ERR_USER_STOP`.
    pub const USER_STOP: u16 = 111;
    /// `XML_WAR_ENCODING_MISMATCH`.
    pub const WAR_ENCODING_MISMATCH: u16 = 113;
    /// `XML_ERR_RESOURCE_LIMIT`.
    pub const RESOURCE_LIMIT: u16 = 114;
    /// `XML_ERR_ARGUMENT`.
    pub const ARGUMENT: u16 = 115;
    /// `XML_ERR_INT_SUBSET_NOT_FINISHED`.
    pub const INT_SUBSET_NOT_FINISHED: u16 = 118;
    /// `XML_NS_ERR_XML_NAMESPACE`.
    pub const NS_XML_NAMESPACE: u16 = 200;
    /// `XML_NS_ERR_UNDEFINED_NAMESPACE`.
    pub const NS_UNDEFINED_NAMESPACE: u16 = 201;
    /// `XML_NS_ERR_QNAME`.
    pub const NS_QNAME: u16 = 202;
    /// `XML_NS_ERR_ATTRIBUTE_REDEFINED`.
    pub const NS_ATTRIBUTE_REDEFINED: u16 = 203;
    /// `XML_NS_ERR_COLON`.
    pub const NS_COLON: u16 = 205;
    /// `XML_DTD_XMLID_VALUE`, raised by the parser.
    pub const DTD_XMLID_VALUE: u16 = 539;
    /// `XML_DTD_XMLID_TYPE`, raised by the parser.
    pub const DTD_XMLID_TYPE: u16 = 540;
    /// `XML_HTML_STRUCURE_ERROR`.
    pub const HTML_STRUCTURE_ERROR: u16 = 800;
    /// `XML_IO_LOAD_ERROR`.
    pub const IO_LOAD_ERROR: u16 = 1549;
    /// `XML_XINCLUDE_BUILD_FAILED`.
    pub const XINCLUDE_BUILD_FAILED: u16 = 1609;
}

use code::*;

/// libxml2's `xmlIOErr` code (`XML_IO_*`, domain I/O, level error) for an
/// I/O failure, such as a [`ReadError::Io`](crate::ReadError::Io) or a
/// resolver's error: by its OS error number where no [`std::io::ErrorKind`]
/// tells it apart (`EPERM`, `EBADF`, ...), else by its kind;
/// `XML_IO_UNKNOWN` (1500) for anything libxml2 does not name.
///
/// ```
/// use std::io::{Error, ErrorKind};
/// assert_eq!(yaxml::diagnostic::io_code(&Error::from(ErrorKind::NotFound)), 1524);
/// assert_eq!(yaxml::diagnostic::io_code(&Error::other("refused")), 1500);
/// ```
pub fn io_code(error: &std::io::Error) -> u16 {
    use std::io::ErrorKind as K;
    // (errno, code); numbers 1-34 are the same on Linux and Apple systems.
    #[rustfmt::skip]
    const SHARED: [(i32, u16); 14] = [
        (1, 1535), (3, 1540), (5, 1516), (6, 1534), (8, 1525), (9, 1503), (10, 1507),
        (14, 1511), (19, 1523), (23, 1522), (24, 1518), (25, 1533), (33, 1509), (34, 1537),
    ];
    // ENOLCK, EBADMSG, ECANCELED, ENOTSUP, EALREADY, ENOTSOCK, EMSGSIZE,
    // EAFNOSUPPORT, EISCONN, EINPROGRESS.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[rustfmt::skip]
    const PLATFORM: [(i32, u16); 10] = [
        (37, 1526), (74, 1504), (125, 1506), (95, 1532), (114, 1555),
        (88, 1550), (90, 1520), (97, 1556), (106, 1551), (115, 1513),
    ];
    #[cfg(target_vendor = "apple")]
    #[rustfmt::skip]
    const PLATFORM: [(i32, u16); 10] = [
        (77, 1526), (94, 1504), (89, 1506), (45, 1532), (37, 1555),
        (38, 1550), (40, 1520), (47, 1556), (56, 1551), (36, 1513),
    ];
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    const PLATFORM: [(i32, u16); 0] = [];
    let errno = error.raw_os_error().filter(|_| cfg!(unix));
    let named = errno.and_then(|errno| {
        (SHARED.iter().chain(&PLATFORM)).find_map(|&(n, code)| (n == errno).then_some(code))
    });
    named.unwrap_or(match error.kind() {
        K::PermissionDenied => 1501,
        K::WouldBlock => 1502,
        K::ResourceBusy => 1505,
        K::Deadlock => 1508,
        K::AlreadyExists => 1510,
        K::FileTooLarge => 1512,
        K::Interrupted => 1514,
        K::InvalidInput => 1515,
        K::IsADirectory => 1517,
        K::TooManyLinks => 1519,
        K::InvalidFilename => 1521,
        K::NotFound => 1524,
        K::OutOfMemory => 1527,
        K::StorageFull => 1528,
        K::Unsupported => 1529,
        K::NotADirectory => 1530,
        K::DirectoryNotEmpty => 1531,
        K::BrokenPipe => 1536,
        K::ReadOnlyFilesystem => 1538,
        K::NotSeekable => 1539,
        K::TimedOut => 1541,
        K::CrossesDevices => 1542,
        K::ConnectionRefused => 1552,
        K::NetworkUnreachable => 1553,
        K::AddrInUse => 1554,
        _ => 1500,
    })
}

/// The domain and level libxml2 raises a parser code with.
const fn raised(code: u16) -> (Domain, Level) {
    match code {
        NS_XML_NAMESPACE..=NS_COLON => (Domain::Namespace, Level::Error),
        INVALID_ENCODING => (Domain::Io, Level::Fatal),
        1500..=1599 => (Domain::Io, Level::Error),
        1600..=1699 => (Domain::XInclude, Level::Error),
        _ => (Domain::Parser, Level::Fatal),
    }
}

impl Warning {
    /// libxml2's code, domain and level for this warning.
    pub(crate) const fn raised(self) -> (u16, Domain, Level) {
        let (code, domain) = match self {
            Self::UnknownVersion => (WAR_UNKNOWN_VERSION, Domain::Parser),
            Self::NamespaceUri => (WAR_NS_URI, Domain::Namespace),
            Self::RelativeNamespaceUri => (WAR_NS_URI_RELATIVE, Domain::Namespace),
            Self::SpaceValue => (WAR_SPACE_VALUE, Domain::Parser),
            Self::ReservedTarget => (RESERVED_XML_NAME, Domain::Parser),
            Self::EncodingMismatch => (WAR_ENCODING_MISMATCH, Domain::Parser),
            Self::UndeclaredEntity { .. } => (WAR_UNDECLARED_ENTITY, Domain::Parser),
            Self::PredefinedEntity => (REDECL_PREDEF_ENTITY, Domain::Parser),
            Self::XmlIdValue => (DTD_XMLID_VALUE, Domain::Parser),
            Self::XmlIdType => (DTD_XMLID_TYPE, Domain::Parser),
            Self::LangValue => (WAR_LANG_VALUE, Domain::Parser),
            Self::EntityRedefined => (WAR_ENTITY_REDEFINED, Domain::Parser),
            Self::CatalogPi => (WAR_CATALOG_PI, Domain::Parser),
        };
        let level = match self {
            Self::NamespaceUri
            | Self::UndeclaredEntity { error: true }
            | Self::XmlIdValue
            | Self::XmlIdType => Level::Error,
            _ => Level::Warning,
        };
        (code, domain, level)
    }
}

impl ErrorKind {
    /// libxml2's domain and code for this kind without the input;
    /// [`Error::describe`] refines it from the input.
    pub const fn libxml2_code(self) -> (Domain, u16) {
        let code = match self {
            Self::InvalidUtf8
            | Self::InvalidUtf16
            | Self::UnmappedByte
            | Self::EncodingMismatch => INVALID_ENCODING,
            Self::InvalidCharacter => INVALID_CHAR,
            Self::UnexpectedEof => TAG_NOT_FINISHED,
            Self::Syntax | Self::InvalidName => NAME_REQUIRED,
            Self::MismatchedEndTag => TAG_NAME_MISMATCH,
            Self::DuplicateAttribute => ATTRIBUTE_REDEFINED,
            Self::Namespace => NS_UNDEFINED_NAMESPACE,
            Self::InvalidReference => INVALID_CHARREF,
            Self::UnknownEntity => UNDECLARED_ENTITY,
            Self::EntityLoop => ENTITY_LOOP,
            Self::UnparsedEntity => UNPARSED_ENTITY,
            Self::ExternalEntityInAttribute => ENTITY_IS_EXTERNAL,
            Self::UnbalancedEntity => NOT_WELL_BALANCED,
            Self::InvalidDtd => DOCTYPE_NOT_FINISHED,
            Self::ExternalResource => IO_LOAD_ERROR,
            Self::XInclude => XINCLUDE_BUILD_FAILED,
            Self::MissingRoot => DOCUMENT_EMPTY,
            Self::MultipleRoots | Self::TextOutsideRoot => DOCUMENT_END,
            Self::InvalidDeclaration => XMLDECL_NOT_FINISHED,
            Self::UnsupportedEncoding => UNSUPPORTED_ENCODING,
            Self::UnsupportedOption => ARGUMENT,
            Self::LimitExceeded(_) | Self::IdentifierExhausted => RESOURCE_LIMIT,
            Self::Allocation => NO_MEMORY,
            Self::Aborted => USER_STOP,
            Self::InvalidState => INTERNAL_ERROR,
            Self::Warning(warning) => {
                let (code, domain, _) = warning.raised();
                return (domain, code);
            }
        };
        (raised(code).0, code)
    }
}

// ---- positions ----

/// Maps input byte offsets to libxml2's lines and columns.
///
/// Lines end at `\n`; columns count characters from 1, and a leading byte
/// order mark is not a column. UTF-16, declared single-byte and (feature
/// `encodings`) UCS-4 and EBCDIC input is decoded; offsets are always input
/// bytes.
pub struct LineIndex<'s> {
    input: Cow<'s, [u8]>,
    /// The input transcoded to UTF-8; `None` when the input is UTF-8 itself.
    text: Option<Vec<u8>>,
    /// Text offset of each line's first byte.
    starts: Vec<usize>,
    /// Transcoded input: its decoder and the input offset of each line's first byte.
    decoded: Option<(Decoder, Vec<usize>)>,
    /// HTML input re-decoded after a charset switch, before conversion:
    /// what earlier reports show.
    #[cfg_attr(not(feature = "html"), allow(dead_code))]
    raw: Option<&'s [u8]>,
    /// Content parsed in a node's context, not a document.
    content: bool,
}

/// The decoders of transcoded input.
#[derive(Clone, Copy)]
enum Decoder {
    Utf16 {
        big: bool,
    },
    Byte(&'static [u16; 128]),
    #[cfg_attr(not(feature = "encodings"), allow(dead_code))]
    Ucs4 {
        big: bool,
    },
    #[cfg_attr(not(feature = "encodings"), allow(dead_code))]
    Ebcdic(&'static [u16; 256]),
}

impl Decoder {
    /// The character `bytes` start with and its input length; U+FFFD for an
    /// unmapped byte or an unpaired surrogate.
    fn next(self, bytes: &[u8]) -> Option<(char, usize)> {
        let replaced = |c: Option<char>| c.unwrap_or(char::REPLACEMENT_CHARACTER);
        match self {
            Self::Byte(table) => {
                let &b = bytes.first()?;
                let c = if b < 0x80 {
                    u32::from(b)
                } else {
                    u32::from(table[usize::from(b) - 0x80])
                };
                Some((
                    replaced(char::from_u32(c).filter(|&c| c != '\0' || b == 0)),
                    1,
                ))
            }
            Self::Utf16 { big } => {
                let units = bytes.as_chunks::<2>().0.iter().take(2).map(|&pair| {
                    if big {
                        u16::from_be_bytes(pair)
                    } else {
                        u16::from_le_bytes(pair)
                    }
                });
                let c = char::decode_utf16(units).next()?.ok();
                Some((replaced(c), 2 * c.map_or(1, char::len_utf16)))
            }
            Self::Ucs4 { big } => {
                let unit = *bytes.first_chunk::<4>()?;
                let c = if big {
                    u32::from_be_bytes(unit)
                } else {
                    u32::from_le_bytes(unit)
                };
                Some((replaced(char::from_u32(c)), 4))
            }
            Self::Ebcdic(table) => {
                let &b = bytes.first()?;
                let c = char::from_u32(table[usize::from(b)].into());
                Some((replaced(c.filter(|&c| c != '\0' || b == 0)), 1))
            }
        }
    }
}

impl<'s> LineIndex<'s> {
    /// Indexes `input`, the bytes that were parsed.
    pub fn new(input: &'s [u8]) -> Self {
        use crate::encoding::{SourceEncoding, declared};
        let decoder = match input {
            [0xfe, 0xff, ..] | [0, b'<', 0, b'?', ..] => Decoder::Utf16 { big: true },
            [0xff, 0xfe, ..] | [b'<', 0, b'?', 0, ..] => Decoder::Utf16 { big: false },
            #[cfg(feature = "encodings")]
            [0, 0, 0, b'<', ..] => Decoder::Ucs4 { big: true },
            #[cfg(feature = "encodings")]
            [b'<', 0, 0, 0, ..] => Decoder::Ucs4 { big: false },
            #[cfg(feature = "encodings")]
            [0x4c, 0x6f, 0xa7, 0x94, ..] => Decoder::Ebcdic(match declared(input) {
                Some(SourceEncoding::Ebcdic(table)) => table,
                _ => crate::encoding::IBM037,
            }),
            _ => match declared(input) {
                Some(SourceEncoding::Byte(table)) => Decoder::Byte(table),
                _ => {
                    let mut index = Self::utf8(input);
                    index.starts[0] = if input.starts_with(b"\xef\xbb\xbf") {
                        3
                    } else {
                        0
                    };
                    return index;
                }
            },
        };
        let bom = if matches!(input, [0xfe, 0xff, ..] | [0xff, 0xfe, ..]) {
            2
        } else {
            0
        };
        let (mut text, mut starts, mut raw, mut at) = (String::new(), vec![0], vec![bom], bom);
        while let Some((c, length)) = decoder.next(&input[at..]) {
            text.push(c);
            at += length;
            if c == '\n' {
                starts.push(text.len());
                raw.push(at);
            }
        }
        Self {
            input: Cow::Borrowed(input),
            text: Some(text.into_bytes()),
            starts,
            decoded: Some((decoder, raw)),
            raw: None,
            content: false,
        }
    }

    /// The index of [`Transcoded`](crate::Transcoded) text, whose offsets a
    /// transcoded parse reports.
    pub fn transcoded(input: &'s crate::Transcoded<'_>) -> Self {
        match input.decoded {
            Some(_) => Self::utf8(input.as_bytes()),
            None => Self::new(input.as_bytes()),
        }
    }

    /// An index over UTF-8 text, whatever it declares.
    fn utf8(text: &'s [u8]) -> Self {
        let starts = std::iter::once(0)
            .chain(memchr::memchr_iter(b'\n', text).map(|i| i + 1))
            .collect();
        Self {
            input: Cow::Borrowed(text),
            text: None,
            starts,
            decoded: None,
            raw: None,
            content: false,
        }
    }

    /// Indexes UTF-8 `input` parsed as content in a node's context
    /// ([`Editor::parse_content`](crate::Editor::parse_content)): errors
    /// are described as libxml2's content parser reports them (an end tag
    /// with nothing open is `XML_ERR_NOT_WELL_BALANCED`, there is no
    /// prolog or epilog, and `<?xml` is an ordinary misnamed PI).
    ///
    /// ```
    /// use yaxml::{EditErrorKind, EditLimits, Error, LineIndex, OwnedDocument, ParseOptions};
    /// let mut document = OwnedDocument::parse("<r/>")?;
    /// let r = document.root_element().id();
    /// let input = b"<a/></r>";
    /// let failure = document
    ///     .edit(EditLimits::default())?
    ///     .parse_content(r, input, ParseOptions::default())
    ///     .unwrap_err();
    /// let (EditErrorKind::Parse(kind), Some(offset)) = (failure.kind, failure.value_offset) else {
    ///     unreachable!()
    /// };
    /// let diagnostic = Error { kind, offset }.describe(&LineIndex::content(input));
    /// assert_eq!((diagnostic.code, diagnostic.column), (85, 5));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn content(input: &'s [u8]) -> Self {
        Self {
            content: true,
            ..Self::utf8(input)
        }
    }

    /// [`LineIndex::new`] over an input it owns, to keep beside a long-lived tree.
    pub fn owned(input: Vec<u8>) -> LineIndex<'static> {
        let LineIndex {
            text,
            starts,
            decoded,
            ..
        } = LineIndex::new(&input);
        LineIndex {
            input: Cow::Owned(input),
            text,
            starts,
            decoded,
            raw: None,
            content: false,
        }
    }

    /// The indexed input.
    pub fn input(&self) -> &[u8] {
        &self.input
    }

    /// The input as UTF-8.
    fn text(&self) -> &[u8] {
        self.text.as_deref().unwrap_or(&self.input)
    }

    /// The text offset of an input offset (clamped to the input).
    fn to_text(&self, offset: usize) -> usize {
        let Some((decoder, raw)) = &self.decoded else {
            return offset.min(self.text().len());
        };
        let line = raw.partition_point(|&start| start <= offset).max(1) - 1;
        let (mut at, mut width) = (raw[line], 0);
        while let Some((c, length)) = decoder
            .next(&self.input[at..])
            .filter(|&(_, l)| at + l <= offset)
        {
            at += length;
            width += c.len_utf8();
        }
        (self.starts[line] + width).min(self.text().len())
    }

    /// 1-based line of an input offset.
    pub fn line(&self, offset: usize) -> usize {
        self.text_line(self.to_text(offset))
    }

    /// 1-based column of an input offset, in characters.
    pub fn column(&self, offset: usize) -> usize {
        self.text_column(self.to_text(offset))
    }

    /// The index of an HTML document's input: offsets count bytes of the
    /// text the HTML parser decoded (BOM skipped, `<meta>` and guessed
    /// charsets applied), which is what HTML node origins and
    /// [`html::Diagnostic`](crate::html::Diagnostic) offsets are. `document`
    /// must be the one parsed from `input` ([`html::parse`](crate::html::parse)
    /// or [`html::parse_fragment`](crate::html::parse_fragment)); for another
    /// document this is [`LineIndex::new`].
    #[cfg(feature = "html")]
    pub fn html(input: &'s [u8], document: &crate::OwnedDocument) -> Self {
        match document.html_decoding() {
            Some(decoding) => Self::decoded_html(input, decoding),
            None => Self::new(input),
        }
    }

    #[cfg(feature = "html")]
    pub(crate) fn decoded_html(input: &'s [u8], decoding: &crate::html::Decoding) -> Self {
        let (text, raw) = decoding.text(input);
        let starts = std::iter::once(0)
            .chain(memchr::memchr_iter(b'\n', &text).map(|i| i + 1))
            .collect();
        // Unconverted text is the input itself (after a BOM).
        let (input, text) = match text {
            Cow::Borrowed(body) => (Cow::Borrowed(body), None),
            Cow::Owned(text) => (Cow::Borrowed(input), Some(text)),
        };
        Self {
            input,
            text,
            starts,
            decoded: None,
            raw,
            content: false,
        }
    }

    /// libxml2's context lines for an HTML report at text offset `at`, from
    /// the text before a charset switch when it was raised `early`.
    #[cfg(feature = "html")]
    pub(crate) fn html_context(&self, at: usize, early: bool, bytes: bool) -> String {
        let text = self.raw.filter(|_| early).unwrap_or(self.text());
        let at = at.min(text.len());
        // The window reaches 100 bytes back and forward.
        let (from, to) = (at.saturating_sub(100), (at + 100).min(text.len()));
        context(&text[from..to], at - from, bytes)
    }

    pub(crate) fn text_line(&self, at: usize) -> usize {
        self.starts.partition_point(|&start| start <= at).max(1)
    }

    pub(crate) fn text_column(&self, at: usize) -> usize {
        let at = at.min(self.text().len());
        let start = self.starts[self.text_line(at) - 1].min(at);
        1 + self.text()[start..at]
            .iter()
            .filter(|&&b| b & 0xc0 != 0x80)
            .count()
    }

    /// The text offset of a 1-based line and column.
    fn text_at(&self, line: usize, column: usize) -> usize {
        let start = self.starts[line.clamp(1, self.starts.len()) - 1];
        let rest = &self.text()[start.min(self.text().len())..];
        let width = rest
            .iter()
            .enumerate()
            .filter(|&(_, &b)| b & 0xc0 != 0x80)
            .nth(column.saturating_sub(1))
            .map_or(rest.len(), |(i, _)| i);
        start + width
    }

    /// libxml2's line for the element whose start tag begins at text offset
    /// `start`: where the tag ends, as `xmlSAX2StartElementNs` records it.
    fn tag_line(&self, start: usize) -> usize {
        self.text_line(unquoted(self.text(), start + 1, b">").unwrap_or(self.text().len()))
    }

    /// `xmlGetLineNo` for a parsed node: elements count where their start
    /// tag ends, text, comments and PIs where they end, other nodes take
    /// their previous sibling's line or else their parent element's.
    pub(crate) fn node_line<T: Navigate>(&self, tree: &T, id: T::Id) -> Option<usize> {
        let own = |id| match tree.item(id) {
            Item::Element(_) => Some(true),
            Item::Text(_) if !tree.is_cdata(id) => Some(false),
            Item::Comment(_) | Item::Pi(..) => Some(false),
            _ => None,
        };
        let Some(element) = own(id) else {
            let parent = tree.parent(id)?;
            let siblings =
                std::iter::successors(tree.first_child(parent), |&s| tree.next_sibling(s));
            // Navigation hides a DOCTYPE from its siblings: it has none.
            let previous = siblings
                .take_while(|&s| s != id)
                .last()
                .filter(|&s| tree.next_sibling(s) == Some(id) && own(s).is_some());
            let from = previous.or((own(parent) == Some(true)).then_some(parent))?;
            return self.node_line(tree, from);
        };
        let range = tree.range(id)?;
        if tree.marks_lines() {
            // The text offset whose line libxml2 recorded.
            return Some(self.text_line(range.start));
        }
        let range = Some(range).filter(|range| range.start < self.input.len())?;
        Some(if element {
            self.tag_line(self.to_text(range.start))
        } else {
            self.line(range.end)
        })
    }
}

impl LineIndex<'_> {
    fn diagnostic(&self, found: Found, kind: ErrorKind, work: &Work) -> Diagnostic {
        let (domain, level) = match kind {
            ErrorKind::Warning(warning) => {
                let (_, domain, level) = warning.raised();
                (domain, level)
            }
            _ => raised(found.code),
        };
        let line = self.text_line(found.at);
        // Counting a column's characters is charged too; without work it is unknown.
        let start = self.starts[line - 1].min(found.at);
        let column = || {
            work.charge(found.at - start)
                .map_or(0, |()| self.text_column(found.at))
        };
        let (line, column) = found.place.unwrap_or_else(|| (line, column()));
        let [str1, str2, str3] = found.strs.map(|s| s.map(String::from));
        Diagnostic {
            domain,
            code: found.code,
            level,
            line,
            column,
            message: found.message,
            str1,
            str2,
            str3,
            int1: found.int1,
            entity: found.place.is_some(),
            ..Diagnostic::default()
        }
    }

    /// The errors of a recovered parse as libxml2 reports them, up to its 100;
    /// an `XML_ERR_TAG_NOT_FINISHED` or `XML_ERR_DOCUMENT_EMPTY` after a
    /// fatal error is dropped.
    ///
    /// Work is linear in the input (eight bytes of work per input byte plus
    /// 64 KiB, shared by all errors); once it is spent, the remaining errors
    /// get the context-free [`ErrorKind::libxml2_code`], a generic message
    /// and no column.
    pub fn describe_all(&self, errors: &[Error]) -> Vec<Diagnostic> {
        self.describe_counted(errors).0
    }

    /// [`describe_all`](Self::describe_all) and the work it used against its
    /// ceiling, for tests and fuzzing.
    #[doc(hidden)]
    pub fn describe_counted(&self, errors: &[Error]) -> (Vec<Diagnostic>, usize, usize) {
        let work = Work::new(self.text().len());
        let mut scan = Scan::new(self.content);
        let mut described: Vec<Diagnostic> = Vec::new();
        for error in errors {
            let at = self.to_text(error.offset);
            let found = found(error.kind, self, at, &mut scan, &work);
            let diagnostic = self.diagnostic(found, error.kind, &work);
            let late = matches!(diagnostic.code, TAG_NOT_FINISHED | DOCUMENT_EMPTY);
            if !(late
                && described
                    .iter()
                    .any(|earlier| earlier.level == Level::Fatal))
            {
                described.push(diagnostic);
            }
        }
        let (used, ceiling) = work.spent();
        (described, used, ceiling)
    }

    /// Where libxml2's input cursor is when it raises `code` at `at`: for a
    /// misplaced `]]>` the ASCII fast path of `xmlParseCharDataInternal` has
    /// not moved past the text run's start or its last `\r\n`. Character
    /// data holds no `<`, so the run starts after the `>` that ends the
    /// markup before it. The cursor is one byte past a predefined entity's
    /// redeclaration reported at its value's closing quote (or a reference's
    /// `;`), past an `xml:id` type error at a `;`, and past an `xml:id`
    /// value error not at the tag end.
    fn cursor(&self, code: u16, at: usize) -> usize {
        let t = self.text();
        let lags = match code {
            REDECL_PREDEF_ENTITY => matches!(t.get(at), Some(b'"' | b'\'' | b';')),
            DTD_XMLID_TYPE => t.get(at) == Some(&b';'),
            // The column lags the tag end after an expanded entity.
            DTD_XMLID_VALUE => !matches!(t.get(at), Some(b'/' | b'>')),
            _ => false,
        };
        if lags {
            return at + 1;
        }
        if code != MISPLACED_CDATA_END {
            return at;
        }
        let markup = memchr::memrchr(b'<', &t[..at]).unwrap_or(0);
        let start = memchr::memchr(b'>', &t[markup..at]).map_or(markup, |gt| markup + gt + 1);
        let run = &t[start..at];
        if run
            .iter()
            .any(|&b| b >= 0x80 || (b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r')))
        {
            return at;
        }
        memchr::memmem::rfind(run, b"\r\n").map_or(start, |cr| start + cr + 1)
    }

    /// libxml2's window around a text offset (`xmlParserInputGetWindow`):
    /// up to `size` bytes of its line and the offset's byte column in them.
    pub(crate) fn window(&self, cur: usize, size: usize) -> (Cow<'_, str>, usize) {
        let t = self.text();
        let eol = |p: usize| matches!(t.get(p), Some(b'\n' | b'\r'));
        let mut p = cur.min(t.len());
        while p > 0 && eol(p) {
            p -= 1;
        }
        let mut n = 0;
        while n < size && p > 0 && !eol(p) {
            p -= 1;
            n += 1;
        }
        if n > 0 && eol(p) {
            p += 1;
        } else {
            while p < cur && t[p] & 0xc0 == 0x80 {
                p += 1;
            }
        }
        let (start, col) = (p, cur.saturating_sub(p));
        let (mut n, work) = (0, Work::new(0));
        while let Some((_, len)) = Cur::new(self, &work, start + n)
            .ch()
            .filter(|&(c, _)| !matches!(c, '\0' | '\n' | '\r'))
        {
            if n + len > size {
                break;
            }
            n += len;
        }
        let col = if col >= n {
            if n < size { n } else { size.saturating_sub(1) }
        } else {
            col
        };
        (String::from_utf8_lossy(&t[start..start + n]), col)
    }
}

impl Error {
    /// libxml2's `xmlError` for this error in the input `lines` indexes.
    ///
    /// ```
    /// use yaxml::{Document, LineIndex};
    /// let input = "<a>\n  <b></c>\n</a>";
    /// let error = Document::parse(input).unwrap_err();
    /// let diagnostic = error.describe(&LineIndex::new(input.as_bytes()));
    /// assert_eq!((diagnostic.code, diagnostic.line), (76, 2));
    /// assert_eq!(diagnostic.message, "Opening and ending tag mismatch: b line 2 and c");
    /// ```
    pub fn describe(&self, lines: &LineIndex<'_>) -> Diagnostic {
        let at = lines.to_text(self.offset);
        let work = Work::new(lines.text().len());
        lines.diagnostic(
            found(self.kind, lines, at, &mut Scan::new(lines.content), &work),
            self.kind,
            &work,
        )
    }
}

impl Diagnostic {
    /// `xmlFormatError`'s report: `FILE:LINE: [element NAME: ]DOMAIN LEVEL :
    /// MESSAGE`, then for parser, namespace and I/O errors the input's line
    /// around the position with a caret (and the bytes, for an encoding error).
    pub fn render(&self, file: &str, lines: &LineIndex<'_>) -> String {
        let mut out = self.header(file);
        if self.line == 0
            || self.entity
            || !matches!(self.domain, Domain::Parser | Domain::Namespace | Domain::Io)
        {
            return out;
        }
        let at = lines.text_at(self.line, self.column);
        let cursor = lines.cursor(self.code, at);
        // Only input read without a converter shows its bytes.
        out += &lines.context(
            cursor,
            self.code == INVALID_ENCODING && lines.decoded.is_none(),
        );
        out
    }

    /// The report's first line: `FILE:LINE: [element NAME: ]DOMAIN LEVEL : MESSAGE`.
    pub(crate) fn header(&self, file: &str) -> String {
        let mut out = match self.line {
            line if self.entity => format!("Entity: line {line}: "),
            0 => format!("{file}: "),
            line => format!("{file}:{line}: "),
        };
        if let Some(element) = &self.element {
            out += &format!("element {element}: ");
        }
        out += match self.domain {
            Domain::Parser | Domain::XPointer => "parser ",
            Domain::Namespace => "namespace ",
            Domain::Output => "output ",
            Domain::Regexp => "regexp ",
            Domain::RelaxNgParser => "Relax-NG parser ",
            Domain::Catalog => "Catalog ",
            Domain::C14n => "C14N ",
            Domain::Datatype | Domain::Writer => "",
            Domain::Html => "HTML parser ",
            Domain::Io => "I/O ",
            Domain::XInclude => "XInclude ",
            Domain::XPath => "XPath ",
            Domain::SchemasParser => "Schemas parser ",
            Domain::Valid | Domain::Dtd => "validity ",
            Domain::SchemasValidity => "Schemas validity ",
            Domain::RelaxNgValidity => "Relax-NG validity ",
            Domain::SchematronValidity => "schematron ",
        };
        out += if self.level == Level::Warning {
            "warning : "
        } else {
            "error : "
        };
        out += &self.message;
        out.push('\n');
        out
    }
}

/// libxml2's context lines at `cur` in UTF-8 `text`, as `xmlFormatError`
/// prints them, with the next bytes first when `bytes`.
#[cfg(feature = "html")]
pub(crate) fn context(text: &[u8], cur: usize, bytes: bool) -> String {
    LineIndex::utf8(text).context(cur, bytes)
}

impl LineIndex<'_> {
    /// The input's bytes at `at` (for an invalid encoding), its line and a caret.
    fn context(&self, at: usize, bytes: bool) -> String {
        let mut out = String::new();
        if bytes && at < self.text().len() {
            out += "Bytes:";
            out.extend(
                self.text()[at..]
                    .iter()
                    .take(4)
                    .map(|b| format!(" 0x{b:02X}")),
            );
            out.push('\n');
        }
        let (content, col) = self.window(at, 80);
        let caret: String = content.as_bytes()[..col]
            .iter()
            .map(|&b| if b == b'\t' { '\t' } else { ' ' })
            .collect();
        out + &format!("{content}\n{caret}^\n")
    }
}

// ---- validators and tree nodes ----

impl LineIndex<'_> {
    /// A validator's diagnostic about the element starting at `offset`, as
    /// libxml2 raises it: the element's line as `xmlGetLineNo` counts it, its
    /// name when `named`, no column.
    fn validity(
        &self,
        domain: Domain,
        code: u16,
        warning: bool,
        offset: Option<usize>,
        message: &str,
        named: bool,
    ) -> Diagnostic {
        let at = offset
            .map(|offset| self.to_text(offset))
            .filter(|&at| at < self.text().len());
        let element = at.filter(|_| named).and_then(|at| {
            let work = Work::new(0);
            let name = Cur::new(self, &work, at + 1).name(true)?;
            Some(local(name).to_owned())
        });
        Diagnostic {
            domain,
            code,
            level: if warning {
                Level::Warning
            } else {
                Level::Error
            },
            line: at.map_or(0, |at| self.tag_line(at)),
            message: message.trim_end_matches('\n').to_owned(),
            element,
            ..Diagnostic::default()
        }
    }
}

impl<Id> crate::valid::Diagnostic<Id> {
    /// This diagnostic as libxml2's `xmlError` (`XML_FROM_VALID`), with the
    /// line of its element in the validated input `lines` indexes.
    pub fn describe(&self, lines: &LineIndex<'_>) -> Diagnostic {
        lines.validity(
            Domain::Valid,
            self.code as u16,
            self.code.is_warning(),
            self.offset,
            &self.message,
            true,
        )
    }
}

#[cfg(feature = "schemas")]
impl<Id> crate::relaxng::Diagnostic<Id> {
    /// This diagnostic as libxml2's `xmlError` (`XML_FROM_RELAXNGV`).
    pub fn describe(&self, lines: &LineIndex<'_>) -> Diagnostic {
        lines.validity(
            Domain::RelaxNgValidity,
            self.code as u16,
            false,
            self.offset,
            &self.message,
            true,
        )
    }
}

#[cfg(feature = "schemas")]
impl<Id> crate::xsd::Diagnostic<Id> {
    /// This diagnostic as libxml2's `xmlError` (`XML_FROM_SCHEMASV`, which
    /// names no element).
    pub fn describe(&self, lines: &LineIndex<'_>) -> Diagnostic {
        let (code, warning) = (self.code as u16, self.code.is_warning());
        lines.validity(
            Domain::SchemasValidity,
            code,
            warning,
            self.offset,
            &self.message,
            false,
        )
    }
}

#[cfg(feature = "schemas")]
impl<Id> crate::schematron::Diagnostic<Id> {
    /// This diagnostic as libxml2's `xmlError`: `XML_FROM_SCHEMATRONV` for
    /// asserts and reports, otherwise the domain libxml2 raises it in; the
    /// line is the one validation recorded.
    pub fn describe(&self) -> Diagnostic {
        use crate::schematron::Code;
        let domain = match self.code {
            Code::Assert | Code::Report => Domain::SchematronValidity,
            Code::Internal => Domain::SchemasValidity,
            Code::XPath(_) => Domain::XPath,
            _ => Domain::SchemasParser,
        };
        Diagnostic {
            domain,
            code: self.code.number(),
            line: self.line,
            message: self.message.trim_end_matches('\n').to_owned(),
            element: self.element.clone(),
            ..Diagnostic::default()
        }
    }
}

impl crate::Node<'_, '_> {
    /// `xmlGetLineNo` in the input `lines` indexes: elements count where
    /// their start tag ends, text, comments and PIs where they end, others
    /// take their previous sibling's or parent element's line.
    pub fn line(self, lines: &LineIndex<'_>) -> Option<usize> {
        lines.node_line(self.document(), self.id())
    }
}

// Kept here, beside `Node::line`, rather than in the owned module.
impl<A: crate::allocator::Allocator> crate::OwnedNode<'_, A> {
    /// `xmlGetLineNo` in the input `lines` indexes, as [`crate::Node::line`];
    /// `None` for constructed nodes. An HTML node's origin is the decoded
    /// text offset whose line the parser recorded, so `lines` must be
    /// `LineIndex::html` of its document.
    pub fn line(self, lines: &LineIndex<'_>) -> Option<usize> {
        lines.node_line(self.document(), self.id())
    }
}
