//! HTML parsing, push chunking and serialization on arbitrary bytes. Options
//! and a chunk schedule derive from a hash of the input, so every input is a
//! document.

use yaxml::html::{self, Options, Parsed, PushParser};
use yaxml::{
    EditLimits, Error, ErrorKind, Limit, Limits, OwnedDocument, OwnedNodeKind, Writer, c14n,
};

const FLAGS: [u32; 7] = [
    html::NOBLANKS,
    html::NOIMPLIED,
    html::NODEFDTD,
    html::HUGE,
    html::IGNORE_ENC,
    html::NOERROR,
    // Accepted flags without effect.
    html::RECOVER
        | html::NOWARNING
        | html::PEDANTIC
        | html::NONET
        | html::COMPACT
        | html::BIG_LINES,
];

fn options(seed: u64) -> Options {
    let flags = FLAGS
        .iter()
        .enumerate()
        .filter(|(index, _)| seed >> index & 1 == 1)
        .fold(0, |flags, (_, flag)| flags | flag);
    let mut limits = Limits::default();
    if seed >> 8 & 3 == 0 {
        // Small ceilings reach the halting and failing limit paths.
        limits.max_depth = 1 + (seed >> 10 & 7) as usize;
        limits.max_token_bytes = 1 + (seed >> 13 & 63) as usize;
        limits.max_attributes = 1 + (seed >> 19 & 3) as usize;
        limits.max_events = (seed >> 21 & 63) as usize;
        if seed >> 37 & 3 == 0 {
            limits.max_owned_bytes = (seed >> 39 & 1023) as usize;
        }
        if seed >> 27 & 3 == 0 {
            limits.max_input_bytes = (seed >> 29 & 255) as usize;
        }
    }
    Options { flags, limits }
}

fn allowed(error: Error) {
    assert!(
        matches!(
            error.kind,
            ErrorKind::UnsupportedEncoding
                | ErrorKind::LimitExceeded(
                    Limit::InputBytes | Limit::Attributes | Limit::Events | Limit::OwnedBytes
                )
        ),
        "unexpected HTML failure {error:?}"
    );
}

/// Structural invariants of a parsed tree.
fn check(document: &OwnedDocument, options: Options) {
    assert!(document.is_html());
    let root = document.root();
    let nodes: Vec<_> = root.descendants().collect();
    assert_eq!(nodes.len(), document.nodes().len());
    let max_depth = options
        .limits
        .max_depth
        .max(if options.flags & html::HUGE != 0 {
            2048
        } else {
            0
        });
    for node in &nodes[1..] {
        let parent = node.parent().expect("non-root nodes have parents");
        assert!(parent.children().any(|child| child.id() == node.id()));
        let mut depth = 0;
        let mut ancestor = node.parent();
        while let Some(next) = ancestor {
            depth += 1;
            ancestor = next.parent();
        }
        match node.kind() {
            OwnedNodeKind::Element => {
                assert!(depth <= max_depth);
                assert!(!node.name().unwrap().qualified().is_empty());
                for (index, attribute) in node.attributes().enumerate() {
                    assert!(node.attribute_has_value(index) || attribute.value.is_empty());
                }
            }
            OwnedNodeKind::Text | OwnedNodeKind::Cdata => {
                assert!(!node.text().unwrap().is_empty(), "empty character data");
                assert_eq!(parent.kind(), OwnedNodeKind::Element);
                let previous = parent
                    .children()
                    .take_while(|child| child.id() != node.id())
                    .last();
                assert!(
                    previous.is_none_or(|previous| previous.kind() != node.kind()),
                    "unmerged text"
                );
            }
            OwnedNodeKind::Comment => {}
            OwnedNodeKind::DocumentType => {
                assert_eq!(root.first_child().map(|first| first.id()), Some(node.id()));
            }
            kind => panic!("unexpected {kind:?}"),
        }
    }
}

fn outcome(parsed: &Parsed) -> (Vec<u8>, Vec<html::Diagnostic>, bool) {
    (
        html::save(&parsed.document, false).expect("parsed encodings serialize"),
        parsed.diagnostics.clone(),
        parsed.stopped,
    )
}

/// XML output of an HTML tree is well-formed XML or an error.
fn xml_output(document: &OwnedDocument) {
    let mut writer = Writer::new(Vec::new());
    if writer.write_owned_document(document).is_ok() {
        let output = String::from_utf8(writer.into_inner()).unwrap();
        assert!(
            OwnedDocument::parse(&output).is_ok(),
            "writer emitted {output:?}"
        );
    }
    let mut options = c14n::Options::new(c14n::Mode::C14n10);
    options.with_comments = true;
    if let Ok(output) = c14n::to_string(document, &options)
        && document
            .root()
            .children()
            .any(|node| node.kind() == OwnedNodeKind::Element)
    {
        let parsed = OwnedDocument::parse(&output);
        assert!(parsed.is_ok(), "c14n emitted {output:?}: {parsed:?}");
    }
}

