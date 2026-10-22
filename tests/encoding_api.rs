use yaxml::ErrorKind;
use yaxml::encoding::{CharEncoding, EncError, Handler};

#[test]
fn encodings_are_detected_parsed_and_named_as_libxml2() {
    for (input, expected) in [
        (&b"\0\0\0<"[..], CharEncoding::Ucs4Be),
        (b"<\0\0\0", CharEncoding::Ucs4Le),
        (b"\x4c\x6f\xa7\x94", CharEncoding::Ebcdic),
        (b"<?xml", CharEncoding::Utf8),
        (b"\xef\xbb\xbf", CharEncoding::Utf8),
        (b"<\0?\0", CharEncoding::Utf16Le),
        (b"\0<\0?", CharEncoding::Utf16Be),
        (b"\xff\xfe<", CharEncoding::Utf16Le),
        (b"\xfe\xff", CharEncoding::Utf16Be),
        (b"<r/>", CharEncoding::None),
        (b"\xef\xbb", CharEncoding::None),
        (b"", CharEncoding::None),
    ] {
        assert_eq!(CharEncoding::detect(input), expected, "{input:?}");
    }
    // Recorded with xmlParseCharEncoding.
    for (label, expected) in [
        ("unicodefffe", CharEncoding::Utf16Be),
        ("UNICODEFFFE", CharEncoding::Utf16Be),
        // Out of order in libxml2's table: its binary search misses it.
        ("unicodefeff", CharEncoding::Error),
        ("utf-16", CharEncoding::Utf16Le),
        ("UTF16", CharEncoding::Utf16Le),
        ("HTML", CharEncoding::Html),
        ("ucs-4", CharEncoding::Ucs4Le),
        ("ucs-2", CharEncoding::Ucs2),
        ("iso latin 1", CharEncoding::Iso8859_1),
        ("iso8859-10", CharEncoding::Iso8859_1),
        ("x-cp1252", CharEncoding::Windows1252),
        ("foo", CharEncoding::Error),
        ("", CharEncoding::Error),
    ] {
        assert_eq!(CharEncoding::parse(label), expected, "{label}");
    }
    for (encoding, name) in [
        (CharEncoding::Error, None),
        (CharEncoding::None, None),
        (CharEncoding::Utf8, Some("UTF-8")),
        (CharEncoding::Utf16Le, Some("UTF-16")),
        (CharEncoding::Utf16Be, Some("UTF-16")),
        (CharEncoding::Ucs4Le, Some("UCS-4")),
        (CharEncoding::Ebcdic, Some("IBM037")),
        (CharEncoding::Ucs4_2143, None),
        (CharEncoding::ShiftJis, Some("Shift_JIS")),
        (CharEncoding::Windows1252, Some("windows-1252")),
    ] {
        assert_eq!(encoding.name(), name, "{encoding:?}");
    }
    assert_eq!(CharEncoding::Windows1252 as i32, 31);
    assert_eq!(CharEncoding::Error as i32, -1);
    assert_eq!(EncError::Memory as i32, -4);
}

