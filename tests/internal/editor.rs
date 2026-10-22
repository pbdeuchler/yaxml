use super::*;

fn error(kind: EditErrorKind, offset: Option<usize>) -> EditError {
    EditError::new(kind, offset)
}

fn limits(owned: usize) -> EditLimits {
    EditLimits {
        max_owned_bytes: owned,
        ..EditLimits::default()
    }
}

#[test]
fn lexical_validation_uses_utf8_indices_and_documented_precedence() {
    for (kind, value, expected) in [
        (
            ValueKind::Text,
            "é\0",
            error(EditErrorKind::InvalidCharacter, Some(2)),
        ),
        (
            ValueKind::Cdata,
            "🙂]]>",
            error(EditErrorKind::InvalidValue, Some(4)),
        ),
        (
            ValueKind::Comment,
            "é-",
            error(EditErrorKind::InvalidValue, Some(2)),
        ),
        (
            ValueKind::Comment,
            "🙂--x\r",
            error(EditErrorKind::InvalidValue, Some(4)),
        ),
        (
            ValueKind::Comment,
            "é\r--",
            error(EditErrorKind::InvalidValue, Some(2)),
        ),
        (
            ValueKind::Pi,
            "é?>",
            error(EditErrorKind::InvalidValue, Some(2)),
        ),
        (
            ValueKind::Pi,
            "\tx?>",
            error(EditErrorKind::InvalidValue, Some(0)),
        ),
        (
            ValueKind::Cdata,
            "]]>\0",
            error(EditErrorKind::InvalidCharacter, Some(3)),
        ),
        (
            ValueKind::Pi,
            " \u{ffff}",
            error(EditErrorKind::InvalidCharacter, Some(1)),
        ),
    ] {
        assert_eq!(
            validate_value(value, kind, value.len()).unwrap_err(),
            expected
        );
        assert_eq!(
            validate_value(value, kind, value.len() - 1).unwrap_err(),
            error(EditErrorKind::LimitExceeded(EditLimit::ValueBytes), None)
        );
    }
    for kind in [
        ValueKind::Text,
        ValueKind::Cdata,
        ValueKind::Comment,
        ValueKind::Pi,
    ] {
        for value in ["", "é🙂\n\t", "valid"] {
            validate_value(value, kind, value.len()).unwrap();
        }
    }
    validate_value("\r]]>--?>", ValueKind::Text, 20).unwrap();
    assert_eq!(
        validate_value("a\r?>", ValueKind::Pi, 10)
            .unwrap_err()
            .value_offset,
        Some(1)
    );
    assert_eq!(
        validate_value("a\r]]>", ValueKind::Cdata, 10)
            .unwrap_err()
            .value_offset,
        Some(1)
    );
}

#[test]
fn full_capacity_golden_is_inclusive_at_two_three_and_four() {
    for ceiling in [2, 3, 4] {
        let mut document = OwnedDocument::parse("<r>a</r>").unwrap();
        assert_eq!(
            (
                document.pool.0.len(),
                document.pool.0.capacity(),
                document.edit_owned_bytes
            ),
            (2, 2, 0)
        );
        let id = document.root_element().first_child().unwrap().id();
        let range = document.node(id).unwrap().range();
        let mut editor = document.edit(limits(ceiling)).unwrap();
        let result = editor.set_node_value(id, "a");
        assert_eq!(result.is_ok(), ceiling >= 3);
        assert_eq!(editor.document().node(id).unwrap().text(), Some("a"));
        assert_eq!(editor.document().node(id).unwrap().range(), range);
        if ceiling == 2 {
            assert_eq!(
                result.unwrap_err(),
                error(EditErrorKind::LimitExceeded(EditLimit::OwnedBytes), None)
            );
            assert_eq!(editor.document().edit_owned_bytes, 0);
            editor.set_node_value(id, "").unwrap();
            assert_eq!(editor.document().node(id).unwrap().text(), Some(""));
        } else {
            assert_eq!(editor.document().edit_owned_bytes, ceiling);
            assert_eq!(editor.document().pool.0.capacity(), ceiling);
        }
    }
}

