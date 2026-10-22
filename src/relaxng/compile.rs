//! From [`El`] trees to interned patterns: RELAX NG sections 4.12 to 4.21
//! (child grouping, `mixed`, `optional`, `zeroOrMore`, name class
//! constraints, `combine`, grammars, `ref`/`parentRef`, inlined defines,
//! `notAllowed` and `empty` simplification) and the datatypes.

use std::collections::{HashMap, TryReserveError};
use std::sync::Arc;

use super::datatype::{Datatype, Problem, ValueDef};
use super::load::{El, Loader, Loc};
use super::pattern::{Arena, EMPTY, Id, NOT_ALLOWED, NameClass, Pat, Store, TEXT, Tables};
use super::{SchemaCode, SchemaError, check};
use crate::fallible::{TryPush, cloned};
use crate::name::is_ncname;
use crate::schema_types::Type;
use crate::xpath::tree::XML_NAMESPACE;

/// The namespace section 4.16 reserves, and the XML namespaces one.
const XMLNS_NAMESPACES: [&str; 2] = [
    "http://www.w3.org/2000/xmlns",
    "http://www.w3.org/2000/xmlns/",
];
/// Nesting of patterns, counting inlined definitions; compilation recurses this deep.
const MAX_DEPTH: usize = 256;

struct Grammar<'e> {
    parent: Option<usize>,
    defines: HashMap<&'e str, usize>,
    start: Option<usize>,
}

#[derive(Clone, Copy)]
enum State {
    Todo,
    Busy,
    Done(Id),
}

/// A `define` (or `start`) of one grammar with all its combined parts.
struct Define<'e> {
    name: &'e str,
    parts: Vec<&'e El>,
    combine: Option<&'e str>,
    uncombined: usize,
    grammar: usize,
    state: State,
}

