#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::ops::Range;

use yaxml::{
    EditError, EditErrorKind, EditLimit, EditLimits, Editor, Limits, Name, OwnedDocument,
    OwnedNodeId, OwnedNodeKind, Writer,
};

#[derive(Clone, Debug, Eq, PartialEq)]
struct NameValue(String, Option<String>, String, Option<String>);

fn name(value: Name<'_>) -> NameValue {
    NameValue(
        value.qualified().into(),
        value.prefix().map(str::to_owned),
        value.local().into(),
        value.namespace().map(str::to_owned),
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Value {
    kind: OwnedNodeKind,
    name: Option<NameValue>,
    attributes: Vec<(NameValue, String)>,
    text: Option<String>,
    target: Option<String>,
}

#[derive(Clone, Debug)]
struct Node {
    value: Value,
    parent: Option<OwnedNodeId>,
    children: Vec<OwnedNodeId>,
    range: Option<Range<usize>>,
}

// Expected topology uses ordered child vectors, never production sibling links,
// arena slots, subtree sizes, validation helpers or the mutation implementation.
struct Model {
    root: OwnedNodeId,
    declaration: Option<(String, Option<String>, Option<bool>)>,
    nodes: HashMap<OwnedNodeId, Node>,
    removed: Vec<OwnedNodeId>,
}

impl Model {
    fn initial(document: &OwnedDocument) -> Self {
        Self {
            root: document.root().id(),
            declaration: declaration(document),
            nodes: document
                .nodes()
                .map(|node| {
                    (
                        node.id(),
                        Node {
                            value: value(node),
                            parent: node.parent().map(|n| n.id()),
                            children: node.children().map(|n| n.id()).collect(),
                            range: node.range(),
                        },
                    )
                })
                .collect(),
            removed: Vec::new(),
        }
    }

    fn preorder(&self, root: OwnedNodeId) -> Vec<OwnedNodeId> {
        let mut result = Vec::new();
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            result.push(id);
            pending.extend(self.nodes[&id].children.iter().rev().copied());
        }
        result
    }

    fn append(
        &mut self,
        parent: OwnedNodeId,
        id: OwnedNodeId,
        leaf: Leaf,
        data: &str,
        target: Option<&str>,
    ) {
        assert!(!self.nodes.contains_key(&id));
        assert!(!self.removed.contains(&id), "a removed slot was reused");
        self.nodes.get_mut(&parent).unwrap().children.push(id);
        self.nodes.insert(
            id,
            Node {
                value: Value {
                    kind: leaf.kind(),
                    name: None,
                    attributes: Vec::new(),
                    text: Some(data.into()),
                    target: target.map(str::to_owned),
                },
                parent: Some(parent),
                children: Vec::new(),
                range: None,
            },
        );
    }

    fn remove(&mut self, id: OwnedNodeId) {
        let parent = self.nodes[&id].parent.unwrap();
        self.nodes
            .get_mut(&parent)
            .unwrap()
            .children
            .retain(|child| *child != id);
        for dead in self.preorder(id) {
            self.nodes.remove(&dead).unwrap();
            self.removed.push(dead);
        }
    }

    fn check(&self, document: &OwnedDocument) {
        assert_eq!(declaration(document), self.declaration);
        assert_eq!(document.root().id(), self.root);
        let element = self.nodes[&self.root]
            .children
            .iter()
            .find(|id| self.nodes[id].value.kind == OwnedNodeKind::Element)
            .unwrap();
        assert_eq!(document.root_element().id(), *element);
        exact(document.nodes(), &self.preorder(self.root));
        let mut next = HashMap::new();
        for node in self.nodes.values() {
            for pair in node.children.windows(2) {
                next.insert(pair[0], pair[1]);
            }
        }
        for (&id, expected) in &self.nodes {
            let actual = document.node(id).expect("live model handle became stale");
            assert_eq!(value(actual), expected.value);
            assert_eq!(actual.range(), expected.range);
            assert_eq!(actual.parent().map(|n| n.id()), expected.parent);
            assert_eq!(
                actual.first_child().map(|n| n.id()),
                expected.children.first().copied()
            );
            assert_eq!(
                actual.last_child().map(|n| n.id()),
                expected.children.last().copied()
            );
            assert_eq!(
                actual.next_sibling().map(|n| n.id()),
                next.get(&id).copied()
            );
            assert_eq!(
                actual.children().map(|n| n.id()).collect::<Vec<_>>(),
                expected.children
            );
            exact(actual.descendants(), &self.preorder(id));
        }
        for id in &self.removed {
            assert!(document.node(*id).is_none(), "removed handle resolves");
        }
    }

    fn round_trip(&self, document: &OwnedDocument) {
        let bytes = output(document);
        let reparsed = OwnedDocument::parse_bytes(&bytes).unwrap();
        let reparsed_model = Model::initial(&reparsed);
        assert_eq!(self.projected(), reparsed_model.projected());
        let mut expected = self.declaration.clone();
        if let Some((_, Some(encoding), _)) = &mut expected {
            *encoding = "UTF-8".into();
        }
        assert_eq!(declaration(&reparsed), expected);
    }

    fn projected(&self) -> Vec<(usize, Value)> {
        let mut result: Vec<(usize, Value)> = Vec::new();
        let mut previous_parent = None;
        let mut pending = vec![(self.root, 0)];
        while let Some((id, depth)) = pending.pop() {
            let node = &self.nodes[&id];
            pending.extend(node.children.iter().rev().map(|child| (*child, depth + 1)));
            if node.value.kind == OwnedNodeKind::Text {
                let text = node.value.text.as_deref().unwrap();
                if text.is_empty() {
                    continue;
                }
                if previous_parent == node.parent
                    && let Some((_, prior)) = result.last_mut()
                    && prior.kind == OwnedNodeKind::Text
                {
                    prior.text.as_mut().unwrap().push_str(text);
                    continue;
                }
            }
            previous_parent = node.parent;
            result.push((depth, node.value.clone()));
        }
        result
    }
}

fn declaration(document: &OwnedDocument) -> Option<(String, Option<String>, Option<bool>)> {
    document.declaration().map(|d| {
        (
            d.version.into(),
            d.encoding.map(str::to_owned),
            d.standalone,
        )
    })
}

fn value(node: yaxml::OwnedNode<'_>) -> Value {
    Value {
        kind: node.kind(),
        name: node.name().map(name),
        attributes: node
            .attributes()
            .map(|a| (name(a.name), a.value.into_owned()))
            .collect(),
        text: node.text().map(str::to_owned),
        target: node.processing_instruction_target().map(str::to_owned),
    }
}

fn exact<'a>(
    mut actual: impl ExactSizeIterator<Item = yaxml::OwnedNode<'a>>,
    expected: &[OwnedNodeId],
) {
    for (index, id) in expected.iter().enumerate() {
        assert_eq!(actual.len(), expected.len() - index);
        assert_eq!(
            actual.size_hint(),
            (expected.len() - index, Some(expected.len() - index))
        );
        assert_eq!(actual.next().unwrap().id(), *id);
    }
    assert_eq!(actual.len(), 0);
    assert!(actual.next().is_none());
    assert!(actual.next().is_none());
}

