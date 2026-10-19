#![doc = include_str!("../docs/schematron.md")]

use std::fmt;

use crate::pattern::{self, Pattern as Matcher};
use crate::xpath::tree::{
    Item, Navigate, Tree, attributes, descendants, namespaces, visible_preorder,
};
use crate::xpath::{self, Context, Node, Value, XPath, truthy};
use crate::{Document, ParseOptions, Resolver, options};

/// The ISO Schematron and Schematron 1.5 namespaces, both accepted.
const NAMESPACES: [&str; 2] = [
    "http://purl.oclc.org/dsdl/schematron",
    "http://www.ascc.net/xml/schematron",
];

/// `XML_SCHEMATRON_OUT_QUIET`: one pass over the document; reports are not output.
pub const OUT_QUIET: u32 = 1;
/// `XML_SCHEMATRON_OUT_TEXT`: a textual report, pattern by pattern.
pub const OUT_TEXT: u32 = 1 << 1;
/// `XML_SCHEMATRON_OUT_XML`: SVRL, which libxml2 never writes; nothing is output.
pub const OUT_XML: u32 = 1 << 2;
/// `XML_SCHEMATRON_OUT_ERROR`: asserts and reports as structured errors.
pub const OUT_ERROR: u32 = 1 << 3;

/// What a [`Diagnostic`] reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Code {
    /// `XML_ERR_INTERNAL_ERROR`: a `let` failed to evaluate.
    Internal,
    /// `XML_SCHEMAP_FAILED_LOAD`: the schema document is not well-formed.
    FailedLoad,
    /// `XML_SCHEMAP_NOROOT`, libxml2's code for every structural schema error.
    NoRoot,
    /// `XML_SCHEMAV_ATTRINVALID`: a `value-of` without a usable `select`.
    AttrInvalid,
    /// `XML_SCHEMATRONV_ASSERT`: an assert failed.
    Assert,
    /// `XML_SCHEMATRONV_REPORT`: a report fired.
    Report,
    /// An XPath error (`XML_FROM_XPATH`).
    XPath(xpath::ErrorKind),
}

impl Code {
    /// libxml2's `xmlParserErrors` number.
    pub fn number(self) -> u16 {
        match self {
            Self::Internal => 1,
            Self::FailedLoad => 1757,
            Self::NoRoot => 1759,
            Self::AttrInvalid => 1821,
            Self::Assert => 4000,
            Self::Report => 4001,
            Self::XPath(kind) => xpath::Error::new(kind).libxml2("").0,
        }
    }
}

/// One structured error, as libxml2 raises it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic<Id> {
    pub code: Code,
    /// The element concerned: an instance element for asserts and reports,
    /// `Some(())` for a schema element in [`SchemaErrors`].
    pub node: Option<Id>,
    /// libxml2's line of that element, 0 when unknown.
    pub line: usize,
    /// That element's local name.
    pub element: Option<String>,
    /// The pattern of an assert or report.
    pub pattern: Option<String>,
    /// libxml2's message, for asserts and reports `PATH line N: TEXT\n`.
    pub message: String,
}

impl<Id> Diagnostic<Id> {
    fn new(code: Code, message: String) -> Self {
        Self {
            code,
            node: None,
            line: 0,
            element: None,
            pattern: None,
            message,
        }
    }

    /// The text libxml2's `xmlFormatError` prints; `file` names the
    /// document holding `node`.
    pub fn render(&self, file: &str) -> String {
        let mut out = String::new();
        if self.node.is_some() {
            out += &format!("{file}:{}: ", self.line);
        }
        if let Some(element) = &self.element {
            out += &format!("element {element}: ");
        }
        let domain = match self.code {
            Code::Assert | Code::Report => "schematron",
            Code::Internal => "Schemas validity",
            Code::XPath(_) => "XPath",
            _ => "Schemas parser",
        };
        out += &format!("{domain} error : {}", self.message);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }
}

/// The errors that rejected a schema, in libxml2's order; never empty.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaErrors {
    /// The schema document's URI.
    pub uri: String,
    pub errors: Vec<Diagnostic<()>>,
}

