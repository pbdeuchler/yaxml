//! Tree output as libxml2 2.15.4's `xmlsave.c` writes it: `xmlSaveDoc` for
//! a document and `xmlSaveTree` for any other node of either DOM, under
//! `xmlSaveOption` flags and an output encoding.
//!
//! The internal subset is regenerated from its declarations, as libxml2
//! does. See the
//! [writer contract](https://github.com/pbdeuchler/yaxml/blob/main/docs/writer.md#libxml2-tree-output-save).
//!
//! ```
//! use yaxml::{OwnedDocument, save};
//!
//! let document = OwnedDocument::parse("<r><a>caf\u{e9}</a><b/></r>")?;
//! let options = save::Options::new(save::FORMAT);
//! assert_eq!(
//!     save::to_vec(&document, &options)?,
//!     b"<?xml version=\"1.0\"?>\n<r>\n  <a>caf&#xE9;</a>\n  <b/>\n</r>\n"
//! );
//! let latin1 = options.encoding(Some("ISO-8859-1"));
//! let a = document.root_element().first_child().unwrap();
//! assert_eq!(save::to_vec(a, &latin1)?, b"<a>caf\xe9</a>");
//! # Ok::<(), yaxml::Error>(())
//! ```

use std::borrow::Cow;
use std::io::Write;

use crate::allocator::Allocator;
use crate::encoding::Output;
use crate::fallible::Memory;
use crate::{
    Attribute, Declaration, Doctype, Document, Error, ErrorKind, Event, Limit, Name, Node,
    NodeKind, OwnedDocument, OwnedNode, OwnedNodeKind, WriteError,
};

pub(crate) mod dtd;

/// `XML_SAVE_FORMAT`: indent element content, except inside elements with
/// text, CDATA or entity-reference children.
pub const FORMAT: u32 = 1 << 0;
/// `XML_SAVE_NO_DECL`: omit the XML declaration.
pub const NO_DECL: u32 = 1 << 1;
/// `XML_SAVE_NO_EMPTY`: write `<a></a>` instead of `<a/>`.
pub const NO_EMPTY: u32 = 1 << 2;
/// `XML_SAVE_NO_XHTML`: do not switch to XHTML rules for an XHTML 1.0 DOCTYPE.
pub const NO_XHTML: u32 = 1 << 3;
/// `XML_SAVE_XHTML`: XHTML 1.0 compatibility rules (Appendix C).
pub const XHTML: u32 = 1 << 4;
/// `XML_SAVE_AS_XML`: save HTML documents as XML.
pub const AS_XML: u32 = 1 << 5;
/// `XML_SAVE_AS_HTML`: save documents with the HTML serializer.
pub const AS_HTML: u32 = 1 << 6;
/// `XML_SAVE_WSNONSIG`: indent with insignificant whitespace inside tags.
pub const WSNONSIG: u32 = 1 << 7;
/// `XML_SAVE_EMPTY`: accepted; there is no global `xmlSaveNoEmptyTags` to override.
pub const EMPTY: u32 = 1 << 8;
/// `XML_SAVE_NO_INDENT`: [`FORMAT`] line breaks without indentation.
pub const NO_INDENT: u32 = 1 << 9;
/// `XML_SAVE_INDENT`: accepted; indentation is on by default (`xmlIndentTreeOutput`).
pub const INDENT: u32 = 1 << 10;
/// Without the `html` feature `AS_HTML` is unsupported (and no document is HTML).
const SUPPORTED: u32 = ((1 << 11) - 1) & !(AS_HTML * !cfg!(feature = "html") as u32);

const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
const XHTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
/// libxml2's `MAX_INDENT`: the longest indent string and widest indentation.
const MAX_INDENT: usize = 60;
/// The deepest starting level `xmlNodeDump` accepts.
const MAX_LEVEL: usize = 100;

/// `xmlSaveOption` flags, the output encoding and the indent string.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Options<'a> {
    /// `XML_SAVE_*` flags from this module's constants.
    pub flags: u32,
    /// The output encoding: UTF-8, UTF-16 (little-endian with a BOM),
    /// UTF-16LE/BE, US-ASCII, ISO-8859-x, windows-1252, and UCS-4/EBCDIC with
    /// feature `encodings`, under any libxml2 alias, written verbatim into the
    /// declaration. `None` uses a saved document's declared encoding; without
    /// one, and for any other node, non-ASCII text is written as `&#xH;`.
    pub encoding: Option<&'a str>,
    /// The indent string (`xmlSaveSetIndentString`): 1 to 60 spaces or tabs.
    /// Default: two spaces.
    pub indent: &'a str,
    /// The indentation level of a subtree's root (`xmlNodeDump`'s `level`),
    /// capped at 100; documents always start at 0.
    pub level: usize,
}

setters!(const Options['a] { flags: u32, encoding: Option<&'a str>, indent: &'a str, level: usize });

impl Options<'_> {
    /// No flags, the document's encoding, two-space indent, level 0.
    pub const DEFAULT: Self = Self {
        flags: 0,
        encoding: None,
        indent: "  ",
        level: 0,
    };
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Options<'_> {
    /// `flags` with default settings.
    pub fn new(flags: u32) -> Self {
        Self {
            flags,
            ..Self::default()
        }
    }

    fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }
}

/// A node of either DOM. A document node saves the whole document.
#[derive(Clone, Copy, Debug)]
pub enum Tree<'document, 'input> {
    /// A [`Document`] node.
    Borrowed(Node<'document, 'input>),
    /// An [`OwnedDocument`] node.
    Owned(OwnedNode<'document>),
}

impl<'document, 'input> From<Node<'document, 'input>> for Tree<'document, 'input> {
    fn from(node: Node<'document, 'input>) -> Self {
        Self::Borrowed(node)
    }
}

impl<'document, 'input> From<OwnedNode<'document>> for Tree<'document, 'input> {
    fn from(node: OwnedNode<'document>) -> Self {
        Self::Owned(node)
    }
}

impl<'document, 'input> From<&'document Document<'input>> for Tree<'document, 'input> {
    fn from(document: &'document Document<'input>) -> Self {
        Self::Borrowed(document.root())
    }
}

impl<'document, 'input> From<&'document OwnedDocument> for Tree<'document, 'input> {
    fn from(document: &'document OwnedDocument) -> Self {
        Self::Owned(document.root())
    }
}