#[derive(Clone, Copy, Debug)]
enum Leaf {
    Text,
    Cdata,
    Comment,
    Pi,
}

const LEAVES: [Leaf; 4] = [Leaf::Text, Leaf::Cdata, Leaf::Comment, Leaf::Pi];

impl Leaf {
    fn kind(self) -> OwnedNodeKind {
        match self {
            Self::Text => OwnedNodeKind::Text,
            Self::Cdata => OwnedNodeKind::Cdata,
            Self::Comment => OwnedNodeKind::Comment,
            Self::Pi => OwnedNodeKind::ProcessingInstruction,
        }
    }
    fn append(
        self,
        editor: &mut Editor<'_>,
        parent: OwnedNodeId,
        data: &str,
    ) -> Result<OwnedNodeId, EditError> {
        match self {
            Self::Text => editor.append_text(parent, data),
            Self::Cdata => editor.append_cdata(parent, data),
            Self::Comment => editor.append_comment(parent, data),
            Self::Pi => editor.append_processing_instruction(parent, "p", data),
        }
    }
}

fn append(
    editor: &mut Editor<'_>,
    model: &mut Model,
    parent: OwnedNodeId,
    leaf: Leaf,
    data: &str,
) -> OwnedNodeId {
    let id = leaf.append(editor, parent, data).unwrap();
    model.append(
        parent,
        id,
        leaf,
        data,
        matches!(leaf, Leaf::Pi).then_some("p"),
    );
    model.check(editor.document());
    id
}

fn remove(editor: &mut Editor<'_>, model: &mut Model, id: OwnedNodeId) {
    editor.remove_subtree(id).unwrap();
    model.remove(id);
    model.check(editor.document());
}

fn encoded(input: &str, mode: usize) -> Vec<u8> {
    if mode == 0 {
        return input.as_bytes().to_vec();
    }
    std::iter::once(0xfeff)
        .chain(input.replace("uTf-8", "UTF-16").encode_utf16())
        .flat_map(|unit| {
            if mode == 1 {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            }
        })
        .collect()
}

