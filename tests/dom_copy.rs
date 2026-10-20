//! Copies within and across owned documents (`Editor::import`,
//! `Editor::copy`, `OwnedDocument::try_clone`) and `list_string`
//! (`xmlNodeListGetString`) on both trees. Nodes are addressed by `path()`,
//! or by preorder position as `#n`.
#![forbid(unsafe_code)]

use yaxml::html::{self, Options};
use yaxml::{
    Document, EditErrorKind, EditLimit, EditLimits, OwnedDocument, OwnedNodeId, OwnedNodeKind,
};

/// `xmlSaveDoc` output without the XML declaration, one line.
fn written(document: &OwnedDocument) -> String {
    let saved = yaxml::save::to_vec(document, &yaxml::save::Options::default()).unwrap();
    let saved = String::from_utf8(saved).unwrap();
    let body = saved
        .strip_prefix("<?xml version=\"1.0\"?>\n")
        .unwrap_or(&saved);
    body.trim_end_matches('\n').replace(">\n<", "><")
}

fn at(document: &OwnedDocument, address: &str) -> OwnedNodeId {
    let found = match address.strip_prefix('#') {
        Some(index) => document.nodes().nth(index.parse().unwrap()),
        None => document
            .nodes()
            .find(|node| node.path().as_deref() == Some(address)),
    };
    found.unwrap_or_else(|| panic!("no node at {address}")).id()
}

/// A document from XML, or from HTML when the text starts with `html:`.
fn parse(text: &str) -> OwnedDocument {
    match text.strip_prefix("html:") {
        Some(markup) => {
            let options = Options::new(html::NODEFDTD | html::NOIMPLIED);
            html::parse(markup.as_bytes(), options).unwrap().document
        }
        None => OwnedDocument::parse(text).expect("well-formed test input"),
    }
}

#[derive(Clone, Copy, Debug)]
enum Op {
    /// Import the source node (deep or not) and append it to the destination node.
    Import(&'static str, &'static str, bool),
    /// Copy a destination node within it and append the copy.
    Copy(&'static str, &'static str, bool),
}

/// `(case, source, destination, operation, limits, expected output or error)`.
type Case = (
    &'static str,
    &'static str,
    &'static str,
    Op,
    EditLimits,
    Result<&'static str, EditErrorKind>,
);

const DEFAULT: EditLimits = {
    let mut cfg = EditLimits::DEFAULT;
    cfg.max_value_bytes = 1024 * 1024;
    cfg.max_owned_bytes = 64 * 1024 * 1024;
    cfg.max_created_nodes = 1_000_000;
    cfg.max_name_bytes = 1024 * 1024;
    cfg.max_supplied_bytes = 8 * 1024 * 1024;
    cfg.max_attributes = 1024;
    cfg.max_depth = 256;
    cfg.max_created_attributes = 1_000_000;
    cfg.max_namespace_work = 64 * 1024 * 1024;
    cfg
};

const fn limit(limits: EditLimits) -> EditLimits {
    limits
}

const DTD_E: &str = "<!DOCTYPE t [<!ENTITY e 'v'>]><t/>";