#[test]
fn reopened_editors_keep_full_capacity_golden_at_six_seven_and_eight() {
    for ceiling in [6, 7, 8] {
        let mut document = OwnedDocument::parse("<r>a</r>").unwrap();
        let id = document.root_element().first_child().unwrap().id();
        document
            .edit(limits(3))
            .unwrap()
            .set_node_value(id, "a")
            .unwrap();
        assert_eq!(
            (
                document.pool.0.len(),
                document.pool.0.capacity(),
                document.edit_owned_bytes
            ),
            (3, 3, 3)
        );
        assert!(matches!(
            document.edit(limits(2)),
            Err(EditError {
                kind: EditErrorKind::LimitExceeded(EditLimit::OwnedBytes),
                value_offset: None,
                ..
            })
        ));
        let mut editor = document.edit(limits(ceiling)).unwrap();
        let result = editor.set_node_value(id, "b");
        assert_eq!(result.is_ok(), ceiling >= 7);
        if ceiling == 6 {
            assert_eq!(editor.document().node(id).unwrap().text(), Some("a"));
            assert_eq!(editor.document().edit_owned_bytes, 3);
            editor.set_node_value(id, "").unwrap();
        } else {
            assert_eq!(editor.document().node(id).unwrap().text(), Some("b"));
            assert_eq!(editor.document().edit_owned_bytes, ceiling);
        }
    }
}

#[test]
fn attribute_and_node_updates_publish_only_the_selected_value() {
    let mut document = OwnedDocument::parse(
        "<r xmlns='u' xmlns:p='p' a='v' p:a='w'>text<![CDATA[c]]><!--m--><?pi d?></r>",
    )
    .unwrap();
    let ids: Vec<_> = document
        .root_element()
        .children()
        .map(|node| node.id())
        .collect();
    let provenance: Vec<_> = document
        .nodes()
        .map(|node| (node.id(), node.range()))
        .collect();
    let root = document.root_element().id();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    for (id, value) in ids.iter().copied().zip(["\r&<>", "", "comment", "data"]) {
        editor.set_node_value(id, value).unwrap();
        assert_eq!(editor.document().node(id).unwrap().text(), Some(value));
    }
    editor
        .set_attribute_value(root, None, "a", "\r\n\t&")
        .unwrap();
    editor
        .set_attribute_value(root, Some("p"), "a", "é🙂")
        .unwrap();
    let values: Vec<_> = editor
        .document()
        .root_element()
        .attributes()
        .map(|attribute| (attribute.name.qualified().to_owned(), attribute.value))
        .collect();
    assert_eq!(
        values,
        [
            ("xmlns".to_owned(), Cow::Borrowed("u")),
            ("xmlns:p".to_owned(), Cow::Borrowed("p")),
            ("a".to_owned(), Cow::Borrowed("\r\n\t&")),
            ("p:a".to_owned(), Cow::Borrowed("é🙂"))
        ]
    );
    assert_eq!(
        editor
            .document()
            .node(ids[3])
            .unwrap()
            .processing_instruction_target(),
        Some("pi")
    );
    assert_eq!(
        editor
            .document()
            .nodes()
            .map(|node| (node.id(), node.range()))
            .collect::<Vec<_>>(),
        provenance
    );
    let mut writer = crate::Writer::new(Vec::new());
    writer.write_owned_document(editor.document()).unwrap();
    assert_eq!(
        String::from_utf8(writer.into_inner()).unwrap(),
        "<r xmlns=\"u\" xmlns:p=\"p\" a=\"&#13;&#10;&#9;&amp;\" p:a=\"é🙂\">&#13;&amp;&lt;&gt;<![CDATA[]]><!--comment--><?pi data?></r>"
    );
}

