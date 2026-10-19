//! Schematron: libxml2's `test/schematron` corpus with runtest's expected
//! output (vendored in `tests/fixtures/schematron`), table-driven schema and
//! validation cases, and (ignored, with `YAXML_SCHEMATRON_HELPER` naming the
//! compiled `tooling/libxml2_schematron.c`) a differential run of all of
//! them, in every output mode, against libxml2.

use std::path::Path;
use std::process::Command;

use yaxml::schematron::{
    Code, Limits, OUT_ERROR, OUT_QUIET, OUT_TEXT, OUT_XML, Output, Schema, Validation,
};
use yaxml::{Document, OwnedDocument, Request, Resolver};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/schematron");
const MODES: [u32; 8] = [
    0,
    OUT_QUIET,
    OUT_TEXT,
    OUT_TEXT | OUT_QUIET,
    OUT_XML,
    OUT_ERROR,
    OUT_ERROR | OUT_QUIET,
    OUT_ERROR | OUT_TEXT,
];

fn none() -> Box<dyn Resolver> {
    Box::new(|_: Request<'_>| Ok(None))
}

/// What the oracle helper prints: schema errors, then per instance the
/// validation output and runtest's verdict. Both DOMs must agree.
fn run(schema: &[u8], name: &str, instances: &[(&str, &[u8])], flags: u32) -> String {
    let schema = match Schema::parse(schema, name, none(), &Limits::default()) {
        Ok(schema) => schema,
        Err(errors) => {
            let text: String = errors.errors.iter().map(|e| e.render(name)).collect();
            return format!("{text}Schematron schema {name} failed to compile\n");
        }
    };
    let mut out = String::new();
    for &(file, bytes) in instances {
        let (Ok(document), Ok(owned)) = (
            Document::parse_bytes(bytes),
            OwnedDocument::with_limits(bytes, Default::default()),
        ) else {
            out += &format!("failed to parse instance {file}\n");
            continue;
        };
        let validation = schema.validate(&document, flags, bytes);
        let text = validation.render(file);
        assert_eq!(text, schema.validate(&owned, flags, bytes).render(file));
        out += &text;
        out += &match validation.errors {
            Some(0) => format!("{file} validates\n"),
            Some(_) => format!("{file} fails to validate\n"),
            None => format!("{file} validation generated an internal error\n"),
        };
    }
    out
}

/// The corpus as runtest names it: (schema, [(instance, expected .err)]).
fn corpus() -> Vec<(String, Vec<(String, String)>)> {
    let directory = Path::new(FIXTURES).join("test");
    let mut names: Vec<String> = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    let schemas = names.iter().filter_map(|name| name.strip_suffix(".sct"));
    schemas
        .map(|prefix| {
            let instances = (names.iter())
                .filter_map(|name| {
                    name.strip_prefix(prefix)?
                        .strip_prefix('_')?
                        .strip_suffix(".xml")
                })
                .filter(|suffix| suffix.len() == 1)
                .map(|suffix| {
                    let result = format!("{FIXTURES}/result/{prefix}_{suffix}.err");
                    (
                        format!("{prefix}_{suffix}.xml"),
                        std::fs::read_to_string(result).unwrap(),
                    )
                })
                .collect();
            (format!("{prefix}.sct"), instances)
        })
        .collect()
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(FIXTURES).join("test").join(name)).unwrap()
}

/// runtest's `schematronTest` (`XML_SCHEMATRON_OUT_ERROR`) reproduces every
/// `result/schematron/*.err` byte for byte.
#[test]
fn libxml2_corpus_matches_runtest_output() {
    let mut checked = 0;
    for (schema, instances) in corpus() {
        for (instance, expected) in instances {
            let file = format!("./test/schematron/{instance}");
            let bytes = read(&instance);
            let found = run(&read(&schema), &schema, &[(&file, &bytes)], OUT_ERROR);
            assert_eq!(found, expected, "{instance}");
            checked += 1;
        }
    }
    assert_eq!(checked, 42);
}

macro_rules! sch {
    ($($body:expr),*) => {
        concat!("<s:schema xmlns:s='http://purl.oclc.org/dsdl/schematron'>", $($body,)* "</s:schema>")
    };
}

