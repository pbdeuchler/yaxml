//! The DTD validation engine, driven by events as libxml2's push validation
//! (`xmlValidatePushElement`, `xmlValidatePushCData`, `xmlValidatePopElement`
//! over `vstateTab`) is: each open element keeps its declaration and the
//! state of its compiled content model, attributes are checked at the start
//! tag, IDs go into a table and IDREFs are resolved when the document element
//! ends (`xmlValidateDocumentFinal`). [`run`] feeds a tree's nodes and
//! [`super::StreamValidator`] parser events.
//!
//! A diagnostic is reported as soon as it is known, so in document order.
//! Each also gets a key: its element's place in `xmlValidateDocument`'s
//! preorder report (content checks, then required attributes, then attribute
//! values, before any descendant), which [`run`] sorts by. A content-model
//! mismatch is worded as the push validator words it (`Misplaced x`, `Text
//! not allowed`, `Expecting more children`); [`run`] words it as
//! `xmlValidateElementContent` does.

use std::collections::{HashMap, HashSet};

use super::{
    AttributeDecl, AttributeKind, Code, Content, Diagnostic, Dtd, blank, predefined_entity,
    value_matches,
};
use crate::dtd_catalog::{EntityBody, StoredDefault};
use crate::fallible::collected;
use crate::xpath::tree::{Item, Navigate, QName};

/// An element as diagnostics name it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Node<Id> {
    pub(crate) id: Id,
    pub(crate) offset: Option<usize>,
}

/// The element's start ordinal (0 before the document element,
/// `usize::MAX` for IDREFs) and the phase within it.
type Key = (usize, u8);

/// `xmlValidateRoot`; then `xmlValidateOneElement`'s content checks, its
/// content model and its required attributes; then `xmlValidateOneAttribute`.
const ROOT: u8 = 0;
const CONTENT: u8 = 1;
const MODEL: u8 = 2;
const REQUIRED: u8 = 3;
const ATTRIBUTES: u8 = 4;

struct Frame<Id> {
    node: Node<Id>,
    ordinal: usize,
    /// The element's declaration, when it declares content.
    decl: Option<usize>,
    /// The content model's state; `None` once it rejected a child, and for
    /// a nondeterministic model.
    state: Option<u32>,
    /// A content diagnostic libxml2 reports once per element was reported.
    noted: bool,
    /// Whether blank text is reported: element content declared in the
    /// external subset of a standalone document.
    watch_blanks: bool,
    /// The character data since the last other node: whether it is all blank.
    run: Option<bool>,
}

struct Reference<Id> {
    node: Node<Id>,
    attribute: Box<str>,
    value: Box<str>,
    multiple: bool,
}

/// One document's validation state: the open elements, the ID and IDREF
/// tables and the diagnostics so far.
pub(crate) struct Validator<Id> {
    /// `xmlValidateDocument` (parse-time issues, `xmlValidateDtdFinal`, the
    /// root name) rather than `xmlValidateDtd`.
    document: bool,
    frames: Vec<Frame<Id>>,
    elements: usize,
    /// The DTD's parse-time issues reported so far.
    issues: usize,
    ids: HashSet<Box<str>>,
    references: Vec<Reference<Id>>,
    nondeterministic: HashSet<usize>,
    /// Local names of each mixed-content declaration, built on first use so a child
    /// test is one lookup rather than a scan of the declared list.
    mixed: HashMap<usize, HashSet<Box<str>>>,
    /// Tokens of each enumerated or notation attribute declaration, by
    /// (element, attribute) slot, built on first use.
    enumerated: HashMap<(usize, usize), HashSet<Box<str>>>,
    out: Vec<Diagnostic<Id>>,
    keys: Vec<Key>,
    /// An allocation was refused: its diagnostic is the only one kept.
    refused: bool,
}

impl<Id: Copy> Validator<Id> {
    pub(crate) fn new(document: bool) -> Self {
        Self {
            document,
            frames: Vec::new(),
            elements: 0,
            issues: 0,
            ids: HashSet::new(),
            references: Vec::new(),
            nondeterministic: HashSet::new(),
            mixed: HashMap::new(),
            enumerated: HashMap::new(),
            out: Vec::new(),
            keys: Vec::new(),
            refused: false,
        }
    }

