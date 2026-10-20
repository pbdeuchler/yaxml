#![forbid(unsafe_code)]
//! XPointer evaluation as libxml2 2.15.4 `xmlXPtrEval` performs it; each
//! expectation was checked with `xmllint --xinclude`.

use yaxml::xpath::{self, Limits, Node, Tree};
use yaxml::xpointer::{Error, evaluate};
use yaxml::{Document, OwnedDocument};

const XML: &str = "<r><a xml:id='i'><b/><c>t</c></a><p:q xmlns:p='urn:p'/></r>";

/// Names (or `#text`/`@attribute`) of the located nodes.
fn names<T: Tree>(tree: &T, pointer: &str) -> Result<Vec<String>, Error>
where
    T::Id: std::fmt::Debug,
{
    let nodes = evaluate(tree, pointer, &Limits::default())?;
    let name = |node: Node<T::Id>| {
        let expression = xpath::XPath::compile("name()").unwrap();
        let context = xpath::Context::new();
        match expression.evaluate(tree, node, &context).unwrap() {
            xpath::Value::String(name) if name.is_empty() => String::from("#text"),
            xpath::Value::String(name) => match node {
                Node::Attribute(..) => format!("@{name}"),
                _ => name,
            },
            other => panic!("{other:?}"),
        }
    };
    Ok(nodes.into_iter().map(name).collect())
}

#[test]
fn pointers_locate_libxml2_nodes() {
    let document = OwnedDocument::parse(XML).unwrap();
    let located: &[(&str, &[&str])] = &[
        ("i", &["a"]),
        (" i ", &["a"]),
        ("i/2", &["c"]),
        ("i/3", &[]),
        ("i/0", &[]),
        ("i/99999999999999999999", &[]),
        ("nothing", &[]),
        ("/1", &["r"]),
        ("/1/1/2", &["c"]),
        ("/1/0", &[]),
        ("element(/1/1)", &["a"]),
        ("element(i/1)", &["b"]),
        ("element(/1/1)junk", &["a"]),
        ("xpointer(//b)", &["b"]),
        ("xpath1(//b|//c)", &["b", "c"]),
        ("xpointer(//c/text())", &["#text"]),
        ("xpointer(//@xml:id)", &["@xml:id"]),
        ("xpointer(//zz) xpointer(//c)", &["c"]),
        ("xpointer(//b)xpointer(//c)", &["b"]),
        ("xpointer(//b))junk", &["b"]),
        ("xpointer(//b) unknown(x)", &["b"]),
        ("xpointer(count(//b))", &[]),
        ("xpointer(//c[not(contains(., '^)'))])", &["c"]),
        ("xpointer(//c[. = 't^^'])", &[]),
        ("xmlns(x=urn:p) xpointer(//x:q)", &["p:q"]),
        ("xpointer(//b) junk", &["b"]),
        ("xmlns(x =urn:p) xpointer(//x:q)", &["p:q"]),
    ];
    for (pointer, expected) in located {
        assert_eq!(
            names(&document, pointer),
            Ok(expected.iter().map(|name| name.to_string()).collect()),
            "{pointer}"
        );
    }
    let failing: &[(&str, Error)] = &[
        ("", Error::Syntax),
        ("i)", Error::Syntax),
        ("i/1 x", Error::Syntax),
        ("1", Error::Syntax),
        ("/2", Error::ChildSequenceStart),
        ("element(/2)", Error::ChildSequenceStart),
        ("element()", Error::Syntax),
        ("xpointer(//b", Error::Syntax),
        ("xmlns( x = urn:p) xpointer(//x:q)", Error::Syntax),
        ("xmlns(=urn:p)", Error::Syntax),
        ("xmlns(x)", Error::Syntax),
        ("unknown(x) xpointer(//b)", Error::UnknownScheme),
        ("unknown(x)", Error::UnknownScheme),
        (
            "xpointer(//x:q)",
            Error::XPath(xpath::Error::new(xpath::ErrorKind::UndefinedPrefix)),
        ),
    ];
    for (pointer, expected) in failing {
        let error = names(&document, pointer).unwrap_err();
        match (error, expected) {
            (Error::XPath(found), Error::XPath(expected)) => {
                assert_eq!(found.kind, expected.kind, "{pointer}")
            }
            (found, expected) => assert_eq!(found, *expected, "{pointer}"),
        }
    }
}

#[test]
fn pointers_work_on_the_borrowed_dom_and_respect_limits() {
    let document = Document::parse(XML).unwrap();
    assert_eq!(names(&document, "i/2").unwrap(), ["c"]);
    let limits = {
        let mut cfg = Limits::default();
        cfg.max_operations = 3;
        cfg
    };
    assert!(matches!(
        evaluate(&document, "xpointer(//*)", &limits),
        Err(Error::XPath(error)) if error.kind == xpath::ErrorKind::LimitExceeded(xpath::Limit::Operations)
    ));
}