/// The walk `save` and `Writer` share: `Tree` covers the borrowed DOM and
/// global-heap owned documents; `OwnedNode` covers any allocator.
pub(crate) trait Walk<'document>: Copy {
    fn first_child(self) -> Option<Self>;
    fn next_sibling(self) -> Option<Self>;
    fn parent(self) -> Option<Self>;
    fn name(self) -> Option<Name<'document>>;
    /// A leaf node's event; documents and elements have none.
    fn leaf(self) -> Option<Event<'document>>;
    /// Replaces `attributes` with this element's, failing above `limit`.
    fn attributes(
        self,
        attributes: &mut Vec<Attribute<'document>>,
        limit: usize,
        offset: usize,
    ) -> Result<(), Error>;
    fn is_document(self) -> bool;
}

impl<'document> Walk<'document> for Tree<'document, '_> {
    fn first_child(self) -> Option<Self> {
        match self {
            Self::Borrowed(node) => node.first_child().map(Self::Borrowed),
            Self::Owned(node) => node.first_child().map(Self::Owned),
        }
    }

    fn next_sibling(self) -> Option<Self> {
        match self {
            Self::Borrowed(node) => node.next_sibling().map(Self::Borrowed),
            Self::Owned(node) => node.next_sibling().map(Self::Owned),
        }
    }

    fn parent(self) -> Option<Self> {
        match self {
            Self::Borrowed(node) => node.parent().map(Self::Borrowed),
            Self::Owned(node) => node.parent().map(Self::Owned),
        }
    }

    fn name(self) -> Option<Name<'document>> {
        match self {
            Self::Borrowed(node) => node.name().map(Name::as_borrowed),
            Self::Owned(node) => node.name(),
        }
    }

    /// A leaf node's event; documents and elements have none.
    fn leaf(self) -> Option<Event<'document>> {
        match self {
            Self::Borrowed(node) => match node.kind() {
                NodeKind::Doctype(doctype) => Some(Event::Doctype(*doctype)),
                NodeKind::Text(value) => Some(Event::Text(Cow::Borrowed(value))),
                NodeKind::Cdata(value) => Some(Event::Cdata(Cow::Borrowed(value))),
                NodeKind::Comment(value) => Some(Event::Comment(Cow::Borrowed(value))),
                NodeKind::ProcessingInstruction { target, data } => {
                    Some(Event::ProcessingInstruction {
                        target: Cow::Borrowed(target),
                        data: Cow::Borrowed(data),
                    })
                }
                NodeKind::EntityRef(name) => Some(Event::EntityStart(Cow::Borrowed(name))),
                NodeKind::Document | NodeKind::Element(_) => None,
            },
            Self::Owned(node) => owned_leaf(node),
        }
    }

    /// Replaces `attributes` with this element's, failing above `limit`.
    fn attributes(
        self,
        attributes: &mut Vec<Attribute<'document>>,
        limit: usize,
        offset: usize,
    ) -> Result<(), Error> {
        match self {
            Self::Borrowed(node) => {
                let items = node.attributes().iter().map(|attribute| Attribute {
                    name: attribute.name.as_borrowed(),
                    value: Cow::Borrowed(&attribute.value),
                });
                collect_attributes(attributes, items, limit, offset)
            }
            Self::Owned(node) => collect_attributes(attributes, node.attributes(), limit, offset),
        }
    }

    fn is_document(self) -> bool {
        match self {
            Self::Borrowed(node) => matches!(node.kind(), NodeKind::Document),
            Self::Owned(node) => node.kind() == OwnedNodeKind::Document,
        }
    }
}

impl<'document, A: Allocator> Walk<'document> for OwnedNode<'document, A> {
    fn first_child(self) -> Option<Self> {
        OwnedNode::first_child(self)
    }

    fn next_sibling(self) -> Option<Self> {
        OwnedNode::next_sibling(self)
    }

    fn parent(self) -> Option<Self> {
        OwnedNode::parent(self)
    }

    fn name(self) -> Option<Name<'document>> {
        OwnedNode::name(self)
    }

    fn leaf(self) -> Option<Event<'document>> {
        owned_leaf(self)
    }

    fn attributes(
        self,
        attributes: &mut Vec<Attribute<'document>>,
        limit: usize,
        offset: usize,
    ) -> Result<(), Error> {
        collect_attributes(attributes, OwnedNode::attributes(self), limit, offset)
    }

    fn is_document(self) -> bool {
        self.kind() == OwnedNodeKind::Document
    }
}

impl<'document, 'input> Tree<'document, 'input> {
    /// The attribute at `position` with its entity references kept, if any.
    fn kept_form(self, position: usize) -> Option<&'document str> {
        match self {
            Self::Borrowed(node) => node.unexpanded_attribute(position),
            Self::Owned(node) => node.unexpanded_attribute(position),
        }
    }

    fn same(self, other: Self) -> bool {
        match (self, other) {
            (Self::Borrowed(a), Self::Borrowed(b)) => a.id() == b.id(),
            (Self::Owned(a), Self::Owned(b)) => a.id() == b.id(),
            _ => false,
        }
    }

    fn children(self) -> impl Iterator<Item = Self> {
        std::iter::successors(self.first_child(), |node| node.next_sibling())
    }

    #[cfg(feature = "html")]
    fn owned_document(self) -> Option<&'document OwnedDocument> {
        match self {
            Self::Borrowed(_) => None,
            Self::Owned(node) => Some(node.document()),
        }
    }

    /// The declaration; HTML documents are standalone with their encoding,
    /// as `htmlNewDocNoDtD` leaves them.
    fn declaration(self) -> Option<Declaration<'document>> {
        match self {
            Self::Borrowed(node) => node.document().declaration().copied(),
            Self::Owned(node) if node.document().is_html() => Some(Declaration {
                version: "1.0",
                encoding: node.document().html_encoding(),
                standalone: Some(true),
            }),
            Self::Owned(node) => node.document().declaration(),
        }
    }

    fn doctype(self) -> Option<Doctype<'document>> {
        match self {
            Self::Borrowed(node) => node.document().doctype().copied(),
            Self::Owned(node) => node.document().doctype(),
        }
    }

    /// `doc->encoding`.
    fn encoding(self) -> Option<&'document str> {
        self.declaration()?.encoding
    }
}