impl fmt::Display for SchemaErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text: String = self.errors.iter().map(|e| e.render(&self.uri)).collect();
        f.write_str(text.trim_end_matches('\n'))
    }
}

impl std::error::Error for SchemaErrors {}

/// One item of validation output, in libxml2's order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Output<Id> {
    /// What libxml2 prints with `xmlPrintErrorMessage` (no [`OUT_ERROR`]):
    /// `Pattern: NAME\n` or `PATH line N: TEXT\n`.
    Text(String),
    /// A structured error: an assert or report under [`OUT_ERROR`], or an
    /// XPath or internal error.
    Error(Diagnostic<Id>),
}

/// The result of [`Schema::validate`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Validation<Id> {
    /// `xmlSchematronValidateDoc`'s result: the number of failed asserts,
    /// fired reports and failed `let`s, or `None` for its internal error
    /// (-1): no rules, a failed `let` in one-pass mode, or no operations left.
    pub errors: Option<usize>,
    pub output: Vec<Output<Id>>,
}

impl<Id> Validation<Id> {
    pub fn is_valid(&self) -> bool {
        self.errors == Some(0)
    }

    /// All output as libxml2 prints it; `file` names the instance.
    pub fn render(&self, file: &str) -> String {
        (self.output.iter())
            .map(|output| match output {
                Output::Text(text) => text.clone(),
                Output::Error(diagnostic) => diagnostic.render(file),
            })
            .collect()
    }
}

/// Bounds for parsing a schema and for each validation.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Limits for parsing the schema document.
    pub xml: crate::Limits,
    /// Compilation limits, and evaluation limits for each expression.
    pub xpath: xpath::Limits,
    /// XPath operations in one validation; when they run out validation
    /// stops with an internal error.
    pub max_operations: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            xml: crate::Limits::default(),
            xpath: xpath::Limits::default(),
            max_operations: 500_000_000,
        }
    }
}

/// A compiled Schematron schema: immutable, `Send + Sync` and reusable.
#[derive(Debug)]
pub struct Schema {
    title: Option<String>,
    namespaces: Vec<(String, String)>,
    patterns: Vec<Pattern>,
    limits: Limits,
}

#[derive(Debug)]
struct Pattern {
    name: String,
    rules: Vec<Rule>,
}

#[derive(Debug)]
struct Rule {
    context: Matcher,
    /// In evaluation order: libxml2 evaluates the last `let` first.
    lets: Vec<(String, XPath)>,
    tests: Vec<Test>,
}

#[derive(Debug)]
struct Test {
    report: bool,
    test: XPath,
    message: Vec<Part>,
}

#[derive(Debug)]
enum Part {
    Text(String),
    /// `name`, with its `path` compiled at use, as libxml2 does.
    Name(Option<(String, Result<XPath, xpath::Error>)>),
    ValueOf(XPath),
}

impl Schema {
    /// Parses the schema document `input` at URI `base` (libxml2's
    /// `xmlSchematronNewParserCtxt` and `xmlSchematronParse`), with
    /// libxml2's `XML_PARSE_NOENT`: entities are substituted and `resolver`
    /// loads external parsed entities, but not the external subset.
    pub fn parse(
        input: &[u8],
        base: &str,
        resolver: Box<dyn Resolver>,
        limits: &Limits,
    ) -> Result<Self, SchemaErrors> {
        let message = format!("xmlSchematronParse: could not load '{base}'.\n");
        let options = ParseOptions::new(options::NOENT).with_limits(limits.xml);
        let document = Document::parse_with(input, options.with_resolver(base, resolver));
        let document = document.map_err(|_| SchemaErrors {
            uri: base.to_owned(),
            errors: vec![Diagnostic::new(Code::FailedLoad, message)],
        })?;
        Self::from_tree(&document, base, input, limits)
    }

