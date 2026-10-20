//! Independent, bounded construction model.
//!
//! The oracle keeps expanded names separate from lexical prefixes and predicts
//! each result before invoking the owned-tree API. It uses a finite namespace
//! catalog rather than reproducing the implementation's prefix planner.

use std::ops::Range;

use yaxml::{
    AttributeSpec, EditComponent, EditError, EditErrorKind, EditLimit, EditLimits, EditLocation,
    ElementItem, ElementSpec, Name, NameSpec, NamespaceSpec, OwnedDocument, OwnedNodeId,
    OwnedNodeKind, PrefixPolicy, Writer,
};

#[path = "construction_validation.rs"]
mod validation;

#[path = "construction_limits.rs"]
mod limits;

#[path = "construction_namespaces.rs"]
mod namespaces;

const XML: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
const MAX_DERIVED_BYTES: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
struct ModelName {
    qualified: String,
    prefix: Option<String>,
    local: String,
    namespace: Option<String>,
}

impl ModelName {
    fn new(prefix: Option<&str>, local: &str, namespace: Option<&str>) -> Self {
        let qualified =
            prefix.map_or_else(|| local.to_owned(), |prefix| format!("{prefix}:{local}"));
        Self {
            qualified,
            prefix: prefix.map(str::to_owned),
            local: local.to_owned(),
            namespace: namespace.map(str::to_owned),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ModelAttribute {
    name: ModelName,
    value: String,
}

fn attribute(
    prefix: Option<&str>,
    local: &str,
    namespace: Option<&str>,
    value: &str,
) -> ModelAttribute {
    ModelAttribute {
        name: ModelName::new(prefix, local, namespace),
        value: value.to_owned(),
    }
}

fn declaration(prefix: Option<&str>, uri: &str) -> ModelAttribute {
    match prefix {
        Some(prefix) => attribute(Some("xmlns"), prefix, Some(XMLNS), uri),
        None => attribute(None, "xmlns", Some(XMLNS), uri),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ModelElement {
    name: ModelName,
    attributes: Vec<ModelAttribute>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ObservedNode {
    id: OwnedNodeId,
    kind: OwnedNodeKind,
    name: Option<ModelName>,
    text: Option<String>,
    target: Option<String>,
    attributes: Vec<ModelAttribute>,
    range: Option<Range<usize>>,
    parent: Option<OwnedNodeId>,
    first_child: Option<OwnedNodeId>,
    last_child: Option<OwnedNodeId>,
    next_sibling: Option<OwnedNodeId>,
    children: Vec<OwnedNodeId>,
    descendants: Vec<OwnedNodeId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    declaration: Option<(String, Option<String>, Option<bool>)>,
    root: OwnedNodeId,
    root_element: OwnedNodeId,
    nodes: Vec<ObservedNode>,
}

fn observed_name(name: Name<'_>) -> ModelName {
    ModelName {
        qualified: name.qualified().to_owned(),
        prefix: name.prefix().map(str::to_owned),
        local: name.local().to_owned(),
        namespace: name.namespace().map(str::to_owned),
    }
}

fn observed_attributes(node: yaxml::OwnedNode<'_>) -> Vec<ModelAttribute> {
    node.attributes()
        .map(|attribute| ModelAttribute {
            name: observed_name(attribute.name),
            value: attribute.value.into_owned(),
        })
        .collect()
}

fn snapshot(document: &OwnedDocument) -> Snapshot {
    Snapshot {
        declaration: document.declaration().map(|declaration| {
            (
                declaration.version.to_owned(),
                declaration.encoding.map(str::to_owned),
                declaration.standalone,
            )
        }),
        root: document.root().id(),
        root_element: document.root_element().id(),
        nodes: document
            .nodes()
            .map(|node| ObservedNode {
                id: node.id(),
                kind: node.kind(),
                name: node.name().map(observed_name),
                text: node.text().map(str::to_owned),
                target: node.processing_instruction_target().map(str::to_owned),
                attributes: observed_attributes(node),
                range: node.range(),
                parent: node.parent().map(|parent| parent.id()),
                first_child: node.first_child().map(|child| child.id()),
                last_child: node.last_child().map(|child| child.id()),
                next_sibling: node.next_sibling().map(|sibling| sibling.id()),
                children: node.children().map(|child| child.id()).collect(),
                descendants: node
                    .descendants()
                    .map(|descendant| descendant.id())
                    .collect(),
            })
            .collect(),
    }
}

fn assert_name(actual: &ModelName, expected: &ModelName) {
    assert_eq!(
        actual.qualified, expected.qualified,
        "qualified name changed"
    );
    assert_eq!(actual.prefix, expected.prefix, "prefix spelling changed");
    assert_eq!(actual.local, expected.local, "local name changed");
    assert_eq!(
        actual.namespace, expected.namespace,
        "expanded namespace changed"
    );
}

fn assert_attributes(actual: &[ModelAttribute], expected: &[ModelAttribute]) {
    assert_eq!(actual.len(), expected.len(), "attribute membership changed");
    for (position, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(
            actual, expected,
            "attribute order/value changed at {position}"
        );
    }
}

fn assert_element(document: &OwnedDocument, id: OwnedNodeId, expected: &ModelElement) {
    let node = document.node(id).expect("constructed handle is not live");
    assert_eq!(node.kind(), OwnedNodeKind::Element);
    let actual = observed_name(node.name().expect("constructed element lost its name"));
    assert_name(&actual, &expected.name);
    assert_attributes(&observed_attributes(node), &expected.attributes);
    assert_eq!(node.text(), None);
    assert_eq!(node.processing_instruction_target(), None);
    assert_eq!(
        node.range(),
        None,
        "constructed element gained source provenance"
    );
}

fn assert_error(actual: EditError, expected: EditError) {
    assert_eq!(actual, expected, "construction error contract changed");
}

fn assert_error_with_display(actual: EditError, expected: EditError) {
    assert_error(actual, expected);
    super::check_display(&actual);
}

fn sparse_format_and_debug_contract(input: &[u8]) {
    if input.first().copied().unwrap_or(0) & 0x3f != 0x2a {
        return;
    }

    let local = derived_name(input);
    let value = derived_value(input);
    let duplicate_items = [
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("same", Some("urn:same"), exact(Some("p"))),
            value: &value,
        }),
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("same", Some("urn:same"), exact(Some("q"))),
            value: &value,
        }),
    ];
    let spec = ElementSpec {
        name: spec_name(&local, None, exact(None)),
        items: &duplicate_items,
    };
    let actual = OwnedDocument::create(spec, EditLimits::default())
        .expect_err("duplicate expanded attribute was accepted");
    assert_error_with_display(
        actual,
        located(
            EditErrorKind::DuplicateAttribute,
            attribute_location(1, None, None),
        ),
    );
    assert_eq!(format!("{:?}", EditErrorKind::Namespace), "Namespace");
    assert_eq!(
        format!("{:?}", EditErrorKind::DuplicateAttribute),
        "DuplicateAttribute"
    );
    assert_eq!(
        format!("{:?}", EditErrorKind::IdentifierExhausted),
        "IdentifierExhausted"
    );
    assert_eq!(format!("{:?}", PrefixPolicy::Automatic), "Automatic");
    assert_eq!(
        format!("{:?}", PrefixPolicy::Exact(Some("p"))),
        "Exact(Some(\"p\"))"
    );

    let name = spec_name(&local, Some("urn:model"), PrefixPolicy::Automatic);
    let attribute = AttributeSpec {
        name,
        value: &value,
    };
    let namespace = NamespaceSpec {
        prefix: Some("p"),
        uri: "urn:model",
    };
    let attribute_debug = format!("{attribute:?}");
    assert!(attribute_debug.contains(&local));
    assert!(attribute_debug.contains(&format!("{value:?}")));
    let namespace_debug = format!("{namespace:?}");
    assert!(namespace_debug.contains("urn:model"));
    let name_debug = format!("{name:?}");
    assert!(name_debug.contains(&local));
    assert!(name_debug.contains("Automatic"));
    assert!(format!("{:?}", ElementItem::Attribute(attribute)).contains("Attribute"));
    assert!(format!("{:?}", ElementItem::Namespace(namespace)).contains("Namespace"));
    let items = [
        ElementItem::Namespace(namespace),
        ElementItem::Attribute(attribute),
    ];
    let element_debug = format!(
        "{:?}",
        ElementSpec {
            name,
            items: &items,
        }
    );
    assert!(element_debug.contains(&local));
    assert!(element_debug.contains("urn:model"));
    assert!(element_debug.contains(&format!("{value:?}")));
}

fn located(kind: EditErrorKind, location: EditLocation) -> EditError {
    EditError {
        kind,
        value_offset: None,
        location: Some(location),
    }
}

fn global(kind: EditErrorKind) -> EditError {
    EditError {
        kind,
        value_offset: None,
        location: None,
    }
}

fn element_location(component: EditComponent, byte_offset: Option<usize>) -> EditLocation {
    EditLocation::ElementName {
        component,
        byte_offset,
    }
}

fn attribute_location(
    index: usize,
    component: Option<EditComponent>,
    byte_offset: Option<usize>,
) -> EditLocation {
    EditLocation::AttributeItem {
        index,
        component,
        byte_offset,
    }
}

fn exact<'a>(prefix: Option<&'a str>) -> PrefixPolicy<'a> {
    PrefixPolicy::Exact(prefix)
}

