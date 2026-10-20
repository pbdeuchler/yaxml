//! Tree-walking evaluator over any [`Navigate`] tree.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::iter::{once, successors};

use super::parse::{
    Ast, Axis, BinOp, Core, Expr, Function, QName, Span, Start, Step, Test, scan_number,
};
use super::tree::{self, Item, Navigate};
use super::{Context, Error, ErrorKind, Limit, Node, Value};
use crate::fallible::{TryPush, collected, copied};

type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy)]
struct Focus<Id> {
    node: Node<Id>,
    position: usize,
    size: usize,
}

/// A node test with its prefix bound; `""` is the null namespace URI.
#[derive(Clone, Copy)]
enum Matcher<'a> {
    Node,
    Text,
    Comment,
    Pi(Option<&'a str>),
    Any(Option<&'a str>),
    Name(&'a str, &'a str),
}

struct Eval<'a, T: Navigate> {
    tree: &'a T,
    source: &'a str,
    ast: &'a Ast,
    context: &'a Context<T::Id>,
    operations: u64,
    max_operations: u64,
    ranks: OnceCell<Option<Vec<usize>>>,
}

/// Evaluates within the context's limits and at most `budget` operations,
/// deducting the operations spent from `budget`.
pub(crate) fn evaluate<T: Navigate>(
    tree: &T,
    source: &str,
    ast: &Ast,
    node: Node<T::Id>,
    context: &Context<T::Id>,
    budget: &mut u64,
) -> Result<Value<T::Id>> {
    let mut eval = Eval {
        tree,
        source,
        ast,
        context,
        operations: 0,
        max_operations: context.limits.max_operations.min(*budget),
        ranks: OnceCell::new(),
    };
    let focus = Focus {
        node,
        position: 1,
        size: 1,
    };
    let value = (eval.checked(Value::NodeSet(vec![node]))).and_then(|_| eval.expr(ast.root, focus));
    *budget -= eval.operations.min(*budget);
    value
}

fn is_blank(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

pub(crate) fn truthy<Id>(value: &Value<Id>) -> bool {
    match value {
        Value::NodeSet(nodes) => !nodes.is_empty(),
        Value::Boolean(value) => *value,
        Value::Number(value) => *value != 0.0 && !value.is_nan(),
        Value::String(value) => !value.is_empty(),
    }
}

/// Number conversion for non-node-set values.
fn scalar_number<Id>(value: &Value<Id>) -> f64 {
    match value {
        Value::Boolean(value) => f64::from(u8::from(*value)),
        Value::Number(value) => *value,
        Value::String(value) => string_to_number(value),
        Value::NodeSet(_) => f64::NAN,
    }
}

/// XPath 1.0 section 3.4 for two non-node-set operands.
fn scalar<Id>(op: BinOp, left: &Value<Id>, right: &Value<Id>) -> bool {
    if matches!(op, BinOp::Eq | BinOp::Ne) {
        let equal = match (left, right) {
            (Value::Boolean(_), _) | (_, Value::Boolean(_)) => truthy(left) == truthy(right),
            (Value::Number(_), _) | (_, Value::Number(_)) => {
                scalar_number(left) == scalar_number(right)
            }
            _ => matches!((left, right), (Value::String(a), Value::String(b)) if a == b),
        };
        return equal == (op == BinOp::Eq);
    }
    let (left, right) = (scalar_number(left), scalar_number(right));
    match op {
        BinOp::Lt => left < right,
        BinOp::Le => left <= right,
        BinOp::Gt => left > right,
        _ => left >= right,
    }
}

fn flipped(op: BinOp) -> BinOp {
    match op {
        BinOp::Lt => BinOp::Gt,
        BinOp::Le => BinOp::Ge,
        BinOp::Gt => BinOp::Lt,
        BinOp::Ge => BinOp::Le,
        other => other,
    }
}

/// libxml2 `xmlXPathStringEvalNumber`: optional blanks and `-`, a number
/// with an optional exponent, optional blanks; anything else is NaN.
pub(crate) fn string_to_number(text: &str) -> f64 {
    let trimmed = text.trim_matches(is_blank).as_bytes();
    let (negative, digits) = match trimmed.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, trimmed),
    };
    match scan_number(digits) {
        Some((value, length)) if length == digits.len() => {
            if negative {
                -value
            } else {
                value
            }
        }
        _ => f64::NAN,
    }
}

