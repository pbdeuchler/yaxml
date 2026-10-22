//! DTD validation: every validity constraint, both DOMs, external DTDs and
//! differential checks against the pinned libxml2 2.15.4 `xmllint --valid`.

use yaxml::valid::{self, Code, Dtd};
use yaxml::{Document, ErrorKind, Limits, OwnedDocument, Request, ResourceKind};

use Code::*;

const D: &str = "<!DOCTYPE r [<!ELEMENT r ANY>";

/// `(document, expected codes in report order)`; each constraint has a
/// passing and a failing row.
const CASES: &[(&str, &[Code])] = &[
    // VC: Root Element Type, Element Valid (declared).
    ("<!DOCTYPE r [<!ELEMENT r EMPTY>]><r/>", &[]),
    ("<!DOCTYPE x [<!ELEMENT r EMPTY>]><r/>", &[RootName]),
    (
        "<!DOCTYPE r [<!ELEMENT r ANY>]><r><u/></r>",
        &[UnknownElement],
    ),
    (
        "<!DOCTYPE r [<!ATTLIST r a CDATA #IMPLIED>]><r/>",
        &[UnknownElement],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY>]><p:r xmlns:p='u'/>",
        &[UnknownAttribute],
    ),
    (
        "<!DOCTYPE p:r [<!ELEMENT p:r EMPTY><!ATTLIST p:r xmlns:p CDATA #FIXED 'u'>]><p:r xmlns:p='u'/>",
        &[],
    ),
    // EMPTY, ANY, (#PCDATA) and mixed content.
    ("<!DOCTYPE r [<!ELEMENT r EMPTY>]><r>x</r>", &[NotEmpty]),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY>]><r><!--c--></r>",
        &[NotEmpty],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (#PCDATA)>]><r>x<?p?><![CDATA[y]]></r>",
        &[],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (#PCDATA)><!ELEMENT a EMPTY>]><r><a/><a/></r>",
        &[NotPcdata],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (#PCDATA|a)*><!ELEMENT a EMPTY>]><r>x<a/>y<a/></r>",
        &[],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (#PCDATA|a)*><!ELEMENT a EMPTY><!ELEMENT b EMPTY>]><r><b/><a/><b/></r>",
        &[InvalidChild, InvalidChild],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (#PCDATA|a|a)*><!ELEMENT a EMPTY>]><r/>",
        &[ContentError],
    ),
    // Element content.
    (
        "<!DOCTYPE r [<!ELEMENT r (a,(b|c)*,a?)><!ELEMENT a EMPTY><!ELEMENT b EMPTY><!ELEMENT c EMPTY>]><r> <a/><c/><b/>\n<a/><!--x--><?p?></r>",
        &[],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (a,b)><!ELEMENT a EMPTY><!ELEMENT b EMPTY>]><r><b/><a/></r>",
        &[ContentModel],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (a,b)><!ELEMENT a EMPTY><!ELEMENT b EMPTY>]><r><a/></r>",
        &[ContentModel],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]><r><a/>text</r>",
        &[ContentModel],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]><r><a/><![CDATA[ ]]></r>",
        &[ContentModel],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (a+)><!ELEMENT a EMPTY>]><r><a/><a/><a/></r>",
        &[],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (s,s)><!ELEMENT s (a|a)><!ELEMENT a EMPTY>]><r><s><a/></s><s><a/></s></r>",
        &[ContentNotDeterministic],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ELEMENT r ANY>]><r/>",
        &[ElementRedefined],
    ),
    // Attribute value types.
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY>]><r a='1'/>",
        &[UnknownAttribute],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY>]><r xml:id='x'/>",
        &[UnknownAttribute],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r a CDATA #IMPLIED>]><r a=' 1 &lt; '/>",
        &[],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r a ID #IMPLIED>]><r a='1x'/>",
        &[AttributeValue],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r a NMTOKEN #IMPLIED b NMTOKENS #IMPLIED>]><r a='-1.x' b=' a  b '/>",
        &[],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r a NMTOKEN #IMPLIED>]><r a='a b'/>",
        &[AttributeValue],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r a (x|y) #IMPLIED>]><r a='y'/>",
        &[],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r a (x|y) #IMPLIED>]><r a='z'/>",
        &[AttributeValue],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r a (x|y|x) #IMPLIED>]><r/>",
        &[DuplicateToken],
    ),
    // ID, IDREF and IDREFS.
    (
        "<!DOCTYPE r [<!ELEMENT r ANY><!ATTLIST r i ID #IMPLIED f IDREF #IMPLIED s IDREFS #IMPLIED>]><r i='a' f='a' s='a a'><r i='b' f='a'/></r>",
        &[],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r ANY><!ATTLIST r i ID #IMPLIED>]><r i='a'><r i='a'/></r>",
        &[IdRedefined],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r i ID #IMPLIED f IDREF #IMPLIED s IDREFS #IMPLIED>]><r i='a' f='b' s='a c d'/>",
        &[UnknownId, UnknownId, UnknownId],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r i ID 'x'>]><r i='y'/>",
        &[IdFixed],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r i ID #IMPLIED j ID #IMPLIED>]><r/>",
        &[IdSubset],
    ),
    // Defaults: #REQUIRED, #FIXED and default syntax.
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r a CDATA #REQUIRED>]><r a=''/>",
        &[],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r a CDATA #REQUIRED>]><r/>",
        &[MissingAttribute],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r p:a CDATA #REQUIRED>]><r p:a='' xmlns:p='u'/>",
        &[UnknownAttribute],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r p:a CDATA #REQUIRED q:a CDATA #IMPLIED xmlns:q CDATA #IMPLIED>]><r q:a='' xmlns:q='u'/>",
        &[DifferentPrefix],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r a CDATA #FIXED 'v'>]><r a='v'/>",
        &[],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r a CDATA #FIXED 'v'>]><r a='w'/>",
        &[AttributeDefault, AttributeValue],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r xmlns CDATA #FIXED 'u'>]><r xmlns='w'/>",
        &[AttributeDefault, ElementNamespace],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r a NMTOKEN '1 2'>]><r a='1'/>",
        &[AttributeDefault],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r a (x|y) 'z'>]><r a='x'/>",
        &[AttributeValue],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r a CDATA 'x' a CDATA 'y'>]><r/>",
        &[AttributeRedefined],
    ),
    // Entities and notations.
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!NOTATION n SYSTEM 'n'><!ENTITY u SYSTEM 'u' NDATA n><!ATTLIST r e ENTITY #IMPLIED s ENTITIES #IMPLIED t NOTATION (n) #IMPLIED>]><r e='u' s='u u'/>",
        &[EmptyNotation],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ENTITY p 'x'><!ATTLIST r e ENTITY #IMPLIED s ENTITIES #IMPLIED>]><r e='p' s='amp q'/>",
        &[EntityType, EntityType, UnknownEntity],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r ANY><!NOTATION n SYSTEM 'n'><!NOTATION m SYSTEM 'm'><!ATTLIST r t NOTATION (n|o) #IMPLIED>]><r t='n'><r t='m'/><r t='o'/></r>",
        &[NotationValue, UnknownNotation, UnknownNotation],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r ANY><!ENTITY u SYSTEM 'u' NDATA n>]><r/>",
        &[UnknownNotation],
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r ANY><!NOTATION n SYSTEM 'a'><!NOTATION n SYSTEM 'b'>]><r/>",
        &[NotationRedefined],
    ),
    // VC: Entity Declared once parameter entities make declarations unread.
    (
        "<!DOCTYPE r [<!ENTITY % p ''>%p;<!ELEMENT r ANY>]><r>&x;</r>",
        &[UndeclaredEntity],
    ),
    (
        "<!DOCTYPE r [<!ENTITY % p ''>%p;<!ELEMENT r ANY><!ATTLIST r a CDATA #IMPLIED>]><r a='&x;'/>",
        &[UndeclaredEntity],
    ),
    (
        "<!DOCTYPE r [%q;<!ELEMENT r ANY>]><r/>",
        &[UndeclaredEntity],
    ),
];

