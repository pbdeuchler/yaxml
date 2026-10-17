use std::borrow::Cow;
use std::mem::{align_of, size_of};

use super::*;

fn parse(raw: &str) -> DtdUnit<'_> {
    DtdUnit::parse(Cow::Borrowed(raw), DtdLimits::default()).unwrap()
}

fn marked(raw: &str) -> (String, usize) {
    let offset = raw.find('^').expect("one failure marker");
    let mut plain = raw.to_owned();
    plain.remove(offset);
    assert!(!plain.contains('^'));
    (plain, offset)
}

fn assert_marked_error(raw: &str, kind: DtdErrorKind) {
    let (raw, offset) = marked(raw);
    assert_eq!(
        DtdUnit::parse(&raw, DtdLimits::default()).unwrap_err(),
        DtdError { kind, offset },
        "{raw}"
    );
}

fn header<'a>(unit: &'a DtdUnit<'a>) -> DoctypeHeaderView<'a> {
    let DtdUnitView::Header(view) = unit.view() else {
        panic!("expected header: {:?}", unit.view());
    };
    view
}

fn element<'a>(unit: &'a DtdUnit<'a>) -> ElementDeclarationView<'a> {
    let DtdUnitView::Element(view) = unit.view() else {
        panic!("expected element: {:?}", unit.view());
    };
    view
}

fn attribute_list<'a>(unit: &'a DtdUnit<'a>) -> AttributeListView<'a> {
    let DtdUnitView::AttributeList(view) = unit.view() else {
        panic!("expected ATTLIST: {:?}", unit.view());
    };
    view
}

fn entity<'a>(unit: &'a DtdUnit<'a>) -> EntityDeclarationView<'a> {
    let DtdUnitView::Entity(view) = unit.view() else {
        panic!("expected entity: {:?}", unit.view());
    };
    view
}

fn notation<'a>(unit: &'a DtdUnit<'a>) -> NotationDeclarationView<'a> {
    let DtdUnitView::Notation(view) = unit.view() else {
        panic!("expected notation: {:?}", unit.view());
    };
    view
}

fn model<'a>(unit: &'a DtdUnit<'a>) -> Vec<ModelTokenView<'a>> {
    match element(unit).content() {
        ElementContentView::Mixed(tokens) | ElementContentView::Children(tokens) => {
            tokens.collect()
        }
        other => panic!("expected flat model, got {other:?}"),
    }
}

#[test]
fn accepted_headers_preserve_names_identifiers_and_terminal_kind() {
    let cases = [
        ("<!DOCTYPE r>", "r", None, None, false),
        ("<!DOCTYPE π:名 [", "π:名", None, None, true),
        (
            "<!DOCTYPE r SYSTEM 'a&%<>.dtd'>",
            "r",
            None,
            Some("'a&%<>.dtd'"),
            false,
        ),
        (
            "<!DOCTYPE r PUBLIC '-//A B//EN' \"a.dtd\" [",
            "r",
            Some("'-//A B//EN'"),
            Some("\"a.dtd\""),
            true,
        ),
        ("<!DOCTYPE a:b:c>", "a:b:c", None, None, false),
        ("<!DOCTYPE 𐀀>", "𐀀", None, None, false),
    ];
    for (raw, root, public, system, subset) in cases {
        let unit = parse(raw);
        let view = header(&unit);
        assert_eq!(unit.raw(), raw);
        assert_eq!(view.root_name(), root);
        assert_eq!(view.has_internal_subset(), subset);
        assert_eq!(
            view.external_id(),
            if public.is_some() || system.is_some() {
                Some(ExternalIdView { public, system })
            } else {
                None
            }
        );
    }
}

