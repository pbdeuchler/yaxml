//! Instance validation as libxml2's `xmlSchemaVDocWalk` performs it: a
//! preorder walk validating each element against its declaration and type,
//! its attributes against the attribute uses and wildcard, and its children
//! against the content model; an invalid element's content is skipped, and
//! after an unexpected child the rest of the parent's content is skipped.

use std::collections::HashSet;

use crate::schema_types::{Type, WhiteSpace, normalize};
use crate::xpath::Tree;
use crate::xpath::tree::{Item, namespaces};

use super::components::*;
use super::fixup::derived;
use super::idc::{Engine, FieldValue, Key, Scope as IdcScope};
use super::model::{Leaf, Matched, Run, StepError};
use super::parse::XSI;
use super::simple::{self, Scope};
use super::{Code, Diagnostic, Schema};

/// `xsi:schemaLocation` pairs and `xsi:noNamespaceSchemaLocation`s in
/// document order, as `(namespace, location)`.
pub(crate) fn schema_locations<T: Tree>(tree: &T) -> Vec<(Option<String>, String)> {
    let mut out = Vec::new();
    for node in crate::xpath::tree::preorder(tree, Some(tree.root()), None) {
        for (_, name, value) in crate::xpath::tree::attributes(tree, node) {
            if name.namespace != Some(XSI) {
                continue;
            }
            let tokens: Vec<&str> = value
                .split([' ', '\t', '\n', '\r'])
                .filter(|t| !t.is_empty())
                .collect();
            match name.local {
                "schemaLocation" => {
                    for pair in tokens.chunks(2).filter(|pair| pair.len() == 2) {
                        out.push((Some(pair[0].to_owned()), pair[1].to_owned()));
                    }
                }
                "noNamespaceSchemaLocation" => {
                    out.extend(tokens.first().map(|t| (None, (*t).to_owned())))
                }
                _ => {}
            }
        }
    }
    out
}

pub(crate) fn run<T: Tree>(schema: &Schema, tree: &T) -> Vec<Diagnostic<T::Id>> {
    let mut validator = Validator {
        schema,
        tree,
        frames: Vec::new(),
        scopes: Vec::new(),
        engine: Engine::new(schema),
        skip: None,
        ids: tree
            .ids()
            .iter()
            .filter_map(|&(element, index)| {
                tree.attribute(element, index)
                    .map(|(_, v)| normalize(v, WhiteSpace::Collapse).into_owned())
            })
            .collect(),
        dtd_ids: tree.ids().iter().copied().collect(),
        out: Vec::new(),
    };
    validator.walk();
    validator.out
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Meta {
    Type,
    Nil,
    Location,
}

struct Attr<'t> {
    index: usize,
    local: &'t str,
    ns: Option<&'t str>,
    name: QName,
    value: &'t str,
    meta: Option<Meta>,
}

struct Frame<'t, Id> {
    node: Id,
    local: &'t str,
    ns: Option<&'t str>,
    name: QName,
    decl: Option<ElemId>,
    wildcard: Option<WildId>,
    ty: Option<TypeId>,
    nilled: bool,
    local_type: bool,
    empty: bool,
    element_content: bool,
    bad_content: bool,
    not_expected: bool,
    bad_type: bool,
    run: Option<Run>,
    text: String,
    value_needed: bool,
    applied_xpath: bool,
    value: Option<Key>,
}

/// The outcome of an attribute.
#[derive(Clone, Debug, PartialEq)]
enum State {
    Unknown,
    Meta,
    Assessed(AttrId, Option<usize>),
    Skip,
    LaxNoDecl,
    StrictNoDecl,
    Invalid,
    FixedMismatch(Box<str>),
}

struct Validator<'s, 't, T: Tree> {
    schema: &'s Schema,
    tree: &'t T,
    frames: Vec<Frame<'t, T::Id>>,
    scopes: Vec<IdcScope<T::Id>>,
    engine: Engine<'s>,
    skip: Option<usize>,
    ids: HashSet<String>,
    dtd_ids: HashSet<(T::Id, usize)>,
    out: Vec<Diagnostic<T::Id>>,
}

fn display(local: &str, ns: Option<&str>) -> String {
    match ns {
        Some(ns) => format!("{{{ns}}}{local}"),
        None => local.to_owned(),
    }
}