fn codes<Id>(diagnostics: &[valid::Diagnostic<Id>]) -> Vec<Code> {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn every_constraint_is_reported_by_both_doms() {
    for &(input, expected) in CASES {
        let borrowed = Document::parse(input).unwrap().validate();
        assert_eq!(codes(&borrowed), expected, "{input}");
        let owned = OwnedDocument::parse(input).unwrap().validate();
        assert_eq!(codes(&owned), expected, "{input}");
        let errors = expected.iter().any(|code| !code.is_warning());
        assert_eq!(valid::is_valid(&borrowed), !errors, "{input}");
    }
}

#[test]
fn diagnostics_locate_elements_and_mirror_libxml2_codes() {
    let input = "<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]><r><a>x</a></r>";
    let document = Document::parse(input).unwrap();
    let [diagnostic] = document.validate().try_into().unwrap();
    let a = document.root_element().first_child().unwrap();
    assert_eq!(diagnostic.node, Some(a.id()));
    assert_eq!(diagnostic.offset, Some(a.range().unwrap().start));
    assert_eq!(diagnostic.code as u16, 528);
    assert_eq!(
        diagnostic.message,
        "Element a was declared EMPTY this one has content"
    );
    assert_eq!(UndeclaredEntity as u16, 26);
    assert_eq!(DuplicateToken as u16, 541);
    assert!(AttributeRedefined.is_warning() && !ContentModel.is_warning());
}

#[test]
fn documents_without_a_dtd_are_invalid() {
    let borrowed = Document::parse("<r/>").unwrap().validate();
    assert_eq!(codes(&borrowed), [NoDtd]);
    assert!(!valid::is_valid(&borrowed));
    assert_eq!(
        codes(&OwnedDocument::parse("<r/>").unwrap().validate()),
        [NoDtd]
    );
    assert!(Document::parse("<r/>").unwrap().dtd().is_none());
    assert_eq!(
        Document::parse(&format!("{D}]><r/>"))
            .unwrap()
            .dtd()
            .unwrap()
            .name(),
        Some("r")
    );
}

#[test]
fn owned_documents_validate_their_current_tree() {
    let input = "<!DOCTYPE r [<!ELEMENT r (#PCDATA)><!ATTLIST r a (x|y) #IMPLIED>]><r a='x'/>";
    let mut document = OwnedDocument::parse(input).unwrap();
    assert!(document.validate().is_empty());
    let root = document.root_element().id();
    let mut editor = document.edit(yaxml::EditLimits::default()).unwrap();
    editor.set_attribute_value(root, None, "a", "z").unwrap();
    editor.append_comment(root, "fine").unwrap();
    assert_eq!(codes(&document.validate()), [AttributeValue]);
}

#[test]
fn rootless_documents_report_no_root() {
    // xmlValidateRoot: a recovered rootless tree reports only that.
    let options = || yaxml::ParseOptions::new(yaxml::options::RECOVER);
    let input = b"<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r i ID #IMPLIED>]><!--c-->";
    let document = OwnedDocument::parse_with(input, options()).unwrap();
    let dtd = document.dtd().unwrap();
    for diagnostics in [document.validate(), valid::validate(&document, dtd)] {
        assert_eq!(codes(&diagnostics), [NoRoot]);
        assert_eq!(diagnostics[0].message, "no root element");
    }
    let borrowed = Document::parse_with(input, options()).unwrap();
    assert_eq!(codes(&valid::validate(&borrowed, dtd)), [NoRoot]);
    // With a root nothing is reported.
    let rooted = OwnedDocument::parse("<!DOCTYPE r [<!ELEMENT r EMPTY>]><!--c--><r/>").unwrap();
    assert!(rooted.validate().is_empty());
}

fn serve(files: &'static [(&'static str, &'static str)]) -> Box<dyn yaxml::Resolver> {
    Box::new(move |request: Request<'_>| {
        Ok(files
            .iter()
            .find(|(uri, _)| *uri == request.uri)
            .map(|(_, text)| text.as_bytes().to_vec()))
    })
}

