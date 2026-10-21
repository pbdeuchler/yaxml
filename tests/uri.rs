//! Table-driven URI tests. Expected values were produced by libxml2 2.15.4
//! (`harness/tooling/libxml2_uri.c`); `uri_oracle.rs` checks many more cases.
#![forbid(unsafe_code)]

use yaxml::uri::{canonic_path, escape, normalize_path, relative, resolve, unescape};
use yaxml::{Uri, UriError};

const RFC_BASE: &str = "http://a/b/c/d;p?q";

fn syntax(offset: usize) -> UriError {
    UriError::Syntax { offset }
}

#[test]
fn rfc3986_section_5_4_examples() {
    // Rows marked `quirk` differ from the RFC: libxml2 keeps `..` above the
    // root and does not normalize absolute-path references.
    let cases = [
        ("g:h", "g:h"),
        ("g", "http://a/b/c/g"),
        ("./g", "http://a/b/c/g"),
        ("g/", "http://a/b/c/g/"),
        ("/g", "http://a/g"),
        ("//g", "http://g"),
        ("?y", "http://a/b/c/d;p?y"),
        ("g?y", "http://a/b/c/g?y"),
        ("#s", "http://a/b/c/d;p?q#s"),
        ("g#s", "http://a/b/c/g#s"),
        ("g?y#s", "http://a/b/c/g?y#s"),
        (";x", "http://a/b/c/;x"),
        ("g;x", "http://a/b/c/g;x"),
        ("g;x?y#s", "http://a/b/c/g;x?y#s"),
        ("", "http://a/b/c/d;p?q"),
        (".", "http://a/b/c/"),
        ("./", "http://a/b/c/"),
        ("..", "http://a/b/"),
        ("../", "http://a/b/"),
        ("../g", "http://a/b/g"),
        ("../..", "http://a/"),
        ("../../", "http://a/"),
        ("../../g", "http://a/g"),
        ("../../../g", "http://a/../g"),    // quirk: RFC http://a/g
        ("../../../../g", "http://a/../g"), // quirk: RFC http://a/g
        ("/./g", "http://a/./g"),           // quirk: RFC http://a/g
        ("/../g", "http://a/../g"),         // quirk: RFC http://a/g
        ("g.", "http://a/b/c/g."),
        (".g", "http://a/b/c/.g"),
        ("g..", "http://a/b/c/g.."),
        ("..g", "http://a/b/c/..g"),
        ("./../g", "http://a/b/g"),
        ("./g/.", "http://a/b/c/g/"),
        ("g/./h", "http://a/b/c/g/h"),
        ("g/../h", "http://a/b/c/h"),
        ("g;x=1/./y", "http://a/b/c/g;x=1/y"),
        ("g;x=1/../y", "http://a/b/c/y"),
        ("g?y/./x", "http://a/b/c/g?y/./x"),
        ("g?y/../x", "http://a/b/c/g?y/../x"),
        ("g#s/./x", "http://a/b/c/g#s/./x"),
        ("g#s/../x", "http://a/b/c/g#s/../x"),
        ("http:g", "http:g"),
    ];
    for (reference, want) in cases {
        assert_eq!(
            resolve(reference, RFC_BASE).as_deref(),
            Ok(want),
            "{reference}"
        );
    }
}

