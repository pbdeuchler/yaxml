//! HTML parsing, push chunking and serialization on arbitrary bytes. Options
//! and a chunk schedule derive from a hash of the input, so every input is a
//! document.

use yaxml::html::{self, Options, Parsed, PushParser};
use yaxml::{Error, ErrorKind, Limit, Limits, OwnedDocument, OwnedNodeKind, Writer, c14n};

const FLAGS: [u32; 6] = [
    html::NOBLANKS,
    html::NOIMPLIED,
    html::NODEFDTD,
    html::HUGE,
    html::IGNORE_ENC,
    html::NOERROR,
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
        limits.max_events = 4 + (seed >> 21 & 63) as usize;
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

pub(super) fn exercise(input: &[u8]) {
    let seed = input.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
    });
    let options = options(seed);
    let whole = html::parse(input, options);
    let mut parser = PushParser::new(options).unwrap();
    let mut rest = input;
    let mut schedule = seed;
    let pushed = loop {
        let size = 1 + (schedule % 17) as usize;
        schedule = schedule.rotate_right(5);
        let (chunk, tail) = rest.split_at(size.min(rest.len()));
        if let Err(error) = parser.feed(chunk) {
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
        }
        (Err(whole), Err(pushed)) => {
            allowed(*whole);
            allowed(*pushed);
            assert_eq!(whole.kind, pushed.kind);
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
