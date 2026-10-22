//! XPath 1.0 over [`Document`](crate::Document) and
//! [`OwnedDocument`](crate::OwnedDocument), libxml2's `xpath.c`.
//!
//! All axes, the 27 core functions, variables and extension functions are
//! supported; number formatting and string edge cases follow libxml2. Work
//! is bounded by [`Limits`]. See the
//! [XPath contract](https://github.com/pbdeuchler/yaxml/blob/main/docs/xpath.md).
//!
//! ```
//! use yaxml::Document;
//! use yaxml::xpath::{Context, Value, XPath};
//!
//! let document = Document::parse("<r><i k='1'/><i k='2'/></r>")?;
//! let sum = XPath::compile("sum(//i/@k)")?;
//! let value = sum.evaluate(&document, document.root().id(), &Context::new())?;
//! assert_eq!(value, Value::Number(3.0));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod eval;
pub(crate) mod parse;
// Shared helpers: some are unused when features are off.
#[cfg_attr(not(feature = "full"), allow(dead_code))]
pub(crate) mod tree;

use std::collections::{HashMap, TryReserveError};
use std::fmt;

use crate::fallible::{cloned, copied};

#[cfg(feature = "schemas")]
pub(crate) use eval::truthy;
pub use tree::Tree;

/// An XPath node: a tree node, or an element's attribute or namespace node.
///
/// `Attribute` indexes the element's attributes as the DOM lists them, so
/// namespace declarations keep their slots but are never selected.
/// `Namespace` indexes the element's in-scope namespaces in libxml2 order:
/// slot 0 is the implicit `xml` binding, then inherited bindings outermost first.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Node<Id> {
    /// A tree node.
    Tree(Id),
    /// An element and an attribute index.
    Attribute(Id, usize),
    /// An element and an in-scope namespace index.
    Namespace(Id, usize),
}

impl<Id> From<Id> for Node<Id> {
    fn from(id: Id) -> Self {
        Self::Tree(id)
    }
}

impl<Id: Copy> Node<Id> {
    /// The tree node: the node itself, or the attribute's or namespace's element.
    fn owner(self) -> Id {
        match self {
            Self::Tree(id) | Self::Attribute(id, _) | Self::Namespace(id, _) => id,
        }
    }
}

/// `xmlXPathCmpNodes`: how `a` and `b` of `tree` compare in document order
/// (an element, then its namespace nodes, then its attributes, then its
/// children). `None` when either is not in `tree` or they share no root
/// (nodes in different detached subtrees), libxml2's -2. Takes the depth of
/// both nodes plus the siblings between their branches; no ranking of the
/// whole document.
///
/// ```
/// use std::cmp::Ordering;
/// use yaxml::Document;
/// use yaxml::xpath::{Node, compare};
/// let document = Document::parse("<r a='1'><x/><y/></r>")?;
/// let r = document.root_element();
/// let (x, y) = (r.first_child().unwrap().id(), r.last_child().unwrap().id());
/// assert_eq!(compare(&document, Node::Tree(x), Node::Tree(y)), Some(Ordering::Less));
/// assert_eq!(compare(&document, Node::Attribute(r.id(), 0), Node::Tree(x)), Some(Ordering::Less));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn compare<T: Tree>(tree: &T, a: Node<T::Id>, b: Node<T::Id>) -> Option<std::cmp::Ordering> {
    let (x, y) = (a.owner(), b.owner());
    if !tree.contains(x) || !tree.contains(y) {
        return None;
    }
    // Within one element: the element, its namespaces, its attributes.
    let rank = |node: Node<T::Id>| match node {
        Node::Tree(_) => (0, 0),
        Node::Namespace(_, index) => (1, index),
        Node::Attribute(_, index) => (2, index),
    };
    if x == y {
        return Some(rank(a).cmp(&rank(b)));
    }
    let chain = |id| {
        let mut chain: Vec<T::Id> =
            std::iter::successors(Some(id), |&id| tree.parent(id)).collect();
        chain.reverse();
        chain
    };
    let (left, right) = (chain(x), chain(y));
    if left[0] != right[0] {
        return None;
    }
    let shared = left.iter().zip(&right).take_while(|(l, r)| l == r).count();
    // An ancestor precedes its descendants, and their attributes.
    let (Some(&l), Some(&r)) = (left.get(shared), right.get(shared)) else {
        return Some(left.len().cmp(&right.len()));
    };
    let mut sibling = tree.next_sibling(l);
    while let Some(id) = sibling {
        if id == r {
            return Some(std::cmp::Ordering::Less);
        }
        sibling = tree.next_sibling(id);
    }
    Some(std::cmp::Ordering::Greater)
}