#[test]
fn external_declarations_carry_standalone_and_nesting_constraints() {
    const DTD: &[(&str, &str)] = &[
        (
            "r.dtd",
            "<!ELEMENT r (a*)><!ELEMENT a EMPTY><!ATTLIST a t NMTOKEN 'x' n NMTOKENS #IMPLIED e ENTITY #IMPLIED>\
             <!NOTATION n SYSTEM 'n'><!ENTITY u SYSTEM 'u' NDATA n>",
        ),
        (
            "split.dtd",
            "<!ENTITY % open '(a'><!ELEMENT r %open;)><!ELEMENT a EMPTY>",
        ),
        (
            "section.dtd",
            "<!ENTITY % end ']]>'><![INCLUDE[<!ELEMENT r ANY>%end;",
        ),
    ];
    let table: &[(&str, &[Code])] = &[
        (
            "<?xml version='1.0' standalone='no'?><!DOCTYPE r SYSTEM 'r.dtd'><r> <a t='x'/> </r>",
            &[],
        ),
        (
            "<?xml version='1.0' standalone='yes'?><!DOCTYPE r SYSTEM 'r.dtd'><r> <a t='x' n=' p  q'/></r>",
            &[NotStandalone, StandaloneWhiteSpace],
        ),
        (
            "<?xml version='1.0' standalone='yes'?><!DOCTYPE r SYSTEM 'r.dtd'><r><a t='x' n='p q' e='u'/></r>",
            &[],
        ),
        (
            "<?xml version='1.0' standalone='yes'?><!DOCTYPE r SYSTEM 'r.dtd'><r><a/></r>",
            &[StandaloneDefaulted],
        ),
        (
            "<!DOCTYPE r SYSTEM 'split.dtd'><r><a/></r>",
            &[EntityBoundary],
        ),
        ("<!DOCTYPE r SYSTEM 'section.dtd'><r/>", &[EntityBoundary]),
    ];
    for &(input, expected) in table {
        let document = Document::parse_with(
            input.as_bytes(),
            loading(Limits::default(), "doc.xml", serve(DTD)),
        )
        .unwrap();
        assert_eq!(codes(&document.validate()), expected, "{input}");
        let owned = OwnedDocument::parse_with(
            input.as_bytes(),
            loading(Limits::default(), "doc.xml", serve(DTD)),
        )
        .unwrap();
        assert_eq!(codes(&owned.validate()), expected, "{input}");
    }
}

