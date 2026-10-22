//! RELAX NG validation (XML syntax), libxml2's `relaxng.c`.
//!
//! [`Schema::parse`] (`xmlRelaxNGParse`) loads a schema and any `include` or
//! `externalRef` documents through a [`Resolver`]; [`Schema::validate`]
//! (`xmlRelaxNGValidateDoc`) checks either DOM, and [`Schema::stream`]
//! (`xmlRelaxNGValidatePushElement`) checks parser events without a tree.
//! A schema is immutable, `Send + Sync` and reusable. Validation uses
//! derivatives instead of libxml2's backtracking, bounded by
//! [`Limits::max_work`]. Built-in and XML Schema datatypes are supported;
//! the compact syntax is not. Details and differences:
//! <https://github.com/pbdeuchler/yaxml/blob/main/docs/relaxng.md>.
//!
//! ```
//! use yaxml::relaxng::{Code, Limits, Schema};
//!
//! let rng = br#"<element name="r" xmlns="http://relaxng.org/ns/structure/1.0"
//!     datatypeLibrary="http://www.w3.org/2001/XMLSchema-datatypes">
//!   <interleave>
//!     <oneOrMore><element name="i"><data type="int"/></element></oneOrMore>
//!     <element name="b"><empty/></element>
//!   </interleave>
//! </element>"#;
//! let none = |_: yaxml::Request<'_>| Ok(None);
//! let schema = Schema::parse(rng, "r.rng", Box::new(none), &Limits::default())?;
//! let check = |xml| schema.validate(&yaxml::Document::parse(xml).unwrap());
//! assert!(check("<r><b/><i>1</i><i>2</i></r>").is_empty());
//! assert_eq!(check("<r><b/><i>x</i></r>")[0].code, Code::Datatype);
//! assert_eq!(check("<r><b/></r>")[0].code, Code::MissingElement);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod check;
mod compile;
mod datatype;
mod load;
mod pattern;

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use crate::fallible::TryPush;
use crate::schema_types::{Context, Type};
use crate::valid::Dtd;
use crate::xpath::tree::{self, Item, Navigate, QName, Tree, attributes, namespaces};
use crate::{Event, OwnedDocument, Resolver, SourceOrigin};
use pattern::{Arena, Deriver, Id, NOT_ALLOWED, Name, NameClass, Pat, Tables, is_whitespace};

pub(crate) const RNG_NS: &str = "http://relaxng.org/ns/structure/1.0";

