//! HTML parser and serializer behavior (libxml2 2.15.4), table-driven.
#![forbid(unsafe_code)]

#[path = "support/html_corpus.rs"]
mod corpus;

use yaxml::html::{self, Diagnostic, DiagnosticKind, Options, Parsed, PushParser};
use yaxml::{EditLimits, ErrorKind, Limit, Limits, OwnedNodeKind};

fn parse(input: &[u8], flags: u32) -> Parsed {
    html::parse(input, Options::new(flags)).unwrap()
}

/// The xmllint-style serialization without the default DOCTYPE line.
fn shape(input: &[u8], flags: u32) -> String {
    let parsed = parse(input, flags | html::NODEFDTD);
    let output = html::save(&parsed.document, false).unwrap();
    String::from_utf8(output)
        .unwrap()
        .trim_end_matches('\n')
        .to_owned()
}

fn kinds(diagnostics: &[Diagnostic]) -> Vec<DiagnosticKind> {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.kind)
        .collect()
}

fn check(cases: &[(&str, &str)], flags: u32) {
    for (input, expected) in cases {
        assert_eq!(shape(input.as_bytes(), flags), *expected, "{input:?}");
    }
}

#[test]
fn implied_elements() {
    check(
        &[
            ("", ""),
            ("x", "<html><body>x</body></html>"),
            (
                "<title>t</title>x",
                "<html><head><title>t</title></head><body>x</body></html>",
            ),
            (
                "<meta><p>",
                "<html><head><meta></head><body><p></p></body></html>",
            ),
            (
                "<head></head>  x",
                "<html><head></head>  <body>x</body></html>",
            ),
            (
                "<frameset></frameset>",
                "<html><frameset></frameset></html>",
            ),
            ("<!--c--><p>", "<!--c--><html><body><p></p></body></html>"),
            ("<body>x</body>y", "<html><body>x</body>y</html>"),
        ],
        0,
    );
}

#[test]
fn no_implied_elements() {
    check(
        &[
            ("x<p>a</p>b<p>c", "<p>a</p><p>c</p>"),
            ("<head><title>t</title>", "<title>t</title>"),
            ("<html><b>x</b></html>", "<html><b>x</b></html>"),
        ],
        html::NOIMPLIED,
    );
}

#[test]
fn start_tags_auto_close() {
    check(
        &[
            ("<p>a<p>b", "<html><body><p>a</p><p>b</p></body></html>"),
            (
                "<li>a<li>b",
                "<html><body><li>a</li><li>b</li></body></html>",
            ),
            (
                "<dt>a<dd>b<dt>c",
                "<html><body><dt>a</dt><dd>b</dd><dt>c</dt></body></html>",
            ),
            (
                "<p>a<div>b",
                "<html><body><p>a</p><div>b</div></body></html>",
            ),
            ("<b>a<p>b", "<html><body><b>a</b><p>b</p></body></html>"),
            (
                "<p>a<span>b",
                "<html><body><p>a<span>b</span></p></body></html>",
            ),
            (
                "<td>a<td>b",
                "<html><body><td>a</td><td>b</td></body></html>",
            ),
            ("<a>1<a>2", "<html><body><a>1</a><a>2</a></body></html>"),
            (
                "<option>a<option>b",
                "<html><body><option>a</option><option>b</option></body></html>",
            ),
        ],
        0,
    );
}

#[test]
fn end_tags_close_by_priority() {
    check(
        &[
            ("<b><i>x</b>y", "<html><body><b><i>x</i></b>y</body></html>"),
            (
                "<div><span>x</div>y",
                "<html><body><div><span>x</span></div>y</body></html>",
            ),
            // td (160) cannot close past table (190).
            (
                "<div><table><tr><td>x</div>y",
                "<html><body><div><table><tr><td>xy</td></tr></table></div></body></html>",
            ),
            ("</p>x", "<html><body>x</body></html>"),
            ("<p>a</span>b", "<html><body><p>ab</p></body></html>"),
        ],
        0,
    );
    let parsed = parse(b"<b><i>x</b></q><div></div></div>", 0);
    assert_eq!(
        kinds(&parsed.diagnostics),
        [
            DiagnosticKind::TagMismatch,
            DiagnosticKind::UnexpectedEndTag,
            DiagnosticKind::UnexpectedEndTag,
        ]
    );
}