    pub(crate) fn diagnostics(&self) -> &[Diagnostic<Id>] {
        &self.out
    }

    /// The elements started so far.
    pub(crate) fn elements(&self) -> usize {
        self.elements
    }

    /// The diagnostics in `xmlValidateDocument`'s order (the report order
    /// sorted by key).
    fn sorted(self) -> Vec<Diagnostic<Id>> {
        let Ok(mut keyed) = collected(self.keys.into_iter().zip(0..).zip(self.out)) else {
            return refused();
        };
        keyed.sort_unstable_by_key(|&(key, _)| key);
        collected(keyed.into_iter().map(|(_, diagnostic)| diagnostic)).unwrap_or_else(|_| refused())
    }

    pub(crate) fn finish(self) -> Vec<Diagnostic<Id>> {
        self.out
    }

    /// Room for one more diagnostic, else validation is refused.
    fn room(&mut self) -> bool {
        let room =
            !self.refused && self.keys.try_reserve(1).is_ok() && self.out.try_reserve(1).is_ok();
        if !room {
            self.refuse();
        }
        room
    }

    /// A refused allocation ends validation: its diagnostic replaces the
    /// others, and the tables are dropped.
    fn refuse(&mut self) {
        self.refused = true;
        (self.ids, self.references) = Default::default();
        (self.keys, self.out) = (vec![(0, ROOT)], refused());
    }

    fn report(&mut self, key: Key, code: Code, node: Option<Node<Id>>, message: String) {
        if !self.room() {
            return;
        }
        self.keys.push(key);
        self.out.push(Diagnostic {
            code,
            node: node.map(|node| node.id),
            offset: node.and_then(|node| node.offset),
            message,
        });
    }

    /// A document-level diagnostic, reported before any element's.
    pub(crate) fn note(&mut self, code: Code, offset: Option<usize>, message: String) {
        if self.room() {
            self.keys.push((0, ROOT));
            self.out.push(Diagnostic {
                code,
                node: None,
                offset,
                message,
            });
        }
    }

    /// The parse-time issues `dtd` recorded since the last call.
    pub(crate) fn issues(&mut self, dtd: &Dtd) {
        if self.document {
            for (code, offset, message) in dtd.issues.get(self.issues..).unwrap_or_default() {
                self.note(*code, *offset, message.clone());
            }
            self.issues = self.issues.max(dtd.issues.len());
        }
    }

    /// The parse-time issues so far, then `xmlValidateDtdFinal`: notation
    /// attributes and unparsed entities.
    pub(crate) fn begin(&mut self, dtd: &Dtd) {
        if !self.document {
            return;
        }
        self.issues(dtd);
        for decl in &dtd.elements {
            for attribute in &decl.attributes {
                if attribute.kind == AttributeKind::Notation
                    && matches!(decl.content, Some(Content::Empty))
                {
                    let message = format!(
                        "NOTATION attribute {} declared for EMPTY element {}",
                        attribute.name, decl.name
                    );
                    self.note(Code::EmptyNotation, None, message);
                }
            }
        }
        let mut missing: Vec<_> = (dtd.declared_ids())
            .filter_map(|id| match &dtd.entities[id.0].body {
                EntityBody::External(external) => external.notation.as_deref(),
                EntityBody::Internal(_) => None,
            })
            .filter(|notation| !dtd.notation_index.contains_key(*notation))
            .collect();
        missing.sort_unstable();
        for notation in missing {
            let message = format!("NOTATION {notation} is not declared");
            self.note(Code::UnknownNotation, None, message);
        }
    }

