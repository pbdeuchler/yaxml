#![forbid(unsafe_code)]

use yaxml::xpath::{Context, Error, ErrorKind, Limit, Limits, Node, Value, XPath};
use yaxml::{
    Document, EditLimits, ElementSpec, NameSpec, NodeId, NodeKind, OwnedDocument, OwnedNodeId,
    OwnedNodeKind, PrefixPolicy,
};

const XML: &str = r#"<?top t?><!--c0--><r xmlns:p="urn:p" xml:lang="en-GB"><a id="1" xml:id="x1" p:q="v">one</a><a id="2">two<![CDATA[!]]></a><p:b id="3"><c/>text<?pi data?><!--note--></p:b><d xmlns="urn:d" xml:lang="de"><e xmlns=""/></d><n>4</n><n>-1.5</n></r>"#;

/// A DOM-independent projection: nodes are rendered by kind and name.
#[derive(Debug, PartialEq)]
enum Out {
    Nodes(Vec<String>),
    Bool(bool),
    Num(f64),
    Str(String),
}

fn borrowed_label(document: &Document<'_>, node: Node<NodeId>) -> String {
    let label = |id| -> String {
        let node = document.node(id).expect("live node");
        match node.kind() {
            NodeKind::Document => "/".into(),
            NodeKind::Doctype(_) => unreachable!("XPath never selects a DOCTYPE"),
            NodeKind::Element(element) => element.name.qualified().into(),
            NodeKind::Text(text) | NodeKind::Cdata(text) => format!("text({text})"),
            NodeKind::Comment(text) => format!("comment({text})"),
            NodeKind::ProcessingInstruction { target, .. } => format!("pi({target})"),
        }
    };
    match node {
        Node::Tree(id) => label(id),
        Node::Attribute(id, index) => {
            let attribute = &document.node(id).expect("owner").attributes()[index];
            format!("{}@{}", label(id), attribute.name.qualified())
        }
        Node::Namespace(id, index) => format!("{}#{index}", label(id)),
    }
}

fn owned_label(document: &OwnedDocument, node: Node<OwnedNodeId>) -> String {
    let label = |id| -> String {
        let node = document.node(id).expect("live node");
        let text = node.text().unwrap_or_default();
        match node.kind() {
            OwnedNodeKind::Element => node.name().expect("name").qualified().into(),
            OwnedNodeKind::Text | OwnedNodeKind::Cdata => format!("text({text})"),
            OwnedNodeKind::Comment => format!("comment({text})"),
            OwnedNodeKind::ProcessingInstruction => {
                format!(
                    "pi({})",
                    node.processing_instruction_target().unwrap_or_default()
                )
            }
            _ => "/".into(),
        }
    };
    match node {
        Node::Tree(id) => label(id),
        Node::Attribute(id, index) => {
            let attribute = document
                .node(id)
                .expect("owner")
                .attributes()
                .nth(index)
                .expect("attribute");
            format!("{}@{}", label(id), attribute.name.qualified())
        }
        Node::Namespace(id, index) => format!("{}#{index}", label(id)),
    }
}

fn project<Id>(value: Value<Id>, label: impl Fn(Node<Id>) -> String) -> Out {
    match value {
        Value::NodeSet(nodes) => Out::Nodes(nodes.into_iter().map(label).collect()),
        Value::Boolean(value) => Out::Bool(value),
        Value::Number(value) => Out::Num(value),
        Value::String(value) => Out::Str(value),
    }
}

/// Evaluates at the root of both DOMs, requiring identical projections.
fn run_with(
    xml: &str,
    expression: &str,
    borrowed: &Context<NodeId>,
    owned: &Context<OwnedNodeId>,
) -> Result<Out, ErrorKind> {
    let document = Document::parse(xml).expect("borrowed parse");
    let owned_document = OwnedDocument::parse(xml).expect("owned parse");
    let xpath = XPath::compile(expression).map_err(|error| error.kind)?;
    let left = xpath
        .evaluate(&document, document.root().id(), borrowed)
        .map(|value| project(value, |node| borrowed_label(&document, node)));
    let right = xpath
        .evaluate(&owned_document, owned_document.root().id(), owned)
        .map(|value| project(value, |node| owned_label(&owned_document, node)));
    assert_eq!(
        format!("{left:?}"),
        format!("{right:?}"),
        "DOM disagreement for {expression}"
    );
    left.map_err(|error| error.kind)
}

fn bound<Id>() -> Context<Id> {
    Context::new()
        .with_namespace("p", "urn:p")
        .with_namespace("dd", "urn:d")
}

fn run(expression: &str) -> Result<Out, ErrorKind> {
    run_with(XML, expression, &bound(), &bound())
}

fn nodes(labels: &[&str]) -> Result<Out, ErrorKind> {
    Ok(Out::Nodes(
        labels.iter().map(|label| (*label).to_owned()).collect(),
    ))
}

fn num(value: f64) -> Result<Out, ErrorKind> {
    Ok(Out::Num(value))
}

fn string(value: &str) -> Result<Out, ErrorKind> {
    Ok(Out::Str(value.to_owned()))
}

fn boolean(value: bool) -> Result<Out, ErrorKind> {
    Ok(Out::Bool(value))
}

fn check(cases: &[(&str, Result<Out, ErrorKind>)]) {
    for (expression, expected) in cases {
        let actual = run(expression);
        let same = match (&actual, expected) {
            (Ok(Out::Num(a)), Ok(Out::Num(b))) => {
                a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
            }
            _ => actual == *expected,
        };
        assert!(same, "{expression}: got {actual:?}, expected {expected:?}");
    }
}