fn parse(input: &str, mode: usize) -> OwnedDocument {
    OwnedDocument::parse_bytes(&encoded(input, mode)).unwrap()
}

fn output(document: &OwnedDocument) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new());
    writer.write_owned_document(document).unwrap();
    writer.finish().unwrap();
    let position = writer.position();
    let bytes = writer.into_inner();
    assert_eq!(position, bytes.len());
    bytes
}

fn error(kind: EditErrorKind, value_offset: Option<usize>) -> EditError {
    EditError {
        kind,
        value_offset,
        location: None,
    }
}
fn quota(limit: EditLimit) -> EditError {
    error(EditErrorKind::LimitExceeded(limit), None)
}
fn limits(created: usize, owned: usize) -> EditLimits {
    EditLimits {
        max_created_nodes: created,
        max_owned_bytes: owned,
        ..EditLimits::default()
    }
}

#[test]
fn golden_document_order_differs_from_new_slot_order_and_survivors_keep_source_ranges() {
    const INPUT: &str = "<!--pre--><r><a>A</a><b>B</b></r><!--post-->";
    for mode in 0..3 {
        let mut document = parse(INPUT, mode);
        let root = document.root().id();
        let element = document.root_element().id();
        let ids: Vec<_> = document.nodes().map(|n| n.id()).collect();
        let mut model = Model::initial(&document);
        let post_start = INPUT.find("<!--post-->").unwrap();
        let post_range = if mode == 0 {
            post_start..INPUT.len()
        } else {
            2 + 2 * post_start..2 + 2 * INPUT.len()
        };
        assert_eq!(document.node(ids[7]).unwrap().range(), Some(post_range));
        let mut editor = document.edit(EditLimits::default()).unwrap();
        let text = append(&mut editor, &mut model, element, Leaf::Text, "x");
        assert_eq!(
            editor
                .document()
                .nodes()
                .map(|n| n.id())
                .collect::<Vec<_>>(),
            [
                ids[0], ids[1], ids[2], ids[3], ids[4], ids[5], ids[6], text, ids[7]
            ]
        );
        assert_eq!(
            output(editor.document()),
            b"<!--pre--><r><a>A</a><b>B</b>x</r><!--post-->"
        );
        append(&mut editor, &mut model, root, Leaf::Comment, "tail");
        remove(&mut editor, &mut model, ids[3]);
        assert_eq!(
            output(editor.document()),
            b"<!--pre--><r><b>B</b>x</r><!--post--><!--tail-->"
        );
        for id in [ids[3], ids[4]] {
            assert_eq!(
                editor.remove_subtree(id),
                Err(error(EditErrorKind::InvalidHandle, None))
            );
            assert_eq!(
                editor.set_node_value(id, "x"),
                Err(error(EditErrorKind::InvalidHandle, None))
            );
        }
        model.check(editor.document());
        model.round_trip(editor.document());
    }
}

#[test]
fn every_leaf_including_empty_payload_changes_topology_and_preserves_lexical_kind() {
    for mode in 0..3 {
        for (leaf, data, fragment) in [
            (Leaf::Text, "é🙂<&>\r\n\t", "é🙂&lt;&amp;&gt;&#13;\n\t"),
            (Leaf::Cdata, "中<&\n\t", "<![CDATA[中<&\n\t]]>"),
            (Leaf::Comment, "好<&\n\t", "<!--好<&\n\t-->"),
            (Leaf::Pi, "中<&\n\t", "<?p 中<&\n\t?>"),
            (Leaf::Text, "", ""),
            (Leaf::Cdata, "", "<![CDATA[]]>"),
            (Leaf::Comment, "", "<!---->"),
            (Leaf::Pi, "", "<?p?>"),
        ] {
            let mut document = parse("<r><e/></r>", mode);
            let parent = document.root_element().first_child().unwrap().id();
            let mut model = Model::initial(&document);
            let mut editor = document.edit(EditLimits::default()).unwrap();
            let id = append(&mut editor, &mut model, parent, leaf, data);
            assert_eq!(
                output(editor.document()),
                format!("<r><e>{fragment}</e></r>").as_bytes()
            );
            model.round_trip(editor.document());
            remove(&mut editor, &mut model, id);
            assert_eq!(output(editor.document()), b"<r><e/></r>");
        }
    }
}

