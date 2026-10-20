//! HTML events (libxml2's SAX callbacks), `HTML_PARSE_HTML5`, fragments,
//! node lines and rendered diagnostics, table-driven.
#![forbid(unsafe_code)]

use std::ops::ControlFlow;

use yaxml::diagnostic::{Domain, Level};
use yaxml::html::{self, DiagnosticKind, Event, EventParser, Options};
use yaxml::{ErrorKind, Limit, Limits, LineIndex, OwnedDocument, OwnedNodeKind};

/// Events as compact strings: `<name a=v b`, `</name`, `"text`, `[cdata`,
/// `!comment`, `#doctype`.
fn events(input: &[u8], flags: u32) -> Vec<String> {
    let mut out = Vec::new();
    let summary = html::parse_events(input, Options::new(flags), |event| {
        out.push(show(&event));
        ControlFlow::Continue(())
    })
    .unwrap();
    assert!(!summary.stopped);
    out
}

fn show(event: &Event<'_>) -> String {
    match event {
        Event::Doctype(doctype) => format!("#{}", doctype.name),
        Event::Start { name, attributes } => {
            let mut out = format!("<{name}");
            for (name, value) in attributes.iter() {
                out += &match value {
                    Some(value) => format!(" {name}={value}"),
                    None => format!(" {name}"),
                };
            }
            out
        }
        Event::End(name) => format!("</{name}"),
        Event::Text(text) => format!("\"{text}"),
        Event::Cdata(text) => format!("[{text}"),
        Event::Comment(text) => format!("!{text}"),
        _ => unreachable!("new event"),
    }
}

#[test]
fn events_follow_libxml2_callbacks() {
    let cases: &[(&str, u32, &[&str])] = &[
        ("", 0, &[]),
        ("x", 0, &["<html", "<body", "\"x", "</body", "</html"]),
        (
            "<p a=1 b checked>t&amp;u",
            html::NOIMPLIED,
            &["<p a=1 b checked", "\"t", "\"&", "\"u", "</p"],
        ),
        (
            "<script>a<b</script>",
            html::NOIMPLIED,
            &["<script", "[a<b", "</script"],
        ),
        ("<!--c--><!DOCTYPE a><!DOCTYPE b>", 0, &["!c", "#a", "#b"]),
        // An unmatched end tag is no event; a misplaced start tag neither.
        (
            "<p></i><html>",
            0,
            &["<html", "<body", "<p", "</p", "</body", "</html"],
        ),
        // Text outside any element reaches the handler, the discarded
        // tag's `>` included (htmlParseElementInternal leaves it).
        ("a<head>b", html::NOIMPLIED, &["\"a", "\">b"]),
        // NOBLANKS installs a no-op ignorableWhitespace.
        (
            "<p>x</p> <p>y",
            html::NOBLANKS | html::NOIMPLIED,
            &["<p", "\"x", "</p", "<p", "\"y", "</p"],
        ),
    ];
    for &(input, flags, expected) in cases {
        assert_eq!(
            events(input.as_bytes(), flags),
            expected,
            "{input:?} {flags:#x}"
        );
    }
}

#[test]
fn html5_reports_tags_as_read() {
    let cases: &[(&str, &[&str])] = &[
        (
            "<p><br>x</q>y</p>z",
            &["<p", "<br", "\"x", "</q", "\"y", "</p", "\"z"],
        ),
        ("<!DOCTYPE HTML><HTML>", &["#html", "<html"]),
        ("<p/></body>", &["<p", "</body"]),
        ("<html><html>", &["<html", "<html"]),
        ("<td>", &["<td"]),
    ];
    for &(input, expected) in cases {
        assert_eq!(events(input.as_bytes(), html::HTML5), expected, "{input:?}");
    }
    // The tree follows the unbalanced calls, as libxml2's SAX2 builder does:
    // `x` goes into `br` (which output omits), `</q>` closes `br`, `</p>`
    // the `p`, and `z` has no parent.
    let parsed = html::parse(b"<p><br>x</q>y</p>z", Options::new(html::HTML5)).unwrap();
    assert_eq!(
        html::save(&parsed.document, false).unwrap(),
        b"<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.0 Transitional//EN\" \
          \"http://www.w3.org/TR/REC-html40/loose.dtd\">\n<p><br>y</p>\n"
    );
}