#[test]
fn handlers_are_looked_up_like_libxml2() {
    let name = |encoding| Handler::lookup(encoding).map(|h| h.map(|h| h.name().to_owned()));
    for (encoding, expected) in [
        (CharEncoding::Utf8, Ok(None)),
        (CharEncoding::Utf16Le, Ok(Some("UTF-16LE"))),
        (CharEncoding::Utf16, Ok(Some("UTF-16"))),
        (CharEncoding::Iso8859_1, Ok(Some("ISO-8859-1"))),
        (CharEncoding::Iso8859_16, Ok(Some("ISO-8859-16"))),
        (CharEncoding::Ascii, Ok(Some("US-ASCII"))),
        (CharEncoding::Windows1252, Ok(Some("windows-1252"))),
        (CharEncoding::None, Err(ErrorKind::UnsupportedEncoding)),
        (CharEncoding::Error, Err(ErrorKind::UnsupportedEncoding)),
        (CharEncoding::Ucs4_3412, Err(ErrorKind::UnsupportedEncoding)),
        (CharEncoding::ShiftJis, Err(ErrorKind::UnsupportedEncoding)),
        (CharEncoding::Html, Err(ErrorKind::UnsupportedEncoding)),
    ] {
        assert_eq!(
            name(encoding),
            expected.map(|n| n.map(String::from)),
            "{encoding:?}"
        );
    }
    let iconv = cfg!(feature = "encodings");
    assert_eq!(Handler::lookup(CharEncoding::Ebcdic).is_ok(), iconv);
    assert_eq!(Handler::open("UCS-4").is_ok(), iconv);
    assert_eq!(Handler::open("latin1").unwrap().name(), "latin1");
    assert_eq!(Handler::open("UTF-8").unwrap().name(), "UTF-8");
    assert_eq!(
        Handler::open("Shift_JIS"),
        Err(ErrorKind::UnsupportedEncoding)
    );
    assert_eq!(
        Handler::open("unicodefeff"),
        Err(ErrorKind::UnsupportedEncoding)
    );
}

#[test]
fn handlers_decode_like_xml_char_enc_in_func() {
    type Row<'a> = (&'a str, &'a [u8], Result<usize, (EncError, usize)>, &'a str);
    // Recorded with xmlCharEncInFunc: bytes read, and what was written.
    let rows: [Row; 12] = [
        ("latin1", b"caf\xe9", Ok(4), "caf\u{e9}"),
        ("US-ASCII", b"ab\x80c", Err((EncError::Input, 2)), "ab"),
        ("UTF-16LE", b"a\0b", Ok(2), "a"),
        ("UTF-16LE", b"a\0\x3d\xd8", Ok(2), "a"),
        (
            "UTF-16LE",
            b"a\0\x3d\xd8z\0",
            Err((EncError::Input, 2)),
            "a",
        ),
        ("UTF-16LE", b"a\0\0\xdc", Err((EncError::Input, 2)), "a"),
        ("UTF-16BE", b"\0a\xd8\x3d\xde\0", Ok(6), "a\u{1f600}"),
        ("UTF-16", b"\xff\xfea\0", Ok(4), "\u{feff}a"),
        ("windows-1252", b"\x80\x81", Ok(2), "\u{20ac}\u{81}"),
        // libxml2 copies UTF-8 unchecked; a cut-off character waits here.
        ("UTF-8", b"a\xc3", Ok(1), "a"),
        ("UTF-8", b"a\xff", Err((EncError::Input, 1)), "a"),
        ("UTF-8", b"", Ok(0), ""),
    ];
    for (label, input, expected, text) in rows {
        let mut out = String::from(">");
        let handler = Handler::open(label).unwrap();
        assert_eq!(
            handler.decode(input, &mut out),
            expected,
            "{label} {input:?}"
        );
        assert_eq!(out, format!(">{text}"), "{label} {input:?}");
    }
}

#[test]
fn handlers_encode_like_xml_char_enc_out_func() {
    // Recorded with xmlCharEncOutFunc after its initialization call.
    for (label, input, expected) in [
        ("latin1", "caf\u{e9} \u{263a}", &b"caf\xe9 &#9786;"[..]),
        ("US-ASCII", "\u{e9}", b"&#233;"),
        ("UTF-16", "a", b"\xff\xfea\0"),
        ("UTF-16BE", "a\u{1f600}", b"\0a\xd8\x3d\xde\0"),
        ("windows-1252", "\u{20ac}\u{153}", b"\x80\x9c"),
        ("UTF-8", "", b""),
    ] {
        let handler = Handler::open(label).unwrap();
        let mut out = handler.bom().to_vec();
        assert_eq!(handler.encode(input, &mut out), Ok(()), "{label}");
        assert_eq!(out, expected, "{label}");
    }
}