/// Replaces `attributes` with `items`, failing above `limit`.
pub(crate) fn collect_attributes<'a>(
    attributes: &mut Vec<Attribute<'a>>,
    items: impl ExactSizeIterator<Item = Attribute<'a>>,
    limit: usize,
    offset: usize,
) -> Result<(), Error> {
    attributes.clear();
    if items.len() > limit {
        return Err(Error::new(
            ErrorKind::LimitExceeded(Limit::Attributes),
            offset,
        ));
    }
    attributes
        .try_reserve_exact(items.len())
        .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
    attributes.extend(items);
    Ok(())
}

/// A leaf node's event; documents and elements have none.
pub(crate) fn owned_leaf<A: Allocator>(node: OwnedNode<'_, A>) -> Option<Event<'_>> {
    // Leaf records always hold their text, target and entity name.
    let text = Cow::Borrowed(node.text().unwrap_or_default());
    Some(match node.kind() {
        OwnedNodeKind::DocumentType => return node.doctype().map(Event::Doctype),
        OwnedNodeKind::Text => Event::Text(text),
        OwnedNodeKind::Cdata => Event::Cdata(text),
        OwnedNodeKind::Comment => Event::Comment(text),
        OwnedNodeKind::ProcessingInstruction => Event::ProcessingInstruction {
            target: Cow::Borrowed(node.processing_instruction_target().unwrap_or_default()),
            data: text,
        },
        OwnedNodeKind::EntityRef => {
            Event::EntityStart(Cow::Borrowed(node.entity_name().unwrap_or_default()))
        }
        OwnedNodeKind::Document | OwnedNodeKind::Element => return None,
    })
}

/// Serializes `tree` to memory, as [`to_writer`] does.
pub fn to_vec<'document, 'input: 'document>(
    tree: impl Into<Tree<'document, 'input>>,
    options: &Options<'_>,
) -> Result<Vec<u8>, Error> {
    let mut bytes = Memory::default();
    match to_writer(tree, options, &mut bytes) {
        Ok(()) => Ok(bytes.0),
        Err(WriteError::Xml(error)) => Err(error),
        // Memory fails only to grow.
        Err(WriteError::Io(_)) => Err(Error::new(ErrorKind::Allocation, bytes.0.len())),
    }
}

/// Serializes `tree` to `sink` in [`CHUNK`]-byte writes, then flushes it
/// (`xmlSaveToIO`).
///
/// A document node is saved with `xmlSaveDoc`; any other node with
/// `xmlSaveTree` at [`Options::level`], with the namespace declarations it
/// uses added as a copy of it would have them: on its root, but a default
/// namespace that would capture an unprefixed element in another one on
/// each element needing it. Text and attribute characters the encoding lacks
/// become `&#N;`. HTML documents use the HTML serializer unless [`AS_XML`]
/// or [`XHTML`] is set.
///
/// # Errors
///
/// - `UnsupportedOption`: unknown flags, an invalid indent, or [`AS_HTML`]
///   on a borrowed node or without the `html` feature.
/// - `UnsupportedEncoding`: an unknown encoding.
/// - `Allocation`: the output buffer cannot grow.
/// - `InvalidCharacter`: a character the encoding lacks in a name, comment,
///   PI, CDATA section or DOCTYPE identifier (where libxml2 would change
///   the content); nothing after it is written.
/// - [`WriteError::Io`]: the sink failed, possibly after partial output.
pub fn to_writer<'document, 'input: 'document>(
    tree: impl Into<Tree<'document, 'input>>,
    options: &Options<'_>,
    mut sink: impl Write,
) -> Result<(), WriteError> {
    let tree = tree.into();
    let unsupported = WriteError::Xml(Error::new(ErrorKind::UnsupportedOption, 0));
    let indent = &options.indent;
    if options.flags & !SUPPORTED != 0
        || !(1..=MAX_INDENT).contains(&indent.len())
        || indent.bytes().any(|byte| byte != b' ' && byte != b'\t')
    {
        return Err(unsupported);
    }
    let document = tree.is_document();
    #[cfg(feature = "html")]
    let html = tree.owned_document().is_some_and(OwnedDocument::is_html);
    #[cfg(feature = "html")]
    if options.has(AS_HTML) || html && !options.has(AS_XML) && !options.has(XHTML) {
        let Tree::Owned(node) = tree else {
            return Err(unsupported);
        };
        // `htmlNodeDumpOutputInternal` falls back to the document's encoding.
        let encoding = options.encoding.or(tree.encoding()).unwrap_or("HTML");
        return crate::html::save_node(node, options.has(FORMAT), encoding, &mut sink);
    }
    let encoding = options
        .encoding
        .or_else(|| document.then(|| tree.encoding()).flatten());
    let output = match encoding {
        Some(label) => Output::for_label(label).ok_or(WriteError::Xml(Error::new(
            ErrorKind::UnsupportedEncoding,
            0,
        )))?,
        None => Output::Utf8,
    };
    let mut saver = Saver {
        out: Out::new(output, encoding.is_none(), Some(&mut sink)),
        options,
        format: if options.has(FORMAT) {
            1
        } else if options.has(WSNONSIG) {
            2
        } else {
            0
        },
        level: options.level.min(MAX_LEVEL),
        xhtml_attributes: options.has(XHTML),
        encoding,
    };
    if document {
        saver.document(tree);
    } else {
        saver.node(tree, options.has(XHTML), &missing_namespaces(tree));
    }
    saver.out.finish()
}

/// How [`encode_entities`] escapes text, after the document it is meant
/// for (`xmlEncodeEntitiesReentrant`'s `doc`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Escape {
    /// An XML document with a declared encoding: non-ASCII is written as is.
    Xml,
    /// No document, or one without a declared encoding: non-ASCII becomes
    /// `&#xH;` (U+FFFE and U+FFFF become `&#xFFFD;`).
    Ascii,
    /// An HTML document: only `&`, `<` and `>` are escaped.
    Html,
}

/// Every notation of `dtd` as a `<!NOTATION ... >` line, in declaration
/// order (`xmlDumpNotationTable`, which writes hash order).
///
/// # Errors
///
/// `Allocation` when the output cannot grow.
pub fn notation_table(dtd: &crate::valid::Dtd) -> Result<String, Error> {
    let mut bytes = Memory::default();
    let mut out = Out::new(Output::Utf8, false, Some(&mut bytes));
    for notation in dtd.notations() {
        dtd::notation(
            &mut out,
            notation.name,
            notation.public_id,
            notation.system_id,
        );
    }
    match out.finish() {
        Ok(()) => Ok(String::from_utf8(bytes.0).expect("UTF-8 output")),
        Err(_) => Err(Error::new(ErrorKind::Allocation, bytes.0.len())),
    }
}