#[test]
fn resolve_edge_cases() {
    let cases: [(&str, &str, Result<&str, UriError>); 18] = [
        ("#f", "http://a/b#old", Ok("http://a/b#f")),
        ("", "http://a/b#old", Ok("http://a/b")),
        ("x", "http://a", Ok("http://a/x")),
        ("x", "http://a:81/p/q", Ok("http://a:81/p/x")),
        ("x", "http://u@h/p", Ok("http://u@h/x")),
        ("//h2:9/y", "https://h/z", Ok("https://h2:9/y")),
        ("../a", "file:///x/y.xml", Ok("file:///a")),
        ("a b", "http://h/", Err(syntax(1))),
        ("x", "http://[::1", Ok("x")),
        ("", "http://[::1", Err(syntax(0))),
        ("http://[::1", "http://h/", Err(syntax(11))),
        // Bases without "://" are filesystem paths.
        ("g", "dir/a.xml", Ok("dir/g")),
        ("g#f", "dir/a.xml", Ok("dir/g#f")),
        ("../x", "a/b/c", Ok("a/x")),
        ("a%20b", "d/e", Ok("d/a b")),
        ("x", "", Ok("x")),
        ("", "", Err(syntax(0))),
        ("/abs", "a/b", Ok("/abs")),
    ];
    for (reference, base, want) in cases {
        assert_eq!(
            resolve(reference, base),
            want.map(str::to_owned),
            "{reference} {base}"
        );
    }
    assert_eq!(resolve("%FF", "dir/a"), Err(UriError::InvalidUtf8));
    assert_eq!(
        resolve("x\0junk", "http://h/p"),
        Ok("http://h/x".to_owned())
    );
}

type Parts<'a> = (
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<u32>,
    Option<&'a str>,
);

fn parts(u: &Uri) -> Parts<'_> {
    (
        u.scheme.as_deref(),
        u.user.as_deref(),
        u.host.as_deref(),
        u.port,
        u.path.as_deref(),
    )
}

#[test]
fn parse_components() {
    let cases: [(&str, Parts, Option<&str>, Option<&str>); 15] = [
        (
            "http://u:p%40@h:80/a%20b?q%20=1#f%20g",
            (
                Some("http"),
                Some("u:p@"),
                Some("h"),
                Some(80),
                Some("/a b"),
            ),
            Some("q%20=1"),
            Some("f g"),
        ),
        (
            "http://[::1]:8080/x",
            (Some("http"), None, Some("[::1]"), Some(8080), Some("/x")),
            None,
            None,
        ),
        (
            // libxml2 keeps any byte inside the brackets and saves it verbatim.
            "p://[\u{e9}]",
            (Some("p"), None, Some("[\u{e9}]"), None, None),
            None,
            None,
        ),
        (
            "http://[v1.x]/",
            (Some("http"), None, Some("[v1.x]"), None, Some("/")),
            None,
            None,
        ),
        (
            "http://h:0/",
            (Some("http"), None, Some("h"), None, Some("/")),
            None,
            None,
        ),
        (
            "http://:80/",
            (Some("http"), None, None, Some(80), Some("/")),
            None,
            None,
        ),
        (
            "http://h:2147483647",
            (Some("http"), None, Some("h"), Some(i32::MAX as u32), None),
            None,
            None,
        ),
        (
            "http://1.2.3.4x/",
            (Some("http"), None, Some("1.2.3.4x"), None, Some("/")),
            None,
            None,
        ),
        (
            "//h/p",
            (None, None, Some("h"), None, Some("/p")),
            None,
            None,
        ),
        ("a:b", (Some("a"), None, None, None, Some("b")), None, None),
        ("a/b:c", (None, None, None, None, Some("a/b:c")), None, None),
        (
            "%41%7e?%41#%41",
            (None, None, None, None, Some("A~")),
            Some("%41"),
            Some("A"),
        ),
        ("?", (None, None, None, None, None), Some(""), None),
        ("#", (None, None, None, None, None), None, Some("")),
        (
            "http://h/%00x",
            (Some("http"), None, Some("h"), None, Some("/")),
            None,
            None,
        ),
    ];
    for (input, want, query, fragment) in cases {
        let uri = Uri::parse(input).unwrap_or_else(|e| panic!("{input}: {e}"));
        assert_eq!(
            (parts(&uri), uri.query.as_deref(), uri.fragment.as_deref()),
            (want, query, fragment),
            "{input}"
        );
    }
}

