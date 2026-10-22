//! libxml2's mutable tree core over the arena: moves (`xmlAddChild`,
//! `xmlAddPrevSibling`, `xmlAddNextSibling`, `xmlReplaceNode`,
//! `xmlUnlinkNode`), attribute membership (`xmlSetNsProp`, `xmlUnsetNsProp`),
//! names and namespaces (`xmlNodeSetName`, `xmlSetNs`, `xmlNewNs`) and
//! storage compaction.
//!
//! The attached tree is always namespace-consistent: every element and
//! attribute prefix resolves, where it stands, to the URI its name stores.
//! An edit that could break this reconciles (`reconcile`) before it publishes,
//! declaring each binding a name lost on the outermost element of the edited
//! subtree that needs it, under the name's own prefix. Spellings never change.
//! A name that cannot be repaired that way (its element itself declares the
//! prefix differently) fails the edit with `Namespace`.

use super::construction::{Work, allocation_error, clone_string, limit_global, validate_part};
use super::*;

#[derive(Clone, Copy)]
enum Anchor {
    Last(usize),
    Before(usize),
    After(usize),
}

/// A declaration reconciliation adds on `element`; prefix and URI are the
/// pool ranges of the name that needs it (`uri` `None`: `xmlns=""`).
pub(super) struct Declare {
    pub(super) element: usize,
    pub(super) prefix: Option<Range<usize>>,
    pub(super) uri: Option<Range<usize>>,
}

fn error(kind: EditErrorKind) -> EditError {
    EditError::new(kind, None)
}

/// Single-item locations: `None` the element name, `Some(namespace)` item 0.
fn location(
    item: Option<bool>,
    component: EditComponent,
    byte_offset: Option<usize>,
) -> EditLocation {
    let (index, item_component) = (0, Some(component));
    match item {
        None => EditLocation::ElementName {
            component,
            byte_offset,
        },
        Some(true) => EditLocation::NamespaceItem {
            index,
            component: item_component,
            byte_offset,
        },
        Some(false) => EditLocation::AttributeItem {
            index,
            component: item_component,
            byte_offset,
        },
    }
}

fn namespace_error(item: Option<bool>, component: EditComponent) -> EditError {
    EditError::located(EditErrorKind::Namespace, location(item, component, None))
}

/// Length then character/name checks of a supplied name part.
fn check_part(
    part: &str,
    component: EditComponent,
    item: Option<bool>,
    is_name: bool,
    limits: &EditLimits,
) -> Result<(), EditError> {
    if part.len() > limits.max_name_bytes {
        let at = location(item, component, Some(limits.max_name_bytes));
        return Err(EditError::located(
            EditErrorKind::LimitExceeded(EditLimit::NameBytes),
            at,
        ));
    }
    validate_part(
        part,
        component,
        item.map(|namespace| (0, namespace)),
        is_name,
    )
}

fn check_name(
    name: &NameSpec<'_>,
    item: Option<bool>,
    limits: &EditLimits,
) -> Result<(), EditError> {
    check_part(name.local, EditComponent::LocalName, item, true, limits)?;
    if let Some(uri) = name.namespace {
        check_part(uri, EditComponent::NamespaceUri, item, false, limits)?;
    }
    if let PrefixPolicy::Exact(Some(prefix)) = name.prefix {
        check_part(prefix, EditComponent::Prefix, item, true, limits)?;
    }
    Ok(())
}

impl StoredName {
    fn prefix(&self) -> Option<Range<usize>> {
        let length = self.prefix_length?.get();
        Some(self.qualified.start..self.qualified.start + length)
    }

    /// The prefix a namespace declaration binds (`""`: the default namespace).
    fn declared<'p>(&self, pool: &'p str) -> Option<&'p str> {
        let namespace = self.namespace.as_ref()?;
        (&pool[namespace.clone()] == XMLNS).then(|| match self.prefix_length {
            Some(length) => &pool[self.qualified.start + length.get() + 1..self.qualified.end],
            None => "",
        })
    }
}

impl<A: Allocator> OwnedDocument<A> {
    fn attribute_range(&self, element: usize) -> Range<usize> {
        match &self.nodes[element].content {
            Content::Element { attributes, .. } => attributes.clone(),
            _ => 0..0,
        }
    }

    fn set_attribute_range(&mut self, element: usize, range: Range<usize>) {
        if let Content::Element { attributes, .. } = &mut self.nodes[element].content {
            *attributes = range;
        }
    }

    fn is_kind(&self, index: usize, kind: OwnedNodeKind) -> bool {
        self.view(index).is_some_and(|node| node.kind() == kind)
    }

    /// Whether `index` lies in entity-reference content, which `save`
    /// writes as the reference: such content is read-only.
    fn in_entity(&self, index: usize) -> bool {
        std::iter::successors(self.nodes[index].parent.get(), |id| {
            self.nodes[id.index()].parent.get()
        })
        .any(|id| matches!(self.nodes[id.index()].content, Content::EntityRef(_)))
    }

    /// Elements on the path from `index` up to its root, `index` included.
    fn depth(&self, index: usize) -> usize {
        std::iter::successors(Some(NodeId::new(index)), |id| {
            self.nodes[id.index()].parent.get()
        })
        .filter(|id| matches!(self.nodes[id.index()].content, Content::Element { .. }))
        .count()
    }

