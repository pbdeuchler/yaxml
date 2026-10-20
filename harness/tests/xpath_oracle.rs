#![forbid(unsafe_code)]
//! Differential XPath check against the pinned libxml2 2.15.4 `xmllint`.
//!
//! `cargo test --test xpath_oracle -- --ignored --nocapture`; set
//! `YAXML_XMLLINT` to override the binary. Node-sets are compared through
//! `count()` plus each member's `name()` and `string()` in document order;
//! numbers through `string()` so libxml2's own number formatting is checked.

use std::path::PathBuf;
use std::process::Command;

use yaxml::Document;
use yaxml::xpath::{Context, Value, XPath};

const DEFAULT_XMLLINT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../yaxml-oracle/build/xmllint");

const FIXTURE: &str = r#"<?xml version="1.0"?>
<!-- lead -->
<?top data?>
<root xmlns:a="urn:a" xml:lang="en-US"><item id="1" xml:id="i1" a:k="x">10</item><item id="2" xml:id="i2">20<![CDATA[5]]></item><a:item id="3">abc <b>def</b> ghi</a:item><group xml:lang="fr" xmlns:c="urn:c"><item id="4">-3.5</item><empty/><?target value?><!-- note --><c:deep xmlns:d="urn:d"><leaf n="0.1"/><dflt xmlns="urn:e"><undecl xmlns=""/></dflt></c:deep></group><num>1e3</num><space>  a   b  </space><n>.5</n><n>12345678901</n><n>0.000001</n></root>
<!-- tail -->"#;