/// Why a schema was rejected. The discriminant is libxml2's
/// `xmlParserErrors` value (`XML_RNGP_*`, and `XML_ERR_RESOURCE_LIMIT`).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u16)]
#[non_exhaustive]
pub enum SchemaCode {
    /// An allocation the schema sized failed (`XML_ERR_NO_MEMORY`).
    Allocation = 2,
    /// A load or work limit was reached.
    ResourceLimit = 114,
    /// An `anyName` attribute outside `oneOrMore` (section 7.3).
    AnyNameAttrAncestor = 1000,
    /// Attributes of interleaved patterns overlap.
    AttrConflict = 1001,
    /// An `attribute` with more than one pattern.
    AttributeChildren = 1002,
    /// An `attribute` with neither a name nor children.
    AttributeEmpty = 1004,
    /// A name class that is not `name`, `anyName`, `nsName` or `choice`.
    ChoiceContent = 1006,
    /// A name-class `choice` with no children.
    ChoiceEmpty = 1007,
    /// Unexpected content in `data`.
    DataContent = 1009,
    /// A `define` combined with both `choice` and `interleave`.
    DefChoiceAndInterleave = 1010,
    /// A `define` with no pattern.
    DefineEmpty = 1012,
    /// An `include` overrides a `define` the included grammar lacks.
    DefineMissing = 1013,
    /// A `define` without `name`.
    DefineNameMissing = 1014,
    /// An element's content has no content type (section 7.2).
    ElemContentError = 1016,
    /// An `element` with neither a name nor children.
    ElementEmpty = 1017,
    /// A malformed name class or name.
    ElementName = 1019,
    /// An `element` without a content pattern.
    ElementNoContent = 1020,
    /// Elements or text of interleaved patterns overlap.
    ElemTextConflict = 1021,
    /// The document element is not a RELAX NG pattern.
    Empty = 1022,
    /// A pattern element with no pattern inside.
    EmptyConstruct = 1023,
    /// An `empty` with content.
    EmptyNotEmpty = 1025,
    /// An empty `except` in a name class.
    ExceptEmpty = 1027,
    /// A name class whose child is not `except`.
    ExceptMissing = 1028,
    /// More than one `except` in a name class.
    ExceptMultiple = 1029,
    /// An empty `except` in `data`.
    ExceptNoContent = 1030,
    /// An `externalRef` document failed to load.
    ExternalRefFailure = 1032,
    /// An `externalRef` refers to itself.
    ExternalRefRecurse = 1033,
    /// A RELAX NG attribute on an element that does not take it.
    ForbiddenAttribute = 1034,
    /// A foreign element inside `name`, `value` or `param`.
    ForeignElement = 1035,
    /// An unexpected child of `grammar`.
    GrammarContent = 1036,
    /// A `grammar` with no children.
    GrammarEmpty = 1037,
    /// An included document is not a `grammar`.
    GrammarMissing = 1038,
    /// A `grammar` without `start`.
    GrammarNoStart = 1039,
    /// Attributes of grouped patterns overlap.
    GroupAttrConflict = 1040,
    /// An invalid `href` URI.
    HrefError = 1041,
    /// An `include` document failed to load.
    IncludeFailure = 1043,
    /// An `include` refers to itself.
    IncludeRecurse = 1044,
    /// An `interleave` with no children.
    InterleaveNoContent = 1048,
    /// A `define` without an NCName `name`.
    InvalidDefineName = 1049,
    /// A `datatypeLibrary` that is not a URI.
    InvalidUri = 1050,
    /// A `value` its datatype does not accept.
    InvalidValue = 1051,
    /// An `include` or `externalRef` without `href`.
    MissingHref = 1052,
    /// A `define` without `name` inside an `include`.
    NameMissing = 1053,
    /// Several `define`s (or `start`s) of one name, without `combine`.
    NeedCombine = 1054,
    /// A `notAllowed` with content.
    NotAllowedNotEmpty = 1055,
    /// An `nsName` attribute outside `oneOrMore` (section 7.3).
    NsNameAttrAncestor = 1056,
    /// A `param` for a datatype library that takes none.
    ParamForbidden = 1058,
    /// A `param` without `name`.
    ParamNameMissing = 1059,
    /// A `parentRef` whose `name` is not an NCName.
    ParentRefNameInvalid = 1061,
    /// A `parentRef` without `name`.
    ParentRefNoName = 1062,
    /// A `parentRef` outside a nested grammar.
    ParentRefNoParent = 1063,
    /// A `parentRef` with content.
    ParentRefNotEmpty = 1064,
    /// A schema document is not well-formed or nesting is too deep.
    ParseError = 1065,
    /// `anyName/except//anyName`.
    PatAnyNameExceptAnyName = 1066,
    /// `attribute//attribute` (section 7.1).
    PatAttrAttr = 1067,
    /// `attribute//element`.
    PatAttrElem = 1068,
    /// `data/except//attribute`.
    PatDataExceptAttr = 1069,
    /// `data/except//element`.
    PatDataExceptElem = 1070,
    /// `data/except//empty`.
    PatDataExceptEmpty = 1071,
    /// `data/except//group`.
    PatDataExceptGroup = 1072,
    /// `data/except//interleave`.
    PatDataExceptInterleave = 1073,
    /// `data/except//list`.
    PatDataExceptList = 1074,
    /// `data/except//oneOrMore`.
    PatDataExceptOneMore = 1075,
    /// `data/except//text`.
    PatDataExceptText = 1077,
    /// `list//attribute`.
    PatListAttr = 1078,
    /// `list//element`.
    PatListElem = 1079,
    /// `list//interleave`.
    PatListInterleave = 1080,
    /// `list//list`.
    PatListList = 1081,
    /// `list//text`.
    PatListText = 1083,
    /// `nsName/except//anyName`.
    PatNsNameExceptAnyName = 1084,
    /// `nsName/except//nsName`.
    PatNsNameExceptNsName = 1085,
    /// `oneOrMore//group//attribute`.
    PatOneMoreGroupAttr = 1086,
    /// `oneOrMore//interleave//attribute`.
    PatOneMoreInterleaveAttr = 1087,
    /// `start//attribute`.
    PatStartAttr = 1088,
    /// `start//data`.
    PatStartData = 1089,
    /// `start//empty`.
    PatStartEmpty = 1090,
    /// `start//group`.
    PatStartGroup = 1091,
    /// `start//interleave`.
    PatStartInterleave = 1092,
    /// `start//list`.
    PatStartList = 1093,
    /// `start//oneOrMore`.
    PatStartOneMore = 1094,
    /// `start//text`.
    PatStartText = 1095,
    /// `start//value`.
    PatStartValue = 1096,
    /// An undeclared namespace prefix in a name.
    PrefixUndefined = 1097,
    /// A reference cycle not broken by an `element`.
    RefCycle = 1099,
    /// A `ref` whose `name` is not an NCName.
    RefNameInvalid = 1100,
    /// A `ref` naming no `define`.
    RefNoDef = 1101,
    /// A `ref` without `name`.
    RefNoName = 1102,
    /// A `ref` with content.
    RefNotEmpty = 1103,
    /// `start` combined with both `choice` and `interleave`.
    StartChoiceAndInterleave = 1104,
    /// A `start` with more than one pattern.
    StartContent = 1105,
    /// A `start` with no pattern.
    StartEmpty = 1106,
    /// An `include` overrides `start` but the included grammar has none.
    StartMissing = 1107,
    /// Text where the RELAX NG syntax allows none.
    TextExpected = 1108,
    /// A `text` with content.
    TextHasChild = 1109,
    /// A `data` without `type`.
    TypeMissing = 1110,
    /// A datatype its library does not define.
    TypeNotFound = 1111,
    /// An invalid datatype parameter.
    TypeValue = 1112,
    /// An attribute RELAX NG does not define.
    UnknownAttribute = 1113,
    /// A `combine` other than `choice` or `interleave`.
    UnknownCombine = 1114,
    /// An element that is not a RELAX NG pattern.
    UnknownConstruct = 1115,
    /// An unsupported `datatypeLibrary`.
    UnknownTypeLib = 1116,
    /// A `datatypeLibrary` or `href` with a fragment.
    UriFragment = 1117,
    /// A `datatypeLibrary` that is not an absolute URI.
    UriNotAbsolute = 1118,
    /// An attribute named `xmlns`.
    XmlnsName = 1121,
    /// An attribute name class in the xmlns namespace.
    XmlNs = 1122,
}