    /// Appends `attribute` to `element`'s attributes. A range at the arena
    /// tail grows in place; otherwise it moves to the tail, leaving garbage
    /// that `compact` reclaims. Capacity was reserved by the caller.
    fn push_attribute(&mut self, element: usize, attribute: StoredAttribute) {
        let range = self.attribute_range(element);
        let start = if range.end == self.attributes.len() {
            range.start
        } else {
            let start = self.attributes.len();
            for index in range.clone() {
                let copy = self.attributes[index].clone();
                self.attributes.push(copy);
            }
            let moved = |index: &mut usize| *index = start + (*index - range.start);
            relocate(
                &mut self.kept,
                &range,
                |entry| entry.0,
                |entry| moved(&mut entry.0),
            );
            if let Some(html) = &mut self.html {
                relocate(&mut html.valueless, &range, |&index| index, moved);
            }
            start
        };
        self.attributes.push(attribute);
        self.set_attribute_range(element, start..start + range.len() + 1);
    }

    /// Undoes the [`Self::push_attribute`] of a created attribute; the record
    /// stays behind as garbage, since other (empty) ranges may lie past it.
    fn pop_attribute(&mut self, element: usize) {
        let range = self.attribute_range(element);
        self.set_attribute_range(element, range.start..range.end - 1);
        self.edit_created_attributes -= 1;
    }

    /// Removes the attribute at `position` from `element`'s range by rotating
    /// it past the end; [`Self::restore_attribute`] undoes this exactly.
    fn hide_attribute(&mut self, element: usize, position: usize) {
        let range = self.attribute_range(element);
        self.attributes[range.start + position..range.end].rotate_left(1);
        self.set_attribute_range(element, range.start..range.end - 1);
    }

    fn restore_attribute(&mut self, element: usize, position: usize) {
        let range = self.attribute_range(element);
        self.attributes[range.start + position..=range.end].rotate_right(1);
        self.set_attribute_range(element, range.start..range.end + 1);
    }

    /// Completes [`Self::hide_attribute`]: side tables drop the attribute and
    /// shift later positions. The record stays as garbage for `compact`.
    fn forget_attribute(&mut self, element: usize, position: usize) {
        let range = self.attribute_range(element);
        let removed = range.start + position;
        let old_end = range.end + 1;
        let shift = |index: &mut usize| *index -= usize::from(*index > removed && *index < old_end);
        if let Ok(found) = self.kept.binary_search_by_key(&removed, |entry| entry.0) {
            self.kept.remove(found);
        }
        self.kept.iter_mut().for_each(|entry| shift(&mut entry.0));
        if let Some(html) = &mut self.html {
            html.valueless.retain(|&index| index != removed);
            html.valueless.iter_mut().for_each(shift);
        }
        let owner = OwnedNodeId {
            document: self.identity,
            slot: element,
        };
        self.ids.retain(|&id| id != (owner, position));
        for (id, index) in &mut self.ids {
            *index -= usize::from(*id == owner && *index > position);
        }
    }

    /// The URI bound to `prefix` at `at`, reading `plan` as declarations
    /// already added and continuing at `context` above `root`.
    #[allow(clippy::too_many_arguments)]
    fn bound(
        &self,
        mut at: usize,
        prefix: &str,
        root: usize,
        context: Option<usize>,
        plan: &[Declare],
        work: &mut Work,
    ) -> Result<Option<&str>, EditError> {
        let pool = &self.pool.0;
        let text = |range: &Option<Range<usize>>| range.clone().map_or("", |range| &pool[range]);
        loop {
            let Content::Element { attributes, .. } = &self.nodes[at].content else {
                return Ok(None);
            };
            work.charge(1 + plan.len())?;
            let planned = plan
                .iter()
                .find(|declare| declare.element == at && text(&declare.prefix) == prefix);
            if let Some(declare) = planned {
                return Ok(Some(text(&declare.uri)));
            }
            for attribute in &self.attributes[attributes.clone()] {
                work.charge(1)?;
                if attribute.name.declared(pool) == Some(prefix) {
                    return Ok(Some(&pool[attribute.value.clone()]));
                }
            }
            let up = match (at == root, context) {
                (true, Some(context)) => Some(context),
                _ => self.nodes[at].parent.get().map(|id| id.index()),
            };
            let Some(up) = up else {
                return Ok(None);
            };
            at = up;
        }
    }

    /// The declarations `root`'s subtree needs once it stands under `context`
    /// (`None`: where it is), after the `seed` declarations. Entity-reference
    /// content is not entered, as in `xmlReconciliateNs`; it is written as
    /// the reference.
    pub(super) fn plan(
        &self,
        root: usize,
        context: Option<usize>,
        seed: Vec<Declare>,
        work: &mut Work,
    ) -> Result<Vec<Declare>, EditError> {
        let pool = &self.pool.0;
        let mut plan = seed;
        let mut current = root;
        loop {
            let record = &self.nodes[current];
            if let Content::Element { name, attributes } = &record.content {
                // A declaration planned for one name can rebind another on
                // the same element: the second pass finds that conflict.
                for verify in [false, true] {
                    let ordinary = self.attributes[attributes.clone()]
                        .iter()
                        .filter(|attribute| attribute.name.declared(pool).is_none());
                    let names = std::iter::once((name, true))
                        .chain(ordinary.map(|attribute| (&attribute.name, false)));
                    for (name, element) in names {
                        work.charge(1)?;
                        let prefix = name.prefix();
                        let text = prefix.clone().map_or("", |range| &pool[range]);
                        // Unprefixed attributes have no namespace, `xml` is
                        // always bound, and a recovered unbound prefix cannot
                        // be declared.
                        let unbound = prefix.is_some() && name.namespace.is_none();
                        if (!element && prefix.is_none()) || text == "xml" || unbound {
                            continue;
                        }
                        let wanted = name.namespace.clone().map(|range| &pool[range]);
                        let bound = self.bound(current, text, root, context, &plan, work)?;
                        if bound.filter(|uri| !uri.is_empty()) == wanted {
                            continue;
                        }
                        let own = self.attributes[attributes.clone()]
                            .iter()
                            .any(|attribute| attribute.name.declared(pool) == Some(text));
                        let planned = plan.iter().any(|declare| {
                            declare.element == current
                                && declare.prefix.clone().map_or("", |range| &pool[range]) == text
                        });
                        if verify || own || planned {
                            return Err(error(EditErrorKind::Namespace));
                        }
                        plan.try_reserve(1).map_err(|_| allocation_error())?;
                        plan.push(Declare {
                            element: current,
                            prefix,
                            uri: name.namespace.clone(),
                        });
                    }
                }
            }
            let first = match record.content {
                Content::EntityRef(_) => None,
                _ => record.first_child.get(),
            };
            if let Some(child) = first {
                current = child.index();
                continue;
            }
            loop {
                if current == root {
                    return Ok(plan);
                }
                let record = &self.nodes[current];
                if let Some(next) = record.next_sibling.get() {
                    current = next.index();
                    break;
                }
                current = record
                    .parent
                    .get()
                    .expect("subtree node has a parent")
                    .index();
            }
        }
    }

