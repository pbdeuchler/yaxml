//! Owned storage must preserve the borrowing tree after source independence.

use yaxml::{Document, Error, ErrorKind, Limit, Limits, NodeKind, OwnedDocument, OwnedNodeKind};

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
        }
        Err(error) => {
            assert!(error.offset <= input.len());
            assert!(!error.to_string().is_empty());
            // Pool reservations share the normalization budget. They can fail
            // before the borrowing reader reaches a later syntax error.
            if error.kind != ErrorKind::LimitExceeded(Limit::OwnedBytes) {
                assert_eq!(Some(error), borrowed.err(), "owned parse differs");
            }
        }
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
    check_nodes(owned, borrowed, |range| range);
}

pub(super) fn check_nodes(
    owned: &OwnedDocument,
    borrowed: &Document<'_>,
    mut source_range: impl FnMut(std::ops::Range<usize>) -> std::ops::Range<usize>,
) {
    assert_eq!(owned.root().id(), borrowed.root().id());
    assert_eq!(owned.root_element().id(), borrowed.root_element().id());
    assert_eq!(owned.nodes().len(), borrowed.nodes().len());
    for (actual, expected) in owned.nodes().zip(borrowed.nodes()) {
        assert_eq!(actual.id(), expected.id());
        assert_eq!(actual.range(), source_range(expected.range()));
        assert!(
            actual.name().as_ref() == expected.name(),
            "owned name changed"
        );
        assert!(actual.text() == expected.text(), "owned payload changed");
        let mut attributes = actual.attributes();
        assert_eq!(attributes.len(), expected.attributes().len());
        for expected in expected.attributes() {
            let actual = attributes.next().expect("owned attribute missing");
            assert!(actual == *expected, "owned attribute changed");
        }
        assert!(attributes.next().is_none());
        let (kind, target) = match expected.kind() {
            NodeKind::Document => (OwnedNodeKind::Document, None),
            NodeKind::Element(_) => (OwnedNodeKind::Element, None),
            NodeKind::Text(_) => (OwnedNodeKind::Text, None),
            NodeKind::Cdata(_) => (OwnedNodeKind::Cdata, None),
            NodeKind::Comment(_) => (OwnedNodeKind::Comment, None),
            NodeKind::ProcessingInstruction { target, .. } => {
                (OwnedNodeKind::ProcessingInstruction, Some(*target))
            }
        };
        assert_eq!(actual.kind(), kind);
        assert!(
            actual.processing_instruction_target() == target,
            "owned PI target changed"
        );
        assert_eq!(
            actual.parent().map(|n| n.id()),
            expected.parent().map(|n| n.id())
        );
        assert_eq!(
            actual.first_child().map(|n| n.id()),
            expected.first_child().map(|n| n.id())
        );
        assert_eq!(
            actual.last_child().map(|n| n.id()),
            expected.last_child().map(|n| n.id())
        );
        assert_eq!(
            actual.next_sibling().map(|n| n.id()),
            expected.next_sibling().map(|n| n.id())
        );
        assert!(
            actual
                .children()
                .map(|n| n.id())
                .eq(expected.children().map(|n| n.id()))
        );
        assert_eq!(actual.descendants().len(), expected.descendants().len());
        assert!(
            actual
                .descendants()
                .map(|n| n.id())
                .eq(expected.descendants().map(|n| n.id()))
        );
        assert_eq!(owned.node(actual.id()).unwrap().id(), actual.id());
    }
    if let Some(last) = owned.nodes().last() {
        let small = OwnedDocument::parse("<r/>").unwrap();
        if owned.nodes().len() > small.nodes().len() {
            assert!(small.node(last.id()).is_none());
        }
    }
}
