//! Identity constraints evaluated while walking the instance, as libxml2
//! does: each `unique`, `key` or `keyref` on an element starts a streaming
//! selector match there; selected nodes start field matches; key sequences
//! are checked for duplicates when a selected node ends, and node tables
//! bubble up to the elements whose keyrefs refer to them.

use std::collections::HashMap;

use crate::pattern::StreamMatcher;

use super::Code;
use super::Schema;
use super::components::{IdcId, IdcKind, Typed};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Role {
    Selector,
    Field(usize),
}

/// An active XPath state object (`xmlSchemaIDCStateObj`).
struct State<'s> {
    role: Role,
    /// The depth of the element where it started.
    depth: usize,
    /// Depths of the matches still to process.
    history: Vec<usize>,
    /// The matcher: its scope's depth and index.
    matcher: (usize, usize),
    stream: StreamMatcher<'s>,
}

/// A key: the computed value and its normalized text for messages.
#[derive(Clone, Debug)]
pub(crate) struct Key {
    pub(crate) value: Typed,
    pub(crate) text: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Target<Id> {
    node: Id,
    keys: Vec<Key>,
}

/// One identity constraint being evaluated on one element.
#[derive(Debug)]
pub(crate) struct Matcher<Id> {
    idc: IdcId,
    depth: usize,
    /// Key sequences under construction by target depth relative to `depth`.
    pending: Vec<Option<Vec<Option<Key>>>>,
    targets: Vec<Target<Id>>,
    index: HashMap<u64, Vec<usize>>,
}

/// A node table (`xmlSchemaPSVIIDCBinding`).
#[derive(Debug)]
pub(crate) struct Binding<Id> {
    idc: IdcId,
    nodes: Vec<Target<Id>>,
    dupls: Vec<Target<Id>>,
}

/// The identity-constraint state of one element.
#[derive(Debug)]
pub(crate) struct Scope<Id> {
    matchers: Vec<Matcher<Id>>,
    bindings: Vec<Binding<Id>>,
    pub(crate) has_keyrefs: bool,
}

impl<Id> Default for Scope<Id> {
    fn default() -> Self {
        Self {
            matchers: Vec::new(),
            bindings: Vec::new(),
            has_keyrefs: false,
        }
    }
}

/// What a matched field node offers.
pub(crate) enum FieldValue<'a> {
    /// A simple-typed node with its value, if it was valid.
    Simple(Option<&'a Key>),
    /// An element without simple content.
    NotSimple,
}

fn sequence_hash(keys: &[Key]) -> u64 {
    keys.iter().fold(0xcbf2_9ce4_8422_2325, |hash, key| {
        (hash ^ key.value.hash_key()).wrapping_mul(0x100_0000_01b3)
    })
}

fn equal(a: &[Key], b: &[Key]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.value.equals(&b.value))
}

fn format_sequence(keys: &[Key]) -> String {
    let items: Vec<String> = keys.iter().map(|k| format!("'{}'", k.text)).collect();
    format!("[{}]", items.join(", "))
}

pub(crate) struct Engine<'s> {
    schema: &'s Schema,
    states: Vec<State<'s>>,
    /// Per IDC, the depth of the topmost keyref that refers to it.
    keyref_depth: Vec<Option<usize>>,
    /// Whether the schema has keyrefs at all (node tables are built only then).
    keyrefs: bool,
    /// Key sequences kept so far, up to the schema's `max_idc_keys`; once
    /// over, keys are dropped and keyrefs no longer checked.
    keys: usize,
}

impl<'s> Engine<'s> {
    pub(crate) fn new(schema: &'s Schema) -> Self {
        Self {
            schema,
            states: Vec::new(),
            keyref_depth: vec![None; schema.idcs.len()],
            keyrefs: schema.idcs.iter().any(|idc| idc.kind == IdcKind::KeyRef),
            keys: 0,
        }
    }