#[test]
fn adjacent_and_empty_text_have_distinct_live_handles_and_only_text_is_projected() {
    for mode in 0..3 {
        let mut document = parse("<r>a<c/>b</r>", mode);
        let parent = document.root_element().id();
        let ids: Vec<_> = document.root_element().children().map(|n| n.id()).collect();
        let mut model = Model::initial(&document);
        let mut editor = document.edit(EditLimits::default()).unwrap();
        remove(&mut editor, &mut model, ids[1]);
        assert_eq!(
            editor
                .document()
                .root_element()
                .children()
                .map(|n| n.id())
                .collect::<Vec<_>>(),
            [ids[0], ids[2]]
        );
        let empty = append(&mut editor, &mut model, parent, Leaf::Text, "");
        let tail = append(&mut editor, &mut model, parent, Leaf::Text, "c");
        assert_ne!(empty, tail);
        assert_eq!(output(editor.document()), b"<r>abc</r>");
        model.round_trip(editor.document());
        append(&mut editor, &mut model, parent, Leaf::Cdata, "");
        append(&mut editor, &mut model, parent, Leaf::Cdata, "d");
        append(&mut editor, &mut model, parent, Leaf::Text, "e");
        assert_eq!(
            output(editor.document()),
            b"<r>abc<![CDATA[]]><![CDATA[d]]>e</r>"
        );
        model.round_trip(editor.document());
    }
}

#[test]
fn top_level_appends_follow_the_entire_epilog_and_removals_preserve_the_root() {
    for mode in 0..3 {
        let mut document = parse(
            "<?xml version='1.2' encoding='uTf-8'?><!--pre--><?before x?><r/><!--post--><?after y?>",
            mode,
        );
        let root = document.root().id();
        let top: Vec<_> = document.root().children().map(|n| n.id()).collect();
        let mut model = Model::initial(&document);
        let mut editor = document.edit(EditLimits::default()).unwrap();
        let comment = append(&mut editor, &mut model, root, Leaf::Comment, "tail");
        let pi = append(&mut editor, &mut model, root, Leaf::Pi, "z");
        assert_eq!(
            editor
                .document()
                .root()
                .children()
                .map(|n| n.id())
                .collect::<Vec<_>>(),
            [top[0], top[1], top[2], top[3], top[4], comment, pi]
        );
        for id in [top[0], top[4], top[1], top[3], pi, comment] {
            remove(&mut editor, &mut model, id);
        }
        assert_eq!(
            output(editor.document()),
            b"<?xml version=\"1.2\" encoding=\"UTF-8\"?><r/>"
        );
        model.round_trip(editor.document());
    }
}

#[test]
fn first_middle_last_sole_and_every_kind_removal_leave_exact_sibling_links() {
    for mode in 0..3 {
        for (input, index, expected) in [
            ("<r><a/><b/><c/></r>", 0, "<r><b/><c/></r>"),
            ("<r><a/><b/><c/></r>", 1, "<r><a/><c/></r>"),
            ("<r><a/><b/><c/></r>", 2, "<r><a/><b/></r>"),
            ("<r><a/></r>", 0, "<r/>"),
            (
                "<r>T<![CDATA[C]]><!--M--><?p D?><c/></r>",
                0,
                "<r><![CDATA[C]]><!--M--><?p D?><c/></r>",
            ),
            (
                "<r>T<![CDATA[C]]><!--M--><?p D?><c/></r>",
                1,
                "<r>T<!--M--><?p D?><c/></r>",
            ),
            (
                "<r>T<![CDATA[C]]><!--M--><?p D?><c/></r>",
                2,
                "<r>T<![CDATA[C]]><?p D?><c/></r>",
            ),
            (
                "<r>T<![CDATA[C]]><!--M--><?p D?><c/></r>",
                3,
                "<r>T<![CDATA[C]]><!--M--><c/></r>",
            ),
        ] {
            let mut document = parse(input, mode);
            let id = document.root_element().children().nth(index).unwrap().id();
            let mut model = Model::initial(&document);
            let mut editor = document.edit(EditLimits::default()).unwrap();
            remove(&mut editor, &mut model, id);
            assert_eq!(output(editor.document()), expected.as_bytes());
            assert_eq!(
                editor.remove_subtree(id),
                Err(error(EditErrorKind::InvalidHandle, None))
            );
            let root = model.root;
            append(&mut editor, &mut model, root, Leaf::Comment, "recovered");
            model.round_trip(editor.document());
        }
    }
}

