//! XPath patterns, libxml2's `pattern.c`.
//!
//! [`Pattern::compile`] (`xmlPatterncompile`) accepts steps joined by `/`
//! and `//`, `.`, `*`, `p:*`, `p:n`, `@a`, `child::`/`attribute::` and `|`. A
//! pattern matches nodes of either DOM, or element events incrementally
//! through a [`StreamMatcher`] (`xmlStream*`) in memory bounded by
//! [`Limits`].
//!
//! ```
//! use yaxml::{Document, pattern::Pattern};
//!
//! let document = Document::parse("<r><a><b/></a><b/></r>")?;
//! let pattern = Pattern::compile("a/b", &[], 0)?;
//! let hits = document.nodes().filter(|n| pattern.matches(&document, n.id())).count();
//! assert_eq!(hits, 1);
//!
//! let mut stream = pattern.stream().expect("streamable");
//! assert_eq!(stream.push("r", None), Ok(false));
//! assert_eq!(stream.push("a", None), Ok(false));
//! assert_eq!(stream.push("b", None), Ok(true));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;
use std::sync::Arc;

use crate::name::{name_char, name_start};
use crate::xpath::tree::{Item, QName, XML_NAMESPACE};
use crate::xpath::{Node, Tree};

/// `XML_PATTERN_XPATH`: standard XPath semantics (no implicit `//`).
pub const XPATH: u32 = 1;
/// `XML_PATTERN_XSSEL`: XSD identity-constraint selector (no attribute axis).
pub const XS_SELECTOR: u32 = 2;
/// `XML_PATTERN_XSFIELD`: XSD identity-constraint field.
pub const XS_FIELD: u32 = 4;

const NOT_PATTERN: u32 = XPATH | XS_SELECTOR | XS_FIELD;
const FROM_ROOT: u32 = 1 << 8;
const FROM_CUR: u32 = 1 << 9;

/// A ceiling on compilation or streaming state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Limit {
    /// [`Limits::max_bytes`].
    Bytes,
    /// [`Limits::max_steps`].
    Steps,
    /// [`Limits::max_depth`].
    Depth,
}

/// What went wrong.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    /// The pattern is malformed.
    Syntax,
    /// A stream's states disagree with its level (libxml2's `-1`).
    State,
    /// A name test uses a prefix with no binding.
    UnboundPrefix,
    /// A [`Limits`] ceiling was reached.
    LimitExceeded(Limit),
}

/// A compilation or streaming error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Error {
    /// What went wrong.
    pub kind: ErrorKind,
    /// The byte offset in the pattern; `None` for streaming errors.
    pub offset: Option<usize>,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.offset {
            Some(offset) => write!(f, "pattern error {:?} at byte {offset}", self.kind),
            None => write!(f, "pattern error {:?}", self.kind),
        }
    }
}

impl std::error::Error for Error {}

/// Finite ceilings. State of a [`StreamMatcher`] is at most
/// `steps * depth` entries per alternative.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Limits {
    /// Pattern source bytes.
    pub max_bytes: usize,
    /// Compiled steps per `|` alternative.
    pub max_steps: usize,
    /// Open elements a stream accepts.
    pub max_depth: usize,
}

setters!(const Limits { max_bytes: usize, max_steps: usize, max_depth: usize });

impl Limits {
    /// The defaults: 64 KiB of source, 256 steps and depth 1024.
    pub const DEFAULT: Self = Self {
        max_bytes: 64 * 1024,
        max_steps: 256,
        max_depth: 1024,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

const fn error(kind: ErrorKind, offset: Option<usize>) -> Error {
    Error { kind, offset }
}

/// A name test: `None` name is `*`; a `None` namespace is "no namespace",
/// except that `*` alone accepts any.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Test {
    name: Option<String>,
    ns: Option<String>,
}

impl Test {
    fn matches(&self, name: &str, ns: Option<&str>) -> bool {
        match &self.name {
            None => self.ns.as_deref().is_none_or(|uri| ns == Some(uri)),
            Some(own) => own == name && self.ns.as_deref() == ns,
        }
    }
}

/// Compiled operations in source order. `Elem(Test::default())` is `*`; `p:*`
/// is an `Elem` with only a namespace.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Op {
    Root,
    This,
    Elem(Test),
    Attr(Test),
    Parent,
    Ancestor,
    /// `//.` at the end, introduced by [`simplify`].
    AncestorOrSelf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Element,
    Attribute,
    /// Text, CDATA, comment or processing instruction; as a step, any node.
    Other,
}

