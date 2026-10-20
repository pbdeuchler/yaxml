//! `save::to_vec`: options, encodings, subtrees, the regenerated internal
//! subset and XHTML rules. Expected bytes are pinned libxml2 2.15.4
//! `xmllint --noent --dtdattr` output where xmllint exposes the option.
#![forbid(unsafe_code)]

use std::io::{self, Write};

use yaxml::{Document, ErrorKind, OwnedDocument, WriteError, html, save};

const DECLARATION: &str = "<?xml version=\"1.0\"?>\n";

fn options(flags: u32) -> save::Options<'static> {
    save::Options::new(flags)
}

/// Saves `input` through both DOMs, which must agree.
fn saved(input: &str, options: &save::Options<'_>) -> Result<Vec<u8>, ErrorKind> {
    let owned = save::to_vec(&OwnedDocument::parse(input).unwrap(), options);
    let borrowed = save::to_vec(&Document::parse(input).unwrap(), options);
    assert_eq!(owned, borrowed, "{input}");
    owned.map_err(|error| error.kind)
}

#[test]
fn flags_shape_output_like_xmllint() {
    let tree =
        "<r><a>t</a><b/><!--c--><?p d?><c><![CDATA[x]]><d/></c><e a=\"1\" b=\"2\"><f/></e></r>";
    let cases: &[(&str, save::Options<'_>, &str)] = &[
        (tree, options(0), &format!("{DECLARATION}{tree}\n")),
        (
            tree,
            options(save::FORMAT),
            "<r>\n  <a>t</a>\n  <b/>\n  <!--c-->\n  <?p d?>\n  <c><![CDATA[x]]><d/></c>\n  \
             <e a=\"1\" b=\"2\">\n    <f/>\n  </e>\n</r>\n",
        ),
        (
            tree,
            options(save::WSNONSIG),
            "<r\n  ><a\n    >t</a\n  ><b\n  /><!--c--><?p\n  d?><c\n    ><![CDATA[x]]><d\n    \
             /></c\n  ><e\n      a=\"1\"\n      b=\"2\"\n    ><f\n    /></e\n  ></r\n>\n",
        ),
        (
            tree,
            options(save::NO_EMPTY | save::NO_DECL),
            "<r><a>t</a><b></b><!--c--><?p d?><c><![CDATA[x]]><d></d></c><e a=\"1\" b=\"2\"><f></f></e></r>\n",
        ),
        (
            "<r><a><b/></a></r>",
            options(save::FORMAT | save::NO_INDENT),
            "<r>\n<a>\n<b/>\n</a>\n</r>\n",
        ),
        (
            "<r><a><b/></a></r>",
            save::Options {
                indent: "\t",
                ..options(save::FORMAT)
            },
            "<r>\n\t<a>\n\t\t<b/>\n\t</a>\n</r>\n",
        ),
        (
            "<r><a/></r>",
            options(save::NO_EMPTY | save::WSNONSIG),
            "<r\n  ><a\n    ></a\n  ></r\n>\n",
        ),
        (
            "<r><a>x<b/></a><c><d/></c></r>",
            options(save::FORMAT),
            "<r>\n  <a>x<b/></a>\n  <c>\n    <d/>\n  </c>\n</r>\n",
        ),
        (
            "<!--a--><?b c?><r xmlns=\"urn:d\" a=\"1\" xmlns:p=\"urn:p\" p:b=\"2\"/><!--z-->",
            options(save::FORMAT),
            "<!--a-->\n<?b c?>\n<r xmlns=\"urn:d\" xmlns:p=\"urn:p\" a=\"1\" p:b=\"2\"/>\n<!--z-->\n",
        ),
        (
            "<r><![CDATA[a]]]]><![CDATA[>b]]><![CDATA[]]></r>",
            options(0),
            "<r><![CDATA[a]]]]><![CDATA[>b]]><![CDATA[]]></r>\n",
        ),
        (
            "<r xmlns:xml='http://www.w3.org/XML/1998/namespace' xml:lang='en'><?p?></r>",
            options(0),
            "<r xml:lang=\"en\"><?p?></r>\n",
        ),
        (
            "<?xml version='1.0' standalone='yes'?><r/>",
            options(save::EMPTY | save::INDENT | save::NO_XHTML | save::AS_XML),
            "<?xml version=\"1.0\" standalone=\"yes\"?>\n<r/>\n",
        ),
    ];
    for (input, options, expected) in cases {
        let expected = if options.flags & save::NO_DECL == 0 && !expected.starts_with("<?xml ") {
            format!("{DECLARATION}{expected}")
        } else {
            expected.to_string()
        };
        assert_eq!(
            String::from_utf8(saved(input, options).unwrap()).unwrap(),
            expected,
            "{input} {options:?}"
        );
    }
}

#[test]
fn indentation_stops_at_sixty_columns() {
    let depth = 33;
    let input = format!("{}{}", "<e>".repeat(depth), "</e>".repeat(depth));
    let output = String::from_utf8(saved(&input, &options(save::FORMAT)).unwrap()).unwrap();
    let widest = output
        .lines()
        .map(|line| line.len() - line.trim_start().len())
        .max();
    assert_eq!(widest, Some(60));
    let wide = save::Options {
        indent: &"\t".repeat(60),
        ..options(save::FORMAT)
    };
    let output = String::from_utf8(saved("<r><a><b/></a></r>", &wide).unwrap()).unwrap();
    assert!(output.contains(&format!("\n{}<b/>", "\t".repeat(60))));
}

#[test]
fn encodings_reference_what_they_lack() {
    let input = "<?xml version='1.1' encoding='ISO-8859-1' standalone='no'?><r a='&#233;'>&#233;&#8364;</r>";
    let latin1 = b"<?xml version=\"1.1\" encoding=\"ISO-8859-1\" standalone=\"no\"?>\n<r a=\"\xe9\">\xe9&#8364;</r>\n";
    type Case<'a> = (&'a str, Option<&'a str>, Result<&'a [u8], ErrorKind>);
    let cases: &[Case<'_>] = &[
        (input, None, Ok(latin1)),
        (
            "<r a='&#233;' b='&lt;&quot;&#9;'>&#233;&#xD;</r>",
            Some("US-ASCII"),
            Ok(b"<?xml version=\"1.0\" encoding=\"US-ASCII\"?>\n<r a=\"&#233;\" b=\"&lt;&quot;&#9;\">&#233;&#13;</r>\n"),
        ),
        (
            "<r>&#x1F600;</r>",
            Some("windows-1252"),
            Ok(b"<?xml version=\"1.0\" encoding=\"windows-1252\"?>\n<r>&#128512;</r>\n"),
        ),
        (
            "<r>&#233;</r>",
            None,
            Ok(b"<?xml version=\"1.0\"?>\n<r>&#xE9;</r>\n"),
        ),
        (
            "<r>&#233;</r>",
            Some("utf-8"),
            Ok("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<r>\u{e9}</r>\n".as_bytes()),
        ),
        (
            "<r>&#x105;</r>",
            Some("latin2"),
            Ok(b"<?xml version=\"1.0\" encoding=\"latin2\"?>\n<r>\xb1</r>\n"),
        ),
        ("<r/>", Some("EBCDIC"), Err(ErrorKind::UnsupportedEncoding)),
        ("<r/>", Some("HTML"), Err(ErrorKind::UnsupportedEncoding)),
        ("<r><!--\u{e9}--></r>", Some("ASCII"), Err(ErrorKind::InvalidCharacter)),
        ("<\u{e9}/>", Some("ASCII"), Err(ErrorKind::InvalidCharacter)),
        ("<r><?p \u{e9}?></r>", Some("ASCII"), Err(ErrorKind::InvalidCharacter)),
        ("<r><![CDATA[\u{e9}]]></r>", Some("ASCII"), Err(ErrorKind::InvalidCharacter)),
    ];
    for (input, encoding, expected) in cases {
        let options = save::Options {
            encoding: *encoding,
            ..options(0)
        };
        let owned = save::to_vec(
            &OwnedDocument::parse_bytes(input.as_bytes()).unwrap(),
            &options,
        );
        assert_eq!(
            owned.as_deref().map_err(|error| error.kind),
            *expected,
            "{input} {encoding:?}"
        );
    }
    let utf16 = saved(
        "<r>&#233;</r>",
        &save::Options {
            encoding: Some("UTF-16"),
            ..options(0)
        },
    )
    .unwrap();
    let units: Vec<u16> = utf16
        .chunks(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    assert_eq!(
        String::from_utf16(&units).unwrap(),
        "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n<r>\u{e9}</r>\n"
    );
    let big = saved(
        "<r/>",
        &save::Options {
            encoding: Some("UTF-16BE"),
            ..options(0)
        },
    )
    .unwrap();
    assert_eq!(&big[..4], b"\0<\0?");
    // The output reparses to the same document.
    let reparsed = OwnedDocument::parse_bytes(&utf16).unwrap();
    assert_eq!(
        reparsed.root_element().first_child().unwrap().text(),
        Some("\u{e9}")
    );
}

#[test]
fn invalid_requests_are_rejected() {
    let document = OwnedDocument::parse("<r/>").unwrap();
    for options in [
        options(1 << 11),
        options(u32::MAX),
        save::Options {
            indent: "",
            ..options(0)
        },
        save::Options {
            indent: " -",
            ..options(0)
        },
        save::Options {
            indent: "\n",
            ..options(0)
        },
        save::Options {
            indent: &" ".repeat(61),
            ..options(0)
        },
    ] {
        assert_eq!(
            save::to_vec(&document, &options).unwrap_err().kind,
            ErrorKind::UnsupportedOption,
            "{options:?}"
        );
    }
    // The HTML serializer needs an owned document node.
    let borrowed = Document::parse("<r/>").unwrap();
    assert_eq!(
        save::to_vec(&borrowed, &options(save::AS_HTML))
            .unwrap_err()
            .kind,
        ErrorKind::UnsupportedOption
    );
    // AS_HTML saves an owned XML subtree as HTML.
    assert_eq!(
        save::to_vec(document.root_element(), &options(save::AS_HTML)).unwrap(),
        b"<r></r>"
    );
}

#[test]
fn subtrees_declare_the_namespaces_they_use() {
    let input = "<r xmlns='urn:d' xmlns:a='urn:a'><a:x a:y='&#233;'><c/><!--k--></a:x>t</r>";
    let owned = OwnedDocument::parse(input).unwrap();
    let borrowed = Document::parse(input).unwrap();
    let x = "<a:x xmlns:a=\"urn:a\" xmlns=\"urn:d\" a:y=\"&#xE9;\"><c/><!--k--></a:x>";
    let cases: &[(&str, &[usize], save::Options<'_>, &str)] = &[
        ("element", &[0], options(0), x),
        (
            "formatted",
            &[0],
            options(save::FORMAT),
            "<a:x xmlns:a=\"urn:a\" xmlns=\"urn:d\" a:y=\"&#xE9;\">\n  <c/>\n  <!--k-->\n</a:x>",
        ),
        (
            "encoded",
            &[0],
            save::Options {
                encoding: Some("UTF-8"),
                ..options(0)
            },
            "<a:x xmlns:a=\"urn:a\" xmlns=\"urn:d\" a:y=\"\u{e9}\"><c/><!--k--></a:x>",
        ),
        ("child", &[0, 0], options(0), "<c xmlns=\"urn:d\"/>"),
        ("comment", &[0, 1], options(0), "<!--k-->"),
        ("text", &[1], options(0), "t"),
        (
            "root",
            &[],
            options(0),
            "<r xmlns=\"urn:d\" xmlns:a=\"urn:a\"><a:x a:y=\"&#xE9;\"><c/><!--k--></a:x>t</r>",
        ),
    ];
    for (name, path, options, expected) in cases {
        let mut node = owned.root_element();
        let mut view = borrowed.root_element();
        for &index in *path {
            node = node.children().nth(index).unwrap();
            view = view.children().nth(index).unwrap();
        }
        assert_eq!(
            String::from_utf8(save::to_vec(node, options).unwrap()).unwrap(),
            *expected,
            "{name}"
        );
        assert_eq!(
            String::from_utf8(save::to_vec(view, options).unwrap()).unwrap(),
            *expected,
            "{name}"
        );
    }
    let doctype = OwnedDocument::parse("<!DOCTYPE r SYSTEM 's'><r/>").unwrap();
    let node = doctype.root().first_child().unwrap();
    assert_eq!(
        save::to_vec(node, &options(0)).unwrap(),
        b"<!DOCTYPE r SYSTEM \"s\">"
    );
}

#[test]
fn internal_subsets_are_regenerated() {
    let cases: &[(&str, &str)] = &[
        ("<!DOCTYPE r><r/>", "<!DOCTYPE r>"),
        ("<!DOCTYPE r []><r/>", "<!DOCTYPE r>"),
        ("<!DOCTYPE r [<!-- only --><?p?>]><r/>", "<!DOCTYPE r>"),
        ("<!DOCTYPE r [<!ATTLIST r>]><r/>", "<!DOCTYPE r>"),
        (
            "<!DOCTYPE r PUBLIC 'p' \"s'\"><r/>",
            "<!DOCTYPE r PUBLIC \"p\" \"s'\">",
        ),
        (
            "<!DOCTYPE r [<!ELEMENT r (b?|c|d)+><!ELEMENT s (b|c?|d)+><!ELEMENT t (b?|c|d?)*>\
             <!ELEMENT u (a*)><!ELEMENT v (a?)+><!ELEMENT w ((a,b)?)><!ELEMENT x (a|(b|c))>\
             <!ELEMENT y (a,(b,c)*)><!ELEMENT z (#PCDATA)><!ELEMENT zz (#PCDATA)*><!ELEMENT q ((a)?)>\
             <!ELEMENT m (#PCDATA|a|b)*><!ELEMENT e EMPTY><!ELEMENT any ANY><!ELEMENT o ((a,b)|c)><!ELEMENT p (g?|e?|r)+>]><r/>",
            "<!DOCTYPE r [\n<!ELEMENT r (b? | c | d)+>\n<!ELEMENT s (b | c | d)*>\n<!ELEMENT t (b | c | d)*>\n\
             <!ELEMENT u (a)*>\n<!ELEMENT v (a)*>\n<!ELEMENT w (a , b)?>\n<!ELEMENT x (a | b | c)>\n\
             <!ELEMENT y (a , (b , c)*)>\n<!ELEMENT z (#PCDATA)>\n<!ELEMENT zz (#PCDATA)*>\n<!ELEMENT q (a)?>\n\
             <!ELEMENT m (#PCDATA | a | b)*>\n<!ELEMENT e EMPTY>\n<!ELEMENT any ANY>\n<!ELEMENT o ((a , b) | c)>\n<!ELEMENT p (g? | e | r)*>\n]>",
        ),
        (
            "<!DOCTYPE r [\r\n  <!-- lead -->\r\n  <!ATTLIST r b CDATA \"x&amp;y&#10;z\nw\" c (a|b) #IMPLIED>\n\
             <!ELEMENT r (#PCDATA|a|b)*>\n<!ELEMENT r EMPTY>\n\
             <!ATTLIST r b CDATA \"dup\" d NOTATION (n1|n2) #REQUIRED e ID #IMPLIED f NMTOKENS #FIXED \" a b \">\n\
             <!ENTITY % pe \"<!ELEMENT q (a,(b|c),d*)>\">\n%pe;\n<?pi data?>\n\
             <!ENTITY e1 \"va&#37;l&amp;'q'\">\n<!ENTITY e1 \"second\">\n<!ENTITY e2 'dq\"x'>\n\
             <!ENTITY e3 SYSTEM \"e3.xml\">\n<!ENTITY e4 PUBLIC \"-//p\" \"e4.xml\" NDATA n1>\n\
             <!ENTITY % pe2 SYSTEM \"x.ent\">\n<!NOTATION n1 SYSTEM \"n1\">\n<!NOTATION n2 PUBLIC \"-//n2\">\n\
             <!NOTATION n3 PUBLIC \"-//n3\" \"s3\">\n<!ENTITY lt \"&#38;#60;\">\n]><r/>",
            "<!DOCTYPE r [\n<!NOTATION n1 SYSTEM \"n1\" >\n<!NOTATION n2 PUBLIC \"-//n2\" >\n\
             <!NOTATION n3 PUBLIC \"-//n3\" \"s3\" >\n<!-- lead --><!ATTLIST r b CDATA \"x&amp;y&#10;z w\">\n\
             <!ATTLIST r c (a | b) #IMPLIED>\n<!ELEMENT r (#PCDATA | a | b)*>\n\
             <!ATTLIST r d NOTATION (n1 | n2) #REQUIRED>\n<!ATTLIST r e ID #IMPLIED>\n\
             <!ATTLIST r f NMTOKENS #FIXED \"a b\">\n<!ENTITY % pe \"<!ELEMENT q (a,(b|c),d*)>\">\n\
             <!ELEMENT q (a , (b | c) , d*)>\n<?pi data?><!ENTITY e1 \"va&#37;l&amp;'q'\">\n\
             <!ENTITY e2 'dq\"x'>\n<!ENTITY e3 SYSTEM \"e3.xml\">\n<!ENTITY e4 PUBLIC \"-//p\" \"e4.xml\" NDATA n1>\n\
             <!ENTITY % pe2 SYSTEM \"x.ent\">\n<!ENTITY lt \"&#38;#60;\">\n]>",
        ),
    ];
    for (input, doctype) in cases {
        let output = String::from_utf8(saved(input, &options(save::NO_DECL)).unwrap()).unwrap();
        let (subset, _) = output.rsplit_once('\n').unwrap();
        let (subset, _) = subset.rsplit_once('\n').unwrap();
        assert_eq!(subset, *doctype, "{input}");
    }
    // Entity values allow references for what the encoding lacks; the
    // regenerated subset parses again.
    let input = "<!DOCTYPE r [<!ENTITY e \"\u{e9}\"><!ATTLIST r a CDATA \"\u{e9}\">]><r>&e;</r>";
    let ascii = save::Options {
        encoding: Some("ASCII"),
        ..options(0)
    };
    let output = saved(input, &ascii).unwrap();
    let text = String::from_utf8(output.clone()).unwrap();
    assert!(
        text.contains("<!ENTITY e \"&#233;\">\n<!ATTLIST r a CDATA \"&#233;\">"),
        "{text}"
    );
    let reparsed = OwnedDocument::parse_bytes(&output).unwrap();
    assert_eq!(save::to_vec(&reparsed, &ascii).unwrap(), output);
}

#[test]
fn xhtml_documents_follow_appendix_c() {
    let page = "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"x\"><html lang=\"fr\">\
                <head><title>t</title></head><body><p name=\"n\"/><a name=\"m\" id=\"i\"/><br/><img/>\
                <input checked=\"\"/><x:br xmlns:x=\"u\"/><p xml:lang=\"de\"></p><![CDATA[c]]></body></html>";
    let doctype = "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"x\">\n";
    let meta = "<meta http-equiv=\"Content-Type\" content=\"text/html; charset=UTF-8\" />";
    let cases: &[(&str, save::Options<'_>, String)] = &[
        (
            page,
            options(0),
            format!(
                "{doctype}<html xmlns=\"http://www.w3.org/1999/xhtml\" lang=\"fr\" xml:lang=\"fr\"><head>{meta}\
                 <title>t</title></head><body><p name=\"n\" id=\"n\"></p><a name=\"m\" id=\"i\"></a><br /><img />\
                 <input checked=\"checked\" /><x:br xmlns:x=\"u\"></x:br><p xml:lang=\"de\" lang=\"de\"></p>\
                 <![CDATA[c]]></body></html>\n"
            ),
        ),
        (
            page,
            options(save::FORMAT),
            format!(
                "{doctype}<html xmlns=\"http://www.w3.org/1999/xhtml\" lang=\"fr\" xml:lang=\"fr\">\n  <head>\n    \
                 {meta}\n    <title>t</title>\n  </head>\n  <body>\n    <p name=\"n\" id=\"n\"></p>\n    \
                 <a name=\"m\" id=\"i\"></a>\n    <br />\n    <img />\n    <input checked=\"checked\" />\n    \
                 <x:br xmlns:x=\"u\"></x:br>\n    <p xml:lang=\"de\" lang=\"de\"></p>\n<![CDATA[c]]>\n  </body>\n</html>\n"
            ),
        ),
        (
            "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"x\"><html><head/></html>",
            options(save::FORMAT),
            format!(
                "{doctype}<html xmlns=\"http://www.w3.org/1999/xhtml\">\n  <head>\n    {meta}\n</head>\n</html>\n"
            ),
        ),
        (
            "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"x\"><html><head>\
             <meta http-equiv=\"content-type\" content=\"x\"/></head></html>",
            save::Options {
                encoding: Some("ISO-8859-1"),
                ..options(0)
            },
            format!(
                "{doctype}<html xmlns=\"http://www.w3.org/1999/xhtml\"><head>\
                 <meta http-equiv=\"content-type\" content=\"x\" /></head></html>\n"
            ),
        ),
        (
            "<!DOCTYPE html SYSTEM \"x\"><html><br/></html>",
            options(save::XHTML),
            "<!DOCTYPE html SYSTEM \"x\">\n<html><br/></html>\n".to_owned(),
        ),
        (
            "<html><head/><br/><input checked=''/></html>",
            options(save::XHTML),
            format!(
                "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head>{meta}</head><br /><input checked=\"checked\" /></html>\n"
            ),
        ),
        (
            "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"x\"><html><br/></html>",
            options(save::NO_XHTML | save::NO_DECL),
            format!("{doctype}<html><br/></html>\n"),
        ),
    ];
    for (input, options, expected) in cases {
        let expected = if options.flags & save::NO_DECL == 0 {
            let declaration = match options.encoding {
                Some(encoding) => format!("<?xml version=\"1.0\" encoding=\"{encoding}\"?>\n"),
                None => DECLARATION.to_owned(),
            };
            format!("{declaration}{expected}")
        } else {
            expected.clone()
        };
        let output = saved(input, options).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output),
            expected,
            "{input} {options:?}"
        );
    }
    // XHTML subtrees use the rules only on request.
    let document = OwnedDocument::parse("<html><br/></html>").unwrap();
    let br = document.root_element().first_child().unwrap();
    assert_eq!(save::to_vec(br, &options(save::XHTML)).unwrap(), b"<br />");
    assert_eq!(save::to_vec(br, &options(0)).unwrap(), b"<br/>");
}

#[test]
fn html_documents_choose_their_serializer() {
    let parsed = html::parse(
        b"<p class=x>caf&eacute;<br><div>a</div>",
        html::Options::default(),
    )
    .unwrap();
    let document = parsed.document;
    assert_eq!(
        save::to_vec(&document, &options(0)).unwrap(),
        html::save(&document, false).unwrap()
    );
    assert_eq!(
        save::to_vec(&document, &options(save::FORMAT)).unwrap(),
        html::save(&document, true).unwrap()
    );
    assert_eq!(
        String::from_utf8(save::to_vec(&document, &options(save::AS_XML)).unwrap()).unwrap(),
        "<?xml version=\"1.0\" standalone=\"yes\"?>\n<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.0 Transitional//EN\" \
         \"http://www.w3.org/TR/REC-html40/loose.dtd\">\n<html><body><p class=\"x\">caf&#xE9;<br/></p><div>a</div></body></html>\n"
    );
    let p = document
        .root_element()
        .first_child()
        .unwrap()
        .first_child()
        .unwrap();
    // HTML subtrees are HTML (`htmlNodeDumpOutputInternal`), in the
    // document's encoding or "HTML"; expected bytes are libxml2's.
    let body = p.parent().unwrap();
    let cases: &[(save::Tree<'_, '_>, u32, &str)] = &[
        (p.into(), 0, "<p class=\"x\">caf&eacute;<br></p>"),
        (
            p.into(),
            save::FORMAT,
            "<p class=\"x\">caf&eacute;<br></p>\n",
        ),
        (
            p.into(),
            save::AS_HTML,
            "<p class=\"x\">caf&eacute;<br></p>",
        ),
        (
            body.into(),
            save::FORMAT,
            "<body>\n<p class=\"x\">caf&eacute;<br></p>\n<div>a</div>\n</body>",
        ),
    ];
    for (tree, flags, expected) in cases {
        let saved = save::to_vec(*tree, &options(*flags)).unwrap();
        assert_eq!(String::from_utf8(saved).unwrap(), *expected, "{flags}");
    }
    assert_eq!(
        save::to_vec(p, &options(save::AS_XML)).unwrap(),
        b"<p class=\"x\">caf&#xE9;<br/></p>"
    );
    // AS_HTML saves an XML document with the HTML serializer and its encoding.
    let xml =
        OwnedDocument::parse_bytes(b"<?xml version='1.0' encoding='ISO-8859-1'?><p>\xe9<br/></p>")
            .unwrap();
    assert_eq!(
        save::to_vec(&xml, &options(save::AS_HTML)).unwrap(),
        b"<p>\xe9<br></p>\n"
    );
}

// ---- attribute values that keep entity references (no NOENT) ----

/// Saves `input` parsed with `flags` through both DOMs, which must agree,
/// as Latin-1 text; also checks that attribute values are the substituted ones.
fn saved_with(input: &str, flags: u32, options: &save::Options<'_>) -> Result<String, ErrorKind> {
    use yaxml::{ParseOptions, options::NOENT};
    let input = input.as_bytes();
    let owned = OwnedDocument::parse_with(input, ParseOptions::new(flags)).unwrap();
    let borrowed = Document::parse_with(input, ParseOptions::new(flags)).unwrap();
    let substituted = Document::parse_with(input, ParseOptions::new(flags | NOENT)).unwrap();
    let values = owned
        .nodes()
        .flat_map(|node| node.attributes().map(|a| a.value.into_owned()));
    let expected = substituted
        .nodes()
        .flat_map(|node| node.attributes().iter());
    assert!(values.eq(expected.map(|a| a.value.to_string())));
    let output = save::to_vec(&owned, options);
    assert_eq!(output, save::to_vec(&borrowed, options));
    output
        .map(|bytes| bytes.iter().map(|&byte| char::from(byte)).collect())
        .map_err(|error| error.kind)
}

#[test]
fn attribute_references_are_written_like_xmllint() {
    use yaxml::options::{DTDATTR, NOBLANKS, NOENT};
    let plain = options(save::NO_DECL);
    let latin1 = save::Options {
        encoding: Some("ISO-8859-1"),
        ..options(save::NO_DECL)
    };
    // (input, parse flags, save options, last line of pinned xmllint output)
    let table: &[(&str, u32, &save::Options<'_>, &str)] = &[
        (
            "<!DOCTYPE r [<!ENTITY e \"x\">]><r a=\"&e;\" b=\"1&e;2&amp;3&#38;4&lt;&#10;&#9;5\t6\" c=\"plain\"/>",
            0,
            &plain,
            "<r a=\"&e;\" b=\"1&e;2&amp;3&amp;4&lt;&#10;&#9;5 6\" c=\"plain\"/>",
        ),
        // NOENT substitutes, as before.
        (
            "<!DOCTYPE r [<!ENTITY e \"x\">]><r a=\"&e;\" b=\"1&e;2\"/>",
            NOENT,
            &plain,
            "<r a=\"x\" b=\"1x2\"/>",
        ),
        // Tokenized types normalize around the reference only.
        (
            "<!DOCTYPE r [<!ENTITY s \" b  c \"><!ATTLIST r t NMTOKENS #IMPLIED u ID #IMPLIED>]><r t=\"  a &s;  d  \" u=\" &s; \"/>",
            0,
            &plain,
            "<r t=\"a &s; d\" u=\"&s;\"/>",
        ),
        // Only the outermost reference is kept.
        (
            "<!DOCTYPE r [<!ENTITY i \"in\"><!ENTITY o \"[&i;]\">]><r a=\"&o;\"/>",
            0,
            &plain,
            "<r a=\"&o;\"/>",
        ),
        // Namespace declarations always expand.
        (
            "<!DOCTYPE r [<!ENTITY u \"urn:x\">]><r xmlns:p=\"&u;\" p:a=\"&u;\"/>",
            0,
            &plain,
            "<r xmlns:p=\"urn:x\" p:a=\"&u;\"/>",
        ),
        // A quote from an entity is not escaped; one from the literal is.
        (
            "<!DOCTYPE r [<!ENTITY q '\"'>]><r a='&q;\"'/>",
            0,
            &plain,
            "<r a=\"&q;&quot;\"/>",
        ),
        // An undeclared entity a missing external subset could declare is dropped.
        (
            "<!DOCTYPE r SYSTEM \"missing.dtd\" [<!ENTITY e \"x\">]><r a=\"&e;&undeclared;b\"/>",
            0,
            &plain,
            "<r a=\"&e;b\"/>",
        ),
        // Defaulted attributes keep the references of their default.
        (
            "<!DOCTYPE r [<!ENTITY x \"X\"><!ATTLIST r d CDATA \"p&x;q&amp;\">]><r/>",
            DTDATTR,
            &plain,
            "<r d=\"p&x;q&amp;\"/>",
        ),
        // Element content where blanks are dropped: ids are renumbered.
        (
            "<!DOCTYPE r [<!ENTITY e \"x\">]><r>  <a b=\"&e;\"/>  <c d=\"&e;&e;\"/></r>",
            NOBLANKS,
            &plain,
            "<r><a b=\"&e;\"/><c d=\"&e;&e;\"/></r>",
        ),
        // Encodable names are encoded, the text around them converted.
        (
            "<!DOCTYPE r [<!ENTITY é \"v\">]><r a=\"&é;é\u{20ac}\"/>",
            0,
            &latin1,
            "<r a=\"&\u{e9};\u{e9}&#8364;\"/>",
        ),
    ];
    for &(input, flags, options, expected) in table {
        let output = saved_with(input, flags, options).unwrap();
        assert_eq!(output.lines().last(), Some(expected), "{input}");
    }
    // A start tag from replacement text keeps its references too.
    let content = "<!DOCTYPE r [<!ENTITY e \"x\"><!ENTITY c \"<e a='&e;y'/>\">]><r>&c;</r>";
    let document = Document::parse(content).unwrap();
    let element = document.nodes().find(|node| node.attributes().len() == 1);
    assert_eq!(element.unwrap().unexpanded_attribute(0), Some("&e;y"));
    let output = saved_with(content, NOENT, &plain).unwrap();
    assert!(output.ends_with("<r><e a=\"xy\"/></r>\n"), "{output}");
    // UTF-16 input reaches the owned builder through the push parser.
    let utf16: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain(
            "<!DOCTYPE r [<!ENTITY e \"x\">]><r a=\"1&e;\"/>"
                .encode_utf16()
                .flat_map(u16::to_le_bytes),
        )
        .collect();
    let document = OwnedDocument::parse_bytes(&utf16).unwrap();
    assert_eq!(
        document.root_element().unexpanded_attribute(0),
        Some("1&e;")
    );
}

#[test]
fn attribute_reference_names_an_encoding_lacks_fail() {
    let ascii = save::Options {
        encoding: Some("US-ASCII"),
        ..options(0)
    };
    // libxml2 writes `&&#233;;`, which does not reparse.
    let cases: &[(&str, Result<&str, ErrorKind>)] = &[
        (
            "<!DOCTYPE r [<!ENTITY e \"x\">]><r a=\"&e;é\"/>",
            Ok("<r a=\"&e;&#233;\"/>"),
        ),
        (
            "<!DOCTYPE r [<!ENTITY é \"x\">]><r a=\"&é;\"/>",
            Err(ErrorKind::InvalidCharacter),
        ),
    ];
    for &(input, expected) in cases {
        let output = saved_with(input, 0, &ascii);
        match expected {
            Ok(line) => assert_eq!(output.unwrap().lines().last(), Some(line), "{input}"),
            Err(kind) => assert_eq!(output.unwrap_err(), kind, "{input}"),
        }
    }
}

#[test]
fn entity_value_references_are_markup_in_single_byte_encodings() {
    let encoded = |input: &str, encoding| {
        let document = OwnedDocument::parse(input).unwrap();
        let options = save::Options {
            encoding: Some(encoding),
            ..options(save::NO_DECL)
        };
        save::to_vec(&document, &options)
            .map(|bytes| {
                bytes
                    .iter()
                    .map(|&byte| char::from(byte))
                    .collect::<String>()
            })
            .map_err(|error| error.kind)
    };
    // Outside references the encoder writes `&#N;`; inside one it fails.
    let lacking = "<!DOCTYPE r [<!ENTITY a \"v\"><!ENTITY e \"\u{20ac}&a;\">]><r/>";
    assert!(
        encoded(lacking, "ISO-8859-1")
            .unwrap()
            .contains("<!ENTITY e \"&#8364;&a;\">")
    );
    // A bypassed general reference, in a general and a parameter entity.
    for kind in ["", "% "] {
        let input = format!("<!DOCTYPE r [<!ENTITY {kind}p \"&\u{20ac};\">]><r/>");
        assert_eq!(
            encoded(&input, "windows-1252").map(|_| ()),
            Ok(()),
            "{input}"
        );
        assert_eq!(
            encoded(&input, "ISO-8859-1"),
            Err(ErrorKind::InvalidCharacter),
            "{input}"
        );
    }
}

#[test]
fn edited_attribute_values_drop_their_references() {
    use yaxml::EditLimits;
    let input = "<!DOCTYPE r [<!ENTITY e \"x\">]><r a=\"&e;\" b=\"&e;\"/>";
    let mut document = OwnedDocument::parse(input).unwrap();
    let root = document.root_element();
    assert_eq!(root.unexpanded_attribute(0), Some("&e;"));
    assert_eq!(root.unexpanded_attribute(2), None);
    let id = root.id();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    editor.set_attribute_value(id, None, "a", "&e;").unwrap();
    let root = document.root_element();
    assert_eq!(root.unexpanded_attribute(0), None);
    assert_eq!(root.unexpanded_attribute(1), Some("&e;"));
    let output = save::to_vec(&document, &options(save::NO_DECL)).unwrap();
    assert!(output.ends_with(b"<r a=\"&amp;e;\" b=\"&e;\"/>\n"));
    // Other nodes have no attributes to keep.
    assert_eq!(root.parent().unwrap().unexpanded_attribute(0), None);
}

/// A sink taking at most `step` bytes per call that fails once `fail_at`
/// bytes are taken, or on flush.
struct Sink {
    bytes: Vec<u8>,
    step: usize,
    fail_at: usize,
    fail_flush: bool,
    writes: usize,
}

impl Sink {
    fn new(step: usize, fail_at: usize, fail_flush: bool) -> Self {
        Self {
            bytes: Vec::new(),
            step,
            fail_at,
            fail_flush,
            writes: 0,
        }
    }
}

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writes += 1;
        if self.bytes.len() >= self.fail_at {
            return Err(io::Error::other("full"));
        }
        let length = bytes
            .len()
            .min(self.step)
            .min(self.fail_at - self.bytes.len());
        self.bytes.extend_from_slice(&bytes[..length]);
        Ok(length)
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.fail_flush {
            true => Err(io::Error::other("flush")),
            false => Ok(()),
        }
    }
}

