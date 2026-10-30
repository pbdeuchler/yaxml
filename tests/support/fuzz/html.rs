//! HTML parsing, push chunking, events, fragments and serialization on
//! arbitrary bytes. Options and a chunk schedule derive from a hash of the
//! input, so every input is a document.

use std::ops::ControlFlow;

use yaxml::encoding::Handler;
use yaxml::html::{self, Event, EventParser, Options, Parsed, PushParser};
use yaxml::{
    EditLimits, Error, ErrorKind, Limit, Limits, LineIndex, OwnedDocument, OwnedNodeKind, Writer,
    c14n, save,
};

const FLAGS: [u32; 8] = [
    html::NOBLANKS,
    html::NOIMPLIED,
    html::NODEFDTD,
    html::HUGE,
    html::IGNORE_ENC,
    html::NOERROR,
    html::HTML5,
    // Accepted flags without effect.
    html::RECOVER
        | html::NOWARNING
        | html::PEDANTIC
        | html::NONET
        | html::COMPACT
        | html::BIG_LINES,
];

const FORCED: [&str; 8] = [
    "UTF-8",
    "ISO-8859-1",
    "windows-1252",
    "US-ASCII",
    "ISO-8859-2",
    "UTF-16",
    "UTF-16LE",
    "UTF-16BE",
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
    // A forced encoding (htmlCtxtReadMemory's `encoding`) half the time.
    let encoding = FORCED
        .get((seed >> 56 & 15) as usize)
        .map(|name| Handler::open(name).expect("built in"));
    Options::default()
        .flags(flags)
        .limits(limits)
        .encoding(encoding)
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

/// Structural invariants of a parsed tree; a fragment's text may be top-level.
fn check(document: &OwnedDocument, options: &Options, fragment: bool) {
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
                assert!(fragment || parent.kind() == OwnedNodeKind::Element);
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
                assert!(!fragment, "a fragment keeps no DOCTYPE");
                assert_eq!(root.first_child().map(|first| first.id()), Some(node.id()));
            }
            kind => panic!("unexpected {kind:?}"),
        }
    }
}

/// No decoding makes more line feeds than the input has 0x0A bytes.
fn last_line(input: &[u8]) -> usize {
    1 + input.iter().filter(|&&byte| byte == b'\n').count()
}

/// Recorded lines: one per parsed element, text and comment, never past the
/// input's last line, and non-decreasing in creation order (libxml2 records
/// the line its input is on, which only moves forward).
fn lines(document: &OwnedDocument, input: &[u8]) -> Vec<Option<usize>> {
    let last = last_line(input);
    let index = LineIndex::html(input, document);
    let lines: Vec<_> = document.nodes().map(|node| node.line(&index)).collect();
    let mut previous = 1;
    for (node, line) in document.nodes().zip(&lines) {
        if matches!(
            node.kind(),
            OwnedNodeKind::Element | OwnedNodeKind::Text | OwnedNodeKind::Comment
        ) {
            let line = line.expect("parsed nodes have lines");
            assert!(
                (previous..=last).contains(&line),
                "line {line} after {previous}"
            );
            previous = line;
        }
    }
    lines
}

/// Diagnostics: a line within the input, and a report naming it.
fn diagnosed(diagnostics: &[html::Diagnostic], lines: &LineIndex<'_>, input: &[u8]) {
    let last = last_line(input);
    for diagnostic in diagnostics {
        let record = diagnostic.describe(lines);
        assert!((1..=last).contains(&record.line), "{diagnostic:?}");
        assert!(record.column >= 1, "{diagnostic:?}");
        let report = diagnostic.render("f", lines);
        assert!(
            report.starts_with(&format!("f:{}: ", record.line)),
            "{report}"
        );
        assert!(report.ends_with("^\n"), "{report:?}");
    }
}

/// Diagnostics with their records and reports, which push must reproduce.
fn reported(diagnostics: &[html::Diagnostic], lines: &LineIndex<'_>) -> Vec<String> {
    diagnostics
        .iter()
        .map(|d| format!("{d:?} {:?} {}", d.describe(lines), d.render("", lines)))
        .collect()
}

