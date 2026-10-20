#![forbid(unsafe_code)]
//! The read-only DTD declaration views (`valid::Dtd::entity` and friends,
//! libxml2's `xmlGetDocEntity`, `xmlGetDtdElementDesc`, ...) over both DOMs
//! and `valid::Dtd::load`, and the `save::encode_*` escapers.

use std::collections::HashMap;

use yaxml::valid::{
    self, AttributeDefault, AttributeKind, ContentKind, Dtd, Entity, EntityKind, Notation,
};
use yaxml::{Document, OwnedDocument, ParseOptions, Request, Resolver, options, save};

const SUBSET: &str = r#"<!DOCTYPE r SYSTEM "ext.dtd" [
<!NOTATION gif PUBLIC "-//GIF//EN" "viewer">
<!NOTATION txt SYSTEM "text">
<!NOTATION gif SYSTEM "again">
<!ENTITY % pe "<!ENTITY fromPe 'p'>">
%pe;
<!ENTITY txt "a&#38;b &amp; &other; &#x3c;">
<!ENTITY other "o">
<!ENTITY txt "repeat">
<!ENTITY file SYSTEM "sub/file.ent">
<!ENTITY pic PUBLIC "-//PIC//EN" "pic.gif" NDATA gif>
<!ENTITY % ext SYSTEM "ext.pe">
%ext;
<!ENTITY lt "&#38;#60;">
<!ELEMENT r (a , (b | c)*)>
<!ELEMENT a (#PCDATA | b)*>
<!ELEMENT b EMPTY>
<!ELEMENT c ANY>
<!ATTLIST r id ID #REQUIRED
            kind (x | y) "x"
            ref IDREF #IMPLIED
            fixed CDATA #FIXED "f &other;"
            n NOTATION (gif | txt) #IMPLIED>
<!ATTLIST d x CDATA "1">
]><r id="i"/>"#;

const EXT: &str = "<!ENTITY outside 'out'><!ENTITY other 'ignored'><!ELEMENT d EMPTY>";

fn resolver() -> Box<dyn Resolver> {
    let files: HashMap<&str, &str> = [
        ("dir/ext.dtd", EXT),
        ("dir/sub/file.ent", "file text"),
        ("dir/ext.pe", "<!ENTITY viaExt 'v'>"),
    ]
    .into();
    Box::new(move |request: Request<'_>| {
        Ok(files.get(request.uri).map(|text| text.as_bytes().to_vec()))
    })
}

fn options(flags: u32) -> ParseOptions {
    ParseOptions::new(flags).with_resolver("dir/doc.xml", resolver())
}

fn entity<'a>(dtd: &'a Dtd, name: &str) -> Option<Entity<'a>> {
    dtd.entity(name)
}

/// One case: the lookup and what it should find.
type Case = (
    &'static str,
    fn(&Dtd) -> Option<String>,
    Option<&'static str>,
);

fn show_entity(entity: Option<Entity<'_>>) -> Option<String> {
    let e = entity?;
    Some(format!(
        "{:?} {:?} {:?} {:?} {:?} {:?} ext={}",
        e.kind, e.content, e.public_id, e.system_id, e.uri, e.notation, e.external_subset
    ))
}