    /// Compiles the schema held by a document of either DOM
    /// (`xmlSchematronNewDocParserCtxt`). `source` is the text the tree was
    /// parsed from, for line numbers; pass `&[]` when there is none.
    pub fn from_tree<T: Tree>(
        tree: &T,
        base: &str,
        source: &[u8],
        limits: &Limits,
    ) -> Result<Self, SchemaErrors> {
        let mut parser = Parser {
            tree,
            base,
            lines: Lines::new(source),
            limits,
            namespaces: Vec::new(),
            errors: Vec::new(),
        };
        let schema = parser.schema();
        match parser.errors.is_empty() {
            true => Ok(schema.expect("a schema without errors")),
            false => Err(SchemaErrors {
                uri: base.to_owned(),
                errors: parser.errors,
            }),
        }
    }

    /// The schema's `title`, if any.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Validates a document of either DOM (`xmlSchematronNewValidCtxt` with
    /// `flags`, then `xmlSchematronValidateDoc`). `source` is the text the
    /// tree was parsed from, for line numbers; pass `&[]` when there is none.
    pub fn validate<T: Tree>(&self, tree: &T, flags: u32, source: &[u8]) -> Validation<T::Id> {
        let mut run = Run {
            schema: self,
            tree,
            flags,
            lines: Lines::new(source),
            budget: self.limits.max_operations,
            errors: 0,
            output: Vec::new(),
            variables: Vec::new(),
        };
        let errors = match document_element(tree) {
            None => Some(1),
            Some(root) => run.document(root).ok().map(|()| run.errors),
        };
        Validation {
            errors,
            output: run.output,
        }
    }
}

fn children<T: Navigate>(tree: &T, id: T::Id) -> impl Iterator<Item = T::Id> + '_ {
    std::iter::successors(tree.first_child(id), |&child| tree.next_sibling(child))
}

fn document_element<T: Navigate>(tree: &T) -> Option<T::Id> {
    children(tree, tree.root()).find(|&id| matches!(tree.item(id), Item::Element(_)))
}

/// The local name of an element in a Schematron namespace.
fn schematron_name<T: Navigate>(tree: &T, id: T::Id) -> Option<&str> {
    match tree.item(id) {
        Item::Element(name) if name.namespace.is_some_and(|ns| NAMESPACES.contains(&ns)) => {
            Some(name.local)
        }
        _ => None,
    }
}

fn local_name<T: Navigate>(tree: &T, id: T::Id) -> Option<String> {
    match tree.item(id) {
        Item::Element(name) => Some(name.local.to_owned()),
        _ => None,
    }
}

/// `xmlGetNoNsProp`.
fn attribute<'t, T: Navigate>(tree: &'t T, id: T::Id, name: &str) -> Option<&'t str> {
    attributes(tree, id)
        .find(|(_, attribute, _)| attribute.namespace.is_none() && attribute.qualified == name)
        .map(|(.., value)| value)
}

/// Line numbers as libxml2 records them: where an element's start tag ends.
struct Lines<'s> {
    source: &'s [u8],
    newlines: Vec<usize>,
}

impl<'s> Lines<'s> {
    fn new(source: &'s [u8]) -> Self {
        Self {
            source,
            newlines: memchr::memchr_iter(b'\n', source).collect(),
        }
    }

    fn line<T: Navigate>(&self, tree: &T, id: T::Id) -> usize {
        let Some(start) = tree.offset(id).filter(|&start| start < self.source.len()) else {
            return 0;
        };
        let mut quote = None;
        let end = self.source[start..].iter().position(|&byte| match quote {
            Some(open) => {
                quote = (byte != open).then_some(open);
                false
            }
            None => {
                quote = matches!(byte, b'"' | b'\'').then_some(byte);
                byte == b'>'
            }
        });
        let end = end.map_or(self.source.len(), |end| start + end);
        1 + self.newlines.partition_point(|&newline| newline < end)
    }
}