fn outcome(parsed: &Parsed, input: &[u8]) -> (Vec<u8>, Vec<String>, bool, Vec<Option<usize>>) {
    (
        html::save(&parsed.document, false).expect("parsed encodings serialize"),
        reported(
            &parsed.diagnostics,
            &LineIndex::html(input, &parsed.document),
        ),
        parsed.stopped,
        lines(&parsed.document, input),
    )
}

/// An event as owned data, text merged as a tree merges it.
#[derive(Debug, PartialEq)]
enum Owned {
    Doctype(String),
    Start(String, Vec<(String, Option<String>)>),
    End(String),
    Text(String, bool),
    Comment(String),
}

fn record(events: &mut Vec<Owned>, event: Event<'_>) {
    let (text, cdata) = match event {
        Event::Text(text) => (text, false),
        Event::Cdata(text) => (text, true),
        Event::Doctype(doctype) => return events.push(Owned::Doctype(doctype.name.to_owned())),
        Event::Start { name, attributes } => {
            return events.push(Owned::Start(name.to_owned(), attributes.to_vec()));
        }
        Event::End(name) => return events.push(Owned::End(name.to_owned())),
        Event::Comment(text) => return events.push(Owned::Comment(text.to_owned())),
        _ => unreachable!("new event"),
    };
    assert!(!text.is_empty(), "empty character data event");
    match events.last_mut() {
        Some(Owned::Text(last, kind)) if *kind == cdata => last.push_str(text),
        _ => events.push(Owned::Text(text.to_owned(), cdata)),
    }
}

/// The tree the SAX2 builder makes from events: (depth, element name or
/// text, cdata) in preorder, text outside any element dropped.
fn replay(events: &[Owned]) -> Vec<(usize, String)> {
    let (mut depth, mut out) = (0_usize, Vec::new());
    for event in events {
        match event {
            Owned::Start(name, _) => {
                out.push((depth, format!("<{name}")));
                depth += 1;
            }
            Owned::End(_) => depth = depth.saturating_sub(1),
            Owned::Text(text, cdata) if depth > 0 => match out.last_mut() {
                Some((at, last))
                    if *at == depth && last.starts_with(if *cdata { 'c' } else { 't' }) =>
                {
                    last.push_str(text);
                }
                _ => out.push((depth, format!("{}{text}", if *cdata { 'c' } else { 't' }))),
            },
            Owned::Comment(text) => out.push((depth, format!("!{text}"))),
            _ => {}
        }
    }
    out
}

/// The same signature from a parsed tree.
fn signature(document: &OwnedDocument) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for node in document.root().descendants().skip(1) {
        let depth = std::iter::successors(node.parent(), |n| n.parent()).count() - 1;
        let item = match node.kind() {
            OwnedNodeKind::Element => format!("<{}", node.name().unwrap().qualified()),
            OwnedNodeKind::Text => format!("t{}", node.text().unwrap()),
            OwnedNodeKind::Cdata => format!("c{}", node.text().unwrap()),
            OwnedNodeKind::Comment => format!("!{}", node.text().unwrap()),
            _ => continue,
        };
        out.push((depth, item));
    }
    out
}