/// Expressions the comparison covers; intentional differences are listed in docs/xpath.md.
const EXPRESSIONS: &[&str] = &[
    // Location paths and axes.
    "/",
    "/root",
    "/*",
    "//item",
    "//item[2]",
    "(//item)[2]",
    "//item[last()]",
    "//item[position() < 3]",
    "//*[@id]",
    "//@id",
    "//@*",
    "/root/item/@a:k",
    "//a:*",
    "//a:item/b",
    "//b/..",
    "//b/ancestor::*",
    "//b/ancestor::*[1]",
    "//b/ancestor-or-self::*",
    "//leaf/ancestor::*[2]",
    "//group/descendant::*",
    "//group/descendant-or-self::node()",
    "//group/child::node()",
    "//item[1]/following::*",
    "//item[1]/following::*[1]",
    "//item[1]/following-sibling::*",
    "//item[1]/following-sibling::*[2]",
    "//group/preceding::*",
    "//group/preceding::*[1]",
    "//group/preceding-sibling::*",
    "//group/preceding-sibling::*[1]",
    "//group/self::group",
    "//group/self::item",
    "//@id/parent::*",
    "//@id/..",
    "//item/@id/following::*",
    "//empty/preceding::node()[1]",
    "/descendant::node()",
    "//text()",
    "//comment()",
    "//processing-instruction()",
    "//processing-instruction('target')",
    "//processing-instruction('none')",
    "/node()",
    "//node()[not(self::*)]",
    "//leaf/namespace::*",
    "count(//leaf/namespace::*)",
    "count(//undecl/namespace::*)",
    "//undecl/namespace::*",
    "//undecl/namespace::*[string() = '']",
    "//leaf/namespace::c",
    "//*[local-name() = 'leaf']",
    "//item | //b",
    "//b | //item",
    "(//item | //b)[3]",
    "//item[@id > 1][2]",
    "//item[. = 10]",
    "//item[contains(., '5')]",
    "//*[lang('en')]",
    "//*[lang('fr')]",
    "//item[lang('EN')]",
    "id('i2 i1')",
    "id('none')",
    "id(//item/@xml:id)",
    "//item[1]/@*",
    "//@*[1]",
    "//item/@id[. = 2]/..",
    "//*[count(*) = 2]",
    "//b/preceding::text()",
    "//b/preceding::text()[1]",
    "//leaf/following::*",
    "//@a:k/following::node()[1]",
    "//@a:k/preceding::node()",
    "//@id/ancestor::*",
    "//@id/ancestor-or-self::node()",
    "//@id/self::node()",
    "//@id/self::*",
    "//@id/descendant-or-self::node()",
    "//item[@id][last()]",
    "//*[starts-with(name(), 'a:')]",
    "//n[. > 1]",
    "//*[. = 'def']",
    "//item[@id=1 or @id=2]",
    "//item[ @id = 2 ]/text()",
    "//item[2]/text()[2]",
    "//*[local-name()='div'] | //b",
    "count(//item) div count(//n)",
    "count(//*) mod 7",
    "1-1",
    "2*3",
    "//n[1]*2",
    "//item[1]/@a:*",
    "//@xml:*",
    "//*[@xml:lang]",
    "/root////b",
    "//./b",
    "//item[1]/..//b",
    "(//item)[last()]/preceding-sibling::*[1]",
    // Numbers, booleans and strings.
    "sum(//n)",
    "string(sum(//n))",
    "string(//@a:k)",
    "-//item[1]",
    "string(2147483647)",
    "string(-2147483648)",
    "string(1e300 * 10)",
    "string(4.5e-6)",
    "string(999999999.5)",
    "string(1 div 7 * 1000)",
    "string(0.123456789012345678901234567 * 100000000000000000)",
    "string(123456789.123456789)",
    "string(0.1 * 3)",
    "string(number('0.0000000000000000000001') * 1e22)",
    "string(4.35)",
    "string(-999999.9999995)",
    "1 + 2 * 3",
    "(1 + 2) * 3",
    "10 div 4",
    "10 mod 3",
    "-10 mod 3",
    "1 div 0",
    "-1 div 0",
    "0 div 0",
    "- - 2",
    "2 - -2",
    "0.1 + 0.2",
    "1 div 3",
    "100000 * 100000",
    "123456789 * 10",
    "0.00001",
    "0.000001",
    "1e3",
    "1.5e-7",
    "3 > 2 = 1",
    "true() and false() or true()",
    "1 = 1 and 2 != 2",
    "'a' = 'a'",
    "'1' = 1",
    "true() = 'false'",
    "//item = 20",
    "//item != 10",
    "//item < 0",
    "//item > 19",
    "//item = //n",
    "//item != //item",
    "//item < //n",
    "//nothing = //nothing",
    "//nothing != 1",
    "//item = true()",
    "//nothing = false()",
    "string(//num)",
    "number(//num)",
    "number(' 12 ')",
    "number('1.')",
    "number('-.5')",
    "number('abc')",
    "number(true())",
    "string(number(//n[2]))",
    "string(//n[3] * 1)",
    "string(1 div 3)",
    "string(-0)",
    "string(true())",
    "boolean('')",
    "boolean(0 div 0)",
    "boolean(//empty)",
    "not(//nothing)",
    "//dflt",
    "//*[local-name() = 'dflt']",
    "namespace-uri(//*[local-name() = 'dflt'])",
    "//*[local-name() = 'dflt']/namespace::*",
    "name(//*[local-name() = 'dflt']/namespace::*[last()])",
    "//leaf/namespace::*[name() = 'd']",
    "//leaf/namespace::node()",
    "//leaf/@*/namespace::*",
    "//leaf/namespace::*/..",
    // Core functions.
    "count(//item)",
    "local-name(//a:item)",
    "local-name(//@a:k)",
    "local-name(//processing-instruction())",
    "namespace-uri(//a:item)",
    "namespace-uri(//leaf)",
    "name(//a:item)",
    "name(//@a:k)",
    "name(//leaf/namespace::*[last()])",
    "name()",
    "string(//leaf/namespace::*[last()])",
    "concat('a', 1, true())",
    "starts-with('abc', 'ab')",
    "starts-with('abc', 'b')",
    "contains('abc', '')",
    "substring-before('1999/04/01', '/')",
    "substring-after('1999/04/01', '/')",
    "substring-after('abc', '')",
    "substring('12345', 2, 3)",
    "substring('12345', 2)",
    "substring('12345', 1.5, 2.6)",
    "substring('12345', 0, 3)",
    "substring('12345', 0 div 0, 3)",
    "substring('12345', 1, 0 div 0)",
    "substring('12345', -42, 1 div 0)",
    "substring('12345', -1 div 0, 1 div 0)",
    "substring('ébc', 2)",
    "string-length('ébc')",
    "string-length(//space)",
    "normalize-space(//space)",
    "normalize-space('  x  ')",
    "translate('bar', 'abc', 'ABC')",
    "translate('--aaa--', 'abc-', 'ABC')",
    "sum(//item)",
    "sum(//nothing)",
    "sum(//@id)",
    "floor(2.5)",
    "floor(-2.5)",
    "ceiling(2.1)",
    "ceiling(-2.1)",
    "round(2.5)",
    "round(-2.5)",
    "round(-0.2)",
    "round(0 div 0)",
    "string(round(-0.4))",
    "string(//a:item)",
    "string(/)",
    // Errors.
    "",
    "//",
    "1 +",
    "count()",
    "count(1)",
    "unknown()",
    "$undefined",
    "//x:y",
    "'open",
    "(1",
    "//item[1",
    "@",
    "child::",
    "bogus::item",
    "processing-instruction(1)",
    "1 | 2",
    "'a'/b",
    "#",
];