#[test]
fn misplaced_structure_tags_are_dropped() {
    check(
        &[
            ("<p><html a=1>x", "<html><body><p>x</p></body></html>"),
            (
                "<body><body>x</body></body>y",
                "<html><body>x</body>y</html>",
            ),
            ("<div><head>x", "<html><body><div>x</div></body></html>"),
            ("<div><body/>x", "<html><body><div></div>x</body></html>"),
        ],
        0,
    );
    assert_eq!(
        kinds(&parse(b"<p><head><html>", 0).diagnostics),
        [DiagnosticKind::MisplacedTag, DiagnosticKind::MisplacedTag]
    );
    assert!(parse(b"<p><head>", html::NOERROR).diagnostics.is_empty());
}

#[test]
fn character_references() {
    let cases: &[(&str, &str)] = &[
        ("&amp;&lt;&gt;", "&<>"),
        ("&amp &notit; &notin;", "& \u{ac}it; \u{2209}"),
        (
            "&#65;&#x42;&#128;&#0;&#x110000;",
            "AB\u{20ac}\u{fffd}\u{fffd}",
        ),
        ("&nosuch; & &#; &#x;", "&nosuch; & &#; &#x;"),
        ("&CounterClockwiseContourIntegral;", "\u{2233}"),
        ("&fjlig;", "fj"),
    ];
    for (input, expected) in cases {
        let parsed = parse(input.as_bytes(), 0);
        let text: String = parsed.document.root().text_chunks().collect();
        assert_eq!(text, *expected, "{input:?}");
    }
    let parsed = parse(b"<a href='?x=1&copy=2&amp;y' title='&notit'>", 0);
    let anchor = parsed
        .document
        .nodes()
        .find(|node| node.name().is_some_and(|n| n.local() == "a"))
        .unwrap();
    let values: Vec<String> = anchor.attributes().map(|a| a.value.into_owned()).collect();
    assert_eq!(values, ["?x=1&copy=2&y", "&notit"]);
}

#[test]
fn raw_text_elements() {
    check(
        &[
            (
                "<script>a<b>&amp;</script>",
                "<html><head><script>a<b>&amp;</script></head></html>",
            ),
            (
                "<textarea>a<b>&amp;</textarea>",
                "<html><body><textarea>a&lt;b&gt;&amp;</textarea></body></html>",
            ),
            (
                "<plaintext>a</plaintext>",
                "<html><body><plaintext>a</plaintext></plaintext></body></html>",
            ),
            (
                "<style>a</style",
                "<html><head><style>a</style</style></head></html>",
            ),
        ],
        0,
    );
    let parsed = parse(b"<script>x</script>", 0);
    let script = parsed
        .document
        .nodes()
        .find(|n| n.kind() == OwnedNodeKind::Cdata)
        .unwrap();
    assert_eq!(script.text(), Some("x"));
}

#[test]
fn comments_and_doctypes() {
    check(
        &[
            (
                "<!--a--><!-->x",
                "<!--a--><!----><html><body>x</body></html>",
            ),
            ("<?pi?>", "<!--?pi?-->"),
            ("<p>a<!x>b", "<html><body><p>a<!--x-->b</p></body></html>"),
        ],
        0,
    );
    let doctype = |input: &[u8], flags| {
        let document = parse(input, flags).document;
        let first = document.root().first_child().map(|node| node.kind());
        let doctype = document.doctype().map(|d| {
            let owned = |s: Option<&str>| s.map(str::to_owned);
            (d.name.to_owned(), owned(d.public_id), owned(d.system_id))
        });
        if doctype.is_some() {
            assert_eq!(first, Some(OwnedNodeKind::DocumentType));
        }
        doctype
    };
    let declared = doctype(b"<!--c--><!DOCTYPE html SYSTEM 'about:legacy-compat'>", 0).unwrap();
    assert_eq!(declared.2.as_deref(), Some("about:legacy-compat"));
    assert_eq!(
        doctype(b"<p>", 0).unwrap().1.as_deref(),
        Some("-//W3C//DTD HTML 4.0 Transitional//EN")
    );
    assert_eq!(
        doctype(b"<!DOCTYPE>", 0).unwrap(),
        (String::new(), None, None)
    );
    assert_eq!(doctype(b"<p>", html::NODEFDTD), None);
    assert_eq!(doctype(b"<p><!DOCTYPE x>", html::NODEFDTD), None);
    assert_eq!(doctype(b"<!DOCTYPE a><!DOCTYPE b>", 0).unwrap().0, "a");
}