fn spec_name<'a>(
    local: &'a str,
    namespace: Option<&'a str>,
    prefix: PrefixPolicy<'a>,
) -> NameSpec<'a> {
    NameSpec {
        local,
        namespace,
        prefix,
    }
}

fn derived_name(input: &[u8]) -> String {
    const PARTS: [&str; 8] = ["a", "b", "é", "中", "λ", "名", "x", "_"];
    let mut name = String::from("n");
    for byte in input.iter().take(MAX_DERIVED_BYTES) {
        name.push_str(PARTS[usize::from(byte & 7)]);
    }
    name
}

fn derived_value(input: &[u8]) -> String {
    const VALUES: [&str; 8] = ["a", "&", "<", "\"", "é", "🙂", "\t", "\n"];
    input
        .iter()
        .take(MAX_DERIVED_BYTES)
        .map(|byte| VALUES[usize::from(byte & 7)])
        .collect()
}

fn encode(source: &str, mode: u8) -> Vec<u8> {
    match mode % 3 {
        0 => source.as_bytes().to_vec(),
        mode => std::iter::once(0xfeff)
            .chain(source.encode_utf16())
            .flat_map(|unit| {
                if mode == 1 {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                }
            })
            .collect(),
    }
}

fn assert_serializable(document: &OwnedDocument) {
    let mut writer = Writer::new(Vec::new());
    writer.write_owned_document(document).unwrap();
    writer.finish().unwrap();
    let bytes = writer.into_inner();
    assert!(yaxml::Document::parse_bytes(&bytes).is_ok());
}

