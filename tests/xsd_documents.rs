//! Multi-document schemas through an in-memory resolver, and pathological
//! schemas that must stay bounded. Expectations use the oracle format of
//! `tests/xsd.rs`; the multi-document ones were checked with xmllint over
//! the same files on disk.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use yaxml::OwnedDocument;
use yaxml::xsd::{Code, Limits, Schema};
use yaxml::{Request, Resolver, ResourceKind};

const BASE: &str = "file:///s/";

fn head(attributes: &str) -> String {
    format!("<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema' {attributes}>")
}

/// `files` served under [`BASE`], counting requests.
fn resolver(files: &[(&str, String)], requests: Arc<Mutex<Vec<String>>>) -> Box<dyn Resolver> {
    let files: HashMap<String, Vec<u8>> = files
        .iter()
        .map(|(name, text)| (format!("{BASE}{name}"), text.as_bytes().to_vec()))
        .collect();
    Box::new(move |request: Request<'_>| {
        assert_eq!(request.kind, ResourceKind::Schema);
        requests.lock().unwrap().push(request.uri.to_owned());
        Ok(files.get(request.uri).cloned())
    })
}

fn codes<'a>(prefix: &str, codes: impl Iterator<Item = &'a Code>) -> String {
    let codes: Vec<String> = codes
        .filter(|c| !c.is_warning())
        .map(|&c| (c as u16).to_string())
        .collect();
    format!("{prefix} {}", codes.join(" "))
        .trim_end()
        .to_owned()
}

fn outcome(main: &str, files: &[(&str, String)], instance: &str, limits: Limits) -> String {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let uri = format!("{BASE}main.xsd");
    let built = Schema::with_resolver(
        main.as_bytes(),
        &uri,
        Some(resolver(files, requests.clone())),
        limits,
    );
    match built {
        Err(error) => codes("S", error.diagnostics.iter().map(|d| &d.code)),
        Ok(schema) => {
            let document = OwnedDocument::parse(instance).unwrap();
            let diagnostics = schema.validate(&document);
            codes("V", diagnostics.iter().map(|d| &d.code))
        }
    }
}