impl SchemaCode {
    /// What a schema document that does not parse reports.
    fn loading(error: crate::Error) -> Self {
        match error.kind {
            crate::ErrorKind::Allocation => Self::Allocation,
            _ => Self::ParseError,
        }
    }
}

/// One schema error, in the schema document `uri` at the source `offset`
/// of the offending element when it has one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaError {
    /// What went wrong.
    pub code: SchemaCode,
    /// The schema document holding the error.
    pub uri: String,
    /// Byte offset of the offending element in that document.
    pub offset: Option<usize>,
    /// A libxml2-style description.
    pub message: String,
}

/// The errors that rejected a schema; never empty.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaErrors(
    /// The errors, in the order found.
    pub Vec<SchemaError>,
);

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
    /// An allocation the document sized failed; validation stopped (`MEMORY`).
    Allocation = 1,
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
    /// What went wrong.
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
#[non_exhaustive]
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

setters!(const Limits { xml: crate::Limits, max_include_depth: usize, max_work: usize });

impl Limits {
    /// The defaults: 64 nested includes and 100 million work units.
    pub const DEFAULT: Self = Self {
        xml: crate::Limits::DEFAULT,
        max_include_depth: 64,
        max_work: 100_000_000,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A compiled RELAX NG schema.
#[derive(Debug)]
pub struct Schema {
    arena: Arc<Arena>,
    tables: Arc<Tables>,
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
        let document = OwnedDocument::parse_legacy(input, limits.xml).map_err(|error| {
            SchemaErrors(vec![SchemaError {
                code: SchemaCode::loading(error),
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
            arena: Arc::new(arena),
            tables: Arc::new(tables),
            start,
            max_work: limits.max_work,
        })
    }

    /// Validates a document. An empty result means it is valid.
    pub fn validate<T: Tree>(&self, tree: &T) -> Vec<Diagnostic<T::Id>> {
        let mut engine = Engine::new(self);
        let scope = |element| Scope {
            tree,
            element,
            bindings: OnceCell::new(),
        };
        let open = |engine: &mut Engine<T::Id>, element| {
            let Item::Element(name) = tree.item(element) else {
                return None;
            };
            let attributes = attributes(tree, element).map(|(_, name, value)| (name, value));
            let offset = tree.offset(element);
            (engine.start(element, offset, name, attributes, &scope(element)))
                .then(|| (element, tree::flattened_first(tree, element)))
        };
        let root =
            std::iter::successors(tree.first_child(tree.root()), |&id| tree.next_sibling(id))
                .find(|&id| matches!(tree.item(id), Item::Element(_)));
        let mut stack: Vec<_> = root
            .and_then(|root| open(&mut engine, root))
            .into_iter()
            .collect();
        while let Some((element, next)) = stack.last_mut()
            && !engine.deriver.exhausted()
        {
            let element = *element;
            let Some(child) = *next else {
                stack.pop();
                engine.end(&scope(element));
                continue;
            };
            *next = tree::flattened_next(tree, element, child);
            match tree.item(child) {
                Item::Text(text) => engine.text(text),
                Item::Element(_) => {
                    engine.flush(&scope(element));
                    stack.extend(open(&mut engine, child));
                }
                _ => {}
            }
        }
        engine.finish();
        engine.diagnostics
    }

    /// A validator for one document read as events, without a tree.
    pub fn stream(&self) -> Stream {
        Stream {
            engine: Engine::new(self),
            scope: Bindings::default(),
            elements: 0,
            depth: 0,
            skipped: 0,
        }
    }
}

/// A validator for one document fed as events, libxml2's
/// `xmlRelaxNGValidatePushElement`, `PushCData` and `PopElement`.
///
/// Feed it every event of a [`Reader`](crate::Reader),
/// [`PushParser`](crate::PushParser) or
/// [`BufferedReader`](crate::BufferedReader) in order. Its diagnostics equal
/// [`Schema::validate`]'s, with [`Diagnostic::node`] the element's index in
/// document order. It works for every schema (no `FullElement` fallback) and
/// holds only the open elements' text and bindings, the `xsd:ID`s seen, and
/// a memo bounded by [`Limits::max_work`].
///
/// ```
/// use std::ops::ControlFlow;
/// use yaxml::relaxng::{Code, Limits, Schema};
///
/// let rng = br#"<element name="r" xmlns="http://relaxng.org/ns/structure/1.0"><element name="a"><empty/></element></element>"#;
/// let none = |_: yaxml::Request<'_>| Ok(None);
/// let schema = Schema::parse(rng, "r.rng", Box::new(none), &Limits::default())?;
/// let mut stream = schema.stream();
/// let mut parser = yaxml::PushParser::new();
/// let mut feed = |event: &yaxml::Event<'_>, origin| {
///     stream.event(event, origin);
///     ControlFlow::Continue(())
/// };
/// for chunk in [b"<r><b".as_slice(), b"/></r>"] {
///     parser.feed(chunk, &mut feed)?;
/// }
/// parser.finish(&mut feed)?;
/// assert_eq!(stream.diagnostics()[0].code, Code::ElementName);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Stream {
    engine: Engine<usize>,
    scope: Bindings,
    elements: usize,
    depth: usize,
    /// Depth within an element the schema does not allow, which is skipped.
    skipped: usize,
}

impl Stream {
    /// Supplies the document's DTD, which `ENTITY`, `ENTITIES` and
    /// `NOTATION` values consult; without one they are invalid.
    pub fn set_dtd(&mut self, dtd: &Dtd) {
        self.scope.dtd = Some(dtd.clone());
    }

    /// Consumes the next event of the document.
    pub fn event(&mut self, event: &Event<'_>, origin: SourceOrigin) {
        match event {
            Event::Start(element) | Event::Empty(element) => {
                self.depth += 1;
                self.elements += 1;
                if self.skipped > 0 {
                    self.skipped += 1;
                } else if !self.engine.deriver.exhausted() {
                    self.engine.flush(&self.scope);
                    self.scope.push(&element.attributes);
                    let attributes = (element.attributes.iter())
                        .map(|attribute| (QName::of(&attribute.name), &*attribute.value));
                    let offset = origin.input_range().map(|range| range.start);
                    let (index, name) = (self.elements - 1, QName::of(&element.name));
                    if !(self.engine).start(index, offset, name, attributes, &self.scope) {
                        self.skipped = 1;
                    }
                }
                if matches!(event, Event::Empty(_)) {
                    self.end();
                }
            }
            Event::End(_) => self.end(),
            Event::Text(text) | Event::Cdata(text)
                if self.skipped == 0 && !self.engine.deriver.exhausted() =>
            {
                self.engine.text(text)
            }
            _ => {}
        }
    }

    fn end(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        if !self.engine.deriver.exhausted() {
            match self.skipped {
                0 => {
                    self.engine.end(&self.scope);
                    self.scope.pop();
                }
                1 => {
                    self.skipped = 0;
                    self.scope.pop();
                }
                _ => self.skipped -= 1,
            }
        }
        if self.depth == 0 {
            self.engine.finish();
        }
    }

    /// The diagnostics so far; complete once the document element ends.
    pub fn diagnostics(&self) -> &[Diagnostic<usize>] {
        &self.engine.diagnostics
    }
}

/// The namespace bindings of a stream's open elements, and its DTD.
#[derive(Default)]
struct Bindings {
    bindings: Vec<(String, String)>,
    marks: Vec<usize>,
    dtd: Option<Dtd>,
}

impl Bindings {
    fn push(&mut self, attributes: &[crate::Attribute<'_>]) {
        self.marks.push(self.bindings.len());
        for attribute in attributes {
            if let Some(prefix) = QName::of(&attribute.name).declared_prefix() {
                (self.bindings).push((prefix.to_owned(), attribute.value.to_string()));
            }
        }
    }

    fn pop(&mut self) {
        self.bindings.truncate(self.marks.pop().unwrap_or(0));
    }
}

impl Context for Bindings {
    fn namespace(&self, prefix: &str) -> Option<&str> {
        let mut bindings = self.bindings.iter().rev();
        (bindings.find(|(bound, _)| bound == prefix)).map(|(_, uri)| uri.as_str())
    }

    fn has_notation(&self, name: &str) -> bool {
        (self.dtd.as_ref()).is_some_and(|dtd| dtd.notation(name).is_some())
    }

    fn has_unparsed_entity(&self, name: &str) -> bool {
        (self.dtd.as_ref()).is_some_and(|dtd| {
            dtd.entity(name)
                .is_some_and(|entity| entity.notation.is_some())
        })
    }
}

/// An open element and its content's derivative so far.
struct Frame<I> {
    element: I,
    offset: Option<usize>,
    /// The element's name key.
    name: u32,
    pattern: Id,
    /// Text since the last element child.
    text: String,
    /// Whether an element child was seen, so whitespace text is ignored.
    mixed: bool,
    /// Whether an error was already reported for this element.
    errored: bool,
}

/// One validation, fed start tags, text and end tags in document order by
/// both the tree walk and [`Stream`]. Each call takes the namespace and DTD
/// context of the element it concerns.
struct Engine<I> {
    deriver: Deriver,
    start: Id,
    keys: HashMap<String, u32>,
    /// Local name of each key.
    locals: Vec<Box<str>>,
    key: String,
    /// Values of `xsd:ID` attributes seen, and `xsd:IDREF(S)` references
    /// (element, offset, attribute, ID) to resolve at the end.
    ids: HashSet<String>,
    references: Vec<(I, Option<usize>, String, String)>,
    stack: Vec<Frame<I>>,
    root: Option<(I, Option<usize>)>,
    diagnostics: Vec<Diagnostic<I>>,
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
        (self.tree.dtd()).is_some_and(|dtd| dtd.notation(name).is_some())
    }

    fn has_unparsed_entity(&self, name: &str) -> bool {
        (self.tree.dtd()).is_some_and(|dtd| {
            dtd.entity(name)
                .is_some_and(|entity| entity.notation.is_some())
        })
    }
}

impl<I: Copy> Engine<I> {
    fn new(schema: &Schema) -> Self {
        Self {
            deriver: Deriver::new(schema.arena.clone(), schema.tables.clone(), schema.max_work),
            start: schema.start,
            keys: HashMap::new(),
            locals: Vec::new(),
            key: String::new(),
            ids: HashSet::new(),
            references: Vec::new(),
            stack: Vec::new(),
            root: None,
            diagnostics: Vec::new(),
        }
    }

    fn name<'n>(&mut self, ns: Option<&'n str>, local: &'n str) -> Name<'n> {
        let ns = ns.unwrap_or("");
        self.key.clear();
        self.key.extend([ns, "\0", local]);
        let key = match self.keys.get(self.key.as_str()) {
            Some(&key) => key,
            None => {
                let store = &mut self.deriver.store;
                if store.refuses(self.locals.try_reserve(1))
                    || store.refuses(self.keys.try_reserve(1))
                {
                    // Validation stops: no name is looked up again.
                    return Name { ns, local, key: 0 };
                }
                self.locals.push(local.into());
                *self
                    .keys
                    .entry(self.key.clone())
                    .or_insert(self.locals.len() as u32 - 1)
            }
        };
        Name { ns, local, key }
    }

    fn report(&mut self, code: Code, (element, offset): (I, Option<usize>), message: String) {
        if self.deriver.store.refuses(self.diagnostics.try_reserve(1)) {
            return;
        }
        self.diagnostics.push(Diagnostic {
            code,
            node: Some(element),
            offset,
            message,
        });
    }

    /// The start tag and attributes of `element`; false when the content
    /// does not allow it, which leaves the parent unchanged and its subtree
    /// to be skipped.
    fn start<'a>(
        &mut self,
        element: I,
        offset: Option<usize>,
        qname: QName<'_>,
        attributes: impl Iterator<Item = (QName<'a>, &'a str)>,
        cx: &dyn Context,
    ) -> bool {
        let at = (element, offset);
        self.root.get_or_insert(at);
        let pattern = self.stack.last().map_or(self.start, |frame| frame.pattern);
        let name = self.name(qname.namespace, qname.local);
        let opened = self.deriver.start_tag_open(pattern, name);
        if opened == NOT_ALLOWED && !self.deriver.exhausted() {
            let (code, message) = self.unexpected(pattern, name);
            self.report(code, at, message);
            return false;
        }
        let mut current = opened;
        let mut errored = false;
        for (attribute, value) in attributes {
            if attribute.is_namespace_declaration() {
                continue;
            }
            let attribute_name = self.name(attribute.namespace, attribute.local);
            let next = self.deriver.attribute(current, attribute_name, value, cx);
            if next != NOT_ALLOWED || self.deriver.exhausted() {
                current = next;
                self.identify(at, qname, attribute, value);
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
            self.report(code, at, message);
        }
        // Missing attributes; after an attribute error, the dropped one is missing too.
        let mut closed = self.deriver.start_tag_close(current, false);
        if closed == NOT_ALLOWED && !self.deriver.exhausted() {
            if !errored {
                let message = format!("Element {} failed to validate attributes", qname.local);
                self.report(Code::Attributes, at, message);
            }
            errored = true;
            closed = self.deriver.start_tag_close(current, true);
        }
        self.stack.push(Frame {
            element,
            offset,
            name: name.key,
            pattern: closed,
            text: String::new(),
            mixed: false,
            errored,
        });
        true
    }

    /// Records `xsd:ID`, `xsd:IDREF` and `xsd:IDREFS` attribute values.
    fn identify(
        &mut self,
        at: (I, Option<usize>),
        owner: QName<'_>,
        attribute: QName<'_>,
        value: &str,
    ) {
        let tables = &self.deriver.tables;
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
                let refused = self.deriver.store.refuses(self.ids.try_reserve(1));
                if !refused && !self.ids.insert(id.to_owned()) {
                    self.report(Code::DuplicateId, at, format!("ID {id} redefined"));
                }
            }
            _ => {
                for token in tokens {
                    let (element, offset) = at;
                    let reference = (element, offset, attribute.qualified.into(), token.into());
                    if self
                        .deriver
                        .store
                        .refuses(self.references.try_push(reference))
                    {
                        return;
                    }
                }
            }
        }
    }

    /// Text inside the current element.
    fn text(&mut self, text: &str) {
        if let Some(frame) = self.stack.last_mut() {
            self.deriver.store.refuses(frame.text.try_push(text));
        }
    }

    /// An element child is next: the text before it is complete, and
    /// whitespace there is insignificant.
    fn flush(&mut self, cx: &dyn Context) {
        let Some(mut frame) = self.stack.pop() else {
            return;
        };
        frame.mixed = true;
        let text = std::mem::take(&mut frame.text);
        if !is_whitespace(&text) {
            self.characters(&mut frame, &text, cx);
        }
        self.stack.push(frame);
    }

    fn characters(&mut self, frame: &mut Frame<I>, text: &str, cx: &dyn Context) {
        let mut next = self.deriver.text(frame.pattern, text, cx);
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
        let local = &self.locals[frame.name as usize];
        let unexpected = (
            Code::UnexpectedText,
            format!("Did not expect text in element {local} content"),
        );
        let (code, message) = self.wanted(frame.pattern, unexpected);
        self.report(code, (frame.element, frame.offset), message);
    }

    /// The datatype `pattern` wanted, if any, else `fallback`.
    fn wanted(&self, pattern: Id, fallback: (Code, String)) -> (Code, String) {
        let tables = &self.deriver.tables;
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

    /// The end tag of the current element.
    fn end(&mut self, cx: &dyn Context) {
        let Some(mut frame) = self.stack.pop() else {
            return;
        };
        let text = std::mem::take(&mut frame.text);
        if !frame.mixed || !is_whitespace(&text) {
            self.characters(&mut frame, &text, cx);
        }
        let mut ended = self.deriver.end_tag(frame.pattern, false);
        if ended == NOT_ALLOWED && !self.deriver.exhausted() {
            if !frame.errored {
                let (code, message) = self.missing(&frame);
                self.report(code, (frame.element, frame.offset), message);
            }
            ended = self.deriver.end_tag(frame.pattern, true);
        }
        if let Some(parent) = self.stack.last_mut() {
            parent.pattern = ended;
        }
    }

    /// Classifies content that ended too early.
    fn missing(&self, frame: &Frame<I>) -> (Code, String) {
        let tables = &self.deriver.tables;
        let missing = (self.deriver.front(frame.pattern).into_iter()).find_map(|leaf| match leaf {
            Pat::Element(element) => Some(
                tables
                    .describe(tables.elements[element as usize].0)
                    .unwrap_or(""),
            ),
            _ => None,
        });
        match missing {
            Some(name) => (
                Code::MissingElement,
                format!("Expecting an element {name}, got nothing"),
            ),
            None => {
                let local = &self.locals[frame.name as usize];
                let fallback = format!("Element {local} failed to validate content");
                self.wanted(frame.pattern, (Code::Content, fallback))
            }
        }
    }

    /// The end of the document: the work limit and unresolved references.
    fn finish(&mut self) {
        let Some(root) = self.root else {
            return;
        };
        if self.deriver.store.refused {
            self.diagnostics = vec![Diagnostic {
                code: Code::Allocation,
                node: Some(root.0),
                offset: root.1,
                message: "Memory allocation failed".into(),
            }];
            return;
        }
        if self.deriver.exhausted() {
            let message = "validation work limit exceeded".into();
            return self.report(Code::Internal, root, message);
        }
        for (element, offset, attribute, reference) in std::mem::take(&mut self.references) {
            if !self.ids.contains(&reference) {
                let message =
                    format!("IDREF attribute {attribute} references an unknown ID \"{reference}\"");
                self.report(Code::UnknownId, (element, offset), message);
            }
        }
    }

    /// Classifies an element that `pattern` does not allow.
    fn unexpected(&self, pattern: Id, name: Name<'_>) -> (Code, String) {
        let element = name.local;
        let tables = &self.deriver.tables;
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

#[cfg(test)]
#[path = "../tests/internal/relaxng.rs"]
mod tests;