/// libxml2 `xmlXPathFormatNumber`: integers in `int` range print exactly;
/// other magnitudes in [1e-5, 1e9] use up to 15 significant digits, and the
/// rest use `%.14e` notation, all without trailing fraction zeros.
pub(crate) fn number_to_string(value: f64) -> String {
    let trim = |text: &str| -> String {
        let text = text.trim_end_matches('0');
        text.strip_suffix('.').unwrap_or(text).to_owned()
    };
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    if value == 0.0 {
        return "0".to_owned();
    }
    if value > f64::from(i32::MIN) && value < f64::from(i32::MAX) && value == value.trunc() {
        return (value as i32).to_string();
    }
    let magnitude = value.abs();
    if !(1e-5..=1e9).contains(&magnitude) {
        let text = format!("{value:.14e}");
        let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
        let exponent: i32 = exponent.parse().unwrap_or(0);
        let sign = if exponent < 0 { '-' } else { '+' };
        return format!("{}e{sign}{:02}", trim(mantissa), exponent.unsigned_abs());
    }
    let integer_place = magnitude.log10() as i32;
    let fraction = if integer_place > 0 {
        14 - integer_place
    } else {
        15 - integer_place
    };
    trim(&format!("{value:.*}", fraction as usize))
}

/// libxml2 rounding: halves round up, and (-0.5, 0) rounds to -0.
fn round(value: f64) -> f64 {
    if (-0.5..0.5).contains(&value) {
        return value * 0.0;
    }
    let floor = value.floor();
    if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// libxml2 `xmlXPathSubstringFunction` over 1-based character positions.
fn substring(text: &str, start: f64, length: Option<f64>) -> &str {
    let max = f64::from(i32::MAX);
    let first = if start.is_nan() || start >= max {
        max
    } else if start >= 1.0 {
        round(start)
    } else {
        1.0
    };
    let end = length.map_or(max, |length| {
        let end = round(start) + round(length);
        if end.is_nan() || end < 1.0 {
            1.0
        } else {
            end.min(max)
        }
    });
    let (first, end) = (first as usize, end as usize);
    // The byte offset of 0-based character `chars`.
    let at = |chars: usize| {
        text.char_indices()
            .nth(chars)
            .map_or(text.len(), |(at, _)| at)
    };
    match end > first {
        true => &text[at(first - 1)..at(end - 1)],
        false => "",
    }
}

fn translate(text: &str, from: &str, to: &str) -> Result<String> {
    let to = collected(to.chars())?;
    let mut map = HashMap::new();
    for (index, c) in from.chars().enumerate() {
        map.try_reserve(1)?;
        map.entry(c).or_insert(index);
    }
    let translated = || {
        text.chars().filter_map(|c| match map.get(&c) {
            Some(&index) => to.get(index).copied(),
            None => Some(c),
        })
    };
    let mut out = String::new();
    out.try_reserve_exact(translated().map(char::len_utf8).sum())?;
    out.extend(translated());
    Ok(out)
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(is_blank).filter(|word| !word.is_empty())
}

fn error<T>(kind: ErrorKind) -> Result<T> {
    Err(Error::new(kind))
}

impl<'a, T: Navigate> Eval<'a, T> {
    fn tick(&mut self, count: u64) -> Result<()> {
        self.operations = self.operations.saturating_add(count);
        if self.operations > self.max_operations {
            return error(ErrorKind::LimitExceeded(Limit::Operations));
        }
        Ok(())
    }

    /// Room for `additional` more nodes in `nodes`, within the node ceiling.
    fn reserve(&self, nodes: &mut Vec<Node<T::Id>>, additional: usize) -> Result<()> {
        if nodes.len() + additional > self.context.limits.max_nodes {
            return error(ErrorKind::LimitExceeded(Limit::Nodes));
        }
        Ok(nodes.try_reserve(additional)?)
    }

    fn namespace(&self, prefix: Span) -> Result<&'a str> {
        self.context
            .namespace(prefix.text(self.source))
            .ok_or(Error::at(ErrorKind::UndefinedPrefix, prefix.range().start))
    }

    fn expand(&self, name: QName) -> Result<(&'a str, &'a str)> {
        let uri = name
            .prefix
            .map_or(Ok(""), |prefix| self.namespace(prefix))?;
        Ok((uri, name.local.text(self.source)))
    }

    /// Requires [`Self::ranked`].
    fn rank(&self, id: T::Id) -> usize {
        let slot = self.tree.slot(id);
        match self.ranks.get() {
            Some(Some(ranks)) => ranks.get(slot).copied().unwrap_or(usize::MAX),
            _ => slot,
        }
    }

    /// Computes the document-order ranks [`Self::rank`] reads, once.
    fn ranked(&self) -> Result<()> {
        if self.ranks.get().is_none() {
            let _ = self.ranks.set(self.tree.ranks()?);
        }
        Ok(())
    }

    /// Document order: a node, then its namespace nodes, then its attributes.
    fn key(&self, node: Node<T::Id>) -> (usize, u8, usize) {
        match node {
            Node::Tree(id) => (self.rank(id), 0, 0),
            Node::Namespace(id, index) => (self.rank(id), 1, index),
            Node::Attribute(id, index) => (self.rank(id), 2, index),
        }
    }

    /// Document order without duplicates, sorted in place: equal keys are
    /// the same node, so an unstable sort is exact.
    fn sort(&self, mut nodes: Vec<Node<T::Id>>) -> Result<Vec<Node<T::Id>>> {
        self.ranked()?;
        if !nodes
            .windows(2)
            .all(|pair| self.key(pair[0]) < self.key(pair[1]))
        {
            nodes.sort_unstable_by_key(|&node| self.key(node));
            nodes.dedup();
        }
        Ok(nodes)
    }

    /// Validates caller-supplied node-sets and restores document order.
    fn checked(&mut self, value: Value<T::Id>) -> Result<Value<T::Id>> {
        let Value::NodeSet(mut nodes) = value else {
            return Ok(value);
        };
        self.reserve(&mut nodes, 0)?;
        let tree = self.tree;
        for &node in &nodes {
            let valid = match node {
                Node::Tree(id) => tree.contains(id),
                Node::Attribute(id, index) => tree
                    .attribute(id, index)
                    .is_some_and(|(name, _)| !name.is_namespace_declaration()),
                Node::Namespace(id, index) => index < self.namespaces(id)?.len(),
            };
            if !valid {
                return error(ErrorKind::InvalidContext);
            }
        }
        Ok(Value::NodeSet(self.sort(nodes)?))
    }

    /// [`tree::namespaces`], charging one operation per ancestor.
    fn namespaces(&mut self, element: T::Id) -> Result<Vec<(&'a str, &'a str)>> {
        let tree = self.tree;
        if matches!(tree.item(element), Item::Element(_)) {
            self.tick(successors(Some(element), |&id| tree.parent(id)).count() as u64)?;
        }
        Ok(tree::namespaces(tree, element))
    }

    fn string_value(&mut self, node: Node<T::Id>) -> Result<String> {
        let tree = self.tree;
        let id = match node {
            Node::Attribute(id, index) => {
                let value = tree.attribute(id, index).map_or("", |(_, value)| value);
                return Ok(copied(value)?);
            }
            Node::Namespace(id, index) => {
                let uri = self.namespaces(id)?.get(index).map_or("", |&(_, uri)| uri);
                return Ok(copied(uri)?);
            }
            Node::Tree(id) => id,
        };
        if let Item::Text(text) | Item::Comment(text) | Item::Pi(_, text) = tree.item(id) {
            return Ok(copied(text)?);
        }
        let mut value = String::new();
        for id in tree::descendants(tree, id) {
            self.tick(1)?;
            if let Item::Text(text) = tree.item(id) {
                self.tick(text.len() as u64 / 64)?;
                value.try_push(text)?;
            }
        }
        Ok(value)
    }

    fn cast_string(&mut self, value: Value<T::Id>) -> Result<String> {
        Ok(match value {
            Value::NodeSet(nodes) => match nodes.first() {
                Some(&node) => self.string_value(node)?,
                None => String::new(),
            },
            Value::Boolean(value) => value.to_string(),
            Value::Number(value) => number_to_string(value),
            Value::String(value) => value,
        })
    }

    fn cast_number(&mut self, value: Value<T::Id>) -> Result<f64> {
        Ok(match value {
            Value::NodeSet(_) => string_to_number(&self.cast_string(value)?),
            scalar => scalar_number(&scalar),
        })
    }

    fn nodes(&mut self, expr: u32, focus: Focus<T::Id>) -> Result<Vec<Node<T::Id>>> {
        match self.expr(expr, focus)? {
            Value::NodeSet(nodes) => Ok(nodes),
            _ => error(ErrorKind::InvalidType),
        }
    }

    fn number(&mut self, expr: u32, focus: Focus<T::Id>) -> Result<f64> {
        let value = self.expr(expr, focus)?;
        self.cast_number(value)
    }

    fn string(&mut self, expr: u32, focus: Focus<T::Id>) -> Result<String> {
        let value = self.expr(expr, focus)?;
        self.cast_string(value)
    }

    fn boolean(&mut self, expr: u32, focus: Focus<T::Id>) -> Result<bool> {
        Ok(truthy(&self.expr(expr, focus)?))
    }

    fn expr(&mut self, id: u32, focus: Focus<T::Id>) -> Result<Value<T::Id>> {
        self.tick(1)?;
        let ast = self.ast;
        Ok(match ast.exprs[id as usize] {
            Expr::Number(value) => Value::Number(value),
            Expr::Literal(text) => Value::String(copied(text.text(self.source))?),
            Expr::Variable(name) => {
                let (uri, local) = self.expand(name)?;
                let context = self.context;
                let value = context
                    .variables
                    .get(uri)
                    .and_then(|scope| scope.get(local));
                self.checked(
                    value
                        .ok_or(Error::at(ErrorKind::UndefinedVariable, name.start()))?
                        .try_clone()?,
                )?
            }
            Expr::Call(function, args) => self.call(function, &ast.lists[args.range()], focus)?,
            Expr::Binary(op, left, right) => self.binary(op, left, right, focus)?,
            Expr::Negate(operand) => Value::Number(-self.number(operand, focus)?),
            Expr::Path(start, steps) => {
                let mut nodes = match start {
                    Start::Root => vec![Node::Tree(self.tree.root())],
                    Start::Context => vec![focus.node],
                    Start::Expr(base) => self.nodes(base, focus)?,
                };
                for step in &ast.steps[steps.range()] {
                    nodes = self.step(&nodes, step)?;
                }
                Value::NodeSet(nodes)
            }
            Expr::Filter(base, predicates) => {
                let mut nodes = self.nodes(base, focus)?;
                self.filter(&mut nodes, predicates)?;
                Value::NodeSet(nodes)
            }
        })
    }

    fn binary(
        &mut self,
        op: BinOp,
        left: u32,
        right: u32,
        focus: Focus<T::Id>,
    ) -> Result<Value<T::Id>> {
        Ok(match op {
            BinOp::Or => Value::Boolean(self.boolean(left, focus)? || self.boolean(right, focus)?),
            BinOp::And => Value::Boolean(self.boolean(left, focus)? && self.boolean(right, focus)?),
            BinOp::Union => {
                let left = self.nodes(left, focus)?;
                let right = self.nodes(right, focus)?;
                let (left, right) = (self.sort(left)?, self.sort(right)?);
                // Merged in document order: a sort of the concatenation would
                // not be linear without a buffer.
                let mut nodes = Vec::new();
                self.reserve(&mut nodes, left.len() + right.len())?;
                let (mut left, mut right) =
                    (left.into_iter().peekable(), right.into_iter().peekable());
                while let (Some(&a), Some(&b)) = (left.peek(), right.peek()) {
                    let order = self.key(a).cmp(&self.key(b));
                    nodes.push(if order.is_le() { a } else { b });
                    if order.is_le() {
                        left.next();
                    }
                    if order.is_ge() {
                        right.next();
                    }
                }
                nodes.extend(left.chain(right));
                Value::NodeSet(nodes)
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                let (left, right) = (self.number(left, focus)?, self.number(right, focus)?);
                Value::Number(match op {
                    BinOp::Add => left + right,
                    BinOp::Sub => left - right,
                    BinOp::Mul => left * right,
                    BinOp::Div => left / right,
                    _ => left % right,
                })
            }
            _ => {
                let (left, right) = (self.expr(left, focus)?, self.expr(right, focus)?);
                Value::Boolean(self.compare(op, left, right)?)
            }
        })
    }

    fn strings(&mut self, nodes: &[Node<T::Id>]) -> Result<Vec<String>> {
        let mut strings = Vec::new();
        strings.try_reserve_exact(nodes.len())?;
        for &node in nodes {
            strings.push(self.string_value(node)?);
        }
        Ok(strings)
    }

    fn compare(&mut self, op: BinOp, left: Value<T::Id>, right: Value<T::Id>) -> Result<bool> {
        let (left, right) = match (left, right) {
            (Value::NodeSet(left), Value::NodeSet(right)) => {
                (self.strings(&left)?, self.strings(&right)?)
            }
            (Value::NodeSet(nodes), other) => return self.compare_set(op, &nodes, &other),
            (other, Value::NodeSet(nodes)) => return self.compare_set(flipped(op), &nodes, &other),
            (left, right) => return Ok(scalar(op, &left, &right)),
        };
        Ok(match op {
            BinOp::Eq => {
                let mut set = HashSet::new();
                set.try_reserve(left.len())?;
                set.extend(left.iter().map(String::as_str));
                right.iter().any(|value| set.contains(value.as_str()))
            }
            BinOp::Ne => left.first().is_some_and(|first| {
                !right.is_empty() && left.iter().chain(&right).any(|value| value != first)
            }),
            _ => {
                // Some pair satisfies the relation iff the extremes do.
                let bounds = |values: &[String]| {
                    values
                        .iter()
                        .map(|value| string_to_number(value))
                        .filter(|value| !value.is_nan())
                        .fold(None, |bounds: Option<(f64, f64)>, value| {
                            Some(bounds.map_or((value, value), |(low, high)| {
                                (low.min(value), high.max(value))
                            }))
                        })
                };
                let (Some((low, high)), Some((other_low, other_high))) =
                    (bounds(&left), bounds(&right))
                else {
                    return Ok(false);
                };
                match op {
                    BinOp::Lt => low < other_high,
                    BinOp::Le => low <= other_high,
                    BinOp::Gt => high > other_low,
                    _ => high >= other_low,
                }
            }
        })
    }

    fn compare_set(
        &mut self,
        op: BinOp,
        nodes: &[Node<T::Id>],
        other: &Value<T::Id>,
    ) -> Result<bool> {
        if let Value::Boolean(_) = other {
            return Ok(scalar(op, &Value::Boolean(!nodes.is_empty()), other));
        }
        for &node in nodes {
            if scalar(op, &Value::String(self.string_value(node)?), other) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn matcher(&self, test: Test) -> Result<Matcher<'a>> {
        Ok(match test {
            Test::Node => Matcher::Node,
            Test::Text => Matcher::Text,
            Test::Comment => Matcher::Comment,
            Test::Pi(target) => Matcher::Pi(target.map(|target| target.text(self.source))),
            Test::Any(prefix) => {
                Matcher::Any(prefix.map(|prefix| self.namespace(prefix)).transpose()?)
            }
            Test::Name(name) => {
                let (uri, local) = self.expand(name)?;
                Matcher::Name(uri, local)
            }
        })
    }

    /// Node tests for non-namespace nodes; the namespace axis tests its own nodes.
    fn matches(&self, node: Node<T::Id>, axis: Axis, matcher: Matcher<'_>) -> bool {
        let tree = self.tree;
        let item = match node {
            Node::Tree(id) => Some(tree.item(id)),
            _ => None,
        };
        let name = match (node, item) {
            (Node::Attribute(id, index), _) if axis == Axis::Attribute => {
                tree.attribute(id, index).map(|(name, _)| name)
            }
            (_, Some(Item::Element(name))) if axis != Axis::Attribute => Some(name),
            _ => None,
        };
        let namespace = |name: tree::QName<'a>| name.namespace.unwrap_or("");
        match matcher {
            // libxml2's `node()` never selects an entity reference.
            Matcher::Node => !matches!(item, Some(Item::EntityRef(_))),
            Matcher::Text => matches!(item, Some(Item::Text(_))),
            Matcher::Comment => matches!(item, Some(Item::Comment(_))),
            Matcher::Pi(target) => {
                matches!(item, Some(Item::Pi(name, _)) if target.is_none_or(|target| target == name))
            }
            Matcher::Any(uri) => {
                name.is_some_and(|name| uri.is_none_or(|uri| uri == namespace(name)))
            }
            Matcher::Name(uri, local) => {
                name.is_some_and(|name| name.local == local && namespace(name) == uri)
            }
        }
    }

    fn collect(
        &mut self,
        nodes: impl Iterator<Item = Node<T::Id>>,
        axis: Axis,
        matcher: Matcher<'_>,
        out: &mut Vec<Node<T::Id>>,
    ) -> Result<()> {
        for node in nodes {
            self.tick(1)?;
            if self.matches(node, axis, matcher) {
                self.reserve(out, 1)?;
                out.push(node);
            }
        }
        Ok(())
    }

    /// Appends the axis's matching nodes in axis order: reverse axes yield
    /// reverse document order, which predicate positions count in.
    fn axis(
        &mut self,
        node: Node<T::Id>,
        axis: Axis,
        matcher: Matcher<'_>,
        out: &mut Vec<Node<T::Id>>,
    ) -> Result<()> {
        let tree = self.tree;
        let (id, owner) = match node {
            Node::Tree(id) => (Some(id), id),
            Node::Attribute(owner, _) | Node::Namespace(owner, _) => (None, owner),
        };
        let parent = id.map_or(Some(owner), |id| tree.parent(id));
        let ancestors = successors(parent, |&id| tree.parent(id)).map(Node::Tree);
        let start = out.len();
        match axis {
            Axis::Self_ => self.collect(once(node), axis, matcher, out),
            Axis::Child => {
                let children = successors(id.and_then(|id| tree.first_child(id)), |&id| {
                    tree.next_sibling(id)
                });
                self.collect(children.map(Node::Tree), axis, matcher, out)
            }
            Axis::Descendant => {
                let descendants = id
                    .into_iter()
                    .flat_map(|id| tree::visible_descendants(tree, id));
                self.collect(descendants.map(Node::Tree), axis, matcher, out)
            }
            Axis::DescendantOrSelf => {
                let descendants = id
                    .into_iter()
                    .flat_map(|id| tree::visible_descendants(tree, id));
                self.collect(
                    once(node).chain(descendants.map(Node::Tree)),
                    axis,
                    matcher,
                    out,
                )
            }
            Axis::Parent => self.collect(parent.map(Node::Tree).into_iter(), axis, matcher, out),
            Axis::Ancestor => self.collect(ancestors, axis, matcher, out),
            Axis::AncestorOrSelf => self.collect(once(node).chain(ancestors), axis, matcher, out),
            Axis::FollowingSibling => {
                let siblings = successors(id.and_then(|id| tree.next_sibling(id)), |&id| {
                    tree.next_sibling(id)
                });
                self.collect(siblings.map(Node::Tree), axis, matcher, out)
            }
            Axis::PrecedingSibling => {
                let first = id.and(parent).and_then(|parent| tree.first_child(parent));
                let siblings = successors(first, |&id| tree.next_sibling(id))
                    .take_while(|&sibling| Some(sibling) != id);
                self.collect(siblings.map(Node::Tree), axis, matcher, out)?;
                out[start..].reverse();
                Ok(())
            }
            Axis::Following => {
                let first = match id {
                    Some(id) => tree::skip_subtree(tree, id, None),
                    None => tree
                        .first_child(owner)
                        .or_else(|| tree::skip_subtree(tree, owner, None)),
                };
                self.collect(
                    tree::visible_preorder(tree, first, None).map(Node::Tree),
                    axis,
                    matcher,
                    out,
                )
            }
            Axis::Preceding => {
                // Nodes before `owner`, except its ancestors; an attribute's owner is its parent.
                let mut ancestors: Vec<T::Id> =
                    successors(tree.parent(owner), |&id| tree.parent(id)).collect();
                let before = tree::visible_preorder(tree, Some(tree.root()), None)
                    .take_while(|&other| other != owner)
                    .filter(|other| {
                        let ancestor = ancestors.last() == Some(other);
                        if ancestor {
                            ancestors.pop();
                        }
                        !ancestor
                    });
                self.collect(before.map(Node::Tree), axis, matcher, out)?;
                out[start..].reverse();
                Ok(())
            }
            Axis::Attribute => {
                let attributes = id
                    .into_iter()
                    .flat_map(|id| {
                        tree::attributes(tree, id).map(move |(index, name, _)| (id, index, name))
                    })
                    .filter(|(_, _, name)| !name.is_namespace_declaration())
                    .map(|(id, index, _)| Node::Attribute(id, index));
                self.collect(attributes, axis, matcher, out)
            }
            Axis::Namespace => {
                let Some(id) = id else {
                    return Ok(());
                };
                let namespaces = self.namespaces(id)?;
                let hits = namespaces
                    .iter()
                    .enumerate()
                    .filter(|(_, (prefix, _))| match matcher {
                        Matcher::Node | Matcher::Any(None) => true,
                        Matcher::Name("", local) => *prefix == local,
                        _ => false,
                    })
                    .map(|(index, _)| Node::Namespace(id, index));
                self.tick(namespaces.len() as u64)?;
                self.reserve(out, namespaces.len())?;
                out.extend(hits);
                Ok(())
            }
        }
    }

    fn step(&mut self, input: &[Node<T::Id>], step: &Step) -> Result<Vec<Node<T::Id>>> {
        let matcher = self.matcher(step.test)?;
        let mut output = Vec::new();
        let mut scratch = Vec::new();
        for &node in input {
            if step.predicates.is_empty() {
                self.axis(node, step.axis, matcher, &mut output)?;
                continue;
            }
            scratch.clear();
            self.axis(node, step.axis, matcher, &mut scratch)?;
            self.filter(&mut scratch, step.predicates)?;
            self.reserve(&mut output, scratch.len())?;
            output.extend_from_slice(&scratch);
        }
        self.sort(output)
    }

    /// Applies predicates in turn; positions follow the order of `nodes`.
    fn filter(&mut self, nodes: &mut Vec<Node<T::Id>>, predicates: Span) -> Result<()> {
        let ast = self.ast;
        for &predicate in &ast.lists[predicates.range()] {
            let size = nodes.len();
            let mut kept = 0;
            for position in 0..size {
                let node = nodes[position];
                let focus = Focus {
                    node,
                    position: position + 1,
                    size,
                };
                let keep = match self.expr(predicate, focus)? {
                    Value::Number(value) => value == (position + 1) as f64,
                    value => truthy(&value),
                };
                if keep {
                    nodes[kept] = node;
                    kept += 1;
                }
            }
            nodes.truncate(kept);
        }
        Ok(())
    }

    fn argument_string(
        &mut self,
        args: &[u32],
        index: usize,
        focus: Focus<T::Id>,
    ) -> Result<String> {
        match args.get(index) {
            Some(&arg) => self.string(arg, focus),
            None => self.string_value(focus.node),
        }
    }

    fn name(&mut self, node: Node<T::Id>, core: Core) -> Result<String> {
        let tree = self.tree;
        let unnamed = ("", "", "");
        let named =
            |name: tree::QName<'a>| (name.namespace.unwrap_or(""), name.local, name.qualified);
        let (uri, local, qualified) = match node {
            Node::Tree(id) => match tree.item(id) {
                Item::Element(name) => named(name),
                Item::Pi(target, _) => ("", target, target),
                _ => unnamed,
            },
            Node::Attribute(id, index) => tree
                .attribute(id, index)
                .map_or(unnamed, |(name, _)| named(name)),
            Node::Namespace(id, index) => {
                let prefix = self
                    .namespaces(id)?
                    .get(index)
                    .map_or("", |&(prefix, _)| prefix);
                ("", prefix, prefix)
            }
        };
        Ok(copied(match core {
            Core::LocalName => local,
            Core::NamespaceUri => uri,
            _ => qualified,
        })?)
    }

    fn lang(&mut self, node: Node<T::Id>, lang: &str) -> Result<bool> {
        let tree = self.tree;
        let (Node::Tree(start) | Node::Attribute(start, _) | Node::Namespace(start, _)) = node;
        for id in tree::scope(tree, start) {
            self.tick(1)?;
            if let Some(value) = tree::xml_attribute(tree, id, "lang").map(str::as_bytes) {
                return Ok(value
                    .get(..lang.len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(lang.as_bytes()))
                    && matches!(value.get(lang.len()), None | Some(b'-')));
            }
        }
        Ok(false)
    }

    /// `id()` finds elements by `xml:id` and DTD-declared `ID` attributes;
    /// for a repeated value only the first element counts, as in `xmlGetID`.
    fn id(&mut self, value: Value<T::Id>) -> Result<Vec<Node<T::Id>>> {
        let texts = match value {
            Value::NodeSet(nodes) => self.strings(&nodes)?,
            other => vec![self.cast_string(other)?],
        };
        let mut tokens = HashSet::new();
        for word in texts.iter().flat_map(|text| words(text)) {
            tokens.try_reserve(1)?;
            tokens.insert(word);
        }
        let tree = self.tree;
        let mut found = Vec::new();
        for &(id, index) in tree.ids() {
            if tokens.is_empty() {
                break;
            }
            self.tick(1)?;
            let value = tree.attribute(id, index).map(|(_, value)| value);
            if value.is_some_and(|value| tokens.remove(value))
                && found.last() != Some(&Node::Tree(id))
            {
                found.try_push(Node::Tree(id))?;
            }
        }
        Ok(found)
    }

    fn call(
        &mut self,
        function: Function,
        args: &[u32],
        focus: Focus<T::Id>,
    ) -> Result<Value<T::Id>> {
        let core = match function {
            Function::Core(core) => core,
            Function::Extension(name) => {
                let (uri, local) = self.expand(name)?;
                let context = self.context;
                let function = context
                    .functions
                    .get(uri)
                    .and_then(|scope| scope.get(local))
                    .ok_or(Error::at(ErrorKind::UnknownFunction, name.start()))?;
                let values = args
                    .iter()
                    .map(|&arg| self.expr(arg, focus))
                    .collect::<Result<Vec<_>>>()?;
                return self.checked(function(&values)?);
            }
        };
        let string = |eval: &mut Self, index| eval.argument_string(args, index, focus);
        Ok(match core {
            Core::Last => Value::Number(focus.size as f64),
            Core::Position => Value::Number(focus.position as f64),
            Core::Count => Value::Number(self.nodes(args[0], focus)?.len() as f64),
            Core::Id => {
                let value = self.expr(args[0], focus)?;
                Value::NodeSet(self.id(value)?)
            }
            Core::LocalName | Core::NamespaceUri | Core::Name => {
                let node = match args.first() {
                    Some(&arg) => self.nodes(arg, focus)?.first().copied(),
                    None => Some(focus.node),
                };
                Value::String(node.map_or(Ok(String::new()), |node| self.name(node, core))?)
            }
            Core::String => Value::String(string(self, 0)?),
            Core::Concat => {
                let mut joined = String::new();
                for index in 0..args.len() {
                    joined.try_push(&string(self, index)?)?;
                }
                Value::String(joined)
            }
            Core::StartsWith | Core::Contains | Core::SubstringBefore | Core::SubstringAfter => {
                let (text, pattern) = (string(self, 0)?, string(self, 1)?);
                let found = text.find(&pattern);
                match core {
                    Core::StartsWith => Value::Boolean(text.starts_with(&pattern)),
                    Core::Contains => Value::Boolean(found.is_some()),
                    Core::SubstringBefore => {
                        Value::String(copied(found.map_or("", |at| &text[..at]))?)
                    }
                    _ => Value::String(copied(found.map_or("", |at| &text[at + pattern.len()..]))?),
                }
            }
            Core::Substring => {
                let text = string(self, 0)?;
                let start = self.number(args[1], focus)?;
                let length = args
                    .get(2)
                    .map(|&arg| self.number(arg, focus))
                    .transpose()?;
                Value::String(copied(substring(&text, start, length))?)
            }
            Core::StringLength => Value::Number(string(self, 0)?.chars().count() as f64),
            Core::NormalizeSpace => {
                let text = string(self, 0)?;
                // No longer than the text.
                let mut joined = String::new();
                joined.try_reserve_exact(text.len())?;
                for word in words(&text) {
                    if !joined.is_empty() {
                        joined.push(' ');
                    }
                    joined.push_str(word);
                }
                Value::String(joined)
            }
            Core::Translate => {
                let (text, from, to) = (string(self, 0)?, string(self, 1)?, string(self, 2)?);
                Value::String(translate(&text, &from, &to)?)
            }
            Core::Boolean => Value::Boolean(self.boolean(args[0], focus)?),
            Core::Not => Value::Boolean(!self.boolean(args[0], focus)?),
            Core::True => Value::Boolean(true),
            Core::False => Value::Boolean(false),
            Core::Lang => {
                let lang = string(self, 0)?;
                Value::Boolean(self.lang(focus.node, &lang)?)
            }
            Core::Number => Value::Number(match args.first() {
                Some(&arg) => self.number(arg, focus)?,
                None => string_to_number(&self.string_value(focus.node)?),
            }),
            Core::Sum => {
                let mut total = 0.0;
                for node in self.nodes(args[0], focus)? {
                    total += string_to_number(&self.string_value(node)?);
                }
                Value::Number(total)
            }
            Core::Floor => Value::Number(self.number(args[0], focus)?.floor()),
            Core::Ceiling => Value::Number(self.number(args[0], focus)?.ceil()),
            Core::Round => Value::Number(round(self.number(args[0], focus)?)),
        })
    }
}

#[cfg(test)]
#[path = "../../tests/internal/xpath_values.rs"]
mod tests;
