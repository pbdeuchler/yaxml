//! libxml2's debugging dumps (feature `debug`), pinned against
//! `xmllint --debug` 2.15.4 (` compact`/` interned` notes and the `URL=`
//! line dropped: these parses have no URL).

use yaxml::{EditLimits, OwnedDocument, ParseOptions, debug, html};

/// (name, input, `xmllint --debug` output).
const XML: &[(&str, &str, &str)] = &[
    (
        "rich",
        r#"<!DOCTYPE r [
<!-- c1 -->
<!ENTITY % pe "<!ENTITY inner 'i'>">
%pe;
<!ENTITY a "va">
<!ENTITY b "v&a;b">
<!ENTITY unused "zz">
<!NOTATION n SYSTEM "n">
<!ENTITY u SYSTEM "u.bin" NDATA n>
<!ENTITY ext SYSTEM "ext.xml">
<!ATTLIST r x CDATA "a&a;" y (p|q|r|s|t|u|v) #IMPLIED z NOTATION (n) #IMPLIED w ID #REQUIRED>
<!ATTLIST r x CDATA "dup">
<!ELEMENT r (#PCDATA|e)*>
<!ELEMENT e ((r,e)?|(e+))>
<!ATTLIST q k CDATA #FIXED "f">
<?pi in dtd?>
]>
<r x="&a;!" w="i1" xml:lang="en">&b;&#233;&#x41;é long text that is more than forty characters long ok<e/>&ext;
</r>"#,
        r#"DOCUMENT
version=1.0
standalone=true
namespace xml href=http://www.w3.org/XML/1998/namespace
  DTD(r)
    COMMENT
      content= c1 
    ENTITYDECL(pe), parameter
     content=<!ENTITY inner 'i'>
    ENTITYDECL(inner), internal
     content=i
    ENTITYDECL(a), internal
     content=va
      TEXT
        content=va
    ENTITYDECL(b), internal
     content=v&a;b
      TEXT
        content=v
      ENTITY_REF(a)
        INTERNAL_GENERAL_ENTITY a
        content=va
      TEXT
        content=b
    ENTITYDECL(unused), internal
     content=zz
    ENTITYDECL(u), unparsed
     SystemID=u.bin
     URI=u.bin
     content=n
    ENTITYDECL(ext), external parsed
     SystemID=ext.xml
     URI=ext.xml
    ATTRDECL(x) for r CDATA"a&a;"
    ATTRDECL(y) for r ENUMERATION (p|q|r|s|t...) IMPLIED
    ATTRDECL(z) for r NOTATION  (n) IMPLIED
    ATTRDECL(w) for r ID REQUIRED
    ELEMDECL(r), MIXED (#PCDATA | e)*
    ELEMDECL(e), MIXED ((r , e)? | e+)
    ATTRDECL(k) for q CDATA FIXED"f"
    PI pi
      content=in dtd
  ELEMENT r
    ATTRIBUTE x
      ENTITY_REF(a)
        INTERNAL_GENERAL_ENTITY a
        content=va
      TEXT
        content=!
    ATTRIBUTE w
      TEXT
        content=i1
    ATTRIBUTE lang
      TEXT
        content=en
    ENTITY_REF(b)
      INTERNAL_GENERAL_ENTITY b
      content=v&a;b
    TEXT
      content=#C3#A9A#C3#A9 long text that is more than forty ...
    ELEMENT e
    ENTITY_REF(ext)
      EXTERNAL_GENERAL_PARSED_ENTITY ext
      SystemID=ext.xml
      URI=ext.xml
    TEXT
      content= 
"#,
    ),
    (
        "ns",
        r#"<?xml version="1.0" encoding="ISO-8859-1" standalone="no"?>
<p:r xmlns:p="urn:p" xmlns="urn:d" xml:lang="en"><c p:a="1" b="&amp;&#38;&lt;"/><![CDATA[x<y]]><?t?><?u  data here?><!--	c
--></p:r>"#,
        r#"DOCUMENT
version=1.0
encoding=ISO-8859-1
namespace xml href=http://www.w3.org/XML/1998/namespace
  ELEMENT p:r
    namespace p href=urn:p
    default namespace href=urn:d
    ATTRIBUTE lang
      TEXT
        content=en
    ELEMENT c
      ATTRIBUTE a
        TEXT
          content=1
      ATTRIBUTE b
        TEXT
          content=&&<
    CDATA_SECTION
      content=x<y
    PI t
    PI u
      content=data here
    COMMENT
      content= c 
"#,
    ),
    (
        "long",
        r#"<r>café 0123456789012345678901234567890123456789</r>"#,
        r#"DOCUMENT
version=1.0
standalone=true
  ELEMENT r
    TEXT
      content=caf#C3#A9 0123456789012345678901234567890123...
"#,
    ),
    (
        "exact",
        r#"<r>0123456789012345678901234567890123456789</r>"#,
        r#"DOCUMENT
version=1.0
standalone=true
  ELEMENT r
    TEXT
      content=0123456789012345678901234567890123456789...
"#,
    ),
    (
        "short",
        r#"<r>012345678901234567890123456789012345678</r>"#,
        r#"DOCUMENT
version=1.0
standalone=true
  ELEMENT r
    TEXT
      content=012345678901234567890123456789012345678
"#,
    ),
    (
        "attr",
        r#"<!DOCTYPE r [<!ENTITY e "x&f;y"><!ENTITY f "t">]><r a="&e;"/>"#,
        r#"DOCUMENT
version=1.0
standalone=true
  DTD(r)
    ENTITYDECL(e), internal
     content=x&f;y
      TEXT
        content=x
      ENTITY_REF(f)
        INTERNAL_GENERAL_ENTITY f
        content=t
      TEXT
        content=y
    ENTITYDECL(f), internal
     content=t
      TEXT
        content=t
  ELEMENT r
    ATTRIBUTE a
      ENTITY_REF(e)
        INTERNAL_GENERAL_ENTITY e
        content=x&f;y
"#,
    ),
];

/// (input, `xmllint --html --debug` output).
const HTML: &[(&str, &str)] = &[
    (
        r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>t</title></head><body><p class=a id="b">x &amp; y<br>z<!-- c --></p><script>if(a<b)x()</script></body></html>"#,
        r#"HTML DOCUMENT
encoding=utf-8
standalone=true
  DTD(html)
  ELEMENT html
    ELEMENT head
      ELEMENT meta
        ATTRIBUTE charset
          TEXT
            content=utf-8
      ELEMENT title
        TEXT
          content=t
    ELEMENT body
      ELEMENT p
        ATTRIBUTE class
          TEXT
            content=a
        ATTRIBUTE id
          TEXT
            content=b
        TEXT
          content=x & y
        ELEMENT br
        TEXT
          content=z
        COMMENT
          content= c 
      ELEMENT script
        CDATA_SECTION
          content=if(a<b)x()
"#,
    ),
    (
        r#"<p>caf&eacute; <a href="u">l</a>"#,
        r#"HTML DOCUMENT
standalone=true
  DTD(html), PUBLIC -//W3C//DTD HTML 4.0 Transitional//EN, SYSTEM http://www.w3.org/TR/REC-html40/loose.dtd
  ELEMENT html
    ELEMENT body
      ELEMENT p
        TEXT
          content=caf#C3#A9 
        ELEMENT a
          ATTRIBUTE href
            TEXT
              content=u
          TEXT
            content=l
"#,
    ),
    (
        r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Strict//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd"><html xmlns="http://www.w3.org/1999/xhtml"><body><p xml:lang="en">t</p></body></html>"#,
        r#"HTML DOCUMENT
standalone=true
  DTD(html), PUBLIC -//W3C//DTD XHTML 1.0 Strict//EN, SYSTEM http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd
  ELEMENT html
    ATTRIBUTE xmlns
      TEXT
        content=http://www.w3.org/1999/xhtml
    ELEMENT body
      ELEMENT p
        ATTRIBUTE xml:lang
          TEXT
            content=en
        TEXT
          content=t
"#,
    ),
    (
        r#"<table><tr><td>1<td>2</table><?pi x>"#,
        r#"HTML DOCUMENT
standalone=true
  DTD(html), PUBLIC -//W3C//DTD HTML 4.0 Transitional//EN, SYSTEM http://www.w3.org/TR/REC-html40/loose.dtd
  ELEMENT html
    ELEMENT body
      ELEMENT table
        ELEMENT tr
          ELEMENT td
            TEXT
              content=1
          ELEMENT td
            TEXT
              content=2
      COMMENT
        content=?pi x
"#,
    ),
];

#[test]
fn documents_dump_like_xmllint_debug() {
    for &(name, input, expected) in XML {
        let document = OwnedDocument::parse(input).unwrap();
        assert_eq!(debug::dump_document(&document), expected, "{name}");
        assert_eq!(
            debug::check_document(&document),
            (0, String::new()),
            "{name}"
        );
    }
    for &(input, expected) in HTML {
        let document = html::parse(input.as_bytes(), html::Options::DEFAULT)
            .unwrap()
            .document;
        assert_eq!(debug::dump_document(&document), expected, "{input}");
    }
    let options = ParseOptions::default().base(Some("a.xml".into()));
    let document = OwnedDocument::parse_with(b"<r/>", options).unwrap();
    assert_eq!(
        debug::dump_document_head(&document),
        "DOCUMENT\nversion=1.0\nURL=a.xml\nstandalone=true\n"
    );
}

const PARTS: &str = r#"<!DOCTYPE r [<!ENTITY e "v"><!ENTITY x SYSTEM "x.xml">]><r xmlns:p="urn:p" p:a="1" b="2"><c/>t<!--k--></r>"#;

#[test]
fn partial_dumps_follow_debugxml() {
    let document = OwnedDocument::parse(PARTS).unwrap();
    let parse = |input: &str| OwnedDocument::parse(input).unwrap();
    let root = document.document_element().unwrap();
    let child = root.first_child().unwrap();
    let (x39, x40) = ("x".repeat(39), format!("{}x...", "x".repeat(39)));
    let cases: Vec<(&str, String, &str)> = vec![
        ("empty string", debug::dump_string(""), ""),
        ("blanks", debug::dump_string("a\tb\nc\rd e"), "a b c d e"),
        ("high bytes", debug::dump_string("é"), "#C3#A9"),
        ("39 bytes", debug::dump_string(&x39), &x39),
        ("40 bytes", debug::dump_string(&format!("{x39}x")), &x40),
        (
            "head",
            debug::dump_document_head(&document),
            "DOCUMENT\nversion=1.0\nstandalone=true\n",
        ),
        (
            "dtd",
            debug::dump_dtd(&document),
            "DTD(r)\n  ENTITYDECL(e), internal\n   content=v\n  ENTITYDECL(x), external parsed\n   \
             SystemID=x.xml\n   URI=x.xml\n",
        ),
        (
            "empty dtd",
            debug::dump_dtd(&parse("<!DOCTYPE r []><r/>")),
            "DTD(r)\n    DTD is empty\n",
        ),
        (
            "external dtd only",
            debug::dump_dtd(&parse("<!DOCTYPE r SYSTEM 'r.dtd'><r/>")),
            "DTD(r), SYSTEM r.dtd\n    DTD is empty\n",
        ),
        ("no dtd", debug::dump_dtd(&parse("<r/>")), "DTD is NULL\n"),
        (
            "entities",
            debug::dump_entities(&document),
            "DOCUMENT\nEntities in internal subset\ne : INTERNAL GENERAL, \n content \"v\"\n\
             x : EXTERNAL PARSED, SYSTEM \"x.xml\"\nNo entities in external subset\n",
        ),
        (
            "no entities",
            debug::dump_entities(&parse("<r/>")),
            "DOCUMENT\nNo entities in internal subset\nNo entities in external subset\n",
        ),
        (
            "one node",
            debug::dump_one_node(root, 1),
            "  ELEMENT r\n    namespace p href=urn:p\n    ATTRIBUTE a\n      TEXT\n        content=1\n    \
             ATTRIBUTE b\n      TEXT\n        content=2\n",
        ),
        (
            "node list",
            debug::dump_node_list(child, 0),
            "ELEMENT c\nTEXT\n  content=t\nCOMMENT\n  content=k\n",
        ),
        (
            "attribute",
            debug::dump_attribute(root, 1, 0),
            "ATTRIBUTE a\n  TEXT\n    content=1\n",
        ),
        (
            "no attribute",
            debug::dump_attribute(root, 9, 1),
            "  Attr is NULL",
        ),
        (
            "attributes",
            debug::dump_attributes(root, 0),
            "ATTRIBUTE a\n  TEXT\n    content=1\nATTRIBUTE b\n  TEXT\n    content=2\n",
        ),
        ("no attributes", debug::dump_attributes(child, 0), ""),
    ];
    for (name, actual, expected) in cases {
        assert_eq!(actual, expected, "{name}");
    }
}

#[test]
fn checks_report_detached_roots_and_non_names() {
    let mut document = OwnedDocument::parse("<r><c>t</c></r>").unwrap();
    assert_eq!(
        debug::dump_node(document.root(), 0),
        "Error, DOCUMENT found here\nERROR 5013: Node has no parent\n  ELEMENT r\n    ELEMENT c\n      \
         TEXT\n        content=t\n"
    );
    let child = document
        .document_element()
        .unwrap()
        .first_child()
        .unwrap()
        .id();
    document
        .edit(EditLimits::DEFAULT)
        .unwrap()
        .unlink(child)
        .unwrap();
    let detached = document.node(child).unwrap();
    assert_eq!(
        debug::dump_node(detached, 0),
        "ELEMENT c\nERROR 5013: Node has no parent\n  TEXT\n    content=t\n"
    );
    assert_eq!(debug::check_document(&document), (0, String::new()));
    // The HTML parser keeps attribute names XML would reject.
    let html = html::parse(b"<p 1a=x>", html::Options::DEFAULT)
        .unwrap()
        .document;
    assert_eq!(
        debug::check_document(&html),
        (1, "ERROR 5034: Name is not an NCName '1a'".into())
    );
}

/// Documents whose dump differs for a documented reason: CRLF kept in an
/// entity value, or `content=` of a `<?pi ?>` without data.
const DIVERGENT: &[&str] = &[
    "ibm/valid/P16/ibm16v02.xml",
    "ibm/valid/P16/out/ibm16v01.xml",
    "ibm/valid/P16/out/ibm16v02.xml",
    "japanese/pr-xml-utf-8.xml",
    "oasis/p16pass1.xml",
    "xmltest/valid/sa/108.xml",
    "xmltest/valid/sa/out/016.xml",
    "xmltest/valid/sa/out/017.xml",
];

#[test]
#[ignore = "requires YAXML_XMLLINT (pinned xmllint 2.15.4) and YAXML_SUITES"]
fn xmlts_documents_dump_like_xmllint_debug() {
    let xmllint = std::env::var_os("YAXML_XMLLINT").expect("YAXML_XMLLINT");
    let suites = std::env::var_os("YAXML_SUITES").expect("YAXML_SUITES");
    let root = std::path::Path::new(&suites).join("xmlconf");
    let mut pending = vec![root.clone()];
    let (mut compared, mut differences) = (0, Vec::new());
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            pending.extend(
                std::fs::read_dir(&path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path()),
            );
            continue;
        }
        if path.extension().is_none_or(|extension| extension != "xml") {
            continue;
        }
        let output = std::process::Command::new(&xmllint)
            .arg("--debug")
            .arg(&path)
            .output()
            .unwrap();
        let input = std::fs::read(&path).unwrap();
        let options = ParseOptions::default().base(Some(path.display().to_string()));
        let (true, Ok(document)) = (
            output.status.success(),
            OwnedDocument::parse_with(&input, options),
        ) else {
            continue;
        };
        compared += 1;
        let expected: String = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| {
                let line = line.strip_suffix(" compact").unwrap_or(line);
                format!("{}\n", line.strip_suffix(" interned").unwrap_or(line))
            })
            .collect();
        let name = path.strip_prefix(&root).unwrap().display().to_string();
        if debug::dump_document(&document) != expected && !DIVERGENT.contains(&name.as_str()) {
            differences.push(name);
        }
    }
    assert!(compared > 1900, "compared {compared}");
    assert!(
        differences.is_empty(),
        "{} of {compared}: {differences:?}",
        differences.len()
    );
}
