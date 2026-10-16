//! Independent, hand-counted construction limit model.
//!
//! This child of `construction` selects one bounded family from input byte 8;
//! byte 9 selects a literal case inside that family. Expected errors and public
//! states are fixed before each API call.

use super::*;

#[derive(Debug, Eq, PartialEq)]
struct PublicSnapshot {
    tree: Snapshot,
    serialized: Vec<u8>,
}

fn public_snapshot(document: &OwnedDocument) -> PublicSnapshot {
    let mut writer = Writer::new(Vec::new());
    writer.write_owned_document(document).unwrap();
    writer.finish().unwrap();
    PublicSnapshot {
        tree: snapshot(document),
        serialized: writer.into_inner(),
    }
}

fn empty_spec(local: &str) -> ElementSpec<'_> {
    ElementSpec {
        name: spec_name(local, None, exact(None)),
        items: &[],
    }
}

fn namespace_limits(max_namespace_work: usize) -> EditLimits {
    EditLimits {
        max_namespace_work,
        ..EditLimits::default()
    }
}

fn expect_namespace_work_rejection(
    document: &mut OwnedDocument,
    parent: OwnedNodeId,
    spec: ElementSpec<'_>,
    limit: usize,
    recovery_parent: OwnedNodeId,
) {
    let expected = global(EditErrorKind::LimitExceeded(EditLimit::NamespaceWork));
    let before = public_snapshot(document);
    let mut editor = document.edit(namespace_limits(limit)).unwrap();
    let actual = editor
        .append_element(parent, spec)
        .expect_err("below-bound namespace work was accepted");
    assert_error(actual, expected);
    assert_eq!(
        public_snapshot(editor.document()),
        before,
        "namespace-work rejection published state"
    );
    editor
        .append_text(recovery_parent, "recovered")
        .expect("namespace-work rejection poisoned the editor");
}

fn assert_plain_child(document: &OwnedDocument, id: OwnedNodeId, local: &str) {
    assert_element(
        document,
        id,
        &ModelElement {
            name: ModelName::new(None, local, None),
            attributes: Vec::new(),
        },
    );
}

fn namespace_scope_depth_boundary() {
    // Exact(None) probes one default binding per element ancestor: c, b, a.
    const EXACT: usize = 3;
    let spec = empty_spec("d");

    let mut below = OwnedDocument::parse("<a><b><c/></b></a>").unwrap();
    let root = below.root_element().id();
    let parent = below
        .root_element()
        .first_child()
        .unwrap()
        .first_child()
        .unwrap()
        .id();
    expect_namespace_work_rejection(&mut below, parent, spec, EXACT - 1, root);

    for limit in [EXACT, EXACT + 1] {
        let mut document = OwnedDocument::parse("<a><b><c/></b></a>").unwrap();
        let parent = document
            .root_element()
            .first_child()
            .unwrap()
            .first_child()
            .unwrap()
            .id();
        let child = document
            .edit(namespace_limits(limit))
            .unwrap()
            .append_element(parent, spec)
            .expect("exact/above ancestor-step budget was rejected");
        assert_plain_child(&document, child, "d");
    }
}

fn namespace_wide_ancestor_boundary() {
    // Three ancestor probes plus the two ordinary root attributes cost five.
    const EXACT: usize = 5;
    let spec = empty_spec("d");

    let mut below = OwnedDocument::parse("<r a='1' b='2'><e><f/></e></r>").unwrap();
    let root = below.root_element().id();
    let parent = below
        .root_element()
        .first_child()
        .unwrap()
        .first_child()
        .unwrap()
        .id();
    expect_namespace_work_rejection(&mut below, parent, spec, EXACT - 1, root);

    for limit in [EXACT, EXACT + 1] {
        let mut document = OwnedDocument::parse("<r a='1' b='2'><e><f/></e></r>").unwrap();
        let parent = document
            .root_element()
            .first_child()
            .unwrap()
            .first_child()
            .unwrap()
            .id();
        let child = document
            .edit(namespace_limits(limit))
            .unwrap()
            .append_element(parent, spec)
            .expect("exact/above ancestor-attribute budget was rejected");
        assert_plain_child(&document, child, "d");
    }
}

