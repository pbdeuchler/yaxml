//! DOM accessors on both trees and the owned editor's structural, attribute,
//! name and namespace edits. Nodes are addressed by `path()`.
#![forbid(unsafe_code)]

use yaxml::{
    Document, EditError, EditErrorKind, EditLimit, EditLimits, NameSpec, NamespaceBinding,
    OwnedDocument, OwnedNodeId, ParseOptions, PrefixPolicy, Writer,
};

const XML: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
/// `xmlSaveDoc` output (declarations before attributes) without the XML
/// declaration; the Writer also checks the tree is namespace-well-formed.
fn written(document: &OwnedDocument) -> String {
    let mut writer = Writer::new(Vec::new());
    writer
        .write_owned_document(document)
        .expect("writable tree");
    let saved = yaxml::save::to_vec(document, &yaxml::save::Options::default()).unwrap();
    let saved = String::from_utf8(saved).expect("UTF-8 output");
    let body = saved
        .strip_prefix("<?xml version=\"1.0\"?>\n")
        .unwrap_or(&saved);
    body.trim_end_matches('\n').replace(">\n<", "><")
}

fn at(document: &OwnedDocument, path: &str) -> OwnedNodeId {
    document
        .nodes()
        .find(|node| node.path().as_deref() == Some(path))
        .unwrap_or_else(|| panic!("no node at {path}"))
        .id()
}

fn parse(xml: &str) -> OwnedDocument {
    OwnedDocument::parse(xml).expect("well-formed test input")
}

/// Asserts that `check` gives `expected` on the borrowed and the owned tree
/// of `xml`, for the node at preorder position `index`.
macro_rules! both {
    ($xml:expr, $index:expr, |$node:ident| $check:expr, $expected:expr) => {{
        let borrowed = Document::parse($xml).unwrap();
        let $node = borrowed.nodes().nth($index).unwrap();
        assert_eq!($check, $expected, "borrowed {} node {}", $xml, $index);
        let owned = parse($xml);
        let $node = owned.nodes().nth($index).unwrap();
        assert_eq!($check, $expected, "owned {} node {}", $xml, $index);
    }};
}

#[test]
fn previous_sibling_inverts_next_sibling_on_both_trees() {
    let xml = "<!DOCTYPE r><!--a--><r><a><b/>t</a><?p?><c/>x<![CDATA[y]]></r><!--z-->";
    let borrowed = Document::parse(xml).unwrap();
    for node in borrowed.nodes() {
        if let Some(next) = node.next_sibling() {
            assert_eq!(next.previous_sibling().map(|n| n.id()), Some(node.id()));
        }
        let first = node.parent().and_then(|parent| parent.first_child());
        assert_eq!(
            node.previous_sibling().is_none(),
            first.is_none_or(|first| first.id() == node.id())
        );
    }
    let owned = parse(xml);
    for node in owned.nodes() {
        if let Some(next) = node.next_sibling() {
            assert_eq!(next.previous_sibling().map(|n| n.id()), Some(node.id()));
        }
    }
    assert!(owned.root().previous_sibling().is_none());
    assert!(borrowed.root().previous_sibling().is_none());
}

#[test]
fn attribute_lookups_follow_get_prop_and_get_ns_prop() {
    let xml = concat!(
        "<!DOCTYPE r [<!ATTLIST r d CDATA 'dv' p:q CDATA 'pq' xml:lang CDATA 'en'>]>",
        "<r xmlns:p='urn:p' a='1' p:a='2' p:b='3'/>"
    );
    // (name, namespace selector, expected): `None` selector is xmlGetProp.
    type Case<'a> = (&'a str, Option<Option<&'a str>>, Option<&'a str>);
    let cases: &[Case<'_>] = &[
        ("a", None, Some("1")),
        ("b", None, Some("3")),
        ("a", Some(None), Some("1")),
        ("a", Some(Some("urn:p")), Some("2")),
        ("b", Some(None), None),
        ("missing", None, None),
        ("p", None, None),
        ("xmlns", None, None),
        ("d", None, Some("dv")),
        ("d", Some(None), Some("dv")),
        ("q", Some(Some("urn:p")), Some("pq")),
        ("q", Some(Some("urn:other")), None),
        ("lang", Some(Some(XML)), Some("en")),
    ];
    for &(name, namespace, expected) in cases {
        both!(
            xml,
            2,
            |node| match namespace {
                None => node.attribute(name),
                Some(namespace) => node.attribute_ns(name, namespace),
            },
            expected
        );
    }
    // Non-elements have no attributes.
    both!(xml, 0, |node| node.attribute("a"), None);
}

#[test]
fn content_concatenates_text_and_skips_the_doctype() {
    let xml = "<!DOCTYPE r [<!ENTITY e 'ent'>]><r>a<b>b<![CDATA[c]]></b><!--m-->&e;<?p d?></r>";
    let cases: &[(usize, Option<&str>)] = &[
        (0, Some("abcent")),
        (1, None),
        (2, Some("abcent")),
        (3, Some("a")),
        (4, Some("bc")),
        (7, Some("m")),
        (8, Some("ent")),
        (10, Some("d")),
    ];
    for &(index, expected) in cases {
        both!(
            xml,
            index,
            |node| node.content(),
            expected.map(str::to_owned)
        );
    }
}