    /// The element height of `root`'s subtree (0 for a leaf).
    fn height(&self, root: usize) -> usize {
        let element = |index: usize| usize::from(self.is_kind(index, OwnedNodeKind::Element));
        let (mut current, mut depth) = (root, element(root));
        let mut height = depth;
        loop {
            if let Some(child) = self.nodes[current].first_child.get() {
                current = child.index();
                depth += element(current);
                height = height.max(depth);
                continue;
            }
            loop {
                if current == root {
                    return height;
                }
                depth -= element(current);
                if let Some(next) = self.nodes[current].next_sibling.get() {
                    current = next.index();
                    depth += element(current);
                    height = height.max(depth);
                    break;
                }
                current = self.nodes[current]
                    .parent
                    .get()
                    .expect("subtree node")
                    .index();
            }
        }
    }
}

/// Moves the run of `entries` (sorted by `index`) whose indices lie in
/// `range` to the end, rewriting each with `moved`: the range moved to the
/// arena tail, past every other index.
fn relocate<T>(
    entries: &mut [T],
    range: &Range<usize>,
    index: impl Fn(&T) -> usize,
    moved: impl Fn(&mut T),
) {
    let first = entries.partition_point(|entry| index(entry) < range.start);
    let last = entries.partition_point(|entry| index(entry) < range.end);
    entries[first..].rotate_left(last - first);
    let total = entries.len();
    entries[total - (last - first)..].iter_mut().for_each(moved);
}