#[test]
fn element_models_are_flat_ordered_and_iterative() {
    for (raw, expected) in [
        (
            "<!ELEMENT r (#PCDATA)>",
            vec![
                ModelTokenView::Open,
                ModelTokenView::Pcdata,
                ModelTokenView::Close,
            ],
        ),
        (
            "<!ELEMENT r (#PCDATA)*>",
            vec![
                ModelTokenView::Open,
                ModelTokenView::Pcdata,
                ModelTokenView::Close,
                ModelTokenView::Occurrence(Occurrence::ZeroOrMore),
            ],
        ),
        (
            "<!ELEMENT r (#PCDATA|a|π:名)*>",
            vec![
                ModelTokenView::Open,
                ModelTokenView::Pcdata,
                ModelTokenView::Connector(Connector::Choice),
                ModelTokenView::Name("a"),
                ModelTokenView::Connector(Connector::Choice),
                ModelTokenView::Name("π:名"),
                ModelTokenView::Close,
                ModelTokenView::Occurrence(Occurrence::ZeroOrMore),
            ],
        ),
        (
            "<!ELEMENT r (a,b?,(c|d+)*)+>",
            vec![
                ModelTokenView::Open,
                ModelTokenView::Name("a"),
                ModelTokenView::Connector(Connector::Sequence),
                ModelTokenView::Name("b"),
                ModelTokenView::Occurrence(Occurrence::Optional),
                ModelTokenView::Connector(Connector::Sequence),
                ModelTokenView::Open,
                ModelTokenView::Name("c"),
                ModelTokenView::Connector(Connector::Choice),
                ModelTokenView::Name("d"),
                ModelTokenView::Occurrence(Occurrence::OneOrMore),
                ModelTokenView::Close,
                ModelTokenView::Occurrence(Occurrence::ZeroOrMore),
                ModelTokenView::Close,
                ModelTokenView::Occurrence(Occurrence::OneOrMore),
            ],
        ),
        (
            "<!ELEMENT r ( a | ( b , c )* )?>",
            vec![
                ModelTokenView::Open,
                ModelTokenView::Name("a"),
                ModelTokenView::Connector(Connector::Choice),
                ModelTokenView::Open,
                ModelTokenView::Name("b"),
                ModelTokenView::Connector(Connector::Sequence),
                ModelTokenView::Name("c"),
                ModelTokenView::Close,
                ModelTokenView::Occurrence(Occurrence::ZeroOrMore),
                ModelTokenView::Close,
                ModelTokenView::Occurrence(Occurrence::Optional),
            ],
        ),
    ] {
        let unit = parse(raw);
        assert_eq!(element(&unit).name(), "r");
        assert_eq!(model(&unit), expected, "{raw}");
    }

    for raw in [
        "<!ELEMENT r (a)>",
        "<!ELEMENT r (a|a)>",
        "<!ELEMENT r (#PCDATA|a|a)*>",
        "<!ELEMENT a:b:c EMPTY>",
    ] {
        parse(raw);
    }
    assert_eq!(
        element(&parse("<!ELEMENT r EMPTY>")).content(),
        ElementContentView::Empty
    );
    assert_eq!(
        element(&parse("<!ELEMENT r ANY >")).content(),
        ElementContentView::Any
    );
}

