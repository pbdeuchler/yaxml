#![forbid(unsafe_code)]
//! Canonical XML against libxml2 2.15.4's `test/c14n` suite, the W3C
//! recommendations' examples and hand-checked edge cases.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use yaxml::c14n::{self, Error, Mode, Options};
use yaxml::xpath::{Context, Tree, XPath};
use yaxml::{Document, FileResolver, Limits, OwnedDocument, Resolver};

const DEFAULT_SOURCE: &str =
    "/Users/pbdeuchler/workspace/src/github.com/pbdeuchler/yaxml-oracle/libxml2-2.15.4";

/// An XPath expression and the prefixes it binds.
struct Selection {
    expression: String,
    namespaces: Vec<(String, String)>,
}

fn selection(path: &Path) -> Option<Selection> {
    let bytes = fs::read(path).ok()?;
    let document = Document::parse_bytes(&bytes).expect("XPath file parses");
    let element = document.root_element();
    assert_eq!(element.name().expect("element").local(), "XPath");
    Some(Selection {
        expression: element.text_chunks().collect(),
        namespaces: element
            .attributes()
            .iter()
            .filter_map(|attribute| {
                let prefix = attribute.name.qualified().strip_prefix("xmlns:")?;
                Some((prefix.to_owned(), attribute.value.to_string()))
            })
            .collect(),
    })
}

fn canonical<T: Tree>(
    tree: &T,
    root: T::Id,
    mode: Mode,
    with_comments: bool,
    selection: Option<&Selection>,
    prefixes: &[&str],
) -> Result<String, Error> {
    let nodes = selection.map(|selection| {
        let context = (selection.namespaces.iter())
            .fold(Context::new(), |context, (prefix, uri)| {
                context.with_namespace(prefix, uri)
            });
        XPath::compile(&selection.expression)
            .expect("expression compiles")
            .select_nodes(tree, root, &context)
            .expect("expression selects nodes")
    });
    let options = Options {
        with_comments,
        nodes: nodes.as_deref(),
        inclusive_prefixes: prefixes,
        ..Options::new(mode)
    };
    c14n::to_string(tree, &options)
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Pass,
    /// The input has a DTD, which neither DOM accepts yet.
    Dtd,
    Mismatch,
}

#[test]
fn libxml2_suite() {
    let source = std::env::var_os("YAXML_LIBXML2_SRC").map_or(DEFAULT_SOURCE.into(), PathBuf::from);
    if !source.join("test/c14n").is_dir() {
        eprintln!("skipping: no libxml2 source at {}", source.display());
        return;
    }
    let suites = [
        ("with-comments", Mode::C14n10, true),
        ("without-comments", Mode::C14n10, false),
        ("exc-without-comments", Mode::Exclusive10, false),
        ("1-1-without-comments", Mode::C14n11, false),
    ];
    let mut outcomes = Vec::new();
    for (directory, mode, with_comments) in suites {
        let inputs = source.join("test/c14n").join(directory);
        let mut names: Vec<_> = fs::read_dir(&inputs)
            .expect("suite directory")
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().into_string().ok()?;
                Some(name.strip_suffix(".xml")?.to_owned())
            })
            .collect();
        names.sort();
        for name in names {
            // runtest.c also lists merlin-c14n-two-{15,16,25}, which have no result file.
            let Ok(expected) = fs::read(source.join("result/c14n").join(directory).join(&name))
            else {
                continue;
            };
            let input = fs::read(inputs.join(format!("{name}.xml"))).expect("input");
            let list = fs::read_to_string(inputs.join(format!("{name}.ns"))).unwrap_or_default();
            let list = list.trim_matches('\'');
            let prefixes: Vec<&str> = match list.is_empty() {
                true => Vec::new(),
                false => list.split(',').collect(),
            };
            // Like runtest.c's xmlReadFile with NOENT, entities load beside the input.
            let base = format!("{name}.xml");
            let resolver =
                || -> Box<dyn Resolver> { Box::new(FileResolver::new(&inputs).unwrap()) };
            // The owned DOM decodes every supported encoding; the borrowed one must agree when it parses.
            let outcome =
                match OwnedDocument::with_resolver(&input, Limits::default(), &base, resolver()) {
                    Ok(owned) => {
                        let selection = selection(&inputs.join(format!("{name}.xpath")));
                        let selection = selection.as_ref();
                        let root = owned.root().id();
                        let actual =
                            canonical(&owned, root, mode, with_comments, selection, &prefixes)
                                .expect("owned c14n");
                        if let Ok(borrowed) =
                            Document::with_resolver(&input, Limits::default(), &base, resolver())
                        {
                            let root = borrowed.root().id();
                            let other = canonical(
                                &borrowed,
                                root,
                                mode,
                                with_comments,
                                selection,
                                &prefixes,
                            );
                            assert_eq!(
                                other.expect("borrowed c14n"),
                                actual,
                                "{directory}/{name}: DOMs disagree"
                            );
                        }
                        match actual.as_bytes() == expected {
                            true => Outcome::Pass,
                            false => {
                                eprintln!(
                                    "{directory}/{name}:\n{actual}\n--- expected\n{}",
                                    String::from_utf8_lossy(&expected)
                                );
                                Outcome::Mismatch
                            }
                        }
                    }
                    Err(_) if input.windows(9).any(|window| window == b"<!DOCTYPE") => Outcome::Dtd,
                    Err(error) => panic!("{directory}/{name}: unexpected parse error {error:?}"),
                };
            outcomes.push(outcome);
        }
    }
    let count = |wanted: Outcome| {
        outcomes
            .iter()
            .filter(|&outcome| *outcome == wanted)
            .count()
    };
    let (pass, dtd, mismatch) = (
        count(Outcome::Pass),
        count(Outcome::Dtd),
        count(Outcome::Mismatch),
    );
    eprintln!("libxml2 c14n suite: {pass} pass, {dtd} need DTD support, {mismatch} mismatch");
    // without-comments/test-3 and exc-without-comments/test-{0,1} select an
    // `xmlns=""` namespace node, which yaxml's XPath does not model (c14n.md).
    assert_eq!((pass, dtd, mismatch), (70, 0, 3));
}

