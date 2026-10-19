//! Drafts to a [`Schema`], as libxml2's `xmlSchemaFixupComponents`: resolve
//! references, reject circular definitions, derive simple types and their
//! facets, build attribute uses and wildcards, compute complex content,
//! check the component constraints libxml2 checks, and compile content
//! models (rejecting those that violate Unique Particle Attribution).
//! Phases run in libxml2's order and stop at the first phase with errors.

use std::collections::{HashMap, HashSet};

use crate::schema_types::{Comparison, Facet, Type, WhiteSpace};

use super::automaton;
use super::components::*;
use super::model::{AllGroup, Graph, Kind as NodeKind, Leaf, Model, Node, TooComplex};
use super::parse::{
    AttrItem, Compositor, Drafts, GlobalKind, Loc, Particle, RawFacet, Ref, Shape, Term,
};
use super::simple::{self, Scope};
use super::{Code, Error, Limits, Schema};

use super::parse::Table;

const XS: &str = super::parse::XS;

/// Which of the nodes `0..n` lie on a cycle of `edges`: Tarjan's strongly
/// connected components, iteratively, in linear time.
fn on_cycle(n: usize, edges: impl Fn(usize) -> Vec<usize>) -> Vec<bool> {
    let (mut index, mut low) = (vec![usize::MAX; n], vec![0; n]);
    let (mut on_stack, mut cyclic) = (vec![false; n], vec![false; n]);
    let (mut stack, mut next) = (Vec::new(), 0);
    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        let mut calls = vec![(root, edges(root), 0)];
        (index[root], low[root], next) = (next, next, next + 1);
        stack.push(root);
        on_stack[root] = true;
        while let Some((v, succ, i)) = calls.last_mut() {
            let v = *v;
            if let Some(&w) = succ.get(*i) {
                *i += 1;
                cyclic[v] |= w == v;
                if index[w] == usize::MAX {
                    (index[w], low[w], next) = (next, next, next + 1);
                    stack.push(w);
                    on_stack[w] = true;
                    calls.push((w, edges(w), 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            calls.pop();
            if let Some((parent, ..)) = calls.last() {
                low[*parent] = low[*parent].min(low[v]);
            }
            if low[v] == index[v] {
                let at = stack.iter().rposition(|&w| w == v).expect("on stack");
                let component = stack.split_off(at);
                for &w in &component {
                    on_stack[w] = false;
                    cyclic[w] |= component.len() > 1;
                }
            }
        }
    }
    cyclic
}

/// The built-in types: `anyType`, then every [`Type`].
fn builtins(names: &mut Names) -> Vec<TypeDef> {
    let xs = names.intern(XS);
    let mut types = vec![TypeDef {
        name: Some(QName {
            ns: xs,
            local: names.intern("anyType"),
        }),
        base: ANY_TYPE,
        derivation: RESTRICTION,
        final_: 0,
        block: 0,
        abstract_: false,
        kind: Kind::Complex(ComplexType {
            content: Content::Empty,
            attributes: Vec::new(),
            wildcard: None,
        }),
    }];
    for ty in Type::all() {
        let variety = match ty.item() {
            Some(item) => Variety::List(builtin(item)),
            None => Variety::Atomic,
        };
        types.push(TypeDef {
            name: Some(QName {
                ns: xs,
                local: names.intern(ty.name()),
            }),
            base: ty.base().map_or(ANY_TYPE, builtin),
            derivation: RESTRICTION,
            final_: 0,
            block: 0,
            abstract_: false,
            kind: Kind::Simple(SimpleType {
                builtin: ty,
                variety,
                whitespace: ty.whitespace(),
                facets: Vec::new(),
                patterns: Vec::new(),
                enumeration: None,
            }),
        });
    }
    types
}

pub(crate) fn is_builtin(ty: TypeId) -> bool {
    ty < Drafts::FIRST_TYPE
}

/// `xmlSchemaCheckCOSSTDerivedOK`: `ty` derives from `base` by
/// restriction steps not blocked by `set`.
pub(crate) fn st_derived(types: &[TypeDef], mut ty: TypeId, base: TypeId, set: Set) -> bool {
    loop {
        if ty == base {
            return true;
        }
        let tb = types[ty].base;
        if set & RESTRICTION != 0 || types[tb].final_ & RESTRICTION != 0 {
            return false;
        }
        if tb == base {
            return true;
        }
        let variety = types[ty].simple().map(|s| &s.variety);
        if base == builtin(Type::AnySimpleType)
            && matches!(variety, Some(Variety::List(_) | Variety::Union(_)))
        {
            return true;
        }
        if let Some(Variety::Union(members)) = types[base].simple().map(|s| &s.variety)
            && !is_builtin(base)
            && members
                .iter()
                .any(|&member| member != base && st_derived(types, ty, member, set))
        {
            return true;
        }
        if tb == ANY_TYPE || tb == ty {
            return false;
        }
        ty = tb;
    }
}

/// `xmlSchemaCheckCOSCTDerivedOK` and the simple case.
pub(crate) fn derived(types: &[TypeDef], mut ty: TypeId, base: TypeId, set: Set) -> bool {
    loop {
        if types[ty].simple().is_some() {
            return st_derived(types, ty, base, set);
        }
        if ty == base {
            return true;
        }
        if types[ty].derivation & set != 0 {
            return false;
        }
        let tb = types[ty].base;
        if tb == base {
            return true;
        }
        if tb == ANY_TYPE || tb == ty {
            return false;
        }
        if types[tb].simple().is_some() {
            return st_derived(types, tb, base, set);
        }
        ty = tb;
    }
}

/// Whether `ty` is or derives from built-in `target` (`xmlSchemaIsDerivedFromBuiltInType`).
pub(crate) fn derives_from_builtin(types: &[TypeDef], ty: TypeId, target: Type) -> bool {
    let mut t = ty;
    for _ in 0..types.len() {
        if t == builtin(target) {
            return true;
        }
        if t == ANY_TYPE || t == builtin(Type::AnySimpleType) || types[t].simple().is_none() {
            return false;
        }
        t = types[t].base;
    }
    false
}

/// `xmlSchemaUnionWildcards` on namespace constraints.
fn ns_union(a: &NsConstraint, b: &NsConstraint) -> Result<NsConstraint, ()> {
    use NsConstraint::{Any, Not, Set};
    Ok(match (a, b) {
        (a, b) if same(a, b) => a.clone(),
        (Any, _) | (_, Any) => Any,
        (Set(x), Set(y)) => Set(x.iter().chain(y).fold(Vec::new(), |mut set, &ns| {
            if !set.contains(&ns) {
                set.push(ns);
            }
            set
        })),
        (Not(_), Not(_)) => Not(NO_NS),
        (Not(x), Set(s)) | (Set(s), Not(x)) => {
            let absent = s.contains(&NO_NS);
            match (*x, s.contains(x)) {
                (NO_NS, _) if absent => Any,
                (NO_NS, _) => Not(NO_NS),
                (_, true) if absent => Any,
                (_, true) => Not(NO_NS),
                (_, false) if absent => return Err(()),
                (x, false) => Not(x),
            }
        }
    })
}

fn same(a: &NsConstraint, b: &NsConstraint) -> bool {
    match (a, b) {
        (NsConstraint::Set(x), NsConstraint::Set(y)) => {
            x.iter().all(|ns| y.contains(ns)) && y.iter().all(|ns| x.contains(ns))
        }
        _ => a == b,
    }
}

/// `xmlSchemaIntersectWildcards`.
fn ns_intersect(a: &NsConstraint, b: &NsConstraint) -> Result<NsConstraint, ()> {
    use NsConstraint::{Any, Not, Set};
    Ok(match (a, b) {
        (a, b) if same(a, b) => a.clone(),
        (Any, other) | (other, Any) => other.clone(),
        (Not(x), Set(s)) | (Set(s), Not(x)) => Set(s
            .iter()
            .copied()
            .filter(|&ns| ns != NO_NS && ns != *x)
            .collect()),
        (Set(x), Set(y)) => Set(x.iter().copied().filter(|ns| y.contains(ns)).collect()),
        (Not(x), Not(y)) if *x != NO_NS && *y != NO_NS => return Err(()),
        (Not(x), Not(y)) => Not(if *x == NO_NS { *y } else { *x }),
    })
}

/// `xmlSchemaCheckCOSNSSubset`.
fn ns_subset(sub: &NsConstraint, sup: &NsConstraint) -> bool {
    use NsConstraint::{Any, Not, Set};
    match (sub, sup) {
        (_, Any) => true,
        (Not(x), Not(y)) => x == y,
        (Set(s), Set(t)) => s.iter().all(|ns| t.contains(ns)),
        (Set(s), Not(x)) => !s.contains(x),
        _ => false,
    }
}

struct Fix {
    d: Drafts,
    limits: Limits,
    table: Table,
    /// Per redefinition, the component it replaces.
    redefined: Vec<Option<usize>>,
    types: Vec<TypeDef>,
    fixed: Vec<bool>,
    invalid: Vec<bool>,
    bases: Vec<Option<TypeId>>,
    items: HashMap<TypeId, TypeId>,
    members: HashMap<TypeId, Vec<TypeId>>,
    elem_types: Vec<TypeId>,
    elem_heads: Vec<Option<ElemId>>,
    attr_types: Vec<TypeId>,
    attr_values: Vec<Option<ValueConstraint>>,
    /// Expanded attribute groups: uses and complete wildcard.
    group_attrs: Vec<Option<(Vec<AttrUse>, Option<WildId>)>>,
    wildcards: Vec<Wildcard>,
    refers: Vec<Option<IdcId>>,
    notations: HashSet<QName>,
    /// Each derivation step's own facets and whether they are fixed.
    own_facets: HashMap<TypeId, Vec<(&'static str, bool)>>,
    /// Effective content particles of complex types with element content.
    particles: HashMap<TypeId, Particle>,
    elem_values: Vec<Option<ValueConstraint>>,
    members_of: Vec<Vec<ElemId>>,
    model_work: usize,
}

/// Builds the schema, or fails with every diagnostic.
pub(crate) fn build(drafts: Drafts, limits: &Limits) -> Result<Schema, Error> {
    let mut drafts = drafts;
    if drafts.docs.is_empty() || drafts.errors() {
        if drafts.docs.is_empty() && !drafts.errors() {
            drafts.report(
                None,
                Code::FailedLoad,
                "Failed to locate the main schema resource.".into(),
            );
        }
        return Err(Error {
            diagnostics: drafts.diagnostics,
        });
    }
    let mut names = std::mem::take(&mut drafts.names);
    let mut types = builtins(&mut names);
    drafts.names = names;
    let count = drafts.types.len();
    types.extend(drafts.types.iter().map(|draft| TypeDef {
        name: draft.name,
        base: ANY_TYPE,
        derivation: draft.derivation,
        final_: draft.final_,
        block: draft.block,
        abstract_: draft.abstract_,
        kind: Kind::Complex(ComplexType {
            content: Content::Empty,
            attributes: Vec::new(),
            wildcard: None,
        }),
    }));
    let all = types.len();
    let mut fix = Fix {
        wildcards: drafts.wildcards.iter().map(|(w, _)| w.clone()).collect(),
        elem_types: vec![ANY_TYPE; drafts.elements.len()],
        elem_heads: vec![None; drafts.elements.len()],
        attr_types: vec![builtin(Type::AnySimpleType); drafts.attributes.len()],
        attr_values: vec![None; drafts.attributes.len()],
        group_attrs: vec![None; drafts.attr_groups.len()],
        refers: vec![None; drafts.idcs.len()],
        redefined: vec![None; drafts.redefines.len()],
        d: drafts,
        limits: *limits,
        table: Table::new(),
        types,
        fixed: (0..all).map(is_builtin).collect(),
        invalid: vec![false; all],
        bases: vec![None; all],
        items: HashMap::new(),
        members: HashMap::new(),
        notations: HashSet::new(),
        own_facets: HashMap::new(),
        particles: HashMap::new(),
        elem_values: Vec::new(),
        members_of: Vec::new(),
        model_work: limits.max_model_work,
    };
    fix.elem_values = vec![None; fix.d.elements.len()];
    let any = fix.new_wildcard(Wildcard {
        ns: NsConstraint::Any,
        process: Process::Lax,
    });
    let loc = Loc {
        doc: 0,
        node: fix.d.docs[0].1.root().id(),
    };
    let any_particle = Particle {
        min: 0,
        max: None,
        term: Term::Wildcard(any),
        loc,
    };
    fix.particles.insert(
        ANY_TYPE,
        Particle {
            min: 1,
            max: Some(1),
            term: Term::Group(Compositor::Sequence, vec![any_particle]),
            loc,
        },
    );
    fix.types[ANY_TYPE].kind = Kind::Complex(ComplexType {
        content: Content::Elements {
            mixed: true,
            model: placeholder_model(),
        },
        attributes: Vec::new(),
        wildcard: Some(any),
    });
    fix.members_of = vec![Vec::new(); fix.d.elements.len()];
    let _ = count;
    let phases: [fn(&mut Fix); 10] = [
        Fix::redefine_first,
        Fix::resolve,
        Fix::circularity,
        Fix::attribute_groups,
        Fix::simple_types,
        Fix::attribute_values,
        Fix::redefine_second,
        Fix::complex_types,
        Fix::element_decls,
        Fix::content_models,
    ];
    for phase in phases {
        phase(&mut fix);
        if fix.d.errors() {
            return Err(Error {
                diagnostics: fix.d.diagnostics,
            });
        }
    }
    Ok(fix.finish())
}

impl Fix {
    fn report(&mut self, loc: Loc, code: Code, message: String) {
        self.d.report(Some(loc), code, message);
    }

    fn format(&self, name: QName) -> String {
        self.d.names.format(name)
    }

    fn type_name(&self, ty: TypeId) -> String {
        match self.types[ty].name {
            Some(name) => format!("'{}'", self.format(name)),
            None => "local type".into(),
        }
    }

    fn lookup(&self, kind: GlobalKind, name: QName) -> Option<usize> {
        if kind == GlobalKind::Type && self.d.names.str(name.ns) == XS {
            let local = self.d.names.str(name.local);
            if local == "anyType" {
                return Some(ANY_TYPE);
            }
            if let Some(ty) = Type::from_name(local) {
                return Some(builtin(ty));
            }
        }
        self.table.get(&(kind, name)).copied()
    }

    fn unresolved(&mut self, loc: Loc, attr: &str, name: QName, what: &str) {
        let message = format!(
            "The QName value '{}' of the attribute '{attr}' does not resolve to a(n) {what}.",
            self.format(name)
        );
        self.report(loc, Code::SrcResolve, message);
    }

    // ---- redefinitions and tables ----------------------------------------

    /// `xmlSchemaFindRedefCompInGraph`: the component in `bucket` or below.
    fn find_in_graph(&self, bucket: usize, kind: GlobalKind, name: QName) -> Option<usize> {
        let mut seen = HashSet::new();
        let mut stack = vec![bucket];
        while let Some(b) = stack.pop() {
            if !seen.insert(b) {
                continue;
            }
            let bucket = &self.d.buckets[b];
            if let Some(global) = bucket
                .globals
                .iter()
                .find(|g| g.kind == kind && g.name == name)
            {
                return Some(global.id);
            }
            stack.extend(bucket.children.iter().rev());
        }
        None
    }

    /// `xmlSchemaCheckSRCRedefineFirst` and `xmlSchemaAddComponents`.
    fn redefine_first(&mut self) {
        let mut excluded = HashSet::new();
        for r in 0..self.d.redefines.len() {
            let redefine = self.d.redefines[r].clone();
            let Some(prev) = self.find_in_graph(redefine.bucket, redefine.kind, redefine.name)
            else {
                let message = format!(
                    "The {} '{}' to be redefined could not be found in the redefined schema.",
                    match redefine.kind {
                        GlobalKind::Type => "type definition",
                        GlobalKind::Group => "model group definition",
                        _ => "attribute group definition",
                    },
                    self.format(redefine.name)
                );
                self.report(redefine.loc, Code::SrcRedefine, message);
                continue;
            };
            if !excluded.insert((redefine.kind, prev)) {
                let message = "The referenced component was already redefined. Multiple redefinition of the same component is not supported.";
                self.report(redefine.loc, Code::SrcRedefine, message.into());
                continue;
            }
            self.redefined[r] = Some(prev);
            if redefine.kind == GlobalKind::Type {
                self.bases[redefine.id] = Some(prev);
            }
        }
        let globals: Vec<_> = self
            .d
            .buckets
            .iter()
            .flat_map(|b| b.globals.iter().copied())
            .collect();
        let mut added = HashSet::new();
        for global in globals {
            if excluded.contains(&(global.kind, global.id))
                || !added.insert((global.kind, global.id, global.loc.doc))
            {
                continue;
            }
            if global.kind == GlobalKind::Notation {
                if !self.notations.insert(global.name) {
                    self.duplicate(global.loc, "notation", global.name);
                }
                continue;
            }
            if self
                .table
                .insert((global.kind, global.name), global.id)
                .is_some()
            {
                let what = match global.kind {
                    GlobalKind::Type => "type definition",
                    GlobalKind::Element => "element declaration",
                    GlobalKind::Attribute => "attribute declaration",
                    GlobalKind::Group => "model group definition",
                    GlobalKind::AttrGroup => "attribute group definition",
                    GlobalKind::Idc => "identity-constraint definition",
                    GlobalKind::Notation => "notation",
                };
                self.duplicate(global.loc, what, global.name);
            }
        }
    }

    fn duplicate(&mut self, loc: Loc, what: &str, name: QName) {
        let message = format!(
            "A global {what} '{}' does already exist.",
            self.format(name)
        );
        self.report(loc, Code::RedefinedType, message);
    }

    // ---- reference resolution --------------------------------------------

    fn resolve(&mut self) {
        for e in 0..self.d.elements.len() {
            let draft = self.d.elements[e].clone();
            if let Some(Ref::Named(name)) = draft.ty {
                match self.lookup(GlobalKind::Type, name) {
                    Some(ty) => self.elem_types[e] = ty,
                    None => self.unresolved(draft.loc, "type", name, "type definition"),
                }
            } else if let Some(Ref::Id(ty)) = draft.ty {
                self.elem_types[e] = ty;
            }
            if let Some(head) = draft.head {
                match self.lookup(GlobalKind::Element, head) {
                    Some(id) => self.elem_heads[e] = Some(id),
                    None => {
                        self.unresolved(draft.loc, "substitutionGroup", head, "element declaration")
                    }
                }
            }
        }
        for index in 0..self.d.types.len() {
            self.resolve_type(index);
        }
        for a in 0..self.d.attributes.len() {
            let draft = self.d.attributes[a].clone();
            match draft.ty {
                Some(Ref::Named(name)) => match self.lookup(GlobalKind::Type, name) {
                    Some(ty) if self.is_simple(ty) => self.attr_types[a] = ty,
                    _ => self.unresolved(draft.loc, "type", name, "simple type definition"),
                },
                Some(Ref::Id(ty)) => self.attr_types[a] = ty,
                None => {}
            }
        }
        let mut items: Vec<Vec<AttrItem>> = self
            .d
            .types
            .iter_mut()
            .map(|t| std::mem::take(&mut t.attrs))
            .collect();
        for list in &mut items {
            self.resolve_attr_items(list);
        }
        for (draft, list) in self.d.types.iter_mut().zip(items) {
            draft.attrs = list;
        }
        for g in 0..self.d.attr_groups.len() {
            let mut list = std::mem::take(&mut self.d.attr_groups[g].attrs);
            self.resolve_attr_items(&mut list);
            self.d.attr_groups[g].attrs = list;
        }
        for t in 0..self.d.types.len() {
            if let Some(mut particle) = self.d.types[t].particle.take() {
                let top = self.resolve_top_particle(&mut particle);
                if top {
                    self.resolve_particles(&mut particle);
                    self.d.types[t].particle = Some(particle);
                }
            }
        }
        for g in 0..self.d.groups.len() {
            // Cloned, not taken: a self-reference must still see the model.
            if let Some((compositor, mut particles, loc)) = self.d.groups[g].model.clone() {
                for particle in &mut particles {
                    self.resolve_particle(particle);
                }
                self.d.groups[g].model = Some((compositor, particles, loc));
            }
        }
        for i in 0..self.d.idcs.len() {
            let draft = self.d.idcs[i].clone();
            let Some(refer) = draft.refer else { continue };
            match self.lookup(GlobalKind::Idc, refer) {
                None => {
                    self.unresolved(draft.loc, "refer", refer, "identity constraint definition")
                }
                Some(target) if self.d.idcs[target].kind == IdcKind::KeyRef => {
                    self.report(
                        draft.loc,
                        Code::CPropsCorrect,
                        "The keyref references a keyref.".into(),
                    );
                }
                Some(target) if self.d.idcs[target].fields.len() != draft.fields.len() => {
                    let message = format!(
                        "The cardinality of the keyref differs from the cardinality of the referenced key/unique '{}'.",
                        self.format(refer)
                    );
                    self.report(draft.loc, Code::CPropsCorrect, message);
                }
                Some(target) => self.refers[i] = Some(target),
            }
        }
    }

    fn is_simple(&self, ty: TypeId) -> bool {
        match is_builtin(ty) {
            true => ty != ANY_TYPE,
            false => !matches!(
                self.d.types[ty - Drafts::FIRST_TYPE].shape,
                Shape::Complex { .. }
            ),
        }
    }

    fn resolve_ref(
        &mut self,
        r: Ref<TypeId>,
        loc: Loc,
        attr: &str,
        simple_only: bool,
    ) -> Option<TypeId> {
        match r {
            Ref::Id(ty) => Some(ty),
            Ref::Named(name) => match self.lookup(GlobalKind::Type, name) {
                Some(ty) if !simple_only || self.is_simple(ty) => Some(ty),
                _ => {
                    let what = if simple_only {
                        "simple type definition"
                    } else {
                        "type definition"
                    };
                    self.unresolved(loc, attr, name, what);
                    None
                }
            },
        }
    }

    /// `xmlSchemaResolveTypeReferences`.
    fn resolve_type(&mut self, index: usize) {
        let id = Drafts::type_id(index);
        let draft = self.d.types[index].clone();
        if self.bases[id].is_none() {
            match draft.base {
                Some(r) => match self.resolve_ref(r, draft.loc, "base", false) {
                    Some(base) => self.bases[id] = Some(base),
                    None => return,
                },
                None => return,
            }
        }
        match draft.shape {
            Shape::List(Some(item)) => {
                if let Some(item) = self.resolve_ref(item, draft.loc, "itemType", true) {
                    self.items.insert(id, item);
                }
            }
            Shape::Union(members) => {
                let resolved: Vec<_> = members
                    .into_iter()
                    .filter_map(|m| self.resolve_ref(m, draft.loc, "memberTypes", true))
                    .collect();
                self.members.insert(id, resolved);
            }
            _ => {}
        }
    }

    fn resolve_attr_items(&mut self, list: &mut [AttrItem]) {
        for item in list.iter_mut() {
            match item {
                AttrItem::Use(use_) => {
                    if let Ref::Named(name) = use_.decl {
                        match self.lookup(GlobalKind::Attribute, name) {
                            Some(decl) => use_.decl = Ref::Id(decl),
                            None => self.unresolved(use_.loc, "ref", name, "attribute declaration"),
                        }
                    }
                }
                AttrItem::Group(name, loc)
                    if self.lookup(GlobalKind::AttrGroup, *name).is_none() =>
                {
                    let (name, loc) = (*name, *loc);
                    self.unresolved(loc, "ref", name, "attribute group definition");
                }
                AttrItem::Prohibit(name, loc, true)
                    if self.lookup(GlobalKind::Attribute, *name).is_none() =>
                {
                    let (name, loc) = (*name, *loc);
                    self.unresolved(loc, "ref", name, "attribute declaration");
                }
                _ => {}
            }
        }
    }

    /// The top particle of a complex type: a group reference resolving to
    /// nothing removes the particle; to an `all` group needs `maxOccurs` 1.
    fn resolve_top_particle(&mut self, particle: &mut Particle) -> bool {
        let group = match particle.term {
            Term::GroupRef(name) => match self.lookup(GlobalKind::Group, name) {
                Some(group) => group,
                None => {
                    self.unresolved(particle.loc, "ref", name, "model group definition");
                    return false;
                }
            },
            Term::RedefinedGroup(r) => match self.redefined[r] {
                Some(group) => group,
                None => return false,
            },
            _ => return true,
        };
        match &self.d.groups[group].model {
            None => return false,
            Some((Compositor::All, ..)) if particle.max != Some(1) => self.report(
                particle.loc,
                Code::CosAllLimited,
                "The particle's {max occurs} must be 1, since the reference resolves to an 'all' model group.".into(),
            ),
            _ => {}
        }
        particle.term = Term::GroupDef(group);
        true
    }

    /// `xmlSchemaResolveModelGroupParticleReferences`: the particles inside
    /// a model group, recursively.
    fn resolve_particles(&mut self, particle: &mut Particle) {
        if let Term::Group(_, children) = &mut particle.term {
            for child in children {
                self.resolve_particle(child);
            }
        }
    }

    fn resolve_particle(&mut self, particle: &mut Particle) {
        let resolved = match particle.term {
            Term::ElementRef(name) => match self.lookup(GlobalKind::Element, name) {
                Some(e) => Term::Element(e),
                None => {
                    self.unresolved(particle.loc, "ref", name, "element declaration");
                    Term::Missing
                }
            },
            Term::GroupRef(name) => match self.lookup(GlobalKind::Group, name) {
                Some(g) => self.group_term(g, particle.loc),
                None => {
                    self.unresolved(particle.loc, "ref", name, "model group definition");
                    Term::Missing
                }
            },
            Term::RedefinedGroup(r) => match self.redefined[r] {
                Some(g) => self.group_term(g, particle.loc),
                None => Term::Missing,
            },
            Term::Group(..) => return self.resolve_particles(particle),
            _ => return,
        };
        particle.term = resolved;
    }

    fn group_term(&mut self, group: GroupId, loc: Loc) -> Term {
        match &self.d.groups[group].model {
            None => Term::Missing,
            Some((Compositor::All, ..)) => {
                self.report(
                    loc,
                    Code::CosAllLimited,
                    "A model group definition is referenced, but it contains an 'all' model group, which cannot be contained by model groups.".into(),
                );
                Term::Missing
            }
            Some(_) => Term::GroupDef(group),
        }
    }

    // ---- circularity -----------------------------------------------------

    /// Reports the first circular type, model group or attribute group
    /// definition, as libxml2 stops at the first.
    fn circularity(&mut self) {
        let types = on_cycle(self.d.types.len(), |index| {
            let base = self.bases[Drafts::type_id(index)];
            base.filter(|&b| !is_builtin(b))
                .map(|b| b - Drafts::FIRST_TYPE)
                .into_iter()
                .collect()
        });
        if let Some(index) = types.iter().position(|&c| c) {
            let loc = self.d.types[index].loc;
            return self.report(
                loc,
                Code::StPropsCorrect2,
                "The definition is circular.".into(),
            );
        }
        let groups = on_cycle(self.d.groups.len(), |g| self.group_edges(g));
        if let Some(g) = groups.iter().position(|&c| c) {
            let message = format!(
                "Circular reference to the model group definition '{}' defined.",
                self.format(self.d.groups[g].name)
            );
            let loc = self.d.groups[g].loc;
            return self.report(loc, Code::MgPropsCorrect2, message);
        }
        let attr_groups = on_cycle(self.d.attr_groups.len(), |g| {
            let items = &self.d.attr_groups[g].attrs;
            items
                .iter()
                .filter_map(|item| self.attr_group_target(item))
                .collect()
        });
        if let Some(g) = attr_groups.iter().position(|&c| c) {
            let message = format!(
                "Circular reference to the attribute group '{}' defined.",
                self.format(self.d.attr_groups[g].name)
            );
            let loc = self.d.attr_groups[g].loc;
            self.report(loc, Code::SrcAttributeGroup3, message);
        }
    }

    fn shape(&self, ty: TypeId) -> Option<&Shape> {
        (!is_builtin(ty)).then(|| &self.d.types[ty - Drafts::FIRST_TYPE].shape)
    }

    /// The member types of the union `ty` derives from, if any.
    fn union_members(&self, ty: TypeId) -> Option<&[TypeId]> {
        let mut seen = HashSet::new();
        let mut t = ty;
        loop {
            match self.shape(t)? {
                Shape::Union(_) => return self.members.get(&t).map(Vec::as_slice),
                Shape::Restriction if seen.insert(t) => t = self.bases[t]?,
                _ => return None,
            }
        }
    }

    /// `xmlSchemaCheckUnionTypeDefCircularRecur`.
    fn union_reaches(
        &self,
        root: TypeId,
        members: &[TypeId],
        visited: &mut HashSet<TypeId>,
    ) -> bool {
        for &member in members {
            let mut t = Some(member);
            let mut chain = HashSet::new();
            while let Some(ty) = t {
                if ty == root {
                    return true;
                }
                if is_builtin(ty) || !chain.insert(ty) {
                    break;
                }
                if let Some(inner) = self.union_members(ty)
                    && visited.insert(ty)
                    && self.union_reaches(root, inner, visited)
                {
                    return true;
                }
                t = self.bases[ty];
            }
        }
        false
    }

    /// `xmlSchemaCheckUnionTypeDefCircular` for every union type.
    fn union_circularity(&mut self) {
        for id in Drafts::FIRST_TYPE..self.types.len() {
            if !matches!(self.shape(id), Some(Shape::Union(_))) {
                continue;
            }
            let members = self.members.get(&id).cloned().unwrap_or_default();
            if self.union_reaches(id, &members, &mut HashSet::new()) {
                let loc = self.d.types[id - Drafts::FIRST_TYPE].loc;
                self.report(
                    loc,
                    Code::SrcSimpleType4,
                    "The union type definition is circular.".into(),
                );
            }
        }
    }

    /// The model group definitions `g` refers to directly, through nested
    /// local groups.
    fn group_edges(&self, g: GroupId) -> Vec<GroupId> {
        let mut out = Vec::new();
        let mut stack: Vec<&Particle> = match &self.d.groups[g].model {
            Some((_, particles, _)) => particles.iter().collect(),
            None => Vec::new(),
        };
        while let Some(particle) = stack.pop() {
            match &particle.term {
                Term::GroupDef(h) => out.push(*h),
                Term::Group(_, children) => stack.extend(children),
                _ => {}
            }
        }
        out
    }

    fn attr_group_target(&self, item: &AttrItem) -> Option<usize> {
        match item {
            AttrItem::Group(name, _) => self.lookup(GlobalKind::AttrGroup, *name),
            AttrItem::RedefinedGroup(r) => self.redefined[*r],
            _ => None,
        }
    }

    // ---- attribute groups and wildcards ----------------------------------

    fn new_wildcard(&mut self, wildcard: Wildcard) -> WildId {
        self.wildcards.push(wildcard);
        self.wildcards.len() - 1
    }

    /// Intersects `next` into the complete wildcard, copying the first one
    /// before changing it.
    fn intersect(
        &mut self,
        complete: &mut Option<WildId>,
        created: &mut bool,
        next: WildId,
        loc: Loc,
    ) {
        let Some(current) = *complete else {
            *complete = Some(next);
            return;
        };
        let merged = ns_intersect(&self.wildcards[current].ns, &self.wildcards[next].ns);
        match merged {
            Ok(ns) => {
                if !*created {
                    let copy = self.wildcards[current].clone();
                    *complete = Some(self.new_wildcard(copy));
                    *created = true;
                }
                let id = complete.expect("set");
                self.wildcards[id].ns = ns;
            }
            Err(()) => {
                let message = "The intersection of the wildcard is not expressible.".into();
                self.report(loc, Code::IntersectionNotExpressible, message);
            }
        }
    }

    /// `xmlSchemaExpandAttributeGroupRefs`: uses, prohibitions and the
    /// complete wildcard of an attribute list.
    fn expand(
        &mut self,
        items: &[AttrItem],
        own: Option<WildId>,
        loc: Loc,
    ) -> (Vec<AttrUse>, Vec<(QName, Loc)>, Option<WildId>) {
        let mut uses = Vec::new();
        let mut prohibitions = Vec::new();
        let mut wildcard = own;
        let mut created = false;
        for item in items {
            match item {
                AttrItem::Use(use_) => {
                    let Ref::Id(decl) = use_.decl else { continue };
                    uses.push(AttrUse {
                        decl,
                        required: use_.required,
                        value: use_.value.clone().map(|(fixed, lexical)| ValueConstraint {
                            fixed,
                            lexical: lexical.into(),
                            value: None,
                        }),
                    });
                }
                AttrItem::Prohibit(name, loc, _) => prohibitions.push((*name, *loc)),
                AttrItem::Group(..) | AttrItem::RedefinedGroup(..) => {
                    let Some(group) = self.attr_group_target(item) else {
                        continue;
                    };
                    let (group_uses, group_wildcard) = self.group_attributes(group);
                    if let Some(next) = group_wildcard {
                        self.intersect(&mut wildcard, &mut created, next, loc);
                    }
                    uses.extend(group_uses);
                }
            }
        }
        (uses, prohibitions, wildcard)
    }

    fn group_attributes(&mut self, group: usize) -> (Vec<AttrUse>, Option<WildId>) {
        if let Some(done) = &self.group_attrs[group] {
            return done.clone();
        }
        self.group_attrs[group] = Some((Vec::new(), None));
        let draft = self.d.attr_groups[group].clone();
        let (uses, _, wildcard) = self.expand(&draft.attrs, draft.wildcard, draft.loc);
        self.group_attrs[group] = Some((uses.clone(), wildcard));
        (uses, wildcard)
    }

    fn attribute_groups(&mut self) {
        for g in 0..self.d.attr_groups.len() {
            self.group_attributes(g);
        }
    }

    // ---- simple types ----------------------------------------------------

    /// Types in an order where bases, item, member and content types come
    /// before the types depending on them (cycles are broken arbitrarily).
    fn type_order(&self) -> Vec<TypeId> {
        let deps = |id: TypeId| -> Vec<TypeId> {
            let mut deps: Vec<TypeId> = self.bases[id].into_iter().collect();
            deps.extend(self.items.get(&id));
            deps.extend(self.members.get(&id).into_iter().flatten());
            if let Some(content) = self.d.types[id - Drafts::FIRST_TYPE].content_type {
                deps.push(content);
            }
            deps.retain(|&d| !is_builtin(d));
            deps
        };
        let mut state = vec![0u8; self.types.len()];
        let mut order = Vec::new();
        for root in Drafts::FIRST_TYPE..self.types.len() {
            if state[root] != 0 {
                continue;
            }
            let mut stack = vec![(root, false)];
            while let Some((id, done)) = stack.pop() {
                if done {
                    state[id] = 2;
                    order.push(id);
                    continue;
                }
                if state[id] != 0 {
                    continue;
                }
                state[id] = 1;
                stack.push((id, true));
                for dep in deps(id) {
                    if state[dep] == 0 {
                        stack.push((dep, false));
                    }
                }
            }
        }
        order
    }

    fn variety_of(&self, ty: TypeId) -> Option<&Variety> {
        self.types[ty].simple().map(|s| &s.variety)
    }

    fn simple_types(&mut self) {
        self.union_circularity();
        if self.d.errors() {
            return;
        }
        for id in self.type_order() {
            if self.is_simple(id) {
                self.fix_simple(id);
            }
        }
    }

    fn scope_for(&self, loc: Loc) -> Vec<(String, String)> {
        self.d
            .scope(loc)
            .into_iter()
            .map(|(p, u)| (p.to_owned(), u.to_owned()))
            .collect()
    }

    /// Validates `lexical` against `ty` with the namespaces in scope at `loc`.
    fn value(&self, ty: TypeId, lexical: &str, loc: Loc) -> Result<Typed, simple::Failure> {
        let scope = self.scope_for(loc);
        let load = || {
            scope
                .iter()
                .map(|(p, u)| (p.as_str(), u.as_str()))
                .collect()
        };
        let no_entities = |_: &str| false;
        let cx = Scope::new(&load, &self.d.names, &self.notations, &no_entities);
        simple::validate(&self.types, &self.d.names, ty, lexical, &cx)
    }

    /// `xmlSchemaFixupSimpleTypeStageOne` and `StageTwo`.
    fn fix_simple(&mut self, id: TypeId) {
        if self.fixed[id] {
            return;
        }
        self.fixed[id] = true;
        let draft = self.d.types[id - Drafts::FIRST_TYPE].clone();
        let Some(base) = self.bases[id] else {
            self.invalid[id] = true;
            return;
        };
        let errors = self.d.diagnostics.len();
        let base_def = self.types[base].clone();
        let name = self.type_name(id);
        let loc = draft.loc;
        let any_simple = builtin(Type::AnySimpleType);
        // Variety and st-props-correct.
        let variety = match draft.shape {
            Shape::List(_) => match self.items.get(&id) {
                Some(&item) => Variety::List(item),
                None => {
                    self.invalid[id] = true;
                    return;
                }
            },
            Shape::Union(_) => {
                let mut flat = Vec::new();
                for &member in self.members.get(&id).into_iter().flatten() {
                    match self.variety_of(member) {
                        Some(Variety::Union(inner)) if !is_builtin(member) => {
                            flat.extend(inner.clone())
                        }
                        _ => flat.push(member),
                    }
                }
                if flat.contains(&id) {
                    self.report(
                        loc,
                        Code::SrcSimpleType4,
                        "The union type definition is circular.".into(),
                    );
                    return;
                }
                Variety::Union(flat)
            }
            _ => {
                if base_def.simple().is_none() {
                    let message = format!(
                        "The base type {} is not a simple type.",
                        self.type_name(base)
                    );
                    self.report(loc, Code::StPropsCorrect1, message);
                    self.invalid[id] = true;
                    return;
                }
                if base == any_simple {
                    self.report(loc, Code::StPropsCorrect1, "The variety is absent.".into());
                    self.invalid[id] = true;
                    return;
                }
                base_def.simple().expect("simple").variety.clone()
            }
        };
        if base_def.final_ & RESTRICTION != 0 {
            let message = format!(
                "The 'final' of its base type {} must not contain 'restriction'.",
                self.type_name(base)
            );
            self.report(loc, Code::StPropsCorrect3, message);
            return;
        }
        let builtin_type = match &variety {
            Variety::Atomic => base_def.simple().expect("simple").builtin,
            _ => Type::AnySimpleType,
        };
        // cos-st-restricts.
        let facet_kinds: Vec<&str> = draft.facets.iter().map(|f| f.kind).collect();
        let restricting = matches!(draft.shape, Shape::Restriction | Shape::Complex { .. });
        match &variety {
            Variety::Atomic => {
                let primitive = builtin_type.primitive();
                if let Some(bad) = draft
                    .facets
                    .iter()
                    .find(|f| !facet_allowed(primitive, f.kind))
                {
                    let message = format!(
                        "The facet '{}' is not allowed on types derived from the type 'xs:{}'.",
                        bad.kind,
                        primitive.name()
                    );
                    self.report(bad.loc, Code::CosStRestricts1_3_1, message);
                    return;
                }
            }
            Variety::List(item) => {
                let item = *item;
                if !restricting {
                    if !matches!(
                        self.variety_of(item),
                        Some(Variety::Atomic | Variety::Union(_))
                    ) || self.union_has_non_atomic(item)
                    {
                        let message = format!(
                            "The item type {} does not have a variety of atomic or union.",
                            self.type_name(item)
                        );
                        self.report(loc, Code::CosStRestricts2_1, message);
                        return;
                    }
                    if self.types[item].final_ & LIST != 0 {
                        let message = format!(
                            "The final of its item type {} must not contain 'list'.",
                            self.type_name(item)
                        );
                        self.report(loc, Code::CosStRestricts2_3_1_1, message);
                        return;
                    }
                } else if let Some(bad) = draft.facets.iter().find(|f| {
                    ![
                        "length",
                        "minLength",
                        "maxLength",
                        "whiteSpace",
                        "pattern",
                        "enumeration",
                    ]
                    .contains(&f.kind)
                }) {
                    let message = format!("The facet '{}' is not allowed.", bad.kind);
                    self.report(bad.loc, Code::CosStRestricts2_3_2_4, message);
                    return;
                }
            }
            Variety::Union(members) => {
                let members = members.clone();
                if let Some(&bad) = members.iter().find(|&&m| {
                    !matches!(self.variety_of(m), Some(Variety::Atomic | Variety::List(_)))
                }) {
                    let message = format!(
                        "The member type {} is neither an atomic, nor a list type.",
                        self.type_name(bad)
                    );
                    self.report(loc, Code::CosStRestricts3_1, message);
                    return;
                }
                if !restricting {
                    if let Some(&bad) = members.iter().find(|&&m| self.types[m].final_ & UNION != 0)
                    {
                        let message = format!(
                            "The 'final' of member type {} contains 'union'.",
                            self.type_name(bad)
                        );
                        self.report(loc, Code::CosStRestricts3_3_1, message);
                        return;
                    }
                } else if let Some(bad) = draft
                    .facets
                    .iter()
                    .find(|f| !["pattern", "enumeration"].contains(&f.kind))
                {
                    let message = format!("The facet '{}' is not allowed.", bad.kind);
                    self.report(bad.loc, Code::CosStRestricts3_3_2_4, message);
                    return;
                }
            }
        }
        let _ = facet_kinds;
        // Facet values (xmlSchemaCheckFacetValues) and derivation.
        let mut own: Vec<(Facet, bool, Loc)> = Vec::new();
        let mut patterns = Vec::new();
        let mut enumeration: Option<Vec<Typed>> = None;
        for facet in &draft.facets {
            match self.facet(base, facet) {
                Some(FacetValue::Pattern(regex)) => patterns.push(regex),
                Some(FacetValue::Enumeration(value)) => {
                    enumeration.get_or_insert_with(Vec::new).push(value)
                }
                Some(FacetValue::Other(value)) => own.push((value, facet.fixed, facet.loc)),
                None => {}
            }
        }
        if self.d.diagnostics.len() != errors {
            return;
        }
        let base_facets = base_def
            .simple()
            .map(|s| s.facets.clone())
            .unwrap_or_default();
        let base_fixed = self.fixed_facets(base);
        let facets = self.derive_facets(&own, &base_facets, &base_fixed, loc);
        let whitespace = match &variety {
            Variety::List(_) => WhiteSpace::Collapse,
            Variety::Union(_) => WhiteSpace::Preserve,
            Variety::Atomic => facets
                .iter()
                .find_map(|f| match f {
                    Facet::WhiteSpace(ws) => Some(*ws),
                    _ => None,
                })
                .unwrap_or(match builtin_type {
                    Type::String | Type::AnySimpleType => WhiteSpace::Preserve,
                    Type::NormalizedString => WhiteSpace::Replace,
                    _ => WhiteSpace::Collapse,
                }),
        };
        let _ = name;
        self.types[id] = TypeDef {
            name: draft.name,
            base,
            derivation: RESTRICTION,
            final_: draft.final_,
            block: 0,
            abstract_: false,
            kind: Kind::Simple(SimpleType {
                builtin: builtin_type,
                variety,
                whitespace,
                facets,
                patterns,
                enumeration,
            }),
        };
        self.own_facets.insert(
            id,
            own.iter()
                .map(|(f, fixed, _)| (facet_name(f), *fixed))
                .collect(),
        );
    }

    fn union_has_non_atomic(&self, ty: TypeId) -> bool {
        match self.variety_of(ty) {
            Some(Variety::Union(members)) => members
                .iter()
                .any(|&m| !matches!(self.variety_of(m), Some(Variety::Atomic))),
            _ => false,
        }
    }
}

enum FacetValue {
    Pattern(crate::regexp::Regex),
    Enumeration(Typed),
    Other(Facet),
}

fn facet_name(facet: &Facet) -> &'static str {
    match facet {
        Facet::Length(_) => "length",
        Facet::MinLength(_) => "minLength",
        Facet::MaxLength(_) => "maxLength",
        Facet::Pattern(_) => "pattern",
        Facet::Enumeration(_) => "enumeration",
        Facet::WhiteSpace(_) => "whiteSpace",
        Facet::MinInclusive(_) => "minInclusive",
        Facet::MaxInclusive(_) => "maxInclusive",
        Facet::MinExclusive(_) => "minExclusive",
        Facet::MaxExclusive(_) => "maxExclusive",
        Facet::TotalDigits(_) => "totalDigits",
        Facet::FractionDigits(_) => "fractionDigits",
    }
}

/// `xmlSchemaIsBuiltInTypeFacet`.
fn facet_allowed(primitive: Type, kind: &str) -> bool {
    use Type::*;
    let bounds = [
        "maxInclusive",
        "maxExclusive",
        "minInclusive",
        "minExclusive",
    ];
    let common = ["pattern", "enumeration", "whiteSpace"];
    match primitive {
        Boolean => ["pattern", "whiteSpace"].contains(&kind),
        String | Notation | QName | AnyUri | Base64Binary | HexBinary => {
            ["length", "minLength", "maxLength"].contains(&kind) || common.contains(&kind)
        }
        Decimal => {
            ["totalDigits", "fractionDigits"].contains(&kind)
                || common.contains(&kind)
                || bounds.contains(&kind)
        }
        Time | GDay | GMonth | GMonthDay | GYear | GYearMonth | Date | DateTime | Duration
        | Float | Double => common.contains(&kind) || bounds.contains(&kind),
        _ => false,
    }
}

fn compare_facets(a: &Facet, b: &Facet) -> Comparison {
    use Facet::*;
    let count = |f: &Facet| match f {
        Length(n) | MinLength(n) | MaxLength(n) | TotalDigits(n) | FractionDigits(n) => Some(*n),
        _ => None,
    };
    let value = |f: &Facet| match f {
        MinInclusive(v) | MaxInclusive(v) | MinExclusive(v) | MaxExclusive(v) => Some(v.clone()),
        _ => None,
    };
    match (count(a), count(b), value(a), value(b)) {
        (Some(x), Some(y), _, _) => x.cmp(&y).into(),
        (_, _, Some(x), Some(y)) => x.compare(&y),
        _ => Comparison::Incomparable,
    }
}

impl Fix {
    fn facet(&mut self, base: TypeId, raw: &RawFacet) -> Option<FacetValue> {
        let invalid = |fix: &mut Fix, message: String| {
            fix.report(raw.loc, Code::InvalidFacetValue, message);
            None
        };
        match raw.kind {
            "pattern" => match crate::regexp::Regex::new(&raw.value) {
                Ok(regex) => Some(FacetValue::Pattern(regex)),
                Err(_) => {
                    let message = format!(
                        "The value '{}' of the facet 'pattern' is not a valid regular expression.",
                        raw.value
                    );
                    self.report(raw.loc, Code::RegexpInvalid, message);
                    None
                }
            },
            "enumeration" | "minInclusive" | "maxInclusive" | "minExclusive" | "maxExclusive" => {
                match self.value(base, &raw.value, raw.loc) {
                    Ok(typed) => Some(match (raw.kind, typed) {
                        ("enumeration", typed) => FacetValue::Enumeration(typed),
                        (kind, Typed::Atomic(value)) => FacetValue::Other(match kind {
                            "minInclusive" => Facet::MinInclusive(value),
                            "maxInclusive" => Facet::MaxInclusive(value),
                            "minExclusive" => Facet::MinExclusive(value),
                            _ => Facet::MaxExclusive(value),
                        }),
                        _ => return None,
                    }),
                    Err(failure) => {
                        for (code, message) in failure {
                            self.report(raw.loc, code, message);
                        }
                        let message = format!(
                            "The value '{}' of the facet does not validate against the base type {}.",
                            raw.value,
                            self.type_name(base)
                        );
                        invalid(self, message)
                    }
                }
            }
            kind => match Facet::new(kind, &raw.value, Type::String, None) {
                Ok(facet) => Some(FacetValue::Other(facet)),
                Err(_) => {
                    let message = match kind {
                        "whiteSpace" => format!(
                            "The value '{}' of the facet 'whitespace' is not valid.",
                            raw.value
                        ),
                        "totalDigits" => format!(
                            "The value '{}' of the facet '{kind}' is not a valid 'positiveInteger'.",
                            raw.value
                        ),
                        _ => format!(
                            "The value '{}' of the facet '{kind}' is not a valid 'nonNegativeInteger'.",
                            raw.value
                        ),
                    };
                    invalid(self, message)
                }
            },
        }
    }

    fn fixed_facets(&self, ty: TypeId) -> Vec<&'static str> {
        let mut out = Vec::new();
        let mut t = ty;
        while !is_builtin(t) {
            if let Some(own) = self.own_facets.get(&t) {
                out.extend(
                    own.iter()
                        .filter(|(_, fixed)| *fixed)
                        .map(|(name, _)| *name),
                );
            }
            t = self.types[t].base;
        }
        out
    }

    /// `xmlSchemaDeriveAndValidateFacets`: checks this step's facets
    /// against each other and the base's, and returns the effective set.
    fn derive_facets(
        &mut self,
        own: &[(Facet, bool, Loc)],
        base: &[Facet],
        base_fixed: &[&'static str],
        loc: Loc,
    ) -> Vec<Facet> {
        let find = |set: &[Facet], name: &str| set.iter().find(|f| facet_name(f) == name).cloned();
        let own_set: Vec<Facet> = own.iter().map(|(f, ..)| f.clone()).collect();
        let mine = |name: &str| find(&own_set, name);
        let theirs = |name: &str| find(base, name);
        let mut errors: Vec<String> = Vec::new();
        let mut err = |msg: String| errors.push(msg);
        if mine("length").is_some() && (mine("minLength").is_some() || mine("maxLength").is_some())
        {
            err("It is an error for both 'length' and either of 'minLength' or 'maxLength' to be specified on the same type definition".into());
        }
        for (a, b) in [
            ("maxInclusive", "maxExclusive"),
            ("minInclusive", "minExclusive"),
        ] {
            if mine(a).is_some() && mine(b).is_some() {
                err(format!(
                    "It is an error for both '{a}' and '{b}' to be specified on the same type definition"
                ));
            }
        }
        // (facet, other, other-from-base, failing comparisons)
        use Comparison::*;
        let rules: &[(&str, &str, bool, &[Comparison])] = &[
            ("length", "length", true, &[Less, Greater, Indeterminate]),
            ("minLength", "minLength", true, &[Less]),
            ("maxLength", "maxLength", true, &[Greater]),
            ("maxInclusive", "minInclusive", false, &[Less]),
            ("maxInclusive", "maxInclusive", true, &[Greater]),
            (
                "maxInclusive",
                "maxExclusive",
                true,
                &[Equal, Greater, Indeterminate],
            ),
            ("maxInclusive", "minInclusive", true, &[Less]),
            (
                "maxInclusive",
                "minExclusive",
                true,
                &[Less, Equal, Indeterminate],
            ),
            ("maxExclusive", "minExclusive", false, &[Less]),
            ("maxExclusive", "maxExclusive", true, &[Greater]),
            ("maxExclusive", "maxInclusive", true, &[Greater]),
            (
                "maxExclusive",
                "minInclusive",
                true,
                &[Less, Equal, Indeterminate],
            ),
            (
                "maxExclusive",
                "minExclusive",
                true,
                &[Less, Equal, Indeterminate],
            ),
            (
                "minExclusive",
                "maxInclusive",
                false,
                &[Equal, Greater, Indeterminate],
            ),
            ("minExclusive", "minExclusive", true, &[Less]),
            ("minExclusive", "maxInclusive", true, &[Greater]),
            ("minExclusive", "minInclusive", true, &[Less]),
            (
                "minExclusive",
                "maxExclusive",
                true,
                &[Equal, Greater, Indeterminate],
            ),
            (
                "minInclusive",
                "maxExclusive",
                false,
                &[Equal, Greater, Indeterminate],
            ),
            ("minInclusive", "minInclusive", true, &[Less]),
            ("minInclusive", "maxInclusive", true, &[Greater]),
            (
                "minInclusive",
                "minExclusive",
                true,
                &[Less, Equal, Indeterminate],
            ),
            (
                "minInclusive",
                "maxExclusive",
                true,
                &[Equal, Greater, Indeterminate],
            ),
            ("totalDigits", "totalDigits", true, &[Greater]),
            ("fractionDigits", "fractionDigits", true, &[Greater]),
        ];
        for &(name, other, from_base, failing) in rules {
            let (Some(facet), Some(against)) = (
                mine(name),
                if from_base {
                    theirs(other)
                } else {
                    mine(other)
                },
            ) else {
                continue;
            };
            let result = compare_facets(&facet, &against);
            if result == Incomparable || failing.contains(&result) {
                let whose = if from_base { " of the base type" } else { "" };
                err(format!(
                    "'{name}' is not a valid restriction of '{other}'{whose}"
                ));
            }
            if from_base && name == other && result != Equal && base_fixed.contains(&name) {
                err("The base type's facet is 'fixed', thus the value must not differ".into());
            }
        }
        let length = mine("length").or_else(|| theirs("length"));
        if let Some(length) = length {
            for (other, failing) in [("minLength", Less), ("maxLength", Greater)] {
                if let Some(against) = mine(other).or_else(|| theirs(other))
                    && compare_facets(&length, &against) == failing
                {
                    err(format!("'length' is not a valid restriction of '{other}'"));
                }
            }
        }
        if let (Some(fraction), Some(total)) = (
            mine("fractionDigits").or_else(|| theirs("fractionDigits")),
            mine("totalDigits").or_else(|| theirs("totalDigits")),
        ) && compare_facets(&fraction, &total) == Greater
        {
            err("'fractionDigits' has to be less than or equal to 'totalDigits'".into());
        }
        if let (Some(Facet::WhiteSpace(ws)), Some(Facet::WhiteSpace(bws))) =
            (mine("whiteSpace"), theirs("whiteSpace"))
        {
            let rank = |w: WhiteSpace| match w {
                WhiteSpace::Preserve => 0,
                WhiteSpace::Replace => 1,
                WhiteSpace::Collapse => 2,
            };
            if rank(ws) < rank(bws) {
                err("The 'whitespace' value has to be equal to or stronger than the 'whitespace' value of the base type".into());
            }
            if ws != bws && base_fixed.contains(&"whiteSpace") {
                err("The base type's facet is 'fixed', thus the value must not differ".into());
            }
        }
        for message in errors {
            self.report(loc, Code::InvalidFacetValue, message);
        }
        let mut effective = own_set;
        for facet in base {
            if !effective.iter().any(|f| facet_name(f) == facet_name(facet)) {
                effective.push(facet.clone());
            }
        }
        effective
    }
}

impl Fix {
    fn attr_name(&self, decl: AttrId) -> QName {
        self.d.attributes[decl].name
    }

    fn is_id(&self, ty: TypeId) -> bool {
        derives_from_builtin(&self.types, ty, Type::Id)
    }

    /// `xmlSchemaCheckAttrPropsCorrect`, the use-level part of
    /// `xmlSchemaCheckAttrUsePropsCorrect` and `xmlSchemaCheckAGPropsCorrect`.
    fn attribute_values(&mut self) {
        for a in 0..self.d.attributes.len() {
            let draft = self.d.attributes[a].clone();
            let Some((fixed, lexical)) = draft.value else {
                continue;
            };
            let ty = self.attr_types[a];
            if self.is_id(ty) {
                let message = "Value constraints are not allowed if the type definition is or is derived from xs:ID.";
                self.report(draft.loc, Code::APropsCorrect3, message.into());
                continue;
            }
            match self.value(ty, &lexical, draft.loc) {
                Ok(value) => {
                    self.attr_values[a] = Some(ValueConstraint {
                        fixed,
                        lexical: lexical.into(),
                        value: Some(value),
                    })
                }
                Err(failure) => {
                    for (code, message) in failure {
                        self.report(draft.loc, code, message);
                    }
                    let message = "The value of the value constraint is not valid.";
                    self.report(draft.loc, Code::APropsCorrect2, message.into());
                }
            }
        }
        let uses: Vec<_> = self
            .d
            .types
            .iter()
            .flat_map(|t| &t.attrs)
            .chain(self.d.attr_groups.iter().flat_map(|g| &g.attrs))
            .filter_map(|item| match item {
                AttrItem::Use(use_) => Some(use_.clone()),
                _ => None,
            })
            .collect();
        for use_ in uses {
            let (Ref::Id(decl), Some((false, _))) = (use_.decl, &use_.value) else {
                continue;
            };
            if self.attr_values[decl].as_ref().is_some_and(|v| v.fixed) {
                let message = "The attribute declaration has a 'fixed' value constraint , thus the attribute use must also have a 'fixed' value constraint.";
                self.report(use_.loc, Code::AuPropsCorrect2, message.into());
            }
        }
        for g in 0..self.d.attr_groups.len() {
            let uses = self.group_attrs[g]
                .as_ref()
                .map(|(u, _)| u.clone())
                .unwrap_or_default();
            let loc = self.d.attr_groups[g].loc;
            let kept = self.check_uses(uses, loc);
            if let Some((stored, _)) = &mut self.group_attrs[g] {
                *stored = kept;
            }
        }
    }

    /// Duplicate and multiple-ID attribute uses (ag- and ct-props-correct);
    /// returns the uses without the reported ones.
    fn check_uses(&mut self, uses: Vec<AttrUse>, loc: Loc) -> Vec<AttrUse> {
        let mut keep = vec![true; uses.len()];
        let mut has_id = false;
        for i in (0..uses.len()).rev() {
            let name = self.attr_name(uses[i].decl);
            if (0..i).any(|j| self.attr_name(uses[j].decl) == name) {
                let message = format!("Duplicate attribute use '{}'.", self.format(name));
                self.report(loc, Code::AgPropsCorrect, message);
                keep[i] = false;
                continue;
            }
            if self.is_id(self.attr_types[uses[i].decl]) {
                if has_id {
                    let message = format!(
                        "There must not exist more than one attribute declaration of type 'xs:ID' (or derived from 'xs:ID'). The attribute use '{}' violates this constraint.",
                        self.format(name)
                    );
                    self.report(loc, Code::AgPropsCorrect, message);
                    keep[i] = false;
                }
                has_id = true;
            }
        }
        uses.into_iter()
            .zip(keep)
            .filter_map(|(u, k)| k.then_some(u))
            .collect()
    }

    /// The effective value constraint of a use.
    fn use_value(&self, use_: &AttrUse) -> Option<ValueConstraint> {
        use_.value
            .clone()
            .or_else(|| self.attr_values[use_.decl].clone())
    }

    /// `xmlSchemaCheckDerivationOKRestriction2to4`.
    #[allow(clippy::too_many_arguments)]
    fn restricts_attributes(
        &mut self,
        loc: Loc,
        uses: &[AttrUse],
        base_uses: &[AttrUse],
        wildcard: Option<WildId>,
        base_wildcard: Option<WildId>,
        base_is_any: bool,
    ) {
        for use_ in uses {
            let name = self.attr_name(use_.decl);
            match base_uses.iter().find(|b| self.attr_name(b.decl) == name) {
                Some(base) => {
                    if !use_.required && base.required {
                        let message = format!(
                            "The 'optional' attribute use '{}' is inconsistent with the corresponding 'required' attribute use of the base.",
                            self.format(name)
                        );
                        self.report(loc, Code::DerivationOkRestriction2_1_1, message);
                    } else if !st_derived(
                        &self.types,
                        self.attr_types[use_.decl],
                        self.attr_types[base.decl],
                        0,
                    ) {
                        let message = format!(
                            "The attribute declaration's type of '{}' is not validly derived from the corresponding type of the base.",
                            self.format(name)
                        );
                        self.report(loc, Code::DerivationOkRestriction2_1_2, message);
                    } else if let Some(fixed) = self.use_value(base).filter(|v| v.fixed) {
                        let own = self
                            .use_value(base)
                            .filter(|_| false)
                            .or_else(|| self.use_value(use_));
                        let _ = own;
                        let ok = self
                            .use_value(base)
                            .is_some_and(|v| v.fixed && v.lexical == fixed.lexical);
                        if !ok {
                            let message = format!(
                                "The effective value constraint of the attribute use '{}' is inconsistent with its correspondent in the base.",
                                self.format(name)
                            );
                            self.report(loc, Code::DerivationOkRestriction2_1_3, message);
                        }
                    }
                }
                None => {
                    let allowed =
                        base_wildcard.is_some_and(|w| self.wildcards[w].ns.allows(name.ns));
                    if !allowed {
                        let message = format!(
                            "Neither a matching attribute use, nor a matching wildcard exists in the base for '{}'.",
                            self.format(name)
                        );
                        self.report(loc, Code::DerivationOkRestriction2_2, message);
                    }
                }
            }
        }
        for base in base_uses.iter().filter(|b| b.required) {
            let name = self.attr_name(base.decl);
            if !uses.iter().any(|u| self.attr_name(u.decl) == name) {
                let message = format!(
                    "A matching attribute use for the 'required' attribute use '{}' of the base is missing.",
                    self.format(name)
                );
                self.report(loc, Code::DerivationOkRestriction3, message);
            }
        }
        if let Some(own) = wildcard {
            match base_wildcard {
                None => {
                    let message =
                        "The type has an attribute wildcard, but the base does not have one.";
                    self.report(loc, Code::DerivationOkRestriction4_1, message.into());
                }
                Some(base) => {
                    let (own_w, base_w) = (&self.wildcards[own], &self.wildcards[base]);
                    if !ns_subset(&own_w.ns, &base_w.ns) {
                        let message = "The attribute wildcard is not a valid subset of the wildcard in the base.";
                        self.report(loc, Code::DerivationOkRestriction4_2, message.into());
                    } else if !base_is_any && own_w.process < base_w.process {
                        let message = "The {process contents} of the attribute wildcard is weaker than the one in the base.";
                        self.report(loc, Code::DerivationOkRestriction4_3, message.into());
                    }
                }
            }
        }
    }

    /// `xmlSchemaCheckSRCRedefineSecond`: attribute groups redefined without
    /// a self-reference must restrict the originals.
    fn redefine_second(&mut self) {
        for r in 0..self.d.redefines.len() {
            let redefine = self.d.redefines[r].clone();
            let (GlobalKind::AttrGroup, false, Some(prev)) =
                (redefine.kind, redefine.self_ref, self.redefined[r])
            else {
                continue;
            };
            let (uses, wildcard) = self.group_attributes(redefine.id);
            let (base_uses, base_wildcard) = self.group_attributes(prev);
            self.restricts_attributes(
                redefine.loc,
                &uses,
                &base_uses,
                wildcard,
                base_wildcard,
                false,
            );
        }
    }

    // ---- complex types ---------------------------------------------------

    fn particle_emptiable(&self, particle: &Particle) -> bool {
        if particle.min == 0 {
            return true;
        }
        match &particle.term {
            Term::Missing => true,
            Term::Group(compositor, children) => self.group_emptiable(*compositor, children),
            Term::GroupDef(g) => match &self.d.groups[*g].model {
                Some((compositor, children, _)) => self.group_emptiable(*compositor, children),
                None => true,
            },
            _ => false,
        }
    }

    fn group_emptiable(&self, compositor: Compositor, children: &[Particle]) -> bool {
        if children.is_empty() {
            return true;
        }
        let mut each = children.iter().map(|child| match child.term {
            Term::Element(_) | Term::Wildcard(_) => child.min == 0,
            _ => self.particle_emptiable(child),
        });
        match compositor {
            Compositor::Choice => each.any(|e| e),
            _ => each.all(|e| e),
        }
    }

    /// The top term's compositor and children, through a group definition.
    fn top_group<'p>(&'p self, particle: &'p Particle) -> Option<(Compositor, &'p [Particle])> {
        match &particle.term {
            Term::Group(compositor, children) => Some((*compositor, children)),
            Term::GroupDef(g) => self.d.groups[*g]
                .model
                .as_ref()
                .map(|(c, children, _)| (*c, children.as_slice())),
            _ => None,
        }
    }

    fn base_emptiable(&self, base: TypeId) -> bool {
        self.particles
            .get(&base)
            .is_none_or(|p| self.particle_emptiable(p))
    }

    fn complex_types(&mut self) {
        for id in self.type_order() {
            if !self.is_simple(id) {
                self.fix_complex(id);
            }
        }
    }

    fn new_content_type(&mut self, index: usize, base: TypeId) -> TypeId {
        let mut draft = self.d.types[index].clone();
        draft.name = None;
        draft.shape = Shape::Restriction;
        draft.final_ = 0;
        self.d.types.push(draft);
        let id = self.types.len();
        self.types.push(self.types[builtin(Type::String)].clone());
        self.fixed.push(false);
        self.invalid.push(false);
        self.bases.push(Some(base));
        self.fix_simple(id);
        id
    }

    /// `xmlSchemaFixupComplexType`.
    fn fix_complex(&mut self, id: TypeId) {
        if self.fixed[id] {
            return;
        }
        self.fixed[id] = true;
        let index = id - Drafts::FIRST_TYPE;
        let draft = self.d.types[index].clone();
        let loc = draft.loc;
        let Some(base) = self.bases[id] else {
            self.invalid[id] = true;
            return;
        };
        if self.invalid[base] {
            return;
        }
        let errors = self.d.diagnostics.len();
        let simple_content = matches!(
            draft.shape,
            Shape::Complex {
                simple_content: true
            }
        );
        let extension = draft.derivation == EXTENSION;
        let base_def = self.types[base].clone();
        let base_content = base_def.complex().map(|c| c.content.clone());
        // src-ct
        let src_ct = |fix: &mut Fix, message: &str| {
            fix.report(loc, Code::SrcCt1, message.into());
            fix.invalid[id] = true;
        };
        if !simple_content {
            if base_content.is_none() {
                let message = format!(
                    "If using <complexContent>, the base type is expected to be a complex type. The base type {} is a simple type.",
                    self.type_name(base)
                );
                return src_ct(self, &message);
            }
        } else {
            match &base_content {
                None if !extension => {
                    return src_ct(
                        self,
                        "If using <simpleContent> and <restriction>, the base type must be a complex type.",
                    );
                }
                None | Some(Content::Simple(_)) => {}
                Some(Content::Elements { mixed: true, .. }) if !extension => {
                    if !self.base_emptiable(base) {
                        return src_ct(
                            self,
                            "If <simpleContent> and <restriction> is used, the base type must be a simple type or a complex type with mixed content and particle emptiable.",
                        );
                    }
                    if draft.content_type.is_none() {
                        return src_ct(
                            self,
                            "A <simpleType> is expected among the children of <restriction>, if <simpleContent> is used and the base type is a complex type.",
                        );
                    }
                }
                Some(_) if extension => {
                    return src_ct(
                        self,
                        "If <simpleContent> and <extension> is used, the base type must be a simple type.",
                    );
                }
                Some(_) => {
                    return src_ct(
                        self,
                        "If <simpleContent> and <restriction> is used, the base type must be a simple type or a complex type with mixed content and particle emptiable.",
                    );
                }
            }
        }
        // Content type.
        let mut particle = None;
        let content = if simple_content {
            match (&base_content, extension) {
                (Some(Content::Simple(ct)), false) => {
                    let content_base = draft.content_type.unwrap_or(*ct);
                    Content::Simple(self.new_content_type(index, content_base))
                }
                (Some(Content::Elements { .. }), false) => {
                    Content::Simple(draft.content_type.expect("checked"))
                }
                (Some(Content::Simple(ct)), true) => Content::Simple(*ct),
                (None, true) => Content::Simple(base),
                _ => Content::Empty,
            }
        } else {
            let own = draft.particle.clone();
            let empty = own.as_ref().is_none_or(|p| match self.top_group(p) {
                Some((Compositor::Choice, children)) => p.min == 0 && children.is_empty(),
                Some((_, children)) => children.is_empty(),
                None => matches!(p.term, Term::Missing),
            });
            let dummy = empty && draft.mixed;
            let sequence = |children: Vec<Particle>| Particle {
                min: 1,
                max: Some(1),
                term: Term::Group(Compositor::Sequence, children),
                loc,
            };
            let own = match empty {
                true => dummy.then(|| sequence(Vec::new())),
                false => own,
            };
            let base_particle = self.particles.get(&base).cloned();
            let elements = Content::Elements {
                mixed: draft.mixed,
                model: placeholder_model(),
            };
            if !extension {
                particle = own;
                match particle {
                    None => Content::Empty,
                    Some(_) => elements,
                }
            } else if empty && !dummy {
                particle = base_particle;
                base_content.clone().unwrap_or(Content::Empty)
            } else if matches!(base_content, Some(Content::Empty)) {
                particle = own;
                elements
            } else {
                let is_all = |fix: &Fix, p: &Option<Particle>| {
                    p.as_ref()
                        .is_some_and(|p| matches!(fix.top_group(p), Some((Compositor::All, _))))
                };
                if is_all(self, &own) {
                    self.report(loc, Code::CosAllLimited, "The type has an 'all' model group in its {content type} and thus cannot be derived from a non-empty type, since this would produce a 'sequence' model group containing the 'all' model group; 'all' model groups are not allowed to appear inside other model groups.".into());
                } else if is_all(self, &base_particle) {
                    self.report(loc, Code::CosAllLimited, "A type cannot be derived by extension from a type which has an 'all' model group in its {content type}, since this would produce a 'sequence' model group containing the 'all' model group; 'all' model groups are not allowed to appear inside other model groups.".into());
                }
                particle = match (base_particle, own, dummy) {
                    (Some(b), Some(o), false) => Some(sequence(vec![b, o])),
                    (Some(b), _, true) => Some(sequence(vec![b])),
                    // libxml2 replaces the own group's particles with the
                    // base's, which are absent here.
                    (None, Some(o), _) => Some(Particle {
                        term: match self.top_group(&o) {
                            Some((compositor, _)) => Term::Group(compositor, Vec::new()),
                            None => o.term.clone(),
                        },
                        ..o
                    }),
                    (b, o, _) => o.or(b),
                };
                elements
            }
        };
        if let Some(particle) = particle.clone()
            && matches!(content, Content::Elements { .. })
        {
            self.particles.insert(id, particle);
        }
        // Attribute uses and wildcard.
        let (mut uses, prohibitions, mut wildcard) = self.expand(&draft.attrs, draft.wildcard, loc);
        let (base_uses, base_wildcard) = match base_def.complex() {
            Some(c) => (c.attributes.clone(), c.wildcard),
            None => (Vec::new(), None),
        };
        if !extension {
            let mut prohibited = Vec::new();
            for (name, ploc) in prohibitions {
                if uses.iter().any(|u| self.attr_name(u.decl) == name) {
                    let message = format!(
                        "Skipping pointless attribute use prohibition '{}', since a corresponding attribute use exists already in the type definition.",
                        self.format(name)
                    );
                    self.report(ploc, Code::WarnAttrPointlessProh, message);
                } else {
                    prohibited.push(name);
                }
            }
            let own_count = uses.len();
            for use_ in &base_uses {
                let name = self.attr_name(use_.decl);
                if !prohibited.contains(&name)
                    && !uses[..own_count]
                        .iter()
                        .any(|u| self.attr_name(u.decl) == name)
                {
                    uses.push(use_.clone());
                }
            }
        } else {
            uses.extend(base_uses.iter().cloned());
            if let Some(base_w) = base_wildcard {
                wildcard = match wildcard {
                    None => Some(base_w),
                    Some(own) => {
                        match ns_union(&self.wildcards[own].ns, &self.wildcards[base_w].ns) {
                            Ok(ns) => Some(self.new_wildcard(Wildcard {
                                ns,
                                process: self.wildcards[own].process,
                            })),
                            Err(()) => {
                                self.report(
                                    loc,
                                    Code::UnionNotExpressible,
                                    "The union of the wildcard is not expressible.".into(),
                                );
                                Some(own)
                            }
                        }
                    }
                };
            }
        }
        let own_uses = uses.clone();
        let uses = self.check_uses(uses, loc);
        let _ = own_uses;
        self.types[id] = TypeDef {
            name: draft.name,
            base,
            derivation: draft.derivation,
            final_: draft.final_,
            block: draft.block,
            abstract_: draft.abstract_,
            kind: Kind::Complex(ComplexType {
                content: content.clone(),
                attributes: uses.clone(),
                wildcard,
            }),
        };
        if errors != self.d.diagnostics.len() {
            return;
        }
        // cos-ct-extends or derivation-ok-restriction.
        match (extension, &base_content) {
            (true, Some(base_c)) => {
                if base_def.final_ & EXTENSION != 0 {
                    self.report(
                        loc,
                        Code::CosCtExtends1_1,
                        "The 'final' of the base type definition contains 'extension'.".into(),
                    );
                    return;
                }
                let ok = match (&content, base_c) {
                    (Content::Simple(a), Content::Simple(b)) => a == b,
                    (Content::Empty, Content::Empty) => true,
                    _ if particle.is_none() => {
                        self.report(
                            loc,
                            Code::CosCtExtends1_1,
                            "The content type must specify a particle.".into(),
                        );
                        return;
                    }
                    (_, Content::Empty) => true,
                    (Content::Elements { mixed: a, .. }, Content::Elements { mixed: b, .. }) => {
                        a == b
                    }
                    _ => false,
                };
                if !ok {
                    self.report(loc, Code::CosCtExtends1_1, "The content type of both, the type and its base type, must either 'mixed' or 'element-only'.".into());
                }
            }
            (true, None) => {
                if base_def.final_ & EXTENSION != 0 {
                    self.report(
                        loc,
                        Code::CosCtExtends1_1,
                        "The 'final' of the base type definition contains 'extension'.".into(),
                    );
                }
            }
            (false, None) => {
                self.report(
                    loc,
                    Code::DerivationOkRestriction1,
                    "The base type must be a complex type.".into(),
                );
            }
            (false, Some(base_c)) => {
                if base_def.final_ & RESTRICTION != 0 {
                    self.report(
                        loc,
                        Code::DerivationOkRestriction1,
                        "The 'final' of the base type definition contains 'restriction'.".into(),
                    );
                    return;
                }
                self.restricts_attributes(
                    loc,
                    &uses,
                    &base_uses,
                    wildcard,
                    base_wildcard,
                    base == ANY_TYPE,
                );
                if base == ANY_TYPE {
                    return;
                }
                let base_mixed_emptiable = matches!(base_c, Content::Elements { mixed: true, .. })
                    && self.base_emptiable(base);
                let message = match (&content, base_c) {
                    (Content::Simple(a), Content::Simple(b)) => (!st_derived(&self.types, *a, *b, 0))
                        .then_some("The {content type} is not validly derived from the base type's {content type}."),
                    (Content::Simple(_), _) => (!base_mixed_emptiable).then_some(
                        "The content type of the base type must be either a simple type or 'mixed' and an emptiable particle.",
                    ),
                    (Content::Empty, Content::Empty) => None,
                    (Content::Empty, Content::Elements { .. }) => (!self.base_emptiable(base)).then_some(
                        "The content type of the base type must be either empty or 'mixed' (or 'elements-only') and an emptiable particle.",
                    ),
                    (Content::Empty, _) => Some(
                        "The content type of the base type must be either empty or 'mixed' (or 'elements-only') and an emptiable particle.",
                    ),
                    (Content::Elements { mixed: true, .. }, Content::Elements { mixed: false, .. } | Content::Empty | Content::Simple(_)) => {
                        Some("If the content type is 'mixed', then the content type of the base type must also be 'mixed'.")
                    }
                    (Content::Elements { .. }, Content::Elements { .. }) => None,
                    (Content::Elements { .. }, _) => Some("The type is not a valid restriction of its base type."),
                };
                if let Some(message) = message {
                    self.report(loc, Code::DerivationOkRestriction1, message.into());
                }
            }
        }
    }

    // ---- element declarations --------------------------------------------

    /// The type of an element: its own, its head's, or `anyType`.
    fn element_types(&mut self) {
        for e in 0..self.d.elements.len() {
            if self.d.elements[e].ty.is_some() {
                continue;
            }
            let mut seen = HashSet::new();
            let mut head = self.elem_heads[e];
            let mut ty = ANY_TYPE;
            while let Some(h) = head {
                if !seen.insert(h) {
                    break;
                }
                if self.d.elements[h].ty.is_some() {
                    ty = self.elem_types[h];
                    break;
                }
                head = self.elem_heads[h];
            }
            self.elem_types[e] = ty;
        }
    }

    fn element_decls(&mut self) {
        self.element_types();
        for e in 0..self.d.elements.len() {
            let draft = self.d.elements[e].clone();
            let errors = self.d.diagnostics.len();
            let ty = self.elem_types[e];
            if let Some(head) = self.elem_heads[e] {
                let mut seen = HashSet::new();
                let mut h = Some(head);
                while let Some(current) = h {
                    if current == e || !seen.insert(current) {
                        let message = format!(
                            "The element declaration '{}' defines a circular substitution group to element declaration '{}'.",
                            self.format(draft.name),
                            self.format(self.d.elements[head].name)
                        );
                        self.report(draft.loc, Code::EPropsCorrect6, message);
                        break;
                    }
                    h = self.elem_heads[current];
                }
                let head_type = self.elem_types[head];
                if errors == self.d.diagnostics.len() && ty != head_type {
                    let set = self.d.elements[head].final_ & (EXTENSION | RESTRICTION);
                    if !derived(&self.types, ty, head_type, set) {
                        let message = format!(
                            "The type definition {} was either rejected by the substitution group affiliation '{}', or not validly derived from its type definition {}.",
                            self.type_name(ty),
                            self.format(self.d.elements[head].name),
                            self.type_name(head_type)
                        );
                        self.report(draft.loc, Code::EPropsCorrect4, message);
                    }
                }
            }
            if let Some((fixed, lexical)) = &draft.value {
                let content = match &self.types[ty].kind {
                    Kind::Simple(_) => Some(ty),
                    Kind::Complex(c) => match &c.content {
                        Content::Simple(ct) => Some(*ct),
                        _ => None,
                    },
                };
                if content.is_some_and(|ct| self.is_id(ct)) {
                    let message = "The type definition (or type definition's content type) is or is derived from ID; value constraints are not allowed in conjunction with such a type definition.";
                    self.report(draft.loc, Code::EPropsCorrect5, message.into());
                } else {
                    let mixed_emptiable = matches!(
                        self.types[ty].complex().map(|c| &c.content),
                        Some(Content::Elements { mixed: true, .. })
                    ) && self.base_emptiable(ty);
                    match content {
                        None if !mixed_emptiable => {
                            let message = "For a string to be a valid default, the type definition must be a simple type or a complex type with mixed content and a particle emptiable.";
                            self.report(draft.loc, Code::CosValidDefault2_1, message.into());
                        }
                        None => {
                            self.elem_values[e] = Some(ValueConstraint {
                                fixed: *fixed,
                                lexical: lexical.as_str().into(),
                                value: None,
                            })
                        }
                        Some(ct) => match self.value(ct, lexical, draft.loc) {
                            Ok(value) => {
                                self.elem_values[e] = Some(ValueConstraint {
                                    fixed: *fixed,
                                    lexical: lexical.as_str().into(),
                                    value: Some(value),
                                })
                            }
                            Err(failure) => {
                                for (code, message) in failure {
                                    self.report(draft.loc, code, message);
                                }
                            }
                        },
                    }
                }
            }
            if errors == self.d.diagnostics.len() && !draft.abstract_ {
                self.substitution_group(e);
            }
        }
    }

    /// `xmlSchemaCheckElemSubstGroup`: adds `e` to each head that admits it.
    fn substitution_group(&mut self, e: ElemId) {
        let ty = self.elem_types[e];
        let mut seen = HashSet::new();
        let mut head = self.elem_heads[e];
        while let Some(h) = head {
            if !seen.insert(h) {
                break;
            }
            head = self.elem_heads[h];
            let head_decl = &self.d.elements[h];
            if head_decl.block & SUBSTITUTION != 0 {
                continue;
            }
            let head_type = self.elem_types[h];
            if ty != head_type {
                let mut block = head_decl.block & (EXTENSION | RESTRICTION);
                let mut methods = 0;
                let mut t = ty;
                let mut steps = 0;
                while t != head_type && t != ANY_TYPE && steps < self.types.len() {
                    methods |= self.types[t].derivation & (EXTENSION | RESTRICTION);
                    t = self.types[t].base;
                    steps += 1;
                }
                let mut t = self.types[ty].base;
                let mut steps = 0;
                while self.types[t].complex().is_some() && steps < self.types.len() {
                    block |= self.types[t].block;
                    if t == head_type || t == ANY_TYPE {
                        break;
                    }
                    t = self.types[t].base;
                    steps += 1;
                }
                if block & methods != 0 {
                    continue;
                }
            }
            if !self.members_of[h].contains(&e) {
                self.members_of[h].push(e);
            }
        }
    }

    // ---- content models ----------------------------------------------------

    fn leaves(&self, e: ElemId) -> Vec<Leaf> {
        std::iter::once(e)
            .chain(self.members_of[e].iter().copied())
            .map(|m| Leaf::Element(self.d.elements[m].name, m))
            .collect()
    }

    /// The particle tree of `particle` for the content model builders.
    fn node(&self, particle: &Particle, depth: usize) -> Result<Node, TooComplex> {
        if depth > self.limits.max_depth {
            return Err(TooComplex);
        }
        let kind = match &particle.term {
            Term::Element(e) if !self.members_of[*e].is_empty() => NodeKind::Leaves {
                leaves: self.leaves(*e),
                substitution: true,
            },
            Term::Element(e) if self.d.elements[*e].abstract_ => NodeKind::Abstract,
            Term::Element(e) => NodeKind::Leaves {
                leaves: vec![Leaf::Element(self.d.elements[*e].name, *e)],
                substitution: false,
            },
            Term::Wildcard(w) => NodeKind::Leaves {
                leaves: vec![Leaf::Wildcard(self.wildcards[*w].ns.clone(), *w)],
                substitution: false,
            },
            Term::Missing | Term::ElementRef(_) | Term::GroupRef(_) | Term::RedefinedGroup(_) => {
                NodeKind::Empty
            }
            Term::Group(..) | Term::GroupDef(_) => {
                let (compositor, children) = self.top_group(particle).expect("group");
                let children = children
                    .iter()
                    .map(|child| self.node(child, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?;
                match compositor {
                    Compositor::Choice => NodeKind::Choice(children),
                    _ => NodeKind::Sequence(children),
                }
            }
        };
        Ok(Node {
            min: particle.min,
            max: particle.max,
            kind,
        })
    }

    /// The model of a particle; `Ok(None)` when libxml2 would find it not
    /// deterministic.
    fn compile(&mut self, particle: &Particle) -> Result<Option<Model>, TooComplex> {
        if let Some((Compositor::All, children)) = self.top_group(particle) {
            let items = children
                .iter()
                .filter(|child| child.max != Some(0))
                .filter_map(|child| match child.term {
                    Term::Element(e) => Some((self.leaves(e), child.min > 0)),
                    _ => None,
                })
                .collect();
            return Ok(Some(Model::All(AllGroup {
                items,
                optional: particle.min == 0,
            })));
        }
        let node = self.node(particle, 0)?;
        let (deterministic, shadowed) =
            automaton::deterministic(&node, &mut self.model_work).map_err(|_| TooComplex)?;
        if !deterministic {
            return Ok(None);
        }
        Graph::compile(&node, shadowed, &mut self.model_work).map(|graph| Some(Model::Graph(graph)))
    }

    /// `xmlSchemaBuildContentModel` for every complex type with element content.
    fn content_models(&mut self) {
        let ids: Vec<TypeId> = self.particles.keys().copied().collect();
        let mut ids = ids;
        ids.sort_unstable();
        for id in ids {
            let particle = self.particles[&id].clone();
            let loc = match id {
                ANY_TYPE => particle.loc,
                id => self.d.types[id - Drafts::FIRST_TYPE].loc,
            };
            match self.compile(&particle) {
                Ok(Some(model)) => {
                    if let Kind::Complex(ComplexType {
                        content: Content::Elements { model: slot, .. },
                        ..
                    }) = &mut self.types[id].kind
                    {
                        *slot = model;
                    }
                }
                Ok(None) => {
                    let what = match self.types[id].name {
                        Some(name) => format!("complex type '{}'", self.format(name)),
                        None => "local complex type".into(),
                    };
                    self.report(
                        loc,
                        Code::NotDeterministic,
                        format!("{what}: The content model is not determinist."),
                    );
                }
                Err(TooComplex) => {
                    self.report(
                        loc,
                        Code::Internal,
                        "The content model is too complex to compile within the limits.".into(),
                    );
                }
            }
        }
    }

    // ---- assembly ----------------------------------------------------------

    fn finish(self) -> Schema {
        let elements = self
            .d
            .elements
            .iter()
            .enumerate()
            .map(|(e, draft)| ElementDecl {
                ty: self.elem_types[e],
                nillable: draft.nillable,
                abstract_: draft.abstract_,
                value: self.elem_values[e].clone(),
                block: draft.block,
                idcs: draft.idcs.clone(),
            })
            .collect();
        let attributes = self
            .d
            .attributes
            .iter()
            .enumerate()
            .map(|(a, draft)| AttributeDecl {
                name: draft.name,
                ty: self.attr_types[a],
                value: self.attr_values[a].clone(),
            })
            .collect();
        let idcs = self
            .d
            .idcs
            .iter()
            .enumerate()
            .map(|(i, draft)| Idc {
                kind: draft.kind,
                name: draft.name,
                selector: draft.selector.clone().unwrap_or_else(|| {
                    crate::pattern::Pattern::compile(".", &[], crate::pattern::XS_SELECTOR)
                        .expect("pattern")
                }),
                fields: draft.fields.clone(),
                refer: self.refers[i],
            })
            .collect();
        let pick = |kind: GlobalKind| -> HashMap<QName, usize> {
            self.table
                .iter()
                .filter(|((k, _), _)| *k == kind)
                .map(|((_, name), &id)| (*name, id))
                .collect()
        };
        Schema {
            global_elements: pick(GlobalKind::Element),
            global_types: pick(GlobalKind::Type),
            global_attributes: pick(GlobalKind::Attribute),
            notations: self.notations.clone(),
            names: self.d.names.clone(),
            types: self.types.clone(),
            elements,
            attributes,
            wildcards: self.wildcards.clone(),
            idcs,
            warnings: self.d.diagnostics.clone(),
            max_counter_vectors: self.limits.max_counter_vectors,
        }
    }
}

fn placeholder_model() -> Model {
    Model::All(AllGroup {
        items: Vec::new(),
        optional: true,
    })
}