#[test]
fn attribute_lists_expose_all_types_defaults_and_reference_spelling() {
    let raw = concat!(
        "<!ATTLIST r ",
        "a CDATA #REQUIRED ",
        "b ID #IMPLIED ",
        "c IDREF 'x' ",
        "d IDREFS \"x y\" ",
        "e ENTITY #IMPLIED ",
        "f ENTITIES #REQUIRED ",
        "g NMTOKEN '1:x' ",
        "h NMTOKENS '1 :x a.b' ",
        "i NOTATION (gif|π:名) #IMPLIED ",
        "j (1|:x|a:b|-.|a|a) 'outside' ",
        "k CDATA #FIXED \"x&amp;&#65;&#x41;\">"
    );
    let unit = parse(raw);
    let view = attribute_list(&unit);
    assert_eq!(view.element_name(), "r");
    assert_eq!(view.definition_count(), 11);
    let definitions: Vec<_> = view.definitions().collect();
    assert_eq!(
        definitions
            .iter()
            .map(|item| item.name())
            .collect::<Vec<_>>(),
        ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"]
    );
    assert_eq!(definitions[0].attribute_type(), AttributeTypeView::Cdata);
    assert_eq!(definitions[1].attribute_type(), AttributeTypeView::Id);
    assert_eq!(definitions[2].attribute_type(), AttributeTypeView::IdRef);
    assert_eq!(definitions[3].attribute_type(), AttributeTypeView::IdRefs);
    assert_eq!(definitions[4].attribute_type(), AttributeTypeView::Entity);
    assert_eq!(definitions[5].attribute_type(), AttributeTypeView::Entities);
    assert_eq!(definitions[6].attribute_type(), AttributeTypeView::NmToken);
    assert_eq!(definitions[7].attribute_type(), AttributeTypeView::NmTokens);
    let AttributeTypeView::Notation(values) = definitions[8].attribute_type() else {
        panic!("expected NOTATION")
    };
    assert_eq!(values.collect::<Vec<_>>(), ["gif", "π:名"]);
    let AttributeTypeView::Enumeration(values) = definitions[9].attribute_type() else {
        panic!("expected enumeration")
    };
    assert_eq!(
        values.collect::<Vec<_>>(),
        ["1", ":x", "a:b", "-.", "a", "a"]
    );
    assert_eq!(definitions[0].default(), DefaultDeclarationView::Required);
    assert_eq!(definitions[1].default(), DefaultDeclarationView::Implied);
    assert_eq!(
        definitions[2].default(),
        DefaultDeclarationView::Value {
            fixed: false,
            quoted_literal: "'x'"
        }
    );
    assert_eq!(
        definitions[10].default(),
        DefaultDeclarationView::Value {
            fixed: true,
            quoted_literal: "\"x&amp;&#65;&#x41;\""
        }
    );
    assert_eq!(
        unit.references().collect::<Vec<_>>(),
        [
            ReferenceView {
                kind: ReferenceKind::General,
                spelling: "&amp;"
            },
            ReferenceView {
                kind: ReferenceKind::DecimalCharacter,
                spelling: "&#65;"
            },
            ReferenceView {
                kind: ReferenceKind::HexadecimalCharacter,
                spelling: "&#x41;"
            },
        ]
    );

    assert_eq!(attribute_list(&parse("<!ATTLIST r>")).definition_count(), 0);
    parse("<!ATTLIST r a CDATA '%p;'>");
    parse("<!ATTLIST r id1 ID 'x' id2 ID #REQUIRED>");
    parse("<!ATTLIST r a CDATA 'x' a CDATA 'y'>");
    parse("<!ATTLIST a:b:c π:名 CDATA \"v\">");
    parse("<!ATTLIST r a (́|x) #IMPLIED>");
}

#[test]
fn entities_preserve_kind_external_ids_ndata_and_literal_references() {
    let unit = parse("<!ENTITY e \"&amp;&#65;&#x41;%p;\">");
    let view = entity(&unit);
    assert!(!view.is_parameter());
    assert_eq!(view.name(), "e");
    assert_eq!(
        view.definition(),
        EntityDefinitionView::Internal {
            quoted_literal: "\"&amp;&#65;&#x41;%p;\""
        }
    );
    assert_eq!(
        unit.references().collect::<Vec<_>>(),
        [
            ReferenceView {
                kind: ReferenceKind::General,
                spelling: "&amp;"
            },
            ReferenceView {
                kind: ReferenceKind::DecimalCharacter,
                spelling: "&#65;"
            },
            ReferenceView {
                kind: ReferenceKind::HexadecimalCharacter,
                spelling: "&#x41;"
            },
            ReferenceView {
                kind: ReferenceKind::Parameter,
                spelling: "%p;"
            },
        ]
    );

    let unit = parse("<!ENTITY image SYSTEM 'a.gif' NDATA gif>");
    assert_eq!(
        entity(&unit).definition(),
        EntityDefinitionView::External {
            identifier: ExternalIdView {
                public: None,
                system: Some("'a.gif'")
            },
            notation: Some("gif"),
        }
    );
    let unit = parse("<!ENTITY % p PUBLIC '-//A//EN' 'p.ent'>");
    let view = entity(&unit);
    assert!(view.is_parameter());
    assert_eq!(view.name(), "p");
    assert_eq!(
        view.definition(),
        EntityDefinitionView::External {
            identifier: ExternalIdView {
                public: Some("'-//A//EN'"),
                system: Some("'p.ent'")
            },
            notation: None,
        }
    );

    for raw in [
        "<!ENTITY e \"plain < > ' text\">",
        "<!ENTITY e '&quot; \" %p;'>",
        "<!ENTITY % p \"value\">",
        "<!ENTITY e SYSTEM 'a&%<>.ent'>",
        "<!ENTITY e PUBLIC '-//A B//EN' 'a.ent'>",
        "<!ENTITY % p SYSTEM 'p.ent'>",
        "<!ENTITY a:b:c \"v\">",
        "<!ENTITY e \"&#60;a/>\">",
        "<!ENTITY e \"AT&amp;T;\">",
    ] {
        parse(raw);
    }
}