const CASES: &[Case] = &[
    // Namespaces: bindings from outside the copy go on its root.
    (
        "outer binding declared on the copy root",
        "<r xmlns:p='urn:p'><p:a><p:b/><p:c xmlns:q='urn:q' q:x='1'/></p:a></r>",
        "<t/>",
        Op::Import("/r/p:a", "/t", true),
        DEFAULT,
        Ok("<t><p:a xmlns:p=\"urn:p\"><p:b/><p:c xmlns:q=\"urn:q\" q:x=\"1\"/></p:a></t>"),
    ),
    (
        "branches share one root declaration",
        "<r xmlns:p='urn:p'><a><p:b/><x><p:c/></x></a></r>",
        "<t/>",
        Op::Import("/r/a", "/t", true),
        DEFAULT,
        Ok("<t><a xmlns:p=\"urn:p\"><p:b/><x><p:c/></x></a></t>"),
    ),
    (
        "default namespace declared on the root",
        "<r xmlns='urn:d'><a><b/></a></r>",
        "<t/>",
        Op::Import("/*/*", "/t", true),
        DEFAULT,
        Ok("<t><a xmlns=\"urn:d\"><b/></a></t>"),
    ),
    (
        "redundant declaration kept, as xmlDocCopyNode",
        "<r xmlns:p='urn:p'><p:a/></r>",
        "<t xmlns:p='urn:p'/>",
        Op::Import("/r/p:a", "/t", true),
        DEFAULT,
        Ok("<t xmlns:p=\"urn:p\"><p:a xmlns:p=\"urn:p\"/></t>"),
    ),
    (
        "no namespace under a default namespace undeclares it",
        "<r><a/></r>",
        "<t xmlns='urn:d'/>",
        Op::Import("/r/a", "/*", true),
        DEFAULT,
        Ok("<t xmlns=\"urn:d\"><a xmlns=\"\"/></t>"),
    ),
    (
        "attribute prefix bound outside",
        "<r xmlns:p='urn:p'><a p:k='1'/></r>",
        "<t/>",
        Op::Import("/r/a", "/t", true),
        DEFAULT,
        Ok("<t><a xmlns:p=\"urn:p\" p:k=\"1\"/></t>"),
    ),
    (
        "xml prefix is never declared",
        "<r><a xml:lang='en'/></r>",
        "<t/>",
        Op::Import("/r/a", "/t", true),
        DEFAULT,
        Ok("<t><a xml:lang=\"en\"/></t>"),
    ),
    (
        "shallow copy keeps attributes and declarations",
        "<r xmlns:p='urn:p'><p:a k='1' xmlns:q='urn:q'><p:b/></p:a></r>",
        "<t/>",
        Op::Import("/r/p:a", "/t", false),
        DEFAULT,
        Ok("<t><p:a xmlns:q=\"urn:q\" xmlns:p=\"urn:p\" k=\"1\"/></t>"),
    ),
    // Leaves and merging.
    (
        "text merges through append_child",
        "<r>x</r>",
        "<t>y</t>",
        Op::Import("/r/text()", "/t", true),
        DEFAULT,
        Ok("<t>yx</t>"),
    ),
    (
        "comment and cdata",
        "<r><!--c--><![CDATA[d]]></r>",
        "<t/>",
        Op::Import("/r", "/t", true),
        DEFAULT,
        Ok("<t><r><!--c--><![CDATA[d]]></r></t>"),
    ),
    (
        "processing instruction under the document",
        "<r><?p d?></r>",
        "<t/>",
        Op::Import("/r/processing-instruction('p')", "#0", true),
        DEFAULT,
        Ok("<t/><?p d?>"),
    ),
    // Within one document.
    (
        "deep copy within the document",
        "<r/>",
        "<r><a k='1'><b>x</b></a></r>",
        Op::Copy("/r/a", "/r", true),
        DEFAULT,
        Ok("<r><a k=\"1\"><b>x</b></a><a k=\"1\"><b>x</b></a></r>"),
    ),
    (
        "copy within declares what the detached copy lacks",
        "<r/>",
        "<r xmlns:p='urn:p'><p:a/></r>",
        Op::Copy("/r/p:a", "/r", true),
        DEFAULT,
        Ok("<r xmlns:p=\"urn:p\"><p:a/><p:a xmlns:p=\"urn:p\"/></r>"),
    ),
    (
        "copy within keeps entity references",
        "<r/>",
        "<!DOCTYPE r [<!ENTITY e 'v'>]><r><a>&e;</a></r>",
        Op::Copy("/r/a", "/r", true),
        DEFAULT,
        Ok("<!DOCTYPE r [\n<!ENTITY e \"v\">\n]><r><a>&e;</a><a>&e;</a></r>"),
    ),
    // Entities: the destination decides.
    (
        "undeclared reference becomes its content",
        "<!DOCTYPE r [<!ENTITY e '<i>v</i>w'>]><r>&e;</r>",
        "<t/>",
        Op::Import("/r", "/t", true),
        DEFAULT,
        Ok("<t><r><i>v</i>w</r></t>"),
    ),
    (
        "declared reference stays",
        "<!DOCTYPE r [<!ENTITY e 'v'>]><r>&e;</r>",
        DTD_E,
        Op::Import("/r", "/t", true),
        DEFAULT,
        Ok("<!DOCTYPE t [\n<!ENTITY e \"v\">\n]><t><r>&e;</r></t>"),
    ),
    (
        "undeclared kept attribute form is substituted",
        "<!DOCTYPE r [<!ENTITY e 'v'>]><r a='1&e;&amp;'/>",
        "<t/>",
        Op::Import("/r", "/t", true),
        DEFAULT,
        Ok("<t><r a=\"1v&amp;\"/></t>"),
    ),
    (
        "declared kept attribute form stays",
        "<!DOCTYPE r [<!ENTITY e 'v'>]><r a='1&e;&amp;'/>",
        DTD_E,
        Op::Import("/r", "/t", true),
        DEFAULT,
        Ok("<!DOCTYPE t [\n<!ENTITY e \"v\">\n]><t><r a=\"1&e;&amp;\"/></t>"),
    ),
    (
        "undeclared reference cannot be the copy",
        "<!DOCTYPE r [<!ENTITY e 'v'>]><r>&e;</r>",
        "<t/>",
        Op::Import("#3", "/t", true),
        DEFAULT,
        Err(EditErrorKind::Hierarchy),
    ),
    (
        "declared reference copies with its content",
        "<!DOCTYPE r [<!ENTITY e 'v'>]><r>&e;</r>",
        DTD_E,
        Op::Import("#3", "/t", false),
        DEFAULT,
        Ok("<!DOCTYPE t [\n<!ENTITY e \"v\">\n]><t>&e;</t>"),
    ),
    // HTML and XML.
    (
        "html into xml",
        "html:<p class=x a>hi<input checked></p>",
        "<t/>",
        Op::Import("/p", "/t", true),
        DEFAULT,
        Ok("<t><p class=\"x\" a=\"\">hi<input checked=\"checked\"/></p></t>"),
    ),
    (
        "html name xml cannot write",
        "html:<p a:b=1></p>",
        "<t/>",
        Op::Import("/p", "/t", true),
        DEFAULT,
        Err(EditErrorKind::InvalidName),
    ),
    (
        "html comment xml cannot write",
        "html:<p><!--a--b--></p>",
        "<t/>",
        Op::Import("/p", "/t", true),
        DEFAULT,
        Err(EditErrorKind::InvalidValue),
    ),
    (
        "namespaced xml into html",
        "<r xmlns:p='urn:p'><p:a/></r>",
        "html:<div></div>",
        Op::Import("/r/p:a", "/div", true),
        DEFAULT,
        Err(EditErrorKind::Namespace),
    ),
    (
        "xml lang into html",
        "<r><a xml:lang='en'/></r>",
        "html:<div></div>",
        Op::Import("/r/a", "/div", true),
        DEFAULT,
        Err(EditErrorKind::Namespace),
    ),
    // Handles and kinds.
    (
        "document node",
        "<r/>",
        "<t/>",
        Op::Import("#0", "/t", true),
        DEFAULT,
        Err(EditErrorKind::WrongNodeKind),
    ),
    (
        "doctype",
        "<!DOCTYPE r><r/>",
        "<t/>",
        Op::Import("#1", "/t", true),
        DEFAULT,
        Err(EditErrorKind::WrongNodeKind),
    ),
    // Limits.
    (
        "created nodes",
        "<r><a/><b/></r>",
        "<t/>",
        Op::Import("/r", "/t", true),
        limit({
            let mut cfg = DEFAULT;
            cfg.max_created_nodes = 2;
            cfg
        }),
        Err(EditErrorKind::LimitExceeded(EditLimit::CreatedNodes)),
    ),
    (
        "created nodes at the ceiling",
        "<r><a/><b/></r>",
        "<t/>",
        Op::Import("/r", "/t", true),
        limit({
            let mut cfg = DEFAULT;
            cfg.max_created_nodes = 3;
            cfg
        }),
        Ok("<t><r><a/><b/></r></t>"),
    ),
    (
        "created attributes",
        "<r a='1' b='2'/>",
        "<t/>",
        Op::Import("/r", "/t", true),
        limit({
            let mut cfg = DEFAULT;
            cfg.max_created_attributes = 1;
            cfg
        }),
        Err(EditErrorKind::LimitExceeded(EditLimit::CreatedAttributes)),
    ),
    (
        "declarations count as created attributes",
        "<r xmlns:p='urn:p'><p:a k='1'/></r>",
        "<t/>",
        Op::Import("/r/p:a", "/t", true),
        limit({
            let mut cfg = DEFAULT;
            cfg.max_created_attributes = 1;
            cfg
        }),
        Err(EditErrorKind::LimitExceeded(EditLimit::CreatedAttributes)),
    ),
    (
        "owned bytes",
        "<r>a long text value</r>",
        "<t/>",
        Op::Import("/r", "/t", true),
        limit({
            let mut cfg = DEFAULT;
            cfg.max_owned_bytes = 8;
            cfg
        }),
        Err(EditErrorKind::LimitExceeded(EditLimit::OwnedBytes)),
    ),
    (
        "copies within share strings",
        "<r/>",
        "<r><a k='v'>a long text value</a></r>",
        Op::Copy("/r/a", "/r", true),
        limit({
            let mut cfg = DEFAULT;
            cfg.max_owned_bytes = 0;
            cfg
        }),
        Ok("<r><a k=\"v\">a long text value</a><a k=\"v\">a long text value</a></r>"),
    ),
    (
        "depth",
        "<r><a><b/></a></r>",
        "<t/>",
        Op::Import("/r", "/t", true),
        limit({
            let mut cfg = DEFAULT;
            cfg.max_depth = 2;
            cfg
        }),
        Err(EditErrorKind::LimitExceeded(EditLimit::Depth)),
    ),
    (
        "namespace work",
        "<r xmlns:p='urn:p'><p:a/></r>",
        "<t/>",
        Op::Import("/r/p:a", "/t", true),
        limit({
            let mut cfg = DEFAULT;
            cfg.max_namespace_work = 1;
            cfg
        }),
        Err(EditErrorKind::LimitExceeded(EditLimit::NamespaceWork)),
    ),
];