/// An XPath error as libxml2 prints it; compilation errors show the
/// expression with a caret, after the name test for an unbound prefix.
fn xpath_error<Id>(error: xpath::Error, expression: &str, compiled: bool) -> Diagnostic<Id> {
    let (_, mut message) = error.libxml2(expression);
    message.push('\n');
    let caret = error.offset.filter(|_| compiled).map(|offset| {
        let rest = expression.get(offset..).unwrap_or_default();
        let test = rest.find(|c| c != '*' && !crate::name::name_char(c, true));
        match error.kind {
            xpath::ErrorKind::UndefinedPrefix => offset + test.unwrap_or(rest.len()),
            _ => offset,
        }
    });
    if let Some(caret) = caret.filter(|&caret| caret < 100 && caret < expression.len()) {
        message += &format!("{expression}\n{}^\n", " ".repeat(caret));
    }
    Diagnostic::new(Code::XPath(error.kind), message)
}

struct Parser<'a, T: Navigate> {
    tree: &'a T,
    base: &'a str,
    lines: Lines<'a>,
    limits: &'a Limits,
    namespaces: Vec<(String, String)>,
    errors: Vec<Diagnostic<()>>,
}

impl<'a, T: Navigate> Parser<'a, T> {
    fn error(&mut self, code: Code, id: T::Id, message: String) {
        self.errors.push(Diagnostic {
            node: Some(()),
            line: self.lines.line(self.tree, id),
            element: local_name(self.tree, id),
            ..Diagnostic::new(code, message)
        });
    }

    /// The Schematron elements among `id`'s children.
    fn children(&self, id: T::Id) -> Vec<(T::Id, String)> {
        (children(self.tree, id))
            .filter_map(|child| Some((child, schematron_name(self.tree, child)?.to_owned())))
            .collect()
    }

    /// `xmlXPathCtxtCompile` with `XML_XPATH_CHECKNS` and the `ns` prefixes.
    fn xpath(&mut self, id: T::Id, expression: &str, what: &str, code: Code) -> Option<XPath> {
        let bound = |prefix: &str| {
            prefix == "xml" || self.namespaces.iter().any(|(bound, _)| bound == prefix)
        };
        let compiled = XPath::compile_with_limits(expression, &self.limits.xpath)
            .and_then(|xpath| xpath.check_prefixes(bound).map(|()| xpath));
        compiled
            .map_err(|error| {
                self.errors.push(xpath_error(error, expression, true));
                let message = format!("Failed to compile {what} expression {expression}");
                self.error(code, id, message);
            })
            .ok()
    }

    fn schema(&mut self) -> Option<Schema> {
        let tree = self.tree;
        let Some(root) = document_element(tree) else {
            let message = "The schema has no document element.\n".into();
            self.errors.push(Diagnostic::new(Code::NoRoot, message));
            return None;
        };
        if schematron_name(tree, root) != Some("schema") {
            let message = format!(
                "The XML document '{}' is not a XML schematron document",
                self.base
            );
            self.error(Code::NoRoot, root, message);
            return None;
        }
        let mut children = self.children(root).into_iter().peekable();
        let title = children
            .next_if(|(_, name)| name == "title")
            .map(|(id, _)| {
                descendants(tree, id)
                    .filter_map(|node| match tree.item(node) {
                        Item::Text(text) => Some(text),
                        _ => None,
                    })
                    .collect()
            });
        while let Some((id, _)) = children.next_if(|(_, name)| name == "ns") {
            let (prefix, uri) = (attribute(tree, id, "prefix"), attribute(tree, id, "uri"));
            if uri.is_none_or(str::is_empty) {
                self.error(Code::NoRoot, id, "ns element has no uri".into());
            }
            if prefix.is_none_or(str::is_empty) {
                self.error(Code::NoRoot, id, "ns element has no prefix".into());
            }
            if let (Some(prefix), Some(uri)) = (prefix, uri) {
                self.namespaces.push((prefix.to_owned(), uri.to_owned()));
            }
        }
        let mut patterns = Vec::new();
        let mut count = 0;
        for (id, name) in children {
            match name.as_str() {
                "pattern" => {
                    patterns.extend(self.pattern(id));
                    count += 1;
                }
                _ => {
                    let message = format!("Expecting a pattern element instead of {name}");
                    self.error(Code::NoRoot, id, message);
                }
            }
        }
        if count == 0 {
            let message = format!("The schematron document '{}' has no pattern", self.base);
            self.error(Code::NoRoot, root, message);
        }
        Some(Schema {
            title,
            namespaces: std::mem::take(&mut self.namespaces),
            patterns,
            limits: *self.limits,
        })
    }