#[test]
fn handle_kind_lookup_and_namespace_fail_before_value_validation() {
    let mut document = OwnedDocument::parse("<r xmlns='u' a='v'>text</r>").unwrap();
    let foreign = OwnedDocument::parse("<r/>").unwrap().root().id();
    let invalid = OwnedNodeId {
        document: document.identity,
        slot: usize::MAX,
    };
    let root = document.root_element().id();
    let text = document.root_element().first_child().unwrap().id();
    let document_id = document.root().id();
    let before = document.pool.0.len();
    let mut editor = document
        .edit(EditLimits {
            max_value_bytes: 0,
            max_owned_bytes: 0,
            ..EditLimits::default()
        })
        .unwrap();
    for id in [foreign, invalid] {
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
    assert_eq!(
        editor.set_attribute_value(text, None, "a", "\0"),
        Err(error(EditErrorKind::WrongNodeKind, None))
    );
    for (namespace, local) in [
        (None, "missing"),
        (None, "bad name"),
        (Some(""), "a"),
        (Some("u"), "a"),
        (Some(XMLNS), "missing"),
    ] {
        assert_eq!(
            editor.set_attribute_value(root, namespace, local, "\0"),
            Err(error(EditErrorKind::MissingAttribute, None))
        );
    }
    assert_eq!(
        editor.set_attribute_value(root, Some(XMLNS), "xmlns", "\0"),
        Err(error(EditErrorKind::NamespaceDeclaration, None))
    );
    assert_eq!(
        editor.set_attribute_value(root, None, "a", "\0"),
        Err(error(
            EditErrorKind::LimitExceeded(EditLimit::ValueBytes),
            None
        ))
    );
    assert_eq!(
        editor.set_node_value(text, "\0"),
        Err(error(
            EditErrorKind::LimitExceeded(EditLimit::ValueBytes),
            None
        ))
    );
    assert_eq!(editor.document().pool.0.len(), before);
    assert_eq!(editor.document().edit_owned_bytes, 0);
    editor.set_attribute_value(root, None, "a", "").unwrap();
    editor.set_node_value(text, "").unwrap();
}

#[test]
fn invalid_payloads_leave_storage_and_values_unchanged_and_edits_recover() {
    let mut document = OwnedDocument::parse("<r>text<![CDATA[c]]><!--m--><?pi d?></r>").unwrap();
    let ids: Vec<_> = document
        .root_element()
        .children()
        .map(|node| node.id())
        .collect();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    for (id, invalid, kind) in [
        (ids[0], "é\0", EditErrorKind::InvalidCharacter),
        (ids[1], "é]]>", EditErrorKind::InvalidValue),
        (ids[2], "é--", EditErrorKind::InvalidValue),
        (ids[3], "é?>", EditErrorKind::InvalidValue),
    ] {
        let length = editor.document().pool.0.len();
        let spent = editor.document().edit_owned_bytes;
        let old = editor
            .document()
            .node(id)
            .unwrap()
            .text()
            .unwrap()
            .to_owned();
        let provenance = editor.document().node(id).unwrap().range();
        assert_eq!(
            editor.set_node_value(id, invalid),
            Err(error(kind, Some(2)))
        );
        assert_eq!(
            editor.document().node(id).unwrap().text(),
            Some(old.as_str())
        );
        assert_eq!(editor.document().node(id).unwrap().range(), provenance);
        assert_eq!(
            (
                editor.document().pool.0.len(),
                editor.document().edit_owned_bytes
            ),
            (length, spent)
        );
        editor.set_node_value(id, "valid").unwrap();
        assert_eq!(editor.document().node(id).unwrap().text(), Some("valid"));
    }
}

#[test]
fn edit_errors_display_optional_value_indices_and_defaults_are_finite() {
    assert_eq!(
        EditLimits::default(),
        EditLimits {
            max_value_bytes: 1_048_576,
            max_owned_bytes: 67_108_864,
            max_created_nodes: 1_000_000,
            max_name_bytes: 1_048_576,
            max_supplied_bytes: 8_388_608,
            max_attributes: 1_024,
            max_depth: 256,
            max_created_attributes: 1_000_000,
            max_namespace_work: 67_108_864,
        }
    );
    let indexed = error(EditErrorKind::InvalidCharacter, Some(2));
    assert_eq!(
        indexed.to_string(),
        "XML edit error InvalidCharacter at value byte 2"
    );
    let plain = error(EditErrorKind::Allocation, None);
    assert_eq!(plain.to_string(), "XML edit error Allocation");
    let located = EditError::located(
        EditErrorKind::Namespace,
        EditLocation::AttributeItem {
            index: 2,
            component: None,
            byte_offset: None,
        },
    );
    assert_eq!(
        located.to_string(),
        "XML edit error Namespace at AttributeItem { index: 2, component: None, byte_offset: None }"
    );
    assert!(std::error::Error::source(&indexed).is_none());
    let mut pool = Pool("r".to_owned());
    let mut spent = 2;
    assert_eq!(
        pool.store_edit("x", &mut spent, 1),
        Err(error(
            EditErrorKind::LimitExceeded(EditLimit::OwnedBytes),
            None
        ))
    );
    assert_eq!(pool.0, "r");
    assert_eq!(spent, 2);
}