/// Canonicalizes `xml` over both DOMs, checking they agree.
fn both(
    xml: &str,
    mode: Mode,
    with_comments: bool,
    selection: Option<&Selection>,
    prefixes: &[&str],
) -> Result<String, Error> {
    let borrowed = Document::parse(xml).expect("borrowed parse");
    let owned = OwnedDocument::parse(xml).expect("owned parse");
    let left = canonical(
        &borrowed,
        borrowed.root().id(),
        mode,
        with_comments,
        selection,
        prefixes,
    );
    let right = canonical(
        &owned,
        owned.root().id(),
        mode,
        with_comments,
        selection,
        prefixes,
    );
    assert_eq!(
        format!("{left:?}"),
        format!("{right:?}"),
        "DOMs disagree on {xml}"
    );
    left
}

const ALL: [Mode; 3] = [Mode::C14n10, Mode::C14n11, Mode::Exclusive10];

/// Whole documents; expectations recorded from `xmllint --c14n`, `--c14n11`
/// and `--exc-c14n` (which keep comments) unless marked `false`.
#[test]
fn whole_documents() {
    let cases: &[(&str, &[Mode], bool, &str)] = &[
        (
            "<?xml version='1.0' encoding='UTF-8'?>  <?p?><!--a--><r>x</r><?q  data ?><!--b-->",
            &ALL,
            true,
            "<?p?>\n<!--a-->\n<r>x</r>\n<?q data ?>\n<!--b-->",
        ),
        ("<!--a--><r><!--in-->x</r><!--b-->", &ALL, false, "<r>x</r>"),
        (
            "<r a=\"&lt;&amp;&gt;&quot;'&#9;&#10;&#13;\">&lt;&amp;&gt;\"'&#13;<![CDATA[<&>]]></r>",
            &ALL,
            true,
            "<r a=\"&lt;&amp;>&quot;'&#x9;&#xA;&#xD;\">&lt;&amp;&gt;\"'&#xD;&lt;&amp;&gt;</r>",
        ),
        (
            "<r>\r\n<?p a\rb?><!--c&#13;\r--></r>",
            &ALL,
            true,
            "<r>\n<?p a\nb?><!--c&#13;\n--></r>",
        ),
        (
            "<r z='1' b:a='2' a:b='3' a='4' xmlns:a='urn:b' xmlns:b='urn:a'/>",
            &ALL,
            true,
            r#"<r xmlns:a="urn:b" xmlns:b="urn:a" a="4" z="1" b:a="2" a:b="3"></r>"#,
        ),
        (
            "<r xmlns='urn:d' xmlns:p='urn:p'><p:e xmlns:p='urn:p' xmlns='urn:d'><f xmlns=''/></p:e></r>",
            &[Mode::C14n10, Mode::C14n11],
            true,
            r#"<r xmlns="urn:d" xmlns:p="urn:p"><p:e><f xmlns=""></f></p:e></r>"#,
        ),
        (
            "<r xmlns='urn:d' xmlns:p='urn:p'><p:e xmlns:p='urn:p' xmlns='urn:d'><f xmlns=''/></p:e></r>",
            &[Mode::Exclusive10],
            true,
            r#"<r xmlns="urn:d"><p:e xmlns:p="urn:p"><f xmlns=""></f></p:e></r>"#,
        ),
        (
            "<r xmlns:p='urn:p' xmlns:q='urn:q'><p:e q:a='1'><e xmlns:p='urn:p2'/></p:e></r>",
            &[Mode::C14n10, Mode::C14n11],
            true,
            r#"<r xmlns:p="urn:p" xmlns:q="urn:q"><p:e q:a="1"><e xmlns:p="urn:p2"></e></p:e></r>"#,
        ),
        (
            "<r xmlns:p='urn:p' xmlns:q='urn:q'><p:e q:a='1'><e xmlns:p='urn:p2'/></p:e></r>",
            &[Mode::Exclusive10],
            true,
            r#"<r><p:e xmlns:p="urn:p" xmlns:q="urn:q" q:a="1"><e></e></p:e></r>"#,
        ),
        (
            "<r xml:base='' xml:lang='en' xml:space='preserve' xml:id='i'/>",
            &[Mode::C14n10, Mode::Exclusive10],
            true,
            r#"<r xml:base="" xml:id="i" xml:lang="en" xml:space="preserve"></r>"#,
        ),
        // C14N 1.1 drops an empty xml:base, as libxml2's fixup does.
        (
            "<r xml:base='' xml:lang='en' xml:space='preserve' xml:id='i'/>",
            &[Mode::C14n11],
            true,
            r#"<r xml:id="i" xml:lang="en" xml:space="preserve"></r>"#,
        ),
        (
            "<p:r xmlns:p='http://e/' xmlns='http://d/'><c xmlns=''><d xmlns='http://d/'/></c></p:r>",
            &[Mode::C14n10, Mode::C14n11],
            true,
            r#"<p:r xmlns="http://d/" xmlns:p="http://e/"><c xmlns=""><d xmlns="http://d/"></d></c></p:r>"#,
        ),
        (
            "<p:r xmlns:p='http://e/' xmlns='http://d/'><c xmlns=''><d xmlns='http://d/'/></c></p:r>",
            &[Mode::Exclusive10],
            true,
            r#"<p:r xmlns:p="http://e/"><c><d xmlns="http://d/"></d></c></p:r>"#,
        ),
        (
            "<r xmlns:xml='http://www.w3.org/XML/1998/namespace' xml:lang='en'><xml:e/></r>",
            &ALL,
            true,
            r#"<r xml:lang="en"><xml:e></xml:e></r>"#,
        ),
        // Namespace URIs are written as stored, like xmlOutputBufferWriteQuotedString.
        (
            "<a xmlns:p='http://a/?x&amp;y'/>",
            &[Mode::C14n10],
            true,
            r#"<a xmlns:p="http://a/?x&y"></a>"#,
        ),
        // W3C C14N 1.0 section 3.2: whitespace in document content is kept.
        (
            "<doc>\n   <clean>   </clean>\n   <dirty>   A   B   </dirty>\n   <mixed>\n      A\n      <clean>   </clean>\n      B\n      <dirty>   A   B   </dirty>\n      C\n   </mixed>\n</doc>",
            &ALL,
            false,
            "<doc>\n   <clean>   </clean>\n   <dirty>   A   B   </dirty>\n   <mixed>\n      A\n      <clean>   </clean>\n      B\n      <dirty>   A   B   </dirty>\n      C\n   </mixed>\n</doc>",
        ),
    ];
    for &(xml, modes, with_comments, expected) in cases {
        for &mode in modes {
            let canonical = both(xml, mode, with_comments, None, &[]).expect("c14n");
            assert_eq!(canonical, expected, "{mode:?} {xml}");
            // Raw namespace URIs can make the output malformed, as in libxml2.
            if Document::parse(&canonical).is_ok() {
                let again =
                    both(&canonical, mode, with_comments, None, &[]).expect("idempotent c14n");
                assert_eq!(again, canonical, "{mode:?} is not idempotent on {xml}");
            }
        }
    }
}

