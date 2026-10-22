//! libxml2's document property bits (`xmlDocProperties`, `doc->properties`),
//! returned by `Document::properties` and `OwnedDocument::properties`.
//!
//! A parsed XML document is [`WELLFORMED`] unless a `RECOVER` parse recovered
//! from an error; then it is also [`NSVALID`] without namespace errors and
//! [`DTDVALID`] unless `DTDVALID` validation failed, an undeclared entity was
//! referenced or an element or notation was declared twice (libxml2 clears
//! `ctxt->valid` for those even without validation). A parsed HTML document
//! is [`HTML`]; a document built by `OwnedDocument::create` or copied by
//! `try_clone` is [`USERBUILT`] (`xmlNewDoc`, which `xmlCopyDoc` calls).
//! A document parsed with `XML_PARSE_OLD10` is also [`OLD10`], even when
//! recovered from errors. [`XINCLUDE`] and [`INTERNAL`] are never set:
//! libxml2 2.15.4 never sets `XINCLUDE`, and `INTERNAL` marks its scratch
//! documents.

use crate::error::{Error, ErrorKind, Warning};
use crate::valid::{Code, Diagnostic, Dtd};

/// `XML_DOC_WELLFORMED`: no well-formedness error.
pub const WELLFORMED: u32 = 1 << 0;
/// `XML_DOC_NSVALID`: no namespace error.
pub const NSVALID: u32 = 1 << 1;
/// `XML_DOC_OLD10`: parsed with `XML_PARSE_OLD10`.
pub const OLD10: u32 = 1 << 2;
/// `XML_DOC_DTDVALID`: nothing made the document invalid.
pub const DTDVALID: u32 = 1 << 3;
/// `XML_DOC_XINCLUDE`: never set, as in libxml2 2.15.4.
pub const XINCLUDE: u32 = 1 << 4;
/// `XML_DOC_USERBUILT`: built through the API, not parsed.
pub const USERBUILT: u32 = 1 << 5;
/// `XML_DOC_INTERNAL`: libxml2's internal scratch documents; never set.
pub const INTERNAL: u32 = 1 << 6;
/// `XML_DOC_HTML`: a parsed HTML document.
pub const HTML: u32 = 1 << 7;

/// The bits a kept parse error clears.
pub(crate) const fn clears(kind: ErrorKind) -> u32 {
    match kind {
        ErrorKind::Namespace | ErrorKind::Warning(Warning::NamespaceUri) => NSVALID,
        ErrorKind::Warning(Warning::UndeclaredEntity { .. }) => DTDVALID,
        ErrorKind::Warning(_) => 0,
        _ => WELLFORMED,
    }
}

/// The properties of a parsed XML document (`xmlSAX2EndDocument`).
pub(crate) fn parsed<Id>(errors: &[Error], validity: &[Diagnostic<Id>], dtd: Option<&Dtd>) -> u32 {
    let cleared = errors
        .iter()
        .fold(0, |bits, error| bits | clears(error.kind));
    let redefined = dtd.is_some_and(|dtd| {
        let redefinition =
            |code: &Code| matches!(code, Code::ElementRedefined | Code::NotationRedefined);
        dtd.issues.iter().any(|(code, ..)| redefinition(code))
    });
    let invalid = !crate::valid::is_valid(validity) || redefined;
    match cleared & WELLFORMED {
        0 => (WELLFORMED | NSVALID | if invalid { 0 } else { DTDVALID }) & !cleared,
        _ => 0,
    }
}