/// (name, flags, schema, instance, expected output with files `schema.sct`
/// and `doc.xml`); every expectation was produced by libxml2.
const CASES: &[(&str, u32, &str, &str, &str)] = &[
    // Schema errors.
    (
        "not schematron",
        OUT_TEXT,
        "<schema/>",
        "<r/>",
        "schema.sct:1: element schema: Schemas parser error : The XML document 'schema.sct' is not a XML schematron document\nSchematron schema schema.sct failed to compile\n",
    ),
    (
        "old namespace",
        OUT_TEXT,
        "<schema xmlns='http://www.ascc.net/xml/schematron'><pattern name='p'><rule context='r'><assert test='a'>no a</assert></rule></pattern></schema>",
        "<r/>",
        "Pattern: p\n/r line 1: no a\ndoc.xml fails to validate\n",
    ),
    (
        "no pattern",
        OUT_TEXT,
        sch!("<s:title>T</s:title>"),
        "<r/>",
        "schema.sct:1: element schema: Schemas parser error : The schematron document 'schema.sct' has no pattern\nSchematron schema schema.sct failed to compile\n",
    ),
    (
        "foreign elements skipped",
        OUT_TEXT,
        sch!(
            "<x/><s:pattern id='p'><y/><s:rule context='r'><z/><s:assert test='a'>no a</s:assert></s:rule></s:pattern>"
        ),
        "<r/>",
        "Pattern: p\n/r line 1: no a\ndoc.xml fails to validate\n",
    ),
    (
        "phase include and diagnostics",
        OUT_TEXT,
        sch!(
            "<s:phase id='x'/><s:pattern id='p'><s:rule context='r'><s:assert test='a'/></s:rule></s:pattern><s:include href='x'/><s:diagnostics/>"
        ),
        "<r/>",
        "schema.sct:1: element phase: Schemas parser error : Expecting a pattern element instead of phase\nschema.sct:1: element include: Schemas parser error : Expecting a pattern element instead of include\nschema.sct:1: element diagnostics: Schemas parser error : Expecting a pattern element instead of diagnostics\nSchematron schema schema.sct failed to compile\n",
    ),
    (
        "title after ns",
        OUT_TEXT,
        sch!(
            "<s:ns prefix='p' uri='u'/><s:title>T</s:title><s:pattern id='p'><s:rule context='r'><s:assert test='a'/></s:rule></s:pattern>"
        ),
        "<r/>",
        "schema.sct:1: element title: Schemas parser error : Expecting a pattern element instead of title\nSchematron schema schema.sct failed to compile\n",
    ),
    (
        "ns errors",
        OUT_TEXT,
        sch!(
            "<s:ns prefix='' uri=''/><s:ns/><s:pattern id='p'><s:rule context='r'><s:assert test='a'/></s:rule></s:pattern>"
        ),
        "<r/>",
        "schema.sct:1: element ns: Schemas parser error : ns element has no uri\nschema.sct:1: element ns: Schemas parser error : ns element has no prefix\nschema.sct:1: element ns: Schemas parser error : ns element has no uri\nschema.sct:1: element ns: Schemas parser error : ns element has no prefix\nSchematron schema schema.sct failed to compile\n",
    ),
    (
        "pattern without rules",
        OUT_TEXT,
        sch!("<s:pattern id='p'><s:assert test='a'/></s:pattern>"),
        "<r/>",
        "schema.sct:1: element assert: Schemas parser error : Expecting a rule element instead of assert\nschema.sct:1: element pattern: Schemas parser error : Pattern has no rule element\nSchematron schema schema.sct failed to compile\n",
    ),
    (
        "rule without context",
        OUT_TEXT,
        sch!(
            "<s:pattern id='p'><s:rule><s:assert test='a'/></s:rule><s:rule context=''><s:assert test='a'/></s:rule></s:pattern>"
        ),
        "<r/>",
        "schema.sct:1: element rule: Schemas parser error : rule has no context attribute\nschema.sct:1: element rule: Schemas parser error : rule has an empty context attribute\nSchematron schema schema.sct failed to compile\n",
    ),
    (
        "bad context",
        OUT_TEXT,
        sch!("<s:pattern id='p'><s:rule context='r[1]'><s:assert test='a'/></s:rule></s:pattern>"),
        "<r/>",
        "schema.sct:1: element rule: Schemas parser error : Failed to compile context expression r[1]\nSchematron schema schema.sct failed to compile\n",
    ),
    (
        "unbound context prefix",
        OUT_TEXT,
        sch!("<s:pattern id='p'><s:rule context='q:r'><s:assert test='a'/></s:rule></s:pattern>"),
        "<r/>",
        "schema.sct:1: element rule: Schemas parser error : Failed to compile context expression q:r\nSchematron schema schema.sct failed to compile\n",
    ),
    (
        "rule without checks",
        OUT_TEXT,
        sch!("<s:pattern id='p'><s:rule context='r'><s:value-of select='1'/></s:rule></s:pattern>"),
        "<r/>",
        "schema.sct:1: element value-of: Schemas parser error : Expecting an assert or a report element instead of value-of\nschema.sct:1: element rule: Schemas parser error : rule has no assert nor report element\nSchematron schema schema.sct failed to compile\n",
    ),
    (
        "assert and report without test",
        OUT_TEXT,
        sch!(
            "<s:pattern id='p'><s:rule context='r'><s:assert/><s:report test=''/></s:rule></s:pattern>"
        ),
        "<r/>",
        "schema.sct:1: element assert: Schemas parser error : assert has no test attribute\nschema.sct:1: element report: Schemas parser error : assert has an empty test attribute\nSchematron schema schema.sct failed to compile\n",
    ),
    (
        "bad test",
        OUT_TEXT,
        sch!(
            "<s:pattern id='p'><s:rule context='r'><s:assert test='a[1'>x</s:assert><s:report test='1 +'>y</s:report></s:rule></s:pattern>"
        ),
        "<r/>",
        "XPath error : Invalid predicate\nschema.sct:1: element assert: Schemas parser error : Failed to compile test expression a[1\nXPath error : Invalid expression\nschema.sct:1: element report: Schemas parser error : Failed to compile test expression 1 +\nSchematron schema schema.sct failed to compile\n",
    ),
    (
        "unbound test prefix",
        OUT_TEXT,
        sch!(
            "<s:ns prefix='p' uri='urn:p'/><s:pattern id='p'><s:rule context='r'><s:assert test='p:a or q:b'/></s:rule></s:pattern>"
        ),
        "<r/>",
        "XPath error : Undefined namespace prefix: q\nschema.sct:1: element assert: Schemas parser error : Failed to compile test expression p:a or q:b\nSchematron schema schema.sct failed to compile\n",
    ),
    (
        "let errors",
        OUT_TEXT,
        sch!(
            "<s:pattern id='p'><s:rule context='r'><s:let value='1'/><s:assert test='1'/></s:rule><s:rule context='r'><s:let name='' value='1'/></s:rule><s:rule context='r'><s:let name='v'/></s:rule><s:rule context='r'><s:let name='v' value=''/></s:rule><s:rule context='r'><s:let name='v' value='(('/></s:rule></s:pattern>"
        ),
        "<r/>",
        "schema.sct:1: element let: Schemas parser error : let has no name attribute\nschema.sct:1: element let: Schemas parser error : let has an empty name attribute\nschema.sct:1: element let: Schemas parser error : let has no value attribute\nschema.sct:1: element let: Schemas parser error : let has an empty value attribute\nXPath error : Invalid expression\nschema.sct:1: element let: Schemas parser error : Failed to compile let expression ((\nSchematron schema schema.sct failed to compile\n",
    ),
    (
        "value-of errors",
        OUT_TEXT,
        sch!(
            "<s:pattern id='p'><s:rule context='r'><s:assert test='1'><s:value-of/><s:value-of select='a b'/></s:assert></s:rule></s:pattern>"
        ),
        "<r/>",
        "schema.sct:1: element value-of: Schemas parser error : value-of has no select attribute\nXPath error : Invalid expression\na b\n  ^\nschema.sct:1: element value-of: Schemas parser error : Failed to compile select expression a b\nSchematron schema schema.sct failed to compile\n",
    ),
    (
        "unnamed pattern is skipped",
        OUT_TEXT,
        sch!("<s:pattern><s:rule/></s:pattern>"),
        "<r/>",
        "doc.xml validation generated an internal error\n",
    ),
    // Validation.
    (
        "text mode",
        OUT_TEXT,
        sch!(
            "<s:pattern id='a'><s:rule context='r'><s:assert test='x'>no x</s:assert><s:report test='y'>a y</s:report></s:rule></s:pattern><s:pattern name='b'><s:rule context='y'><s:report test='true()'>y here</s:report></s:rule></s:pattern>"
        ),
        "<r><y/></r>",
        "Pattern: a\n/r line 1: no x\n/r line 1: a y\nPattern: b\n/r/y line 1: y here\ndoc.xml fails to validate\n",
    ),
    (
        "quiet mode",
        OUT_QUIET,
        sch!(
            "<s:pattern id='a'><s:rule context='r'><s:assert test='x'>no x</s:assert><s:report test='y'>a y</s:report></s:rule></s:pattern>"
        ),
        "<r><y/></r>",
        "/r line 1: no x\ndoc.xml fails to validate\n",
    ),
    (
        "no flags",
        0,
        sch!(
            "<s:pattern id='a'><s:rule context='r'><s:assert test='x'>no x</s:assert><s:report test='y'>a y</s:report></s:rule></s:pattern><s:pattern id='b'><s:rule context='r'><s:assert test='z'>no z</s:assert></s:rule></s:pattern>"
        ),
        "<r><y/></r>",
        "/r line 1: no x\n/r line 1: a y\n/r line 1: no z\ndoc.xml fails to validate\n",
    ),
    (
        "svrl outputs nothing",
        OUT_XML,
        sch!(
            "<s:pattern id='a'><s:rule context='r'><s:assert test='x'>no x</s:assert></s:rule></s:pattern>"
        ),
        "<r/>",
        "doc.xml fails to validate\n",
    ),
    (
        "valid",
        OUT_TEXT,
        sch!(
            "<s:pattern id='a'><s:rule context='r'><s:assert test='x'>no x</s:assert><s:report test='y'>a y</s:report></s:rule></s:pattern>"
        ),
        "<r><x/></r>",
        "Pattern: a\ndoc.xml validates\n",
    ),
    (
        "no rules",
        OUT_TEXT,
        sch!("<s:pattern><s:rule context='r'><s:assert test='x'/></s:rule></s:pattern>"),
        "<r/>",
        "doc.xml validation generated an internal error\n",
    ),
    (
        "empty message",
        OUT_ERROR,
        sch!(
            "<s:pattern id='a'><s:rule context='r'><s:assert test='x'/><s:report test='1'><!--c--></s:report></s:rule></s:pattern>"
        ),
        "<r/>",
        "doc.xml:1: element r: schematron error : /r line 1: node failed assert\ndoc.xml:1: element r: schematron error : /r line 1: node failed report\ndoc.xml fails to validate\n",
    ),
    (
        "paths and lines",
        OUT_TEXT,
        sch!(
            "<s:pattern id='a'><s:rule context='b'><s:report test='1'>b</s:report></s:rule></s:pattern>"
        ),
        "<r xmlns:p='urn:p'>\n<b/><p:b/><b\n  a='>'\n/><c><b/></c>\n<b xmlns='urn:d'/><b xmlns='urn:d'/></r>",
        "Pattern: a\n/r/b[1] line 2: b\n/r/b[2] line 4: b\n/r/c/b line 4: b\ndoc.xml fails to validate\n",
    ),
    (
        "namespaces",
        OUT_TEXT,
        sch!(
            "<s:ns prefix='p' uri='urn:p'/><s:pattern id='a'><s:rule context='p:b'><s:report test='@p:x'><s:name/> <s:name path='@p:x'/> <s:value-of select='namespace-uri()'/></s:report></s:rule></s:pattern>"
        ),
        "<r xmlns:q='urn:p'><q:b q:x='1'/></r>",
        "Pattern: a\n/r/q:b line 1: q:b q:x urn:p\ndoc.xml fails to validate\n",
    ),
    (
        "name targets",
        OUT_TEXT,
        sch!(
            "<s:pattern id='a'><s:rule context='b'><s:report test='1'>[<s:name path='text()'/>][<s:name path='..'/>][<s:name path='@a'/>][<s:name path='missing'/>][<s:name path='1'/>][<s:name path='/'/>]</s:report></s:rule></s:pattern>"
        ),
        "<r><b a='1'>t</b></r>",
        "Pattern: a\n/r/b line 1: [][r][a][b][b][]\ndoc.xml fails to validate\n",
    ),
    (
        "bad name path",
        OUT_TEXT,
        sch!(
            "<s:pattern id='a'><s:rule context='b'><s:report test='1'>[<s:name path='a[['/>][<s:name path='$v'/>]</s:report></s:rule></s:pattern>"
        ),
        "<r><b/></r>",
        "Pattern: a\nXPath error : Invalid expression\na[[\n  ^\nXPath error : Undefined variable: v\n/r/b line 1: [b][b]\ndoc.xml fails to validate\n",
    ),
    (
        "value-of types",
        OUT_TEXT,
        sch!(
            "<s:pattern id='a'><s:rule context='b'><s:report test='1'><s:value-of select='1 div 3'/>|<s:value-of select='1234567'/>|<s:value-of select='0 div 0'/>|<s:value-of select='-1 div 0'/>|<s:value-of select='1=1'/>|<s:value-of select='1=2'/>|<s:value-of select='concat(\"a\", \"b\")'/>|<s:value-of select='\"\"'/>|</s:report></s:rule></s:pattern>"
        ),
        "<r><b/></r>",
        "Pattern: a\n/r/b line 1: 0.333333|1.23457e+06|nan|-inf|True|False|ab||\ndoc.xml fails to validate\n",
    ),
    (
        "value-of node names",
        OUT_TEXT,
        sch!(
            "<s:pattern id='a'><s:rule context='b'><s:report test='1'><s:value-of select='node() | @*'/>|<s:value-of select='namespace::*'/>|<s:value-of select='/'/>|<s:value-of select='nothing'/>|</s:report></s:rule></s:pattern>"
        ),
        "<r><b xmlns:p='urn:p' p:a='1' c='2'><p:i/>t<!--c--><?pi x?><![CDATA[d]]></b></r>",
        "Pattern: a\n/r/b line 1: a c i text comment pi |http://www.w3.org/XML/1998/namespace urn:p|||\ndoc.xml fails to validate\n",
    ),
    (
        "value-of failure stops the message",
        OUT_TEXT,
        sch!(
            "<s:pattern id='a'><s:rule context='b'><s:report test='1'>before <s:value-of select='nope()'/> after</s:report><s:report test='1'><s:value-of select='$nope'/></s:report></s:rule></s:pattern>"
        ),
        "<r><b/></r>",
        "Pattern: a\nXPath error : Unregistered function: nope\n/r/b line 1: before \nXPath error : Undefined variable: nope\n/r/b line 1: node failed report\ndoc.xml fails to validate\n",
    ),
    (
        "whitespace",
        OUT_TEXT,
        sch!(
            "<s:pattern id='a'><s:rule context='b'><s:report test='1'>\n  one\t <s:value-of select='\"two  \"'/>  <![CDATA[three \n]]>\n</s:report><s:report test='1'>   </s:report></s:rule></s:pattern>"
        ),
        "<r><b/></r>",
        "Pattern: a\n/r/b line 1: \n  one two three \n/r/b line 1:  \ndoc.xml fails to validate\n",
    ),
    (
        "test values",
        OUT_TEXT,
        sch!(
            "<s:pattern id='a'><s:rule context='b'><s:assert test='0'>0</s:assert><s:assert test='0 div 0'>nan</s:assert><s:assert test='\"\"'>empty</s:assert><s:assert test='x'>none</s:assert><s:assert test='2'>2</s:assert><s:assert test='\"a\"'>a</s:assert><s:assert test='nope()'>error</s:assert></s:rule></s:pattern>"
        ),
        "<r><b/></r>",
        "Pattern: a\n/r/b line 1: 0\n/r/b line 1: nan\n/r/b line 1: empty\n/r/b line 1: none\nXPath error : Unregistered function: nope\n/r/b line 1: error\ndoc.xml fails to validate\n",
    ),
    (
        "lets",
        OUT_TEXT,
        sch!(
            "<s:pattern id='a'><s:rule context='b'><s:let name='n' value='count(*)'/><s:let name='m' value='$n + 1'/><s:report test='$n = 2'>n=<s:value-of select='$n'/></s:report></s:rule></s:pattern>"
        ),
        "<r><b><i/><i/></b></r>",
        "Pattern: a\nXPath error : Undefined variable: n\nSchemas validity error : Evaluation of compiled expression failed\nXPath error : Undefined variable: n\nSchemas validity error : Unregistering a let variable failed\ndoc.xml fails to validate\n",
    ),
    (
        "let order",
        OUT_TEXT,
        sch!(
            "<s:pattern id='a'><s:rule context='b'><s:let name='n' value='1'/><s:let name='m' value='$n + 1'/><s:report test='true()'>m=<s:value-of select='$m'/></s:report></s:rule></s:pattern>"
        ),
        "<r><b/></r>",
        "Pattern: a\nXPath error : Undefined variable: n\nSchemas validity error : Evaluation of compiled expression failed\nXPath error : Undefined variable: m\n/r/b line 1: m=\nSchemas validity error : Unregistering a let variable failed\ndoc.xml fails to validate\n",
    ),
    (
        "let order quiet",
        OUT_QUIET,
        sch!(
            "<s:pattern id='a'><s:rule context='b'><s:let name='n' value='1'/><s:let name='m' value='$n + 1'/><s:assert test='false()'>m</s:assert></s:rule></s:pattern>"
        ),
        "<r><b/></r>",
        "XPath error : Undefined variable: n\nSchemas validity error : Evaluation of compiled expression failed\ndoc.xml validation generated an internal error\n",
    ),
    (
        "let scope",
        OUT_TEXT,
        sch!(
            "<s:pattern id='a'><s:rule context='b'><s:let name='v' value='1'/><s:assert test='$v'>b</s:assert></s:rule><s:rule context='r'><s:assert test='$v'>r</s:assert></s:rule></s:pattern>"
        ),
        "<r><b/></r>",
        "Pattern: a\nXPath error : Undefined variable: v\n/r line 1: r\ndoc.xml fails to validate\n",
    ),
    (
        "rule order and unions",
        OUT_ERROR,
        sch!(
            "<s:pattern id='a'><s:rule context='r|c'><s:report test='1'>first</s:report></s:rule><s:rule context='//c'><s:report test='1'>second</s:report></s:rule></s:pattern><s:pattern id='b'><s:rule context='r/c'><s:report test='1'>third</s:report></s:rule></s:pattern>"
        ),
        "<r><c/><c/></r>",
        "doc.xml:1: element r: schematron error : /r line 1: first\ndoc.xml:1: element c: schematron error : /r/c[1] line 1: first\ndoc.xml:1: element c: schematron error : /r/c[1] line 1: second\ndoc.xml:1: element c: schematron error : /r/c[2] line 1: first\ndoc.xml:1: element c: schematron error : /r/c[2] line 1: second\ndoc.xml:1: element c: schematron error : /r/c[1] line 1: third\ndoc.xml:1: element c: schematron error : /r/c[2] line 1: third\ndoc.xml fails to validate\n",
    ),
    (
        "error mode quiet",
        OUT_ERROR | OUT_QUIET,
        sch!(
            "<s:pattern id='a'><s:rule context='*'><s:assert test='false()'>a</s:assert><s:report test='true()'>r</s:report></s:rule></s:pattern>"
        ),
        "<r><c/></r>",
        "doc.xml:1: element r: schematron error : /r line 1: a\ndoc.xml:1: element c: schematron error : /r/c line 1: a\ndoc.xml fails to validate\n",
    ),
    (
        "comments and pis are not contexts",
        OUT_TEXT,
        sch!(
            "<s:pattern id='a'><s:rule context='*'><s:report test='1'>e</s:report></s:rule></s:pattern>"
        ),
        "<?pi?><!--c--><r/><!--d-->",
        "Pattern: a\n/r line 1: e\ndoc.xml fails to validate\n",
    ),
    (
        "xpath compile errors",
        OUT_TEXT,
        sch!(
            "<s:pattern id='p'><s:rule context='r'><s:assert test='q:b or 1'/><s:assert test='q:* or 1'/><s:assert test='1 or q:b[1]'/><s:assert test='@q:x or 1'/><s:assert test='child::q:x | 2'/><s:assert test='a[[' /><s:assert test='1 = = 2'/><s:assert test='a b'/><s:assert test='a/'/></s:rule></s:pattern>"
        ),
        "<r/>",
        "XPath error : Undefined namespace prefix: q\nq:b or 1\n   ^\nschema.sct:1: element assert: Schemas parser error : Failed to compile test expression q:b or 1\nXPath error : Undefined namespace prefix: q\nq:* or 1\n   ^\nschema.sct:1: element assert: Schemas parser error : Failed to compile test expression q:* or 1\nXPath error : Undefined namespace prefix: q\n1 or q:b[1]\n        ^\nschema.sct:1: element assert: Schemas parser error : Failed to compile test expression 1 or q:b[1]\nXPath error : Undefined namespace prefix: q\n@q:x or 1\n    ^\nschema.sct:1: element assert: Schemas parser error : Failed to compile test expression @q:x or 1\nXPath error : Undefined namespace prefix: q\nchild::q:x | 2\n          ^\nschema.sct:1: element assert: Schemas parser error : Failed to compile test expression child::q:x | 2\nXPath error : Invalid expression\na[[\n  ^\nschema.sct:1: element assert: Schemas parser error : Failed to compile test expression a[[\nXPath error : Invalid expression\n1 = = 2\n    ^\nschema.sct:1: element assert: Schemas parser error : Failed to compile test expression 1 = = 2\nXPath error : Invalid expression\na b\n  ^\nschema.sct:1: element assert: Schemas parser error : Failed to compile test expression a b\nXPath error : Invalid expression\nschema.sct:1: element assert: Schemas parser error : Failed to compile test expression a/\nSchematron schema schema.sct failed to compile\n",
    ),
    (
        "duplicate lets",
        OUT_TEXT,
        sch!(
            "<s:pattern id='a'><s:rule context='b'><s:let name='v' value='1'/><s:let name='v' value='2'/><s:report test='$v'>v=<s:value-of select='$v'/></s:report></s:rule><s:rule context='c'><s:report test='$v'>leaked <s:value-of select='$v'/></s:report></s:rule></s:pattern>"
        ),
        "<r><b/><c/></r>",
        "Pattern: a\n/r/b line 1: v=1\nSchemas validity error : Unregistering a let variable failed\nXPath error : Undefined variable: v\ndoc.xml fails to validate\n",
    ),
];