impl<A: Allocator> Editor<'_, A> {
    pub(super) fn id(&self, index: usize) -> OwnedNodeId {
        OwnedNodeId {
            document: self.document.identity,
            slot: index,
        }
    }

    /// Moves `node` to be `parent`'s last child and returns its handle
    /// (`xmlAddChild`). Text after a Text last child merges into it: that
    /// child's handle is returned and `node` is removed. Missing bindings are
    /// declared on the moved subtree.
    ///
    /// ```
    /// use yaxml::{EditLimits, OwnedDocument, save};
    /// let mut document = OwnedDocument::parse("<r>a<p:b xmlns:p='urn:p'><p:c/></p:b>z</r>")?;
    /// let root = document.root_element();
    /// let (a, z) = (root.first_child().unwrap().id(), root.last_child().unwrap().id());
    /// let c = root.first_child().unwrap().next_sibling().unwrap().first_child().unwrap().id();
    /// let root = root.id();
    /// let mut editor = document.edit(EditLimits::default())?;
    /// assert_eq!(editor.append_child(root, a)?, z); // "a" merges into "z"
    /// assert!(editor.document().node(a).is_none());
    /// editor.append_child(root, c)?;
    /// let saved = save::to_vec(editor.document(), &save::Options::default())?;
    /// let saved = String::from_utf8(saved)?;
    /// assert!(saved.ends_with("<r><p:b xmlns:p=\"urn:p\"/>za<p:c xmlns:p=\"urn:p\"/></r>\n"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn append_child(
        &mut self,
        parent: OwnedNodeId,
        node: OwnedNodeId,
    ) -> Result<OwnedNodeId, EditError> {
        let parent = self.resolve(parent)?;
        self.place(node, Anchor::Last(parent))
    }

    /// `xmlAddPrevSibling`: moves `node` before `reference`, without merging Text.
    pub fn insert_before(
        &mut self,
        reference: OwnedNodeId,
        node: OwnedNodeId,
    ) -> Result<OwnedNodeId, EditError> {
        let reference = self.resolve(reference)?;
        self.place(node, Anchor::Before(reference))
    }

    /// `xmlAddNextSibling`: moves `node` after `reference`, without merging Text.
    pub fn insert_after(
        &mut self,
        reference: OwnedNodeId,
        node: OwnedNodeId,
    ) -> Result<OwnedNodeId, EditError> {
        let reference = self.resolve(reference)?;
        self.place(node, Anchor::After(reference))
    }

    /// Detaches `node` and its subtree, whose handles stay live for a later
    /// insert or removal (`xmlUnlinkNode`).
    pub fn unlink(&mut self, node: OwnedNodeId) -> Result<(), EditError> {
        let node = self.resolve(node)?;
        self.check_movable(node)?;
        self.document.detach(node);
        Ok(())
    }

    /// Moves `new` into `old`'s place and detaches `old` (`xmlReplaceNode`).
    /// The document element can be replaced only by an element.
    pub fn replace(&mut self, old: OwnedNodeId, new: OwnedNodeId) -> Result<(), EditError> {
        let (old, new) = (self.resolve(old)?, self.resolve(new)?);
        let document = &*self.document;
        let root = old == document.root_element.index();
        if old == 0 || document.is_kind(old, OwnedNodeKind::DocumentType) || document.in_entity(old)
        {
            return Err(error(EditErrorKind::ProtectedNode));
        }
        self.check_movable(new)?;
        let parent = self.document.nodes[old].parent.get();
        let parent = parent.ok_or(error(EditErrorKind::Hierarchy))?.index();
        if old == new || root && !self.document.is_kind(new, OwnedNodeKind::Element) {
            return Err(error(EditErrorKind::Hierarchy));
        }
        self.check_target(new, parent, root)?;
        self.reconcile(new, Some(parent))?;
        self.document.detach(new);
        self.document
            .attach(new, parent, Link::new(NodeId::new(old)));
        self.document.detach(old);
        if root {
            self.document.root_element = NodeId::new(new);
        }
        Ok(())
    }

    /// The synthetic root, DOCTYPE, document element and entity content
    /// cannot move.
    fn check_movable(&self, node: usize) -> Result<(), EditError> {
        let document = &*self.document;
        if node == 0
            || node == document.root_element.index()
            || document.is_kind(node, OwnedNodeKind::DocumentType)
            || document.in_entity(node)
        {
            return Err(error(EditErrorKind::ProtectedNode));
        }
        Ok(())
    }

    /// Whether `node` can become a child of `parent`: entity content is
    /// read-only; parents are elements or the document, which takes only
    /// comments and PIs (or the document element: a replacement, or the
    /// first one of a rootless document); no cycles;
    /// a move may not make the tree deeper than `max_depth`.
    fn check_target(&self, node: usize, parent: usize, root: bool) -> Result<(), EditError> {
        let document = &*self.document;
        let parent_kind = document.view(parent).map(OwnedNode::kind);
        if parent_kind == Some(OwnedNodeKind::EntityRef) || document.in_entity(parent) {
            return Err(error(EditErrorKind::ProtectedNode));
        }
        let kind = document.view(node).map(OwnedNode::kind);
        let top_level = matches!(
            kind,
            Some(OwnedNodeKind::Comment | OwnedNodeKind::ProcessingInstruction)
        ) || root;
        match parent_kind {
            Some(OwnedNodeKind::Element) => {}
            Some(OwnedNodeKind::Document) if top_level => {}
            Some(OwnedNodeKind::Document) => return Err(error(EditErrorKind::Hierarchy)),
            _ => return Err(error(EditErrorKind::WrongNodeKind)),
        }
        let mut ancestors = std::iter::successors(Some(NodeId::new(parent)), |id| {
            document.nodes[id.index()].parent.get()
        });
        if ancestors.any(|id| id.index() == node) {
            return Err(error(EditErrorKind::Hierarchy));
        }
        // Only a move under a parent at least as deep as the node's current
        // one deepens it; then its height decides.
        let target = document.depth(parent);
        if kind == Some(OwnedNodeKind::Element)
            && target >= document.depth(node)
            && target + document.height(node) > self.limits.max_depth
        {
            return Err(limit_global(EditLimit::Depth));
        }
        Ok(())
    }

    fn place(&mut self, node: OwnedNodeId, anchor: Anchor) -> Result<OwnedNodeId, EditError> {
        let node = self.resolve(node)?;
        self.check_movable(node)?;
        let parent = match anchor {
            Anchor::Last(parent) => Some(parent),
            Anchor::Before(reference) | Anchor::After(reference) if reference != node => {
                self.document.nodes[reference]
                    .parent
                    .get()
                    .map(|id| id.index())
            }
            _ => None,
        };
        let parent = parent.ok_or(error(EditErrorKind::Hierarchy))?;
        // A rootless (HTML or RECOVER) document adopts its first element.
        let root = parent == 0
            && self.document.root_element == NodeId::new(0)
            && self.document.is_kind(node, OwnedNodeKind::Element);
        self.check_target(node, parent, root)?;
        let last = self.document.nodes[parent]
            .last_child
            .get()
            .map(|id| id.index());
        if let Anchor::Last(_) = anchor {
            if last == Some(node) {
                return Ok(self.id(node));
            }
            let text = |index| self.document.is_kind(index, OwnedNodeKind::Text);
            if let Some(last) = last.filter(|&last| text(last) && text(node)) {
                return self.merge_text(last, node);
            }
        }
        self.reconcile(node, Some(parent))?;
        self.document.detach(node);
        let next = match anchor {
            Anchor::Last(_) => Link::NONE,
            Anchor::Before(reference) => Link::new(NodeId::new(reference)),
            Anchor::After(reference) => self.document.nodes[reference].next_sibling,
        };
        self.document.attach(node, parent, next);
        if root {
            self.document.root_element = NodeId::new(node);
        }
        Ok(self.id(node))
    }

    /// Appends Text `node`'s data to Text `last` and removes `node`.
    fn merge_text(&mut self, last: usize, node: usize) -> Result<OwnedNodeId, EditError> {
        let range = |index: usize| match &self.document.nodes[index].content {
            Content::Text(range) => range.clone(),
            _ => unreachable!("merging Text nodes"),
        };
        let (kept, added) = (range(last), range(node));
        validate_value_length_sum(kept.len(), added.len(), self.limits.max_value_bytes)?;
        let in_place = kept.end == self.document.pool.0.len();
        let needed = added.len() + if in_place { 0 } else { kept.len() };
        let pool = &mut self.document.pool;
        let capacity = pool.edit_capacity(
            needed,
            self.document.edit_owned_bytes,
            self.limits.max_owned_bytes,
        )?;
        pool.reserve_edit(capacity, &mut self.document.edit_owned_bytes)?;
        let start = match in_place {
            true => kept.start,
            false => {
                let start = pool.0.len();
                pool.0.extend_from_within(kept);
                start
            }
        };
        pool.0.extend_from_within(added);
        self.document.nodes[last].content = Content::Text(start..pool.0.len());
        self.document.remove(node);
        Ok(self.id(last))
    }

    /// Plans, charges, reserves and then adds the declarations `root` needs
    /// under `context`; nothing changes on failure.
    fn reconcile(&mut self, root: usize, context: Option<usize>) -> Result<(), EditError> {
        let mut work = Work {
            used: 0,
            limit: self.limits.max_namespace_work,
        };
        let plan = self.document.plan(root, context, Vec::new(), &mut work)?;
        self.declare(plan)
    }

    /// Charges, reserves and then adds `plan`'s declarations; nothing
    /// changes on failure.
    pub(super) fn declare(&mut self, plan: Vec<Declare>) -> Result<(), EditError> {
        if plan.is_empty() {
            return Ok(());
        }
        let document = &mut *self.document;
        let created = document.edit_created_attributes + plan.len();
        if created > self.limits.max_created_attributes {
            return Err(limit_global(EditLimit::CreatedAttributes));
        }
        let bytes = plan.iter().fold(XMLNS.len(), |bytes, declare| {
            bytes + 5 + declare.prefix.as_ref().map_or(0, |prefix| 1 + prefix.len())
        });
        let capacity = document.pool.edit_capacity(
            bytes,
            document.edit_owned_bytes,
            self.limits.max_owned_bytes,
        )?;
        let slots = plan.iter().fold(0, |slots, declare| {
            slots + document.attribute_range(declare.element).len() + 1
        });
        reserve_attributes(document, slots)?;
        document
            .pool
            .reserve_edit(capacity, &mut document.edit_owned_bytes)?;
        let pool = &mut document.pool.0;
        let namespace = pool.len()..pool.len() + XMLNS.len();
        pool.push_str(XMLNS);
        for declare in plan {
            let pool = &mut document.pool.0;
            let start = pool.len();
            pool.push_str("xmlns");
            if let Some(prefix) = &declare.prefix {
                pool.push(':');
                pool.extend_from_within(prefix.clone());
            }
            let name = StoredName {
                qualified: start..pool.len(),
                prefix_length: declare.prefix.as_ref().and(NonZeroUsize::new(5)),
                namespace: Some(namespace.clone()),
            };
            let value = declare.uri.unwrap_or(0..0);
            document.push_attribute(declare.element, StoredAttribute { name, value });
        }
        document.edit_created_attributes = created;
        Ok(())
    }

    /// Resolves an element that is not entity content.
    fn editable_element(&self, element: OwnedNodeId) -> Result<usize, EditError> {
        let index = self.resolve(element)?;
        if !matches!(self.document.nodes[index].content, Content::Element { .. }) {
            return Err(error(EditErrorKind::WrongNodeKind));
        }
        if self.document.in_entity(index) {
            return Err(error(EditErrorKind::ProtectedNode));
        }
        Ok(index)
    }

    /// The prefix a name in `name.namespace` takes on `element`. `Exact`
    /// keeps the request; `Automatic` takes a matching in-scope binding
    /// (an element first the default namespace), else the first unbound
    /// `nsN`. Reconciliation declares whatever is not yet bound.
    fn choose_prefix(
        &self,
        element: usize,
        name: &NameSpec<'_>,
        item: Option<bool>,
    ) -> Result<Option<String>, EditError> {
        let is_element = item.is_none();
        let node = self.document.view(element).expect("live element");
        let prefix = match (name.namespace, name.prefix) {
            (None, PrefixPolicy::Exact(None) | PrefixPolicy::Automatic) => {
                if !is_element && name.local == "xmlns" {
                    return Err(namespace_error(item, EditComponent::LocalName));
                }
                return Ok(None);
            }
            (Some(XML), PrefixPolicy::Automatic | PrefixPolicy::Exact(Some("xml"))) => "xml",
            (None, _) | (Some(XML), _) | (_, PrefixPolicy::Exact(Some("xml" | "xmlns"))) => {
                return Err(namespace_error(item, EditComponent::Prefix));
            }
            (Some(uri), _) if uri.is_empty() || uri == XMLNS => {
                return Err(namespace_error(item, EditComponent::NamespaceUri));
            }
            (Some(_), PrefixPolicy::Exact(None)) if !is_element => {
                return Err(namespace_error(item, EditComponent::Prefix));
            }
            (Some(_), PrefixPolicy::Exact(prefix)) => match prefix {
                Some(prefix) => prefix,
                None => return Ok(None),
            },
            (Some(uri), PrefixPolicy::Automatic) => {
                if is_element && node.search_ns(None) == Some(uri) {
                    return Ok(None);
                }
                let bindings = node.namespaces();
                let bound = bindings
                    .iter()
                    .find(|binding| binding.uri == uri && binding.prefix.is_some());
                if let Some(prefix) = bound.and_then(|binding| binding.prefix) {
                    return clone_string(prefix).map(Some);
                }
                let fresh = (1usize..)
                    .map(|number| format!("ns{number}"))
                    .find(|prefix| node.search_ns(Some(prefix)).is_none());
                return Ok(fresh);
            }
        };
        clone_string(prefix).map(Some)
    }

    /// Sets an attribute by expanded name (`xmlSetNsProp`). An existing one
    /// keeps its spelling and position; a new one is appended with the
    /// prefix [`PrefixPolicy`] selects, declared on `element` if needed. Name
    /// errors locate attribute item 0. For namespace declarations, use
    /// [`declare_namespace`](Self::declare_namespace).
    pub fn set_attribute(
        &mut self,
        element: OwnedNodeId,
        name: NameSpec<'_>,
        value: &str,
    ) -> Result<(), EditError> {
        let index = self.editable_element(element)?;
        check_name(&name, Some(false), &self.limits)?;
        if self.document.is_html() && name.namespace.is_some() {
            return Err(namespace_error(Some(false), EditComponent::NamespaceUri));
        }
        if name.namespace == Some(XMLNS) {
            return Err(error(EditErrorKind::NamespaceDeclaration));
        }
        let node = self.document.view(index).expect("live element");
        let exists = node.attributes().any(|attribute| {
            attribute.name.local() == name.local && attribute.name.namespace() == name.namespace
        });
        if exists {
            return self.set_attribute_value(element, name.namespace, name.local, value);
        }
        validate_value(value, ValueKind::Text, self.limits.max_value_bytes)?;
        let prefix = self.choose_prefix(index, &name, Some(false))?;
        let document = &mut *self.document;
        let created = document.edit_created_attributes + 1;
        if created > self.limits.max_created_attributes {
            return Err(limit_global(EditLimit::CreatedAttributes));
        }
        let qualified = prefix.as_ref().map_or(0, |prefix| prefix.len() + 1) + name.local.len();
        let namespace = name.namespace.unwrap_or("");
        let bytes = qualified + namespace.len() + value.len();
        let capacity = document.pool.edit_capacity(
            bytes,
            document.edit_owned_bytes,
            self.limits.max_owned_bytes,
        )?;
        let element_name = node_name(document, index);
        let qualified_name = match &prefix {
            Some(prefix) => format!("{prefix}:{}", name.local),
            None => name.local.to_owned(),
        };
        let id = document.is_id(&element_name, &qualified_name);
        reserve_attributes(document, document.attribute_range(index).len() + 1)?;
        if id {
            document
                .ids
                .try_reserve(1)
                .map_err(|_| allocation_error())?;
        }
        document
            .pool
            .reserve_edit(capacity, &mut document.edit_owned_bytes)?;
        let pool = &mut document.pool.0;
        let start = pool.len();
        pool.push_str(&qualified_name);
        let qualified = start..pool.len();
        let namespace = name.namespace.map(|uri| {
            pool.push_str(uri);
            qualified.end..pool.len()
        });
        let value_start = pool.len();
        pool.push_str(value);
        let stored = StoredAttribute {
            name: StoredName {
                qualified,
                prefix_length: prefix.as_ref().and_then(|p| NonZeroUsize::new(p.len())),
                namespace,
            },
            value: value_start..pool.len(),
        };
        document.push_attribute(index, stored);
        document.edit_created_attributes = created;
        let unbound = prefix.as_deref().is_some_and(|prefix| {
            let node = document.view(index).expect("live element");
            node.search_ns(Some(prefix)) != name.namespace
        });
        if unbound && let Err(failure) = self.reconcile(index, None) {
            self.document.pop_attribute(index);
            return Err(failure);
        }
        let owner = self.id(index);
        let document = &mut *self.document;
        if id {
            let entry = (owner, document.attribute_range(index).len() - 1);
            let position = document.ids.partition_point(|&found| found < entry);
            document.ids.insert(position, entry);
        }
        Ok(())
    }

    /// Removes the attribute with this expanded name (`xmlUnsetNsProp`). A
    /// namespace declaration (XMLNS namespace; local `xmlns` for the
    /// default) can be removed only while no name in the subtree uses it.
    pub fn remove_attribute(
        &mut self,
        element: OwnedNodeId,
        namespace: Option<&str>,
        local: &str,
    ) -> Result<(), EditError> {
        let index = self.editable_element(element)?;
        let node = self.document.view(index).expect("live element");
        let position = node.attributes().position(|attribute| {
            attribute.name.local() == local && attribute.name.namespace() == namespace
        });
        let position = position.ok_or(error(EditErrorKind::MissingAttribute))?;
        self.document.hide_attribute(index, position);
        if namespace == Some(XMLNS) {
            let mut work = Work {
                used: 0,
                limit: self.limits.max_namespace_work,
            };
            let unused = self.document.plan(index, None, Vec::new(), &mut work);
            let failure = match unused {
                Ok(plan) if plan.is_empty() => None,
                Ok(_) => Some(error(EditErrorKind::Namespace)),
                Err(failure) => Some(failure),
            };
            if let Some(failure) = failure {
                self.document.restore_attribute(index, position);
                return Err(failure);
            }
        }
        self.document.forget_attribute(index, position);
        Ok(())
    }

    /// Declares `prefix` (`None`: the default namespace) as `uri` on
    /// `element`; an empty `uri` undeclares the default (`xmlNewNs`).
    /// Descendants that used an outer binding of the prefix get it declared
    /// on them. Errors locate namespace item 0.
    ///
    /// # Errors
    ///
    /// The element already declares the prefix or uses it for another URI;
    /// a reserved prefix or URI (`xml`, `xmlns`, an empty URI for a prefix);
    /// an HTML document.
    pub fn declare_namespace(
        &mut self,
        element: OwnedNodeId,
        prefix: Option<&str>,
        uri: &str,
    ) -> Result<(), EditError> {
        let index = self.editable_element(element)?;
        let item = Some(true);
        if let Some(prefix) = prefix {
            check_part(prefix, EditComponent::Prefix, item, true, &self.limits)?;
        }
        check_part(uri, EditComponent::NamespaceUri, item, false, &self.limits)?;
        let reserved = prefix == Some("xmlns")
            || uri == XMLNS
            || prefix == Some("xml")
            || uri == XML
            || (prefix.is_some() && uri.is_empty())
            || self.document.is_html();
        if reserved {
            return Err(namespace_error(item, EditComponent::NamespaceUri));
        }
        let document = &mut *self.document;
        let pool = &document.pool.0;
        let wanted = prefix.unwrap_or("");
        let declared = document.attributes[document.attribute_range(index)]
            .iter()
            .any(|attribute| attribute.name.declared(pool) == Some(wanted));
        if declared {
            return Err(EditError::located(
                EditErrorKind::DuplicateAttribute,
                location(item, EditComponent::Prefix, None),
            ));
        }
        let created = document.edit_created_attributes + 1;
        if created > self.limits.max_created_attributes {
            return Err(limit_global(EditLimit::CreatedAttributes));
        }
        let qualified = 5 + prefix.map_or(0, |prefix| prefix.len() + 1);
        let capacity = document.pool.edit_capacity(
            qualified + XMLNS.len() + uri.len(),
            document.edit_owned_bytes,
            self.limits.max_owned_bytes,
        )?;
        reserve_attributes(document, document.attribute_range(index).len() + 1)?;
        document
            .pool
            .reserve_edit(capacity, &mut document.edit_owned_bytes)?;
        let pool = &mut document.pool.0;
        let start = pool.len();
        pool.push_str("xmlns");
        if let Some(prefix) = prefix {
            pool.push(':');
            pool.push_str(prefix);
        }
        let qualified = start..pool.len();
        pool.push_str(XMLNS);
        let namespace = qualified.end..pool.len();
        pool.push_str(uri);
        let stored = StoredAttribute {
            name: StoredName {
                qualified,
                prefix_length: prefix.and(NonZeroUsize::new(5)),
                namespace: Some(namespace.clone()),
            },
            value: namespace.end..pool.len(),
        };
        document.push_attribute(index, stored);
        document.edit_created_attributes = created;
        if let Err(failure) = self.reconcile(index, None) {
            self.document.pop_attribute(index);
            return Err(failure);
        }
        Ok(())
    }

    /// Renames an element (`xmlNodeSetName` with `xmlSetNs`). The prefix
    /// follows [`PrefixPolicy`]; a binding not in scope is declared on the
    /// element without changing what descendants resolve to.
    pub fn set_name(&mut self, element: OwnedNodeId, name: NameSpec<'_>) -> Result<(), EditError> {
        let index = self.editable_element(element)?;
        check_name(&name, None, &self.limits)?;
        if self.document.is_html() && name.namespace.is_some() {
            return Err(namespace_error(None, EditComponent::NamespaceUri));
        }
        let prefix = self.choose_prefix(index, &name, None)?;
        let qualified = match &prefix {
            Some(prefix) => format!("{prefix}:{}", name.local),
            None => name.local.to_owned(),
        };
        let namespace = name.namespace.unwrap_or("");
        let document = &mut *self.document;
        let capacity = document.pool.edit_capacity(
            qualified.len() + namespace.len(),
            document.edit_owned_bytes,
            self.limits.max_owned_bytes,
        )?;
        document
            .pool
            .reserve_edit(capacity, &mut document.edit_owned_bytes)?;
        let pool = &mut document.pool.0;
        let start = pool.len();
        pool.push_str(&qualified);
        let stored = StoredName {
            qualified: start..pool.len(),
            prefix_length: prefix.as_ref().and_then(|p| NonZeroUsize::new(p.len())),
            namespace: name.namespace.map(|uri| {
                let start = pool.len();
                pool.push_str(uri);
                start..pool.len()
            }),
        };
        let Content::Element { name: current, .. } = &mut document.nodes[index].content else {
            unreachable!("checked element");
        };
        let previous = std::mem::replace(current, stored);
        let node = document.view(index).expect("live element");
        let bound = node
            .search_ns(prefix.as_deref())
            .filter(|uri| !uri.is_empty());
        if bound != name.namespace
            && let Err(failure) = self.reconcile(index, None)
        {
            if let Content::Element { name, .. } = &mut self.document.nodes[index].content {
                *name = previous;
            }
            return Err(failure);
        }
        Ok(())
    }

    /// Sets the document URL that [`OwnedNode::base_uri`] resolves against.
    pub fn set_url(&mut self, url: Option<&str>) -> Result<(), EditError> {
        let url = url
            .map(|url| {
                validate_value_length(url, self.limits.max_value_bytes)?;
                clone_string(url)
            })
            .transpose()?;
        self.document.url = url;
        Ok(())
    }

    /// Reclaims the string and attribute storage that edits left behind.
    /// Handles stay valid, and stale ones stay rejected.
    pub fn compact(&mut self) -> Result<(), EditError> {
        let document = &mut *self.document;
        // (old range, element, new start) of each element with attributes.
        let mut owners: Vec<(Range<usize>, usize, usize)> = Vec::new();
        let elements = document
            .nodes
            .iter()
            .filter_map(|record| match &record.content {
                Content::Element { attributes, .. } if !attributes.is_empty() => {
                    Some((attributes.clone(), record.id.index(), 0))
                }
                _ => None,
            });
        let count = elements.clone().count();
        #[cfg(test)]
        inject_edit_reservation_failure(EditReservation::Scratch)?;
        owners
            .try_reserve_exact(count)
            .map_err(|_| allocation_error())?;
        owners.extend(elements);
        // Attribute storage is relaid without changing any view, so a later
        // pool failure leaves a consistent document. Empty ranges move to 0
        // so that none lies past the truncated arena.
        for record in document.nodes.iter_mut() {
            if let Content::Element { attributes, .. } = &mut record.content
                && attributes.start == attributes.end
            {
                *attributes = 0..0;
            }
        }
        owners.sort_unstable_by_key(|(range, ..)| range.start);
        let mut write = 0;
        for (range, element, start) in &mut owners {
            for offset in 0..range.len() {
                document
                    .attributes
                    .swap(write + offset, range.start + offset);
            }
            document.set_attribute_range(*element, write..write + range.len());
            *start = write;
            write += range.len();
        }
        document.attributes.truncate(write);
        let moved = |index: &mut usize| {
            let owner = owners.partition_point(|(range, ..)| range.end <= *index);
            let (range, _, start) = owners
                .get(owner)
                .filter(|(range, ..)| range.contains(index))?;
            *index = start + (*index - range.start);
            Some(())
        };
        document
            .kept
            .retain_mut(|(index, _)| moved(index).is_some());
        if let Some(html) = &mut document.html {
            html.valueless.retain_mut(|index| moved(index).is_some());
        }
        let mut live = 0;
        document.visit_strings(|range| live += range.len());
        let mut pool = String::new();
        #[cfg(test)]
        inject_edit_reservation_failure(EditReservation::Pool)?;
        pool.try_reserve_exact(live)
            .map_err(|_| allocation_error())?;
        let old = std::mem::take(&mut document.pool.0);
        document.visit_strings(|range| {
            let start = pool.len();
            pool.push_str(&old[range.clone()]);
            *range = start..pool.len();
        });
        document.pool.0 = pool;
        document.edit_owned_bytes = 0;
        Ok(())
    }
}

