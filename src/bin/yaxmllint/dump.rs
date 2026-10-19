//! `--xpath` output: node dumps like `xmlNodeDumpOutput` and values like
//! xmllint's `doXPathDump`.

use yaxml::xpath::{Node, Value};
use yaxml::{OwnedDocument, OwnedNode, OwnedNodeId, OwnedNodeKind, Writer};

fn escape(text: &str, attribute: bool, ascii: bool, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            '"' if attribute => out.push_str("&quot;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\t' if attribute => out.push_str("&#9;"),
            c if ascii && !c.is_ascii() => out.push_str(&format!("&#x{:X};", c as u32)),
            c => out.push(c),
        }
    }
}

fn attribute(name: &str, value: &str, ascii: bool, out: &mut String) {
    out.push_str(&format!(" {name}=\""));
    escape(value, true, ascii, out);
    out.push('"');
}

fn element(node: OwnedNode<'_>, ascii: bool, out: &mut String) {
    let name = node
        .name()
        .map(|name| name.qualified().to_owned())
        .unwrap_or_default();
    out.push_str(&format!("<{name}"));
    node.attributes()
        .for_each(|a| attribute(a.name.qualified(), &a.value, ascii, out));
    if node.first_child().is_none() {
        return out.push_str("/>");
    }
    out.push('>');
    node.children().for_each(|child| dump(child, ascii, out));
    out.push_str(&format!("</{name}>"));
}

fn dump(node: OwnedNode<'_>, ascii: bool, out: &mut String) {
    let text = node.text().unwrap_or_default();
    match node.kind() {
        OwnedNodeKind::Element => element(node, ascii, out),
        OwnedNodeKind::Text => escape(text, false, ascii, out),
        OwnedNodeKind::Cdata => out.push_str(&format!("<![CDATA[{text}]]>")),
        OwnedNodeKind::Comment => out.push_str(&format!("<!--{text}-->")),
        OwnedNodeKind::ProcessingInstruction => {
            let target = node.processing_instruction_target().unwrap_or_default();
            let space = if text.is_empty() { "" } else { " " };
            out.push_str(&format!("<?{target}{space}{text}?>"));
        }
        _ => {}
    }
}

/// An XPath node as `xmlNodeDumpOutput` writes it; the document node is the
/// whole document.
fn node(document: &OwnedDocument, node: Node<OwnedNodeId>, out: &mut Vec<u8>) {
    let ascii = document.declaration().is_none_or(|d| d.encoding.is_none());
    let mut text = String::new();
    match node {
        Node::Tree(id) if id == document.root().id() => {
            let mut writer = Writer::new(&mut *out);
            let _ = writer.write_owned_document(document);
        }
        Node::Tree(id) => document
            .node(id)
            .into_iter()
            .for_each(|n| dump(n, ascii, &mut text)),
        Node::Attribute(id, index) => {
            let found = document.node(id).and_then(|n| n.attributes().nth(index));
            found
                .iter()
                .for_each(|a| attribute(a.name.qualified(), &a.value, ascii, &mut text));
        }
        Node::Namespace(..) => {}
    }
    out.extend_from_slice(text.as_bytes());
}

/// C's `%g`: six significant digits, trailing zeros removed.
fn percent_g(number: f64) -> String {
    let strip = |text: &str| match text.contains('.') {
        true => text.trim_end_matches('0').trim_end_matches('.').to_owned(),
        false => text.to_owned(),
    };
    if number == 0.0 {
        return if number.is_sign_negative() { "-0" } else { "0" }.into();
    }
    let scientific = format!("{number:.5e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if (-4..6).contains(&exponent) {
        return strip(&format!("{number:.*}", (5 - exponent) as usize));
    }
    format!(
        "{}e{}{:02}",
        strip(mantissa),
        if exponent < 0 { '-' } else { '+' },
        exponent.abs()
    )
}

/// Writes an XPath value as xmllint prints it. Returns false for an empty node-set.
pub fn value(
    document: &OwnedDocument,
    value: &Value<OwnedNodeId>,
    separator: &str,
    out: &mut Vec<u8>,
) -> bool {
    let line = |out: &mut Vec<u8>, text: &str| {
        out.extend_from_slice(format!("{text}{separator}").as_bytes())
    };
    match value {
        Value::NodeSet(nodes) => {
            for found in nodes {
                node(document, *found, out);
                out.extend_from_slice(separator.as_bytes());
            }
            return !nodes.is_empty();
        }
        Value::Boolean(b) => line(out, &b.to_string()),
        Value::Number(n) if n.is_nan() => line(out, "NaN"),
        Value::Number(n) if n.is_infinite() => {
            line(out, if *n > 0.0 { "Infinity" } else { "-Infinity" })
        }
        Value::Number(n) => line(out, &percent_g(*n)),
        Value::String(s) => line(out, s),
    }
    true
}
