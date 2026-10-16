//! Literal namespace-resolution expectations for owned-tree construction.
//!
//! Each selected case has a small fixed catalog of bindings and names. Expected
//! lexical prefixes and expanded names are written out here; this model never
//! calls the implementation's prefix planner to predict its answer.

use super::*;

const CASE_COUNT: u8 = 17;

fn check_created(document: &OwnedDocument, id: OwnedNodeId, expected: &ModelElement) {
    assert_element(document, id, expected);
    assert_serializable(document);
}

fn make_local(input: &[u8]) -> String {
    // Keep names small and valid; byte 6 selects the namespace case, byte 7
    // selects the parent encoding, and up to 8 remaining bytes vary the name.
    derived_name(input.get(8..).unwrap_or_default())
}

fn assert_existing_nodes_preserved(
    document: &OwnedDocument,
    before: &Snapshot,
    appended_parent: OwnedNodeId,
) {
    for expected in &before.nodes {
        let actual = document
            .node(expected.id)
            .expect("append invalidated an existing node");
        assert_eq!(actual.kind(), expected.kind, "existing node kind changed");
        assert_eq!(
            actual.name().map(observed_name),
            expected.name,
            "existing node name changed"
        );
        assert_eq!(
            actual.text().map(str::to_owned),
            expected.text,
            "existing node text changed"
        );
        assert_eq!(
            actual.processing_instruction_target().map(str::to_owned),
            expected.target,
            "existing PI target changed"
        );
        assert_eq!(
            observed_attributes(actual),
            expected.attributes,
            "existing attributes changed"
        );
        assert_eq!(
            actual.range(),
            expected.range,
            "existing provenance changed"
        );
        assert_eq!(
            actual.parent().map(|node| node.id()),
            expected.parent,
            "existing parent changed"
        );
        if expected.id != appended_parent {
            assert_eq!(
                actual.first_child().map(|node| node.id()),
                expected.first_child,
                "existing first child changed"
            );
        }
    }
}

fn create_case(input: &[u8], selector: u8) {
    let local = make_local(input);
    match selector {
        0 => {
            let expected = ModelElement {
                name: ModelName::new(Some("xml"), &local, Some(XML)),
                attributes: Vec::new(),
            };
            let document = OwnedDocument::create(
                ElementSpec {
                    name: spec_name(&local, Some(XML), PrefixPolicy::Automatic),
                    items: &[],
                },
                EditLimits::default(),
            )
            .expect("Automatic XML namespace must use the implicit xml binding");
            check_created(&document, document.root_element().id(), &expected);
        }
        1 | 2 => {
            let prefix = if selector == 1 { None } else { Some("p") };
            let expected = located(
                EditErrorKind::Namespace,
                element_location(EditComponent::Prefix, None),
            );
            let error = OwnedDocument::create(
                ElementSpec {
                    name: spec_name(&local, Some(XML), exact(prefix)),
                    items: &[],
                },
                EditLimits::default(),
            )
            .expect_err("XML namespace must reject an incorrect exact prefix");
            assert_error_with_display(error, expected);
        }
        3 => {
            let expected = located(
                EditErrorKind::Namespace,
                element_location(EditComponent::NamespaceUri, None),
            );
            let error = OwnedDocument::create(
                ElementSpec {
                    name: spec_name(&local, Some("urn:not-xml"), exact(Some("xml"))),
                    items: &[],
                },
                EditLimits::default(),
            )
            .expect_err("xml prefix must reject a non-XML namespace");
            assert_error_with_display(error, expected);
        }
        _ => {
            let expected = ModelElement {
                name: ModelName::new(None, &local, Some("urn:root")),
                attributes: vec![declaration(None, "urn:root")],
            };
            let document = OwnedDocument::create(
                ElementSpec {
                    name: spec_name(&local, Some("urn:root"), exact(None)),
                    items: &[],
                },
                EditLimits::default(),
            )
            .expect("exact unprefixed namespaced root should bind the default namespace");
            check_created(&document, document.root_element().id(), &expected);
        }
    }
}

fn parsed_parent(mode: u8, source: &str) -> OwnedDocument {
    let mut bytes = encode(source, mode);
    let document = OwnedDocument::parse_bytes(&bytes).expect("fixed namespace fixture parses");
    bytes.fill(0);
    drop(bytes);
    document
}

