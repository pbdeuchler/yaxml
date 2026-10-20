#![forbid(unsafe_code)]

use std::ops::Range;

use yaxml::{
    EditError, EditErrorKind, EditLimit, EditLimits, Editor, Limits, Name, OwnedDocument,
    OwnedNodeId, OwnedNodeKind, Writer, WriterLimits,
};

const XMLNS: &str = "http://www.w3.org/2000/xmlns/";

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
    children: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NodeState {
    id: OwnedNodeId,
    range: Option<Range<usize>>,
    parent: Option<OwnedNodeId>,
    first: Option<OwnedNodeId>,
    last: Option<OwnedNodeId>,
    next: Option<OwnedNodeId>,
    children: Vec<OwnedNodeId>,
    value: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct State {
    declaration: Option<(String, Option<String>, Option<bool>)>,
    nodes: Vec<NodeState>,
}

fn snapshot(document: &OwnedDocument) -> State {
    State {
        declaration: document.declaration().map(|d| {
            (
                d.version.into(),
                d.encoding.map(str::to_owned),
                d.standalone,
            )
        }),
        nodes: document
            .nodes()
            .map(|node| NodeState {
                id: node.id(),
                range: node.range(),
                parent: node.parent().map(|n| n.id()),
                first: node.first_child().map(|n| n.id()),
                last: node.last_child().map(|n| n.id()),
                next: node.next_sibling().map(|n| n.id()),
                children: node.children().map(|n| n.id()).collect(),
                value: Value {
                    kind: node.kind(),
                    name: node.name().map(name),
                    attributes: node
                        .attributes()
                        .map(|a| (name(a.name), a.value.into_owned()))
                        .collect(),
                    text: node.text().map(str::to_owned),
                    target: node.processing_instruction_target().map(str::to_owned),
                    children: node.children().count(),
                },
            })
            .collect(),
    }
}

fn expected_node(state: &mut State, id: OwnedNodeId) -> &mut Value {
    &mut state.nodes.iter_mut().find(|n| n.id == id).unwrap().value
}

fn set_leaf(editor: &mut Editor<'_>, expected: &mut State, id: OwnedNodeId, value: &str) {
    editor.set_node_value(id, value).unwrap();
    expected_node(expected, id).text = Some(value.into());
    assert_eq!(snapshot(editor.document()), *expected);
}

fn set_attribute(
    editor: &mut Editor<'_>,
    expected: &mut State,
    id: OwnedNodeId,
    namespace: Option<&str>,
    local: &str,
    value: &str,
) {
    editor
        .set_attribute_value(id, namespace, local, value)
        .unwrap();
    let attribute = expected_node(expected, id)
        .attributes
        .iter_mut()
        .find(|(name, _)| name.2 == local && name.3.as_deref() == namespace)
        .unwrap();
    attribute.1 = value.into();
    assert_eq!(snapshot(editor.document()), *expected);
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
    output_with_limits(document, WriterLimits::default())
}

fn output_with_limits(document: &OwnedDocument, limits: WriterLimits) -> Vec<u8> {
    let mut writer = Writer::with_limits(Vec::new(), limits);
    writer.write_owned_document(document).unwrap();
    writer.finish().unwrap();
    let position = writer.position();
    let bytes = writer.into_inner();
    assert_eq!(position, bytes.len());
    bytes
}

// Call only when Text nodes are nonempty: this compares every logical kind and
// ordered attribute without an exception for normalization or merged CDATA.
fn round_trip(document: &OwnedDocument) {
    let bytes = output(document);
    let reparsed = OwnedDocument::parse_bytes(&bytes).unwrap();
    let mut before = snapshot(document);
    if let Some((_, Some(encoding), _)) = &mut before.declaration {
        *encoding = "UTF-8".into();
    }
    let after = snapshot(&reparsed);
    assert_eq!(after.declaration, before.declaration);
    assert_eq!(
        after.nodes.into_iter().map(|n| n.value).collect::<Vec<_>>(),
        before
            .nodes
            .into_iter()
            .map(|n| n.value)
            .collect::<Vec<_>>()
    );
    assert_eq!(output(&reparsed), bytes);
}

fn error(kind: EditErrorKind, value_offset: Option<usize>) -> EditError {
    EditError {
        kind,
        value_offset,
        location: None,
    }
}

fn owned_limit() -> EditError {
    error(EditErrorKind::LimitExceeded(EditLimit::OwnedBytes), None)
}

fn limits(owned: usize) -> EditLimits {
    {
        let mut cfg = EditLimits::default();
        cfg.max_owned_bytes = owned;
        cfg
    }
}

#[test]
fn every_value_kind_has_an_independent_complete_document_byte_golden() {
    const INPUT: &str = concat!(
        "<?xml version='1.2' encoding='uTf-8' standalone='yes'?>",
        "<!--pre--><?pre x?><r xmlns='u' xmlns:p='v' a='old' p:a='ns'>",
        "old<![CDATA[cd]]><!--co--><?pi data?><c xmlns=''/></r><!--post-->"
    );
    const EXPECTED: &str = concat!(
        "<?xml version=\"1.2\" encoding=\"UTF-8\" standalone=\"yes\"?>",
        "<!--pré--><?pre z?><r xmlns=\"u\" xmlns:p=\"v\" ",
        "a=\"&quot;'&amp;&lt;>&#9;&#10;&#13;é🙂\" p:a=\"namespaced\">",
        "é🙂&lt;&amp;&gt;&#13;\n\t",
        "<![CDATA[中<&\n]]><!--好\n--><?pi x\t中\n?>",
        "<c xmlns=\"\"/></r><!--fin-->"
    );
    for mode in 0..3 {
        let mut document = parse(INPUT, mode);
        let ids: Vec<_> = document.nodes().map(|n| n.id()).collect();
        assert_eq!(ids.len(), 10);
        let mut expected = snapshot(&document);
        let mut editor = document.edit(EditLimits::default()).unwrap();
        for (index, value) in [
            (1, "pré"),
            (2, "z"),
            (4, "é🙂<&>\r\n\t"),
            (5, "中<&\n"),
            (6, "好\n"),
            (7, "x\t中\n"),
            (9, "fin"),
        ] {
            set_leaf(&mut editor, &mut expected, ids[index], value);
        }
        set_attribute(
            &mut editor,
            &mut expected,
            ids[3],
            None,
            "a",
            "\"'&<>\t\n\ré🙂",
        );
        set_attribute(
            &mut editor,
            &mut expected,
            ids[3],
            Some("v"),
            "a",
            "namespaced",
        );
        assert_eq!(output(editor.document()), EXPECTED.as_bytes());
        round_trip(editor.document());
    }
}

#[test]
fn empty_values_preserve_nodes_but_only_empty_text_disappears_on_output() {
    for mode in 0..3 {
        let mut document = parse("<r a='x'>x<![CDATA[x]]><!--x--><?p x?></r>", mode);
        let root = document.root_element().id();
        let ids: Vec<_> = document.root_element().children().map(|n| n.id()).collect();
        let mut expected = snapshot(&document);
        let mut editor = document.edit(EditLimits::default()).unwrap();
        for id in ids {
            set_leaf(&mut editor, &mut expected, id, "");
        }
        set_attribute(&mut editor, &mut expected, root, None, "a", "");
        let bytes = output(editor.document());
        assert_eq!(bytes, b"<r a=\"\"><![CDATA[]]><!----><?p?></r>");
        let reparsed = OwnedDocument::parse_bytes(&bytes).unwrap();
        assert_eq!(
            reparsed
                .root_element()
                .children()
                .map(|n| (n.kind(), n.text()))
                .collect::<Vec<_>>(),
            [
                (OwnedNodeKind::Cdata, Some("")),
                (OwnedNodeKind::Comment, Some("")),
                (OwnedNodeKind::ProcessingInstruction, Some("")),
            ]
        );
        assert_eq!(editor.document().root_element().children().count(), 4);
    }
}

#[test]
fn expanded_attribute_names_preserve_prefixes_order_and_namespace_scopes() {
    const INPUT: &str = concat!(
        "<r xmlns='d' xmlns:p='u' xmlns:q='v' a='0' p:a='1' q:a='2'>",
        "<c xmlns:p='v' p:a='3'/><c p:a='4'/><c xmlns='' a='5'/>",
        "<é:c xmlns:é='u' é:中='6'/></r>"
    );
    for mode in 0..3 {
        let mut document = parse(INPUT, mode);
        let root = document.root_element().id();
        let ids: Vec<_> = document.root_element().children().map(|n| n.id()).collect();
        let mut expected = snapshot(&document);
        let mut editor = document.edit(EditLimits::default()).unwrap();
        for (namespace, value) in [(None, "A"), (Some("u"), "B"), (Some("v"), "C")] {
            set_attribute(&mut editor, &mut expected, root, namespace, "a", value);
        }
        for (id, namespace, local, value) in [
            (ids[0], Some("v"), "a", "D"),
            (ids[1], Some("u"), "a", "E"),
            (ids[2], None, "a", "F"),
            (ids[3], Some("u"), "中", "G"),
        ] {
            set_attribute(&mut editor, &mut expected, id, namespace, local, value);
        }
        for (namespace, local) in [
            (Some(""), "a"),
            (Some("d"), "a"),
            (None, "p:a"),
            (None, ""),
            (None, "a\0"),
            (Some("u\0"), "a"),
        ] {
            assert_eq!(
                editor.set_attribute_value(root, namespace, local, "bad"),
                Err(error(EditErrorKind::MissingAttribute, None))
            );
            assert_eq!(snapshot(editor.document()), expected);
        }
        assert_eq!(
            output(editor.document()),
            concat!(
                "<r xmlns=\"d\" xmlns:p=\"u\" xmlns:q=\"v\" a=\"A\" p:a=\"B\" q:a=\"C\">",
                "<c xmlns:p=\"v\" p:a=\"D\"/><c p:a=\"E\"/><c xmlns=\"\" a=\"F\"/>",
                "<é:c xmlns:é=\"u\" é:中=\"G\"/></r>"
            )
            .as_bytes()
        );
        round_trip(editor.document());
    }
}

#[test]
fn errors_have_exact_utf8_offsets_and_rejected_editors_recover() {
    use EditErrorKind::{InvalidCharacter as Character, InvalidValue as Lexical};
    let cases = [
        (0, "é🙂\u{1}", Character, 6),
        (0, "é\0", Character, 2),
        (0, "\u{fffe}", Character, 0),
        (0, "\u{ffff}", Character, 0),
        (1, "é🙂]]>", Lexical, 6),
        (1, "é🙂\r", Lexical, 6),
        (1, "]]>\u{1}", Character, 3),
        (2, "é🙂--x", Lexical, 6),
        (2, "é🙂-", Lexical, 6),
        (2, "é🙂\r", Lexical, 6),
        (2, "é\r--", Lexical, 2),
        (2, "--é\r", Lexical, 0),
        (2, "--\u{1}", Character, 2),
        (3, "é🙂?>", Lexical, 6),
        (3, "é🙂\r", Lexical, 6),
        (3, " x", Lexical, 0),
        (3, "\tx", Lexical, 0),
        (3, "\nx", Lexical, 0),
        (3, "\rx", Lexical, 0),
        (3, " x?>", Lexical, 0),
        (3, " \u{1}", Character, 1),
    ];
    for mode in 0..3 {
        let mut document = parse("<r a='old'>t<![CDATA[c]]><!--m--><?p d?></r>", mode);
        let root = document.root_element().id();
        let ids: Vec<_> = document.root_element().children().map(|n| n.id()).collect();
        let mut expected = snapshot(&document);
        for (index, value, kind, offset) in cases {
            let mut editor = document.edit(EditLimits::default()).unwrap();
            assert_eq!(
                editor.set_node_value(ids[index], value),
                Err(error(kind, Some(offset)))
            );
            assert_eq!(snapshot(editor.document()), expected, "{value:?}");
            set_leaf(&mut editor, &mut expected, ids[index], "recovered");
        }
        let mut editor = document.edit(EditLimits::default()).unwrap();
        assert_eq!(
            editor.set_attribute_value(root, None, "a", "é🙂\0"),
            Err(error(Character, Some(6)))
        );
        assert_eq!(snapshot(editor.document()), expected);
        set_attribute(&mut editor, &mut expected, root, None, "a", "&amp;");
        set_leaf(&mut editor, &mut expected, ids[0], "&amp;");
        assert!(
            String::from_utf8(output(editor.document()))
                .unwrap()
                .contains("&amp;amp;")
        );
        round_trip(editor.document());
    }
}

#[test]
fn target_resolution_and_value_length_precede_character_and_lexical_checks() {
    for mode in 0..3 {
        let mut document = parse(
            "<r xmlns='u' xmlns:p='v' a='x'>t<![CDATA[c]]><!--m--><?p d?></r>",
            mode,
        );
        let root = document.root_element().id();
        let document_id = document.root().id();
        let ids: Vec<_> = document.root_element().children().map(|n| n.id()).collect();
        let foreign = parse("<r a='x'>t</r>", mode);
        let expected = snapshot(&document);
        let mut editor = document
            .edit({
                let mut cfg = EditLimits::default();
                cfg.max_value_bytes = 0;
                cfg.max_owned_bytes = 0;
                cfg
            })
            .unwrap();
        for id in [
            foreign.root().id(),
            foreign.root_element().id(),
            foreign.root_element().first_child().unwrap().id(),
        ] {
            assert_eq!(
                editor.set_node_value(id, "\0"),
                Err(error(EditErrorKind::InvalidHandle, None))
            );
            assert_eq!(
                editor.set_attribute_value(id, None, "a", "\0"),
                Err(error(EditErrorKind::InvalidHandle, None))
            );
        }
        for id in [root, document_id] {
            assert_eq!(
                editor.set_node_value(id, "\0"),
                Err(error(EditErrorKind::WrongNodeKind, None))
            );
        }
        for id in std::iter::once(document_id).chain(ids.iter().copied()) {
            assert_eq!(
                editor.set_attribute_value(id, None, "a", "\0"),
                Err(error(EditErrorKind::WrongNodeKind, None))
            );
        }
        assert_eq!(
            editor.set_attribute_value(root, None, "missing", "\0"),
            Err(error(EditErrorKind::MissingAttribute, None))
        );
        assert_eq!(
            editor.set_attribute_value(root, Some(XMLNS), "missing", "\0"),
            Err(error(EditErrorKind::MissingAttribute, None))
        );
        for local in ["xmlns", "p"] {
            assert_eq!(
                editor.set_attribute_value(root, Some(XMLNS), local, "\0"),
                Err(error(EditErrorKind::NamespaceDeclaration, None))
            );
        }
        for (id, value) in ids.into_iter().zip(["\0", "]]>\0", "--\0", " \0"]) {
            assert_eq!(
                editor.set_node_value(id, value),
                Err(error(
                    EditErrorKind::LimitExceeded(EditLimit::ValueBytes),
                    None
                ))
            );
        }
        assert_eq!(
            editor.set_attribute_value(root, None, "a", "\0"),
            Err(error(
                EditErrorKind::LimitExceeded(EditLimit::ValueBytes),
                None
            ))
        );
        assert_eq!(snapshot(editor.document()), expected);
    }
}

#[test]
fn value_byte_ceilings_are_inclusive_for_both_setters_and_all_leaf_kinds() {
    for mode in 0..3 {
        for ceiling in [5, 6, 7] {
            let mut document = parse("<r a='x'>t<![CDATA[c]]><!--m--><?p d?></r>", mode);
            let root = document.root_element().id();
            let ids: Vec<_> = document.root_element().children().map(|n| n.id()).collect();
            let mut expected = snapshot(&document);
            let mut editor = document
                .edit({
                    let mut cfg = EditLimits::default();
                    cfg.max_value_bytes = ceiling;
                    cfg
                })
                .unwrap();
            for id in ids {
                let result = editor.set_node_value(id, "é🙂");
                if ceiling == 5 {
                    assert_eq!(
                        result,
                        Err(error(
                            EditErrorKind::LimitExceeded(EditLimit::ValueBytes),
                            None
                        ))
                    );
                } else {
                    result.unwrap();
                    expected_node(&mut expected, id).text = Some("é🙂".into());
                }
                assert_eq!(snapshot(editor.document()), expected);
            }
            let result = editor.set_attribute_value(root, None, "a", "é🙂");
            if ceiling == 5 {
                assert_eq!(
                    result,
                    Err(error(
                        EditErrorKind::LimitExceeded(EditLimit::ValueBytes),
                        None
                    ))
                );
            } else {
                result.unwrap();
                expected_node(&mut expected, root).attributes[0].1 = "é🙂".into();
            }
            assert_eq!(snapshot(editor.document()), expected);
            round_trip(editor.document());
        }
    }
}

#[test]
fn exact_full_capacity_and_persistent_reopen_quotas_match_literal_goldens() {
    for mode in 0..3 {
        for ceiling in [2, 3, 4] {
            let mut document = parse("<r>a</r>", mode);
            let id = document.root_element().first_child().unwrap().id();
            let mut expected = snapshot(&document);
            let expected_range = if mode == 0 { 3..4 } else { 8..10 };
            assert_eq!(document.node(id).unwrap().range(), Some(expected_range));
            let mut editor = document.edit(limits(ceiling)).unwrap();
            let result = editor.set_node_value(id, "b");
            if ceiling == 2 {
                assert_eq!(result, Err(owned_limit()));
                assert_eq!(snapshot(editor.document()), expected);
                set_leaf(&mut editor, &mut expected, id, "");
            } else {
                result.unwrap();
                expected_node(&mut expected, id).text = Some("b".into());
                assert_eq!(snapshot(editor.document()), expected);
            }
        }
        for ceiling in [3, 4, 6, 7, 8] {
            let mut document = parse("<r>a</r>", mode);
            let id = document.root_element().first_child().unwrap().id();
            document
                .edit(limits(3))
                .unwrap()
                .set_node_value(id, "a")
                .unwrap();
            let expected = snapshot(&document);
            assert_eq!(document.edit(limits(2)).err().unwrap(), owned_limit());
            let mut editor = document.edit(limits(ceiling)).unwrap();
            let result = editor.set_node_value(id, "a"); // Equal values still append.
            if ceiling < 7 {
                assert_eq!(result, Err(owned_limit()));
            } else {
                result.unwrap();
            }
            assert_eq!(snapshot(editor.document()), expected);
        }
        // Failed validation cannot spend the exact three-byte growth allowance.
        let mut document = parse("<r>a</r>", mode);
        let id = document.root_element().first_child().unwrap().id();
        let mut expected = snapshot(&document);
        let mut editor = document
            .edit({
                let mut cfg = EditLimits::default();
                cfg.max_value_bytes = 1;
                cfg.max_owned_bytes = 3;
                cfg
            })
            .unwrap();
        assert_eq!(
            editor.set_node_value(id, "\0"),
            Err(error(EditErrorKind::InvalidCharacter, Some(0)))
        );
        assert_eq!(snapshot(editor.document()), expected);
        set_leaf(&mut editor, &mut expected, id, "b");
    }
}

#[test]
fn parsed_spare_capacity_and_attribute_growth_use_the_same_edit_budget() {
    for mode in 0..3 {
        for ceiling in [4, 5, 6] {
            let mut document = parse("<r a='x'/>", mode);
            let root = document.root_element().id();
            let mut expected = snapshot(&document);
            {
                let mut editor = document.edit(limits(0)).unwrap();
                set_attribute(&mut editor, &mut expected, root, None, "a", "y");
                assert_eq!(
                    editor.set_attribute_value(root, None, "a", "y"),
                    Err(owned_limit())
                );
                assert_eq!(snapshot(editor.document()), expected);
            }
            let mut editor = document.edit(limits(ceiling)).unwrap();
            let result = editor.set_attribute_value(root, None, "a", "z");
            if ceiling == 4 {
                assert_eq!(result, Err(owned_limit()));
                assert_eq!(snapshot(editor.document()), expected);
                set_attribute(&mut editor, &mut expected, root, None, "a", "");
            } else {
                result.unwrap();
                expected_node(&mut expected, root).attributes[0].1 = "z".into();
                assert_eq!(snapshot(editor.document()), expected);
                assert_eq!(output(editor.document()), b"<r a=\"z\"/>");
            }
        }
    }
}

#[test]
fn node_and_attribute_setters_share_one_cumulative_growth_counter() {
    // Parsed strings r/a/x/a exactly fill capacity4. A one-byte edit needs
    // full capacity5; the next needs full capacity6, for cumulative work11.
    for mode in 0..3 {
        for attribute_first in [false, true] {
            for ceiling in [10, 11, 12] {
                let mut document = parse("<r a='x'>a</r>", mode);
                let root = document.root_element().id();
                let text = document.root_element().first_child().unwrap().id();
                let mut expected = snapshot(&document);
                {
                    let mut editor = document.edit(limits(5)).unwrap();
                    if attribute_first {
                        set_attribute(&mut editor, &mut expected, root, None, "a", "y");
                    } else {
                        set_leaf(&mut editor, &mut expected, text, "b");
                    }
                }
                let mut editor = document.edit(limits(ceiling)).unwrap();
                let result = if attribute_first {
                    editor.set_node_value(text, "b")
                } else {
                    editor.set_attribute_value(root, None, "a", "y")
                };
                if ceiling == 10 {
                    assert_eq!(result, Err(owned_limit()));
                } else {
                    result.unwrap();
                    if attribute_first {
                        expected_node(&mut expected, text).text = Some("b".into());
                    } else {
                        expected_node(&mut expected, root).attributes[0].1 = "y".into();
                    }
                }
                assert_eq!(snapshot(editor.document()), expected);
                round_trip(editor.document());
            }
        }
    }
}

#[test]
fn source_overwrite_and_document_moves_preserve_checked_identity_and_provenance() {
    for mode in 0..3 {
        let mut source = encoded("<r a='x'>a</r>", mode);
        let document = OwnedDocument::parse_bytes(&source).unwrap();
        let root = document.root_element().id();
        let text = document.root_element().first_child().unwrap().id();
        let mut expected = snapshot(&document);
        let foreign = parse("<r a='x'>a</r>", mode);
        let foreign_root = foreign.root_element().id();
        let foreign_text = foreign.root_element().first_child().unwrap().id();
        assert_ne!(root, foreign_root);
        assert_ne!(text, foreign_text);
        source.fill(0);
        drop(source);
        let mut documents = vec![document];
        let document = documents.pop().unwrap();
        let mut parked = Some(document);
        let mut document = parked.take().unwrap();
        assert!(parked.is_none());
        assert_eq!(snapshot(&document), expected);
        assert!(document.node(foreign_root).is_none());
        assert!(document.node(foreign_text).is_none());
        let mut editor = document.edit(EditLimits::default()).unwrap();
        assert_eq!(
            editor.set_node_value(foreign_text, "x"),
            Err(error(EditErrorKind::InvalidHandle, None))
        );
        assert_eq!(
            editor.set_attribute_value(foreign_root, None, "a", "x"),
            Err(error(EditErrorKind::InvalidHandle, None))
        );
        assert_eq!(snapshot(editor.document()), expected);
        set_leaf(&mut editor, &mut expected, text, "é🙂");
        set_attribute(&mut editor, &mut expected, root, None, "a", "中");
        assert_eq!(output(editor.document()), "<r a=\"中\">é🙂</r>".as_bytes());
        round_trip(editor.document());
        assert_eq!(foreign.node(foreign_text).unwrap().text(), Some("a"));
    }
}

#[test]
fn generated_long_short_replacements_preserve_the_strict_round_trip_domain() {
    let long = "é中🙂".repeat(128);
    let texts = ["é🙂", "<&>", "\r\n\t", "&amp;", long.as_str(), "x"];
    let cdata = ["中", "]]", "]>", "<&\n\t", long.as_str(), "x"];
    let comments = ["好", "-x", "x-y", "<&\n\t", long.as_str(), "x"];
    let pi = ["🙂", "?x", "x>", "x\n\t", long.as_str(), "\u{a0}x"];
    for mode in 0..3 {
        let mut document = parse("<r a='x'>t<![CDATA[c]]><!--m--><?p d?></r>", mode);
        let root = document.root_element().id();
        let ids: Vec<_> = document.root_element().children().map(|n| n.id()).collect();
        let mut expected = snapshot(&document);
        let mut editor = document.edit(EditLimits::default()).unwrap();
        for index in 0..24 {
            for (id, values) in ids.iter().copied().zip([texts, cdata, comments, pi]) {
                set_leaf(&mut editor, &mut expected, id, values[index % values.len()]);
            }
            set_attribute(
                &mut editor,
                &mut expected,
                root,
                None,
                "a",
                texts[(index + 2) % texts.len()],
            );
            round_trip(editor.document());
        }
    }
}

#[test]
fn deep_and_wide_edits_serialize_and_drop_without_recursive_traversal() {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 8192;
            let input = format!("{}old{}", "<n>".repeat(DEPTH), "</n>".repeat(DEPTH));
            let expected = format!("{}é🙂{}", "<n>".repeat(DEPTH), "</n>".repeat(DEPTH));
            let mut document = OwnedDocument::with_limits(input.as_bytes(), {
                let mut cfg = Limits::default();
                cfg.max_depth = DEPTH;
                cfg
            })
            .unwrap();
            let id = document.nodes().last().unwrap().id();
            let root = document.root_element().id();
            let range = document.node(id).unwrap().range();
            document
                .edit(EditLimits::default())
                .unwrap()
                .set_node_value(id, "é🙂")
                .unwrap();
            assert_eq!(document.node(id).unwrap().range(), range);
            assert_eq!(document.root_element().id(), root);
            assert_eq!(document.nodes().count(), DEPTH + 2);
            assert_eq!(
                output_with_limits(&document, {
                    let mut cfg = WriterLimits::default();
                    cfg.max_depth = DEPTH;
                    cfg
                }),
                expected.as_bytes()
            );
        })
        .unwrap()
        .join()
        .unwrap();

    let mut document =
        OwnedDocument::parse(&format!("<r>{}</r>", "<c a='x'>old</c>".repeat(10_000))).unwrap();
    let children: Vec<_> = document.root_element().children().map(|n| n.id()).collect();
    let mut expected = snapshot(&document);
    let mut editor = document.edit(EditLimits::default()).unwrap();
    for index in [0, 4_999, 9_999] {
        let child = children[index];
        let text = editor
            .document()
            .node(child)
            .unwrap()
            .first_child()
            .unwrap()
            .id();
        set_attribute(&mut editor, &mut expected, child, None, "a", "中");
        set_leaf(&mut editor, &mut expected, text, "é🙂");
    }
    let mut bytes = String::from("<r>");
    for index in 0..10_000 {
        bytes.push_str(if [0, 4_999, 9_999].contains(&index) {
            "<c a=\"中\">é🙂</c>"
        } else {
            "<c a=\"x\">old</c>"
        });
    }
    bytes.push_str("</r>");
    assert_eq!(output(editor.document()), bytes.as_bytes());
    round_trip(editor.document());
}
