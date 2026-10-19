//! DTD validation over both DOMs. A testcase is an XML document, optionally
//! preceded by external DTD text and a NUL byte (`xmllint --dtdvalid`). A
//! document that does not start with `<` becomes an internal subset of a
//! fixed document, so mutations reach declarations and content models.

use std::collections::HashMap;
use std::hash::Hash;

use yaxml::valid::{self, Diagnostic, Dtd};
use yaxml::{Document, OwnedDocument};

const CONTENT: &[u8] = b"]><r><a/>t<b a='x' i='y'> <a/></b><![CDATA[c]]><!--k--></r>";

/// A DOM-independent diagnostic: the node becomes its preorder rank.
type Projection = (u16, Option<usize>, Option<usize>, String);

fn project<Id: Copy + Eq + Hash>(
    diagnostics: Vec<Diagnostic<Id>>,
    ranks: &HashMap<Id, usize>,
) -> Vec<Projection> {
    diagnostics
        .into_iter()
        .map(|diagnostic| {
            let rank = diagnostic.node.map(|node| ranks[&node]);
            (
                diagnostic.code as u16,
                rank,
                diagnostic.offset,
                diagnostic.message,
            )
        })
        .collect()
}

/// Checks validity bookkeeping that holds for any diagnostics list.
fn check<Id: std::fmt::Debug>(
    diagnostics: &[Diagnostic<Id>],
    offset: impl Fn(&Id) -> Option<usize>,
) {
    for diagnostic in diagnostics {
        assert!(!diagnostic.message.is_empty());
        if let Some(node) = &diagnostic.node {
            assert_eq!(diagnostic.offset, offset(node), "{diagnostic:?}");
        }
    }
    let errors = diagnostics.iter().any(|d| !d.code.is_warning());
    assert_eq!(valid::is_valid(diagnostics), !errors);
}

pub fn exercise(input: &[u8]) {
    let (dtd, xml) = match input.iter().position(|&byte| byte == 0) {
        Some(at) => (Some(&input[..at]), &input[at + 1..]),
        None => (None, input),
    };
    let xml = match xml.first() {
        Some(b'<') => xml.to_vec(),
        _ => [b"<!DOCTYPE r [".as_slice(), xml, CONTENT].concat(),
    };
    let (Ok(borrowed), Ok(owned)) = (
        Document::parse_bytes(&xml),
        OwnedDocument::parse_bytes(&xml),
    ) else {
        return;
    };
    let ranks: HashMap<_, _> = borrowed
        .nodes()
        .enumerate()
        .map(|(i, n)| (n.id(), i))
        .collect();
    let owned_ranks: HashMap<_, _> = owned
        .nodes()
        .enumerate()
        .map(|(i, n)| (n.id(), i))
        .collect();
    let offset = |id: &yaxml::NodeId| borrowed.node(*id)?.range().map(|range| range.start);
    let owned_offset = |id: &yaxml::OwnedNodeId| owned.node(*id)?.range().map(|range| range.start);

    let diagnostics = borrowed.validate();
    check(&diagnostics, offset);
    assert_eq!(
        diagnostics,
        borrowed.validate(),
        "validation is deterministic"
    );
    let owned_diagnostics = owned.validate();
    check(&owned_diagnostics, owned_offset);
    assert_eq!(
        project(diagnostics, &ranks),
        project(owned_diagnostics, &owned_ranks),
        "both DOMs report the same diagnostics"
    );

    let separate = match dtd {
        Some(text) => Dtd::parse(text).ok(),
        None => borrowed.dtd().cloned(),
    };
    if let Some(dtd) = separate {
        let diagnostics = valid::validate(&borrowed, &dtd);
        check(&diagnostics, offset);
        let owned_diagnostics = valid::validate(&owned, &dtd);
        check(&owned_diagnostics, owned_offset);
        assert_eq!(
            project(diagnostics, &ranks),
            project(owned_diagnostics, &owned_ranks)
        );
    }
}
