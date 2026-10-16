#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod dom;
mod error;
mod escape;
mod parser;
mod push;
mod reader;
mod tokenizer;
mod writer;

pub use dom::{Document, Node, NodeId, NodeKind};
pub use error::{Error, ErrorKind, Limit, ReadError, WriteError};
pub use push::PushParser;
pub use reader::{Attribute, Declaration, Element, Event, Limits, Name, Reader};
pub use writer::{Writer, WriterLimits};