#[test]
fn axes_follow_direction_and_document_order() {
    check(&[
        ("/r/child::*", nodes(&["a", "a", "p:b", "d", "n", "n"])),
        (
            "/r/a/child::text()",
            nodes(&["text(one)", "text(two)", "text(!)"]),
        ),
        ("/r/child::nothing", nodes(&[])),
        (
            "/r/p:b/descendant::node()",
            nodes(&["c", "text(text)", "pi(pi)", "comment(note)"]),
        ),
        ("/r/p:b/descendant-or-self::*", nodes(&["p:b", "c"])),
        ("/r/p:b/c/parent::*", nodes(&["p:b"])),
        ("/parent::node()", nodes(&[])),
        ("//c/ancestor::*", nodes(&["r", "p:b"])),
        ("//c/ancestor::node()[1]", nodes(&["p:b"])),
        ("//c/ancestor::node()[last()]", nodes(&["/"])),
        ("//c/ancestor-or-self::*[1]", nodes(&["c"])),
        ("//c/ancestor-or-self::*", nodes(&["r", "p:b", "c"])),
        (
            "/r/a[1]/following-sibling::*",
            nodes(&["a", "p:b", "d", "n", "n"]),
        ),
        ("/r/a[1]/following-sibling::*[2]", nodes(&["p:b"])),
        (
            "/r/n[1]/preceding-sibling::*",
            nodes(&["a", "a", "p:b", "d"]),
        ),
        ("/r/n[1]/preceding-sibling::*[1]", nodes(&["d"])),
        ("/r/n[1]/preceding-sibling::*[last()]", nodes(&["a"])),
        ("/r/following-sibling::node()", nodes(&[])),
        ("//c/following::*", nodes(&["d", "e", "n", "n"])),
        ("//c/following::node()[1]", nodes(&["text(text)"])),
        ("//c/preceding::*", nodes(&["a", "a"])),
        ("//c/preceding::node()[1]", nodes(&["text(!)"])),
        ("//c/preceding::node()[last()]", nodes(&["pi(top)"])),
        (
            "/r/a[1]/attribute::*",
            nodes(&["a@id", "a@xml:id", "a@p:q"]),
        ),
        ("/r/attribute::*", nodes(&["r@xml:lang"])),
        ("/r/a[1]/@p:q", nodes(&["a@p:q"])),
        ("/r/a[1]/@xml:*", nodes(&["a@xml:id"])),
        ("/r/a[1]/@node()[2]", nodes(&["a@xml:id"])),
        ("/r/a[1]/@text()", nodes(&[])),
        ("//c/@*", nodes(&[])),
        ("/r/namespace::*", nodes(&["r#0", "r#1"])),
        ("//e/namespace::node()", nodes(&["e#0", "e#1", "e#2"])),
        ("//e/namespace::*[string() = '']", nodes(&["e#2"])),
        ("//d/namespace::*[string() = '']", nodes(&[])),
        ("//dd:d/namespace::*", nodes(&["d#0", "d#1", "d#2"])),
        ("//dd:d/namespace::p", nodes(&["d#1"])),
        ("//dd:d/namespace::p:x", nodes(&[])),
        ("/namespace::*", nodes(&[])),
        ("/r/self::r", nodes(&["r"])),
        ("/r/self::a", nodes(&[])),
        ("/self::node()", nodes(&["/"])),
        // Axes from attribute and namespace nodes.
        ("/r/a[1]/@id/parent::a", nodes(&["a"])),
        ("/r/a[1]/@id/ancestor::*", nodes(&["r", "a"])),
        (
            "/r/a[1]/@id/ancestor-or-self::node()",
            nodes(&["/", "r", "a", "a@id"]),
        ),
        ("/r/a[1]/@id/self::node()", nodes(&["a@id"])),
        ("/r/a[1]/@id/self::*", nodes(&[])),
        ("/r/a[1]/@id/child::node()", nodes(&[])),
        ("/r/a[1]/@id/descendant-or-self::node()", nodes(&["a@id"])),
        ("/r/a[1]/@id/following::node()[1]", nodes(&["text(one)"])),
        ("/r/a[2]/@id/preceding::*", nodes(&["a"])),
        ("/r/a[1]/@id/following-sibling::node()", nodes(&[])),
        ("/r/a[1]/@id/preceding-sibling::node()", nodes(&[])),
        ("/r/a[1]/@id/attribute::*", nodes(&[])),
        ("//dd:d/namespace::*/parent::node()", nodes(&["d"])),
        ("//dd:d/namespace::*/..", nodes(&["d"])),
        ("//dd:d/namespace::*[1]/self::node()", nodes(&["d#0"])),
        // Abbreviations.
        ("//a/..", nodes(&["r"])),
        ("//a/.", nodes(&["a", "a"])),
        ("/r//c", nodes(&["c"])),
        ("//a[1]", nodes(&["a"])),
        ("(//a)[2]/text()[1]", nodes(&["text(two)"])),
        ("//p:b//text()", nodes(&["text(text)"])),
        ("/", nodes(&["/"])),
        ("/r/..", nodes(&["/"])),
    ]);
}

