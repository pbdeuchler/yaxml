#![forbid(unsafe_code)]

use std::ops::Range;

use yaxml::{
    AttributeSpec, EditComponent, EditError, EditErrorKind, EditLimit, EditLimits, EditLocation,
    ElementItem, ElementSpec, Name, NameSpec, NamespaceSpec, OwnedDocument, OwnedNodeId,
    OwnedNodeKind, PrefixPolicy, Writer,
};

const XML: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS: &str = "http://www.w3.org/2000/xmlns/";

fn exact<'a>(local: &'a str, namespace: Option<&'a str>, prefix: Option<&'a str>) -> NameSpec<'a> {
    NameSpec {
        local,
        namespace,
        prefix: PrefixPolicy::Exact(prefix),
    }
}

fn automatic<'a>(local: &'a str, namespace: Option<&'a str>) -> NameSpec<'a> {
    NameSpec {
        local,
        namespace,
        prefix: PrefixPolicy::Automatic,
    }
}

fn attribute<'a>(name: NameSpec<'a>, value: &'a str) -> ElementItem<'a> {
    ElementItem::Attribute(AttributeSpec { name, value })
}

fn namespace<'a>(prefix: Option<&'a str>, uri: &'a str) -> ElementItem<'a> {
    ElementItem::Namespace(NamespaceSpec { prefix, uri })
}

fn empty(local: &str) -> ElementSpec<'_> {
    ElementSpec {
        name: exact(local, None, None),
        items: &[],
    }
}

fn output(document: &OwnedDocument) -> String {
    let mut writer = Writer::new(Vec::new());
    writer.write_owned_document(document).unwrap();
    writer.finish().unwrap();
    String::from_utf8(writer.into_inner()).unwrap()
}

fn name_tuple(name: Name<'_>) -> NameSnapshot {
    NameSnapshot(
        name.qualified().to_owned(),
        name.prefix().map(str::to_owned),
        name.local().to_owned(),
        name.namespace().map(str::to_owned),
    )
}

fn assert_name(name: Name<'_>, expected: (&str, Option<&str>, &str, Option<&str>)) {
    assert_eq!(
        (
            name.qualified(),
            name.prefix(),
            name.local(),
            name.namespace(),
        ),
        expected
    );
}

fn assert_error<T>(
    result: Result<T, EditError>,
    kind: EditErrorKind,
    location: Option<EditLocation>,
) {
    let error = match result {
        Ok(_) => panic!("expected {kind:?}, got success"),
        Err(error) => error,
    };
    assert_eq!(error.kind, kind);
    assert_eq!(error.value_offset, None);
    assert_eq!(error.location, location);
}

#[derive(Debug, Eq, PartialEq)]
struct NameSnapshot(String, Option<String>, String, Option<String>);

#[derive(Debug, Eq, PartialEq)]
struct NodeSnapshot {
    id: OwnedNodeId,
    kind: OwnedNodeKind,
    name: Option<NameSnapshot>,
    attributes: Vec<(NameSnapshot, String)>,
    text: Option<String>,
    parent: Option<OwnedNodeId>,
    first_child: Option<OwnedNodeId>,
    last_child: Option<OwnedNodeId>,
    next_sibling: Option<OwnedNodeId>,
    range: Option<Range<usize>>,
}

#[derive(Debug, Eq, PartialEq)]
struct Snapshot {
    declaration: Option<(String, Option<String>, Option<bool>)>,
    nodes: Vec<NodeSnapshot>,
    output: String,
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
        nodes: document
            .nodes()
            .map(|node| NodeSnapshot {
                id: node.id(),
                kind: node.kind(),
                name: node.name().map(name_tuple),
                attributes: node
                    .attributes()
                    .map(|attribute| (name_tuple(attribute.name), attribute.value.into_owned()))
                    .collect(),
                text: node.text().map(str::to_owned),
                parent: node.parent().map(|parent| parent.id()),
                first_child: node.first_child().map(|child| child.id()),
                last_child: node.last_child().map(|child| child.id()),
                next_sibling: node.next_sibling().map(|sibling| sibling.id()),
                range: node.range(),
            })
            .collect(),
        output: output(document),
    }
}

#[test]
fn create_and_append_publish_complete_unprovenanced_trees() {
    let root_items = [
        namespace(Some("s"), "urn:shop"),
        attribute(exact("id", None, None), "A-17"),
    ];
    let mut document = OwnedDocument::create(
        ElementSpec {
            name: exact("catalog", None, None),
            items: &root_items,
        },
        EditLimits::default(),
    )
    .unwrap();

    assert!(document.declaration().is_none());
    assert_eq!(document.nodes().count(), 2);
    let synthetic = document.root();
    let root = document.root_element();
    assert_eq!(synthetic.kind(), OwnedNodeKind::Document);
    assert_ne!(synthetic.id(), root.id());
    assert_eq!(document.node(synthetic.id()).unwrap().id(), synthetic.id());
    assert_eq!(document.node(root.id()).unwrap().id(), root.id());
    assert_eq!(synthetic.range(), None);
    assert_eq!(root.range(), None);
    assert_eq!(root.parent().unwrap().id(), synthetic.id());
    assert_eq!(synthetic.first_child().unwrap().id(), root.id());
    assert_eq!(synthetic.last_child().unwrap().id(), root.id());
    assert!(root.first_child().is_none());
    assert_name(root.name().unwrap(), ("catalog", None, "catalog", None));
    let mut attributes = root.attributes();
    assert_eq!(attributes.len(), 2);
    let declaration = attributes.next().unwrap();
    assert_name(
        declaration.name,
        ("xmlns:s", Some("xmlns"), "s", Some(XMLNS)),
    );
    assert_eq!(declaration.value, "urn:shop");
    let id = attributes.next().unwrap();
    assert_name(id.name, ("id", None, "id", None));
    assert_eq!(id.value, "A-17");
    assert!(attributes.next().is_none());
    drop(attributes);
    assert_eq!(
        output(&document),
        "<catalog xmlns:s=\"urn:shop\" id=\"A-17\"/>"
    );

    let root_id = root.id();
    let child_items = [attribute(exact("sku", None, None), "1")];
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let child = editor
        .append_element(
            root_id,
            ElementSpec {
                name: exact("item", Some("urn:shop"), Some("s")),
                items: &child_items,
            },
        )
        .unwrap();
    let text = editor.append_text(child, "literal text").unwrap();
    let plain = editor.append_element(root_id, empty("plain")).unwrap();
    editor
        .set_attribute_value(root_id, None, "id", "B-29")
        .unwrap();

    assert_eq!(document.nodes().count(), 5);
    assert_eq!(document.node(child).unwrap().range(), None);
    assert_eq!(document.node(text).unwrap().range(), None);
    assert_eq!(document.node(plain).unwrap().range(), None);
    assert_eq!(
        document.node(child).unwrap().parent().unwrap().id(),
        root_id
    );
    assert_eq!(
        document.node(child).unwrap().next_sibling().unwrap().id(),
        plain
    );
    assert!(document.node(plain).unwrap().next_sibling().is_none());
    assert_name(
        document.node(child).unwrap().name().unwrap(),
        ("s:item", Some("s"), "item", Some("urn:shop")),
    );
    assert_eq!(document.node(text).unwrap().text(), Some("literal text"));
    let serialized = output(&document);
    assert_eq!(
        serialized,
        "<catalog xmlns:s=\"urn:shop\" id=\"B-29\"><s:item sku=\"1\">literal text</s:item><plain/></catalog>"
    );
    let reparsed = OwnedDocument::parse(&serialized).unwrap();
    assert_eq!(output(&reparsed), serialized);
    assert_name(
        reparsed.root_element().name().unwrap(),
        ("catalog", None, "catalog", None),
    );
    assert_eq!(reparsed.root_element().children().count(), 2);
    assert_eq!(
        reparsed
            .root_element()
            .first_child()
            .unwrap()
            .first_child()
            .unwrap()
            .text(),
        Some("literal text")
    );
}