#[test]
fn table_cases_match_libxml2_output() {
    for &(name, flags, schema, instance, expected) in CASES {
        let found = run(
            schema.as_bytes(),
            "schema.sct",
            &[("doc.xml", instance.as_bytes())],
            flags,
        );
        assert_eq!(found, expected, "{name}");
    }
}

const ISO: &str = "http://purl.oclc.org/dsdl/schematron";

fn schema(body: &str) -> Schema {
    let text =
        format!("<s:schema xmlns:s='{ISO}'><s:title>T<b>itle</b></s:title>{body}</s:schema>");
    Schema::parse(text.as_bytes(), "s.sct", none(), &Limits::default()).unwrap()
}

#[test]
fn structured_diagnostics_carry_node_line_and_pattern() {
    let schema = schema(
        "<s:pattern id='p'><s:rule context='b'><s:assert test='0'>no</s:assert></s:rule></s:pattern>",
    );
    assert_eq!(schema.title(), Some("Title"));
    let source = b"<r>\n<b\n/></r>";
    let document = Document::parse_bytes(source).unwrap();
    let validation = schema.validate(&document, OUT_ERROR, source);
    assert_eq!((validation.errors, validation.is_valid()), (Some(1), false));
    let [Output::Error(diagnostic)] = validation.output.as_slice() else {
        panic!("{validation:?}");
    };
    let b = document
        .root_element()
        .first_child()
        .unwrap()
        .next_sibling()
        .unwrap();
    assert_eq!(diagnostic.code, Code::Assert);
    assert_eq!(diagnostic.code.number(), 4000);
    assert_eq!(diagnostic.node, Some(b.id()));
    assert_eq!(
        (diagnostic.line, diagnostic.element.as_deref()),
        (3, Some("b"))
    );
    assert_eq!(diagnostic.pattern.as_deref(), Some("p"));
    assert_eq!(diagnostic.message, "/r/b line 3: no\n");
    // Without a source there are no line numbers, as for constructed nodes.
    let unlined = schema.validate(&document, OUT_TEXT, &[]);
    assert_eq!(unlined.output[1], Output::Text("/r/b line 0: no\n".into()));
    assert!(
        schema
            .validate(&Document::parse("<r/>").unwrap(), OUT_TEXT, &[])
            .is_valid()
    );
}