/// The four XPath 1.0 value types. Node-sets are sorted in document order
/// without duplicates.
#[derive(Clone, Debug, PartialEq)]
pub enum Value<Id> {
    /// A node-set.
    NodeSet(Vec<Node<Id>>),
    /// A boolean.
    Boolean(bool),
    /// A number.
    Number(f64),
    /// A string.
    String(String),
}

impl<Id: Copy> Value<Id> {
    /// A copy that reports a refused allocation.
    pub(crate) fn try_clone(&self) -> Result<Self, TryReserveError> {
        Ok(match self {
            Self::NodeSet(nodes) => Self::NodeSet(cloned(nodes)?),
            Self::String(text) => Self::String(copied(text)?),
            scalar => scalar.clone(),
        })
    }
}

/// The [`Limits`] field an XPath error exceeded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Limit {
    /// [`Limits::max_expression_bytes`].
    ExpressionBytes,
    /// [`Limits::max_depth`].
    Depth,
    /// [`Limits::max_operations`].
    Operations,
    /// [`Limits::max_nodes`].
    Nodes,
}

/// Finite resource ceilings, checked before the work they bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Limits {
    /// Bytes in one compiled expression.
    pub max_expression_bytes: usize,
    /// Expression nesting, bounding parser and evaluator recursion.
    pub max_depth: usize,
    /// Evaluation steps: expression nodes, visited axis nodes and text chunks.
    pub max_operations: u64,
    /// Nodes in one intermediate or final node-set.
    pub max_nodes: usize,
}

setters!(const Limits { max_expression_bytes: usize, max_depth: usize, max_operations: u64, max_nodes: usize });

impl Limits {
    /// The default ceilings.
    pub const DEFAULT: Self = Self {
        max_expression_bytes: 64 * 1024,
        max_depth: 200,
        max_operations: 50_000_000,
        max_nodes: 10_000_000,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// libxml2 `xmlXPathError` categories that this implementation reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// `XPATH_UNFINISHED_LITERAL_ERROR`
    UnfinishedLiteral,
    /// `XPATH_START_LITERAL_ERROR`: `processing-instruction()` needs a literal.
    StartLiteral,
    /// `XPATH_VARIABLE_REF_ERROR`: `$` without a QName.
    VariableRef,
    /// `XPATH_UNDEF_VARIABLE_ERROR`
    UndefinedVariable,
    /// `XPATH_INVALID_PREDICATE_ERROR`: a predicate without its `]`.
    InvalidPredicate,
    /// `XPATH_EXPR_ERROR`: any other syntax error.
    Expression,
    /// `XPATH_UNCLOSED_ERROR`: a missing `)`.
    Unclosed,
    /// `XPATH_UNKNOWN_FUNC_ERROR`
    UnknownFunction,
    /// `XPATH_INVALID_TYPE`: a non-node-set where a node-set is required.
    InvalidType,
    /// `XPATH_INVALID_ARITY`
    InvalidArity,
    /// `XPATH_UNDEF_PREFIX_ERROR`
    UndefinedPrefix,
    /// `XPATH_INVALID_CHAR_ERROR`
    InvalidChar,
    /// `XPATH_FORBID_VARIABLE_ERROR`: a variable reference under [`NOVAR`].
    ForbiddenVariable,
    /// `XPATH_INVALID_CTXT`: a node that does not belong to the evaluated tree.
    InvalidContext,
    /// `XPATH_OP_LIMIT_EXCEEDED` / `XPATH_RECURSION_LIMIT_EXCEEDED` and size ceilings.
    LimitExceeded(Limit),
    /// `XPATH_MEMORY_ERROR`: an allocation the expression or tree sized failed.
    Allocation,
}

/// An XPath compilation or evaluation error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Error {
    /// What went wrong.
    pub kind: ErrorKind,
    /// The expression byte offset: always set for a compilation error, and
    /// for an evaluation error caused by an unknown name.
    pub offset: Option<usize>,
}

impl Error {
    /// An error without an expression position, e.g. from an extension function.
    pub const fn new(kind: ErrorKind) -> Self {
        Self { kind, offset: None }
    }

    pub(crate) const fn at(kind: ErrorKind, offset: usize) -> Self {
        Self {
            kind,
            offset: Some(offset),
        }
    }