    /// A pattern with neither `id` nor `name` is skipped unread, as in libxml2.
    fn pattern(&mut self, id: T::Id) -> Option<Pattern> {
        let tree = self.tree;
        let name = attribute(tree, id, "id").or_else(|| attribute(tree, id, "name"))?;
        let mut rules = Vec::new();
        let mut count = 0;
        for (child, local) in self.children(id) {
            match local.as_str() {
                "rule" => {
                    rules.extend(self.rule(child));
                    count += 1;
                }
                _ => {
                    let message = format!("Expecting a rule element instead of {local}");
                    self.error(Code::NoRoot, child, message);
                }
            }
        }
        if count == 0 {
            self.error(Code::NoRoot, id, "Pattern has no rule element".into());
        }
        Some(Pattern {
            name: name.to_owned(),
            rules,
        })
    }

    /// Reads a required, non-empty attribute, reporting `ELEMENT has no
    /// NAME attribute` or `ELEMENT has an empty NAME attribute`.
    fn required(&mut self, id: T::Id, element: &str, name: &str) -> Option<&'a str> {
        let value = attribute(self.tree, id, name);
        let message = match value {
            Some("") => format!("{element} has an empty {name} attribute"),
            Some(value) => return Some(value),
            None => format!("{element} has no {name} attribute"),
        };
        self.error(Code::NoRoot, id, message);
        None
    }

    fn rule(&mut self, id: T::Id) -> Option<Rule> {
        let context = self.required(id, "rule", "context")?;
        let namespaces: Vec<_> = (self.namespaces.iter())
            .map(|(prefix, uri)| (prefix.as_str(), uri.as_str()))
            .collect();
        let matcher = Matcher::compile(context, &namespaces, pattern::XPATH);
        if matcher.is_err() {
            let message = format!("Failed to compile context expression {context}");
            self.error(Code::NoRoot, id, message);
        }
        let (mut lets, mut tests, mut checks) = (Vec::new(), Vec::new(), 0);
        for (child, local) in self.children(id) {
            match local.as_str() {
                "let" => {
                    let name = self.required(child, "let", "name")?;
                    let value = self.required(child, "let", "value")?;
                    let value = self.xpath(child, value, "let", Code::NoRoot)?;
                    lets.insert(0, (name.to_owned(), value));
                }
                "assert" | "report" => {
                    checks += 1;
                    // libxml2 names a report "assert" here too.
                    let Some(test) = self.required(child, "assert", "test") else {
                        continue;
                    };
                    let message = self.message(child);
                    if let Some(test) = self.xpath(child, test, "test", Code::NoRoot) {
                        let report = local == "report";
                        tests.push(Test {
                            report,
                            test,
                            message,
                        });
                    }
                }
                _ => {
                    let message =
                        format!("Expecting an assert or a report element instead of {local}");
                    self.error(Code::NoRoot, child, message);
                }
            }
        }
        if checks == 0 {
            let message = "rule has no assert nor report element".into();
            self.error(Code::NoRoot, id, message);
        }
        Some(Rule {
            context: matcher.ok()?,
            lets,
            tests,
        })
    }

    /// The message of an assert or report: text, `name` and `value-of`.
    fn message(&mut self, id: T::Id) -> Vec<Part> {
        let tree = self.tree;
        let mut parts = Vec::new();
        for child in children(tree, id) {
            match (tree.item(child), schematron_name(tree, child)) {
                (Item::Text(text), _) => parts.push(Part::Text(text.to_owned())),
                (_, Some("name")) => {
                    parts.push(Part::Name(attribute(tree, child, "path").map(|path| {
                        let compiled = XPath::compile_with_limits(path, &self.limits.xpath);
                        (path.to_owned(), compiled)
                    })))
                }
                (_, Some("value-of")) => match attribute(tree, child, "select") {
                    None => {
                        let message = "value-of has no select attribute".into();
                        self.error(Code::AttrInvalid, child, message);
                    }
                    Some(select) => {
                        let compiled = self.xpath(child, select, "select", Code::AttrInvalid);
                        parts.extend(compiled.map(Part::ValueOf));
                    }
                },
                _ => {}
            }
        }
        parts
    }
}