fn run(
    source: &OwnedDocument,
    destination: &mut OwnedDocument,
    op: Op,
    limits: EditLimits,
) -> Result<(), yaxml::EditError> {
    let (node, parent, copied) = match op {
        Op::Import(node, parent, deep) => {
            let (node, parent) = (at(source, node), at(destination, parent));
            let mut editor = destination.edit(limits)?;
            (node, parent, editor.import(source, node, deep)?)
        }
        Op::Copy(node, parent, deep) => {
            let (node, parent) = (at(destination, node), at(destination, parent));
            let mut editor = destination.edit(limits)?;
            (node, parent, editor.copy(node, deep)?)
        }
    };
    assert_ne!(node, copied);
    let copy = destination.node(copied).unwrap();
    assert_eq!((copy.parent().map(|n| n.id()), copy.range()), (None, None));
    destination
        .edit(limits)?
        .append_child(parent, copied)
        .map(drop)
}

#[test]
fn copies_match_their_tables() {
    for &(case, source, destination, op, limits, expected) in CASES {
        let source = parse(source);
        let mut document = parse(destination);
        let (before, nodes) = (written(&document), document.nodes().count());
        let result = run(&source, &mut document, op, limits);
        match expected {
            Ok(output) => {
                assert_eq!(result, Ok(()), "{case}");
                assert_eq!(written(&document), output, "{case}");
                let reparsed = parse(&written(&document));
                assert_eq!(written(&reparsed), output, "{case}: reparse");
            }
            Err(kind) => {
                assert_eq!(result.map_err(|error| error.kind), Err(kind), "{case}");
                assert_eq!(written(&document), before, "{case}: unchanged");
                assert_eq!(document.nodes().count(), nodes, "{case}: unchanged");
                // The failure published nothing: a later edit still works.
                let root = document.root_element().id();
                document
                    .edit(EditLimits::default())
                    .unwrap()
                    .append_comment(root, "ok")
                    .unwrap();
            }
        }
    }
}

