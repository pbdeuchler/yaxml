//! libxml2's `xmlParserOption` flags and [`ParseOptions`], which every XML
//! parser and DOM takes.
//!
//! Flags keep libxml2's names and numeric values, so a binding can pass an
//! existing `int`. A surface that does not implement a flag returns
//! [`ErrorKind::UnsupportedOption`] instead of ignoring it; flags that have no
//! observable effect in libxml2 2.15.4 are accepted. See
//! [option compatibility](https://github.com/pbdeuchler/yaxml/blob/main/docs/compatibility.md#option-compatibility).
//!
//! The default is libxml2's: entity references stay in the tree as
//! [`NodeKind::EntityRef`](crate::NodeKind::EntityRef) nodes and no DTD
//! defaults are added. String values see through references.
//!
//! ```
//! use yaxml::{Document, ParseOptions, options};
//!
//! let xml = "<!DOCTYPE r [<!ENTITY e '<b>x</b>'><!ATTLIST r a CDATA 'd'>]><r>&e;</r>";
//! let kept = Document::parse(xml)?;
//! assert_eq!(kept.root_element().first_child().unwrap().entity_name(), Some("e"));
//! assert_eq!(kept.root_element().text_chunks().collect::<String>(), "x");
//! assert!(kept.root_element().attributes().is_empty());
//!
//! let options = ParseOptions::new(options::NOENT | options::DTDATTR);
//! let complete = Document::parse_with(xml.as_bytes(), options)?;
//! assert_eq!(complete.root_element().first_child().unwrap().name().unwrap().local(), "b");
//! assert_eq!(complete.root_element().attributes()[0].value, "d");
//! # Ok::<(), yaxml::Error>(())
//! ```

use std::fmt;

use crate::{DecodeLimits, Error, ErrorKind, Limits, Resolver, ResourceKind};