impl<'s, 't, T: Tree> Validator<'s, 't, T> {
    fn report(&mut self, node: T::Id, code: Code, message: String) {
        self.out.push(Diagnostic {
            code,
            node: Some(node),
            offset: self.tree.offset(node),
            message,
        });
    }

    /// Reports against the element at `depth`.
    fn element_error(&mut self, depth: usize, code: Code, message: &str) {
        let frame = &self.frames[depth];
        let text = format!("Element '{}': {message}", display(frame.local, frame.ns));
        let node = frame.node;
        self.report(node, code, text);
    }

    fn attribute_error(&mut self, depth: usize, attr: &Attr<'t>, code: Code, message: &str) {
        let frame = &self.frames[depth];
        let text = format!(
            "Element '{}', attribute '{}': {message}",
            display(frame.local, frame.ns),
            display(attr.local, attr.ns)
        );
        let node = frame.node;
        self.report(node, code, text);
    }

    fn qname(&self, local: &str, ns: Option<&str>) -> QName {
        QName {
            ns: ns.map_or(NO_NS, |ns| self.schema.names.get(ns)),
            local: self.schema.names.get(local),
        }
    }

    fn walk(&mut self) {
        let tree = self.tree;
        let root = std::iter::successors(tree.first_child(tree.root()), |&n| tree.next_sibling(n))
            .find(|&n| matches!(tree.item(n), Item::Element(_)));
        let Some(root) = root else {
            self.out.push(Diagnostic {
                code: Code::DocumentElementMissing,
                node: None,
                offset: None,
                message: "The document has no document element.".into(),
            });
            return;
        };
        let mut node = root;
        loop {
            let skipped = self.skip.is_some_and(|skip| self.frames.len() > skip);
            let descend = !skipped
                && match tree.item(node) {
                    Item::Element(name) => self.enter(node, name.local, name.namespace),
                    Item::Text(text) => {
                        self.text(node, text);
                        false
                    }
                    _ => false,
                };
            if let Some(child) = tree.first_child(node).filter(|_| descend) {
                node = child;
                continue;
            }
            loop {
                if !skipped_element(self, node) && matches!(tree.item(node), Item::Element(_)) {
                    self.leave();
                    if node == root {
                        return;
                    }
                }
                if let Some(next) = tree.next_sibling(node) {
                    node = next;
                    break;
                }
                match tree.parent(node) {
                    Some(parent) => node = parent,
                    None => return,
                }
            }
        }
    }

    // ---- elements ------------------------------------------------------------

    /// Starts an element; whether to validate its content.
    fn enter(&mut self, node: T::Id, local: &'t str, ns: Option<&'t str>) -> bool {
        let depth = self.frames.len();
        let name = self.qname(local, ns);
        self.frames.push(Frame {
            node,
            local,
            ns,
            name,
            decl: None,
            wildcard: None,
            ty: None,
            nilled: false,
            local_type: false,
            empty: true,
            element_content: false,
            bad_content: false,
            not_expected: false,
            bad_type: false,
            run: None,
            text: String::new(),
            value_needed: false,
            applied_xpath: false,
            value: None,
        });
        self.scopes.push(IdcScope::default());
        let attrs: Vec<Attr<'t>> = crate::xpath::tree::attributes(self.tree, node)
            .filter(|(_, name, _)| !name.is_namespace_declaration())
            .map(|(index, name, value)| Attr {
                index,
                local: name.local,
                ns: name.namespace,
                name: self.qname(name.local, name.namespace),
                value,
                meta: (name.namespace == Some(XSI))
                    .then_some(match name.local {
                        "type" => Some(Meta::Type),
                        "nil" => Some(Meta::Nil),
                        "schemaLocation" | "noNamespaceSchemaLocation" => Some(Meta::Location),
                        _ => None,
                    })
                    .flatten(),
            })
            .collect();
        let ok = self.validate_element(depth, &attrs);
        if !ok {
            self.skip = Some(depth);
            return false;
        }
        self.skip.is_none_or(|skip| depth < skip)
    }

