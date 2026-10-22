//! Schema documents to component drafts, as libxml2's `xmlSchemaParse*`
//! functions do: the XML representation constraints ("schema for schemas"
//! checks), then one draft per component with references left as names.
//! Included, imported and redefined documents load through the caller's
//! resolver, breadth first from a queue, each location at most once.

use std::collections::{HashMap, HashSet, TryReserveError, VecDeque};
use std::rc::Rc;

use crate::fallible::TryPush;
use crate::name::{name_char, name_start};
use crate::pattern::{self, Pattern};
use crate::xpath::tree::{Item, Navigate, attributes, namespaces};
use crate::{ErrorKind, OwnedDocument, OwnedNodeId, Request, Resolver, ResourceKind};

use super::components::*;
use super::{Code, Error, Limits, SchemaDiagnostic};

pub(crate) const XS: &str = "http://www.w3.org/2001/XMLSchema";
pub(crate) const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";
use crate::XML_NAMESPACE as XML_NS;

/// Where a component was declared.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Loc {
    pub(crate) doc: usize,
    pub(crate) node: OwnedNodeId,
}

/// A reference to a named component, or an anonymous one by id.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Ref<T> {
    Named(QName),
    Id(T),
}

#[derive(Clone, Debug)]
pub(crate) struct RawFacet {
    pub(crate) kind: &'static str,
    pub(crate) value: String,
    pub(crate) fixed: bool,
    pub(crate) loc: Loc,
}