/// Escapes text for content (`xmlEncodeEntitiesReentrant`): `&`, `<` and
/// `>` become references; for XML also `\r` becomes `&#13;` and the other C0
/// controls but tab and newline, NUL included, `&#xFFFD;`.
pub fn encode_entities(text: &str, escape: Escape) -> String {
    let mut out = String::new();
    escape_text(
        &mut out,
        text,
        escape == Escape::Html,
        escape == Escape::Ascii,
    );
    out
}

/// Escapes text for content or attribute values (`xmlEncodeSpecialChars`):
/// `&`, `<`, `>` and `"` become references, `\r` `&#13;` and the other C0
/// controls but tab and newline `&#xFFFD;`; non-ASCII is kept.
pub fn encode_special_chars(text: &str) -> String {
    let mut out = String::new();
    escape_into(text, &QUOT, false, |piece| out.push_str(piece));
    out
}

/// `htmlIsBooleanAttr` (HTML 4 minimized attributes), ASCII case-insensitive.
pub(crate) fn boolean_attribute(name: &str) -> bool {
    const NAMES: [&str; 13] = [
        "checked", "compact", "declare", "defer", "disabled", "ismap", "multiple", "nohref",
        "noresize", "noshade", "nowrap", "readonly", "selected",
    ];
    NAMES.iter().any(|known| known.eq_ignore_ascii_case(name))
}

/// `xmlEscapeText` of content into `out`: HTML (`html`), or XML for a
/// document without an encoding (`ascii`) or with one.
pub(crate) fn escape_text(out: &mut String, text: &str, html: bool, ascii: bool) {
    let table = if html { &HTML } else { &CONTENT };
    escape_into(text, table, ascii && !html, |piece| out.push_str(piece));
}

/// `xmlEscapeText` into UTF-8 pieces for `push`: ASCII through `table`,
/// non-ASCII as `&#xH;` when `hex` (U+FFFE and U+FFFF as U+FFFD), otherwise
/// as is.
pub(crate) fn escape_into(text: &str, table: &Table, hex: bool, mut push: impl FnMut(&str)) {
    let bytes = text.as_bytes();
    let marked = |b: u8| table.get(usize::from(b)).map_or(hex, Option::is_some);
    // Every table escapes only C0 controls and `&<>"`.
    let screen = |b: u8| {
        (b < 0x20) | (b == b'&') | (b == b'<') | (b == b'>') | (b == b'"') | (hex & (b >= 0x80))
    };
    let mut start = 0;
    while let Some(index) = crate::escape::find_byte(bytes, start, marked, screen) {
        push(&text[start..index]);
        match table.get(usize::from(bytes[index])) {
            Some(escape) => {
                push(escape.unwrap_or_default());
                start = index + 1;
            }
            None => {
                start = index;
                for c in text[index..].chars().take_while(|c| !c.is_ascii()) {
                    push(char_reference(unicode_char(c), true, &mut [0; 12]));
                    start += c.len_utf8();
                }
            }
        }
    }
    push(&text[start..]);
}

/// `xmlEscapeText` writes the noncharacters U+FFFE and U+FFFF as U+FFFD.
fn unicode_char(c: char) -> char {
    if matches!(c, '\u{fffe}' | '\u{ffff}') {
        '\u{fffd}'
    } else {
        c
    }
}

/// Bytes [`to_writer`] buffers before handing them to the sink.
pub const CHUNK: usize = 32 * 1024;

/// Encoded output, handed to `sink` in chunks of [`CHUNK`] bytes (a sibling
/// without a sink keeps everything). The first character the encoding cannot
/// write in markup is recorded, as libxml2's `buf->error`, and nothing after
/// it reaches the sink.
struct Out<'s> {
    bytes: Vec<u8>,
    sink: Option<&'s mut dyn Write>,
    /// Bytes before `bytes`, already handed to the sink.
    written: usize,
    output: Output,
    /// No encoding: text escapes non-ASCII as hexadecimal references.
    ascii: bool,
    error: Option<WriteError>,
}