    /// libxml2's `xmlXPathError` number and message for this error in
    /// `expression`, naming the prefix, variable or function as `xmlXPathErrFmt` does
    /// (domain `XML_FROM_XPATH`, code 1200 + `xmlXPathError`).
    ///
    /// ```
    /// use yaxml::xpath::XPath;
    /// let error = XPath::compile("'open").unwrap_err();
    /// assert_eq!(error.libxml2("'open"), (1202, "Unfinished literal".to_owned()));
    /// ```
    pub fn libxml2(&self, expression: &str) -> (u16, String) {
        let rest = (self.offset.and_then(|offset| expression.get(offset..))).unwrap_or("");
        let name = |colon| {
            let end = rest.find(|c| !crate::name::name_char(c, colon));
            &rest[..end.unwrap_or(rest.len())]
        };
        let (code, message) = match self.kind {
            ErrorKind::UnfinishedLiteral => (2, "Unfinished literal"),
            ErrorKind::StartLiteral => (3, "Start of literal"),
            ErrorKind::VariableRef => (4, "Expected $ for variable reference"),
            ErrorKind::UndefinedVariable => (5, "Undefined variable"),
            ErrorKind::InvalidPredicate => (6, "Invalid predicate"),
            ErrorKind::Expression => (7, "Invalid expression"),
            ErrorKind::Unclosed => (8, "Missing closing curly brace"),
            ErrorKind::UnknownFunction => (9, "Unregistered function"),
            ErrorKind::InvalidType => (11, "Invalid type"),
            ErrorKind::InvalidArity => (12, "Invalid number of arguments"),
            ErrorKind::UndefinedPrefix => (19, "Undefined namespace prefix"),
            ErrorKind::InvalidChar => (21, "Char out of XML range"),
            ErrorKind::InvalidContext => (22, "Invalid or incomplete context"),
            ErrorKind::ForbiddenVariable => (24, "Forbidden variable"),
            ErrorKind::LimitExceeded(Limit::Depth) => (26, "Recursion limit exceeded"),
            ErrorKind::LimitExceeded(_) => (25, "Operation limit exceeded"),
            ErrorKind::Allocation => (15, "Memory allocation error"),
        };
        let name = match self.kind {
            ErrorKind::UndefinedPrefix => name(false),
            ErrorKind::UndefinedVariable | ErrorKind::UnknownFunction => name(true),
            _ => "",
        };
        let message = match name {
            "" => message.to_owned(),
            name => format!("{message}: {name}"),
        };
        (1200 + code, message)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.offset {
            Some(offset) => write!(f, "XPath error {:?} at byte {offset}", self.kind),
            None => write!(f, "XPath error {:?}", self.kind),
        }
    }
}

impl std::error::Error for Error {}

impl From<TryReserveError> for Error {
    fn from(_: TryReserveError) -> Self {
        Self::new(ErrorKind::Allocation)
    }
}

/// An extension function. It receives evaluated arguments.
pub type Function<Id> = Box<dyn Fn(&[Value<Id>]) -> Result<Value<Id>, Error> + Send + Sync>;

/// Static evaluation context: prefix bindings, variables, extension functions
/// and limits. Names are expanded as (namespace URI, local name), with `""`
/// for no namespace. The `xml` prefix is always bound.
pub struct Context<Id> {
    flags: u32,
    namespaces: HashMap<String, String>,
    variables: HashMap<String, HashMap<String, Value<Id>>>,
    functions: HashMap<String, HashMap<String, Function<Id>>>,
    limits: Limits,
}

impl<Id> Default for Context<Id> {
    fn default() -> Self {
        Self {
            flags: 0,
            namespaces: HashMap::new(),
            variables: HashMap::new(),
            functions: HashMap::new(),
            limits: Limits::default(),
        }
    }
}

impl<Id> fmt::Debug for Context<Id> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Context")
            .field("flags", &self.flags)
            .field("namespaces", &self.namespaces)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl<Id> Context<Id> {
    /// An empty context with default limits.
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds `prefix` for name tests, variables and function names.
    pub fn with_namespace(mut self, prefix: impl Into<String>, uri: impl Into<String>) -> Self {
        self.namespaces.insert(prefix.into(), uri.into());
        self
    }

    /// Binds the unprefixed variable `$local`.
    pub fn with_variable(self, local: impl Into<String>, value: Value<Id>) -> Self {
        self.with_variable_ns("", local, value)
    }

    /// Binds the variable `local` in namespace `uri`.
    pub fn with_variable_ns(
        mut self,
        uri: impl Into<String>,
        local: impl Into<String>,
        value: Value<Id>,
    ) -> Self {
        self.variables
            .entry(uri.into())
            .or_default()
            .insert(local.into(), value);
        self
    }

    /// Registers an unprefixed extension function. Core functions take precedence.
    pub fn with_function(self, local: impl Into<String>, function: Function<Id>) -> Self {
        self.with_function_ns("", local, function)
    }

    /// Registers an extension function `local` in namespace `uri`.
    pub fn with_function_ns(
        mut self,
        uri: impl Into<String>,
        local: impl Into<String>,
        function: Function<Id>,
    ) -> Self {
        self.functions
            .entry(uri.into())
            .or_default()
            .insert(local.into(), function);
        self
    }

    /// Evaluation ceilings; compilation ceilings go to [`XPath::compile_with_limits`].
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// libxml2's `XML_XPATH_*` compilation flags ([`CHECKNS`], [`NOVAR`]),
    /// applied by [`XPath::compile_in`]; other bits are unused, as in libxml2.
    pub fn with_flags(mut self, flags: u32) -> Self {
        self.flags = flags;
        self
    }

    fn namespace(&self, prefix: &str) -> Option<&str> {
        match prefix {
            "xml" => Some(tree::XML_NAMESPACE),
            _ => self.namespaces.get(prefix).map(String::as_str),
        }
    }
}

