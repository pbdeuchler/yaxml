//! Copies: `xmlDocCopyNode` across documents ([`Editor::import`]),
//! `xmlCopyNode` within one ([`Editor::copy`]) and `xmlCopyDoc`
//! ([`OwnedDocument::try_clone`]); and content parsed in a node's context
//! ([`Editor::parse_content`]), which is a copy of the nodes parsed into a
//! scratch document.
//!
//! A copy is staged from the source without changing anything, then appended
//! to the destination's arenas as a detached subtree and reconciled there:
//! each binding a name of the copy took from outside it is declared on the
//! copy's root, as `xmlDocCopyNode` declares it, unless that would need more
//! declarations than declaring each where it is needed. A failure truncates
//! the arenas back, so nothing is published.
//!
//! The destination decides what the copy means: its DTD types IDs, an entity
//! reference it does not declare is replaced by the reference's content (and
//! an attribute's kept references by its substituted value), and an HTML
//! destination takes no namespaces, an XML one no HTML-only names.

use super::construction::{Work, allocation_error, limit_global};
use super::edit::{Declare, reserve_attributes};
use super::*;

/// A copy numbered as it will be appended: `nodes[i]` is slot `base + i` and
/// `attributes[j]` attribute `attribute_base + j`. Pool ranges still index
/// the source's pool; `bytes` is what copying them takes.
#[derive(Default)]
struct Staged {
    nodes: Vec<OwnedRecord>,
    attributes: Vec<StoredAttribute>,
    kept: Vec<(usize, Range<usize>)>,
    valueless: Vec<usize>,
    /// `(slot, position)` of attributes the destination types ID.
    ids: Vec<(usize, usize)>,
    bytes: usize,
}

/// Where staging copies to and what it may create there.
struct Target<'t, A: Allocator> {
    document: &'t OwnedDocument<A>,
    /// References resolve in the destination as in the source: it is the
    /// destination, or was parsed in its context.
    same: bool,
    limits: &'t EditLimits,
}

fn push<T>(items: &mut Vec<T>, item: T) -> Result<(), EditError> {
    #[cfg(test)]
    if items.len() == items.capacity() {
        inject_edit_reservation_failure(EditReservation::Scratch)?;
    }
    items.try_reserve(1).map_err(|_| allocation_error())?;
    items.push(item);
    Ok(())
}

impl<A: Allocator> OwnedDocument<A> {
    /// Whether the DTD declares the parsed general entity `name`.
    fn declares_entity(&self, name: &str) -> bool {
        self.dtd()
            .and_then(|dtd| dtd.entity(name))
            .is_some_and(|entity| {
                entity.notation.is_none() && entity.kind != crate::valid::EntityKind::Predefined
            })
    }

    /// One declaration on `root` per prefix that `plan` binds to a single
    /// URI, in first-use order: `xmlDocCopyNode`'s placement.
    fn hoisted(
        &self,
        plan: &[Declare],
        root: usize,
        work: &mut Work,
    ) -> Result<Vec<Declare>, EditError> {
        let pool = &self.pool.0;
        let text = |range: &Option<Range<usize>>| range.clone().map(|range| &pool[range]);
        let mut seed: Vec<Declare> = Vec::new();
        for declare in plan {
            work.charge(plan.len() + seed.len())?;
            let prefix = text(&declare.prefix);
            let same = |other: &&Declare| text(&other.prefix) == prefix;
            let agreed = plan
                .iter()
                .filter(same)
                .all(|other| text(&other.uri) == text(&declare.uri));
            if agreed && !seed.iter().any(|seeded| same(&seeded)) {
                push(
                    &mut seed,
                    Declare {
                        element: root,
                        prefix: declare.prefix.clone(),
                        uri: declare.uri.clone(),
                    },
                )?;
            }
        }
        Ok(seed)
    }
}

