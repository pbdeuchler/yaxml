#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod dom;
mod error;
mod escape;
mod parser;
mod push;
mod reader;
mod tokenizer;

pub use dom::{Document, Node, NodeId, NodeKind};
pub use error::{Error, ErrorKind, Limit, ReadError};
pub use push::PushParser;
pub use reader::{Attribute, Declaration, Element, Event, Limits, Name, Reader};