#[test]
fn empty_authority_and_libxml2_quirks() {
    assert!(Uri::parse("file:///x").unwrap().empty_authority);
    assert!(Uri::parse("x://").unwrap().empty_authority);
    // `///x` as a relative reference loses its empty authority.
    let relative = Uri::parse("///x").unwrap();
    assert!(!relative.empty_authority && !relative.has_authority());
    assert_eq!(relative.to_string(), "/x");
    // An empty userinfo is unescaped with length zero, i.e. the whole rest.
    assert_eq!(Uri::parse("http://@/").unwrap().user.as_deref(), Some("@/"));
    assert_eq!(
        Uri::parse_raw("http://@/").unwrap().user.as_deref(),
        Some("")
    );
    // xmlSaveUri writes the host verbatim, so a bracketed host stays non-ASCII.
    let raw = Uri::parse_raw("p://[\u{e9}]").unwrap();
    assert_eq!(raw.to_string(), "p://[\u{e9}]");
    // A reg-name host cannot hold non-ASCII text.
    assert_eq!(Uri::parse_raw("p://\u{e9}"), Err(syntax(4)));
}

#[test]
fn parse_rejects_malformed_input() {
    let cases = [
        ("a b", 1),
        (":x", 0),
        ("1a:b", 2),
        ("http://[::1", 11),
        ("http://h:", 9),
        ("http://h:80x", 11),
        ("http://h:2147483648", 9),
        ("http://h:99999999999999999999", 9),
        ("http://h/p q", 10),
        ("#a#b", 2),
        ("%zz", 0),
        ("\u{e9}", 0),
        ("http://h/\u{e9}", 9),
        ("[x]", 0),
        ("//[", 3),
    ];
    for (input, offset) in cases {
        assert_eq!(Uri::parse(input), Err(syntax(offset)), "{input}");
        assert_eq!(Uri::parse_raw(input), Err(syntax(offset)), "{input} raw");
    }
    assert_eq!(Uri::parse("http://h/%FF"), Err(UriError::InvalidUtf8));
    assert_eq!(
        Uri::parse_raw("http://h/%FF").unwrap().path.as_deref(),
        Some("/%FF")
    );
    assert_eq!(syntax(3).to_string(), "invalid URI syntax at byte 3");
    assert!(UriError::InvalidUtf8.to_string().contains("UTF-8"));
}

#[test]
fn display_matches_xml_save_uri() {
    let cases = [
        (
            "http://u:p%40@h:80/a%20b?q%20=1#f%20g",
            "http://u:p%40@h:80/a%20b?q%20=1#f%20g",
        ),
        ("http://h:0/", "http://h/"),
        ("http://@/", "http://%40%2F@/"),
        ("file:///d:/x%20y", "file:///d:/x%20y"),
        ("http://h/%C3%A9", "http://h/%C3%A9"),
        ("a/b:c", "a/b%3Ac"),
        ("%41%7e?%41#%41", "A~?%41#A"),
        ("http://[::1]:8080/x", "http://[::1]:8080/x"),
        ("g#a[b]c", "g#a[b]c"),
    ];
    for (input, want) in cases {
        assert_eq!(Uri::parse(input).unwrap().to_string(), want, "{input}");
    }
    assert_eq!(Uri::parse_raw("%41%7e").unwrap().to_string(), "%2541%257e");
    assert_eq!(Uri::default().to_string(), "");
}

#[test]
fn escape_cases() {
    let cases = [
        ("a b/c@d%\u{e9}", "/", "a%20b/c@d%25%C3%A9"),
        ("AZaz09-_.!~*'()", "", "AZaz09-_.!~*'()"),
        ("", "x", ""),
        ("a+b", "+", "a+b"),
        ("a+b", "", "a%2Bb"),
        ("[x]#", "", "%5Bx%5D%23"),
        ("\u{e9}", "\u{e9}", "%C3%A9"),
    ];
    for (input, allowed, want) in cases {
        assert_eq!(escape(input, allowed), want, "{input:?} {allowed:?}");
    }
}