#[test]
fn explicit_items_keep_order_and_automatic_declarations_follow_first_use() {
    let ordered_items = [
        attribute(exact("plain", None, None), "1"),
        namespace(Some("p"), "urn:element"),
        namespace(Some("q"), "urn:attribute"),
        attribute(exact("value", Some("urn:attribute"), Some("q")), "2"),
    ];
    let ordered = OwnedDocument::create(
        ElementSpec {
            name: exact("root", Some("urn:element"), Some("p")),
            items: &ordered_items,
        },
        EditLimits::default(),
    )
    .unwrap();
    assert_eq!(
        output(&ordered),
        "<p:root plain=\"1\" xmlns:p=\"urn:element\" xmlns:q=\"urn:attribute\" q:value=\"2\"/>"
    );

    let generated_items = [
        attribute(automatic("first", Some("urn:first")), "a"),
        attribute(exact("later", Some("urn:later"), Some("ns1")), "b"),
        attribute(automatic("again", Some("urn:first")), "c"),
    ];
    let generated = OwnedDocument::create(
        ElementSpec {
            name: automatic("root", Some("urn:root")),
            items: &generated_items,
        },
        EditLimits::default(),
    )
    .unwrap();
    assert_eq!(
        output(&generated),
        "<root ns2:first=\"a\" ns1:later=\"b\" ns2:again=\"c\" xmlns=\"urn:root\" xmlns:ns2=\"urn:first\" xmlns:ns1=\"urn:later\"/>"
    );
    let mut attributes = generated.root_element().attributes();
    assert_eq!(attributes.len(), 6);
    let first = attributes.next().unwrap();
    assert_name(
        first.name,
        ("ns2:first", Some("ns2"), "first", Some("urn:first")),
    );
    let later = attributes.next().unwrap();
    assert_name(
        later.name,
        ("ns1:later", Some("ns1"), "later", Some("urn:later")),
    );
    let again = attributes.next().unwrap();
    assert_name(
        again.name,
        ("ns2:again", Some("ns2"), "again", Some("urn:first")),
    );
    let default = attributes.next().unwrap();
    assert_name(default.name, ("xmlns", None, "xmlns", Some(XMLNS)));
    let ns2 = attributes.next().unwrap();
    assert_name(ns2.name, ("xmlns:ns2", Some("xmlns"), "ns2", Some(XMLNS)));
    let ns1 = attributes.next().unwrap();
    assert_name(ns1.name, ("xmlns:ns1", Some("xmlns"), "ns1", Some(XMLNS)));
    assert!(attributes.next().is_none());
}

#[test]
fn namespaces_handle_defaults_xml_aliases_and_shadowed_bindings() {
    let root_items = [
        namespace(None, "urn:default"),
        namespace(Some("p"), "urn:outer"),
        namespace(Some("alias"), "urn:outer"),
        attribute(exact("lang", Some(XML), Some("xml")), "en"),
    ];
    let mut document = OwnedDocument::create(
        ElementSpec {
            name: exact("root", Some("urn:default"), None),
            items: &root_items,
        },
        EditLimits::default(),
    )
    .unwrap();
    let root = document.root_element().id();
    let mut editor = document.edit(EditLimits::default()).unwrap();

    let shadow_items = [
        attribute(exact("a", Some("urn:inner"), Some("p")), "1"),
        attribute(exact("b", Some("urn:inner"), Some("p")), "2"),
    ];
    let shadow = editor
        .append_element(
            root,
            ElementSpec {
                name: exact("child", Some("urn:inner"), Some("p")),
                items: &shadow_items,
            },
        )
        .unwrap();
    let old_binding_items = [attribute(automatic("old", Some("urn:outer")), "v")];
    editor
        .append_element(
            shadow,
            ElementSpec {
                name: exact("leaf", None, None),
                items: &old_binding_items,
            },
        )
        .unwrap();
    editor.append_element(root, empty("plain")).unwrap();

    assert_eq!(
        output(&document),
        "<root xmlns=\"urn:default\" xmlns:p=\"urn:outer\" xmlns:alias=\"urn:outer\" xml:lang=\"en\"><p:child p:a=\"1\" p:b=\"2\" xmlns:p=\"urn:inner\"><leaf alias:old=\"v\" xmlns=\"\"/></p:child><plain xmlns=\"\"/></root>"
    );
    assert_eq!(
        document.root_element().attributes().count(),
        4,
        "the implicit xml binding must not be materialized"
    );
}