#[test]
fn node_tests_cover_names_wildcards_and_types() {
    check(&[
        (
            "//*",
            nodes(&["r", "a", "a", "p:b", "c", "d", "e", "n", "n"]),
        ),
        ("//p:*", nodes(&["p:b"])),
        ("//p:b", nodes(&["p:b"])),
        ("//b", nodes(&[])),
        ("//d", nodes(&[])),
        ("//dd:d", nodes(&["d"])),
        ("//e", nodes(&["e"])),
        (
            "//node()[self::comment()]",
            nodes(&["comment(c0)", "comment(note)"]),
        ),
        ("/node()", nodes(&["pi(top)", "comment(c0)", "r"])),
        ("//comment()", nodes(&["comment(c0)", "comment(note)"])),
        ("//processing-instruction()", nodes(&["pi(top)", "pi(pi)"])),
        ("//processing-instruction('pi')", nodes(&["pi(pi)"])),
        ("//processing-instruction(\"top\")", nodes(&["pi(top)"])),
        ("//processing-instruction('none')", nodes(&[])),
        ("//a/text()", nodes(&["text(one)", "text(two)", "text(!)"])),
        ("count(//node())", num(19.0)),
        ("//q:*", Err(ErrorKind::UndefinedPrefix)),
        ("//q:x", Err(ErrorKind::UndefinedPrefix)),
    ]);
}

#[test]
fn predicates_unions_and_filters() {
    check(&[
        ("//a[@id = 2]", nodes(&["a"])),
        ("//a[2]", nodes(&["a"])),
        ("//*[@id][position() > 1]", nodes(&["a", "p:b"])),
        ("//*[@id][last()]/@id", nodes(&["p:b@id"])),
        ("//*[@id][1.5]", nodes(&[])),
        ("//a['x']", nodes(&["a", "a"])),
        ("//a['']", nodes(&[])),
        ("//a[0 div 0]", nodes(&[])),
        ("//n | //a", nodes(&["a", "a", "n", "n"])),
        ("//a | //a | /r", nodes(&["r", "a", "a"])),
        ("(//n | //a)[1]", nodes(&["a"])),
        ("(//a/@id | //a)[last()]", nodes(&["a@id"])),
        (
            "//a/@* | //a",
            nodes(&["a", "a@id", "a@xml:id", "a@p:q", "a", "a@id"]),
        ),
        (
            "//dd:d/namespace::* | //dd:d",
            nodes(&["d", "d#0", "d#1", "d#2"]),
        ),
        ("(//a)[1]/../n", nodes(&["n", "n"])),
        ("(//n)[2]", nodes(&["n"])),
        ("(//n)[position() = last()] = -1.5", boolean(true)),
        ("1 | //a", Err(ErrorKind::InvalidType)),
        ("//a | 'x'", Err(ErrorKind::InvalidType)),
        ("'x'[1]", Err(ErrorKind::InvalidType)),
        ("'x'/a", Err(ErrorKind::InvalidType)),
        ("$v/a", Err(ErrorKind::UndefinedVariable)),
        ("-//n[1]", num(-4.0)),
        ("-(//n)[2]", num(1.5)),
    ]);
}

#[test]
fn operators_follow_xpath_semantics() {
    let nan = f64::NAN;
    check(&[
        ("1 + 2", num(3.0)),
        ("5 - 7", num(-2.0)),
        ("3 * 4", num(12.0)),
        ("7 div 2", num(3.5)),
        ("7 mod 3", num(1.0)),
        ("-7 mod 3", num(-1.0)),
        ("7 mod -3", num(1.0)),
        ("5.5 mod 2", num(1.5)),
        ("1 div 0", num(f64::INFINITY)),
        ("-1 div 0", num(f64::NEG_INFINITY)),
        ("0 div 0", num(nan)),
        ("1 mod 0", num(nan)),
        ("--1", num(1.0)),
        ("1 - - 1", num(2.0)),
        ("1-1", num(0.0)),
        ("'3' + true()", num(4.0)),
        ("'x' + 1", num(nan)),
        ("//n[1] * 2", num(8.0)),
        ("2 + 3 * 4 - 6 div 2", num(11.0)),
        ("(2 + 3) * 4", num(20.0)),
        ("1 < 2 < 3", boolean(true)),
        ("3 > 2 > 1", boolean(false)),
        ("1 = 1 = 1", boolean(true)),
        ("true() or false() and false()", boolean(true)),
        ("false() and $missing", boolean(false)),
        ("true() or $missing", boolean(true)),
        ("true() and $missing", Err(ErrorKind::UndefinedVariable)),
        // Scalar comparisons.
        ("1 = 1.0", boolean(true)),
        ("'1' = 1", boolean(true)),
        ("'1.0' = '1'", boolean(false)),
        ("true() = 1", boolean(true)),
        ("false() = ''", boolean(true)),
        ("true() = 'x'", boolean(true)),
        ("0 div 0 = 0 div 0", boolean(false)),
        ("0 div 0 != 0 div 0", boolean(true)),
        ("'b' > 'a'", boolean(false)),
        ("'2' > '10'", boolean(false)),
        ("true() > false()", boolean(true)),
        ("1 <= 1", boolean(true)),
        ("2 >= 3", boolean(false)),
        // Node-set comparisons.
        ("//n = 4", boolean(true)),
        ("//n != 4", boolean(true)),
        ("//n = 5", boolean(false)),
        ("//n < -1", boolean(true)),
        ("//n > 3.5", boolean(true)),
        ("//n >= 5", boolean(false)),
        ("//n <= -1.5", boolean(true)),
        ("4 = //n", boolean(true)),
        ("-2 < //n", boolean(true)),
        ("5 > //n", boolean(true)),
        ("//a = 'one'", boolean(true)),
        ("'two!' = //a", boolean(true)),
        ("//a != 'one'", boolean(true)),
        ("//a < '5'", boolean(false)),
        ("//a = //n", boolean(false)),
        ("//a = //a", boolean(true)),
        ("//a != //a", boolean(true)),
        ("/r/a[1] != /r/a[1]", boolean(false)),
        ("//n < //n", boolean(true)),
        ("//n > //n", boolean(true)),
        ("//n <= //n", boolean(true)),
        ("//n >= //a", boolean(false)),
        ("//none = //none", boolean(false)),
        ("//none != //n", boolean(false)),
        ("//none < //n", boolean(false)),
        ("//none = false()", boolean(true)),
        ("//n = true()", boolean(true)),
        ("true() = //none", boolean(false)),
        ("//n > false()", boolean(true)),
        ("//none = ''", boolean(false)),
        ("//none != 0", boolean(false)),
    ]);
}