#[test]
fn codes_have_libxml2_numbers() {
    let cases = [
        (Code::Internal, 1),
        (Code::FailedLoad, 1757),
        (Code::NoRoot, 1759),
        (Code::AttrInvalid, 1821),
        (Code::Assert, 4000),
        (Code::Report, 4001),
        (Code::XPath(yaxml::xpath::ErrorKind::UnknownFunction), 1209),
        (Code::XPath(yaxml::xpath::ErrorKind::UndefinedPrefix), 1219),
    ];
    for (code, number) in cases {
        assert_eq!(code.number(), number, "{code:?}");
    }
}

#[test]
fn schema_errors_are_structured() {
    let error = Schema::parse(b"<s:schema", "bad.sct", none(), &Limits::default()).unwrap_err();
    assert_eq!(error.uri, "bad.sct");
    assert_eq!(error.errors.len(), 1);
    assert_eq!(error.errors[0].code, Code::FailedLoad);
    assert_eq!(
        error.to_string(),
        "Schemas parser error : xmlSchematronParse: could not load 'bad.sct'."
    );
    let text = format!(
        "<s:schema xmlns:s='{ISO}'><s:pattern id='p'><s:rule context='r'><s:value-of/></s:rule></s:pattern></s:schema>"
    );
    let error = Schema::parse(text.as_bytes(), "s.sct", none(), &Limits::default()).unwrap_err();
    let codes: Vec<_> = error
        .errors
        .iter()
        .map(|e| (e.code, e.node, e.line))
        .collect();
    assert_eq!(
        codes,
        [(Code::NoRoot, Some(()), 1), (Code::NoRoot, Some(()), 1)]
    );
    assert!(
        error
            .to_string()
            .ends_with("rule has no assert nor report element")
    );
}