#[derive(Clone, Debug)]
struct Step {
    test: Test,
    kind: Kind,
    desc: bool,
    fin: bool,
    root: bool,
    in_set: bool,
}

impl Step {
    fn new(test: Test, kind: Kind, desc: bool) -> Self {
        Self {
            test,
            kind,
            desc,
            fin: false,
            root: false,
            in_set: false,
        }
    }

    fn matches(&self, name: &str, ns: Option<&str>) -> bool {
        self.kind == Kind::Other || self.test.matches(name, ns)
    }
}

/// `xmlStreamComp`.
#[derive(Clone, Debug)]
struct Stream {
    steps: Vec<Step>,
    from_root: bool,
    desc: bool,
    any_node: bool,
}

/// One `|` alternative: operations reversed for matching.
#[derive(Clone, Debug)]
struct Alt {
    flags: u32,
    ops: Vec<Op>,
    stream: Option<Arc<Stream>>,
}

/// A compiled pattern, reusable across documents and threads.
#[derive(Clone, Debug)]
pub struct Pattern {
    alts: Vec<Alt>,
    limits: Limits,
}

// ---- compilation -----------------------------------------------------------

const fn is_blank(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    namespaces: &'a [(&'a str, &'a str)],
    flags: u32,
    max_steps: usize,
    ops: Vec<Op>,
}

type Parse = Result<(), Error>;

impl<'a> Parser<'a> {
    fn cur(&self) -> u8 {
        self.at(0)
    }

    fn at(&self, n: usize) -> u8 {
        self.src.as_bytes().get(self.pos + n).copied().unwrap_or(0)
    }

    fn fail<T>(&self, kind: ErrorKind) -> Result<T, Error> {
        Err(error(kind, Some(self.pos)))
    }

    fn syntax<T>(&self) -> Result<T, Error> {
        self.fail(ErrorKind::Syntax)
    }

    fn skip_blanks(&mut self) {
        while is_blank(self.cur()) {
            self.pos += 1;
        }
    }

    fn push(&mut self, op: Op) -> Parse {
        if self.ops.len() >= self.max_steps {
            return self.fail(ErrorKind::LimitExceeded(Limit::Steps));
        }
        self.ops.push(op);
        Ok(())
    }