/// `XML_PARSE_RECOVER`: build what is parsable of a malformed document, as
/// libxml2 does (DOMs only).
///
/// [`Document::errors`](crate::Document::errors) and
/// [`OwnedDocument::errors`](crate::OwnedDocument::errors) list the errors
/// recovered from; namespace errors are listed without `RECOVER` too.
/// Resource limits stay fatal, and input without any element is still an
/// error. See
/// [the repairs](https://github.com/pbdeuchler/yaxml/blob/main/docs/compatibility.md#recovery-recover).
///
/// ```
/// use yaxml::{Document, ErrorKind, ParseOptions, options, save};
///
/// let document = Document::parse_with(b"<r><a>text</b><p:c/>", ParseOptions::new(options::RECOVER))?;
/// let kinds: Vec<_> = document.errors().iter().map(|error| error.kind).collect();
/// assert_eq!(kinds, [ErrorKind::MismatchedEndTag, ErrorKind::Namespace, ErrorKind::UnexpectedEof]);
/// let saved = save::to_vec(&document, &Default::default()).unwrap();
/// assert_eq!(saved, b"<?xml version=\"1.0\"?>\n<r><a>text</a><p:c/></r>\n");
/// # Ok::<(), yaxml::Error>(())
/// ```
pub const RECOVER: u32 = 1 << 0;
/// `XML_PARSE_NOENT`: substitute entity references instead of keeping
/// entity-reference nodes, and load external parsed entities (with a resolver).
pub const NOENT: u32 = 1 << 1;
/// `XML_PARSE_DTDLOAD`: load the external subset and external parameter
/// entities through the resolver.
pub const DTDLOAD: u32 = 1 << 2;
/// `XML_PARSE_DTDATTR`: add DTD default attributes; implies `DTDLOAD`.
pub const DTDATTR: u32 = 1 << 3;
/// `XML_PARSE_DTDVALID`: validate against the DTD after parsing (DOMs only);
/// loads the external subset and entities like `DTDLOAD | NOENT` would.
pub const DTDVALID: u32 = 1 << 4;
/// `XML_PARSE_NOERROR`: accepted; yaxml returns errors and never prints them.
pub const NOERROR: u32 = 1 << 5;
/// `XML_PARSE_NOWARNING`: accepted; yaxml returns warnings and never prints them.
pub const NOWARNING: u32 = 1 << 6;
/// `XML_PARSE_PEDANTIC`: accepted; it only adds printed warnings in libxml2.
pub const PEDANTIC: u32 = 1 << 7;
/// `XML_PARSE_NOBLANKS`: drop ignorable whitespace text (DOMs and `TextReader`), with
/// libxml2's `areBlanks` rules.
pub const NOBLANKS: u32 = 1 << 8;
/// `XML_PARSE_SAX1`: accepted; yaxml has no SAX1 handlers to prefer.
pub const SAX1: u32 = 1 << 9;
/// `XML_PARSE_XINCLUDE`: process XInclude after parsing (`OwnedDocument` only).
pub const XINCLUDE: u32 = 1 << 10;
/// `XML_PARSE_NONET`: accepted; parsing never uses the network.
pub const NONET: u32 = 1 << 11;
/// `XML_PARSE_NODICT`: accepted; strings are never interned in a dictionary.
pub const NODICT: u32 = 1 << 12;
/// `XML_PARSE_NSCLEAN`: drop namespace declarations that rebind a prefix to
/// the URI it already has.
pub const NSCLEAN: u32 = 1 << 13;
/// `XML_PARSE_NOCDATA`: report CDATA sections as text.
pub const NOCDATA: u32 = 1 << 14;
/// `XML_PARSE_NOXINCNODE`: accepted; XInclude never adds start/end marker nodes.
pub const NOXINCNODE: u32 = 1 << 15;
/// `XML_PARSE_COMPACT`: accepted; storage layout is not configurable.
pub const COMPACT: u32 = 1 << 16;
/// `XML_PARSE_OLD10`: pre-Fifth-Edition name rules; not implemented, rejected.
pub const OLD10: u32 = 1 << 17;
/// `XML_PARSE_NOBASEFIX`: no `xml:base` fixup of XInclude results.
pub const NOBASEFIX: u32 = 1 << 18;
/// `XML_PARSE_HUGE`: raise the token and DTD byte ceilings (1 GB), depth
/// (2048) and entity depth (40) to libxml2's huge values.
pub const HUGE: u32 = 1 << 19;
/// `XML_PARSE_OLDSAX`: legacy predefined-entity lookup order; not
/// implemented, rejected.
pub const OLDSAX: u32 = 1 << 20;
/// `XML_PARSE_IGNORE_ENC`: ignore the declared encoding (BOM or UTF-8).
pub const IGNORE_ENC: u32 = 1 << 21;
/// `XML_PARSE_BIG_LINES`: accepted; positions are byte offsets, never truncated.
pub const BIG_LINES: u32 = 1 << 22;
/// `XML_PARSE_NO_XXE`: never load the external subset or external entities,
/// even with a resolver.
pub const NO_XXE: u32 = 1 << 23;
/// `XML_PARSE_UNZIP`: input decompression; not implemented, rejected.
pub const UNZIP: u32 = 1 << 24;
/// `XML_PARSE_NO_SYS_CATALOG`: accepted; there is never a system catalog.
pub const NO_SYS_CATALOG: u32 = 1 << 25;
/// `XML_PARSE_CATALOG_PI`: a `<?oasis-xml-catalog catalog="URL"?>` instruction
/// outside the DTD is passed to the resolver ([`Resolver::catalog_pi`]), which
/// [`crate::catalog::CatalogResolver`] adds as a catalog for the rest of the parse.
/// Without such a resolver it has no effect.
pub const CATALOG_PI: u32 = 1 << 26;
/// `XML_PARSE_SKIP_IDS`: record no ID attributes (`element_by_id`, XPath `id()`).
pub const SKIP_IDS: u32 = 1 << 27;

/// `NOENT | DTDATTR`: substitute entities and add default attributes.
pub const LEGACY: u32 = NOENT | DTDATTR;

/// Flags with no implementation on any surface.
const NEVER: u32 = OLD10 | OLDSAX | UNZIP;
/// Flags only a tree can implement.
const TREE_ONLY: u32 = DTDVALID | XINCLUDE;
const KNOWN: u32 = (1 << 28) - 1;

/// Parser flags, resource ceilings and the resource loader.
///
/// The default has no flags and no resolver, so nothing external is loaded.
#[non_exhaustive]
pub struct ParseOptions {
    /// Bitwise or of this module's flag constants.
    pub flags: u32,
    /// Parser ceilings.
    pub limits: Limits,
    /// Ceilings on decoded UTF-8.
    pub decode_limits: DecodeLimits,
    /// The document's URI or path, against which relative system identifiers resolve.
    pub base: Option<String>,
    /// Loader for external resources, consulted only for what the flags
    /// request (`DTDLOAD`, `DTDATTR`, `DTDVALID`, `NOENT`, `XINCLUDE`) and
    /// never with `NO_XXE`.
    pub resolver: Option<Box<dyn Resolver>>,
}

setters!(fn ParseOptions { flags: u32, limits: Limits, decode_limits: DecodeLimits, base: Option<String>, resolver: Option<Box<dyn Resolver>> });

impl Default for ParseOptions {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl fmt::Debug for ParseOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParseOptions")
            .field("flags", &format_args!("{:#x}", self.flags))
            .field("limits", &self.limits)
            .field("decode_limits", &self.decode_limits)
            .field("base", &self.base)
            .field("resolver", &self.resolver.as_ref().map(|_| ".."))
            .finish()
    }
}