#[test]
fn removing_namespaced_mixed_subtrees_invalidates_every_descendant_only() {
    for mode in 0..3 {
        let mut document = parse(
            "<r xmlns:p='u'><a xmlns:p='v' p:k='x'>A<b>B<![CDATA[C]]><!--D--><?p E?></b></a><p:c p:k='y'/></r>",
            mode,
        );
        let subtree = document.root_element().first_child().unwrap();
        let id = subtree.id();
        let dead: Vec<_> = subtree.descendants().map(|n| n.id()).collect();
        let mut model = Model::initial(&document);
        let mut editor = document.edit(EditLimits::default()).unwrap();
        remove(&mut editor, &mut model, id);
        for id in dead {
            assert_eq!(
                editor.set_node_value(id, "\0"),
                Err(error(EditErrorKind::InvalidHandle, None))
            );
            assert_eq!(
                editor.set_attribute_value(id, None, "k", "\0"),
                Err(error(EditErrorKind::InvalidHandle, None))
            );
            assert_eq!(
                editor.append_text(id, "\0"),
                Err(error(EditErrorKind::InvalidHandle, None))
            );
        }
        assert_eq!(
            output(editor.document()),
            b"<r xmlns:p=\"u\"><p:c p:k=\"y\"/></r>"
        );
        model.check(editor.document());
        model.round_trip(editor.document());
    }
}

#[test]
fn invalid_leaf_payloads_report_exact_byte_offsets_and_leave_complete_state_unchanged() {
    use EditErrorKind::{InvalidCharacter as Character, InvalidValue as Lexical};
    for mode in 0..3 {
        let mut document = parse("<!--pre--><r><c/></r><!--post-->", mode);
        let parent = document.root_element().id();
        let mut model = Model::initial(&document);
        let mut editor = document.edit(EditLimits::default()).unwrap();
        for leaf in LEAVES {
            for value in ["é🙂\0", "é🙂\u{ffff}"] {
                assert_eq!(
                    leaf.append(&mut editor, parent, value),
                    Err(error(Character, Some(6)))
                );
                model.check(editor.document());
            }
        }
        for (leaf, value, kind, offset) in [
            (Leaf::Cdata, "é🙂]]>", Lexical, 6),
            (Leaf::Cdata, "é🙂\r", Lexical, 6),
            (Leaf::Cdata, "]]>\0", Character, 3),
            (Leaf::Comment, "é🙂--x", Lexical, 6),
            (Leaf::Comment, "é🙂-", Lexical, 6),
            (Leaf::Comment, "é\r--", Lexical, 2),
            (Leaf::Comment, "--\0", Character, 2),
            (Leaf::Pi, "é🙂?>", Lexical, 6),
            (Leaf::Pi, "é🙂\r", Lexical, 6),
            (Leaf::Pi, " x?>", Lexical, 0),
            (Leaf::Pi, "\tx", Lexical, 0),
            (Leaf::Pi, "\nx", Lexical, 0),
            (Leaf::Pi, "\rx", Lexical, 0),
            (Leaf::Pi, " \0", Character, 1),
        ] {
            assert_eq!(
                leaf.append(&mut editor, parent, value),
                Err(error(kind, Some(offset)))
            );
            model.check(editor.document());
            append(&mut editor, &mut model, parent, leaf, "recovered");
        }
        model.round_trip(editor.document());
    }
}

#[test]
fn pi_target_names_have_independent_validation_and_precise_argument_precedence() {
    for mode in 0..3 {
        let mut document = parse("<r/>", mode);
        let parent = document.root_element().id();
        let mut model = Model::initial(&document);
        let mut editor = document.edit(EditLimits::default()).unwrap();
        for (target, kind, offset) in [
            ("", EditErrorKind::InvalidName, 0),
            ("1p", EditErrorKind::InvalidName, 0),
            ("é🙂!", EditErrorKind::InvalidName, 6),
            ("p:x", EditErrorKind::InvalidName, 1),
            (":p", EditErrorKind::InvalidName, 0),
            ("a b", EditErrorKind::InvalidName, 1),
            ("1\0", EditErrorKind::InvalidCharacter, 1),
        ] {
            assert_eq!(
                editor.append_processing_instruction(parent, target, "\0"),
                Err(error(kind, Some(offset)))
            );
            model.check(editor.document());
        }
        for bits in 0..8 {
            let target: String = "xml"
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
                editor.append_processing_instruction(parent, &target, ""),
                Err(error(EditErrorKind::InvalidName, Some(0)))
            );
            model.check(editor.document());
        }
        for target in ["é🙂", "xml-stylesheet", "XMLx", "_p", "a.b-2"] {
            let id = editor
                .append_processing_instruction(parent, target, "中")
                .unwrap();
            model.append(parent, id, Leaf::Pi, "中", Some(target));
            model.check(editor.document());
        }
        model.round_trip(editor.document());
    }
    let mut document = OwnedDocument::parse("<r/>").unwrap();
    let parent = document.root_element().id();
    let model = Model::initial(&document);
    let mut editor = document
        .edit(EditLimits {
            max_value_bytes: 0,
            ..EditLimits::default()
        })
        .unwrap();
    assert_eq!(
        editor.append_processing_instruction(parent, "", "\0"),
        Err(quota(EditLimit::ValueBytes))
    );
    model.check(editor.document());
}