#[test]
fn notations_comments_and_processing_instructions_have_typed_views() {
    for (raw, name, public, system) in [
        (
            "<!NOTATION gif SYSTEM 'image/gif'>",
            "gif",
            None,
            Some("'image/gif'"),
        ),
        (
            "<!NOTATION n PUBLIC '-//A B//EN' 'n.sys'>",
            "n",
            Some("'-//A B//EN'"),
            Some("'n.sys'"),
        ),
        (
            "<!NOTATION n PUBLIC '-//A B//EN'>",
            "n",
            Some("'-//A B//EN'"),
            None,
        ),
        ("<!NOTATION a:b:c SYSTEM 's'>", "a:b:c", None, Some("'s'")),
        ("<!NOTATION n PUBLIC ''>", "n", Some("''"), None),
    ] {
        let unit = parse(raw);
        let view = notation(&unit);
        assert_eq!(view.name(), name);
        assert_eq!(view.identifier(), ExternalIdView { public, system });
    }

    for (raw, expected) in [
        (
            "<!-- declarations for <r> & %p; -->",
            " declarations for <r> & %p; ",
        ),
        ("<!---->", ""),
    ] {
        let unit = parse(raw);
        assert_eq!(unit.view(), DtdUnitView::Comment(expected));
        assert!(unit.references().next().is_none());
    }
    for (raw, target, data) in [
        ("<?build?>", "build", None),
        ("<?build a&b %p; <x>?>", "build", Some("a&b %p; <x>")),
        ("<?π:名 中?>", "π:名", Some("中")),
        ("<?p ?>", "p", Some("")),
    ] {
        let unit = parse(raw);
        assert_eq!(
            unit.view(),
            DtdUnitView::ProcessingInstruction(ProcessingInstructionView { target, data })
        );
        assert!(unit.references().next().is_none());
    }
}

#[test]
fn prototype_layouts_are_recorded() {
    eprintln!(
        "DtdUnit={}/{} DtdUnitKind={}/{} DtdToken={}/{} TokenKind={}/{} Event={}/{}",
        size_of::<DtdUnit<'static>>(),
        align_of::<DtdUnit<'static>>(),
        size_of::<DtdUnitKind>(),
        align_of::<DtdUnitKind>(),
        size_of::<DtdToken>(),
        align_of::<DtdToken>(),
        size_of::<TokenKind>(),
        align_of::<TokenKind>(),
        size_of::<crate::Event<'static>>(),
        align_of::<crate::Event<'static>>(),
    );
}