/// Escapes for ASCII bytes; `None` is written as is.
type Table = [Option<&'static str>; 128];

/// `xmlEscapeTabAttr`: other C0 controls become U+FFFD.
pub(crate) const ATTRIBUTE: Table = {
    let mut table = [None; 128];
    let mut byte = 0;
    while byte < 0x20 {
        table[byte] = Some("&#xFFFD;");
        byte += 1;
    }
    table[b'\t' as usize] = Some("&#9;");
    table[b'\n' as usize] = Some("&#10;");
    table[b'\r' as usize] = Some("&#13;");
    table[b'&' as usize] = Some("&amp;");
    table[b'<' as usize] = Some("&lt;");
    table[b'>' as usize] = Some("&gt;");
    table[b'"' as usize] = Some("&quot;");
    table
};

/// `xmlEscapeTab`: tabs, newlines and quotes are written as they are.
const CONTENT: Table = {
    let mut table = QUOT;
    table[b'"' as usize] = None;
    table
};

/// `xmlEscapeTabQuot` (`xmlEncodeSpecialChars`): content escaping and `"`.
pub(crate) const QUOT: Table = {
    let mut table = ATTRIBUTE;
    (table[9], table[10]) = (None, None);
    table
};

/// `htmlEscapeTab`: only `&`, `<` and `>`.
const HTML: Table = {
    let mut table = [None; 128];
    table[b'&' as usize] = CONTENT[b'&' as usize];
    table[b'<' as usize] = CONTENT[b'<' as usize];
    table[b'>' as usize] = CONTENT[b'>' as usize];
    table
};

/// Whether [`Out::text`] escapes `b`: exactly the bytes `ATTRIBUTE`
/// (`attribute`) or `CONTENT` escapes, and non-ASCII unless kept (`unicode`).
#[inline(always)]
const fn special(b: u8, attribute: bool, unicode: bool) -> bool {
    (b < 0x20) & (attribute | (b != b'\t') & (b != b'\n'))
        | (b == b'&')
        | (b == b'<')
        | (b == b'>')
        | (attribute & (b == b'"'))
        | (!unicode & (b >= 0x80))
}

/// [`special`] as tables, by `attribute` and `unicode`.
const SPECIAL: [[[bool; 256]; 2]; 2] = {
    let mut tables = [[[false; 256]; 2]; 2];
    let mut b = 0;
    while b < 256 {
        let mut mode = 0;
        while mode < 4 {
            tables[mode / 2][mode % 2][b] = special(b as u8, mode / 2 == 1, mode % 2 == 1);
            mode += 1;
        }
        b += 1;
    }
    tables
};

impl<'s> Out<'s> {
    fn new(output: Output, ascii: bool, sink: Option<&'s mut dyn Write>) -> Self {
        Self {
            bytes: output.bom().to_vec(),
            sink,
            written: 0,
            output,
            ascii,
            error: None,
        }
    }

    /// An empty in-memory buffer with the same encoding, for [`Out::append`].
    fn sibling(&self) -> Out<'static> {
        Out {
            bytes: Vec::new(),
            ..Out::new(self.output, self.ascii, None)
        }
    }

    fn append(&mut self, other: Out<'_>) {
        if let Some(WriteError::Xml(error)) = other.error {
            self.fail(Error::new(error.kind, self.position() + error.offset));
        }
        if self.room(other.bytes.len()) {
            self.bytes.extend_from_slice(&other.bytes);
        }
    }

    /// Room for `length` more bytes of UTF-8 in the output encoding (UTF-16
    /// at most doubles it); false, and an allocation error, when refused.
    #[inline]
    fn room(&mut self, length: usize) -> bool {
        let width = 1 + usize::from(matches!(self.output, Output::Utf16 { .. }));
        let length = length.saturating_mul(width);
        self.bytes.capacity() - self.bytes.len() >= length || self.grow(length)
    }

    #[cold]
    #[inline(never)]
    fn grow(&mut self, length: usize) -> bool {
        let grown = self.bytes.try_reserve(length).is_ok();
        if !grown {
            self.fail(Error::new(ErrorKind::Allocation, self.position()));
        }
        grown
    }

    /// The output offset of the next byte.
    fn position(&self) -> usize {
        self.written + self.bytes.len()
    }

    fn fail(&mut self, error: Error) {
        if self.error.is_none() {
            self.error = Some(WriteError::Xml(error));
        }
    }

    /// Spills once the buffer holds [`CHUNK`] bytes: called between nodes,
    /// so a chunk can exceed it by one node's markup or text.
    fn checkpoint(&mut self) {
        if self.bytes.len() >= CHUNK {
            self.spill();
        }
    }

    /// Hands the buffer to the sink; after a failure it is discarded.
    #[inline(never)]
    fn spill(&mut self) {
        if let Some(sink) = self.sink.as_mut() {
            if self.error.is_none() {
                let mut confirmed = 0;
                if let Err(error) = crate::writer::write_all(sink, &self.bytes, &mut confirmed) {
                    self.error = Some(error);
                }
            }
            self.written += self.bytes.len();
            self.bytes.clear();
        }
    }

    fn finish(mut self) -> Result<(), WriteError> {
        self.spill();
        if let (None, Some(sink)) = (&self.error, self.sink.as_mut()) {
            crate::writer::flush(sink)?;
        }
        self.error.map_or(Ok(()), Err)
    }

    /// Markup, names, comments, PIs and CDATA, which cannot hold references.
    fn raw(&mut self, text: &str) {
        if self.output == Output::Utf8 {
            if self.bytes.capacity() - self.bytes.len() >= text.len() || self.grow(text.len()) {
                self.bytes.extend_from_slice(text.as_bytes());
            }
        } else if self.room(text.len()) {
            for c in text.chars() {
                if !self.output.push(&mut self.bytes, c) {
                    self.fail(Error::new(ErrorKind::InvalidCharacter, self.position()));
                }
            }
        }
    }

    /// An entity value the encoder converts, writing `&#N;` for what it
    /// lacks. A reference (`&` or `%` through `;`) is markup: libxml2's `&#N;`
    /// inside its name would not reparse, so that is `InvalidCharacter`.
    fn converted(&mut self, text: &str) {
        if !self.room(text.len()) {
            return;
        }
        let mut reference = false;
        for c in text.chars() {
            reference |= matches!(c, '&' | '%');
            if reference {
                self.raw(c.encode_utf8(&mut [0; 4]));
            } else if !self.output.push(&mut self.bytes, c) {
                self.reference(c, false);
            }
            reference &= c != ';';
        }
    }

    /// `xmlSerializeText` followed by the encoder: escaped text and
    /// attribute values, with references for what the encoding lacks.
    fn text(&mut self, text: &str, attribute: bool) {
        let table = if attribute { &ATTRIBUTE } else { &CONTENT };
        let unicode = self.output == Output::Utf8 && !self.ascii;
        if !unicode && !self.room(text.len()) {
            return;
        }
        let marks = &SPECIAL[usize::from(attribute)][usize::from(unicode)];
        let marked = |b: u8| marks[usize::from(b)];
        let bytes = text.as_bytes();
        let mut start = 0;
        while let Some(index) =
            crate::escape::find_byte(bytes, start, marked, |b| special(b, attribute, unicode))
        {
            self.raw(&text[start..index]);
            match table.get(usize::from(bytes[index])) {
                Some(escape) => {
                    self.raw(escape.unwrap_or_default());
                    start = index + 1;
                }
                None => start = index + self.encoded(&text[index..]),
            }
        }
        self.raw(&text[start..]);
    }

    /// Writes the non-ASCII characters `text` starts with in the output
    /// encoding, as references where it lacks them; returns their length.
    #[inline(never)]
    fn encoded(&mut self, text: &str) -> usize {
        let mut length = 0;
        for c in text.chars().take_while(|c| !c.is_ascii()) {
            if self.ascii {
                self.reference(unicode_char(c), true);
            } else if !self.output.push(&mut self.bytes, c) {
                self.reference(c, false);
            }
            length += c.len_utf8();
        }
        length
    }

    #[inline(always)]
    fn reference(&mut self, c: char, hex: bool) {
        self.raw(char_reference(c, hex, &mut [0; 12]));
    }

    /// An attribute value with kept references, as `xmlSaveWriteAttrContent`
    /// writes the text and entity-reference children: `&name;` is markup,
    /// `&#38;` an escaped `&`, the rest escaped text.
    fn kept(&mut self, form: &str) {
        let mut parts = form.split('&');
        self.text(parts.next().unwrap_or_default(), true);
        for part in parts {
            let (reference, text) = part.split_at(part.find(';').map_or(0, |end| end + 1));
            if reference == "#38;" {
                self.text("&", true);
            } else {
                self.raw("&");
                self.raw(reference);
            }
            self.text(text, true);
        }
    }

    /// `xmlOutputBufferWriteQuotedString`; `converted` allows references.
    fn quoted(&mut self, text: &str, converted: bool) {
        let write = |out: &mut Self, text: &str| {
            if converted {
                out.converted(text)
            } else {
                out.raw(text)
            }
        };
        let quote = if !text.contains('"') {
            "\""
        } else if !text.contains('\'') {
            "'"
        } else {
            self.raw("\"");
            for (index, part) in text.split('"').enumerate() {
                if index > 0 {
                    self.raw("&quot;");
                }
                write(self, part);
            }
            return self.raw("\"");
        };
        self.raw(quote);
        write(self, text);
        self.raw(quote);
    }
}

