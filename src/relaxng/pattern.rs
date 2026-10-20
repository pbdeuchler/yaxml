//! Hash-consed simplified patterns (RELAX NG section 4 output) and James
//! Clark's derivative algorithm ("An algorithm for RELAX NG validation").
//!
//! Every pattern is interned, so equal patterns share one id; choices are
//! kept as sorted, duplicate-free balanced trees, so the set of states a
//! validation explores stays small. Derivatives are memoized and every new
//! pattern or memo miss spends one unit of a work budget.

use std::collections::{HashMap, TryReserveError};
use std::hash::Hash;
use std::sync::Arc;

use super::datatype::{Datatype, ValueDef};
use crate::fallible::TryPush;
use crate::schema_types::{Context, Type};

pub(super) type Id = u32;
pub(super) const EMPTY: Id = 0;
pub(super) const NOT_ALLOWED: Id = 1;
pub(super) const TEXT: Id = 2;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Pat {
    Empty,
    NotAllowed,
    Text,
    Choice(Id, Id),
    Interleave(Id, Id),
    Group(Id, Id),
    OneOrMore(Id),
    List(Id),
    /// A datatype index and its `except` pattern (`NOT_ALLOWED` if none).
    Data(u32, Id),
    Value(u32),
    /// A name class index and the value pattern.
    Attribute(u32, Id),
    /// An index into [`Tables::elements`].
    Element(u32),
    After(Id, Id),
}

/// A name class (RELAX NG section 4.16 form).
#[derive(Clone, Debug)]
pub(super) enum NameClass {
    AnyName(Option<u32>),
    NsName(Box<str>, Option<u32>),
    Name(Box<str>, Box<str>),
    Choice(u32, u32),
}

/// Everything patterns refer to by index.
#[derive(Debug, Default)]
pub(super) struct Tables {
    pub(super) names: Vec<NameClass>,
    /// Name class and content of each element pattern.
    pub(super) elements: Vec<(u32, Id)>,
    pub(super) datatypes: Vec<Datatype>,
    pub(super) values: Vec<ValueDef>,
    /// Element class, attribute class and `ID`, `IDREF` or `IDREFS` for
    /// each attribute whose content is one of those types.
    pub(super) ids: Vec<(u32, u32, Type)>,
}

impl Tables {
    pub(super) fn contains(&self, class: u32, ns: &str, local: &str) -> bool {
        match &self.names[class as usize] {
            NameClass::AnyName(except) => except.is_none_or(|e| !self.contains(e, ns, local)),
            NameClass::NsName(uri, except) => {
                **uri == *ns && except.is_none_or(|e| !self.contains(e, ns, local))
            }
            NameClass::Name(uri, name) => **uri == *ns && **name == *local,
            NameClass::Choice(a, b) => self.contains(*a, ns, local) || self.contains(*b, ns, local),
        }
    }

    /// Whether some name belongs to both classes (section 7.3): it suffices
    /// to try every name either mentions plus one name outside all of them.
    pub(super) fn overlap(&self, a: u32, b: u32) -> bool {
        let mut names = Vec::new();
        self.representatives(a, &mut names);
        self.representatives(b, &mut names);
        names
            .iter()
            .any(|(ns, local)| self.contains(a, ns, local) && self.contains(b, ns, local))
    }

