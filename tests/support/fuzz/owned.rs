//! Owned storage must preserve the borrowing tree after source independence.

use yaxml::options::XINCLUDE;
use yaxml::{
    Document, Error, ErrorKind, Limit, Limits, NodeKind, OwnedDocument, OwnedNodeKind, ParseOptions,
};

pub(super) fn exercise(input: &[u8], limits: Limits, borrowed: Result<&Document<'_>, Error>) {
    let mut temporary = input.to_vec();
    let owned = if limits == Limits::default() {
        OwnedDocument::parse_bytes(&temporary)
    } else {
        OwnedDocument::with_limits(&temporary, limits)
    };
    temporary.fill(0);
    drop(temporary);
    // In this DTD-free profile, at most n names each retain a URI of <= n+64
    // bytes. Pool growth plus parser copies is conservatively bounded by
    // 16*n*(n+64): at n=1024 this is 17 MiB, below the 64 MiB default.
    // Do not let the early-OwnedBytes exception hide blanket rejection.
    if limits == Limits::default() && input.len() <= 1024 && borrowed.is_ok() {
        assert!(owned.is_ok(), "small valid input failed owned construction");
    }
    // Independent allocation-work goldens distinguish full replacement
    // capacity from net retained length. Reader acceptance isolates all other
    // ceilings; reference normalization spends its raw length before storage.
    let minimum = match input {
        b"<r/>" => Some(1),
        b"<r>a</r>" => Some(3),
        b"<r>&amp;</r>" => Some(8),
        b"<r a='&#9;'/>" => Some(10),
        _ => None,
    };
    if borrowed.is_ok()
        && let Some(minimum) = minimum
    {
        assert_eq!(
            owned.is_ok(),
            limits.max_owned_bytes >= minimum,
            "owned aggregate budget boundary changed"
        );
    }
    let outcome = owned.as_ref().err().copied();
    match owned {
        Ok(owned) => {
            let borrowed = borrowed.expect("owned tree accepted invalid XML");
            check_document(&owned, borrowed);
            if limits == Limits::default() {
                super::serialize::owned(&owned, borrowed, input);
            }
            drop_kept_form(owned);
        }
        Err(error) => {
            assert!(error.offset <= input.len());
            super::check_display(&error);
            // Pool reservations share the normalization budget. They can fail
            // before the borrowing reader reaches a later syntax error.
            // Input that is not UTF-8 is decoded as a stream, so any earlier
            // error (a token ceiling or a malformed declaration) precedes the
            // invalid byte or character that the borrowing DOM rejects up front
            // (tests/charsets.rs).
            let streamed = borrowed.err().is_some_and(|b| {
                matches!(b.kind, ErrorKind::InvalidUtf8 | ErrorKind::InvalidCharacter)
                    && error.offset <= b.offset
            });
            if error.kind != ErrorKind::LimitExceeded(Limit::OwnedBytes) && !streamed {
                assert_eq!(Some(error), borrowed.err(), "owned parse differs");
            }
        }
    }
    if limits == Limits::default() {
        xinclude(input, outcome);
    }
    if limits == Limits::default()
        && let Ok(text) = std::str::from_utf8(input)
    {
        // The first tree is dropped before the second constructor runs.
        assert_eq!(OwnedDocument::parse(text).err(), outcome);
    }
}

fn check_document(owned: &OwnedDocument, borrowed: &Document<'_>) {
    assert_eq!(owned.declaration(), borrowed.declaration().copied());
    assert_eq!(owned.url(), borrowed.url());
    check_nodes(owned, borrowed, |range| range);
}