#[test]
fn placement_handle_and_protection_checks_precede_payloads_and_quotas() {
    for mode in 0..3 {
        let mut document = parse("<r>x<![CDATA[c]]><!--m--><?p d?><dead/></r>", mode);
        let root = document.root().id();
        let parent = document.root_element().id();
        let children: Vec<_> = document.root_element().children().map(|n| n.id()).collect();
        let foreign = parse("<r/>", mode);
        let mut model = Model::initial(&document);
        let mut editor = document
            .edit(EditLimits {
                max_value_bytes: 0,
                ..limits(1, 0)
            })
            .unwrap();
        remove(&mut editor, &mut model, children[4]);
        for id in [
            foreign.root().id(),
            foreign.root_element().id(),
            children[4],
        ] {
            for leaf in LEAVES {
                assert_eq!(
                    leaf.append(&mut editor, id, "\0"),
                    Err(error(EditErrorKind::InvalidHandle, None))
                );
            }
            assert_eq!(
                editor.append_processing_instruction(id, "", "\0"),
                Err(error(EditErrorKind::InvalidHandle, None))
            );
            assert_eq!(
                editor.remove_subtree(id),
                Err(error(EditErrorKind::InvalidHandle, None))
            );
            model.check(editor.document());
        }
        for &id in &children[..4] {
            for leaf in LEAVES {
                assert_eq!(
                    leaf.append(&mut editor, id, "\0"),
                    Err(error(EditErrorKind::WrongNodeKind, None))
                );
            }
        }
        assert_eq!(
            editor.append_text(root, "\0"),
            Err(error(EditErrorKind::WrongNodeKind, None))
        );
        assert_eq!(
            editor.append_cdata(root, "\0"),
            Err(error(EditErrorKind::WrongNodeKind, None))
        );
        for id in [root, parent] {
            assert_eq!(
                editor.remove_subtree(id),
                Err(error(EditErrorKind::ProtectedNode, None))
            );
        }
        model.check(editor.document());
        append(&mut editor, &mut model, root, Leaf::Comment, "");
    }
}

#[test]
fn byte_and_created_node_ceilings_are_inclusive_and_do_not_mask_validation() {
    for mode in 0..3 {
        for ceiling in [5, 6, 7] {
            for leaf in LEAVES {
                let mut document = parse("<r/>", mode);
                let parent = document.root_element().id();
                let mut model = Model::initial(&document);
                let mut editor = document
                    .edit(EditLimits {
                        max_value_bytes: ceiling,
                        ..EditLimits::default()
                    })
                    .unwrap();
                let result = leaf.append(&mut editor, parent, "é🙂");
                if ceiling == 5 {
                    assert_eq!(result, Err(quota(EditLimit::ValueBytes)));
                } else {
                    model.append(
                        parent,
                        result.unwrap(),
                        leaf,
                        "é🙂",
                        matches!(leaf, Leaf::Pi).then_some("p"),
                    );
                }
                model.check(editor.document());
            }
            let mut document = parse("<r/>", mode);
            let parent = document.root_element().id();
            let mut model = Model::initial(&document);
            let mut editor = document
                .edit(EditLimits {
                    max_value_bytes: ceiling,
                    ..EditLimits::default()
                })
                .unwrap();
            let result = editor.append_processing_instruction(parent, "é🙂", "é🙂");
            if ceiling == 5 {
                assert_eq!(result, Err(quota(EditLimit::ValueBytes)));
            } else {
                model.append(parent, result.unwrap(), Leaf::Pi, "é🙂", Some("é🙂"));
            }
            model.check(editor.document());
        }
        for ceiling in [0, 1, 2] {
            let mut document = parse("<r/>", mode);
            let parent = document.root_element().id();
            let mut model = Model::initial(&document);
            let mut editor = document.edit(limits(ceiling, 0)).unwrap();
            assert_eq!(
                editor.append_text(parent, "é\0"),
                Err(error(EditErrorKind::InvalidCharacter, Some(2)))
            );
            let result = editor.append_text(parent, "");
            if ceiling == 0 {
                assert_eq!(result, Err(quota(EditLimit::CreatedNodes)));
            } else {
                model.append(parent, result.unwrap(), Leaf::Text, "", None);
            }
            model.check(editor.document());
        }
        let mut document = parse("<r/>", mode);
        let parent = document.root_element().id();
        let model = Model::initial(&document);
        let mut editor = document.edit(limits(0, 0)).unwrap();
        assert_eq!(
            editor.append_text(parent, "x"),
            Err(quota(EditLimit::CreatedNodes))
        );
        model.check(editor.document());
    }
}