    pub(crate) fn active(&self) -> bool {
        !self.states.is_empty()
    }

    /// `xmlSchemaIDCRegisterMatchers` for an element at `depth`.
    pub(crate) fn register<Id>(&mut self, idcs: &[IdcId], depth: usize, scope: &mut Scope<Id>) {
        for &idc in idcs {
            let definition = &self.schema.idcs[idc];
            if let Some(refer) = definition.refer {
                scope.has_keyrefs = true;
                let slot = &mut self.keyref_depth[refer];
                if slot.is_none_or(|d| depth < d) {
                    *slot = Some(depth);
                }
            }
            let Some(stream) = definition.selector.stream() else {
                continue;
            };
            scope.matchers.push(Matcher {
                idc,
                depth,
                pending: Vec::new(),
                targets: Vec::new(),
                index: HashMap::new(),
            });
            self.states.push(State {
                role: Role::Selector,
                depth,
                history: Vec::new(),
                matcher: (depth, scope.matchers.len() - 1),
                stream,
            });
        }
    }

    /// `xmlSchemaXPathEvaluate`: pushes a node to every state object;
    /// returns how many fields matched it.
    pub(crate) fn evaluate<Id>(
        &mut self,
        local: &str,
        ns: Option<&str>,
        attribute: bool,
        depth: usize,
        scopes: &[Scope<Id>],
    ) -> usize {
        let match_depth = depth + usize::from(attribute);
        let mut resolved = 0;
        let mut i = 0;
        while i < self.states.len() {
            let state = &mut self.states[i];
            let matched = match attribute {
                true => state.stream.push_attr(local, ns),
                false => state.stream.push(local, ns),
            };
            i += 1;
            if matched != Ok(true) {
                continue;
            }
            state.history.push(match_depth);
            match state.role {
                Role::Field(_) => resolved += 1,
                Role::Selector => {
                    let matcher = state.matcher;
                    let idc = scopes[matcher.0].matchers[matcher.1].idc;
                    for (field, (_, pattern)) in self.schema.idcs[idc].fields.iter().enumerate() {
                        if let Some(stream) = pattern.stream() {
                            self.states.push(State {
                                role: Role::Field(field),
                                depth,
                                history: Vec::new(),
                                matcher,
                                stream,
                            });
                        }
                    }
                }
            }
        }
        resolved
    }

    /// `xmlSchemaXPathPop`: an attribute without field matches ends.
    pub(crate) fn pop(&mut self) {
        for state in &mut self.states {
            state.stream.pop();
        }
    }

    fn designation(&self, idc: IdcId) -> String {
        let definition = &self.schema.idcs[idc];
        let kind = match definition.kind {
            IdcKind::Unique => "unique",
            IdcKind::Key => "key",
            IdcKind::KeyRef => "keyref",
        };
        format!(
            "{kind} identity-constraint '{}'",
            self.schema.names.format(definition.name)
        )
    }

