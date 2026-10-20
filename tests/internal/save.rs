//! Output-buffer and namespace details of `save`.

use super::*;

fn out(label: Option<&str>) -> Out<'static> {
    let output = label.map_or(Output::Utf8, |label| Output::for_label(label).unwrap());
    Out::new(output, label.is_none(), None)
}

/// An in-memory buffer's bytes, or its first error.
fn done(out: Out<'_>) -> Result<Vec<u8>, Error> {
    match out.error {
        None => Ok(out.bytes),
        Some(WriteError::Xml(error)) => Err(error),
        Some(WriteError::Io(error)) => panic!("memory output failed: {error}"),
    }
}

#[test]
fn text_escapes_and_references_follow_the_encoding() {
    // (encoding, attribute value, input, output)
    let cases: &[(Option<&str>, bool, &str, &[u8])] = &[
        (
            None,
            false,
            "a<b>&c\r\t\n\"",
            b"a&lt;b&gt;&amp;c&#13;\t\n\"",
        ),
        (
            None,
            true,
            "a<b>&c\r\t\n\"",
            b"a&lt;b&gt;&amp;c&#13;&#9;&#10;&quot;",
        ),
        (None, false, "\u{1}x\u{1f}", b"&#xFFFD;x&#xFFFD;"),
        (
            None,
            false,
            "\u{e9}\u{20ac}\u{1f600}",
            b"&#xE9;&#x20AC;&#x1F600;",
        ),
        (
            Some("UTF-8"),
            false,
            "\u{e9}\u{20ac}",
            "\u{e9}\u{20ac}".as_bytes(),
        ),
        (Some("ISO-8859-1"), true, "\u{e9}\u{20ac}", b"\xe9&#8364;"),
        (Some("US-ASCII"), false, "\u{e9}<", b"&#233;&lt;"),
        (Some("windows-1252"), false, "\u{20ac}\u{e9}", b"\x80\xe9"),
        (
            Some("ISO-8859-2"),
            false,
            "\u{105}\u{e9}\u{20ac}",
            b"\xb1\xe9&#8364;",
        ),
        (Some("UTF-16BE"), false, "\u{e9}<", b"\0\xe9\0&\0l\0t\0;"),
        (Some("UTF-16"), false, "a", b"\xff\xfea\0"),
        (
            Some("UTF-16LE"),
            false,
            "\u{1f600}",
            &[0x3d, 0xd8, 0x00, 0xde],
        ),
        (Some("UTF-8"), false, "", b""),
    ];
    for (label, attribute, input, expected) in cases {
        let mut out = out(*label);
        out.text(input, *attribute);
        assert_eq!(done(out).unwrap(), *expected, "{label:?} {input:?}");
    }
}

#[test]
fn markup_fails_where_references_would_change_it() {
    // (encoding, markup, first unrepresentable output offset)
    let cases: &[(Option<&str>, &str, Option<usize>)] = &[
        (None, "\u{e9}", None),
        (Some("UTF-16"), "\u{1f600}", None),
        (Some("ISO-8859-1"), "a\u{e9}", None),
        (Some("ISO-8859-1"), "a\u{105}b\u{106}", Some(1)),
        (Some("ASCII"), "\u{e9}", Some(0)),
    ];
    for (label, markup, error) in cases {
        let mut out = out(*label);
        out.raw(markup);
        let offset = done(out).err().map(|error| {
            assert_eq!(error.kind, ErrorKind::InvalidCharacter);
            error.offset
        });
        assert_eq!(offset, *error, "{label:?} {markup:?}");
    }
    // Converted text uses references instead.
    let mut ascii = out(Some("ASCII"));
    ascii.converted("a\u{e9}");
    assert_eq!(done(ascii).unwrap(), b"a&#233;");
}

#[test]
fn quoted_strings_choose_a_quote_like_libxml2() {
    let cases: &[(&str, bool, &[u8])] = &[
        ("a", false, b"\"a\""),
        ("a\"b", false, b"'a\"b'"),
        ("a\"b'c", false, b"\"a&quot;b'c\""),
        ("\u{e9}\"'", true, b"\"&#233;&quot;'\""),
    ];
    for (text, converted, expected) in cases {
        let mut out = out(Some("ASCII"));
        out.quoted(text, *converted);
        assert_eq!(done(out).unwrap(), *expected, "{text:?}");
    }
}

#[test]
fn appended_errors_keep_their_absolute_offset() {
    let mut first = out(Some("ASCII"));
    first.raw("abc");
    let mut sibling = first.sibling();
    sibling.raw("d\u{e9}");
    first.append(sibling);
    assert_eq!(
        done(first).unwrap_err(),
        Error::new(ErrorKind::InvalidCharacter, 4)
    );

    let mut first = out(Some("ASCII"));
    first.raw("\u{e9}");
    let mut sibling = first.sibling();
    sibling.raw("\u{e9}");
    first.append(sibling);
    assert_eq!(done(first).unwrap_err().offset, 0);
}

#[test]
fn subtrees_gain_undeclared_namespaces_in_first_use_order() {
    let input = "<r xmlns='urn:d' xmlns:a='urn:a' xmlns:b='urn:b'>\
                 <a:x b:y='1' xml:lang='en'>t<z xmlns:a='urn:o'><a:w/></z><c/></a:x></r>";
    let expected = [
        (Some("a".to_owned()), "urn:a".to_owned()),
        (Some("b".to_owned()), "urn:b".to_owned()),
        (None, "urn:d".to_owned()),
    ];
    let document = Document::parse(input).unwrap();
    let x = document.root_element().first_child().unwrap();
    assert_eq!(missing_namespaces(Tree::Borrowed(x)), expected);
    let owned = OwnedDocument::parse(input).unwrap();
    let x = owned.root_element().first_child().unwrap();
    assert_eq!(missing_namespaces(Tree::Owned(x)), expected);
    // Documents, leaves and self-contained subtrees need none.
    assert!(missing_namespaces(Tree::Owned(owned.root())).is_empty());
    assert!(missing_namespaces(Tree::Owned(owned.root_element())).is_empty());
    let text = x.first_child().unwrap();
    assert!(missing_namespaces(Tree::Owned(text)).is_empty());
}