#[test]
fn conversions_match_libxml2() {
    let nan = f64::NAN;
    check(&[
        ("string(1)", string("1")),
        ("string(-1)", string("-1")),
        ("string(-0)", string("0")),
        ("string(0.5)", string("0.5")),
        ("string(1 div 3)", string("0.333333333333333")),
        ("string(2 div 3)", string("0.666666666666667")),
        ("string(0.1 + 0.2)", string("0.3")),
        ("string(123.456)", string("123.456")),
        ("string(1000000000)", string("1000000000")),
        ("string(2147483647)", string("2.147483647e+09")),
        ("string(-2147483647)", string("-2147483647")),
        ("string(1e10)", string("1e+10")),
        ("string(1.5e-7)", string("1.5e-07")),
        ("string(0.00001)", string("0.00001")),
        ("string(0.000001)", string("1e-06")),
        ("string(0.0001)", string("0.0001")),
        ("string(1e300 * 1e10)", string("Infinity")),
        ("string(-1 div 0)", string("-Infinity")),
        ("string(0 div 0)", string("NaN")),
        ("string(true())", string("true")),
        ("string(false())", string("false")),
        ("string(//a)", string("one")),
        ("string(//none)", string("")),
        ("string(/)", string("onetwo!text4-1.5")),
        ("string(//a/@p:q)", string("v")),
        ("string(//dd:d/namespace::*[last()])", string("urn:d")),
        ("string(//pi())", Err(ErrorKind::Unclosed)),
        ("string(//processing-instruction())", string("t")),
        ("string(//comment())", string("c0")),
        ("number('12')", num(12.0)),
        ("number(' \t\n12.5\r ')", num(12.5)),
        ("number('-.5')", num(-0.5)),
        ("number('1.')", num(1.0)),
        ("number('1e2')", num(100.0)),
        ("number('1E-2')", num(0.01)),
        ("number('')", num(nan)),
        ("number('.')", num(nan)),
        ("number('-')", num(nan)),
        ("number('+1')", num(nan)),
        ("number('1 2')", num(nan)),
        ("number('0x10')", num(nan)),
        ("number(true())", num(1.0)),
        ("number(false())", num(0.0)),
        ("number(//n)", num(4.0)),
        ("number(//none)", num(nan)),
        ("number(//a)", num(nan)),
        ("boolean(1)", boolean(true)),
        ("boolean(0)", boolean(false)),
        ("boolean(-0)", boolean(false)),
        ("boolean(0 div 0)", boolean(false)),
        ("boolean('false')", boolean(true)),
        ("boolean('')", boolean(false)),
        ("boolean(//a)", boolean(true)),
        ("boolean(//none)", boolean(false)),
        ("0.5", num(0.5)),
        (".5", num(0.5)),
        ("5.", num(5.0)),
        ("1e3", num(1000.0)),
        ("'lit'", string("lit")),
        ("\"dq'\"", string("dq'")),
    ]);
}

