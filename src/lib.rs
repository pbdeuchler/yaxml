//! A safe XML toolkit that behaves like libxml2 2.15.4.
//!
//! yaxml parses, queries, validates, edits and writes XML (and HTML) with
//! libxml2's results: the same trees, error codes and messages, serialized
//! bytes and validation verdicts. It has no `unsafe` code and one dependency,
//! `memchr`.
//!
//! # Choosing an API
//!
//! | Need | Use |
//! | --- | --- |
//! | Read-only tree over a `&str` or `&[u8]`, without copying | [`Document`] |
//! | Tree that owns its strings, outlives the input, or is edited | [`OwnedDocument`] and [`Editor`] |
//! | Pull events from a complete slice | [`Reader`] |
//! | Feed byte chunks as they arrive | [`PushParser`] |
//! | Pull events from any [`BufRead`](std::io::BufRead) in bounded memory | [`BufferedReader`] |
//! | libxml2's `xmlTextReader` cursor | [`TextReader`] |
//! | Write events or trees | [`Writer`], [`save`], [`TextWriter`] (`xmlTextWriter`) |
//!
//! Modules by task: [`xpath`], [`xpointer`] and [`pattern`] select nodes;
//! [`valid`] (DTD), [`xsd`], [`relaxng`] and [`schematron`] validate;
//! [`c14n`] canonicalizes; [`xinclude`] merges documents; [`catalog`] maps
//! identifiers; [`html`] parses and writes HTML; [`schema_types`],
//! [`regexp`], [`uri`] and [`chars`] expose the building blocks.
//!
//! # Options and errors
//!
//! Every XML parser takes one [`ParseOptions`], whose flags are libxml2's
//! `XML_PARSE_*` values ([`options`]); a surface rejects a flag it does not
//! implement with [`ErrorKind::UnsupportedOption`]. An [`Error`] is a kind
//! and a byte offset, and [`Error::describe`] expands it into libxml2's
//! `xmlError` fields ([`Diagnostic`]).
//!
//! # Security
//!
//! Nothing is read from disk or the network unless you pass a [`Resolver`],
//! and every input-controlled resource (depth, sizes, counts, entity
//! expansion, memory) has a finite default limit in [`Limits`]. See
//! [security](https://github.com/pbdeuchler/yaxml/blob/main/docs/security.md).
//!
//! # Example
//!
//! ```
//! use yaxml::{Document, xpath};
//!
//! let document = Document::parse("<shop><item price='3'/><item price='4'/></shop>")?;
//! let prices: Vec<_> = document.root_element().children().filter_map(|i| i.attribute("price")).collect();
//! assert_eq!(prices, ["3", "4"]);
//!
//! let total = xpath::XPath::compile("sum(//item/@price)")?;
//! let value = total.evaluate(&document, document.root().id(), &xpath::Context::new())?;
//! assert_eq!(value, xpath::Value::Number(7.0));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]
// The allocator API is stable from Rust 1.100: the feature raises the MSRV.
#![cfg_attr(feature = "allocator_api", allow(clippy::incompatible_msrv))]

#[macro_use]
mod setters;
mod allocator;
mod blanks;
mod buffered;
pub mod c14n;
pub mod catalog;
pub mod chars;
pub mod diagnostic;
mod dom;
mod dtd;
mod dtd_catalog;
mod dtd_controller;
mod dtd_frames;
mod dtd_scan;
mod dtd_tag;
mod encoding;
mod error;
mod escape;
mod fallible;
pub mod features;
pub mod html;
mod name;
pub mod options;
mod origin;
mod owned;
mod parser;
pub mod pattern;
mod push;
mod reader;
pub mod regexp;
pub mod relaxng;
mod resolve;
pub mod save;
pub mod schema_types;
pub mod schematron;
mod serialize;
mod stream;
mod textreader;
mod textwriter;
mod tokenizer;
pub mod uri;
pub mod valid;
mod writer;
pub mod xinclude;
pub mod xpath;
pub mod xpointer;
pub mod xsd;

pub use buffered::BufferedReader;
pub use diagnostic::{Diagnostic, LineIndex};
pub use dom::{Document, Node, NodeId, NodeKind};
pub use encoding::{DecodeLimits, Transcoded};
pub(crate) use error::DtdError;
pub use error::{
    EditComponent, EditError, EditErrorKind, EditLimit, EditLocation, Error, ErrorKind, Limit,
    ReadError, WriteError,
};
pub use features::{LIBXML2_COMPAT_VERSION, LIBXML2_COMPAT_VERSION_NUMBER, VERSION};
pub use name::Name;
pub use options::ParseOptions;
pub use origin::SourceOrigin;
pub use owned::{
    AttributeSpec, EditLimits, Editor, ElementItem, ElementSpec, Fragment, NameSpec, NamespaceSpec,
    OwnedDocument, OwnedNode, OwnedNodeId, OwnedNodeKind, PrefixPolicy,
};
pub use push::PushParser;
pub use reader::{Attribute, Attributes, Declaration, Doctype, Element, Event, Limits, Reader};
pub use resolve::{FileResolver, Request, Resolver, ResourceKind};
pub use textreader::{NodeType, ParserProperty, ReadState, Standalone, TextReader};
pub use textwriter::TextWriter;
pub use uri::{Uri, UriError};
pub use writer::{Writer, WriterLimits};
pub use xpath::tree::NamespaceBinding;

mod dtd_reference;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