fn root_catalog(input: &[u8], selector: u8) {
    let local = derived_name(input);
    let value = derived_value(input);
    let automatic_uri = format!("urn:auto:{local}");
    let expected;
    let document = match selector % 6 {
        0 => {
            let items = [
                ElementItem::Attribute(AttributeSpec {
                    name: spec_name("id", None, exact(None)),
                    value: &value,
                }),
                ElementItem::Attribute(AttributeSpec {
                    name: spec_name("repeat", None, exact(None)),
                    value: &value,
                }),
            ];
            expected = ModelElement {
                name: ModelName::new(None, &local, None),
                attributes: vec![
                    attribute(None, "id", None, &value),
                    attribute(None, "repeat", None, &value),
                ],
            };
            let prediction = expected.clone();
            let result = OwnedDocument::create(
                ElementSpec {
                    name: spec_name(&local, None, exact(None)),
                    items: &items,
                },
                EditLimits::default(),
            );
            let document = result.expect("valid unnamespaced root was rejected");
            assert_element(&document, document.root_element().id(), &prediction);
            document
        }
        1 => {
            let items = [
                ElementItem::Namespace(NamespaceSpec {
                    prefix: Some("p"),
                    uri: "urn:p",
                }),
                ElementItem::Attribute(AttributeSpec {
                    name: spec_name("id", None, exact(None)),
                    value: &value,
                }),
                ElementItem::Attribute(AttributeSpec {
                    name: spec_name("kind", Some("urn:p"), exact(Some("p"))),
                    value: "P",
                }),
            ];
            expected = ModelElement {
                name: ModelName::new(Some("p"), &local, Some("urn:p")),
                attributes: vec![
                    declaration(Some("p"), "urn:p"),
                    attribute(None, "id", None, &value),
                    attribute(Some("p"), "kind", Some("urn:p"), "P"),
                ],
            };
            OwnedDocument::create(
                ElementSpec {
                    name: spec_name(&local, Some("urn:p"), exact(Some("p"))),
                    items: &items,
                },
                EditLimits::default(),
            )
            .expect("valid explicitly prefixed root was rejected")
        }
        2 => {
            let items = [ElementItem::Attribute(AttributeSpec {
                name: spec_name("id", None, exact(None)),
                value: &value,
            })];
            expected = ModelElement {
                name: ModelName::new(None, &local, Some(&automatic_uri)),
                attributes: vec![
                    attribute(None, "id", None, &value),
                    declaration(None, &automatic_uri),
                ],
            };
            OwnedDocument::create(
                ElementSpec {
                    name: spec_name(&local, Some(&automatic_uri), PrefixPolicy::Automatic),
                    items: &items,
                },
                EditLimits::default(),
            )
            .expect("valid automatic default namespace was rejected")
        }
        3 => {
            expected = ModelElement {
                name: ModelName::new(Some("xml"), &local, Some(XML)),
                attributes: Vec::new(),
            };
            OwnedDocument::create(
                ElementSpec {
                    name: spec_name(&local, Some(XML), PrefixPolicy::Automatic),
                    items: &[],
                },
                EditLimits::default(),
            )
            .expect("implicit XML binding was rejected")
        }
        4 => {
            let items = [
                ElementItem::Attribute(AttributeSpec {
                    name: spec_name("first", Some("urn:first"), PrefixPolicy::Automatic),
                    value: "A",
                }),
                ElementItem::Attribute(AttributeSpec {
                    name: spec_name("second", Some("urn:second"), exact(Some("ns1"))),
                    value: "B",
                }),
            ];
            expected = ModelElement {
                name: ModelName::new(None, &local, None),
                attributes: vec![
                    attribute(Some("ns2"), "first", Some("urn:first"), "A"),
                    attribute(Some("ns1"), "second", Some("urn:second"), "B"),
                    declaration(Some("ns2"), "urn:first"),
                    declaration(Some("ns1"), "urn:second"),
                ],
            };
            OwnedDocument::create(
                ElementSpec {
                    name: spec_name(&local, None, exact(None)),
                    items: &items,
                },
                EditLimits::default(),
            )
            .expect("later exact prefix was not reserved")
        }
        _ => {
            let items = [
                ElementItem::Attribute(AttributeSpec {
                    name: spec_name("kind", Some("urn:q"), exact(Some("q"))),
                    value: &value,
                }),
                ElementItem::Namespace(NamespaceSpec {
                    prefix: Some("q"),
                    uri: "urn:q",
                }),
            ];
            expected = ModelElement {
                name: ModelName::new(None, &local, None),
                attributes: vec![
                    attribute(Some("q"), "kind", Some("urn:q"), &value),
                    declaration(Some("q"), "urn:q"),
                ],
            };
            OwnedDocument::create(
                ElementSpec {
                    name: spec_name(&local, None, exact(None)),
                    items: &items,
                },
                EditLimits::default(),
            )
            .expect("later explicit declaration was not gathered")
        }
    };
    assert_eq!(document.declaration(), None);
    assert_eq!(document.root().children().count(), 1);
    assert_element(&document, document.root_element().id(), &expected);
    assert_serializable(&document);
}