impl<A: Allocator> Target<'_, A> {
    /// An entity reference the destination cannot resolve is replaced by its content.
    fn expands<B: Allocator>(&self, node: OwnedNode<'_, B>) -> bool {
        !self.same
            && node
                .entity_name()
                .is_some_and(|name| !self.document.declares_entity(name))
    }

    /// A kept attribute form survives when each `&name;` in it resolves.
    fn keeps(&self, form: &str) -> bool {
        self.same
            || form.split('&').skip(1).all(|part| {
                let name = part.split_once(';').map_or(part, |(name, _)| name);
                name.starts_with('#') || self.document.declares_entity(name)
            })
    }

    /// Kind compatibility: an HTML destination takes no namespaces, an XML
    /// one nothing [`xml_problem`] rejects.
    fn check<B: Allocator>(&self, node: OwnedNode<'_, B>) -> Result<(), EditError> {
        match (node.document.is_html(), self.document.is_html()) {
            (false, true) => {
                let namespaced = node.name().is_some_and(|name| name.namespace().is_some());
                if namespaced || node.attributes().any(|a| a.name.namespace().is_some()) {
                    return Err(EditError::new(EditErrorKind::Namespace, None));
                }
            }
            (true, false) => {
                if let Some((kind, _)) = xml_problem(node) {
                    return Err(EditError::new(kind, None));
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl Staged {
    /// Appends a copy of `node`'s record under `parent` (an absolute slot)
    /// and returns its slot; an element's attributes come separately.
    fn node<A: Allocator, B: Allocator>(
        &mut self,
        node: OwnedNode<'_, B>,
        parent: Option<usize>,
        target: &Target<'_, A>,
    ) -> Result<usize, EditError> {
        let document = target.document;
        if document.edit_created_nodes + self.nodes.len() >= target.limits.max_created_nodes {
            return Err(limit_global(EditLimit::CreatedNodes));
        }
        let base = document.nodes.len();
        let slot = base + self.nodes.len();
        let mut content = match &node.record().content {
            Content::Element { name, .. } => Content::Element {
                name: name.clone(),
                attributes: 0..0,
            },
            content => content.clone(),
        };
        content.visit_strings(&mut |range| self.bytes += range.len());
        let mut record = OwnedRecord::detached(NodeId(slot), content);
        if let Some(parent) = parent {
            let parent = &mut self.nodes[parent - base];
            record.parent = Link::new(parent.id);
            record.previous_sibling = parent.last_child;
            parent.last_child = Link::new(NodeId(slot));
            if record.previous_sibling == Link::NONE {
                parent.first_child = parent.last_child;
            }
            if let Some(previous) = record.previous_sibling.get() {
                self.nodes[previous.0 - base].next_sibling = Link::new(NodeId(slot));
            }
        }
        push(&mut self.nodes, record)?;
        Ok(slot)
    }

    /// Copies `element`'s attributes for the staged element at `slot`, with
    /// the kept forms, valueless flags and IDs the destination keeps.
    fn attributes<A: Allocator, B: Allocator>(
        &mut self,
        element: OwnedNode<'_, B>,
        slot: usize,
        target: &Target<'_, A>,
    ) -> Result<(), EditError> {
        let (source, document) = (element.document, target.document);
        let Content::Element { attributes, .. } = &element.record().content else {
            return Ok(());
        };
        let created = document.edit_created_attributes + self.attributes.len() + attributes.len();
        if created > target.limits.max_created_attributes {
            return Err(limit_global(EditLimit::CreatedAttributes));
        }
        let start = document.attributes.len() + self.attributes.len();
        let name = element.qualified_name().unwrap_or_default();
        for (position, index) in attributes.clone().enumerate() {
            let at = start + position;
            let mut attribute = source.attributes[index].clone();
            attribute.visit_strings(&mut |range| self.bytes += range.len());
            if let Ok(found) = source.kept.binary_search_by_key(&index, |entry| entry.0) {
                let form = source.kept[found].1.clone();
                if target.keeps(&source.pool.0[form.clone()]) {
                    self.bytes += form.len();
                    push(&mut self.kept, (at, form))?;
                }
            }
            let valued = element.attribute_has_value(position);
            if !valued && document.is_html() {
                push(&mut self.valueless, at)?;
            }
            let qualified = &source.pool.0[attribute.name.qualified.clone()];
            if document.is_id(name, qualified) && (valued || !document.is_html()) {
                push(&mut self.ids, (slot, position))?;
            }
            push(&mut self.attributes, attribute)?;
        }
        let range = start..document.attributes.len() + self.attributes.len();
        let base = document.nodes.len();
        if let Content::Element { attributes, .. } = &mut self.nodes[slot - base].content {
            *attributes = range;
        }
        Ok(())
    }
}

/// Stages a copy of `root` (deep: with its descendants) for `target`, after
/// what `staged` holds. The root's attributes are staged last, so that
/// declarations added to it extend the attribute arena's tail in place.
fn stage<A: Allocator, B: Allocator>(
    source: &OwnedDocument<B>,
    root: OwnedNodeId,
    target: &Target<'_, A>,
    deep: bool,
    mut staged: Staged,
) -> Result<Staged, EditError> {
    let invalid = EditError::new(EditErrorKind::InvalidHandle, None);
    let root = source.node(root).ok_or(invalid)?;
    match root.kind() {
        OwnedNodeKind::Document | OwnedNodeKind::DocumentType => {
            return Err(EditError::new(EditErrorKind::WrongNodeKind, None));
        }
        _ if target.expands(root) => return Err(EditError::new(EditErrorKind::Hierarchy, None)),
        _ => {}
    }
    // A reference's content is part of it, even in a shallow copy.
    let deep = deep || root.kind() == OwnedNodeKind::EntityRef;
    let base = target.document.nodes.len();
    let first = base + staged.nodes.len();
    let (mut node, mut parent, mut depth) = (root, None, 0);
    loop {
        target.check(node)?;
        let element = node.kind() == OwnedNodeKind::Element;
        depth += usize::from(element);
        if depth > target.limits.max_depth {
            return Err(limit_global(EditLimit::Depth));
        }
        let slot = match target.expands(node) {
            true => parent,
            false => Some(staged.node(node, parent, target)?),
        };
        if element && node.id() != root.id() {
            staged.attributes(node, slot.expect("elements are copied"), target)?;
        }
        if let Some(child) = node.first_child().filter(|_| deep) {
            (node, parent) = (child, slot);
            continue;
        }
        // Leave nodes until one has a next sibling; a copied parent's
        // subtree is complete when it is left.
        loop {
            depth -= usize::from(node.kind() == OwnedNodeKind::Element);
            if node.id() == root.id() {
                staged.attributes(root, first, target)?;
                staged.ids.sort_unstable();
                return Ok(staged);
            }
            if let Some(next) = node.next_sibling() {
                node = next;
                break;
            }
            node = node.parent().expect("a subtree node has a parent");
            if !target.expands(node) {
                let (slot, end) = (parent.expect("a copied parent"), base + staged.nodes.len());
                let record = &mut staged.nodes[slot - base];
                record.subtree_size = end - slot;
                parent = record.parent.get().map(|id| id.0);
            }
        }
    }
}

impl<A: Allocator> Editor<'_, A> {
    /// Copies `node` of another document into this one, detached
    /// (`xmlDocCopyNode`); `deep` includes descendants, otherwise an element
    /// keeps only its attributes. Insert the copy with
    /// [`append_child`](Self::append_child) or another move. Bindings it
    /// needs are declared on its root.
    ///
    /// This document's DTD decides IDs; an entity reference it does not
    /// declare is replaced by its content.
    ///
    /// # Errors
    ///
    /// The document node and DOCTYPE are `WrongNodeKind` (use
    /// [`OwnedDocument::try_clone`]); an HTML destination rejects namespaced
    /// names (`Namespace`), and an XML one what XML cannot write
    /// (`InvalidName`, `InvalidCharacter`, `InvalidValue`).
    ///
    /// ```
    /// use yaxml::{EditLimits, OwnedDocument, save};
    /// let source = OwnedDocument::parse("<r xmlns:p='urn:p'><p:a><p:b/></p:a></r>")?;
    /// let a = source.root_element().first_child().unwrap().id();
    /// let mut document = OwnedDocument::parse("<t/>")?;
    /// let t = document.root_element().id();
    /// let mut editor = document.edit(EditLimits::default())?;
    /// let copy = editor.import(&source, a, true)?;
    /// editor.append_child(t, copy)?;
    /// let saved = String::from_utf8(save::to_vec(&document, &save::Options::default())?)?;
    /// assert!(saved.ends_with("<t><p:a xmlns:p=\"urn:p\"><p:b/></p:a></t>\n"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// To move a node between documents (`xmlDOMWrapAdoptNode`), import it
    /// and `remove_subtree` it from the source.
    pub fn import<B: Allocator>(
        &mut self,
        source: &OwnedDocument<B>,
        node: OwnedNodeId,
        deep: bool,
    ) -> Result<OwnedNodeId, EditError> {
        let target = Target {
            document: self.document,
            same: false,
            limits: &self.limits,
        };
        let staged = stage(source, node, &target, deep, Staged::default())?;
        self.publish(staged, Some(&source.pool.0), Self::reconcile_copy)
    }

    /// [`import`](Self::import) from this document (`xmlCopyNode`). The copy
    /// shares the source's strings.
    pub fn copy(&mut self, node: OwnedNodeId, deep: bool) -> Result<OwnedNodeId, EditError> {
        let target = Target {
            document: self.document,
            same: true,
            limits: &self.limits,
        };
        let staged = stage(target.document, node, &target, deep, Staged::default())?;
        self.publish(staged, None, Self::reconcile_copy)
    }

    /// Reserves, appends and `reconcile`s (given its first slot) a staged
    /// copy; `pool` is the source's when its strings must be copied. A
    /// failure truncates every store back.
    fn publish(
        &mut self,
        staged: Staged,
        pool: Option<&str>,
        reconcile: impl FnOnce(&mut Self, usize) -> Result<(), EditError>,
    ) -> Result<OwnedNodeId, EditError> {
        let document = &mut *self.document;
        let capacity = document.pool.edit_capacity(
            if pool.is_some() { staged.bytes } else { 0 },
            document.edit_owned_bytes,
            self.limits.max_owned_bytes,
        )?;
        let allocation = |_| allocation_error();
        #[cfg(test)]
        if document.nodes.capacity() - document.nodes.len() < staged.nodes.len() {
            inject_edit_reservation_failure(EditReservation::Node)?;
        }
        document
            .nodes
            .try_reserve(staged.nodes.len())
            .map_err(allocation)?;
        reserve_attributes(document, staged.attributes.len())?;
        document
            .kept
            .try_reserve(staged.kept.len())
            .map_err(allocation)?;
        document
            .ids
            .try_reserve(staged.ids.len())
            .map_err(allocation)?;
        if let Some(html) = &mut document.html {
            html.valueless
                .try_reserve(staged.valueless.len())
                .map_err(allocation)?;
        }
        document
            .pool
            .reserve_edit(capacity, &mut document.edit_owned_bytes)?;

        // Unreachable until returned: nothing links to the new slots.
        let lengths = [
            document.nodes.len(),
            document.attributes.len(),
            document.kept.len(),
            document.ids.len(),
            document
                .html
                .as_ref()
                .map_or(0, |html| html.valueless.len()),
            document.pool.0.len(),
        ];
        let counters = (
            document.edit_created_nodes,
            document.edit_created_attributes,
        );
        let target = &mut document.pool.0;
        let mut copy = |range: &mut Range<usize>| {
            if let Some(pool) = pool {
                let start = target.len();
                target.push_str(&pool[range.clone()]);
                *range = start..target.len();
            }
        };
        for mut record in staged.nodes {
            record.content.visit_strings(&mut copy);
            document.nodes.push(record);
        }
        for mut attribute in staged.attributes {
            attribute.visit_strings(&mut copy);
            document.attributes.push(attribute);
        }
        for (index, mut form) in staged.kept {
            copy(&mut form);
            document.kept.push((index, form));
        }
        if let Some(html) = &mut document.html {
            html.valueless.extend(staged.valueless);
        }
        let document_id = document.identity;
        let id = |slot| OwnedNodeId {
            document: document_id,
            slot: NodeId(slot),
        };
        let ids = staged.ids.into_iter();
        document
            .ids
            .extend(ids.map(|(slot, position)| (id(slot), position)));
        document.edit_created_nodes += document.nodes.len() - lengths[0];
        document.edit_created_attributes += document.attributes.len() - lengths[1];
        let root = lengths[0];
        if let Err(failure) = reconcile(self, root) {
            let document = &mut *self.document;
            document.nodes.truncate(lengths[0]);
            document.attributes.truncate(lengths[1]);
            document.kept.truncate(lengths[2]);
            document.ids.truncate(lengths[3]);
            if let Some(html) = &mut document.html {
                html.valueless.truncate(lengths[4]);
            }
            document.pool.0.truncate(lengths[5]);
            (
                document.edit_created_nodes,
                document.edit_created_attributes,
            ) = counters;
            return Err(failure);
        }
        Ok(self.id(root))
    }

    /// Declares what the detached copy at `root` needs, on its root when
    /// that takes no more declarations than placing each where it is needed.
    fn reconcile_copy(&mut self, root: usize) -> Result<(), EditError> {
        let mut work = Work {
            used: 0,
            limit: self.limits.max_namespace_work,
        };
        let document = &*self.document;
        let mut plan = document.plan(root, None, Vec::new(), &mut work)?;
        if plan.iter().any(|declare| declare.element != root) {
            let hoisted = document
                .hoisted(&plan, root, &mut work)
                .and_then(|seed| document.plan(root, None, seed, &mut work));
            if let Ok(hoisted) = hoisted
                && hoisted.len() <= plan.len()
            {
                plan = hoisted;
            }
        }
        self.declare(plan)
    }

    /// Parses `input` as content of `context` (`xmlParseInNodeContext`,
    /// `xmlCtxtParseContent`) and returns the top-level nodes, detached in
    /// this document. Insert them with [`append_child`](Self::append_child)
    /// or another move; under `context` they need no new declarations.
    ///
    /// The content is parsed in the nearest element or document at or above
    /// `context`, with the namespaces in scope there and this document's
    /// entities, from UTF-8 `input` (any sequence of content: text, several
    /// elements, no XML declaration or DOCTYPE). As in libxml2, the DTD's
    /// attribute defaults and types do not apply while parsing; this
    /// document's DTD still types IDs. `xmlParseBalancedChunkMemory` is this
    /// on [`OwnedDocument::root`] with default options.
    ///
    /// Parse budgets apply as to a document, edit limits as to an
    /// [`import`](Self::import) of the result.
    ///
    /// # Errors
    ///
    /// Malformed content is [`EditErrorKind::Parse`] at its byte offset
    /// ([`LineIndex::content`](crate::LineIndex::content) gives libxml2's
    /// diagnostic); under `RECOVER` only limits fail, and
    /// [`Fragment::errors`] lists the rest. A DOCTYPE context or an HTML
    /// document is `WrongNodeKind` (see
    /// [`html::parse_fragment`](crate::html::parse_fragment)); `XINCLUDE`
    /// and `DTDVALID` are `Parse(UnsupportedOption)`.
    ///
    /// ```
    /// use yaxml::{EditLimits, OwnedDocument, ParseOptions, save};
    /// let mut document = OwnedDocument::parse(
    ///     "<!DOCTYPE r [<!ENTITY who 'world'>]><r xmlns:p='urn:p'/>",
    /// )?;
    /// let r = document.root_element().id();
    /// let mut editor = document.edit(EditLimits::default())?;
    /// let input = b"hello <p:b>&who;</p:b>!";
    /// let fragment = editor.parse_content(r, input, ParseOptions::default())?;
    /// for node in fragment.nodes {
    ///     editor.append_child(r, node)?;
    /// }
    /// let saved = String::from_utf8(save::to_vec(&document, &save::Options::default())?)?;
    /// assert!(saved.ends_with("<r xmlns:p=\"urn:p\">hello <p:b>&who;</p:b>!</r>\n"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn parse_content(
        &mut self,
        context: OwnedNodeId,
        input: &[u8],
        options: ParseOptions,
    ) -> Result<Fragment, EditError> {
        let document = &*self.document;
        let node = document.view(NodeId(self.resolve(context)?));
        let node = node.filter(|node| node.kind() != OwnedNodeKind::DocumentType);
        let node = node
            .filter(|_| !document.is_html())
            .ok_or(EditError::new(EditErrorKind::WrongNodeKind, None))?;
        // libxml2's parent chain, where entity content's parent is the
        // entity declaration under the document.
        let element = std::iter::successors(Some(node), |node| {
            node.parent()
                .filter(|parent| parent.kind() != OwnedNodeKind::EntityRef)
        })
        .find(|node| node.kind() == OwnedNodeKind::Element);
        let scope = element.map(OwnedNode::namespaces).unwrap_or_default();
        let bindings: Vec<_> = (scope.iter())
            .filter(|binding| binding.prefix != Some("xml"))
            .map(|binding| (binding.prefix.unwrap_or(""), binding.uri))
            .collect();
        let unsupported = crate::options::XINCLUDE | crate::options::DTDVALID;
        let context = Some((&bindings[..], document.dtd()));
        let (parsed, _, _) =
            OwnedDocument::parse_tree(input, options, unsupported, Global, context).map_err(
                |error| EditError::new(EditErrorKind::Parse(error.kind), Some(error.offset)),
            )?;
        let context = element.map_or(0, |element| element.record().id.0);
        let target = Target {
            document,
            same: true,
            limits: &self.limits,
        };
        let (mut staged, mut roots) = (Staged::default(), Vec::new());
        for node in parsed.root().children() {
            roots.push(document.nodes.len() + staged.nodes.len());
            staged = stage(&parsed, node.id(), &target, true, staged)?;
        }
        self.publish(staged, Some(&parsed.pool.0), |editor, _| {
            let mut work = Work {
                used: 0,
                limit: editor.limits.max_namespace_work,
            };
            let mut plan = Vec::new();
            for &root in &roots {
                plan = editor.document.plan(root, Some(context), plan, &mut work)?;
            }
            editor.declare(plan)
        })?;
        Ok(Fragment {
            nodes: roots.iter().map(|&slot| self.id(slot)).collect(),
            errors: parsed.errors,
        })
    }
}

/// The nodes [`Editor::parse_content`] parsed.
#[derive(Debug)]
#[non_exhaustive]
pub struct Fragment {
    /// The top-level nodes in input order, each detached.
    pub nodes: Vec<OwnedNodeId>,
    /// The errors the parse kept (namespace errors, and under `RECOVER` what
    /// it recovered from), as [`OwnedDocument::errors`].
    pub errors: Vec<Error>,
}

/// A fallible element-by-element clone in the same allocator.
fn cloned<T: Clone, A: Allocator + Clone>(
    items: &allocator::Vec<T, A>,
) -> Result<allocator::Vec<T, A>, EditError> {
    let mut copy = allocator::Vec::new_in(items.allocator().clone());
    copy.try_reserve_exact(items.len())
        .map_err(|_| allocation_error())?;
    copy.extend(items.iter().cloned());
    Ok(copy)
}

impl<A: Allocator + Clone> OwnedDocument<A> {
    /// A copy of the whole document in the same allocator, DTD and detached
    /// subtrees included (`xmlCopyDoc`). It has a new identity, so this
    /// document's handles do not resolve in it. Editing counters carry over.
    ///
    /// # Errors
    ///
    /// Allocation failure or identity exhaustion only.
    ///
    /// ```
    /// use yaxml::OwnedDocument;
    /// let document = OwnedDocument::parse("<r a='1'>text</r>")?;
    /// let copy = document.try_clone()?;
    /// assert!(copy.node(document.root_element().id()).is_none());
    /// assert!(document.nodes().map(|n| n.content()).eq(copy.nodes().map(|n| n.content())));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn try_clone(&self) -> Result<Self, EditError> {
        let identity = document_identity(&LAST_DOCUMENT_ID)
            .map_err(|_| EditError::new(EditErrorKind::IdentifierExhausted, None))?;
        let remap = |id: OwnedNodeId| OwnedNodeId {
            document: identity,
            slot: id.slot,
        };
        let mut pool = String::new();
        pool.try_reserve_exact(self.pool.0.len())
            .map_err(|_| allocation_error())?;
        pool.push_str(&self.pool.0);
        let mut ids = Vec::new();
        ids.try_reserve_exact(self.ids.len())
            .map_err(|_| allocation_error())?;
        ids.extend(self.ids.iter().map(|&(id, index)| (remap(id), index)));
        let validity = self
            .validity
            .iter()
            .map(|diagnostic| crate::valid::Diagnostic {
                node: diagnostic.node.map(remap),
                ..diagnostic.clone()
            });
        Ok(OwnedDocument {
            identity,
            nodes: cloned(&self.nodes)?,
            attributes: cloned(&self.attributes)?,
            kept: cloned(&self.kept)?,
            pool: Pool(pool),
            root_element: self.root_element,
            declaration: self.declaration.clone(),
            edit_owned_bytes: self.edit_owned_bytes,
            edit_created_nodes: self.edit_created_nodes,
            edit_created_attributes: self.edit_created_attributes,
            slots_are_preorder: self.slots_are_preorder,
            html: self.html.clone(),
            ids,
            dtd: self.dtd.clone(),
            validity: validity.collect(),
            errors: self.errors.clone(),
            url: self.url.clone(),
        })
    }
}

#[cfg(test)]
#[path = "../../tests/internal/owned_copy.rs"]
mod tests;
