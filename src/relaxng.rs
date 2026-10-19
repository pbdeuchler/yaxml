#![doc = include_str!("../docs/relaxng.md")]

mod check;
mod compile;
mod datatype;
mod load;
mod pattern;

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::schema_types::{Context, Type};
use crate::xpath::tree::{Item, Navigate, QName, Tree, attributes, namespaces};
use crate::{OwnedDocument, Resolver};
use pattern::{Arena, Deriver, Id, NOT_ALLOWED, Name, NameClass, Pat, Tables, is_whitespace};

pub(crate) const RNG_NS: &str = "http://relaxng.org/ns/structure/1.0";

/// Why a schema was rejected. The discriminant is libxml2's
/// `xmlParserErrors` value (`XML_RNGP_*`, and `XML_ERR_RESOURCE_LIMIT`).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u16)]
#[non_exhaustive]
pub enum SchemaCode {
    /// A load or work limit was reached.
    ResourceLimit = 114,
    AnyNameAttrAncestor = 1000,
    /// Attributes of interleaved patterns overlap.
    AttrConflict = 1001,
    AttributeChildren = 1002,
    DataContent = 1009,
    DefChoiceAndInterleave = 1010,
    DefineMissing = 1013,
    /// An element's content has no content type (section 7.2).
    ElemContentError = 1016,
    ElementEmpty = 1017,
    /// A malformed name class or name.
    ElementName = 1019,
    ElementNoContent = 1020,
    /// Elements or text of interleaved patterns overlap.
    ElemTextConflict = 1021,
    /// The document element is not a RELAX NG pattern.
    Empty = 1022,
    EmptyConstruct = 1023,
    EmptyNotEmpty = 1025,
    ExceptMultiple = 1029,
    ExternalRefFailure = 1032,
    ExternalRefRecurse = 1033,
    ForbiddenAttribute = 1034,
    GrammarContent = 1036,
    GrammarMissing = 1038,
    GrammarNoStart = 1039,
    GroupAttrConflict = 1040,
    HrefError = 1041,
    IncludeFailure = 1043,
    IncludeRecurse = 1044,
    InvalidDefineName = 1049,
    InvalidValue = 1051,
    MissingHref = 1052,
    NeedCombine = 1054,
    NsNameAttrAncestor = 1056,
    ParamForbidden = 1058,
    ParamNameMissing = 1059,
    ParentRefNoParent = 1063,
    /// A schema document is not well-formed or nesting is too deep.
    ParseError = 1065,
    PatAnyNameExceptAnyName = 1066,
    PatAttrAttr = 1067,
    PatAttrElem = 1068,
    PatDataExceptAttr = 1069,
    PatDataExceptElem = 1070,
    PatDataExceptEmpty = 1071,
    PatDataExceptGroup = 1072,
    PatDataExceptInterleave = 1073,
    PatDataExceptList = 1074,
    PatDataExceptOneMore = 1075,
    PatDataExceptText = 1077,
    PatListAttr = 1078,
    PatListElem = 1079,
    PatListInterleave = 1080,
    PatListList = 1081,
    PatListText = 1083,
    PatNsNameExceptAnyName = 1084,
    PatNsNameExceptNsName = 1085,
    PatOneMoreGroupAttr = 1086,
    PatOneMoreInterleaveAttr = 1087,
    PatStartAttr = 1088,
    PatStartData = 1089,
    PatStartEmpty = 1090,
    PatStartGroup = 1091,
    PatStartInterleave = 1092,
    PatStartList = 1093,
    PatStartOneMore = 1094,
    PatStartText = 1095,
    PatStartValue = 1096,
    PrefixUndefined = 1097,
    RefCycle = 1099,
    RefNoDef = 1101,
    RefNoName = 1102,
    StartChoiceAndInterleave = 1104,
    StartContent = 1105,
    StartMissing = 1107,
    /// Text where the RELAX NG syntax allows none.
    TextExpected = 1108,
    TypeMissing = 1110,
    TypeNotFound = 1111,
    /// An invalid datatype parameter.
    TypeValue = 1112,
    UnknownCombine = 1114,
    UnknownConstruct = 1115,
    UnknownTypeLib = 1116,
    UriFragment = 1117,
    UriNotAbsolute = 1118,
    XmlnsName = 1121,
}