#[test]
fn from_tree_matches_parse_on_both_doms() {
    let cases = [
        (
            sch!(
                "<s:pattern id='p'><s:rule context='r'><s:assert test='a'>A</s:assert></s:rule></s:pattern>"
            ),
            true,
        ),
        (
            sch!("<s:pattern id='p'><s:rule><s:assert test='a'/></s:rule></s:pattern>"),
            false,
        ),
    ];
    let instance = "<r/>";
    for (text, ok) in cases {
        let parsed = Schema::parse(text.as_bytes(), "s.sct", none(), &Limits::default());
        let borrowed = Document::parse(text).unwrap();
        let owned = OwnedDocument::parse(text).unwrap();
        let from_borrowed =
            Schema::from_tree(&borrowed, "s.sct", text.as_bytes(), &Limits::default());
        let from_owned = Schema::from_tree(&owned, "s.sct", text.as_bytes(), &Limits::default());
        assert_eq!(parsed.is_ok(), ok);
        let render = |schema: Result<Schema, yaxml::schematron::SchemaErrors>| match schema {
            Ok(schema) => schema
                .validate(&Document::parse(instance).unwrap(), OUT_TEXT, b"")
                .render("d"),
            Err(error) => error.to_string(),
        };
        let expected = render(parsed);
        assert_eq!(render(from_borrowed), expected);
        assert_eq!(render(from_owned), expected);
    }
}