fn append_case(input: &[u8], selector: u8, mode: u8) {
    let local = make_local(input);
    let source = match selector {
        4 => "<r xmlns='urn:default' xmlns:p='urn:p'><old/></r>",
        5 => "<r xmlns='urn:default'><old/></r>",
        6 => "<r xmlns='urn:old'><old/></r>",
        7 => "<r xmlns='urn:old' xmlns:p='urn:old'><old/></r>",
        8 => "<r xmlns:p='urn:old' xmlns:q='urn:old' xmlns:ns1='urn:occupied'><old/></r>",
        9 => "<r xmlns:p='urn:old'><old/></r>",
        10 => {
            "<r xmlns:p='urn:old' xmlns:q='urn:old' plain='root'><m xmlns:p='urn:new' plain='middle'><n a='one' b='two'/></m></r>"
        }
        _ => "<r xmlns='urn:old' xmlns:p='urn:old' keep='v'><old a='a' b='b'/></r>",
    };
    let mut document = parsed_parent(mode, source);
    let before = snapshot(&document);
    let root = document.root_element().id();
    let parent = if selector == 10 {
        let middle = document
            .node(root)
            .unwrap()
            .children()
            .next()
            .expect("middle fixture child")
            .id();
        document
            .node(middle)
            .unwrap()
            .children()
            .next()
            .expect("nested fixture child")
            .id()
    } else {
        root
    };

    match selector {
        4 => {
            let expected = ModelElement {
                name: ModelName::new(None, &local, None),
                attributes: vec![
                    attribute(Some("p"), "a", Some("urn:p"), "v"),
                    declaration(None, ""),
                ],
            };
            let items = [ElementItem::Attribute(AttributeSpec {
                name: spec_name("a", Some("urn:p"), PrefixPolicy::Automatic),
                value: "v",
            })];
            let child = document
                .edit(EditLimits::default())
                .unwrap()
                .append_element(
                    parent,
                    ElementSpec {
                        name: spec_name(&local, None, exact(None)),
                        items: &items,
                    },
                )
                .expect("automatic attribute should reuse an inherited matching prefix");
            check_created(&document, child, &expected);
            assert_existing_nodes_preserved(&document, &before, parent);
        }
        5 => {
            let expected = ModelElement {
                name: ModelName::new(None, &local, Some("urn:default")),
                attributes: Vec::new(),
            };
            let child = document
                .edit(EditLimits::default())
                .unwrap()
                .append_element(
                    parent,
                    ElementSpec {
                        name: spec_name(&local, Some("urn:default"), PrefixPolicy::Automatic),
                        items: &[],
                    },
                )
                .expect("automatic element should reuse inherited default binding");
            check_created(&document, child, &expected);
            assert_existing_nodes_preserved(&document, &before, parent);
        }
        6 => {
            let expected = ModelElement {
                name: ModelName::new(None, &local, None),
                attributes: vec![declaration(None, "")],
            };
            let child = document
                .edit(EditLimits::default())
                .unwrap()
                .append_element(
                    parent,
                    ElementSpec {
                        name: spec_name(&local, None, PrefixPolicy::Automatic),
                        items: &[],
                    },
                )
                .expect("no-namespace element should undeclare inherited default");
            check_created(&document, child, &expected);
            assert_existing_nodes_preserved(&document, &before, parent);
        }
        7 => {
            let expected = ModelElement {
                name: ModelName::new(Some("p"), &local, Some("urn:old")),
                attributes: vec![declaration(None, "urn:local-default")],
            };
            let items = [ElementItem::Namespace(NamespaceSpec {
                prefix: None,
                uri: "urn:local-default",
            })];
            let child = document
                .edit(EditLimits::default())
                .unwrap()
                .append_element(
                    parent,
                    ElementSpec {
                        name: spec_name(&local, Some("urn:old"), PrefixPolicy::Automatic),
                        items: &items,
                    },
                )
                .expect("automatic element should use inherited non-default matching prefix");
            check_created(&document, child, &expected);
            assert_existing_nodes_preserved(&document, &before, parent);
        }
        8 => {
            let expected = ModelElement {
                name: ModelName::new(None, &local, None),
                attributes: vec![
                    attribute(Some("ns2"), "auto", Some("urn:new"), "a"),
                    attribute(Some("ns1"), "fixed", Some("urn:fixed"), "b"),
                    declaration(Some("ns2"), "urn:new"),
                    declaration(Some("ns1"), "urn:fixed"),
                ],
            };
            let items = [
                ElementItem::Attribute(AttributeSpec {
                    name: spec_name("auto", Some("urn:new"), PrefixPolicy::Automatic),
                    value: "a",
                }),
                ElementItem::Attribute(AttributeSpec {
                    name: spec_name("fixed", Some("urn:fixed"), exact(Some("ns1"))),
                    value: "b",
                }),
            ];
            let child = document
                .edit(EditLimits::default())
                .unwrap()
                .append_element(
                    parent,
                    ElementSpec {
                        name: spec_name(&local, None, exact(None)),
                        items: &items,
                    },
                )
                .expect("generated prefix should skip inherited ns1 and reserve exact ns1");
            check_created(&document, child, &expected);
            assert_existing_nodes_preserved(&document, &before, parent);
        }
        9 => {
            let expected = ModelElement {
                name: ModelName::new(None, &local, None),
                attributes: vec![
                    attribute(Some("ns1"), "first", Some("urn:old"), "1"),
                    attribute(Some("ns1"), "reused", Some("urn:old"), "again"),
                    attribute(Some("p"), "second", Some("urn:new"), "2"),
                    declaration(Some("ns1"), "urn:old"),
                    declaration(Some("p"), "urn:new"),
                ],
            };
            let items = [
                ElementItem::Attribute(AttributeSpec {
                    name: spec_name("first", Some("urn:old"), PrefixPolicy::Automatic),
                    value: "1",
                }),
                ElementItem::Attribute(AttributeSpec {
                    name: spec_name("reused", Some("urn:old"), PrefixPolicy::Automatic),
                    value: "again",
                }),
                ElementItem::Attribute(AttributeSpec {
                    name: spec_name("second", Some("urn:new"), exact(Some("p"))),
                    value: "2",
                }),
            ];
            let child = document
                .edit(EditLimits::default())
                .unwrap()
                .append_element(
                    parent,
                    ElementSpec {
                        name: spec_name(&local, None, exact(None)),
                        items: &items,
                    },
                )
                .expect("automatic prefix must not be retargeted by a later Exact intent");
            check_created(&document, child, &expected);
            assert_existing_nodes_preserved(&document, &before, parent);
        }
        10 => {
            // The root binds p to old; the nearer element shadows p with new,
            // while q remains an inherited usable alias for old. Ordinary
            // attributes on both elements must not be mistaken for bindings.
            let items = [ElementItem::Attribute(AttributeSpec {
                name: spec_name("picked", Some("urn:old"), PrefixPolicy::Automatic),
                value: "yes",
            })];
            let expected = ModelElement {
                name: ModelName::new(None, &local, None),
                attributes: vec![attribute(Some("q"), "picked", Some("urn:old"), "yes")],
            };
            let child = document
                .edit(EditLimits::default())
                .unwrap()
                .append_element(
                    parent,
                    ElementSpec {
                        name: spec_name(&local, None, exact(None)),
                        items: &items,
                    },
                )
                .expect("shadowed p must resolve through inherited q");
            check_created(&document, child, &expected);
            assert_existing_nodes_preserved(&document, &before, parent);
        }
        14 => {
            let expected = ModelElement {
                name: ModelName::new(None, &local, None),
                attributes: vec![
                    declaration(Some("p"), "urn:local"),
                    attribute(Some("p"), "value", Some("urn:local"), "x"),
                    declaration(None, ""),
                ],
            };
            let items = [
                ElementItem::Namespace(NamespaceSpec {
                    prefix: Some("p"),
                    uri: "urn:local",
                }),
                ElementItem::Attribute(AttributeSpec {
                    name: spec_name("value", Some("urn:local"), PrefixPolicy::Automatic),
                    value: "x",
                }),
            ];
            let child = document
                .edit(EditLimits::default())
                .unwrap()
                .append_element(
                    parent,
                    ElementSpec {
                        name: spec_name(&local, None, exact(None)),
                        items: &items,
                    },
                )
                .expect("automatic prefix should reuse the matching local declaration");
            check_created(&document, child, &expected);
            assert_existing_nodes_preserved(&document, &before, parent);
        }
        15 => {
            let expected = located(EditErrorKind::Namespace, attribute_location(1, None, None));
            let recovery_name = ModelName::new(None, "recovered", None);
            let items = [
                ElementItem::Namespace(NamespaceSpec {
                    prefix: Some("p"),
                    uri: "urn:local",
                }),
                ElementItem::Attribute(AttributeSpec {
                    name: spec_name("value", Some("urn:other"), exact(Some("p"))),
                    value: "x",
                }),
            ];
            let mut editor = document.edit(EditLimits::default()).unwrap();
            let error = editor
                .append_element(
                    parent,
                    ElementSpec {
                        name: spec_name(&local, None, exact(None)),
                        items: &items,
                    },
                )
                .expect_err("Exact prefix must reject a conflicting local declaration");
            assert_error_with_display(error, expected);
            assert_eq!(
                &snapshot(editor.document()),
                &before,
                "local prefix conflict mutated tree"
            );
            let recovery = editor
                .append_element(
                    parent,
                    ElementSpec {
                        name: spec_name("recovered", None, exact(None)),
                        items: &[],
                    },
                )
                .expect("editor should recover after local prefix conflict");
            assert_eq!(
                observed_name(editor.document().node(recovery).unwrap().name().unwrap()),
                recovery_name
            );
            assert_existing_nodes_preserved(editor.document(), &before, parent);
        }
        16 => {
            let expected = located(EditErrorKind::Namespace, attribute_location(1, None, None));
            let recovery_name = ModelName::new(None, "recovered", None);
            let items = [
                ElementItem::Attribute(AttributeSpec {
                    name: spec_name("first", Some("urn:left"), exact(Some("p"))),
                    value: "L",
                }),
                ElementItem::Attribute(AttributeSpec {
                    name: spec_name("second", Some("urn:right"), exact(Some("p"))),
                    value: "R",
                }),
            ];
            let mut editor = document.edit(EditLimits::default()).unwrap();
            let error = editor
                .append_element(
                    parent,
                    ElementSpec {
                        name: spec_name(&local, None, exact(None)),
                        items: &items,
                    },
                )
                .expect_err("a generated exact binding must reject later conflicting URI");
            assert_error_with_display(error, expected);
            assert_eq!(
                &snapshot(editor.document()),
                &before,
                "generated prefix conflict mutated tree"
            );
            let recovery = editor
                .append_element(
                    parent,
                    ElementSpec {
                        name: spec_name("recovered", None, exact(None)),
                        items: &[],
                    },
                )
                .expect("editor should recover after generated prefix conflict");
            assert_eq!(
                observed_name(editor.document().node(recovery).unwrap().name().unwrap()),
                recovery_name
            );
            assert_existing_nodes_preserved(editor.document(), &before, parent);
        }
        12 => {
            let expected = located(
                EditErrorKind::Namespace,
                element_location(EditComponent::NamespaceUri, None),
            );
            let recovery_name = ModelName::new(None, "recovered", None);
            let items = [ElementItem::Namespace(NamespaceSpec {
                prefix: None,
                uri: "urn:local-default",
            })];
            let mut editor = document.edit(EditLimits::default()).unwrap();
            let error = editor
                .append_element(
                    parent,
                    ElementSpec {
                        name: spec_name(&local, None, PrefixPolicy::Automatic),
                        items: &items,
                    },
                )
                .expect_err("explicit default binding must conflict with Automatic no namespace");
            assert_error_with_display(error, expected);
            assert_eq!(
                &snapshot(editor.document()),
                &before,
                "namespace conflict mutated tree"
            );
            let recovery = editor
                .append_element(
                    parent,
                    ElementSpec {
                        name: spec_name("recovered", None, exact(None)),
                        items: &[],
                    },
                )
                .expect("editor should recover after explicit-default conflict");
            assert_eq!(
                observed_name(editor.document().node(recovery).unwrap().name().unwrap()),
                recovery_name
            );
            assert_existing_nodes_preserved(editor.document(), &before, parent);
            assert_serializable(editor.document());
        }
        _ => duplicate_and_alias_cases(mode, &mut document, &local, &before),
    }
}