impl ParseOptions {
    /// No flags, default limits, no base and no resolver.
    pub const DEFAULT: Self = Self {
        flags: 0,
        limits: Limits::DEFAULT,
        decode_limits: DecodeLimits::DEFAULT,
        base: None,
        resolver: None,
    };

    /// Default options with `flags` set.
    pub fn new(flags: u32) -> Self {
        Self {
            flags,
            ..Self::default()
        }
    }

    /// Replaces the parser ceilings.
    pub fn with_limits(self, limits: Limits) -> Self {
        Self { limits, ..self }
    }

    /// Replaces the decoded-UTF-8 ceilings.
    pub fn with_decode_limits(self, decode_limits: DecodeLimits) -> Self {
        Self {
            decode_limits,
            ..self
        }
    }

    /// Supplies the resource loader and the document base; the flags still
    /// decide what is loaded.
    pub fn with_resolver(self, base: &str, resolver: Box<dyn Resolver>) -> Self {
        Self {
            base: Some(base.to_owned()),
            resolver: Some(resolver),
            ..self
        }
    }

    /// Whether any bit of `flag` is set.
    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

    /// The ceilings in effect, after [`HUGE`] raises them.
    pub fn effective_limits(&self) -> (Limits, DecodeLimits) {
        if !self.has(HUGE) {
            return (self.limits, self.decode_limits);
        }
        const HUGE_LENGTH: usize = 1_000_000_000;
        let limits = Limits {
            max_token_bytes: self.limits.max_token_bytes.max(HUGE_LENGTH),
            max_depth: self.limits.max_depth.max(2048),
            max_entity_depth: self.limits.max_entity_depth.max(40),
            max_dtd_bytes: self.limits.max_dtd_bytes.max(HUGE_LENGTH),
            ..self.limits
        };
        let decode = DecodeLimits {
            max_decoded_token_bytes: self.decode_limits.max_decoded_token_bytes.max(HUGE_LENGTH),
            ..self.decode_limits
        };
        (limits, decode)
    }

    /// Rejects unknown flags, unimplemented ones, and tree-only flags on a
    /// streaming surface (`tree`: false), or `extra` flags the surface lacks.
    pub(crate) fn check(&self, tree: bool, unsupported: u32) -> Result<Flags, Error> {
        let rejected = !KNOWN | NEVER | unsupported | if tree { 0 } else { TREE_ONLY };
        match self.flags & rejected {
            0 => Ok(Flags(self.flags)),
            _ => Err(Error::new(ErrorKind::UnsupportedOption, 0)),
        }
    }
}

/// Validated flags, as the parser internals consult them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Flags(pub(crate) u32);

impl Flags {
    pub(crate) const LEGACY: Self = Self(LEGACY);
    /// Loads every external resource a resolver offers.
    pub(crate) const LOAD_ALL: Self = Self(DTDLOAD | NOENT);

    fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }

    /// Entity references are replaced, so no entity boundaries are reported.
    pub(crate) fn substitutes(self) -> bool {
        self.has(NOENT)
    }

    pub(crate) fn default_attributes(self) -> bool {
        self.has(DTDATTR)
    }

    pub(crate) fn cdata_as_text(self) -> bool {
        self.has(NOCDATA)
    }

    pub(crate) fn clean_namespaces(self) -> bool {
        self.has(NSCLEAN)
    }

    pub(crate) fn ignores_encoding(self) -> bool {
        self.has(IGNORE_ENC)
    }

    pub(crate) fn records_ids(self) -> bool {
        !self.has(SKIP_IDS)
    }

    pub(crate) fn drops_blanks(self) -> bool {
        self.has(NOBLANKS)
    }

    pub(crate) fn recovers(self) -> bool {
        self.has(RECOVER)
    }

    pub(crate) fn validates(self) -> bool {
        self.has(DTDVALID)
    }

    pub(crate) fn includes(self) -> bool {
        self.has(XINCLUDE)
    }

    pub(crate) fn fixes_base(self) -> bool {
        !self.has(NOBASEFIX)
    }

    pub(crate) fn catalog_pi(self) -> bool {
        self.has(CATALOG_PI)
    }

    /// Whether libxml2 would load `kind` with these flags (`xmlSAX2ExternalSubset`,
    /// `xmlParsePERefInternal`, `xmlParseReference`).
    pub(crate) fn loads(self, kind: ResourceKind) -> bool {
        let wanted = match kind {
            ResourceKind::DtdSubset => DTDLOAD | DTDATTR | DTDVALID,
            ResourceKind::ParameterEntity => DTDLOAD | DTDATTR | DTDVALID | NOENT,
            ResourceKind::GeneralEntity => NOENT | DTDVALID,
            _ => return true,
        };
        !self.has(NO_XXE) && self.has(wanted)
    }
}

#[cfg(test)]
#[path = "../tests/internal/options.rs"]
mod tests;
