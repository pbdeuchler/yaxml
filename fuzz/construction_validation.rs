//! Bounded lexical and namespace-validation oracle for construction.
//!
//! Each testcase selects one literal invalid specification and its nearby valid
//! control. Expected errors come from the public construction contract rather
//! than from the implementation's validator or prefix planner.

use super::*;

const FAMILY_COUNT: u8 = 12;
const FAMILY_VARIANTS: [u8; FAMILY_COUNT as usize] = [6, 2, 3, 2, 2, 3, 2, 2, 2, 6, 7, 5];

struct LiteralSpec {
    name: NameSpec<'static>,
    items: Vec<ElementItem<'static>>,
}

impl LiteralSpec {
    fn borrowed(&self) -> ElementSpec<'_> {
        ElementSpec {
            name: self.name,
            items: &self.items,
        }
    }
}

struct ValidationCase {
    invalid: LiteralSpec,
    valid: LiteralSpec,
    expected: EditError,
}

fn plain(local: &'static str) -> NameSpec<'static> {
    spec_name(local, None, exact(None))
}

fn element<const N: usize>(
    name: NameSpec<'static>,
    items: [ElementItem<'static>; N],
) -> LiteralSpec {
    LiteralSpec {
        name,
        items: Vec::from(items),
    }
}

fn namespace_location(
    index: usize,
    component: Option<EditComponent>,
    byte_offset: Option<usize>,
) -> EditLocation {
    EditLocation::NamespaceItem {
        index,
        component,
        byte_offset,
    }
}

fn lexical_element(
    invalid: NameSpec<'static>,
    valid: NameSpec<'static>,
    kind: EditErrorKind,
    component: EditComponent,
    byte_offset: usize,
) -> ValidationCase {
    ValidationCase {
        invalid: element(invalid, []),
        valid: element(valid, []),
        expected: located(kind, element_location(component, Some(byte_offset))),
    }
}

fn lexical_namespace_item(
    invalid: NamespaceSpec<'static>,
    valid: NamespaceSpec<'static>,
    kind: EditErrorKind,
    component: EditComponent,
    byte_offset: usize,
) -> ValidationCase {
    ValidationCase {
        invalid: element(plain("root"), [ElementItem::Namespace(invalid)]),
        valid: element(plain("root"), [ElementItem::Namespace(valid)]),
        expected: located(
            kind,
            namespace_location(0, Some(component), Some(byte_offset)),
        ),
    }
}

fn lexical_attribute(
    invalid: AttributeSpec<'static>,
    valid: AttributeSpec<'static>,
    kind: EditErrorKind,
    component: EditComponent,
    byte_offset: usize,
) -> ValidationCase {
    ValidationCase {
        invalid: element(plain("root"), [ElementItem::Attribute(invalid)]),
        valid: element(plain("root"), [ElementItem::Attribute(valid)]),
        expected: located(
            kind,
            attribute_location(0, Some(component), Some(byte_offset)),
        ),
    }
}

fn semantic_element(
    invalid: NameSpec<'static>,
    valid: NameSpec<'static>,
    component: EditComponent,
) -> ValidationCase {
    ValidationCase {
        invalid: element(invalid, []),
        valid: element(valid, []),
        expected: located(EditErrorKind::Namespace, element_location(component, None)),
    }
}

fn semantic_namespace_item<const N: usize, const M: usize>(
    invalid: [ElementItem<'static>; N],
    valid: [ElementItem<'static>; M],
    kind: EditErrorKind,
    index: usize,
) -> ValidationCase {
    ValidationCase {
        invalid: element(plain("root"), invalid),
        valid: element(plain("root"), valid),
        expected: located(kind, namespace_location(index, None, None)),
    }
}

fn semantic_attribute(
    invalid: AttributeSpec<'static>,
    valid: [ElementItem<'static>; 1],
) -> ValidationCase {
    ValidationCase {
        invalid: element(plain("root"), [ElementItem::Attribute(invalid)]),
        valid: element(plain("root"), valid),
        expected: located(EditErrorKind::Namespace, attribute_location(0, None, None)),
    }
}

fn validation_case(family: u8, subtype: u8) -> ValidationCase {
    let family = family % FAMILY_COUNT;
    let subtype = subtype % FAMILY_VARIANTS[usize::from(family)];
    match (family, subtype) {
        (0, 0) => lexical_element(
            plain("\0root"),
            plain("root"),
            EditErrorKind::InvalidCharacter,
            EditComponent::LocalName,
            0,
        ),
        (0, 1) => lexical_element(
            plain("é\0root"),
            plain("éroot"),
            EditErrorKind::InvalidCharacter,
            EditComponent::LocalName,
            2,
        ),
        (0, 2) => lexical_element(
            plain("1root"),
            plain("_1root"),
            EditErrorKind::InvalidName,
            EditComponent::LocalName,
            0,
        ),
        (0, 3) => lexical_element(
            plain("é:root"),
            plain("é-root"),
            EditErrorKind::InvalidName,
            EditComponent::LocalName,
            2,
        ),
        (0, 4) => lexical_element(
            plain("\u{301}root"),
            plain("r\u{301}oot"),
            EditErrorKind::InvalidName,
            EditComponent::LocalName,
            0,
        ),
        (0, 5) => lexical_element(
            plain("é\u{f0000}"),
            plain("é🙂"),
            EditErrorKind::InvalidName,
            EditComponent::LocalName,
            2,
        ),
        (1, 0) => lexical_element(
            spec_name("root", Some("u\u{1}"), exact(None)),
            spec_name("root", Some("urn:ok"), exact(None)),
            EditErrorKind::InvalidCharacter,
            EditComponent::NamespaceUri,
            1,
        ),
        (1, 1) => lexical_element(
            spec_name("root", Some("é\u{ffff}"), exact(None)),
            spec_name("root", Some("é🙂"), exact(None)),
            EditErrorKind::InvalidCharacter,
            EditComponent::NamespaceUri,
            2,
        ),
        (2, 0) => lexical_element(
            spec_name("root", Some("urn:p"), exact(Some("\0p"))),
            spec_name("root", Some("urn:p"), exact(Some("p"))),
            EditErrorKind::InvalidCharacter,
            EditComponent::Prefix,
            0,
        ),
        (2, 1) => lexical_element(
            spec_name("root", Some("urn:p"), exact(Some("é:p"))),
            spec_name("root", Some("urn:p"), exact(Some("é-p"))),
            EditErrorKind::InvalidName,
            EditComponent::Prefix,
            2,
        ),
        (2, 2) => lexical_element(
            spec_name("root", Some("urn:p"), exact(Some("1p"))),
            spec_name("root", Some("urn:p"), exact(Some("p1"))),
            EditErrorKind::InvalidName,
            EditComponent::Prefix,
            0,
        ),
        (3, 0) => lexical_namespace_item(
            NamespaceSpec {
                prefix: Some("é:p"),
                uri: "urn:p",
            },
            NamespaceSpec {
                prefix: Some("é-p"),
                uri: "urn:p",
            },
            EditErrorKind::InvalidName,
            EditComponent::Prefix,
            2,
        ),
        (3, 1) => lexical_namespace_item(
            NamespaceSpec {
                prefix: Some("p\0"),
                uri: "urn:p",
            },
            NamespaceSpec {
                prefix: Some("p1"),
                uri: "urn:p",
            },
            EditErrorKind::InvalidCharacter,
            EditComponent::Prefix,
            1,
        ),
        (4, 0) => lexical_namespace_item(
            NamespaceSpec {
                prefix: Some("p"),
                uri: "urn:\0p",
            },
            NamespaceSpec {
                prefix: Some("p"),
                uri: "urn:p",
            },
            EditErrorKind::InvalidCharacter,
            EditComponent::NamespaceUri,
            4,
        ),
        (4, 1) => lexical_namespace_item(
            NamespaceSpec {
                prefix: Some("p"),
                uri: "é\u{ffff}",
            },
            NamespaceSpec {
                prefix: Some("p"),
                uri: "é🙂",
            },
            EditErrorKind::InvalidCharacter,
            EditComponent::NamespaceUri,
            2,
        ),
        (5, 0) => lexical_attribute(
            AttributeSpec {
                name: plain(":a"),
                value: "v",
            },
            AttributeSpec {
                name: plain("a"),
                value: "v",
            },
            EditErrorKind::InvalidName,
            EditComponent::LocalName,
            0,
        ),
        (5, 1) => lexical_attribute(
            AttributeSpec {
                name: plain("é a"),
                value: "v",
            },
            AttributeSpec {
                name: plain("é-a"),
                value: "v",
            },
            EditErrorKind::InvalidName,
            EditComponent::LocalName,
            2,
        ),
        (5, 2) => lexical_attribute(
            AttributeSpec {
                name: plain("a\u{ffff}"),
                value: "v",
            },
            AttributeSpec {
                name: plain("a🙂"),
                value: "v",
            },
            EditErrorKind::InvalidCharacter,
            EditComponent::LocalName,
            1,
        ),
        (6, 0) => lexical_attribute(
            AttributeSpec {
                name: spec_name("a", Some("u\0"), exact(Some("p"))),
                value: "v",
            },
            AttributeSpec {
                name: spec_name("a", Some("urn:p"), exact(Some("p"))),
                value: "v",
            },
            EditErrorKind::InvalidCharacter,
            EditComponent::NamespaceUri,
            1,
        ),
        (6, 1) => lexical_attribute(
            AttributeSpec {
                name: spec_name("a", Some("é\u{ffff}"), exact(Some("p"))),
                value: "v",
            },
            AttributeSpec {
                name: spec_name("a", Some("é🙂"), exact(Some("p"))),
                value: "v",
            },
            EditErrorKind::InvalidCharacter,
            EditComponent::NamespaceUri,
            2,
        ),
        (7, 0) => lexical_attribute(
            AttributeSpec {
                name: spec_name("a", Some("urn:p"), exact(Some("é:p"))),
                value: "v",
            },
            AttributeSpec {
                name: spec_name("a", Some("urn:p"), exact(Some("é-p"))),
                value: "v",
            },
            EditErrorKind::InvalidName,
            EditComponent::Prefix,
            2,
        ),
        (7, 1) => lexical_attribute(
            AttributeSpec {
                name: spec_name("a", Some("urn:p"), exact(Some("p\u{ffff}"))),
                value: "v",
            },
            AttributeSpec {
                name: spec_name("a", Some("urn:p"), exact(Some("p1"))),
                value: "v",
            },
            EditErrorKind::InvalidCharacter,
            EditComponent::Prefix,
            1,
        ),
        (8, 0) => lexical_attribute(
            AttributeSpec {
                name: plain("a"),
                value: "é\0",
            },
            AttributeSpec {
                name: plain("a"),
                value: "é<&\"",
            },
            EditErrorKind::InvalidCharacter,
            EditComponent::Value,
            2,
        ),
        (8, 1) => lexical_attribute(
            AttributeSpec {
                name: plain("a"),
                value: "é\u{ffff}",
            },
            AttributeSpec {
                name: plain("a"),
                value: "é🙂\t\n\r",
            },
            EditErrorKind::InvalidCharacter,
            EditComponent::Value,
            2,
        ),
        (9, 0) => semantic_element(
            spec_name("root", Some(""), exact(None)),
            plain("root"),
            EditComponent::NamespaceUri,
        ),
        (9, 1) => semantic_element(
            spec_name("root", Some(XMLNS), exact(Some("xmlns"))),
            plain("root"),
            EditComponent::NamespaceUri,
        ),
        (9, 2) => semantic_element(
            spec_name("root", Some(XML), exact(None)),
            spec_name("root", Some(XML), exact(Some("xml"))),
            EditComponent::Prefix,
        ),
        (9, 3) => semantic_element(
            spec_name("root", Some(XML), exact(Some("p"))),
            spec_name("root", Some(XML), exact(Some("xml"))),
            EditComponent::Prefix,
        ),
        (9, 4) => semantic_element(
            spec_name("root", Some("urn:p"), exact(Some("xml"))),
            spec_name("root", Some("urn:p"), exact(Some("p"))),
            EditComponent::NamespaceUri,
        ),
        (9, 5) => semantic_element(
            spec_name("root", Some("urn:p"), exact(Some("xmlns"))),
            spec_name("root", Some("urn:p"), exact(Some("p"))),
            EditComponent::Prefix,
        ),
        (10, 0) => semantic_namespace_item(
            [ElementItem::Namespace(NamespaceSpec {
                prefix: Some("xml"),
                uri: "urn:not-xml",
            })],
            [ElementItem::Namespace(NamespaceSpec {
                prefix: Some("xml"),
                uri: XML,
            })],
            EditErrorKind::Namespace,
            0,
        ),
        (10, 1) => semantic_namespace_item(
            [ElementItem::Namespace(NamespaceSpec {
                prefix: Some("p"),
                uri: XML,
            })],
            [ElementItem::Namespace(NamespaceSpec {
                prefix: Some("p"),
                uri: "urn:p",
            })],
            EditErrorKind::Namespace,
            0,
        ),
        (10, 2) => semantic_namespace_item(
            [ElementItem::Namespace(NamespaceSpec {
                prefix: Some("xmlns"),
                uri: "urn:p",
            })],
            [ElementItem::Namespace(NamespaceSpec {
                prefix: Some("p"),
                uri: "urn:p",
            })],
            EditErrorKind::Namespace,
            0,
        ),
        (10, 3) => semantic_namespace_item(
            [ElementItem::Namespace(NamespaceSpec {
                prefix: Some("p"),
                uri: XMLNS,
            })],
            [ElementItem::Namespace(NamespaceSpec {
                prefix: Some("p"),
                uri: "urn:p",
            })],
            EditErrorKind::Namespace,
            0,
        ),
        (10, 4) => semantic_namespace_item(
            [ElementItem::Namespace(NamespaceSpec {
                prefix: Some("p"),
                uri: "",
            })],
            [ElementItem::Namespace(NamespaceSpec {
                prefix: None,
                uri: "",
            })],
            EditErrorKind::Namespace,
            0,
        ),
        (10, 5) => semantic_namespace_item(
            [
                ElementItem::Namespace(NamespaceSpec {
                    prefix: Some("p"),
                    uri: "urn:one",
                }),
                ElementItem::Namespace(NamespaceSpec {
                    prefix: Some("p"),
                    uri: "urn:two",
                }),
            ],
            [
                ElementItem::Namespace(NamespaceSpec {
                    prefix: Some("p"),
                    uri: "urn:one",
                }),
                ElementItem::Namespace(NamespaceSpec {
                    prefix: Some("q"),
                    uri: "urn:two",
                }),
            ],
            EditErrorKind::DuplicateAttribute,
            1,
        ),
        (10, 6) => semantic_namespace_item(
            [
                ElementItem::Namespace(NamespaceSpec {
                    prefix: None,
                    uri: "urn:one",
                }),
                ElementItem::Namespace(NamespaceSpec {
                    prefix: None,
                    uri: "urn:two",
                }),
            ],
            [ElementItem::Namespace(NamespaceSpec {
                prefix: None,
                uri: "",
            })],
            EditErrorKind::DuplicateAttribute,
            1,
        ),
        (11, 0) => semantic_attribute(
            AttributeSpec {
                name: plain("xmlns"),
                value: "ordinary",
            },
            [ElementItem::Attribute(AttributeSpec {
                name: plain("xmlns-ok"),
                value: "ordinary",
            })],
        ),
        (11, 1) => semantic_attribute(
            AttributeSpec {
                name: spec_name("a", Some(XMLNS), exact(Some("p"))),
                value: "v",
            },
            [ElementItem::Attribute(AttributeSpec {
                name: spec_name("a", Some("urn:p"), exact(Some("p"))),
                value: "v",
            })],
        ),
        (11, 2) => semantic_attribute(
            AttributeSpec {
                name: spec_name("a", None, exact(Some("p"))),
                value: "v",
            },
            [ElementItem::Attribute(AttributeSpec {
                name: plain("a"),
                value: "v",
            })],
        ),
        (11, 3) => semantic_attribute(
            AttributeSpec {
                name: spec_name("a", Some(XML), exact(Some("p"))),
                value: "v",
            },
            [ElementItem::Attribute(AttributeSpec {
                name: spec_name("a", Some(XML), exact(Some("xml"))),
                value: "v",
            })],
        ),
        (11, 4) => semantic_attribute(
            AttributeSpec {
                name: spec_name("a", Some("urn:p"), exact(Some("xml"))),
                value: "v",
            },
            [ElementItem::Attribute(AttributeSpec {
                name: spec_name("a", Some("urn:p"), exact(Some("p"))),
                value: "v",
            })],
        ),
        _ => unreachable!("validation family catalog is complete"),
    }
}

fn assert_valid_control(
    document: &OwnedDocument,
    id: OwnedNodeId,
    spec: ElementSpec<'_>,
    parent: OwnedNodeId,
) {
    let node = document.node(id).expect("valid control handle is live");
    assert_eq!(node.kind(), OwnedNodeKind::Element);
    assert_eq!(node.parent().map(|node| node.id()), Some(parent));
    assert_eq!(node.range(), None, "constructed control gained provenance");
    let actual = node.name().expect("valid control lost its element name");
    assert_eq!(actual.local(), spec.name.local);
    assert_eq!(actual.namespace(), spec.name.namespace);
    let PrefixPolicy::Exact(prefix) = spec.name.prefix else {
        unreachable!("validation controls use exact prefixes")
    };
    assert_eq!(actual.prefix(), prefix);
    let qualified = prefix.map_or_else(
        || spec.name.local.to_owned(),
        |prefix| format!("{prefix}:{}", spec.name.local),
    );
    assert_eq!(actual.qualified(), qualified);
    let attributes: Vec<_> = node.attributes().collect();
    assert!(
        attributes.len() >= spec.items.len(),
        "valid control dropped a caller item"
    );
    for (actual, expected) in attributes.iter().zip(spec.items) {
        match expected {
            ElementItem::Namespace(namespace) => {
                assert_eq!(actual.name.prefix(), namespace.prefix.map(|_| "xmlns"));
                assert_eq!(
                    actual.name.local(),
                    namespace.prefix.unwrap_or("xmlns"),
                    "valid control changed a declaration prefix"
                );
                assert_eq!(actual.name.namespace(), Some(XMLNS));
                assert_eq!(actual.value.as_ref(), namespace.uri);
            }
            ElementItem::Attribute(attribute) => {
                assert_eq!(actual.name.local(), attribute.name.local);
                assert_eq!(actual.name.namespace(), attribute.name.namespace);
                let PrefixPolicy::Exact(prefix) = attribute.name.prefix else {
                    unreachable!("validation controls use exact prefixes")
                };
                assert_eq!(actual.name.prefix(), prefix);
                assert_eq!(actual.value.as_ref(), attribute.value);
            }
        }
    }
    assert_serializable(document);
}

fn check_create(case: ValidationCase) {
    let expected = case.expected;
    let actual = OwnedDocument::create(case.invalid.borrowed(), EditLimits::default())
        .expect_err("invalid root construction was accepted");
    assert_error_with_display(actual, expected);

    let control = case.valid.borrowed();
    let document = OwnedDocument::create(control, EditLimits::default())
        .expect("paired valid root construction was rejected");
    let root = document.root().id();
    assert_valid_control(&document, document.root_element().id(), control, root);
}

fn check_append(case: ValidationCase) {
    let (mut document, root_model) = simple_document("host", "stable");
    let parent = document.root_element().id();
    let before = snapshot(&document);
    let expected = case.expected;
    let invalid = case.invalid.borrowed();
    let control = case.valid.borrowed();

    let inserted = {
        let mut editor = document.edit(EditLimits::default()).unwrap();
        let actual = editor
            .append_element(parent, invalid)
            .expect_err("invalid appended construction was accepted");
        assert_error_with_display(actual, expected);
        assert_eq!(
            snapshot(editor.document()),
            before,
            "append validation failure published partial state"
        );
        editor
            .append_element(parent, control)
            .expect("paired valid retry was rejected")
    };

    assert_element(&document, parent, &root_model);
    assert_valid_control(&document, inserted, control, parent);
    let parent_node = document.node(parent).unwrap();
    assert_eq!(
        parent_node.first_child().map(|node| node.id()),
        Some(inserted)
    );
    assert_eq!(
        parent_node.last_child().map(|node| node.id()),
        Some(inserted)
    );
    assert_eq!(parent_node.children().count(), 1);
}

const BINDING_SURFACE_COUNT: u8 = 4;
const BINDING_VARIANT_COUNT: u8 = 3;

#[cfg(test)]
thread_local! {
    static LAST_BINDING_SELECTION: std::cell::Cell<Option<(u8, u8)>> = const {
        std::cell::Cell::new(None)
    };
}

fn binding_items(compatible: bool) -> [ElementItem<'static>; 3] {
    [
        ElementItem::Namespace(NamespaceSpec {
            prefix: None,
            uri: "v",
        }),
        ElementItem::Namespace(NamespaceSpec {
            prefix: Some("r"),
            uri: "u",
        }),
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("a", compatible.then_some("u"), exact(Some("r"))),
            value: "x",
        }),
    ]
}

fn binding_spec<'a>(items: &'a [ElementItem<'static>]) -> ElementSpec<'a> {
    ElementSpec {
        name: spec_name("e", Some("u"), PrefixPolicy::Automatic),
        items,
    }
}

fn binding_limits(variant: u8) -> EditLimits {
    EditLimits {
        max_name_bytes: if variant == 1 { 3 } else { 2 },
        ..EditLimits::default()
    }
}

fn binding_expected_error(variant: u8) -> EditError {
    if variant == 0 {
        located(
            EditErrorKind::LimitExceeded(EditLimit::NameBytes),
            element_location(EditComponent::Prefix, Some(2)),
        )
    } else {
        located(EditErrorKind::Namespace, attribute_location(2, None, None))
    }
}

fn serialized(document: &OwnedDocument) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new());
    writer.write_owned_document(document).unwrap();
    writer.finish().unwrap();
    writer.into_inner()
}

fn assert_binding_success(document: &OwnedDocument, id: OwnedNodeId, parent: OwnedNodeId) {
    assert_element(
        document,
        id,
        &ModelElement {
            name: ModelName::new(Some("r"), "e", Some("u")),
            attributes: vec![
                declaration(None, "v"),
                declaration(Some("r"), "u"),
                attribute(Some("r"), "a", Some("u"), "x"),
            ],
        },
    );
    assert_eq!(
        document.node(id).unwrap().parent().map(|node| node.id()),
        Some(parent)
    );
}

fn check_binding_create(variant: u8) {
    let compatible = variant == 2;
    let items = binding_items(compatible);
    let limits = binding_limits(variant);
    if compatible {
        let document = OwnedDocument::create(binding_spec(&items), limits)
            .expect("compatible exact prefix was rejected");
        assert_binding_success(
            &document,
            document.root_element().id(),
            document.root().id(),
        );
        assert_eq!(
            serialized(&document),
            b"<r:e xmlns=\"v\" xmlns:r=\"u\" r:a=\"x\"/>"
        );
        return;
    }

    let expected = binding_expected_error(variant);
    let actual = OwnedDocument::create(binding_spec(&items), limits)
        .expect_err("incompatible exact prefix was accepted");
    assert_error(actual, expected);

    // A failed constructor has no editor to recover. The paired exact control
    // proves that the same bounds do not reject an unrelated valid root.
    let document = OwnedDocument::create(
        ElementSpec {
            name: plain("ok"),
            items: &[],
        },
        limits,
    )
    .expect("constructor did not accept its paired valid control");
    assert_eq!(serialized(&document), b"<ok/>");
}

fn assert_binding_append_topology(
    document: &OwnedDocument,
    before: &Snapshot,
    parent: OwnedNodeId,
    inserted: OwnedNodeId,
) {
    let old_ids: Vec<_> = before.nodes.iter().map(|node| node.id).collect();
    let old_child = before
        .nodes
        .iter()
        .find(|node| node.parent == Some(parent))
        .expect("fixed parent lost its original child")
        .id;
    let parent_node = document.node(parent).unwrap();
    assert_eq!(
        parent_node
            .children()
            .map(|node| node.id())
            .collect::<Vec<_>>(),
        vec![old_child, inserted]
    );
    assert_eq!(
        parent_node.first_child().map(|node| node.id()),
        Some(old_child)
    );
    assert_eq!(
        parent_node.last_child().map(|node| node.id()),
        Some(inserted)
    );
    assert_eq!(
        parent_node
            .descendants()
            .map(|node| node.id())
            .collect::<Vec<_>>(),
        vec![parent, old_child, inserted]
    );
    assert_eq!(
        document.nodes().map(|node| node.id()).collect::<Vec<_>>(),
        old_ids
            .iter()
            .copied()
            .chain(std::iter::once(inserted))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        document
            .node(old_child)
            .unwrap()
            .next_sibling()
            .map(|node| node.id()),
        Some(inserted)
    );
    let inserted_node = document.node(inserted).unwrap();
    assert_eq!(inserted_node.parent().map(|node| node.id()), Some(parent));
    assert!(inserted_node.next_sibling().is_none());
    assert!(inserted_node.first_child().is_none());
    assert!(inserted_node.last_child().is_none());
}

fn check_binding_append(surface: u8, variant: u8) {
    const SOURCE: &str = "<host keep='stable'><old/></host>";
    let mut bytes = encode(SOURCE, surface - 1);
    let (parent_range, old_range) = match surface {
        1 => {
            assert_eq!(bytes, SOURCE.as_bytes());
            (0..33, 20..26)
        }
        2 => {
            assert_eq!(&bytes[..6], &[0xff, 0xfe, 0x3c, 0x00, 0x68, 0x00]);
            (2..68, 42..54)
        }
        3 => {
            assert_eq!(&bytes[..6], &[0xfe, 0xff, 0x00, 0x3c, 0x00, 0x68]);
            (2..68, 42..54)
        }
        _ => unreachable!("append surface is normalized to 1..=3"),
    };
    let mut document = OwnedDocument::parse_bytes(&bytes).expect("fixed parent must parse");
    bytes.fill(0);
    drop(bytes);

    let parent = document.root_element().id();
    let old_child = document
        .node(parent)
        .unwrap()
        .first_child()
        .expect("fixed parent has its old child");
    assert_eq!(document.node(parent).unwrap().range(), Some(parent_range));
    assert_eq!(old_child.range(), Some(old_range));
    let before = snapshot(&document);
    let before_bytes = serialized(&document);
    let compatible = variant == 2;
    let items = binding_items(compatible);
    let limits = binding_limits(variant);
    let expected = (!compatible).then(|| binding_expected_error(variant));
    let mut editor = document.edit(limits).unwrap();
    if compatible {
        let inserted = editor
            .append_element(parent, binding_spec(&items))
            .expect("compatible exact prefix append was rejected");
        assert_binding_success(editor.document(), inserted, parent);
        assert_binding_append_topology(editor.document(), &before, parent, inserted);
        assert_eq!(
            serialized(editor.document()),
            b"<host keep=\"stable\"><old/><r:e xmlns=\"v\" xmlns:r=\"u\" r:a=\"x\"/></host>"
        );
        return;
    }

    let actual = editor
        .append_element(parent, binding_spec(&items))
        .expect_err("incompatible exact prefix append was accepted");
    assert_error(actual, expected.expect("rejection expectation is present"));
    assert_eq!(
        snapshot(editor.document()),
        before,
        "rejection changed topology"
    );
    assert_eq!(
        serialized(editor.document()),
        before_bytes,
        "rejection changed output"
    );

    let recovery = editor
        .append_element(
            parent,
            ElementSpec {
                name: plain("ok"),
                items: &[],
            },
        )
        .expect("same editor did not recover after rejection");
    assert_element(
        editor.document(),
        recovery,
        &ModelElement {
            name: ModelName::new(None, "ok", None),
            attributes: Vec::new(),
        },
    );
    assert_binding_append_topology(editor.document(), &before, parent, recovery);
    assert_eq!(
        serialized(editor.document()),
        b"<host keep=\"stable\"><old/><ok/></host>"
    );
}

fn binding_limit_precedence(surface: u8, variant: u8) {
    let surface = surface % BINDING_SURFACE_COUNT;
    let variant = variant % BINDING_VARIANT_COUNT;
    #[cfg(test)]
    LAST_BINDING_SELECTION.with(|selection| selection.set(Some((surface, variant))));
    if surface == 0 {
        check_binding_create(variant);
    } else {
        check_binding_append(surface, variant);
    }
}

fn exercise_binding_limit_precedence(input: &[u8]) {
    let byte = |index| input.get(index).copied().unwrap_or(0);
    binding_limit_precedence(byte(7), byte(8));
}

pub(super) fn exercise(input: &[u8]) {
    assert!(input.len() <= super::super::MAX_INPUT_BYTES);
    let byte = |index| input.get(index).copied().unwrap_or(0);
    let case = validation_case(byte(4), byte(5));
    if byte(6) & 1 == 0 {
        check_create(case);
    } else {
        check_append(case);
    }
    exercise_binding_limit_precedence(input);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_literal_validation_case_rejects_and_its_control_succeeds() {
        for family in 0..FAMILY_COUNT {
            for subtype in 0..FAMILY_VARIANTS[usize::from(family)] {
                for append in 0..=1 {
                    let result = std::panic::catch_unwind(|| {
                        exercise(&[0, 0, 0, 0, family, subtype, append]);
                    });
                    assert!(
                        result.is_ok(),
                        "validation family {family}, subtype {subtype}, append {append} failed"
                    );
                }
            }
        }
    }

    #[test]
    fn selectors_are_bounded_for_arbitrary_bytes_and_short_inputs() {
        for input in [
            b"".as_slice(),
            b"short",
            b"\xff\xff\xff\xff\xff\xff\xff",
            b"\0\0\0\0\x0b\x04\x01",
        ] {
            exercise(input);
        }
    }

    #[test]
    fn binding_limit_precedence_catalog_covers_all_surfaces_and_variants() {
        for surface in 0..BINDING_SURFACE_COUNT {
            for variant in 0..BINDING_VARIANT_COUNT {
                let result = std::panic::catch_unwind(|| {
                    binding_limit_precedence(surface, variant);
                });
                assert!(
                    result.is_ok(),
                    "surface {surface}, variant {variant} failed"
                );
            }
        }
    }

    #[test]
    fn binding_limit_precedence_dispatch_is_total_and_bounded() {
        for surface in 0..BINDING_SURFACE_COUNT {
            for variant in 0..BINDING_VARIANT_COUNT {
                let mut input = [0_u8; 9];
                input[7] = surface;
                input[8] = variant;
                exercise_binding_limit_precedence(&input);
            }
        }
        exercise_binding_limit_precedence(&[]);
        exercise_binding_limit_precedence(&[0xff; 9]);
    }

    #[test]
    fn top_level_driver_reaches_every_binding_limit_selection() {
        fn assert_selection(input: &[u8], expected: (u8, u8)) {
            LAST_BINDING_SELECTION.with(|selection| selection.set(None));
            super::super::super::exercise(input);
            assert_eq!(
                LAST_BINDING_SELECTION.with(std::cell::Cell::get),
                Some(expected),
                "top-level driver skipped the binding catalog for {input:?}"
            );
        }

        for surface in 0..BINDING_SURFACE_COUNT {
            for variant in 0..BINDING_VARIANT_COUNT {
                let mut input = [0_u8; 9];
                input[7] = surface;
                input[8] = variant;
                assert_selection(&input, (surface, variant));
            }
        }
        assert_selection(&[], (0, 0));
        assert_selection(b"\xff", (0, 0));
        assert_selection(b"\xff\xfe<\0r\0/\0>\0", (0, 2));
    }
}