struct Compiler<'e, 'l> {
    loader: &'l Loader<'l>,
    store: Store,
    tables: Tables,
    /// The schema location that first produced each pattern.
    origin: Vec<Loc>,
    grammars: Vec<Grammar<'e>>,
    defines: Vec<Define<'e>>,
    /// Element patterns whose content is still to be compiled.
    pending: Vec<(usize, &'e [El], Option<usize>)>,
    depth: usize,
    /// False once only unreachable definitions remain, whose reference
    /// cycles section 4.19 discards rather than rejects.
    reachable: bool,
}

type Result<T> = std::result::Result<T, SchemaError>;

/// Compiles a loaded schema, then checks the section 7 restrictions.
pub(super) fn compile(
    root: &El,
    loader: &Loader<'_>,
    max_work: usize,
) -> std::result::Result<(Arena, Tables, Id), Vec<SchemaError>> {
    let mut compiler = Compiler {
        loader,
        store: Store::new(Arc::default()),
        tables: Tables::default(),
        origin: Vec::new(),
        grammars: Vec::new(),
        defines: Vec::new(),
        pending: Vec::new(),
        depth: 0,
        reachable: true,
    };
    let start = compiler
        .schema(root, max_work)
        .map_err(|error| vec![error])?;
    let errors = check::restrictions(
        &compiler.store,
        &compiler.tables,
        start,
        max_work.saturating_sub(compiler.store.work),
    );
    if !errors.is_empty() {
        let at = |id: Id| compiler.origin.get(id as usize).copied().unwrap_or(root.at);
        return Err(errors
            .into_iter()
            .map(|(code, id, message)| loader.error(code, at(id), message))
            .collect());
    }
    compiler.tables.ids = id_attributes(&compiler.store, &compiler.tables)
        .map_err(|_| vec![compiler.refused(root)])?;
    Ok((compiler.store.into_arena(), compiler.tables, start))
}

/// The attributes typed `xsd:ID`, `xsd:IDREF` or `xsd:IDREFS` of each element
/// (the RELAX NG DTD compatibility ID feature, as libxml2 implements it).
fn id_attributes(
    store: &Store,
    tables: &Tables,
) -> std::result::Result<Vec<(u32, u32, Type)>, TryReserveError> {
    let mut found = Vec::new();
    for &(element, content) in &tables.elements {
        let mut stack = vec![content];
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = stack.pop() {
            seen.try_reserve(1)?;
            stack.try_reserve(2)?;
            if !seen.insert(id) {
                continue;
            }
            match store.pat(id) {
                Pat::Choice(a, b) | Pat::Group(a, b) | Pat::Interleave(a, b) => {
                    stack.extend([a, b])
                }
                Pat::OneOrMore(a) => stack.push(a),
                Pat::Attribute(class, value) => {
                    if let Pat::Data(datatype, _) = store.pat(value)
                        && let Datatype::Xsd(ty @ (Type::Id | Type::IdRef | Type::IdRefs), _) =
                            tables.datatypes[datatype as usize]
                    {
                        found.try_push((element, class, ty))?;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(found)
}

impl<'e> Compiler<'e, '_> {
    fn error(&self, code: SchemaCode, el: &El, message: String) -> SchemaError {
        self.loader.error(code, el.at, message)
    }

    /// A refused allocation while compiling `el`.
    fn refused(&self, el: &El) -> SchemaError {
        self.error(
            SchemaCode::Allocation,
            el,
            "Memory allocation failed".into(),
        )
    }

    fn schema(&mut self, root: &'e El, max_work: usize) -> Result<Id> {
        let start = match root.name.as_str() {
            "grammar" => self.grammar_start(root, None)?,
            _ => self.pattern(root, None)?,
        };
        let mut next_define = 0;
        loop {
            if self.store.refused {
                return Err(self.refused(root));
            }
            if self.store.work > max_work {
                return Err(self.error(
                    SchemaCode::ResourceLimit,
                    root,
                    "schema work limit exceeded".into(),
                ));
            }
            if let Some((element, body, grammar)) = self.pending.pop() {
                let content = self.group(body, grammar, &body[0])?;
                self.tables.elements[element].1 = content;
            } else if next_define < self.defines.len() {
                self.reachable &= !matches!(self.defines[next_define].state, State::Todo);
                let part = self.defines[next_define].parts[0];
                self.define(next_define, part)?;
                next_define += 1;
            } else {
                return Ok(start);
            }
        }
    }

    /// Compiles a pattern; each kind has its own function so that the
    /// recursion, which nests as deep as the schema, keeps small frames.
    fn pattern(&mut self, el: &'e El, grammar: Option<usize>) -> Result<Id> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error(
                SchemaCode::ResourceLimit,
                el,
                "patterns are nested too deeply".into(),
            ));
        }
        let id = match el.name.as_str() {
            "element" => self.element(el, grammar),
            "attribute" => self.attribute(el, grammar),
            "empty" => self.leaf(el, EMPTY),
            "text" => self.leaf(el, TEXT),
            "notAllowed" => self.leaf(el, NOT_ALLOWED),
            "ref" | "parentRef" => self.reference(el, grammar),
            "grammar" => self.grammar_start(el, grammar),
            "data" => self.data(el, grammar),
            "value" => self.value(el),
            _ => self.combination(el, grammar),
        }?;
        self.depth -= 1;
        let missing = self.store.len() - self.origin.len();
        self.origin
            .try_reserve(missing)
            .map_err(|_| self.refused(el))?;
        self.origin.extend(std::iter::repeat_n(el.at, missing));
        Ok(id)
    }

    fn element(&mut self, el: &'e El, grammar: Option<usize>) -> Result<Id> {
        let (class, body) = self.named(el, false)?;
        if body.is_empty() {
            return Err(self.error(
                SchemaCode::ElementNoContent,
                el,
                "xmlRelaxNGParseElement: element has no content".into(),
            ));
        }
        let room = self.tables.elements.try_reserve(1);
        room.and(self.pending.try_reserve(1))
            .map_err(|_| self.refused(el))?;
        let index = self.tables.elements.len();
        self.tables.elements.push((class, NOT_ALLOWED));
        self.pending.push((index, body, grammar));
        Ok(self.store.make(Pat::Element(index as u32)))
    }

    fn attribute(&mut self, el: &'e El, grammar: Option<usize>) -> Result<Id> {
        let (class, body) = self.named(el, true)?;
        let content = match body {
            [] => TEXT,
            [one] => self.pattern(one, grammar)?,
            _ => {
                return Err(self.error(
                    SchemaCode::AttributeChildren,
                    el,
                    "attribute has multiple children".into(),
                ));
            }
        };
        Ok(self.store.make(Pat::Attribute(class, content)))
    }

    fn leaf(&self, el: &El, id: Id) -> Result<Id> {
        match el.children.is_empty() {
            true => Ok(id),
            false => Err(self.error(
                SchemaCode::EmptyNotEmpty,
                el,
                format!("element {} is not empty", el.name),
            )),
        }
    }

    /// Sections 4.12 to 4.15: the patterns that combine their children.
    fn combination(&mut self, el: &'e El, grammar: Option<usize>) -> Result<Id> {
        let combine = match el.name.as_str() {
            "choice" => Store::choice,
            "interleave" => Store::interleave,
            "group" | "optional" | "zeroOrMore" | "oneOrMore" | "list" | "mixed" => Store::group,
            name => {
                return Err(self.error(
                    SchemaCode::UnknownConstruct,
                    el,
                    format!("Unexpected node {name} is not a pattern"),
                ));
            }
        };
        let ids = self.patterns(&el.children, grammar, el)?;
        let p = fold(&mut self.store, &ids, combine);
        Ok(match el.name.as_str() {
            "optional" => self.store.choice(p, EMPTY),
            "zeroOrMore" => {
                let more = self.store.one_or_more(p);
                self.store.choice(more, EMPTY)
            }
            "oneOrMore" => self.store.one_or_more(p),
            "list" => self.store.make(Pat::List(p)),
            "mixed" => self.store.interleave(p, TEXT),
            _ => p,
        })
    }

    /// `ref` and `parentRef` (section 4.18).
    fn reference(&mut self, el: &'e El, grammar: Option<usize>) -> Result<Id> {
        self.leaf(el, EMPTY)?;
        let name = el.name.as_str();
        let target = (el.attr("name"))
            .ok_or_else(|| self.error(SchemaCode::RefNoName, el, format!("{name} has no name")))?;
        let scope = match name {
            "ref" => grammar,
            _ => Some(
                grammar
                    .and_then(|g| self.grammars[g].parent)
                    .ok_or_else(|| {
                        self.error(
                            SchemaCode::ParentRefNoParent,
                            el,
                            "Use of parentRef without a parent grammar".into(),
                        )
                    })?,
            ),
        };
        let define = scope
            .and_then(|g| self.grammars[g].defines.get(target).copied())
            .ok_or_else(|| {
                self.error(
                    SchemaCode::RefNoDef,
                    el,
                    format!("Reference {target} has no matching definition"),
                )
            })?;
        self.define(define, el)
    }

    fn patterns(&mut self, els: &'e [El], grammar: Option<usize>, parent: &El) -> Result<Vec<Id>> {
        if els.is_empty() {
            return Err(self.error(
                SchemaCode::EmptyConstruct,
                parent,
                format!("Element {} is empty", parent.name),
            ));
        }
        let mut ids = Vec::new();
        ids.try_reserve_exact(els.len())
            .map_err(|_| self.refused(parent))?;
        for el in els {
            ids.push(self.pattern(el, grammar)?);
        }
        Ok(ids)
    }

    /// Section 4.12: several patterns in a row form a group.
    fn group(&mut self, els: &'e [El], grammar: Option<usize>, parent: &El) -> Result<Id> {
        let ids = self.patterns(els, grammar, parent)?;
        Ok(fold(&mut self.store, &ids, Store::group))
    }

    /// The name class of an `element` or `attribute` and its pattern children.
    fn named(&mut self, el: &'e El, attribute: bool) -> Result<(u32, &'e [El])> {
        let (class, body) = match el.attr("name") {
            Some(qname) => {
                // Section 4.8: an attribute's name is unqualified unless it has `ns`.
                let ns = match attribute && !el.own_ns {
                    true => "",
                    false => &el.ns,
                };
                let (ns, local) = self.qname(qname, ns, el)?;
                self.tables
                    .names
                    .try_reserve(1)
                    .map_err(|_| self.refused(el))?;
                (
                    self.push_class(NameClass::Name(ns, local)),
                    &el.children[..],
                )
            }
            None => match el.children.split_first() {
                Some((first, body)) => (self.name_class(first, false, false)?, body),
                None => {
                    return Err(self.error(
                        SchemaCode::ElementEmpty,
                        el,
                        format!("xmlRelaxNGParse{}: {} has no children", el.name, el.name),
                    ));
                }
            },
        };
        if attribute && self.xmlns(class) {
            return Err(self.error(
                SchemaCode::XmlnsName,
                el,
                "Attribute with QName 'xmlns' or the xmlns namespace is not allowed".into(),
            ));
        }
        Ok((class, body))
    }

    /// Section 4.16: whether an attribute name class mentions the name
    /// `xmlns` or the xmlns namespace anywhere, `except` included.
    fn xmlns(&self, class: u32) -> bool {
        let reserved = |ns: &str| XMLNS_NAMESPACES.contains(&ns);
        match &self.tables.names[class as usize] {
            NameClass::Name(ns, local) => (ns.is_empty() && &**local == "xmlns") || reserved(ns),
            NameClass::NsName(ns, except) => reserved(ns) || except.is_some_and(|e| self.xmlns(e)),
            NameClass::AnyName(except) => except.is_some_and(|e| self.xmlns(e)),
            NameClass::Choice(a, b) => self.xmlns(*a) || self.xmlns(*b),
        }
    }

    /// Section 4.10: a QName to (namespace, local name); unprefixed names take `ns`.
    fn qname(&self, qname: &str, ns: &str, el: &El) -> Result<(Box<str>, Box<str>)> {
        let invalid = || {
            self.error(
                SchemaCode::ElementName,
                el,
                format!("name '{qname}' is not a QName"),
            )
        };
        let qname = qname.trim_matches([' ', '\t', '\n', '\r']);
        let (prefix, local) = match qname.split_once(':') {
            Some((prefix, local)) => (Some(prefix), local),
            None => (None, qname),
        };
        if !prefix.into_iter().chain([local]).all(is_ncname) {
            return Err(invalid());
        }
        let ns = match prefix {
            None => ns,
            Some("xml") => XML_NAMESPACE,
            Some(prefix) => el
                .scope
                .iter()
                .find(|(bound, uri)| bound == prefix && !uri.is_empty())
                .map(|(_, uri)| uri.as_str())
                .ok_or_else(|| {
                    self.error(
                        SchemaCode::PrefixUndefined,
                        el,
                        format!("xmlRelaxNGParse: no namespace for prefix {prefix}"),
                    )
                })?,
        };
        Ok((ns.into(), local.into()))
    }

    fn push_class(&mut self, class: NameClass) -> u32 {
        self.tables.names.push(class);
        self.tables.names.len() as u32 - 1
    }

    /// A name class; `any` and `ns` say whether we are inside the `except`
    /// of an `anyName` or an `nsName` (section 4.16).
    fn name_class(&mut self, el: &El, any: bool, ns: bool) -> Result<u32> {
        let class = match el.name.as_str() {
            "name" => {
                let (uri, local) = self.qname(&el.text, &el.ns, el)?;
                NameClass::Name(uri, local)
            }
            "anyName" if ns => {
                return Err(self.error(
                    SchemaCode::PatNsNameExceptAnyName,
                    el,
                    "Found nsName/except//anyName forbidden construct".into(),
                ));
            }
            "anyName" if any => {
                return Err(self.error(
                    SchemaCode::PatAnyNameExceptAnyName,
                    el,
                    "Found anyName/except//anyName forbidden construct".into(),
                ));
            }
            "anyName" => NameClass::AnyName(self.except_class(el, true, ns)?),
            "nsName" if ns => {
                return Err(self.error(
                    SchemaCode::PatNsNameExceptNsName,
                    el,
                    "Found nsName/except//nsName forbidden construct".into(),
                ));
            }
            "nsName" => NameClass::NsName(el.ns.as_ref().into(), self.except_class(el, any, true)?),
            "choice" if !el.children.is_empty() => {
                let classes = el
                    .children
                    .iter()
                    .map(|child| self.name_class(child, any, ns))
                    .collect::<Result<Vec<_>>>()?;
                let room = self.tables.names.try_reserve(classes.len());
                room.map_err(|_| self.refused(el))?;
                return Ok(fold(self, &classes, |this, a, b| {
                    this.push_class(NameClass::Choice(a, b))
                }));
            }
            other => {
                return Err(self.error(
                    SchemaCode::ElementName,
                    el,
                    format!("expecting name, anyName, nsName or choice : got {other}"),
                ));
            }
        };
        if el.name == "name" && !el.children.is_empty() {
            return Err(self.error(
                SchemaCode::ElementName,
                el,
                "name has element children".into(),
            ));
        }
        self.tables
            .names
            .try_reserve(1)
            .map_err(|_| self.refused(el))?;
        Ok(self.push_class(class))
    }

    fn except_class(&mut self, el: &El, any: bool, ns: bool) -> Result<Option<u32>> {
        match el.children.as_slice() {
            [] => Ok(None),
            [except] if except.name == "except" && !except.children.is_empty() => {
                let classes = except
                    .children
                    .iter()
                    .map(|child| self.name_class(child, any, ns))
                    .collect::<Result<Vec<_>>>()?;
                let room = self.tables.names.try_reserve(classes.len());
                room.map_err(|_| self.refused(el))?;
                Ok(Some(fold(self, &classes, |this, a, b| {
                    this.push_class(NameClass::Choice(a, b))
                })))
            }
            _ => Err(self.error(
                SchemaCode::ExceptMultiple,
                el,
                "exceptNameClass allows only a single except node".into(),
            )),
        }
    }

    /// Collects a grammar's components (through `div`s) and returns its start.
    fn grammar_start(&mut self, el: &'e El, parent: Option<usize>) -> Result<Id> {
        let grammar = self.grammars.len();
        self.grammars.push(Grammar {
            parent,
            defines: HashMap::new(),
            start: None,
        });
        self.components(&el.children, grammar)?;
        for define in self.defines.iter().filter(|d| d.grammar == grammar) {
            if define.uncombined > 1 {
                let message = match define.name {
                    "" => "Some <start> element miss the combine attribute".into(),
                    name => format!("Some defines for {name} needs the combine attribute"),
                };
                return Err(self.error(SchemaCode::NeedCombine, define.parts[0], message));
            }
        }
        match self.grammars[grammar].start {
            Some(start) => self.define(start, el),
            None => Err(self.error(
                SchemaCode::GrammarNoStart,
                el,
                "Element <grammar> has no <start>".into(),
            )),
        }
    }

    fn components(&mut self, children: &'e [El], grammar: usize) -> Result<()> {
        for child in children {
            match child.name.as_str() {
                "start" => self.component(grammar, None, child)?,
                "define" => {
                    let name = child
                        .attr("name")
                        .filter(|name| is_ncname(name))
                        .ok_or_else(|| {
                            self.error(
                                SchemaCode::InvalidDefineName,
                                child,
                                "define has no name or its name is not an NCName".into(),
                            )
                        })?;
                    self.component(grammar, Some(name), child)?
                }
                "div" => self.components(&child.children, grammar)?,
                other => {
                    return Err(self.error(
                        SchemaCode::GrammarContent,
                        child,
                        format!("grammar has unexpected child {other}"),
                    ));
                }
            }
        }
        Ok(())
    }

    fn component(&mut self, grammar: usize, name: Option<&'e str>, el: &'e El) -> Result<()> {
        let existing = match name {
            None => self.grammars[grammar].start,
            Some(name) => self.grammars[grammar].defines.get(name).copied(),
        };
        let room = self.defines.try_reserve(1);
        let room = room.and(self.grammars[grammar].defines.try_reserve(1));
        room.map_err(|_| self.refused(el))?;
        let index = existing.unwrap_or_else(|| {
            self.defines.push(Define {
                name: name.unwrap_or(""),
                parts: Vec::new(),
                combine: None,
                uncombined: 0,
                grammar,
                state: State::Todo,
            });
            let index = self.defines.len() - 1;
            match name {
                None => self.grammars[grammar].start = Some(index),
                Some(name) => {
                    self.grammars[grammar].defines.insert(name, index);
                }
            }
            index
        });
        let combine = el.attr("combine");
        let define = &self.defines[index];
        let conflict = match combine {
            Some("choice" | "interleave") => define.combine.is_some_and(|c| Some(c) != combine),
            Some(other) => {
                return Err(self.error(
                    SchemaCode::UnknownCombine,
                    el,
                    format!("define combine value '{other}' is unknown"),
                ));
            }
            None => false,
        };
        if conflict {
            let (code, what) = match name {
                None => (SchemaCode::StartChoiceAndInterleave, "<start>".to_owned()),
                Some(name) => (SchemaCode::DefChoiceAndInterleave, name.to_owned()),
            };
            return Err(self.error(
                code,
                el,
                format!("Defines for {what} use both 'choice' and 'interleave'"),
            ));
        }
        let define = &mut self.defines[index];
        define.combine = define.combine.or(combine);
        define.uncombined += usize::from(combine.is_none());
        define.parts.try_push(el).map_err(|_| self.refused(el))
    }

    /// A definition's pattern, inlined (section 4.19); `at` is the reference.
    fn define(&mut self, index: usize, at: &El) -> Result<Id> {
        match self.defines[index].state {
            State::Done(id) => return Ok(id),
            State::Busy if !self.reachable => return Ok(NOT_ALLOWED),
            State::Busy => {
                return Err(self.error(
                    SchemaCode::RefCycle,
                    at,
                    "Detected a cycle in inline references".into(),
                ));
            }
            State::Todo => self.defines[index].state = State::Busy,
        }
        let Define {
            grammar, combine, ..
        } = self.defines[index];
        let parts = cloned(&self.defines[index].parts).map_err(|_| self.refused(at))?;
        if let Some(start) = parts
            .iter()
            .find(|part| part.name == "start" && part.children.len() > 1)
        {
            return Err(self.error(
                SchemaCode::StartContent,
                start,
                "start has more than one pattern".into(),
            ));
        }
        let mut ids = Vec::new();
        ids.try_reserve_exact(parts.len())
            .map_err(|_| self.refused(at))?;
        for part in &parts {
            ids.push(self.group(&part.children, Some(grammar), part)?);
        }
        let combine = match combine {
            Some("interleave") => Store::interleave,
            _ => Store::choice,
        };
        let id = fold(&mut self.store, &ids, combine);
        self.defines[index].state = State::Done(id);
        Ok(id)
    }

    fn datatype(
        &self,
        el: &El,
        library: &str,
        name: &str,
        params: &[(&str, &str)],
    ) -> Result<Datatype> {
        Datatype::new(library, name, params, el)
            .map_err(|problem| self.problem(el, problem, library, name))
    }

    fn problem(&self, el: &El, problem: Problem, library: &str, name: &str) -> SchemaError {
        let (code, message) = match problem {
            Problem::UnknownLibrary => (
                SchemaCode::UnknownTypeLib,
                format!("Use of unregistered type library '{library}'"),
            ),
            Problem::UnknownType => (
                SchemaCode::TypeNotFound,
                format!("Error type '{name}' is not exported by type library '{library}'"),
            ),
            Problem::ParamsForbidden => (
                SchemaCode::ParamForbidden,
                format!("Type library '{library}' does not allow type parameters"),
            ),
            Problem::BadParam(param) => (
                SchemaCode::TypeValue,
                format!("Invalid parameter '{param}' for type '{name}'"),
            ),
            Problem::BadValue => (
                SchemaCode::InvalidValue,
                format!("Value '{}' is not acceptable for type '{name}'", el.text),
            ),
        };
        self.error(code, el, message)
    }

    fn data(&mut self, el: &'e El, grammar: Option<usize>) -> Result<Id> {
        let name = el
            .attr("type")
            .ok_or_else(|| self.error(SchemaCode::TypeMissing, el, "data has no type".into()))?;
        let mut params = Vec::new();
        let mut except = NOT_ALLOWED;
        for (index, child) in el.children.iter().enumerate() {
            match child.name.as_str() {
                "param" if except == NOT_ALLOWED => {
                    let param = child.attr("name").ok_or_else(|| {
                        self.error(
                            SchemaCode::ParamNameMissing,
                            child,
                            "param has no name".into(),
                        )
                    })?;
                    params.push((param, child.text.as_str()));
                }
                "except" if index + 1 == el.children.len() => {
                    let ids = self.patterns(&child.children, grammar, child)?;
                    except = fold(&mut self.store, &ids, Store::choice);
                }
                other => {
                    return Err(self.error(
                        SchemaCode::DataContent,
                        el,
                        format!("Element data has unexpected content {other}"),
                    ));
                }
            }
        }
        let datatype = self.datatype(el, &el.library, name, &params)?;
        self.tables
            .datatypes
            .try_reserve(1)
            .map_err(|_| self.refused(el))?;
        self.tables.datatypes.push(datatype);
        let index = self.tables.datatypes.len() as u32 - 1;
        Ok(self.store.make(Pat::Data(index, except)))
    }

    fn value(&mut self, el: &El) -> Result<Id> {
        // Section 4.4: no type means the built-in token type.
        let (library, name) = match el.attr("type") {
            Some(name) => (&*el.library, name),
            None => ("", "token"),
        };
        let datatype = self.datatype(el, library, name, &[])?;
        let value = ValueDef::new(datatype, &el.text, el)
            .map_err(|problem| self.problem(el, problem, library, name))?;
        self.tables
            .values
            .try_reserve(1)
            .map_err(|_| self.refused(el))?;
        self.tables.values.push(value);
        Ok(self
            .store
            .make(Pat::Value(self.tables.values.len() as u32 - 1)))
    }
}

/// Combines a non-empty list as a balanced tree, so nesting stays logarithmic.
fn fold<S, T: Copy>(state: &mut S, items: &[T], combine: fn(&mut S, T, T) -> T) -> T {
    match items {
        [one] => *one,
        _ => {
            let (left, right) = items.split_at(items.len() / 2);
            let left = fold(state, left, combine);
            let right = fold(state, right, combine);
            combine(state, left, right)
        }
    }
}