#[test]
fn core_functions_happy_and_sad_paths() {
    let nan = f64::NAN;
    check(&[
        ("last()", num(1.0)),
        ("position()", num(1.0)),
        ("//a[last()]/@id = 2", boolean(true)),
        ("//a[position() = 1]/@id = 1", boolean(true)),
        ("last(1)", Err(ErrorKind::InvalidArity)),
        ("position(1)", Err(ErrorKind::InvalidArity)),
        ("count(//a)", num(2.0)),
        ("count(//none)", num(0.0)),
        ("count()", Err(ErrorKind::InvalidArity)),
        ("count(1)", Err(ErrorKind::InvalidType)),
        ("id('x1')", nodes(&["a"])),
        ("id(' x1  x1 missing ')", nodes(&["a"])),
        ("id(//a/@xml:id)", nodes(&["a"])),
        ("id('')", nodes(&[])),
        ("id(1)", nodes(&[])),
        ("id()", Err(ErrorKind::InvalidArity)),
        ("local-name(//p:b)", string("b")),
        ("local-name(//a/@p:q)", string("q")),
        ("local-name(//processing-instruction())", string("top")),
        ("local-name(//dd:d/namespace::*[2])", string("p")),
        ("local-name(//dd:d/namespace::*[3])", string("")),
        ("local-name(//text())", string("")),
        ("local-name(//none)", string("")),
        ("local-name()", string("")),
        ("local-name(1)", Err(ErrorKind::InvalidType)),
        ("local-name(1, 2)", Err(ErrorKind::InvalidArity)),
        ("namespace-uri(//p:b)", string("urn:p")),
        ("namespace-uri(//*[local-name() = 'd'])", string("urn:d")),
        ("namespace-uri(//e)", string("")),
        (
            "namespace-uri(//a/@xml:id)",
            string("http://www.w3.org/XML/1998/namespace"),
        ),
        ("namespace-uri(//dd:d/namespace::*[2])", string("")),
        ("namespace-uri('x')", Err(ErrorKind::InvalidType)),
        ("name(//p:b)", string("p:b")),
        ("name(//a/@p:q)", string("p:q")),
        ("name(//dd:d/namespace::*[1])", string("xml")),
        ("name(/)", string("")),
        ("name(//comment())", string("")),
        ("name(true())", Err(ErrorKind::InvalidType)),
        ("string()", string("onetwo!text4-1.5")),
        ("string(1, 2)", Err(ErrorKind::InvalidArity)),
        ("concat('a', 'b')", string("ab")),
        ("concat('a', 1, true(), //n)", string("a1true4")),
        ("concat('a')", Err(ErrorKind::InvalidArity)),
        ("starts-with('abc', 'ab')", boolean(true)),
        ("starts-with('abc', 'bc')", boolean(false)),
        ("starts-with('abc', '')", boolean(true)),
        ("starts-with('abc')", Err(ErrorKind::InvalidArity)),
        ("contains('abc', 'b')", boolean(true)),
        ("contains('abc', 'd')", boolean(false)),
        ("contains('abc', 'b', 'c')", Err(ErrorKind::InvalidArity)),
        ("substring-before('1999/04/01', '/')", string("1999")),
        ("substring-before('abc', 'x')", string("")),
        ("substring-before('abc', '')", string("")),
        ("substring-after('1999/04/01', '/')", string("04/01")),
        ("substring-after('abc', 'x')", string("")),
        ("substring-after('abc', '')", string("abc")),
        ("substring-after('abc')", Err(ErrorKind::InvalidArity)),
        ("substring('12345', 2, 3)", string("234")),
        ("substring('12345', 2)", string("2345")),
        ("substring('12345', 1.5, 2.6)", string("234")),
        ("substring('12345', 0, 3)", string("12")),
        ("substring('12345', 0 div 0, 3)", string("")),
        ("substring('12345', 1, 0 div 0)", string("")),
        ("substring('12345', -42, 1 div 0)", string("12345")),
        ("substring('12345', -1 div 0, 1 div 0)", string("")),
        ("substring('12345', 6)", string("")),
        ("substring('héllo', 2, 2)", string("él")),
        ("substring('12345')", Err(ErrorKind::InvalidArity)),
        ("string-length('héllo')", num(5.0)),
        ("string-length('')", num(0.0)),
        ("string-length()", num(16.0)),
        ("//a[string-length() = 3]", nodes(&["a"])),
        ("string-length(1, 2)", Err(ErrorKind::InvalidArity)),
        ("normalize-space('  a \t b\n\r c  ')", string("a b c")),
        ("normalize-space('')", string("")),
        ("//a[normalize-space() = 'one']", nodes(&["a"])),
        ("normalize-space(1, 2)", Err(ErrorKind::InvalidArity)),
        ("translate('bar', 'abc', 'ABC')", string("BAr")),
        ("translate('--aaa--', 'abc-', 'ABC')", string("AAA")),
        ("translate('aba', 'aa', 'xy')", string("xbx")),
        ("translate('é', 'é', 'e')", string("e")),
        ("translate('a', 'b')", Err(ErrorKind::InvalidArity)),
        ("boolean('x')", boolean(true)),
        ("boolean()", Err(ErrorKind::InvalidArity)),
        ("not(1)", boolean(false)),
        ("not(//none)", boolean(true)),
        ("not()", Err(ErrorKind::InvalidArity)),
        ("true()", boolean(true)),
        ("true(1)", Err(ErrorKind::InvalidArity)),
        ("false()", boolean(false)),
        ("false(1)", Err(ErrorKind::InvalidArity)),
        ("//a[lang('en')]", nodes(&["a", "a"])),
        ("//a[lang('EN-gb')]", nodes(&["a", "a"])),
        ("//a[lang('en-US')]", nodes(&[])),
        ("//a[lang('e')]", nodes(&[])),
        ("//e[lang('de')]", nodes(&["e"])),
        ("//a/@id[lang('en')]", nodes(&["a@id", "a@id"])),
        (
            "//dd:d/namespace::*[lang('de')]",
            nodes(&["d#0", "d#1", "d#2"]),
        ),
        ("(//text()[lang('en')])[1]", nodes(&["text(one)"])),
        ("lang('en')", boolean(false)),
        ("lang()", Err(ErrorKind::InvalidArity)),
        ("number()", num(nan)),
        ("//n[number() < 0]", nodes(&["n"])),
        ("number(1, 2)", Err(ErrorKind::InvalidArity)),
        ("sum(//n)", num(2.5)),
        ("sum(//none)", num(0.0)),
        ("sum(//a)", num(nan)),
        ("sum(1)", Err(ErrorKind::InvalidType)),
        ("sum()", Err(ErrorKind::InvalidArity)),
        ("floor(2.7)", num(2.0)),
        ("floor(-2.5)", num(-3.0)),
        ("floor()", Err(ErrorKind::InvalidArity)),
        ("ceiling(2.1)", num(3.0)),
        ("ceiling(-2.5)", num(-2.0)),
        ("ceiling()", Err(ErrorKind::InvalidArity)),
        ("round(2.5)", num(3.0)),
        ("round(-2.5)", num(-2.0)),
        ("round(-0.2)", num(-0.0)),
        ("round(0.2)", num(0.0)),
        ("round(1 div 0)", num(f64::INFINITY)),
        ("round(0 div 0)", num(nan)),
        ("round()", Err(ErrorKind::InvalidArity)),
        ("p:f()", Err(ErrorKind::UnknownFunction)),
        ("q:f()", Err(ErrorKind::UndefinedPrefix)),
        ("f()", Err(ErrorKind::UnknownFunction)),
    ]);
}