#[test]
fn unicode_components_succeed_and_invalid_components_have_typed_offsets() {
    let unicode_items = [
        namespace(Some("名"), "urn:ü"),
        attribute(exact("属性", Some("urn:ü"), Some("名")), "é中🙂"),
    ];
    let unicode = OwnedDocument::create(
        ElementSpec {
            name: exact("根", Some("urn:ü"), Some("名")),
            items: &unicode_items,
        },
        EditLimits::default(),
    )
    .unwrap();
    assert_eq!(
        output(&unicode),
        "<名:根 xmlns:名=\"urn:ü\" 名:属性=\"é中🙂\"/>"
    );

    struct Case {
        label: &'static str,
        name: NameSpec<'static>,
        items: Vec<ElementItem<'static>>,
        kind: EditErrorKind,
        location: EditLocation,
    }
    for case in [
        Case {
            label: "element local name",
            name: exact("a:b", None, None),
            items: vec![],
            kind: EditErrorKind::InvalidName,
            location: EditLocation::ElementName {
                component: EditComponent::LocalName,
                byte_offset: Some(1),
            },
        },
        Case {
            label: "element namespace character",
            name: exact("r", Some("urn:\u{1}"), None),
            items: vec![],
            kind: EditErrorKind::InvalidCharacter,
            location: EditLocation::ElementName {
                component: EditComponent::NamespaceUri,
                byte_offset: Some(4),
            },
        },
        Case {
            label: "element prefix name",
            name: exact("r", Some("urn:r"), Some("1p")),
            items: vec![],
            kind: EditErrorKind::InvalidName,
            location: EditLocation::ElementName {
                component: EditComponent::Prefix,
                byte_offset: Some(0),
            },
        },
        Case {
            label: "declaration prefix name",
            name: exact("r", None, None),
            items: vec![namespace(Some("p:q"), "urn:p")],
            kind: EditErrorKind::InvalidName,
            location: EditLocation::NamespaceItem {
                index: 0,
                component: Some(EditComponent::Prefix),
                byte_offset: Some(1),
            },
        },
        Case {
            label: "declaration URI character",
            name: exact("r", None, None),
            items: vec![namespace(Some("p"), "urn:\u{1}")],
            kind: EditErrorKind::InvalidCharacter,
            location: EditLocation::NamespaceItem {
                index: 0,
                component: Some(EditComponent::NamespaceUri),
                byte_offset: Some(4),
            },
        },
        Case {
            label: "attribute local name",
            name: exact("r", None, None),
            items: vec![attribute(exact(":a", None, None), "v")],
            kind: EditErrorKind::InvalidName,
            location: EditLocation::AttributeItem {
                index: 0,
                component: Some(EditComponent::LocalName),
                byte_offset: Some(0),
            },
        },
        Case {
            label: "attribute namespace character",
            name: exact("r", None, None),
            items: vec![attribute(exact("a", Some("u\u{1}"), Some("p")), "v")],
            kind: EditErrorKind::InvalidCharacter,
            location: EditLocation::AttributeItem {
                index: 0,
                component: Some(EditComponent::NamespaceUri),
                byte_offset: Some(1),
            },
        },
        Case {
            label: "attribute prefix name",
            name: exact("r", None, None),
            items: vec![attribute(exact("a", Some("u"), Some("p:q")), "v")],
            kind: EditErrorKind::InvalidName,
            location: EditLocation::AttributeItem {
                index: 0,
                component: Some(EditComponent::Prefix),
                byte_offset: Some(1),
            },
        },
        Case {
            label: "attribute value character",
            name: exact("r", None, None),
            items: vec![attribute(exact("a", None, None), "v\u{1}")],
            kind: EditErrorKind::InvalidCharacter,
            location: EditLocation::AttributeItem {
                index: 0,
                component: Some(EditComponent::Value),
                byte_offset: Some(1),
            },
        },
    ] {
        let error = OwnedDocument::create(
            ElementSpec {
                name: case.name,
                items: &case.items,
            },
            EditLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.kind, case.kind, "{}", case.label);
        assert_eq!(error.value_offset, None, "{}", case.label);
        assert_eq!(error.location, Some(case.location), "{}", case.label);
    }
}

#[test]
fn namespace_and_duplicate_errors_identify_the_implicated_input() {
    struct Case {
        label: &'static str,
        name: NameSpec<'static>,
        items: Vec<ElementItem<'static>>,
        kind: EditErrorKind,
        location: EditLocation,
    }
    for case in [
        Case {
            label: "empty expanded namespace",
            name: exact("r", Some(""), None),
            items: vec![],
            kind: EditErrorKind::Namespace,
            location: EditLocation::ElementName {
                component: EditComponent::NamespaceUri,
                byte_offset: None,
            },
        },
        Case {
            label: "xml prefix with wrong URI",
            name: exact("r", Some("urn:not-xml"), Some("xml")),
            items: vec![],
            kind: EditErrorKind::Namespace,
            location: EditLocation::ElementName {
                component: EditComponent::NamespaceUri,
                byte_offset: None,
            },
        },
        Case {
            label: "XML namespace without xml prefix",
            name: exact("r", Some(XML), None),
            items: vec![],
            kind: EditErrorKind::Namespace,
            location: EditLocation::ElementName {
                component: EditComponent::Prefix,
                byte_offset: None,
            },
        },
        Case {
            label: "XML namespace with an ordinary prefix",
            name: exact("r", Some(XML), Some("p")),
            items: vec![],
            kind: EditErrorKind::Namespace,
            location: EditLocation::ElementName {
                component: EditComponent::Prefix,
                byte_offset: None,
            },
        },
        Case {
            label: "xmlns prefix on an element",
            name: exact("r", Some(XMLNS), Some("xmlns")),
            items: vec![],
            kind: EditErrorKind::Namespace,
            location: EditLocation::ElementName {
                // The intrinsically forbidden XMLNS namespace is validated before
                // considering whether its requested prefix is also reserved.
                component: EditComponent::NamespaceUri,
                byte_offset: None,
            },
        },
        Case {
            label: "xmlns prefix with an ordinary URI",
            name: exact("r", Some("urn:ordinary"), Some("xmlns")),
            items: vec![],
            kind: EditErrorKind::Namespace,
            location: EditLocation::ElementName {
                component: EditComponent::Prefix,
                byte_offset: None,
            },
        },
        Case {
            label: "reserved xml declaration",
            name: exact("r", None, None),
            items: vec![namespace(Some("xml"), "urn:not-xml")],
            kind: EditErrorKind::Namespace,
            location: EditLocation::NamespaceItem {
                index: 0,
                component: None,
                byte_offset: None,
            },
        },
        Case {
            label: "reserved xmlns URI",
            name: exact("r", None, None),
            items: vec![namespace(Some("p"), XMLNS)],
            kind: EditErrorKind::Namespace,
            location: EditLocation::NamespaceItem {
                index: 0,
                component: None,
                byte_offset: None,
            },
        },
        Case {
            label: "prefixed undeclaration",
            name: exact("r", None, None),
            items: vec![namespace(Some("p"), "")],
            kind: EditErrorKind::Namespace,
            location: EditLocation::NamespaceItem {
                index: 0,
                component: None,
                byte_offset: None,
            },
        },
        Case {
            label: "duplicate declaration prefix",
            name: exact("r", None, None),
            items: vec![
                namespace(Some("p"), "urn:one"),
                namespace(Some("p"), "urn:two"),
            ],
            kind: EditErrorKind::DuplicateAttribute,
            location: EditLocation::NamespaceItem {
                index: 1,
                component: None,
                byte_offset: None,
            },
        },
        Case {
            label: "duplicate expanded attribute through aliases",
            name: exact("r", None, None),
            items: vec![
                namespace(Some("p"), "urn:u"),
                namespace(Some("q"), "urn:u"),
                attribute(exact("a", Some("urn:u"), Some("p")), "1"),
                attribute(exact("a", Some("urn:u"), Some("q")), "2"),
            ],
            kind: EditErrorKind::DuplicateAttribute,
            location: EditLocation::AttributeItem {
                index: 3,
                component: None,
                byte_offset: None,
            },
        },
        Case {
            label: "ordinary unprefixed xmlns attribute",
            name: exact("r", None, None),
            items: vec![attribute(exact("xmlns", None, None), "ordinary")],
            kind: EditErrorKind::Namespace,
            location: EditLocation::AttributeItem {
                index: 0,
                component: None,
                byte_offset: None,
            },
        },
    ] {
        let error = OwnedDocument::create(
            ElementSpec {
                name: case.name,
                items: &case.items,
            },
            EditLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.kind, case.kind, "{}", case.label);
        assert_eq!(error.value_offset, None, "{}", case.label);
        assert_eq!(error.location, Some(case.location), "{}", case.label);
    }

    let alias_items = [
        namespace(Some("p"), "urn:u"),
        namespace(Some("q"), "urn:u"),
        attribute(exact("a", Some("urn:u"), Some("p")), "1"),
        attribute(exact("b", Some("urn:u"), Some("q")), "2"),
    ];
    let aliases = OwnedDocument::create(
        ElementSpec {
            name: exact("r", None, None),
            items: &alias_items,
        },
        EditLimits::default(),
    )
    .unwrap();
    assert_eq!(
        output(&aliases),
        "<r xmlns:p=\"urn:u\" xmlns:q=\"urn:u\" p:a=\"1\" q:b=\"2\"/>"
    );

    let xmlns_element = OwnedDocument::create(empty("xmlns"), EditLimits::default()).unwrap();
    assert_eq!(output(&xmlns_element), "<xmlns/>");

    let explicit_xml_items = [namespace(Some("xml"), XML)];
    let explicit_xml = OwnedDocument::create(
        ElementSpec {
            name: exact("r", None, None),
            items: &explicit_xml_items,
        },
        EditLimits::default(),
    )
    .unwrap();
    assert_eq!(
        output(&explicit_xml),
        "<r xmlns:xml=\"http://www.w3.org/XML/1998/namespace\"/>"
    );
}