/// Nodes the parser never builds (PIs, text in raw-text elements, quotes in
/// values) serialize, and the output reparses.
fn edited(input: &[u8], options: Options) {
    let mut document = html::parse(input, options).expect("reparse").document;
    let elements: Vec<_> = (document.nodes())
        .filter(|node| node.kind() == OwnedNodeKind::Element)
        .take(8)
        .map(|node| {
            let first = node.attributes().next().map(|attribute| {
                let name = attribute.name;
                (name.namespace().map(str::to_owned), name.local().to_owned())
            });
            (node.id(), first)
        })
        .collect();
    let mut editor = document.edit(EditLimits::default()).expect("fresh editor");
    for (index, (element, first)) in elements.into_iter().enumerate() {
        let _ = editor.append_text(element, "a<&>\"'\u{e9}\u{4e00} ");
        let _ = editor.append_processing_instruction(element, "pi", ["", "d"][index % 2]);
        let _ = editor.append_comment(element, "c");
        if let Some((namespace, local)) = &first {
            let _ =
                editor.set_attribute_value(element, namespace.as_deref(), local, " \u{e9}\"'&<x y");
        }
    }
    for format in [false, true] {
        let saved = html::save(&document, format).expect("parsed encodings serialize");
        if let Err(error) = html::parse(&saved, Options::new(options.flags & html::HUGE)) {
            allowed(error);
        }
    }
    assert!(html::to_html(&document).is_ok());
    xml_output(&document);
}

pub(super) fn exercise(input: &[u8]) {
    let seed = input.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
    });
    let options = options(seed);
    if seed >> 40 & 63 == 0 {
        // libxml2's HTML5 mode is rejected up front.
        let html5 = Options::new(options.flags | html::HTML5);
        let unsupported = Err(ErrorKind::UnsupportedOption);
        assert_eq!(
            html::parse(input, html5).map(drop).map_err(|e| e.kind),
            unsupported
        );
        assert_eq!(
            PushParser::new(html5).map(drop).map_err(|e| e.kind),
            unsupported
        );
    }
    let whole = html::parse(input, options);
    let mut parser = PushParser::new(options).unwrap();
    let mut rest = input;
    let mut schedule = seed;
    let pushed = loop {
        let size = 1 + (schedule % 17) as usize;
        schedule = schedule.rotate_right(5);
        let (chunk, tail) = rest.split_at(size.min(rest.len()));
        if let Err(error) = parser.feed(chunk) {
            // Failures are terminal.
            let state = Err(ErrorKind::InvalidState);
            assert_eq!(parser.feed(b"").map_err(|e| e.kind), state);
            assert_eq!(parser.finish().map(drop).map_err(|e| e.kind), state);
            break Err(error);
        }
        rest = tail;
        if rest.is_empty() {
            break parser.finish();
        }
    };
    match (&whole, &pushed) {
        (Ok(whole), Ok(pushed)) => {
            check(&whole.document, options);
            assert_eq!(
                outcome(whole),
                outcome(pushed),
                "push result depends on chunking: {:?} {options:?}",
                String::from_utf8_lossy(input)
            );
            assert!(html::to_html(&whole.document).is_ok());
            xml_output(&whole.document);
            let saved = html::save(&whole.document, true).unwrap();
            // Saved output reparses; a <meta> the first parse ignored can now
            // name an encoding this crate rejects.
            if let Err(error) = html::parse(&saved, Options::new(options.flags & html::HUGE)) {
                allowed(error);
            }
            if seed >> 46 & 3 == 0 {
                edited(input, options);
            }
        }
        (Err(whole), Err(pushed)) => {
            allowed(*whole);
            allowed(*pushed);
            // Pull counts all input up front; push may fail first on a chunk.
            let input = ErrorKind::LimitExceeded(Limit::InputBytes);
            assert!(whole.kind == pushed.kind || whole.kind == input);
        }
        _ => panic!(
            "pull {:?} and push {:?} disagree",
            whole.as_ref().err(),
            pushed.as_ref().err()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::exercise;

    /// The prescan stops at NUL, which output drops; the reparse then finds
    /// an encoding this crate rejects.
    #[test]
    fn reparse_may_meet_a_rejected_prescan_label() {
        exercise(include_bytes!("regressions/html/reparse-prescan.html"));
    }

    #[test]
    fn token_soup_keeps_every_invariant() {
        const TOKENS: &[&[u8]] = &[
            b"<p>",
            b"</p>",
            b"<div>",
            b"</div>",
            b"<table>",
            b"<td>",
            b"<script>",
            b"</script>",
            b"<!--",
            b"-->",
            b"<!DOCTYPE html>",
            b"&amp;",
            b"&#x41;",
            b"&not",
            b"<a href='",
            b"'>",
            b"<meta charset=utf-8>",
            b"\xe9",
            b"\xc3\xa9",
            b"\x00",
            b"\r\n",
            b" ",
            b"<",
            b"</",
            b"<head>",
            b"<body>",
            b"<title>",
            b"<textarea>",
            b"<plaintext>",
            b"x",
        ];
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..1000 {
            let mut input = Vec::new();
            for _ in 0..1 + state % 24 {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                input.extend_from_slice(TOKENS[(state >> 33) as usize % TOKENS.len()]);
            }
            exercise(&input);
        }
    }
}
