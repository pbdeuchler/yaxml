#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod buffered;
mod dom;
mod encoding;
mod error;
mod escape;
mod owned;
mod parser;
mod push;
mod reader;
mod stream;
mod tokenizer;
mod writer;

pub use buffered::BufferedReader;
pub use dom::{Document, Node, NodeId, NodeKind};
pub use encoding::DecodeLimits;
pub use error::{Error, ErrorKind, Limit, ReadError, WriteError};
pub use owned::{OwnedDocument, OwnedNode, OwnedNodeKind};
pub use push::PushParser;
pub use reader::{Attribute, Declaration, Element, Event, Limits, Name, Reader};
pub use writer::{Writer, WriterLimits};
