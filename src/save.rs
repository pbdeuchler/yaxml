//! Tree output with libxml2 2.15.4's `xmlsave.c` behavior: `xmlSaveDoc` for
//! a document and `xmlSaveTree`/`xmlNodeDump` for any other node of either
//! DOM, under `xmlSaveOption` flags and an output encoding. The internal
//! subset is regenerated from its declarations, as libxml2 dumps its parsed
//! DTD. See `docs/writer.md` for the contract and the differences.
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
//! let latin1 = save::Options { encoding: Some("ISO-8859-1"), ..options };
//! let a = document.root_element().first_child().unwrap();
//! assert_eq!(save::to_vec(a, &latin1)?, b"<a>caf\xe9</a>");
//! # Ok::<(), yaxml::Error>(())
//! ```

use std::borrow::Cow;

use crate::encoding::Output;
use crate::{
    Attribute, Declaration, Doctype, Document, Error, ErrorKind, Event, Limit, Name, Node,
    NodeKind, OwnedDocument, OwnedNode, OwnedNodeKind,
};

mod dtd;

/// `XML_SAVE_FORMAT`: indent element content, except inside elements with
/// text or CDATA children.
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
const SUPPORTED: u32 = (1 << 11) - 1;

const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
const XHTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
/// libxml2's `MAX_INDENT`: the longest indent string and widest indentation.
const MAX_INDENT: usize = 60;

/// `xmlSaveOption` flags, the output encoding and the indent string.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Options<'a> {
    pub flags: u32,
    /// The output encoding: UTF-8, UTF-16 (little-endian with a BOM),
    /// UTF-16LE, UTF-16BE, US-ASCII, ISO-8859-x or windows-1252, under any
    /// libxml2 alias. The label is written verbatim into the XML declaration.
    /// `None` uses a document's declared encoding; with no encoding at all,
    /// text and attribute values escape non-ASCII characters as hexadecimal
    /// references and the declaration names none.
    pub encoding: Option<&'a str>,
    /// `xmlSaveSetIndentString`: 1 to 60 spaces or tabs, two spaces by
    /// default. libxml2 takes any string, which can make the output malformed.
    pub indent: &'a str,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self {
            flags: 0,
            encoding: None,
            indent: "  ",
        }
    }
}

impl Options<'_> {
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
    Borrowed(Node<'document, 'input>),
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

impl<'document, 'input> Tree<'document, 'input> {
    pub(crate) fn first_child(self) -> Option<Self> {
        match self {
            Self::Borrowed(node) => node.first_child().map(Self::Borrowed),
            Self::Owned(node) => node.first_child().map(Self::Owned),
        }
    }

    pub(crate) fn next_sibling(self) -> Option<Self> {
        match self {
            Self::Borrowed(node) => node.next_sibling().map(Self::Borrowed),
            Self::Owned(node) => node.next_sibling().map(Self::Owned),
        }
    }

    pub(crate) fn parent(self) -> Option<Self> {
        match self {
            Self::Borrowed(node) => node.parent().map(Self::Borrowed),
            Self::Owned(node) => node.parent().map(Self::Owned),
        }
    }