fn simple_document(local: &str, value: &str) -> (OwnedDocument, ModelElement) {
    let items = [ElementItem::Attribute(AttributeSpec {
        name: spec_name("seed", None, exact(None)),
        value,
    })];
    let expected = ModelElement {
        name: ModelName::new(None, local, None),
        attributes: vec![attribute(None, "seed", None, value)],
    };
    let prediction = expected.clone();
    let document = OwnedDocument::create(
        ElementSpec {
            name: spec_name(local, None, exact(None)),
            items: &items,
        },
        EditLimits::default(),
    )
    .expect("valid model root was rejected");
    assert_element(&document, document.root_element().id(), &prediction);
    (document, expected)
}

fn construction_interplay(input: &[u8]) {
    let local = derived_name(input);
    let initial = derived_value(input);
    let (mut document, root_model) = simple_document(&local, &initial);
    let root = document.root_element().id();
    let child_local = format!("c{local}");
    let child_value = format!("child-{initial}");
    let child_items = [
        ElementItem::Namespace(NamespaceSpec {
            prefix: Some("p"),
            uri: "urn:child",
        }),
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("key", Some("urn:child"), exact(Some("p"))),
            value: &child_value,
        }),
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("plain", None, exact(None)),
            value: &initial,
        }),
    ];
    let mut child_model = ModelElement {
        name: ModelName::new(Some("p"), &child_local, Some("urn:child")),
        attributes: vec![
            declaration(Some("p"), "urn:child"),
            attribute(Some("p"), "key", Some("urn:child"), &child_value),
            attribute(None, "plain", None, &initial),
        ],
    };
    let predicted = child_model.clone();
    {
        let mut editor = document.edit(EditLimits::default()).unwrap();
        let child = editor
            .append_element(
                root,
                ElementSpec {
                    name: spec_name(&child_local, Some("urn:child"), exact(Some("p"))),
                    items: &child_items,
                },
            )
            .expect("valid child construction was rejected");
        assert_element(editor.document(), child, &predicted);
        assert_element(editor.document(), root, &root_model);

        let replacement = format!("edited-{initial}");
        editor
            .set_attribute_value(child, Some("urn:child"), "key", &replacement)
            .unwrap();
        child_model.attributes[1].value.clone_from(&replacement);
        assert_element(editor.document(), child, &child_model);

        let text_value = format!("literal-{initial}");
        let text = editor.append_text(child, &text_value).unwrap();
        let text_node = editor.document().node(text).unwrap();
        assert_eq!(text_node.kind(), OwnedNodeKind::Text);
        assert_eq!(text_node.text(), Some(text_value.as_str()));
        assert_eq!(text_node.range(), None);
        assert_eq!(text_node.parent().map(|parent| parent.id()), Some(child));
        assert_eq!(
            editor
                .document()
                .node(child)
                .unwrap()
                .last_child()
                .map(|node| node.id()),
            Some(text)
        );
        editor.remove_subtree(text).unwrap();
        assert!(editor.document().node(text).is_none());
        assert!(
            editor
                .document()
                .node(child)
                .unwrap()
                .children()
                .next()
                .is_none()
        );
        assert_element(editor.document(), child, &child_model);
    }
    assert_serializable(&document);
}