#[test]
fn path_matches_libxml2_node_paths() {
    let xml = concat!(
        "<!DOCTYPE r [<!ENTITY e '<i/>'>]><!--c--><r xmlns='urn:d' xmlns:p='urn:p'>",
        "<a/>t<p:a/><a/><x xmlns=''><y/><y/>&e;</x><?pi?><?pi?><?q?><!--k--></r>"
    );
    let expected = [
        Some("/"),
        None,
        Some("/comment()"),
        Some("/*"),
        Some("/*/*[1]"),
        Some("/*/text()"),
        Some("/*/p:a"),
        Some("/*/*[3]"),
        Some("/*/x"),
        Some("/*/x/y[1]"),
        Some("/*/x/y[2]"),
        None,
        None,
        Some("/*/processing-instruction('pi')[1]"),
        Some("/*/processing-instruction('pi')[2]"),
        Some("/*/processing-instruction('q')"),
        Some("/*/comment()"),
    ];
    for (index, expected) in expected.iter().enumerate() {
        both!(xml, index, |node| node.path(), expected.map(str::to_owned));
    }
}

#[test]
fn base_lang_and_space_inherit_with_libxml2_rules() {
    let xml = concat!(
        "<!DOCTYPE r [<!ENTITY e '<i>z</i>'>]>",
        "<r xml:base='http://h/a/' xml:lang='en' xml:space='preserve'>",
        "<s xml:base='b/' xml:space='bogus'><t xml:base='urn:x' xml:lang='fr'/>&e;</s>",
        "<u xml:base='/abs/' xml:space='default'>x</u></r>"
    );
    // (index, base, lang, space)
    type Case<'a> = (usize, Option<&'a str>, Option<&'a str>, Option<bool>);
    let cases: &[Case<'_>] = &[
        (0, None, None, None),
        (2, Some("http://h/a/"), Some("en"), Some(true)),
        (3, Some("http://h/a/b/"), Some("en"), Some(true)),
        (4, Some("urn:x"), Some("fr"), Some(true)),
        // Entity content stops at the reference, as at libxml2's declaration.
        (5, Some("http://h/a/b/"), Some("en"), None),
        (6, None, None, None),
        (7, None, None, None),
        (8, Some("http://h/abs/"), Some("en"), Some(false)),
        (9, Some("http://h/abs/"), Some("en"), None),
    ];
    for &(index, base, lang, space) in cases {
        both!(
            xml,
            index,
            |node| (node.base_uri(), node.lang(), node.space_preserve()),
            (base.map(str::to_owned), lang, space)
        );
    }
}

#[test]
fn base_resolves_against_the_document_url() {
    let xml = b"<r xml:base='sub/'><a xml:base='f.xml'/><b/></r>";
    let options = || ParseOptions::default().base(Some("/doc/main.xml".to_owned()));
    let borrowed = Document::parse_with(xml, options()).unwrap();
    let owned = OwnedDocument::parse_with(xml, options()).unwrap();
    assert_eq!(borrowed.url(), Some("/doc/main.xml"));
    assert_eq!(owned.url(), Some("/doc/main.xml"));
    let a = borrowed.root_element().first_child().unwrap();
    assert_eq!(a.base_uri().as_deref(), Some("/doc/sub/f.xml"));
    assert_eq!(owned.root().base_uri().as_deref(), Some("/doc/main.xml"));
    assert_eq!(Document::parse("<r/>").unwrap().url(), None);
    let mut owned = owned;
    owned
        .edit(EditLimits::default())
        .unwrap()
        .set_url(None)
        .unwrap();
    assert_eq!(owned.root().base_uri(), None);
    let limits = EditLimits::default().max_value_bytes(2);
    let error = owned
        .edit(limits)
        .unwrap()
        .set_url(Some("long"))
        .unwrap_err();
    assert_eq!(
        error.kind,
        EditErrorKind::LimitExceeded(EditLimit::ValueBytes)
    );
}

#[test]
#[cfg(feature = "html")]
fn html_base_comes_from_the_base_element() {
    let cases: &[(&[u8], Option<&str>)] = &[
        (
            b"<html><head><base href='http://x/'></head><body>t</body></html>",
            Some("http://x/"),
        ),
        (b"<html><head><base></head></html>", None),
        (b"<html><body>t</body></html>", None),
    ];
    for &(input, expected) in cases {
        let document = yaxml::html::parse(input, yaxml::html::Options::default())
            .unwrap()
            .document;
        let last = document.nodes().last().unwrap();
        assert_eq!(last.base_uri().as_deref(), expected);
    }
}

