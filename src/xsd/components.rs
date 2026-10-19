//! Schema components after construction: an arena of types, declarations
//! and wildcards addressed by index, with names interned to [`Sym`]s.

use std::collections::HashMap;

use crate::pattern::Pattern;
use crate::regexp::Regex;
use crate::schema_types::{Facet, Type, Value, WhiteSpace};

use super::model::Model;

/// An interned string; [`NO_NS`] is the empty string, the absent namespace.
pub(crate) type Sym = u32;
pub(crate) const NO_NS: Sym = 0;
/// A name the schema never mentions; it equals no declared name.
pub(crate) const UNKNOWN: Sym = u32::MAX;

pub(crate) type TypeId = usize;
pub(crate) type ElemId = usize;
pub(crate) type AttrId = usize;
pub(crate) type WildId = usize;
pub(crate) type IdcId = usize;
pub(crate) type GroupId = usize;

/// The built-in `xs:anyType`; built-in simple types follow it in
/// [`Type::all`] order.
pub(crate) const ANY_TYPE: TypeId = 0;

pub(crate) fn builtin(ty: Type) -> TypeId {
    ty as usize + 1
}

/// An expanded name.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct QName {
    pub(crate) ns: Sym,
    pub(crate) local: Sym,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Names {
    map: HashMap<Box<str>, Sym>,
    strings: Vec<Box<str>>,
}

impl Names {
    pub(crate) fn new() -> Self {
        let mut names = Self::default();
        names.intern("");
        names
    }

    pub(crate) fn intern(&mut self, s: &str) -> Sym {
        if let Some(&sym) = self.map.get(s) {
            return sym;
        }
        let sym = self.strings.len() as Sym;
        self.strings.push(s.into());
        self.map.insert(s.into(), sym);
        sym
    }

    /// The symbol of an instance string, [`UNKNOWN`] if never interned.
    pub(crate) fn get(&self, s: &str) -> Sym {
        self.map.get(s).copied().unwrap_or(UNKNOWN)
    }

    pub(crate) fn str(&self, sym: Sym) -> &str {
        self.strings.get(sym as usize).map_or("", |s| s)
    }

    /// `{ns}local`, or `local` without a namespace, as libxml2 prints names.
    pub(crate) fn format(&self, name: QName) -> String {
        match name.ns {
            NO_NS => self.str(name.local).to_owned(),
            ns => format!("{{{}}}{}", self.str(ns), self.str(name.local)),
        }
    }
}

/// Derivation methods, as `final`, `block` and blocking sets.
pub(crate) type Set = u8;
pub(crate) const EXTENSION: Set = 1;
pub(crate) const RESTRICTION: Set = 2;
pub(crate) const SUBSTITUTION: Set = 4;
pub(crate) const LIST: Set = 8;
pub(crate) const UNION: Set = 16;

/// A computed simple value: atomic, or the items of a list.
#[derive(Clone, Debug)]
pub(crate) enum Typed {
    Atomic(Value),
    List(Vec<Value>),
}

impl Typed {
    /// `xmlSchemaAreValuesEqual`.
    pub(crate) fn equals(&self, other: &Typed) -> bool {
        match (self, other) {
            (Typed::Atomic(x), Typed::Atomic(y)) => x.equals(y),
            (Typed::List(x), Typed::List(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(x, y)| x.equals(y))
            }
            (Typed::Atomic(x), Typed::List(y)) | (Typed::List(y), Typed::Atomic(x)) => {
                y.len() == 1 && x.equals(&y[0])
            }
        }
    }

    pub(crate) fn hash_key(&self) -> u64 {
        match self {
            Typed::Atomic(value) => value.hash_key(),
            Typed::List(items) => items.first().map_or(0, Value::hash_key),
        }
    }
}

/// A default or fixed value: its lexical form and, once checked, its value.
#[derive(Clone, Debug)]
pub(crate) struct ValueConstraint {
    pub(crate) fixed: bool,
    pub(crate) lexical: Box<str>,
    pub(crate) value: Option<Typed>,
}

#[derive(Clone, Debug)]
pub(crate) enum Variety {
    Atomic,
    List(TypeId),
    Union(Vec<TypeId>),
}

