use super::*;
use std::borrow::Cow;

fn parse(raw: &str) -> DtdUnit<'_> {
    DtdUnit::parse(raw, DtdLimits::STANDALONE).expect(raw)
}

fn marked(marked: &str) -> (String, usize) {
    let offset = marked.find('^').expect("fixture has an error marker");
    assert_eq!(marked[offset + 1..].find('^'), None, "one error marker");
    let mut raw = marked.to_owned();
    raw.remove(offset);
    (raw, offset)
}

fn rejects(marked_raw: &str, kind: ErrorKind) {
    let (raw, offset) = marked(marked_raw);
    let expected = Error { kind, offset };
    assert_eq!(
        DtdUnit::parse(&raw, DtdLimits::STANDALONE),
        Err(expected),
        "{raw:?}"
    );
}

#[derive(Debug, Eq, PartialEq)]
enum FlatModel {
    Open,
    Close,
    Pcdata,
    Name(String),
    Connector(Connector),
    Occurrence(Occurrence),
}

fn name(value: &str) -> FlatModel {
    FlatModel::Name(value.to_owned())
}

fn model(raw: &str) -> Vec<FlatModel> {
    let unit = parse(raw);
    let DtdUnitView::Element(element) = unit.view() else {
        panic!("expected ELEMENT view for {raw}");
    };
    match element.content() {
        ElementContentView::Mixed(tokens) | ElementContentView::Children(tokens) => tokens
            .map(|token| match token {
                ModelTokenView::Open => FlatModel::Open,
                ModelTokenView::Close => FlatModel::Close,
                ModelTokenView::Pcdata => FlatModel::Pcdata,
                ModelTokenView::Name(value) => FlatModel::Name(value.to_owned()),
                ModelTokenView::Connector(value) => FlatModel::Connector(value),
                ModelTokenView::Occurrence(value) => FlatModel::Occurrence(value),
            })
            .collect(),
        content => panic!("expected model tokens, got {content:?}"),
    }
}

#[test]
fn doctype_headers_preserve_names_identifiers_and_terminal_form() {
    struct Case<'a> {
        raw: &'a str,
        root: &'a str,
        public: Option<&'a str>,
        system: Option<&'a str>,
        subset: bool,
    }
    let cases = [
        Case {
            raw: "<!DOCTYPE r>",
            root: "r",
            public: None,
            system: None,
            subset: false,
        },
        Case {
            raw: "<!DOCTYPE π:名 [",
            root: "π:名",
            public: None,
            system: None,
            subset: true,
        },
        Case {
            raw: "<!DOCTYPE r SYSTEM 'a&%<>.dtd'>",
            root: "r",
            public: None,
            system: Some("'a&%<>.dtd'"),
            subset: false,
        },
        Case {
            raw: "<!DOCTYPE r PUBLIC '-//A B//EN' \"a.dtd\" [",
            root: "r",
            public: Some("'-//A B//EN'"),
            system: Some("\"a.dtd\""),
            subset: true,
        },
        Case {
            raw: "<!DOCTYPE a:b:c>",
            root: "a:b:c",
            public: None,
            system: None,
            subset: false,
        },
        Case {
            raw: "<!DOCTYPE \u{10000}>",
            root: "\u{10000}",
            public: None,
            system: None,
            subset: false,
        },
    ];

    for case in cases {
        let unit = parse(case.raw);
        assert_eq!(unit.raw(), case.raw);
        let DtdUnitView::Header(header) = unit.view() else {
            panic!("expected DOCTYPE header for {}", case.raw);
        };
        assert_eq!(header.root_name(), case.root);
        assert_eq!(header.has_internal_subset(), case.subset);
        assert_eq!(
            header.external_id(),
            (case.public.is_some() || case.system.is_some()).then_some(ExternalIdView {
                public: case.public,
                system: case.system,
            })
        );
    }
}