const CASES: &[Case] = &[
    (
        "internal general",
        |d| show_entity(entity(d, "txt")),
        Some(r#"InternalGeneral Some("a&b &amp; &other; <") None None None None ext=false"#),
    ),
    (
        "declared in a parameter entity",
        |d| show_entity(entity(d, "fromPe")),
        Some(r#"InternalGeneral Some("p") None None None None ext=false"#),
    ),
    (
        "external parsed, not referenced",
        |d| show_entity(entity(d, "file")),
        Some(
            r#"ExternalParsedGeneral None None Some("sub/file.ent") Some("dir/sub/file.ent") None ext=false"#,
        ),
    ),
    (
        "unparsed: content is the notation",
        |d| show_entity(entity(d, "pic")),
        Some(
            r#"ExternalUnparsedGeneral Some("gif") Some("-//PIC//EN") Some("pic.gif") Some("dir/pic.gif") Some("gif") ext=false"#,
        ),
    ),
    (
        "declared in an external parameter entity",
        |d| show_entity(entity(d, "viaExt")),
        Some(r#"InternalGeneral Some("v") None None None None ext=false"#),
    ),
    (
        "external subset",
        |d| show_entity(entity(d, "outside")),
        Some(r#"InternalGeneral Some("out") None None None None ext=true"#),
    ),
    (
        "first declaration wins",
        |d| show_entity(entity(d, "other")),
        Some(r#"InternalGeneral Some("o") None None None None ext=false"#),
    ),
    (
        "redeclared predefined",
        |d| show_entity(entity(d, "lt")),
        Some(r##"InternalGeneral Some("&#60;") None None None None ext=false"##),
    ),
    (
        "predefined fallback",
        |d| show_entity(entity(d, "quot")),
        Some(r#"Predefined Some("\"") None None None None ext=false"#),
    ),
    ("undeclared", |d| show_entity(entity(d, "nothing")), None),
    (
        "a parameter entity is not general",
        |d| show_entity(entity(d, "pe")),
        None,
    ),
    (
        "internal parameter",
        |d| show_entity(d.parameter_entity("pe")),
        Some(r#"InternalParameter Some("<!ENTITY fromPe 'p'>") None None None None ext=false"#),
    ),
    (
        "external parameter",
        |d| show_entity(d.parameter_entity("ext")),
        Some(
            r#"ExternalParameter Some("<!ENTITY viaExt 'v'>") None Some("ext.pe") Some("dir/ext.pe") None ext=false"#,
        ),
    ),
    (
        "a general entity is not a parameter",
        |d| show_entity(d.parameter_entity("txt")),
        None,
    ),
    (
        "element content",
        |d| {
            d.element("r")
                .map(|e| format!("{:?} {:?}", e.kind, e.model))
        },
        Some(r#"Element Some("(a , (b | c)*)")"#),
    ),
    (
        "mixed content",
        |d| {
            d.element("a")
                .map(|e| format!("{:?} {:?}", e.kind, e.model))
        },
        Some(r#"Mixed Some("(#PCDATA | b)*")"#),
    ),
    (
        "empty",
        |d| {
            d.element("b")
                .map(|e| format!("{:?} {:?}", e.kind, e.model))
        },
        Some("Empty None"),
    ),
    (
        "any",
        |d| {
            d.element("c")
                .map(|e| format!("{:?} {:?}", e.kind, e.model))
        },
        Some("Any None"),
    ),
    (
        "declared in the external subset after an attribute list",
        |d| {
            d.element("d")
                .map(|e| format!("{:?} {}", e.kind, e.attributes().count()))
        },
        Some("Empty 1"),
    ),
    (
        "undeclared element",
        |d| d.element("z").map(|e| e.name.to_owned()),
        None,
    ),
    (
        "required ID",
        |d| {
            d.attribute("r", "id")
                .map(|a| format!("{:?} {:?}", a.kind, a.default))
        },
        Some("Id Required"),
    ),
    (
        "enumeration default",
        |d| {
            d.attribute("r", "kind")
                .map(|a| format!("{:?} {:?} {:?}", a.kind, a.default, a.values))
        },
        Some(r#"Enumeration Value("x") ["x", "y"]"#),
    ),
    (
        "implied IDREF",
        |d| {
            d.attribute("r", "ref")
                .map(|a| format!("{:?} {:?}", a.kind, a.default))
        },
        Some("IdRef Implied"),
    ),
    (
        "fixed keeps references",
        |d| {
            d.attribute("r", "fixed")
                .map(|a| format!("{:?}", a.default))
        },
        Some(r#"Fixed("f &other;")"#),
    ),
    (
        "notation type",
        |d| {
            d.attribute("r", "n")
                .map(|a| format!("{:?} {:?} {}", a.kind, a.values, a.element))
        },
        Some(r#"Notation ["gif", "txt"] r"#),
    ),
    (
        "undeclared attribute",
        |d| d.attribute("r", "zz").map(|a| a.name.to_owned()),
        None,
    ),
    (
        "attribute of an undeclared element",
        |d| d.attribute("z", "id").map(|a| a.name.to_owned()),
        None,
    ),
    (
        "public notation, first wins",
        |d| d.notation("gif").map(|n| format!("{n:?}")),
        Some(
            r#"Notation { name: "gif", public_id: Some("-//GIF//EN"), system_id: Some("viewer") }"#,
        ),
    ),
    (
        "undeclared notation",
        |d| d.notation("png").map(|n| n.name.to_owned()),
        None,
    ),
];

fn check(dtd: &Dtd, surface: &str) {
    for (name, lookup, expected) in CASES {
        assert_eq!(lookup(dtd).as_deref(), *expected, "{surface}: {name}");
    }
    let names = |kind: bool| -> Vec<&str> {
        dtd.entities()
            .filter(|e| {
                matches!(
                    e.kind,
                    EntityKind::InternalParameter | EntityKind::ExternalParameter
                ) == kind
            })
            .map(|e| e.name)
            .collect()
    };
    assert_eq!(
        names(false),
        [
            "fromPe", "txt", "other", "file", "pic", "viaExt", "lt", "outside"
        ],
        "{surface}"
    );
    assert_eq!(names(true), ["pe", "ext"], "{surface}");
    let elements: Vec<_> = dtd.elements().map(|e| (e.name, e.kind)).collect();
    assert_eq!(
        elements,
        [
            ("r", ContentKind::Element),
            ("a", ContentKind::Mixed),
            ("b", ContentKind::Empty),
            ("c", ContentKind::Any),
            ("d", ContentKind::Empty)
        ],
        "{surface}"
    );
    let attributes: Vec<_> = dtd
        .element("r")
        .unwrap()
        .attributes()
        .map(|a| a.name)
        .collect();
    assert_eq!(attributes, ["id", "kind", "ref", "fixed", "n"], "{surface}");
    let notations: Vec<_> = dtd.notations().map(|n| n.name).collect();
    assert_eq!(notations, ["gif", "txt"], "{surface}");
}

#[test]
fn both_doms_expose_every_declaration() {
    let flags = options::DTDLOAD | options::DTDATTR;
    let owned = OwnedDocument::parse_with(SUBSET.as_bytes(), options(flags)).unwrap();
    check(owned.dtd().unwrap(), "owned");
    let borrowed = Document::parse_with(SUBSET.as_bytes(), options(flags)).unwrap();
    check(borrowed.dtd().unwrap(), "borrowed");
}

#[test]
fn substitution_expands_stored_defaults_and_unloaded_entities_have_no_text() {
    let referenced = SUBSET.replace(r#"<r id="i"/>"#, r#"<r id="i">&file;</r>"#);
    let owned = OwnedDocument::parse_with(
        referenced.as_bytes(),
        options(options::DTDLOAD | options::NOENT),
    )
    .unwrap();
    let dtd = owned.dtd().unwrap();
    assert_eq!(
        dtd.attribute("r", "fixed").unwrap().default,
        AttributeDefault::Fixed("f o")
    );
    assert_eq!(
        dtd.entity("file").unwrap().content,
        Some("file text"),
        "loaded when referenced"
    );
    // Without a resolver nothing loads, and the URI is the system identifier.
    let plain = OwnedDocument::parse(SUBSET).unwrap();
    let dtd = plain.dtd().unwrap();
    let file = dtd.entity("file").unwrap();
    assert_eq!((file.content, file.uri), (None, Some("sub/file.ent")));
    assert!(dtd.entity("outside").is_none(), "external subset not read");
    assert!(
        dtd.element("d")
            .is_some_and(|d| d.kind == ContentKind::Undefined && d.model.is_none())
    );
}

#[test]
fn standalone_documents_do_not_find_external_general_entities() {
    let input = r#"<?xml version="1.0" standalone="yes"?><!DOCTYPE r SYSTEM "ext.dtd" [<!ENTITY in "i">]><r/>"#;
    let owned = OwnedDocument::parse_with(input.as_bytes(), options(options::DTDLOAD)).unwrap();
    let dtd = owned.dtd().unwrap();
    assert!(dtd.entity("in").is_some());
    assert!(
        dtd.entity("outside").is_none(),
        "xmlGetDocEntity skips the external subset"
    );
    let listed = dtd.entities().find(|e| e.name == "outside").unwrap();
    assert!(listed.external_subset, "xmlGetDtdEntity still finds it");
}

#[test]
fn documents_without_a_doctype_have_no_dtd() {
    assert!(OwnedDocument::parse("<r/>").unwrap().dtd().is_none());
    assert!(Document::parse("<r/>").unwrap().dtd().is_none());
}

#[test]
fn loaded_dtds_expose_their_declarations() {
    let dtd = Dtd::parse(EXT.as_bytes()).unwrap();
    let outside = dtd.entity("outside").unwrap();
    assert_eq!(
        (outside.content, outside.external_subset),
        (Some("out"), true)
    );
    assert_eq!(dtd.element("d").map(|d| d.kind), Some(ContentKind::Empty));
    assert!(dtd.notation("gif").is_none());
    assert!(Dtd::parse(b"<!ENTITY broken").is_err());
}

#[test]
fn predefined_entities() {
    let cases: [(&str, Option<&str>); 7] = [
        ("lt", Some("<")),
        ("gt", Some(">")),
        ("amp", Some("&")),
        ("apos", Some("'")),
        ("quot", Some("\"")),
        ("Lt", None),
        ("", None),
    ];
    for (name, content) in cases {
        let entity = valid::predefined_entity(name);
        assert_eq!(entity.and_then(|e| e.content), content, "{name}");
        assert!(entity.is_none_or(|e| e.kind == EntityKind::Predefined && e.name == name));
    }
}

#[test]
fn discriminants_are_libxml2_values() {
    assert_eq!(
        [
            EntityKind::InternalGeneral,
            EntityKind::ExternalParsedGeneral,
            EntityKind::ExternalUnparsedGeneral,
            EntityKind::InternalParameter,
            EntityKind::ExternalParameter,
            EntityKind::Predefined
        ]
        .map(|k| k as u8),
        [1, 2, 3, 4, 5, 6]
    );
    assert_eq!(
        [
            ContentKind::Undefined,
            ContentKind::Empty,
            ContentKind::Any,
            ContentKind::Mixed,
            ContentKind::Element
        ]
        .map(|k| k as u8),
        [0, 1, 2, 3, 4]
    );
    assert_eq!(
        [
            AttributeKind::Cdata,
            AttributeKind::Enumeration,
            AttributeKind::Notation
        ]
        .map(|k| k as u8),
        [1, 9, 10]
    );
    let _: Option<Notation<'_>> = None;
}

#[test]
fn encoders_follow_xml_escape_text() {
    let cases: [(&str, save::Escape, &str); 9] = [
        ("a<b>&c\"'", save::Escape::Xml, "a&lt;b&gt;&amp;c\"'"),
        ("x\r\n\ty", save::Escape::Xml, "x&#13;\n\ty"),
        ("\u{1}\u{1f}", save::Escape::Xml, "&#xFFFD;&#xFFFD;"),
        ("caf\u{e9}", save::Escape::Xml, "caf\u{e9}"),
        (
            "caf\u{e9} \u{1f600}",
            save::Escape::Ascii,
            "caf&#xE9; &#x1F600;",
        ),
        ("\u{fffe}\u{ffff}", save::Escape::Ascii, "&#xFFFD;&#xFFFD;"),
        (
            "a<\r\u{1}\u{e9}\"",
            save::Escape::Html,
            "a&lt;\r\u{1}\u{e9}\"",
        ),
        ("", save::Escape::Ascii, ""),
        ("plain", save::Escape::Html, "plain"),
    ];
    for (text, escape, expected) in cases {
        assert_eq!(
            save::encode_entities(text, escape),
            expected,
            "{text:?} {escape:?}"
        );
    }
    let special: [(&str, &str); 4] = [
        ("<\"&>", "&lt;&quot;&amp;&gt;"),
        ("\r\u{e9}'", "&#13;\u{e9}'"),
        ("\u{2}", "&#xFFFD;"),
        ("", ""),
    ];
    for (text, expected) in special {
        assert_eq!(save::encode_special_chars(text), expected, "{text:?}");
    }
}