#[test]
fn component_and_aggregate_byte_limits_are_inclusive_and_ordered() {
    let name_error = OwnedDocument::create(
        empty("four"),
        EditLimits {
            max_name_bytes: 3,
            max_supplied_bytes: 0,
            ..EditLimits::default()
        },
    );
    assert_error(
        name_error,
        EditErrorKind::LimitExceeded(EditLimit::NameBytes),
        Some(EditLocation::ElementName {
            component: EditComponent::LocalName,
            byte_offset: Some(3),
        }),
    );
    assert!(
        OwnedDocument::create(
            empty("four"),
            EditLimits {
                max_name_bytes: 4,
                max_supplied_bytes: 4,
                max_owned_bytes: 4,
                ..EditLimits::default()
            }
        )
        .is_ok()
    );

    struct NameLimitCase {
        name: NameSpec<'static>,
        items: Vec<ElementItem<'static>>,
        location: EditLocation,
    }
    for case in [
        NameLimitCase {
            name: exact("r", Some("urn:u"), None),
            items: vec![],
            location: EditLocation::ElementName {
                component: EditComponent::NamespaceUri,
                byte_offset: Some(4),
            },
        },
        NameLimitCase {
            name: exact("r", Some("u"), Some("pre")),
            items: vec![],
            location: EditLocation::ElementName {
                component: EditComponent::Prefix,
                byte_offset: Some(2),
            },
        },
        NameLimitCase {
            name: exact("r", None, None),
            items: vec![namespace(Some("long"), "u")],
            location: EditLocation::NamespaceItem {
                index: 0,
                component: Some(EditComponent::Prefix),
                byte_offset: Some(3),
            },
        },
        NameLimitCase {
            name: exact("r", None, None),
            items: vec![namespace(Some("p"), "urn:u")],
            location: EditLocation::NamespaceItem {
                index: 0,
                component: Some(EditComponent::NamespaceUri),
                byte_offset: Some(4),
            },
        },
        NameLimitCase {
            name: exact("r", None, None),
            items: vec![attribute(exact("long", None, None), "v")],
            location: EditLocation::AttributeItem {
                index: 0,
                component: Some(EditComponent::LocalName),
                byte_offset: Some(3),
            },
        },
        NameLimitCase {
            name: exact("r", None, None),
            items: vec![attribute(exact("a", Some("urn:u"), Some("p")), "v")],
            location: EditLocation::AttributeItem {
                index: 0,
                component: Some(EditComponent::NamespaceUri),
                byte_offset: Some(4),
            },
        },
        NameLimitCase {
            name: exact("r", None, None),
            items: vec![attribute(exact("a", Some("u"), Some("long")), "v")],
            location: EditLocation::AttributeItem {
                index: 0,
                component: Some(EditComponent::Prefix),
                byte_offset: Some(3),
            },
        },
    ] {
        let limit = match case.location {
            EditLocation::ElementName {
                byte_offset: Some(offset),
                ..
            }
            | EditLocation::NamespaceItem {
                byte_offset: Some(offset),
                ..
            }
            | EditLocation::AttributeItem {
                byte_offset: Some(offset),
                ..
            } => offset,
            _ => unreachable!(),
        };
        assert_error(
            OwnedDocument::create(
                ElementSpec {
                    name: case.name,
                    items: &case.items,
                },
                EditLimits {
                    max_name_bytes: limit,
                    ..EditLimits::default()
                },
            ),
            EditErrorKind::LimitExceeded(EditLimit::NameBytes),
            Some(case.location),
        );
    }

    let exact_name_items = [
        namespace(Some("p"), "urn:u"),
        attribute(exact("attr", Some("urn:u"), Some("p")), "v"),
    ];
    assert!(
        OwnedDocument::create(
            ElementSpec {
                name: exact("root", Some("urn:u"), Some("p")),
                items: &exact_name_items,
            },
            EditLimits {
                max_name_bytes: 5,
                ..EditLimits::default()
            },
        )
        .is_ok()
    );

    let value_items = [attribute(exact("a", None, None), "four")];
    assert_error(
        OwnedDocument::create(
            ElementSpec {
                name: exact("r", None, None),
                items: &value_items,
            },
            EditLimits {
                max_value_bytes: 3,
                ..EditLimits::default()
            },
        ),
        EditErrorKind::LimitExceeded(EditLimit::ValueBytes),
        Some(EditLocation::AttributeItem {
            index: 0,
            component: Some(EditComponent::Value),
            byte_offset: Some(3),
        }),
    );
    assert!(
        OwnedDocument::create(
            ElementSpec {
                name: exact("r", None, None),
                items: &value_items,
            },
            EditLimits {
                max_value_bytes: 4,
                ..EditLimits::default()
            },
        )
        .is_ok()
    );

    let shared = "same";
    let repeated_items = [
        attribute(exact("a", None, None), shared),
        attribute(exact("b", None, None), shared),
    ];
    assert_error(
        OwnedDocument::create(
            ElementSpec {
                name: exact("r", None, None),
                items: &repeated_items,
            },
            EditLimits {
                max_supplied_bytes: 10,
                ..EditLimits::default()
            },
        ),
        EditErrorKind::LimitExceeded(EditLimit::SuppliedBytes),
        Some(EditLocation::AttributeItem {
            index: 1,
            component: Some(EditComponent::Value),
            byte_offset: Some(3),
        }),
    );
    let repeated = OwnedDocument::create(
        ElementSpec {
            name: exact("r", None, None),
            items: &repeated_items,
        },
        EditLimits {
            max_supplied_bytes: 11,
            ..EditLimits::default()
        },
    )
    .unwrap();
    assert_eq!(output(&repeated), "<r a=\"same\" b=\"same\"/>");

    assert_error(
        OwnedDocument::create(
            empty("bad:name"),
            EditLimits {
                max_name_bytes: 8,
                max_supplied_bytes: 7,
                ..EditLimits::default()
            },
        ),
        EditErrorKind::LimitExceeded(EditLimit::SuppliedBytes),
        Some(EditLocation::ElementName {
            component: EditComponent::LocalName,
            byte_offset: Some(7),
        }),
    );
}

