//! RELAX NG section 7 restrictions on the simplified, reachable schema:
//! prohibited paths (7.1), string sequences (7.2), repeated attributes
//! (7.3) and interleave overlaps (7.4). Each check is memoized per pattern,
//! and name class comparisons spend a work budget.

use std::collections::{HashMap, HashSet};

use super::SchemaCode;
use super::pattern::{Id, NOT_ALLOWED, NameClass, Pat, Store, Tables};

const ATTRIBUTE: u8 = 1;
const ONE_OR_MORE: u8 = 2;
const ONE_OR_MORE_GROUP: u8 = 4;
const ONE_OR_MORE_INTERLEAVE: u8 = 8;
const LIST: u8 = 16;
const EXCEPT: u8 = 32;
const START: u8 = 64;

/// The content types of section 7.2, ordered so that `max` combines them.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum ContentType {
    Empty,
    Complex,
    Simple,
}

fn groupable(a: ContentType, b: ContentType) -> bool {
    a == ContentType::Empty
        || b == ContentType::Empty
        || (a == ContentType::Complex && b == ContentType::Complex)
}

type Violation = (SchemaCode, Id, String);

struct Checker<'a, 's> {
    store: &'a Store<'s>,
    tables: &'a Tables,
    budget: usize,
    walked: HashSet<(Id, u8)>,
    elements: HashSet<u32>,
    queue: Vec<u32>,
    attributes: HashMap<Id, Vec<u32>>,
    content: HashMap<Id, Option<ContentType>>,
    errors: Vec<Violation>,
}

/// Every violation in the patterns reachable from `start`.
pub(super) fn restrictions(
    store: &Store<'_>,
    tables: &Tables,
    start: Id,
    budget: usize,
) -> Vec<Violation> {
    let mut checker = Checker {
        store,
        tables,
        budget,
        walked: HashSet::new(),
        elements: HashSet::new(),
        queue: Vec::new(),
        attributes: HashMap::new(),
        content: HashMap::new(),
        errors: Vec::new(),
    };
    checker.walk(start, START);
    while let Some(element) = checker.queue.pop()
        && checker.budget > 0
    {
        let (class, content) = tables.elements[element as usize];
        checker.walk(content, 0);
        if checker.content_type(content).is_none() {
            let name = tables.describe(class).unwrap_or("");
            let message = format!("Element {name} has a content type error");
            checker
                .errors
                .push((SchemaCode::ElemContentError, content, message));
        }
    }
    if checker.budget == 0 {
        let message = "schema work limit exceeded".into();
        checker
            .errors
            .push((SchemaCode::ResourceLimit, start, message));
    }
    checker.errors
}

impl Checker<'_, '_> {
    fn forbid(&mut self, id: Id, context: u8, rules: &[(u8, SchemaCode, &str)]) {
        if let Some(&(_, code, path)) = rules.iter().find(|(flag, ..)| context & flag != 0) {
            self.errors
                .push((code, id, format!("Found forbidden pattern {path}")));
        }
    }

