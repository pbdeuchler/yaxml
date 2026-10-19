//! XPath compile and evaluate over both DOMs. A testcase is an expression,
//! optionally followed by a NUL byte and the XML document to query.
//!
//! Both contexts bind the same variables (`$s`, `$n`, `$b`, `$ns`, `$p:v`,
//! `$bad`) and extension functions (`f`, `p:g`), some of which return nodes
//! the evaluator must reject. Expressions start at the root, the document
//! element and its first attribute and namespace node. A second evaluation
//! under small limits derived from the expression must agree or fail on a
//! limit.

use std::collections::HashMap;
use std::hash::Hash;

use yaxml::xpath::{Context, Error, ErrorKind, Function, Limits, Node, Value, XPath};
use yaxml::{Document, OwnedDocument};

const DOCUMENT: &str = "<?t d?><r xmlns:p='urn:p' xml:lang='en'><a id='1' xml:id='i'>x<b/>2</a>\
<p:c xmlns='urn:d'><?pi d?><!--c--><![CDATA[y]]><e xmlns=''/></p:c></r>";

/// A larger document whose last node is foreign to every testcase tree.
const FOREIGN: &str = "<r><a/><a/><a/><a/><a/><a/><a/><a/><a/><a/><a/><a/><a/><a/><a/><a/>\
<a/><a/><a/><a/><a/><a/><a/><a/><a/><a/><a/><a/><a/><a/><a/><a/></r>";

const LIMITS: Limits = Limits {
    max_expression_bytes: 4096,
    max_depth: 64,
    max_operations: 200_000,
    max_nodes: 20_000,
};

/// Small ceilings chosen by the expression bytes.
fn tight(expression: &str) -> Limits {
    let hash = expression.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte.into())
    });
    Limits {
        max_expression_bytes: expression
            .len()
            .saturating_sub((hash & 1) as usize)
            .min(LIMITS.max_expression_bytes),
        max_depth: 1 + (hash >> 1 & 7) as usize,
        max_operations: u64::from(hash >> 4 & 63),
        max_nodes: (hash >> 10 & 7) as usize,
    }
}

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

/// Extension function `f`: arity errors, scalar pass-through and the
/// arguments' node-sets concatenated in reverse, which must be re-sorted.
fn concatenated<Id: Copy + Send + Sync + 'static>() -> Function<Id> {
    Box::new(|args: &[Value<Id>]| match args {
        [] => Err(Error::new(ErrorKind::InvalidArity)),
        [first, ..] if !matches!(first, Value::NodeSet(_)) => Ok(first.clone()),
        _ => {
            let mut nodes = Vec::new();
            for arg in args.iter().rev() {
                if let Value::NodeSet(set) = arg {
                    nodes.extend(set.iter().rev().copied());
                }
            }
            Ok(Value::NodeSet(nodes))
        }
    })
}

/// Extension function `p:g`: by arity, nodes that are foreign, missing
/// attributes or namespaces, or valid but unsorted.
fn crafted<Id: Copy + Send + Sync + 'static>(root: Id, element: Id, foreign: Id) -> Function<Id> {
    Box::new(move |args: &[Value<Id>]| {
        Ok(Value::NodeSet(match args.len() {
            0 => vec![Node::Tree(foreign)],
            1 => vec![Node::Attribute(element, usize::MAX)],
            2 => vec![Node::Namespace(element, usize::MAX)],
            _ => vec![
                Node::Tree(element),
                Node::Namespace(element, 0),
                Node::Tree(root),
                Node::Tree(element),
            ],
        }))
    })
}

fn context<Id: Copy + Send + Sync + 'static>(
    root: Id,
    element: Id,
    foreign: Id,
    limits: Limits,
) -> Context<Id> {
    Context::new()
        .with_namespace("p", "urn:p")
        .with_limits(limits)
        .with_variable("s", Value::String(" 1.5 ".to_owned()))
        .with_variable("n", Value::Number(2.0))
        .with_variable("b", Value::Boolean(true))
        .with_variable(
            "ns",
            Value::NodeSet(vec![Node::Tree(element), Node::Tree(root)]),
        )
        .with_variable("bad", Value::NodeSet(vec![Node::Attribute(element, 1000)]))
        .with_variable_ns(
            "urn:p",
            "v",
            Value::NodeSet(vec![Node::Namespace(element, 0)]),
        )
        .with_function("f", concatenated())
        .with_function_ns("urn:p", "g", crafted(root, element, foreign))
}