#[derive(Clone, Debug)]
pub(crate) struct SimpleType {
    /// The nearest built-in ancestor that parses atomic values.
    pub(crate) builtin: Type,
    pub(crate) variety: Variety,
    pub(crate) whitespace: WhiteSpace,
    /// Effective length, bound and digit facets (libxml2's `facetSet`).
    pub(crate) facets: Vec<Facet>,
    /// This derivation step's patterns; any one must match.
    pub(crate) patterns: Vec<Regex>,
    /// This derivation step's enumeration, if it has one.
    pub(crate) enumeration: Option<Vec<Typed>>,
}

#[derive(Clone, Debug)]
pub(crate) enum Content {
    Empty,
    Simple(TypeId),
    Elements { mixed: bool, model: Model },
}

#[derive(Clone, Debug)]
pub(crate) struct ComplexType {
    pub(crate) content: Content,
    pub(crate) attributes: Vec<AttrUse>,
    pub(crate) wildcard: Option<WildId>,
}

#[derive(Clone, Debug)]
pub(crate) enum Kind {
    Simple(SimpleType),
    Complex(ComplexType),
}

#[derive(Clone, Debug)]
pub(crate) struct TypeDef {
    pub(crate) name: Option<QName>,
    pub(crate) base: TypeId,
    /// `EXTENSION` or `RESTRICTION` (list and union types restrict
    /// `anySimpleType` in the derivation sense).
    pub(crate) derivation: Set,
    pub(crate) final_: Set,
    pub(crate) block: Set,
    pub(crate) abstract_: bool,
    pub(crate) kind: Kind,
}

impl TypeDef {
    pub(crate) fn simple(&self) -> Option<&SimpleType> {
        match &self.kind {
            Kind::Simple(simple) => Some(simple),
            Kind::Complex(_) => None,
        }
    }

    pub(crate) fn complex(&self) -> Option<&ComplexType> {
        match &self.kind {
            Kind::Complex(complex) => Some(complex),
            Kind::Simple(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ElementDecl {
    pub(crate) name: QName,
    pub(crate) ty: TypeId,
    pub(crate) nillable: bool,
    pub(crate) abstract_: bool,
    pub(crate) value: Option<ValueConstraint>,
    pub(crate) block: Set,
    pub(crate) final_: Set,
    pub(crate) head: Option<ElemId>,
    /// Substitution group members, transitively, that may replace this one.
    pub(crate) members: Vec<ElemId>,
    pub(crate) idcs: Vec<IdcId>,
}

#[derive(Clone, Debug)]
pub(crate) struct AttributeDecl {
    pub(crate) name: QName,
    pub(crate) ty: TypeId,
    pub(crate) value: Option<ValueConstraint>,
}

#[derive(Clone, Debug)]
pub(crate) struct AttrUse {
    pub(crate) decl: AttrId,
    pub(crate) required: bool,
    /// The use's own value constraint, else the declaration's.
    pub(crate) value: Option<ValueConstraint>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NsConstraint {
    Any,
    /// Any namespace but this one and the absent namespace (`##other`).
    Not(Sym),
    Set(Vec<Sym>),
}

impl NsConstraint {
    /// `xmlSchemaCheckCVCWildcardNamespace`.
    pub(crate) fn allows(&self, ns: Sym) -> bool {
        match self {
            NsConstraint::Any => true,
            NsConstraint::Not(not) => ns != NO_NS && ns != *not,
            NsConstraint::Set(set) => set.contains(&ns),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Process {
    Skip,
    Lax,
    Strict,
}

#[derive(Clone, Debug)]
pub(crate) struct Wildcard {
    pub(crate) ns: NsConstraint,
    pub(crate) process: Process,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IdcKind {
    Unique,
    Key,
    KeyRef,
}

#[derive(Clone, Debug)]
pub(crate) struct Idc {
    pub(crate) kind: IdcKind,
    pub(crate) name: QName,
    pub(crate) selector: Pattern,
    pub(crate) fields: Vec<(Box<str>, Pattern)>,
    /// The key or unique a keyref refers to.
    pub(crate) refer: Option<IdcId>,
}