/// Document subsets, checked by hand against the recommendations.
#[test]
fn node_sets() {
    // Exclusive XML Canonicalization section 2.2.
    const SPEC: &str = "<n0:local xmlns:n0='foo:bar' xmlns:n3='ftp://example.org'><n1:elem2 xmlns:n1='http://example.net' xml:lang='en'><n3:stuff xmlns:n3='ftp://example.org'/></n1:elem2></n0:local>";
    const BASE: &str =
        "<r xml:lang='en' xml:id='a' xml:base='http://x/a/'><s xml:base='b/'><t/></s></r>";
    let subtree = |apex: &str| Selection {
        expression: format!("(//. | //@* | //namespace::*)[ancestor-or-self::{apex}]"),
        namespaces: vec![("n1".into(), "http://example.net".into())],
    };
    let cases: &[(&str, Selection, Mode, &[&str], &str)] = &[
        (
            SPEC,
            subtree("n1:elem2"),
            Mode::C14n10,
            &[],
            r#"<n1:elem2 xmlns:n0="foo:bar" xmlns:n1="http://example.net" xmlns:n3="ftp://example.org" xml:lang="en"><n3:stuff></n3:stuff></n1:elem2>"#,
        ),
        (
            SPEC,
            subtree("n1:elem2"),
            Mode::Exclusive10,
            &[],
            r#"<n1:elem2 xmlns:n1="http://example.net" xml:lang="en"><n3:stuff xmlns:n3="ftp://example.org"></n3:stuff></n1:elem2>"#,
        ),
        (
            SPEC,
            subtree("n1:elem2"),
            Mode::Exclusive10,
            &["n0", "xml", "absent"],
            r#"<n1:elem2 xmlns:n0="foo:bar" xmlns:n1="http://example.net" xml:lang="en"><n3:stuff xmlns:n3="ftp://example.org"></n3:stuff></n1:elem2>"#,
        ),
        // An orphan inherits xml:* attributes in 1.0; 1.1 skips xml:id and fixes up xml:base.
        (
            BASE,
            subtree("t"),
            Mode::C14n10,
            &[],
            r#"<t xml:base="b/" xml:id="a" xml:lang="en"></t>"#,
        ),
        (
            BASE,
            subtree("t"),
            Mode::C14n11,
            &[],
            r#"<t xml:base="http://x/a/b/" xml:lang="en"></t>"#,
        ),
        (BASE, subtree("t"), Mode::Exclusive10, &[], "<t></t>"),
        (
            BASE,
            subtree("s"),
            Mode::C14n11,
            &[],
            r#"<s xml:base="http://x/a/b/" xml:lang="en"><t></t></s>"#,
        ),
        // Attribute nodes are rendered even when their element is not in the set.
        (
            "<r a='1' b='2'><s/></r>",
            Selection {
                expression: "//@a | //s".into(),
                namespaces: Vec::new(),
            },
            Mode::C14n10,
            &[],
            r#" a="1"<s></s>"#,
        ),
        // A default namespace kept for a selected descendant becomes xmlns="" below it.
        (
            "<r xmlns='urn:d'><s><t xmlns=''/></s></r>",
            Selection {
                expression: "//*[local-name()!='r'] | //*[local-name()='s']/namespace::*".into(),
                namespaces: Vec::new(),
            },
            Mode::C14n10,
            &[],
            r#"<s xmlns="urn:d"><t xmlns=""></t></s>"#,
        ),
        (
            "<r xmlns='urn:d'><s/></r>",
            Selection {
                expression: "//*[local-name()='s']".into(),
                namespaces: Vec::new(),
            },
            Mode::Exclusive10,
            &["#default"],
            "<s></s>",
        ),
        // Like libxml2, a listed namespace node renders even on an omitted element.
        (
            "<r xmlns='urn:d'><s/></r>",
            Selection {
                expression: "//namespace::* | //*[local-name()='s']".into(),
                namespaces: Vec::new(),
            },
            Mode::Exclusive10,
            &["#default"],
            r#" xmlns="urn:d"<s xmlns="urn:d"></s>"#,
        ),
        // ...and a listed default namespace omitted from the set is undeclared.
        (
            "<r xmlns='urn:d'><p:e xmlns:p='urn:p'/></r>",
            Selection {
                expression: "//* | /*/namespace::*".into(),
                namespaces: Vec::new(),
            },
            Mode::Exclusive10,
            &["#default"],
            r#"<r xmlns="urn:d"><p:e xmlns=""></p:e></r>"#,
        ),
        (
            "<r xmlns:xml='http://www.w3.org/XML/1998/namespace'><s/></r>",
            Selection {
                expression: "//s | //namespace::*".into(),
                namespaces: Vec::new(),
            },
            Mode::Exclusive10,
            &["xml"],
            "<s></s>",
        ),
        // An outermost selected element stands in for an omitted document element.
        (
            "<r><a/><?p?><b/></r>",
            Selection {
                expression: "//a | //b | //processing-instruction()".into(),
                namespaces: Vec::new(),
            },
            Mode::C14n10,
            &[],
            "<a></a>\n<?p?><b></b>",
        ),
        // Exclusive mode: a prefix utilized by a selected attribute is declared
        // unless an output ancestor rendered its namespace node.
        (
            "<p:e xmlns:p='urn:p' p:a='1'/>",
            Selection {
                expression: "//* | //@*".into(),
                namespaces: Vec::new(),
            },
            Mode::Exclusive10,
            &[],
            r#"<p:e xmlns:p="urn:p" p:a="1"></p:e>"#,
        ),
        (
            "<r><p:e xmlns:p='urn:p' p:a='1'/></r>",
            Selection {
                expression: "//@*".into(),
                namespaces: Vec::new(),
            },
            Mode::Exclusive10,
            &[],
            r#" p:a="1""#,
        ),
    ];
    for (xml, selection, mode, prefixes, expected) in cases {
        let canonical = both(xml, *mode, false, Some(selection), prefixes).expect("c14n");
        assert_eq!(&canonical, expected, "{mode:?} {}", selection.expression);
    }
}

