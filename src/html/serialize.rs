//! HTML output, ported from libxml2 2.15.4 `HTMLtree.c` (htmlNodeDumpInternal,
//! htmlAttrDumpOutput, htmlSerializeUri, htmlDtdDumpOutput) and the HTML
//! escape tables of `xmlIO.c`. The walk is iterative.

use super::Doctype;
use super::encoding::{Output, content_charset};
use super::tables;
use super::tokenizer::blank;
use crate::{Error, OwnedDocument, OwnedNode, OwnedNodeId, OwnedNodeKind};

/// `htmlDocDumpMemory`: formatted, in the document's encoding (ASCII with
/// HTML 4 entities when it has none), leaving `<meta>` declarations alone.
pub fn to_html(document: &OwnedDocument) -> Result<Vec<u8>, Error> {
    dump(document, true, None)
}

/// `xmlSaveDoc` for an HTML document, which is what `xmllint --html` prints:
/// `<meta>` charset declarations name the output encoding and a top-level
/// head without one gains `<meta charset>`. `format` is XML_SAVE_FORMAT.
pub fn save(document: &OwnedDocument, format: bool) -> Result<Vec<u8>, Error> {
    dump(
        document,
        format,
        Some(document.html_encoding().unwrap_or("HTML")),
    )
}

fn dump(document: &OwnedDocument, format: bool, encoding: Option<&str>) -> Result<Vec<u8>, Error> {
    let output = Output::for_label(document.html_encoding())?;
    let mut dumper = Dumper {
        out: Vec::new(),
        format,
        encoding,
        meta_head: None,
    };
    dumper.document(document);
    Ok(output.encode(dumper.out))
}

struct Dumper<'a> {
    out: Vec<u8>,
    format: bool,
    /// Encoding for `<meta>` declarations; None leaves them unchanged.
    encoding: Option<&'a str>,
    /// The head that received a generated `<meta charset>`.
    meta_head: Option<OwnedNodeId>,
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
        self.out.extend_from_slice(text.as_bytes());
    }

    /// `xmlSerializeText` with the HTML tables: `&`, `<`, `>`, and `"` in attributes.
    fn escape(&mut self, text: &str, attribute: bool) {
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

    fn doctype(&mut self, doctype: &Doctype) {
        self.write("<!DOCTYPE ");
        self.write(doctype.name.as_deref().unwrap_or(""));
        match (&doctype.public_id, &doctype.system_id) {
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

    fn document(&mut self, document: &OwnedDocument) {
        if let Some(doctype) = document.doctype() {
            self.doctype(doctype);
        }
        let Some(mut current) = document.root().first_child() else {
            self.write("\n");
            return;
        };
        let mut raw = false;
        loop {
            let descend = match current.kind() {
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
                if let Some(next) = current.next_sibling() {
                    current = next;
                    break;
                }
                raw = false;
                let Some(parent) = current
                    .parent()
                    .filter(|parent| parent.kind() == OwnedNodeKind::Element)
                else {
                    self.write("\n");
                    return;
                };
                self.end(parent);
                current = parent;
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