/// Validation stopped with libxml2's internal error.
struct Abort;

struct Run<'a, T: Navigate> {
    schema: &'a Schema,
    tree: &'a T,
    flags: u32,
    lines: Lines<'a>,
    budget: u64,
    errors: usize,
    output: Vec<Output<T::Id>>,
    /// libxml2's XPath variable table: `let` names and values.
    variables: Vec<(String, Value<T::Id>)>,
}

impl<'a, T: Navigate> Run<'a, T> {
    /// `xmlSchematronValidateDoc` from the document element.
    fn document(&mut self, root: T::Id) -> Result<(), Abort> {
        let (tree, schema, flags) = (self.tree, self.schema, self.flags);
        if schema
            .patterns
            .iter()
            .all(|pattern| pattern.rules.is_empty())
        {
            return Err(Abort);
        }
        let elements: Vec<_> = visible_preorder(tree, Some(root), Some(root))
            .filter(|&id| matches!(tree.item(id), Item::Element(_)))
            .collect();
        let rules = |pattern: &'a Pattern| pattern.rules.iter().map(move |rule| (pattern, rule));
        if flags & OUT_QUIET != 0 || flags == 0 {
            // One pass: every rule of every pattern at each element.
            for &node in &elements {
                for (pattern, rule) in schema.patterns.iter().flat_map(rules) {
                    self.rule(pattern, rule, node, true)?;
                }
            }
            return Ok(());
        }
        for pattern in &schema.patterns {
            if flags & (OUT_ERROR | OUT_XML) == 0 {
                let header = truncate(format!("Pattern: {}\n", pattern.name));
                self.output.push(Output::Text(header));
            }
            for &node in &elements {
                for (pattern, rule) in rules(pattern) {
                    self.rule(pattern, rule, node, false)?;
                }
            }
        }
        Ok(())
    }

    /// Evaluates at `node`, outputting an XPath error as `None`.
    fn eval(
        &mut self,
        xpath: &XPath,
        node: T::Id,
        context: &Context<T::Id>,
    ) -> Result<Option<Value<T::Id>>, Abort> {
        let value = xpath.evaluate_within(self.tree, Node::Tree(node), context, &mut self.budget);
        value.map(Some).or_else(|error| {
            let diagnostic = xpath_error(error, xpath.as_str(), false);
            self.output.push(Output::Error(diagnostic));
            if self.budget == 0 {
                Err(Abort)
            } else {
                Ok(None)
            }
        })
    }

    /// Runs a matching rule's tests with its `let`s registered in the
    /// shared variable table, which, as in libxml2, keeps what a failed
    /// unregistration leaves behind.
    fn rule(
        &mut self,
        pattern: &Pattern,
        rule: &Rule,
        node: T::Id,
        one_pass: bool,
    ) -> Result<(), Abort> {
        if !rule.context.matches(self.tree, node) {
            return Ok(());
        }
        let context = |variables: &[(String, Value<T::Id>)]| {
            let context = Context::new().with_limits(self.schema.limits.xpath);
            let context = (self.schema.namespaces.iter())
                .fold(context, |context, (prefix, uri)| {
                    context.with_namespace(prefix, uri)
                });
            (variables.iter()).fold(context, |context, (name, value)| {
                context.with_variable(name.as_str(), value.clone())
            })
        };
        for (name, value) in &rule.lets {
            let Some(value) = self.eval(value, node, &context(&self.variables))? else {
                self.internal("Evaluation of compiled expression failed\n", one_pass)?;
                break;
            };
            self.variables.retain(|(known, _)| known != name);
            self.variables.push((name.clone(), value));
        }
        let context = context(&self.variables);
        for test in &rule.tests {
            self.test(pattern, test, node, &context)?;
        }
        for (name, _) in &rule.lets {
            let Some(index) = self.variables.iter().position(|(known, _)| known == name) else {
                return self.internal("Unregistering a let variable failed\n", one_pass);
            };
            self.variables.remove(index);
        }
        Ok(())
    }