fn default_uri_comparison_boundary() {
    // One ancestor, one attribute, one declaration candidate, and two
    // five-byte URI operands cost 13 work units.
    const EXACT: usize = 13;
    let root_items = [
        ElementItem::Namespace(NamespaceSpec {
            prefix: None,
            uri: "urn:u",
        }),
        ElementItem::Namespace(NamespaceSpec {
            prefix: Some("p"),
            uri: "urn:u",
        }),
    ];
    let child_spec = ElementSpec {
        name: spec_name("child", Some("urn:u"), PrefixPolicy::Automatic),
        items: &[],
    };
    let make_document = || {
        OwnedDocument::create(
            ElementSpec {
                name: spec_name("root", Some("urn:u"), PrefixPolicy::Automatic),
                items: &root_items,
            },
            EditLimits::default(),
        )
        .unwrap()
    };

    let mut below = make_document();
    let root = below.root_element().id();
    expect_namespace_work_rejection(&mut below, root, child_spec, EXACT - 1, root);

    for limit in [EXACT, EXACT + 1] {
        let mut document = make_document();
        let root = document.root_element().id();
        let child = document
            .edit(namespace_limits(limit))
            .unwrap()
            .append_element(root, child_spec)
            .expect("exact/above default-URI comparison budget was rejected");
        assert_element(
            &document,
            child,
            &ModelElement {
                name: ModelName::new(None, "child", Some("urn:u")),
                attributes: Vec::new(),
            },
        );
    }
}

fn explicit_prefix_and_uri_comparison_boundary() {
    // The unnamespaced child checks the nondefault local declaration (2), then
    // the same local scope (2) and its parent (1). Resolving p:a probes and
    // compares p (3), then compares both five-byte URI operands (10): 18.
    const EXACT: usize = 18;
    let items = [
        ElementItem::Namespace(NamespaceSpec {
            prefix: Some("p"),
            uri: "urn:x",
        }),
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("a", Some("urn:x"), exact(Some("p"))),
            value: "v",
        }),
    ];
    let spec = ElementSpec {
        name: spec_name("child", None, exact(None)),
        items: &items,
    };

    let mut below = OwnedDocument::parse("<r/>").unwrap();
    let root = below.root_element().id();
    expect_namespace_work_rejection(&mut below, root, spec, EXACT - 1, root);

    for limit in [EXACT, EXACT + 1] {
        let mut document = OwnedDocument::parse("<r/>").unwrap();
        let root = document.root_element().id();
        let child = document
            .edit(namespace_limits(limit))
            .unwrap()
            .append_element(root, spec)
            .expect("exact/above prefix-and-URI comparison budget was rejected");
        assert_element(
            &document,
            child,
            &ModelElement {
                name: ModelName::new(None, "child", None),
                attributes: vec![
                    declaration(Some("p"), "urn:x"),
                    attribute(Some("p"), "a", Some("urn:x"), "v"),
                ],
            },
        );
    }
}