#[test]
fn rejects_first_invalid_byte_for_headers_and_element_models() {
    for (raw, kind) in [
        ("<!DOCTYPE^r>", DtdErrorKind::Syntax),
        ("<!DOCTYPE ^1r>", DtdErrorKind::InvalidName),
        ("<!DOCTYPE r PUBLIC 'p'^>", DtdErrorKind::Syntax),
        ("<!DOCTYPE r SYSTEM^'s'>", DtdErrorKind::Syntax),
        ("<!DOCTYPE r SYSTEM 's'>^x", DtdErrorKind::Syntax),
        (
            "<!DOCTYPE r PUBLIC 'bad^[pub' 's'>",
            DtdErrorKind::InvalidPublicIdentifier,
        ),
        ("<!DOCTYPE r [^]>", DtdErrorKind::Syntax),
        ("<!^BOGUS r>", DtdErrorKind::Syntax),
        ("<!ELEMENT^r EMPTY>", DtdErrorKind::Syntax),
        ("<!ELEMENT rEMPTY^>", DtdErrorKind::Syntax),
        ("<!ELEMENT r ^empty>", DtdErrorKind::Syntax),
        ("<!ELEMENT r (^)> ", DtdErrorKind::InvalidName),
        ("<!ELEMENT r (a|^)>", DtdErrorKind::InvalidName),
        ("<!ELEMENT r (a,b^|c)>", DtdErrorKind::Syntax),
        ("<!ELEMENT r (#PCDATA|a)^>", DtdErrorKind::Syntax),
        ("<!ELEMENT r (#PCDATA)^?>", DtdErrorKind::Syntax),
        ("<!ELEMENT r (a ^?)>", DtdErrorKind::Syntax),
        ("<!ELEMENT r (a)*^*>", DtdErrorKind::Syntax),
        ("<!ELEMENT r ANY ^x>", DtdErrorKind::Syntax),
    ] {
        assert_marked_error(raw, kind);
    }
    for raw in ["<!DOCTYP", "<!ELEMENT r (a,(b|c)", "<!DOCTYPE r SYSTEM 's"] {
        assert_eq!(
            DtdUnit::parse(raw, DtdLimits::default()).unwrap_err(),
            DtdError {
                kind: DtdErrorKind::Syntax,
                offset: raw.len(),
            },
            "{raw}"
        );
    }
}

#[test]
fn rejects_first_invalid_byte_for_attlists_entities_and_misc() {
    for (raw, kind) in [
        ("<!ATTLIST^r>", DtdErrorKind::Syntax),
        ("<!ATTLIST r aCDATA ^#IMPLIED>", DtdErrorKind::Syntax),
        ("<!ATTLIST r a ^UNKNOWN #IMPLIED>", DtdErrorKind::Syntax),
        ("<!ATTLIST r a NOTATION (^)> ", DtdErrorKind::InvalidName),
        (
            "<!ATTLIST r a NOTATION (^1|x) #IMPLIED>",
            DtdErrorKind::InvalidName,
        ),
        ("<!ATTLIST r a (^)> ", DtdErrorKind::InvalidNmtoken),
        (
            "<!ATTLIST r a (x|^) #IMPLIED>",
            DtdErrorKind::InvalidNmtoken,
        ),
        ("<!ATTLIST r a CDATA #FIXED^'x'>", DtdErrorKind::Syntax),
        ("<!ATTLIST r a CDATA \"a^<b\">", DtdErrorKind::Syntax),
        (
            "<!ATTLIST r a CDATA \"&bad^\">",
            DtdErrorKind::InvalidReference,
        ),
        (
            "<!ATTLIST r a CDATA \"^&#x110000;\">",
            DtdErrorKind::InvalidReference,
        ),
        ("<!ATTLIST r a CDATA #IMPLIED x^>", DtdErrorKind::Syntax),
        ("<!ENTITY^e 'v'>", DtdErrorKind::Syntax),
        ("<!ENTITY %^p 'v'>", DtdErrorKind::Syntax),
        ("<!ENTITY % p^'v'>", DtdErrorKind::Syntax),
        ("<!ENTITY e \"&^;\">", DtdErrorKind::InvalidReference),
        ("<!ENTITY e \"%^;\">", DtdErrorKind::InvalidReference),
        ("<!ENTITY e \"&#^;\">", DtdErrorKind::InvalidReference),
        ("<!ENTITY e \"&#x^;\">", DtdErrorKind::InvalidReference),
        ("<!ENTITY e \"^&#0;\">", DtdErrorKind::InvalidReference),
        ("<!ENTITY e PUBLIC 'p'^>", DtdErrorKind::Syntax),
        ("<!ENTITY % p SYSTEM 's' ^NDATA n>", DtdErrorKind::Syntax),
        ("<!ENTITY e 'v' ^NDATA n>", DtdErrorKind::Syntax),
        ("<!ENTITY e SYSTEM 's' NDATA ^>", DtdErrorKind::InvalidName),
        ("<!ENTITY e SYSTEM^'s'>", DtdErrorKind::Syntax),
        (
            "<!ENTITY e PUBLIC 'bad^[pub' 's'>",
            DtdErrorKind::InvalidPublicIdentifier,
        ),
        ("<!NOTATION n^>", DtdErrorKind::Syntax),
        ("<!NOTATION n PUBLIC^'p'>", DtdErrorKind::Syntax),
        ("<!NOTATION n SYSTEM 's' ^x>", DtdErrorKind::Syntax),
        ("<!-- a-^-b -->", DtdErrorKind::Syntax),
        ("<!-- a -^-->", DtdErrorKind::Syntax),
        ("<?^xml data?>", DtdErrorKind::ReservedPiTarget),
        ("<?^XmL?>", DtdErrorKind::ReservedPiTarget),
        ("<?^1x?>", DtdErrorKind::InvalidName),
        ("<?p data?>^x", DtdErrorKind::Syntax),
        ("<?p data ?>^ more?>", DtdErrorKind::Syntax),
        ("<!ELEMENT r ^́x>", DtdErrorKind::Syntax),
    ] {
        assert_marked_error(raw, kind);
    }
    for raw in ["<!ENTITY e \"unterminated>", "<!-- a"] {
        assert_eq!(
            DtdUnit::parse(raw, DtdLimits::default()).unwrap_err(),
            DtdError {
                kind: DtdErrorKind::Syntax,
                offset: raw.len(),
            }
        );
    }
}

