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

/// Texts with each byte class `find_byte` screens at every offset across
/// two 16-byte blocks and the tail, and in runs.
fn screened_texts() -> Vec<String> {
    let specials = [
        "&",
        "<",
        ">",
        "\"",
        "\t",
        "\n",
        "\r",
        "\u{1}",
        "\u{e9}",
        "\u{4e2d}",
        "\u{fffe}",
        "\u{1f600}",
    ];
    let mut texts: Vec<String> = specials.iter().map(|s| s.repeat(20)).collect();
    texts.push(specials.concat().repeat(3));
    texts.push("a".repeat(70));
    for special in specials {
        for at in 0..40 {
            texts.push(format!(
                "{}{special}{}",
                "a".repeat(at),
                "b".repeat(40 - at)
            ));
        }
    }
    texts
}

#[test]
fn screened_text_escapes_as_character_by_character() {
    // A single character never reaches the block screen, so each text must
    // come out as its characters written one at a time.
    let labels = [
        None,
        Some("UTF-8"),
        Some("ISO-8859-1"),
        Some("US-ASCII"),
        Some("UTF-16"),
    ];
    for text in screened_texts() {
        for label in labels {
            for attribute in [false, true] {
                let (mut whole, mut chars) = (out(label), out(label));
                whole.text(&text, attribute);
                for c in text.chars() {
                    chars.text(c.encode_utf8(&mut [0; 4]), attribute);
                }
                assert_eq!(done(whole), done(chars), "{label:?} {attribute} {text:?}");
            }
        }
        for (table, hex) in [
            (&ATTRIBUTE, true),
            (&QUOT, false),
            (&CONTENT, true),
            (&CONTENT, false),
            (&HTML, false),
        ] {
            let (mut whole, mut chars) = (String::new(), String::new());
            escape_into(&text, table, hex, |piece| whole.push_str(piece));
            for c in text.chars() {
                escape_into(c.encode_utf8(&mut [0; 4]), table, hex, |piece| {
                    chars.push_str(piece)
                });
            }
            assert_eq!(whole, chars, "{hex} {text:?}");
        }
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
    let entry = |ordinal, prefix: Option<&str>, uri: &str| {
        (ordinal, prefix.map(str::to_owned), uri.to_owned())
    };
    // The default z and c use is hoisted to the root: one declaration, not two.
    let expected = [
        entry(0, Some("a"), "urn:a"),
        entry(0, Some("b"), "urn:b"),
        entry(0, None, "urn:d"),
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
    // An unprefixed root takes the default once; an own `xmlns=''` keeps
    // its subtree out of it.
    let owned =
        OwnedDocument::parse("<r xmlns='urn:d'><x><y xmlns=''><z/></y><w/></x></r>").unwrap();
    let x = owned.root_element().first_child().unwrap();
    assert_eq!(
        missing_namespaces(Tree::Owned(x)),
        [entry(0, None, "urn:d")]
    );
    let y = x.first_child().unwrap();
    assert!(missing_namespaces(Tree::Owned(y)).is_empty());
}
