//! Bounded edits use stable model IDs and ordered live children, not arena links.

use std::ops::Range;

use yaxml::{
    Attribute, Document, EditError, EditErrorKind, EditLimit, EditLimits, Editor, Name, NodeKind,
    OwnedDocument, OwnedNodeId, OwnedNodeKind, Writer, WriterLimits,
};

const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
const OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Kind {
    Document,
    Element,
    Text,
    Cdata,
    Comment,
    Pi,
}

#[derive(Clone, Copy)]
struct ModelName {
    qualified: &'static str,
    prefix: Option<&'static str>,
    local: &'static str,
    namespace: Option<&'static str>,
}

fn name(qualified: &'static str, namespace: Option<&'static str>) -> ModelName {
    let (prefix, local) = qualified
        .split_once(':')
        .map_or((None, qualified), |(prefix, local)| (Some(prefix), local));
    ModelName {
        qualified,
        prefix,
        local,
        namespace,
    }
}

struct ModelAttribute {
    name: ModelName,
    value: String,
}

struct ModelNode {
    live: bool,
    kind: Kind,
    name: Option<ModelName>,
    value: Option<String>,
    target: Option<String>,
    attributes: Vec<ModelAttribute>,
    children: Vec<usize>,
}

fn node(kind: Kind, value: Option<&str>, children: &[usize]) -> ModelNode {
    ModelNode {
        live: true,
        kind,
        name: None,
        value: value.map(str::to_owned),
        target: None,
        attributes: Vec::new(),
        children: children.to_vec(),
    }
}

fn element(
    qualified: &'static str,
    namespace: Option<&'static str>,
    children: &[usize],
) -> ModelNode {
    let mut node = node(Kind::Element, None, children);
    node.name = Some(name(qualified, namespace));
    node
}

fn attribute(
    qualified: &'static str,
    namespace: Option<&'static str>,
    value: &str,
) -> ModelAttribute {
    ModelAttribute {
        name: name(qualified, namespace),
        value: value.into(),
    }
}

struct Model {
    nodes: Vec<ModelNode>,
    declaration: Option<(&'static str, bool)>,
    created: usize,
}

impl Model {
    fn parent(&self, index: usize) -> Option<usize> {
        self.nodes
            .iter()
            .position(|node| node.live && node.children.contains(&index))
    }

    fn subtree(&self, index: usize) -> Vec<usize> {
        let mut order = Vec::new();
        let mut pending = vec![index];
        while let Some(index) = pending.pop() {
            let node = &self.nodes[index];
            if !node.live {
                continue;
            }
            order.push(index);
            pending.extend(node.children.iter().rev().copied());
        }
        order
    }

    fn expected(
        &self,
        owner: usize,
        request: Request<'_>,
        limits: EditLimits,
    ) -> Result<(), EditError> {
        if request.handle.owner != owner {
            return Err(error(EditErrorKind::InvalidHandle, None));
        }
        let node = self
            .nodes
            .get(request.handle.node)
            .filter(|node| node.live)
            .ok_or(error(EditErrorKind::InvalidHandle, None))?;
        let kind = if let Some(selector) = request.selector {
            if node.kind != Kind::Element {
                return Err(error(EditErrorKind::WrongNodeKind, None));
            }
            let attribute = node
                .attributes
                .iter()
                .find(|attribute| {
                    attribute.name.local == selector.local
                        && attribute.name.namespace == selector.namespace
                })
                .ok_or(error(EditErrorKind::MissingAttribute, None))?;
            if attribute.name.namespace == Some(XMLNS) {
                return Err(error(EditErrorKind::NamespaceDeclaration, None));
            }
            Kind::Text
        } else {
            match node.kind {
                Kind::Document | Kind::Element => {
                    return Err(error(EditErrorKind::WrongNodeKind, None));
                }
                kind => kind,
            }
        };
        expected_value(request.value, kind, limits.max_value_bytes)
    }

    fn replace(&mut self, request: Request<'_>) {
        let node = &mut self.nodes[request.handle.node];
        if let Some(selector) = request.selector {
            node.attributes
                .iter_mut()
                .find(|attribute| {
                    attribute.name.local == selector.local
                        && attribute.name.namespace == selector.namespace
                })
                .unwrap()
                .value = request.value.into();
        } else {
            node.value = Some(request.value.into());
        }
    }

    fn rescue(&self, owner: usize, handles: &[OwnedNodeId]) -> Option<Request<'static>> {
        self.nodes.iter().enumerate().find_map(|(index, node)| {
            if !node.live {
                return None;
            }
            let selector = if matches!(node.kind, Kind::Document | Kind::Element) {
                let attribute = node
                    .attributes
                    .iter()
                    .find(|attribute| attribute.name.namespace != Some(XMLNS))?;
                Some(Selector {
                    namespace: attribute.name.namespace,
                    local: attribute.name.local,
                })
            } else {
                None
            };
            Some(Request {
                handle: Saved {
                    owner,
                    node: index,
                    id: handles[index],
                },
                selector,
                value: "",
            })
        })
    }

    fn expected_structure(
        &self,
        owner: usize,
        action: Action<'_>,
        limits: EditLimits,
    ) -> Result<(), EditError> {
        let handle = action.handle();
        if handle.owner != owner || !self.nodes.get(handle.node).is_some_and(|node| node.live) {
            return Err(error(EditErrorKind::InvalidHandle, None));
        }
        let node = &self.nodes[handle.node];
        match action {
            Action::Remove(_) => {
                if handle.node <= 1 {
                    return Err(error(EditErrorKind::ProtectedNode, None));
                }
            }
            Action::Append {
                kind,
                target,
                value,
                ..
            } => {
                if node.kind != Kind::Element
                    && !(node.kind == Kind::Document && matches!(kind, Kind::Comment | Kind::Pi))
                {
                    return Err(error(EditErrorKind::WrongNodeKind, None));
                }
                if value.len() > limits.max_value_bytes
                    || (kind == Kind::Pi && target.len() > limits.max_value_bytes)
                {
                    return Err(error(
                        EditErrorKind::LimitExceeded(EditLimit::ValueBytes),
                        None,
                    ));
                }
                if kind == Kind::Pi {
                    expected_target(target)?;
                }
                expected_value(value, kind, limits.max_value_bytes)?;
                if self.created >= limits.max_created_nodes {
                    return Err(error(
                        EditErrorKind::LimitExceeded(EditLimit::CreatedNodes),
                        None,
                    ));
                }
            }
        }
        Ok(())
    }

    fn append(&mut self, parent: usize, kind: Kind, target: &str, value: &str) -> usize {
        let id = self.nodes.len(); // Monotonic model identity; never reuse holes.
        let mut fresh = node(kind, Some(value), &[]);
        if kind == Kind::Pi {
            fresh.target = Some(target.into());
        }
        self.nodes.push(fresh);
        self.nodes[parent].children.push(id);
        self.created += 1;
        id
    }