#[test]
fn rejects_non_xml_characters_before_grammar_allocations() {
    for raw in [
        "<!ENTITY e \"a\0b\">",
        "<!ATTLIST r a CDATA 'a\u{b}b'>",
        "<!NOTATION n SYSTEM 'a\u{fffe}b'>",
        "<!ELEMENT r a\u{b}b>",
        "<!-- a\u{b}b -->",
        "<?p a\u{b}b?>",
    ] {
        let offset = raw
            .char_indices()
            .find(|(_, character)| !xml_char(*character))
            .unwrap()
            .0;
        assert_eq!(
            DtdUnit::parse(raw, DtdLimits::default()).unwrap_err(),
            DtdError {
                kind: DtdErrorKind::InvalidCharacter,
                offset,
            },
            "{raw:?}"
        );
    }
}

#[test]
fn limits_accept_exact_boundaries_and_reject_before_growth() {
    let header = "<!DOCTYPE root>";
    let exact_bytes = DtdLimits {
        max_unit_bytes: header.len(),
        ..DtdLimits::default()
    };
    DtdUnit::parse(header, exact_bytes).unwrap();
    assert_eq!(
        DtdUnit::parse(
            header,
            DtdLimits {
                max_unit_bytes: header.len() - 1,
                ..DtdLimits::default()
            },
        )
        .unwrap_err(),
        DtdError {
            kind: DtdErrorKind::LimitExceeded(DtdLimit::UnitBytes),
            offset: header.len() - 1,
        }
    );

    DtdUnit::parse(
        header,
        DtdLimits {
            max_tokens: 1,
            ..DtdLimits::default()
        },
    )
    .unwrap();
    assert_eq!(
        DtdUnit::parse(
            header,
            DtdLimits {
                max_tokens: 0,
                ..DtdLimits::default()
            },
        )
        .unwrap_err(),
        DtdError {
            kind: DtdErrorKind::LimitExceeded(DtdLimit::Tokens),
            offset: "<!DOCTYPE ".len(),
        }
    );

    let one_level = "<!ELEMENT r (a)>";
    DtdUnit::parse(
        one_level,
        DtdLimits {
            max_model_depth: 1,
            ..DtdLimits::default()
        },
    )
    .unwrap();
    assert_eq!(
        DtdUnit::parse(
            one_level,
            DtdLimits {
                max_model_depth: 0,
                ..DtdLimits::default()
            },
        )
        .unwrap_err(),
        DtdError {
            kind: DtdErrorKind::LimitExceeded(DtdLimit::ModelDepth),
            offset: one_level.find('(').unwrap(),
        }
    );

    let one_attribute = "<!ATTLIST r a CDATA #IMPLIED>";
    DtdUnit::parse(
        one_attribute,
        DtdLimits {
            max_attribute_definitions: 1,
            ..DtdLimits::default()
        },
    )
    .unwrap();
    assert_eq!(
        DtdUnit::parse(
            one_attribute,
            DtdLimits {
                max_attribute_definitions: 0,
                ..DtdLimits::default()
            },
        )
        .unwrap_err(),
        DtdError {
            kind: DtdErrorKind::LimitExceeded(DtdLimit::AttributeDefinitions),
            offset: one_attribute.find("a CDATA").unwrap(),
        }
    );
}