    fn representatives<'a>(&'a self, class: u32, out: &mut Vec<(&'a str, &'a str)>) {
        const ILLEGAL: &str = "\u{0}";
        match &self.names[class as usize] {
            NameClass::AnyName(except) => {
                out.push((ILLEGAL, ILLEGAL));
                except.inspect(|&e| self.representatives(e, out));
            }
            NameClass::NsName(ns, except) => {
                out.push((ns, ILLEGAL));
                except.inspect(|&e| self.representatives(e, out));
            }
            NameClass::Name(ns, local) => out.push((ns, local)),
            NameClass::Choice(x, y) => {
                self.representatives(*x, out);
                self.representatives(*y, out);
            }
        }
    }

    /// A readable name for diagnostics: the local name of a single name.
    pub(super) fn describe(&self, class: u32) -> Option<&str> {
        match &self.names[class as usize] {
            NameClass::Name(_, local) => Some(local),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct Arena {
    pats: Vec<Pat>,
    nullable: Vec<bool>,
    index: HashMap<Pat, Id>,
}

impl Arena {
    pub(super) fn len(&self) -> usize {
        self.pats.len()
    }
}

/// Patterns being built: a shared base arena plus this store's own.
pub(super) struct Store {
    base: Arc<Arena>,
    own: Arena,
    /// Patterns created and choice alternatives merged.
    pub(super) work: usize,
    /// An allocation was refused: patterns since are `NOT_ALLOWED`, and
    /// the work is spent.
    pub(super) refused: bool,
}

impl Store {
    /// A store over `base`; an empty base gets the fixed patterns.
    pub(super) fn new(base: Arc<Arena>) -> Self {
        let empty = base.len() == 0;
        let mut store = Self {
            base,
            own: Arena::default(),
            work: 0,
            refused: false,
        };
        if empty {
            for pat in [Pat::Empty, Pat::NotAllowed, Pat::Text] {
                store.insert(pat);
            }
        }
        store
    }

    pub(super) fn into_arena(self) -> Arena {
        self.own
    }

    pub(super) fn len(&self) -> usize {
        self.base.len() + self.own.len()
    }

    pub(super) fn pat(&self, id: Id) -> Pat {
        let id = id as usize;
        match id.checked_sub(self.base.len()) {
            None => self.base.pats[id],
            Some(own) => self.own.pats[own],
        }
    }

    pub(super) fn nullable(&self, id: Id) -> bool {
        let id = id as usize;
        match id.checked_sub(self.base.len()) {
            None => self.base.nullable[id],
            Some(own) => self.own.nullable[own],
        }
    }

    /// Interns `pat` as is.
    fn intern(&mut self, pat: Pat) -> Id {
        if let Some(&id) = self
            .base
            .index
            .get(&pat)
            .or_else(|| self.own.index.get(&pat))
        {
            return id;
        }
        let own = &mut self.own;
        let room = (own.pats.try_reserve(1))
            .and(own.nullable.try_reserve(1))
            .and(own.index.try_reserve(1));
        match self.refuses(room) {
            true => NOT_ALLOWED,
            false => self.insert(pat),
        }
    }

    fn insert(&mut self, pat: Pat) -> Id {
        let nullable = match pat {
            Pat::Empty | Pat::Text => true,
            Pat::Choice(a, b) => self.nullable(a) || self.nullable(b),
            Pat::Interleave(a, b) | Pat::Group(a, b) => self.nullable(a) && self.nullable(b),
            Pat::OneOrMore(a) => self.nullable(a),
            _ => false,
        };
        let id = self.len() as Id;
        self.work += 1;
        self.own.pats.push(pat);
        self.own.nullable.push(nullable);
        self.own.index.insert(pat, id);
        id
    }

    pub(super) fn choice(&mut self, a: Id, b: Id) -> Id {
        if a == NOT_ALLOWED || a == b {
            return b;
        }
        if b == NOT_ALLOWED {
            return a;
        }
        let mut alternatives = Vec::new();
        self.alternatives(a, &mut alternatives);
        self.alternatives(b, &mut alternatives);
        if self.refused {
            return NOT_ALLOWED;
        }
        alternatives.sort_unstable();
        alternatives.dedup();
        self.work += alternatives.len();
        self.balanced(&alternatives)
    }

    fn alternatives(&mut self, id: Id, out: &mut Vec<Id>) {
        match self.pat(id) {
            Pat::Choice(a, b) => {
                self.alternatives(a, out);
                self.alternatives(b, out);
            }
            _ => drop(self.refuses(out.try_push(id))),
        }
    }

    /// Whether `growth` was refused, which refuses the store.
    pub(super) fn refuses(&mut self, growth: Result<(), TryReserveError>) -> bool {
        self.refused |= growth.is_err();
        growth.is_err()
    }

    /// Memoizes `r` under `key`, unless the allocation is refused.
    pub(super) fn remember<K: Eq + Hash>(
        &mut self,
        memo: &mut HashMap<K, Id>,
        key: K,
        r: Id,
    ) -> Id {
        if !self.refuses(memo.try_reserve(1)) {
            memo.insert(key, r);
        }
        r
    }

    fn balanced(&mut self, alternatives: &[Id]) -> Id {
        match alternatives {
            [one] => *one,
            _ => {
                let (left, right) = alternatives.split_at(alternatives.len() / 2);
                let (left, right) = (self.balanced(left), self.balanced(right));
                self.intern(Pat::Choice(left, right))
            }
        }
    }

    pub(super) fn group(&mut self, a: Id, b: Id) -> Id {
        match (a, b) {
            (NOT_ALLOWED, _) | (_, NOT_ALLOWED) => NOT_ALLOWED,
            (EMPTY, other) | (other, EMPTY) => other,
            _ => self.intern(Pat::Group(a, b)),
        }
    }

    pub(super) fn interleave(&mut self, a: Id, b: Id) -> Id {
        match (a, b) {
            (NOT_ALLOWED, _) | (_, NOT_ALLOWED) => NOT_ALLOWED,
            (EMPTY, other) | (other, EMPTY) => other,
            _ => self.intern(Pat::Interleave(a, b)),
        }
    }

    pub(super) fn after(&mut self, a: Id, b: Id) -> Id {
        match (a, b) {
            (NOT_ALLOWED, _) | (_, NOT_ALLOWED) => NOT_ALLOWED,
            _ => self.intern(Pat::After(a, b)),
        }
    }

    pub(super) fn one_or_more(&mut self, a: Id) -> Id {
        match a {
            NOT_ALLOWED | EMPTY => a,
            _ => self.intern(Pat::OneOrMore(a)),
        }
    }

    /// `list`, `attribute` and `data` with their section 4.20 simplification.
    pub(super) fn make(&mut self, pat: Pat) -> Id {
        match pat {
            Pat::List(NOT_ALLOWED) | Pat::Attribute(_, NOT_ALLOWED) => NOT_ALLOWED,
            _ => self.intern(pat),
        }
    }
}

/// How [`Deriver::apply_after`] rebuilds the continuation of an `after`.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum Then {
    /// `group(x, id)`
    Group(Id),
    /// `interleave(x, id)`
    InterleaveLeft(Id),
    /// `interleave(id, x)`
    InterleaveRight(Id),
    /// `after(x, id)`
    After(Id),
}

/// Derivative computation over a schema's patterns, within a work budget.
pub(super) struct Deriver {
    pub(super) store: Store,
    pub(super) tables: Arc<Tables>,
    budget: usize,
    open: HashMap<(Id, u32), Id>,
    close: HashMap<(Id, bool), Id>,
    end: HashMap<(Id, bool), Id>,
    then: HashMap<(Then, Id), Id>,
}

/// An element or attribute name: namespace (`""` for none) and local part,
/// with a key that identifies it within one validation.
#[derive(Clone, Copy)]
pub(super) struct Name<'n> {
    pub(super) ns: &'n str,
    pub(super) local: &'n str,
    pub(super) key: u32,
}