/// Events: push equals pull, a handler can stop, and without limits the
/// SAX2 builder's tree is the one `parse` builds, with the same diagnostics.
fn events(input: &[u8], options: &Options, seed: u64, whole: Option<&Parsed>) {
    let mut pulled = Vec::new();
    let summary = html::parse_events(input, options.clone(), |event| {
        record(&mut pulled, event);
        ControlFlow::Continue(())
    });
    let mut pushed = Vec::new();
    let mut parser = EventParser::new(options.clone(), |event| {
        record(&mut pushed, event);
        ControlFlow::Continue(())
    })
    .unwrap();
    let mut rest = input;
    let mut schedule = seed.rotate_left(17);
    let streamed = loop {
        let size = 1 + (schedule % 13) as usize;
        schedule = schedule.rotate_right(3);
        let (chunk, tail) = rest.split_at(size.min(rest.len()));
        if let Err(error) = parser.feed(chunk) {
            break Err(error);
        }
        rest = tail;
        if rest.is_empty() {
            break parser.finish();
        }
    };
    let (summary, streamed) = match (summary, streamed) {
        (Ok(summary), Ok(streamed)) => (summary, streamed),
        (Err(error), _) | (_, Err(error)) => return allowed(error),
    };
    assert_eq!(pulled, pushed, "events depend on chunking");
    assert_eq!(
        reported(&summary.diagnostics, &summary.line_index(input)),
        reported(&streamed.diagnostics, &streamed.line_index(input))
    );
    assert_eq!(summary.stopped, streamed.stopped);
    diagnosed(&summary.diagnostics, &summary.line_index(input), input);
    // Without a tree, areBlanks keeps what NOBLANKS drops from one.
    if let Some(whole) = whole
        && !whole.stopped
        && !summary.stopped
        && options.flags & html::NOBLANKS == 0
    {
        assert_eq!(
            replay(&pulled),
            signature(&whole.document),
            "SAX2 tree differs"
        );
        assert_eq!(summary.diagnostics, whole.diagnostics);
    }
    // A handler that breaks gets nothing more.
    let stop = (seed >> 53) as usize % (pulled.len() + 1);
    let mut seen = 0;
    let stopped = html::parse_events(input, options.clone(), |_| {
        seen += 1;
        if seen > stop {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    if let Ok(stopped) = stopped {
        assert!(seen <= stop + 1);
        assert!(stopped.stopped || seen <= stop);
    }
}

/// Fragments: tree invariants without a DOCTYPE, and the whole input once
/// more as content of a document in the encoding the first parse chose.
fn fragment(input: &[u8], options: &Options, whole: Option<&Parsed>) {
    let encoding = whole.and_then(|whole| whole.document.html_encoding());
    match html::parse_fragment(input, encoding, options.clone()) {
        Ok(parsed) => {
            check(&parsed.document, options, true);
            lines(&parsed.document, input);
            let index = LineIndex::html(input, &parsed.document);
            diagnosed(&parsed.diagnostics, &index, input);
            assert!(html::save(&parsed.document, false).is_ok());
        }
        Err(error) => allowed(error),
    }
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

/// `save::to_vec` on an HTML document: the HTML serializer unless `AS_XML`
/// or `XHTML` asks for XML output. Saved as HTML, the top-level subtrees
/// concatenate to the document's output after its DOCTYPE.
fn saved_as_xml(document: &OwnedDocument, seed: u64) {
    let format = (seed >> 50 & 1) as u32 * save::FORMAT;
    let html = save::to_vec(
        document,
        &save::Options::new(format | ((seed >> 51 & 1) as u32 * save::AS_HTML)),
    );
    assert_eq!(html, html::save(document, format != 0));
    // libxml2 adds `<meta charset>` to the first head of a top-level `html`
    // only, unless that head has children (`metaHead` is reset only after
    // them), so separate subtrees can differ when an HTML5 tree has several.
    let elements = document
        .root()
        .children()
        .filter(|node| node.kind() == OwnedNodeKind::Element)
        .count();
    // Wide output (UTF-16, UCS-4) starts each subtree with a BOM or encodes
    // the newline below differently; labels keep their declared case
    // (`<meta charset=utf-16>`).
    let wide = (document.html_encoding()).is_some_and(|name| {
        let mut newline = Vec::new();
        yaxml::encoding::Handler::open(name).is_ok_and(|handler| {
            let encoded = handler.encode("\n", &mut newline).is_ok();
            !handler.bom().is_empty() || !encoded || newline != b"\n"
        })
    });
    if let Ok(html) = &html
        && document.root().first_child().is_some()
        && elements <= 1
        && !wide
    {
        let mut subtrees = Vec::new();
        for node in document.root().children() {
            let flags = format | ((seed >> 52 & 1) as u32 * save::AS_HTML);
            subtrees.extend(save::to_vec(node, &save::Options::new(flags)).unwrap());
        }
        subtrees.push(b'\n');
        assert!(
            html.ends_with(&subtrees),
            "subtrees differ from the document"
        );
    }
    if let Some(node) = document.root().first_child() {
        let _ = save::to_vec(node, &save::Options::new(save::AS_XML | format));
    }
    for flags in [save::AS_XML, save::XHTML, save::XHTML | save::NO_EMPTY] {
        if let Err(error) = save::to_vec(document, &save::Options::new(flags | format)) {
            let lossy = [ErrorKind::InvalidCharacter, ErrorKind::UnsupportedEncoding];
            assert!(lossy.contains(&error.kind), "{error:?}");
        }
    }
}

/// Nodes the parser never builds (PIs, text in raw-text elements, quotes in
/// values) serialize, and the output reparses.
fn edited(input: &[u8], options: &Options) {
    let mut document = html::parse(input, options.clone())
        .expect("reparse")
        .document;
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

/// The input's hash, from which options and chunk schedules derive.
fn seed(input: &[u8]) -> u64 {
    input.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
    })
}

/// Target `html_events`: events against the tree (`HTML5` among the options).
pub(super) fn exercise_events(input: &[u8]) {
    let seed = seed(input);
    let options = options(seed);
    events(
        input,
        &options,
        seed,
        html::parse(input, options.clone()).ok().as_ref(),
    );
}

/// Target `html_fragment`: in-context parsing.
pub(super) fn exercise_fragment(input: &[u8]) {
    let options = options(seed(input));
    fragment(
        input,
        &options,
        html::parse(input, options.clone()).ok().as_ref(),
    );
}

pub(super) fn exercise(input: &[u8]) {
    let seed = seed(input);
    let options = options(seed);
    if seed >> 40 & 63 == 0 {
        // Unknown option bits are rejected up front.
        let unknown = Options::new(options.flags | 1 << 3);
        let unsupported = Err(ErrorKind::UnsupportedOption);
        assert_eq!(
            html::parse(input, unknown.clone())
                .map(drop)
                .map_err(|e| e.kind),
            unsupported
        );
        assert_eq!(
            PushParser::new(unknown).map(drop).map_err(|e| e.kind),
            unsupported
        );
    }
    let whole = html::parse(input, options.clone());
    let mut parser = PushParser::new(options.clone()).unwrap();
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
            check(&whole.document, &options, false);
            let index = LineIndex::html(input, &whole.document);
            diagnosed(&whole.diagnostics, &index, input);
            assert_eq!(
                outcome(whole, input),
                outcome(pushed, input),
                "push result depends on chunking: {:?} {options:?}",
                String::from_utf8_lossy(input)
            );
            assert!(html::to_html(&whole.document).is_ok());
            xml_output(&whole.document);
            saved_as_xml(&whole.document, seed);
            let saved = html::save(&whole.document, true).unwrap();
            // Saved output reparses; a <meta> the first parse ignored can now
            // name an encoding this crate rejects.
            if let Err(error) = html::parse(&saved, Options::new(options.flags & html::HUGE)) {
                allowed(error);
            }
            if seed >> 46 & 3 == 0 {
                edited(input, &options);
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
    use super::{exercise, exercise_events, exercise_fragment};

    /// The prescan stops at NUL, which output drops; the reparse then finds
    /// an encoding this crate rejects.
    #[test]
    fn reparse_may_meet_a_rejected_prescan_label() {
        exercise(include_bytes!(
            "../../fixtures/fuzz-regressions/html/reparse-prescan.html"
        ));
    }

    #[test]
    fn html5_tree_subtrees() {
        exercise(include_bytes!(
            "../../fixtures/fuzz-regressions/html/html5-subtrees.html"
        ));
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
            b"<br/>",
            b"</q>",
            b"<html>",
            b"\n",
            b"\xff",
            b"<meta charset=iso-8859-2>",
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
            exercise_events(&input);
            exercise_fragment(&input);
        }
    }
}