#[test]
fn imported_ids_follow_the_destination() {
    let declared = "<!DOCTYPE t [<!ATTLIST a id ID #IMPLIED>]><t/>";
    let cases = [
        (
            "xml:id everywhere",
            "<r><a xml:id='x'/></r>",
            "<t/>",
            "x",
            true,
        ),
        (
            "dtd id in a declaring destination",
            "<r><a id='z'/></r>",
            declared,
            "z",
            true,
        ),
        (
            "dtd id elsewhere is plain",
            "<!DOCTYPE r [<!ATTLIST a id ID #IMPLIED>]><r><a id='z'/></r>",
            "<t/>",
            "z",
            false,
        ),
        (
            "html id into html",
            "html:<div><a id=h></a></div>",
            "html:<div></div>",
            "h",
            true,
        ),
    ];
    for (case, source, destination, id, found) in cases {
        let source = parse(source);
        let mut document = parse(destination);
        let node = source
            .nodes()
            .find(|node| node.name().is_some_and(|name| name.local() == "a"))
            .unwrap()
            .id();
        let copy = document
            .edit(EditLimits::default())
            .unwrap()
            .import(&source, node, true)
            .unwrap();
        let by_id = document.element_by_id(id).map(|element| element.id());
        assert_eq!(by_id, found.then_some(copy), "{case}");
    }
}