pub(super) fn is_whitespace(text: &str) -> bool {
    text.bytes()
        .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
}

impl Deriver {
    pub(super) fn new(base: Arc<Arena>, tables: Arc<Tables>, budget: usize) -> Self {
        Self {
            store: Store::new(base),
            tables,
            budget,
            open: HashMap::new(),
            close: HashMap::new(),
            end: HashMap::new(),
            then: HashMap::new(),
        }
    }

    pub(super) fn exhausted(&self) -> bool {
        self.store.work > self.budget || self.store.refused
    }

    /// Spends one unit; false once the budget is gone.
    fn spend(&mut self) -> bool {
        self.store.work += 1;
        !self.exhausted()
    }

    pub(super) fn start_tag_open(&mut self, p: Id, name: Name<'_>) -> Id {
        if let Some(&r) = self.open.get(&(p, name.key)) {
            return r;
        }
        if !self.spend() {
            return NOT_ALLOWED;
        }
        let r = match self.store.pat(p) {
            Pat::Choice(a, b) => {
                let (x, y) = (self.start_tag_open(a, name), self.start_tag_open(b, name));
                self.store.choice(x, y)
            }
            Pat::Element(e) => {
                let (class, content) = self.tables.elements[e as usize];
                match self.tables.contains(class, name.ns, name.local) {
                    true => self.store.after(content, EMPTY),
                    false => NOT_ALLOWED,
                }
            }
            Pat::Interleave(a, b) => {
                let x = self.start_tag_open(a, name);
                let x = self.apply_after(Then::InterleaveLeft(b), x);
                let y = self.start_tag_open(b, name);
                let y = self.apply_after(Then::InterleaveRight(a), y);
                self.store.choice(x, y)
            }
            Pat::OneOrMore(a) => {
                let rest = self.store.choice(p, EMPTY);
                let x = self.start_tag_open(a, name);
                self.apply_after(Then::Group(rest), x)
            }
            Pat::Group(a, b) => {
                let x = self.start_tag_open(a, name);
                let x = self.apply_after(Then::Group(b), x);
                match self.store.nullable(a) {
                    true => {
                        let y = self.start_tag_open(b, name);
                        self.store.choice(x, y)
                    }
                    false => x,
                }
            }
            Pat::After(a, b) => {
                let x = self.start_tag_open(a, name);
                self.apply_after(Then::After(b), x)
            }
            _ => NOT_ALLOWED,
        };
        self.store.remember(&mut self.open, (p, name.key), r)
    }