    /// `xmlPatScanName` (`colon`) or `xmlPatScanNCName`, after blanks.
    fn name(&mut self, colon: bool) -> Option<&'a str> {
        self.skip_blanks();
        let rest = &self.src[self.pos..];
        let mut chars = rest.char_indices();
        let end = match chars.next() {
            Some((_, first)) if name_start(first, colon) => chars
                .find(|&(_, c)| !name_char(c, colon))
                .map_or(rest.len(), |(i, _)| i),
            _ => return None,
        };
        self.pos += end;
        Some(&rest[..end])
    }

    fn uri(&self, prefix: &str) -> Result<String, Error> {
        match prefix {
            "xml" => Ok(XML_NAMESPACE.into()),
            _ => self
                .namespaces
                .iter()
                .find(|&&(bound, _)| bound == prefix)
                .map(|&(_, uri)| uri.into())
                .ok_or_else(|| error(ErrorKind::UnboundPrefix, Some(self.pos))),
        }
    }

    /// After `prefix:`: the local name (`None` for `*`) and the prefix's URI.
    fn qualified(&mut self, prefix: &str) -> Result<Test, Error> {
        if is_blank(self.cur()) {
            return self.syntax();
        }
        let name = self.name(true);
        let ns = Some(self.uri(prefix)?);
        if name.is_none() && self.cur() != b'*' {
            return self.syntax();
        }
        self.pos += usize::from(name.is_none());
        Ok(Test {
            name: name.map(Into::into),
            ns,
        })
    }

    /// `xmlCompileAttributeTest`.
    fn attribute(&mut self) -> Parse {
        self.skip_blanks();
        let test = match self.name(false) {
            None if self.cur() == b'*' => {
                self.pos += 1;
                Test::default()
            }
            None => return self.syntax(),
            Some(prefix) if self.cur() == b':' => {
                self.pos += 1;
                self.qualified(prefix)?
            }
            Some(name) => Test {
                name: Some(name.into()),
                ns: None,
            },
        };
        self.push(Op::Attr(test))
    }

    /// `xmlCompileStepPattern`.
    fn step(&mut self) -> Parse {
        self.skip_blanks();
        match self.cur() {
            b'.' => {
                self.pos += 1;
                return self.push(Op::This);
            }
            b'@' if self.flags & XS_SELECTOR != 0 => return self.syntax(),
            b'@' => {
                self.pos += 1;
                return self.attribute();
            }
            _ => {}
        }
        let Some(name) = self.name(false) else {
            if self.cur() != b'*' {
                return self.syntax();
            }
            self.pos += 1;
            return self.push(Op::Elem(Test::default()));
        };
        let blanks = is_blank(self.cur());
        self.skip_blanks();
        match self.cur() {
            b':' if self.at(1) != b':' => {
                self.pos += 1;
                if blanks {
                    return self.syntax();
                }
                let test = self.qualified(name)?;
                self.push(Op::Elem(test))
            }
            b':' => {
                self.pos += 2;
                match name {
                    "child" => self.child(),
                    "attribute" if self.flags & XS_SELECTOR == 0 => self.attribute(),
                    _ => self.syntax(),
                }
            }
            b'*' => self.syntax(),
            _ => self.push(Op::Elem(Test {
                name: Some(name.into()),
                ns: None,
            })),
        }
    }

    /// The node test after `child::`. `xmlPatScanName` takes colons, so as in
    /// libxml2 a prefix is part of the name.
    fn child(&mut self) -> Parse {
        let test = match self.name(true) {
            None if self.cur() == b'*' => {
                self.pos += 1;
                Test::default()
            }
            None => return self.syntax(),
            Some(name) => Test {
                name: Some(name.into()),
                ns: None,
            },
        };
        self.push(Op::Elem(test))
    }

    fn unfinished(&self) -> Parse {
        if self.cur() == 0 {
            self.syntax()
        } else {
            Ok(())
        }
    }

    /// `xmlCompilePathPattern`; returns the flags it adds.
    fn path(&mut self) -> Result<u32, Error> {
        self.skip_blanks();
        let flags = match self.cur() {
            b'/' => FROM_ROOT,
            b'.' => FROM_CUR,
            _ if self.flags & NOT_PATTERN != 0 => FROM_CUR,
            _ => 0,
        };
        if self.cur() == b'/' && self.at(1) == b'/' {
            self.push(Op::Ancestor)?;
            self.pos += 2;
        } else if self.cur() == b'.' && self.at(1) == b'/' && self.at(2) == b'/' {
            self.push(Op::Ancestor)?;
            self.pos += 3;
            self.skip_blanks();
            self.unfinished()?;
        }
        if self.cur() == b'@' {
            self.pos += 1;
            self.attribute()?;
            self.skip_blanks();
        } else {
            if self.cur() == b'/' {
                self.push(Op::Root)?;
                self.pos += 1;
                self.skip_blanks();
                self.unfinished()?;
            }
            self.step()?;
            self.skip_blanks();
            while self.cur() == b'/' {
                if self.at(1) == b'/' {
                    self.push(Op::Ancestor)?;
                    self.pos += 2;
                } else {
                    self.push(Op::Parent)?;
                    self.pos += 1;
                    self.skip_blanks();
                    self.unfinished()?;
                }
                self.skip_blanks();
                self.step()?;
            }
        }
        match self.cur() {
            0 => Ok(flags),
            _ => self.syntax(),
        }
    }

    /// `xmlCompileIDCXPathPath`: `.`, `./`, `.//` and relative steps only.
    fn idc_path(&mut self) -> Result<u32, Error> {
        self.skip_blanks();
        if self.cur() == b'/' {
            return self.syntax();
        }
        if self.cur() == b'.' {
            self.pos += 1;
            self.skip_blanks();
            if self.cur() == 0 {
                self.push(Op::This)?;
                return Ok(FROM_CUR);
            }
            if self.cur() != b'/' {
                return self.syntax();
            }
            self.pos += 1;
            self.skip_blanks();
            if self.cur() == b'/' {
                if is_blank(self.src.as_bytes()[self.pos - 1]) {
                    return self.syntax();
                }
                self.push(Op::Ancestor)?;
                self.pos += 1;
                self.skip_blanks();
            }
            self.unfinished()?;
        }
        loop {
            self.step()?;
            self.skip_blanks();
            if self.cur() != b'/' {
                break;
            }
            self.push(Op::Parent)?;
            self.pos += 1;
            self.skip_blanks();
            if self.cur() == b'/' {
                return self.syntax();
            }
            self.unfinished()?;
        }
        match self.cur() {
            0 => Ok(FROM_CUR),
            _ => self.syntax(),
        }
    }
}