#[test]
fn item_depth_and_persistent_limits_cover_exact_boundaries_and_reopen() {
    let invalid_items = [
        attribute(exact(":bad", None, None), "v"),
        attribute(exact("also", None, None), "v"),
    ];
    assert_error(
        OwnedDocument::create(
            ElementSpec {
                name: exact(":bad", None, None),
                items: &invalid_items,
            },
            EditLimits {
                max_depth: 0,
                max_attributes: 0,
                ..EditLimits::default()
            },
        ),
        EditErrorKind::LimitExceeded(EditLimit::Depth),
        None,
    );
    assert_error(
        OwnedDocument::create(
            ElementSpec {
                name: exact("r", None, None),
                items: &invalid_items,
            },
            EditLimits {
                max_attributes: 1,
                ..EditLimits::default()
            },
        ),
        EditErrorKind::LimitExceeded(EditLimit::Attributes),
        None,
    );

    assert_error(
        OwnedDocument::create(
            empty("r"),
            EditLimits {
                max_created_nodes: 0,
                ..EditLimits::default()
            },
        ),
        EditErrorKind::LimitExceeded(EditLimit::CreatedNodes),
        None,
    );
    let one_attribute = [attribute(exact("a", None, None), "v")];
    assert_error(
        OwnedDocument::create(
            ElementSpec {
                name: exact("r", None, None),
                items: &one_attribute,
            },
            EditLimits {
                max_created_nodes: 0,
                max_created_attributes: 0,
                ..EditLimits::default()
            },
        ),
        EditErrorKind::LimitExceeded(EditLimit::CreatedNodes),
        None,
    );
    assert!(
        OwnedDocument::create(
            empty("r"),
            EditLimits {
                max_created_nodes: 1,
                ..EditLimits::default()
            }
        )
        .is_ok()
    );

    let generated_items = [attribute(automatic("a", Some("urn:a")), "v")];
    assert_error(
        OwnedDocument::create(
            ElementSpec {
                name: exact("r", None, None),
                items: &generated_items,
            },
            EditLimits {
                max_created_attributes: 1,
                ..EditLimits::default()
            },
        ),
        EditErrorKind::LimitExceeded(EditLimit::CreatedAttributes),
        None,
    );
    assert!(
        OwnedDocument::create(
            ElementSpec {
                name: exact("r", None, None),
                items: &generated_items,
            },
            EditLimits {
                max_created_attributes: 2,
                ..EditLimits::default()
            },
        )
        .is_ok()
    );

    let initial_items = [attribute(exact("a", None, None), "v")];
    let mut document = OwnedDocument::create(
        ElementSpec {
            name: exact("r", None, None),
            items: &initial_items,
        },
        EditLimits::default(),
    )
    .unwrap();
    assert_error(
        document.edit(EditLimits {
            max_created_nodes: 0,
            max_owned_bytes: 0,
            max_created_attributes: 0,
            ..EditLimits::default()
        }),
        EditErrorKind::LimitExceeded(EditLimit::CreatedNodes),
        None,
    );
    assert_error(
        document.edit(EditLimits {
            max_created_nodes: 1,
            max_owned_bytes: 0,
            max_created_attributes: 0,
            ..EditLimits::default()
        }),
        EditErrorKind::LimitExceeded(EditLimit::OwnedBytes),
        None,
    );
    assert_error(
        document.edit(EditLimits {
            max_created_nodes: 1,
            max_created_attributes: 0,
            ..EditLimits::default()
        }),
        EditErrorKind::LimitExceeded(EditLimit::CreatedAttributes),
        None,
    );

    let root = document.root_element().id();
    let child_items = [attribute(exact("b", None, None), "w")];
    let child;
    {
        let mut editor = document.edit(EditLimits::default()).unwrap();
        child = editor
            .append_element(
                root,
                ElementSpec {
                    name: exact("c", None, None),
                    items: &child_items,
                },
            )
            .unwrap();
        editor.remove_subtree(child).unwrap();
    }
    let before = snapshot(&document);
    assert_error(
        document.edit(EditLimits {
            max_created_nodes: 2,
            max_created_attributes: 1,
            ..EditLimits::default()
        }),
        EditErrorKind::LimitExceeded(EditLimit::CreatedAttributes),
        None,
    );
    let mut editor = document
        .edit(EditLimits {
            max_created_nodes: 2,
            max_created_attributes: 2,
            ..EditLimits::default()
        })
        .unwrap();
    assert_error(
        editor.append_element(root, empty("new")),
        EditErrorKind::LimitExceeded(EditLimit::CreatedNodes),
        None,
    );
    assert_eq!(
        snapshot(&document),
        before,
        "removed nodes are not refunded"
    );

    let mut editor = document
        .edit(EditLimits {
            max_created_nodes: 3,
            max_created_attributes: 2,
            ..EditLimits::default()
        })
        .unwrap();
    assert_error(
        editor.append_element(
            root,
            ElementSpec {
                name: exact("new", None, None),
                items: &child_items,
            },
        ),
        EditErrorKind::LimitExceeded(EditLimit::CreatedAttributes),
        None,
    );
    assert!(editor.append_element(root, empty("new")).is_ok());

    let mut precedence = OwnedDocument::create(empty("r"), EditLimits::default()).unwrap();
    let precedence_root = precedence.root_element().id();
    let mut editor = precedence
        .edit(EditLimits {
            max_created_nodes: 1,
            max_created_attributes: 0,
            ..EditLimits::default()
        })
        .unwrap();
    assert_error(
        editor.append_element(
            precedence_root,
            ElementSpec {
                name: exact("child", None, None),
                items: &one_attribute,
            },
        ),
        EditErrorKind::LimitExceeded(EditLimit::CreatedNodes),
        None,
    );
}