    /// libxml2's `xmlSchematronVErr`, fatal in one-pass mode.
    fn internal(&mut self, message: &str, one_pass: bool) -> Result<(), Abort> {
        self.errors += 1;
        let diagnostic = Diagnostic::new(Code::Internal, message.to_owned());
        self.output.push(Output::Error(diagnostic));
        if one_pass { Err(Abort) } else { Ok(()) }
    }

    fn test(
        &mut self,
        pattern: &Pattern,
        test: &Test,
        node: T::Id,
        context: &Context<T::Id>,
    ) -> Result<(), Abort> {
        let passed = self
            .eval(&test.test, node, context)?
            .is_some_and(|value| truthy(&value));
        if passed != test.report {
            return Ok(());
        }
        self.errors += 1;
        let flags = self.flags;
        if flags & OUT_XML != 0 || (flags & OUT_QUIET != 0 && test.report) {
            return Ok(());
        }
        let line = self.lines.line(self.tree, node);
        let path = node_path(self.tree, node);
        let kind = if test.report { "report" } else { "assert" };
        let text = self.format(&test.message, node, context)?;
        let text = text.unwrap_or_else(|| format!("node failed {kind}"));
        let message = truncate(format!("{path} line {line}: {text}\n"));
        self.output.push(match flags & OUT_ERROR {
            0 => Output::Text(message),
            _ => Output::Error(Diagnostic {
                code: if test.report {
                    Code::Report
                } else {
                    Code::Assert
                },
                node: Some(node),
                line,
                element: local_name(self.tree, node),
                pattern: Some(pattern.name.clone()),
                message,
            }),
        });
        Ok(())
    }

    /// `xmlSchematronFormatReport`: `None` until something is appended.
    fn format(
        &mut self,
        parts: &[Part],
        node: T::Id,
        context: &Context<T::Id>,
    ) -> Result<Option<String>, Abort> {
        let tree = self.tree;
        let mut out: Option<String> = None;
        let push =
            |out: &mut Option<String>, text: &str| out.get_or_insert_default().push_str(text);
        for part in parts {
            match part {
                Part::Text(text) => push(&mut out, text),
                Part::Name(path) => {
                    let mut target = Node::Tree(node);
                    match path {
                        Some((_, Ok(xpath))) => {
                            if let Some(Value::NodeSet(nodes)) = self.eval(xpath, node, context)? {
                                target = nodes.first().copied().unwrap_or(target);
                            }
                        }
                        Some((path, Err(error))) => {
                            let diagnostic = xpath_error(*error, path, true);
                            self.output.push(Output::Error(diagnostic));
                        }
                        None => {}
                    }
                    let name = match target {
                        Node::Tree(id) => match tree.item(id) {
                            Item::Element(name) => Some(name.qualified),
                            _ => None,
                        },
                        Node::Attribute(owner, index) => {
                            tree.attribute(owner, index).map(|(name, _)| name.qualified)
                        }
                        Node::Namespace(..) => None,
                    };
                    name.into_iter().for_each(|name| push(&mut out, name));
                }
                Part::ValueOf(xpath) => match self.eval(xpath, node, context)? {
                    None => return Ok(out),
                    Some(Value::NodeSet(nodes)) => {
                        for (index, &member) in nodes.iter().enumerate() {
                            if index > 0 {
                                push(&mut out, " ");
                            }
                            value_name(tree, member)
                                .into_iter()
                                .for_each(|name| push(&mut out, name));
                        }
                    }
                    Some(Value::Boolean(value)) => {
                        push(&mut out, if value { "True" } else { "False" })
                    }
                    Some(Value::Number(value)) => push(&mut out, &printf_g(value)),
                    Some(Value::String(value)) => push(&mut out, &value),
                },
            }
            // Trailing whitespace becomes one space.
            if let Some(text) = &mut out {
                let kept = text.trim_end_matches([' ', '\n', '\r', '\t']).len();
                if kept < text.len() {
                    text.truncate(kept);
                    text.push(' ');
                }
            }
        }
        Ok(out)
    }
}