#[test]
fn element_declarations_expose_exact_flat_models() {
    for (raw, expected) in [
        (
            "<!ELEMENT r (#PCDATA)>",
            vec![FlatModel::Open, FlatModel::Pcdata, FlatModel::Close],
        ),
        (
            "<!ELEMENT r (#PCDATA|a|π:名)*>",
            vec![
                FlatModel::Open,
                FlatModel::Pcdata,
                FlatModel::Connector(Connector::Choice),
                name("a"),
                FlatModel::Connector(Connector::Choice),
                name("π:名"),
                FlatModel::Close,
                FlatModel::Occurrence(Occurrence::ZeroOrMore),
            ],
        ),
        (
            "<!ELEMENT r (a,b?,(c|d+)*)+>",
            vec![
                FlatModel::Open,
                name("a"),
                FlatModel::Connector(Connector::Sequence),
                name("b"),
                FlatModel::Occurrence(Occurrence::Optional),
                FlatModel::Connector(Connector::Sequence),
                FlatModel::Open,
                name("c"),
                FlatModel::Connector(Connector::Choice),
                name("d"),
                FlatModel::Occurrence(Occurrence::OneOrMore),
                FlatModel::Close,
                FlatModel::Occurrence(Occurrence::ZeroOrMore),
                FlatModel::Close,
                FlatModel::Occurrence(Occurrence::OneOrMore),
            ],
        ),
        (
            "<!ELEMENT r ( a | ( b , c )* )?>",
            vec![
                FlatModel::Open,
                name("a"),
                FlatModel::Connector(Connector::Choice),
                FlatModel::Open,
                name("b"),
                FlatModel::Connector(Connector::Sequence),
                name("c"),
                FlatModel::Close,
                FlatModel::Occurrence(Occurrence::ZeroOrMore),
                FlatModel::Close,
                FlatModel::Occurrence(Occurrence::Optional),
            ],
        ),
    ] {
        assert_eq!(model(raw), expected, "{raw}");
    }

    for (raw, expected) in [
        ("<!ELEMENT r EMPTY>", ElementContentView::Empty),
        ("<!ELEMENT r ANY >", ElementContentView::Any),
    ] {
        let unit = parse(raw);
        let DtdUnitView::Element(element) = unit.view() else {
            panic!("expected ELEMENT view");
        };
        assert_eq!(element.name(), "r");
        assert_eq!(element.content(), expected);
    }
}

#[test]
fn grammatical_but_nonvalid_models_remain_lexical_successes() {
    let duplicate_children = model("<!ELEMENT r (a|a)>");
    assert_eq!(
        duplicate_children,
        [
            FlatModel::Open,
            name("a"),
            FlatModel::Connector(Connector::Choice),
            name("a"),
            FlatModel::Close,
        ]
    );
    let duplicate_mixed = model("<!ELEMENT r (#PCDATA|a|a)*>");
    assert_eq!(
        duplicate_mixed
            .iter()
            .filter(|token| **token == name("a"))
            .count(),
        2
    );
    assert_eq!(
        model("<!ELEMENT r (#PCDATA)*>"),
        [
            FlatModel::Open,
            FlatModel::Pcdata,
            FlatModel::Close,
            FlatModel::Occurrence(Occurrence::ZeroOrMore),
        ]
    );
    let unit = parse("<!ELEMENT a:b:c EMPTY>");
    let DtdUnitView::Element(element) = unit.view() else {
        panic!("expected ELEMENT view");
    };
    assert_eq!(element.name(), "a:b:c");
}

#[derive(Debug, Eq, PartialEq)]
enum ExpectedAttributeType<'a> {
    Cdata,
    Id,
    IdRef,
    IdRefs,
    Entity,
    Entities,
    NmToken,
    NmTokens,
    Notation(&'a [&'a str]),
    Enumeration(&'a [&'a str]),
}

fn assert_attribute_type(
    actual: AttributeDefinitionView<'_>,
    expected: &ExpectedAttributeType<'_>,
) {
    let values = actual.values().collect::<Vec<_>>();
    match (actual.kind(), expected) {
        (AttributeKind::Cdata, ExpectedAttributeType::Cdata)
        | (AttributeKind::Id, ExpectedAttributeType::Id)
        | (AttributeKind::IdRef, ExpectedAttributeType::IdRef)
        | (AttributeKind::IdRefs, ExpectedAttributeType::IdRefs)
        | (AttributeKind::Entity, ExpectedAttributeType::Entity)
        | (AttributeKind::Entities, ExpectedAttributeType::Entities)
        | (AttributeKind::NmToken, ExpectedAttributeType::NmToken)
        | (AttributeKind::NmTokens, ExpectedAttributeType::NmTokens) => assert!(values.is_empty()),
        (AttributeKind::Notation, ExpectedAttributeType::Notation(expected))
        | (AttributeKind::Enumeration, ExpectedAttributeType::Enumeration(expected)) => {
            assert_eq!(values, *expected)
        }
        (actual, expected) => panic!("type mismatch: {actual:?} != {expected:?}"),
    }
}

/// A default declaration with its literal's spelling.
#[derive(Debug, Eq, PartialEq)]
enum ExpectedDefault<'a> {
    Required,
    Implied,
    Value {
        fixed: bool,
        quoted_literal: &'a str,
    },
}