    fn walk(&mut self, id: Id, context: u8) {
        if !self.walked.insert((id, context)) {
            return;
        }
        use SchemaCode::*;
        match self.store.pat(id) {
            Pat::Element(element) => {
                self.forbid(
                    id,
                    context,
                    &[
                        (ATTRIBUTE, PatAttrElem, "attribute//element(ref)"),
                        (LIST, PatListElem, "list//element(ref)"),
                        (EXCEPT, PatDataExceptElem, "data/except//element(ref)"),
                    ],
                );
                if self.elements.insert(element) {
                    self.queue.push(element);
                }
            }
            Pat::Attribute(class, content) => {
                self.forbid(
                    id,
                    context,
                    &[
                        (ATTRIBUTE, PatAttrAttr, "attribute//attribute"),
                        (
                            ONE_OR_MORE_GROUP,
                            PatOneMoreGroupAttr,
                            "oneOrMore//group//attribute",
                        ),
                        (
                            ONE_OR_MORE_INTERLEAVE,
                            PatOneMoreInterleaveAttr,
                            "oneOrMore//interleave//attribute",
                        ),
                        (LIST, PatListAttr, "list//attribute"),
                        (EXCEPT, PatDataExceptAttr, "data/except//attribute"),
                        (START, PatStartAttr, "start//attribute"),
                    ],
                );
                if context & ONE_OR_MORE == 0
                    && let Some((code, what)) = self.infinite(class)
                {
                    let message = format!("Found {what} attribute without oneOrMore ancestor");
                    self.errors.push((code, id, message));
                }
                self.walk(content, context | ATTRIBUTE);
            }
            Pat::OneOrMore(a) => {
                self.forbid(
                    id,
                    context,
                    &[
                        (EXCEPT, PatDataExceptOneMore, "data/except//oneOrMore"),
                        (START, PatStartOneMore, "start//oneOrMore"),
                    ],
                );
                self.walk(a, context | ONE_OR_MORE);
            }
            Pat::Group(a, b) => {
                self.forbid(
                    id,
                    context,
                    &[
                        (EXCEPT, PatDataExceptGroup, "data/except//group"),
                        (START, PatStartGroup, "start//group"),
                    ],
                );
                let inner = context
                    | if context & ONE_OR_MORE != 0 {
                        ONE_OR_MORE_GROUP
                    } else {
                        0
                    };
                self.walk(a, inner);
                self.walk(b, inner);
                self.attribute_conflict(id, a, b, GroupAttrConflict, "group");
            }
            Pat::Interleave(a, b) => {
                self.forbid(
                    id,
                    context,
                    &[
                        (LIST, PatListInterleave, "list//interleave"),
                        (EXCEPT, PatDataExceptInterleave, "data/except//interleave"),
                        (START, PatStartInterleave, "start//interleave"),
                    ],
                );
                let inner = context
                    | if context & ONE_OR_MORE != 0 {
                        ONE_OR_MORE_INTERLEAVE
                    } else {
                        0
                    };
                self.walk(a, inner);
                self.walk(b, inner);
                self.attribute_conflict(id, a, b, AttrConflict, "interleave");
                self.interleave_conflict(id, a, b);
            }
            Pat::Choice(a, b) => {
                self.walk(a, context);
                self.walk(b, context);
            }
            Pat::List(a) => {
                self.forbid(
                    id,
                    context,
                    &[
                        (LIST, PatListList, "list//list"),
                        (EXCEPT, PatDataExceptList, "data/except//list"),
                        (START, PatStartList, "start//list"),
                    ],
                );
                self.walk(a, context | LIST);
            }
            Pat::Text => self.forbid(
                id,
                context,
                &[
                    (LIST, PatListText, "list//text"),
                    (EXCEPT, PatDataExceptText, "data/except//text"),
                    (START, PatStartText, "start//text"),
                ],
            ),
            Pat::Data(_, except) => {
                self.forbid(id, context, &[(START, PatStartData, "start//data")]);
                if except != NOT_ALLOWED {
                    self.walk(except, context | EXCEPT);
                }
            }
            Pat::Value(_) => self.forbid(id, context, &[(START, PatStartValue, "start//value")]),
            Pat::Empty => self.forbid(
                id,
                context,
                &[
                    (EXCEPT, PatDataExceptEmpty, "data/except//empty"),
                    (START, PatStartEmpty, "start//empty"),
                ],
            ),
            Pat::NotAllowed | Pat::After(..) => {}
        }
    }

    /// Section 7.3: an attribute with an `anyName` or `nsName` must repeat.
    fn infinite(&self, class: u32) -> Option<(SchemaCode, &'static str)> {
        match &self.tables.names[class as usize] {
            NameClass::AnyName(_) => Some((SchemaCode::AnyNameAttrAncestor, "anyName")),
            NameClass::NsName(..) => Some((SchemaCode::NsNameAttrAncestor, "nsName")),
            NameClass::Name(..) => None,
            NameClass::Choice(a, b) => self.infinite(*a).or_else(|| self.infinite(*b)),
        }
    }