#[test]
fn handlers_stop_and_push_matches_pull() {
    let input = b"<!DOCTYPE html><title>t</title><p class=a>one<p>two<!--c--></div>";
    let whole = events(input, 0);
    for size in [1, 2, 5, 64] {
        let mut pushed = Vec::new();
        let mut parser = EventParser::new(Options::DEFAULT, |event| {
            pushed.push(show(&event));
            ControlFlow::Continue(())
        })
        .unwrap();
        for chunk in input.chunks(size) {
            parser.feed(chunk).unwrap();
        }
        let summary = parser.finish().unwrap();
        assert_eq!(summary.diagnostics.len(), 1);
        assert_eq!(pushed, whole, "chunks of {size}");
    }
    for stop in 0..whole.len() {
        let mut seen = Vec::new();
        let summary = html::parse_events(input, Options::DEFAULT, |event| {
            seen.push(show(&event));
            if seen.len() > stop {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
        .unwrap();
        assert!(summary.stopped);
        assert_eq!(seen, whole[..=stop]);
    }
}

#[test]
fn event_parsing_fails_like_tree_parsing() {
    let options = |limits| Options::DEFAULT.limits(limits);
    let noop = |_: Event<'_>| ControlFlow::Continue(());
    let unknown = html::parse_events(b"", Options::new(1 << 3), noop).unwrap_err();
    assert_eq!(unknown.kind, ErrorKind::UnsupportedOption);
    assert!(EventParser::new(Options::new(1 << 30), noop).is_err());
    let small = Limits::DEFAULT.max_input_bytes(4);
    let error = html::parse_events(b"<p>hello", options(small), noop).unwrap_err();
    assert_eq!(error.kind, ErrorKind::LimitExceeded(Limit::InputBytes));
    let mut parser = EventParser::new(options(small), noop).unwrap();
    assert!(parser.feed(b"<p>hello").is_err());
    assert_eq!(parser.feed(b"").unwrap_err().kind, ErrorKind::InvalidState);
    assert_eq!(parser.finish().unwrap_err().kind, ErrorKind::InvalidState);
    // Events have no depth ceiling; trees stop at it.
    let deep = "<div>".repeat(300);
    let summary = html::parse_events(deep.as_bytes(), Options::DEFAULT, noop).unwrap();
    assert!(!summary.stopped && summary.diagnostics.is_empty());
    assert!(
        html::parse(deep.as_bytes(), Options::DEFAULT)
            .unwrap()
            .stopped
    );
}

/// The fragment's top-level nodes as the HTML serializer writes them in UTF-8.
fn fragment(input: &[u8], encoding: Option<&str>) -> (String, Vec<DiagnosticKind>) {
    let parsed = html::parse_fragment(input, encoding, Options::DEFAULT).unwrap();
    assert!(parsed.document.doctype().is_none());
    let utf8 = yaxml::save::Options::default().encoding(Some("UTF-8"));
    let saved = yaxml::save::to_vec(&parsed.document, &utf8).unwrap();
    let kinds = parsed.diagnostics.iter().map(|d| d.kind).collect();
    (String::from_utf8_lossy(&saved).trim_end().to_owned(), kinds)
}

#[test]
fn fragments_parse_in_context() {
    type Case<'a> = (&'a [u8], Option<&'a str>, &'a str, &'a [DiagnosticKind]);
    let cases: &[Case<'_>] = &[
        (b"", None, "", &[]),
        (b"  x<b>y", None, "  x<b>y</b>", &[]),
        (b"<!--a-->t", None, "<!--a-->t", &[]),
        (b"<!DOCTYPE html><p>x", None, "<p>x</p>", &[]),
        // No implied elements; `head` is allowed below `#root`, `html` not.
        (
            b"<head><title>t</title></head>",
            None,
            "<head><title>t</title></head>",
            &[],
        ),
        (
            b"<html><p>x</html>",
            None,
            "<p>x</p>",
            &[DiagnosticKind::MisplacedTag],
        ),
        (b"</p>", None, "", &[DiagnosticKind::UnexpectedEndTag]),
        // The context document's encoding, else libxml2's sniffing without a BOM.
        ("<p>\u{e9}".as_bytes(), Some("UTF-8"), "<p>\u{e9}</p>", &[]),
        ("<p>\u{e9}".as_bytes(), None, "<p>\u{c3}\u{a9}</p>", &[]),
        (b"<p>\xe9", Some("iso-8859-1"), "<p>\u{e9}</p>", &[]),
        (b"<p>\xe9", Some("no-such-label"), "<p>\u{e9}</p>", &[]),
    ];
    for &(input, encoding, expected, kinds) in cases {
        let (saved, diagnostics) = fragment(input, encoding);
        assert_eq!(saved, expected, "{:?}", String::from_utf8_lossy(input));
        assert_eq!(diagnostics, kinds, "{:?}", String::from_utf8_lossy(input));
    }
    let sniffed = html::parse_fragment("<p>\u{e9}".as_bytes(), None, Options::DEFAULT).unwrap();
    assert_eq!(sniffed.document.html_encoding(), Some("windows-1252"));
    let text = sniffed
        .document
        .nodes()
        .find(|n| n.kind() == OwnedNodeKind::Text)
        .unwrap();
    assert_eq!(text.text(), Some("\u{c3}\u{a9}"));
    let too_long = html::parse_fragment(
        b"<p>",
        None,
        Options::DEFAULT.limits(Limits::DEFAULT.max_input_bytes(2)),
    );
    assert_eq!(
        too_long.unwrap_err().kind,
        ErrorKind::LimitExceeded(Limit::InputBytes)
    );
    assert!(html::parse_fragment(b"", None, Options::new(1 << 3)).is_err());
}

/// `(name, line)` of every node in preorder.
fn lines(document: &OwnedDocument) -> Vec<(String, Option<usize>)> {
    let index = LineIndex::new(b"");
    document
        .root()
        .descendants()
        .skip(1)
        .map(|node| {
            let name = match node.kind() {
                OwnedNodeKind::Element => node.name().unwrap().qualified().to_owned(),
                OwnedNodeKind::Text => format!("\"{}", node.text().unwrap()),
                kind => format!("{kind:?}"),
            };
            (name, node.line(&index))
        })
        .collect()
}

#[test]
fn nodes_carry_libxml2_lines() {
    let parsed = html::parse(
        b"\n<!DOCTYPE html>\n<p\nclass=a>one\ntwo</i>\nthree<!--\nc-->\n<script>\nx</script>",
        Options::DEFAULT,
    )
    .unwrap();
    let expected: Vec<(String, Option<usize>)> = [
        ("DocumentType", None),
        ("html", Some(3)),
        ("body", Some(3)),
        // Elements where the start tag ends; text where its last run began.
        ("p", Some(4)),
        ("\"one\ntwo\nthree", Some(5)),
        // Comments where they end.
        ("Comment", Some(7)),
        ("\"\n", Some(7)),
        ("script", Some(8)),
        // CDATA takes its previous sibling's or parent's line.
        ("Cdata", Some(8)),
    ]
    .into_iter()
    .map(|(name, line)| (name.to_owned(), line))
    .collect();
    assert_eq!(lines(&parsed.document), expected);
    // Constructed nodes have none.
    let mut document = parsed.document;
    let p = document
        .nodes()
        .find(|n| n.name().is_some_and(|n| n.local() == "p"))
        .unwrap()
        .id();
    let added = document
        .edit(yaxml::EditLimits::default())
        .unwrap()
        .append_comment(p, "b")
        .unwrap();
    assert_eq!(
        document.node(added).unwrap().line(&LineIndex::new(b"")),
        None
    );
}

#[test]
fn diagnostics_render_as_xmllint_does() {
    let cases: &[(&[u8], &str)] = &[
        (
            b"<p>\n<b>x</i>",
            "f:2: HTML parser error : Unexpected end tag : i\n<b>x</i>\n        ^\n",
        ),
        (
            b"<div><b></div>",
            "f:1: HTML parser error : Opening and ending tag mismatch: div and b\n\
             <div><b></div>\n              ^\n",
        ),
        (
            b"<p><html>",
            "f:1: HTML parser error : htmlParseStartTag: misplaced <html> tag\n\
             <p><html>\n        ^\n",
        ),
        (
            b"<meta charset=nope>",
            "f:1: HTML parser error : Unsupported encoding: nope\n\
             <meta charset=nope>\n                  ^\n",
        ),
        (
            b"<meta charset=utf-8>\n<p>ab\xffc",
            "f:2: HTML parser error : Invalid bytes in character encoding\n\
             Bytes: 0x61 0x62 0xFF 0x63\n<p>ab\n   ^\n",
        ),
    ];
    for &(input, expected) in cases {
        let parsed = html::parse(input, Options::DEFAULT).unwrap();
        let rendered: String = parsed.diagnostics.iter().map(|d| d.render("f")).collect();
        assert_eq!(rendered, expected, "{:?}", String::from_utf8_lossy(input));
    }
    let parsed = html::parse(b"<p>\n  </i>", Options::DEFAULT).unwrap();
    let diagnostic = &parsed.diagnostics[0];
    assert_eq!(
        (diagnostic.line, diagnostic.column, diagnostic.offset),
        (2, 7, 10)
    );
    let record = diagnostic.describe();
    assert_eq!(
        (record.domain, record.code, record.level, record.line),
        (Domain::Html, 76, Level::Error, 2)
    );
    assert_eq!(record.message, "Unexpected end tag : i");
    // Depth: a parser-domain fatal error.
    let deep = html::parse("<div>".repeat(300).as_bytes(), Options::DEFAULT).unwrap();
    let record = deep.diagnostics[0].describe();
    assert_eq!(
        (record.domain, record.code, record.level),
        (Domain::Parser, 114, Level::Fatal)
    );
    assert_eq!(
        record.message,
        "Excessive depth in document: 256, use XML_PARSE_HUGE option"
    );
    // libxml2 reports 100 errors at most.
    let many = html::parse("</i>".repeat(150).as_bytes(), Options::DEFAULT).unwrap();
    assert_eq!(many.diagnostics.len(), 100);
}