fn parsed_parent(input: &[u8], mode: u8, selector: u8) {
    let local = derived_name(input);
    let source = "<r xmlns='urn:default' xmlns:q='urn:q' keep='old'><q:old a='v'/></r>";
    let mut bytes = encode(source, mode);
    let mut document = OwnedDocument::parse_bytes(&bytes).unwrap();
    bytes.fill(0);
    drop(bytes);
    let before = snapshot(&document);
    let root = document.root_element().id();
    let (items, name, expected) = match selector % 4 {
        0 => (
            Vec::new(),
            spec_name(&local, None, exact(None)),
            ModelElement {
                name: ModelName::new(None, &local, None),
                attributes: vec![declaration(None, "")],
            },
        ),
        1 => (
            Vec::new(),
            spec_name(&local, Some("urn:default"), PrefixPolicy::Automatic),
            ModelElement {
                name: ModelName::new(None, &local, Some("urn:default")),
                attributes: Vec::new(),
            },
        ),
        2 => (
            vec![ElementItem::Attribute(AttributeSpec {
                name: spec_name("kind", Some("urn:new"), exact(Some("p"))),
                value: "new",
            })],
            spec_name(&local, Some("urn:new"), exact(Some("p"))),
            ModelElement {
                name: ModelName::new(Some("p"), &local, Some("urn:new")),
                attributes: vec![
                    attribute(Some("p"), "kind", Some("urn:new"), "new"),
                    declaration(Some("p"), "urn:new"),
                ],
            },
        ),
        _ => (
            vec![ElementItem::Attribute(AttributeSpec {
                name: spec_name("kind", Some("urn:q"), PrefixPolicy::Automatic),
                value: "q",
            })],
            spec_name(&local, None, exact(None)),
            ModelElement {
                name: ModelName::new(None, &local, None),
                attributes: vec![
                    attribute(Some("q"), "kind", Some("urn:q"), "q"),
                    declaration(None, ""),
                ],
            },
        ),
    };
    let old_ids: Vec<_> = before.nodes.iter().map(|node| node.id).collect();
    let old_semantics: Vec<_> = before
        .nodes
        .iter()
        .map(|node| {
            (
                node.id,
                node.kind,
                node.name.clone(),
                node.text.clone(),
                node.target.clone(),
                node.attributes.clone(),
                node.range.clone(),
                node.parent,
            )
        })
        .collect();
    let predicted = expected.clone();
    let child = document
        .edit(EditLimits::default())
        .unwrap()
        .append_element(
            root,
            ElementSpec {
                name,
                items: &items,
            },
        )
        .expect("valid parsed-parent append was rejected");
    assert_element(&document, child, &predicted);
    assert_eq!(
        document
            .nodes()
            .take(old_ids.len())
            .map(|node| node.id())
            .collect::<Vec<_>>(),
        old_ids,
        "append reordered parsed survivors"
    );
    for (id, kind, name, text, target, attributes, range, parent) in old_semantics {
        let node = document
            .node(id)
            .expect("append invalidated a parsed survivor");
        assert_eq!(node.kind(), kind);
        assert_eq!(node.name().map(observed_name), name);
        assert_eq!(node.text().map(str::to_owned), text);
        assert_eq!(
            node.processing_instruction_target().map(str::to_owned),
            target
        );
        assert_eq!(observed_attributes(node), attributes);
        assert_eq!(node.range(), range);
        assert_eq!(node.parent().map(|value| value.id()), parent);
    }
    let root_node = document.node(root).unwrap();
    assert_eq!(
        root_node.first_child().map(|node| node.id()),
        Some(old_ids[2])
    );
    assert_eq!(root_node.last_child().map(|node| node.id()), Some(child));
    assert_serializable(&document);
}