#[test]
fn attributes_keep_libxml2_values() {
    let parsed = parse(b"<input a b='' c=x checked a=2>", 0);
    let input = parsed
        .document
        .nodes()
        .find(|n| n.name().is_some_and(|n| n.local() == "input"))
        .unwrap();
    let names: Vec<String> = input
        .attributes()
        .map(|a| a.name.qualified().to_owned())
        .collect();
    assert_eq!(names, ["a", "b", "c", "checked"]);
    let present: Vec<bool> = (0..5).map(|i| input.attribute_has_value(i)).collect();
    assert_eq!(present, [false, true, true, true, false]);
    assert_eq!(input.attributes().nth(3).unwrap().value, "checked");
    assert!(!parsed.document.root().attribute_has_value(0));
}

#[test]
fn editing_gives_a_valueless_attribute_a_value() {
    let mut parsed = parse(b"<p a>", 0);
    let p = parsed
        .document
        .nodes()
        .find(|n| n.name().is_some_and(|n| n.local() == "p"))
        .unwrap()
        .id();
    assert!(!parsed.document.node(p).unwrap().attribute_has_value(0));
    let mut editor = parsed.document.edit(EditLimits::default()).unwrap();
    editor.set_attribute_value(p, None, "a", "v").unwrap();
    let node = parsed.document.node(p).unwrap();
    assert!(node.attribute_has_value(0));
    assert!(
        html::save(&parsed.document, false)
            .unwrap()
            .ends_with(b"<p a=\"v\"></p></body></html>\n")
    );
}

#[test]
fn whitespace_and_noblanks() {
    let input =
        b"<html> <head> <title>t</title> </head> <body> <div> <p>x</p> </div> </body> </html>";
    assert_eq!(
        shape(input, 0),
        "<html> <head> <title>t</title> </head> <body> <div> <p>x</p> </div> </body> </html>"
    );
    assert_eq!(
        shape(input, html::NOBLANKS),
        "<html><head><title>t</title></head><body> <div> <p>x</p> </div> </body></html>"
    );
}

#[test]
fn prescan_utf16_label_saves_as_utf16() {
    // (input, encoding label, first bytes of the saved document)
    let cases: &[(&[u8], Option<&str>, &[u8])] = &[
        (
            b"<p>\xe9 http-equiv content charset=utf-16",
            Some("utf-16"),
            &[0xFF, 0xFE, b'<', 0],
        ),
        (
            b"<p>\xe9 http-equiv content charset=utf-8",
            Some("UTF-8"),
            b"<!DOCTYPE",
        ),
        (
            b"<p>\xe9 http-equiv content charset=nope",
            Some("UTF-8"),
            b"<!DOCTYPE",
        ),
    ];
    for (input, encoding, saved) in cases {
        let document = parse(input, 0).document;
        assert_eq!(document.html_encoding(), *encoding, "{input:?}");
        let out = html::save(&document, false);
        assert_eq!(
            out.as_ref().map(|o| &o[..saved.len()]),
            Ok(*saved),
            "{input:?}"
        );
    }
}