#[test]
fn exact_pool_requests_and_pi_staging_are_shared_and_never_refunded() {
    for mode in 0..3 {
        for ceiling in [1, 2, 3] {
            let mut document = parse("<r/>", mode);
            let parent = document.root_element().id();
            let mut model = Model::initial(&document);
            let mut editor = document.edit(limits(1, ceiling)).unwrap();
            let result = editor.append_text(parent, "x");
            if ceiling == 1 {
                assert_eq!(result, Err(quota(EditLimit::OwnedBytes)));
                model.check(editor.document());
                append(&mut editor, &mut model, parent, Leaf::Comment, "");
            } else {
                model.append(parent, result.unwrap(), Leaf::Text, "x", None);
            }
            model.check(editor.document());
        }
        for ceiling in [2, 3, 4] {
            let mut document = parse("<r/>", mode);
            let parent = document.root_element().id();
            let mut model = Model::initial(&document);
            let mut editor = document.edit(limits(1, ceiling)).unwrap();
            let result = editor.append_processing_instruction(parent, "p", "x");
            if ceiling == 2 {
                assert_eq!(result, Err(quota(EditLimit::OwnedBytes)));
            } else {
                model.append(parent, result.unwrap(), Leaf::Pi, "x", Some("p"));
            }
            model.check(editor.document());
        }
        for ceiling in [6, 7, 8] {
            let mut document = parse("<r/>", mode);
            let parent = document.root_element().id();
            let mut model = Model::initial(&document);
            {
                let mut editor = document.edit(limits(1, 3)).unwrap();
                let id = append(&mut editor, &mut model, parent, Leaf::Pi, "x");
                remove(&mut editor, &mut model, id);
            }
            assert_eq!(
                document.edit(limits(1, 2)).err().unwrap(),
                quota(EditLimit::OwnedBytes)
            );
            assert_eq!(
                document.edit(limits(0, 3)).err().unwrap(),
                quota(EditLimit::CreatedNodes)
            );
            {
                let mut editor = document.edit(limits(1, 3)).unwrap();
                assert_eq!(
                    editor.append_text(parent, ""),
                    Err(quota(EditLimit::CreatedNodes))
                );
                model.check(editor.document());
            }
            let mut editor = document.edit(limits(2, ceiling)).unwrap();
            let result = editor.append_text(parent, "x");
            if ceiling == 6 {
                assert_eq!(result, Err(quota(EditLimit::OwnedBytes)));
                model.check(editor.document());
                append(&mut editor, &mut model, parent, Leaf::Cdata, "");
            } else {
                model.append(parent, result.unwrap(), Leaf::Text, "x", None);
            }
            model.check(editor.document());
        }
    }
}

#[test]
fn parsed_spare_capacity_and_removed_slots_do_not_reset_creation_work() {
    for mode in 0..3 {
        let mut document = parse("<r a='x'/>", mode);
        let parent = document.root_element().id();
        let mut model = Model::initial(&document);
        {
            let mut editor = document.edit(limits(1, 0)).unwrap();
            let id = append(&mut editor, &mut model, parent, Leaf::Text, "y");
            remove(&mut editor, &mut model, id);
            assert_eq!(
                editor.append_text(parent, ""),
                Err(quota(EditLimit::CreatedNodes))
            );
        }
        let mut editor = document.edit(limits(2, 0)).unwrap();
        append(&mut editor, &mut model, parent, Leaf::Comment, "");
        model.round_trip(editor.document());
    }
}

