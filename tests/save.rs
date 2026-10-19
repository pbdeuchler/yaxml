//! `save::to_vec`: options, encodings, subtrees, the regenerated internal
//! subset and XHTML rules. Expected bytes are pinned libxml2 2.15.4
//! `xmllint --noent --dtdattr` output where xmllint exposes the option.
#![forbid(unsafe_code)]

use yaxml::{Document, ErrorKind, OwnedDocument, html, save};

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
    assert_eq!(
        save::to_vec(document.root_element(), &options(save::AS_HTML))
            .unwrap_err()
            .kind,
        ErrorKind::UnsupportedOption
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
    let parsed = html::parse(b"<p class=x>caf&eacute;<br>", html::Options::default()).unwrap();
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
         \"http://www.w3.org/TR/REC-html40/loose.dtd\">\n<html><body><p class=\"x\">caf&#xE9;<br/></p></body></html>\n"
    );
    let p = document
        .root_element()
        .first_child()
        .unwrap()
        .first_child()
        .unwrap();
    assert_eq!(
        save::to_vec(p, &options(0)).unwrap_err().kind,
        ErrorKind::UnsupportedOption
    );
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