#[test]
fn encodings() {
    let cases: &[(&[u8], Option<&str>, &str)] = &[
        (b"<p>caf\xc3\xa9", Some("windows-1252"), "caf\u{c3}\u{a9}"),
        (
            b"<meta charset=utf-8><p>caf\xc3\xa9",
            Some("utf-8"),
            "caf\u{e9}",
        ),
        (
            b"<meta charset=latin1><p>\xe9\x80",
            Some("latin1"),
            "\u{e9}\u{20ac}",
        ),
        (
            b"<meta http-equiv=Content-Type content='text/html; charset=UTF-8'><p>\xc3\xa9",
            Some("UTF-8"),
            "\u{e9}",
        ),
        (
            b"<p>\xc3\xa9<meta http-equiv=Content-Type content='charset=utf-8'>",
            Some("UTF-8"),
            "\u{e9}",
        ),
        (
            b"<meta charset=bogus><p>\xe9",
            Some("windows-1252"),
            "\u{e9}",
        ),
        (b"\xef\xbb\xbf<p>\xc3\xa9", None, "\u{e9}"),
        (b"\xff\xfe<\0p\0>\0\xe9\0", None, "\u{e9}"),
        (b"\xfe\xff\0<\0p\0>\0\xe9", None, "\u{e9}"),
        (b"<?xml?><p>\xc3\xa9", Some("UTF-8"), "\u{e9}"),
        (b"<p>ascii", None, "ascii"),
    ];
    for (input, encoding, text) in cases {
        let parsed = parse(input, 0);
        assert_eq!(parsed.document.html_encoding(), *encoding, "{input:?}");
        let found: String = parsed.document.root_element().text_chunks().collect();
        assert_eq!(found, *text, "{input:?}");
    }
    let ignored = parse(b"<meta charset=utf-8><p>\xc3\xa9", html::IGNORE_ENC);
    assert_eq!(ignored.document.html_encoding(), Some("windows-1252"));
    assert_eq!(
        kinds(&parse(b"<meta charset=utf-16><meta charset=nope>", 0).diagnostics),
        [
            DiagnosticKind::UnsupportedEncoding,
            DiagnosticKind::UnsupportedEncoding
        ]
    );
    assert_eq!(
        kinds(&parse(b"<meta charset=utf-8>\xff\xff", 0).diagnostics),
        [DiagnosticKind::InvalidEncoding]
    );
    // Built-in single-byte charsets decode, and serialize back in kind.
    let cases: &[(&[u8], &str, &[u8])] = &[
        (
            b"<meta charset=iso-8859-2><p>\xb1\xe9&#x20ac;",
            "\u{105}\u{e9}\u{20ac}",
            b"<p>\xb1\xe9&#8364;</p>",
        ),
        (
            b"<meta charset=ISO-8859-7><p>\xe1",
            "\u{3b1}",
            b"<p>\xe1</p>",
        ),
        (
            b"<p>\xe1<!--http-equiv content charset=iso-8859-7 -->",
            "\u{3b1}",
            b"<p>\xe1<!--",
        ),
        (
            b"<meta charset=latin1><p>\x80",
            "\u{20ac}",
            b"<p>&#8364;</p>",
        ),
    ];
    for (input, text, output) in cases {
        let parsed = parse(input, html::NODEFDTD);
        let found: String = parsed.document.root_element().text_chunks().collect();
        assert_eq!(found, *text, "{input:?}");
        let saved = html::save(&parsed.document, false).unwrap();
        assert!(
            saved.windows(output.len()).any(|w| w == *output),
            "{saved:?}"
        );
    }
    assert_eq!(
        kinds(&parse(b"<meta charset=koi8-r><p>", 0).diagnostics),
        [DiagnosticKind::UnsupportedEncoding]
    );
}

#[test]
fn serializer_formats_like_htmldocdumpmemory() {
    let cases: &[(&[u8], &str)] = &[
        (b"<p>x", "<html><body><p>x</p></body></html>"),
        (
            b"<div><p>a</p><p>b</p></div>",
            "<html><body><div>\n<p>a</p>\n<p>b</p>\n</div></body></html>",
        ),
        (
            b"<ul><li>a<li>b</ul>",
            "<html><body><ul>\n<li>a</li>\n<li>b</li>\n</ul></body></html>",
        ),
        (
            b"<br><img src='a b'><input disabled=x>",
            "<html><body>\n<br><img src=\"a%20b\"><input disabled>\n</body></html>",
        ),
        (
            b"<a href='&eacute; x' name='a b'>&lt;</a>",
            "<html><body><a href=\"%C3%A9%20x\" name=\"a%20b\">&lt;</a></body></html>",
        ),
        (
            b"<p title='\"&<>'>&nbsp;&euro;</p>",
            "<html><body><p title=\"&quot;&amp;&lt;&gt;\">&nbsp;&euro;</p></body></html>",
        ),
        (
            b"<!DOCTYPE x PUBLIC 'a\"b' \"c'd\">",
            "<!DOCTYPE x PUBLIC 'a\"b' \"c'd\">",
        ),
    ];
    for (input, expected) in cases {
        let parsed = html::parse(input, Options::new(html::NODEFDTD)).unwrap();
        let output = String::from_utf8(html::to_html(&parsed.document).unwrap()).unwrap();
        assert_eq!(
            output.trim_end_matches('\n'),
            *expected,
            "{:?}",
            String::from_utf8_lossy(input)
        );
    }
}

