#![forbid(unsafe_code)]
//! The read-only DTD declaration views (`valid::Dtd::entity` and friends,
//! libxml2's `xmlGetDocEntity`, `xmlGetDtdElementDesc`, ...) over both DOMs
//! and `valid::Dtd::load`, and the `save::encode_*` escapers.

use std::collections::HashMap;

use yaxml::valid::{
    self, AttributeDefault, AttributeKind, ContentKind, Dtd, Entity, EntityKind, Notation,
};
use yaxml::{
    Document, EditErrorKind, EditLimits, ErrorKind, OwnedDocument, ParseOptions, Request, Resolver,
    options, save,
};

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
fn detached_declarations_come_from_markup() {
    // xmlNewDtd(NULL, ..): an empty DTD; xmlNewEntity(NULL, ..): one entity.
    let empty = Dtd::parse(b"").unwrap();
    assert_eq!(empty.entities().count(), 0);
    assert!(empty.name().is_none());
    let one = Dtd::parse(b"<!ENTITY e 'x'>").unwrap();
    assert_eq!(one.entity("e").map(|e| e.content), Some(Some("x")));
    assert!(one.entity("f").is_none());
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
        assert_eq!(entity.and_then(|e| e.original), content, "{name}");
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

#[test]
fn values_match_libxml2_name_validators() {
    use yaxml::valid::value_matches;
    // (value, xmlValidateNameValue, NamesValue, NmtokenValue, NmtokensValue)
    // as libxml2 2.15.4 returns them.
    for (value, name, names, nmtoken, nmtokens) in [
        ("", false, false, false, false),
        ("a", true, true, true, true),
        ("1a", false, false, true, true),
        ("a b", false, true, false, true),
        ("a  b", false, true, false, true),
        (" a", false, false, false, true),
        ("a ", false, false, false, true),
        ("a\tb", false, false, false, false),
        ("\ta", false, false, false, true),
        ("a:b", true, true, true, true),
        ("-x", false, false, true, true),
        ("x-1.y", true, true, true, true),
        ("é", true, true, true, true),
        ("a b ", false, false, false, true),
        (".5", false, false, true, true),
        ("1 2", false, false, false, true),
    ] {
        let found = [
            AttributeKind::Id,
            AttributeKind::IdRefs,
            AttributeKind::NmToken,
            AttributeKind::NmTokens,
        ]
        .map(|kind| value_matches(kind, value));
        assert_eq!(found, [name, names, nmtoken, nmtokens], "{value:?}");
        assert!(value_matches(AttributeKind::Cdata, value), "{value:?}");
    }
}

fn saved(document: &OwnedDocument) -> String {
    String::from_utf8(save::to_vec(document, &save::Options::default()).unwrap()).unwrap()
}

#[test]
fn editors_add_declarations_after_the_documents() {
    // Outputs of xmlCreateIntSubset/xmlAddDocEntity/xmlAddNotationDecl and
    // xmlDocDumpMemory in libxml2 2.15.4 (the DOCTYPE takes the qualified name).
    let cases = [
        // xmlCreateIntSubset: a DOCTYPE named after the document element.
        (
            "<p:r xmlns:p='u'/>",
            "<!ENTITY e 'v'>",
            "<!DOCTYPE p:r [\n<!ENTITY e \"v\">\n]>\n<p:r xmlns:p=\"u\"/>\n",
        ),
        // The first declaration of a name wins; new ones follow.
        (
            "<!DOCTYPE r [<!ENTITY a 'x'>]><r/>",
            "<!ENTITY a 'y'><!ENTITY b 'z'>",
            "<!DOCTYPE r [\n<!ENTITY a \"x\">\n<!ENTITY b \"z\">\n]>\n<r/>\n",
        ),
        // Parameter entities declared before expand.
        (
            "<!DOCTYPE r [<!ENTITY % p '<!ELEMENT r EMPTY>'>]><r/>",
            "%p;<!ATTLIST r a CDATA 'd'>",
            "<!DOCTYPE r [\n<!ENTITY % p \"<!ELEMENT r EMPTY>\">\n<!ELEMENT r EMPTY>\n<!ATTLIST r a CDATA \"d\">\n]>\n<r/>\n",
        ),
        (
            "<?pi?><!--c--><r/>",
            "<!NOTATION n SYSTEM 'n'>",
            "<?pi?>\n<!--c-->\n<!DOCTYPE r [\n<!NOTATION n SYSTEM \"n\" >\n]>\n<r/>\n",
        ),
    ];
    for (input, markup, expected) in cases {
        let mut document = OwnedDocument::parse(input).unwrap();
        let mut editor = document.edit(EditLimits::default()).unwrap();
        editor.add_declarations(markup).unwrap();
        let expected = format!("<?xml version=\"1.0\"?>\n{expected}");
        assert_eq!(saved(&document), expected, "{markup}");
    }
}

#[test]
fn added_declarations_type_and_validate_later_edits() {
    let mut document = OwnedDocument::parse("<r><a id='x'/>t</r>").unwrap();
    let root = document.root_element().id();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    editor
        .add_declarations("<!ELEMENT r (a)*><!ELEMENT a EMPTY><!ATTLIST a id ID #IMPLIED>")
        .unwrap();
    assert!(editor.document().is_id("a", "id"));
    assert!(!editor.document().is_id("r", "id"));
    assert!(editor.document().is_id("r", "xml:id"));
    // As in libxml2, the attribute already in the tree is not retyped.
    assert!(editor.document().element_by_id("x").is_none());
    let a = editor.document().root_element().first_child().unwrap().id();
    editor.remove_attribute(a, None, "id").unwrap();
    let name = yaxml::NameSpec {
        local: "id",
        namespace: None,
        prefix: yaxml::PrefixPolicy::Automatic,
    };
    editor.set_attribute(a, name, "y").unwrap();
    assert_eq!(
        editor.document().element_by_id("y").map(|n| n.id()),
        Some(a)
    );
    let codes: Vec<_> = document.validate().iter().map(|d| d.code).collect();
    assert_eq!(codes.len(), 1, "text in element content of {root:?}");
}

#[test]
fn malformed_or_misplaced_declarations_change_nothing() {
    let parse = |kind| EditErrorKind::Parse(kind);
    let cases = [
        (
            "<r/>",
            "<!ENTITY a>",
            EditLimits::default(),
            parse(ErrorKind::InvalidDtd),
            Some(10),
        ),
        (
            "<r/>",
            "<!ENTITY a 'x'>]><x/>",
            EditLimits::default(),
            parse(ErrorKind::InvalidDtd),
            None,
        ),
        (
            "<r/>",
            "<!ELEMENT r EMPTY",
            EditLimits::default(),
            parse(ErrorKind::InvalidDtd),
            None,
        ),
        (
            "<!DOCTYPE r [<!ENTITY a 'x'>]><r/>",
            "&a;",
            EditLimits::default(),
            parse(ErrorKind::InvalidDtd),
            None,
        ),
        (
            "<r/>",
            "<!ENTITY a 'x'>",
            EditLimits::DEFAULT.max_created_nodes(0),
            EditErrorKind::LimitExceeded(yaxml::EditLimit::CreatedNodes),
            None,
        ),
        (
            "<r/>",
            "<!ENTITY a 'x'>",
            EditLimits::DEFAULT.max_value_bytes(4),
            EditErrorKind::LimitExceeded(yaxml::EditLimit::ValueBytes),
            None,
        ),
    ];
    for (input, markup, limits, kind, offset) in cases {
        let mut document = OwnedDocument::parse(input).unwrap();
        let before = saved(&document);
        let mut editor = document.edit(limits).unwrap();
        let error = editor.add_declarations(markup).unwrap_err();
        assert_eq!(error.kind, kind, "{markup}");
        if offset.is_some() {
            assert_eq!(error.value_offset, offset, "{markup}");
        }
        assert_eq!(saved(&document), before, "{markup}");
    }
}

/// A DTD edit, as a test case applies it.
#[derive(Clone, Copy, Debug)]
enum DtdEdit {
    Replace(&'static str),
    Remove,
    Ids(Option<&'static str>, Option<&'static str>),
}

fn apply(document: &mut OwnedDocument, edit: DtdEdit) -> Result<(), EditErrorKind> {
    let mut editor = document.edit(EditLimits::default()).unwrap();
    match edit {
        DtdEdit::Replace(markup) => editor.set_internal_subset(markup),
        DtdEdit::Remove => editor.remove_doctype(),
        DtdEdit::Ids(public, system) => editor.set_doctype_ids(public, system),
    }
    .map_err(|error| error.kind)
}

#[test]
fn editors_replace_and_remove_the_internal_subset() {
    // Outputs of libxml2 2.15.4 after freeing the declaration nodes and
    // xmlAddDocEntity, setting ExternalID/SystemID, or unlinking intSubset.
    let input = "<!DOCTYPE r SYSTEM 'r.dtd' [<!ENTITY a 'x'><!ELEMENT r EMPTY>]><r/>";
    let cases = [
        (
            input,
            DtdEdit::Replace("<!ENTITY b 'y'>"),
            "<!DOCTYPE r SYSTEM \"r.dtd\" [\n<!ENTITY b \"y\">\n]>\n<r/>\n",
        ),
        // A name declared before is redefined.
        (
            input,
            DtdEdit::Replace("<!ENTITY a 'z'>"),
            "<!DOCTYPE r SYSTEM \"r.dtd\" [\n<!ENTITY a \"z\">\n]>\n<r/>\n",
        ),
        (
            input,
            DtdEdit::Replace(""),
            "<!DOCTYPE r SYSTEM \"r.dtd\">\n<r/>\n",
        ),
        (
            "<r/>",
            DtdEdit::Replace("<!ELEMENT r EMPTY>"),
            "<!DOCTYPE r [\n<!ELEMENT r EMPTY>\n]>\n<r/>\n",
        ),
        (input, DtdEdit::Remove, "<r/>\n"),
        (
            input,
            DtdEdit::Ids(Some("-//P//EN"), Some("r.dtd")),
            "<!DOCTYPE r PUBLIC \"-//P//EN\" \"r.dtd\" [\n<!ENTITY a \"x\">\n<!ELEMENT r EMPTY>\n]>\n<r/>\n",
        ),
        (
            "<!DOCTYPE r PUBLIC '-//P//EN' 'r.dtd'><r/>",
            DtdEdit::Ids(None, Some("it's.dtd")),
            "<!DOCTYPE r SYSTEM \"it's.dtd\">\n<r/>\n",
        ),
        (
            "<!DOCTYPE r SYSTEM 'r.dtd'><r/>",
            DtdEdit::Ids(None, None),
            "<!DOCTYPE r>\n<r/>\n",
        ),
    ];
    for (input, edit, expected) in cases {
        let mut document = OwnedDocument::parse(input).unwrap();
        apply(&mut document, edit).unwrap();
        let expected = format!("<?xml version=\"1.0\"?>\n{expected}");
        assert_eq!(saved(&document), expected, "{edit:?}");
    }
    let mut document = OwnedDocument::parse(input).unwrap();
    apply(&mut document, DtdEdit::Replace("<!ENTITY b 'y'>")).unwrap();
    let dtd = document.dtd().unwrap();
    assert!(dtd.entity("a").is_none() && dtd.element("r").is_none());
    apply(&mut document, DtdEdit::Remove).unwrap();
    assert!(document.dtd().is_none() && document.doctype().is_none());
}

#[test]
fn invalid_predefined_redeclarations_are_dropped_with_a_warning() {
    use yaxml::Warning;
    // (subset, `lt`'s stored content, warnings, saved subset), from xmllint
    // 2.15.4: XML 1.0 section 4.6 allows only the character or its
    // reference, double-escaped for `lt` and `amp`.
    let cases: [(&str, Option<&str>, usize, &str); 6] = [
        ("<!ENTITY lt \"x\">", None, 1, ">"),
        ("<!ENTITY lt SYSTEM \"x.ent\">", None, 1, ">"),
        ("<!ENTITY lt \"<\">", None, 1, ">"),
        (
            "<!ENTITY lt \"&#38;#60;\">",
            Some("&#60;"),
            0,
            " [\n<!ENTITY lt \"&#38;#60;\">\n]>",
        ),
        (
            "<!ENTITY lt \"x\"><!ENTITY lt \"&#38;#x3C;\">",
            Some("&#x3C;"),
            1,
            " [\n<!ENTITY lt \"&#38;#x3C;\">\n]>",
        ),
        (
            "<!ENTITY % lt \"x\">",
            None,
            0,
            " [\n<!ENTITY % lt \"x\">\n]>",
        ),
    ];
    for (subset, content, warnings, saved_subset) in cases {
        let input = format!("<!DOCTYPE r [{subset}]><r>&lt;</r>");
        let document = OwnedDocument::parse(&input).unwrap();
        let entity = document.dtd().unwrap().entity("lt").unwrap();
        let stored = (entity.kind != EntityKind::Predefined).then_some(entity.content);
        assert_eq!(stored, content.map(Some), "{subset}");
        let kinds: Vec<_> = document.errors().iter().map(|error| error.kind).collect();
        let warning = ErrorKind::Warning(Warning::PredefinedEntity);
        assert_eq!(kinds, vec![warning; warnings], "{subset}");
        let expected = format!("<?xml version=\"1.0\"?>\n<!DOCTYPE r{saved_subset}\n<r>&lt;</r>\n");
        assert_eq!(saved(&document), expected, "{subset}");
        // An editor drops it the same way, without an error.
        let mut edited = OwnedDocument::parse("<!DOCTYPE r><r>&lt;</r>").unwrap();
        apply(&mut edited, DtdEdit::Replace(subset)).unwrap();
        assert_eq!(saved(&edited), expected, "edited {subset}");
    }
}

#[test]
fn external_subset_declarations_outlive_internal_edits() {
    let input = "<!DOCTYPE r SYSTEM 'ext.dtd' [<!ENTITY outside 'in'><!ENTITY gone 'g'>]><r/>";
    let cases: [(DtdEdit, Option<&str>, bool); 3] = [
        (
            DtdEdit::Replace("<!ENTITY outside 'new'>"),
            Some("new"),
            true,
        ),
        // The external declaration the internal one shadowed was dropped at
        // parse time (libxml2 keeps it in extSubset).
        (DtdEdit::Replace(""), None, true),
        (DtdEdit::Remove, None, false),
    ];
    for (edit, outside, doctype) in cases {
        let options = options(options::DTDLOAD);
        let mut document = OwnedDocument::parse_with(input.as_bytes(), options).unwrap();
        assert_eq!(
            document.dtd().unwrap().entity("outside").unwrap().content,
            Some("in")
        );
        apply(&mut document, edit).unwrap();
        let dtd = document.dtd().unwrap();
        let found = dtd.entity("outside").and_then(|entity| entity.content);
        assert_eq!(found, outside, "{edit:?}");
        assert!(dtd.entity("gone").is_none(), "{edit:?}");
        assert_eq!(dtd.entity("other").unwrap().content, Some("ignored"));
        assert_eq!(dtd.element("d").map(|d| d.kind), Some(ContentKind::Empty));
        assert_eq!(document.doctype().is_some(), doctype, "{edit:?}");
    }
}

#[test]
fn rejected_dtd_edits_change_nothing() {
    let input = "<!DOCTYPE r [<!ENTITY a 'x'>]><r/>";
    let cases = [
        (
            input,
            DtdEdit::Replace("<!ENTITY b>"),
            EditErrorKind::Parse(ErrorKind::InvalidDtd),
        ),
        (
            input,
            DtdEdit::Replace("&a;"),
            EditErrorKind::Parse(ErrorKind::InvalidDtd),
        ),
        ("<r/>", DtdEdit::Remove, EditErrorKind::WrongNodeKind),
        (
            "<r/>",
            DtdEdit::Ids(None, Some("s")),
            EditErrorKind::WrongNodeKind,
        ),
        (
            input,
            DtdEdit::Ids(Some("p"), None),
            EditErrorKind::InvalidValue,
        ),
        (
            input,
            DtdEdit::Ids(Some("p\"q"), Some("s")),
            EditErrorKind::InvalidValue,
        ),
        (
            input,
            DtdEdit::Ids(None, Some("'\"")),
            EditErrorKind::InvalidValue,
        ),
        (
            input,
            DtdEdit::Ids(None, Some("\u{1}")),
            EditErrorKind::InvalidCharacter,
        ),
    ];
    for (input, edit, kind) in cases {
        let mut document = OwnedDocument::parse(input).unwrap();
        let before = saved(&document);
        assert_eq!(apply(&mut document, edit), Err(kind), "{edit:?}");
        assert_eq!(saved(&document), before, "{edit:?}");
    }
}

#[test]
fn external_attribute_lists_and_notations_survive_a_replaced_subset() {
    let ext = "<!NOTATION n SYSTEM 'n'><!ATTLIST d x CDATA '1'><!ELEMENT d EMPTY>";
    let resolver = move |_: Request<'_>| Ok(Some(ext.as_bytes().to_vec()));
    let options = ParseOptions::new(options::DTDLOAD).with_resolver("doc.xml", Box::new(resolver));
    let input = "<!DOCTYPE r SYSTEM 'ext.dtd' [<!ATTLIST d y CDATA 'i'><!ELEMENT d ANY>]><r/>";
    let mut document = OwnedDocument::parse_with(input.as_bytes(), options).unwrap();
    apply(&mut document, DtdEdit::Replace("<!ATTLIST d z CDATA 'n'>")).unwrap();
    let dtd = document.dtd().unwrap();
    let d = dtd.element("d").unwrap();
    let names: Vec<_> = d.attributes().map(|attribute| attribute.name).collect();
    // The external <!ELEMENT d> lost to the internal one when read.
    assert_eq!((d.kind, names), (ContentKind::Undefined, vec!["z", "x"]));
    assert!(dtd.notation("n").is_some());
}

/// `Entity::original` is libxml2's `orig` (`xmlGetDocEntity(..)->orig` and
/// `xmlGetParameterEntity`, libxml2 2.15.4): the literal of the first
/// declaration, references unexpanded; none for external entities.
#[test]
fn originals_are_libxml2_orig() {
    const INTERNAL: &str = r#"<!DOCTYPE r [<!ENTITY % p "x&#37;"><!ENTITY e "a&#60;&f;z">
<!ENTITY f "plain"><!ENTITY f "second&#60;"><!ENTITY lt "&#38;#60;">
<!ENTITY % d "<!ENTITY g 'in&#38;#38;pe'>">%d;<!ENTITY u SYSTEM "u.xml"><!ENTITY ee "">]><r/>"#;
    const EXTERNAL: &str =
        "<!ENTITY % q 'Q&#60;'><!ENTITY ex 'ext&#38;&#60;%q;'><!ENTITY ok 'same'>";
    type Expected<'a> = &'a [(&'a str, Option<&'a str>, Option<&'a str>)];
    let cases: [(&str, u32, Expected); 2] = [
        (
            INTERNAL,
            0,
            &[
                ("e", Some("a&#60;&f;z"), Some("a<&f;z")),
                ("f", Some("plain"), Some("plain")),
                ("%p", Some("x&#37;"), Some("x%")),
                ("lt", Some("&#38;#60;"), Some("&#60;")),
                ("g", Some("in&#38;pe"), Some("in&pe")),
                ("u", None, None),
                ("ee", Some(""), Some("")),
                ("amp", Some("&"), Some("&")),
                (
                    "%d",
                    Some("<!ENTITY g 'in&#38;#38;pe'>"),
                    Some("<!ENTITY g 'in&#38;pe'>"),
                ),
                ("missing", None, None),
            ],
        ),
        (
            r#"<!DOCTYPE r SYSTEM "ext.dtd"><r/>"#,
            options::DTDLOAD,
            &[
                ("%q", Some("Q&#60;"), Some("Q<")),
                ("ex", Some("ext&#38;&#60;%q;"), Some("ext&<Q<")),
                ("ok", Some("same"), Some("same")),
            ],
        ),
    ];
    for (input, flags, expected) in cases {
        let resolver = move |request: Request<'_>| {
            Ok((request.uri == "dir/ext.dtd").then(|| EXTERNAL.as_bytes().to_vec()))
        };
        let options = ParseOptions::new(flags).with_resolver("dir/doc.xml", Box::new(resolver));
        let document = OwnedDocument::parse_with(input.as_bytes(), options).unwrap();
        let dtd = document.dtd().unwrap();
        for &(name, original, content) in expected {
            let entity = match name.strip_prefix('%') {
                Some(name) => dtd.parameter_entity(name),
                None => dtd.entity(name),
            };
            let found = entity.map_or((None, None), |e| (e.original, e.content));
            assert_eq!(found, (original, content), "{name}");
        }
    }
}

#[test]
fn notation_tables_print_as_libxml2_dumps_them() {
    // xmlDumpNotationTable output from libxml2 2.15.4 (its hash order
    // happens to match declaration order here).
    let cases = [
        (
            "<!NOTATION a SYSTEM 'x.gif'>",
            "<!NOTATION a SYSTEM \"x.gif\" >\n",
        ),
        ("<!NOTATION a PUBLIC 'p'>", "<!NOTATION a PUBLIC \"p\" >\n"),
        (
            "<!NOTATION a PUBLIC 'p' \"s'q\">",
            "<!NOTATION a PUBLIC \"p\" \"s'q\" >\n",
        ),
        (
            "<!NOTATION a SYSTEM '\"&amp;'>",
            "<!NOTATION a SYSTEM '\"&amp;' >\n",
        ),
        (
            "<!NOTATION b SYSTEM 'b'><!NOTATION a SYSTEM 'a'><!NOTATION b SYSTEM 'c'>",
            "<!NOTATION b SYSTEM \"b\" >\n<!NOTATION a SYSTEM \"a\" >\n",
        ),
        ("", ""),
    ];
    for (subset, expected) in cases {
        let input = format!("<!DOCTYPE r [{subset}]><r/>");
        let document = OwnedDocument::parse(&input).unwrap();
        let table = save::notation_table(document.dtd().unwrap());
        assert_eq!(table.as_deref(), Ok(expected), "{subset}");
    }
}
