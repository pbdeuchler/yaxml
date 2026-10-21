//! `TextWriter` (libxml2 `xmlTextWriter`): output bytes are libxml2's for
//! the same calls (harness/tests/text_writer_oracle.rs checks that at scale); here
//! the documented cases, failures, limits and sinks.

use std::io::{self, Write};

use yaxml::{ErrorKind, Limit, OwnedDocument, TextWriter, WriteError, WriterLimits};

type Script = fn(&mut TextWriter<Vec<u8>>) -> Result<(), WriteError>;

fn kind(result: Result<(), WriteError>) -> Option<ErrorKind> {
    match result {
        Ok(()) => None,
        Err(WriteError::Xml(error)) => Some(error.kind),
        Err(error) => panic!("{error}"),
    }
}

#[test]
fn call_sequences_write_libxml2_bytes() {
    let cases: &[(&str, Script, &str)] = &[
        (
            "auto-closed start tags and escaping",
            |w| {
                w.start_element("r")?;
                w.write_attribute("a", "\"<&>\t\n\u{e9}")?;
                w.write_string("x<&>\"\r\u{e9}")?;
                w.start_element("e")?;
                w.end_element()?;
                w.full_end_element()
            },
            "<r a=\"&quot;&lt;&amp;&gt;&#9;&#10;&#xE9;\">x&lt;&amp;&gt;&quot;&#13;\u{e9}<e/></r>",
        ),
        (
            "indentation",
            |w| {
                w.set_indent(true);
                w.set_indent_string("\t");
                w.start_element("r")?;
                w.write_element("a", Some("t"))?;
                w.start_element("b")?;
                w.write_comment("c")?;
                w.end_document()
            },
            "<r>\n\t<a>t</a>\n\t<b>\n\t\t<!--c-->\n</b>\n</r>\n",
        ),
        (
            "namespaces are declared when the start tag closes",
            |w| {
                w.start_element_ns(Some("p"), "r", Some("urn:p"))?;
                w.write_attribute_ns(Some("q"), "a", Some("urn:q"), "v")?;
                w.write_attribute_ns(Some("p"), "b", Some("urn:p"), "w")?;
                w.write_element_ns(None, "c", Some("urn:d"), "")?;
                w.end_element()
            },
            "<p:r q:a=\"v\" p:b=\"w\" xmlns:q=\"urn:q\" xmlns:p=\"urn:p\"><c xmlns=\"urn:d\"></c></p:r>",
        ),
        (
            "declaration, quote character (not escaped in values), PI, CDATA",
            |w| {
                w.set_quote_char('\'')?;
                w.start_document(Some("1.0"), Some("utf8"), Some("yes"))?;
                w.write_pi("t", Some("d"))?;
                w.start_element("r")?;
                w.write_attribute("a", "'")?;
                w.write_cdata(Some("x]]"))?;
                w.write_raw("&raw;")?;
                w.end_document()
            },
            "<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\n<?t d?><r a='''><![CDATA[x]]]]>&raw;</r>\n",
        ),
        (
            "DTD declarations",
            |w| {
                w.start_dtd("r", Some("-//P"), Some("r.dtd"))?;
                w.write_dtd_element("r", "(#PCDATA)")?;
                w.write_dtd_attlist("r", "a CDATA #IMPLIED")?;
                w.write_dtd_internal_entity(false, "e", "v")?;
                w.write_dtd_external_entity(true, "pe", None, Some("p.ent"), None)?;
                w.write_dtd_entity(false, "u", Some("-//U"), Some("u.gif"), Some("gif"), None)?;
                w.write_dtd_notation("gif", Some("-//G"), None)?;
                w.end_dtd()
            },
            "<!DOCTYPE r PUBLIC \"-//P\" \"r.dtd\" [<!ELEMENT r (#PCDATA)><!ATTLIST r a CDATA #IMPLIED><!ENTITY e \"v\"><!ENTITY % pe SYSTEM \"p.ent\"><!ENTITY u PUBLIC \"-//U\" \"u.gif\" NDATA gif><!NOTATION gif PUBLIC \"-//G\">]>",
        ),
        (
            "base64 lines and BinHex",
            |w| {
                w.start_element("r")?;
                w.write_base64(&[0xff; 56])?;
                w.write_bin_hex(&[0, 0xab])?;
                w.end_element()
            },
            "<r>////////////////////////////////////////////////////////////////////////\r\n//8=00AB</r>",
        ),
        (
            "top-level strings are raw; EndPI with nothing open succeeds",
            |w| {
                w.write_string("<&>")?;
                w.end_pi()
            },
            "<&>",
        ),
    ];
    for (name, script, expected) in cases {
        let mut writer = TextWriter::new(Vec::new());
        script(&mut writer).unwrap_or_else(|error| panic!("{name}: {error}"));
        let output = writer.into_inner();
        assert_eq!(String::from_utf8(output).unwrap(), *expected, "{name}");
    }
}