    /// `xmlSchemaXPathProcessHistory`: the node at `depth` ends.
    pub(crate) fn process<Id: Copy>(
        &mut self,
        depth: usize,
        node: Id,
        value: FieldValue<'_>,
        scopes: &mut [Scope<Id>],
        report: &mut dyn FnMut(Id, Code, String),
    ) {
        let mut remove = Vec::new();
        for i in (0..self.states.len()).rev() {
            let state = &mut self.states[i];
            state.stream.pop();
            if let Some(&last) = state.history.last() {
                if last != depth {
                    continue;
                }
                let (scope, index) = state.matcher;
                let role = state.role;
                let start = state.depth;
                state.history.pop();
                let matcher = &mut scopes[scope].matchers[index];
                let idc = matcher.idc;
                let definition = &self.schema.idcs[idc];
                match role {
                    Role::Field(field) => {
                        let xpath = &definition.fields[field].0;
                        match &value {
                            FieldValue::NotSimple => report(
                                node,
                                Code::CvcIdc,
                                format!(
                                    "The XPath '{xpath}' of a field of {} does evaluate to a node of non-simple type.",
                                    self.designation(idc)
                                ),
                            ),
                            FieldValue::Simple(None) => report(
                                node,
                                Code::CvcIdc,
                                "Warning: No precomputed value available, the value was either invalid or something strange happened.".into(),
                            ),
                            FieldValue::Simple(Some(key)) => {
                                let pos = start - matcher.depth;
                                if matcher.pending.len() <= pos {
                                    matcher.pending.resize(pos + 1, None);
                                }
                                let count = definition.fields.len();
                                let sequence = matcher.pending[pos].get_or_insert_with(|| vec![None; count]);
                                if sequence[field].is_some() {
                                    report(
                                        node,
                                        Code::CvcIdc,
                                        format!(
                                            "The XPath '{xpath}' of a field of {} evaluates to a node-set with more than one member.",
                                            self.designation(idc)
                                        ),
                                    );
                                } else {
                                    sequence[field] = Some((*key).clone());
                                }
                            }
                        }
                    }
                    Role::Selector => {
                        let pos = depth - matcher.depth;
                        let sequence = matcher.pending.get_mut(pos).and_then(Option::take);
                        let complete: Option<Vec<Key>> =
                            sequence.and_then(|s| s.into_iter().collect());
                        match complete {
                            None if definition.kind == IdcKind::Key => report(
                                node,
                                Code::CvcIdc,
                                format!(
                                    "Not all fields of {} evaluate to a node.",
                                    self.designation(idc)
                                ),
                            ),
                            None => {}
                            Some(keys) => {
                                let hash = sequence_hash(&keys);
                                let duplicate = definition.kind != IdcKind::KeyRef
                                    && matcher.index.get(&hash).is_some_and(|hits| {
                                        hits.iter().any(|&t| equal(&matcher.targets[t].keys, &keys))
                                    });
                                if duplicate {
                                    report(
                                        node,
                                        Code::CvcIdc,
                                        format!(
                                            "Duplicate key-sequence {} in {}.",
                                            format_sequence(&keys),
                                            self.designation(idc)
                                        ),
                                    );
                                } else if self.keys == self.schema.max_idc_keys {
                                    self.keys += 1;
                                    report(
                                        node,
                                        Code::VInternal,
                                        "Too many identity-constraint key sequences.".into(),
                                    );
                                } else if self.keys < self.schema.max_idc_keys {
                                    self.keys += 1;
                                    if definition.kind != IdcKind::KeyRef {
                                        matcher
                                            .index
                                            .entry(hash)
                                            .or_default()
                                            .push(matcher.targets.len());
                                    }
                                    matcher.targets.push(Target { node, keys });
                                }
                            }
                        }
                    }
                }
            }
            let state = &self.states[i];
            if state.history.is_empty() && state.depth == depth {
                remove.push(i);
            }
        }
        for i in remove {
            self.states.remove(i);
        }
    }