fn expanded_local_comparison_boundary() {
    // The child probes its parent once. Comparing the second attribute with the
    // first costs one candidate probe plus two two-byte local operands: six total.
    const EXACT: usize = 6;
    let duplicate_items = [
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("ab", None, exact(None)),
            value: "1",
        }),
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("ab", None, exact(None)),
            value: "2",
        }),
    ];
    let distinct_items = [
        duplicate_items[0],
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("ac", None, exact(None)),
            value: "2",
        }),
    ];
    let duplicate = ElementSpec {
        name: spec_name("child", None, exact(None)),
        items: &duplicate_items,
    };
    let distinct = ElementSpec {
        name: duplicate.name,
        items: &distinct_items,
    };

    let mut below = OwnedDocument::parse("<r/>").unwrap();
    let root = below.root_element().id();
    expect_namespace_work_rejection(&mut below, root, duplicate, EXACT - 1, root);

    let mut semantic = OwnedDocument::parse("<r/>").unwrap();
    let root = semantic.root_element().id();
    let expected = located(
        EditErrorKind::DuplicateAttribute,
        attribute_location(1, None, None),
    );
    let before = public_snapshot(&semantic);
    let mut editor = semantic.edit(namespace_limits(EXACT)).unwrap();
    let actual = editor
        .append_element(root, duplicate)
        .expect_err("expanded duplicate was accepted at its exact work bound");
    assert_error(actual, expected);
    assert_eq!(public_snapshot(editor.document()), before);
    editor
        .append_element(root, empty_spec("ok"))
        .expect("duplicate rejection poisoned the editor");

    for limit in [EXACT, EXACT + 1] {
        let mut document = OwnedDocument::parse("<r/>").unwrap();
        let root = document.root_element().id();
        let child = document
            .edit(namespace_limits(limit))
            .unwrap()
            .append_element(root, distinct)
            .expect("exact/above expanded-name comparison budget was rejected");
        assert_element(
            &document,
            child,
            &ModelElement {
                name: ModelName::new(None, "child", None),
                attributes: vec![
                    attribute(None, "ab", None, "1"),
                    attribute(None, "ac", None, "2"),
                ],
            },
        );
    }
}

fn zero_namespace_work_boundary() {
    for limit in [0, 1] {
        let document = OwnedDocument::create(empty_spec("root"), namespace_limits(limit))
            .expect("zero-work construction was rejected");
        assert_element(
            &document,
            document.root_element().id(),
            &ModelElement {
                name: ModelName::new(None, "root", None),
                attributes: Vec::new(),
            },
        );
    }
}

fn namespace_work_family(variant: u8) {
    match variant % 6 {
        0 => zero_namespace_work_boundary(),
        1 => namespace_scope_depth_boundary(),
        2 => namespace_wide_ancestor_boundary(),
        3 => default_uri_comparison_boundary(),
        4 => explicit_prefix_and_uri_comparison_boundary(),
        _ => expanded_local_comparison_boundary(),
    }
}

fn generated_prefix_name_boundary() {
    let items = [ElementItem::Attribute(AttributeSpec {
        name: spec_name("a", Some("u"), PrefixPolicy::Automatic),
        value: "v",
    })];
    let spec = ElementSpec {
        name: spec_name("c", None, exact(None)),
        items: &items,
    };

    let mut below = OwnedDocument::parse("<r/>").unwrap();
    let root = below.root_element().id();
    let expected = located(
        EditErrorKind::LimitExceeded(EditLimit::NameBytes),
        attribute_location(0, Some(EditComponent::Prefix), Some(2)),
    );
    let before = public_snapshot(&below);
    let mut editor = below
        .edit(EditLimits {
            max_name_bytes: 2,
            ..EditLimits::default()
        })
        .unwrap();
    let actual = editor
        .append_element(root, spec)
        .expect_err("over-limit generated prefix was accepted");
    assert_error(actual, expected);
    assert_eq!(public_snapshot(editor.document()), before);
    let recovered = editor
        .append_element(root, empty_spec("ok"))
        .expect("generated-prefix rejection poisoned the editor");
    assert_plain_child(editor.document(), recovered, "ok");

    for limit in [3, 4] {
        let mut document = OwnedDocument::parse("<r/>").unwrap();
        let root = document.root_element().id();
        let child = document
            .edit(EditLimits {
                max_name_bytes: limit,
                ..EditLimits::default()
            })
            .unwrap()
            .append_element(root, spec)
            .expect("exact/above generated-prefix name bound was rejected");
        assert_element(
            &document,
            child,
            &ModelElement {
                name: ModelName::new(None, "c", None),
                attributes: vec![
                    attribute(Some("ns1"), "a", Some("u"), "v"),
                    declaration(Some("ns1"), "u"),
                ],
            },
        );
    }
}