#[test]
fn separate_dtds_validate_like_dtdvalid() {
    let dtd = Dtd::parse(b"<!ELEMENT r (a)><!ELEMENT a EMPTY><!ATTLIST a i ID #REQUIRED>").unwrap();
    assert_eq!(dtd.name(), None);
    let table: &[(&str, &[Code])] = &[
        ("<r><a i='x'/></r>", &[]),
        // The DOCTYPE name and the document's own declarations are ignored.
        ("<!DOCTYPE q [<!ELEMENT r EMPTY>]><r><a i='x'/></r>", &[]),
        (
            "<r><a/><b/></r>",
            &[ContentModel, MissingAttribute, UnknownElement],
        ),
    ];
    for &(input, expected) in table {
        let document = Document::parse(input).unwrap();
        assert_eq!(
            codes(&valid::validate(&document, &dtd)),
            expected,
            "{input}"
        );
        let owned = OwnedDocument::parse(input).unwrap();
        assert_eq!(codes(&valid::validate(&owned, &dtd)), expected, "{input}");
    }
}

#[test]
fn separate_dtd_loading_reports_failures() {
    let loaded = Dtd::load(
        "dtd/r.dtd",
        Limits::default(),
        serve(&[
            ("dtd/r.dtd", "<!ENTITY % e SYSTEM 'e.ent'>%e;"),
            ("dtd/e.ent", "<!ELEMENT r EMPTY>"),
        ]),
    )
    .unwrap();
    let document = Document::parse("<r/>").unwrap();
    assert!(valid::validate(&document, &loaded).is_empty());
    let declined = Dtd::load("missing.dtd", Limits::default(), serve(&[])).unwrap_err();
    assert_eq!(declined.kind, ErrorKind::ExternalResource);
    let malformed = Dtd::parse(b"<!ELEMENT r (a>").unwrap_err();
    assert_eq!(malformed.kind, ErrorKind::InvalidDtd);
    let declined = |request: Request<'_>| {
        assert_eq!(request.kind, ResourceKind::DtdSubset);
        Ok(None)
    };
    assert!(Dtd::load("x", Limits::default(), Box::new(declined)).is_err());
}