/// libxml2's `snprintf(msg, 999, ...)`: at most 998 bytes.
fn truncate(mut text: String) -> String {
    let mut end = text.len().min(998);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}

/// The `name` field libxml2 reads for a `value-of` node-set member.
fn value_name<T: Navigate>(tree: &T, node: Node<T::Id>) -> Option<&str> {
    match node {
        Node::Tree(id) => match tree.item(id) {
            Item::Element(name) => Some(name.local),
            Item::Text(_) if tree.is_cdata(id) => None,
            Item::Text(_) => Some("text"),
            Item::Comment(_) => Some("comment"),
            Item::Pi(target, _) => Some(target),
            Item::EntityRef(name) => Some(name),
            Item::Root => None,
        },
        Node::Attribute(owner, index) => tree.attribute(owner, index).map(|(name, _)| name.local),
        // An `xmlNs` read as a node: its "name" is the URI.
        Node::Namespace(owner, index) => namespaces(tree, owner).get(index).map(|&(_, uri)| uri),
    }
}

/// `xmlGetNodePath` for an element: `/a/p:b[2]/*`, with `*` for an
/// unprefixed name in a namespace.
fn node_path<T: Navigate>(tree: &T, node: T::Id) -> String {
    let chain: Vec<_> = std::iter::successors(Some(node), |&id| tree.parent(id)).collect();
    let mut path = String::new();
    for &id in chain.iter().rev() {
        let Item::Element(name) = tree.item(id) else {
            continue;
        };
        let generic = name.namespace.is_some() && !name.qualified.contains(':');
        path.push('/');
        path.push_str(if generic { "*" } else { name.qualified });
        let same = |other: T::Id| match tree.item(other) {
            Item::Element(other) => {
                generic
                    || (other.qualified == name.qualified
                        && other.namespace.is_some() == name.namespace.is_some())
            }
            _ => false,
        };
        let siblings = tree.parent(id).map(|parent| children(tree, parent));
        let (mut before, mut after, mut seen) = (0, false, false);
        for sibling in siblings
            .into_iter()
            .flatten()
            .filter(|&sibling| sibling == id || same(sibling))
        {
            match (sibling == id, seen) {
                (true, _) => seen = true,
                (false, false) => before += 1,
                (false, true) => after = true,
            }
        }
        match (before, after) {
            (0, false) => {}
            (0, true) => path.push_str("[1]"),
            (before, _) => path.push_str(&format!("[{}]", before + 1)),
        }
    }
    path
}

/// C's `printf("%g")`: six significant digits, exponent form outside
/// [1e-4, 1e6), trailing zeros removed.
fn printf_g(value: f64) -> String {
    if !value.is_finite() {
        let text = match value {
            _ if value.is_nan() => "nan",
            _ if value > 0.0 => "inf",
            _ => "-inf",
        };
        return text.to_owned();
    }
    let trim = |text: &str| match text.contains('.') {
        true => text.trim_end_matches('0').trim_end_matches('.').to_owned(),
        false => text.to_owned(),
    };
    let scientific = format!("{value:.5e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if (-4..6).contains(&exponent) {
        return trim(&format!("{value:.*}", (5 - exponent) as usize));
    }
    let sign = if exponent < 0 { '-' } else { '+' };
    format!("{}e{sign}{:02}", trim(mantissa), exponent.unsigned_abs())
}

#[cfg(test)]
#[path = "../tests/internal/schematron.rs"]
mod tests;