    /// An element starts: it is a child of the open element, and its
    /// declaration, required attributes and attribute values are checked.
    pub(crate) fn start<'x, A>(&mut self, dtd: &Dtd, node: Node<Id>, name: QName<'x>, attributes: A)
    where
        A: Iterator<Item = (QName<'x>, &'x str)> + Clone,
    {
        self.child(dtd, name);
        self.elements += 1;
        let ordinal = self.elements;
        if ordinal == 1
            && self.document
            && let Some(expected) = dtd.name.as_deref()
            && expected != name.local
            && expected != name.qualified
            && !(expected == "HTML" && name.local == "html")
        {
            let message = format!(
                "root and DTD name do not match '{}' and '{expected}'",
                name.local
            );
            self.report((ordinal, ROOT), Code::RootName, Some(node), message);
        }
        let mut frame = Frame {
            node,
            ordinal,
            decl: None,
            state: None,
            noted: false,
            watch_blanks: false,
            run: None,
        };
        match dtd.element_for(name) {
            Some((index, decl)) if decl.content.is_some() => {
                frame.decl = Some(index);
                if let Some(Content::Children(model)) = &decl.content {
                    frame.watch_blanks = dtd.standalone && !decl.internal;
                    if model.deterministic() {
                        frame.state = Some(0);
                    } else if self.nondeterministic.insert(index) {
                        let message =
                            format!("Content model of {} is not deterministic", name.local);
                        let code = Code::ContentNotDeterministic;
                        self.report((ordinal, MODEL), code, None, message);
                    }
                }
                self.required((ordinal, node), name, &decl.attributes, attributes.clone());
            }
            _ => {
                let message = format!("No declaration for element {}", name.local);
                self.report(
                    (ordinal, CONTENT),
                    Code::UnknownElement,
                    Some(node),
                    message,
                );
            }
        }
        // libxml2 keeps no namespace node for the predefined `xml` binding.
        let attributes = attributes.filter(|(attribute, _)| attribute.qualified != "xmlns:xml");
        let ordinary = (attributes.clone()).filter(|(name, _)| !name.is_namespace_declaration());
        let namespaces = attributes.filter(|(name, _)| name.is_namespace_declaration());
        for (attribute, value) in ordinary.chain(namespaces) {
            self.attribute(dtd, (ordinal, node), name, attribute, value);
        }
        self.frames.push(frame);
    }

    /// Character data in the open element.
    pub(crate) fn text(&mut self, dtd: &Dtd, text: &str, cdata: bool) {
        if cdata {
            self.flush(dtd);
        }
        let Some(frame) = self.frames.last_mut() else {
            return;
        };
        let content = frame
            .decl
            .and_then(|index| dtd.elements[index].content.as_ref());
        match content {
            _ if text.is_empty() => {}
            Some(Content::Empty) => self.once(dtd, Code::NotEmpty),
            Some(Content::Children(_)) => {
                let blank = !cdata && text.chars().all(blank);
                if !cdata && frame.watch_blanks {
                    frame.run = Some(frame.run.unwrap_or(true) && blank);
                }
                if !blank && frame.state.take().is_some() {
                    self.mismatch(dtd, "Text not allowed");
                }
            }
            _ => {}
        }
    }

    /// A comment, processing instruction or entity reference in the open
    /// element: content for `EMPTY`, as libxml2 counts any child node.
    pub(crate) fn other(&mut self, dtd: &Dtd) {
        self.flush(dtd);
        if let Some(frame) = self.frames.last()
            && let Some(index) = frame.decl
            && matches!(dtd.elements[index].content, Some(Content::Empty))
        {
            self.once(dtd, Code::NotEmpty);
        }
    }

    /// The end of a run of character data: an entity boundary or another node.
    pub(crate) fn flush(&mut self, dtd: &Dtd) {
        if let Some(frame) = self.frames.last_mut()
            && frame.run.take() == Some(true)
        {
            self.once(dtd, Code::StandaloneWhiteSpace);
        }
    }

    /// The open element ends; after the document element, IDREFs are resolved.
    pub(crate) fn end(&mut self, dtd: &Dtd) {
        self.flush(dtd);
        if let Some(frame) = self.frames.last()
            && let Some(state) = frame.state
            && let Some(index) = frame.decl
            && let Some(Content::Children(model)) = &dtd.elements[index].content
            && !model.accepts(state)
        {
            self.mismatch(dtd, "Expecting more children");
        }
        self.frames.pop();
        if self.frames.is_empty() {
            self.references();
        }
    }

    /// Element `name` is a child of the open element.
    fn child(&mut self, dtd: &Dtd, name: QName<'_>) {
        self.flush(dtd);
        let Some(frame) = self.frames.last_mut() else {
            return;
        };
        let Some(index) = frame.decl else {
            return;
        };
        match dtd.elements[index].content.as_ref().expect("declared") {
            Content::Empty => self.once(dtd, Code::NotEmpty),
            Content::Any => {}
            Content::Mixed(names) if names.is_empty() => self.once(dtd, Code::NotPcdata),
            Content::Mixed(names) => {
                let allowed = self.mixed.entry(index).or_insert_with(|| {
                    names
                        .iter()
                        .map(|allowed| allowed.rsplit(':').next().unwrap_or_default().into())
                        .collect()
                });
                if !allowed.contains(name.local) {
                    let (key, node) = ((frame.ordinal, CONTENT), frame.node);
                    let message = format!(
                        "Element {} is not declared in {} list of possible children",
                        name.local,
                        local(&dtd.elements[index].name)
                    );
                    self.report(key, Code::InvalidChild, Some(node), message);
                }
            }
            Content::Children(model) => {
                if let Some(state) = frame.state {
                    frame.state = model.step(state, name.qualified);
                    if frame.state.is_none() {
                        self.mismatch(dtd, &format!("Misplaced {}", name.qualified));
                    }
                }
            }
        }
    }

    /// A content diagnostic of the open element that libxml2 reports once.
    fn once(&mut self, dtd: &Dtd, code: Code) {
        let frame = self.frames.last_mut().expect("open element");
        if std::mem::replace(&mut frame.noted, true) {
            return;
        }
        let (key, node) = ((frame.ordinal, CONTENT), frame.node);
        let name = local(&dtd.elements[frame.decl.expect("declared")].name);
        let message = match code {
            Code::NotEmpty => format!("Element {name} was declared EMPTY this one has content"),
            Code::NotPcdata => {
                format!("Element {name} was declared #PCDATA but contains non text nodes")
            }
            _ => format!(
                "standalone: {name} declared in the external subset contains white spaces nodes"
            ),
        };
        self.report(key, code, Some(node), message);
    }

    /// The open element's content does not follow its model.
    fn mismatch(&mut self, dtd: &Dtd, why: &str) {
        let frame = self.frames.last().expect("open element");
        let (key, node) = ((frame.ordinal, MODEL), frame.node);
        let name = local(&dtd.elements[frame.decl.expect("declared")].name);
        let message = format!("Element {name} content does not follow the DTD, {why}");
        self.report(key, Code::ContentModel, Some(node), message);
    }

    /// VC: Required Attribute, with libxml2's prefix leniency.
    fn required<'x>(
        &mut self,
        (ordinal, node): (usize, Node<Id>),
        name: QName<'_>,
        declared: &[AttributeDecl],
        present: impl Iterator<Item = (QName<'x>, &'x str)>,
    ) {
        // Indexed once per element so each required attribute is one lookup,
        // not a scan of the present attributes.
        let mut namespaces = HashSet::new();
        let mut ordinary: HashMap<&str, (bool, HashSet<Option<&str>>)> = HashMap::new();
        for (name, _) in present {
            if name.is_namespace_declaration() {
                namespaces.insert(name.qualified);
            } else {
                let prefix = name.qualified.split_once(':').map(|(prefix, _)| prefix);
                let (prefixed, prefixes) = ordinary.entry(name.local).or_default();
                *prefixed |= prefix.is_some();
                prefixes.insert(prefix);
            }
        }
        for attribute in declared {
            if !matches!(attribute.default, StoredDefault::Required) {
                continue;
            }
            let wanted = attribute.name.as_str();
            let namespace = wanted == "xmlns" || wanted.starts_with("xmlns:");
            let code = match wanted.split_once(':') {
                _ if namespace => (!namespaces.contains(wanted)).then_some(Code::MissingAttribute),
                None => (!ordinary.contains_key(wanted)).then_some(Code::MissingAttribute),
                Some((prefix, local)) => match ordinary.get(local) {
                    None => Some(Code::MissingAttribute),
                    Some((_, prefixes)) if prefixes.contains(&Some(prefix)) => None,
                    Some((true, _)) => Some(Code::DifferentPrefix),
                    Some((false, _)) => Some(Code::NoPrefix),
                },
            };
            let element = name.local;
            let message = match code {
                None => continue,
                Some(Code::MissingAttribute) => {
                    format!("Element {element} does not carry attribute {wanted}")
                }
                Some(Code::NoPrefix) => {
                    format!("Element {element} required attribute {wanted} has no prefix")
                }
                Some(_) => {
                    format!("Element {element} required attribute {wanted} has different prefix")
                }
            };
            self.report(
                (ordinal, REQUIRED),
                code.expect("reported"),
                Some(node),
                message,
            );
        }
    }

    /// `xmlValidateOneAttribute`, or `xmlValidateOneNamespace` for declarations.
    fn attribute(
        &mut self,
        dtd: &Dtd,
        (ordinal, node): (usize, Node<Id>),
        element: QName<'_>,
        attribute: QName<'_>,
        value: &str,
    ) {
        let key = (ordinal, ATTRIBUTES);
        let namespace = attribute.is_namespace_declaration();
        let shown = match namespace {
            true => attribute.qualified,
            false => attribute.local,
        };
        let owner = element.local;
        let Some((slot, decl)) = dtd.attribute_for(element, attribute.qualified) else {
            let message = format!("No declaration for attribute {shown} of element {owner}");
            return self.report(key, Code::UnknownAttribute, Some(node), message);
        };
        if !value_matches(decl.kind, value) {
            let code = match namespace {
                true => Code::InvalidDefault,
                false => Code::AttributeValue,
            };
            let message = format!("Syntax of value for attribute {shown} of {owner} is not valid");
            self.report(key, code, Some(node), message);
        }
        let fixed = decl.fixed().filter(|&fixed| fixed != value);
        if let Some(fixed) = fixed {
            let message = format!(
                "Value for attribute {shown} of {owner} is different from default \"{fixed}\""
            );
            self.report(key, Code::AttributeDefault, Some(node), message);
        }
        match decl.kind {
            AttributeKind::Id if !namespace => {
                // libxml2's xmlAddID refuses an empty value as a duplicate.
                if self.refused || self.ids.try_reserve(1).is_err() {
                    self.refuse();
                } else if value.is_empty() || !self.ids.insert(value.into()) {
                    let message = format!("ID {value} already defined");
                    self.report(key, Code::IdRedefined, Some(node), message);
                }
            }
            AttributeKind::IdRef | AttributeKind::IdRefs if !namespace => {
                if self.refused || self.references.try_reserve(1).is_err() {
                    return self.refuse();
                }
                self.references.push(Reference {
                    node,
                    attribute: attribute.local.into(),
                    value: value.into(),
                    multiple: decl.kind == AttributeKind::IdRefs,
                });
            }
            AttributeKind::Notation => {
                if !dtd.notation_index.contains_key(value) {
                    let message = format!(
                        "Value \"{value}\" for attribute {shown} of {owner} is not a declared Notation"
                    );
                    self.report(key, Code::UnknownNotation, Some(node), message);
                }
                if !self.enumerated(slot, decl).contains(value) {
                    let message = format!(
                        "Value \"{value}\" for attribute {shown} of {owner} is not among the enumerated notations"
                    );
                    self.report(key, Code::NotationValue, Some(node), message);
                }
            }
            AttributeKind::Enumeration if !self.enumerated(slot, decl).contains(value) => {
                let message = format!(
                    "Value \"{value}\" for attribute {shown} of {owner} is not among the enumerated set"
                );
                self.report(key, Code::AttributeValue, Some(node), message);
            }
            _ => {}
        }
        if let Some(fixed) = fixed {
            let code = match namespace {
                true => Code::ElementNamespace,
                false => Code::AttributeValue,
            };
            let message = format!("Value for attribute {shown} of {owner} must be \"{fixed}\"");
            self.report(key, code, Some(node), message);
        }
        // xmlValidateAttributeValue2
        match decl.kind {
            AttributeKind::Entity => self.entity(dtd, (key, node), ("ENTITY", shown), value),
            AttributeKind::Entities => {
                for name in value.split(blank).filter(|name| !name.is_empty()) {
                    self.entity(dtd, (key, node), ("ENTITIES", shown), name);
                }
            }
            AttributeKind::Notation if !dtd.notation_index.contains_key(value) => {
                let message =
                    format!("NOTATION attribute {shown} reference an unknown notation \"{value}\"");
                self.report(key, Code::UnknownNotation, Some(node), message);
            }
            _ => {}
        }
    }

    fn enumerated(&mut self, slot: (usize, usize), decl: &AttributeDecl) -> &HashSet<Box<str>> {
        self.enumerated.entry(slot).or_insert_with(|| {
            decl.values
                .iter()
                .map(|token| token.as_str().into())
                .collect()
        })
    }

    fn entity(
        &mut self,
        dtd: &Dtd,
        (key, node): (Key, Node<Id>),
        (kind, attribute): (&str, &str),
        value: &str,
    ) {
        let entity = dtd.declared(value).map(|id| &dtd.entities[id.0].body);
        let message = match entity {
            Some(EntityBody::External(external)) if external.notation.is_some() => return,
            None if predefined_entity(value).is_none() => {
                let message =
                    format!("{kind} attribute {attribute} reference an unknown entity \"{value}\"");
                return self.report(key, Code::UnknownEntity, Some(node), message);
            }
            _ => format!(
                "{kind} attribute {attribute} reference an entity \"{value}\" of wrong type"
            ),
        };
        self.report(key, Code::EntityType, Some(node), message);
    }

    /// `xmlValidateDocumentFinal`: every IDREF names an ID.
    fn references(&mut self) {
        for reference in std::mem::take(&mut self.references) {
            let (value, multiple) = (&*reference.value, reference.multiple);
            let tokens = multiple.then(|| value.split(blank).filter(|name| !name.is_empty()));
            let single = (!multiple).then_some(value);
            for name in tokens.into_iter().flatten().chain(single) {
                if !self.ids.contains(name) {
                    let kind = if multiple { "IDREFS" } else { "IDREF" };
                    let attribute = &reference.attribute;
                    let message =
                        format!("{kind} attribute {attribute} references an unknown ID \"{name}\"");
                    let key = (usize::MAX, ROOT);
                    self.report(key, Code::UnknownId, Some(reference.node), message);
                }
            }
        }
    }
}