#[test]
fn owned_byte_growth_is_cumulative_from_creation() {
    assert_error(
        OwnedDocument::create(
            empty("r"),
            EditLimits {
                max_owned_bytes: 0,
                ..EditLimits::default()
            },
        ),
        EditErrorKind::LimitExceeded(EditLimit::OwnedBytes),
        None,
    );
    let mut document = OwnedDocument::create(
        empty("r"),
        EditLimits {
            max_owned_bytes: 1,
            ..EditLimits::default()
        },
    )
    .unwrap();
    assert_error(
        document.edit(EditLimits {
            max_owned_bytes: 0,
            ..EditLimits::default()
        }),
        EditErrorKind::LimitExceeded(EditLimit::OwnedBytes),
        None,
    );
    let root = document.root_element().id();
    assert_error(
        document
            .edit(EditLimits {
                max_owned_bytes: 2,
                ..EditLimits::default()
            })
            .unwrap()
            .append_element(root, empty("c")),
        EditErrorKind::LimitExceeded(EditLimit::OwnedBytes),
        None,
    );
    assert!(
        document
            .edit(EditLimits {
                max_owned_bytes: 3,
                ..EditLimits::default()
            })
            .unwrap()
            .append_element(root, empty("c"))
            .is_ok()
    );
}

#[test]
fn looser_construction_ceilings_never_create_scratch_overflow_failures() {
    let loose = EditLimits {
        max_name_bytes: usize::MAX,
        max_supplied_bytes: usize::MAX,
        max_attributes: usize::MAX,
        max_depth: usize::MAX,
        max_created_attributes: usize::MAX,
        max_namespace_work: usize::MAX,
        ..EditLimits::default()
    };
    let mut document = OwnedDocument::create(empty("r"), loose).unwrap();
    let root = document.root_element().id();
    document
        .edit(loose)
        .unwrap()
        .append_element(root, empty("child"))
        .unwrap();
    assert_eq!(output(&document), "<r><child/></r>");

    let namespace_only = EditLimits {
        max_namespace_work: usize::MAX,
        ..EditLimits::default()
    };
    let mut document = OwnedDocument::create(empty("r"), namespace_only).unwrap();
    let root = document.root_element().id();
    document
        .edit(namespace_only)
        .unwrap()
        .append_element(root, empty("child"))
        .unwrap();
    assert_eq!(output(&document), "<r><child/></r>");
}

#[test]
fn namespace_work_counts_empty_ancestors_and_every_wide_attribute() {
    assert!(
        OwnedDocument::create(
            empty("r"),
            EditLimits {
                max_namespace_work: 0,
                ..EditLimits::default()
            },
        )
        .is_ok()
    );

    let mut empty_chain = OwnedDocument::create(empty("r"), EditLimits::default()).unwrap();
    let root = empty_chain.root_element().id();
    let (middle, parent);
    {
        let mut editor = empty_chain.edit(EditLimits::default()).unwrap();
        middle = editor.append_element(root, empty("e")).unwrap();
        parent = editor.append_element(middle, empty("f")).unwrap();
    }
    let before = snapshot(&empty_chain);
    assert_error(
        empty_chain
            .edit(EditLimits {
                max_namespace_work: 2,
                ..EditLimits::default()
            })
            .unwrap()
            .append_element(parent, empty("g")),
        EditErrorKind::LimitExceeded(EditLimit::NamespaceWork),
        None,
    );
    assert_eq!(snapshot(&empty_chain), before);
    assert!(
        empty_chain
            .edit(EditLimits {
                max_namespace_work: 3,
                ..EditLimits::default()
            })
            .unwrap()
            .append_element(parent, empty("g"))
            .is_ok()
    );

    let mut wide = OwnedDocument::parse("<r a='1' b='2'><e><f/></e></r>").unwrap();
    let wide_parent = wide
        .root_element()
        .first_child()
        .unwrap()
        .first_child()
        .unwrap()
        .id();
    let before = snapshot(&wide);
    assert_error(
        wide.edit(EditLimits {
            max_namespace_work: 4,
            ..EditLimits::default()
        })
        .unwrap()
        .append_element(wide_parent, empty("g")),
        EditErrorKind::LimitExceeded(EditLimit::NamespaceWork),
        None,
    );
    assert_eq!(snapshot(&wide), before);
    assert!(
        wide.edit(EditLimits {
            max_namespace_work: 5,
            ..EditLimits::default()
        })
        .unwrap()
        .append_element(wide_parent, empty("g"))
        .is_ok()
    );
}

#[test]
fn automatic_resolution_does_not_reuse_a_shadowed_ancestor_prefix() {
    let root_items = [namespace(Some("p"), "urn:outer")];
    let mut document = OwnedDocument::create(
        ElementSpec {
            name: exact("r", None, None),
            items: &root_items,
        },
        EditLimits::default(),
    )
    .unwrap();
    let root = document.root_element().id();
    let shadow_items = [namespace(Some("p"), "urn:inner")];
    let request_items = [attribute(automatic("a", Some("urn:outer")), "v")];
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let shadow = editor
        .append_element(
            root,
            ElementSpec {
                name: exact("shadow", None, None),
                items: &shadow_items,
            },
        )
        .unwrap();
    editor
        .append_element(
            shadow,
            ElementSpec {
                name: exact("leaf", None, None),
                items: &request_items,
            },
        )
        .unwrap();
    assert_eq!(
        output(editor.document()),
        "<r xmlns:p=\"urn:outer\"><shadow xmlns:p=\"urn:inner\"><leaf ns1:a=\"v\" xmlns:ns1=\"urn:outer\"/></shadow></r>"
    );
}