enum Oracle {
    Output(String),
    Failure,
}

fn xmllint(binary: &PathBuf, file: &PathBuf, expression: &str) -> Oracle {
    let output = Command::new(binary)
        .args(["--xpath", expression])
        .arg(file)
        .output()
        .expect("run xmllint");
    let stdout = String::from_utf8(output.stdout).expect("xmllint UTF-8");
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("XPath set is empty") {
        return Oracle::Output("0".to_owned());
    }
    if !output.status.success() {
        return Oracle::Failure;
    }
    Oracle::Output(stdout.strip_suffix('\n').unwrap_or(&stdout).to_owned())
}

fn ours(document: &Document<'_>, expression: &str) -> Result<Value<yaxml::NodeId>, String> {
    XPath::compile(expression)
        .and_then(|xpath| xpath.evaluate(document, document.root().id(), &Context::new()))
        .map_err(|error| error.to_string())
}

fn scalar(value: Value<yaxml::NodeId>) -> String {
    match value {
        Value::String(text) => text,
        Value::Boolean(value) => value.to_string(),
        other => panic!("unexpected {other:?}"),
    }
}

/// The two-sided probe expressions for one result of ours.
fn probes(expression: &str, value: &Value<yaxml::NodeId>) -> Vec<String> {
    match value {
        Value::NodeSet(nodes) if nodes.is_empty() => vec![format!("count({expression})")],
        Value::NodeSet(nodes) => {
            let members: Vec<String> = (1..=nodes.len())
                .map(|i| {
                    format!("name(({expression})[{i}]), '=', string(({expression})[{i}]), ';'")
                })
                .collect();
            vec![
                format!("count({expression})"),
                format!("concat('', {})", members.join(", ")),
            ]
        }
        Value::Number(_) => vec![format!("string({expression})")],
        _ => vec![expression.to_owned()],
    }
}

/// Intentional differences (docs/xpath.md); each must still differ.
const DIFFERENCES: &[&str] = &[
    // xmllint evaluates with an unset context size and position.
    "last()",
    "position()",
];

fn compare(
    binary: &PathBuf,
    file: &PathBuf,
    document: &Document<'_>,
    expression: &str,
) -> Result<(), String> {
    let value = match ours(document, expression) {
        Err(error) => {
            return match xmllint(binary, file, expression) {
                Oracle::Failure => Ok(()),
                Oracle::Output(text) => Err(format!("ours {error}, libxml2 {text:?}")),
            };
        }
        Ok(value) => value,
    };
    probes(expression, &value)
        .into_iter()
        .try_for_each(|probe| {
            let expected = match xmllint(binary, file, &probe) {
                Oracle::Failure => return Err(format!("libxml2 failed on {probe}")),
                Oracle::Output(text) => text,
            };
            let actual = scalar(match ours(document, &probe)? {
                Value::Number(count) => Value::String(count.to_string()),
                other => other,
            });
            if actual == expected {
                Ok(())
            } else {
                Err(format!("{probe}: ours {actual:?}, libxml2 {expected:?}"))
            }
        })
}

#[test]
#[ignore = "requires the pinned libxml2 xmllint"]
fn xpath_matches_pinned_xmllint() {
    let binary =
        PathBuf::from(std::env::var("YAXML_XMLLINT").unwrap_or(DEFAULT_XMLLINT.to_owned()));
    let directory = std::env::temp_dir().join(format!("yaxml-xpath-oracle-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("temporary directory");
    let file = directory.join("fixture.xml");
    std::fs::write(&file, FIXTURE).expect("write fixture");
    let document = Document::parse(FIXTURE).expect("fixture parses");
    let mut failures = Vec::new();
    for (expressions, expect_match) in [(EXPRESSIONS, true), (DIFFERENCES, false)] {
        for &expression in expressions {
            let outcome = compare(&binary, &file, &document, expression);
            println!(
                "{:<8} {expression} {}",
                if outcome.is_ok() { "match" } else { "differ" },
                outcome.clone().err().unwrap_or_default()
            );
            if outcome.is_ok() != expect_match {
                failures.push(expression);
            }
        }
    }
    let _ = std::fs::remove_dir_all(&directory);
    println!(
        "{} expressions matched, {} intentional differences, {} failures",
        EXPRESSIONS.len(),
        DIFFERENCES.len(),
        failures.len()
    );
    assert!(failures.is_empty(), "failures: {failures:?}");
}