fn generated_item_count_boundary() {
    let items = [ElementItem::Attribute(AttributeSpec {
        name: spec_name("a", Some("u"), PrefixPolicy::Automatic),
        value: "v",
    })];
    let spec = ElementSpec {
        name: spec_name("c", None, exact(None)),
        items: &items,
    };

    let mut below = OwnedDocument::parse("<r/>").unwrap();
    let root = below.root_element().id();
    let expected = global(EditErrorKind::LimitExceeded(EditLimit::Attributes));
    let before = public_snapshot(&below);
    let mut editor = below
        .edit(EditLimits {
            max_attributes: 1,
            ..EditLimits::default()
        })
        .unwrap();
    let actual = editor
        .append_element(root, spec)
        .expect_err("generated declaration exceeded the item ceiling");
    assert_error(actual, expected);
    assert_eq!(public_snapshot(editor.document()), before);
    editor
        .append_element(root, empty_spec("ok"))
        .expect("item-count rejection poisoned the editor");

    for limit in [2, 3] {
        let mut document = OwnedDocument::parse("<r/>").unwrap();
        let root = document.root_element().id();
        let child = document
            .edit(EditLimits {
                max_attributes: limit,
                ..EditLimits::default()
            })
            .unwrap()
            .append_element(root, spec)
            .expect("exact/above generated item count was rejected");
        assert_eq!(document.node(child).unwrap().attributes().count(), 2);
    }
}

fn generated_bound_family(variant: u8) {
    if variant & 1 == 0 {
        generated_prefix_name_boundary();
    } else {
        generated_item_count_boundary();
    }
}

fn append_depth_boundary() {
    let spec = empty_spec("child");
    let mut below = OwnedDocument::parse("<r><p/></r>").unwrap();
    let root = below.root_element().id();
    let parent = below.root_element().first_child().unwrap().id();
    let expected = global(EditErrorKind::LimitExceeded(EditLimit::Depth));
    let before = public_snapshot(&below);
    let mut editor = below
        .edit(EditLimits {
            max_depth: 2,
            ..EditLimits::default()
        })
        .unwrap();
    let actual = editor
        .append_element(parent, spec)
        .expect_err("over-depth append was accepted");
    assert_error(actual, expected);
    assert_eq!(public_snapshot(editor.document()), before);
    editor
        .append_element(root, empty_spec("ok"))
        .expect("depth rejection poisoned the editor");

    for limit in [3, 4] {
        let mut document = OwnedDocument::parse("<r><p/></r>").unwrap();
        let parent = document.root_element().first_child().unwrap().id();
        let child = document
            .edit(EditLimits {
                max_depth: limit,
                ..EditLimits::default()
            })
            .unwrap()
            .append_element(parent, spec)
            .expect("exact/above append depth was rejected");
        assert_plain_child(&document, child, "child");
    }
}

fn append_explicit_item_boundary() {
    let items = [
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("a", None, exact(None)),
            value: "1",
        }),
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("b", None, exact(None)),
            value: "2",
        }),
    ];
    let spec = ElementSpec {
        name: spec_name("child", None, exact(None)),
        items: &items,
    };
    let mut below = OwnedDocument::parse("<r/>").unwrap();
    let root = below.root_element().id();
    let expected = global(EditErrorKind::LimitExceeded(EditLimit::Attributes));
    let before = public_snapshot(&below);
    let mut editor = below
        .edit(EditLimits {
            max_attributes: 1,
            ..EditLimits::default()
        })
        .unwrap();
    let actual = editor
        .append_element(root, spec)
        .expect_err("over-count explicit items were accepted");
    assert_error(actual, expected);
    assert_eq!(public_snapshot(editor.document()), before);
    editor
        .append_element(root, empty_spec("ok"))
        .expect("item-count rejection poisoned the editor");

    for limit in [2, 3] {
        let mut document = OwnedDocument::parse("<r/>").unwrap();
        let root = document.root_element().id();
        let child = document
            .edit(EditLimits {
                max_attributes: limit,
                ..EditLimits::default()
            })
            .unwrap()
            .append_element(root, spec)
            .expect("exact/above explicit item count was rejected");
        assert_eq!(document.node(child).unwrap().attributes().count(), 2);
    }
}