#[test]
fn generated_attribute_count_precedes_later_namespace_semantics() {
    let items = [
        attribute(automatic("a", Some("urn:auto")), "v"),
        attribute(exact("b", None, Some("unbound")), "v"),
    ];
    assert_error(
        OwnedDocument::create(
            ElementSpec {
                name: exact("r", None, None),
                items: &items,
            },
            EditLimits {
                max_attributes: 2,
                ..EditLimits::default()
            },
        ),
        EditErrorKind::LimitExceeded(EditLimit::Attributes),
        None,
    );
    assert_error(
        OwnedDocument::create(
            ElementSpec {
                name: exact("r", None, None),
                items: &items,
            },
            EditLimits {
                max_attributes: 3,
                ..EditLimits::default()
            },
        ),
        EditErrorKind::Namespace,
        Some(EditLocation::AttributeItem {
            index: 1,
            component: None,
            byte_offset: None,
        }),
    );
}

#[test]
fn namespace_work_stops_at_the_first_default_binding_uri_comparison() {
    let items = [namespace(None, "urn:u"), namespace(Some("p"), "urn:u")];
    let mut document = OwnedDocument::create(
        ElementSpec {
            name: exact("r", Some("urn:u"), None),
            items: &items,
        },
        EditLimits::default(),
    )
    .unwrap();
    let root = document.root_element().id();
    let before = snapshot(&document);

    // One ancestor node + its first declaration + one declaration-candidate
    // probe + both five-byte URI operands. Reserved-name classification belongs
    // to SuppliedBytes validation, so the inclusive namespace-work total is 13.
    // The later matching `p` declaration must not be tried after the final URI
    // comparison exhausts the operation's budget.
    assert_error(
        document
            .edit(EditLimits {
                max_namespace_work: 12,
                ..EditLimits::default()
            })
            .unwrap()
            .append_element(
                root,
                ElementSpec {
                    name: automatic("child", Some("urn:u")),
                    items: &[],
                },
            ),
        EditErrorKind::LimitExceeded(EditLimit::NamespaceWork),
        None,
    );
    assert_eq!(snapshot(&document), before);
    let child = document
        .edit(EditLimits {
            max_namespace_work: 13,
            ..EditLimits::default()
        })
        .unwrap()
        .append_element(
            root,
            ElementSpec {
                name: automatic("child", Some("urn:u")),
                items: &[],
            },
        )
        .unwrap();
    assert_name(
        document.node(child).unwrap().name().unwrap(),
        ("child", None, "child", Some("urn:u")),
    );
    assert_eq!(document.node(child).unwrap().attributes().count(), 0);
}

#[test]
fn inherited_long_automatic_prefix_grows_scratch_and_keeps_failures_atomic() {
    let prefix = "π".repeat(2_048);
    assert_eq!(prefix.len(), 4_096);
    let input = format!("<r xmlns:{prefix}='urn:long'/>");
    let mut document = OwnedDocument::parse(&input).unwrap();
    let parent = document.root_element().id();
    let parent_range = document.node(parent).unwrap().range();
    let document_range = document.root().range();
    let before = snapshot(&document);
    let items = [attribute(automatic("a", Some("urn:long")), "value")];
    let child = ElementSpec {
        name: exact("child", None, None),
        items: &items,
    };

    assert_error(
        document
            .edit(EditLimits {
                max_name_bytes: prefix.len() - 1,
                ..EditLimits::default()
            })
            .unwrap()
            .append_element(parent, child),
        EditErrorKind::LimitExceeded(EditLimit::NameBytes),
        Some(EditLocation::AttributeItem {
            index: 0,
            component: Some(EditComponent::Prefix),
            byte_offset: Some(prefix.len() - 1),
        }),
    );
    assert_eq!(snapshot(&document), before);

    assert_error(
        document
            .edit(EditLimits {
                max_namespace_work: 0,
                ..EditLimits::default()
            })
            .unwrap()
            .append_element(parent, child),
        EditErrorKind::LimitExceeded(EditLimit::NamespaceWork),
        None,
    );
    assert_eq!(snapshot(&document), before);

    let child = document
        .edit(EditLimits::default())
        .unwrap()
        .append_element(parent, child)
        .unwrap();

    let parent_node = document.node(parent).unwrap();
    assert_name(parent_node.name().unwrap(), ("r", None, "r", None));
    assert_eq!(parent_node.range(), parent_range);
    assert_eq!(parent_node.first_child().unwrap().id(), child);
    assert_eq!(parent_node.last_child().unwrap().id(), child);
    let declaration = parent_node.attributes().next().unwrap();
    let declaration_name = format!("xmlns:{prefix}");
    assert_name(
        declaration.name,
        (&declaration_name, Some("xmlns"), &prefix, Some(XMLNS)),
    );
    assert_eq!(declaration.value, "urn:long");
    assert_eq!(document.root().range(), document_range);

    let child_node = document.node(child).unwrap();
    assert_eq!(child_node.range(), None);
    assert_name(child_node.name().unwrap(), ("child", None, "child", None));
    assert_eq!(child_node.parent().unwrap().id(), parent);
    assert!(child_node.first_child().is_none());
    assert!(child_node.last_child().is_none());
    assert!(child_node.next_sibling().is_none());
    let mut attributes = child_node.attributes();
    let attribute = attributes.next().unwrap();
    let qualified = format!("{prefix}:a");
    assert_name(
        attribute.name,
        (&qualified, Some(&prefix), "a", Some("urn:long")),
    );
    assert_eq!(attribute.value, "value");
    assert!(attributes.next().is_none());
    assert_eq!(
        output(&document),
        format!("<r xmlns:{prefix}=\"urn:long\"><child {prefix}:a=\"value\"/></r>")
    );
}

