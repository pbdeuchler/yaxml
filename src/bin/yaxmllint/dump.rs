//! `--xpath` output: nodes as `xmlNodeDumpOutput` writes them, values as
//! xmllint's `doXPathDump` prints them.

use yaxml::xpath::{Node, Value};
use yaxml::{OwnedDocument, OwnedNodeId, save};

/// An attribute node dumps as ` name="value"`; kept entity references
/// (`unexpanded`) stay references.
fn attribute(name: &str, value: &str, unexpanded: Option<&str>) -> String {
    let escape = |text: &str| {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\n', "&#10;")
            .replace('\r', "&#13;")
            .replace('\t', "&#9;")
    };
    let Some(form) = unexpanded else {
        return format!(" {name}=\"{}\"", escape(value));
    };
    let mut parts = form.split('&');
    let mut out = format!(" {name}=\"{}", escape(parts.next().unwrap_or_default()));
    for part in parts {
        let (reference, text) = part.split_at(part.find(';').map_or(0, |end| end + 1));
        match reference {
            "#38;" => out.push_str("&amp;"),
            reference => out.push_str(&format!("&{reference}")),
        }
        out.push_str(&escape(text));
    }
    out + "\""
}

/// In-scope namespace declarations at `id` as `(attribute name, URI)`, outermost first.
fn namespaces(document: &OwnedDocument, id: OwnedNodeId) -> Vec<(String, String)> {
    let mut chain: Vec<_> = std::iter::successors(document.node(id), |n| n.parent()).collect();
    chain.reverse();
    let mut bound: Vec<(String, String)> = Vec::new();
    for attribute in chain.iter().flat_map(|n| n.attributes()) {
        let name = attribute.name.qualified();
        if name == "xmlns" || name.starts_with("xmlns:") {
            bound.retain(|(bound, _)| bound != name);
            bound.push((name.to_owned(), attribute.value.to_string()));
        }
    }
    bound
}

fn node(document: &OwnedDocument, node: Node<OwnedNodeId>) -> Vec<u8> {
    let utf8 = save::Options {
        encoding: Some("UTF-8"),
        ..save::Options::new(0)
    };
    match node {
        Node::Tree(id) => document
            .node(id)
            .and_then(|n| save::to_vec(n, &utf8).ok())
            .unwrap_or_default(),
        Node::Attribute(id, index) => document
            .node(id)
            .and_then(|n| {
                let a = n.attributes().nth(index)?;
                let unexpanded = n.unexpanded_attribute(index);
                Some(attribute(a.name.qualified(), &a.value, unexpanded).into_bytes())
            })
            .unwrap_or_default(),
        // Slot 0 is the implicit `xml` binding, which prints as nothing.
        Node::Namespace(id, index) => index
            .checked_sub(1)
            .and_then(|slot| namespaces(document, id).into_iter().nth(slot))
            .map(|(name, uri)| attribute(&name, &uri, None).into_bytes())
            .unwrap_or_default(),
    }
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
    let sign = if exponent < 0 { '-' } else { '+' };
    format!("{}e{sign}{:02}", strip(mantissa), exponent.abs())
}

/// Appends an XPath value as xmllint prints it. False for an empty node-set.
pub fn value(
    document: &OwnedDocument,
    value: &Value<OwnedNodeId>,
    separator: &str,
    out: &mut Vec<u8>,
) -> bool {
    let text = match value {
        Value::NodeSet(nodes) => {
            for found in nodes {
                out.extend(node(document, *found));
                out.extend_from_slice(separator.as_bytes());
            }
            return !nodes.is_empty();
        }
        Value::Boolean(b) => b.to_string(),
        Value::Number(n) if n.is_nan() => "NaN".into(),
        Value::Number(n) if n.is_infinite() => {
            (if *n > 0.0 { "Infinity" } else { "-Infinity" }).into()
        }
        Value::Number(n) => percent_g(*n),
        Value::String(s) => s.clone(),
    };
    out.extend_from_slice(format!("{text}{separator}").as_bytes());
    true
}