fn append_shape_family(variant: u8) {
    if variant & 1 == 0 {
        append_depth_boundary();
    } else {
        append_explicit_item_boundary();
    }
}

fn persistent_created_nodes() {
    let items = [ElementItem::Attribute(AttributeSpec {
        name: spec_name("a", None, exact(None)),
        value: "1",
    })];
    let attributed = ElementSpec {
        name: spec_name("child", None, exact(None)),
        items: &items,
    };
    let mut document = OwnedDocument::parse("<r/>").unwrap();
    let root = document.root_element().id();
    let child = {
        let mut editor = document
            .edit(EditLimits {
                max_created_nodes: 1,
                max_created_attributes: 1,
                ..EditLimits::default()
            })
            .unwrap();
        let child = editor
            .append_element(root, attributed)
            .expect("first node expenditure was rejected");
        let before_failure = public_snapshot(editor.document());
        let expected = global(EditErrorKind::LimitExceeded(EditLimit::CreatedNodes));
        let actual = editor
            .append_element(root, empty_spec("second"))
            .expect_err("exhausted created-node quota was accepted");
        assert_error(actual, expected);
        assert_eq!(public_snapshot(editor.document()), before_failure);
        // Removal is a valid same-editor recovery operation even though this
        // immutable creation ceiling cannot admit another append.
        editor.remove_subtree(child).unwrap();
        child
    };

    let after_removal = public_snapshot(&document);
    let expected_reopen = global(EditErrorKind::LimitExceeded(EditLimit::CreatedNodes));
    let actual_reopen = document
        .edit(EditLimits {
            max_created_nodes: 0,
            max_created_attributes: 1,
            ..EditLimits::default()
        })
        .err()
        .expect("removal refunded the created-node expenditure");
    assert_error(actual_reopen, expected_reopen);
    assert_eq!(public_snapshot(&document), after_removal);

    let replacement = document
        .edit(EditLimits {
            max_created_nodes: 2,
            max_created_attributes: 1,
            ..EditLimits::default()
        })
        .unwrap()
        .append_element(root, empty_spec("replacement"))
        .expect("higher created-node ceiling did not recover");
    assert_ne!(replacement, child, "removed handle was reused");
}

fn persistent_created_attributes() {
    let items = [ElementItem::Attribute(AttributeSpec {
        name: spec_name("a", None, exact(None)),
        value: "1",
    })];
    let attributed = ElementSpec {
        name: spec_name("child", None, exact(None)),
        items: &items,
    };
    let mut document = OwnedDocument::parse("<r/>").unwrap();
    let root = document.root_element().id();
    {
        let mut editor = document
            .edit(EditLimits {
                max_created_nodes: 2,
                max_created_attributes: 1,
                ..EditLimits::default()
            })
            .unwrap();
        let child = editor
            .append_element(root, attributed)
            .expect("first attribute expenditure was rejected");
        let before_failure = public_snapshot(editor.document());
        let expected = global(EditErrorKind::LimitExceeded(EditLimit::CreatedAttributes));
        let actual = editor
            .append_element(root, attributed)
            .expect_err("exhausted created-attribute quota was accepted");
        assert_error(actual, expected);
        assert_eq!(public_snapshot(editor.document()), before_failure);
        // Removal proves the exhausted editor remains usable; the raised-limit
        // editor below separately proves later append recovery.
        editor.remove_subtree(child).unwrap();
    }

    let after_removal = public_snapshot(&document);
    let expected_reopen = global(EditErrorKind::LimitExceeded(EditLimit::CreatedAttributes));
    let actual_reopen = document
        .edit(EditLimits {
            max_created_nodes: 1,
            max_created_attributes: 0,
            ..EditLimits::default()
        })
        .err()
        .expect("removal refunded the created-attribute expenditure");
    assert_error(actual_reopen, expected_reopen);
    assert_eq!(public_snapshot(&document), after_removal);

    document
        .edit(EditLimits {
            max_created_nodes: 2,
            max_created_attributes: 2,
            ..EditLimits::default()
        })
        .unwrap()
        .append_element(root, attributed)
        .expect("higher created-attribute ceiling did not recover");
}