#[test]
fn import_fails_on_foreign_and_removed_handles() {
    let source = parse("<r><a/></r>");
    let other = parse("<o/>");
    let mut document = parse("<t/>");
    let (mut removed_source, a) = (parse("<r><a/></r>"), at(&source, "/r/a"));
    let removed = at(&removed_source, "/r/a");
    removed_source
        .edit(EditLimits::default())
        .unwrap()
        .remove_subtree(removed)
        .unwrap();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let invalid = Err(EditErrorKind::InvalidHandle);
    let foreign = other.root_element().id();
    assert_eq!(
        editor.import(&source, foreign, true).map_err(|e| e.kind),
        invalid
    );
    assert_eq!(
        editor
            .import(&removed_source, removed, true)
            .map_err(|e| e.kind),
        invalid
    );
    assert_eq!(editor.copy(a, true).map_err(|e| e.kind), invalid);
    assert!(editor.import(&source, a, true).is_ok());
}

#[test]
fn moving_between_documents_is_import_then_remove() {
    let mut source = parse("<r><a k='1'><b/></a><c/></r>");
    let mut destination = parse("<t/>");
    let (a, t) = (at(&source, "/r/a"), at(&destination, "/t"));
    {
        let mut from = source.edit(EditLimits::default()).unwrap();
        let mut to = destination.edit(EditLimits::default()).unwrap();
        let moved = to.import(from.document(), a, true).unwrap();
        from.remove_subtree(a).unwrap();
        to.append_child(t, moved).unwrap();
    }
    assert_eq!(written(&source), "<r><c/></r>");
    assert_eq!(written(&destination), "<t><a k=\"1\"><b/></a></t>");
    assert!(source.node(a).is_none());
}

#[test]
fn detached_and_entity_sources_copy() {
    // A detached subtree, and a node inside entity content.
    let mut source = parse("<!DOCTYPE r [<!ENTITY e '<i>v</i>'>]><r xmlns:p='urn:p'><p:a/>&e;</r>");
    let a = at(&source, "/r/p:a");
    source
        .edit(EditLimits::default())
        .unwrap()
        .unlink(a)
        .unwrap();
    let inner = source
        .nodes()
        .find(|node| node.name().is_some_and(|name| name.local() == "i"))
        .unwrap()
        .id();
    let mut document = parse("<t/>");
    let t = at(&document, "/t");
    let mut editor = document.edit(EditLimits::default()).unwrap();
    for node in [a, inner] {
        let copy = editor.import(&source, node, true).unwrap();
        editor.append_child(t, copy).unwrap();
    }
    assert_eq!(
        written(&document),
        "<t><p:a xmlns:p=\"urn:p\"/><i>v</i></t>"
    );
}

#[test]
fn copies_are_independent_of_their_source() {
    let mut document = parse("<r><a>x</a></r>");
    let (r, a) = (at(&document, "/r"), at(&document, "/r/a"));
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let copy = editor.copy(a, true).unwrap();
    let text = editor
        .document()
        .node(copy)
        .unwrap()
        .first_child()
        .unwrap()
        .id();
    editor.set_node_value(text, "y").unwrap();
    editor.append_child(r, copy).unwrap();
    editor.compact().unwrap();
    assert_eq!(written(&document), "<r><a>x</a><a>y</a></r>");
}