#[test]
fn syntax_errors_report_kind_and_offset() {
    let cases: &[(&str, ErrorKind, usize)] = &[
        ("", ErrorKind::Expression, 0),
        ("   ", ErrorKind::Expression, 3),
        ("//", ErrorKind::Expression, 2),
        ("1 +", ErrorKind::Expression, 3),
        ("1 2", ErrorKind::Expression, 2),
        ("'open", ErrorKind::UnfinishedLiteral, 0),
        ("\"open", ErrorKind::UnfinishedLiteral, 0),
        ("$", ErrorKind::VariableRef, 0),
        ("$1", ErrorKind::VariableRef, 0),
        ("$p:", ErrorKind::VariableRef, 0),
        ("//a[1", ErrorKind::InvalidPredicate, 5),
        ("//a[1)", ErrorKind::InvalidPredicate, 5),
        ("(1", ErrorKind::Unclosed, 2),
        ("count(//a", ErrorKind::Unclosed, 9),
        ("count(//a, ", ErrorKind::Expression, 11),
        ("node(", ErrorKind::Unclosed, 5),
        ("text(1)", ErrorKind::Unclosed, 5),
        ("processing-instruction(1)", ErrorKind::StartLiteral, 23),
        ("count(1, 2)", ErrorKind::InvalidArity, 0),
        ("1 + count()", ErrorKind::InvalidArity, 4),
        ("#", ErrorKind::InvalidChar, 0),
        ("a ! b", ErrorKind::InvalidChar, 2),
        ("a:", ErrorKind::InvalidChar, 0),
        ("bogus::a", ErrorKind::Expression, 0),
        ("child::", ErrorKind::Expression, 7),
        ("@", ErrorKind::Expression, 1),
        ("a::b", ErrorKind::Expression, 0),
        (".[1]", ErrorKind::Expression, 1),
        ("..[1]", ErrorKind::Expression, 2),
        ("a b", ErrorKind::Expression, 2),
        ("1 and", ErrorKind::Expression, 5),
        ("p:*:x", ErrorKind::InvalidChar, 3),
        (")", ErrorKind::Expression, 0),
        ("1 |", ErrorKind::Expression, 3),
    ];
    for &(expression, kind, offset) in cases {
        assert_eq!(
            XPath::compile(expression).map(|_| ()),
            Err(Error {
                kind,
                offset: Some(offset)
            }),
            "{expression}"
        );
    }
}

#[test]
fn lexer_disambiguates_operators_and_names() {
    let xml = "<and><or>1</or><div>2</div><mod>3</mod><x-y>4</x-y><x>9</x><y>5</y></and>";
    let cases = [
        ("/and/or + /and/div", num(3.0)),
        ("/and/mod * 2", num(6.0)),
        ("/and/div div /and/div", num(1.0)),
        ("/and/mod mod 2", num(1.0)),
        ("count(/and/*)", num(6.0)),
        ("/and/x-y", nodes(&["x-y"])),
        ("/and/x -/and/y", num(4.0)),
        ("/and/or and /and/div", boolean(true)),
        ("/and/or or false()", boolean(true)),
        ("2*3", num(6.0)),
        ("count(*)", num(1.0)),
        ("count(*/*)*2", num(12.0)),
        ("child::and/child::or", nodes(&["or"])),
        ("and", nodes(&["and"])),
    ];
    for (expression, expected) in cases {
        assert_eq!(
            run_with(xml, expression, &Context::new(), &Context::new()),
            expected,
            "{expression}"
        );
    }
}

#[test]
fn context_variables_namespaces_and_functions() {
    let document = Document::parse(XML).unwrap();
    let large = format!("<r>{}</r>", "<x/>".repeat(40));
    let large = Document::parse(&large).unwrap();
    let foreign = large.nodes().last().unwrap().id();
    let a = XPath::compile("/r/a[1]")
        .unwrap()
        .select_nodes(&document, document.root().id(), &Context::new())
        .unwrap();
    let both = XPath::compile("//a")
        .unwrap()
        .select_nodes(&document, document.root().id(), &Context::new())
        .unwrap();
    let context = Context::new()
        .with_namespace("q", "urn:p")
        .with_namespace("ext", "urn:ext")
        .with_variable("n", Value::Number(2.0))
        .with_variable("s", Value::String("two!".into()))
        .with_variable("b", Value::Boolean(true))
        .with_variable("set", Value::NodeSet(vec![both[1], both[0], both[1]]))
        .with_variable_ns("urn:ext", "n", Value::Number(5.0))
        .with_function(
            "double",
            Box::new(|args: &[Value<NodeId>]| match args {
                [Value::Number(n)] => Ok(Value::Number(n * 2.0)),
                _ => Err(Error::new(ErrorKind::InvalidArity)),
            }),
        )
        .with_function_ns(
            "urn:ext",
            "first",
            Box::new(move |_: &[Value<NodeId>]| Ok(Value::NodeSet(a.clone()))),
        )
        .with_function(
            "bad",
            Box::new(move |_: &[Value<NodeId>]| Ok(Value::NodeSet(vec![Node::Tree(foreign)]))),
        )
        .with_function(
            "count",
            Box::new(|_: &[Value<NodeId>]| Ok(Value::Number(-1.0))),
        );
    let evaluate = |expression: &str| {
        XPath::compile(expression)
            .and_then(|xpath| xpath.evaluate(&document, document.root().id(), &context))
            .map_err(|error| error.kind)
    };
    let cases: &[(&str, Result<Value<NodeId>, ErrorKind>)] = &[
        ("$n + 1", Ok(Value::Number(3.0))),
        ("$s", Ok(Value::String("two!".into()))),
        ("$b", Ok(Value::Boolean(true))),
        ("$ext:n", Ok(Value::Number(5.0))),
        ("$set", Ok(Value::NodeSet(both.clone()))),
        ("count($set)", Ok(Value::Number(2.0))),
        ("$set[2] = $s", Ok(Value::Boolean(true))),
        ("$set/@id = 2", Ok(Value::Boolean(true))),
        ("//a[@id = $n]/@id = 2", Ok(Value::Boolean(true))),
        ("$q:n", Err(ErrorKind::UndefinedVariable)),
        ("$missing", Err(ErrorKind::UndefinedVariable)),
        ("$nope:n", Err(ErrorKind::UndefinedPrefix)),
        ("count(//q:b)", Ok(Value::Number(1.0))),
        ("count(//q:*)", Ok(Value::Number(1.0))),
        ("count(//@q:q)", Ok(Value::Number(1.0))),
        ("double(4)", Ok(Value::Number(8.0))),
        ("double()", Err(ErrorKind::InvalidArity)),
        ("ext:first()/@id = 1", Ok(Value::Boolean(true))),
        ("count(ext:first() | //a)", Ok(Value::Number(2.0))),
        ("count(//a)", Ok(Value::Number(2.0))),
        ("ext:double(1)", Err(ErrorKind::UnknownFunction)),
        ("bad()", Err(ErrorKind::InvalidContext)),
    ];
    for (expression, expected) in cases {
        assert_eq!(&evaluate(expression), expected, "{expression}");
    }
}