fn persistent_quota_family(variant: u8) {
    if variant & 1 == 0 {
        persistent_created_nodes();
    } else {
        persistent_created_attributes();
    }
}

#[derive(Clone, Copy)]
enum LengthCase {
    AttributeLocal,
    AttributeNamespace,
    AttributePrefix,
    AttributeValue,
    AggregateValue,
}

fn length_item(case: LengthCase) -> ElementItem<'static> {
    match case {
        LengthCase::AttributeLocal => ElementItem::Attribute(AttributeSpec {
            name: spec_name("name", None, exact(None)),
            value: "v",
        }),
        LengthCase::AttributeNamespace => ElementItem::Attribute(AttributeSpec {
            name: spec_name("a", Some("urn:u"), exact(Some("p"))),
            value: "v",
        }),
        LengthCase::AttributePrefix => ElementItem::Attribute(AttributeSpec {
            name: spec_name("a", Some("u"), exact(Some("pre"))),
            value: "v",
        }),
        LengthCase::AttributeValue | LengthCase::AggregateValue => {
            ElementItem::Attribute(AttributeSpec {
                name: spec_name("a", None, exact(None)),
                value: "four",
            })
        }
    }
}

fn length_limits(case: LengthCase, exact: bool) -> (EditLimits, EditError) {
    match case {
        LengthCase::AttributeLocal => (
            EditLimits {
                max_name_bytes: if exact { 4 } else { 3 },
                max_supplied_bytes: if exact { usize::MAX } else { 3 },
                ..EditLimits::default()
            },
            located(
                EditErrorKind::LimitExceeded(EditLimit::NameBytes),
                attribute_location(0, Some(EditComponent::LocalName), Some(3)),
            ),
        ),
        LengthCase::AttributeNamespace => (
            EditLimits {
                max_name_bytes: if exact { 5 } else { 4 },
                max_supplied_bytes: if exact { usize::MAX } else { 6 },
                ..EditLimits::default()
            },
            located(
                EditErrorKind::LimitExceeded(EditLimit::NameBytes),
                attribute_location(0, Some(EditComponent::NamespaceUri), Some(4)),
            ),
        ),
        LengthCase::AttributePrefix => (
            EditLimits {
                max_name_bytes: if exact { 3 } else { 2 },
                max_supplied_bytes: if exact { usize::MAX } else { 5 },
                ..EditLimits::default()
            },
            located(
                EditErrorKind::LimitExceeded(EditLimit::NameBytes),
                attribute_location(0, Some(EditComponent::Prefix), Some(2)),
            ),
        ),
        LengthCase::AttributeValue => (
            EditLimits {
                max_value_bytes: if exact { 4 } else { 3 },
                max_supplied_bytes: if exact { usize::MAX } else { 5 },
                ..EditLimits::default()
            },
            located(
                EditErrorKind::LimitExceeded(EditLimit::ValueBytes),
                attribute_location(0, Some(EditComponent::Value), Some(3)),
            ),
        ),
        LengthCase::AggregateValue => (
            EditLimits {
                max_value_bytes: 4,
                max_supplied_bytes: if exact { 6 } else { 5 },
                ..EditLimits::default()
            },
            located(
                EditErrorKind::LimitExceeded(EditLimit::SuppliedBytes),
                attribute_location(0, Some(EditComponent::Value), Some(3)),
            ),
        ),
    }
}