/// `(name, main schema, other files, instance, expected)`.
type DocCase = (
    &'static str,
    String,
    Vec<(&'static str, String)>,
    &'static str,
    &'static str,
);

#[test]
fn documents_combine_as_libxml2_does() {
    let tns = "targetNamespace='urn:t' xmlns:t='urn:t' elementFormDefault='qualified'";
    let element = |name: &str, ty: &str| format!("<xs:element name='{name}' type='{ty}'/>");
    let doc = |attributes: &str, body: &str| format!("{}{body}</xs:schema>", head(attributes));
    let int_root = "<r xmlns='urn:t'>1</r>";
    let cases: Vec<DocCase> = vec![
        (
            "include",
            doc(tns, "<xs:include schemaLocation='a.xsd'/>"),
            vec![("a.xsd", doc(tns, &element("r", "xs:int")))],
            int_root,
            "V",
        ),
        (
            "include invalid instance",
            doc(tns, "<xs:include schemaLocation='a.xsd'/>"),
            vec![("a.xsd", doc(tns, &element("r", "xs:int")))],
            "<r xmlns='urn:t'>x</r>",
            "V 1824",
        ),
        (
            "include missing",
            doc(tns, "<xs:include schemaLocation='a.xsd'/>"),
            vec![],
            int_root,
            "S 3050",
        ),
        (
            "include other namespace",
            doc(tns, "<xs:include schemaLocation='a.xsd'/>"),
            vec![(
                "a.xsd",
                doc("targetNamespace='urn:other'", &element("r", "xs:int")),
            )],
            int_root,
            "S 3050",
        ),
        (
            "chameleon include",
            doc(
                tns,
                "<xs:include schemaLocation='a.xsd'/><xs:element name='r' type='t:c'/>",
            ),
            vec![(
                "a.xsd",
                doc(
                    "",
                    "<xs:simpleType name='c'><xs:restriction base='xs:int'/></xs:simpleType>",
                ),
            )],
            int_root,
            "V",
        ),
        (
            "include not a schema",
            doc(tns, "<xs:include schemaLocation='a.xsd'/>"),
            vec![("a.xsd", "<schema/>".into())],
            int_root,
            "S 1772",
        ),
        (
            "include malformed",
            doc(tns, "<xs:include schemaLocation='a.xsd'/>"),
            vec![("a.xsd", "<xs:schema".into())],
            int_root,
            "S 3067",
        ),
        (
            "include cycle",
            doc(tns, "<xs:include schemaLocation='a.xsd'/>"),
            vec![(
                "a.xsd",
                doc(
                    tns,
                    &format!(
                        "<xs:include schemaLocation='main.xsd'/>{}",
                        element("r", "xs:int")
                    ),
                ),
            )],
            int_root,
            "V",
        ),
        (
            "import",
            doc(
                "xmlns:b='urn:b'",
                "<xs:import namespace='urn:b' schemaLocation='b.xsd'/><xs:element name='r'><xs:complexType><xs:sequence><xs:element ref='b:e'/></xs:sequence></xs:complexType></xs:element>",
            ),
            vec![(
                "b.xsd",
                doc("targetNamespace='urn:b'", &element("e", "xs:int")),
            )],
            "<r><e xmlns='urn:b'>5</e></r>",
            "V",
        ),
        (
            "import missing location",
            doc(
                "xmlns:b='urn:b'",
                "<xs:import namespace='urn:b' schemaLocation='b.xsd'/><xs:element name='r' type='b:t'/>",
            ),
            vec![],
            "<r/>",
            "S 3004",
        ),
        (
            "import namespace mismatch",
            doc("", "<xs:import namespace='urn:b' schemaLocation='b.xsd'/>"),
            vec![("b.xsd", doc("targetNamespace='urn:c'", ""))],
            "<r/>",
            "V 1845",
        ),
        (
            "import cycle",
            doc(
                "xmlns:b='urn:b'",
                "<xs:import namespace='urn:b' schemaLocation='b.xsd'/><xs:element name='r' type='b:t'/>",
            ),
            vec![(
                "b.xsd",
                doc(
                    "targetNamespace='urn:b'",
                    "<xs:import schemaLocation='main.xsd'/><xs:simpleType name='t'><xs:restriction base='xs:int'/></xs:simpleType>",
                ),
            )],
            "<r>x</r>",
            "V 1824",
        ),
        (
            "redefine simple type",
            doc(
                tns,
                "<xs:redefine schemaLocation='a.xsd'><xs:simpleType name='s'><xs:restriction base='t:s'><xs:maxInclusive value='5'/></xs:restriction></xs:simpleType></xs:redefine><xs:element name='r' type='t:s'/>",
            ),
            vec![(
                "a.xsd",
                doc(
                    tns,
                    "<xs:simpleType name='s'><xs:restriction base='xs:int'/></xs:simpleType>",
                ),
            )],
            "<r xmlns='urn:t'>9</r>",
            "V 1834",
        ),
        (
            "redefine group",
            doc(
                tns,
                "<xs:redefine schemaLocation='a.xsd'><xs:group name='g'><xs:sequence><xs:group ref='t:g'/><xs:element name='b'/></xs:sequence></xs:group></xs:redefine><xs:element name='r'><xs:complexType><xs:group ref='t:g'/></xs:complexType></xs:element>",
            ),
            vec![(
                "a.xsd",
                doc(
                    tns,
                    "<xs:group name='g'><xs:sequence><xs:element name='a'/></xs:sequence></xs:group>",
                ),
            )],
            "<r xmlns='urn:t'><a/></r>",
            "V 1871",
        ),
        (
            "redefine missing component",
            doc(
                tns,
                "<xs:redefine schemaLocation='a.xsd'><xs:simpleType name='z'><xs:restriction base='t:z'/></xs:simpleType></xs:redefine>",
            ),
            vec![("a.xsd", doc(tns, ""))],
            "<r/>",
            "S 3081",
        ),
    ];
    let mut failures = Vec::new();
    for (name, main, files, instance, want) in &cases {
        let got = outcome(main, files, instance, Limits::default());
        if got != *want {
            failures.push(format!("{name}: expected {want}, got {got}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn without_resolver_nothing_loads() {
    let main = format!(
        "{}<xs:include schemaLocation='a.xsd'/></xs:schema>",
        head("")
    );
    let error = Schema::parse(main.as_bytes(), "main.xsd").unwrap_err();
    assert_eq!(error.diagnostics[0].code, Code::SrcInclude);
    let main = format!(
        "{}<xs:import namespace='urn:b' schemaLocation='b.xsd'/></xs:schema>",
        head("")
    );
    assert!(Schema::parse(main.as_bytes(), "main.xsd").is_ok());
}

#[test]
fn document_budgets_bound_loading() {
    // A chain of imports, each document importing the next.
    let chain: Vec<(String, String)> = (0..50)
        .map(|i| {
            let body = format!(
                "{}<xs:import namespace='urn:{}' schemaLocation='d{}.xsd'/></xs:schema>",
                head(&format!("targetNamespace='urn:{i}'")),
                i + 1,
                i + 1
            );
            (format!("d{i}.xsd"), body)
        })
        .collect();
    let files: Vec<(&str, String)> = chain.iter().map(|(n, t)| (n.as_str(), t.clone())).collect();
    let main = format!(
        "{}<xs:import namespace='urn:0' schemaLocation='d0.xsd'/></xs:schema>",
        head("")
    );
    let run = |limits: Limits| {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let built = Schema::with_resolver(
            main.as_bytes(),
            "file:///s/main.xsd",
            Some(resolver(&files, requests.clone())),
            limits,
        );
        let count = requests.lock().unwrap().len();
        (built.is_ok(), count)
    };
    assert_eq!(run(Limits::default()), (true, 51));
    let limits = {
        let mut cfg = Limits::default();
        cfg.max_documents = 10;
        cfg
    };
    assert_eq!(run(limits), (false, 10));
    let limits = {
        let mut cfg = Limits::default();
        cfg.max_bytes = 1000;
        cfg
    };
    let (_, count) = run(limits);
    assert!(count < 51);
}

fn timed<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{what} took {:?}",
        start.elapsed()
    );
    out
}

fn build_with(body: &str, limits: Limits) -> Result<Schema, Vec<Code>> {
    let text = format!("{}{body}</xs:schema>", head(""));
    Schema::with_resolver(text.as_bytes(), "p.xsd", None, limits)
        .map_err(|e| e.diagnostics.iter().map(|d| d.code).collect())
}

fn build(body: &str) -> Result<Schema, Vec<Code>> {
    build_with(body, Limits::default())
}

fn valid(schema: &Schema, instance: &str) -> Vec<Code> {
    let document = OwnedDocument::parse(instance).unwrap();
    schema.validate(&document).iter().map(|d| d.code).collect()
}

#[test]
fn huge_occurrence_bounds_use_counters() {
    let schema = timed("compile", || {
        build("<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a' minOccurs='3' maxOccurs='1000000000'/></xs:sequence></xs:complexType></xs:element>").unwrap()
    });
    let many = format!("<r>{}</r>", "<a/>".repeat(100_000));
    assert_eq!(timed("validate", || valid(&schema, &many)), vec![]);
    assert_eq!(
        valid(&schema, "<r><a/><a/></r>"),
        vec![Code::ElementContent]
    );
    let bounded = build("<xs:element name='r'><xs:complexType><xs:sequence maxOccurs='50000'><xs:element name='a'/></xs:sequence></xs:complexType></xs:element>").unwrap();
    let over = format!("<r>{}</r>", "<a/>".repeat(50_001));
    assert_eq!(
        timed("validate", || valid(&bounded, &over)),
        vec![Code::ElementContent]
    );
}

#[test]
fn nested_counters_are_capped() {
    // (a{1,2}){1,30}: counter vectors multiply but stay under the cap.
    let schema = build(
        "<xs:element name='r'><xs:complexType><xs:sequence minOccurs='1' maxOccurs='30'><xs:sequence minOccurs='1' maxOccurs='2'><xs:element name='a' minOccurs='1' maxOccurs='2'/></xs:sequence></xs:sequence></xs:complexType></xs:element>",
    );
    let codes = match schema {
        Ok(schema) => timed("validate", || {
            valid(&schema, &format!("<r>{}</r>", "<a/>".repeat(200)))
        }),
        Err(codes) => codes,
    };
    assert!(codes.iter().all(|&c| c != Code::VInternal) || codes == vec![Code::VInternal]);
}

#[test]
fn deep_and_wide_models_stay_bounded() {
    let deep = |n: usize| {
        format!(
            "<xs:element name='r'><xs:complexType>{}<xs:element name='a'/>{}</xs:complexType></xs:element>",
            "<xs:sequence>".repeat(n),
            "</xs:sequence>".repeat(n)
        )
    };
    let schema = timed("deep", || build(&deep(100)).unwrap());
    assert_eq!(valid(&schema, "<r><a/></r>"), vec![]);
    // Past the XML nesting limit the schema document itself is rejected.
    assert!(timed("deeper", || build(&deep(2000))).is_err());
    // n optional elements: quadratic transitions after epsilon removal.
    let wide = |n: usize| {
        let items: String = (0..n)
            .map(|i| format!("<xs:element name='e{i}' minOccurs='0'/>"))
            .collect();
        format!(
            "<xs:element name='r'><xs:complexType><xs:sequence>{items}</xs:sequence></xs:complexType></xs:element>"
        )
    };
    let schema = timed("wide", || build(&wide(300)).unwrap());
    assert_eq!(valid(&schema, "<r><e1/><e299/></r>"), vec![]);
    assert_eq!(
        valid(&schema, "<r><e299/><e1/></r>"),
        vec![Code::ElementContent]
    );
    let small = {
        let mut cfg = Limits::default();
        cfg.max_model_work = 1 << 18;
        cfg
    };
    assert_eq!(
        timed("too wide", || build_with(&wide(20_000), small).err()),
        Some(vec![Code::Internal])
    );
    let chain: String = (0..5000)
        .map(|i| {
            format!(
                "<xs:group name='g{i}'><xs:sequence><xs:group ref='g{}'/></xs:sequence></xs:group>",
                i + 1
            )
        })
        .collect();
    let body = format!(
        "{chain}<xs:group name='g5000'><xs:sequence><xs:element name='a'/></xs:sequence></xs:group><xs:element name='r'><xs:complexType><xs:group ref='g0'/></xs:complexType></xs:element>"
    );
    let built = timed("group chain", || build(&body));
    assert!(built.is_ok() || built.unwrap_err().contains(&Code::Internal));
}

#[test]
fn recursive_definitions_are_rejected_or_bounded() {
    let cases = [
        (
            "<xs:group name='g'><xs:sequence><xs:group ref='g'/></xs:sequence></xs:group>",
            Some(Code::MgPropsCorrect2),
        ),
        (
            "<xs:attributeGroup name='g'><xs:attributeGroup ref='g'/></xs:attributeGroup>",
            Some(Code::SrcAttributeGroup3),
        ),
        (
            "<xs:simpleType name='s'><xs:restriction base='s'/></xs:simpleType>",
            Some(Code::StPropsCorrect2),
        ),
        (
            "<xs:complexType name='c'><xs:complexContent><xs:extension base='c'/></xs:complexContent></xs:complexType>",
            Some(Code::StPropsCorrect2),
        ),
        // Recursion through element declarations is legal.
        (
            "<xs:element name='r'><xs:complexType><xs:sequence><xs:element ref='r' minOccurs='0'/></xs:sequence></xs:complexType></xs:element>",
            None,
        ),
    ];
    for (body, want) in cases {
        match (build(body), want) {
            (Ok(_), None) => {}
            (Err(codes), Some(code)) => assert!(codes.contains(&code), "{body}: {codes:?}"),
            (got, want) => panic!("{body}: {:?} vs {want:?}", got.err()),
        }
    }
    let schema = build(cases[4].0).unwrap();
    let nested = format!("{}{}", "<r>".repeat(200), "</r>".repeat(200));
    assert_eq!(
        timed("recursive instance", || valid(&schema, &nested)),
        vec![]
    );
}

#[test]
fn model_work_budget_reports_internal_error() {
    let text = format!(
        "{}<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a'/><xs:element name='b'/></xs:sequence></xs:complexType></xs:element></xs:schema>",
        head("")
    );
    let limits = {
        let mut cfg = Limits::default();
        cfg.max_model_work = 3;
        cfg
    };
    let error = Schema::with_resolver(text.as_bytes(), "p.xsd", None, limits).unwrap_err();
    assert_eq!(error.diagnostics[0].code, Code::Internal);
    assert!(Schema::with_resolver(text.as_bytes(), "p.xsd", None, Limits::default()).is_ok());
}