/// One schema error, in the schema document `uri` at the source `offset`
/// of the offending element when it has one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaError {
    pub code: SchemaCode,
    pub uri: String,
    pub offset: Option<usize>,
    /// A libxml2-style description.
    pub message: String,
}

/// The errors that rejected a schema; never empty.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaErrors(pub Vec<SchemaError>);

impl fmt::Display for SchemaErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.0.iter().enumerate() {
            let separator = if index == 0 { "" } else { "\n" };
            write!(f, "{separator}{}: {}", error.uri, error.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for SchemaErrors {}

/// A validity error; the discriminant is libxml2's `xmlRelaxNGValidErr`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u16)]
#[non_exhaustive]
pub enum Code {
    /// Text that its datatype does not allow (`XML_RELAXNG_ERR_TYPE`).
    Datatype = 2,
    /// An `xsd:ID` value used twice (`DUPID`).
    DuplicateId = 4,
    /// Another element was expected (`ELEMNAME`).
    ElementName = 13,
    /// The element lacks the expected namespace (`ELEMNONS`).
    ElementNoNamespace = 15,
    /// The element has another namespace than expected (`ELEMWRONGNS`).
    ElementWrongNamespace = 17,
    /// The element has a namespace where none is expected (`ELEMEXTRANS`).
    ElementExtraNamespace = 19,
    /// Content ended while an element was required (`NOELEM`).
    MissingElement = 22,
    /// An attribute value is invalid or a required attribute is missing (`ATTRVALID`).
    Attributes = 24,
    /// The content is incomplete (`CONTENTVALID`).
    Content = 25,
    /// An attribute the element does not allow (`INVALIDATTR`).
    InvalidAttribute = 27,
    /// Text other than the required value (`VALUE`).
    Value = 32,
    /// A list value that does not match (`LIST`).
    List = 33,
    /// [`Limits::max_work`] was reached; validation stopped (`INTERNAL`).
    Internal = 37,
    /// An element the content does not allow here (`ELEMWRONG`).
    UnexpectedElement = 38,
    /// Text the content does not allow (`TEXTWRONG`).
    UnexpectedText = 39,
    /// An `xsd:IDREF` naming no `xsd:ID`; libxml2 reports it as the DTD
    /// error `XML_DTD_UNKNOWN_ID`.
    UnknownId = 536,
}

/// One validity error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic<Id> {
    pub code: Code,
    /// The element concerned.
    pub node: Option<Id>,
    /// The start of `node`'s source range, when it has one.
    pub offset: Option<usize>,
    /// A libxml2-style description.
    pub message: String,
}

/// Bounds for parsing a schema and for each validation.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Limits for parsing the schema document and every document it
    /// includes; `max_external_loads` and `max_external_bytes` bound the
    /// included and referenced documents together.
    pub xml: crate::Limits,
    /// Nested `include` and `externalRef` documents (libxml2's include
    /// limit). Loading recurses this deep.
    pub max_include_depth: usize,
    /// Work units for building a schema and, separately, for each
    /// validation: patterns created, derivatives computed and name classes
    /// compared.
    pub max_work: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            xml: crate::Limits::default(),
            max_include_depth: 64,
            max_work: 100_000_000,
        }
    }
}

/// A compiled RELAX NG schema.
#[derive(Debug)]
pub struct Schema {
    arena: Arena,
    tables: Tables,
    start: Id,
    max_work: usize,
}

impl Schema {
    /// Parses the schema document `input`, whose URI `base` bases `href`s.
    pub fn parse(
        input: &[u8],
        base: &str,
        resolver: Box<dyn Resolver>,
        limits: &Limits,
    ) -> Result<Self, SchemaErrors> {
        let document = OwnedDocument::with_limits(input, limits.xml).map_err(|error| {
            SchemaErrors(vec![SchemaError {
                code: SchemaCode::ParseError,
                uri: base.to_owned(),
                offset: Some(error.offset),
                message: format!("xmlRelaxNGParse: could not load {base}: {error}"),
            }])
        })?;
        Self::from_tree(&document, base, resolver, limits)
    }

    /// Compiles the schema held by a parsed document of either DOM.
    pub fn from_tree<T: Tree>(
        tree: &T,
        base: &str,
        resolver: Box<dyn Resolver>,
        limits: &Limits,
    ) -> Result<Self, SchemaErrors> {
        let mut loader = load::Loader::new(base, resolver, limits);
        let root = loader
            .document(tree)
            .map_err(|error| SchemaErrors(vec![error]))?;
        let (arena, tables, start) =
            compile::compile(&root, &loader, limits.max_work).map_err(SchemaErrors)?;
        Ok(Self {
            arena,
            tables,
            start,
            max_work: limits.max_work,
        })
    }