fn length_precedence_family(variant: u8) {
    let case = match variant % 5 {
        0 => LengthCase::AttributeLocal,
        1 => LengthCase::AttributeNamespace,
        2 => LengthCase::AttributePrefix,
        3 => LengthCase::AttributeValue,
        _ => LengthCase::AggregateValue,
    };
    let items = [length_item(case)];
    let spec = ElementSpec {
        name: spec_name("c", None, exact(None)),
        items: &items,
    };
    let (below_limits, expected) = length_limits(case, false);
    let mut below = OwnedDocument::parse("<r/>").unwrap();
    let root = below.root_element().id();
    let before = public_snapshot(&below);
    let mut editor = below.edit(below_limits).unwrap();
    let actual = editor
        .append_element(root, spec)
        .expect_err("below-bound non-root string was accepted");
    assert_error(actual, expected);
    assert_eq!(public_snapshot(editor.document()), before);
    editor
        .append_element(root, empty_spec("ok"))
        .expect("length rejection poisoned the editor");

    let (exact_limits, _) = length_limits(case, true);
    for limits in [
        exact_limits,
        EditLimits {
            max_name_bytes: exact_limits.max_name_bytes.saturating_add(1),
            max_value_bytes: exact_limits.max_value_bytes.saturating_add(1),
            max_supplied_bytes: exact_limits.max_supplied_bytes.saturating_add(1),
            ..exact_limits
        },
    ] {
        let mut document = OwnedDocument::parse("<r/>").unwrap();
        let root = document.root_element().id();
        let child = document
            .edit(limits)
            .unwrap()
            .append_element(root, spec)
            .expect("exact/above non-root string bound was rejected");
        let expected_attributes = match case {
            LengthCase::AttributeLocal => vec![attribute(None, "name", None, "v")],
            LengthCase::AttributeNamespace => vec![
                attribute(Some("p"), "a", Some("urn:u"), "v"),
                declaration(Some("p"), "urn:u"),
            ],
            LengthCase::AttributePrefix => vec![
                attribute(Some("pre"), "a", Some("u"), "v"),
                declaration(Some("pre"), "u"),
            ],
            LengthCase::AttributeValue | LengthCase::AggregateValue => {
                vec![attribute(None, "a", None, "four")]
            }
        };
        assert_element(
            &document,
            child,
            &ModelElement {
                name: ModelName::new(None, "c", None),
                attributes: expected_attributes,
            },
        );
    }
}

pub(super) fn exercise(input: &[u8]) {
    assert!(input.len() <= super::super::MAX_INPUT_BYTES);
    let family = input.get(8).copied().unwrap_or(0) % 5;
    let variant = input.get(9).copied().unwrap_or(0);
    match family {
        0 => namespace_work_family(variant),
        1 => generated_bound_family(variant),
        2 => append_shape_family(variant),
        3 => persistent_quota_family(variant),
        _ => length_precedence_family(variant),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected(family: u8, variant: u8) -> [u8; 10] {
        let mut input = [0; 10];
        input[8] = family;
        input[9] = variant;
        input
    }

    #[test]
    fn namespace_work_catalog_checks_below_exact_and_above() {
        for variant in 0..6 {
            exercise(&selected(0, variant));
        }
    }

    #[test]
    fn generated_bounds_check_name_and_final_item_count() {
        for variant in 0..2 {
            exercise(&selected(1, variant));
        }
    }

    #[test]
    fn append_shape_checks_depth_and_explicit_items_before_strings() {
        for variant in 0..2 {
            exercise(&selected(2, variant));
        }
    }

    #[test]
    fn successful_append_expenditure_persists_across_removal_and_reopen() {
        for variant in 0..2 {
            exercise(&selected(3, variant));
        }
    }

    #[test]
    fn non_root_string_limits_preserve_argument_precedence() {
        for variant in 0..5 {
            exercise(&selected(4, variant));
        }
    }

    #[test]
    #[should_panic(expected = "construction error contract changed")]
    fn limit_oracle_rejects_a_shifted_non_root_offset() {
        assert_error(
            located(
                EditErrorKind::LimitExceeded(EditLimit::NameBytes),
                attribute_location(0, Some(EditComponent::Prefix), Some(1)),
            ),
            located(
                EditErrorKind::LimitExceeded(EditLimit::NameBytes),
                attribute_location(0, Some(EditComponent::Prefix), Some(2)),
            ),
        );
    }
}