#[test]
fn misuse_fails_like_libxml2_and_keeps_partial_output() {
    // (case, failing call, error, output including earlier successful calls)
    let cases: &[(&str, Script, ErrorKind, &str)] = &[
        (
            "end with nothing open",
            |w| w.end_element(),
            ErrorKind::InvalidState,
            "",
        ),
        (
            "empty name",
            |w| w.start_element(""),
            ErrorKind::InvalidName,
            "",
        ),
        (
            "attribute after content",
            |w| {
                w.start_element("r")?;
                w.write_string("t")?;
                w.write_attribute("a", "v")
            },
            ErrorKind::InvalidState,
            "<r>t",
        ),
        (
            "PI named xml",
            |w| w.start_pi("XmL"),
            ErrorKind::InvalidDeclaration,
            "",
        ),
        (
            "element inside a PI",
            |w| {
                w.start_pi("p")?;
                w.start_element("r")
            },
            ErrorKind::InvalidState,
            "<?p",
        ),
        (
            "public identifier without system identifier",
            |w| w.start_dtd("r", Some("-//P"), None),
            ErrorKind::InvalidDtd,
            "<!DOCTYPE r",
        ),
        (
            "DTD after content",
            |w| {
                w.start_element("r")?;
                w.start_dtd("r", None, None)
            },
            ErrorKind::InvalidState,
            "<r",
        ),
        (
            "prefix bound twice on one element",
            |w| {
                w.start_element_ns(Some("p"), "r", Some("urn:a"))?;
                w.write_attribute_ns(Some("p"), "a", Some("urn:b"), "v")
            },
            ErrorKind::Namespace,
            "<p:r",
        ),
        (
            "unknown encoding",
            |w| w.start_document(None, Some("x-bogus"), None),
            ErrorKind::UnsupportedEncoding,
            "",
        ),
        (
            "bad quote",
            |w| w.set_quote_char('x'),
            ErrorKind::UnsupportedOption,
            "",
        ),
        (
            "NDATA on a parameter entity",
            |w| w.write_dtd_external_entity(true, "e", None, Some("s"), Some("n")),
            ErrorKind::InvalidDtd,
            "",
        ),
        (
            "notation outside a DTD while indenting",
            |w| {
                w.set_indent(true);
                w.write_dtd_notation("n", None, Some("s"))
            },
            ErrorKind::InvalidState,
            "",
        ),
    ];
    for (name, script, error, expected) in cases {
        let mut writer = TextWriter::new(Vec::new());
        assert_eq!(kind(script(&mut writer)), Some(*error), "{name}");
        // libxml2's failures are not terminal.
        writer.flush().unwrap();
        let output = String::from_utf8(writer.into_inner()).unwrap();
        assert_eq!(output, *expected, "{name}");
    }
    // A stray end deletes libxml2's namespace list: namespaced calls and
    // closing a start tag fail from then on.
    let mut writer = TextWriter::new(Vec::new());
    assert!(writer.end_element().is_err());
    writer.start_element("r").unwrap();
    assert_eq!(kind(writer.end_element()), Some(ErrorKind::InvalidState));
    assert_eq!(
        kind(writer.start_element_ns(None, "s", None)),
        Some(ErrorKind::InvalidState)
    );
    assert_eq!(writer.into_inner(), b"<r");
}