/// Node name as `is_id` compares it.
fn node_name<A: Allocator>(document: &OwnedDocument<A>, index: usize) -> String {
    document
        .view(index)
        .and_then(OwnedNode::qualified_name)
        .unwrap_or_default()
        .to_owned()
}

fn validate_value_length_sum(left: usize, right: usize, limit: usize) -> Result<(), EditError> {
    if left.saturating_add(right) > limit {
        return Err(limit_global(EditLimit::ValueBytes));
    }
    Ok(())
}

pub(super) fn reserve_attributes<A: Allocator>(
    document: &mut OwnedDocument<A>,
    additional: usize,
) -> Result<(), EditError> {
    #[cfg(test)]
    if document.attributes.len().saturating_add(additional) > document.attributes.capacity() {
        inject_edit_reservation_failure(EditReservation::Attribute)?;
    }
    document
        .attributes
        .try_reserve(additional)
        .map_err(|_| allocation_error())
}

impl<A: Allocator> OwnedDocument<A> {
    /// libxml2's `xmlIsID`: whether an attribute of this qualified name on
    /// an element of this qualified name is an ID (`xml:id`, an HTML `id`, or
    /// `name` on `a`, or one the DTD declares `ID`). Setting such an
    /// attribute adds it to the IDs [`element_by_id`](Self::element_by_id)
    /// finds, removing it removes it (`xmlAddID`, `xmlRemoveID`).
    pub fn is_id(&self, element: &str, attribute: &str) -> bool {
        attribute == "xml:id"
            || self.html.is_some() && (attribute == "id" || element == "a" && attribute == "name")
            || self
                .dtd()
                .is_some_and(|dtd| dtd.declares_id(element, attribute))
    }