#[test]
fn append_precedence_is_handle_kind_depth_items_then_input() {
    let mut document = OwnedDocument::create(empty("r"), EditLimits::default()).unwrap();
    let root = document.root_element().id();
    let foreign = OwnedDocument::create(empty("foreign"), EditLimits::default())
        .unwrap()
        .root_element()
        .id();
    let invalid_items = [
        attribute(exact(":bad", None, None), "v"),
        attribute(exact("extra", None, None), "v"),
    ];
    let invalid = ElementSpec {
        name: exact(":bad", None, None),
        items: &invalid_items,
    };

    let mut editor = document.edit(EditLimits::default()).unwrap();
    let text = editor.append_text(root, "text").unwrap();
    let stale = editor.append_element(root, empty("stale")).unwrap();
    editor.remove_subtree(stale).unwrap();
    let before = snapshot(editor.document());

    assert_error(
        editor.append_element(foreign, invalid),
        EditErrorKind::InvalidHandle,
        None,
    );
    assert_eq!(snapshot(editor.document()), before);
    assert_error(
        editor.append_element(stale, invalid),
        EditErrorKind::InvalidHandle,
        None,
    );
    assert_eq!(snapshot(editor.document()), before);
    assert_error(
        editor.append_element(text, invalid),
        EditErrorKind::WrongNodeKind,
        None,
    );
    assert_eq!(snapshot(editor.document()), before);

    let mut editor = document
        .edit(EditLimits {
            max_depth: 1,
            max_attributes: 0,
            ..EditLimits::default()
        })
        .unwrap();
    assert_error(
        editor.append_element(root, invalid),
        EditErrorKind::LimitExceeded(EditLimit::Depth),
        None,
    );
    assert_eq!(snapshot(editor.document()), before);

    let mut editor = document
        .edit(EditLimits {
            max_depth: 2,
            max_attributes: 1,
            ..EditLimits::default()
        })
        .unwrap();
    assert_error(
        editor.append_element(root, invalid),
        EditErrorKind::LimitExceeded(EditLimit::Attributes),
        None,
    );
    assert_eq!(snapshot(editor.document()), before);
    let recovered = editor.append_element(root, empty("recovered")).unwrap();
    assert_eq!(editor.document().node(recovered).unwrap().range(), None);
}

#[test]
fn failed_namespace_plans_are_atomic_and_the_editor_recovers() {
    let root_items = [namespace(Some("p"), "urn:outer")];
    let mut document = OwnedDocument::create(
        ElementSpec {
            name: exact("r", None, None),
            items: &root_items,
        },
        EditLimits::default(),
    )
    .unwrap();
    let root = document.root_element().id();
    let conflicting_items = [
        attribute(exact("a", Some("urn:inner"), Some("p")), "1"),
        attribute(exact("b", Some("urn:other"), Some("p")), "2"),
    ];
    let duplicate_items = [
        attribute(exact("a", Some("urn:u"), Some("p")), "1"),
        attribute(exact("a", Some("urn:u"), Some("q")), "2"),
    ];
    let automatic_items = [attribute(automatic("a", Some("urn:a")), "v")];
    let invalid_items = [attribute(exact("a", None, None), "bad\u{1}")];
    let conflicting_default = [namespace(None, "urn:unexpected")];
    let before = snapshot(&document);
    let mut editor = document.edit(EditLimits::default()).unwrap();

    assert_error(
        editor.append_element(
            root,
            ElementSpec {
                name: exact("child", Some("urn:inner"), Some("p")),
                items: &conflicting_items,
            },
        ),
        EditErrorKind::Namespace,
        Some(EditLocation::AttributeItem {
            index: 1,
            component: None,
            byte_offset: None,
        }),
    );
    assert_eq!(snapshot(editor.document()), before);
    assert_error(
        editor.append_element(
            root,
            ElementSpec {
                name: exact("child", None, None),
                items: &duplicate_items,
            },
        ),
        EditErrorKind::DuplicateAttribute,
        Some(EditLocation::AttributeItem {
            index: 1,
            component: None,
            byte_offset: None,
        }),
    );
    assert_eq!(snapshot(editor.document()), before);
    assert_error(
        editor.append_element(
            root,
            ElementSpec {
                name: exact("child", None, None),
                items: &invalid_items,
            },
        ),
        EditErrorKind::InvalidCharacter,
        Some(EditLocation::AttributeItem {
            index: 0,
            component: Some(EditComponent::Value),
            byte_offset: Some(3),
        }),
    );
    assert_eq!(snapshot(editor.document()), before);
    assert_error(
        editor.append_element(
            root,
            ElementSpec {
                name: exact("child", None, None),
                items: &conflicting_default,
            },
        ),
        EditErrorKind::Namespace,
        Some(EditLocation::ElementName {
            component: EditComponent::NamespaceUri,
            byte_offset: None,
        }),
    );
    assert_eq!(snapshot(editor.document()), before);

    let mut editor = document
        .edit(EditLimits {
            max_attributes: 1,
            ..EditLimits::default()
        })
        .unwrap();
    assert_error(
        editor.append_element(
            root,
            ElementSpec {
                name: exact("child", None, None),
                items: &automatic_items,
            },
        ),
        EditErrorKind::LimitExceeded(EditLimit::Attributes),
        None,
    );
    assert_eq!(snapshot(editor.document()), before);
    let recovered = editor.append_element(root, empty("ok")).unwrap();
    assert_eq!(
        editor
            .document()
            .node(recovered)
            .unwrap()
            .parent()
            .unwrap()
            .id(),
        root
    );
}

#[test]
fn append_remove_keeps_survivor_topology_and_never_reuses_handles() {
    let mut document = OwnedDocument::create(empty("r"), EditLimits::default()).unwrap();
    let root = document.root_element().id();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let first = editor.append_element(root, empty("first")).unwrap();
    let middle = editor.append_element(root, empty("middle")).unwrap();
    let middle_leaf = editor.append_text(middle, "gone").unwrap();
    let middle_child = editor.append_element(middle, empty("nested")).unwrap();
    let last = editor.append_element(root, empty("last")).unwrap();
    assert_eq!(
        output(editor.document()),
        "<r><first/><middle>gone<nested/></middle><last/></r>"
    );

    editor.remove_subtree(middle).unwrap();
    assert!(editor.document().node(middle).is_none());
    assert!(editor.document().node(middle_leaf).is_none());
    assert!(editor.document().node(middle_child).is_none());
    let children: Vec<_> = editor
        .document()
        .root_element()
        .children()
        .map(|node| node.id())
        .collect();
    assert_eq!(children, [first, last]);
    assert_eq!(
        editor
            .document()
            .node(first)
            .unwrap()
            .next_sibling()
            .unwrap()
            .id(),
        last
    );
    assert!(
        editor
            .document()
            .node(last)
            .unwrap()
            .next_sibling()
            .is_none()
    );

    let fresh = editor.append_element(root, empty("fresh")).unwrap();
    assert_ne!(fresh, middle);
    assert_ne!(fresh, middle_leaf);
    assert_ne!(fresh, middle_child);
    assert_eq!(output(editor.document()), "<r><first/><last/><fresh/></r>");
}
