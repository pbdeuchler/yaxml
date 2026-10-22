//! HTML output, ported from libxml2 2.15.4 `HTMLtree.c` (htmlNodeDumpInternal,
//! htmlAttrDumpOutput, htmlSerializeUri, htmlDtdDumpOutput) and the HTML
//! escape tables of `xmlIO.c`. The walk is iterative; output reaches the
//! sink in chunks of `save::CHUNK` bytes.

use std::io::Write;

use super::encoding::{content_charset, encode, output};
use super::tables;
use super::tokenizer::blank;
use crate::encoding::Output;
use crate::fallible::Memory;
use crate::save::CHUNK;
use crate::{
    Doctype, Error, ErrorKind, OwnedDocument, OwnedNode, OwnedNodeId, OwnedNodeKind, WriteError,
};

/// `htmlDocDumpMemory`: formatted, in the document's encoding (ASCII with
/// HTML 4 entities when it has none), leaving `<meta>` declarations alone.
pub fn to_html(document: &OwnedDocument) -> Result<Vec<u8>, Error> {
    to_vec(document.root(), true, None, None)
}

/// `htmlNodeDump` (`format` true) and `htmlNodeDumpFormatOutput` into an
/// output buffer without a converter: `node` and its subtree, or the whole
/// document for its root (`htmlDocContentDumpFormatOutput`), in UTF-8 with
/// `<meta>` declarations left alone.
///
/// ```
/// use yaxml::html;
/// let document = html::parse(b"<p>a&lt;<b>x</b></p>", html::Options::default())?.document;
/// let p = document.document_element().unwrap().last_child().unwrap().first_child().unwrap();
/// assert_eq!(html::dump_node(p, false)?, b"<p>a&lt;<b>x</b></p>");
/// // As in libxml2, formatting adds a line end only before a next sibling.
/// assert_eq!(html::dump_node(p, true)?, b"<p>a&lt;<b>x</b></p>");
/// # Ok::<(), yaxml::Error>(())
/// ```
pub fn dump_node(node: OwnedNode<'_>, format: bool) -> Result<Vec<u8>, Error> {
    to_vec(node, format, Some("UTF-8"), None)
}

/// `xmlSaveDoc` for an HTML document, which is what `xmllint --html` prints:
/// `<meta>` charset declarations name the output encoding and a top-level
/// head without one gains `<meta charset>`. `format` is XML_SAVE_FORMAT.
pub fn save(document: &OwnedDocument, format: bool) -> Result<Vec<u8>, Error> {
    to_vec(
        document.root(),
        format,
        Some(document.html_encoding().unwrap_or("HTML")),
        Some(document.html_encoding().unwrap_or("HTML")),
    )
}

/// `xmlSaveTree` with `XML_SAVE_AS_HTML` (`htmlNodeDumpOutputInternal`) and
/// encoding `label`, which names both the output converter and the `<meta>`
/// charset; a document node is `xmlSaveDoc`.
pub(crate) fn save_node(
    node: OwnedNode<'_>,
    format: bool,
    label: &str,
    sink: &mut dyn Write,
) -> Result<(), WriteError> {
    dump(node, format, Some(label), Some(label), sink)
}

fn to_vec(
    node: OwnedNode<'_>,
    format: bool,
    label: Option<&str>,
    meta: Option<&str>,
) -> Result<Vec<u8>, Error> {
    let mut bytes = Memory::default();
    match dump(node, format, label, meta, &mut bytes) {
        Ok(()) => Ok(bytes.0),
        Err(WriteError::Xml(error)) => Err(error),
        // Memory fails only to grow.
        Err(WriteError::Io(_)) => Err(Error::new(ErrorKind::Allocation, bytes.0.len())),
    }
}

/// Writes `node` converted by the `label` converter (the document's
/// encoding when None), naming `meta` in `<meta>` declarations (None leaves
/// them unchanged).
fn dump(
    node: OwnedNode<'_>,
    format: bool,
    label: Option<&str>,
    meta: Option<&str>,
    sink: &mut dyn Write,
) -> Result<(), WriteError> {
    let output = output(label.or(node.document().html_encoding())).map_err(WriteError::Xml)?;
    let mut dumper = Dumper {
        out: Vec::new(),
        encoded: output.map_or(&[][..], Output::bom).to_vec(),
        sink,
        output,
        error: None,
        format,
        encoding: meta,
        meta_head: None,
    };
    dumper.node(node);
    dumper.drain(0);
    match dumper.error {
        Some(error) => Err(error),
        None => crate::writer::flush(dumper.sink),
    }
}