/// `&#xH;` as `xmlSerializeHexCharRef` writes it, or the encoders' `&#N;`.
#[inline(always)]
pub(crate) fn char_reference(c: char, hex: bool, buffer: &mut [u8; 12]) -> &str {
    let (base, prefix) = if hex { (16, "&#x") } else { (10, "&#") };
    let (mut value, mut start) = (u32::from(c), buffer.len() - 1);
    buffer[start] = b';';
    loop {
        start -= 1;
        buffer[start] = b"0123456789ABCDEF"[(value % base) as usize];
        value /= base;
        if value == 0 {
            break;
        }
    }
    start -= prefix.len();
    buffer[start..start + prefix.len()].copy_from_slice(prefix.as_bytes());
    std::str::from_utf8(&buffer[start..]).unwrap_or_default()
}

/// An attribute value and its kept-reference form, if any.
type Value<'v> = (&'v str, Option<&'v str>);

struct Saver<'a, 's> {
    out: Out<'s>,
    options: &'a Options<'a>,
    /// libxml2's `ctxt->format`: 0, 1 for [`FORMAT`], 2 for [`WSNONSIG`].
    format: u8,
    level: usize,
    /// `XML_SAVE_XHTML` in the context: empty boolean attributes are expanded.
    xhtml_attributes: bool,
    encoding: Option<&'a str>,
}

