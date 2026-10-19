//! `xmllint --debug` (debugXML.c) rendering of an HTML document, for
//! differential tree comparison. The `URL=` line is omitted.

use std::fmt::Write;

use yaxml::{OwnedDocument, OwnedNode, OwnedNodeKind};

/// `xmlCtxtDumpString`: 40 bytes at most, blanks as spaces, high bytes as `#XX`.
fn string(out: &mut String, text: &str) {
    for byte in text.bytes().take(40) {
        match byte {
            b' ' | b'\t' | b'\n' | b'\r' => out.push(' '),
            0x80.. => write!(out, "#{byte:X}").unwrap(),
            _ => out.push(char::from(byte)),
        }
    }
    if text.len() >= 40 {
        out.push_str("...");
    }
}

fn indent(out: &mut String, depth: usize) {
    out.push_str(&"  ".repeat(depth.min(50)));
}

fn content(out: &mut String, depth: usize, text: &str) {
    indent(out, depth);
    out.push_str("content=");
    string(out, text);
    out.push('\n');
}

fn node(out: &mut String, node: OwnedNode<'_>, depth: usize) {
    indent(out, depth);
    match node.kind() {
        OwnedNodeKind::Element => {
            out.push_str("ELEMENT ");
            string(out, node.name().unwrap().qualified());
            out.push('\n');
            for (index, attribute) in node.attributes().enumerate() {
                indent(out, depth + 1);
                out.push_str("ATTRIBUTE ");
                string(out, attribute.name.qualified());
                out.push('\n');
                if node.attribute_has_value(index) {
                    indent(out, depth + 2);
                    out.push_str("TEXT\n");
                    content(out, depth + 3, &attribute.value);
                }
            }
        }
        OwnedNodeKind::Text => {
            out.push_str("TEXT\n");
            content(out, depth + 1, node.text().unwrap());
        }
        OwnedNodeKind::Cdata => {
            out.push_str("CDATA_SECTION\n");
            content(out, depth + 1, node.text().unwrap());
        }
        OwnedNodeKind::Comment => {
            out.push_str("COMMENT\n");
            content(out, depth + 1, node.text().unwrap());
        }
        kind => panic!("unexpected {kind:?} in an HTML document"),
    }
}

pub fn dump(document: &OwnedDocument) -> String {
    let mut out = String::from("HTML DOCUMENT\n");
    if let Some(encoding) = document.html_encoding() {
        out.push_str("encoding=");
        string(&mut out, encoding);
        out.push('\n');
    }
    out.push_str("standalone=true\n");
    if let Some(doctype) = document.doctype() {
        out.push_str("  DTD");
        if let Some(name) = &doctype.name {
            write!(out, "({name})").unwrap();
        }
        if let Some(public) = &doctype.public_id {
            write!(out, ", PUBLIC {public}").unwrap();
        }
        if let Some(system) = &doctype.system_id {
            write!(out, ", SYSTEM {system}").unwrap();
        }
        out.push('\n');
    }
    let root = document.root();
    for descendant in root.descendants().skip(1) {
        let mut depth = 0;
        let mut parent = descendant.parent();
        while let Some(ancestor) = parent {
            depth += 1;
            parent = ancestor.parent();
        }
        node(&mut out, descendant, depth);
    }
    out
}