fn rejection_catalog(input: &[u8], selector: u8) {
    let local = derived_name(input);
    // Keep this nonempty so the below-boundary value case is meaningful even
    // for the empty fuzz input.
    let value = format!("v{}", derived_value(input));
    let one_attribute = [ElementItem::Attribute(AttributeSpec {
        name: spec_name("a", None, exact(None)),
        value: &value,
    })];
    let base = ElementSpec {
        name: spec_name(&local, None, exact(None)),
        items: &one_attribute,
    };
    let duplicate_items = [
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("same", Some("urn:same"), exact(Some("p"))),
            value: "one",
        }),
        ElementItem::Attribute(AttributeSpec {
            name: spec_name("same", Some("urn:same"), exact(Some("q"))),
            value: "two",
        }),
    ];
    let defaults = EditLimits::default();
    let (spec, limits, expected) = match selector % 8 {
        0 => (
            base,
            {
                let mut cfg = defaults;
                cfg.max_name_bytes = local.len() - 1;
                cfg
            },
            located(
                EditErrorKind::LimitExceeded(EditLimit::NameBytes),
                element_location(EditComponent::LocalName, Some(local.len() - 1)),
            ),
        ),
        1 => (
            base,
            {
                let mut cfg = defaults;
                cfg.max_supplied_bytes = local.len() - 1;
                cfg
            },
            located(
                EditErrorKind::LimitExceeded(EditLimit::SuppliedBytes),
                element_location(EditComponent::LocalName, Some(local.len() - 1)),
            ),
        ),
        2 => (
            base,
            {
                let mut cfg = defaults;
                cfg.max_attributes = 0;
                cfg
            },
            global(EditErrorKind::LimitExceeded(EditLimit::Attributes)),
        ),
        3 => (
            base,
            {
                let mut cfg = defaults;
                cfg.max_depth = 0;
                cfg
            },
            global(EditErrorKind::LimitExceeded(EditLimit::Depth)),
        ),
        4 => (
            base,
            {
                let mut cfg = defaults;
                cfg.max_created_nodes = 0;
                cfg
            },
            global(EditErrorKind::LimitExceeded(EditLimit::CreatedNodes)),
        ),
        5 => (
            base,
            {
                let mut cfg = defaults;
                cfg.max_created_attributes = 0;
                cfg
            },
            global(EditErrorKind::LimitExceeded(EditLimit::CreatedAttributes)),
        ),
        6 => (
            base,
            {
                let mut cfg = defaults;
                cfg.max_value_bytes = value.len().saturating_sub(1);
                cfg
            },
            located(
                EditErrorKind::LimitExceeded(EditLimit::ValueBytes),
                attribute_location(
                    0,
                    Some(EditComponent::Value),
                    Some(value.len().saturating_sub(1)),
                ),
            ),
        ),
        _ => (
            ElementSpec {
                name: spec_name(&local, None, exact(None)),
                items: &duplicate_items,
            },
            defaults,
            located(
                EditErrorKind::DuplicateAttribute,
                attribute_location(1, None, None),
            ),
        ),
    };
    let prediction = expected;
    let actual =
        OwnedDocument::create(spec, limits).expect_err("invalid construction was accepted");
    assert_error(actual, prediction);
}

fn repeated_borrowed_strings(input: &[u8]) {
    let mut shared = derived_name(input);
    let expected_name = shared.clone();
    let expected = ModelElement {
        name: ModelName::new(None, &expected_name, None),
        attributes: vec![attribute(None, &expected_name, None, &expected_name)],
    };
    let supplied = shared.len() * 3;
    let (rejection, document) = {
        let items = [ElementItem::Attribute(AttributeSpec {
            name: spec_name(&shared, None, exact(None)),
            value: &shared,
        })];
        let rejection = OwnedDocument::create(
            ElementSpec {
                name: spec_name(&shared, None, exact(None)),
                items: &items,
            },
            {
                let mut cfg = EditLimits::default();
                cfg.max_supplied_bytes = supplied - 1;
                cfg
            },
        )
        .expect_err("repeated strings were counted by pointer identity");
        let document = OwnedDocument::create(
            ElementSpec {
                name: spec_name(&shared, None, exact(None)),
                items: &items,
            },
            {
                let mut cfg = EditLimits::default();
                cfg.max_supplied_bytes = supplied;
                cfg
            },
        )
        .expect("exact repeated-string supplied budget was rejected");
        (rejection, document)
    };
    assert_error(
        rejection,
        located(
            EditErrorKind::LimitExceeded(EditLimit::SuppliedBytes),
            attribute_location(0, Some(EditComponent::Value), Some(shared.len() - 1)),
        ),
    );
    shared.clear();
    shared.push_str("mutated-after-call");
    assert_element(&document, document.root_element().id(), &expected);
}

