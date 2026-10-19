#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod buffered;
pub mod c14n;
mod dom;
#[cfg(test)]
mod dtd;
#[cfg(test)]
mod dtd_catalog;
#[cfg(test)]
mod dtd_controller;
#[cfg(test)]
mod dtd_frames;
#[cfg(test)]
mod dtd_scan;
#[cfg(test)]
mod dtd_tag;
#[cfg(test)]
mod dtd_writer;
mod encoding;
mod error;
mod escape;
mod name;
mod origin;
mod owned;
mod parser;
mod push;
mod reader;
mod serialize;
mod stream;
mod tokenizer;
pub mod uri;
mod writer;
pub mod xpath;

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
pub use reader::{Attribute, Declaration, Element, Event, Limits, Reader};
pub use uri::{Uri, UriError};
pub use writer::{Writer, WriterLimits};

#[cfg(test)]
mod dtd_reference;

#[cfg(test)]
#[path = "../tests/internal/dtd_content_foundations_spec.rs"]
mod dtd_content_foundations_spec;

#[cfg(test)]
#[path = "../tests/internal/dtd_text_reference_spec.rs"]
mod dtd_text_reference_spec;
