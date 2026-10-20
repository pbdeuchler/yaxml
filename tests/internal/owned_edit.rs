use super::*;

/// Everything a caller can observe: serialization, every node's handle and
/// path, attribute views, and the created-attribute counter.
type Snapshot = (Vec<u8>, Vec<(OwnedNodeId, Option<String>)>, usize);

fn snapshot(document: &OwnedDocument) -> Snapshot {
    let output = crate::save::to_vec(document, &crate::save::Options::default()).unwrap();
    let nodes = document
        .nodes()
        .map(|node| (node.id(), node.path()))
        .collect();
    (output, nodes, document.edit_created_attributes)
}

/// Leaves no spare capacity, so the next growth of each store is a reservation.
fn tighten(document: &mut OwnedDocument) {
    document.attributes.shrink_to_fit();
    document.pool.0.shrink_to_fit();
}

fn find(document: &OwnedDocument, path: &str) -> OwnedNodeId {
    let node = document
        .nodes()
        .find(|node| node.path().as_deref() == Some(path));
    node.unwrap_or_else(|| panic!("no node at {path}")).id()
}

type Operation = fn(&mut Editor<'_>) -> Result<(), EditError>;

fn name<'a>(local: &'a str, namespace: Option<&'a str>, prefix: PrefixPolicy<'a>) -> NameSpec<'a> {
    NameSpec {
        local,
        namespace,
        prefix,
    }
}

#[test]
fn failed_reservations_publish_nothing_and_retry() {
    let moved: Operation = |editor| {
        let (b, c) = (
            find(editor.document(), "/r/p:a/p:b"),
            find(editor.document(), "/r/c"),
        );
        editor.append_child(c, b).map(drop)
    };
    let merged: Operation = |editor| {
        let (r, text) = (
            find(editor.document(), "/r"),
            find(editor.document(), "/r/text()[1]"),
        );
        editor.append_child(r, text).map(drop)
    };
    let attribute: Operation = |editor| {
        let element = find(editor.document(), "/r/c");
        editor.set_attribute(
            element,
            name("x", Some("urn:q"), PrefixPolicy::Automatic),
            "v",
        )
    };
    let declared: Operation = |editor| {
        let element = find(editor.document(), "/r/c");
        editor.declare_namespace(element, Some("q"), "urn:q")
    };
    let renamed: Operation = |editor| {
        let element = find(editor.document(), "/r/p:a");
        editor.set_name(
            element,
            name("a", Some("urn:q"), PrefixPolicy::Exact(Some("q"))),
        )
    };
    let compacted: Operation = |editor| editor.compact();
    let input = "<r>a<p:a xmlns:p='urn:p'><p:b/></p:a><c k='1'/>b</r>";
    let cases = [
        ("move", moved, EditReservation::Attribute),
        ("move", moved, EditReservation::Pool),
        ("merge", merged, EditReservation::Pool),
        ("set", attribute, EditReservation::Attribute),
        ("set", attribute, EditReservation::Pool),
        ("declare", declared, EditReservation::Attribute),
        ("declare", declared, EditReservation::Pool),
        ("rename", renamed, EditReservation::Pool),
        ("compact", compacted, EditReservation::Scratch),
        ("compact", compacted, EditReservation::Pool),
    ];
    for (case, operation, reservation) in cases {
        let mut document = OwnedDocument::parse(input).unwrap();
        tighten(&mut document);
        let before = snapshot(&document);
        {
            let failure = EditReservationFailureGuard::arm(reservation, 0);
            let mut editor = document.edit(EditLimits::default()).unwrap();
            let result = operation(&mut editor);
            assert!(
                result.is_err() || !failure.fired(),
                "{case} {reservation:?}"
            );
            assert!(failure.fired(), "{case} {reservation:?} never reserved");
            assert_eq!(
                result.unwrap_err().kind,
                EditErrorKind::Allocation,
                "{case} {reservation:?}"
            );
        }
        assert_eq!(snapshot(&document), before, "{case} {reservation:?}");
        let mut editor = document.edit(EditLimits::default()).unwrap();
        operation(&mut editor).unwrap();
    }
}

#[test]
fn reconciliation_failure_undoes_the_applied_edit() {
    // The element itself declares `q` differently: the new attribute, the
    // declaration and the rename all roll back.
    let input = "<r xmlns:q='urn:o'><q:a xmlns:z='urn:z'/></r>";
    let mut document = OwnedDocument::parse(input).unwrap();
    let before = snapshot(&document);
    let (r, a) = (find(&document, "/r"), find(&document, "/r/q:a"));
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let conflict = |result: Result<(), EditError>| {
        assert_eq!(result.unwrap_err().kind, EditErrorKind::Namespace);
    };
    let exact = PrefixPolicy::Exact(Some("q"));
    conflict(editor.set_attribute(r, name("x", Some("urn:p"), exact), "1"));
    conflict(editor.declare_namespace(a, Some("q"), "urn:n"));
    conflict(editor.set_name(r, name("r", Some("urn:p"), exact)));
    conflict(editor.remove_attribute(r, Some(XMLNS), "q"));
    assert_eq!(snapshot(&document), before);
}

#[test]
fn namespace_work_bounds_planning_before_publication() {
    let input = "<r xmlns:p='urn:p'><p:a><p:b/><p:c/></p:a><d/></r>";
    let mut document = OwnedDocument::parse(input).unwrap();
    let before = snapshot(&document);
    let (a, d) = (find(&document, "/r/p:a"), find(&document, "/r/d"));
    for work in 0..8 {
        let limits = EditLimits {
            max_namespace_work: work,
            ..EditLimits::default()
        };
        let result = document.edit(limits).unwrap().append_child(d, a);
        if let Err(error) = result {
            assert_eq!(
                error.kind,
                EditErrorKind::LimitExceeded(EditLimit::NamespaceWork)
            );
            assert_eq!(snapshot(&document), before);
        }
    }
}

#[test]
fn compaction_rewrites_every_range_and_drops_garbage() {
    let input =
        "<!DOCTYPE r [<!ENTITY e 'v'>]><?xml-stylesheet x?><r a='&e;' b='2'><s c='3'>t</s></r>";
    let mut document =
        OwnedDocument::parse_with(input.as_bytes(), crate::ParseOptions::default()).unwrap();
    let s = find(&document, "/r/s");
    let r = find(&document, "/r");
    let mut editor = document.edit(EditLimits::default()).unwrap();
    editor
        .set_attribute(s, name("d", None, PrefixPolicy::Automatic), "4")
        .unwrap();
    editor
        .set_attribute(r, name("e", None, PrefixPolicy::Automatic), "5")
        .unwrap();
    editor.remove_attribute(r, None, "b").unwrap();
    let text = find(editor.document(), "/r/s/text()");
    editor.remove_subtree(text).unwrap();
    let before = snapshot(editor.document());
    editor.compact().unwrap();
    assert_eq!(snapshot(editor.document()), before);
    let live: usize = document.attributes.len();
    assert_eq!(live, 4);
    let mut used = 0;
    document.visit_strings(|range| used += range.len());
    assert_eq!(document.pool.0.len(), used);
    assert_eq!(document.edit_owned_bytes, 0);
}