#[test]
fn moved_documents_keep_created_handles_and_foreign_documents_reject_them() {
    for mode in 0..3 {
        let mut source = encoded("<r/>", mode);
        let mut document = OwnedDocument::parse_bytes(&source).unwrap();
        let parent = document.root_element().id();
        let mut model = Model::initial(&document);
        let id = {
            let mut editor = document.edit(EditLimits::default()).unwrap();
            append(&mut editor, &mut model, parent, Leaf::Text, "é🙂")
        };
        source.fill(0);
        drop(source);
        let mut parked = Some(document);
        let mut documents = vec![parked.take().unwrap()];
        let mut document = documents.pop().unwrap();
        model.check(&document);
        assert_eq!(document.node(id).unwrap().range(), None);
        let mut foreign = parse("<r/>", mode);
        assert!(foreign.node(id).is_none());
        assert_eq!(
            foreign
                .edit(EditLimits::default())
                .unwrap()
                .remove_subtree(id),
            Err(error(EditErrorKind::InvalidHandle, None))
        );
        let mut editor = document.edit(EditLimits::default()).unwrap();
        remove(&mut editor, &mut model, id);
        let replacement = append(&mut editor, &mut model, parent, Leaf::Text, "new");
        assert_ne!(replacement, id);
        assert_eq!(output(editor.document()), b"<r>new</r>");
    }
}

#[test]
fn deterministic_append_remove_sequences_preserve_the_independent_live_model() {
    let long = "é中🙂".repeat(32);
    for mode in 0..3 {
        let mut document = parse("<r><a/><b/></r>", mode);
        let parents: Vec<_> = document.root_element().children().map(|n| n.id()).collect();
        let mut model = Model::initial(&document);
        let mut saved = Vec::new();
        let mut editor = document.edit(EditLimits::default()).unwrap();
        for index in 0..48 {
            let leaf = LEAVES[index % LEAVES.len()];
            let data = ["", "x", "é🙂", "<&\n\t", long.as_str()][index % 5];
            let id = append(&mut editor, &mut model, parents[index % 2], leaf, data);
            saved.push(id);
            if index % 3 == 2 {
                remove(&mut editor, &mut model, saved[index - 1]);
            }
            model.round_trip(editor.document());
        }
    }
    let mut document = OwnedDocument::parse("<r/>").unwrap();
    let parent = document.root_element().id();
    let mut model = Model::initial(&document);
    for _ in 0..64 {
        let mut editor = document.edit(limits(64, 0)).unwrap();
        let id = append(&mut editor, &mut model, parent, Leaf::Comment, "");
        remove(&mut editor, &mut model, id);
    }
    let mut editor = document.edit(limits(64, 0)).unwrap();
    assert_eq!(
        editor.append_comment(parent, ""),
        Err(quota(EditLimit::CreatedNodes))
    );
    model.check(editor.document());
}

#[test]
fn deep_removal_and_wide_tombstone_navigation_are_iterative() {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 8192;
            let input = format!(
                "<r>{}x{}<keep/></r>",
                "<n>".repeat(DEPTH),
                "</n>".repeat(DEPTH)
            );
            let mut document = OwnedDocument::with_limits(
                input.as_bytes(),
                Limits {
                    max_depth: DEPTH + 1,
                    ..Limits::default()
                },
            )
            .unwrap();
            let subtree = document.root_element().first_child().unwrap();
            let id = subtree.id();
            let dead: Vec<_> = subtree.descendants().map(|n| n.id()).collect();
            assert_eq!(dead.len(), DEPTH + 1);
            let keep = document.root_element().last_child().unwrap().id();
            let range = document.node(keep).unwrap().range();
            let mut editor = document.edit(EditLimits::default()).unwrap();
            editor.remove_subtree(id).unwrap();
            for id in dead {
                assert!(editor.document().node(id).is_none());
            }
            assert_eq!(editor.document().nodes().len(), 3);
            assert_eq!(editor.document().node(keep).unwrap().range(), range);
            assert_eq!(output(editor.document()), b"<r><keep/></r>");
        })
        .unwrap()
        .join()
        .unwrap();

    let mut input = String::from("<r>");
    for index in 0..10_000 {
        input.push_str(&format!("<c a='{index}'/>"));
    }
    input.push_str("</r>");
    let mut document = OwnedDocument::parse(&input).unwrap();
    let parent = document.root_element().id();
    let children: Vec<_> = document.root_element().children().map(|n| n.id()).collect();
    let mut model = Model::initial(&document);
    let mut editor = document.edit(EditLimits::default()).unwrap();
    append(&mut editor, &mut model, parent, Leaf::Comment, "tail");
    for index in [0, 4_999, 9_999] {
        remove(&mut editor, &mut model, children[index]);
    }
    let mut expected = String::from("<r>");
    for index in 0..10_000 {
        if ![0, 4_999, 9_999].contains(&index) {
            expected.push_str(&format!("<c a=\"{index}\"/>"));
        }
    }
    expected.push_str("<!--tail--></r>");
    assert_eq!(output(editor.document()), expected.as_bytes());
    model.round_trip(editor.document());
}