#[test]
fn unescape_cases() {
    let cases: [(&str, Result<&str, UriError>); 12] = [
        ("%41%62", Ok("Ab")),
        ("%4", Ok("%4")),
        ("%", Ok("%")),
        ("%G1", Ok("%G1")),
        ("%%41", Ok("%A")),
        ("a%2", Ok("a%2")),
        ("%41%", Ok("A%")),
        ("%c3%a9", Ok("\u{e9}")),
        ("%00x", Ok("")),
        ("ok\0cut", Ok("ok")),
        ("%FF", Err(UriError::InvalidUtf8)),
        ("%C3", Err(UriError::InvalidUtf8)),
    ];
    for (input, want) in cases {
        assert_eq!(unescape(input), want.map(str::to_owned), "{input:?}");
    }
}

#[test]
fn normalize_path_cases() {
    let cases = [
        ("/a/./b/../c", "/a/c"),
        ("a/../..", ".."),
        ("../a", "../a"),
        ("/..", "/.."),
        ("/a//b///", "/a/b/"),
        (".", ""),
        ("./", ""),
        ("", ""),
        ("/", "/"),
        ("a/.", "a/"),
        ("a/..", ""),
        ("/a/..", "/"),
        ("../..", "../.."),
        ("a/b/../../../c", "../c"),
        ("/./a", "/a"),
        ("//a", "/a"),
    ];
    for (input, want) in cases {
        assert_eq!(normalize_path(input), want, "{input:?}");
    }
}

#[test]
fn canonic_path_cases() {
    assert_eq!(canonic_path("http://a/b c"), "http://a/b%20c");
    assert_eq!(canonic_path("x://h/\u{e9}%41[]"), "x://h/%C3%A9%41[]");
    assert_eq!(canonic_path("/tmp/a b"), "/tmp/a b");
    assert_eq!(canonic_path(""), "");
}

#[test]
fn relative_cases() {
    let cases = [
        (
            "http://site1.com/docs/pic1.gif",
            "http://site1.com/docs/book1.html",
            "pic1.gif",
        ),
        (
            "http://site2.com/docs/pic1.gif",
            "http://site1.com/docs/book1.html",
            "http://site2.com/docs/pic1.gif",
        ),
        ("docs/pic1.gif", "docs/book1.html", "pic1.gif"),
        ("docs/img/pic1.gif", "docs/book1.html", "img/pic1.gif"),
        ("img/pic1.gif", "docs/book1.html", "../img/pic1.gif"),
        (
            "http://site1.com/docs/pic1.gif",
            "docs/book1.html",
            "http://site1.com/docs/pic1.gif",
        ),
        ("foo/", "foo/bar", "./"),
        ("http://a/b/c", "http://a/b/c", ""),
        ("http://a/b c", "http://a/b/d", "http://a/b c"),
        ("http://a", "http://a/x", "./"),
        ("http://a/x", "http://a", "/x"),
        ("/a/b", "c/d", "/a/b"),
        ("a/b", "", "a/b"),
        ("http://a/b/c/d", "http://a/b/x/y/z", "../../c/d"),
        ("http://a:80/x", "http://a/x", "http://a:80/x"),
        ("http://a/b;x/c", "http://a/b/d", "../b;x/c"),
        ("http://[::1", "http://a/", "http://[::1"),
    ];
    for (uri, base, want) in cases {
        assert_eq!(relative(uri, base).as_deref(), Ok(want), "{uri} {base}");
    }
    assert_eq!(relative("", "http://a/"), Err(syntax(0)));
}

#[test]
fn parse_display_round_trips_and_resolution_is_total() {
    for input in [
        "http://h/a%20b?x#y",
        "a/b",
        "//h",
        "x:",
        "file:///d:/p",
        "?q",
        "#f",
        "http://[::1]:1/",
    ] {
        let uri = Uri::parse(input).unwrap();
        let text = uri.to_string();
        assert_eq!(Uri::parse(&text).unwrap().to_string(), text, "{input}");
        for base in ["", "x", "http://h/a/b", "file:///"] {
            let _ = resolve(input, base);
            let _ = relative(input, base);
        }
    }
}