#[test]
fn the_resolver_loads_schema_entities() {
    let text = format!(
        "<!DOCTYPE s:schema SYSTEM 'e.dtd'><s:schema xmlns:s='{ISO}'><s:pattern id='p'><s:rule context='r'><s:assert test='a'>&e;</s:assert></s:rule></s:pattern></s:schema>"
    );
    let resolver = |request: Request<'_>| {
        Ok((request.uri == "e.dtd").then(|| b"<!ENTITY e 'loaded'>".to_vec()))
    };
    let schema = Schema::parse(
        text.as_bytes(),
        "s.sct",
        Box::new(resolver),
        &Limits::default(),
    )
    .unwrap();
    let validation = schema.validate(&Document::parse("<r/>").unwrap(), OUT_QUIET, b"<r/>");
    assert_eq!(
        validation.output,
        [Output::Text("/r line 1: loaded\n".into())]
    );
    let missing = Schema::parse(text.as_bytes(), "s.sct", none(), &Limits::default());
    let missing = missing
        .unwrap()
        .validate(&Document::parse("<r/>").unwrap(), OUT_QUIET, b"<r/>");
    assert_eq!(
        missing.output,
        [Output::Text("/r line 1: node failed assert\n".into())]
    );
}

#[test]
fn operation_budget_stops_validation() {
    let body = "<s:pattern id='p'><s:rule context='*'><s:assert test='count(//*) > 0'>x</s:assert></s:rule></s:pattern>";
    let text = format!("<s:schema xmlns:s='{ISO}'>{body}</s:schema>");
    let instance = Document::parse("<r><a/><b/><c/></r>").unwrap();
    let cases: [(u64, Option<usize>); 3] = [(1_000, Some(0)), (30, None), (0, None)];
    for (max_operations, errors) in cases {
        let limits = Limits {
            max_operations,
            ..Limits::default()
        };
        let schema = Schema::parse(text.as_bytes(), "s.sct", none(), &limits).unwrap();
        let validation: Validation<_> = schema.validate(&instance, OUT_TEXT, b"");
        assert_eq!(validation.errors, errors, "{max_operations}");
        if errors.is_none() {
            let Some(Output::Error(last)) = validation.output.last() else {
                panic!()
            };
            assert_eq!(last.message, "Operation limit exceeded\n");
        }
    }
    // The per-expression limit fails one test without stopping validation.
    let mut limits = Limits::default();
    limits.xpath.max_operations = 3;
    let schema = Schema::parse(text.as_bytes(), "s.sct", none(), &limits).unwrap();
    assert_eq!(schema.validate(&instance, OUT_TEXT, b"").errors, Some(4));
}