/// `xmlStreamCompile` over source-order operations; `None` when the
/// alternative cannot be streamed.
fn stream_compile(ops: &[Op], flags: u32) -> Option<Stream> {
    if ops == [Op::This] {
        return Some(Stream {
            steps: Vec::new(),
            from_root: false,
            desc: false,
            any_node: true,
        });
    }
    let (mut steps, mut root, mut desc, mut any_node) = (Vec::<Step>::new(), false, false, false);
    let (mut pending_desc, mut prev) = (false, None);
    for (i, op) in ops.iter().enumerate() {
        let (test, kind) = match op {
            Op::Root if i == 0 => {
                root = true;
                continue;
            }
            Op::Root => return None,
            Op::Parent | Op::AncestorOrSelf => continue,
            Op::Ancestor => {
                desc = true;
                pending_desc = true;
                continue;
            }
            Op::This if pending_desc && i + 1 == ops.len() => {
                any_node = true;
                if let Some(step) = prev.take().and_then(|p: usize| steps.get_mut(p)) {
                    step.in_set = true;
                }
                (Test::default(), Kind::Other)
            }
            Op::This => continue,
            Op::Elem(test) => (test.clone(), Kind::Element),
            Op::Attr(test) => (test.clone(), Kind::Attribute),
        };
        prev = (kind == Kind::Element).then_some(steps.len());
        steps.push(Step::new(test, kind, pending_desc));
        pending_desc = false;
    }
    let pattern_like = !root && flags & NOT_PATTERN == 0;
    if pattern_like {
        desc = true;
        if let Some(first) = steps.first_mut() {
            first.desc = true;
        }
    }
    steps.last_mut()?.fin = true;
    if root {
        steps[0].root = true;
    }
    Some(Stream {
        steps,
        from_root: flags & FROM_ROOT != 0,
        desc,
        any_node,
    })
}

/// Drops `.` steps, as the streaming compiler does: libxml2's matcher would
/// read `a/.` as "a child of `a`" where XPath (and its stream) mean `a`.
/// `x//.` becomes ancestor-or-self.
fn simplify(ops: Vec<Op>) -> Vec<Op> {
    let mut out = Vec::with_capacity(ops.len());
    let mut rest = ops.into_iter().peekable();
    while let Some(op) = rest.next() {
        if op == Op::Ancestor && out.last() == Some(&Op::Ancestor) {
            continue;
        }
        if op == Op::This && !(out.is_empty() && rest.peek().is_none()) {
            match (out.last(), rest.peek()) {
                (_, Some(Op::Parent)) => {
                    rest.next();
                    continue;
                }
                (Some(Op::Parent | Op::Root), Some(Op::Ancestor)) | (Some(Op::Parent), None) => {
                    out.pop();
                    continue;
                }
                (Some(Op::Ancestor), Some(Op::Ancestor)) => continue,
                (Some(Op::Ancestor), None) => {
                    out.pop();
                    out.push(Op::AncestorOrSelf);
                    continue;
                }
                _ => {}
            }
        }
        out.push(op);
    }
    out
}

impl Pattern {
    /// Compiles `source`, binding `namespaces` as `(prefix, uri)`; `xml` is
    /// predefined and the default namespace cannot be bound. `flags` is zero
    /// (`XML_PATTERN_DEFAULT`) or a combination of [`XPATH`], [`XS_SELECTOR`]
    /// and [`XS_FIELD`]. Text after a NUL byte is ignored, as in C.
    pub fn compile(source: &str, namespaces: &[(&str, &str)], flags: u32) -> Result<Self, Error> {
        Self::compile_with_limits(source, namespaces, flags, Limits::default())
    }