/// `XML_XPATH_CHECKNS`: [`XPath::compile_in`] rejects a name test whose
/// prefix the context does not bind.
pub const CHECKNS: u32 = 1 << 0;
/// `XML_XPATH_NOVAR`: [`XPath::compile_in`] rejects variable references.
pub const NOVAR: u32 = 1 << 1;

/// A compiled XPath 1.0 expression, reusable across documents and threads.
#[derive(Clone, Debug)]
pub struct XPath {
    source: Box<str>,
    ast: parse::Ast,
}

impl XPath {
    /// Compiles an expression with default limits (`xmlXPathCompile`).
    pub fn compile(expression: &str) -> Result<Self, Error> {
        Self::compile_with_limits(expression, &Limits::default())
    }

    /// Compiles an expression within `limits`.
    pub fn compile_with_limits(expression: &str, limits: &Limits) -> Result<Self, Error> {
        Self::compiled(expression, limits, parse::Checks::NONE)
    }

    pub(crate) fn compiled(
        expression: &str,
        limits: &Limits,
        checks: parse::Checks<'_>,
    ) -> Result<Self, Error> {
        Ok(Self {
            ast: parse::parse(expression, limits, checks)?,
            source: copied(expression)?.into(),
        })
    }

    /// Compiles within `context`'s limits and [flags](Context::with_flags)
    /// (`xmlXPathCtxtCompile`): under [`CHECKNS`] a name-test prefix the
    /// context does not bind is `UndefinedPrefix`, under [`NOVAR`] a variable
    /// reference is `ForbiddenVariable`, where the parser meets it (before any later
    /// syntax error, after lexical ones).
    ///
    /// ```
    /// use yaxml::xpath::{CHECKNS, Context, ErrorKind, NOVAR, XPath};
    /// use yaxml::NodeId;
    ///
    /// let context = Context::<NodeId>::new()
    ///     .with_namespace("p", "urn:p")
    ///     .with_flags(CHECKNS | NOVAR);
    /// assert!(XPath::compile_in("/p:a/@p:b", &context).is_ok());
    /// let kind = |expression| XPath::compile_in(expression, &context).unwrap_err().kind;
    /// assert_eq!(kind("q:a"), ErrorKind::UndefinedPrefix);
    /// assert_eq!(kind("a[$v]/q:b"), ErrorKind::ForbiddenVariable);
    /// assert!(XPath::compile_in("q:a[$v]", &Context::<NodeId>::new()).is_ok());
    /// ```
    pub fn compile_in<Id>(expression: &str, context: &Context<Id>) -> Result<Self, Error> {
        let bound =
            |prefix: &str| context.flags & CHECKNS == 0 || context.namespace(prefix).is_some();
        let checks = parse::Checks {
            bound: &bound,
            variables: context.flags & NOVAR == 0,
        };
        Self::compiled(expression, &context.limits, checks)
    }

    /// The source expression.
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Evaluates with `node` as the context node, at position 1 of 1
    /// (`xmlXPathCompiledEval`).
    pub fn evaluate<T: Tree>(
        &self,
        tree: &T,
        node: impl Into<Node<T::Id>>,
        context: &Context<T::Id>,
    ) -> Result<Value<T::Id>, Error> {
        let mut unlimited = u64::MAX;
        self.evaluate_within(tree, node.into(), context, &mut unlimited)
    }

    /// Evaluates within at most `budget` operations, deducting those spent.
    pub(crate) fn evaluate_within<T: Tree>(
        &self,
        tree: &T,
        node: Node<T::Id>,
        context: &Context<T::Id>,
        budget: &mut u64,
    ) -> Result<Value<T::Id>, Error> {
        eval::evaluate(tree, &self.source, &self.ast, node, context, budget)
    }

    /// Evaluates an expression that must produce a node-set.
    ///
    /// # Errors
    ///
    /// `InvalidType` for any other result.
    pub fn select_nodes<T: Tree>(
        &self,
        tree: &T,
        node: impl Into<Node<T::Id>>,
        context: &Context<T::Id>,
    ) -> Result<Vec<Node<T::Id>>, Error> {
        match self.evaluate(tree, node, context)? {
            Value::NodeSet(nodes) => Ok(nodes),
            _ => Err(Error::new(ErrorKind::InvalidType)),
        }
    }
}
