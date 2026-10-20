//! DTD validation over both DOMs and over parser events. A testcase is an XML
//! document, optionally preceded by external DTD text and a NUL byte
//! (`xmllint --dtdvalid`). A document that does not start with `<` becomes an
//! internal subset of a fixed document, so mutations reach declarations and
//! content models. Stream validation must report the tree'"'"'s diagnostics.

use std::collections::HashMap;
use std::hash::Hash;

use std::ops::ControlFlow;

use yaxml::valid::{self, Code, Diagnostic, Dtd, StreamValidator};
use yaxml::{BufferedReader, Document, OwnedDocument, OwnedNodeKind, PushParser, Reader};

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
    let separate = match dtd {
        Some(text) => Dtd::parse(text).ok(),
        None => borrowed.dtd().cloned(),
    };
    compare(&borrowed, &owned, separate.as_ref());
    stream(&xml, &owned);
}

/// A diagnostic both modes word alike: code, element index, offset and
/// message, without the content-model and missing-DTD wording.
fn neutral(code: Code, node: Option<usize>, offset: Option<usize>, message: &str) -> Projection {
    let worded = !matches!(code, Code::ContentModel | Code::NoDtd);
    let message = if worded {
        message.to_owned()
    } else {
        String::new()
    };
    (
        code as u16,
        node,
        offset.filter(|_| code != Code::NoDtd),
        message,
    )
}

/// Validates `xml`'"'"'s events from every reader: the buffered reader, passing its
/// DTD after each event, reports what the tree reports (unless the DTD
/// declares nothing: libxml2'"'"'s parser then validates nothing); the slice
/// reader and the push parser, which use the internal subset, agree with it
/// on documents without parameter-entity references.
fn stream(xml: &[u8], owned: &OwnedDocument) {
    let mut reader = BufferedReader::new(xml);
    let mut validator = StreamValidator::new();
    while let Ok(Some((event, origin))) = reader.read_event() {
        validator.event(&event, origin);
        if let Some(dtd) = reader.dtd() {
            validator.set_dtd(dtd);
        }
    }
    let pending = validator.diagnostics().to_vec();
    let diagnostics = validator.finish();
    assert!(
        diagnostics.starts_with(&pending),
        "finish rewrote reported diagnostics"
    );
    let lines = yaxml::LineIndex::new(xml);
    for diagnostic in owned.validate().iter() {
        super::check_described(diagnostic.describe(&lines), &lines);
    }
    let elements: Vec<_> = (owned.nodes())
        .filter(|node| node.kind() == OwnedNodeKind::Element)
        .map(|node| node.id())
        .collect();
    let offset = |&index: &usize| {
        let element = owned.node(*elements.get(index).expect("an element"))?;
        element.range().map(|range| range.start)
    };
    check(&diagnostics, offset);
    let index = |id| elements.iter().position(|&element| element == id);
    let mut tree: Vec<_> = (owned.validate().iter())
        .map(|d| neutral(d.code, d.node.and_then(index), d.offset, &d.message))
        .collect();
    let mut ours: Vec<_> = (diagnostics.iter())
        .map(|d| neutral(d.code, d.node, d.offset, &d.message))
        .collect();
    tree.sort();
    ours.sort();
    let excused = owned.dtd().is_some() && diagnostics.iter().any(|d| d.code == Code::NoDtd);
    if !excused {
        assert_eq!(tree, ours, "stream and tree validation agree");
    }

    let Ok(mut reader) = Reader::new(xml) else {
        return;
    };
    let mut slice = StreamValidator::new();
    while let Ok(Some(event)) = reader.read_event() {
        slice.event(&event, reader.event_origin().expect("event origin"));
    }
    let mut push = StreamValidator::new();
    let mut parser = PushParser::new();
    let fed = xml.chunks(7).try_for_each(|chunk| {
        parser
            .feed(chunk, |event, origin| {
                push.event(event, origin);
                ControlFlow::Continue(())
            })
            .map(drop)
    });
    if fed.is_ok() {
        let _ = parser.finish(|event, origin| {
            push.event(event, origin);
            ControlFlow::Continue(())
        });
    }
    let (slice, push) = (slice.finish(), push.finish());
    check(&slice, offset);
    if fed.is_ok() {
        assert_eq!(slice, push, "slice and push events validate alike");
    }
}

/// Validates both DOMs of one document by its own DTD and by `separate`:
/// identical projected diagnostics, determinism and `is_valid` bookkeeping.
pub(super) fn compare(borrowed: &Document<'_>, owned: &OwnedDocument, separate: Option<&Dtd>) {
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

    if let Some(dtd) = separate {
        assert!(dtd.name().is_none_or(|name| !name.is_empty()));
        let diagnostics = valid::validate(borrowed, dtd);
        check(&diagnostics, offset);
        let owned_diagnostics = valid::validate(owned, dtd);
        check(&owned_diagnostics, owned_offset);
        assert_eq!(
            project(diagnostics, &ranks),
            project(owned_diagnostics, &owned_ranks)
        );
    }
}