#[test]
fn namespace_searches_follow_search_ns_and_get_ns_list() {
    let xml = "<r xmlns='urn:d' xmlns:a='urn:a'><s xmlns:a='urn:b' xmlns:c='urn:a'><t xmlns=''>x</t></s></r>";
    let binding =
        |prefix: Option<&'static str>, uri: &'static str| NamespaceBinding { prefix, uri };
    // (index, prefix, search_ns)
    let searches: &[(usize, Option<&str>, Option<&str>)] = &[
        (1, None, Some("urn:d")),
        (1, Some("a"), Some("urn:a")),
        (2, Some("a"), Some("urn:b")),
        (3, None, Some("")),
        (4, Some("c"), Some("urn:a")),
        (4, Some("xml"), Some(XML)),
        (4, Some("z"), None),
        (0, Some("a"), None),
    ];
    for &(index, prefix, expected) in searches {
        both!(xml, index, |node| node.search_ns(prefix), expected);
    }
    // (index, uri, search_ns_by_href): `a` is shadowed in `s`, so `c` serves
    // urn:a, and `xmlns=""` hides urn:d in `t`.
    let hrefs: &[(usize, &str, Option<NamespaceBinding<'_>>)] = &[
        (1, "urn:a", Some(binding(Some("a"), "urn:a"))),
        (2, "urn:a", Some(binding(Some("c"), "urn:a"))),
        (2, "urn:d", Some(binding(None, "urn:d"))),
        (3, "urn:d", None),
        (3, XML, Some(binding(Some("xml"), XML))),
        (3, "urn:none", None),
    ];
    for &(index, uri, expected) in hrefs {
        both!(xml, index, |node| node.search_ns_by_href(uri), expected);
    }
    let list = vec![
        binding(None, ""),
        binding(Some("a"), "urn:b"),
        binding(Some("c"), "urn:a"),
    ];
    both!(xml, 4, |node| node.namespaces(), list);
    both!(xml, 0, |node| node.namespaces().is_empty(), true);
}

#[derive(Clone, Copy, Debug)]
enum Op {
    Append(&'static str, &'static str),
    Before(&'static str, &'static str),
    After(&'static str, &'static str),
    Unlink(&'static str),
    Replace(&'static str, &'static str),
    Set(&'static str, NameSpec<'static>, &'static str),
    Unset(&'static str, Option<&'static str>, &'static str),
    Declare(&'static str, Option<&'static str>, &'static str),
    Rename(&'static str, NameSpec<'static>),
    Compact,
}

const fn name<'a>(
    local: &'a str,
    namespace: Option<&'a str>,
    prefix: PrefixPolicy<'a>,
) -> NameSpec<'a> {
    NameSpec {
        local,
        namespace,
        prefix,
    }
}

const AUTO: PrefixPolicy<'static> = PrefixPolicy::Automatic;
const NONE: PrefixPolicy<'static> = PrefixPolicy::Exact(None);

const fn exact(prefix: &'static str) -> PrefixPolicy<'static> {
    PrefixPolicy::Exact(Some(prefix))
}

fn apply(document: &mut OwnedDocument, op: Op, limits: EditLimits) -> Result<(), EditError> {
    let find = |document: &OwnedDocument, path| at(document, path);
    let ids = |document: &OwnedDocument, a, b| (find(document, a), find(document, b));
    match op {
        Op::Append(parent, node) => {
            let (parent, node) = ids(document, parent, node);
            document.edit(limits)?.append_child(parent, node).map(drop)
        }
        Op::Before(reference, node) => {
            let (reference, node) = ids(document, reference, node);
            document
                .edit(limits)?
                .insert_before(reference, node)
                .map(drop)
        }
        Op::After(reference, node) => {
            let (reference, node) = ids(document, reference, node);
            document
                .edit(limits)?
                .insert_after(reference, node)
                .map(drop)
        }
        Op::Unlink(node) => {
            let node = find(document, node);
            document.edit(limits)?.unlink(node)
        }
        Op::Replace(old, new) => {
            let (old, new) = ids(document, old, new);
            document.edit(limits)?.replace(old, new)
        }
        Op::Set(element, name, value) => {
            let element = find(document, element);
            document.edit(limits)?.set_attribute(element, name, value)
        }
        Op::Unset(element, namespace, local) => {
            let element = find(document, element);
            document
                .edit(limits)?
                .remove_attribute(element, namespace, local)
        }
        Op::Declare(element, prefix, uri) => {
            let element = find(document, element);
            document
                .edit(limits)?
                .declare_namespace(element, prefix, uri)
        }
        Op::Rename(element, name) => {
            let element = find(document, element);
            document.edit(limits)?.set_name(element, name)
        }
        Op::Compact => document.edit(limits)?.compact(),
    }
}

/// `(case, input, operations, expected output or the last operation's error)`.
type Case = (
    &'static str,
    &'static str,
    &'static [Op],
    Result<&'static str, EditErrorKind>,
);

const CASES: &[Case] = &[
    // Moves.
    (
        "append moves",
        "<r><a/><b/></r>",
        &[Op::Append("/r", "/r/a")],
        Ok("<r><b/><a/></r>"),
    ),
    (
        "append merges text",
        "<r>x<a/>y</r>",
        &[Op::Append("/r", "/r/text()[1]")],
        Ok("<r><a/>yx</r>"),
    ),
    (
        "append keeps cdata",
        "<r>x<![CDATA[y]]></r>",
        &[Op::Append("/r", "/r/text()[1]")],
        Ok("<r><![CDATA[y]]>x</r>"),
    ),
    (
        "append last is no-op",
        "<r><a/>t</r>",
        &[Op::Append("/r", "/r/text()")],
        Ok("<r><a/>t</r>"),
    ),
    (
        "append into element",
        "<r><a/>t</r>",
        &[Op::Append("/r/a", "/r/text()")],
        Ok("<r><a>t</a></r>"),
    ),
    (
        "before",
        "<r><a/><b/></r>",
        &[Op::Before("/r/a", "/r/b")],
        Ok("<r><b/><a/></r>"),
    ),
    (
        "before keeps text apart",
        "<r>x<a/>y</r>",
        &[Op::Before("/r/a", "/r/text()[2]")],
        Ok("<r>xy<a/></r>"),
    ),
    (
        "after",
        "<r><a/><b/><c/></r>",
        &[Op::After("/r/c", "/r/a")],
        Ok("<r><b/><c/><a/></r>"),
    ),
    (
        "after next is no-op",
        "<r><a/><b/></r>",
        &[Op::After("/r/a", "/r/b")],
        Ok("<r><a/><b/></r>"),
    ),
    (
        "comment before root",
        "<r><!--c--></r>",
        &[Op::Before("/r", "/r/comment()")],
        Ok("<!--c--><r/>"),
    ),
    (
        "unlink",
        "<r><a><b/></a>t</r>",
        &[Op::Unlink("/r/a")],
        Ok("<r>t</r>"),
    ),
    (
        "replace",
        "<r><a/><b>t</b></r>",
        &[Op::Replace("/r/a", "/r/b")],
        Ok("<r><b>t</b></r>"),
    ),
    (
        "replace with descendant",
        "<r><a><b/></a></r>",
        &[Op::Replace("/r/a", "/r/a/b")],
        Ok("<r><b/></r>"),
    ),
    (
        "replace root",
        "<r><a>t</a></r>",
        &[Op::Replace("/r", "/r/a")],
        Ok("<a>t</a>"),
    ),
    (
        "cycle",
        "<r><a><b/></a></r>",
        &[Op::Append("/r/a/b", "/r/a")],
        Err(EditErrorKind::Hierarchy),
    ),
    (
        "self parent",
        "<r><a/></r>",
        &[Op::Append("/r/a", "/r/a")],
        Err(EditErrorKind::Hierarchy),
    ),
    (
        "self sibling",
        "<r><a/></r>",
        &[Op::Before("/r/a", "/r/a")],
        Err(EditErrorKind::Hierarchy),
    ),
    (
        "replace self",
        "<r><a/></r>",
        &[Op::Replace("/r/a", "/r/a")],
        Err(EditErrorKind::Hierarchy),
    ),
    (
        "replace ancestor",
        "<r><a><b/></a></r>",
        &[Op::Replace("/r/a/b", "/r/a")],
        Err(EditErrorKind::Hierarchy),
    ),
    (
        "replace root by text",
        "<r>t</r>",
        &[Op::Replace("/r", "/r/text()")],
        Err(EditErrorKind::Hierarchy),
    ),
    (
        "second root",
        "<r><a/></r>",
        &[Op::After("/r", "/r/a")],
        Err(EditErrorKind::Hierarchy),
    ),
    (
        "text at top",
        "<r>t</r>",
        &[Op::Before("/r", "/r/text()")],
        Err(EditErrorKind::Hierarchy),
    ),
    (
        "document anchor",
        "<r><a/></r>",
        &[Op::Before("/", "/r/a")],
        Err(EditErrorKind::Hierarchy),
    ),
    (
        "leaf parent",
        "<r>t<a/></r>",
        &[Op::Append("/r/text()", "/r/a")],
        Err(EditErrorKind::WrongNodeKind),
    ),
    (
        "move root",
        "<r/>",
        &[Op::Unlink("/r")],
        Err(EditErrorKind::ProtectedNode),
    ),
    (
        "move document",
        "<r/>",
        &[Op::Unlink("/")],
        Err(EditErrorKind::ProtectedNode),
    ),
    (
        "replace document",
        "<r/>",
        &[Op::Replace("/", "/r")],
        Err(EditErrorKind::ProtectedNode),
    ),
    (
        "move into root's place",
        "<r><a/></r>",
        &[Op::Replace("/r/a", "/r")],
        Err(EditErrorKind::ProtectedNode),
    ),
    // Namespaces travel with moved subtrees.
    (
        "move keeps prefix",
        "<r><p:a xmlns:p='urn:p'><p:b/></p:a><c/></r>",
        &[Op::Append("/r/c", "/r/p:a/p:b")],
        Ok("<r><p:a xmlns:p=\"urn:p\"/><c><p:b xmlns:p=\"urn:p\"/></c></r>"),
    ),
    (
        "move keeps default",
        "<r><a xmlns='urn:d'><b/></a><c/></r>",
        &[Op::Append("/r/c", "/r/*[1]/*")],
        Ok("<r><a xmlns=\"urn:d\"/><c><b xmlns=\"urn:d\"/></c></r>"),
    ),
    (
        "move into default",
        "<r><a/><b xmlns='urn:d'/></r>",
        &[Op::Append("/r/*[2]", "/r/a")],
        Ok("<r><b xmlns=\"urn:d\"><a xmlns=\"\"/></b></r>"),
    ),
    (
        "move attribute prefix",
        "<r xmlns:p='urn:p'><a p:x='1'/><b xmlns:p='urn:q'/></r>",
        &[Op::Append("/r/b", "/r/a")],
        Ok("<r xmlns:p=\"urn:p\"><b xmlns:p=\"urn:q\"><a xmlns:p=\"urn:p\" p:x=\"1\"/></b></r>"),
    ),
    // Attributes.
    (
        "set new",
        "<r a='1'/>",
        &[Op::Set("/r", name("b", None, NONE), "2")],
        Ok("<r a=\"1\" b=\"2\"/>"),
    ),
    (
        "set existing",
        "<r a='1' b='2'/>",
        &[Op::Set("/r", name("a", None, AUTO), "x")],
        Ok("<r a=\"x\" b=\"2\"/>"),
    ),
    (
        "set reuses prefix",
        "<r xmlns:p='urn:p'><a/></r>",
        &[Op::Set("/r/a", name("x", Some("urn:p"), AUTO), "1")],
        Ok("<r xmlns:p=\"urn:p\"><a p:x=\"1\"/></r>"),
    ),
    (
        "set generates prefix",
        "<r xmlns:ns1='urn:z'/>",
        &[Op::Set("/r", name("x", Some("urn:p"), AUTO), "1")],
        Ok("<r xmlns:ns1=\"urn:z\" xmlns:ns2=\"urn:p\" ns2:x=\"1\"/>"),
    ),
    (
        "set generated prefix skips an unbound spelling",
        "<r ns1:x='a'/>",
        &[Op::Set("/r", name("x", Some("urn:p"), AUTO), "1")],
        Ok("<r xmlns:ns2=\"urn:p\" ns1:x=\"a\" ns2:x=\"1\"/>"),
    ),
    (
        "set exact prefix spelled unbound",
        "<r ns1:x='a'/>",
        &[Op::Set("/r", name("x", Some("urn:p"), exact("ns1")), "1")],
        Err(EditErrorKind::DuplicateAttribute),
    ),
    (
        "set exact prefix",
        "<r/>",
        &[Op::Set("/r", name("x", Some("urn:p"), exact("q")), "1")],
        Ok("<r xmlns:q=\"urn:p\" q:x=\"1\"/>"),
    ),
    (
        "set shadows outer prefix",
        "<r xmlns:q='urn:o'><a><q:b/></a></r>",
        &[Op::Set("/r/a", name("x", Some("urn:p"), exact("q")), "1")],
        Ok("<r xmlns:q=\"urn:o\"><a xmlns:q=\"urn:p\" q:x=\"1\"><q:b xmlns:q=\"urn:o\"/></a></r>"),
    ),
    (
        "set xml",
        "<r/>",
        &[Op::Set("/r", name("lang", Some(XML), AUTO), "en")],
        Ok("<r xml:lang=\"en\"/>"),
    ),
    (
        "set conflicts with own declaration",
        "<r xmlns:q='urn:o'/>",
        &[Op::Set("/r", name("x", Some("urn:p"), exact("q")), "1")],
        Err(EditErrorKind::Namespace),
    ),
    (
        "unset declaration its element uses",
        "<q:r xmlns:q='urn:o' xmlns:z='urn:z'/>",
        &[Op::Unset("/q:r", Some(XMLNS), "q")],
        Err(EditErrorKind::Namespace),
    ),
    (
        "set namespace without prefix",
        "<r/>",
        &[Op::Set("/r", name("x", Some("urn:p"), NONE), "1")],
        Err(EditErrorKind::Namespace),
    ),
    (
        "set prefix without namespace",
        "<r/>",
        &[Op::Set("/r", name("x", None, exact("p")), "1")],
        Err(EditErrorKind::Namespace),
    ),
    (
        "set xmlns name",
        "<r/>",
        &[Op::Set("/r", name("xmlns", None, AUTO), "1")],
        Err(EditErrorKind::Namespace),
    ),
    (
        "set declaration",
        "<r/>",
        &[Op::Set("/r", name("p", Some(XMLNS), AUTO), "1")],
        Err(EditErrorKind::NamespaceDeclaration),
    ),
    (
        "set xml prefix elsewhere",
        "<r/>",
        &[Op::Set("/r", name("x", Some("urn:p"), exact("xml")), "1")],
        Err(EditErrorKind::Namespace),
    ),
    (
        "set bad name",
        "<r/>",
        &[Op::Set("/r", name("1x", None, AUTO), "1")],
        Err(EditErrorKind::InvalidName),
    ),
    (
        "set bad value",
        "<r/>",
        &[Op::Set("/r", name("x", None, AUTO), "\u{1}")],
        Err(EditErrorKind::InvalidCharacter),
    ),
    (
        "set on text",
        "<r>t</r>",
        &[Op::Set("/r/text()", name("x", None, AUTO), "1")],
        Err(EditErrorKind::WrongNodeKind),
    ),
    (
        "unset",
        "<r a='1' b='2' c='3'/>",
        &[Op::Unset("/r", None, "b")],
        Ok("<r a=\"1\" c=\"3\"/>"),
    ),
    (
        "unset namespaced",
        "<r xmlns:p='urn:p' p:a='1' a='2'/>",
        &[Op::Unset("/r", Some("urn:p"), "a")],
        Ok("<r xmlns:p=\"urn:p\" a=\"2\"/>"),
    ),
    (
        "unset unused declaration",
        "<r xmlns:p='urn:p' a='1'/>",
        &[Op::Unset("/r", Some(XMLNS), "p")],
        Ok("<r a=\"1\"/>"),
    ),
    (
        "unset used declaration",
        "<r xmlns:p='urn:p'><p:a/></r>",
        &[Op::Unset("/r", Some(XMLNS), "p")],
        Err(EditErrorKind::Namespace),
    ),
    (
        "unset shadowed declaration",
        "<r xmlns:p='urn:p'><p:a xmlns:p='urn:p'/></r>",
        &[Op::Unset("/r", Some(XMLNS), "p")],
        Ok("<r><p:a xmlns:p=\"urn:p\"/></r>"),
    ),
    (
        "unset missing",
        "<r a='1'/>",
        &[Op::Unset("/r", Some("urn:p"), "a")],
        Err(EditErrorKind::MissingAttribute),
    ),
    (
        "set then unset then compact",
        "<r a='1'><b c='2'/></r>",
        &[
            Op::Set("/r", name("x", None, AUTO), "1"),
            Op::Set("/r/b", name("y", None, AUTO), "2"),
            Op::Unset("/r", None, "a"),
            Op::Compact,
        ],
        Ok("<r x=\"1\"><b c=\"2\" y=\"2\"/></r>"),
    ),
    // Declarations and names.
    (
        "declare",
        "<r/>",
        &[Op::Declare("/r", Some("p"), "urn:p")],
        Ok("<r xmlns:p=\"urn:p\"/>"),
    ),
    (
        "declare default",
        "<r><a/></r>",
        &[Op::Declare("/r", None, "urn:d")],
        Err(EditErrorKind::Namespace),
    ),
    (
        "declare shadows descendants",
        "<r xmlns:p='urn:o'><a><p:b/></a></r>",
        &[Op::Declare("/r/a", Some("p"), "urn:n")],
        Ok("<r xmlns:p=\"urn:o\"><a xmlns:p=\"urn:n\"><p:b xmlns:p=\"urn:o\"/></a></r>"),
    ),
    (
        "declare twice",
        "<r xmlns:p='urn:p'/>",
        &[Op::Declare("/r", Some("p"), "urn:q")],
        Err(EditErrorKind::DuplicateAttribute),
    ),
    (
        "declare used by own name",
        "<p:r xmlns:q='urn:q' xmlns:p='urn:p'><p:a/></p:r>",
        &[Op::Declare("/p:r/p:a", Some("p"), "urn:x")],
        Err(EditErrorKind::Namespace),
    ),
    (
        "declare xmlns",
        "<r/>",
        &[Op::Declare("/r", Some("xmlns"), "urn:p")],
        Err(EditErrorKind::Namespace),
    ),
    (
        "declare empty prefixed",
        "<r/>",
        &[Op::Declare("/r", Some("p"), "")],
        Err(EditErrorKind::Namespace),
    ),
    (
        "declare xml uri",
        "<r/>",
        &[Op::Declare("/r", Some("x"), XML)],
        Err(EditErrorKind::Namespace),
    ),
    (
        "declare bad prefix",
        "<r/>",
        &[Op::Declare("/r", Some("a:b"), "urn:p")],
        Err(EditErrorKind::InvalidName),
    ),
    (
        "rename",
        "<r><a x='1'>t</a></r>",
        &[Op::Rename("/r/a", name("b", None, AUTO))],
        Ok("<r><b x=\"1\">t</b></r>"),
    ),
    (
        "rename into namespace",
        "<r><a/></r>",
        &[Op::Rename("/r/a", name("a", Some("urn:p"), exact("p")))],
        Ok("<r><p:a xmlns:p=\"urn:p\"/></r>"),
    ),
    (
        "rename reuses default",
        "<r xmlns='urn:d'><a/></r>",
        &[Op::Rename("/*/*", name("b", Some("urn:d"), AUTO))],
        Ok("<r xmlns=\"urn:d\"><b/></r>"),
    ),
    (
        "rename generates prefix",
        "<r><a/></r>",
        &[Op::Rename("/r/a", name("a", Some("urn:p"), AUTO))],
        Ok("<r><ns1:a xmlns:ns1=\"urn:p\"/></r>"),
    ),
    (
        "rename out of namespace",
        "<r xmlns='urn:d'><a/></r>",
        &[Op::Rename("/*/*", name("a", None, AUTO))],
        Ok("<r xmlns=\"urn:d\"><a xmlns=\"\"/></r>"),
    ),
    (
        "rename default keeps children",
        "<r><a><b/></a></r>",
        &[Op::Rename("/r/a", name("a", Some("urn:d"), NONE))],
        Ok("<r><a xmlns=\"urn:d\"><b xmlns=\"\"/></a></r>"),
    ),
    (
        "rename against own declaration",
        "<r><a xmlns:p='urn:o'/></r>",
        &[Op::Rename("/r/a", name("a", Some("urn:p"), exact("p")))],
        Err(EditErrorKind::Namespace),
    ),
    (
        "rename bad local",
        "<r/>",
        &[Op::Rename("/r", name("a:b", None, AUTO))],
        Err(EditErrorKind::InvalidName),
    ),
    (
        "rename prefix without namespace",
        "<r/>",
        &[Op::Rename("/r", name("a", None, exact("p")))],
        Err(EditErrorKind::Namespace),
    ),
];

#[test]
fn edits_match_their_tables() {
    for &(case, input, ops, expected) in CASES {
        let mut document = parse(input);
        let before = written(&document);
        let mut result = Ok(());
        for &op in ops {
            result = apply(&mut document, op, EditLimits::default());
            if result.is_err() {
                break;
            }
        }
        match expected {
            Ok(output) => {
                assert_eq!(result, Ok(()), "{case}");
                assert_eq!(written(&document), output, "{case}");
                let reparsed = parse(&written(&document));
                assert_eq!(written(&reparsed), output, "{case}: reparse");
            }
            Err(kind) => {
                assert_eq!(result.map_err(|error| error.kind), Err(kind), "{case}");
                if ops.len() == 1 {
                    assert_eq!(written(&document), before, "{case}: unchanged");
                }
            }
        }
    }
}

#[test]
fn stale_and_foreign_handles_are_rejected_by_every_edit() {
    let mut document = parse("<r><a/><b/></r>");
    let foreign = parse("<r><a/></r>").root_element().id();
    let (root, a) = (at(&document, "/r"), at(&document, "/r/a"));
    document
        .edit(EditLimits::default())
        .unwrap()
        .remove_subtree(a)
        .unwrap();
    let invalid = |result: Result<(), EditError>| {
        assert_eq!(result.unwrap_err().kind, EditErrorKind::InvalidHandle);
    };
    for stale in [a, foreign] {
        let mut editor = document.edit(EditLimits::default()).unwrap();
        invalid(editor.append_child(root, stale).map(drop));
        invalid(editor.append_child(stale, root).map(drop));
        invalid(editor.insert_before(stale, root).map(drop));
        invalid(editor.insert_after(root, stale).map(drop));
        invalid(editor.unlink(stale));
        invalid(editor.replace(stale, root));
        invalid(editor.replace(root, stale));
        invalid(editor.set_attribute(stale, name("x", None, AUTO), "1"));
        invalid(editor.remove_attribute(stale, None, "x"));
        invalid(editor.declare_namespace(stale, Some("p"), "urn:p"));
        invalid(editor.set_name(stale, name("x", None, AUTO)));
    }
}

#[test]
fn unlinked_subtrees_keep_handles_and_can_return() {
    let mut document = parse("<r><p:a xmlns:p='urn:p'><p:b>t</p:b></p:a><c/></r>");
    let (a, b, c) = (
        at(&document, "/r/p:a"),
        at(&document, "/r/p:a/p:b"),
        at(&document, "/r/c"),
    );
    let text = at(&document, "/r/p:a/p:b/text()");
    let mut editor = document.edit(EditLimits::default()).unwrap();
    editor.unlink(b).unwrap();
    editor.unlink(b).unwrap();
    let detached = editor.document().node(b).unwrap();
    assert!(detached.parent().is_none());
    assert_eq!(detached.path().as_deref(), Some("/p:b"));
    assert_eq!(detached.search_ns(Some("p")), None);
    assert_eq!(
        editor.document().node(text).unwrap().content().as_deref(),
        Some("t")
    );
    // A detached root has no siblings to anchor to.
    let error = editor.insert_after(b, c).unwrap_err();
    assert_eq!(error.kind, EditErrorKind::Hierarchy);
    editor.append_child(c, b).unwrap();
    assert_eq!(
        written(editor.document()),
        "<r><p:a xmlns:p=\"urn:p\"/><c><p:b xmlns:p=\"urn:p\">t</p:b></c></r>"
    );
    editor.unlink(a).unwrap();
    editor.remove_subtree(a).unwrap();
    assert!(editor.document().node(a).is_none());
    assert_eq!(editor.document().nodes().count(), 5);
}

#[test]
fn merged_text_removes_the_moved_node() {
    let mut document = parse("<r>a<b/>c</r>");
    let (r, first, last) = (
        at(&document, "/r"),
        at(&document, "/r/text()[1]"),
        at(&document, "/r/text()[2]"),
    );
    let mut editor = document.edit(EditLimits::default()).unwrap();
    assert_eq!(editor.append_child(r, first).unwrap(), last);
    assert!(editor.document().node(first).is_none());
    assert_eq!(editor.document().node(last).unwrap().text(), Some("ca"));
    let limits = EditLimits::default().max_value_bytes(2);
    let t = editor.append_text(r, "x").unwrap();
    let b = at(editor.document(), "/r/b");
    editor.insert_before(b, t).unwrap();
    let error = document
        .edit(limits)
        .unwrap()
        .append_child(r, t)
        .unwrap_err();
    assert_eq!(
        error.kind,
        EditErrorKind::LimitExceeded(EditLimit::ValueBytes)
    );
    assert_eq!(written(&document), "<r>x<b/>ca</r>");
}

#[test]
fn moves_respect_depth_namespace_work_and_attribute_quotas() {
    let deep = "<r><a><b><c/></b></a><d><e/></d></r>";
    let limits = EditLimits::default().max_depth(5);
    let mut document = parse(deep);
    let error = apply(&mut document, Op::Append("/r/d/e", "/r/a"), limits).unwrap_err();
    assert_eq!(error.kind, EditErrorKind::LimitExceeded(EditLimit::Depth));
    apply(&mut document, Op::Append("/r/d", "/r/a"), limits).unwrap();
    // Shallower and equal-depth moves stay allowed above the ceiling.
    let tight = EditLimits::default().max_depth(1);
    apply(&mut document, Op::Append("/r", "/r/d"), tight).unwrap();

    let namespaced = "<r><p:a xmlns:p='urn:p'><p:b/></p:a><c/></r>";
    let mut document = parse(namespaced);
    let work = EditLimits::default().max_namespace_work(2);
    let error = apply(&mut document, Op::Append("/r/c", "/r/p:a/p:b"), work).unwrap_err();
    assert_eq!(
        error.kind,
        EditErrorKind::LimitExceeded(EditLimit::NamespaceWork)
    );
    let created = EditLimits::default().max_created_attributes(0);
    let error = apply(&mut document, Op::Append("/r/c", "/r/p:a/p:b"), created).unwrap_err();
    assert_eq!(
        error.kind,
        EditErrorKind::LimitExceeded(EditLimit::CreatedAttributes)
    );
    let error = apply(
        &mut document,
        Op::Declare("/r", Some("q"), "urn:q"),
        created,
    )
    .unwrap_err();
    assert_eq!(
        error.kind,
        EditErrorKind::LimitExceeded(EditLimit::CreatedAttributes)
    );
    let error = apply(
        &mut document,
        Op::Set("/r", name("x", None, AUTO), "1"),
        created,
    )
    .unwrap_err();
    assert_eq!(
        error.kind,
        EditErrorKind::LimitExceeded(EditLimit::CreatedAttributes)
    );
    assert_eq!(
        written(&document),
        "<r><p:a xmlns:p=\"urn:p\"><p:b/></p:a><c/></r>"
    );
    let names = EditLimits::default().max_name_bytes(1);
    let error = apply(
        &mut document,
        Op::Rename("/r", name("long", None, AUTO)),
        names,
    )
    .unwrap_err();
    assert_eq!(
        error.kind,
        EditErrorKind::LimitExceeded(EditLimit::NameBytes)
    );
    let bytes = EditLimits::default().max_owned_bytes(0);
    let long = "l".repeat(64 * 1024);
    let root = document.root_element().id();
    let mut editor = document.edit(bytes).unwrap();
    let error = editor.set_name(root, name(&long, None, AUTO)).unwrap_err();
    assert_eq!(
        error.kind,
        EditErrorKind::LimitExceeded(EditLimit::OwnedBytes)
    );
}

#[test]
fn ids_follow_attribute_membership() {
    let mut document = parse("<r><a/><b xml:id='x' c='1'/></r>");
    let (a, b) = (at(&document, "/r/a"), at(&document, "/r/b"));
    assert_eq!(document.element_by_id("x").map(|node| node.id()), Some(b));
    let mut editor = document.edit(EditLimits::default()).unwrap();
    editor
        .set_attribute(a, name("id", Some(XML), AUTO), "y")
        .unwrap();
    editor.remove_attribute(b, Some(XML), "id").unwrap();
    editor.set_attribute(b, name("c", None, AUTO), "2").unwrap();
    assert!(editor.document().element_by_id("x").is_none());
    assert_eq!(
        editor.document().element_by_id("y").map(|node| node.id()),
        Some(a)
    );
}

#[test]
fn compaction_reclaims_storage_and_keeps_handles() {
    let input = "<!DOCTYPE r [<!ENTITY e 'v'>]><r a='&e;'><b c='1'>t</b></r>";
    let mut document =
        OwnedDocument::parse_with(input.as_bytes(), ParseOptions::default()).unwrap();
    let (r, b) = (at(&document, "/r"), at(&document, "/r/b"));
    let text = at(&document, "/r/b/text()");
    let limits = EditLimits::default().max_owned_bytes(4096);
    let mut editor = document.edit(limits).unwrap();
    for round in 0..200 {
        let value = format!("value {round}");
        editor.set_node_value(text, &value).unwrap();
        editor
            .set_attribute(b, name("d", None, AUTO), &value)
            .unwrap();
        editor.remove_attribute(b, None, "d").unwrap();
        if round % 10 == 9 {
            editor.compact().unwrap();
        }
    }
    editor.compact().unwrap();
    let node = editor.document().node(r).unwrap();
    assert_eq!(node.unexpanded_attribute(0), Some("&e;"));
    let saved = yaxml::save::to_vec(editor.document(), &yaxml::save::Options::default()).unwrap();
    assert!(
        String::from_utf8(saved)
            .unwrap()
            .contains("<r a=\"&e;\"><b c=\"1\">value 199</b></r>")
    );
    assert_eq!(
        editor.document().node(text).unwrap().text(),
        Some("value 199")
    );
}

#[test]
fn entity_content_is_read_only_for_structure() {
    let input = b"<!DOCTYPE r [<!ENTITY e '<i>z</i>'>]><r>&e;<a/></r>";
    let mut document = OwnedDocument::parse_with(input, ParseOptions::default()).unwrap();
    let reference = document.root_element().first_child().unwrap();
    let (reference, inner) = (reference.id(), reference.first_child().unwrap().id());
    let (root, a) = (at(&document, "/r"), at(&document, "/r/a"));
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let protected = |result: Result<(), EditError>| {
        assert_eq!(result.unwrap_err().kind, EditErrorKind::ProtectedNode);
    };
    protected(editor.append_child(reference, a).map(drop));
    protected(editor.append_child(inner, a).map(drop));
    protected(editor.append_child(root, inner).map(drop));
    protected(editor.unlink(inner));
    protected(editor.replace(inner, a));
    protected(editor.set_attribute(inner, name("x", None, AUTO), "1"));
    protected(editor.set_name(inner, name("x", None, AUTO)));
    // The reference itself moves with its content.
    editor.append_child(root, reference).unwrap();
    assert!(written(editor.document()).ends_with("]><r><a/>&e;</r>"));
}

#[test]
#[cfg(feature = "html")]
fn html_documents_take_attributes_but_no_namespaces() {
    let parsed = yaxml::html::parse(b"<p id=a>t</p>", yaxml::html::Options::default()).unwrap();
    let mut document = parsed.document;
    let p = document
        .nodes()
        .find(|node| node.name().is_some_and(|name| name.local() == "p"))
        .unwrap()
        .id();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let refused = |result: Result<(), EditError>| {
        assert_eq!(result.unwrap_err().kind, EditErrorKind::Namespace);
    };
    refused(editor.set_attribute(p, name("x", Some("urn:p"), AUTO), "1"));
    refused(editor.declare_namespace(p, Some("q"), "urn:q"));
    refused(editor.set_name(p, name("p", Some("urn:p"), AUTO)));
    editor
        .set_attribute(p, name("id", None, AUTO), "b")
        .unwrap();
    editor
        .set_attribute(p, name("title", None, AUTO), "c")
        .unwrap();
    assert_eq!(
        editor.document().element_by_id("b").map(|node| node.id()),
        Some(p)
    );
    let node = editor.document().node(p).unwrap();
    assert_eq!(node.attribute("title"), Some("c"));
}