    fn remove(&mut self, index: usize) -> Vec<usize> {
        let removed = self.subtree(index);
        let parent = self.parent(index).unwrap();
        self.nodes[parent].children.retain(|child| *child != index);
        for &index in &removed {
            self.nodes[index].live = false;
            self.nodes[index].children.clear();
        }
        removed
    }
}

#[derive(Clone, Copy)]
enum Action<'a> {
    Append {
        parent: Saved,
        kind: Kind,
        target: &'a str,
        value: &'a str,
    },
    Remove(Saved),
}

impl Action<'_> {
    fn handle(self) -> Saved {
        match self {
            Self::Append { parent, .. } => parent,
            Self::Remove(handle) => handle,
        }
    }
}

fn expected_target(target: &str) -> Result<(), EditError> {
    expected_value(target, Kind::Text, usize::MAX)?;
    let start = |character: char| {
        matches!(u32::from(character),
        0x41..=0x5a | 0x5f | 0x61..=0x7a | 0xc0..=0xd6 | 0xd8..=0xf6 |
        0xf8..=0x2ff | 0x370..=0x37d | 0x37f..=0x1fff | 0x200c..=0x200d |
        0x2070..=0x218f | 0x2c00..=0x2fef | 0x3001..=0xd7ff | 0xf900..=0xfdcf |
        0xfdf0..=0xfffd | 0x10000..=0xeffff)
    };
    if target.is_empty() {
        return Err(error(EditErrorKind::InvalidName, Some(0)));
    }
    for (offset, character) in target.char_indices() {
        if !(start(character)
            || (offset > 0
                && matches!(u32::from(character), 0x2d..=0x2e | 0x30..=0x39 | 0xb7 | 0x300..=0x36f | 0x203f..=0x2040)))
        {
            return Err(error(EditErrorKind::InvalidName, Some(offset)));
        }
    }
    if target.eq_ignore_ascii_case("xml") {
        return Err(error(EditErrorKind::InvalidName, Some(0)));
    }
    Ok(())
}

fn error(kind: EditErrorKind, value_offset: Option<usize>) -> EditError {
    EditError { kind, value_offset, location: None }
}