fn expected_default(actual: DefaultDeclarationView<'_>) -> ExpectedDefault<'_> {
    match actual {
        DefaultDeclarationView::Required => ExpectedDefault::Required,
        DefaultDeclarationView::Implied => ExpectedDefault::Implied,
        DefaultDeclarationView::Value { fixed, literal } => ExpectedDefault::Value {
            fixed,
            quoted_literal: literal.quoted,
        },
    }
}

/// An entity definition with an internal literal's spelling.
#[derive(Debug, Eq, PartialEq)]
enum ExpectedDefinition<'a> {
    Internal {
        quoted_literal: &'a str,
    },
    External {
        identifier: ExternalIdView<'a>,
        notation: Option<&'a str>,
    },
}

fn expected_definition(actual: EntityDefinitionView<'_>) -> ExpectedDefinition<'_> {
    match actual {
        EntityDefinitionView::Internal(literal) => ExpectedDefinition::Internal {
            quoted_literal: literal.quoted,
        },
        EntityDefinitionView::External {
            identifier,
            notation,
        } => ExpectedDefinition::External {
            identifier,
            notation,
        },
    }
}

#[test]
fn attlist_exposes_every_type_default_and_original_spelling() {
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
    let expected = [
        ("a", ExpectedAttributeType::Cdata, ExpectedDefault::Required),
        ("b", ExpectedAttributeType::Id, ExpectedDefault::Implied),
        (
            "c",
            ExpectedAttributeType::IdRef,
            ExpectedDefault::Value {
                fixed: false,
                quoted_literal: "'x'",
            },
        ),
        (
            "d",
            ExpectedAttributeType::IdRefs,
            ExpectedDefault::Value {
                fixed: false,
                quoted_literal: "\"x y\"",
            },
        ),
        ("e", ExpectedAttributeType::Entity, ExpectedDefault::Implied),
        (
            "f",
            ExpectedAttributeType::Entities,
            ExpectedDefault::Required,
        ),
        (
            "g",
            ExpectedAttributeType::NmToken,
            ExpectedDefault::Value {
                fixed: false,
                quoted_literal: "'1:x'",
            },
        ),
        (
            "h",
            ExpectedAttributeType::NmTokens,
            ExpectedDefault::Value {
                fixed: false,
                quoted_literal: "'1 :x a.b'",
            },
        ),
        (
            "i",
            ExpectedAttributeType::Notation(&["gif", "π:名"]),
            ExpectedDefault::Implied,
        ),
        (
            "j",
            ExpectedAttributeType::Enumeration(&["1", ":x", "a:b", "-.", "a", "a"]),
            ExpectedDefault::Value {
                fixed: false,
                quoted_literal: "'outside'",
            },
        ),
        (
            "k",
            ExpectedAttributeType::Cdata,
            ExpectedDefault::Value {
                fixed: true,
                quoted_literal: "\"x&amp;&#65;&#x41;\"",
            },
        ),
    ];

    let unit = parse(raw);
    let DtdUnitView::AttributeList(list) = unit.view() else {
        panic!("expected ATTLIST view");
    };
    assert_eq!(list.element_name(), "r");
    assert_eq!(list.definitions().count(), expected.len());
    let actual = list.definitions().collect::<Vec<_>>();
    assert_eq!(actual.len(), expected.len());
    for (definition, (name, attribute_type, default)) in actual.into_iter().zip(expected) {
        assert_eq!(definition.name(), name);
        assert_attribute_type(definition, &attribute_type);
        assert_eq!(expected_default(definition.default()), default);
    }
    assert_eq!(
        unit.reference_positions()
            .map(|(_, reference)| reference)
            .collect::<Vec<_>>(),
        [
            ReferenceView {
                kind: ReferenceKind::General,
                spelling: "&amp;",
            },
            ReferenceView {
                kind: ReferenceKind::DecimalCharacter,
                spelling: "&#65;",
            },
            ReferenceView {
                kind: ReferenceKind::HexadecimalCharacter,
                spelling: "&#x41;",
            },
        ]
    );
}

#[test]
fn empty_duplicate_and_percent_attlists_stay_lexical() {
    let unit = parse("<!ATTLIST r>");
    let DtdUnitView::AttributeList(list) = unit.view() else {
        panic!("expected ATTLIST view");
    };
    assert_eq!(list.definitions().count(), 0);
    assert_eq!(list.definitions().count(), 0);

    let unit = parse("<!ATTLIST r a CDATA '%p;' a CDATA 'y' id1 ID 'x' id2 ID #REQUIRED>");
    let DtdUnitView::AttributeList(list) = unit.view() else {
        panic!("expected ATTLIST view");
    };
    let definitions = list.definitions().collect::<Vec<_>>();
    assert_eq!(definitions.len(), 4);
    assert_eq!(definitions[0].name(), "a");
    assert_eq!(
        expected_default(definitions[0].default()),
        ExpectedDefault::Value {
            fixed: false,
            quoted_literal: "'%p;'",
        }
    );
    assert_eq!(definitions[1].name(), "a");
    assert!(
        unit.reference_positions()
            .map(|(_, reference)| reference)
            .next()
            .is_none()
    );
}

#[test]
fn entities_preserve_definition_kind_literals_and_reference_spelling() {
    struct Case<'a> {
        raw: &'a str,
        parameter: bool,
        name: &'a str,
        definition: ExpectedDefinition<'a>,
        references: &'a [(ReferenceKind, &'a str)],
    }
    let cases = [
        Case {
            raw: "<!ENTITY e \"plain < > ' text\">",
            parameter: false,
            name: "e",
            definition: ExpectedDefinition::Internal {
                quoted_literal: "\"plain < > ' text\"",
            },
            references: &[],
        },
        Case {
            raw: "<!ENTITY e \"&amp;&#65;&#x41;%p;\">",
            parameter: false,
            name: "e",
            definition: ExpectedDefinition::Internal {
                quoted_literal: "\"&amp;&#65;&#x41;%p;\"",
            },
            references: &[
                (ReferenceKind::General, "&amp;"),
                (ReferenceKind::DecimalCharacter, "&#65;"),
                (ReferenceKind::HexadecimalCharacter, "&#x41;"),
                (ReferenceKind::Parameter, "%p;"),
            ],
        },
        Case {
            raw: "<!ENTITY % p \"value\">",
            parameter: true,
            name: "p",
            definition: ExpectedDefinition::Internal {
                quoted_literal: "\"value\"",
            },
            references: &[],
        },
        Case {
            raw: "<!ENTITY e SYSTEM 'a&%<>.ent'>",
            parameter: false,
            name: "e",
            definition: ExpectedDefinition::External {
                identifier: ExternalIdView {
                    public: None,
                    system: Some("'a&%<>.ent'"),
                },
                notation: None,
            },
            references: &[],
        },
        Case {
            raw: "<!ENTITY image SYSTEM 'a.gif' NDATA gif>",
            parameter: false,
            name: "image",
            definition: ExpectedDefinition::External {
                identifier: ExternalIdView {
                    public: None,
                    system: Some("'a.gif'"),
                },
                notation: Some("gif"),
            },
            references: &[],
        },
        Case {
            raw: "<!ENTITY % p PUBLIC '-//A//EN' 'p.ent'>",
            parameter: true,
            name: "p",
            definition: ExpectedDefinition::External {
                identifier: ExternalIdView {
                    public: Some("'-//A//EN'"),
                    system: Some("'p.ent'"),
                },
                notation: None,
            },
            references: &[],
        },
    ];

    for case in cases {
        let expected_references = case
            .references
            .iter()
            .map(|(kind, spelling)| ReferenceView {
                kind: *kind,
                spelling,
            })
            .collect::<Vec<_>>();
        let unit = parse(case.raw);
        let DtdUnitView::Entity(entity) = unit.view() else {
            panic!("expected ENTITY view for {}", case.raw);
        };
        assert_eq!(entity.is_parameter(), case.parameter);
        assert_eq!(entity.name(), case.name);
        assert_eq!(expected_definition(entity.definition()), case.definition);
        assert_eq!(
            unit.reference_positions()
                .map(|(_, reference)| reference)
                .collect::<Vec<_>>(),
            expected_references
        );
    }
}