/// Runs the libxml2 helper in `directory`.
fn oracle(helper: &str, directory: &Path, flags: u32, schema: &str, instances: &[&str]) -> String {
    let output = Command::new(helper)
        .current_dir(directory)
        .arg(flags.to_string())
        .arg(schema)
        .args(instances)
        .output()
        .unwrap();
    String::from_utf8(output.stderr).unwrap()
}

#[test]
#[ignore = "requires YAXML_SCHEMATRON_HELPER pointing at the compiled tooling/libxml2_schematron.c"]
fn libxml2_agrees_in_every_mode() {
    let helper = std::env::var("YAXML_SCHEMATRON_HELPER").unwrap();
    let (mut total, mut differences) = (0, Vec::new());
    let directory = Path::new(FIXTURES);
    for (schema, instances) in corpus() {
        for flags in MODES {
            let files: Vec<String> = instances
                .iter()
                .map(|(name, _)| format!("test/{name}"))
                .collect();
            let files: Vec<&str> = files.iter().map(String::as_str).collect();
            let expected = oracle(&helper, directory, flags, &format!("test/{schema}"), &files);
            let bytes: Vec<Vec<u8>> = instances.iter().map(|(name, _)| read(name)).collect();
            let inputs: Vec<(&str, &[u8])> = files
                .iter()
                .copied()
                .zip(bytes.iter().map(Vec::as_slice))
                .collect();
            let found = run(&read(&schema), &format!("test/{schema}"), &inputs, flags);
            total += 1;
            if found != expected {
                differences.push(format!(
                    "{schema} flags {flags}:\n--- libxml2\n{expected}--- yaxml\n{found}"
                ));
            }
        }
    }
    let scratch = std::env::temp_dir().join(format!("yaxml-schematron-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    for &(name, flags, schema, instance, expected) in CASES {
        std::fs::write(scratch.join("schema.sct"), schema).unwrap();
        std::fs::write(scratch.join("doc.xml"), instance).unwrap();
        let oracle = oracle(&helper, &scratch, flags, "schema.sct", &["doc.xml"]);
        total += 1;
        if oracle != expected {
            differences.push(format!(
                "{name}:\n--- libxml2\n{oracle}--- expected\n{expected}"
            ));
        }
    }
    println!("{} of {total} runs agree", total - differences.len());
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn documents_without_an_element() {
    let empty = yaxml::html::parse(b"<!--c-->", Default::default())
        .unwrap()
        .document;
    assert!(
        empty
            .root()
            .first_child()
            .is_none_or(|child| child.name().is_none())
    );
    let error = Schema::from_tree(&empty, "s.sct", b"", &Limits::default()).unwrap_err();
    assert_eq!(error.errors[0].code, Code::NoRoot);
    assert_eq!(
        error.to_string(),
        "Schemas parser error : The schema has no document element."
    );
    let schema =
        schema("<s:pattern id='p'><s:rule context='r'><s:assert test='a'/></s:rule></s:pattern>");
    let validation = schema.validate(&empty, OUT_TEXT, b"");
    assert_eq!((validation.errors, validation.output.len()), (Some(1), 0));
}