#[test]
fn foreign_and_stale_nodes_are_invalid_context() {
    let owned = OwnedDocument::parse("<r><a/></r>").unwrap();
    let other = OwnedDocument::parse("<r><a/></r>").unwrap();
    let xpath = XPath::compile("count(*)").unwrap();
    let invalid = |node: Node<OwnedNodeId>| {
        xpath
            .evaluate(&owned, node, &Context::new())
            .map_err(|error| error.kind)
    };
    let root = owned.root_element().id();
    assert_eq!(invalid(Node::Tree(root)), Ok(Value::Number(1.0)));
    assert_eq!(
        invalid(Node::Tree(other.root().id())),
        Err(ErrorKind::InvalidContext)
    );
    assert_eq!(
        invalid(Node::Attribute(root, 0)),
        Err(ErrorKind::InvalidContext)
    );
    assert_eq!(
        invalid(Node::Namespace(root, 1)),
        Err(ErrorKind::InvalidContext)
    );
    assert_eq!(invalid(Node::Namespace(root, 0)), Ok(Value::Number(0.0)));

    let document = Document::parse("<r xmlns:p='urn:p' p:x='1'/>").unwrap();
    let element = document.root_element().id();
    let at = |node: Node<NodeId>, expression: &str| {
        XPath::compile(expression)
            .unwrap()
            .evaluate(&document, node, &Context::new())
            .map_err(|error| error.kind)
    };
    // Attribute 0 is the namespace declaration, which is not an attribute node.
    assert_eq!(
        at(Node::Attribute(element, 0), "."),
        Err(ErrorKind::InvalidContext)
    );
    assert_eq!(
        at(Node::Attribute(element, 1), "name()"),
        Ok(Value::String("p:x".into()))
    );
    assert_eq!(
        at(Node::Namespace(element, 1), "concat(name(), '=', .)"),
        Ok(Value::String("p=urn:p".into()))
    );
    assert_eq!(
        at(Node::Namespace(element, 2), "."),
        Err(ErrorKind::InvalidContext)
    );
    assert_eq!(
        at(Node::Tree(document.root().id()), "name(*)"),
        Ok(Value::String("r".into()))
    );
}

#[test]
fn select_nodes_requires_a_node_set() {
    let document = Document::parse(XML).unwrap();
    let root = document.root().id();
    let context = Context::new();
    assert_eq!(
        XPath::compile("//n")
            .unwrap()
            .select_nodes(&document, root, &context)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        XPath::compile("1")
            .unwrap()
            .select_nodes(&document, root, &context),
        Err(Error::new(ErrorKind::InvalidType))
    );
    assert_eq!(
        XPath::compile("$x")
            .unwrap()
            .select_nodes(&document, root, &context)
            .map_err(|error| error.kind),
        Err(ErrorKind::UndefinedVariable)
    );
    assert_eq!(XPath::compile(" //n ").unwrap().as_str(), " //n ");
}

#[test]
fn edited_owned_documents_keep_document_order() {
    let mut document = OwnedDocument::parse("<r><a/><b/></r>").unwrap();
    let a = document.root_element().first_child().unwrap().id();
    let spec = |local| ElementSpec {
        name: NameSpec {
            local,
            namespace: None,
            prefix: PrefixPolicy::Exact(None),
        },
        items: &[],
    };
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let late = editor.append_element(a, spec("late")).unwrap();
    editor.append_text(late, "x").unwrap();
    let b = editor.document().root_element().last_child().unwrap().id();
    editor.remove_subtree(b).unwrap();
    let names = |expression: &str| {
        let nodes = XPath::compile(expression)
            .unwrap()
            .select_nodes(&document, document.root().id(), &Context::new())
            .unwrap();
        nodes
            .into_iter()
            .map(|node| owned_label(&document, node))
            .collect::<Vec<_>>()
    };
    assert_eq!(names("//*"), ["r", "a", "late"]);
    assert_eq!(names("//text() | //*"), ["r", "a", "late", "text(x)"]);
    assert_eq!(
        names("//late/preceding::node() | //late/ancestor::*"),
        ["r", "a"]
    );
    assert_eq!(names("//b"), Vec::<String>::new());
}