#[test]
fn encodings_convert_output_and_name_the_converter() {
    let cases: &[(&str, &[u8])] = &[
        (
            "UTF-8",
            b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<r a=\"\xc3\xa9\">\xe2\x82\xac</r>\n",
        ),
        (
            "latin1",
            b"<?xml version=\"1.0\" encoding=\"latin1\"?>\n<r a=\"\xe9\">&#8364;</r>\n",
        ),
        (
            "US-ASCII",
            b"<?xml version=\"1.0\" encoding=\"US-ASCII\"?>\n<r a=\"&#233;\">&#8364;</r>\n",
        ),
    ];
    for (encoding, expected) in cases {
        let mut writer = TextWriter::new(Vec::new());
        writer.start_document(None, Some(encoding), None).unwrap();
        writer.write_element("r", None).unwrap();
        writer.end_document().unwrap();
        let mut writer = TextWriter::new(Vec::new());
        writer.start_document(None, Some(encoding), None).unwrap();
        writer.start_element("r").unwrap();
        writer.write_attribute("a", "\u{e9}").unwrap();
        writer.write_string("\u{20ac}").unwrap();
        writer.end_document().unwrap();
        assert_eq!(writer.into_inner(), *expected, "{encoding}");
    }
    // UTF-16 writes a BOM; without an encoding attributes escape non-ASCII.
    let mut writer = TextWriter::new(Vec::new());
    writer.start_document(None, Some("UTF-16"), None).unwrap();
    assert!(writer.into_inner().starts_with(&[0xff, 0xfe, b'<', 0]));
    let mut writer = TextWriter::new(Vec::new());
    writer.start_element("r").unwrap();
    writer.write_attribute("a", "\u{e9}").unwrap();
    assert_eq!(writer.into_inner(), b"<r a=\"&#xE9;\"");
}

/// A sink failing every call, or on flush only.
struct Failing(bool);

impl Write for Failing {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self.0 {
            true => Ok(bytes.len()),
            false => Err(io::Error::other("full")),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("flush"))
    }
}

#[test]
fn limits_and_io_failures_are_terminal() {
    let limits = |change: fn(&mut WriterLimits)| {
        let mut limits = WriterLimits::default();
        change(&mut limits);
        limits
    };
    // (limits, script, limit, bytes written before the failing call)
    let cases: &[(WriterLimits, Script, Limit, &str)] = &[
        (
            limits(|l| l.max_depth = 2),
            |w| {
                w.start_element("a")?;
                w.start_element("b")?;
                w.start_element("c")
            },
            Limit::Depth,
            "<a><b",
        ),
        (
            limits(|l| l.max_output_bytes = 6),
            |w| {
                w.start_element("a")?;
                w.write_string("xyz")?;
                w.write_string("w")
            },
            Limit::OutputBytes,
            "<a>xyz",
        ),
        (
            limits(|l| l.max_token_bytes = 8),
            |w| w.write_element("a", Some("&&")),
            Limit::TokenBytes,
            "",
        ),
        (
            limits(|l| l.max_attributes = 1),
            |w| {
                w.start_element_ns(None, "a", Some("urn:a"))?;
                w.write_attribute_ns(Some("p"), "b", Some("urn:b"), "")
            },
            Limit::Attributes,
            "<a",
        ),
    ];
    for (limits, script, limit, expected) in cases {
        let mut writer = TextWriter::with_limits(Vec::new(), *limits);
        let error = script(&mut writer).unwrap_err();
        assert_eq!(kind(Err(error)), Some(ErrorKind::LimitExceeded(*limit)));
        assert_eq!(kind(writer.write_raw("x")), Some(ErrorKind::InvalidState));
        assert_eq!(writer.position(), expected.len());
        assert_eq!(writer.into_inner(), expected.as_bytes(), "{limit:?}");
    }
    for flush_only in [false, true] {
        let mut writer = TextWriter::new(Failing(flush_only));
        let result = match flush_only {
            false => writer.start_element("a"),
            true => writer.flush(),
        };
        assert!(matches!(result, Err(WriteError::Io(_))));
        assert_eq!(
            kind(writer.start_element("b")),
            Some(ErrorKind::InvalidState)
        );
    }
}

#[test]
fn well_formed_call_sequences_reparse() {
    let mut writer = TextWriter::new(Vec::new());
    writer.set_indent(true);
    writer
        .start_document(None, Some("ISO-8859-1"), None)
        .unwrap();
    writer.write_dtd("r", None, Some("r.dtd"), None).unwrap();
    writer
        .start_element_ns(Some("p"), "r", Some("urn:p"))
        .unwrap();
    writer
        .write_attribute("a", "\"<&>\t\u{e9}\u{1f600}")
        .unwrap();
    writer.write_element("p:c", Some("x\r\n]]>")).unwrap();
    writer.write_comment("note").unwrap();
    writer.write_pi("t", Some("d")).unwrap();
    writer.write_cdata(Some("<raw>")).unwrap();
    writer.end_document().unwrap();
    let output = writer.into_inner();
    let document = OwnedDocument::parse_bytes(&output).unwrap();
    let root = document.root_element();
    assert_eq!(
        root.attributes().next().unwrap().value,
        "\"<&>\t\u{e9}\u{1f600}"
    );
    assert_eq!(root.first_child().unwrap().text(), Some("\n "));
}