#[test]
fn pathological_content_models_are_bounded() {
    // A chain of optional names has a quadratic Glushkov automaton (about
    // 10 MB for 1300 names); the DTD storage budget stops a series of them.
    let names: Vec<_> = (0..1300).map(|index| format!("e{index}?")).collect();
    let chain = |elements: usize| {
        let declarations: String = (0..elements)
            .map(|index| format!("<!ELEMENT r{index} ({})>", names.join(",")))
            .collect();
        format!("<!DOCTYPE r0 [{declarations}]><r0/>")
    };
    let one = chain(1);
    assert!(Document::parse(&one).unwrap().validate().is_empty());
    let error = Document::parse(&chain(8)).err().map(|error| error.kind);
    assert_eq!(
        error,
        Some(ErrorKind::LimitExceeded(yaxml::Limit::OwnedBytes))
    );
    // Nested repetition stays linear in the document's children.
    let model = format!("{}a{}", "(".repeat(200), ")*".repeat(200));
    let children = "<a/>".repeat(10_000);
    let input = format!("<!DOCTYPE r [<!ELEMENT r {model}><!ELEMENT a EMPTY>]><r>{children}</r>");
    let document = Document::parse(&input).unwrap();
    assert!(document.validate().is_empty());
}

/// Each table row has the same number of validity errors and warnings as
/// `xmllint --valid --noent --dtdattr` reports.
#[test]
#[ignore = "requires YAXML_XMLLINT pointing at the pinned libxml2 2.15.4 xmllint"]
fn oracle_xmllint_reports_the_same_number_of_validity_diagnostics() {
    let xmllint = std::env::var_os("YAXML_XMLLINT").expect("YAXML_XMLLINT");
    let directory = std::env::temp_dir().join(format!("yaxml-valid-oracle-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("case.xml");
    let mut differences = Vec::new();
    for &(input, _) in CASES {
        std::fs::write(&path, input).unwrap();
        let output = std::process::Command::new(&xmllint)
            .args(["--valid", "--noent", "--dtdattr", "--noout"])
            .arg(&path)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        let oracle = stderr.matches("validity ").count();
        let ours = Document::parse(input).unwrap().validate().len();
        if oracle != ours {
            differences.push(format!("{input}: libxml2 {oracle}, yaxml {ours}\n{stderr}"));
        }
    }
    std::fs::remove_dir_all(&directory).ok();
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

/// libxml2's own `test/valid` corpus: the number of validity errors in each
/// `result/valid/*.err` matches yaxml's.
#[test]
#[ignore = "requires YAXML_LIBXML2_SOURCE pointing at the pinned libxml2 2.15.4 source tree"]
fn libxml2_valid_corpus_has_the_same_error_counts() {
    let source = std::path::PathBuf::from(std::env::var_os("YAXML_LIBXML2_SOURCE").unwrap());
    let resolver = || Box::new(yaxml::FileResolver::new(&source).unwrap());
    let mut compared = 0;
    let mut differences = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(source.join("test/valid"))
        .unwrap()
        .collect();
    entries.sort_by_key(|entry| entry.as_ref().unwrap().file_name());
    for entry in entries {
        let name = entry.unwrap().file_name().into_string().unwrap();
        if !name.ends_with(".xml") {
            continue;
        }
        let errors =
            std::fs::read_to_string(source.join("result/valid").join(format!("{name}.err")))
                .unwrap_or_default();
        if errors.contains("parser error") {
            continue;
        }
        let input = std::fs::read(source.join("test/valid").join(&name)).unwrap();
        let base = format!("test/valid/{name}");
        let document =
            OwnedDocument::parse_with(&input, loading(Limits::default(), &base, resolver()));
        let Ok(document) = document else {
            differences.push(format!("{name}: {:?}", document.err()));
            continue;
        };
        let ours = document
            .validate()
            .iter()
            .filter(|d| !d.code.is_warning())
            .count();
        let oracle = errors.matches("validity error").count();
        compared += 1;
        if ours != oracle {
            differences.push(format!("{name}: libxml2 {oracle}, yaxml {ours}"));
        }
    }
    assert_eq!(compared, 30);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

/// Complete DTD processing (`NOENT | DTDATTR | DTDLOAD`) through `resolver`.
fn loading(
    limits: yaxml::Limits,
    base: &str,
    resolver: Box<dyn yaxml::Resolver>,
) -> yaxml::ParseOptions {
    let flags = yaxml::options::LEGACY | yaxml::options::DTDLOAD;
    yaxml::ParseOptions::new(flags)
        .with_limits(limits)
        .with_resolver(base, resolver)
}

/// libxml2's binary `xmlElementContent` shape of a particle: group items
/// right-nested through `c2`, the occurrence on the outermost node.
fn libxml2_tree(particle: valid::Particle<'_>) -> String {
    use valid::{Occurrence, ParticleKind};
    let occurrence = match particle.occurrence() {
        Occurrence::Once => "",
        Occurrence::Optional => "?",
        Occurrence::ZeroOrMore => "*",
        Occurrence::OneOrMore => "+",
    };
    let connector = match particle.kind() {
        ParticleKind::Pcdata => return format!("#PCDATA{occurrence}"),
        ParticleKind::Element => return format!("{}{occurrence}", particle.name().unwrap()),
        ParticleKind::Sequence => "SEQ",
        ParticleKind::Choice => "OR",
    };
    let items: Vec<String> = particle.children().map(libxml2_tree).collect();
    let chain = (items.iter().rev()).fold(None, |tail: Option<String>, item| match tail {
        None => Some(item.clone()),
        Some(tail) => Some(format!("{connector}[{item} {tail}]")),
    });
    format!("{}{occurrence}", chain.unwrap())
}

#[test]
fn content_models_have_libxml2_trees() {
    // Recorded from xmlGetDtdElementDesc(...)->content (c1/c2, ocur), except
    // that a same-connector group without an indicator merges into its
    // parent (libxml2: SEQ[SEQ[a b] c] and OR[OR[a b] c]), as
    // xmlBufDumpElementContent prints it.
    let cases = [
        ("(a)", "a"),
        ("(a)*", "a*"),
        ("(a*)", "a*"),
        ("(a,b,c)", "SEQ[a SEQ[b c]]"),
        ("(a,(b,c))", "SEQ[a SEQ[b c]]"),
        ("((a,b),c)", "SEQ[a SEQ[b c]]"),
        ("((a,b)*,c)", "SEQ[SEQ[a b]* c]"),
        ("(a|b|c)+", "OR[a OR[b c]]+"),
        ("(a,(b|c)*,d?)", "SEQ[a SEQ[OR[b c]* d?]]"),
        ("(#PCDATA)", "#PCDATA"),
        ("(#PCDATA)*", "#PCDATA*"),
        ("(#PCDATA|a|b)*", "OR[#PCDATA OR[a b]]*"),
        ("((a))", "a"),
        ("((a)*)", "a*"),
        ("((a,b))?", "SEQ[a b]?"),
        ("(p:a,b)", "SEQ[p:a b]"),
        ("(a?,b+)*", "SEQ[a? b+]*"),
        ("((a|b),(c|d))", "SEQ[OR[a b] OR[c d]]"),
        ("(a|(b|c))", "OR[a OR[b c]]"),
        ("((a|b)|c)", "OR[a OR[b c]]"),
        ("((a,b)*)", "SEQ[a b]*"),
        ("(((a)))+", "a+"),
    ];
    for (model, tree) in cases {
        let input = format!("<!DOCTYPE r [<!ELEMENT r {model}>]><r/>");
        let document = Document::parse(&input).unwrap();
        let content = document.dtd().unwrap().element("r").unwrap().content();
        assert_eq!(content.map(libxml2_tree).as_deref(), Some(tree), "{model}");
    }
    // No tree without a mixed or element model.
    let input =
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ELEMENT a ANY><!ATTLIST b c CDATA #IMPLIED>]><r/>";
    let document = Document::parse(input).unwrap();
    let dtd = document.dtd().unwrap();
    for name in ["r", "a", "b"] {
        assert_eq!(dtd.element(name).unwrap().content(), None, "{name}");
        assert_eq!(
            dtd.element(name)
                .unwrap()
                .content()
                .map(|c| c.children().count()),
            None
        );
    }
    let leaf = Document::parse("<!DOCTYPE r [<!ELEMENT r (a)>]><r/>").unwrap();
    let leaf = leaf.dtd().unwrap().element("r").unwrap().content().unwrap();
    assert_eq!((leaf.children().count(), leaf.name()), (0, Some("a")));
}