pub(super) fn check_nodes(
    owned: &OwnedDocument,
    borrowed: &Document<'_>,
    mut source_range: impl FnMut(std::ops::Range<usize>) -> std::ops::Range<usize>,
) {
    assert_eq!(owned.nodes().len(), borrowed.nodes().len());
    // Identity belongs to one tree, while correspondence follows preorder.
    // Map the reference IDs to owned handles before checking every link.
    let handles: std::collections::HashMap<_, _> = borrowed
        .nodes()
        .zip(owned.nodes())
        .map(|(expected, actual)| (expected.id(), actual.id()))
        .collect();
    assert_eq!(
        handles
            .values()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        handles.len(),
        "owned handles alias different nodes"
    );
    assert_eq!(owned.root().id(), handles[&borrowed.root().id()]);
    assert_eq!(
        owned.document_element().map(|element| element.id()),
        borrowed
            .document_element()
            .map(|element| handles[&element.id()])
    );
    for (actual, expected) in owned.nodes().zip(borrowed.nodes()) {
        assert_eq!(actual.id(), handles[&expected.id()]);
        assert_eq!(actual.range(), expected.range().map(&mut source_range));
        assert!(
            actual.name().as_ref() == expected.name(),
            "owned name changed"
        );
        assert!(actual.text() == expected.text(), "owned payload changed");
        assert!(
            actual.text_chunks().eq(expected.text_chunks()),
            "owned string-value chunks changed"
        );
        let mut attributes = actual.attributes();
        assert_eq!(attributes.len(), expected.attributes().len());
        for expected in expected.attributes() {
            let actual = attributes.next().expect("owned attribute missing");
            assert!(actual == *expected, "owned attribute changed");
        }
        assert!(attributes.next().is_none());
        // XML attributes always carry a value; past the end there is none.
        let count = expected.attributes().len();
        assert!((0..count).all(|position| actual.attribute_has_value(position)));
        assert!(!actual.attribute_has_value(count));
        assert!(
            (0..=count)
                .all(|at| actual.unexpanded_attribute(at) == expected.unexpanded_attribute(at)),
            "owned kept attribute form changed"
        );
        #[allow(clippy::clone_on_copy)]
        let copy = actual.clone();
        assert_eq!(copy.id(), actual.id());
        let (kind, target) = match expected.kind() {
            NodeKind::Document => (OwnedNodeKind::Document, None),
            NodeKind::Doctype(doctype) => {
                assert_eq!(actual.doctype().as_ref(), Some(doctype));
                (OwnedNodeKind::DocumentType, None)
            }
            NodeKind::Element(_) => (OwnedNodeKind::Element, None),
            NodeKind::Text(_) => (OwnedNodeKind::Text, None),
            NodeKind::Cdata(_) => (OwnedNodeKind::Cdata, None),
            NodeKind::Comment(_) => (OwnedNodeKind::Comment, None),
            NodeKind::ProcessingInstruction { target, .. } => {
                (OwnedNodeKind::ProcessingInstruction, Some(target.as_ref()))
            }
            NodeKind::EntityRef(name) => {
                assert_eq!(actual.entity_name(), Some(name.as_ref()));
                (OwnedNodeKind::EntityRef, None)
            }
        };
        assert_eq!(actual.kind(), kind);
        assert!(
            actual.processing_instruction_target() == target,
            "owned PI target changed"
        );
        assert_eq!(
            actual.parent().map(|n| n.id()),
            expected.parent().map(|n| handles[&n.id()])
        );
        assert_eq!(
            actual.first_child().map(|n| n.id()),
            expected.first_child().map(|n| handles[&n.id()])
        );
        assert_eq!(
            actual.last_child().map(|n| n.id()),
            expected.last_child().map(|n| handles[&n.id()])
        );
        assert_eq!(
            actual.next_sibling().map(|n| n.id()),
            expected.next_sibling().map(|n| handles[&n.id()])
        );
        assert!(
            actual
                .children()
                .map(|n| n.id())
                .eq(expected.children().map(|n| handles[&n.id()]))
        );
        assert_eq!(actual.descendants().len(), expected.descendants().len());
        assert!(
            actual
                .descendants()
                .map(|n| n.id())
                .eq(expected.descendants().map(|n| handles[&n.id()]))
        );
        check_accessors(actual, expected, &handles);
        let (document, slot) = actual.id().parts();
        assert_eq!(
            yaxml::OwnedNodeId::from_parts(document, slot),
            Some(actual.id())
        );
        assert_eq!(yaxml::OwnedNodeId::from_parts(0, slot), None);
        let resolved = owned.node(actual.id()).unwrap();
        assert_eq!(resolved.id(), actual.id());
        assert_eq!(resolved.kind(), actual.kind());
        assert_eq!(resolved.name(), actual.name());
        assert_eq!(resolved.text(), actual.text());
        assert_eq!(resolved.range(), actual.range());
        assert_eq!(
            resolved.processing_instruction_target(),
            actual.processing_instruction_target()
        );
        assert!(resolved.attributes().eq(actual.attributes()));
        // ID lookup agrees with the borrowed tree.
        for attribute in expected.attributes() {
            if attribute.name.qualified() == "xml:id" {
                assert_eq!(
                    owned.element_by_id(&attribute.value).map(|n| n.id()),
                    borrowed
                        .element_by_id(&attribute.value)
                        .map(|n| handles[&n.id()])
                );
            }
        }
    }
    assert_eq!(owned.doctype(), borrowed.doctype().copied());
    assert!(owned.element_by_id("\u{0}").is_none());
    assert_eq!(owned.html_encoding(), None);
    if owned.nodes().len() <= 64 {
        assert!(!format!("{owned:?}").is_empty());
    }
    if let Some(last) = owned.nodes().last() {
        let small = OwnedDocument::parse("<r/>").unwrap();
        assert!(small.node(last.id()).is_none());
        assert!(small.node(owned.root().id()).is_none());
        assert!(owned.node(small.root().id()).is_none());
    }
}