    /// `xmlSchemaIDCFillNodeTables`, the keyref check and
    /// `xmlSchemaBubbleIDCNodeTables` for the element at `depth` ending.
    pub(crate) fn finish<Id: Copy>(
        &mut self,
        depth: usize,
        scopes: &mut Vec<Scope<Id>>,
        report: &mut dyn FnMut(Id, Code, String),
    ) {
        let Some(mut scope) = scopes.pop() else {
            return;
        };
        if self.keyrefs {
            for matcher in std::mem::take(&mut scope.matchers) {
                let kind = self.schema.idcs[matcher.idc].kind;
                if kind == IdcKind::KeyRef {
                    scope.matchers.push(matcher);
                    continue;
                }
                if matcher.targets.is_empty()
                    || self.keyref_depth[matcher.idc].is_none_or(|d| d > depth)
                {
                    continue;
                }
                let binding = binding(&mut scope.bindings, matcher.idc);
                if binding.nodes.is_empty() && binding.dupls.is_empty() {
                    binding.nodes = matcher.targets;
                    continue;
                }
                for target in matcher.targets {
                    if binding.dupls.iter().any(|d| equal(&d.keys, &target.keys)) {
                        continue;
                    }
                    match binding
                        .nodes
                        .iter()
                        .position(|n| equal(&n.keys, &target.keys))
                    {
                        Some(j) => {
                            let duplicate = binding.nodes.swap_remove(j);
                            binding.dupls.push(duplicate);
                        }
                        None => binding.nodes.push(target),
                    }
                }
            }
        }
        if scope.has_keyrefs && self.keys <= self.schema.max_idc_keys {
            for matcher in &scope.matchers {
                let definition = &self.schema.idcs[matcher.idc];
                let (IdcKind::KeyRef, Some(refer)) = (definition.kind, definition.refer) else {
                    continue;
                };
                let table = scope.bindings.iter().find(|b| b.idc == refer);
                let index: HashMap<u64, Vec<usize>> = table.map_or_else(HashMap::new, |b| {
                    let mut index: HashMap<u64, Vec<usize>> = HashMap::new();
                    for (i, node) in b.nodes.iter().enumerate() {
                        index.entry(sequence_hash(&node.keys)).or_default().push(i);
                    }
                    index
                });
                for target in &matcher.targets {
                    let found = table.is_some_and(|b| {
                        index.get(&sequence_hash(&target.keys)).is_some_and(|hits| {
                            hits.iter().any(|&i| equal(&b.nodes[i].keys, &target.keys))
                        })
                    });
                    if found {
                        continue;
                    }
                    let name = self.schema.names.format(definition.name);
                    let sequence = format_sequence(&target.keys);
                    let in_dupls =
                        table.is_some_and(|b| b.dupls.iter().any(|d| equal(&d.keys, &target.keys)));
                    let message = match in_dupls {
                        true => format!(
                            "More than one match found for key-sequence {sequence} of keyref '{name}'."
                        ),
                        false => format!(
                            "No match found for key-sequence {sequence} of keyref '{name}'."
                        ),
                    };
                    report(target.node, Code::CvcIdc, message);
                }
            }
        }
        if let Some(parent) = scopes.last_mut()
            && depth > 0
        {
            for child in scope.bindings {
                if (child.nodes.is_empty() && child.dupls.is_empty())
                    || self.keyref_depth[child.idc].is_none_or(|d| d >= depth)
                {
                    continue;
                }
                match parent.bindings.iter_mut().find(|b| b.idc == child.idc) {
                    None => parent.bindings.push(child),
                    Some(target) => {
                        let mut old = target.nodes.len();
                        let old_dupls = target.dupls.len();
                        for node in child.nodes {
                            if target.dupls[..old_dupls]
                                .iter()
                                .any(|d| equal(&d.keys, &node.keys))
                            {
                                continue;
                            }
                            match target.nodes[..old]
                                .iter()
                                .position(|n| equal(&n.keys, &node.keys))
                            {
                                Some(j) => {
                                    old -= 1;
                                    let duplicate = target.nodes.remove(j);
                                    target.dupls.push(duplicate);
                                }
                                None => target.nodes.push(node),
                            }
                        }
                    }
                }
            }
        }
        for slot in &mut self.keyref_depth {
            if *slot == Some(depth) && depth > 0 {
                *slot = None;
            }
        }
    }
}

fn binding<Id>(bindings: &mut Vec<Binding<Id>>, idc: IdcId) -> &mut Binding<Id> {
    match bindings.iter().position(|b| b.idc == idc) {
        Some(i) => &mut bindings[i],
        None => {
            bindings.push(Binding {
                idc,
                nodes: Vec::new(),
                dupls: Vec::new(),
            });
            bindings.last_mut().expect("pushed")
        }
    }
}