// This oracle walks UTF-8 scalars and then byte positions, independently of
// production's character helper and delimiter-search implementation.
fn expected_value(value: &str, kind: Kind, maximum: usize) -> Result<(), EditError> {
    if value.len() > maximum {
        return Err(error(
            EditErrorKind::LimitExceeded(EditLimit::ValueBytes),
            None,
        ));
    }
    for (offset, character) in value.char_indices() {
        let scalar = u32::from(character);
        if !matches!(scalar, 9 | 10 | 13 | 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff) {
            return Err(error(EditErrorKind::InvalidCharacter, Some(offset)));
        }
    }
    let bytes = value.as_bytes();
    for (offset, byte) in bytes.iter().copied().enumerate() {
        let invalid = match kind {
            Kind::Cdata => byte == b'\r' || bytes.get(offset..offset + 3) == Some(b"]]>"),
            Kind::Comment => {
                byte == b'\r'
                    || (byte == b'-'
                        && (offset + 1 == bytes.len() || bytes.get(offset + 1) == Some(&b'-')))
            }
            Kind::Pi => {
                byte == b'\r'
                    || bytes.get(offset..offset + 2) == Some(b"?>")
                    || (offset == 0 && matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
            }
            _ => false,
        };
        if invalid {
            return Err(error(EditErrorKind::InvalidValue, Some(offset)));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Saved {
    owner: usize,
    node: usize,
    id: OwnedNodeId,
}

#[derive(Clone, Copy)]
struct Selector {
    namespace: Option<&'static str>,
    local: &'static str,
}

fn selector(byte: u8) -> Selector {
    let (namespace, local) = match byte % 12 {
        0 => (None, "a"),
        1 => (Some("urn:p"), "a"),
        2 => (Some("urn:q"), "a"),
        3 => (Some("urn:r"), "a"),
        4 => (Some(""), "a"),
        5 => (None, "missing"),
        6 => (Some(XMLNS), "xmlns"),
        7 => (Some(XMLNS), "p"),
        8 => (Some(XMLNS), "missing"),
        9 => (None, "bad name"),
        10 => (Some("urn:shadow"), "a"),
        _ => (None, "a:bad"),
    };
    Selector { namespace, local }
}

#[derive(Clone, Copy)]
struct Request<'a> {
    handle: Saved,
    selector: Option<Selector>,
    value: &'a str,
}

struct State {
    owner: usize,
    document: OwnedDocument,
    model: Model,
    handles: Vec<OwnedNodeId>,
    provenance: Vec<Option<Range<usize>>>,
}

impl State {
    fn new(owner: usize, source: &str, model: Model, encoding: u8) -> Self {
        let mut input = encoded(source, encoding);
        let document = OwnedDocument::parse_bytes(&input).unwrap();
        let handles: Vec<_> = document.nodes().map(|node| node.id()).collect();
        let provenance: Vec<_> = document.nodes().map(|node| node.range()).collect();
        assert_eq!(handles.len(), model.nodes.len());
        for (index, id) in handles.iter().enumerate() {
            assert!(
                !handles[..index].contains(id),
                "owned handles alias model nodes"
            );
        }
        assert!(provenance.iter().all(|range| {
            range
                .as_ref()
                .is_some_and(|range| range.start <= range.end && range.end <= input.len())
        }));
        input.fill(0);
        drop(input);
        let state = Self {
            owner,
            document,
            model,
            handles,
            provenance,
        };
        state.check();
        state
    }

    fn saved(&self, node: usize) -> Saved {
        Saved {
            owner: self.owner,
            node,
            id: self.handles[node],
        }
    }

    fn check(&self) {
        check_document(&self.document, &self.model, &self.handles, &self.provenance);
    }

    fn session(&mut self, limits: EditLimits, actions: &[(Request<'_>, bool)], recover: bool) {
        if self.model.created > limits.max_created_nodes {
            assert_eq!(
                self.document.edit(limits).err(),
                Some(error(
                    EditErrorKind::LimitExceeded(EditLimit::CreatedNodes),
                    None
                ))
            );
            self.check();
            return;
        }
        let mut editor = self.document.edit(limits).unwrap();
        for (request, storage_ok) in actions.iter().copied() {
            let mut expected = self.model.expected(self.owner, request, limits);
            if expected.is_ok() && !storage_ok {
                expected = Err(error(
                    EditErrorKind::LimitExceeded(EditLimit::OwnedBytes),
                    None,
                ));
            }
            if expected.is_ok() {
                self.model.replace(request);
            }
            let result = invoke(&mut editor, request);
            assert_eq!(
                result, expected,
                "owned edit disagrees with independent model"
            );
            if let Err(error) = result {
                super::check_display(&error);
                assert!(std::error::Error::source(&error).is_none());
            }
            check_document(
                editor.document(),
                &self.model,
                &self.handles,
                &self.provenance,
            );
            if recover && expected.is_err() {
                // Empty replacement needs neither value bytes nor pool growth.
                if let Some(rescue) = self.model.rescue(self.owner, &self.handles) {
                    assert_eq!(self.model.expected(self.owner, rescue, limits), Ok(()));
                    self.model.replace(rescue);
                    assert_eq!(
                        invoke(&mut editor, rescue),
                        Ok(()),
                        "editor became terminal after rejection"
                    );
                }
                check_document(
                    editor.document(),
                    &self.model,
                    &self.handles,
                    &self.provenance,
                );
            }
        }
    }

    fn structure(
        &mut self,
        limits: EditLimits,
        action: Action<'_>,
        storage_ok: bool,
    ) -> Option<Saved> {
        self.structure_with_recovery(limits, action, storage_ok, false)
    }

    fn structure_with_recovery(
        &mut self,
        limits: EditLimits,
        action: Action<'_>,
        storage_ok: bool,
        recover: bool,
    ) -> Option<Saved> {
        if self.model.created > limits.max_created_nodes {
            assert_eq!(
                self.document.edit(limits).err(),
                Some(error(
                    EditErrorKind::LimitExceeded(EditLimit::CreatedNodes),
                    None
                ))
            );
            self.check();
            return None;
        }
        let mut expected = self.model.expected_structure(self.owner, action, limits);
        if expected.is_ok() && matches!(action, Action::Append { .. }) && !storage_ok {
            expected = Err(error(
                EditErrorKind::LimitExceeded(EditLimit::OwnedBytes),
                None,
            ));
        }
        let mut editor = self.document.edit(limits).unwrap();
        let actual = match action {
            Action::Remove(handle) => editor.remove_subtree(handle.id).map(|()| None),
            Action::Append {
                parent,
                kind,
                target,
                value,
            } => match kind {
                Kind::Text => editor.append_text(parent.id, value),
                Kind::Cdata => editor.append_cdata(parent.id, value),
                Kind::Comment => editor.append_comment(parent.id, value),
                Kind::Pi => editor.append_processing_instruction(parent.id, target, value),
                _ => unreachable!(),
            }
            .map(Some),
        };
        assert_eq!(
            actual.as_ref().map(|_| ()).map_err(|error| *error),
            expected,
            "structural edit disagrees with independent prediction"
        );
        if let Err(error) = &actual {
            super::check_display(error);
            assert!(std::error::Error::source(error).is_none());
        }
        let mut saved = None;
        if let Ok(result) = actual {
            match action {
                Action::Remove(handle) => {
                    for index in self.model.remove(handle.node) {
                        self.provenance[index] = None;
                    }
                }
                Action::Append {
                    parent,
                    kind,
                    target,
                    value,
                } => {
                    let id = result.unwrap();
                    assert!(
                        !self.handles.contains(&id),
                        "created handle aliases a live or stale ID"
                    );
                    let index = self.model.append(parent.node, kind, target, value);
                    assert_eq!(index, self.handles.len());
                    self.handles.push(id);
                    self.provenance.push(None);
                    saved = Some(Saved {
                        owner: self.owner,
                        node: index,
                        id,
                    });
                }
            }
        }
        check_document(
            editor.document(),
            &self.model,
            &self.handles,
            &self.provenance,
        );
        if recover && expected.is_err() {
            if self.model.created < limits.max_created_nodes {
                let parent = Saved {
                    owner: self.owner,
                    node: 0,
                    id: self.handles[0],
                };
                let rescue = Action::Append {
                    parent,
                    kind: Kind::Comment,
                    target: "",
                    value: "",
                };
                assert_eq!(
                    self.model.expected_structure(self.owner, rescue, limits),
                    Ok(())
                );
                let id = editor
                    .append_comment(parent.id, "")
                    .expect("structural rejection poisoned its editor");
                assert!(!self.handles.contains(&id));
                let index = self.model.append(0, Kind::Comment, "", "");
                assert_eq!(index, self.handles.len());
                self.handles.push(id);
                self.provenance.push(None);
            } else if let Some(rescue) = self.model.rescue(self.owner, &self.handles) {
                assert_eq!(self.model.expected(self.owner, rescue, limits), Ok(()));
                self.model.replace(rescue);
                invoke(&mut editor, rescue).expect("structural rejection poisoned value editing");
            }
            check_document(
                editor.document(),
                &self.model,
                &self.handles,
                &self.provenance,
            );
        }
        saved
    }
}

fn invoke(editor: &mut Editor<'_>, request: Request<'_>) -> Result<(), EditError> {
    if let Some(selector) = request.selector {
        editor.set_attribute_value(
            request.handle.id,
            selector.namespace,
            selector.local,
            request.value,
        )
    } else {
        editor.set_node_value(request.handle.id, request.value)
    }
}

fn check_name(actual: &Name<'_>, expected: ModelName) {
    assert!(
        actual.qualified == expected.qualified
            && actual.prefix == expected.prefix
            && actual.local == expected.local
            && actual.namespace.as_deref() == expected.namespace,
        "mutation changed a qualified or expanded name"
    );
}

fn check_attributes<'a>(actual: impl Iterator<Item = Attribute<'a>>, expected: &[ModelAttribute]) {
    let mut actual = actual;
    for expected in expected {
        let actual = actual.next().expect("mutation omitted an attribute");
        check_name(&actual.name, expected.name);
        assert!(
            actual.value == expected.value,
            "mutation changed the wrong attribute value"
        );
    }
    assert!(actual.next().is_none(), "mutation added an attribute");
}

fn owned_kind(kind: Kind) -> OwnedNodeKind {
    match kind {
        Kind::Document => OwnedNodeKind::Document,
        Kind::Element => OwnedNodeKind::Element,
        Kind::Text => OwnedNodeKind::Text,
        Kind::Cdata => OwnedNodeKind::Cdata,
        Kind::Comment => OwnedNodeKind::Comment,
        Kind::Pi => OwnedNodeKind::ProcessingInstruction,
    }
}

fn check_document(
    document: &OwnedDocument,
    model: &Model,
    handles: &[OwnedNodeId],
    provenance: &[Option<Range<usize>>],
) {
    assert_eq!(
        document.nodes().len(),
        model.nodes.iter().filter(|node| node.live).count(),
        "mutation changed topology"
    );
    match (document.declaration(), model.declaration) {
        (None, None) => {}
        (Some(actual), Some((version, standalone))) => {
            assert_eq!(actual.version, version);
            assert_eq!(actual.encoding, None);
            assert_eq!(actual.standalone, Some(standalone));
        }
        _ => panic!("mutation changed declaration presence"),
    }
    assert_eq!(document.root().id(), handles[0]);
    assert_eq!(document.root_element().id(), handles[1]);
    let order = model.subtree(0);
    for (position, actual) in document.nodes().enumerate() {
        let index = order[position];
        let expected = &model.nodes[index];
        assert_eq!(actual.id(), handles[index]);
        assert_eq!(actual.range(), provenance[index]);
        assert_eq!(actual.kind(), owned_kind(expected.kind));
        assert!(
            actual.text() == expected.value.as_deref(),
            "mutation changed the wrong node value"
        );
        assert_eq!(
            actual.processing_instruction_target(),
            expected.target.as_deref()
        );
        match (actual.name(), expected.name) {
            (None, None) => {}
            (Some(actual), Some(expected)) => check_name(&actual, expected),
            _ => panic!("mutation changed name presence"),
        }
        check_attributes(actual.attributes(), &expected.attributes);
        let parent = model.parent(index);
        assert_eq!(
            actual.parent().map(|node| node.id()),
            parent.map(|index| handles[index])
        );
        assert_eq!(
            actual.first_child().map(|node| node.id()),
            expected.children.first().map(|index| handles[*index])
        );
        assert_eq!(
            actual.last_child().map(|node| node.id()),
            expected.children.last().map(|index| handles[*index])
        );
        let next = parent.and_then(|parent| {
            let children = &model.nodes[parent].children;
            children
                .iter()
                .position(|child| *child == index)
                .and_then(|position| children.get(position + 1))
                .copied()
        });
        assert_eq!(
            actual.next_sibling().map(|node| node.id()),
            next.map(|index| handles[index])
        );
        assert!(
            actual
                .children()
                .map(|node| node.id())
                .eq(expected.children.iter().map(|index| handles[*index])),
            "mutation changed child order"
        );
        let subtree = model.subtree(index);
        let expected_chunks = subtree
            .iter()
            .filter(|&&child| {
                child == index || matches!(model.nodes[child].kind, Kind::Text | Kind::Cdata)
            })
            .filter_map(|&child| model.nodes[child].value.as_deref());
        assert!(
            actual.text_chunks().eq(expected_chunks),
            "mutation changed string-value chunks"
        );
        assert_eq!(actual.descendants().len(), subtree.len());
        assert!(
            actual
                .descendants()
                .map(|node| node.id())
                .eq(subtree.iter().map(|index| handles[*index])),
            "mutation changed subtree traversal"
        );
        assert_eq!(actual.descendants().count(), subtree.len());
        assert_eq!(
            actual.descendants().last().map(|node| node.id()),
            subtree.last().map(|index| handles[*index])
        );
        // Check progress independently of the iterator's cached length. A
        // midpoint exercises both contiguous slots and linked/tombstoned trees.
        let midpoint = subtree.len() / 2;
        let mut partial = actual.descendants();
        assert_eq!(
            partial.nth(midpoint).map(|node| node.id()),
            Some(handles[subtree[midpoint]])
        );
        let remaining = subtree.len() - midpoint - 1;
        assert_eq!(partial.len(), remaining);
        assert_eq!(partial.size_hint(), (remaining, Some(remaining)));
        assert_eq!(partial.count(), remaining);
        let mut exhausted = actual.descendants();
        assert!(exhausted.nth(subtree.len()).is_none());
        assert_eq!(exhausted.len(), 0);
        assert_eq!(exhausted.size_hint(), (0, Some(0)));
        assert!(exhausted.next().is_none());
        assert!(exhausted.next().is_none());
        let resolved = document
            .node(handles[index])
            .expect("mutation invalidated a live handle");
        assert_eq!(resolved.id(), actual.id());
        assert_eq!(resolved.kind(), actual.kind());
        assert_eq!(resolved.name(), actual.name());
        assert_eq!(resolved.text(), actual.text());
        assert_eq!(
            resolved.processing_instruction_target(),
            actual.processing_instruction_target()
        );
        assert_eq!(resolved.range(), actual.range());
        assert!(resolved.attributes().eq(actual.attributes()));
    }
    for (index, expected) in model.nodes.iter().enumerate() {
        assert_eq!(
            document.node(handles[index]).is_some(),
            expected.live,
            "lookup disagrees with live model membership"
        );
    }
}

fn serialized(state: &State) -> Vec<u8> {
    let mut writer = Writer::with_limits(
        Vec::new(),
        WriterLimits {
            max_output_bytes: OUTPUT_BYTES,
            max_token_bytes: 16 * 1024,
            ..WriterLimits::default()
        },
    );
    writer.write_owned_document(&state.document).unwrap();
    let position = writer.position();
    writer.finish().unwrap();
    let output = writer.into_inner();
    assert_eq!(position, output.len());
    assert!(output.len() <= OUTPUT_BYTES);
    let parsed = Document::parse_bytes(&output).unwrap();
    // Only empty/adjacent Text is projected. CDATA remains a separate kind.
    let mut order: Vec<usize> = Vec::new();
    let mut values: Vec<Option<String>> = Vec::new();
    let mut representative = vec![None; state.model.nodes.len()];
    for index in state.model.subtree(0) {
        let expected = &state.model.nodes[index];
        if expected.kind == Kind::Text {
            let value = expected.value.as_deref().unwrap();
            if value.is_empty() {
                continue;
            }
            if let Some(&previous) = order.last()
                && state.model.nodes[previous].kind == Kind::Text
                && state.model.parent(previous) == state.model.parent(index)
            {
                values.last_mut().unwrap().as_mut().unwrap().push_str(value);
                representative[index] = Some(previous);
                continue;
            }
        }
        representative[index] = Some(index);
        order.push(index);
        values.push(expected.value.clone());
    }
    let nodes: Vec<_> = parsed.nodes().collect();
    assert_eq!(
        nodes.len(),
        order.len(),
        "mutated serialization lost or added nodes"
    );
    assert_eq!(
        parsed.declaration().map(|declaration| (
            declaration.version,
            declaration.encoding,
            declaration.standalone
        )),
        state
            .model
            .declaration
            .map(|(version, standalone)| (version, None, Some(standalone)))
    );
    for (position, index) in order.iter().copied().enumerate() {
        let expected = &state.model.nodes[index];
        let actual = nodes[position];
        let kind = match actual.kind() {
            NodeKind::Document => Kind::Document,
            NodeKind::Element(_) => Kind::Element,
            NodeKind::Text(_) => Kind::Text,
            NodeKind::Cdata(_) => Kind::Cdata,
            NodeKind::Comment(_) => Kind::Comment,
            NodeKind::ProcessingInstruction { target, .. } => {
                assert_eq!(Some(*target), expected.target.as_deref());
                Kind::Pi
            }
        };
        assert!(
            kind == expected.kind,
            "mutated serialization changed node kind"
        );
        assert!(
            actual.text() == values[position].as_deref(),
            "mutated serialization changed logical value"
        );
        match (actual.name(), expected.name) {
            (None, None) => {}
            (Some(actual), Some(expected)) => check_name(actual, expected),
            _ => panic!("mutated serialization changed name presence"),
        }
        assert_eq!(actual.attributes().len(), expected.attributes.len());
        for (actual, expected) in actual.attributes().iter().zip(&expected.attributes) {
            check_name(&actual.name, expected.name);
            assert!(
                actual.value == expected.value,
                "serialized attribute value changed"
            );
        }
        let mut expected_children: Vec<_> = expected
            .children
            .iter()
            .copied()
            .filter_map(|child| representative[child])
            .collect();
        expected_children.dedup();
        let id = |index| nodes[order.iter().position(|value| *value == index).unwrap()].id();
        assert_eq!(
            actual.parent().map(|node| node.id()),
            state.model.parent(index).map(id)
        );
        assert_eq!(
            actual.first_child().map(|node| node.id()),
            expected_children.first().copied().map(id)
        );
        assert_eq!(
            actual.last_child().map(|node| node.id()),
            expected_children.last().copied().map(id)
        );
        assert!(
            actual
                .children()
                .map(|node| node.id())
                .eq(expected_children.into_iter().map(id)),
            "mutated serialization changed parent/child order"
        );
    }
    state.check();
    output
}

fn encoded(source: &str, encoding: u8) -> Vec<u8> {
    match encoding & 3 {
        0 => source.as_bytes().to_vec(),
        1 => [b"\xef\xbb\xbf".as_slice(), source.as_bytes()].concat(),
        mode => {
            let mut output = Vec::with_capacity(source.len() * 2 + 2);
            for unit in std::iter::once(0xfeff).chain(source.encode_utf16()) {
                output.extend_from_slice(&if mode == 2 {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            output
        }
    }
}

fn seed(owner: usize, family: u8, encoding: u8) -> State {
    let (root, child, target, text, version, standalone) = if family & 1 == 0 {
        ("r", "p:c", "pi", "x", "1.0", true)
    } else {
        ("根", "p:子", "空", "🙂", "1.00", false)
    };
    let source = format!(
        "<?xml version='{version}' standalone='{}'?><{root} xmlns='urn:r' xmlns:p='urn:p' xmlns:q='urn:q' a='A' p:a='P' q:a='Q'>{text}<![CDATA[]]><!----><?{target}?><{child} xmlns:p='urn:shadow' a='C'><n xmlns=''/>tail</{child}></{root}><!--after-->",
        if standalone { "yes" } else { "no" }
    );
    let mut nodes = vec![
        node(Kind::Document, None, &[1, 9]),
        element(root, Some("urn:r"), &[2, 3, 4, 5, 6]),
        node(Kind::Text, Some(text), &[]),
        node(Kind::Cdata, Some(""), &[]),
        node(Kind::Comment, Some(""), &[]),
        node(Kind::Pi, Some(""), &[]),
        element(child, Some("urn:shadow"), &[7, 8]),
        element("n", None, &[]),
        node(Kind::Text, Some("tail"), &[]),
        node(Kind::Comment, Some("after"), &[]),
    ];
    nodes[1].attributes = vec![
        attribute("xmlns", Some(XMLNS), "urn:r"),
        attribute("xmlns:p", Some(XMLNS), "urn:p"),
        attribute("xmlns:q", Some(XMLNS), "urn:q"),
        attribute("a", None, "A"),
        attribute("p:a", Some("urn:p"), "P"),
        attribute("q:a", Some("urn:q"), "Q"),
    ];
    nodes[5].target = Some(target.into());
    nodes[6].attributes = vec![
        attribute("xmlns:p", Some(XMLNS), "urn:shadow"),
        attribute("a", None, "C"),
    ];
    nodes[7].attributes = vec![attribute("xmlns", Some(XMLNS), "")];
    State::new(
        owner,
        &source,
        Model {
            nodes,
            declaration: Some((version, standalone)),
            created: 0,
        },
        encoding,
    )
}

fn value(payload: &[u8], operation: &[u8]) -> String {
    match operation[6] % 16 {
        0 => String::from_utf8_lossy(payload).into_owned(),
        1 => String::new(),
        2 => "é🙂\r\n\t&<>\"".into(),
        3 => "é\0".into(),
        4 => "🙂]]>".into(),
        5 => "é--".into(),
        6 => "é-".into(),
        7 => "🙂?>".into(),
        8 => "é\r".into(),
        9 => "\tdata".into(),
        10 => "]]>\0".into(),
        11 => " \u{ffff}".into(),
        12 => "valid".into(),
        13 => "é\u{fffe}".into(),
        14 => "\r--?>".into(),
        _ => "🙂".repeat(usize::from(operation[7] & 31)),
    }
}

fn pi_target_value(payload: &[u8], selector: u8) -> String {
    match selector % 16 {
        0 => String::from_utf8_lossy(payload).into_owned(),
        1 => "".into(),
        2 => "1bad".into(),
        3 => "a:b".into(),
        4 => "xml".into(),
        5 => "XmL".into(),
        6 => "é🙂!".into(),
        7 => "1\0".into(),
        8 => "é\0".into(),
        9 => "é🙂".into(),
        10 => "xml-stylesheet".into(),
        11 => "_p.2".into(),
        12 => "p".into(),
        13 => "a b".into(),
        14 => "\u{f0000}".into(),
        _ => "a\u{b7}".into(),
    }
}

pub(super) fn exercise(input: &[u8]) {
    let byte = |index| input.get(index).copied().unwrap_or(0);
    let mut states = [
        seed(0, byte(0), byte(1)),
        seed(1, byte(0) ^ 1, byte(1).wrapping_add(1)),
    ];
    assert_ne!(states[0].handles[0], states[1].handles[0]);
    assert!(states[0].document.node(states[1].handles[0]).is_none());
    assert!(states[1].document.node(states[0].handles[0]).is_none());
    let mut saved: Vec<_> = states
        .iter()
        .flat_map(|state| (0..state.handles.len()).map(|index| state.saved(index)))
        .collect();
    // Both setters must succeed independently of arbitrary operation selectors.
    for state in &mut states {
        let text = Request {
            handle: state.saved(2),
            selector: None,
            value: "ok",
        };
        let attribute = Request {
            handle: state.saved(1),
            selector: Some(selector(0)),
            value: "v",
        };
        state.session(
            EditLimits::default(),
            &[(text, true), (attribute, true)],
            false,
        );
        // One document exercises every mandatory structural success; the other
        // supplies independent foreign handles and arbitrary-operation coverage.
        if state.owner != 0 {
            continue;
        }
        // Mandatory acceptance and removal prevent a self-consistent blanket
        // rejection from passing regardless of arbitrary operation selectors.
        let parent = state.saved(1);
        for kind in [Kind::Text, Kind::Cdata, Kind::Comment, Kind::Pi] {
            let created = state
                .structure(
                    EditLimits::default(),
                    Action::Append {
                        parent,
                        kind,
                        target: "p",
                        value: "ok",
                    },
                    true,
                )
                .unwrap();
            saved.push(created);
            if kind == Kind::Comment {
                state.structure(EditLimits::default(), Action::Remove(created), true);
            }
        }
        let root = state.saved(0);
        saved.push(
            state
                .structure(
                    EditLimits::default(),
                    Action::Append {
                        parent: root,
                        kind: Kind::Comment,
                        target: "",
                        value: "tail",
                    },
                    true,
                )
                .unwrap(),
        );
    }
    let count = usize::from(byte(7) & 31).min(input.len().saturating_sub(8) / 8);
    let start = input.len().min(8);
    let end = start + count * 8;
    let payload = &input[end..input.len().min(end + 256)];
    let limits = EditLimits {
        max_value_bytes: if byte(2) & 1 == 0 {
            EditLimits::default().max_value_bytes
        } else {
            usize::from(byte(3))
        },
        max_created_nodes: if byte(2) & 2 == 0 {
            EditLimits::default().max_created_nodes
        } else {
            usize::from(byte(5) & 63)
        },
        ..EditLimits::default()
    };
    for operation in input[start..end].chunks_exact(8) {
        let target = usize::from(operation[1] & 1);
        let opcode = operation[0] & 15;
        let handle = if matches!(opcode, 6 | 7 | 15) {
            let owner = 1 - states[target].owner;
            let foreign = states.iter().find(|state| state.owner == owner).unwrap();
            foreign.saved(usize::from(operation[2]) % foreign.handles.len())
        } else {
            saved[usize::from(operation[2]) % saved.len()]
        };
        match opcode {
            0 | 1 | 6 | 7 => {
                let start = usize::from(operation[4]).min(payload.len());
                let end = start + usize::from(operation[5]).min(payload.len() - start);
                let value = value(&payload[start..end], operation);
                let request = Request {
                    handle,
                    selector: (opcode & 1 != 0).then(|| selector(operation[3])),
                    value: &value,
                };
                states[target].session(limits, &[(request, true)], byte(6) & 1 != 0);
            }
            2 => {
                let resolved = states[target].document.node(handle.id);
                assert_eq!(
                    resolved.is_some(),
                    handle.owner == states[target].owner
                        && states[target].model.nodes[handle.node].live
                );
                if operation[6] & 1 != 0 {
                    let replacement = states[target]
                        .saved(usize::from(operation[2]) % states[target].handles.len());
                    let index = usize::from(operation[7]) % saved.len();
                    saved[index] = replacement;
                }
            }
            3 => {
                serialized(&states[target]);
            }
            4 => {
                assert_eq!(
                    states[target].document.edit(limits).err(),
                    (states[target].model.created > limits.max_created_nodes).then(|| error(
                        EditErrorKind::LimitExceeded(EditLimit::CreatedNodes),
                        None
                    ))
                );
            }
            5 => states.swap(0, 1),
            8..=11 | 15 => {
                let start = usize::from(operation[4]).min(payload.len());
                let end = start + usize::from(operation[5]).min(payload.len() - start);
                let payload = &payload[start..end];
                let data = value(payload, operation);
                let pi_target = pi_target_value(payload, operation[3]);
                let kind = [Kind::Text, Kind::Cdata, Kind::Comment, Kind::Pi][usize::from(
                    if opcode == 15 {
                        operation[4] & 3
                    } else {
                        opcode - 8
                    },
                )];
                let action = if opcode == 15 && operation[3] & 16 != 0 {
                    Action::Remove(handle)
                } else {
                    Action::Append {
                        parent: handle,
                        kind,
                        target: &pi_target,
                        value: &data,
                    }
                };
                if let Some(created) =
                    states[target].structure_with_recovery(limits, action, true, byte(6) & 1 != 0)
                {
                    saved.push(created);
                }
            }
            12 => {
                states[target].structure_with_recovery(
                    limits,
                    Action::Remove(handle),
                    true,
                    byte(6) & 1 != 0,
                );
            }
            13 => {
                saved.push(
                    states[target].saved(usize::from(operation[2]) % states[target].handles.len()),
                );
            }
            14 => {
                let count = states[target].model.created;
                let maximum =
                    [0, count.saturating_sub(1), count, count + 1][usize::from(operation[6] & 3)];
                assert_eq!(
                    states[target]
                        .document
                        .edit(EditLimits {
                            max_created_nodes: maximum,
                            ..limits
                        })
                        .err(),
                    (maximum < count).then(|| error(
                        EditErrorKind::LimitExceeded(EditLimit::CreatedNodes),
                        None
                    ))
                );
            }
            _ => unreachable!(),
        }
        for state in &states {
            state.check();
        }
        assert!(states[0].document.node(states[1].handles[0]).is_none());
        assert!(states[1].document.node(states[0].handles[0]).is_none());
    }
    for state in &states {
        serialized(state);
    }
    quota_case(byte(4), byte(5), byte(1), byte(6) & 2 != 0);
    structure_quota(byte(4) >> 3, byte(5), byte(1));
    if byte(4) == b'*' {
        byte_golden(byte(1));
    }
}

fn byte_golden(encoding: u8) {
    let mut state = seed(0, 0, encoding);
    let text = Request {
        handle: state.saved(2),
        selector: None,
        value: "\r&<>🙂",
    };
    let attribute = Request {
        handle: state.saved(1),
        selector: Some(selector(0)),
        value: "\t\n\r&<>\"",
    };
    state.session(
        EditLimits::default(),
        &[(text, true), (attribute, true)],
        false,
    );
    assert_eq!(serialized(&state), "<?xml version=\"1.0\" standalone=\"yes\"?><r xmlns=\"urn:r\" xmlns:p=\"urn:p\" xmlns:q=\"urn:q\" a=\"&#9;&#10;&#13;&amp;&lt;>&quot;\" p:a=\"P\" q:a=\"Q\">&#13;&amp;&lt;&gt;🙂<![CDATA[]]><!----><?pi?><p:c xmlns:p=\"urn:shadow\" a=\"C\"><n xmlns=\"\"/>tail</p:c></r><!--after-->".as_bytes(), "mutation changed escaping or namespace byte policy");
}

fn tiny(attribute_value: bool, encoding: u8) -> State {
    let mut nodes = vec![
        node(Kind::Document, None, &[1]),
        element("r", None, if attribute_value { &[] } else { &[2] }),
    ];
    if attribute_value {
        nodes[1].attributes.push(attribute("a", None, "x"));
    } else {
        nodes.push(node(Kind::Text, Some("x"), &[]));
    }
    State::new(
        0,
        if attribute_value {
            "<r a='x'/>"
        } else {
            "<r>x</r>"
        },
        Model {
            nodes,
            declaration: None,
            created: 0,
        },
        encoding,
    )
}

fn empty(encoding: u8) -> State {
    State::new(
        0,
        "<r/>",
        Model {
            nodes: vec![node(Kind::Document, None, &[1]), element("r", None, &[])],
            declaration: None,
            created: 0,
        },
        encoding,
    )
}

fn structural_limits(created: usize, owned: usize) -> EditLimits {
    EditLimits {
        max_created_nodes: created,
        max_owned_bytes: owned,
        ..EditLimits::default()
    }
}

fn structure_quota(case: u8, boundary: u8, encoding: u8) {
    let case = case & 7;
    let boundary = usize::from(boundary % 3);
    let mut state = if case == 5 {
        tiny(true, encoding)
    } else {
        empty(encoding)
    };
    let parent = state.saved(1);
    let append = |kind, target, value| Action::Append {
        parent,
        kind,
        target,
        value,
    };
    match case {
        0 => {
            state.structure(
                structural_limits(boundary, 0),
                append(Kind::Text, "", ""),
                true,
            );
        }
        1 => {
            let maximum = [1, 2, 3][boundary];
            state.structure(
                structural_limits(1, maximum),
                append(Kind::Text, "", "x"),
                maximum >= 2,
            );
        }
        2 => {
            // A single combined PI reservation grows baseline1 to capacity3.
            let maximum = [2, 3, 4][boundary];
            state.structure(
                structural_limits(1, maximum),
                append(Kind::Pi, "p", "x"),
                maximum >= 3,
            );
            if maximum == 2 {
                // Quota rejection must neither spend creation work nor stage
                // PI strings/capacity before the independent two-byte probe.
                state.document.edit(structural_limits(1, 0)).unwrap();
                state.structure(structural_limits(1, 1), append(Kind::Text, "", "x"), false);
                state.structure(structural_limits(1, 2), append(Kind::Text, "", "x"), true);
            }
        }
        3 => {
            let saved = state
                .structure(structural_limits(1, 0), append(Kind::Comment, "", ""), true)
                .unwrap();
            state.structure(structural_limits(1, 0), Action::Remove(saved), true);
            state.structure(
                structural_limits(boundary, 0),
                append(Kind::Cdata, "", ""),
                true,
            );
        }
        4 => {
            let saved = state
                .structure(structural_limits(1, 3), append(Kind::Pi, "p", "x"), true)
                .unwrap();
            state.structure(structural_limits(1, 3), Action::Remove(saved), true);
            assert_eq!(
                state.document.edit(structural_limits(1, 2)).err(),
                Some(error(
                    EditErrorKind::LimitExceeded(EditLimit::OwnedBytes),
                    None
                ))
            );
            assert_eq!(
                state.document.edit(structural_limits(0, 2)).err(),
                Some(error(
                    EditErrorKind::LimitExceeded(EditLimit::CreatedNodes),
                    None
                ))
            );
            let maximum = [6, 7, 8][boundary];
            state.structure(
                structural_limits(2, maximum),
                append(Kind::Text, "", "x"),
                maximum >= 7,
            );
            if maximum == 6 {
                state.structure(
                    structural_limits(2, maximum),
                    append(Kind::Cdata, "", ""),
                    true,
                );
            }
        }
        5 => {
            let saved = state
                .structure(structural_limits(1, 0), append(Kind::Text, "", "x"), true)
                .unwrap();
            state.structure(structural_limits(1, 0), Action::Remove(saved), true);
            state.structure(structural_limits(1, 0), append(Kind::Text, "", ""), true);
            state.structure(structural_limits(2, 0), append(Kind::Comment, "", ""), true);
        }
        6 => {
            state.structure(
                EditLimits {
                    max_value_bytes: [5, 6, 7][boundary],
                    ..EditLimits::default()
                },
                append(Kind::Pi, "é🙂", "é🙂"),
                true,
            );
        }
        7 => {
            let limits = structural_limits(0, 0);
            for action in [
                append(Kind::Pi, "1\0", ""),
                append(Kind::Pi, "1", "\0"),
                append(Kind::Cdata, "", "]]>\0"),
                append(Kind::Text, "", "x"),
            ] {
                state.structure(limits, action, true);
            }
        }
        _ => unreachable!(),
    }
    serialized(&state);
}

fn quota_case(case: u8, boundary: u8, encoding: u8, reopen: bool) {
    let case = case & 7;
    let mut state = tiny(matches!(case, 2 | 3), encoding);
    let request = Request {
        handle: state.saved(if matches!(case, 2 | 3) { 1 } else { 2 }),
        selector: matches!(case, 2 | 3).then(|| selector(0)),
        value: match case {
            2 => "ab",
            4 => "🙂",
            7 => "",
            _ => "x",
        },
    };
    let limits = |maximum| EditLimits {
        max_owned_bytes: maximum,
        ..EditLimits::default()
    };
    match case {
        0 => {
            let maximum = [2, 3, 4][usize::from(boundary % 3)];
            state.session(limits(maximum), &[(request, maximum >= 3)], false);
        }
        1 => {
            // Baseline len/cap=2. Requests are 4, then 6 (clipped by the
            // remaining allowance): four appends fit and the fifth must fail.
            let actions = [
                (request, true),
                (request, true),
                (request, true),
                (request, true),
                (request, false),
            ];
            if reopen {
                for action in actions {
                    state.session(limits(10), &[action], false);
                }
            } else {
                state.session(limits(10), &actions, false);
            }
        }
        2 => {
            let maximum = [4, 5, 6][usize::from(boundary % 3)];
            state.session(limits(maximum), &[(request, maximum >= 5)], false);
        }
        3 => state.session(limits(0), &[(request, true), (request, false)], false),
        4 => state.session(
            EditLimits {
                max_value_bytes: [3, 4, 5][usize::from(boundary % 3)],
                ..EditLimits::default()
            },
            &[(request, true)],
            false,
        ),
        5 => {
            let invalid = Request {
                value: "\0",
                ..request
            };
            state.session(limits(3), &[(invalid, true)], false);
            // Invalid values neither spend the allowance nor grow spare
            // capacity. A zero-ceiling follow-up probes both independently.
            state.document.edit(limits(0)).unwrap();
            state.session(limits(0), &[(request, false)], false);
            state.session(limits(3), &[(request, true)], false);
        }
        6 => {
            state.session(limits(3), &[(request, true)], false);
            assert!(
                matches!(
                    state.document.edit(limits(2)),
                    Err(EditError {
                        kind: EditErrorKind::LimitExceeded(EditLimit::OwnedBytes),
                        value_offset: None,
                        ..
                    })
                ),
                "opening an editor reset cumulative expenditure"
            );
            state.check();
            let maximum = [6, 7, 8][usize::from(boundary % 3)];
            let request = Request {
                value: "b",
                ..request
            };
            state.session(limits(maximum), &[(request, maximum >= 7)], false);
        }
        7 => state.session(
            EditLimits {
                max_value_bytes: 0,
                max_owned_bytes: 0,
                ..EditLimits::default()
            },
            &[(request, true)],
            false,
        ),
        _ => unreachable!(),
    }
    let output = serialized(&state);
    let expected = match case {
        2 if !boundary.is_multiple_of(3) => b"<r a=\"ab\"/>".as_slice(),
        2 | 3 => b"<r a=\"x\"/>",
        4 if !boundary.is_multiple_of(3) => "<r>🙂</r>".as_bytes(),
        6 if !boundary.is_multiple_of(3) => b"<r>b</r>",
        7 => b"<r></r>",
        _ => b"<r>x</r>",
    };
    assert_eq!(output, expected, "mutation changed literal output policy");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_name_oracle_has_literal_happy_and_sad_boundaries() {
        for valid in [
            "p",
            "é🙂",
            "xml-stylesheet",
            "_p.2",
            "a\u{b7}",
            "a\u{300}",
            "\u{37d}",
            "\u{37f}",
            "\u{effff}",
        ] {
            assert_eq!(expected_target(valid), Ok(()));
        }
        for (invalid, kind, offset) in [
            ("", EditErrorKind::InvalidName, 0),
            ("1p", EditErrorKind::InvalidName, 0),
            ("p:x", EditErrorKind::InvalidName, 1),
            ("é🙂!", EditErrorKind::InvalidName, 6),
            ("a b", EditErrorKind::InvalidName, 1),
            ("\u{300}", EditErrorKind::InvalidName, 0),
            ("\u{37e}", EditErrorKind::InvalidName, 0),
            ("\u{f0000}", EditErrorKind::InvalidName, 0),
            ("1\0", EditErrorKind::InvalidCharacter, 1),
            ("é\u{ffff}", EditErrorKind::InvalidCharacter, 2),
        ] {
            assert_eq!(expected_target(invalid), Err(error(kind, Some(offset))));
        }
        for bits in 0..8 {
            let value: String = "xml"
                .bytes()
                .enumerate()
                .map(|(index, byte)| {
                    char::from(if bits & (1 << index) == 0 {
                        byte
                    } else {
                        byte.to_ascii_uppercase()
                    })
                })
                .collect();
            assert_eq!(
                expected_target(&value),
                Err(error(EditErrorKind::InvalidName, Some(0)))
            );
        }
    }

    #[test]
    fn structural_model_predicts_every_semantic_error_before_mutation() {
        let mut state = seed(0, 0, 0);
        let parent = state.saved(1);
        let append = |parent, kind, target, value| Action::Append {
            parent,
            kind,
            target,
            value,
        };
        let normal = EditLimits::default();
        for (action, limits, expected) in [
            (
                append(
                    Saved {
                        node: usize::MAX,
                        ..parent
                    },
                    Kind::Pi,
                    "",
                    "\0",
                ),
                normal,
                error(EditErrorKind::InvalidHandle, None),
            ),
            (
                append(Saved { owner: 1, ..parent }, Kind::Pi, "", "\0"),
                normal,
                error(EditErrorKind::InvalidHandle, None),
            ),
            (
                append(state.saved(2), Kind::Pi, "", "\0"),
                normal,
                error(EditErrorKind::WrongNodeKind, None),
            ),
            (
                append(state.saved(0), Kind::Text, "", ""),
                normal,
                error(EditErrorKind::WrongNodeKind, None),
            ),
            (
                append(parent, Kind::Pi, "", "\0"),
                EditLimits {
                    max_value_bytes: 0,
                    ..normal
                },
                error(EditErrorKind::LimitExceeded(EditLimit::ValueBytes), None),
            ),
            (
                append(parent, Kind::Pi, "1\0", ""),
                normal,
                error(EditErrorKind::InvalidCharacter, Some(1)),
            ),
            (
                append(parent, Kind::Pi, "1", "\0"),
                normal,
                error(EditErrorKind::InvalidName, Some(0)),
            ),
            (
                append(parent, Kind::Pi, "p", " \0"),
                normal,
                error(EditErrorKind::InvalidCharacter, Some(1)),
            ),
            (
                append(parent, Kind::Pi, "p", "?>"),
                normal,
                error(EditErrorKind::InvalidValue, Some(0)),
            ),
            (
                append(parent, Kind::Text, "", ""),
                structural_limits(0, 0),
                error(EditErrorKind::LimitExceeded(EditLimit::CreatedNodes), None),
            ),
            (
                Action::Remove(state.saved(0)),
                normal,
                error(EditErrorKind::ProtectedNode, None),
            ),
            (
                Action::Remove(parent),
                normal,
                error(EditErrorKind::ProtectedNode, None),
            ),
        ] {
            assert_eq!(
                state.model.expected_structure(0, action, limits),
                Err(expected)
            );
        }
        assert_eq!(
            state
                .model
                .expected_structure(0, Action::Remove(state.saved(6)), normal),
            Ok(())
        );
        assert_eq!(state.model.remove(6), [6, 7, 8]);
        for index in [6, 7, 8] {
            assert_eq!(
                state
                    .model
                    .expected_structure(0, Action::Remove(state.saved(index)), normal),
                Err(error(EditErrorKind::InvalidHandle, None))
            );
            assert_eq!(
                state.model.expected(
                    0,
                    Request {
                        handle: state.saved(index),
                        selector: None,
                        value: "\0"
                    },
                    normal
                ),
                Err(error(EditErrorKind::InvalidHandle, None))
            );
        }
        assert_eq!(state.model.subtree(0), [0, 1, 2, 3, 4, 5, 9]);
        let id = state.model.append(1, Kind::Pi, "p", "");
        assert_eq!(id, 10);
        assert_eq!(state.model.created, 1);
        assert_eq!(state.model.nodes[id].target.as_deref(), Some("p"));
        assert_eq!(state.model.remove(id), [id]);
        assert_eq!(state.model.created, 1);
        assert_eq!(state.model.append(1, Kind::Text, "", "x"), 11);
        assert_eq!(state.model.created, 2);
    }

    #[test]
    fn recovery_model_selects_existing_attributes_or_leaves_and_can_have_no_operand() {
        let empty = empty(0);
        assert!(empty.model.rescue(empty.owner, &empty.handles).is_none());
        let attribute = tiny(true, 0);
        let rescue = attribute
            .model
            .rescue(attribute.owner, &attribute.handles)
            .unwrap();
        assert_eq!(rescue.handle.node, 1);
        assert!(rescue.selector.is_some());
        assert_eq!(
            attribute
                .model
                .expected(attribute.owner, rescue, structural_limits(0, 0)),
            Ok(())
        );
        let mut text = tiny(false, 0);
        let rescue = text.model.rescue(text.owner, &text.handles).unwrap();
        assert_eq!(rescue.handle.node, 2);
        assert!(rescue.selector.is_none());
        assert_eq!(
            text.model
                .expected(text.owner, rescue, structural_limits(0, 0)),
            Ok(())
        );
        text.model.remove(2);
        assert!(text.model.rescue(text.owner, &text.handles).is_none());
    }
}
