//! XPath 1.0 over the borrowed [`Document`](crate::Document) and the
//! [`OwnedDocument`](crate::OwnedDocument).
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
mod parse;
mod tree;

use std::collections::HashMap;
use std::fmt;

pub use tree::Tree;

/// An XPath node: a tree node, or an element's attribute or namespace node.
///
/// `Attribute` indexes the element's attributes as the DOM lists them, so
/// namespace declarations keep their slots but are never selected.
/// `Namespace` indexes the element's in-scope namespaces in libxml2 order:
/// slot 0 is the implicit `xml` binding, then inherited bindings outermost first.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Node<Id> {
    Tree(Id),
    Attribute(Id, usize),
    Namespace(Id, usize),
}

impl<Id> From<Id> for Node<Id> {
    fn from(id: Id) -> Self {
        Self::Tree(id)
    }
}

/// The four XPath 1.0 value types. Node-sets are sorted in document order
/// without duplicates.
#[derive(Clone, Debug, PartialEq)]
pub enum Value<Id> {
    NodeSet(Vec<Node<Id>>),
    Boolean(bool),
    Number(f64),
    String(String),
}

/// A ceiling on compilation or evaluation work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Limit {
    ExpressionBytes,
    Depth,
    Operations,
    Nodes,
}

/// Finite resource ceilings, checked before the work they bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_expression_bytes: 64 * 1024,
            max_depth: 200,
            max_operations: 50_000_000,
            max_nodes: 10_000_000,
        }
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
    /// `XPATH_INVALID_CTXT`: a node that does not belong to the evaluated tree.
    InvalidContext,
    /// `XPATH_OP_LIMIT_EXCEEDED` / `XPATH_RECURSION_LIMIT_EXCEEDED` and size ceilings.
    LimitExceeded(Limit),
}

/// A compilation error carries the expression byte offset; evaluation errors do not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Error {
    pub kind: ErrorKind,
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

/// An extension function. It receives evaluated arguments.
pub type Function<Id> = Box<dyn Fn(&[Value<Id>]) -> Result<Value<Id>, Error> + Send + Sync>;

/// Static evaluation context: prefix bindings, variables, extension functions
/// and limits. Names are expanded as (namespace URI, local name), with `""`
/// for no namespace. The `xml` prefix is always bound.
pub struct Context<Id> {
    namespaces: HashMap<String, String>,
    variables: HashMap<String, HashMap<String, Value<Id>>>,
    functions: HashMap<String, HashMap<String, Function<Id>>>,
    limits: Limits,
}

impl<Id> Default for Context<Id> {
    fn default() -> Self {
        Self {
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
            .field("namespaces", &self.namespaces)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl<Id> Context<Id> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds `prefix` for name tests, variables and function names.
    pub fn with_namespace(mut self, prefix: impl Into<String>, uri: impl Into<String>) -> Self {
        self.namespaces.insert(prefix.into(), uri.into());
        self
    }

    pub fn with_variable(self, local: impl Into<String>, value: Value<Id>) -> Self {
        self.with_variable_ns("", local, value)
    }

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

    fn namespace(&self, prefix: &str) -> Option<&str> {
        match prefix {
            "xml" => Some(tree::XML_NAMESPACE),
            _ => self.namespaces.get(prefix).map(String::as_str),
        }
    }
}

/// A compiled XPath 1.0 expression, reusable across documents and threads.
#[derive(Clone, Debug)]
pub struct XPath {
    source: Box<str>,
    ast: parse::Ast,
}

impl XPath {
    pub fn compile(expression: &str) -> Result<Self, Error> {
        Self::compile_with_limits(expression, &Limits::default())
    }

    pub fn compile_with_limits(expression: &str, limits: &Limits) -> Result<Self, Error> {
        Ok(Self {
            ast: parse::parse(expression, limits)?,
            source: expression.into(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Evaluates with `node` as the context node, at position 1 of 1.
    pub fn evaluate<T: Tree>(
        &self,
        tree: &T,
        node: impl Into<Node<T::Id>>,
        context: &Context<T::Id>,
    ) -> Result<Value<T::Id>, Error> {
        eval::evaluate(tree, &self.source, &self.ast, node.into(), context)
    }

    /// Evaluates an expression that must produce a node-set.
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