#[derive(Clone, Debug)]
pub(crate) enum Shape {
    /// `<simpleType><restriction>`.
    Restriction,
    List(Option<Ref<TypeId>>),
    Union(Vec<Ref<TypeId>>),
    /// `<complexType>`, with `<simpleContent>` or not.
    Complex {
        simple_content: bool,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct TypeDraft {
    pub(crate) name: Option<QName>,
    pub(crate) loc: Loc,
    pub(crate) shape: Shape,
    pub(crate) derivation: Set,
    pub(crate) base: Option<Ref<TypeId>>,
    /// `<simpleContent><restriction><simpleType>`.
    pub(crate) content_type: Option<TypeId>,
    pub(crate) facets: Vec<RawFacet>,
    pub(crate) particle: Option<Particle>,
    pub(crate) attrs: Vec<AttrItem>,
    pub(crate) wildcard: Option<WildId>,
    pub(crate) mixed: bool,
    pub(crate) final_: Set,
    pub(crate) block: Set,
    pub(crate) abstract_: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct UseDraft {
    pub(crate) decl: Ref<AttrId>,
    pub(crate) required: bool,
    pub(crate) value: Option<(bool, String)>,
    pub(crate) loc: Loc,
}

#[derive(Clone, Debug)]
pub(crate) enum AttrItem {
    Use(UseDraft),
    /// A prohibition by name, and whether it was a `ref`.
    Prohibit(QName, Loc, bool),
    Group(QName, Loc),
    /// The attribute group a `<redefine>` replaces, by redefinition index.
    RedefinedGroup(usize),
}

#[derive(Clone, Debug)]
pub(crate) struct AttrDraft {
    pub(crate) name: QName,
    pub(crate) loc: Loc,
    pub(crate) ty: Option<Ref<TypeId>>,
    pub(crate) value: Option<(bool, String)>,
}

#[derive(Clone, Debug)]
pub(crate) struct ElemDraft {
    pub(crate) name: QName,
    pub(crate) loc: Loc,
    pub(crate) ty: Option<Ref<TypeId>>,
    pub(crate) head: Option<QName>,
    pub(crate) nillable: bool,
    pub(crate) abstract_: bool,
    pub(crate) value: Option<(bool, String)>,
    pub(crate) block: Set,
    pub(crate) final_: Set,
    pub(crate) idcs: Vec<IdcId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Compositor {
    Sequence,
    Choice,
    All,
}

#[derive(Clone, Debug)]
pub(crate) enum Term {
    Element(ElemId),
    ElementRef(QName),
    Wildcard(WildId),
    Group(Compositor, Vec<Particle>),
    GroupRef(QName),
    /// The group a `<redefine>` replaces, by redefinition index.
    RedefinedGroup(usize),
    /// A resolved model group definition reference.
    GroupDef(GroupId),
    /// A reference that did not resolve; it matches nothing and is empty.
    Missing,
}

#[derive(Clone, Debug)]
pub(crate) struct Particle {
    pub(crate) min: u32,
    pub(crate) max: Option<u32>,
    pub(crate) term: Term,
    pub(crate) loc: Loc,
}

impl Particle {
    /// A deep copy (see [`copies`]).
    pub(crate) fn copy(&self) -> Result<Particle, TryReserveError> {
        let term = match &self.term {
            Term::Group(compositor, children) => Term::Group(*compositor, copies(children)?),
            term => term.clone(),
        };
        Ok(Particle { term, ..*self })
    }
}

/// Deep copies of `particles`, reserved fallibly: a model group is as
/// large as its schema.
pub(crate) fn copies(particles: &[Particle]) -> Result<Vec<Particle>, TryReserveError> {
    let mut out = Vec::new();
    out.try_reserve_exact(particles.len())?;
    for particle in particles {
        out.push(particle.copy()?);
    }
    Ok(out)
}

#[derive(Clone, Debug)]
pub(crate) struct GroupDraft {
    pub(crate) name: QName,
    pub(crate) loc: Loc,
    pub(crate) model: Option<(Compositor, Vec<Particle>, Loc)>,
}

#[derive(Clone, Debug)]
pub(crate) struct AttrGroupDraft {
    pub(crate) name: QName,
    pub(crate) loc: Loc,
    pub(crate) attrs: Vec<AttrItem>,
    pub(crate) wildcard: Option<WildId>,
}

#[derive(Clone, Debug)]
pub(crate) struct IdcDraft {
    pub(crate) kind: IdcKind,
    pub(crate) name: QName,
    pub(crate) loc: Loc,
    pub(crate) selector: Option<Pattern>,
    pub(crate) fields: Vec<(Box<str>, Pattern)>,
    pub(crate) refer: Option<QName>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum GlobalKind {
    Type,
    Element,
    Attribute,
    Group,
    AttrGroup,
    Idc,
    Notation,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Global {
    pub(crate) kind: GlobalKind,
    pub(crate) name: QName,
    pub(crate) id: usize,
    pub(crate) loc: Loc,
}

#[derive(Clone, Debug)]
pub(crate) struct Redefine {
    pub(crate) kind: GlobalKind,
    pub(crate) id: usize,
    pub(crate) name: QName,
    pub(crate) bucket: usize,
    /// Whether the redefinition refers to the component it redefines.
    pub(crate) self_ref: bool,
    pub(crate) loc: Loc,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Relation {
    Import(Sym),
    Include,
}

/// One schema document parsed for one target namespace.
#[derive(Debug)]
pub(crate) struct Bucket {
    pub(crate) doc: usize,
    location: String,
    tns: Sym,
    orig_tns: Option<Sym>,
    imported: bool,
    relations: Vec<(Relation, Option<usize>)>,
    pub(crate) globals: Vec<Global>,
    pub(crate) children: Vec<usize>,
}

/// Everything parsed from the schema documents.
pub(crate) struct Drafts {
    pub(crate) names: Names,
    pub(crate) docs: Vec<(String, Rc<OwnedDocument>)>,
    pub(crate) buckets: Vec<Bucket>,
    pub(crate) types: Vec<TypeDraft>,
    pub(crate) elements: Vec<ElemDraft>,
    pub(crate) attributes: Vec<AttrDraft>,
    pub(crate) groups: Vec<GroupDraft>,
    pub(crate) attr_groups: Vec<AttrGroupDraft>,
    pub(crate) wildcards: Vec<(Wildcard, Loc)>,
    pub(crate) idcs: Vec<IdcDraft>,
    pub(crate) redefines: Vec<Redefine>,
    pub(crate) diagnostics: Vec<SchemaDiagnostic>,
}

impl Drafts {
    pub(crate) fn report(&mut self, loc: Option<Loc>, code: Code, message: String) {
        let (uri, offset) = match loc {
            Some(loc) => {
                let (uri, tree) = &self.docs[loc.doc];
                (uri.clone(), tree.offset(loc.node))
            }
            None => (
                self.docs.first().map(|d| d.0.clone()).unwrap_or_default(),
                None,
            ),
        };
        self.diagnostics.push(SchemaDiagnostic {
            code,
            uri,
            offset,
            message,
        });
    }

    pub(crate) fn errors(&self) -> bool {
        self.names.refused || self.diagnostics.iter().any(|d| !d.code.is_warning())
    }

    /// Reports a refused allocation.
    pub(crate) fn refuse(&mut self) {
        self.report(None, Code::Allocation, "Memory allocation failed".into());
    }

    /// A build that failed for a refused allocation.
    pub(crate) fn refusal(&self) -> Error {
        let uri = self.docs.first().map(|d| d.0.clone()).unwrap_or_default();
        Error {
            diagnostics: vec![SchemaDiagnostic {
                code: Code::Allocation,
                uri,
                offset: None,
                message: "Memory allocation failed".into(),
            }],
        }
    }

    /// The failed build: every diagnostic, and a refused name table's.
    pub(crate) fn failure(mut self) -> Error {
        if self.names.refused {
            self.refuse();
        }
        Error {
            diagnostics: self.diagnostics,
        }
    }

    /// The in-scope namespaces of a schema element.
    pub(crate) fn scope(&self, loc: Loc) -> Vec<(&str, &str)> {
        namespaces(&*self.docs[loc.doc].1, loc.node)
    }

    pub(crate) fn type_id(index: usize) -> TypeId {
        index + Self::FIRST_TYPE
    }

    pub(crate) const FIRST_TYPE: TypeId = 46;
}

/// Per-document parsing state.
struct Cx {
    tree: Rc<OwnedDocument>,
    doc: usize,
    bucket: usize,
    tns: Sym,
    chameleon: bool,
    element_form: bool,
    attribute_form: bool,
    block_default: Set,
    final_default: Set,
    /// The bucket being redefined, inside `<redefine>`.
    redefining: Option<usize>,
}

type Id = OwnedNodeId;

impl Cx {
    fn loc(&self, node: Id) -> Loc {
        Loc {
            doc: self.doc,
            node,
        }
    }

    fn local(&self, node: Id) -> Option<&str> {
        match self.tree.item(node) {
            Item::Element(name) if name.namespace == Some(XS) => Some(name.local),
            _ => None,
        }
    }

    fn is(&self, node: Option<Id>, local: &str) -> bool {
        node.is_some_and(|node| self.local(node) == Some(local))
    }

    fn attr(&self, node: Id, name: &str) -> Option<&str> {
        attributes(&*self.tree, node)
            .find(|(_, qname, _)| qname.namespace.is_none() && qname.qualified == name)
            .map(|(_, _, value)| value)
    }

    fn display(&self, node: Id) -> String {
        match self.tree.item(node) {
            Item::Element(name) => match name.namespace {
                Some(ns) => format!("Element '{{{ns}}}{}'", name.local),
                None => format!("Element '{}'", name.local),
            },
            _ => "Text".into(),
        }
    }
}

/// Element children, skipping comments, PIs and blank text as libxml2's
/// `xmlSchemaCleanupDoc` does; other text stays and fails S4S checks.
struct Children<'t> {
    tree: &'t OwnedDocument,
    next: Option<Id>,
}

impl<'t> Children<'t> {
    fn new(tree: &'t OwnedDocument, node: Id) -> Self {
        let next = Self::from(tree, tree.first_child(node));
        Self { tree, next }
    }

    /// The first child kept from `node` on.
    fn from(tree: &OwnedDocument, node: Option<Id>) -> Option<Id> {
        std::iter::successors(node, |&n| tree.next_sibling(n)).find(|&child| {
            match tree.item(child) {
                Item::Element(_) => true,
                Item::Text(text) => !text
                    .bytes()
                    .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r')),
                _ => false,
            }
        })
    }

    fn peek(&self) -> Option<Id> {
        self.next
    }

    /// The next child, moving past it.
    fn bump(&mut self) -> Option<Id> {
        let node = self.next?;
        self.next = Self::from(self.tree, self.tree.next_sibling(node));
        Some(node)
    }

    fn take(&mut self, cx: &Cx, local: &str) -> Option<Id> {
        self.peek().filter(|&node| cx.local(node) == Some(local))?;
        self.bump()
    }

    fn take_any(&mut self, cx: &Cx, locals: &[&'static str]) -> Option<(Id, &'static str)> {
        let node = self.peek()?;
        let local = cx.local(node)?;
        let found = locals.iter().copied().find(|&known| known == local)?;
        self.bump();
        Some((node, found))
    }
}

const FACETS: [&str; 12] = [
    "minInclusive",
    "minExclusive",
    "maxInclusive",
    "maxExclusive",
    "totalDigits",
    "fractionDigits",
    "pattern",
    "enumeration",
    "whiteSpace",
    "length",
    "maxLength",
    "minLength",
];

fn is_ncname(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| name_start(c, false)) && chars.all(|c| name_char(c, false))
}

fn is_qname(text: &str) -> bool {
    match text.split_once(':') {
        Some((prefix, local)) => is_ncname(prefix) && is_ncname(local),
        None => is_ncname(text),
    }
}

fn blank(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// `xmlSchemaPValAttrBlockFinal`: `#all` or a list from `allowed`.
fn derivation_set(value: &str, allowed: Set) -> Option<Set> {
    if value == "#all" {
        return Some(allowed);
    }
    value
        .split(blank)
        .filter(|s| !s.is_empty())
        .try_fold(0, |set, item| {
            let flag = match item {
                "extension" => EXTENSION,
                "restriction" => RESTRICTION,
                "substitution" => SUBSTITUTION,
                "list" => LIST,
                "union" => UNION,
                _ => return None,
            };
            (allowed & flag != 0).then_some(set | flag)
        })
}

/// Loads and parses documents for one schema.
pub(crate) struct Parser {
    pub(crate) drafts: Drafts,
    resolver: Option<Box<dyn Resolver>>,
    limits: Limits,
    loads: usize,
    bytes_left: usize,
    queue: VecDeque<usize>,
    /// Per-document `xs:ID` attribute values.
    ids: Vec<HashSet<String>>,
}

impl Parser {
    pub(crate) fn new(resolver: Option<Box<dyn Resolver>>, limits: Limits) -> Self {
        Self {
            drafts: Drafts {
                names: Names::new(),
                docs: Vec::new(),
                buckets: Vec::new(),
                types: Vec::new(),
                elements: Vec::new(),
                attributes: Vec::new(),
                groups: Vec::new(),
                attr_groups: Vec::new(),
                wildcards: Vec::new(),
                idcs: Vec::new(),
                redefines: Vec::new(),
                diagnostics: Vec::new(),
            },
            resolver,
            bytes_left: limits.max_bytes,
            limits,
            loads: 0,
            queue: VecDeque::new(),
            ids: Vec::new(),
        }
    }

    fn error(&mut self, cx: &Cx, node: Id, code: Code, message: &str) {
        let text = format!("{}: {message}", cx.display(node));
        self.drafts.report(Some(cx.loc(node)), code, text);
    }

    /// Appends `item` to the drafts' `list`, returning its index; a refused
    /// allocation is reported, failing the build, and gives `None`.
    fn add<T>(&mut self, list: impl FnOnce(&mut Drafts) -> &mut Vec<T>, item: T) -> Option<usize> {
        let items = list(&mut self.drafts);
        let added = items.try_push(item).map(|()| items.len() - 1);
        added.map_err(|_| self.drafts.refuse()).ok()
    }

    fn not_allowed(&mut self, cx: &Cx, node: Id, child: Id, expected: &str) {
        let message = format!("This element is not expected. Expected is ( {expected} ).");
        let _ = node;
        self.error(cx, child, Code::S4sElemNotAllowed, &message);
    }

    fn missing_attr(&mut self, cx: &Cx, node: Id, name: &str) {
        self.error(
            cx,
            node,
            Code::S4sAttrMissing,
            &format!("The attribute '{name}' is required but missing."),
        );
    }

    fn invalid_attr(
        &mut self,
        cx: &Cx,
        node: Id,
        code: Code,
        name: &str,
        value: &str,
        expected: &str,
    ) {
        let message = format!(
            "Attribute '{name}': The value '{value}' is not valid. Expected is '{expected}'."
        );
        self.error(cx, node, code, &message);
    }

    /// Unknown unqualified attributes and attributes in the XSD namespace
    /// are not allowed; others pass.
    fn check_attrs(&mut self, cx: &Cx, node: Id, allowed: &[&str]) {
        let bad: Vec<String> = attributes(&*cx.tree, node)
            .filter(|(_, name, _)| !name.is_namespace_declaration())
            .filter(|(_, name, _)| match name.namespace {
                None => !allowed.contains(&name.qualified),
                Some(ns) => ns == XS,
            })
            .map(|(_, name, _)| name.qualified.to_owned())
            .collect();
        for name in bad {
            self.error(
                cx,
                node,
                Code::S4sAttrNotAllowed,
                &format!("The attribute '{name}' is not allowed."),
            );
        }
    }

    /// `xmlSchemaPValAttrID`: an NCName unique in its document.
    fn check_id(&mut self, cx: &Cx, node: Id) {
        let Some(value) = cx
            .attr(node, "id")
            .map(|v| v.trim_matches(blank).to_owned())
        else {
            return;
        };
        let duplicate = is_ncname(&value) && !self.ids[cx.doc].insert(value.clone());
        if !is_ncname(&value) || duplicate {
            self.invalid_attr(cx, node, Code::S4sAttrInvalidValue, "id", &value, "xs:ID");
        }
    }

    fn ncname(&mut self, cx: &Cx, node: Id, attr: &str) -> Option<Sym> {
        let value = cx.attr(node, attr)?.trim_matches(blank).to_owned();
        if !is_ncname(&value) {
            self.invalid_attr(
                cx,
                node,
                Code::S4sAttrInvalidValue,
                attr,
                &value,
                "xs:NCName",
            );
            return None;
        }
        Some(self.drafts.names.intern(&value))
    }

    /// `xmlSchemaPValAttrNodeQNameValue`: resolves a QName-valued attribute
    /// with the element's in-scope namespaces.
    fn qname_value(&mut self, cx: &Cx, node: Id, attr: &str, value: &str) -> Option<QName> {
        let value = value.trim_matches(blank);
        if !is_qname(value) {
            self.invalid_attr(cx, node, Code::S4sAttrInvalidValue, attr, value, "xs:QName");
            return None;
        }
        let scope = namespaces(&*cx.tree, node);
        let lookup = |prefix: &str| {
            scope
                .iter()
                .find(|&&(p, _)| p == prefix)
                .map(|&(_, uri)| uri.to_owned())
        };
        let (ns, local) = match value.split_once(':') {
            Some((prefix, local)) => match lookup(prefix) {
                Some(uri) => (self.drafts.names.intern(&uri), local),
                None => {
                    let message = format!(
                        "Attribute '{attr}': The value '{value}' of simple type 'xs:QName' has no corresponding namespace declaration in scope."
                    );
                    self.error(cx, node, Code::S4sAttrInvalidValue, &message);
                    return None;
                }
            },
            None => match lookup("").filter(|uri| !uri.is_empty()) {
                Some(uri) => (self.drafts.names.intern(&uri), value),
                None if cx.chameleon => (cx.tns, value),
                None => (NO_NS, value),
            },
        };
        let local = self.drafts.names.intern(local);
        Some(QName { ns, local })
    }

    fn qname(&mut self, cx: &Cx, node: Id, attr: &str) -> Option<QName> {
        let value = cx.attr(node, attr)?.to_owned();
        self.qname_value(cx, node, attr, &value)
    }

    /// `xmlSchemaCheckReference`: references must stay in the target, XSD
    /// or an imported namespace.
    fn check_reference(&mut self, cx: &Cx, node: Id, ns: Sym) -> bool {
        let names = &self.drafts.names;
        let imported = self.drafts.buckets[cx.bucket]
            .relations
            .iter()
            .any(|(relation, _)| *relation == Relation::Import(ns));
        if ns == cx.tns || names.str(ns) == XS || imported {
            return true;
        }
        let message = match ns {
            NO_NS => "References from this schema to components in no namespace are not allowed, since not indicated by an import statement".to_owned(),
            ns => format!(
                "References from this schema to components in the namespace '{}' are not allowed, since not indicated by an import statement",
                names.str(ns)
            ),
        };
        self.error(cx, node, Code::SrcResolve, &message);
        false
    }

    fn reference(&mut self, cx: &Cx, node: Id, attr: &str) -> Option<QName> {
        let name = self.qname(cx, node, attr)?;
        self.check_reference(cx, node, name.ns).then_some(name)
    }

    fn boolean(&mut self, cx: &Cx, node: Id, attr: &str) -> bool {
        match cx.attr(node, attr) {
            None | Some("false" | "0") => false,
            Some("true" | "1") => true,
            Some(value) => {
                let value = value.to_owned();
                self.invalid_attr(cx, node, Code::InvalidBoolean, attr, &value, "xs:boolean");
                false
            }
        }
    }

    /// `xmlGetMinOccurs` / `xmlGetMaxOccurs`; `all` limits both to 0 or 1
    /// (and `maxOccurs` to 1).
    fn occurs(&mut self, cx: &Cx, node: Id, all: bool) -> (u32, Option<u32>) {
        const UNBOUNDED: u64 = 1 << 30;
        let parse = |value: &str| -> Option<u64> {
            let trimmed = value.trim_matches(blank);
            (!trimmed.is_empty() && trimmed.bytes().all(|b| b.is_ascii_digit())).then(|| {
                trimmed.bytes().fold(0u64, |acc, b| {
                    (acc * 10 + u64::from(b - b'0')).min(i32::MAX as u64)
                })
            })
        };
        let (min_range, max_range) = match all {
            true => (0..=1, 1..=1),
            false => (0..=u64::MAX, 0..=UNBOUNDED),
        };
        let min = match cx.attr(node, "minOccurs").map(str::to_owned) {
            None => 1,
            Some(value) => match parse(&value).filter(|n| min_range.contains(n)) {
                Some(n) => n as u32,
                None => {
                    let expected = if all {
                        "(0 | 1)"
                    } else {
                        "xs:nonNegativeInteger"
                    };
                    self.invalid_attr(
                        cx,
                        node,
                        Code::S4sAttrInvalidValue,
                        "minOccurs",
                        &value,
                        expected,
                    );
                    1
                }
            },
        };
        let max = match cx.attr(node, "maxOccurs").map(str::to_owned) {
            None => Some(1),
            Some(value) if value == "unbounded" && !all => None,
            Some(value) => match parse(&value).filter(|n| max_range.contains(n)) {
                Some(n) => Some(n as u32),
                None => {
                    let expected = if all {
                        "1"
                    } else {
                        "(xs:nonNegativeInteger | unbounded)"
                    };
                    self.invalid_attr(
                        cx,
                        node,
                        Code::S4sAttrInvalidValue,
                        "maxOccurs",
                        &value,
                        expected,
                    );
                    Some(1)
                }
            },
        };
        // xmlSchemaPCheckParticleCorrect_2
        match max {
            Some(0) if min == 0 => {}
            Some(0) => self.error(
                cx,
                node,
                Code::PPropsCorrect2_2,
                "The value must be greater than or equal to 1.",
            ),
            Some(max) if min > max => self.error(
                cx,
                node,
                Code::PPropsCorrect2_1,
                "The value must not be greater than the value of 'maxOccurs'.",
            ),
            _ => {}
        }
        (min, max)
    }

    // ---- loading -----------------------------------------------------------

    fn fetch(&mut self, uri: &str) -> Result<Option<Vec<u8>>, ()> {
        let Some(resolver) = self.resolver.as_mut() else {
            return Ok(None);
        };
        if self.loads >= self.limits.max_documents {
            return Err(());
        }
        self.loads += 1;
        let request = Request {
            kind: ResourceKind::Schema,
            uri,
            public_id: None,
            max_bytes: self.bytes_left,
        };
        match resolver.resolve(request) {
            Ok(Some(bytes)) if bytes.len() <= self.bytes_left => {
                self.bytes_left -= bytes.len();
                Ok(Some(bytes))
            }
            Ok(Some(_)) => Err(()),
            Ok(None) | Err(_) => Ok(None),
        }
    }

    /// Parses `bytes` into a document whose root must be `<xs:schema>`;
    /// `None` after reporting why not.
    fn load_document(&mut self, uri: &str, bytes: &[u8], from: Option<(&Cx, Id)>) -> Option<usize> {
        let report = |parser: &mut Self, code: Code, message: String| match from {
            Some((cx, node)) => parser.error(cx, node, code, &message),
            None => parser.drafts.report(None, code, message),
        };
        let tree = match OwnedDocument::parse_legacy(bytes, self.limits.xml) {
            Ok(tree) => tree,
            Err(error) => {
                let code = match error.kind {
                    ErrorKind::Allocation => Code::Allocation,
                    _ => Code::SrcImport2_1,
                };
                let message = format!("Failed to parse the XML resource '{uri}': {error}.");
                report(self, code, message);
                return None;
            }
        };
        let root = tree.root_element().id();
        if !matches!(tree.item(root), Item::Element(name) if name.local == "schema" && name.namespace == Some(XS))
        {
            report(
                self,
                Code::NotSchema,
                format!("The XML document '{uri}' is not a schema document."),
            );
            return None;
        }
        self.drafts.docs.push((uri.to_owned(), Rc::new(tree)));
        self.ids.push(HashSet::new());
        Some(self.drafts.docs.len() - 1)
    }

    fn target_namespace(&mut self, doc: usize) -> Option<Sym> {
        let tree = Rc::clone(&self.drafts.docs[doc].1);
        let root = tree.root_element().id();
        let value = attributes(&*tree, root)
            .find(|(_, name, _)| name.namespace.is_none() && name.qualified == "targetNamespace")
            .map(|(_, _, value)| value.to_owned())?;
        Some(self.drafts.names.intern(&value))
    }

    fn new_bucket(
        &mut self,
        doc: usize,
        location: String,
        tns: Sym,
        orig_tns: Option<Sym>,
        imported: bool,
    ) -> usize {
        self.drafts.buckets.push(Bucket {
            doc,
            location,
            tns,
            orig_tns,
            imported,
            relations: Vec::new(),
            globals: Vec::new(),
            children: Vec::new(),
        });
        self.queue.push_back(self.drafts.buckets.len() - 1);
        self.drafts.buckets.len() - 1
    }

    /// Parses the main document and everything it includes or imports.
    pub(crate) fn run(&mut self, bytes: &[u8], uri: &str) {
        let Some(doc) = self.load_document(uri, bytes, None) else {
            return;
        };
        let tns = self.target_namespace(doc);
        self.new_bucket(doc, uri.to_owned(), tns.unwrap_or(NO_NS), tns, true);
        while let Some(bucket) = self.queue.pop_front() {
            self.parse_bucket(bucket);
        }
    }

    /// Adds `location` (with `namespace`) as an extra imported document, as
    /// `xsi:schemaLocation` does.
    pub(crate) fn add_import(&mut self, namespace: Option<&str>, location: &str) {
        let ns = namespace.map_or(NO_NS, |ns| self.drafts.names.intern(ns));
        if self.drafts.buckets.iter().any(|b| b.location == location) {
            return;
        }
        if let Some(existing) = self
            .drafts
            .buckets
            .iter()
            .position(|b| b.imported && b.orig_tns.unwrap_or(NO_NS) == ns)
        {
            let _ = existing;
            return;
        }
        let Ok(Some(bytes)) = self.fetch(location) else {
            let message = format!("The document at location '{location}' could not be acquired.");
            self.drafts.report(None, Code::WarnUnlocatedSchema, message);
            return;
        };
        let Some(doc) = self.load_document(location, &bytes, None) else {
            return;
        };
        let tns = self.target_namespace(doc);
        self.new_bucket(doc, location.to_owned(), tns.unwrap_or(NO_NS), tns, true);
        while let Some(bucket) = self.queue.pop_front() {
            self.parse_bucket(bucket);
        }
    }

    fn parse_bucket(&mut self, bucket: usize) {
        let doc = self.drafts.buckets[bucket].doc;
        let tree = Rc::clone(&self.drafts.docs[doc].1);
        let root = tree.root_element().id();
        let tns = self.drafts.buckets[bucket].tns;
        let chameleon = self.drafts.buckets[bucket].orig_tns.is_none() && tns != NO_NS;
        let mut cx = Cx {
            tree,
            doc,
            bucket,
            tns,
            chameleon,
            element_form: false,
            attribute_form: false,
            block_default: 0,
            final_default: 0,
            redefining: None,
        };
        if !self.schema_element(&mut cx, root) {
            return;
        }
        self.top_level(&mut cx, root);
    }

    /// `xmlSchemaParseSchemaElement`: the document-wide defaults.
    fn schema_element(&mut self, cx: &mut Cx, node: Id) -> bool {
        self.check_id(cx, node);
        let errors = self.drafts.diagnostics.len();
        if let Some(value) = cx.attr(node, "targetNamespace").map(str::to_owned)
            && crate::schema_types::Type::AnyUri
                .parse(&value, None)
                .is_err()
        {
            self.invalid_attr(
                cx,
                node,
                Code::S4sAttrInvalidValue,
                "targetNamespace",
                &value,
                "xs:anyURI",
            );
            return false;
        }
        for (attr, code) in [
            ("elementFormDefault", Code::ElemFormDefaultValue),
            ("attributeFormDefault", Code::AttrFormDefaultValue),
        ] {
            let qualified = match cx.attr(node, attr) {
                None | Some("unqualified") => false,
                Some("qualified") => true,
                Some(value) => {
                    let value = value.to_owned();
                    self.invalid_attr(cx, node, code, attr, &value, "(qualified | unqualified)");
                    false
                }
            };
            match attr {
                "elementFormDefault" => cx.element_form = qualified,
                _ => cx.attribute_form = qualified,
            }
        }
        for (attr, allowed) in [
            ("finalDefault", EXTENSION | RESTRICTION | LIST | UNION),
            ("blockDefault", EXTENSION | RESTRICTION | SUBSTITUTION),
        ] {
            let Some(value) = cx.attr(node, attr).map(str::to_owned) else {
                continue;
            };
            match derivation_set(&value, allowed) {
                Some(set) if attr == "finalDefault" => cx.final_default = set,
                Some(set) => cx.block_default = set,
                None => self.invalid_attr(
                    cx,
                    node,
                    Code::S4sAttrInvalidValue,
                    attr,
                    &value,
                    "(#all | List of ...)",
                ),
            }
        }
        errors == self.drafts.diagnostics.len()
    }

    /// `xmlSchemaParseSchemaTopLevel`.
    fn top_level(&mut self, cx: &mut Cx, root: Id) {
        let tree = Rc::clone(&cx.tree);
        let mut children = Children::new(&tree, root);
        while let Some((node, kind)) =
            children.take_any(cx, &["include", "import", "redefine", "annotation"])
        {
            let errors = self.drafts.diagnostics.len();
            match kind {
                "annotation" => self.annotation(cx, node),
                "import" => self.import(cx, node),
                _ => self.include(cx, node, kind == "redefine"),
            }
            if kind != "annotation"
                && self.drafts.diagnostics[errors..]
                    .iter()
                    .any(|d| !d.code.is_warning())
            {
                return;
            }
        }
        while let Some(node) = children.bump() {
            match cx.local(node) {
                Some("complexType") => {
                    self.complex_type(cx, node, true);
                }
                Some("simpleType") => {
                    self.simple_type(cx, node, true);
                }
                Some("element") => {
                    self.element(cx, node, true);
                }
                Some("attribute") => self.global_attribute(cx, node),
                Some("attributeGroup") => self.attribute_group(cx, node),
                Some("group") => self.group_definition(cx, node),
                Some("notation") => self.notation(cx, node),
                Some("annotation") => self.annotation(cx, node),
                _ => self.not_allowed(
                    cx,
                    root,
                    node,
                    "(include | import | redefine | annotation)*, (((simpleType | complexType | group | attributeGroup) | element | attribute | notation), annotation*)*",
                ),
            }
        }
    }

    fn global(&mut self, cx: &Cx, kind: GlobalKind, name: QName, id: usize, node: Id) {
        let loc = cx.loc(node);
        let global = Global {
            kind,
            name,
            id,
            loc,
        };
        if self
            .add(|d| &mut d.buckets[cx.bucket].globals, global)
            .is_none()
        {
            return;
        }
        if let Some(target) = cx.redefining
            && matches!(
                kind,
                GlobalKind::Type | GlobalKind::Group | GlobalKind::AttrGroup
            )
        {
            let redefine = Redefine {
                kind,
                id,
                name,
                bucket: target,
                self_ref: false,
                loc,
            };
            self.add(|d| &mut d.redefines, redefine);
        }
    }

    // ---- composition -------------------------------------------------------

    fn import(&mut self, cx: &Cx, node: Id) {
        self.check_attrs(cx, node, &["id", "namespace", "schemaLocation"]);
        self.check_id(cx, node);
        let mut children = Children::new(&cx.tree, node);
        children.take(cx, "annotation");
        if let Some(child) = children.peek() {
            self.not_allowed(cx, node, child, "(annotation?)");
        }
        let namespace = cx.attr(node, "namespace").map(str::to_owned);
        let location = cx.attr(node, "schemaLocation").map(str::to_owned);
        let own = self.drafts.buckets[cx.bucket].orig_tns;
        match namespace.as_deref() {
            Some(ns) if own == Some(self.drafts.names.intern(ns)) => {
                let message = format!(
                    "The value of the attribute 'namespace' must not match the target namespace '{ns}' of the importing schema."
                );
                return self.error(cx, node, Code::SrcImport1_1, &message);
            }
            None if own.is_none() => {
                return self.error(
                    cx,
                    node,
                    Code::SrcImport1_2,
                    "The attribute 'namespace' must be existent if the importing schema has no target namespace.",
                );
            }
            _ => {}
        }
        let ns = namespace
            .as_deref()
            .map_or(NO_NS, |ns| self.drafts.names.intern(ns));
        let Some(location) = location else {
            self.drafts.buckets[cx.bucket]
                .relations
                .push((Relation::Import(ns), None));
            return;
        };
        let uri = crate::uri::resolve(&location, &self.drafts.docs[cx.doc].0).unwrap_or(location);
        let existing = self.drafts.buckets.iter().position(|b| b.location == uri);
        if existing == Some(cx.bucket) {
            return self.error(
                cx,
                node,
                Code::SrcImport,
                "The schema must not import/include/redefine itself.",
            );
        }
        let existing = existing.or_else(|| {
            self.drafts
                .buckets
                .iter()
                .position(|b| b.imported && b.orig_tns.unwrap_or(NO_NS) == ns)
        });
        let bucket = match existing {
            Some(bucket) => {
                if self.drafts.buckets[bucket].location != uri
                    && self.drafts.buckets[bucket].imported
                {
                    let message = format!(
                        "Skipping import of schema located at '{uri}' for the namespace '{}', since this namespace was already imported with the schema located at '{}'.",
                        self.drafts.names.str(ns),
                        self.drafts.buckets[bucket].location
                    );
                    self.error(cx, node, Code::WarnSkipSchema, &message);
                }
                Some(bucket)
            }
            None => match self.fetch(&uri) {
                Ok(Some(bytes)) => self
                    .load_document(&uri, &bytes, Some((cx, node)))
                    .map(|doc| {
                        let tns = self.target_namespace(doc);
                        self.new_bucket(doc, uri.clone(), tns.unwrap_or(NO_NS), tns, true)
                    }),
                Ok(None) => {
                    let message = format!(
                        "Failed to locate a schema at location '{uri}'. Skipping the import."
                    );
                    self.error(cx, node, Code::WarnUnlocatedSchema, &message);
                    None
                }
                Err(()) => {
                    self.error(
                        cx,
                        node,
                        Code::FailedLoad,
                        "Schema loading limits exceeded.",
                    );
                    None
                }
            },
        };
        let relations = &mut self.drafts.buckets[cx.bucket];
        relations.relations.push((Relation::Import(ns), bucket));
        relations.children.extend(bucket);
    }

    /// `xmlSchemaParseIncludeOrRedefine`.
    fn include(&mut self, cx: &mut Cx, node: Id, redefine: bool) {
        self.check_attrs(cx, node, &["id", "schemaLocation"]);
        self.check_id(cx, node);
        let code = if redefine {
            Code::SrcRedefine
        } else {
            Code::SrcInclude
        };
        let Some(location) = cx.attr(node, "schemaLocation").map(str::to_owned) else {
            return self.missing_attr(cx, node, "schemaLocation");
        };
        let uri = crate::uri::resolve(&location, &self.drafts.docs[cx.doc].0).unwrap_or(location);
        if uri == self.drafts.docs[cx.doc].0 {
            let what = if redefine { "redefine" } else { "include" };
            return self.error(
                cx,
                node,
                code,
                &format!("The schema document '{uri}' cannot {what} itself."),
            );
        }
        let tns = cx.tns;
        let reuse = self
            .drafts
            .buckets
            .iter()
            .position(|b| b.location == uri && (b.orig_tns.is_some() || b.tns == tns));
        let loaded = match self.drafts.buckets.iter().find(|b| b.location == uri) {
            Some(bucket) => Some(bucket.doc),
            None => match self.fetch(&uri) {
                // load_document reported why the document is unusable.
                Ok(Some(bytes)) => match self.load_document(&uri, &bytes, Some((cx, node))) {
                    Some(doc) => Some(doc),
                    None => return,
                },
                _ => None,
            },
        };
        let Some(doc) = loaded else {
            let what = if redefine {
                "redefinition"
            } else {
                "inclusion"
            };
            return self.error(
                cx,
                node,
                code,
                &format!("Failed to load the document '{uri}' for {what}."),
            );
        };
        let orig = self.target_namespace(doc);
        match orig {
            Some(_) if tns == NO_NS => {
                let message = format!(
                    "The target namespace of the included/redefined schema '{uri}' has to be absent, since the including/redefining schema has no target namespace."
                );
                return self.error(cx, node, Code::SrcInclude, &message);
            }
            Some(orig) if orig != tns => {
                let message = format!(
                    "The target namespace '{}' of the included/redefined schema '{uri}' differs from '{}' of the including/redefining schema.",
                    self.drafts.names.str(orig),
                    self.drafts.names.str(tns)
                );
                return self.error(cx, node, Code::SrcInclude, &message);
            }
            _ => {}
        }
        let bucket =
            Some(reuse.unwrap_or_else(|| self.new_bucket(doc, uri.clone(), tns, orig, false)));
        let current = &mut self.drafts.buckets[cx.bucket];
        current.relations.push((Relation::Include, bucket));
        current.children.extend(bucket);
        let mut children = Children::new(&cx.tree, node);
        if redefine {
            cx.redefining = bucket;
            while let Some((child, kind)) = children.take_any(
                cx,
                &[
                    "annotation",
                    "simpleType",
                    "complexType",
                    "group",
                    "attributeGroup",
                ],
            ) {
                match kind {
                    "simpleType" => {
                        self.simple_type(cx, child, true);
                    }
                    "complexType" => {
                        self.complex_type(cx, child, true);
                    }
                    "group" => self.group_definition(cx, child),
                    "attributeGroup" => self.attribute_group(cx, child),
                    _ => {}
                }
            }
            cx.redefining = None;
            if let Some(child) = children.peek() {
                self.not_allowed(
                    cx,
                    node,
                    child,
                    "(annotation | (simpleType | complexType | group | attributeGroup))*",
                );
            }
        } else {
            children.take(cx, "annotation");
            if let Some(child) = children.peek() {
                self.not_allowed(cx, node, child, "(annotation?)");
            }
        }
    }

    // ---- annotations, notations, facets, wildcards --------------------------

    /// `xmlSchemaParseAnnotation`: content is not inspected beyond
    /// `appinfo` and `documentation` attributes.
    fn annotation(&mut self, cx: &Cx, node: Id) {
        self.check_attrs(cx, node, &["id"]);
        self.check_id(cx, node);
        let mut barked = false;
        let mut children = Children::new(&cx.tree, node);
        while let Some(child) = children.bump() {
            match cx.local(child) {
                Some("appinfo") => self.check_attrs(cx, child, &["source"]),
                Some("documentation") => {
                    self.check_attrs(cx, child, &["source"]);
                    let bad_lang = attributes(&*cx.tree, child).any(|(_, name, _)| {
                        name.local == "lang"
                            && name.namespace.is_some_and(|ns| ns != XML_NS && ns != XS)
                    });
                    if bad_lang {
                        self.error(
                            cx,
                            child,
                            Code::S4sAttrNotAllowed,
                            "The attribute 'lang' is not allowed.",
                        );
                    }
                }
                _ if !barked => {
                    barked = true;
                    self.not_allowed(cx, node, child, "(appinfo | documentation)*");
                }
                _ => {}
            }
        }
    }

    fn skip_annotation(&mut self, cx: &Cx, children: &mut Children) {
        if let Some(node) = children.take(cx, "annotation") {
            self.annotation(cx, node);
        }
    }

    fn annotation_only(&mut self, cx: &Cx, node: Id) {
        let mut children = Children::new(&cx.tree, node);
        self.skip_annotation(cx, &mut children);
        if let Some(child) = children.peek() {
            self.not_allowed(cx, node, child, "(annotation?)");
        }
    }

    fn notation(&mut self, cx: &Cx, node: Id) {
        let Some(name) = cx.attr(node, "name").map(str::to_owned) else {
            return self.error(cx, node, Code::NotationNoName, "Notation has no name.");
        };
        let local = self.drafts.names.intern(&name);
        self.global(
            cx,
            GlobalKind::Notation,
            QName { ns: cx.tns, local },
            0,
            node,
        );
        self.check_id(cx, node);
        self.annotation_only(cx, node);
    }

    /// `xmlSchemaParseWildcardNs`.
    fn wildcard(&mut self, cx: &Cx, node: Id) -> WildId {
        let process = match cx.attr(node, "processContents") {
            None | Some("strict") => Process::Strict,
            Some("skip") => Process::Skip,
            Some("lax") => Process::Lax,
            Some(value) => {
                let value = value.to_owned();
                self.invalid_attr(
                    cx,
                    node,
                    Code::S4sAttrInvalidValue,
                    "processContents",
                    &value,
                    "(strict | skip | lax)",
                );
                Process::Strict
            }
        };
        let ns = match cx.attr(node, "namespace").map(str::to_owned).as_deref() {
            None | Some("##any") => NsConstraint::Any,
            Some("##other") => NsConstraint::Not(cx.tns),
            Some(list) => {
                let mut set = Vec::new();
                for item in list.split(blank).filter(|s| !s.is_empty()) {
                    let ns = match item {
                        "##other" | "##any" => {
                            self.invalid_attr(cx, node, Code::WildcardInvalidNsMember, "namespace", item, "((##any | ##other) | List of (xs:anyURI | (##targetNamespace | ##local)))");
                            continue;
                        }
                        "##targetNamespace" => cx.tns,
                        "##local" => NO_NS,
                        uri => self.drafts.names.intern(uri),
                    };
                    if !set.contains(&ns) {
                        set.push(ns);
                    }
                }
                NsConstraint::Set(set)
            }
        };
        let wildcard = (Wildcard { ns, process }, cx.loc(node));
        // A refusal fails the build, so the index is never read.
        self.add(|d| &mut d.wildcards, wildcard).unwrap_or_default()
    }

    fn any(&mut self, cx: &Cx, node: Id) -> Option<Particle> {
        self.check_attrs(
            cx,
            node,
            &[
                "id",
                "minOccurs",
                "maxOccurs",
                "namespace",
                "processContents",
            ],
        );
        self.check_id(cx, node);
        let (min, max) = self.occurs(cx, node, false);
        let wildcard = self.wildcard(cx, node);
        self.annotation_only(cx, node);
        (max != Some(0)).then(|| Particle {
            min,
            max,
            term: Term::Wildcard(wildcard),
            loc: cx.loc(node),
        })
    }

    fn any_attribute(&mut self, cx: &Cx, node: Id) -> WildId {
        self.check_attrs(cx, node, &["id", "namespace", "processContents"]);
        self.check_id(cx, node);
        let wildcard = self.wildcard(cx, node);
        self.annotation_only(cx, node);
        wildcard
    }

    /// `xmlSchemaParseFacet`.
    fn facet(&mut self, cx: &Cx, node: Id, kind: &'static str) -> Option<RawFacet> {
        let Some(value) = cx.attr(node, "value").map(str::to_owned) else {
            self.error(
                cx,
                node,
                Code::FacetNoValue,
                &format!("Facet {kind} has no value."),
            );
            return None;
        };
        self.check_id(cx, node);
        let fixed =
            !matches!(kind, "pattern" | "enumeration") && cx.attr(node, "fixed") == Some("true");
        let mut children = Children::new(&cx.tree, node);
        self.skip_annotation(cx, &mut children);
        if children.peek().is_some() {
            self.error(
                cx,
                node,
                Code::UnknownFacetChild,
                &format!("Facet {kind} has unexpected child content."),
            );
        }
        Some(RawFacet {
            kind,
            value,
            fixed,
            loc: cx.loc(node),
        })
    }

    // ---- attributes --------------------------------------------------------

    /// `xmlSchemaParseLocalAttributes`: attribute uses and group references.
    fn local_attributes(
        &mut self,
        cx: &Cx,
        children: &mut Children,
        parent: &str,
    ) -> Vec<AttrItem> {
        let mut items = Vec::new();
        while let Some((node, kind)) = children.take_any(cx, &["attribute", "attributeGroup"]) {
            let item = match kind {
                "attribute" => self.local_attribute(cx, node, parent, &items),
                _ => self.attribute_group_ref(cx, node),
            };
            items.extend(item);
        }
        items
    }

    fn value_constraint(&mut self, cx: &Cx, node: Id, code: Code) -> Option<(bool, String)> {
        match (cx.attr(node, "default"), cx.attr(node, "fixed")) {
            (Some(_), Some(_)) => {
                self.error(
                    cx,
                    node,
                    code,
                    "The attributes 'default' and 'fixed' are mutually exclusive.",
                );
                cx.attr(node, "default").map(|v| (false, v.to_owned()))
            }
            (Some(value), None) => Some((false, value.to_owned())),
            (None, Some(value)) => Some((true, value.to_owned())),
            (None, None) => None,
        }
    }

    /// `xmlSchemaParseLocalAttribute`.
    fn local_attribute(
        &mut self,
        cx: &Cx,
        node: Id,
        parent: &str,
        previous: &[AttrItem],
    ) -> Option<AttrItem> {
        let loc = cx.loc(node);
        let reference = match cx.attr(node, "ref") {
            Some(_) => Some(self.reference(cx, node, "ref")?),
            None => None,
        };
        let errors = self.drafts.diagnostics.len();
        let allowed: &[&str] = match reference {
            Some(_) => &["id", "ref", "use", "default", "fixed"],
            None => &["name", "id", "type", "form", "use", "default", "fixed"],
        };
        self.check_attrs(cx, node, allowed);
        self.check_id(cx, node);
        let ty = match (reference, cx.attr(node, "type")) {
            (None, Some(_)) => self.qname(cx, node, "type"),
            _ => None,
        };
        let use_ = cx.attr(node, "use").map(str::to_owned);
        let (required, prohibited) = match use_.as_deref() {
            None | Some("optional") => (false, false),
            Some("required") => (true, false),
            Some("prohibited") => (false, true),
            Some(value) => {
                self.invalid_attr(
                    cx,
                    node,
                    Code::InvalidAttrUse,
                    "use",
                    value,
                    "(optional | prohibited | required)",
                );
                (false, false)
            }
        };
        let value = self.value_constraint(cx, node, Code::SrcAttribute1);
        if value.as_ref().is_some_and(|(fixed, _)| !fixed) && (required || prohibited) {
            self.error(
                cx,
                node,
                Code::SrcAttribute2,
                "The value of the attribute 'use' must be 'optional' if the attribute 'default' is present.",
            );
        }
        let ns = match cx.attr(node, "form") {
            Some("qualified") => cx.tns,
            Some("unqualified") => NO_NS,
            None if cx.attribute_form => cx.tns,
            None => NO_NS,
            Some(form) => {
                let form = form.to_owned();
                self.invalid_attr(
                    cx,
                    node,
                    Code::S4sAttrInvalidValue,
                    "form",
                    &form,
                    "(qualified | unqualified)",
                );
                NO_NS
            }
        };
        if errors != self.drafts.diagnostics.len() {
            return None;
        }
        let name = match reference {
            Some(name) => name,
            None => {
                if self.drafts.names.str(ns) == XSI {
                    self.error(
                        cx,
                        node,
                        Code::NoXsi,
                        &format!("The target namespace must not match '{XSI}'."),
                    );
                }
                if cx.attr(node, "name").is_none() {
                    self.missing_attr(cx, node, "name");
                    return None;
                }
                let local = self.ncname(cx, node, "name")?;
                if self.drafts.names.str(local) == "xmlns" {
                    self.error(
                        cx,
                        node,
                        Code::NoXmlns,
                        "The value of the attribute must not match 'xmlns'.",
                    );
                    return None;
                }
                QName { ns, local }
            }
        };
        let mut children = Children::new(&cx.tree, node);
        self.skip_annotation(cx, &mut children);
        if prohibited {
            if let Some(child) = children.peek() {
                self.not_allowed(cx, node, child, "(annotation?)");
            }
            let pointless = match parent {
                "attributeGroup" => Some("pointless inside an <attributeGroup>"),
                "extension" => Some("pointless when extending a type"),
                _ => None,
            };
            if let Some(why) = pointless {
                let message = format!("Skipping attribute use prohibition, since it is {why}.");
                self.error(cx, node, Code::WarnAttrPointlessProh, &message);
                return None;
            }
            if previous
                .iter()
                .any(|item| matches!(item, AttrItem::Prohibit(other, ..) if *other == name))
            {
                let message = format!(
                    "Skipping duplicate attribute use prohibition '{}'.",
                    self.drafts.names.format(name)
                );
                self.error(cx, node, Code::WarnAttrPointlessProh, &message);
                return None;
            }
            return Some(AttrItem::Prohibit(name, loc, reference.is_some()));
        }
        let decl = match reference {
            Some(name) => {
                if let Some(child) = children.peek() {
                    let code = match cx.is(Some(child), "simpleType") {
                        true => Code::SrcAttribute3_2,
                        false => Code::S4sElemNotAllowed,
                    };
                    self.error(
                        cx,
                        child,
                        code,
                        "This element is not expected. Expected is ( annotation? ).",
                    );
                }
                return Some(AttrItem::Use(UseDraft {
                    decl: Ref::Named(name),
                    required,
                    value,
                    loc,
                }));
            }
            None => {
                let inline = match children.take(cx, "simpleType") {
                    Some(_) if ty.is_some() => {
                        self.error(
                            cx,
                            node,
                            Code::SrcAttribute4,
                            "The attribute 'type' and the <simpleType> child are mutually exclusive.",
                        );
                        None
                    }
                    Some(child) => self.simple_type(cx, child, false),
                    None => None,
                };
                if let Some(child) = children.peek() {
                    self.not_allowed(cx, node, child, "(annotation?, simpleType?)");
                }
                let ty = inline.map(Ref::Id).or(ty.map(Ref::Named));
                let draft = AttrDraft {
                    name,
                    loc,
                    ty,
                    value,
                };
                self.add(|d| &mut d.attributes, draft)?
            }
        };
        Some(AttrItem::Use(UseDraft {
            decl: Ref::Id(decl),
            required,
            value: None,
            loc,
        }))
    }

    fn global_attribute(&mut self, cx: &Cx, node: Id) {
        if cx.attr(node, "name").is_none() {
            return self.missing_attr(cx, node, "name");
        }
        let Some(local) = self.ncname(cx, node, "name") else {
            return;
        };
        if self.drafts.names.str(local) == "xmlns" {
            return self.error(
                cx,
                node,
                Code::NoXmlns,
                "The value of the attribute must not match 'xmlns'.",
            );
        }
        if self.drafts.names.str(cx.tns) == XSI {
            self.error(
                cx,
                node,
                Code::NoXsi,
                &format!("The target namespace must not match '{XSI}'."),
            );
        }
        self.check_attrs(cx, node, &["id", "default", "fixed", "name", "type"]);
        let ty = self.qname(cx, node, "type");
        self.check_id(cx, node);
        let value = self.value_constraint(cx, node, Code::SrcAttribute1);
        let value = match (cx.attr(node, "default"), cx.attr(node, "fixed")) {
            (Some(_), Some(fixed)) => Some((true, fixed.to_owned())),
            _ => value,
        };
        let mut children = Children::new(&cx.tree, node);
        self.skip_annotation(cx, &mut children);
        let inline = match children.take(cx, "simpleType") {
            Some(_) if ty.is_some() => {
                self.error(
                    cx,
                    node,
                    Code::SrcAttribute4,
                    "The attribute 'type' and the <simpleType> child are mutually exclusive.",
                );
                None
            }
            Some(child) => self.simple_type(cx, child, false),
            None => None,
        };
        if let Some(child) = children.peek() {
            self.not_allowed(cx, node, child, "(annotation?, simpleType?)");
        }
        let name = QName { ns: cx.tns, local };
        let draft = AttrDraft {
            name,
            loc: cx.loc(node),
            ty: inline.map(Ref::Id).or(ty.map(Ref::Named)),
            value,
        };
        if let Some(id) = self.add(|d| &mut d.attributes, draft) {
            self.global(cx, GlobalKind::Attribute, name, id, node);
        }
    }

    fn attribute_group_ref(&mut self, cx: &Cx, node: Id) -> Option<AttrItem> {
        if cx.attr(node, "ref").is_none() {
            self.missing_attr(cx, node, "ref");
            return None;
        }
        let name = self.qname(cx, node, "ref")?;
        if !self.check_reference(cx, node, name.ns) {
            return None;
        }
        self.check_attrs(cx, node, &["ref", "id"]);
        self.check_id(cx, node);
        self.annotation_only(cx, node);
        let loc = cx.loc(node);
        if let Some(redefine) = self.current_redefine(cx, GlobalKind::AttrGroup)
            && self.drafts.redefines[redefine].name == name
        {
            if std::mem::replace(&mut self.drafts.redefines[redefine].self_ref, true) {
                let message = format!(
                    "The redefining attribute group definition '{}' must not contain more than one reference to the redefined definition.",
                    self.drafts.names.format(name)
                );
                self.error(cx, node, Code::SrcRedefine, &message);
                return None;
            }
            return Some(AttrItem::RedefinedGroup(redefine));
        }
        Some(AttrItem::Group(name, loc))
    }

    /// The redefinition being parsed, if it is of `kind`.
    fn current_redefine(&self, cx: &Cx, kind: GlobalKind) -> Option<usize> {
        cx.redefining?;
        let last = self.drafts.redefines.len().checked_sub(1)?;
        (self.drafts.redefines[last].kind == kind).then_some(last)
    }

    fn attribute_group(&mut self, cx: &Cx, node: Id) {
        if cx.attr(node, "name").is_none() {
            return self.missing_attr(cx, node, "name");
        }
        let Some(local) = self.ncname(cx, node, "name") else {
            return;
        };
        let name = QName { ns: cx.tns, local };
        let draft = AttrGroupDraft {
            name,
            loc: cx.loc(node),
            attrs: Vec::new(),
            wildcard: None,
        };
        let Some(id) = self.add(|d| &mut d.attr_groups, draft) else {
            return;
        };
        self.global(cx, GlobalKind::AttrGroup, name, id, node);
        self.check_attrs(cx, node, &["name", "id"]);
        self.check_id(cx, node);
        let mut children = Children::new(&cx.tree, node);
        self.skip_annotation(cx, &mut children);
        let attrs = self.local_attributes(cx, &mut children, "attributeGroup");
        let wildcard = children
            .take(cx, "anyAttribute")
            .map(|child| self.any_attribute(cx, child));
        if let Some(child) = children.peek() {
            self.not_allowed(
                cx,
                node,
                child,
                "(annotation?, ((attribute | attributeGroup)*, anyAttribute?))",
            );
        }
        self.drafts.attr_groups[id].attrs = attrs;
        self.drafts.attr_groups[id].wildcard = wildcard;
    }

    // ---- elements and identity constraints ---------------------------------

    /// `xmlSchemaParseElement`: a global declaration, or a local particle.
    fn element(&mut self, cx: &Cx, node: Id, top: bool) -> Option<Particle> {
        let loc = cx.loc(node);
        let has_name = cx.attr(node, "name").is_some();
        let is_ref = !top && cx.attr(node, "ref").is_some();
        if !is_ref && !has_name {
            self.missing_attr(cx, node, "name");
            return None;
        }
        self.check_id(cx, node);
        let mut children = Children::new(&cx.tree, node);
        self.skip_annotation(cx, &mut children);
        let (min, max) = match top {
            true => (1, Some(1)),
            false => self.occurs(cx, node, false),
        };
        if is_ref {
            let name = self.qname(cx, node, "ref");
            if let Some(name) = name {
                self.check_reference(cx, node, name.ns);
            }
            if has_name {
                self.error(
                    cx,
                    node,
                    Code::SrcElement2_1,
                    "The attributes 'ref' and 'name' are mutually exclusive.",
                );
            }
            let extra = attributes(&*cx.tree, node).find(|(_, attr, _)| {
                attr.namespace.is_none()
                    && !["ref", "name", "id", "maxOccurs", "minOccurs"].contains(&attr.qualified)
            });
            if extra.is_some() {
                self.error(
                    cx,
                    node,
                    Code::SrcElement2_2,
                    "Only the attributes 'minOccurs', 'maxOccurs' and 'id' are allowed in addition to 'ref'.",
                );
            }
            let xsd: Vec<String> = attributes(&*cx.tree, node)
                .filter(|(_, attr, _)| attr.namespace == Some(XS))
                .map(|(_, attr, _)| attr.qualified.to_owned())
                .collect();
            for attr in xsd {
                self.error(
                    cx,
                    node,
                    Code::S4sAttrNotAllowed,
                    &format!("The attribute '{attr}' is not allowed."),
                );
            }
            if let Some(child) = children.peek() {
                self.not_allowed(cx, node, child, "(annotation?)");
            }
            return (max != Some(0)).then_some(Particle {
                min,
                max,
                term: Term::ElementRef(name?),
                loc,
            });
        }
        let local = self.ncname(cx, node, "name")?;
        let ns = match top {
            true => cx.tns,
            false => match cx.attr(node, "form") {
                Some("qualified") => cx.tns,
                Some("unqualified") => NO_NS,
                None if cx.element_form => cx.tns,
                None => NO_NS,
                Some(form) => {
                    let form = form.to_owned();
                    self.invalid_attr(
                        cx,
                        node,
                        Code::S4sAttrInvalidValue,
                        "form",
                        &form,
                        "(qualified | unqualified)",
                    );
                    NO_NS
                }
            },
        };
        let name = QName { ns, local };
        let allowed: &[&str] = match top {
            true => &[
                "name",
                "type",
                "id",
                "default",
                "fixed",
                "block",
                "nillable",
                "final",
                "abstract",
                "substitutionGroup",
            ],
            false => &[
                "name",
                "type",
                "id",
                "default",
                "fixed",
                "block",
                "nillable",
                "maxOccurs",
                "minOccurs",
                "form",
            ],
        };
        self.check_attrs(cx, node, allowed);
        let mut draft = ElemDraft {
            name,
            loc,
            ty: None,
            head: None,
            nillable: false,
            abstract_: false,
            value: None,
            block: cx.block_default & (EXTENSION | RESTRICTION | SUBSTITUTION),
            final_: cx.final_default & (EXTENSION | RESTRICTION),
            idcs: Vec::new(),
        };
        if top {
            draft.head = self.qname(cx, node, "substitutionGroup");
            draft.abstract_ = self.boolean(cx, node, "abstract");
            if let Some(value) = cx.attr(node, "final").map(str::to_owned) {
                match derivation_set(&value, EXTENSION | RESTRICTION) {
                    Some(set) => draft.final_ = set,
                    None => self.invalid_attr(
                        cx,
                        node,
                        Code::S4sAttrInvalidValue,
                        "final",
                        &value,
                        "(#all | List of (extension | restriction))",
                    ),
                }
            }
        }
        if let Some(value) = cx.attr(node, "block").map(str::to_owned) {
            match derivation_set(&value, EXTENSION | RESTRICTION | SUBSTITUTION) {
                Some(set) => draft.block = set,
                None => self.invalid_attr(
                    cx,
                    node,
                    Code::S4sAttrInvalidValue,
                    "block",
                    &value,
                    "(#all | List of (extension | restriction | substitution))",
                ),
            }
        }
        draft.nillable = self.boolean(cx, node, "nillable");
        let named_type = match cx.attr(node, "type") {
            Some(_) => {
                let ty = self.qname(cx, node, "type");
                if let Some(ty) = ty {
                    self.check_reference(cx, node, ty.ns);
                }
                ty
            }
            None => None,
        };
        draft.value = self.value_constraint(cx, node, Code::SrcElement1);
        let inline = match children.take_any(cx, &["complexType", "simpleType"]) {
            Some(_) if named_type.is_some() => {
                self.error(
                    cx,
                    node,
                    Code::SrcElement3,
                    "The attribute 'type' and the <complexType> child are mutually exclusive.",
                );
                None
            }
            Some((child, "complexType")) => self.complex_type(cx, child, false),
            Some((child, _)) => self.simple_type(cx, child, false),
            None => None,
        };
        draft.ty = inline.map(Ref::Id).or(named_type.map(Ref::Named));
        while let Some((child, kind)) = children.take_any(cx, &["unique", "key", "keyref"]) {
            let kind = match kind {
                "unique" => IdcKind::Unique,
                "key" => IdcKind::Key,
                _ => IdcKind::KeyRef,
            };
            if let Some(idc) = self.idc(cx, child, kind, ns) {
                draft.idcs.push(idc);
            }
        }
        if let Some(child) = children.peek() {
            self.not_allowed(
                cx,
                node,
                child,
                "(annotation?, ((simpleType | complexType)?, (unique | key | keyref)*))",
            );
        }
        let id = self.add(|d| &mut d.elements, draft)?;
        if top {
            self.global(cx, GlobalKind::Element, name, id, node);
            return None;
        }
        Some(Particle {
            min,
            max,
            term: Term::Element(id),
            loc,
        })
    }

    fn xpath(&mut self, cx: &Cx, node: Id, flags: u32) -> Option<(Box<str>, Pattern)> {
        self.check_attrs(cx, node, &["id", "xpath"]);
        let Some(xpath) = cx.attr(node, "xpath").map(str::to_owned) else {
            self.missing_attr(cx, node, "name");
            return None;
        };
        let scope = namespaces(&*cx.tree, node);
        let bindings: Vec<(&str, &str)> = scope
            .into_iter()
            .filter(|(prefix, _)| !prefix.is_empty() && *prefix != "xml")
            .collect();
        let compiled = Pattern::compile(&xpath, &bindings, flags);
        self.check_id(cx, node);
        self.annotation_only(cx, node);
        match compiled {
            Ok(pattern) => Some((xpath.into(), pattern)),
            Err(_) => {
                let message = format!("The XPath expression '{xpath}' could not be compiled.");
                self.error(cx, node, Code::S4sAttrInvalidValue, &message);
                None
            }
        }
    }

    /// `xmlSchemaParseIDC`.
    fn idc(&mut self, cx: &Cx, node: Id, kind: IdcKind, ns: Sym) -> Option<IdcId> {
        let allowed: &[&str] = match kind {
            IdcKind::KeyRef => &["id", "name", "refer"],
            _ => &["id", "name"],
        };
        self.check_attrs(cx, node, allowed);
        if cx.attr(node, "name").is_none() {
            self.missing_attr(cx, node, "name");
            return None;
        }
        let local = self.ncname(cx, node, "name")?;
        let name = QName { ns, local };
        self.check_id(cx, node);
        let refer = match kind {
            IdcKind::KeyRef if cx.attr(node, "refer").is_none() => {
                self.missing_attr(cx, node, "refer");
                None
            }
            IdcKind::KeyRef => {
                let refer = self.qname(cx, node, "refer");
                if let Some(refer) = refer {
                    self.check_reference(cx, node, refer.ns);
                }
                refer
            }
            _ => None,
        };
        let mut children = Children::new(&cx.tree, node);
        self.skip_annotation(cx, &mut children);
        let expected = "(annotation?, (selector, field+))";
        if children.peek().is_none() {
            self.error(
                cx,
                node,
                Code::S4sElemMissing,
                &format!("A child element is missing. Expected is ( {expected} )."),
            );
        }
        let mut selector = None;
        let mut fields = Vec::new();
        if let Some(child) = children.take(cx, "selector") {
            selector = self.xpath(cx, child, pattern::XS_SELECTOR).map(|(_, p)| p);
            if children
                .peek()
                .is_none_or(|next| cx.local(next) != Some("field"))
            {
                if let Some(next) = children.peek() {
                    self.not_allowed(cx, node, next, expected);
                    children.bump();
                } else {
                    self.error(
                        cx,
                        node,
                        Code::S4sElemNotAllowed,
                        &format!("Expected is ( {expected} )."),
                    );
                }
            }
            while let Some(child) = children.take(cx, "field") {
                fields.extend(self.xpath(cx, child, pattern::XS_FIELD));
            }
        }
        if let Some(child) = children.peek() {
            self.not_allowed(cx, node, child, expected);
        }
        let draft = IdcDraft {
            kind,
            name,
            loc: cx.loc(node),
            selector,
            fields,
            refer,
        };
        let id = self.add(|d| &mut d.idcs, draft)?;
        self.global(cx, GlobalKind::Idc, name, id, node);
        Some(id)
    }

    // ---- model groups --------------------------------------------------------

    /// `xmlSchemaParseModelGroup`.
    fn model_group(
        &mut self,
        cx: &Cx,
        node: Id,
        compositor: Compositor,
        with_particle: bool,
    ) -> Option<Particle> {
        let (min, max) = match (with_particle, compositor) {
            (false, _) => (1, Some(1)),
            (true, Compositor::All) => self.occurs(cx, node, true),
            (true, _) => self.occurs(cx, node, false),
        };
        let allowed: &[&str] = if with_particle {
            &["id", "maxOccurs", "minOccurs"]
        } else {
            &["id"]
        };
        self.check_attrs(cx, node, allowed);
        self.check_id(cx, node);
        let mut children = Children::new(&cx.tree, node);
        self.skip_annotation(cx, &mut children);
        let mut particles = Vec::new();
        if compositor == Compositor::All {
            while let Some(child) = children.take(cx, "element") {
                if let Some(mut particle) = self.element(cx, child, false) {
                    if particle.min > 1 {
                        self.error(
                            cx,
                            child,
                            Code::CosAllLimited,
                            "Invalid value for minOccurs (must be 0 or 1).",
                        );
                        particle.min = 1;
                    }
                    if particle.max.is_none_or(|max| max > 1) {
                        self.error(
                            cx,
                            child,
                            Code::CosAllLimited,
                            "Invalid value for maxOccurs (must be 0 or 1).",
                        );
                        particle.max = Some(1);
                    }
                    if particles.try_push(particle).is_err() {
                        self.drafts.refuse();
                        return None;
                    }
                }
            }
            if let Some(child) = children.peek() {
                self.not_allowed(cx, node, child, "(annotation?, (annotation?, element*)");
            }
        } else {
            while let Some((child, kind)) =
                children.take_any(cx, &["element", "group", "any", "choice", "sequence"])
            {
                let particle = match kind {
                    "element" => self.element(cx, child, false),
                    "group" => self.group_ref(cx, child),
                    "any" => self.any(cx, child),
                    "choice" => self.model_group(cx, child, Compositor::Choice, true),
                    _ => self.model_group(cx, child, Compositor::Sequence, true),
                };
                if particles.try_reserve(1).is_err() {
                    self.drafts.refuse();
                    return None;
                }
                particles.extend(particle);
            }
            if let Some(child) = children.peek() {
                self.not_allowed(
                    cx,
                    node,
                    child,
                    "(annotation?, (element | group | choice | sequence | any)*)",
                );
            }
        }
        (max != Some(0)).then(|| Particle {
            min,
            max,
            term: Term::Group(compositor, particles),
            loc: cx.loc(node),
        })
    }

    /// `xmlSchemaParseModelGroupDefRef`.
    fn group_ref(&mut self, cx: &Cx, node: Id) -> Option<Particle> {
        if cx.attr(node, "ref").is_none() {
            self.missing_attr(cx, node, "ref");
            return None;
        }
        let name = self.qname(cx, node, "ref")?;
        self.check_reference(cx, node, name.ns);
        let (min, max) = self.occurs(cx, node, false);
        self.check_attrs(cx, node, &["ref", "id", "minOccurs", "maxOccurs"]);
        self.check_id(cx, node);
        self.annotation_only(cx, node);
        let mut term = Term::GroupRef(name);
        if let Some(redefine) = self.current_redefine(cx, GlobalKind::Group)
            && self.drafts.redefines[redefine].name == name
        {
            let named = self.drafts.names.format(name);
            if self.drafts.redefines[redefine].self_ref {
                let message = format!(
                    "The redefining model group definition '{named}' must not contain more than one reference to the redefined definition."
                );
                self.error(cx, node, Code::SrcRedefine, &message);
                return None;
            }
            self.drafts.redefines[redefine].self_ref = true;
            if min != 1 || max != Some(1) {
                let message = format!(
                    "The redefining model group definition '{named}' must not contain a reference to the redefined definition with a maxOccurs/minOccurs other than 1."
                );
                self.error(cx, node, Code::SrcRedefine, &message);
                return None;
            }
            term = Term::RedefinedGroup(redefine);
        }
        (max != Some(0)).then(|| Particle {
            min,
            max,
            term,
            loc: cx.loc(node),
        })
    }

    /// `xmlSchemaParseModelGroupDefinition`.
    fn group_definition(&mut self, cx: &Cx, node: Id) {
        if cx.attr(node, "name").is_none() {
            return self.missing_attr(cx, node, "name");
        }
        let Some(local) = self.ncname(cx, node, "name") else {
            return;
        };
        let name = QName { ns: cx.tns, local };
        let draft = GroupDraft {
            name,
            loc: cx.loc(node),
            model: None,
        };
        let Some(id) = self.add(|d| &mut d.groups, draft) else {
            return;
        };
        self.global(cx, GlobalKind::Group, name, id, node);
        self.check_attrs(cx, node, &["name", "id"]);
        self.check_id(cx, node);
        let mut children = Children::new(&cx.tree, node);
        self.skip_annotation(cx, &mut children);
        let model = match children.take_any(cx, &["all", "choice", "sequence"]) {
            Some((child, kind)) => {
                let compositor = match kind {
                    "all" => Compositor::All,
                    "choice" => Compositor::Choice,
                    _ => Compositor::Sequence,
                };
                self.model_group(cx, child, compositor, false)
                    .and_then(|particle| match particle.term {
                        Term::Group(compositor, particles) => {
                            Some((compositor, particles, particle.loc))
                        }
                        _ => None,
                    })
            }
            None => None,
        };
        if let Some(child) = children.peek() {
            self.not_allowed(cx, node, child, "(annotation?, (all | choice | sequence)?)");
        }
        self.drafts.groups[id].model = model;
    }

    // ---- type definitions ----------------------------------------------------

    fn new_type(&mut self, name: Option<QName>, loc: Loc, shape: Shape) -> Option<usize> {
        let draft = TypeDraft {
            name,
            loc,
            shape,
            derivation: RESTRICTION,
            base: None,
            content_type: None,
            facets: Vec::new(),
            particle: None,
            attrs: Vec::new(),
            wildcard: None,
            mixed: false,
            final_: 0,
            block: 0,
            abstract_: false,
        };
        self.add(|d| &mut d.types, draft)
    }

    /// `xmlSchemaParseSimpleType`.
    fn simple_type(&mut self, cx: &Cx, node: Id, top: bool) -> Option<TypeId> {
        let name = match top {
            false => None,
            true => {
                if cx.attr(node, "name").is_none() {
                    self.missing_attr(cx, node, "name");
                    return None;
                }
                let local = self.ncname(cx, node, "name")?;
                Some(QName { ns: cx.tns, local })
            }
        };
        if self.drafts.names.str(cx.tns) == XS && top {
            if cx.redefining.is_some() {
                self.error(
                    cx,
                    node,
                    Code::SrcRedefine,
                    "Redefinition of built-in simple types is not supported.",
                );
                return None;
            }
            let builtin = name
                .and_then(|n| crate::schema_types::Type::from_name(self.drafts.names.str(n.local)));
            if let Some(builtin) = builtin {
                return Some(super::components::builtin(builtin));
            }
        }
        let index = self.new_type(name, cx.loc(node), Shape::Restriction)?;
        self.check_attrs(
            cx,
            node,
            if top {
                &["id", "name", "final"]
            } else {
                &["id"]
            },
        );
        if top {
            let mut final_ = cx.final_default & (RESTRICTION | LIST | UNION);
            if let Some(value) = cx.attr(node, "final").map(str::to_owned) {
                match derivation_set(&value, RESTRICTION | LIST | UNION) {
                    Some(set) => final_ = set,
                    None => self.invalid_attr(
                        cx,
                        node,
                        Code::S4sAttrInvalidValue,
                        "final",
                        &value,
                        "(#all | List of (list | union | restriction)",
                    ),
                }
            }
            self.drafts.types[index].final_ = final_;
            self.global(
                cx,
                GlobalKind::Type,
                name.expect("named"),
                Drafts::type_id(index),
                node,
            );
        }
        self.check_id(cx, node);
        let mut children = Children::new(&cx.tree, node);
        self.skip_annotation(cx, &mut children);
        let expected = "(annotation?, (restriction | list | union))";
        let mut restriction = false;
        match children.take_any(cx, &["restriction", "list", "union"]) {
            None => {
                let message = format!("The content is not valid. Expected is {expected}.");
                self.error(cx, node, Code::S4sElemMissing, &message);
            }
            Some((child, "restriction")) => {
                restriction = true;
                self.restriction(cx, child, index, "simpleType");
            }
            Some((child, "list")) => self.list(cx, child, index),
            Some((child, _)) => self.union(cx, child, index),
        }
        if let Some(child) = children.peek() {
            self.not_allowed(cx, node, child, expected);
        }
        if top && cx.redefining.is_some() && !restriction {
            self.error(
                cx,
                node,
                Code::SrcRedefine,
                "This is a redefinition, thus the <simpleType> must have a <restriction> child.",
            );
        }
        Some(Drafts::type_id(index))
    }

    fn list(&mut self, cx: &Cx, node: Id, index: usize) {
        self.check_attrs(cx, node, &["id", "itemType"]);
        self.check_id(cx, node);
        let item = self.qname(cx, node, "itemType");
        let mut children = Children::new(&cx.tree, node);
        self.skip_annotation(cx, &mut children);
        let inline = match children.take(cx, "simpleType") {
            Some(_) if cx.attr(node, "itemType").is_some() => {
                self.error(
                    cx,
                    node,
                    Code::SrcSimpleType1,
                    "The attribute 'itemType' and the <simpleType> child are mutually exclusive.",
                );
                None
            }
            Some(child) => self.simple_type(cx, child, false),
            None => {
                if cx.attr(node, "itemType").is_none() {
                    self.error(cx, node, Code::SrcSimpleType1, "Either the attribute 'itemType' or the <simpleType> child must be present.");
                }
                None
            }
        };
        if let Some(child) = children.peek() {
            self.not_allowed(cx, node, child, "(annotation?, simpleType?)");
        }
        let draft = &mut self.drafts.types[index];
        draft.shape = Shape::List(inline.map(Ref::Id).or(item.map(Ref::Named)));
        draft.base = Some(Ref::Id(builtin(crate::schema_types::Type::AnySimpleType)));
    }

    fn union(&mut self, cx: &Cx, node: Id, index: usize) {
        self.check_attrs(cx, node, &["id", "memberTypes"]);
        self.check_id(cx, node);
        let mut members = Vec::new();
        let list = cx.attr(node, "memberTypes").map(str::to_owned);
        for item in list
            .iter()
            .flat_map(|list| list.split(blank))
            .filter(|s| !s.is_empty())
        {
            members.extend(
                self.qname_value(cx, node, "memberTypes", item)
                    .map(Ref::Named),
            );
        }
        let mut children = Children::new(&cx.tree, node);
        self.skip_annotation(cx, &mut children);
        let mut any_inline = false;
        while let Some(child) = children.take(cx, "simpleType") {
            any_inline = true;
            members.extend(self.simple_type(cx, child, false).map(Ref::Id));
        }
        if let Some(child) = children.peek() {
            self.not_allowed(cx, node, child, "(annotation?, simpleType*)");
        }
        if list.is_none() && !any_inline {
            self.error(
                cx,
                node,
                Code::SrcUnionMemberTypesOrSimpleTypes,
                "Either the attribute 'memberTypes' or at least one <simpleType> child must be present.",
            );
        }
        let draft = &mut self.drafts.types[index];
        draft.shape = Shape::Union(members);
        draft.base = Some(Ref::Id(builtin(crate::schema_types::Type::AnySimpleType)));
    }

    /// `xmlSchemaParseRestriction` for simple types, simple and complex content.
    fn restriction(&mut self, cx: &Cx, node: Id, index: usize, parent: &str) {
        self.check_attrs(cx, node, &["id", "base"]);
        self.check_id(cx, node);
        let complex = parent != "simpleType";
        let mut base = self.qname(cx, node, "base");
        let global = self.drafts.types[index].name.is_some();
        if cx.attr(node, "base").is_none() && complex {
            self.missing_attr(cx, node, "base");
        } else if cx.redefining.is_some() && global && cx.attr(node, "base").is_some() {
            if base.is_some() && base != self.drafts.types[index].name {
                let message = "This is a redefinition, but the QName value of the 'base' attribute does not match the type's designation.";
                self.error(cx, node, Code::SrcRedefine, message);
                base = None;
            }
        } else if cx.redefining.is_some() && global && cx.attr(node, "base").is_none() {
            self.missing_attr(cx, node, "base");
        }
        let mut children = Children::new(&cx.tree, node);
        self.skip_annotation(cx, &mut children);
        let mut inline = None;
        match parent {
            "simpleType" => {
                match children.take(cx, "simpleType") {
                    Some(child) if cx.attr(node, "base").is_some() => {
                        self.error(cx, child, Code::SrcRestrictionBaseOrSimpleType, "The attribute 'base' and the <simpleType> child are mutually exclusive.");
                    }
                    Some(child) => inline = self.simple_type(cx, child, false),
                    None if cx.attr(node, "base").is_none() => {
                        self.error(
                            cx,
                            node,
                            Code::SrcRestrictionBaseOrSimpleType,
                            "Either the attribute 'base' or a <simpleType> child must be present.",
                        );
                    }
                    None => {}
                }
            }
            "complexContent" => {
                self.drafts.types[index].particle = self.content_particle(cx, &mut children);
            }
            _ => {
                if let Some(child) = children.take(cx, "simpleType") {
                    match self.simple_type(cx, child, false) {
                        Some(ty) => self.drafts.types[index].content_type = Some(ty),
                        None => return,
                    }
                }
            }
        }
        if !complex || parent == "simpleContent" {
            while let Some((child, kind)) = children.take_any(cx, &FACETS) {
                let facet = self.facet(cx, child, kind);
                if facet
                    .is_some_and(|facet| self.add(|d| &mut d.types[index].facets, facet).is_none())
                {
                    return;
                }
            }
        }
        if complex {
            let attrs = self.local_attributes(cx, &mut children, "restriction");
            let wildcard = children
                .take(cx, "anyAttribute")
                .map(|c| self.any_attribute(cx, c));
            let draft = &mut self.drafts.types[index];
            draft.attrs = attrs;
            draft.wildcard = wildcard;
        }
        if let Some(child) = children.peek() {
            let expected = match parent {
                "complexContent" => {
                    "annotation?, (group | all | choice | sequence)?, ((attribute | attributeGroup)*, anyAttribute?))"
                }
                "simpleContent" => {
                    "(annotation?, (simpleType?, (minExclusive | minInclusive | maxExclusive | maxInclusive | totalDigits | fractionDigits | length | minLength | maxLength | enumeration | whiteSpace | pattern)*)?, ((attribute | attributeGroup)*, anyAttribute?))"
                }
                _ => {
                    "(annotation?, (simpleType?, (minExclusive | minInclusive | maxExclusive | maxInclusive | totalDigits | fractionDigits | length | minLength | maxLength | enumeration | whiteSpace | pattern)*))"
                }
            };
            self.not_allowed(cx, node, child, expected);
        }
        let draft = &mut self.drafts.types[index];
        draft.derivation = RESTRICTION;
        draft.base = inline.map(Ref::Id).or(base.map(Ref::Named));
        if !complex {
            draft.shape = Shape::Restriction;
        }
    }

    fn content_particle(&mut self, cx: &Cx, children: &mut Children) -> Option<Particle> {
        let (child, kind) = children.take_any(cx, &["all", "choice", "sequence", "group"])?;
        match kind {
            "all" => self.model_group(cx, child, Compositor::All, true),
            "choice" => self.model_group(cx, child, Compositor::Choice, true),
            "sequence" => self.model_group(cx, child, Compositor::Sequence, true),
            _ => self.group_ref(cx, child),
        }
    }

    fn extension(&mut self, cx: &Cx, node: Id, index: usize, parent: &str) {
        self.check_attrs(cx, node, &["id", "base"]);
        self.check_id(cx, node);
        let base = self.qname(cx, node, "base");
        if cx.attr(node, "base").is_none() {
            self.missing_attr(cx, node, "base");
        }
        let mut children = Children::new(&cx.tree, node);
        self.skip_annotation(cx, &mut children);
        if parent == "complexContent" {
            self.drafts.types[index].particle = self.content_particle(cx, &mut children);
        }
        let attrs = self.local_attributes(cx, &mut children, "extension");
        let wildcard = children
            .take(cx, "anyAttribute")
            .map(|c| self.any_attribute(cx, c));
        if let Some(child) = children.peek() {
            let expected = match parent {
                "complexContent" => {
                    "(annotation?, ((group | all | choice | sequence)?, ((attribute | attributeGroup)*, anyAttribute?)))"
                }
                _ => "(annotation?, ((attribute | attributeGroup)*, anyAttribute?))",
            };
            self.not_allowed(cx, node, child, expected);
        }
        let draft = &mut self.drafts.types[index];
        draft.derivation = EXTENSION;
        draft.base = base.map(Ref::Named);
        draft.attrs = attrs;
        draft.wildcard = wildcard;
    }

    /// `<simpleContent>` or `<complexContent>`; whether a derivation was found.
    fn content(&mut self, cx: &Cx, node: Id, index: usize, kind: &str) -> bool {
        let allowed: &[&str] = if kind == "complexContent" {
            &["id", "mixed"]
        } else {
            &["id"]
        };
        self.check_attrs(cx, node, allowed);
        self.check_id(cx, node);
        if kind == "complexContent" && self.boolean(cx, node, "mixed") {
            self.drafts.types[index].mixed = true;
        }
        let mut children = Children::new(&cx.tree, node);
        self.skip_annotation(cx, &mut children);
        let expected = "(annotation?, (restriction | extension))";
        if children.peek().is_none() {
            let message = format!("The content is not valid. Expected is {expected}.");
            self.error(cx, node, Code::S4sElemMissing, &message);
        }
        let found = match children.take_any(cx, &["restriction", "extension"]) {
            Some((child, "restriction")) => {
                self.restriction(cx, child, index, kind);
                true
            }
            Some((child, _)) => {
                self.extension(cx, child, index, kind);
                true
            }
            None => false,
        };
        if let Some(child) = children.peek() {
            self.not_allowed(cx, node, child, expected);
        }
        found
    }

    /// `xmlSchemaParseComplexType`.
    fn complex_type(&mut self, cx: &Cx, node: Id, top: bool) -> Option<TypeId> {
        let name = match top {
            false => None,
            true => {
                if cx.attr(node, "name").is_none() {
                    self.missing_attr(cx, node, "name");
                    return None;
                }
                let local = self.ncname(cx, node, "name")?;
                Some(QName { ns: cx.tns, local })
            }
        };
        let index = self.new_type(
            name,
            cx.loc(node),
            Shape::Complex {
                simple_content: false,
            },
        )?;
        if let Some(name) = name {
            self.global(cx, GlobalKind::Type, name, Drafts::type_id(index), node);
        }
        let mut final_ = None;
        let mut block = None;
        let bad: Vec<String> = attributes(&*cx.tree, node)
            .filter(|(_, attr, _)| !attr.is_namespace_declaration())
            .filter(|(_, attr, _)| match attr.namespace {
                Some(ns) => ns == XS,
                None => {
                    !(["id", "mixed"].contains(&attr.qualified)
                        || (top
                            && ["name", "abstract", "final", "block"].contains(&attr.qualified)))
                }
            })
            .map(|(_, attr, _)| attr.qualified.to_owned())
            .collect();
        for attr in bad {
            self.error(
                cx,
                node,
                Code::S4sAttrNotAllowed,
                &format!("The attribute '{attr}' is not allowed."),
            );
        }
        self.check_id(cx, node);
        if cx.attr(node, "mixed").is_some() && self.boolean(cx, node, "mixed") {
            self.drafts.types[index].mixed = true;
        }
        if top {
            self.drafts.types[index].abstract_ =
                cx.attr(node, "abstract").is_some() && self.boolean(cx, node, "abstract");
            for attr in ["final", "block"] {
                let Some(value) = cx.attr(node, attr).map(str::to_owned) else {
                    continue;
                };
                match derivation_set(&value, EXTENSION | RESTRICTION) {
                    Some(set) if attr == "final" => final_ = Some(set),
                    Some(set) => block = Some(set),
                    None => self.invalid_attr(
                        cx,
                        node,
                        Code::S4sAttrInvalidValue,
                        attr,
                        &value,
                        "(#all | List of (extension | restriction))",
                    ),
                }
            }
        }
        let draft = &mut self.drafts.types[index];
        draft.final_ = final_.unwrap_or(cx.final_default & (EXTENSION | RESTRICTION));
        draft.block = block.unwrap_or(cx.block_default & (EXTENSION | RESTRICTION));
        let mut children = Children::new(&cx.tree, node);
        self.skip_annotation(cx, &mut children);
        let mut derived = false;
        match children.take_any(cx, &["simpleContent", "complexContent"]) {
            Some((child, "simpleContent")) => {
                self.drafts.types[index].mixed = false;
                self.drafts.types[index].shape = Shape::Complex {
                    simple_content: true,
                };
                derived = self.content(cx, child, index, "simpleContent");
            }
            Some((child, _)) => derived = self.content(cx, child, index, "complexContent"),
            None => {
                let particle = self.content_particle(cx, &mut children);
                let attrs = self.local_attributes(cx, &mut children, "restriction");
                let wildcard = children
                    .take(cx, "anyAttribute")
                    .map(|c| self.any_attribute(cx, c));
                let draft = &mut self.drafts.types[index];
                draft.base = Some(Ref::Id(ANY_TYPE));
                draft.derivation = RESTRICTION;
                draft.particle = particle;
                draft.attrs = attrs;
                draft.wildcard = wildcard;
            }
        }
        if let Some(child) = children.peek() {
            self.not_allowed(
                cx,
                node,
                child,
                "(annotation?, (simpleContent | complexContent | ((group | all | choice | sequence)?, ((attribute | attributeGroup)*, anyAttribute?))))",
            );
        }
        if top && cx.redefining.is_some() && !derived {
            self.error(cx, node, Code::SrcRedefine, "This is a redefinition, thus the <complexType> must have a <restriction> or <extension> grand-child.");
        }
        Some(Drafts::type_id(index))
    }
}

/// Global component tables by name, built from the bucket graph.
pub(crate) type Table = HashMap<(GlobalKind, QName), usize>;