impl<'a> Saver<'a, '_> {
    /// `xmlSaveWriteIndent`.
    fn indent(&mut self, extra: usize) {
        if !self.options.has(NO_INDENT) {
            let levels = (self.level + extra).min(MAX_INDENT / self.options.indent.len());
            for _ in 0..levels {
                self.out.raw(self.options.indent);
            }
        }
    }

    /// `xmlOutputBufferWriteWSNonSig`.
    fn whitespace(&mut self, extra: usize) {
        self.out.raw("\n");
        for _ in 0..self.level + extra {
            self.out.raw(self.options.indent);
        }
    }

    /// `xmlSaveDocInternal` for XML output.
    fn document(&mut self, root: Tree<'_, '_>) {
        if !self.options.has(NO_DECL) {
            let declaration = root.declaration();
            self.out.raw("<?xml version=\"");
            self.out
                .raw(declaration.map_or("1.0", |declaration| declaration.version));
            self.out.raw("\"");
            if let Some(encoding) = self.encoding {
                self.out.raw(" encoding=\"");
                self.out.raw(encoding);
                self.out.raw("\"");
            }
            match declaration.and_then(|declaration| declaration.standalone) {
                Some(true) => self.out.raw(" standalone=\"yes\""),
                Some(false) => self.out.raw(" standalone=\"no\""),
                None => {}
            }
            self.out.raw("?>\n");
        }
        let mut xhtml = self.options.has(XHTML);
        if !self.options.has(NO_XHTML)
            && let Some(doctype) = root.doctype()
        {
            xhtml = doctype.is_xhtml();
        }
        self.xhtml_attributes |= xhtml;
        for child in root.children() {
            self.level = 0;
            self.node(child, xhtml, &[]);
            self.out.raw("\n");
        }
    }

    /// `xmlNodeDumpOutputInternal`, or `xhtmlNodeDumpOutput` for `xhtml`,
    /// declaring `added` ([`missing_namespaces`]) on the elements their
    /// ordinals name.
    fn node(
        &mut self,
        root: Tree<'_, '_>,
        xhtml: bool,
        mut added: &[(usize, Option<String>, String)],
    ) {
        let format = self.format;
        let mut unformatted = None;
        let (mut depth, mut ordinal) = (0, 0);
        let mut attributes = Vec::new();
        let mut current = root;
        loop {
            self.out.checkpoint();
            let is_root = current.same(root);
            if let Some(name) = current.name() {
                let own = added.iter().take_while(|(at, ..)| *at == ordinal).count();
                let (own, rest) = added.split_at(own);
                (added, ordinal) = (rest, ordinal + 1);
                if self.element(current, &name, is_root, xhtml, own, &mut attributes) {
                    // XHTML writes its meta before checking for text children.
                    let mut unformat = |saver: &mut Self| {
                        if saver.format == 1 && has_text(current, xhtml) {
                            saver.format = 0;
                            unformatted = Some(depth);
                        }
                    };
                    if !xhtml {
                        unformat(self);
                        if self.format == 2 {
                            self.whitespace(1);
                        }
                    }
                    self.out.raw(">");
                    self.meta(current, xhtml);
                    if xhtml {
                        unformat(self);
                    }
                    if self.format == 1 {
                        self.out.raw("\n");
                    }
                    self.level += 1;
                    depth += 1;
                    current = current.first_child().unwrap_or(current);
                    continue;
                }
            } else if let Some(event) = current.leaf() {
                self.leaf(&event, is_root, xhtml);
            }
            loop {
                // Below `root`, every node has a parent.
                let Some(parent) = current.parent().filter(|_| !current.same(root)) else {
                    return;
                };
                if self.format == 1 {
                    self.out.raw("\n");
                }
                if let Some(next) = current.next_sibling() {
                    current = next;
                    break;
                }
                current = parent;
                depth -= 1;
                self.level = self.level.saturating_sub(1);
                if self.format == 1 {
                    self.indent(0);
                }
                self.out.raw("</");
                self.out
                    .raw(current.name().as_ref().map_or("", Name::qualified));
                if self.format == 2 && !xhtml {
                    self.whitespace(0);
                }
                self.out.raw(">");
                if unformatted == Some(depth) {
                    self.format = format;
                    unformatted = None;
                }
            }
        }
    }

    /// Writes a start tag, or a complete childless element; true when the
    /// start tag awaits its `>` and children.
    fn element<'d>(
        &mut self,
        node: Tree<'d, '_>,
        name: &Name<'_>,
        is_root: bool,
        xhtml: bool,
        added: &[(usize, Option<String>, String)],
        attributes: &mut Vec<Attribute<'d>>,
    ) -> bool {
        if !is_root && self.format == 1 {
            self.indent(0);
        }
        let qualified = name.qualified();
        self.out.raw("<");
        self.out.raw(qualified);
        if let Err(error) = node.attributes(attributes, usize::MAX, self.out.position()) {
            self.out.fail(error);
        }
        let mut declared = false;
        for attribute in attributes.iter().filter(|a| is_declaration(a)) {
            declared = true;
            if attribute.name.local() != "xml" || attribute.name.prefix().is_none() {
                self.attribute(attribute.name.qualified(), (&attribute.value, None), false);
            }
        }
        for (_, prefix, uri) in added {
            declared = true;
            match prefix {
                Some(prefix) => self.attribute(&format!("xmlns:{prefix}"), (uri, None), false),
                None => self.attribute("xmlns", (uri, None), false),
            }
        }
        if xhtml && qualified == "html" && name.namespace().is_none() && !declared {
            self.out.raw(" xmlns=\"http://www.w3.org/1999/xhtml\"");
        }
        let ordinary = attributes
            .iter()
            .enumerate()
            .filter(|(_, a)| !is_declaration(a));
        self.attributes(
            name,
            ordinary.map(|(position, a)| (a, node.kept_form(position))),
            xhtml,
        );
        if node.first_child().is_some() {
            return true;
        }
        let add_meta = xhtml && needs_meta(node);
        if xhtml && name.prefix().is_none() && xhtml_empty(name) && !add_meta {
            self.out.raw(" />");
        } else if xhtml || self.options.has(NO_EMPTY) {
            if self.format == 2 && !xhtml {
                self.whitespace(1);
            }
            self.out.raw(">");
            self.meta(node, xhtml);
            if add_meta && self.format == 1 {
                self.out.raw("\n");
            }
            self.out.raw("</");
            self.out.raw(qualified);
            if self.format == 2 && !xhtml {
                self.whitespace(0);
            }
            self.out.raw(">");
        } else {
            if self.format == 2 {
                self.whitespace(0);
            }
            self.out.raw("/>");
        }
        false
    }

    /// `xmlAttrDumpOutput` and `xmlNsDumpOutput`; `plain` attributes have
    /// no namespace and may be XHTML booleans. A value is written from its
    /// kept-reference form when it has one.
    fn attribute(&mut self, name: &str, value: Value<'_>, plain: bool) {
        if self.format == 2 {
            self.whitespace(2);
        } else {
            self.out.raw(" ");
        }
        self.out.raw(name);
        self.out.raw("=\"");
        if self.xhtml_attributes && plain && value.0.is_empty() && boolean_attribute(name) {
            self.out.raw(name);
        } else {
            self.value(value);
        }
        self.out.raw("\"");
    }

    fn value(&mut self, (value, kept): Value<'_>) {
        match kept {
            Some(form) => self.out.kept(form),
            None => self.out.text(value, true),
        }
    }

    /// The attributes; under XHTML (`xhtmlAttrListDumpOutput`) C.8 adds `id`
    /// from `name` and C.7 pairs `lang` and `xml:lang`.
    fn attributes<'v>(
        &mut self,
        element: &Name<'_>,
        attributes: impl Iterator<Item = (&'v Attribute<'v>, Option<&'v str>)>,
        xhtml: bool,
    ) {
        let (mut id, mut name, mut lang, mut xml_lang) = (false, None, None, None);
        for (attribute, kept) in attributes {
            let namespaced = attribute.name.namespace().is_some();
            let value = (attribute.value.as_ref(), kept);
            match (namespaced, attribute.name.qualified()) {
                (false, "id") => id = true,
                (false, "name") => name = Some(value),
                (false, "lang") => lang = Some(value),
                (true, "xml:lang") => xml_lang = Some(value),
                _ => {}
            }
            self.attribute(attribute.name.qualified(), value, !namespaced);
        }
        if !xhtml {
            return;
        }
        const NAMED: [&str; 9] = [
            "a", "p", "div", "img", "map", "applet", "form", "frame", "iframe",
        ];
        if let Some(value) = name.filter(|_| !id && NAMED.contains(&element.local())) {
            self.extra(" id=\"", value);
        }
        match (lang, xml_lang) {
            (Some(value), None) => self.extra(" xml:lang=\"", value),
            (None, Some(value)) => self.extra(" lang=\"", value),
            _ => {}
        }
    }

    fn extra(&mut self, start: &str, value: Value<'_>) {
        self.out.raw(start);
        self.value(value);
        self.out.raw("\"");
    }

    /// The `<meta>` XHTML adds to a top-level head that lacks a Content-Type.
    fn meta(&mut self, node: Tree<'_, '_>, xhtml: bool) {
        if !(xhtml && needs_meta(node)) {
            return;
        }
        if self.format == 1 {
            self.out.raw("\n");
            self.indent(1);
        }
        self.out
            .raw("<meta http-equiv=\"Content-Type\" content=\"text/html; charset=");
        self.out.raw(self.encoding.unwrap_or("UTF-8"));
        self.out.raw("\" />");
    }

    fn leaf(&mut self, event: &Event<'_>, is_root: bool, xhtml: bool) {
        let indent = !xhtml && !is_root && self.format == 1;
        match event {
            Event::Text(text) => self.out.text(text, false),
            Event::Cdata(text) if text.is_empty() => self.out.raw("<![CDATA[]]>"),
            Event::Cdata(text) => {
                // Each "]]>" splits the section after its "]]".
                let mut rest = &text[..];
                while let Some(end) = rest.find("]]>") {
                    self.out.raw("<![CDATA[");
                    self.out.raw(&rest[..end + 2]);
                    self.out.raw("]]>");
                    rest = &rest[end + 2..];
                }
                if !rest.is_empty() {
                    self.out.raw("<![CDATA[");
                    self.out.raw(rest);
                    self.out.raw("]]>");
                }
            }
            Event::Comment(text) => {
                if indent {
                    self.indent(0);
                }
                self.out.raw("<!--");
                self.out.raw(text);
                self.out.raw("-->");
            }
            Event::ProcessingInstruction { target, data } => {
                if indent {
                    self.indent(0);
                }
                self.out.raw("<?");
                self.out.raw(target);
                if self.format == 2 && !xhtml {
                    self.whitespace(0);
                } else if !data.is_empty() {
                    self.out.raw(" ");
                }
                self.out.raw(data);
                self.out.raw("?>");
            }
            Event::Doctype(doctype) => dtd::doctype(&mut self.out, doctype),
            Event::EntityStart(name) => {
                self.out.raw("&");
                self.out.raw(name);
                self.out.raw(";");
            }
            _ => {}
        }
    }
}