    fn apply_after(&mut self, then: Then, p: Id) -> Id {
        if let Some(&r) = self.then.get(&(then, p)) {
            return r;
        }
        if !self.spend() {
            return NOT_ALLOWED;
        }
        let r = match self.store.pat(p) {
            Pat::After(a, b) => {
                let b = match then {
                    Then::Group(x) => self.store.group(b, x),
                    Then::InterleaveLeft(x) => self.store.interleave(b, x),
                    Then::InterleaveRight(x) => self.store.interleave(x, b),
                    Then::After(x) => self.store.after(b, x),
                };
                self.store.after(a, b)
            }
            Pat::Choice(a, b) => {
                let (x, y) = (self.apply_after(then, a), self.apply_after(then, b));
                self.store.choice(x, y)
            }
            _ => NOT_ALLOWED,
        };
        self.store.remember(&mut self.then, (then, p), r)
    }

    pub(super) fn attribute(&mut self, p: Id, name: Name<'_>, value: &str, cx: &dyn Context) -> Id {
        self.attribute_memo(p, name, value, cx, &mut HashMap::new())
    }

    fn attribute_memo(
        &mut self,
        p: Id,
        name: Name<'_>,
        value: &str,
        cx: &dyn Context,
        memo: &mut HashMap<Id, Id>,
    ) -> Id {
        if let Some(&r) = memo.get(&p) {
            return r;
        }
        if !self.spend() {
            return NOT_ALLOWED;
        }
        let mut derive = |this: &mut Self, q| this.attribute_memo(q, name, value, cx, memo);
        let r = match self.store.pat(p) {
            Pat::After(a, b) => {
                let x = derive(self, a);
                self.store.after(x, b)
            }
            Pat::Choice(a, b) => {
                let (x, y) = (derive(self, a), derive(self, b));
                self.store.choice(x, y)
            }
            Pat::Group(a, b) => {
                let (x, y) = (derive(self, a), derive(self, b));
                let (x, y) = (self.store.group(x, b), self.store.group(a, y));
                self.store.choice(x, y)
            }
            Pat::Interleave(a, b) => {
                let (x, y) = (derive(self, a), derive(self, b));
                let (x, y) = (self.store.interleave(x, b), self.store.interleave(a, y));
                self.store.choice(x, y)
            }
            Pat::OneOrMore(a) => {
                let x = derive(self, a);
                let rest = self.store.choice(p, EMPTY);
                self.store.group(x, rest)
            }
            Pat::Attribute(class, content) => {
                let matched = self.tables.contains(class, name.ns, name.local)
                    && self.value_matches(content, value, cx);
                if matched { EMPTY } else { NOT_ALLOWED }
            }
            _ => NOT_ALLOWED,
        };
        self.store.remember(memo, p, r)
    }

    pub(super) fn value_matches(&mut self, p: Id, value: &str, cx: &dyn Context) -> bool {
        (self.store.nullable(p) && is_whitespace(value)) || {
            let d = self.text(p, value, cx);
            self.store.nullable(d)
        }
    }

    pub(super) fn text(&mut self, p: Id, text: &str, cx: &dyn Context) -> Id {
        self.text_memo(p, text, cx, &mut HashMap::new())
    }

