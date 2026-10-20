//! Lexer and parser to a flat AST: expressions, steps and child lists are
//! vectors addressed by `u32` indices; names and literals are source spans.

use super::{Error, ErrorKind, Limit, Limits};
use crate::fallible::TryPush;
use crate::name::{name_char, name_start};

/// A half-open `u32` range into the source text, or into an AST vector.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Span(u32, u32);

impl Span {
    pub(crate) fn range(self) -> std::ops::Range<usize> {
        self.0 as usize..self.1 as usize
    }

    pub(crate) fn text(self, source: &str) -> &str {
        &source[self.range()]
    }

    pub(crate) fn is_empty(self) -> bool {
        self.0 == self.1
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct QName {
    pub(crate) prefix: Option<Span>,
    pub(crate) local: Span,
}

impl QName {
    pub(crate) fn start(self) -> usize {
        self.prefix.unwrap_or(self.local).range().start
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Axis {
    Ancestor,
    AncestorOrSelf,
    Attribute,
    Child,
    Descendant,
    DescendantOrSelf,
    Following,
    FollowingSibling,
    Namespace,
    Parent,
    Preceding,
    PrecedingSibling,
    Self_,
}

const AXES: [(&str, Axis); 13] = [
    ("ancestor", Axis::Ancestor),
    ("ancestor-or-self", Axis::AncestorOrSelf),
    ("attribute", Axis::Attribute),
    ("child", Axis::Child),
    ("descendant", Axis::Descendant),
    ("descendant-or-self", Axis::DescendantOrSelf),
    ("following", Axis::Following),
    ("following-sibling", Axis::FollowingSibling),
    ("namespace", Axis::Namespace),
    ("parent", Axis::Parent),
    ("preceding", Axis::Preceding),
    ("preceding-sibling", Axis::PrecedingSibling),
    ("self", Axis::Self_),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Test {
    Name(QName),
    /// `*` or `prefix:*`.
    Any(Option<Span>),
    Node,
    Text,
    Comment,
    Pi(Option<Span>),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Step {
    pub(crate) axis: Axis,
    pub(crate) test: Test,
    pub(crate) predicates: Span,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Union,
}

impl BinOp {
    fn precedence(self) -> u8 {
        match self {
            Self::Or => 1,
            Self::And => 2,
            Self::Eq | Self::Ne => 3,
            Self::Lt | Self::Le | Self::Gt | Self::Ge => 4,
            Self::Add | Self::Sub => 5,
            Self::Mul | Self::Div | Self::Mod => 6,
            Self::Union => 7,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Core {
    Last,
    Position,
    Count,
    Id,
    LocalName,
    NamespaceUri,
    Name,
    String,
    Concat,
    StartsWith,
    Contains,
    SubstringBefore,
    SubstringAfter,
    Substring,
    StringLength,
    NormalizeSpace,
    Translate,
    Boolean,
    Not,
    True,
    False,
    Lang,
    Number,
    Sum,
    Floor,
    Ceiling,
    Round,
}

/// Core function names with their minimum and maximum arity.
const CORE: [(&str, Core, usize, usize); 27] = [
    ("last", Core::Last, 0, 0),
    ("position", Core::Position, 0, 0),
    ("count", Core::Count, 1, 1),
    ("id", Core::Id, 1, 1),
    ("local-name", Core::LocalName, 0, 1),
    ("namespace-uri", Core::NamespaceUri, 0, 1),
    ("name", Core::Name, 0, 1),
    ("string", Core::String, 0, 1),
    ("concat", Core::Concat, 2, usize::MAX),
    ("starts-with", Core::StartsWith, 2, 2),
    ("contains", Core::Contains, 2, 2),
    ("substring-before", Core::SubstringBefore, 2, 2),
    ("substring-after", Core::SubstringAfter, 2, 2),
    ("substring", Core::Substring, 2, 3),
    ("string-length", Core::StringLength, 0, 1),
    ("normalize-space", Core::NormalizeSpace, 0, 1),
    ("translate", Core::Translate, 3, 3),
    ("boolean", Core::Boolean, 1, 1),
    ("not", Core::Not, 1, 1),
    ("true", Core::True, 0, 0),
    ("false", Core::False, 0, 0),
    ("lang", Core::Lang, 1, 1),
    ("number", Core::Number, 0, 1),
    ("sum", Core::Sum, 1, 1),
    ("floor", Core::Floor, 1, 1),
    ("ceiling", Core::Ceiling, 1, 1),
    ("round", Core::Round, 1, 1),
];

#[derive(Clone, Copy, Debug)]
pub(crate) enum Function {
    Core(Core),
    Extension(QName),
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Start {
    Root,
    Context,
    Expr(u32),
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Expr {
    Number(f64),
    Literal(Span),
    Variable(QName),
    /// Arguments are a span of `Ast::lists`.
    Call(Function, Span),
    Binary(BinOp, u32, u32),
    Negate(u32),
    /// Steps are a span of `Ast::steps`.
    Path(Start, Span),
    /// Predicates are a span of `Ast::lists`.
    Filter(u32, Span),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Ast {
    pub(crate) exprs: Vec<Expr>,
    pub(crate) steps: Vec<Step>,
    pub(crate) lists: Vec<u32>,
    pub(crate) root: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tok {
    LParen,
    RParen,
    LBracket,
    RBracket,
    Dot,
    DotDot,
    At,
    Comma,
    ColonColon,
    Slash,
    SlashSlash,
    Op(BinOp),
    Literal(Span),
    Number(f64),
    Variable(QName),
    Name(QName),
    Any(Option<Span>),
    End,
}

fn span(start: usize, end: usize) -> Span {
    // The expression limit is checked against u32 before lexing.
    Span(start as u32, end as u32)
}

fn ncname(source: &str, start: usize) -> Option<usize> {
    let mut chars = source[start..].char_indices();
    let (_, first) = chars.next()?;
    if !name_start(first, false) {
        return None;
    }
    Some(
        chars
            .find(|&(_, c)| !name_char(c, false))
            .map_or(source.len(), |(i, _)| start + i),
    )
}

/// A QName (or `prefix:*` when `wildcard`) at `start`, returning its end.
fn qname(source: &str, start: usize, wildcard: bool) -> Option<(Tok, usize)> {
    let end = ncname(source, start)?;
    let bytes = source.as_bytes();
    if bytes.get(end) != Some(&b':') || bytes.get(end + 1) == Some(&b':') {
        return Some((
            Tok::Name(QName {
                prefix: None,
                local: span(start, end),
            }),
            end,
        ));
    }
    let prefix = Some(span(start, end));
    if wildcard && bytes.get(end + 1) == Some(&b'*') {
        return Some((Tok::Any(prefix), end + 2));
    }
    let local_end = ncname(source, end + 1)?;
    Some((
        Tok::Name(QName {
            prefix,
            local: span(end + 1, local_end),
        }),
        local_end,
    ))
}

/// libxml2's number scanner (`xmlXPathCompNumber`): digits, an optional
/// fraction of at most 20 significant digits and an optional exponent.
pub(crate) fn scan_number(bytes: &[u8]) -> Option<(f64, usize)> {
    const MAX_FRAC: i32 = 20;
    let digit = |i: usize| {
        bytes
            .get(i)
            .filter(|b| b.is_ascii_digit())
            .map(|b| f64::from(b - b'0'))
    };
    let mut i = 0;
    let mut value = 0.0;
    while let Some(d) = digit(i) {
        value = value * 10.0 + d;
        i += 1;
    }
    let mut ok = i > 0;
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        ok |= digit(i).is_some();
        let mut frac = 0;
        while bytes.get(i) == Some(&b'0') {
            frac += 1;
            i += 1;
        }
        let (max, mut fraction) = (frac + MAX_FRAC, 0.0);
        while let Some(d) = digit(i).filter(|_| frac < max) {
            fraction = fraction * 10.0 + d;
            frac += 1;
            i += 1;
        }
        value += fraction / 10f64.powf(f64::from(frac));
        while digit(i).is_some() {
            i += 1;
        }
    }
    if !ok {
        return None;
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        i += 1;
        let negative = bytes.get(i) == Some(&b'-');
        i += usize::from(negative || bytes.get(i) == Some(&b'+'));
        let mut exponent = 0;
        while let Some(d) = bytes.get(i).filter(|b| b.is_ascii_digit()) {
            if exponent < 1_000_000 {
                exponent = exponent * 10 + i32::from(d - b'0');
            }
            i += 1;
        }
        value *= 10f64.powf(f64::from(if negative { -exponent } else { exponent }));
    }
    Some((value, i))
}

fn lex(source: &str) -> Result<Vec<(Tok, u32)>, Error> {
    let bytes = source.as_bytes();
    let mut tokens: Vec<(Tok, u32)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
            i += 1;
            continue;
        }
        let next = bytes.get(i + 1).copied();
        // XPath 1.0 section 3.7: after an operand, `*` multiplies and
        // NCNames are operator names.
        let after_operand = tokens.last().is_some_and(|(token, _)| {
            !matches!(
                token,
                Tok::At
                    | Tok::ColonColon
                    | Tok::LParen
                    | Tok::LBracket
                    | Tok::Comma
                    | Tok::Op(_)
                    | Tok::Slash
                    | Tok::SlashSlash
            )
        });
        let error = |kind| Error::at(kind, i);
        let (token, end) = match (byte, next) {
            (b'(', _) => (Tok::LParen, i + 1),
            (b')', _) => (Tok::RParen, i + 1),
            (b'[', _) => (Tok::LBracket, i + 1),
            (b']', _) => (Tok::RBracket, i + 1),
            (b'@', _) => (Tok::At, i + 1),
            (b',', _) => (Tok::Comma, i + 1),
            (b':', Some(b':')) => (Tok::ColonColon, i + 2),
            (b'/', Some(b'/')) => (Tok::SlashSlash, i + 2),
            (b'/', _) => (Tok::Slash, i + 1),
            (b'|', _) => (Tok::Op(BinOp::Union), i + 1),
            (b'+', _) => (Tok::Op(BinOp::Add), i + 1),
            (b'-', _) => (Tok::Op(BinOp::Sub), i + 1),
            (b'=', _) => (Tok::Op(BinOp::Eq), i + 1),
            (b'!', Some(b'=')) => (Tok::Op(BinOp::Ne), i + 2),
            (b'<', Some(b'=')) => (Tok::Op(BinOp::Le), i + 2),
            (b'<', _) => (Tok::Op(BinOp::Lt), i + 1),
            (b'>', Some(b'=')) => (Tok::Op(BinOp::Ge), i + 2),
            (b'>', _) => (Tok::Op(BinOp::Gt), i + 1),
            (b'*', _) if after_operand => (Tok::Op(BinOp::Mul), i + 1),
            (b'*', _) => (Tok::Any(None), i + 1),
            (b'.', Some(b'.')) => (Tok::DotDot, i + 2),
            (b'.' | b'0'..=b'9', _) if byte != b'.' || next.is_some_and(|b| b.is_ascii_digit()) => {
                let (value, length) =
                    scan_number(&bytes[i..]).ok_or(error(ErrorKind::Expression))?;
                (Tok::Number(value), i + length)
            }
            (b'.', _) => (Tok::Dot, i + 1),
            (b'"' | b'\'', _) => {
                let close = memchr::memchr(byte, &bytes[i + 1..])
                    .ok_or(error(ErrorKind::UnfinishedLiteral))?;
                (Tok::Literal(span(i + 1, i + 1 + close)), i + close + 2)
            }
            (b'$', _) => match qname(source, i + 1, false) {
                Some((Tok::Name(name), end)) => (Tok::Variable(name), end),
                _ => return Err(error(ErrorKind::VariableRef)),
            },
            _ => {
                let (token, end) = qname(source, i, true).ok_or(error(ErrorKind::InvalidChar))?;
                let operator = match &source[i..end] {
                    "and" => Some(BinOp::And),
                    "or" => Some(BinOp::Or),
                    "mod" => Some(BinOp::Mod),
                    "div" => Some(BinOp::Div),
                    _ => None,
                };
                match operator.filter(|_| after_operand) {
                    Some(operator) => (Tok::Op(operator), end),
                    None => (token, end),
                }
            }
        };
        tokens.try_push((token, i as u32))?;
        i = end;
    }
    tokens.try_push((Tok::End, bytes.len() as u32))?;
    Ok(tokens)
}

struct Parser<'s> {
    source: &'s str,
    tokens: Vec<(Tok, u32)>,
    position: usize,
    ast: Ast,
    depths: Vec<usize>,
    nesting: usize,
    max_depth: usize,
}

pub(crate) fn parse(source: &str, limits: &Limits) -> Result<Ast, Error> {
    if source.len() > limits.max_expression_bytes || u32::try_from(source.len()).is_err() {
        return Err(Error::at(
            ErrorKind::LimitExceeded(Limit::ExpressionBytes),
            0,
        ));
    }
    let mut parser = Parser {
        source,
        tokens: lex(source)?,
        position: 0,
        ast: Ast::default(),
        depths: Vec::new(),
        nesting: 0,
        max_depth: limits.max_depth,
    };
    parser.ast.root = parser.expr()?;
    match parser.peek() {
        Tok::End => Ok(parser.ast),
        _ => Err(parser.error(ErrorKind::Expression)),
    }
}

const fn self_or_descendants() -> Step {
    Step {
        axis: Axis::DescendantOrSelf,
        test: Test::Node,
        predicates: Span(0, 0),
    }
}

impl Parser<'_> {
    fn peek(&self) -> Tok {
        self.tokens[self.position].0
    }

    fn peek_second(&self) -> Tok {
        self.tokens
            .get(self.position + 1)
            .map_or(Tok::End, |token| token.0)
    }

    fn bump(&mut self) -> Tok {
        let token = self.peek();
        self.position += usize::from(token != Tok::End);
        token
    }

    fn error(&self, kind: ErrorKind) -> Error {
        Error::at(kind, self.tokens[self.position].1 as usize)
    }

    fn expect(&mut self, token: Tok, kind: ErrorKind) -> Result<(), Error> {
        if self.peek() != token {
            return Err(self.error(kind));
        }
        self.bump();
        Ok(())
    }

    fn list(&mut self, items: &[u32]) -> Result<Span, Error> {
        let start = self.ast.lists.len();
        self.ast.lists.try_reserve(items.len())?;
        self.ast.lists.extend_from_slice(items);
        Ok(span(start, self.ast.lists.len()))
    }

    fn deepest(&self, list: Span) -> usize {
        self.ast.lists[list.range()]
            .iter()
            .map(|&id| self.depths[id as usize])
            .max()
            .unwrap_or(0)
    }

    /// Appends an expression, bounding the AST depth the evaluator will recurse through.
    fn push(&mut self, expr: Expr) -> Result<u32, Error> {
        let depth = |id: u32| self.depths[id as usize];
        let children = match expr {
            Expr::Number(_) | Expr::Literal(_) | Expr::Variable(_) => 0,
            Expr::Call(_, args) => self.deepest(args),
            Expr::Binary(_, left, right) => depth(left).max(depth(right)),
            Expr::Negate(operand) => depth(operand),
            Expr::Filter(base, predicates) => depth(base).max(self.deepest(predicates)),
            Expr::Path(start, steps) => self.ast.steps[steps.range()]
                .iter()
                .map(|step| self.deepest(step.predicates))
                .chain(match start {
                    Start::Expr(base) => Some(depth(base)),
                    _ => None,
                })
                .max()
                .unwrap_or(0),
        };
        if children >= self.max_depth {
            return Err(self.error(ErrorKind::LimitExceeded(Limit::Depth)));
        }
        self.depths.try_push(children + 1)?;
        self.ast.exprs.try_push(expr)?;
        Ok((self.ast.exprs.len() - 1) as u32)
    }

    fn expr(&mut self) -> Result<u32, Error> {
        if self.nesting >= self.max_depth {
            return Err(self.error(ErrorKind::LimitExceeded(Limit::Depth)));
        }
        self.nesting += 1;
        let expr = self.binary(1);
        self.nesting -= 1;
        expr
    }

    /// Precedence climbing over the left-associative binary levels.
    fn binary(&mut self, minimum: u8) -> Result<u32, Error> {
        let mut left = self.unary()?;
        while let Tok::Op(op) = self.peek() {
            if op == BinOp::Union || op.precedence() < minimum {
                break;
            }
            self.bump();
            let right = self.binary(op.precedence() + 1)?;
            left = self.push(Expr::Binary(op, left, right))?;
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<u32, Error> {
        let mut negations = 0;
        while self.peek() == Tok::Op(BinOp::Sub) {
            self.bump();
            negations += 1;
        }
        let mut expr = self.path()?;
        while self.peek() == Tok::Op(BinOp::Union) {
            self.bump();
            let right = self.path()?;
            expr = self.push(Expr::Binary(BinOp::Union, expr, right))?;
        }
        for _ in 0..negations {
            expr = self.push(Expr::Negate(expr))?;
        }
        Ok(expr)
    }

    fn is_node_type(&self, token: Tok) -> bool {
        matches!(token, Tok::Name(QName { prefix: None, local })
            if matches!(local.text(self.source), "node" | "text" | "comment" | "processing-instruction"))
    }

    fn starts_step(&self) -> bool {
        matches!(
            self.peek(),
            Tok::Dot | Tok::DotDot | Tok::At | Tok::Any(_) | Tok::Name(_)
        )
    }

    fn path(&mut self) -> Result<u32, Error> {
        let (start, steps) = match self.peek() {
            Tok::Slash => {
                self.bump();
                let steps = if self.starts_step() {
                    self.relative(Vec::new())?
                } else {
                    Span::default()
                };
                (Start::Root, steps)
            }
            Tok::SlashSlash => {
                self.bump();
                (Start::Root, self.relative(vec![self_or_descendants()])?)
            }
            Tok::Variable(_) | Tok::LParen | Tok::Literal(_) | Tok::Number(_) => self.filtered()?,
            token @ Tok::Name(_)
                if self.peek_second() == Tok::LParen && !self.is_node_type(token) =>
            {
                self.filtered()?
            }
            _ => (Start::Context, self.relative(Vec::new())?),
        };
        match start {
            Start::Expr(base) if steps.is_empty() => Ok(base),
            _ => self.push(Expr::Path(start, steps)),
        }
    }

    /// A filter expression, optionally followed by `/` or `//` and a relative path.
    fn filtered(&mut self) -> Result<(Start, Span), Error> {
        let primary = self.primary()?;
        let predicates = self.predicates()?;
        let base = if predicates.is_empty() {
            primary
        } else {
            self.push(Expr::Filter(primary, predicates))?
        };
        let steps = match self.peek() {
            Tok::Slash => {
                self.bump();
                self.relative(Vec::new())?
            }
            Tok::SlashSlash => {
                self.bump();
                self.relative(vec![self_or_descendants()])?
            }
            _ => Span::default(),
        };
        Ok((Start::Expr(base), steps))
    }

    fn relative(&mut self, mut steps: Vec<Step>) -> Result<Span, Error> {
        loop {
            let step = self.step()?;
            // `descendant-or-self::node()/child::t` is `descendant::t` when
            // no predicate observes child positions.
            match steps.last_mut() {
                Some(previous)
                    if step.axis == Axis::Child
                        && step.predicates.is_empty()
                        && previous.axis == Axis::DescendantOrSelf
                        && previous.test == Test::Node
                        && previous.predicates.is_empty() =>
                {
                    *previous = Step {
                        axis: Axis::Descendant,
                        ..step
                    };
                }
                _ => steps.try_push(step)?,
            }
            match self.peek() {
                Tok::Slash => {
                    self.bump();
                }
                Tok::SlashSlash => {
                    self.bump();
                    steps.try_push(self_or_descendants())?;
                }
                _ => break,
            }
        }
        let start = self.ast.steps.len();
        self.ast.steps.try_reserve(steps.len())?;
        self.ast.steps.extend(steps);
        Ok(span(start, self.ast.steps.len()))
    }

    fn step(&mut self) -> Result<Step, Error> {
        let abbreviated = |axis| Step {
            axis,
            test: Test::Node,
            predicates: Span::default(),
        };
        let axis = match (self.peek(), self.peek_second()) {
            (Tok::Dot, _) => {
                self.bump();
                return Ok(abbreviated(Axis::Self_));
            }
            (Tok::DotDot, _) => {
                self.bump();
                return Ok(abbreviated(Axis::Parent));
            }
            (Tok::At, _) => {
                self.bump();
                Axis::Attribute
            }
            (
                Tok::Name(QName {
                    prefix: None,
                    local,
                }),
                Tok::ColonColon,
            ) => {
                let name = local.text(self.source);
                let axis = AXES
                    .iter()
                    .find(|(axis, _)| *axis == name)
                    .ok_or(self.error(ErrorKind::Expression))?
                    .1;
                self.bump();
                self.bump();
                axis
            }
            _ => Axis::Child,
        };
        let test = self.node_test()?;
        let predicates = self.predicates()?;
        Ok(Step {
            axis,
            test,
            predicates,
        })
    }

    fn node_test(&mut self) -> Result<Test, Error> {
        let token = self.peek();
        let name = match token {
            Tok::Any(prefix) => {
                self.bump();
                return Ok(Test::Any(prefix));
            }
            Tok::Name(name) => name,
            _ => return Err(self.error(ErrorKind::Expression)),
        };
        self.bump();
        if !(self.is_node_type(token) && self.peek() == Tok::LParen) {
            return Ok(Test::Name(name));
        }
        self.bump();
        let test = match name.local.text(self.source) {
            "node" => Test::Node,
            "text" => Test::Text,
            "comment" => Test::Comment,
            _ => match self.peek() {
                Tok::RParen => Test::Pi(None),
                Tok::Literal(target) => {
                    self.bump();
                    Test::Pi(Some(target))
                }
                _ => return Err(self.error(ErrorKind::StartLiteral)),
            },
        };
        self.expect(Tok::RParen, ErrorKind::Unclosed)?;
        Ok(test)
    }

    fn predicates(&mut self) -> Result<Span, Error> {
        let mut predicates = Vec::new();
        while self.peek() == Tok::LBracket {
            self.bump();
            predicates.try_push(self.expr()?)?;
            self.expect(Tok::RBracket, ErrorKind::InvalidPredicate)?;
        }
        self.list(&predicates)
    }

    fn primary(&mut self) -> Result<u32, Error> {
        let offset = self.tokens[self.position].1 as usize;
        match self.bump() {
            Tok::Variable(name) => self.push(Expr::Variable(name)),
            Tok::Literal(text) => self.push(Expr::Literal(text)),
            Tok::Number(value) => self.push(Expr::Number(value)),
            Tok::LParen => {
                let expr = self.expr()?;
                self.expect(Tok::RParen, ErrorKind::Unclosed)?;
                Ok(expr)
            }
            Tok::Name(name) => {
                self.bump();
                let mut args = Vec::new();
                if self.peek() != Tok::RParen {
                    args.try_push(self.expr()?)?;
                    while self.peek() == Tok::Comma {
                        self.bump();
                        args.try_push(self.expr()?)?;
                    }
                }
                self.expect(Tok::RParen, ErrorKind::Unclosed)?;
                let core = CORE
                    .iter()
                    .find(|core| name.prefix.is_none() && core.0 == name.local.text(self.source));
                let function = match core {
                    Some(&(_, _, min, max)) if !(min..=max).contains(&args.len()) => {
                        return Err(Error::at(ErrorKind::InvalidArity, offset));
                    }
                    Some(&(_, core, _, _)) => Function::Core(core),
                    None => Function::Extension(name),
                };
                let args = self.list(&args)?;
                self.push(Expr::Call(function, args))
            }
            _ => Err(Error::at(ErrorKind::Expression, offset)),
        }
    }
}