#[test]
fn try_clone_copies_the_whole_document() {
    let input =
        "<!DOCTYPE r [<!ATTLIST a id ID #IMPLIED><!ENTITY e 'v'>]><r><a id='k'>&e;</a><b/></r>";
    let options = {
        let mut cfg = yaxml::ParseOptions::default();
        cfg.base = Some("http://h/d.xml".to_owned());
        cfg
    };
    let mut document = OwnedDocument::parse_with(input.as_bytes(), options).unwrap();
    let b = at(&document, "/r/b");
    document
        .edit(EditLimits::default())
        .unwrap()
        .unlink(b)
        .unwrap();
    let clone = document.try_clone().unwrap();
    assert_eq!(written(&clone), written(&document));
    assert_eq!(clone.url(), Some("http://h/d.xml"));
    assert!(clone.dtd().is_some());
    assert!(clone.node(b).is_none());
    let found = clone.element_by_id("k").map(|node| node.path());
    assert_eq!(found, Some(Some("/r/a".to_owned())));
    let kinds = |document: &OwnedDocument| {
        document
            .nodes()
            .map(|n| (n.kind(), n.content()))
            .collect::<Vec<_>>()
    };
    assert_eq!(kinds(&clone), kinds(&document));
    // Editing the clone leaves the original alone.
    let mut clone = clone;
    let root = clone.root_element().id();
    clone
        .edit(EditLimits::default())
        .unwrap()
        .append_comment(root, "c")
        .unwrap();
    assert_ne!(written(&clone), written(&document));
    let html = parse("html:<p a>x</p>").try_clone().unwrap();
    assert!(html.is_html());
    assert!(!html.root_element().attribute_has_value(0));
}

#[test]
fn list_string_matches_node_list_get_string() {
    // (input, preorder position, substituted, escaped)
    let cases = [
        ("<r>a<b/>c</r>", 2, "ac", "ac"),
        ("<r>a<![CDATA[<]]>&amp;</r>", 2, "a<&", "a&lt;&amp;"),
        ("<r><b/>x</r>", 2, "x", "x"),
        ("<r>\u{e9}&gt;</r>", 2, "\u{e9}>", "&#xE9;&gt;"),
        (
            "<?xml version='1.0' encoding='UTF-8'?><r>\u{e9}</r>",
            2,
            "\u{e9}",
            "\u{e9}",
        ),
        (
            "<!DOCTYPE r [<!ENTITY e 'v<i>w</i>'>]><r>a&e;b</r>",
            3,
            "avwb",
            "a&e;b",
        ),
        ("<r/>", 0, "", ""),
        ("<r/>", 1, "", ""),
        ("<r><!--c--></r>", 2, "", ""),
    ];
    for (input, index, substituted, escaped) in cases {
        let borrowed = Document::parse(input).unwrap();
        let node = borrowed.nodes().nth(index).unwrap();
        assert_eq!(
            (node.list_string(true), node.list_string(false)),
            (substituted.to_owned(), escaped.to_owned()),
            "borrowed {input}"
        );
        let owned = parse(input);
        let node = owned.nodes().nth(index).unwrap();
        assert_eq!(
            (node.list_string(true), node.list_string(false)),
            (substituted.to_owned(), escaped.to_owned()),
            "owned {input}"
        );
    }
    let html = parse("html:<p>&eacute;&amp;</p>");
    let text = html
        .nodes()
        .find(|n| n.kind() == OwnedNodeKind::Text)
        .unwrap();
    assert_eq!(text.list_string(false), "\u{e9}&amp;");
}

#[test]
fn removing_a_fresh_copy_leaves_its_handle_stale() {
    // A detached copy's root has no parent; its removal must still end the
    // all-live preorder fast path, or the tombstone would resolve.
    let mut document = parse("<r><a/></r>");
    let a = at(&document, "/r/a");
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let copy = editor.copy(a, true).unwrap();
    editor.remove_subtree(copy).unwrap();
    assert!(editor.document().node(copy).is_none());
    let error = editor.unlink(copy).unwrap_err();
    assert_eq!(error.kind, EditErrorKind::InvalidHandle);
}