fn persistent_counters(input: &[u8]) {
    let local = derived_name(input);
    let (mut document, root_model) = simple_document(&local, "root");
    let root = document.root_element().id();
    let child_items = [ElementItem::Attribute(AttributeSpec {
        name: spec_name("a", None, exact(None)),
        value: "v",
    })];
    let before = snapshot(&document);
    let expected_depth = global(EditErrorKind::LimitExceeded(EditLimit::Depth));
    let actual = document
        .edit({
            let mut cfg = EditLimits::default();
            cfg.max_depth = 1;
            cfg
        })
        .unwrap()
        .append_element(
            root,
            ElementSpec {
                name: spec_name("child", None, exact(None)),
                items: &child_items,
            },
        )
        .expect_err("depth rejection was lost");
    assert_error(actual, expected_depth);
    assert_eq!(snapshot(&document), before, "failed append published state");

    let child = document
        .edit({
            let mut cfg = EditLimits::default();
            cfg.max_created_nodes = 2;
            cfg.max_created_attributes = 2;
            cfg
        })
        .unwrap()
        .append_element(
            root,
            ElementSpec {
                name: spec_name("child", None, exact(None)),
                items: &child_items,
            },
        )
        .unwrap();
    document
        .edit(EditLimits::default())
        .unwrap()
        .remove_subtree(child)
        .unwrap();
    assert!(document.node(child).is_none());
    assert_element(&document, root, &root_model);
    let stable = snapshot(&document);
    assert_error(
        document
            .edit({
                let mut cfg = EditLimits::default();
                cfg.max_created_nodes = 1;
                cfg
            })
            .err()
            .expect("removal refunded the created-node counter"),
        global(EditErrorKind::LimitExceeded(EditLimit::CreatedNodes)),
    );
    assert_error(
        document
            .edit({
                let mut cfg = EditLimits::default();
                cfg.max_created_nodes = 2;
                cfg.max_created_attributes = 1;
                cfg
            })
            .err()
            .expect("removal refunded the created-attribute counter"),
        global(EditErrorKind::LimitExceeded(EditLimit::CreatedAttributes)),
    );
    assert_eq!(snapshot(&document), stable);
}

fn handle_and_kind_rejections(input: &[u8]) {
    let local = derived_name(input);
    let (mut document, root_model) = simple_document(&local, "root");
    let root = document.root_element().id();
    let (foreign_document, _) = simple_document("foreign", "other");
    let foreign = foreign_document.root_element().id();
    let empty = ElementSpec {
        name: spec_name("child", None, exact(None)),
        items: &[],
    };

    let before_foreign = snapshot(&document);
    let expected_foreign = global(EditErrorKind::InvalidHandle);
    let actual_foreign = document
        .edit(EditLimits::default())
        .unwrap()
        .append_element(foreign, empty)
        .expect_err("foreign parent handle was accepted");
    assert_error(actual_foreign, expected_foreign);
    assert_eq!(snapshot(&document), before_foreign);

    let mut editor = document.edit(EditLimits::default()).unwrap();
    let text = editor.append_text(root, "literal").unwrap();
    let before_kind = snapshot(editor.document());
    let expected_kind = global(EditErrorKind::WrongNodeKind);
    let actual_kind = editor
        .append_element(text, empty)
        .expect_err("text node was accepted as an element parent");
    assert_error(actual_kind, expected_kind);
    assert_eq!(snapshot(editor.document()), before_kind);

    let child = editor.append_element(root, empty).unwrap();
    editor.remove_subtree(child).unwrap();
    let before_stale = snapshot(editor.document());
    let expected_stale = global(EditErrorKind::InvalidHandle);
    let actual_stale = editor
        .append_element(child, empty)
        .expect_err("stale parent handle was accepted");
    assert_error(actual_stale, expected_stale);
    assert_eq!(snapshot(editor.document()), before_stale);
    assert_element(editor.document(), root, &root_model);
}