    fn text_memo(&mut self, p: Id, text: &str, cx: &dyn Context, memo: &mut HashMap<Id, Id>) -> Id {
        if let Some(&r) = memo.get(&p) {
            return r;
        }
        if !self.spend() {
            return NOT_ALLOWED;
        }
        let mut derive = |this: &mut Self, q| this.text_memo(q, text, cx, memo);
        let r = match self.store.pat(p) {
            Pat::Choice(a, b) => {
                let (x, y) = (derive(self, a), derive(self, b));
                self.store.choice(x, y)
            }
            Pat::Interleave(a, b) => {
                let (x, y) = (derive(self, a), derive(self, b));
                let (x, y) = (self.store.interleave(x, b), self.store.interleave(a, y));
                self.store.choice(x, y)
            }
            Pat::Group(a, b) => {
                let x = derive(self, a);
                let x = self.store.group(x, b);
                match self.store.nullable(a) {
                    true => {
                        let y = derive(self, b);
                        self.store.choice(x, y)
                    }
                    false => x,
                }
            }
            Pat::After(a, b) => {
                let x = derive(self, a);
                self.store.after(x, b)
            }
            Pat::OneOrMore(a) => {
                let x = derive(self, a);
                let rest = self.store.choice(p, EMPTY);
                self.store.group(x, rest)
            }
            Pat::Text => TEXT,
            Pat::Value(v) => match self.tables.values[v as usize].matches(text, cx) {
                true => EMPTY,
                false => NOT_ALLOWED,
            },
            Pat::Data(d, except) => {
                let allowed = self.tables.datatypes[d as usize].allows(text, cx)
                    && (except == NOT_ALLOWED || {
                        let x = self.text(except, text, cx);
                        !self.store.nullable(x)
                    });
                if allowed { EMPTY } else { NOT_ALLOWED }
            }
            Pat::List(content) => {
                let mut q = content;
                for token in text
                    .split([' ', '\t', '\n', '\r'])
                    .filter(|t| !t.is_empty())
                {
                    q = self.text(q, token, cx);
                }
                if self.store.nullable(q) {
                    EMPTY
                } else {
                    NOT_ALLOWED
                }
            }
            _ => NOT_ALLOWED,
        };
        self.store.remember(memo, p, r)
    }

    /// `startTagCloseDeriv`; `relaxed` drops missing attributes for recovery.
    pub(super) fn start_tag_close(&mut self, p: Id, relaxed: bool) -> Id {
        if let Some(&r) = self.close.get(&(p, relaxed)) {
            return r;
        }
        if !self.spend() {
            return NOT_ALLOWED;
        }
        let r = match self.store.pat(p) {
            Pat::After(a, b) => {
                let x = self.start_tag_close(a, relaxed);
                self.store.after(x, b)
            }
            Pat::Choice(a, b) => {
                let (x, y) = (
                    self.start_tag_close(a, relaxed),
                    self.start_tag_close(b, relaxed),
                );
                self.store.choice(x, y)
            }
            Pat::Group(a, b) => {
                let (x, y) = (
                    self.start_tag_close(a, relaxed),
                    self.start_tag_close(b, relaxed),
                );
                self.store.group(x, y)
            }
            Pat::Interleave(a, b) => {
                let (x, y) = (
                    self.start_tag_close(a, relaxed),
                    self.start_tag_close(b, relaxed),
                );
                self.store.interleave(x, y)
            }
            Pat::OneOrMore(a) => {
                let x = self.start_tag_close(a, relaxed);
                self.store.one_or_more(x)
            }
            Pat::Attribute(..) if relaxed => EMPTY,
            Pat::Attribute(..) => NOT_ALLOWED,
            _ => p,
        };
        self.store.remember(&mut self.close, (p, relaxed), r)
    }

    /// `endTagDeriv`; `relaxed` ends the element even if content is missing.
    pub(super) fn end_tag(&mut self, p: Id, relaxed: bool) -> Id {
        if let Some(&r) = self.end.get(&(p, relaxed)) {
            return r;
        }
        if !self.spend() {
            return NOT_ALLOWED;
        }
        let r = match self.store.pat(p) {
            Pat::Choice(a, b) => {
                let (x, y) = (self.end_tag(a, relaxed), self.end_tag(b, relaxed));
                self.store.choice(x, y)
            }
            Pat::After(a, b) if relaxed || self.store.nullable(a) => b,
            _ => NOT_ALLOWED,
        };
        self.store.remember(&mut self.end, (p, relaxed), r)
    }

    /// The leaf patterns that could match the next item of `p`, for
    /// diagnostics: elements, attributes, text and datatypes.
    pub(super) fn front(&self, p: Id) -> Vec<Pat> {
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![p];
        let mut out = Vec::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) || seen.len() > 10_000 {
                continue;
            }
            match self.store.pat(id) {
                Pat::Choice(a, b) | Pat::Interleave(a, b) => stack.extend([a, b]),
                Pat::Group(a, b) => {
                    stack.push(a);
                    if self.store.nullable(a) {
                        stack.push(b);
                    }
                }
                Pat::OneOrMore(a) | Pat::After(a, _) => stack.push(a),
                Pat::Empty | Pat::NotAllowed => {}
                leaf => out.push(leaf),
            }
        }
        out
    }
}
