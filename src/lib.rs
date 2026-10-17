#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod buffered;
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
mod writer;

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
pub use writer::{Writer, WriterLimits};
