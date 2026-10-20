//! XPointer as libxml2 2.15's `xpointer.c` evaluates it: shorthand names,
//! child sequences, the `element()` and `xmlns()` schemes, and
//! `xpointer()`/`xpath1()` through [`crate::xpath`]. Like libxml2 2.15, it
//! has no range or point extensions.
//!
//! ```
//! let document = yaxml::OwnedDocument::parse("<r><a xml:id='i'><b/><c/></a></r>")?;
//! let limits = yaxml::xpath::Limits::default();
//! for pointer in ["i/2", "element(/1/1/2)", "xpointer(//c)"] {
//!     let nodes = yaxml::xpointer::evaluate(&document, pointer, &limits)?;
//!     assert_eq!(nodes.len(), 1);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;

use crate::name::{name_char, name_start};
use crate::xpath::tree::{Item, element_by_id};
use crate::xpath::{self, Context, Limits, Node, Tree, Value, XPath};

/// Located nodes, in document order.
pub type Nodes<Id> = Vec<Node<Id>>;

/// An XPointer failure, by libxml2 error code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// `XPTR_SYNTAX_ERROR` or `XPATH_EXPR_ERROR`: malformed pointer or part.
    Syntax,
    /// `XML_XPTR_UNKNOWN_SCHEME`. Other parts are still tried, but as in
    /// libxml2 the pointer as a whole fails.
    UnknownScheme,
    /// `XML_XPTR_CHILDSEQ_START`: a child sequence from the root not starting `/1`.
    ChildSequenceStart,
    /// An `xpointer()` expression failed to compile or evaluate.
    XPath(xpath::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::XPath(error) => write!(f, "XPointer: {error}"),
            other => write!(f, "XPointer error {other:?}"),
        }
    }
}

impl std::error::Error for Error {}

/// Evaluates `pointer` against `tree`, returning the located nodes in
/// document order. Parts of a full pointer are tried left to right and the
/// first non-empty node-set wins; text after it is ignored, as in libxml2.
/// A pointer that locates nothing, or an `xpointer()` part of another value
/// type, yields an empty set.
pub fn evaluate<T: Tree>(tree: &T, pointer: &str, limits: &Limits) -> Result<Nodes<T::Id>, Error> {
    let pointer = skip_blanks(pointer);
    let (nodes, rest) = if pointer.starts_with('/') {
        child_sequence(tree, vec![Node::Tree(tree.root())], pointer, true)?
    } else {
        let (name, rest) = split_name(pointer, true).ok_or(Error::Syntax)?;
        if rest.starts_with('(') {
            return full(tree, pointer, limits);
        }
        child_sequence(tree, by_id(tree, name), rest, false)?
    };
    match skip_blanks(rest).is_empty() {
        true => Ok(nodes),
        false => Err(Error::Syntax),
    }
}

/// `FullXPtr ::= XPtrPart (S? XPtrPart)*`.
fn full<T: Tree>(tree: &T, pointer: &str, limits: &Limits) -> Result<Nodes<T::Id>, Error> {
    let mut context = Context::new().with_limits(*limits);
    let mut unknown = false;
    let mut found = Vec::new();
    let mut rest = pointer;
    while let Some((scheme, after)) = split_name(skip_blanks(rest), true) {
        let (body, after) = part_body(after.strip_prefix('(').ok_or(Error::Syntax)?)?;
        rest = after;
        let nodes = match scheme {
            "xpointer" | "xpath1" => {
                let xpath = XPath::compile_with_limits(&body, limits).map_err(Error::XPath)?;
                match xpath.evaluate(tree, tree.root(), &context) {
                    Ok(Value::NodeSet(nodes)) => nodes,
                    Ok(_) => Vec::new(),
                    Err(error) => return Err(Error::XPath(error)),
                }
            }
            "element" if body.starts_with('/') => {
                child_sequence(tree, vec![Node::Tree(tree.root())], &body, true)?.0
            }
            "element" => {
                let (name, rest) = split_name(&body, true).ok_or(Error::Syntax)?;
                child_sequence(tree, by_id(tree, name), rest, false)?.0
            }
            "xmlns" => {
                let (prefix, rest) = split_name(&body, false).ok_or(Error::Syntax)?;
                let uri = skip_blanks(rest).strip_prefix('=').ok_or(Error::Syntax)?;
                context = context.with_namespace(prefix, skip_blanks(uri));
                Vec::new()
            }
            _ => {
                unknown = true;
                Vec::new()
            }
        };
        if !nodes.is_empty() {
            found = nodes;
            break;
        }
    }
    match unknown {
        true => Err(Error::UnknownScheme),
        false => Ok(found),
    }
}

/// The unescaped text up to the `)` that balances an already consumed `(`;
/// `^` escapes `(`, `)` and `^`.
fn part_body(text: &str) -> Result<(String, &str), Error> {
    let mut body = String::new();
    let mut level = 1;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        match c {
            ')' if level == 1 => return Ok((body, &text[index + 1..])),
            ')' => level -= 1,
            '(' => level += 1,
            '^' => {
                if let Some(&(_, escaped @ ('(' | ')' | '^'))) = chars.peek() {
                    chars.next();
                    body.push(escaped);
                    continue;
                }
            }
            _ => {}
        }
        body.push(c);
    }
    Err(Error::Syntax)
}

/// `ChildSeq`: `/n` steps to the n-th element child of a single node.
fn child_sequence<'a, T: Tree>(
    tree: &T,
    mut nodes: Nodes<T::Id>,
    mut text: &'a str,
    from_root: bool,
) -> Result<(Nodes<T::Id>, &'a str), Error> {
    if from_root && !text.starts_with("/1") {
        return Err(Error::ChildSequenceStart);
    }
    while let Some(rest) = text.strip_prefix('/') {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        // libxml2 treats an overflowing index as 0, which selects nothing.
        let index = rest[..digits].parse::<usize>().unwrap_or(0);
        nodes = match nodes.as_slice() {
            [Node::Tree(parent)] if index > 0 => {
                std::iter::successors(tree.first_child(*parent), |&id| tree.next_sibling(id))
                    .filter(|&id| matches!(tree.item(id), Item::Element(_)))
                    .nth(index - 1)
                    .map(Node::Tree)
                    .into_iter()
                    .collect()
            }
            _ => Vec::new(),
        };
        text = &rest[digits..];
    }
    Ok((nodes, text))
}

fn by_id<T: Tree>(tree: &T, name: &str) -> Nodes<T::Id> {
    element_by_id(tree, tree.ids(), name)
        .map(Node::Tree)
        .into_iter()
        .collect()
}

/// The leading XML Name (an NCName without `colon`) and the rest of `text`.
fn split_name(text: &str, colon: bool) -> Option<(&str, &str)> {
    let first = text.chars().next().filter(|&c| name_start(c, colon))?;
    let end = text[first.len_utf8()..]
        .find(|c| !name_char(c, colon))
        .map_or(text.len(), |end| end + first.len_utf8());
    Some(text.split_at(end))
}

fn skip_blanks(text: &str) -> &str {
    text.trim_start_matches([' ', '\t', '\n', '\r'])
}