    /// `xmlSchemaValidateElem`; false when the content must be skipped.
    fn validate_element(&mut self, depth: usize, attrs: &[Attr<'t>]) -> bool {
        if depth > 0 {
            if !self.validate_child(depth, attrs) {
                return false;
            }
            if self.skip == Some(depth) {
                return true;
            }
        } else {
            let name = self.frames[depth].name;
            match self.schema.global_elements.get(&name) {
                Some(&decl) => self.frames[depth].decl = Some(decl),
                None => {
                    self.element_error(
                        depth,
                        Code::CvcElt1,
                        "No matching global declaration available for the validation root.",
                    );
                    return false;
                }
            }
        }
        if let Some(wildcard) = self.frames[depth].wildcard {
            match self.schema.wildcards[wildcard].process {
                Process::Skip => {
                    self.skip = Some(depth);
                    return true;
                }
                process => {
                    let name = self.frames[depth].name;
                    if let Some(&decl) = self.schema.global_elements.get(&name) {
                        self.frames[depth].decl = Some(decl);
                    } else if process == Process::Strict {
                        self.element_error(depth, Code::CvcElt1, "No matching global element declaration available, but demanded by the strict wildcard.");
                        return false;
                    } else if let Some(attr) = attrs.iter().find(|a| a.meta == Some(Meta::Type)) {
                        self.frames[depth].ty = self.xsi_type(depth, attr, None).ok().flatten();
                    } else {
                        self.frames[depth].ty = Some(ANY_TYPE);
                    }
                }
            }
        }
        if let Some(decl) = self.frames[depth].decl
            && !self.validate_declaration(depth, decl, attrs)
        {
            return false;
        }
        let Some(ty) = self.frames[depth].ty else {
            self.frames[depth].bad_type = true;
            self.element_error(depth, Code::CvcType1, "The type definition is absent.");
            return false;
        };
        if self.schema.types[ty].abstract_ {
            self.frames[depth].bad_type = true;
            self.element_error(depth, Code::CvcType2, "The type definition is abstract.");
            return false;
        }
        if self.engine.active() {
            let (local, ns) = (self.frames[depth].local, self.frames[depth].ns);
            let resolved = self.engine.evaluate(local, ns, false, depth, &self.scopes);
            self.frames[depth].value_needed = resolved > 0;
            self.frames[depth].applied_xpath = true;
        }
        match self.schema.types[ty].complex() {
            Some(_) => self.attributes_complex(depth, ty, attrs),
            None => {
                for attr in attrs.iter().filter(|a| a.meta.is_none()) {
                    let message = format!(
                        "The attribute '{}' is not allowed.",
                        display(attr.local, attr.ns)
                    );
                    self.attribute_error(depth, attr, Code::CvcType3_1_1, &message);
                }
            }
        }
        true
    }

    /// `xmlSchemaValidateChildElem`: the child against its parent's content.
    fn validate_child(&mut self, depth: usize, attrs: &[Attr<'t>]) -> bool {
        let parent = depth - 1;
        self.frames[parent].empty = false;
        self.frames[parent].element_content = true;
        if self.frames[parent].nilled {
            self.element_error(parent, Code::CvcElt3_2_1, "Neither character nor element content is allowed, because the element was 'nilled'.");
            return self.unexpected(depth);
        }
        let ptype = self.frames[parent].ty.expect("validated parent");
        if ptype == ANY_TYPE {
            let name = self.frames[depth].name;
            match self.schema.global_elements.get(&name) {
                Some(&decl) => self.frames[depth].decl = Some(decl),
                None => match attrs.iter().find(|a| a.meta == Some(Meta::Type)) {
                    Some(attr) => match self.xsi_type(depth, attr, None) {
                        Ok(ty) => self.frames[depth].ty = ty,
                        Err(()) => return false,
                    },
                    None => self.frames[depth].ty = Some(ANY_TYPE),
                },
            }
            return true;
        }
        let complex = self.schema.types[ptype].complex();
        match complex.map(|c| &c.content) {
            Some(Content::Empty) => {
                self.element_error(
                    parent,
                    Code::CvcComplexType2_1,
                    "Element content is not allowed, because the content type is empty.",
                );
                self.unexpected(depth)
            }
            Some(Content::Elements { model, .. }) => {
                let name = self.frames[depth].name;
                let cap = self.schema.max_counter_vectors;
                let run = self.frames[parent].run.get_or_insert_with(|| model.start());
                let step = model.step(run, name, cap);
                match step {
                    Ok(Matched::Element(decl)) => {
                        self.frames[depth].decl = Some(decl);
                        true
                    }
                    Ok(Matched::Wildcard(wildcard)) => {
                        self.frames[depth].wildcard = Some(wildcard);
                        true
                    }
                    Err(StepError::Unexpected) => {
                        let run = self.frames[parent].run.as_ref().expect("started");
                        let expected = self.expected(model, run);
                        let message = format!("This element is not expected.{expected}");
                        self.element_error(depth, Code::ElementContent, &message);
                        self.unexpected(depth)
                    }
                    Err(StepError::TooComplex) => {
                        self.element_error(
                            depth,
                            Code::VInternal,
                            "The content model needs more counter states than allowed.",
                        );
                        self.unexpected(depth)
                    }
                }
            }
            Some(Content::Simple(_)) => {
                self.element_error(parent, Code::CvcComplexType2_2, "Element content is not allowed, because the content type is a simple type definition.");
                self.unexpected(depth)
            }
            None => {
                self.element_error(
                    parent,
                    Code::CvcType3_1_2,
                    "Element content is not allowed, because the type definition is simple.",
                );
                self.unexpected(depth)
            }
        }
    }

    fn expected(&self, model: &super::model::Model, run: &Run) -> String {
        let names = &self.schema.names;
        let leaves: Vec<String> = model
            .expected(run)
            .into_iter()
            .map(|leaf| match leaf {
                Leaf::Element(name, _) => names.format(*name),
                Leaf::Wildcard(ns, _) => match ns {
                    NsConstraint::Any => "*".into(),
                    NsConstraint::Not(not) => format!("##other{{{}}}*", names.str(*not)),
                    NsConstraint::Set(set) => set
                        .iter()
                        .map(|ns| format!("{{{}}}*", names.str(*ns)))
                        .collect::<Vec<_>>()
                        .join(", "),
                },
            })
            .collect();
        match leaves.len() {
            0 => String::new(),
            1 => format!(" Expected is ( {} ).", leaves[0]),
            _ => format!(" Expected is one of ( {} ).", leaves.join(", ")),
        }
    }

    fn unexpected(&mut self, depth: usize) -> bool {
        self.skip = Some(depth);
        self.frames[depth].not_expected = true;
        self.frames[depth - 1].bad_content = true;
        false
    }

    /// `xmlSchemaValidateElemDecl`: abstract, `xsi:nil`, `xsi:type`, IDCs.
    fn validate_declaration(&mut self, depth: usize, decl: ElemId, attrs: &[Attr<'t>]) -> bool {
        let declaration = &self.schema.elements[decl];
        if declaration.abstract_ {
            self.element_error(depth, Code::CvcElt2, "The element declaration is abstract.");
            return false;
        }
        let mut ty = declaration.ty;
        if let Some(attr) = attrs.iter().find(|a| a.meta == Some(Meta::Nil)) {
            match Type::Boolean.parse(attr.value, None) {
                Err(_) => {
                    let message = format!(
                        "'{}' is not a valid value of the atomic type 'xs:boolean'.",
                        attr.value
                    );
                    self.attribute_error(depth, attr, Code::CvcDatatypeValid1_2_1, &message);
                }
                Ok(value) => {
                    let nil = value.compare(&Type::Boolean.parse("true", None).expect("true"))
                        == crate::schema_types::Comparison::Equal;
                    if !declaration.nillable {
                        self.element_error(
                            depth,
                            Code::CvcElt3_1,
                            "The element is not 'nillable'.",
                        );
                    } else if nil {
                        if declaration.value.as_ref().is_some_and(|v| v.fixed) {
                            self.element_error(depth, Code::CvcElt3_2_2, "The element cannot be 'nilled' because there is a fixed value constraint defined for it.");
                        } else {
                            self.frames[depth].nilled = true;
                        }
                    }
                }
            }
        }
        if let Some(attr) = attrs.iter().find(|a| a.meta == Some(Meta::Type))
            && let Ok(Some(local)) = self.xsi_type(depth, attr, Some(decl))
        {
            self.frames[depth].local_type = true;
            ty = local;
        }
        let idcs = &self.schema.elements[decl].idcs;
        if !idcs.is_empty() {
            let scope = self.scopes.last_mut().expect("scope");
            self.engine.register(idcs, depth, scope);
        }
        self.frames[depth].ty = Some(ty);
        true
    }

    /// `xmlSchemaProcessXSIType`: `Err` for an unusable QName.
    fn xsi_type(
        &mut self,
        depth: usize,
        attr: &Attr<'t>,
        decl: Option<ElemId>,
    ) -> Result<Option<TypeId>, ()> {
        let value = attr.value.trim_matches([' ', '\t', '\n', '\r']);
        let valid = |part: &str| crate::schema_types::Type::NCName.parse(part, None).is_ok();
        let (prefix, local) = match value.split_once(':') {
            Some((prefix, local)) => (Some(prefix), local),
            None => (None, value),
        };
        if !prefix.is_none_or(valid) || !valid(local) {
            let message = format!("'{value}' is not a valid value of the atomic type 'xs:QName'.");
            self.attribute_error(depth, attr, Code::CvcDatatypeValid1_2_1, &message);
            return Err(());
        }
        let node = self.frames[depth].node;
        let scope = namespaces(self.tree, node);
        let ns = scope
            .iter()
            .find(|&&(p, _)| p == prefix.unwrap_or(""))
            .map(|&(_, uri)| uri)
            .filter(|uri| !uri.is_empty());
        if prefix.is_some() && ns.is_none() {
            let message = format!(
                "The QName value '{value}' has no corresponding namespace declaration in scope."
            );
            self.attribute_error(depth, attr, Code::CvcDatatypeValid1_2_1, &message);
            return Err(());
        }
        let name = self.qname(local, ns);
        let found = match (ns, local) {
            (Some(super::parse::XS), "anyType") => Some(ANY_TYPE),
            (Some(super::parse::XS), local) => Type::from_name(local).map(builtin),
            _ => None,
        }
        .or_else(|| self.schema.global_types.get(&name).copied());
        let Some(local_type) = found else {
            let message = format!(
                "The QName value '{}' of the xsi:type attribute does not resolve to a type definition.",
                display(local, ns)
            );
            self.attribute_error(depth, attr, Code::CvcElt4_2, &message);
            return Ok(None);
        };
        let Some(decl) = decl else {
            return Ok(Some(local_type));
        };
        let declaration = &self.schema.elements[decl];
        let decl_type = &self.schema.types[declaration.ty];
        let set = (declaration.block | decl_type.block) & (EXTENSION | RESTRICTION);
        if !derived(&self.schema.types, local_type, declaration.ty, set) {
            let message = format!(
                "The type definition '{}', specified by xsi:type, is blocked or not validly derived from the type definition of the element declaration.",
                display(local, ns)
            );
            self.attribute_error(depth, attr, Code::CvcElt4_3, &message);
            return Ok(None);
        }
        Ok(Some(local_type))
    }

    // ---- attributes ------------------------------------------------------------

    fn scope_value(
        &self,
        depth: usize,
        ty: TypeId,
        lexical: &str,
        attribute: bool,
    ) -> Result<Typed, simple::Failure> {
        let tree = self.tree;
        let node = self.frames[depth].node;
        let load = || namespaces(tree, node);
        let entity = |name: &str| {
            attribute
                && tree
                    .dtd()
                    .is_some_and(|dtd| dtd.entities.get(name).is_some_and(Option::is_some))
        };
        let mut scope = Scope::new(&load, &self.schema.names, &self.schema.notations, &entity);
        scope.instance = true;
        simple::validate(&self.schema.types, &self.schema.names, ty, lexical, &scope)
    }

    /// Registers the ID values of a valid attribute; false on a duplicate.
    fn register_ids(&mut self, node: T::Id, attr: &Attr<'t>, typed: &Typed) -> bool {
        if self.dtd_ids.contains(&(node, attr.index)) {
            return true;
        }
        let tokens: Vec<String> = match typed {
            Typed::Atomic(value) if value.ty() == Type::Id => {
                vec![normalize(attr.value, WhiteSpace::Collapse).into_owned()]
            }
            Typed::List(items) => normalize(attr.value, WhiteSpace::Collapse)
                .split(' ')
                .zip(items)
                .filter(|(_, item)| item.ty() == Type::Id)
                .map(|(token, _)| token.to_owned())
                .collect(),
            Typed::Atomic(_) => Vec::new(),
        };
        tokens.into_iter().all(|token| self.ids.insert(token))
    }

    /// `xmlSchemaVAttributesComplex`.
    fn attributes_complex(&mut self, depth: usize, ty: TypeId, attrs: &[Attr<'t>]) {
        let schema = self.schema;
        let complex = schema.types[ty].complex().expect("complex");
        let mut states: Vec<State> = attrs
            .iter()
            .map(|a| {
                if a.meta.is_some() {
                    State::Meta
                } else {
                    State::Unknown
                }
            })
            .collect();
        let mut missing = Vec::new();
        let mut defaults = Vec::new();
        for (u, use_) in complex.attributes.iter().enumerate() {
            let decl = &schema.attributes[use_.decl];
            match attrs
                .iter()
                .position(|a| a.meta.is_none() && a.name == decl.name && a.name.local != UNKNOWN)
            {
                Some(i) => states[i] = State::Assessed(use_.decl, Some(u)),
                None if use_.required => missing.push(use_.decl),
                None if use_.value.is_some() || decl.value.is_some() => defaults.push(u),
                None => {}
            }
        }
        if let Some(wildcard) = complex.wildcard.map(|w| &schema.wildcards[w]) {
            for (i, attr) in attrs.iter().enumerate() {
                if states[i] != State::Unknown || !wildcard.ns.allows(attr.name.ns) {
                    continue;
                }
                states[i] = match (wildcard.process, schema.global_attributes.get(&attr.name)) {
                    (Process::Skip, _) => State::Skip,
                    (_, Some(&decl)) => State::Assessed(decl, None),
                    (Process::Lax, None) => State::LaxNoDecl,
                    (Process::Strict, None) => State::StrictNoDecl,
                };
            }
        }
        let node = self.frames[depth].node;
        for (i, attr) in attrs.iter().enumerate() {
            let State::Assessed(decl, use_index) = states[i] else {
                continue;
            };
            let resolved = match self.engine.active() {
                true => self
                    .engine
                    .evaluate(attr.local, attr.ns, true, depth, &self.scopes),
                false => 0,
            };
            let declaration = &schema.attributes[decl];
            let use_ = use_index.map(|u| &complex.attributes[u]);
            let constraint = use_
                .and_then(|u| u.value.as_ref())
                .or(declaration.value.as_ref());
            let fixed = constraint.filter(|c| c.fixed);
            let mut key = None;
            match self.scope_value(depth, declaration.ty, attr.value, true) {
                Err(failure) => {
                    for (code, message) in failure {
                        self.attribute_error(depth, attr, code, &message);
                    }
                    states[i] = State::Invalid;
                }
                Ok(typed) => {
                    if !self.register_ids(node, attr, &typed) {
                        let message = format!(
                            "'{}' is not a valid value of the atomic type 'xs:ID'.",
                            attr.value
                        );
                        self.attribute_error(depth, attr, Code::CvcDatatypeValid1_2_1, &message);
                        states[i] = State::Invalid;
                    } else {
                        if let Some(fixed) = fixed
                            && let Some(expected) = &fixed.value
                            && !expected.equals(&typed)
                        {
                            states[i] = State::FixedMismatch(fixed.lexical.clone());
                        }
                        let text = normalize(
                            attr.value,
                            simple::whitespace(&schema.types, declaration.ty),
                        )
                        .into_owned();
                        key = Some(Key { value: typed, text });
                    }
                }
            }
            if resolved > 0 {
                self.engine.process(
                    depth + 1,
                    node,
                    FieldValue::Simple(key.as_ref()),
                    &mut self.scopes,
                    &mut |n, c, m| {
                        self.out.push(Diagnostic {
                            code: c,
                            node: Some(n),
                            offset: self.tree.offset(n),
                            message: m,
                        })
                    },
                );
            } else if self.engine.active() {
                self.engine.pop();
            }
        }
        for &u in &defaults {
            if !self.engine.active() {
                break;
            }
            let use_ = &complex.attributes[u];
            let declaration = &schema.attributes[use_.decl];
            let local = schema.names.str(declaration.name.local);
            let ns = Some(schema.names.str(declaration.name.ns)).filter(|ns| !ns.is_empty());
            if self.engine.evaluate(local, ns, true, depth, &self.scopes) > 0 {
                let constraint = use_
                    .value
                    .as_ref()
                    .or(declaration.value.as_ref())
                    .expect("default");
                let key = constraint.value.clone().map(|value| Key {
                    value,
                    text: constraint.lexical.to_string(),
                });
                if key.is_none() {
                    self.element_error(
                        depth,
                        Code::VInternal,
                        "The default/fixed value on an attribute use was not precomputed.",
                    );
                }
                self.engine.process(
                    depth + 1,
                    node,
                    FieldValue::Simple(key.as_ref()),
                    &mut self.scopes,
                    &mut |n, c, m| {
                        self.out.push(Diagnostic {
                            code: c,
                            node: Some(n),
                            offset: self.tree.offset(n),
                            message: m,
                        })
                    },
                );
            } else {
                self.engine.pop();
            }
        }
        for (i, attr) in attrs.iter().enumerate() {
            match &states[i] {
                State::FixedMismatch(expected) => {
                    let message = format!("The value '{}' does not match the fixed value constraint '{expected}'.", attr.value);
                    self.attribute_error(depth, attr, Code::CvcAu, &message);
                }
                State::StrictNoDecl => self.attribute_error(
                    depth,
                    attr,
                    Code::CvcWildcard,
                    "No matching global attribute declaration available, but demanded by the strict wildcard.",
                ),
                State::Unknown => {
                    let code = match complex.wildcard {
                        None => Code::CvcComplexType3_2_1,
                        Some(_) => Code::CvcComplexType3_2_2,
                    };
                    let message = format!("The attribute '{}' is not allowed.", display(attr.local, attr.ns));
                    self.attribute_error(depth, attr, code, &message);
                }
                _ => {}
            }
        }
        for decl in missing {
            let message = format!(
                "The attribute '{}' is required but missing.",
                schema.names.format(schema.attributes[decl].name)
            );
            self.element_error(depth, Code::CvcComplexType4, &message);
        }
    }

    // ---- character content -----------------------------------------------------

    /// `xmlSchemaVPushText`.
    fn text(&mut self, node: T::Id, text: &'t str) {
        let Some(depth) = self.frames.len().checked_sub(1) else {
            return;
        };
        self.frames[depth].empty = false;
        let frame = &self.frames[depth];
        if frame.nilled {
            return self.element_error(
                depth,
                Code::CvcElt3_2_1,
                "Neither character nor element content is allowed because the element is 'nilled'.",
            );
        }
        let Some(ty) = frame.ty else { return };
        let content = self.schema.types[ty].complex().map(|c| &c.content);
        match content {
            Some(Content::Empty) => self.element_error(
                depth,
                Code::CvcComplexType2_1,
                "Character content is not allowed, because the content type is empty.",
            ),
            Some(Content::Elements { mixed: false, .. }) => {
                let blank = text
                    .bytes()
                    .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'));
                if self.tree.is_cdata(node) || !blank {
                    self.element_error(
                        depth,
                        Code::CvcComplexType2_3,
                        "Character content other than whitespace is not allowed because the content type is 'element-only'.",
                    );
                }
            }
            Some(Content::Elements { mixed: true, .. })
                if frame
                    .decl
                    .is_none_or(|d| self.schema.elements[d].value.is_none()) => {}
            _ => self.frames[depth].text.push_str(text),
        }
    }

    /// Validates character content against `ty` (`xmlSchemaVCheckINodeDataType`).
    fn check_value(&mut self, depth: usize, ty: TypeId, lexical: &str) -> bool {
        match self.scope_value(depth, ty, lexical, false) {
            Ok(typed) => {
                if self.frames[depth].value_needed {
                    let text =
                        normalize(lexical, simple::whitespace(&self.schema.types, ty)).into_owned();
                    self.frames[depth].value = Some(Key { value: typed, text });
                }
                true
            }
            Err(failure) => {
                for (code, message) in failure {
                    self.element_error(depth, code, &message);
                }
                false
            }
        }
    }

    /// `xmlSchemaValidatorPopElem`, then the identity constraints.
    fn leave(&mut self) {
        let depth = self.frames.len() - 1;
        self.pop_content(depth);
        if self.skip == Some(depth) {
            self.skip = None;
        }
        let frame = &self.frames[depth];
        let node = frame.node;
        if frame.applied_xpath {
            let value = match frame.ty.map(|ty| &self.schema.types[ty].kind) {
                Some(
                    Kind::Complex(ComplexType {
                        content: Content::Simple(_),
                        ..
                    })
                    | Kind::Simple(_),
                ) => FieldValue::Simple(frame.value.as_ref()),
                _ => FieldValue::NotSimple,
            };
            let out = &mut self.out;
            let tree = self.tree;
            self.engine
                .process(depth, node, value, &mut self.scopes, &mut |n, c, m| {
                    out.push(Diagnostic {
                        code: c,
                        node: Some(n),
                        offset: tree.offset(n),
                        message: m,
                    })
                });
        }
        let out = &mut self.out;
        let tree = self.tree;
        self.engine.finish(depth, &mut self.scopes, &mut |n, c, m| {
            out.push(Diagnostic {
                code: c,
                node: Some(n),
                offset: tree.offset(n),
                message: m,
            })
        });
        self.frames.pop();
    }

    fn pop_content(&mut self, depth: usize) {
        let frame = &self.frames[depth];
        if frame.not_expected {
            self.skip = Some(depth - 1);
            return;
        }
        let Some(ty) = frame.ty.filter(|_| !frame.bad_type) else {
            return;
        };
        let schema = self.schema;
        let content = schema.types[ty].complex().map(|c| &c.content);
        if let Some(Content::Elements { model, mixed }) = content
            && ty != ANY_TYPE
        {
            if !frame.bad_content && !frame.nilled {
                let run = frame.run.clone().unwrap_or_else(|| model.start());
                if !model.accepts(&run) {
                    let expected = self.expected(model, &run);
                    self.frames[depth].bad_content = true;
                    self.element_error(
                        depth,
                        Code::ElementContent,
                        &format!("Missing child element(s).{expected}"),
                    );
                }
            }
            if !mixed {
                return;
            }
        }
        let frame = &self.frames[depth];
        let simple_type = match content {
            None => Some(ty),
            Some(Content::Simple(ct)) => Some(*ct),
            _ => None,
        };
        let Some(decl) = frame.decl else {
            if let Some(st) = simple_type {
                let text = std::mem::take(&mut self.frames[depth].text);
                self.check_value(depth, st, &text);
            }
            return;
        };
        let declaration = &schema.elements[decl];
        let constraint = declaration.value.as_ref();
        if let Some(constraint) = constraint.filter(|_| frame.empty && !frame.nilled) {
            if frame.local_type {
                let mixed_emptiable =
                    matches!(content, Some(Content::Elements { mixed: true, .. }));
                if simple_type.is_none() && !mixed_emptiable {
                    self.element_error(depth, Code::CosValidDefault2_1, "For a string to be a valid default, the type definition must be a simple type or a complex type with simple content or mixed content and a particle emptiable.");
                    return;
                }
            }
            if let Some(st) = simple_type {
                let lexical = constraint.lexical.clone();
                self.check_value(depth, st, &lexical);
            }
            return;
        }
        if frame.nilled {
            return;
        }
        let text = std::mem::take(&mut self.frames[depth].text);
        if let Some(st) = simple_type
            && !self.check_value(depth, st, &text)
        {
            return;
        }
        let Some(constraint) = constraint.filter(|c| c.fixed) else {
            return;
        };
        if self.frames[depth].element_content {
            self.element_error(depth, Code::CvcElt5_2_2_1, "The content must not contain element nodes since there is a fixed value constraint.");
        } else if matches!(content, Some(Content::Elements { mixed: true, .. })) {
            if *text != *constraint.lexical {
                let message = format!(
                    "The initial value '{text}' does not match the fixed value constraint '{}'.",
                    constraint.lexical
                );
                self.element_error(depth, Code::CvcElt5_2_2_2_1, &message);
            }
        } else if simple_type.is_some() && *text != *constraint.lexical {
            let message = format!(
                "The actual value '{text}' does not match the fixed value constraint '{}'.",
                constraint.lexical
            );
            self.element_error(depth, Code::CvcElt5_2_2_2_2, &message);
        }
    }
}

/// Whether `node` is an element the walk never entered (inside a skipped
/// subtree); such elements are left without popping a frame.
fn skipped_element<T: Tree>(validator: &Validator<'_, '_, T>, node: T::Id) -> bool {
    validator
        .frames
        .last()
        .is_none_or(|frame| frame.node != node)
}