    pub(crate) fn name(self) -> Option<Name<'document>> {
        match self {
            Self::Borrowed(node) => node.name().map(Name::as_borrowed),
            Self::Owned(node) => node.name(),
        }
    }

    /// A leaf node's event; documents and elements have none.
    pub(crate) fn leaf(self) -> Option<Event<'document>> {
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
                NodeKind::Document | NodeKind::Element(_) => None,
            },
            Self::Owned(node) => owned_leaf(node),
        }
    }

    /// Replaces `attributes` with this element's, failing above `limit`.
    pub(crate) fn attributes(
        self,
        attributes: &mut Vec<Attribute<'document>>,
        limit: usize,
        offset: usize,
    ) -> Result<(), Error> {
        attributes.clear();
        let count = match self {
            Self::Borrowed(node) => node.attributes().len(),
            Self::Owned(node) => node.attributes().len(),
        };
        if count > limit {
            return Err(Error::new(
                ErrorKind::LimitExceeded(Limit::Attributes),
                offset,
            ));
        }
        attributes
            .try_reserve_exact(count)
            .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
        match self {
            Self::Borrowed(node) => {
                attributes.extend(node.attributes().iter().map(|attribute| Attribute {
                    name: attribute.name.as_borrowed(),
                    value: Cow::Borrowed(&attribute.value),
                }))
            }
            Self::Owned(node) => attributes.extend(node.attributes()),
        }
        Ok(())
    }

    fn is_document(self) -> bool {
        match self {
            Self::Borrowed(node) => matches!(node.kind(), NodeKind::Document),
            Self::Owned(node) => node.kind() == OwnedNodeKind::Document,
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

/// A leaf node's event; documents and elements have none.
pub(crate) fn owned_leaf(node: OwnedNode<'_>) -> Option<Event<'_>> {
    match node.kind() {
        OwnedNodeKind::DocumentType => node.doctype().map(Event::Doctype),
        OwnedNodeKind::Text => Some(Event::Text(Cow::Borrowed(node.text()?))),
        OwnedNodeKind::Cdata => Some(Event::Cdata(Cow::Borrowed(node.text()?))),
        OwnedNodeKind::Comment => Some(Event::Comment(Cow::Borrowed(node.text()?))),
        OwnedNodeKind::ProcessingInstruction => Some(Event::ProcessingInstruction {
            target: Cow::Borrowed(node.processing_instruction_target()?),
            data: Cow::Borrowed(node.text()?),
        }),
        OwnedNodeKind::Document | OwnedNodeKind::Element => None,
    }
}

/// Serializes `tree`: `xmlSaveDoc` for a document node, otherwise
/// `xmlSaveTree` with the namespace declarations the subtree uses but does
/// not declare added to its root, as on a copy (`xmlDocCopyNode`).
///
/// Unknown flags or another indent are `UnsupportedOption`;
/// an unknown encoding is `UnsupportedEncoding`. Text and attribute values
/// write characters the encoding lacks as `&#N;`; in names, comments, PIs,
/// CDATA and DOCTYPE identifiers, where libxml2 writes such references too
/// and changes the content, they are `InvalidCharacter` at their output
/// offset. HTML documents use the HTML serializer unless [`AS_XML`] or
/// [`XHTML`] is set; [`AS_HTML`] needs an owned document node, and HTML
/// subtrees need [`AS_XML`].
pub fn to_vec<'document, 'input: 'document>(
    tree: impl Into<Tree<'document, 'input>>,
    options: &Options<'_>,
) -> Result<Vec<u8>, Error> {
    let tree = tree.into();
    let unsupported = Error::new(ErrorKind::UnsupportedOption, 0);
    let indent = &options.indent;
    if options.flags & !SUPPORTED != 0
        || !(1..=MAX_INDENT).contains(&indent.len())
        || indent.bytes().any(|byte| byte != b' ' && byte != b'\t')
    {
        return Err(unsupported);
    }
    let document = tree.is_document();
    let encoding = options
        .encoding
        .or_else(|| document.then(|| tree.encoding()).flatten());
    let html = tree.owned_document().is_some_and(OwnedDocument::is_html);
    if options.has(AS_HTML) || html && !options.has(AS_XML) && !options.has(XHTML) {
        return match tree.owned_document().filter(|_| document) {
            Some(owned) => {
                crate::html::save_as(owned, options.has(FORMAT), encoding.unwrap_or("HTML"))
            }
            None => Err(unsupported),
        };
    }
    let output = match encoding {
        Some(label) => {
            Output::for_label(label).ok_or(Error::new(ErrorKind::UnsupportedEncoding, 0))?
        }
        None => Output::Utf8,
    };
    let mut saver = Saver {
        out: Out::new(output, encoding.is_none()),
        options,
        format: if options.has(FORMAT) {
            1
        } else if options.has(WSNONSIG) {
            2
        } else {
            0
        },
        level: 0,
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

/// Encoded output; the first character the encoding cannot write in markup
/// is recorded and reported at the end, as libxml2's `buf->error`.
struct Out {
    bytes: Vec<u8>,
    output: Output,
    /// No encoding: text escapes non-ASCII as hexadecimal references.
    ascii: bool,
    error: Option<Error>,
}

/// `xmlEscapeTab` and `xmlEscapeTabAttr`: other C0 controls become U+FFFD.
const fn escapes(attribute: bool) -> [Option<&'static str>; 128] {
    let mut table = [None; 128];
    let mut byte = 0;
    while byte < 0x20 {
        table[byte] = Some("&#xFFFD;");
        byte += 1;
    }
    table[b'\t' as usize] = if attribute { Some("&#9;") } else { None };
    table[b'\n' as usize] = if attribute { Some("&#10;") } else { None };
    table[b'\r' as usize] = Some("&#13;");
    table[b'&' as usize] = Some("&amp;");
    table[b'<' as usize] = Some("&lt;");
    table[b'>' as usize] = Some("&gt;");
    if attribute {
        table[b'"' as usize] = Some("&quot;");
    }
    table
}

const CONTENT: [Option<&str>; 128] = escapes(false);
const ATTRIBUTE: [Option<&str>; 128] = escapes(true);

impl Out {
    fn new(output: Output, ascii: bool) -> Self {
        Self {
            bytes: output.bom().to_vec(),
            output,
            ascii,
            error: None,
        }
    }

    /// An empty buffer with the same encoding, for [`Out::append`].
    fn sibling(&self) -> Self {
        Self {
            bytes: Vec::new(),
            ..*self
        }
    }

    fn append(&mut self, other: Self) {
        if self.error.is_none()
            && let Some(error) = other.error
        {
            self.error = Some(Error::new(error.kind, self.bytes.len() + error.offset));
        }
        self.bytes.extend_from_slice(&other.bytes);
    }

    fn finish(self) -> Result<Vec<u8>, Error> {
        self.error.map_or(Ok(self.bytes), Err)
    }

    /// Markup, names, comments, PIs and CDATA, which cannot hold references.
    fn raw(&mut self, text: &str) {
        if self.output == Output::Utf8 {
            return self.bytes.extend_from_slice(text.as_bytes());
        }
        for c in text.chars() {
            if !self.output.push(&mut self.bytes, c) && self.error.is_none() {
                self.error = Some(Error::new(ErrorKind::InvalidCharacter, self.bytes.len()));
            }
        }
    }

    /// Text the encoder converts, writing `&#N;` for what it lacks.
    fn converted(&mut self, text: &str) {
        for c in text.chars() {
            if !self.output.push(&mut self.bytes, c) {
                self.reference(c, false);
            }
        }
    }

    /// `xmlSerializeText` followed by the encoder: escaped text and
    /// attribute values, with references for what the encoding lacks.
    fn text(&mut self, text: &str, attribute: bool) {
        let table = if attribute { &ATTRIBUTE } else { &CONTENT };
        let unicode = self.output == Output::Utf8 && !self.ascii;
        let bytes = text.as_bytes();
        let (mut start, mut index) = (0, 0);
        while index < bytes.len() {
            let byte = bytes[index];
            if byte < 0x80 {
                let Some(escape) = table[usize::from(byte)] else {
                    index += 1;
                    continue;
                };
                self.raw(&text[start..index]);
                self.raw(escape);
                index += 1;
            } else if unicode {
                index += 1;
                continue;
            } else {
                self.raw(&text[start..index]);
                let c = text[index..].chars().next().unwrap_or_default();
                if self.ascii {
                    self.reference(c, true);
                } else if !self.output.push(&mut self.bytes, c) {
                    self.reference(c, false);
                }
                index += c.len_utf8();
            }
            start = index;
        }
        self.raw(&text[start..]);
    }

    /// `&#xH;` as `xmlSerializeHexCharRef` writes it, or the encoder's `&#N;`.
    fn reference(&mut self, c: char, hex: bool) {
        let (base, prefix) = if hex { (16, "&#x") } else { (10, "&#") };
        let mut digits = [0; 8];
        let (mut value, mut start) = (u32::from(c), digits.len());
        loop {
            start -= 1;
            digits[start] = b"0123456789ABCDEF"[(value % base) as usize];
            value /= base;
            if value == 0 {
                break;
            }
        }
        self.raw(prefix);
        self.raw(std::str::from_utf8(&digits[start..]).unwrap_or_default());
        self.raw(";");
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

struct Saver<'a> {
    out: Out,
    options: &'a Options<'a>,
    /// libxml2's `ctxt->format`: 0, 1 for [`FORMAT`], 2 for [`WSNONSIG`].
    format: u8,
    level: usize,
    /// `XML_SAVE_XHTML` in the context: empty boolean attributes are expanded.
    xhtml_attributes: bool,
    encoding: Option<&'a str>,
}

impl<'a> Saver<'a> {
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
            xhtml = is_xhtml(&doctype);
        }
        self.xhtml_attributes |= xhtml;
        for child in root.children() {
            self.level = 0;
            self.node(child, xhtml, &[]);
            self.out.raw("\n");
        }
    }

    /// `xmlNodeDumpOutputInternal`, or `xhtmlNodeDumpOutput` for `xhtml`,
    /// with `added` namespace declarations on the root element.
    fn node(&mut self, root: Tree<'_, '_>, xhtml: bool, added: &[(Option<String>, String)]) {
        let format = self.format;
        let mut unformatted = None;
        let mut depth = 0;
        let mut attributes = Vec::new();
        let mut current = root;
        loop {
            let is_root = current.same(root);
            if let Some(name) = current.name() {
                let added = if is_root { added } else { &[] };
                if self.element(current, &name, is_root, xhtml, added, &mut attributes) {
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
                if current.same(root) {
                    return;
                }
                if self.format == 1 {
                    self.out.raw("\n");
                }
                if let Some(next) = current.next_sibling() {
                    current = next;
                    break;
                }
                let Some(parent) = current.parent() else {
                    return;
                };
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
        added: &[(Option<String>, String)],
        attributes: &mut Vec<Attribute<'d>>,
    ) -> bool {
        if !is_root && self.format == 1 {
            self.indent(0);
        }
        let qualified = name.qualified();
        self.out.raw("<");
        self.out.raw(qualified);
        if let Err(error) = node.attributes(attributes, usize::MAX, self.out.bytes.len()) {
            self.out.error.get_or_insert(error);
        }
        let mut declared = false;
        for attribute in attributes.iter().filter(|a| is_declaration(a)) {
            declared = true;
            if attribute.name.local() != "xml" || attribute.name.prefix().is_none() {
                self.attribute(attribute.name.qualified(), &attribute.value, false);
            }
        }
        for (prefix, uri) in added {
            declared = true;
            match prefix {
                Some(prefix) => self.attribute(&format!("xmlns:{prefix}"), uri, false),
                None => self.attribute("xmlns", uri, false),
            }
        }
        if xhtml && qualified == "html" && name.namespace().is_none() && !declared {
            self.out.raw(" xmlns=\"http://www.w3.org/1999/xhtml\"");
        }
        self.attributes(
            name,
            attributes.iter().filter(|a| !is_declaration(a)),
            xhtml,
        );
        if node.first_child().is_some() {
            return true;
        }
        let add_meta = xhtml && needs_meta(node);
        if xhtml && name.prefix().is_none() && xhtml_empty(node, name) && !add_meta {
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
    /// no namespace and may be XHTML booleans.
    fn attribute(&mut self, name: &str, value: &str, plain: bool) {
        if self.format == 2 {
            self.whitespace(2);
        } else {
            self.out.raw(" ");
        }
        self.out.raw(name);
        self.out.raw("=\"");
        if self.xhtml_attributes
            && plain
            && value.is_empty()
            && crate::html::boolean_attribute(name)
        {
            self.out.raw(name);
        } else {
            self.out.text(value, true);
        }
        self.out.raw("\"");
    }

    /// The attributes; under XHTML (`xhtmlAttrListDumpOutput`) C.8 adds `id`
    /// from `name` and C.7 pairs `lang` and `xml:lang`.
    fn attributes<'v>(
        &mut self,
        element: &Name<'_>,
        attributes: impl Iterator<Item = &'v Attribute<'v>>,
        xhtml: bool,
    ) {
        let (mut id, mut name, mut lang, mut xml_lang) = (false, None, None, None);
        for attribute in attributes {
            let namespaced = attribute.name.namespace().is_some();
            match (namespaced, attribute.name.qualified()) {
                (false, "id") => id = true,
                (false, "name") => name = Some(&attribute.value),
                (false, "lang") => lang = Some(&attribute.value),
                (true, "xml:lang") => xml_lang = Some(&attribute.value),
                _ => {}
            }
            self.attribute(attribute.name.qualified(), &attribute.value, !namespaced);
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

    fn extra(&mut self, start: &str, value: &str) {
        self.out.raw(start);
        self.out.text(value, true);
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
            _ => {}
        }
    }
}

fn is_declaration(attribute: &Attribute<'_>) -> bool {
    attribute.name.namespace() == Some(XMLNS)
}

/// Text (or, outside XHTML, CDATA) children turn formatting off.
fn has_text(node: Tree<'_, '_>, xhtml: bool) -> bool {
    node.children().any(|child| match child.leaf() {
        Some(Event::Text(_)) => true,
        Some(Event::Cdata(_)) => !xhtml,
        _ => false,
    })
}

/// `xhtmlIsEmpty`: a childless XHTML element whose content model is EMPTY.
fn xhtml_empty(node: Tree<'_, '_>, name: &Name<'_>) -> bool {
    const EMPTY: [&str; 13] = [
        "area", "br", "base", "basefont", "col", "frame", "hr", "img", "input", "isindex", "link",
        "meta", "param",
    ];
    node.first_child().is_none()
        && name.namespace().is_none_or(|uri| uri == XHTML_NAMESPACE)
        && EMPTY.contains(&name.local())
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

/// `xmlIsXHTML`: one of the three XHTML 1.0 public or system identifiers.
fn is_xhtml(doctype: &Doctype<'_>) -> bool {
    const PUBLIC: [&str; 3] = [
        "-//W3C//DTD XHTML 1.0 Strict//EN",
        "-//W3C//DTD XHTML 1.0 Frameset//EN",
        "-//W3C//DTD XHTML 1.0 Transitional//EN",
    ];
    const SYSTEM: [&str; 3] = [
        "http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd",
        "http://www.w3.org/TR/xhtml1/DTD/xhtml1-frameset.dtd",
        "http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd",
    ];
    doctype.public_id.is_some_and(|id| PUBLIC.contains(&id))
        || doctype.system_id.is_some_and(|id| SYSTEM.contains(&id))
}

/// Namespaces an element subtree uses but does not declare, in first-use
/// order: what a copy declares on its root.
fn missing_namespaces(root: Tree<'_, '_>) -> Vec<(Option<String>, String)> {
    let mut added: Vec<(Option<String>, String)> = Vec::new();
    if root.name().is_none() {
        return added;
    }
    let (mut declared, mut marks, mut attributes) = (Vec::new(), Vec::new(), Vec::new());
    let mut node = root;
    loop {
        if let Some(name) = node.name() {
            marks.push(declared.len());
            if node.attributes(&mut attributes, usize::MAX, 0).is_err() {
                return added;
            }
            declared.extend(
                attributes
                    .iter()
                    .filter(|attribute| is_declaration(attribute))
                    .map(|attribute| {
                        attribute
                            .name
                            .prefix()
                            .map(|_| attribute.name.local().to_owned())
                    }),
            );
            let names = std::iter::once(&name).chain(
                attributes
                    .iter()
                    .filter(|attribute| !is_declaration(attribute))
                    .map(|attribute| &attribute.name),
            );
            for name in names {
                if let Some(uri) = name.namespace() {
                    let prefix = name.prefix().map(str::to_owned);
                    if prefix.as_deref() != Some("xml")
                        && !declared.contains(&prefix)
                        && !added.iter().any(|(bound, _)| *bound == prefix)
                    {
                        added.push((prefix, uri.to_owned()));
                    }
                }
            }
            if let Some(child) = node.first_child() {
                node = child;
                continue;
            }
            declared.truncate(marks.pop().unwrap_or_default());
        }
        loop {
            if node.same(root) {
                return added;
            }
            if let Some(next) = node.next_sibling() {
                node = next;
                break;
            }
            match node.parent() {
                Some(parent) => node = parent,
                None => return added,
            }
            declared.truncate(marks.pop().unwrap_or_default());
        }
    }
}

#[cfg(test)]
#[path = "../tests/internal/save.rs"]
mod tests;