#[test]
fn notations_comments_and_processing_instructions_preserve_lexical_data() {
    for (raw, name) in [
        ("<!NOTATION gif SYSTEM 'image/gif'>", "gif"),
        ("<!NOTATION n PUBLIC '-//A B//EN' 'n.sys'>", "n"),
        ("<!NOTATION n PUBLIC '-//A B//EN'>", "n"),
        ("<!NOTATION n PUBLIC ''>", "n"),
    ] {
        let unit = parse(raw);
        let DtdUnitView::Notation(notation) = unit.view() else {
            panic!("expected NOTATION view");
        };
        assert_eq!(notation.name(), name);
    }

    for (raw, body) in [
        ("<!---->", ""),
        (
            "<!-- declarations for <r> & %p; -->",
            " declarations for <r> & %p; ",
        ),
    ] {
        let unit = parse(raw);
        assert_eq!(unit.view(), DtdUnitView::Comment(body));
        assert!(
            unit.reference_positions()
                .map(|(_, reference)| reference)
                .next()
                .is_none()
        );
    }

    for (raw, target, data) in [
        ("<?build?>", "build", None),
        ("<?build ?>", "build", Some("")),
        ("<?build a&b %p; <x>?>", "build", Some("a&b %p; <x>")),
        ("<?π:名 中?>", "π:名", Some("中")),
    ] {
        let unit = parse(raw);
        assert_eq!(
            unit.view(),
            DtdUnitView::ProcessingInstruction(ProcessingInstructionView { target, data })
        );
        assert!(
            unit.reference_positions()
                .map(|(_, reference)| reference)
                .next()
                .is_none()
        );
    }
}

