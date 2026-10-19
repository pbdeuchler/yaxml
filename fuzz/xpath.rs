//! XPath compile and evaluate over both DOMs. A testcase is an expression,
//! optionally followed by a NUL byte and the XML document to query.

use std::collections::HashMap;
use std::hash::Hash;

use yaxml::xpath::{Context, Limits, Node, Value, XPath};
use yaxml::{Document, OwnedDocument};

const DOCUMENT: &str = "<?t d?><r xmlns:p='urn:p' xml:lang='en'><a id='1' xml:id='i'>x<b/>2</a>\
<p:c xmlns='urn:d'><?pi d?><!--c--><![CDATA[y]]><e xmlns=''/></p:c></r>";

const LIMITS: Limits = Limits {
    max_expression_bytes: 4096,
    max_depth: 64,
    max_operations: 200_000,
    max_nodes: 20_000,
};

/// A DOM-independent value: nodes become (preorder rank, kind, index).
#[derive(Debug, PartialEq)]
enum Projection {
    Nodes(Vec<(usize, u8, usize)>),
    Boolean(bool),
    Number(Option<u64>),
    String(String),
}

fn project<Id: Copy + Eq + Hash>(value: Value<Id>, ranks: &HashMap<Id, usize>) -> Projection {
    match value {
        Value::NodeSet(nodes) => {
            let keys: Vec<_> = nodes
                .into_iter()
                .map(|node| match node {
                    Node::Tree(id) => (ranks[&id], 0, 0),
                    Node::Namespace(id, index) => (ranks[&id], 1, index),
                    Node::Attribute(id, index) => (ranks[&id], 2, index),
                })
                .collect();
            assert!(
                keys.windows(2).all(|pair| pair[0] < pair[1]),
                "node-set is not in strict document order: {keys:?}"
            );
            Projection::Nodes(keys)
        }
        Value::Boolean(value) => Projection::Boolean(value),
        Value::Number(value) => Projection::Number((!value.is_nan()).then(|| value.to_bits())),
        Value::String(value) => Projection::String(value),
    }
}

pub fn exercise(input: &[u8]) {
    let (expression, xml) = match input.iter().position(|&byte| byte == 0) {
        Some(at) => (&input[..at], &input[at + 1..]),
        None => (input, DOCUMENT.as_bytes()),
    };
    let expression = String::from_utf8_lossy(expression);
    let Ok(xpath) = XPath::compile_with_limits(&expression, &LIMITS) else {
        return;
    };
    assert_eq!(xpath.as_str(), expression);
    let (Ok(borrowed), Ok(owned)) = (Document::parse_bytes(xml), OwnedDocument::parse_bytes(xml))
    else {
        return;
    };
    let borrowed_ranks: HashMap<_, _> = borrowed
        .nodes()
        .enumerate()
        .map(|(rank, node)| (node.id(), rank))
        .collect();
    let owned_ranks: HashMap<_, _> = owned
        .nodes()
        .enumerate()
        .map(|(rank, node)| (node.id(), rank))
        .collect();
    let borrowed_context = Context::new()
        .with_namespace("p", "urn:p")
        .with_limits(LIMITS);
    let owned_context = Context::new()
        .with_namespace("p", "urn:p")
        .with_limits(LIMITS);
    let starts = [
        (borrowed.root().id(), owned.root().id()),
        (borrowed.root_element().id(), owned.root_element().id()),
    ];
    for (borrowed_start, owned_start) in starts {
        let left = xpath
            .evaluate(&borrowed, borrowed_start, &borrowed_context)
            .map(|value| project(value, &borrowed_ranks));
        let right = xpath
            .evaluate(&owned, owned_start, &owned_context)
            .map(|value| project(value, &owned_ranks));
        assert_eq!(left, right, "DOM disagreement for {expression:?}");
    }
}