/// The error's kind and URI, for comparison.
fn failure(result: Result<String, Error>) -> String {
    match result {
        Ok(_) => "ok".into(),
        Err(Error::RelativeNamespace(uri)) => format!("relative {uri}"),
        Err(Error::InvalidUri(uri)) => format!("invalid {uri}"),
        Err(error) => format!("other {error}"),
    }
}

#[test]
fn errors() {
    let everything = None;
    let root = Selection {
        expression: "/a".into(),
        namespaces: Vec::new(),
    };
    let orphan = Selection {
        expression: "//t".into(),
        namespaces: Vec::new(),
    };
    let base = "<r xml:base='http://h/'><t xml:base='a b'/></r>";
    let cases: &[(&str, &[Mode], Option<&Selection>, &str)] = &[
        (
            "<a><b xmlns:p='rel'/></a>",
            &ALL,
            everything,
            "relative rel",
        ),
        ("<a xmlns='../d'/>", &ALL, everything, "relative ../d"),
        (
            "<a xmlns:p='http://[::1'/>",
            &ALL,
            everything,
            "invalid http://[::1",
        ),
        // Omitted elements are checked too.
        (
            "<a><b xmlns:p='rel'/></a>",
            &ALL,
            Some(&root),
            "relative rel",
        ),
        ("<a xmlns:p='urn:x' xmlns=''/>", &ALL, everything, "ok"),
        // Only C14N 1.1 resolves xml:base, and reports an unresolvable reference.
        (base, &[Mode::C14n11], Some(&orphan), "invalid a b"),
        (
            base,
            &[Mode::C14n10, Mode::Exclusive10],
            Some(&orphan),
            "ok",
        ),
    ];
    for &(xml, modes, selection, expected) in cases {
        for &mode in modes {
            let actual = failure(both(xml, mode, false, selection, &[]));
            assert_eq!(actual, expected, "{mode:?} {xml}");
        }
    }
    // libxml2's 1.0 inheritance skips only selected own attributes, unlike the recommendation.
    let inherited = both(base, Mode::C14n10, false, Some(&orphan), &[]);
    assert_eq!(inherited.unwrap(), r#"<t xml:base="http://h/"></t>"#);
}

struct Failing;

impl Write for Failing {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("sink full"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn sinks() {
    let document = Document::parse("<r b='2' a='1'>t</r>").unwrap();
    let options = Options::new(Mode::C14n10);
    let mut out = Vec::new();
    c14n::write(&document, &options, &mut out).unwrap();
    assert_eq!(out, br#"<r a="1" b="2">t</r>"#);
    // Small output fails at the final flush; large output while canonicalizing.
    let large = format!("<r>{}</r>", "x".repeat(20_000));
    for xml in ["<r/>", large.as_str()] {
        let document = Document::parse(xml).unwrap();
        let error = c14n::write(&document, &options, Failing).unwrap_err();
        assert!(matches!(&error, Error::Io(io) if io.to_string() == "sink full"));
        assert_eq!(error.to_string(), "C14N output error: sink full");
        assert!(std::error::Error::source(&error).is_some());
    }
    let relative = Error::RelativeNamespace("x".into());
    assert_eq!(relative.to_string(), "C14N error: relative namespace \"x\"");
    assert!(std::error::Error::source(&relative).is_none());
    assert_eq!(
        Error::InvalidUri("y".into()).to_string(),
        "C14N error: invalid URI \"y\""
    );
}