    /// Whether any pair of classes overlaps, spending the budget.
    fn overlapping(&mut self, xs: &[u32], ys: &[u32]) -> bool {
        for &x in xs {
            for &y in ys {
                self.budget = self.budget.saturating_sub(1);
                if self.budget == 0 || self.tables.overlap(x, y) {
                    return self.budget != 0;
                }
            }
        }
        false
    }

    fn attribute_conflict(&mut self, id: Id, a: Id, b: Id, code: SchemaCode, what: &str) {
        let (xs, ys) = (self.attribute_classes(a), self.attribute_classes(b));
        if self.overlapping(&xs, &ys) {
            self.errors
                .push((code, id, format!("Attributes conflicts in {what}")));
        }
    }

    /// Section 7.4: interleaved patterns share no element name and not both have text.
    fn interleave_conflict(&mut self, id: Id, a: Id, b: Id) {
        let ((xs, x_text), (ys, y_text)) = (self.top_level(a), self.top_level(b));
        if (x_text && y_text) || self.overlapping(&xs, &ys) {
            self.errors.push((
                SchemaCode::ElemTextConflict,
                id,
                "Element or text conflicts in interleave".into(),
            ));
        }
    }

    /// Name classes of the attributes a pattern contains outside elements.
    fn attribute_classes(&mut self, id: Id) -> Vec<u32> {
        if let Some(found) = self.attributes.get(&id) {
            return found.clone();
        }
        let found = match self.store.pat(id) {
            Pat::Attribute(class, _) => vec![class],
            Pat::Choice(a, b) | Pat::Group(a, b) | Pat::Interleave(a, b) => {
                let mut found = self.attribute_classes(a);
                found.extend(self.attribute_classes(b));
                found
            }
            Pat::OneOrMore(a) => self.attribute_classes(a),
            _ => Vec::new(),
        };
        self.budget = self.budget.saturating_sub(found.len());
        self.attributes.insert(id, found.clone());
        found
    }

    /// Element name classes a pattern starts outside attributes, and whether it has text.
    fn top_level(&mut self, id: Id) -> (Vec<u32>, bool) {
        let mut classes = Vec::new();
        let mut text = false;
        let mut seen = HashSet::new();
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            self.budget = self.budget.saturating_sub(1);
            match self.store.pat(id) {
                Pat::Element(element) => classes.push(self.tables.elements[element as usize].0),
                Pat::Text => text = true,
                Pat::Choice(a, b) | Pat::Group(a, b) | Pat::Interleave(a, b) => {
                    stack.extend([a, b])
                }
                Pat::OneOrMore(a) => stack.push(a),
                _ => {}
            }
        }
        (classes, text)
    }

    fn content_type(&mut self, id: Id) -> Option<ContentType> {
        if let Some(&found) = self.content.get(&id) {
            return found;
        }
        let found = self.compute_content_type(id);
        self.content.insert(id, found);
        found
    }

    fn compute_content_type(&mut self, id: Id) -> Option<ContentType> {
        use ContentType::*;
        match self.store.pat(id) {
            Pat::Empty | Pat::NotAllowed | Pat::Attribute(..) | Pat::After(..) => Some(Empty),
            Pat::Text | Pat::Element(_) => Some(Complex),
            Pat::Data(..) | Pat::Value(_) | Pat::List(_) => Some(Simple),
            Pat::Choice(a, b) => Some(self.content_type(a)?.max(self.content_type(b)?)),
            Pat::Group(a, b) | Pat::Interleave(a, b) => {
                let (x, y) = (self.content_type(a)?, self.content_type(b)?);
                groupable(x, y).then(|| x.max(y))
            }
            Pat::OneOrMore(a) => self.content_type(a).filter(|&x| groupable(x, x)),
        }
    }
}
