#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod buffered;
pub mod c14n;
pub mod catalog;
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
pub mod html;
mod name;
mod origin;
mod owned;
mod parser;
pub mod pattern;
mod push;
mod reader;
pub mod regexp;
mod resolve;
pub mod schema_types;
mod serialize;
mod stream;
mod textreader;
mod tokenizer;
pub mod uri;
pub mod valid;
mod writer;
pub mod xinclude;
pub mod xpath;
pub mod xpointer;

pub use buffered::BufferedReader;
pub use dom::{Document, Node, NodeId, NodeKind};
pub use encoding::DecodeLimits;
pub use error::{
    EditComponent, EditError, EditErrorKind, EditLimit, EditLocation, Error, ErrorKind, Limit,
    ReadError, WriteError,
};
pub use name::Name;
pub use origin::SourceOrigin;
pub use owned::{
    AttributeSpec, EditLimits, Editor, ElementItem, ElementSpec, NameSpec, NamespaceSpec,
    OwnedDocument, OwnedNode, OwnedNodeId, OwnedNodeKind, PrefixPolicy,
};
pub use push::PushParser;
pub use reader::{Attribute, Declaration, Doctype, Element, Event, Limits, Reader};
pub use resolve::{FileResolver, Request, Resolver, ResourceKind};
pub use textreader::{NodeType, ParserProperty, ReadState, TextReader};
pub use uri::{Uri, UriError};
pub use writer::{Writer, WriterLimits};

mod dtd_reference;

#[cfg(test)]
#[path = "../tests/internal/dtd_content_foundations_spec.rs"]
mod dtd_content_foundations_spec;

#[cfg(test)]
#[path = "../tests/internal/dtd_text_reference_spec.rs"]
mod dtd_text_reference_spec;