    /// Validates a document. An empty result means it is valid.
    pub fn validate<T: Tree>(&self, tree: &T) -> Vec<Diagnostic<T::Id>> {
        let mut validator = Validator {
            tree,
            deriver: Deriver::new(&self.arena, &self.tables, self.max_work),
            keys: HashMap::new(),
            key: String::new(),
            ids: HashSet::new(),
            references: Vec::new(),
            diagnostics: Vec::new(),
        };
        let root =
            std::iter::successors(tree.first_child(tree.root()), |&id| tree.next_sibling(id))
                .find(|&id| matches!(tree.item(id), Item::Element(_)));
        let Some(root) = root else {
            return Vec::new();
        };
        validator.run(root, self.start);
        if validator.deriver.exhausted() {
            let message = "validation work limit exceeded".into();
            validator.report(Code::Internal, root, message);
        }
        for (element, attribute, reference) in std::mem::take(&mut validator.references) {
            if !validator.deriver.exhausted() && !validator.ids.contains(reference) {
                let message =
                    format!("IDREF attribute {attribute} references an unknown ID \"{reference}\"");
                validator.report(Code::UnknownId, element, message);
            }
        }
        validator.diagnostics
    }
}

/// The open element and its content's derivative so far.
struct Frame<I> {
    element: I,
    pattern: Id,
    next: Option<I>,
    text: String,
    /// Whether the element has element children, so whitespace text is ignored.
    mixed: bool,
    /// Whether an error was already reported for this element.
    errored: bool,
}

struct Validator<'t, 's, T: Navigate> {
    tree: &'t T,
    deriver: Deriver<'s>,
    keys: HashMap<String, u32>,
    key: String,
    /// Values of `xsd:ID` attributes seen, and `xsd:IDREF(S)` references
    /// (element, attribute, ID) to resolve at the end (DTD compatibility).
    ids: HashSet<&'t str>,
    references: Vec<(T::Id, &'t str, &'t str)>,
    diagnostics: Vec<Diagnostic<T::Id>>,
}

/// An element's namespace context, computed only when a datatype asks.
struct Scope<'t, T: Navigate> {
    tree: &'t T,
    element: T::Id,
    bindings: OnceCell<Vec<(&'t str, &'t str)>>,
}

impl<T: Navigate> Context for Scope<'_, T> {
    fn namespace(&self, prefix: &str) -> Option<&str> {
        let bindings = self
            .bindings
            .get_or_init(|| namespaces(self.tree, self.element));
        bindings
            .iter()
            .find(|(bound, _)| *bound == prefix)
            .map(|(_, uri)| *uri)
    }

    fn has_notation(&self, name: &str) -> bool {
        (self.tree.dtd()).is_some_and(|dtd| dtd.notations.contains_key(name))
    }

    fn has_unparsed_entity(&self, name: &str) -> bool {
        (self.tree.dtd()).is_some_and(|dtd| matches!(dtd.entities.get(name), Some(Some(_))))
    }
}