/// What a refused allocation leaves.
fn refused<Id>() -> Vec<Diagnostic<Id>> {
    vec![Diagnostic {
        code: Code::Allocation,
        node: None,
        offset: None,
        message: "Memory allocation failed".to_owned(),
    }]
}

fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// Feeds a tree's nodes from its document element to a validator, entering
/// entity references, and returns the diagnostics in `xmlValidateDocument`'s
/// order and wording.
pub(crate) fn run<T: Navigate>(tree: &T, dtd: &Dtd, document: bool) -> Vec<Diagnostic<T::Id>> {
    let mut validator = Validator::new(document);
    validator.begin(dtd);
    let node = |id| Node {
        id,
        offset: tree.offset(id),
    };
    let first = std::iter::successors(tree.first_child(tree.root()), |&id| tree.next_sibling(id))
        .find(|&id| matches!(tree.item(id), Item::Element(_)));
    if let Some(root) = first {
        let mut id = root;
        'walk: loop {
            match tree.item(id) {
                Item::Element(name) => {
                    let attributes = (0..).map_while(move |index| tree.attribute(id, index));
                    validator.start(dtd, node(id), name, attributes);
                }
                Item::Text(text) => {
                    validator.flush(dtd);
                    validator.text(dtd, text, tree.is_cdata(id));
                }
                Item::Comment(_) | Item::Pi(..) | Item::EntityRef(_) => validator.other(dtd),
                Item::Root => validator.flush(dtd),
            }
            if let Some(child) = tree.first_child(id) {
                id = child;
                continue;
            }
            loop {
                match tree.item(id) {
                    Item::Element(_) => validator.end(dtd),
                    Item::EntityRef(_) => validator.flush(dtd),
                    _ => {}
                }
                if id == root {
                    break 'walk;
                }
                if let Some(next) = tree.next_sibling(id) {
                    id = next;
                    break;
                }
                id = tree.parent(id).expect("inside the document element");
            }
        }
    }
    let mut out = validator.sorted();
    for diagnostic in &mut out {
        if diagnostic.code == Code::ContentModel
            && let Some(Item::Element(name)) = diagnostic.node.map(|id| tree.item(id))
        {
            diagnostic.message = format!("Element {} content does not follow the DTD", name.local);
        }
    }
    out
}