#[test]
fn embedded_reference_name_preserves_token_limit_error() {
    for (raw, marker) in [
        ("<!ENTITY e '&name;'>", "&name;"),
        ("<!ENTITY e '%name;'>", "%name;"),
    ] {
        assert_eq!(
            DtdUnit::parse(
                raw,
                DtdLimits {
                    max_tokens: 1,
                    ..DtdLimits::default()
                },
            )
            .unwrap_err(),
            DtdError {
                kind: DtdErrorKind::LimitExceeded(DtdLimit::Tokens),
                offset: raw.find(marker).unwrap() + 1,
            },
            "{raw}"
        );
    }
}

#[test]
fn borrowed_and_owned_units_retain_their_input_without_rescanning() {
    let borrowed_raw = "<!NOTATION π PUBLIC '-//PI//EN'>";
    let borrowed = DtdUnit::parse(Cow::Borrowed(borrowed_raw), DtdLimits::default()).unwrap();
    assert!(matches!(borrowed.raw, Cow::Borrowed(_)));
    assert_eq!(borrowed.raw().as_ptr(), borrowed_raw.as_ptr());
    assert_eq!(notation(&borrowed).name(), "π");

    let owned_raw = String::from("<!ENTITY e 'value'>");
    let owned_pointer = owned_raw.as_ptr();
    let owned = DtdUnit::parse(Cow::Owned(owned_raw), DtdLimits::default()).unwrap();
    assert!(matches!(owned.raw, Cow::Owned(_)));
    assert_eq!(owned.raw().as_ptr(), owned_pointer);
    assert_eq!(
        entity(&owned).definition(),
        EntityDefinitionView::Internal {
            quoted_literal: "'value'",
        }
    );
}

#[test]
fn bounded_generated_models_and_truncations_are_total() {
    for depth in 1..=8 {
        let model = format!("<!ELEMENT r {}a{}>", "(".repeat(depth), ")".repeat(depth));
        let unit = DtdUnit::parse(
            model.as_str(),
            DtdLimits {
                max_model_depth: depth,
                ..DtdLimits::default()
            },
        )
        .unwrap();
        assert_eq!(element(&unit).name(), "r");
        assert_eq!(
            DtdUnit::parse(
                model.as_str(),
                DtdLimits {
                    max_model_depth: depth - 1,
                    ..DtdLimits::default()
                },
            )
            .unwrap_err()
            .kind,
            DtdErrorKind::LimitExceeded(DtdLimit::ModelDepth)
        );
    }

    let complete = "<!ATTLIST π:名 a (one|two|three) #FIXED '&amp;'>";
    for boundary in complete.char_indices().map(|(offset, _)| offset) {
        let prefix = &complete[..boundary];
        let error = DtdUnit::parse(prefix, DtdLimits::default()).unwrap_err();
        assert!(error.offset <= prefix.len(), "{prefix:?}: {error:?}");
    }
    DtdUnit::parse(complete, DtdLimits::default()).unwrap();
}