    /// [`Pattern::compile`] with explicit `limits`.
    pub fn compile_with_limits(
        source: &str,
        namespaces: &[(&str, &str)],
        flags: u32,
        limits: Limits,
    ) -> Result<Self, Error> {
        let source = source.split('\0').next().unwrap_or_default();
        if source.len() > limits.max_bytes {
            return Err(error(
                ErrorKind::LimitExceeded(Limit::Bytes),
                Some(limits.max_bytes),
            ));
        }
        let mut parts: Vec<_> = source
            .split('|')
            .scan(0, |start, part| {
                let at = *start;
                *start += part.len() + 1;
                Some((at, part))
            })
            .collect();
        if parts.last().is_some_and(|&(_, part)| part.is_empty()) {
            parts.pop();
        }
        if parts.is_empty() {
            return Err(error(ErrorKind::Syntax, Some(0)));
        }
        let mut alts = parts
            .into_iter()
            .map(|(at, part)| {
                let mut parser = Parser {
                    src: part,
                    pos: 0,
                    namespaces,
                    flags,
                    max_steps: limits.max_steps,
                    ops: Vec::new(),
                };
                let added = if flags & (XS_SELECTOR | XS_FIELD) != 0 {
                    parser.idc_path()
                } else {
                    parser.path()
                }
                .map_err(|e| error(e.kind, e.offset.map(|o| o + at)))?;
                let flags = flags | added;
                let stream = stream_compile(&parser.ops, flags).map(Arc::new);
                let ops = simplify(parser.ops);
                let skip = usize::from(ops.first() == Some(&Op::Ancestor));
                Ok(Alt {
                    flags,
                    ops: ops.into_iter().skip(skip).rev().collect(),
                    stream,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        // libxml2 streams only if no alternative mixes root and current anchors.
        let anchors: Vec<_> = alts
            .iter()
            .map(|alt| alt.flags & (FROM_ROOT | FROM_CUR))
            .collect();
        let first = anchors.iter().copied().find(|&a| a != 0).unwrap_or(0);
        let streamable = anchors.iter().all(|&a| a == 0 || a == first);
        if !streamable {
            alts.iter_mut().for_each(|alt| alt.stream = None);
        }
        Ok(Self { alts, limits })
    }

    /// `xmlPatternMatch`: whether `node` (an element, or an attribute of one)
    /// matches any alternative. Unlike libxml2, an attribute test also requires
    /// the same namespace, `.` steps are dropped and `//` before `*` or `p:*`
    /// searches ancestors: all as the streaming matcher does.
    pub fn matches<T: Tree>(&self, tree: &T, node: impl Into<Node<T::Id>>) -> bool {
        let chain: Vec<_> =
            std::iter::successors(Some(node.into()), |&n| parent(tree, n)).collect();
        self.alts.iter().any(|alt| walk(tree, &alt.ops, &chain))
    }

    /// `xmlPatternGetStreamCtxt`, already at the document node as after
    /// `xmlStreamPush(NULL, NULL)`; `None` if any alternative is not streamable.
    pub fn stream(&self) -> Option<StreamMatcher> {
        let contexts = self
            .alts
            .iter()
            .map(|alt| {
                alt.stream.as_ref().map(|stream| Context {
                    stream: Arc::clone(stream),
                    flags: alt.flags,
                    cursor: Cursor::default(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let mut matcher = StreamMatcher {
            contexts,
            max_depth: self.limits.max_depth,
        };
        matcher.reset();
        Some(matcher)
    }
}

// ---- DOM matching ----------------------------------------------------------

fn parent<T: Tree>(tree: &T, node: Node<T::Id>) -> Option<Node<T::Id>> {
    match node {
        Node::Tree(id) => tree.parent(id).map(Node::Tree),
        Node::Attribute(owner, _) => Some(Node::Tree(owner)),
        Node::Namespace(..) => None,
    }
}

/// Whether `node` is an element (or attribute, per `attribute`) accepted by `test`.
fn accepts<T: Tree>(tree: &T, node: Node<T::Id>, test: &Test, attribute: bool) -> bool {
    let name: Option<QName<'_>> = match node {
        Node::Tree(id) if !attribute => match tree.item(id) {
            Item::Element(name) => Some(name),
            _ => None,
        },
        Node::Attribute(owner, index) if attribute => tree
            .attribute(owner, index)
            .map(|(name, _)| name)
            .filter(|name| !name.is_namespace_declaration()),
        _ => None,
    };
    name.is_some_and(|name| test.matches(name.local, name.namespace))
}

/// Forward set evaluation over the node's ancestor chain (`chain[0]` is the
/// node): `cur` holds the chain positions the operations so far can end on.
/// Operations are reversed: the last step first, then back towards the root.
fn walk<T: Tree>(tree: &T, ops: &[Op], chain: &[Node<T::Id>]) -> bool {
    let len = chain.len();
    let up = |cur: Vec<usize>| cur.into_iter().map(|j| j + 1).filter(move |&j| j < len);
    let keep = |cur: Vec<usize>, test: &Test, attribute| {
        cur.into_iter()
            .filter(|&j| accepts(tree, chain[j], test, attribute))
            .collect::<Vec<_>>()
    };
    let end = ops.iter().fold(vec![0], |cur, op| match op {
        Op::This => keep(cur, &Test::default(), false),
        Op::Elem(test) => keep(cur, test, false),
        Op::Attr(test) => keep(cur, test, true),
        Op::Parent => up(cur).collect(),
        Op::Root => up(cur)
            .filter(|&j| matches!(chain[j], Node::Tree(id) if matches!(tree.item(id), Item::Root)))
            .collect(),
        Op::Ancestor | Op::AncestorOrSelf => cur.into_iter().min().map_or_else(Vec::new, |j| {
            (j + usize::from(*op == Op::Ancestor)..len).collect()
        }),
    });
    !end.is_empty()
}

// ---- streaming -------------------------------------------------------------

/// `xmlStreamCtxt` for one alternative.
#[derive(Clone, Debug)]
struct Context {
    stream: Arc<Stream>,
    flags: u32,
    cursor: Cursor,
}

/// The mutable matching state of a context; its methods take the immutable
/// `Stream` so no per-event reference counting is needed.
#[derive(Clone, Debug, Default)]
struct Cursor {
    /// `(step, level)`, levels nondecreasing, no duplicates.
    states: Vec<(usize, usize)>,
    level: usize,
    block: Option<usize>,
}

impl Cursor {
    fn add(&mut self, step: usize, level: usize) {
        let seen = self
            .states
            .iter()
            .rev()
            .take_while(|&&(_, at)| at == level)
            .any(|&(s, _)| s == step);
        if !seen {
            self.states.push((step, level));
        }
    }

    /// The document node (or a reset); whether it matches.
    fn reset(&mut self, stream: &Stream) -> bool {
        self.states.clear();
        (self.level, self.block) = (0, None);
        let steps = &stream.steps;
        match steps.first() {
            _ if !stream.from_root => false,
            None => true,
            // libxml2 adds no state for a lone `/.//.`, so it selects nothing below the document.
            Some(first) => {
                if first.root {
                    self.add(0, 0);
                }
                steps.len() == 1 && first.kind == Kind::Other && first.desc
            }
        }
    }

    /// Step `nr` against the node being entered at `level`; whether it selects it.
    fn visit(
        &mut self,
        stream: &Stream,
        nr: usize,
        name: &str,
        ns: Option<&str>,
        level: usize,
    ) -> bool {
        let step = &stream.steps[nr];
        let hit = step.matches(name, ns);
        if hit && !step.fin {
            self.add(nr + 1, level);
        }
        if !stream.desc && (!hit || step.fin) {
            self.block = Some(level);
        }
        hit && (step.fin || step.in_set)
    }

    fn push(
        &mut self,
        stream: &Stream,
        flags: u32,
        kind: Kind,
        name: &str,
        ns: Option<&str>,
        depth: usize,
    ) -> Result<bool, Error> {
        let new = self.level + 1;
        if new > depth {
            return Err(error(ErrorKind::LimitExceeded(Limit::Depth), None));
        }
        let notpattern = flags & NOT_PATTERN != 0;
        let idc = flags & (XS_SELECTOR | XS_FIELD) != 0;
        if stream.steps.is_empty() {
            if flags & XPATH != 0 {
                return Ok(false);
            }
            self.level = new;
            return Ok(kind != Kind::Attribute && (!notpattern || new == 1));
        }
        if self.block.is_some() || (kind == Kind::Other && !stream.any_node) {
            self.level = new;
            return Ok(false);
        }
        let mut hit = false;
        let live = if stream.desc {
            0
        } else {
            self.states.len().saturating_sub(1)
        };
        for i in live..self.states.len() {
            let (nr, at) = self.states[i];
            let step = &stream.steps[nr];
            if !stream.desc && at < self.level {
                return Err(error(ErrorKind::State, None));
            }
            if stream.desc && (at > self.level || (at < self.level && !step.desc)) {
                continue;
            }
            if step.kind != kind {
                if step.kind == Kind::Attribute {
                    if !stream.desc {
                        self.block = Some(new);
                    }
                    continue;
                } else if step.kind != Kind::Other {
                    continue;
                }
            }
            hit |= self.visit(stream, nr, name, ns, new);
        }
        self.level = new;
        // Re-enter the expression unless it is anchored at the root.
        let first = &stream.steps[0];
        let compare =
            !notpattern || (new == 1 && !idc) || (new != 1 && (first.desc || (new == 2 && idc)));
        let kind_ok = first.kind == kind || (kind != Kind::Attribute && first.kind == Kind::Other);
        Ok(if first.root || !compare || !kind_ok {
            hit
        } else {
            self.visit(stream, 0, name, ns, new) | hit
        })
    }

    fn pop(&mut self) {
        if self.block == Some(self.level) {
            self.block = None;
        }
        self.level = self.level.saturating_sub(1);
        while self.states.last().is_some_and(|&(_, at)| at > self.level) {
            self.states.pop();
        }
    }
}

/// Incremental matcher (`xmlStreamCtxt`): feed element starts, attributes and
/// ends in document order; memory is bounded by the pattern and the nesting
/// depth, never by the document. Names are local names and namespace URIs
/// (an empty URI is no namespace).
#[derive(Clone, Debug)]
pub struct StreamMatcher {
    contexts: Vec<Context>,
    max_depth: usize,
}

impl StreamMatcher {
    /// Returns to the document node (`xmlStreamPush(NULL, NULL)`); whether the
    /// document itself matches (`/.` or `//.`).
    pub fn reset(&mut self) -> bool {
        self.contexts
            .iter_mut()
            .fold(false, |any, c| c.cursor.reset(&c.stream) | any)
    }

    fn feed(&mut self, kind: Kind, name: &str, ns: Option<&str>) -> Result<bool, Error> {
        let ns = ns.filter(|uri| !uri.is_empty());
        self.contexts.iter_mut().try_fold(false, |any, c| {
            Ok(c.cursor
                .push(&c.stream, c.flags, kind, name, ns, self.max_depth)?
                | any)
        })
    }

    /// `xmlStreamPush`: an element starts; whether it matches. Pair with [`pop`](Self::pop).
    pub fn push(&mut self, local: &str, ns: Option<&str>) -> Result<bool, Error> {
        self.feed(Kind::Element, local, ns)
    }

    /// `xmlStreamPushAttr`: an attribute of the current element; pair with [`pop`](Self::pop).
    pub fn push_attr(&mut self, local: &str, ns: Option<&str>) -> Result<bool, Error> {
        self.feed(Kind::Attribute, local, ns)
    }

    /// `xmlStreamPushNode` for text, CDATA, comment and PI; only needed if
    /// [`wants_any_node`](Self::wants_any_node).
    pub fn push_other(&mut self) -> Result<bool, Error> {
        self.feed(Kind::Other, "", None)
    }

    /// `xmlStreamPop`: the element, attribute or node last pushed ends.
    pub fn pop(&mut self) {
        self.contexts.iter_mut().for_each(|c| c.cursor.pop());
    }

    /// `xmlStreamWantsAnyNode`: whether non-element nodes must be pushed too.
    pub fn wants_any_node(&self) -> bool {
        self.contexts.iter().any(|c| c.stream.any_node)
    }
}