fn starts<Id: Copy>(root: Id, element: Id) -> [Node<Id>; 4] {
    [
        Node::Tree(root),
        Node::Tree(element),
        Node::Attribute(element, 0),
        Node::Namespace(element, 0),
    ]
}

/// The tight result equals the generous one unless it ran out of budget.
fn within<T: PartialEq + std::fmt::Debug>(
    generous: &Result<T, Error>,
    tight: Result<T, Error>,
    what: &str,
) {
    match tight {
        Err(Error {
            kind: ErrorKind::LimitExceeded(_),
            ..
        }) => {}
        tight => assert_eq!(&tight, generous, "small limits changed {what}"),
    }
}

pub fn exercise(input: &[u8]) {
    let (expression, xml) = match input.iter().position(|&byte| byte == 0) {
        Some(at) => (&input[..at], &input[at + 1..]),
        None => (input, DOCUMENT.as_bytes()),
    };
    let expression = String::from_utf8_lossy(expression);
    let compiled = XPath::compile_with_limits(&expression, &LIMITS);
    // The default limits are larger than the harness limits.
    assert!(compiled.is_err() || XPath::compile(&expression).is_ok());
    let small = tight(&expression);
    let tight_compiled = XPath::compile_with_limits(&expression, &small);
    within(
        &compiled.as_ref().map(|_| ()).map_err(|e| *e),
        tight_compiled.as_ref().map(|_| ()).map_err(|e| *e),
        "compilation",
    );
    let xpath = match compiled {
        Ok(xpath) => xpath,
        Err(error) => {
            assert!(!error.to_string().is_empty());
            assert!(
                error
                    .offset
                    .is_some_and(|offset| offset <= expression.len())
            );
            return;
        }
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
    let foreign = Document::parse(FOREIGN).unwrap();
    let foreign_id = foreign.nodes().last().unwrap().id();
    let foreign_owned = OwnedDocument::parse(FOREIGN).unwrap();
    let foreign_owned_id = foreign_owned.root().id();
    let (root, element) = (borrowed.root().id(), borrowed.root_element().id());
    let (owned_root, owned_element) = (owned.root().id(), owned.root_element().id());
    let borrowed_context = context(root, element, foreign_id, LIMITS);
    let owned_context = context(owned_root, owned_element, foreign_owned_id, LIMITS);
    assert!(format!("{borrowed_context:?}").starts_with("Context"));
    let tight_context = context(root, element, foreign_id, small);
    let starts = starts(root, element)
        .into_iter()
        .zip(starts(owned_root, owned_element));
    for (index, (borrowed_start, owned_start)) in starts.enumerate() {
        let value = xpath.evaluate(&borrowed, borrowed_start, &borrowed_context);
        if let Err(error) = &value {
            assert!(error.offset.is_none() && !error.to_string().is_empty());
        }
        let selected = xpath.select_nodes(&borrowed, borrowed_start, &borrowed_context);
        match (&value, &selected) {
            (Ok(Value::NodeSet(nodes)), Ok(selected)) => assert_eq!(nodes, selected),
            (Ok(_), Err(error)) => assert_eq!(error.kind, ErrorKind::InvalidType),
            (Err(error), Err(other)) => assert_eq!(error, other),
            _ => panic!("select_nodes disagrees with evaluate for {expression:?}"),
        }
        if index == 0 {
            // A bare id converts into its tree node.
            let by_id = xpath.select_nodes(&borrowed, root, &borrowed_context);
            assert_eq!(by_id, selected);
        }
        let left = value.map(|value| project(value, &borrowed_ranks));
        if index < 2
            && let Ok(tight) = &tight_compiled
        {
            let small = tight
                .evaluate(&borrowed, borrowed_start, &tight_context)
                .map(|value| project(value, &borrowed_ranks));
            within(&left, small, "evaluation");
        }
        let right = xpath
            .evaluate(&owned, owned_start, &owned_context)
            .map(|value| project(value, &owned_ranks));
        assert_eq!(left, right, "DOM disagreement for {expression:?}");
    }
}