#[test]
fn limits_are_enforced_before_work() {
    let document = Document::parse(XML).unwrap();
    let small = Limits {
        max_expression_bytes: 16,
        max_depth: 4,
        max_operations: 20,
        max_nodes: 3,
    };
    let compile = |expression: &str| {
        XPath::compile_with_limits(expression, &small)
            .map(|_| ())
            .map_err(|error| error.kind)
    };
    assert_eq!(
        compile("12345678901234567"),
        Err(ErrorKind::LimitExceeded(Limit::ExpressionBytes))
    );
    assert_eq!(compile("1234567890123456"), Ok(()));
    assert_eq!(
        compile("((((1))))"),
        Err(ErrorKind::LimitExceeded(Limit::Depth))
    );
    assert_eq!(compile("1+1+1+1"), Ok(()));
    assert_eq!(
        compile("1+1+1+1+1"),
        Err(ErrorKind::LimitExceeded(Limit::Depth))
    );
    assert_eq!(
        compile("----1"),
        Err(ErrorKind::LimitExceeded(Limit::Depth))
    );
    assert_eq!(compile("a[b[c[d]]]"), Ok(()));
    assert_eq!(
        compile("a[b[c[d[e]]]]"),
        Err(ErrorKind::LimitExceeded(Limit::Depth))
    );

    let evaluate_with = |limits: Limits, expression: &str| {
        let context = Context::new().with_limits(limits);
        XPath::compile(expression)
            .unwrap()
            .evaluate(&document, document.root().id(), &context)
            .map(|_| ())
            .map_err(|error| error.kind)
    };
    let evaluate = |expression: &str| {
        evaluate_with(
            Limits {
                max_operations: u64::MAX,
                ..small
            },
            expression,
        )
    };
    assert_eq!(evaluate("/r/a"), Ok(()));
    assert_eq!(evaluate("//*"), Err(ErrorKind::LimitExceeded(Limit::Nodes)));
    assert_eq!(
        evaluate("//a | //n"),
        Err(ErrorKind::LimitExceeded(Limit::Nodes))
    );
    assert_eq!(
        evaluate("//*[local-name() = 'd']/namespace::*|/r"),
        Err(ErrorKind::LimitExceeded(Limit::Nodes))
    );
    assert_eq!(
        evaluate_with(
            Limits {
                max_nodes: usize::MAX,
                ..small
            },
            "count(//node()[1])"
        ),
        Err(ErrorKind::LimitExceeded(Limit::Operations))
    );
    assert_eq!(
        evaluate_with(
            Limits {
                max_nodes: usize::MAX,
                ..small
            },
            "string(/)"
        ),
        Err(ErrorKind::LimitExceeded(Limit::Operations))
    );

    // Default limits admit deep but bounded nesting without exhausting the stack.
    let deep = |depth| format!("{}1{}", "(".repeat(depth), ")".repeat(depth));
    let limits = Limits::default();
    assert!(XPath::compile(&deep(limits.max_depth - 1)).is_ok());
    assert_eq!(
        XPath::compile(&deep(limits.max_depth))
            .map(|_| ())
            .map_err(|error| error.kind),
        Err(ErrorKind::LimitExceeded(Limit::Depth))
    );
    let chain = format!("1{}", "+1".repeat(limits.max_depth - 2));
    let sum = XPath::compile(&chain).unwrap();
    assert_eq!(
        sum.evaluate(&document, document.root().id(), &Context::new()),
        Ok(Value::Number((limits.max_depth - 1) as f64))
    );
    let predicates = format!("count(/r{})", "[*]".repeat(150));
    assert!(XPath::compile(&predicates).is_ok());
    let nested = format!(
        "{}1{}",
        "/r[".repeat(limits.max_depth / 2 - 1),
        "]".repeat(limits.max_depth / 2 - 1)
    );
    let nested = XPath::compile(&nested).unwrap();
    assert!(
        nested
            .evaluate(&document, document.root().id(), &Context::new())
            .is_ok()
    );
}

#[test]
fn errors_display_kind_and_offset() {
    assert_eq!(
        XPath::compile("1 +").unwrap_err().to_string(),
        "XPath error Expression at byte 3"
    );
    assert_eq!(
        Error::new(ErrorKind::InvalidType).to_string(),
        "XPath error InvalidType"
    );
    let error: Box<dyn std::error::Error> = Box::new(Error::new(ErrorKind::InvalidChar));
    assert!(error.source().is_none());
}

#[test]
fn values_and_nodes_convert() {
    let node: Node<u8> = 3.into();
    assert_eq!(node, Node::Tree(3));
    assert!(Node::Tree(1u8) < Node::Attribute(0, 0));
    let context: Context<u8> = Context::default();
    assert!(format!("{context:?}").contains("namespaces"));
}

#[test]
fn doctype_is_invisible_and_expanded_content_is_queryable() {
    let xml = "<!--c--><!DOCTYPE r [<!ENTITY e '<n>x</n>'><!ATTLIST r a CDATA 'd'>]><r>&e;</r>";
    let context = Context::new();
    let run = |expression| run_with(xml, expression, &context, &Context::new());
    assert_eq!(run("/node()"), nodes(&["comment(c)", "r"]));
    assert_eq!(run("count(//node())"), num(4.0));
    assert_eq!(run("string(/r/@a)"), string("d"));
    assert_eq!(run("string(/r/n)"), string("x"));
    assert_eq!(run("/comment()/following-sibling::node()"), nodes(&["r"]));
    assert_eq!(run("/r/preceding-sibling::node()"), nodes(&["comment(c)"]));
}
