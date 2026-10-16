//! Bounded, namespace-aware XML pull parsing and an arena DOM.
//!
//! The current parser accepts XML 1.0 in UTF-8. DTDs and other encodings are
//! explicitly rejected. This crate is not yet a complete libxml2 replacement.
#![forbid(unsafe_code)]
#![cfg_attr(feature = "allocator-api", feature(allocator_api))]

mod dom;
mod error;
mod escape;
mod reader;

pub use dom::{Document, Node, NodeId, NodeKind};
pub use error::{Error, ErrorKind, Limit};
pub use reader::{Attribute, Declaration, Element, Event, Limits, Name, Reader};