struct Dumper<'a> {
    /// UTF-8 output not yet encoded.
    out: Vec<u8>,
    /// Encoded output not yet written.
    encoded: Vec<u8>,
    sink: &'a mut dyn Write,
    output: Option<Output>,
    error: Option<WriteError>,
    format: bool,
    /// Encoding for `<meta>` declarations; None leaves them unchanged.
    encoding: Option<&'a str>,
    /// The head that received a generated `<meta charset>`.
    meta_head: Option<OwnedNodeId>,
}

fn refused() -> WriteError {
    WriteError::Xml(Error::new(ErrorKind::Allocation, 0))
}

fn element_name<'a>(node: OwnedNode<'a>) -> Option<&'a str> {
    node.qualified_name()
}

/// Text in the formatting rules: Text nodes, not CDATA.
fn is_text(node: Option<OwnedNode<'_>>) -> bool {
    node.is_some_and(|node| node.kind() == OwnedNodeKind::Text)
}

/// `htmlTagLookup` for elements without a namespace.
fn info(node: OwnedNode<'_>) -> Option<&'static tables::Element> {
    let name = node.name()?;
    if name.namespace().is_some() {
        return None;
    }
    tables::element(name.qualified())
}

/// Not p, pre or param: libxml2 tests only the first letter.
fn not_p(node: Option<OwnedNode<'_>>) -> bool {
    node.and_then(|node| node.name())
        .is_some_and(|name| !name.qualified().starts_with('p'))
}

/// `htmlFindMetaEncodingAttr`: the position of a charset attribute, or of
/// the content attribute of an http-equiv Content-Type meta element.
fn meta_attribute(node: OwnedNode<'_>) -> Option<(usize, bool)> {
    if !node.name()?.qualified().eq_ignore_ascii_case("meta") {
        return None;
    }
    let mut content = None;
    let mut content_type = false;
    for (index, attribute) in node.attributes().enumerate() {
        if attribute.name.namespace().is_some() {
            continue;
        }
        let name = attribute.name.qualified();
        if name.eq_ignore_ascii_case("charset") {
            return Some((index, false));
        }
        if name.eq_ignore_ascii_case("content") {
            content = Some(index);
        }
        if name.eq_ignore_ascii_case("http-equiv")
            && node.attribute_has_value(index)
            && attribute.value.eq_ignore_ascii_case("Content-Type")
        {
            content_type = true;
        }
    }
    content.filter(|_| content_type).map(|index| (index, true))
}

/// `htmlParseMetaEncoding`: the attribute and byte range naming the charset.
fn meta_encoding(node: OwnedNode<'_>) -> Option<(usize, usize, usize)> {
    let (index, content_type) = meta_attribute(node)?;
    let attribute = node.attributes().nth(index)?;
    let value = attribute.value.as_ref();
    if content_type {
        let (start, end) = content_charset(value)?;
        return Some((index, start, end));
    }
    let start = value.len()
        - value
            .trim_start_matches(|c: char| c.is_ascii() && blank(c as u8))
            .len();
    let end = value
        .trim_end_matches(|c: char| c.is_ascii() && blank(c as u8))
        .len()
        .max(start);
    Some((index, start, end))
}

impl Dumper<'_> {
    fn write(&mut self, text: &str) {
        if self.room(text) {
            self.out.extend_from_slice(text.as_bytes());
        }
        self.drain(CHUNK);
    }

    /// Room for `text` before the next drain; a refusal is an allocation
    /// error, after which nothing reaches the sink.
    fn room(&mut self, text: &str) -> bool {
        let room = self.out.try_reserve(text.len()).is_ok();
        if !room {
            self.error.get_or_insert(refused());
        }
        room
    }

    /// Encodes and writes the buffered output once it holds `threshold`
    /// bytes. It breaks only between characters: `escape` pushes every
    /// non-ASCII byte and calls `write` only for ASCII.
    fn drain(&mut self, threshold: usize) {
        if self.out.len() < threshold {
            return;
        }
        // A character reference or entity is at most 5 bytes per input byte.
        let width = if self.output == Some(Output::Utf8) {
            1
        } else {
            5
        };
        if self.encoded.try_reserve(width * self.out.len()).is_err() {
            self.out.clear();
            self.error.get_or_insert(refused());
            return;
        }
        encode(self.output, &self.out, &mut self.encoded);
        self.out.clear();
        if self.error.is_none()
            && let Err(error) = crate::writer::write_all(self.sink, &self.encoded, &mut 0)
        {
            self.error = Some(error);
        }
        self.encoded.clear();
    }

    /// `xmlSerializeText` with the HTML tables: `&`, `<`, `>`, and `"` in attributes.
    fn escape(&mut self, text: &str, attribute: bool) {
        if !self.room(text) {
            return;
        }
        for c in text.bytes() {
            match c {
                b'&' => self.write("&amp;"),
                b'<' => self.write("&lt;"),
                b'>' => self.write("&gt;"),
                b'"' if attribute => self.write("&quot;"),
                0 => return,
                _ => self.out.push(c),
            }
        }
    }

    /// `htmlSerializeUri`: leading blanks kept, then controls, space, DEL and
    /// non-ASCII bytes percent-encoded.
    fn uri(&mut self, text: &str) {
        let rest = text.trim_start_matches(|c: char| c.is_ascii() && blank(c as u8));
        self.write(&text[..text.len() - rest.len()]);
        if !self.room(rest) {
            return;
        }
        for c in rest.bytes() {
            match c {
                0 => return,
                b'"' => self.write("&quot;"),
                b'&' => self.write("&amp;"),
                0x21..=0x7E => self.out.push(c),
                _ => self.write(&format!("%{c:02X}")),
            }
        }
    }

    /// `xmlOutputBufferWriteQuotedString`.
    fn quoted(&mut self, text: &str) {
        if !text.contains('"') {
            self.write(&format!("\"{text}\""));
        } else if !text.contains('\'') {
            self.write(&format!("'{text}'"));
        } else {
            self.write(&format!("\"{}\"", text.replace('"', "&quot;")));
        }
    }

    fn doctype(&mut self, doctype: Doctype<'_>) {
        self.write("<!DOCTYPE ");
        self.write(doctype.name);
        match (doctype.public_id, doctype.system_id) {
            (Some(public), system) => {
                self.write(" PUBLIC ");
                self.quoted(public);
                if let Some(system) = system {
                    self.write(" ");
                    self.quoted(system);
                }
            }
            (None, Some(system)) if system != "about:legacy-compat" => {
                self.write(" SYSTEM ");
                self.quoted(system);
            }
            _ => {}
        }
        self.write(">\n");
    }

    /// `htmlNodeDumpInternal` from `root`: a document writes its DOCTYPE and
    /// ends with a newline.
    fn node(&mut self, root: OwnedNode<'_>) {
        let mut current = root;
        let mut raw = false;
        loop {
            let descend = match current.kind() {
                OwnedNodeKind::Document => {
                    if let Some(doctype) = current.document().doctype() {
                        self.doctype(doctype);
                    }
                    if current.first_child().is_none() {
                        self.write("\n");
                    }
                    true
                }
                OwnedNodeKind::Element => self.start(current, &mut raw),
                OwnedNodeKind::Text if raw => {
                    self.write(current.text().unwrap_or(""));
                    false
                }
                OwnedNodeKind::Text => {
                    self.escape(current.text().unwrap_or(""), false);
                    false
                }
                OwnedNodeKind::Cdata => {
                    self.write(current.text().unwrap_or(""));
                    false
                }
                OwnedNodeKind::Comment => {
                    self.write("<!--");
                    self.write(current.text().unwrap_or(""));
                    self.write("-->");
                    false
                }
                OwnedNodeKind::ProcessingInstruction => {
                    self.write("<?");
                    self.write(current.processing_instruction_target().unwrap_or(""));
                    if let Some(data) = current.text().filter(|data| !data.is_empty()) {
                        self.write(" ");
                        self.write(data);
                    }
                    self.write(">");
                    false
                }
                _ => false,
            };
            if descend && let Some(child) = current.first_child() {
                current = child;
                continue;
            }
            loop {
                if current.id() == root.id() {
                    return;
                }
                if let Some(next) = current.next_sibling() {
                    current = next;
                    break;
                }
                raw = false;
                // Below `root`, every node has a parent.
                let Some(parent) = current.parent() else {
                    return;
                };
                current = parent;
                if parent.kind() == OwnedNodeKind::Element {
                    self.end(parent);
                } else {
                    self.write("\n");
                }
            }
        }
    }

    /// The element case of htmlNodeDumpInternal; true to visit the children.
    fn start(&mut self, node: OwnedNode<'_>, raw: &mut bool) -> bool {
        let info = info(node);
        let name = element_name(node).unwrap_or("");
        let parent = node.parent();
        let meta = self.encoding.and_then(|_| meta_encoding(node));
        let add_meta = self
            .encoding
            .is_some_and(|encoding| !encoding.eq_ignore_ascii_case("HTML"))
            && name.eq_ignore_ascii_case("head")
            && parent
                .and_then(|parent| parent.name())
                .is_some_and(|parent| parent.qualified().eq_ignore_ascii_case("html"))
            && parent
                .and_then(|parent| parent.parent())
                .is_some_and(|grandparent| grandparent.kind() == OwnedNodeKind::Document)
            && self.meta_head.is_none()
            && node.children().all(|child| meta_attribute(child).is_none());
        if add_meta {
            self.meta_head = Some(node.id());
        }
        let encoding = self.encoding.unwrap_or("");
        self.write("<");
        self.write(name);
        for (index, attribute) in node.attributes().enumerate() {
            let attribute_name = attribute.name.qualified();
            match meta {
                Some((meta, start, end)) if meta == index => {
                    self.write(" ");
                    self.write(attribute_name);
                    self.write("=\"");
                    let value = attribute.value.as_ref();
                    self.escape(&value[..start], true);
                    self.escape(encoding, true);
                    self.escape(&value[end..], true);
                    self.write("\"");
                }
                _ => {
                    self.write(" ");
                    self.write(attribute_name);
                    if node.attribute_has_value(index) && !tables::boolean_attribute(attribute_name)
                    {
                        self.write("=\"");
                        let uri = attribute.name.namespace().is_none()
                            && node.name().is_some_and(|name| name.namespace().is_none())
                            && (["href", "action", "src"]
                                .iter()
                                .any(|uri| attribute_name.eq_ignore_ascii_case(uri))
                                || attribute_name.eq_ignore_ascii_case("name")
                                    && name.eq_ignore_ascii_case("a"));
                        if uri {
                            self.uri(&attribute.value);
                        } else {
                            self.escape(&attribute.value, true);
                        }
                        self.write("\"");
                    }
                }
            }
        }
        let first = node.first_child();
        if info.is_some_and(|info| info.empty) {
            self.write(">");
        } else if first.is_none() {
            if add_meta {
                self.write("><meta charset=\"");
                self.escape(encoding, true);
                self.write("\"></");
            } else {
                self.write("></");
            }
            self.write(name);
            self.write(">");
        } else {
            self.write(">");
            let block = info.is_some_and(|info| !info.inline)
                && !is_text(first)
                && node.first_child().map(|child| child.id())
                    != node.last_child().map(|child| child.id())
                && !name.starts_with('p');
            if self.format && (add_meta || block) {
                self.write("\n");
            }
            if add_meta {
                self.write("<meta charset=\"");
                self.escape(encoding, true);
                self.write("\">");
                if self.format && !is_text(first) {
                    self.write("\n");
                }
            }
            if info.is_some_and(|info| info.mode.raw()) {
                *raw = true;
            }
            return true;
        }
        if self.format && info.is_some_and(|info| !info.inline) {
            self.newline_after(node, parent);
        }
        false
    }

    /// A newline between a block element and a following non-text sibling.
    fn newline_after(&mut self, node: OwnedNode<'_>, parent: Option<OwnedNode<'_>>) {
        if let Some(next) = node.next_sibling()
            && !is_text(Some(next))
            && not_p(parent)
        {
            self.write("\n");
        }
    }

    /// The end tag written when leaving an element's children.
    fn end(&mut self, node: OwnedNode<'_>) {
        let info = if self.format { info(node) } else { None };
        let block = info.is_some_and(|info| !info.inline);
        let name = element_name(node).unwrap_or("");
        let several =
            node.first_child().map(|child| child.id()) != node.last_child().map(|child| child.id());
        if block
            && !is_text(node.last_child())
            && (several || self.meta_head == Some(node.id()))
            && !name.starts_with('p')
        {
            self.write("\n");
        }
        self.write("</");
        self.write(name);
        self.write(">");
        if block {
            self.newline_after(node, node.parent());
        }
        if self.meta_head == Some(node.id()) {
            self.meta_head = None;
        }
    }
}