fn duplicate_and_alias_cases(
    mode: u8,
    document: &mut OwnedDocument,
    local: &str,
    before: &Snapshot,
) {
    // Two different prefixes do not make duplicate expanded attribute names
    // distinct. Reject atomically, then reuse the same Editor successfully.
    let duplicate_items = [
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("same", Some("urn:dup"), exact(Some("p"))),
            value: "one",
        }),
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("same", Some("urn:dup"), exact(Some("q"))),
            value: "two",
        }),
    ];
    let duplicate_error = located(
        EditErrorKind::DuplicateAttribute,
        attribute_location(1, None, None),
    );
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let root_id = editor.document().root_element().id();
    let error = editor
        .append_element(
            root_id,
            ElementSpec {
                name: spec_name(local, None, exact(None)),
                items: &duplicate_items,
            },
        )
        .expect_err("duplicate expanded names with lexical aliases must fail");
    assert_error_with_display(error, duplicate_error);
    assert_eq!(
        &snapshot(editor.document()),
        before,
        "duplicate changed tree"
    );
    let recovery_name = ModelName::new(None, "recovered", None);
    let recovery_parent = editor.document().root_element().id();
    let recovery = editor
        .append_element(
            recovery_parent,
            ElementSpec {
                name: spec_name("recovered", None, exact(None)),
                items: &[],
            },
        )
        .expect("editor should recover after duplicate-name rejection");
    assert_eq!(
        observed_name(editor.document().node(recovery).unwrap().name().unwrap()),
        recovery_name
    );
    assert_existing_nodes_preserved(editor.document(), before, recovery_parent);

    // Equal local names in distinct namespaces are not duplicate expanded
    // names, even when both are explicitly prefixed on the new element.
    let aliases = [
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("same", Some("urn:left"), exact(Some("l"))),
            value: "L",
        }),
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("same", Some("urn:right"), exact(Some("r"))),
            value: "R",
        }),
    ];
    let alias_expected = ModelElement {
        name: ModelName::new(None, local, None),
        attributes: vec![
            attribute(Some("l"), "same", Some("urn:left"), "L"),
            attribute(Some("r"), "same", Some("urn:right"), "R"),
            declaration(Some("l"), "urn:left"),
            declaration(Some("r"), "urn:right"),
        ],
    };
    let mut bytes = encode("<r/>", mode);
    let mut fresh = OwnedDocument::parse_bytes(&bytes).unwrap();
    bytes.fill(0);
    let fresh_before = snapshot(&fresh);
    let fresh_root = fresh.root_element().id();
    let child = fresh
        .edit(EditLimits::default())
        .unwrap()
        .append_element(
            fresh_root,
            ElementSpec {
                name: spec_name(local, None, exact(None)),
                items: &aliases,
            },
        )
        .expect("different expanded attribute names should remain distinct");
    assert_element(&fresh, child, &alias_expected);
    assert_existing_nodes_preserved(&fresh, &fresh_before, fresh_root);
    assert_serializable(&fresh);
}