fn long_inherited_prefix(input: &[u8]) {
    if input.first().copied().unwrap_or(0) & 0x3f != 0x2b {
        return;
    }

    let length_selector = input.get(1).copied().unwrap_or(0);
    let prefix_length = 256 + usize::from(length_selector.max(16)) * 16;
    debug_assert!(prefix_length <= 4_336);
    let prefix = "p".repeat(prefix_length);
    let source = format!("<r xmlns:{prefix}='urn:long'/>");
    let mut bytes = encode(&source, input.get(2).copied().unwrap_or(0));
    let mut document = OwnedDocument::parse_bytes(&bytes).unwrap();
    bytes.fill(0);
    drop(bytes);

    let parent = document.root_element().id();
    let value = match input.get(3).copied().unwrap_or(0) & 3 {
        0 => "v",
        1 => "é",
        2 => "<&",
        _ => "中",
    };
    let items = [ElementItem::Attribute(AttributeSpec {
        name: spec_name("a", Some("urn:long"), PrefixPolicy::Automatic),
        value,
    })];
    let child = ElementSpec {
        name: spec_name("c", None, exact(None)),
        items: &items,
    };

    let before = snapshot(&document);
    let rejection = document
        .edit({
            let mut cfg = EditLimits::default();
            cfg.max_name_bytes = prefix.len() - 1;
            cfg
        })
        .unwrap()
        .append_element(parent, child)
        .expect_err("an over-limit inherited prefix was selected");
    assert_error(
        rejection,
        located(
            EditErrorKind::LimitExceeded(EditLimit::NameBytes),
            attribute_location(0, Some(EditComponent::Prefix), Some(prefix.len() - 1)),
        ),
    );
    assert_eq!(
        snapshot(&document),
        before,
        "prefix rejection published state"
    );

    let inserted = document
        .edit(EditLimits::default())
        .unwrap()
        .append_element(parent, child)
        .expect("a bounded inherited prefix was rejected");
    assert_element(
        &document,
        inserted,
        &ModelElement {
            name: ModelName::new(None, "c", None),
            attributes: vec![attribute(Some(&prefix), "a", Some("urn:long"), value)],
        },
    );
    assert_eq!(
        document.node(inserted).unwrap().name().unwrap().qualified(),
        "c"
    );
    assert_serializable(&document);
}

pub(super) fn exercise(input: &[u8]) {
    assert!(input.len() <= super::MAX_INPUT_BYTES);
    let byte = |index| input.get(index).copied().unwrap_or(0);
    root_catalog(input, byte(0));
    construction_interplay(input);
    parsed_parent(input, byte(1), byte(2));
    rejection_catalog(input, byte(3));
    repeated_borrowed_strings(input);
    persistent_counters(input);
    handle_and_kind_rejections(input);
    sparse_format_and_debug_contract(input);
    long_inherited_prefix(input);
    validation::exercise(input);
    limits::exercise(input);
    namespaces::exercise(input);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "expanded namespace changed")]
    fn name_oracle_rejects_a_changed_expanded_namespace() {
        assert_name(
            &ModelName::new(Some("p"), "n", Some("urn:wrong")),
            &ModelName::new(Some("p"), "n", Some("urn:right")),
        );
    }

    #[test]
    #[should_panic(expected = "attribute order/value changed at 0")]
    fn attribute_oracle_rejects_reordered_items() {
        let expected = vec![
            attribute(None, "a", None, "1"),
            attribute(None, "b", None, "2"),
        ];
        let actual = vec![expected[1].clone(), expected[0].clone()];
        assert_attributes(&actual, &expected);
    }

    #[test]
    #[should_panic(expected = "construction error contract changed")]
    fn error_oracle_rejects_a_changed_quota_result() {
        assert_error(
            global(EditErrorKind::LimitExceeded(EditLimit::CreatedNodes)),
            global(EditErrorKind::LimitExceeded(EditLimit::CreatedAttributes)),
        );
    }

    #[test]
    fn construction_display_catalog_propagates_sink_failures() {
        let cases = [
            (
                located(
                    EditErrorKind::Namespace,
                    EditLocation::ElementName {
                        component: EditComponent::LocalName,
                        byte_offset: Some(3),
                    },
                ),
                "XML edit error Namespace at ElementName { component: LocalName, byte_offset: Some(3) }",
            ),
            (
                located(
                    EditErrorKind::Namespace,
                    EditLocation::NamespaceItem {
                        index: 2,
                        component: Some(EditComponent::NamespaceUri),
                        byte_offset: Some(5),
                    },
                ),
                "XML edit error Namespace at NamespaceItem { index: 2, component: Some(NamespaceUri), byte_offset: Some(5) }",
            ),
            (
                located(
                    EditErrorKind::DuplicateAttribute,
                    EditLocation::AttributeItem {
                        index: 7,
                        component: Some(EditComponent::Prefix),
                        byte_offset: None,
                    },
                ),
                "XML edit error DuplicateAttribute at AttributeItem { index: 7, component: Some(Prefix), byte_offset: None }",
            ),
            (
                located(
                    EditErrorKind::InvalidCharacter,
                    EditLocation::AttributeItem {
                        index: 1,
                        component: Some(EditComponent::Value),
                        byte_offset: Some(9),
                    },
                ),
                "XML edit error InvalidCharacter at AttributeItem { index: 1, component: Some(Value), byte_offset: Some(9) }",
            ),
            (
                global(EditErrorKind::IdentifierExhausted),
                "XML edit error IdentifierExhausted",
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected);
            super::super::check_display(&error);
        }
    }

    #[test]
    fn sparse_format_path_exercises_bounded_public_debug_values() {
        sparse_format_and_debug_contract(&[0x2a, 1, 2, 3]);
    }
}
