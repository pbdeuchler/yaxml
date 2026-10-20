#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
// The allocator API is stable from Rust 1.100: the feature raises the MSRV.
#![cfg_attr(feature = "allocator_api", allow(clippy::incompatible_msrv))]

#[macro_use]
mod setters;
mod allocator;
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
pub use encoding::DecodeLimits;
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
    AttributeSpec, EditLimits, Editor, ElementItem, ElementSpec, NameSpec, NamespaceSpec,
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