pub(super) fn exercise(input: &[u8]) {
    assert!(input.len() <= super::super::MAX_INPUT_BYTES);
    let selector = input.get(6).copied().unwrap_or(0) % CASE_COUNT;
    if selector <= 3 {
        create_case(input, selector);
    } else if selector == 13 {
        create_case(input, 4);
    } else {
        append_case(input, selector, input.get(7).copied().unwrap_or(0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finite_namespace_catalog_matches_literal_expectations_in_utf8_and_utf16() {
        for selector in 0..CASE_COUNT {
            let mut input = [0, 1, 2, 3, 4, 5, selector, 0, 4, 5, 6, 7, 0xff];
            if selector <= 3 || selector == 13 {
                exercise(&input);
            } else {
                for mode in 0..3 {
                    input[7] = mode;
                    exercise(&input);
                }
            }
        }
    }

    #[test]
    fn later_exact_prefix_intent_does_not_retarget_earlier_automatic_names() {
        let input = [0, 1, 2, 3, 4, 5, 9, 0, 6, 7, 8, 9, 10];
        append_case(&input, 9, 0);
    }

    #[test]
    #[should_panic(expected = "expanded namespace changed")]
    fn literal_expanded_name_oracle_rejects_a_mutated_namespace_expectation() {
        assert_name(
            &ModelName::new(None, "root", Some("urn:wrong")),
            &ModelName::new(None, "root", Some("urn:expected")),
        );
    }
}
