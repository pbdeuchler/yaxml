use super::*;

/// Everything a caller can observe, plus the arenas a failed copy must
/// truncate back and the cumulative counters it must restore.
type Snapshot = (Vec<u8>, Vec<Option<String>>, [usize; 5], (usize, usize));

fn snapshot(document: &OwnedDocument) -> Snapshot {
    let output = crate::save::to_vec(document, &crate::save::Options::default()).unwrap();
    let paths = document.nodes().map(|node| node.path()).collect();
    let lengths = [
        document.nodes.len(),
        document.attributes.len(),
        document.kept.len(),
        document.ids.len(),
        document.pool.0.len(),
    ];
    let counters = (
        document.edit_created_nodes,
        document.edit_created_attributes,
    );
    (output, paths, lengths, counters)
}

fn tighten(document: &mut OwnedDocument) {
    document.nodes.shrink_to_fit();
    document.attributes.shrink_to_fit();
    document.pool.0.shrink_to_fit();
}

fn written(document: &OwnedDocument) -> String {
    let saved = crate::save::to_vec(document, &crate::save::Options::default()).unwrap();
    let saved = String::from_utf8(saved).unwrap();
    saved.lines().skip(1).collect()
}

#[test]
fn failed_reservations_publish_nothing_and_retry() {
    let source =
        "<!DOCTYPE r [<!ENTITY e 'v'>]><r xmlns:p='urn:p'><p:a k='&e;' xml:id='i'><b/>t</p:a></r>";
    let source = OwnedDocument::parse(source).unwrap();
    let a = source
        .nodes()
        .find(|node| node.path().as_deref() == Some("/r/p:a"))
        .unwrap()
        .id();
    // `skip` 1 passes the copy's own reservation and fails the one for
    // the root's declarations, after the copy was appended.
    let cases = [
        (EditReservation::Scratch, 0),
        (EditReservation::Node, 0),
        (EditReservation::Attribute, 0),
        (EditReservation::Attribute, 1),
        (EditReservation::Pool, 0),
        (EditReservation::Pool, 1),
    ];
    for (reservation, skip) in cases {
        let mut document = OwnedDocument::parse("<t><u/></t>").unwrap();
        tighten(&mut document);
        let before = snapshot(&document);
        {
            let failure = EditReservationFailureGuard::arm(reservation, skip);
            let mut editor = document.edit(EditLimits::default()).unwrap();
            let result = editor.import(&source, a, true);
            assert!(failure.fired(), "{reservation:?} {skip} never reserved");
            let kind = result.unwrap_err().kind;
            assert_eq!(kind, EditErrorKind::Allocation, "{reservation:?} {skip}");
        }
        assert_eq!(snapshot(&document), before, "{reservation:?} {skip}");
        let mut editor = document.edit(EditLimits::default()).unwrap();
        let copy = editor.import(&source, a, true).unwrap();
        let t = editor.document().root_element().id();
        editor.append_child(t, copy).unwrap();
        assert_eq!(
            written(&document),
            "<t><u/><p:a xmlns:p=\"urn:p\" k=\"v\" xml:id=\"i\"><b/>t</p:a></t>"
        );
        assert!(document.element_by_id("i").is_some());
    }
}

#[test]
fn in_document_copies_fail_atomically() {
    let input = "<r xmlns:p='urn:p'><p:a k='1'><b/></p:a></r>";
    for reservation in [
        EditReservation::Scratch,
        EditReservation::Node,
        EditReservation::Attribute,
        EditReservation::Pool,
    ] {
        let mut document = OwnedDocument::parse(input).unwrap();
        tighten(&mut document);
        let a = document.root_element().first_child().unwrap().id();
        let before = snapshot(&document);
        {
            let failure = EditReservationFailureGuard::arm(reservation, 0);
            let result = document.edit(EditLimits::default()).unwrap().copy(a, true);
            assert!(failure.fired(), "{reservation:?}");
            assert_eq!(result.unwrap_err().kind, EditErrorKind::Allocation);
        }
        assert_eq!(snapshot(&document), before, "{reservation:?}");
    }
}

/// Makes the elements named `local` claim the default namespace `uri`
/// without a declaration, as a name that lost its binding.
fn rebind(document: &mut OwnedDocument, local: &str, uri: &str) {
    let range = document.pool.0.len()..document.pool.0.len() + uri.len();
    document.pool.0.push_str(uri);
    let pool = &document.pool.0;
    for record in document.nodes.iter_mut() {
        if let Content::Element { name, .. } = &mut record.content
            && &pool[name.qualified.clone()] == local
        {
            name.namespace = Some(range.clone());
        }
    }
}

#[test]
fn declarations_fall_back_to_where_they_are_needed() {
    // (input, element rebound to urn:d, copied element path, copy)
    let cases = [
        // Hoisting `xmlns="urn:d"` would capture the unnamespaced root.
        (
            "<r><a><b/><b/></a></r>",
            "b",
            "/r/a",
            "<a><b xmlns=\"urn:d\"/><b xmlns=\"urn:d\"/></a>",
        ),
        // Hoisting would need `xmlns=""` on x as well: three, not one.
        (
            "<r xmlns:p='urn:p'><p:a><x><b/></x></p:a></r>",
            "b",
            "/r/p:a",
            "<p:a xmlns:p=\"urn:p\"><x><b xmlns=\"urn:d\"/></x></p:a>",
        ),
        // Shared by two branches under a prefixed root: hoisted.
        (
            "<r xmlns:p='urn:p'><p:a><b/><b/></p:a></r>",
            "b",
            "/r/p:a",
            "<p:a xmlns:p=\"urn:p\" xmlns=\"urn:d\"><b/><b/></p:a>",
        ),
    ];
    for (input, local, path, expected) in cases {
        let mut document = OwnedDocument::parse(input).unwrap();
        let node = document
            .nodes()
            .find(|node| node.path().as_deref() == Some(path))
            .unwrap()
            .id();
        rebind(&mut document, local, "urn:d");
        let mut target = OwnedDocument::parse("<t/>").unwrap();
        let t = target.root_element().id();
        let mut editor = target.edit(EditLimits::default()).unwrap();
        let copy = editor.import(&document, node, true).unwrap();
        editor.append_child(t, copy).unwrap();
        assert_eq!(written(&target), format!("<t>{expected}</t>"), "{input}");
    }
}

#[test]
fn try_clone_keeps_storage_and_counters() {
    let mut document = OwnedDocument::parse("<r>text</r>").unwrap();
    let root = document.root_element().id();
    document
        .edit(EditLimits::default())
        .unwrap()
        .append_text(root, "more")
        .unwrap();
    let clone = document.try_clone().unwrap();
    assert_eq!(clone.pool.0, document.pool.0);
    assert_eq!(clone.edit_created_nodes, document.edit_created_nodes);
    assert_ne!(clone.identity, document.identity);
}