fn is_declaration(attribute: &Attribute<'_>) -> bool {
    attribute.name.namespace() == Some(XMLNS)
}

/// Text, entity reference (or, outside XHTML, CDATA) children turn formatting off.
fn has_text(node: Tree<'_, '_>, xhtml: bool) -> bool {
    node.children().any(|child| match child.leaf() {
        Some(Event::Text(_) | Event::EntityStart(_)) => true,
        Some(Event::Cdata(_)) => !xhtml,
        _ => false,
    })
}

/// `xhtmlIsEmpty` for a childless element: its XHTML content model is EMPTY.
fn xhtml_empty(name: &Name<'_>) -> bool {
    const EMPTY: [&str; 13] = [
        "area", "br", "base", "basefont", "col", "frame", "hr", "img", "input", "isindex", "link",
        "meta", "param",
    ];
    name.namespace().is_none_or(|uri| uri == XHTML_NAMESPACE) && EMPTY.contains(&name.local())
}

/// An element with this local name.
fn named(node: Tree<'_, '_>, local: &str) -> bool {
    node.name().is_some_and(|name| name.local() == local)
}

/// A `head` child of a top-level `html` without a Content-Type `meta`.
fn needs_meta(node: Tree<'_, '_>) -> bool {
    let content_type = |child: Tree<'_, '_>| {
        let mut attributes = Vec::new();
        named(child, "meta")
            && child.attributes(&mut attributes, usize::MAX, 0).is_ok()
            && attributes.iter().any(|attribute| {
                attribute.name.namespace().is_none()
                    && attribute.name.qualified() == "http-equiv"
                    && attribute.value.eq_ignore_ascii_case("Content-Type")
            })
    };
    named(node, "head")
        && node.parent().is_some_and(|parent| {
            named(parent, "html") && parent.parent().is_some_and(Tree::is_document)
        })
        && !node.children().any(content_type)
}

/// Namespaces an element subtree uses but does not declare, as (element
/// preorder ordinal, prefix, URI), what a copy declares (`reconcile_copy`):
/// prefixes on the root (ordinal 0) in first-use order; the default
/// namespace on each element whose own differs from the one in scope, or
/// the first one used below the root hoisted to it when that takes no more
/// declarations (a detached subtree's undeclared default would otherwise
/// capture its unprefixed descendants in no namespace).
fn missing_namespaces(root: Tree<'_, '_>) -> Vec<(usize, Option<String>, String)> {
    if root.name().is_none() {
        return Vec::new();
    }
    let each = scan_namespaces(root, None).unwrap_or_default();
    let first = each.iter().find(|(_, prefix, _)| prefix.is_none());
    first
        .filter(|(at, ..)| *at > 0)
        .and_then(|(at, _, uri)| scan_namespaces(root, Some((*at, uri))))
        .filter(|hoisted| hoisted.len() <= each.len())
        .unwrap_or(each)
}

/// [`missing_namespaces`] with `seed`'s (ordinal, URI) default declared on
/// the root at its first use; None if the root needs its own default.
fn scan_namespaces(
    root: Tree<'_, '_>,
    seed: Option<(usize, &str)>,
) -> Option<Vec<(usize, Option<String>, String)>> {
    let mut added: Vec<(usize, Option<String>, String)> = Vec::new();
    let mut declared: Vec<_> = seed
        .map(|(_, uri)| (None, uri.to_owned()))
        .into_iter()
        .collect();
    let (mut marks, mut attributes) = (Vec::new(), Vec::new());
    // Below the root, in preorder: `added` stays as short as its prefixes.
    let mut defaults = Vec::new();
    let mut node = root;
    let mut ordinal = 0;
    loop {
        if let Some(name) = node.name() {
            marks.push(declared.len());
            if node.attributes(&mut attributes, usize::MAX, 0).is_err() {
                return Some(added);
            }
            let before = declared.len();
            declared.extend(
                attributes
                    .iter()
                    .filter(|attribute| is_declaration(attribute))
                    .map(|attribute| {
                        let prefix = attribute.name.prefix();
                        let prefix = prefix.map(|_| attribute.name.local().to_owned());
                        (prefix, attribute.value.to_string())
                    }),
            );
            let own = declared[before..]
                .iter()
                .any(|(prefix, _)| prefix.is_none());
            let wanted = name.namespace().unwrap_or("");
            let scope = declared.iter().rev().find(|(prefix, _)| prefix.is_none());
            let differs = name.prefix().is_none()
                && !own
                && scope.map_or("", |(_, uri)| uri.as_str()) != wanted;
            match seed {
                Some(_) if ordinal == 0 && (own || differs) => return None,
                Some((at, uri)) if at == ordinal => added.push((0, None, uri.to_owned())),
                _ => {}
            }
            if differs {
                declared.push((None, wanted.to_owned()));
                match ordinal {
                    0 => &mut added,
                    _ => &mut defaults,
                }
                .push((ordinal, None, wanted.to_owned()));
            }
            let names = std::iter::once(&name).chain(
                attributes
                    .iter()
                    .filter(|attribute| !is_declaration(attribute))
                    .map(|attribute| &attribute.name),
            );
            for name in names {
                if let (Some(prefix), Some(uri)) = (name.prefix(), name.namespace()) {
                    let prefix = Some(prefix.to_owned());
                    if prefix.as_deref() != Some("xml")
                        && !declared.iter().any(|(bound, _)| *bound == prefix)
                        && !added.iter().any(|(_, bound, _)| *bound == prefix)
                    {
                        added.push((0, prefix, uri.to_owned()));
                    }
                }
            }
            ordinal += 1;
            if let Some(child) = node.first_child() {
                node = child;
                continue;
            }
            declared.truncate(marks.pop().unwrap_or_default());
        }
        loop {
            let Some(parent) = node.parent().filter(|_| !node.same(root)) else {
                added.append(&mut defaults);
                return Some(added);
            };
            if let Some(next) = node.next_sibling() {
                node = next;
                break;
            }
            node = parent;
            declared.truncate(marks.pop().unwrap_or_default());
        }
    }
}

#[cfg(test)]
#[path = "../tests/internal/save.rs"]
mod tests;