#[test]
fn syntax_and_lexical_failures_report_the_selected_production_offset() {
    let cases = [
        ("<!DOCTYPE^r>", ErrorKind::InvalidDtd),
        ("<!DOCTYPE ^1r>", ErrorKind::InvalidName),
        ("<!DOCTYPE r PUBLIC 'p'^>", ErrorKind::InvalidDtd),
        ("<!DOCTYPE r SYSTEM^'s'>", ErrorKind::InvalidDtd),
        ("<!DOCTYPE r SYSTEM 's'>^x", ErrorKind::InvalidDtd),
        ("<!DOCTYPE r SYSTEM 's^", ErrorKind::InvalidDtd),
        ("<!DOCTYPE r PUBLIC 'bad^[pub' 's'>", ErrorKind::InvalidDtd),
        ("<!DOCTYPE r [^]>", ErrorKind::InvalidDtd),
        ("<!^BOGUS r>", ErrorKind::InvalidDtd),
        ("<!ELEMENT^r EMPTY>", ErrorKind::InvalidDtd),
        ("<!ELEMENT rEMPTY^>", ErrorKind::InvalidDtd),
        ("<!ELEMENT r ^empty>", ErrorKind::InvalidDtd),
        ("<!ELEMENT r (^)>", ErrorKind::InvalidName),
        ("<!ELEMENT r (a|^)>", ErrorKind::InvalidName),
        ("<!ELEMENT r (a,b^|c)>", ErrorKind::InvalidDtd),
        ("<!ELEMENT r (#PCDATA|a)^>", ErrorKind::InvalidDtd),
        ("<!ELEMENT r (#PCDATA)^?>", ErrorKind::InvalidDtd),
        ("<!ELEMENT r (a ^?)>", ErrorKind::InvalidDtd),
        ("<!ELEMENT r (a)*^*>", ErrorKind::InvalidDtd),
        ("<!ELEMENT r (a,(b|c)^", ErrorKind::InvalidDtd),
        ("<!ELEMENT r ANY ^x>", ErrorKind::InvalidDtd),
        ("<!ATTLIST^r>", ErrorKind::InvalidDtd),
        ("<!ATTLIST r aCDATA ^#IMPLIED>", ErrorKind::InvalidDtd),
        ("<!ATTLIST r a ^UNKNOWN #IMPLIED>", ErrorKind::InvalidDtd),
        (
            "<!ATTLIST r a NOTATION (^) #IMPLIED>",
            ErrorKind::InvalidName,
        ),
        (
            "<!ATTLIST r a NOTATION (^1|x) #IMPLIED>",
            ErrorKind::InvalidName,
        ),
        ("<!ATTLIST r a (^) #IMPLIED>", ErrorKind::InvalidDtd),
        ("<!ATTLIST r a (x|^) #IMPLIED>", ErrorKind::InvalidDtd),
        ("<!ATTLIST r a CDATA #FIXED^'x'>", ErrorKind::InvalidDtd),
        ("<!ATTLIST r a CDATA \"a^<b\">", ErrorKind::InvalidDtd),
        (
            "<!ATTLIST r a CDATA \"&bad^\">",
            ErrorKind::InvalidReference,
        ),
        (
            "<!ATTLIST r a CDATA \"^&#x110000;\">",
            ErrorKind::InvalidReference,
        ),
        ("<!ATTLIST r a CDATA #IMPLIED x^>", ErrorKind::InvalidDtd),
        ("<!ENTITY^e 'v'>", ErrorKind::InvalidDtd),
        ("<!ENTITY %^p 'v'>", ErrorKind::InvalidDtd),
        ("<!ENTITY % p^'v'>", ErrorKind::InvalidDtd),
        ("<!ENTITY e \"&^;\">", ErrorKind::InvalidReference),
        ("<!ENTITY e \"%^;\">", ErrorKind::InvalidReference),
        ("<!ENTITY e \"&#^;\">", ErrorKind::InvalidReference),
        ("<!ENTITY e \"&#x^;\">", ErrorKind::InvalidReference),
        ("<!ENTITY e \"^&#0;\">", ErrorKind::InvalidReference),
        ("<!ENTITY e \"unterminated>^", ErrorKind::InvalidDtd),
        ("<!ENTITY e PUBLIC 'p'^>", ErrorKind::InvalidDtd),
        ("<!ENTITY % p SYSTEM 's' ^NDATA n>", ErrorKind::InvalidDtd),
        ("<!ENTITY e 'v' ^NDATA n>", ErrorKind::InvalidDtd),
        ("<!ENTITY e SYSTEM 's' NDATA ^>", ErrorKind::InvalidName),
        ("<!ENTITY e SYSTEM^'s'>", ErrorKind::InvalidDtd),
        ("<!ENTITY e PUBLIC 'bad^[pub' 's'>", ErrorKind::InvalidDtd),
        ("<!NOTATION n^>", ErrorKind::InvalidDtd),
        ("<!NOTATION n PUBLIC^'p'>", ErrorKind::InvalidDtd),
        ("<!NOTATION n SYSTEM 's' ^x>", ErrorKind::InvalidDtd),
        ("<!-- a-^-b -->", ErrorKind::InvalidDtd),
        ("<!-- a -^-->", ErrorKind::InvalidDtd),
        ("<!-- a^", ErrorKind::InvalidDtd),
        ("<?^xml data?>", ErrorKind::InvalidDtd),
        ("<?^XmL?>", ErrorKind::InvalidDtd),
        ("<?^1x?>", ErrorKind::InvalidName),
        ("<?p data?>^x", ErrorKind::InvalidDtd),
        ("<?p data?>^ more?>", ErrorKind::InvalidDtd),
        ("<!DOCTYPE r SYSTEM 'a^#b'>", ErrorKind::InvalidDtd),
    ];
    let mut mismatches = Vec::new();
    for (marked_raw, kind) in cases {
        let (raw, offset) = marked(marked_raw);
        let expected = Err(Error { kind, offset });
        let actual = DtdUnit::parse(&raw, DtdLimits::STANDALONE);
        if actual != expected {
            mismatches.push(format!("{raw:?}: actual {actual:?}, expected {expected:?}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn invalid_xml_characters_are_rejected_in_every_lexical_context() {
    let cases = [
        ("<!ELEMENT r a\u{b}b>", "\u{b}"),
        ("<!ENTITY e \"a\u{0}b\">", "\u{0}"),
        ("<!ATTLIST r a CDATA 'a\u{b}b'>", "\u{b}"),
        ("<!NOTATION n SYSTEM 'a\u{fffe}b'>", "\u{fffe}"),
        ("<!NOTATION n PUBLIC 'a\u{b}b'>", "\u{b}"),
        ("<!-- a\u{b}b -->", "\u{b}"),
        ("<?p a\u{b}b?>", "\u{b}"),
    ];
    for (raw, invalid) in cases {
        let offset = raw.find(invalid).expect("invalid scalar is in fixture");
        let expected = Error {
            kind: ErrorKind::InvalidCharacter,
            offset,
        };
        assert_eq!(
            DtdUnit::parse(raw, DtdLimits::STANDALONE),
            Err(expected),
            "{raw:?}"
        );
    }

    let combining = parse("<!ATTLIST r a (\u{301}|x) #IMPLIED>");
    let DtdUnitView::AttributeList(list) = combining.view() else {
        panic!("expected ATTLIST view");
    };
    let definition = list.definitions().next().expect("one definition");
    assert_eq!(definition.kind(), AttributeKind::Enumeration);
    assert_eq!(definition.values().collect::<Vec<_>>(), ["\u{301}", "x"]);
    rejects("<!ELEMENT ^\u{301}x EMPTY>", ErrorKind::InvalidName);
}

#[test]
fn public_literals_and_character_references_use_their_distinct_grammars() {
    let public_body = " AZaz09-'()+,./:=?;!*#@$_%\r\n";
    let notation_raw = format!("<!NOTATION n PUBLIC \"{public_body}\">");
    assert!(matches!(
        parse(&notation_raw).view(),
        DtdUnitView::Notation(_)
    ));

    for invalid in ['[', ']', '&', '<', 'π'] {
        let raw = format!("<!NOTATION n PUBLIC \"a{invalid}b\">");
        let offset = raw.rfind(invalid).unwrap();
        assert_eq!(
            DtdUnit::parse(raw, DtdLimits::STANDALONE),
            Err(Error {
                kind: ErrorKind::InvalidDtd,
                offset,
            })
        );
    }

    let entity_raw = "<!ENTITY e \"&#9;&#13;&#x20;&#x10FFFF;\">";
    let entity = parse(entity_raw);
    assert_eq!(
        entity
            .reference_positions()
            .map(|(_, reference)| reference)
            .collect::<Vec<_>>(),
        [
            ReferenceView {
                kind: ReferenceKind::DecimalCharacter,
                spelling: "&#9;",
            },
            ReferenceView {
                kind: ReferenceKind::DecimalCharacter,
                spelling: "&#13;",
            },
            ReferenceView {
                kind: ReferenceKind::HexadecimalCharacter,
                spelling: "&#x20;",
            },
            ReferenceView {
                kind: ReferenceKind::HexadecimalCharacter,
                spelling: "&#x10FFFF;",
            },
        ]
    );
    for reference in ["&#0;", "&#xD800;", "&#xFFFE;", "&#x110000;"] {
        let raw = format!("<!ENTITY e \"{reference}\">");
        let offset = raw.find('&').unwrap();
        assert_eq!(
            DtdUnit::parse(raw, DtdLimits::STANDALONE),
            Err(Error {
                kind: ErrorKind::InvalidReference,
                offset,
            })
        );
    }
}

#[test]
fn every_incomplete_unit_prefix_fails_and_trailing_input_is_rejected() {
    let complete = [
        "<!DOCTYPE π:名 PUBLIC '-//A//EN' \"a.dtd\" [",
        "<!ELEMENT r (a,b?,(c|d+)*)+>",
        "<!ATTLIST r a CDATA #FIXED \"x&amp;&#65;&#x41;\">",
        "<!ENTITY e \"&amp;&#65;&#x41;%p;\">",
        "<!NOTATION n PUBLIC '-//A B//EN' 'n.sys'>",
        "<!-- declarations -->",
        "<?build data?>",
    ];
    for raw in complete {
        parse(raw);
        for end in raw.char_indices().map(|(index, _)| index) {
            let prefix = &raw[..end];
            assert!(
                DtdUnit::parse(prefix, DtdLimits::STANDALONE).is_err(),
                "proper prefix unexpectedly completed: {prefix:?}"
            );
        }
        let suffixed = format!("{raw}x");
        let expected = Error {
            kind: ErrorKind::InvalidDtd,
            offset: raw.len(),
        };
        assert_eq!(
            DtdUnit::parse(suffixed, DtdLimits::STANDALONE),
            Err(expected)
        );
    }
}

fn nested_model(depth: usize) -> String {
    format!("<!ELEMENT r {}a{}>", "(".repeat(depth), ")".repeat(depth))
}

#[test]
fn independent_limits_have_exact_boundaries_and_recover() {
    let raw = "<!ELEMENT r (a,b?,(c|d+)*)+>";
    let exact = DtdLimits {
        unit_bytes: raw.len(),
        ..DtdLimits::STANDALONE
    };
    assert_eq!(DtdUnit::parse(raw, exact).unwrap().raw(), raw);
    let short = DtdLimits {
        unit_bytes: raw.len() - 1,
        ..DtdLimits::STANDALONE
    };
    assert_eq!(
        DtdUnit::parse(raw, short),
        Err(Error {
            kind: ErrorKind::LimitExceeded(Limit::TokenBytes),
            offset: raw.len() - 1,
        })
    );

    // One declaration Name plus fifteen observable flat-model tokens.
    let exact_tokens = DtdLimits {
        unit_tokens: 16,
        ..DtdLimits::STANDALONE
    };
    DtdUnit::parse(raw, exact_tokens).unwrap();
    let short_tokens = DtdLimits {
        unit_tokens: 15,
        ..DtdLimits::STANDALONE
    };
    let outer_occurrence = raw.rfind('+').unwrap();
    assert_eq!(
        DtdUnit::parse(raw, short_tokens),
        Err(Error {
            kind: ErrorKind::LimitExceeded(Limit::TokenBytes),
            offset: outer_occurrence,
        })
    );

    let depth_three = nested_model(3);
    DtdUnit::parse(
        depth_three.as_str(),
        DtdLimits {
            model_depth: 3,
            ..DtdLimits::STANDALONE
        },
    )
    .unwrap();
    assert_eq!(
        DtdUnit::parse(
            depth_three.as_str(),
            DtdLimits {
                model_depth: 2,
                ..DtdLimits::STANDALONE
            },
        ),
        Err(Error {
            kind: ErrorKind::LimitExceeded(Limit::Depth),
            offset: "<!ELEMENT r ((".len(),
        })
    );

    let attributes = "<!ATTLIST r a CDATA #IMPLIED b ID #REQUIRED c NMTOKEN 'x' d (x|y) 'x'>";
    DtdUnit::parse(
        attributes,
        DtdLimits {
            attribute_definitions: 4,
            ..DtdLimits::STANDALONE
        },
    )
    .unwrap();
    assert_eq!(
        DtdUnit::parse(
            attributes,
            DtdLimits {
                attribute_definitions: 3,
                ..DtdLimits::STANDALONE
            },
        ),
        Err(Error {
            kind: ErrorKind::LimitExceeded(Limit::Attributes),
            offset: attributes.find("d (x|y)").unwrap(),
        })
    );

    // A failed parse has no persistent state: each restored independent limit succeeds.
    assert_eq!(DtdUnit::parse(raw, exact).unwrap().raw(), raw);
    assert_eq!(
        DtdUnit::parse(attributes, DtdLimits::STANDALONE)
            .unwrap()
            .raw(),
        attributes
    );
}

#[test]
fn borrowed_and_owned_inputs_have_identical_views_and_owned_storage_moves() {
    let raw = "<!ENTITY e \"&amp;&#65;%p;\">";
    let borrowed = DtdUnit::parse(Cow::Borrowed(raw), DtdLimits::STANDALONE).unwrap();
    assert!(matches!(borrowed.raw, Cow::Borrowed(_)));
    let borrowed_view = borrowed.view();
    let borrowed_references = borrowed
        .reference_positions()
        .map(|(_, reference)| reference)
        .collect::<Vec<_>>();

    let owned_input = raw.to_owned();
    let owned_pointer = owned_input.as_ptr();
    let owned = DtdUnit::parse(Cow::Owned(owned_input), DtdLimits::STANDALONE).unwrap();
    assert!(matches!(owned.raw, Cow::Owned(_)));
    assert_eq!(owned.raw().as_ptr(), owned_pointer);
    assert_eq!(owned.raw(), raw);
    assert_eq!(owned.view(), borrowed_view);
    assert_eq!(
        owned
            .reference_positions()
            .map(|(_, reference)| reference)
            .collect::<Vec<_>>(),
        borrowed_references
    );
}

#[test]
fn bounded_whitespace_name_quote_and_depth_properties_keep_literal_oracles() {
    for whitespace in [" ", "\t", "\n", "\r"] {
        let header =
            format!("<!DOCTYPE{whitespace}r{whitespace}SYSTEM{whitespace}'s'{whitespace}>");
        let unit = parse(&header);
        let DtdUnitView::Header(header) = unit.view() else {
            panic!("expected header view");
        };
        assert_eq!(header.root_name(), "r");
        assert_eq!(header.external_id().unwrap().system, Some("'s'"));

        let element = format!("<!ELEMENT{whitespace}r{whitespace}(a{whitespace}|{whitespace}b)>");
        assert_eq!(
            model(&element),
            [
                FlatModel::Open,
                name("a"),
                FlatModel::Connector(Connector::Choice),
                name("b"),
                FlatModel::Close,
            ]
        );
        let entity = format!("<!ENTITY{whitespace}%{whitespace}p{whitespace}'v'>");
        let unit = parse(&entity);
        let DtdUnitView::Entity(entity) = unit.view() else {
            panic!("expected ENTITY view");
        };
        assert!(entity.is_parameter());
        assert_eq!(entity.name(), "p");

        let attlist = format!(
            "<!ATTLIST{whitespace}r{whitespace}a{whitespace}CDATA{whitespace}#IMPLIED{whitespace}>"
        );
        let unit = parse(&attlist);
        let DtdUnitView::AttributeList(list) = unit.view() else {
            panic!("expected ATTLIST view");
        };
        assert_eq!(list.definitions().count(), 1);

        let notation =
            format!("<!NOTATION{whitespace}n{whitespace}PUBLIC{whitespace}'p'{whitespace}>");
        let unit = parse(&notation);
        assert!(matches!(unit.view(), DtdUnitView::Notation(_)));
    }

    for name in ["a", "_", ":", "π", "名", "a:b:c", "\u{10000}"] {
        let raw = format!("<!ELEMENT {name} EMPTY>");
        let unit = parse(&raw);
        let DtdUnitView::Element(element) = unit.view() else {
            panic!("expected ELEMENT view");
        };
        assert_eq!(element.name(), name);
    }
    for non_name_start in ["0", "-", ".", "\u{301}"] {
        let raw = format!("<!ELEMENT {non_name_start}x EMPTY>");
        assert_eq!(
            DtdUnit::parse(raw, DtdLimits::STANDALONE).unwrap_err().kind,
            ErrorKind::InvalidName
        );
    }

    for (quote, opposite) in [('\'', '"'), ('"', '\'')] {
        let raw = format!("<!ENTITY e {quote}a{opposite}&amp;%p;{quote}>");
        let unit = parse(&raw);
        let DtdUnitView::Entity(entity) = unit.view() else {
            panic!("expected ENTITY view");
        };
        let expected_literal = format!("{quote}a{opposite}&amp;%p;{quote}");
        assert_eq!(
            expected_definition(entity.definition()),
            ExpectedDefinition::Internal {
                quoted_literal: &expected_literal,
            }
        );
    }

    for depth in 1..=8 {
        let raw = nested_model(depth);
        let tokens = model(&raw);
        assert_eq!(
            tokens
                .iter()
                .filter(|token| **token == FlatModel::Open)
                .count(),
            depth
        );
        assert_eq!(
            tokens
                .iter()
                .filter(|token| **token == FlatModel::Close)
                .count(),
            depth
        );
        assert_eq!(
            tokens.iter().filter(|token| **token == name("a")).count(),
            1
        );
    }
}