/// libxml2's tree accessors (`xmlGetProp`, `xmlNodeGetContent`, ...) agree
/// across both DOMs; an attribute is found by its expanded name.
fn check_accessors(
    actual: yaxml::OwnedNode<'_>,
    expected: yaxml::Node<'_, '_>,
    handles: &std::collections::HashMap<yaxml::NodeId, yaxml::OwnedNodeId>,
) {
    assert_eq!(
        actual.previous_sibling().map(|n| n.id()),
        expected.previous_sibling().map(|n| handles[&n.id()])
    );
    assert_eq!(actual.content(), expected.content());
    for substitute in [false, true] {
        assert_eq!(
            actual.list_string(substitute),
            expected.list_string(substitute)
        );
    }
    assert_eq!(actual.path(), expected.path());
    assert_eq!(actual.base_uri(), expected.base_uri());
    assert_eq!(actual.lang(), expected.lang());
    assert_eq!(actual.space_preserve(), expected.space_preserve());
    let bindings = expected.namespaces();
    assert_eq!(actual.namespaces(), bindings);
    let unbound = yaxml::NamespaceBinding {
        prefix: Some("q"),
        uri: "u",
    };
    for binding in bindings.iter().chain([&unbound]) {
        assert_eq!(
            actual.search_ns(binding.prefix),
            expected.search_ns(binding.prefix)
        );
        assert_eq!(
            actual.search_ns_by_href(binding.uri),
            expected.search_ns_by_href(binding.uri)
        );
    }
    // libxml2's attribute name: a name in no namespace is its QName (one a
    // namespace error left prefixed included), and a lookup finds the first
    // match, which kept duplicate expanded names make ambiguous.
    fn key<'n>(name: &'n yaxml::Name<'_>) -> &'n str {
        match name.namespace() {
            Some(_) => name.local(),
            None => name.qualified(),
        }
    }
    for attribute in expected.attributes() {
        let name = &attribute.name;
        let (local, namespace) = (key(name), name.namespace());
        assert_eq!(actual.attribute(local), expected.attribute(local));
        if name.qualified() != "xmlns" && name.prefix() != Some("xmlns") {
            let first = expected
                .attributes()
                .iter()
                .find(|other| key(&other.name) == local && other.name.namespace() == namespace)
                .map(|other| &*other.value);
            assert_eq!(expected.attribute_ns(local, namespace), first);
            assert_eq!(actual.attribute_ns(local, namespace), first);
        }
    }
}

/// `XINCLUDE` through the owned parser, with no resolver: a document without
/// XInclude elements parses as without the flag; failures keep an offset.
fn xinclude(input: &[u8], plain: Option<Error>) {
    let included = OwnedDocument::parse_with(input, ParseOptions::new(XINCLUDE));
    // A resolver that has nothing behaves as none.
    let none = Box::new(|_: yaxml::Request<'_>| Ok(None));
    let resolved = ParseOptions::new(XINCLUDE).with_resolver("d.xml", none);
    let resolved = OwnedDocument::parse_with(input, resolved);
    assert_eq!(resolved.as_ref().err(), included.as_ref().err());
    match &included {
        Err(error) => assert!(error.offset <= input.len()),
        Ok(_) => assert_eq!(plain, None, "XINCLUDE accepted a rejected document"),
    }
    let marker = |ns: &[u8]| input.windows(ns.len()).any(|w| w == ns);
    // References could spell the namespace.
    if !marker(b"Include") && !input.contains(&b'&') {
        assert_eq!(included.err(), plain);
    }
}

/// Setting an attribute value drops the kept-reference form it replaces.
fn drop_kept_form(mut owned: OwnedDocument) {
    let Some(element) = owned.nodes().find(|n| n.unexpanded_attribute(0).is_some()) else {
        return;
    };
    let id = element.id();
    let name = element.attributes().next().unwrap().name.clone();
    let (namespace, local) = (name.namespace().map(str::to_owned), name.local().to_owned());
    // Values become `v` without a kept form; only a repeated name could differ.
    let mut editor = owned.edit(yaxml::EditLimits::default()).unwrap();
    if editor
        .set_attribute_value(id, namespace.as_deref(), &local, "v")
        .is_err()
    {
        return;
    }
    assert_eq!(owned.node(id).unwrap().unexpanded_attribute(0), None);
}