#[test]
fn save_declares_the_output_encoding() {
    let cases: &[(&[u8], &[u8])] = &[
        (
            b"<html><head><meta charset=bogus></head>\xe9</html>",
            b"<html><head><meta charset=\"windows-1252\"></head><body>\xe9</body></html>",
        ),
        (
            b"<head><title>t</title></head><p>\xe9",
            b"<html><head><meta charset=\"windows-1252\"><title>t</title></head><body><p>\xe9</p></body></html>",
        ),
        (
            b"<meta http-equiv=content-type content='text/html; charset=utf-8'><p>\xe2\x82\xac",
            b"<html><head><meta http-equiv=\"content-type\" content=\"text/html; charset=utf-8\"></head><body><p>\xe2\x82\xac</p></body></html>",
        ),
        (b"<head></head>&#x20ac;", b"<html><head></head><body>&euro;</body></html>"),
    ];
    for (input, expected) in cases {
        let parsed = html::parse(input, Options::new(html::NODEFDTD)).unwrap();
        let output = html::save(&parsed.document, false).unwrap();
        assert_eq!(
            output.strip_suffix(b"\n").unwrap(),
            *expected,
            "{:?}",
            String::from_utf8_lossy(input)
        );
    }
}

#[test]
fn options_are_checked() {
    for flags in [1 << 3, 1 << 30] {
        let error = html::parse(b"", Options::new(flags)).unwrap_err();
        assert_eq!(error.kind, ErrorKind::UnsupportedOption, "{flags:#x}");
        assert!(PushParser::new(Options::new(flags)).is_err());
    }
    let accepted = html::RECOVER
        | html::NOWARNING
        | html::PEDANTIC
        | html::NONET
        | html::COMPACT
        | html::BIG_LINES
        | (1 << 1);
    assert!(html::parse(b"<p>", Options::new(accepted)).is_ok());
}

#[test]
fn depth_and_length_ceilings_stop_parsing() {
    let deep = "<div>".repeat(300) + "x";
    let parsed = parse(deep.as_bytes(), 0);
    assert!(parsed.stopped);
    assert_eq!(kinds(&parsed.diagnostics), [DiagnosticKind::ResourceLimit]);
    let divs = parsed
        .document
        .nodes()
        .filter(|n| n.name().is_some_and(|n| n.local() == "div"))
        .count();
    assert_eq!(divs, 254);
    let huge = parse(deep.as_bytes(), html::HUGE);
    assert!(!huge.stopped);

    let mut options = Options::default();
    options.limits.max_token_bytes = 8;
    for input in [
        &b"<p>aaaaa&amp;bbbbb"[..],
        b"<!--123456789-->",
        b"<p a='123456789'>",
    ] {
        let parsed = html::parse(input, options).unwrap();
        assert!(parsed.stopped, "{input:?}");
    }
    assert!(!html::parse(b"<p>12345678</p>", options).unwrap().stopped);
}

#[test]
fn hard_limits_fail() {
    let cases: &[(Limits, &[u8], Limit)] = &[
        (
            Limits::default().max_input_bytes(3),
            b"<p>x",
            Limit::InputBytes,
        ),
        (
            Limits::default().max_attributes(2),
            b"<p a b c>",
            Limit::Attributes,
        ),
        (Limits::default().max_events(3), b"<p>a<p>b", Limit::Events),
        (
            Limits::default().max_owned_bytes(8),
            b"<p>aaaaaaaaaaaa",
            Limit::OwnedBytes,
        ),
    ];
    for (limits, input, limit) in cases {
        let options = Options::DEFAULT.flags(0).limits(*limits);
        let error = html::parse(input, options).unwrap_err();
        assert_eq!(error.kind, ErrorKind::LimitExceeded(*limit), "{input:?}");
        let mut parser = PushParser::new(options).unwrap();
        let fed = input.chunks(2).try_for_each(|chunk| parser.feed(chunk));
        let error = fed.and_then(|()| parser.finish().map(drop)).unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::LimitExceeded(*limit),
            "push {input:?}"
        );
    }
}

#[test]
fn push_parsing_is_chunk_independent() {
    for flags in [0, html::NOBLANKS | html::NOIMPLIED] {
        for input in corpus::CASES {
            let whole = parse(input, flags);
            let expected = html::save(&whole.document, false).unwrap();
            let sizes: Vec<usize> = if input.len() <= 40 {
                (1..=input.len().max(1)).collect()
            } else {
                vec![1, 2, 3, 7, 64]
            };
            for size in sizes {
                let mut parser = PushParser::new(Options::new(flags)).unwrap();
                for chunk in input.chunks(size) {
                    parser.feed(chunk).unwrap();
                }
                let pushed = parser.finish().unwrap();
                let output = html::save(&pushed.document, false).unwrap();
                let context = format!("{:?} in chunks of {size}", String::from_utf8_lossy(input));
                assert_eq!(output, expected, "{context}");
                assert_eq!(pushed.diagnostics, whole.diagnostics, "{context}");
            }
        }
    }
}