impl<'t, T: Navigate> Validator<'t, '_, T> {
    fn scope(&self, element: T::Id) -> Scope<'t, T> {
        Scope {
            tree: self.tree,
            element,
            bindings: OnceCell::new(),
        }
    }

    fn name<'n>(&mut self, ns: Option<&'n str>, local: &'n str) -> Name<'n> {
        let ns = ns.unwrap_or("");
        self.key.clear();
        self.key.extend([ns, "\0", local]);
        let next = self.keys.len() as u32;
        let key = match self.keys.get(self.key.as_str()) {
            Some(&key) => key,
            None => *self.keys.entry(self.key.clone()).or_insert(next),
        };
        Name { ns, local, key }
    }

    fn report(&mut self, code: Code, element: T::Id, message: String) {
        self.diagnostics.push(Diagnostic {
            code,
            node: Some(element),
            offset: self.tree.offset(element),
            message,
        });
    }

    /// Validates the document element against `start`, one event at a time.
    fn run(&mut self, root: T::Id, start: Id) {
        let mut stack = Vec::new();
        stack.extend(self.open(root, start));
        while let Some(frame) = stack.last_mut() {
            if self.deriver.exhausted() {
                return;
            }
            let Some(child) = frame.next else {
                let frame = stack.pop().expect("open frame");
                let pattern = self.close(frame);
                if let Some(parent) = stack.last_mut() {
                    parent.pattern = pattern;
                }
                continue;
            };
            frame.next = self.tree.next_sibling(child);
            match self.tree.item(child) {
                Item::Text(text) => frame.text.push_str(text),
                Item::Element(_) => {
                    self.flush(frame);
                    let opened = self.open(child, frame.pattern);
                    stack.extend(opened);
                }
                _ => {}
            }
        }
    }

    /// Start tag and attributes; `None` when the element is not allowed,
    /// which leaves `pattern` unchanged and skips the element.
    fn open(&mut self, element: T::Id, pattern: Id) -> Option<Frame<T::Id>> {
        let tree = self.tree;
        let Item::Element(qname) = tree.item(element) else {
            return None;
        };
        let name = self.name(qname.namespace, qname.local);
        let opened = self.deriver.start_tag_open(pattern, name);
        if opened == NOT_ALLOWED && !self.deriver.exhausted() {
            let (code, message) = self.unexpected(pattern, name);
            self.report(code, element, message);
            return None;
        }
        let scope = self.scope(element);
        let mut current = opened;
        let mut errored = false;
        for (_, attribute, value) in attributes(tree, element) {
            if attribute.is_namespace_declaration() {
                continue;
            }
            let attribute_name = self.name(attribute.namespace, attribute.local);
            let next = self
                .deriver
                .attribute(current, attribute_name, value, &scope);
            if next != NOT_ALLOWED || self.deriver.exhausted() {
                current = next;
                self.identify(element, qname, attribute, value);
                continue;
            }
            errored = true;
            let known = self.deriver.front(current).iter().any(|leaf| {
                matches!(leaf, Pat::Attribute(class, _)
                    if self.deriver.tables.contains(*class, attribute_name.ns, attribute_name.local))
            });
            let (code, message) = match known {
                true => (
                    Code::Attributes,
                    format!("Element {} failed to validate attributes", qname.local),
                ),
                false => (
                    Code::InvalidAttribute,
                    format!(
                        "Invalid attribute {} for element {}",
                        attribute.local, qname.local
                    ),
                ),
            };
            self.report(code, element, message);
        }
        // Missing attributes; after an attribute error, the dropped one is missing too.
        let mut closed = self.deriver.start_tag_close(current, false);
        if closed == NOT_ALLOWED && !self.deriver.exhausted() {
            if !errored {
                let message = format!("Element {} failed to validate attributes", qname.local);
                self.report(Code::Attributes, element, message);
            }
            errored = true;
            closed = self.deriver.start_tag_close(current, true);
        }
        let first = tree.first_child(element);
        let mixed = std::iter::successors(first, |&child| tree.next_sibling(child))
            .any(|child| matches!(tree.item(child), Item::Element(_)));
        Some(Frame {
            element,
            pattern: closed,
            next: first,
            text: String::new(),
            mixed,
            errored,
        })
    }

    /// Records `xsd:ID`, `xsd:IDREF` and `xsd:IDREFS` attribute values.
    fn identify(&mut self, element: T::Id, owner: QName<'_>, attribute: QName<'t>, value: &'t str) {
        let tables = self.deriver.tables;
        let Some(&(.., ty)) = tables.ids.iter().find(|&&(owner_class, class, _)| {
            tables.contains(owner_class, owner.namespace.unwrap_or(""), owner.local)
                && tables.contains(class, attribute.namespace.unwrap_or(""), attribute.local)
        }) else {
            return;
        };
        let mut tokens = value
            .split([' ', '\t', '\n', '\r'])
            .filter(|token| !token.is_empty());
        match ty {
            Type::Id => {
                let id = tokens.next().unwrap_or("");
                if !self.ids.insert(id) {
                    self.report(Code::DuplicateId, element, format!("ID {id} redefined"));
                }
            }
            _ => self
                .references
                .extend(tokens.map(|token| (element, attribute.qualified, token))),
        }
    }

    /// Text between element children; whitespace there is insignificant.
    fn flush(&mut self, frame: &mut Frame<T::Id>) {
        let text = std::mem::take(&mut frame.text);
        if !is_whitespace(&text) {
            self.text(frame, &text);
        }
    }

    fn text(&mut self, frame: &mut Frame<T::Id>, text: &str) {
        let scope = self.scope(frame.element);
        let mut next = self.deriver.text(frame.pattern, text, &scope);
        if is_whitespace(text) {
            next = self.deriver.store.choice(frame.pattern, next);
        }
        if next != NOT_ALLOWED {
            frame.pattern = next;
            return;
        }
        if self.deriver.exhausted() {
            return;
        }
        frame.errored = true;
        let local = local_name(self.tree, frame.element);
        let unexpected = (
            Code::UnexpectedText,
            format!("Did not expect text in element {local} content"),
        );
        let (code, message) = self.wanted(frame.pattern, unexpected);
        self.report(code, frame.element, message);
    }

    /// The datatype `pattern` wanted, if any, else `fallback`.
    fn wanted(&self, pattern: Id, fallback: (Code, String)) -> (Code, String) {
        let tables = self.deriver.tables;
        (self.deriver.front(pattern).into_iter())
            .find_map(|leaf| match leaf {
                Pat::Data(datatype, _) => Some((
                    Code::Datatype,
                    format!(
                        "failed to validate type {}",
                        tables.datatypes[datatype as usize].name()
                    ),
                )),
                Pat::Value(_) => Some((Code::Value, "Error validating value".into())),
                Pat::List(_) => Some((Code::List, "Error validating list".into())),
                _ => None,
            })
            .unwrap_or(fallback)
    }

    /// The end tag; returns the parent's pattern after this element.
    fn close(&mut self, mut frame: Frame<T::Id>) -> Id {
        match frame.mixed {
            true => self.flush(&mut frame),
            false => {
                let text = std::mem::take(&mut frame.text);
                self.text(&mut frame, &text);
            }
        }
        let ended = self.deriver.end_tag(frame.pattern, false);
        if ended != NOT_ALLOWED || self.deriver.exhausted() {
            return ended;
        }
        if !frame.errored {
            let local = local_name(self.tree, frame.element);
            let tables = self.deriver.tables;
            let missing =
                self.deriver
                    .front(frame.pattern)
                    .into_iter()
                    .find_map(|leaf| match leaf {
                        Pat::Element(element) => Some(
                            tables
                                .describe(tables.elements[element as usize].0)
                                .unwrap_or(""),
                        ),
                        _ => None,
                    });
            let (code, message) = match missing {
                Some(name) => (
                    Code::MissingElement,
                    format!("Expecting an element {name}, got nothing"),
                ),
                None => self.wanted(
                    frame.pattern,
                    (
                        Code::Content,
                        format!("Element {local} failed to validate content"),
                    ),
                ),
            };
            self.report(code, frame.element, message);
        }
        self.deriver.end_tag(frame.pattern, true)
    }

    /// Classifies an element that `pattern` does not allow.
    fn unexpected(&self, pattern: Id, name: Name<'_>) -> (Code, String) {
        let element = name.local;
        let tables = self.deriver.tables;
        let expected: Vec<(&str, &str)> = (self.deriver.front(pattern).into_iter())
            .filter_map(|leaf| match leaf {
                Pat::Element(element) => {
                    match &tables.names[tables.elements[element as usize].0 as usize] {
                        NameClass::Name(ns, local) => Some((&**ns, &**local)),
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect();
        match (expected.iter()).find(|&&(ns, local)| local == name.local && ns != name.ns) {
            Some(("", _)) => (
                Code::ElementExtraNamespace,
                format!("Expecting no namespace for element {element}"),
            ),
            Some((ns, _)) if name.ns.is_empty() => (
                Code::ElementNoNamespace,
                format!("Expecting a namespace for element {element}: {ns}"),
            ),
            Some((ns, _)) => (
                Code::ElementWrongNamespace,
                format!("Element {element} has wrong namespace: expecting {ns}"),
            ),
            None => match expected.as_slice() {
                [(_, local)] if *local != name.local => (
                    Code::ElementName,
                    format!("Expecting element {local}, got {element}"),
                ),
                _ => (
                    Code::UnexpectedElement,
                    format!("Did not expect element {element} there"),
                ),
            },
        }
    }
}

fn local_name<T: Navigate>(tree: &T, element: T::Id) -> &str {
    match tree.item(element) {
        Item::Element(name) => name.local,
        _ => "",
    }
}

#[cfg(test)]
#[path = "../tests/internal/relaxng.rs"]
mod tests;