#[test]
fn to_writer_streams_what_to_vec_returns() {
    // Large enough for several chunks; one HTML document as well.
    let xml = format!("<r>{}</r>", "<a b='&amp;'>t\u{e9}</a>".repeat(8000));
    let owned = OwnedDocument::parse(&xml).unwrap();
    let borrowed = Document::parse(&xml).unwrap();
    let parsed = html::parse(xml.as_bytes(), html::Options::default()).unwrap();
    let latin1 = save::Options {
        encoding: Some("ISO-8859-1"),
        ..options(save::FORMAT)
    };
    let cases: &[(save::Tree<'_, '_>, save::Options<'_>)] = &[
        ((&owned).into(), options(0)),
        ((&borrowed).into(), options(save::FORMAT)),
        (owned.root_element().into(), latin1),
        ((&parsed.document).into(), options(0)),
        (parsed.document.root_element().into(), options(save::FORMAT)),
    ];
    for (tree, options) in cases {
        let expected = save::to_vec(*tree, options).unwrap();
        assert!(expected.len() > 3 * save::CHUNK);
        for step in [1, 7, usize::MAX] {
            let mut sink = Sink::new(step, usize::MAX, false);
            save::to_writer(*tree, options, &mut sink).unwrap();
            assert_eq!(sink.bytes, expected, "{options:?} {step}");
            if step == usize::MAX {
                // Bounded chunks: a few writes, not one per node or one in all.
                assert!(
                    (3..=expected.len() / 1000).contains(&sink.writes),
                    "{}",
                    sink.writes
                );
            }
        }
    }
}

#[test]
fn to_writer_reports_sink_and_encoding_failures() {
    let xml = format!("<r>{}<\u{105}/></r>", "<a>t</a>".repeat(9000));
    let document = OwnedDocument::parse(&xml).unwrap();
    let latin1 = save::Options {
        encoding: Some("ISO-8859-1"),
        ..options(0)
    };
    let expected = save::to_vec(&document, &options(0)).unwrap();
    // (options, fail_at, fail_flush, error, bytes the sink holds)
    let cases: &[(save::Options<'_>, usize, bool, Option<ErrorKind>, usize)] = &[
        (options(0), 10, false, None, 10),
        (options(0), usize::MAX, true, None, expected.len()),
        (options(0), 0, false, None, 0),
        // The element named outside Latin-1 fails; only whole chunks before
        // it (each a node past CHUNK at most) are written.
        (
            latin1,
            usize::MAX,
            false,
            Some(ErrorKind::InvalidCharacter),
            2 * save::CHUNK + 64,
        ),
    ];
    for (options, fail_at, fail_flush, kind, held) in cases {
        let mut sink = Sink::new(usize::MAX, *fail_at, *fail_flush);
        let error = save::to_writer(&document, options, &mut sink).unwrap_err();
        match (error, kind) {
            (WriteError::Io(_), None) => {}
            (WriteError::Xml(error), Some(kind)) => {
                assert_eq!(error.kind, *kind);
                assert!(error.offset >= sink.bytes.len());
            }
            (error, _) => panic!("{error:?}"),
        }
        assert!(sink.bytes.len() <= *held, "{} > {held}", sink.bytes.len());
    }
    // Option errors come first, without I/O.
    let mut sink = Sink::new(usize::MAX, 0, true);
    let bogus = save::Options {
        encoding: Some("x-bogus"),
        ..options(0)
    };
    assert!(matches!(
        save::to_writer(&document, &bogus, &mut sink),
        Err(WriteError::Xml(error)) if error.kind == ErrorKind::UnsupportedEncoding
    ));
    assert_eq!(sink.writes, 0);
}

#[test]
fn subtree_levels_indent_like_xml_node_dump() {
    let document = OwnedDocument::parse("<r><a><b>t</b><c/></a></r>").unwrap();
    let a = document.root_element().first_child().unwrap();
    // Expected bytes are libxml2's `xmlNodeDump(buf, doc, a, level, 1)`.
    let cases: &[(usize, u32, &str)] = &[
        (0, save::FORMAT, "<a>\n  <b>t</b>\n  <c/>\n</a>"),
        (2, save::FORMAT, "<a>\n      <b>t</b>\n      <c/>\n    </a>"),
        (2, 0, "<a><b>t</b><c/></a>"),
        // Clamped to 100 levels, then to the 60-column indentation limit.
        (
            500,
            save::FORMAT,
            &format!("<a>\n{0}<b>t</b>\n{0}<c/>\n{0}</a>", " ".repeat(60)),
        ),
    ];
    for (level, flags, expected) in cases {
        let options = save::Options {
            level: *level,
            ..options(*flags)
        };
        let saved = String::from_utf8(save::to_vec(a, &options).unwrap()).unwrap();
        assert_eq!(saved, *expected, "{level} {flags}");
    }
    // Documents start at level 0 regardless.
    let options = save::Options {
        level: 3,
        ..options(save::FORMAT | save::NO_DECL)
    };
    assert_eq!(
        save::to_vec(&document, &options).unwrap(),
        b"<r>\n  <a>\n    <b>t</b>\n    <c/>\n  </a>\n</r>\n"
    );
}