    /// Calls `visit` on every pool range a live record, attribute or kept
    /// form refers to, in a fixed order.
    fn visit_strings(&mut self, mut visit: impl FnMut(&mut Range<usize>)) {
        if let Some(declaration) = &mut self.declaration {
            visit(&mut declaration.version);
            if let Some(encoding) = &mut declaration.encoding {
                visit(encoding);
            }
        }
        for record in self.nodes.iter_mut() {
            record.content.visit_strings(&mut visit);
        }
        for attribute in self.attributes.iter_mut() {
            attribute.visit_strings(&mut visit);
        }
        for (_, range) in self.kept.iter_mut() {
            visit(range);
        }
    }
}

impl StoredName {
    fn visit_strings(&mut self, visit: &mut impl FnMut(&mut Range<usize>)) {
        visit(&mut self.qualified);
        if let Some(namespace) = &mut self.namespace {
            visit(namespace);
        }
    }
}

impl StoredAttribute {
    pub(super) fn visit_strings(&mut self, visit: &mut impl FnMut(&mut Range<usize>)) {
        self.name.visit_strings(visit);
        visit(&mut self.value);
    }
}

impl Content {
    /// Calls `visit` on each pool range the record holds.
    pub(super) fn visit_strings(&mut self, visit: &mut impl FnMut(&mut Range<usize>)) {
        match self {
            Content::DocumentType(doctype) => {
                visit(&mut doctype.name);
                let optional = [
                    &mut doctype.public_id,
                    &mut doctype.system_id,
                    &mut doctype.internal_subset,
                ];
                optional.into_iter().flatten().for_each(visit);
            }
            Content::Element { name, .. } => name.visit_strings(visit),
            Content::Text(range)
            | Content::Cdata(range)
            | Content::Comment(range)
            | Content::EntityRef(range) => visit(range),
            Content::ProcessingInstruction { target, data } => {
                visit(target);
                visit(data);
            }
            Content::Document | Content::Removed => {}
        }
    }
}

#[cfg(test)]
#[path = "../../tests/internal/owned_edit.rs"]
mod tests;
