#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod dom;
mod error;
mod escape;
mod reader;

pub use dom::{Document, Node, NodeId, NodeKind};
pub use error::{Error, ErrorKind, Limit};
pub use reader::{Attribute, Declaration, Element, Event, Limits, Name, Reader};