#[test]
fn push_errors_are_terminal() {
    let options = Options::DEFAULT
        .flags(0)
        .limits(Limits::default().max_input_bytes(4));
    let mut parser = PushParser::new(options).unwrap();
    parser.feed(b"<p>").unwrap();
    assert!(parser.feed(b"xy").is_err());
    assert_eq!(parser.feed(b"").unwrap_err().kind, ErrorKind::InvalidState);
    assert_eq!(parser.finish().unwrap_err().kind, ErrorKind::InvalidState);
}

#[test]
fn documents_are_marked_html() {
    let parsed = parse(b"", html::NODEFDTD);
    assert!(parsed.document.is_html());
    assert_eq!(
        parsed.document.root_element().kind(),
        OwnedNodeKind::Document
    );
    assert_eq!(html::save(&parsed.document, false).unwrap(), b"\n");
    let xml = yaxml::OwnedDocument::parse("<r/>").unwrap();
    assert!(!xml.is_html() && xml.doctype().is_none() && xml.html_encoding().is_none());
    assert_eq!(html::to_html(&xml).unwrap(), b"<r></r>\n");
}

#[test]
fn html_ids_follow_xmlisid() {
    let parsed = parse(
        b"<p id=a>1<a name=n>2</a><b name=m id>3<i id=a>4<i ID=c>",
        0,
    );
    let document = &parsed.document;
    let name = |value: &str| {
        document
            .element_by_id(value)
            .map(|node| node.name().unwrap().qualified().to_owned())
    };
    assert_eq!(name("a"), Some("p".to_owned()));
    assert_eq!(name("n"), Some("a".to_owned()));
    assert_eq!(name("c"), Some("i".to_owned()));
    assert_eq!((name("m"), name(""), name("zz")), (None, None, None));
    let id = yaxml::xpath::XPath::compile("count(id('a c n'))").unwrap();
    let value = id.evaluate(
        document,
        document.root().id(),
        &yaxml::xpath::Context::new(),
    );
    assert_eq!(value.unwrap(), yaxml::xpath::Value::Number(3.0));
}

#[test]
fn node_dumps_match_html_node_dump() {
    // htmlNodeDump (format) and htmlNodeDumpFormatOutput/htmlDocContentDumpFormatOutput
    // without a converter, from libxml2 2.15.4.
    let page = "<!DOCTYPE html><html><head><meta charset=\"latin1\"><title>t</title></head>\
                <body><p>\u{e9}&amp;<b>x</b> \u{20ac}</p><!--c--></body></html>";
    let parsed = parse(page.as_bytes(), 0);
    let document = &parsed.document;
    let html = document.document_element().unwrap();
    let head = html.first_child().unwrap();
    let p = html.last_child().unwrap().first_child().unwrap();
    let cases = [
        (
            head,
            true,
            "<head>\n<meta charset=\"latin1\">\n<title>t</title>\n</head>\n",
        ),
        (
            head,
            false,
            "<head><meta charset=\"latin1\"><title>t</title></head>",
        ),
        (p, true, "<p>Ã©&amp;<b>x</b> â\u{201a}¬</p>\n"),
        (p, false, "<p>Ã©&amp;<b>x</b> â\u{201a}¬</p>"),
        (
            document.root(),
            false,
            "<!DOCTYPE html>\n<html><head><meta charset=\"latin1\"><title>t</title></head>\
             <body><p>Ã©&amp;<b>x</b> â\u{201a}¬</p><!--c--></body></html>\n",
        ),
        (
            document.root(),
            true,
            "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"latin1\">\n<title>t</title>\n</head>\n\
             <body>\n<p>Ã©&amp;<b>x</b> â\u{201a}¬</p>\n<!--c-->\n</body>\n</html>\n",
        ),
    ];
    for (node, format, expected) in cases {
        let dumped = html::dump_node(node, format).unwrap();
        assert_eq!(String::from_utf8(dumped).unwrap(), expected, "{format}");
    }
}
