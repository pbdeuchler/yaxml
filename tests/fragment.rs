//! `Editor::parse_content`: content parsed in a node's context
//! (`xmlParseInNodeContext`, `xmlCtxtParseContent`,
//! `xmlParseBalancedChunkMemory`), its errors as libxml2 codes, and the
//! limits on parsing and inserting it; `Editor::parse_external_entity`
//! (`xmlParseCtxtExternalEntity`).
#![forbid(unsafe_code)]

use yaxml::options::{DTDVALID, NOBLANKS, NOENT, NSCLEAN, RECOVER, XINCLUDE};
use yaxml::{
    EditErrorKind, EditLimit, EditLimits, Error, ErrorKind, Fragment, Limit, Limits, LineIndex,
    OwnedDocument, OwnedNodeId, OwnedNodeKind, ParseOptions, save,
};

const DTD: &str = "<!DOCTYPE r [<!ENTITY e 'ent'><!ENTITY m '<p:x/>'>\
<!ATTLIST a d CDATA 'default'>]>";

fn saved(document: &OwnedDocument) -> String {
    let saved = save::to_vec(document, &save::Options::default()).unwrap();
    let saved = String::from_utf8(saved).unwrap();
    let body = saved.split_once("?>\n").map_or(&*saved, |(_, body)| body);
    body.strip_suffix('\n').unwrap().to_owned()
}

/// The `index`th node of `document` in preorder.
fn nth(document: &OwnedDocument, index: usize) -> OwnedNodeId {
    document.nodes().nth(index).unwrap().id()
}

/// Parses `content` in the `context`th node of `xml`, appends the nodes to
/// `parent` (a preorder index), and returns the saved document.
fn parse_and_append(
    xml: &str,
    context: usize,
    parent: usize,
    content: &str,
    flags: u32,
) -> Result<(String, Vec<OwnedNodeKind>, usize), EditErrorKind> {
    let mut document = OwnedDocument::parse(xml).unwrap();
    let (context, parent) = (nth(&document, context), nth(&document, parent));
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let options = ParseOptions::new(flags);
    let Fragment { nodes, errors, .. } = editor
        .parse_content(context, content.as_bytes(), options)
        .map_err(|error| error.kind)?;
    let kinds = nodes
        .iter()
        .map(|&node| {
            let node = editor.document().node(node).unwrap();
            assert!(node.parent().is_none(), "parsed nodes are detached");
            node.kind()
        })
        .collect();
    for node in nodes {
        editor.append_child(parent, node).unwrap();
    }
    Ok((saved(&document), kinds, errors.len()))
}

/// Context XML, context and parent preorder indices, content, flags, the
/// saved document's end and the parsed node kinds.
type Case<'a> = (
    &'a str,
    usize,
    usize,
    &'a str,
    u32,
    &'a str,
    &'a [OwnedNodeKind],
);

#[test]
fn content_parses_in_context() {
    use OwnedNodeKind::{Cdata, Comment, Element, EntityRef, ProcessingInstruction, Text};
    let ns = "<r xmlns:p='urn:p' xmlns='urn:d'><c xmlns=''/></r>";
    let entities = format!("{DTD}<r xmlns:p='urn:p'/>");
    #[rustfmt::skip]
    let cases: &[Case<'_>] = &[
        // Several elements and top-level text.
        ("<r/>", 1, 1, "a<b/>c<d/>", 0, "<r>a<b/>c<d/></r>", &[Text, Element, Text, Element]),
        ("<r/>", 1, 1, "", 0, "<r/>", &[]),
        ("<r/>", 1, 1, "<!--k--><?pi d?><![CDATA[<]]>", 0, "<r><!--k--><?pi d?><![CDATA[<]]></r>",
            &[Comment, ProcessingInstruction, Cdata]),
        // In-scope prefixes and the default namespace, without new declarations.
        (ns, 1, 1, "<p:a/><b/>", 0, "<r xmlns:p=\"urn:p\" xmlns=\"urn:d\"><c xmlns=\"\"/><p:a/><b/></r>",
            &[Element, Element]),
        // `xmlns=""` in scope at the context.
        (ns, 2, 2, "<b/>", 0, "<r xmlns:p=\"urn:p\" xmlns=\"urn:d\"><c xmlns=\"\"><b/></c></r>", &[Element]),
        // Redeclarations and NSCLEAN.
        (ns, 1, 1, "<p:a xmlns:p='urn:p'/>", 0,
            "<r xmlns:p=\"urn:p\" xmlns=\"urn:d\"><c xmlns=\"\"/><p:a xmlns:p=\"urn:p\"/></r>", &[Element]),
        (ns, 1, 1, "<p:a xmlns:p='urn:p'/>", NSCLEAN,
            "<r xmlns:p=\"urn:p\" xmlns=\"urn:d\"><c xmlns=\"\"/><p:a/></r>", &[Element]),
        // A text context parses in its element; the document node in none.
        ("<r xmlns:p='urn:p'>t</r>", 2, 1, "<p:a/>", 0, "<r xmlns:p=\"urn:p\">t<p:a/></r>", &[Element]),
        // The DTD's entities, kept or substituted; no attribute defaults.
        (&entities, 2, 2, "&e;<a>&m;</a>", 0,
            "<r xmlns:p=\"urn:p\">&e;<a>&m;</a></r>", &[EntityRef, Element]),
        (&entities, 2, 2, "&e;<a>&m;</a>", NOENT,
            "<r xmlns:p=\"urn:p\">ent<a><p:x/></a></r>", &[Text, Element]),
        // NOBLANKS drops blanks between top-level markup, but not at the end.
        ("<r/>", 1, 1, " <a/> <b/> ", NOBLANKS, "<r><a/><b/> </r>", &[Element, Element, Text]),
        // Appended Text merges with the parent's last Text.
        ("<r>t</r>", 1, 1, "u", 0, "<r>tu</r>", &[Text]),
    ];
    for &(xml, context, parent, content, flags, expected, kinds) in cases {
        let (saved, parsed, errors) = parse_and_append(xml, context, parent, content, flags)
            .unwrap_or_else(|error| panic!("{content:?} in {xml}: {error:?}"));
        assert!(saved.ends_with(expected), "{content:?} in {xml}: {saved}");
        assert_eq!(parsed, kinds, "{content:?}");
        assert_eq!(errors, 0);
    }
}

#[test]
fn the_document_node_parses_a_balanced_chunk() {
    // `xmlParseBalancedChunkMemory`: the document's entities, no bindings.
    let mut document = OwnedDocument::parse(&format!("{DTD}<r xmlns:p='urn:p'/>")).unwrap();
    let root = document.root().id();
    let r = document.root_element().id();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let chunk = editor
        .parse_content(root, b"<a>&e;</a>", ParseOptions::default())
        .unwrap();
    editor.append_child(r, chunk.nodes[0]).unwrap();
    // The document node binds no prefix: `p` is a namespace error, kept.
    let unbound = editor.parse_content(root, b"<p:a/>", ParseOptions::default());
    let unbound = unbound.unwrap();
    assert_eq!(
        unbound.errors,
        [Error {
            kind: ErrorKind::Namespace,
            offset: 1
        }]
    );
    let name = editor
        .document()
        .node(unbound.nodes[0])
        .unwrap()
        .name()
        .unwrap();
    assert_eq!((name.qualified(), name.namespace()), ("p:a", None));
    assert!(saved(&document).ends_with("<r xmlns:p=\"urn:p\"><a>&e;</a></r>"));
}

#[test]
fn entity_content_parses_in_the_document() {
    // libxml2's parent of entity content is the declaration: a context in
    // it parses in the document, without the reference's bindings. Entity
    // content is read-only.
    let mut document = OwnedDocument::parse(&format!("{DTD}<r xmlns:p='urn:p'>&e;</r>")).unwrap();
    let text = document
        .nodes()
        .find(|node| node.text() == Some("ent"))
        .unwrap()
        .id();
    let r = document.root_element().id();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let unbound = editor.parse_content(text, b"<p:a/>", ParseOptions::default());
    assert_eq!(
        unbound.unwrap().errors,
        [Error {
            kind: ErrorKind::Namespace,
            offset: 1
        }]
    );
    let fragment = editor
        .parse_content(text, b"<a/>", ParseOptions::default())
        .unwrap();
    assert_eq!(
        editor
            .append_child(text, fragment.nodes[0])
            .unwrap_err()
            .kind,
        EditErrorKind::ProtectedNode
    );
    editor.append_child(r, fragment.nodes[0]).unwrap();
    assert!(saved(&document).ends_with("<r xmlns:p=\"urn:p\">&e;<a/></r>"));
}

#[test]
fn moved_content_is_declared_where_it_goes() {
    let mut document = OwnedDocument::parse("<r><s xmlns:p='urn:p'/><t/></r>").unwrap();
    let (s, t) = (nth(&document, 2), nth(&document, 3));
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let fragment = editor
        .parse_content(s, b"<p:a/>", ParseOptions::default())
        .unwrap();
    editor.append_child(t, fragment.nodes[0]).unwrap();
    assert!(saved(&document).ends_with("<t><p:a xmlns:p=\"urn:p\"/></t></r>"));
}

#[test]
fn parsed_ids_are_found() {
    let mut document = OwnedDocument::parse("<r/>").unwrap();
    let r = document.root_element().id();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let fragment = editor
        .parse_content(r, b"<a xml:id='x'/>", ParseOptions::default())
        .unwrap();
    editor.append_child(r, fragment.nodes[0]).unwrap();
    assert_eq!(
        document.element_by_id("x").map(|node| node.id()),
        Some(fragment.nodes[0])
    );
}

/// libxml2's code for malformed content, through `LineIndex::content`.
#[test]
fn malformed_content_reports_libxml2_codes() {
    let entities = format!("{DTD}<r/>");
    #[rustfmt::skip]
    let cases: &[(&str, &str, ErrorKind, usize, u16)] = &[
        ("<r/>", "<a/></r>", ErrorKind::MismatchedEndTag, 4, 85),
        ("<r/>", "</r>", ErrorKind::MismatchedEndTag, 0, 85),
        ("<r/>", "<a>", ErrorKind::UnexpectedEof, 3, 77),
        ("<r/>", "<a></b>", ErrorKind::MismatchedEndTag, 3, 76),
        ("<r/>", "&undefined;", ErrorKind::UnknownEntity, 0, 26),
        ("<r/>", "<?xml version='1.0'?>", ErrorKind::InvalidDeclaration, 0, 64),
        ("<r/>", "<!DOCTYPE r>", ErrorKind::Syntax, 0, 68),
        ("<r/>", "]]>", ErrorKind::Syntax, 0, 62),
        ("<r/>", "&bad", ErrorKind::InvalidReference, 0, 23),
        ("<r/>", "<a b='1' b='2'/>", ErrorKind::DuplicateAttribute, 0, 42),
        (&entities, "&undefined;", ErrorKind::UnknownEntity, 0, 26),
        (&entities, "<a>&e;</b>", ErrorKind::MismatchedEndTag, 6, 76),
    ];
    for &(xml, content, kind, offset, code) in cases {
        let mut document = OwnedDocument::parse(xml).unwrap();
        let context = document.root_element().id();
        let before = saved(&document);
        let mut editor = document.edit(EditLimits::default()).unwrap();
        let error = editor
            .parse_content(context, content.as_bytes(), ParseOptions::default())
            .unwrap_err();
        assert_eq!(
            (error.kind, error.value_offset),
            (EditErrorKind::Parse(kind), Some(offset)),
            "{content:?}"
        );
        let diagnostic = Error { kind, offset }.describe(&LineIndex::content(content.as_bytes()));
        assert_eq!(diagnostic.code, code, "{content:?}: {}", diagnostic.message);
        assert_eq!(saved(&document), before, "a failed parse changed nothing");
    }
}

#[test]
fn a_document_index_describes_content_as_a_document() {
    // The same error read as a document is extra content after the root.
    let error = Error {
        kind: ErrorKind::MismatchedEndTag,
        offset: 4,
    };
    assert_eq!(error.describe(&LineIndex::new(b"<a/></r>")).code, 5);
    assert_eq!(error.describe(&LineIndex::content(b"<a/></r>")).code, 85);
}

#[test]
fn recovery_keeps_what_was_parsed() {
    #[rustfmt::skip]
    let cases: &[(&str, &str, usize)] = &[
        ("<a>x</b>y", "<r><a>x</a>y</r>", 1),
        ("t</r>ignored", "<r>t</r>", 1),
        ("<a><b>", "<r><a><b/></a></r>", 1),
        ("&undefined;<c/>", "<r>&undefined;<c/></r>", 1),
    ];
    for &(content, expected, errors) in cases {
        let (saved, _, recovered) = parse_and_append("<r/>", 1, 1, content, RECOVER).unwrap();
        assert!(saved.ends_with(expected), "{content:?}: {saved}");
        assert_eq!(recovered, errors, "{content:?}");
    }
}

#[test]
fn unusable_contexts_and_options_fail() {
    let mut document = OwnedDocument::parse("<!DOCTYPE r><r/>").unwrap();
    let doctype = nth(&document, 1);
    let foreign = OwnedDocument::parse("<f/>").unwrap().root().id();
    let r = document.root_element().id();
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let parse = |editor: &mut yaxml::Editor<'_>, node, flags| {
        let options = ParseOptions::new(flags);
        editor
            .parse_content(node, b"<a/>", options)
            .unwrap_err()
            .kind
    };
    let unsupported = EditErrorKind::Parse(ErrorKind::UnsupportedOption);
    assert_eq!(parse(&mut editor, doctype, 0), EditErrorKind::WrongNodeKind);
    assert_eq!(parse(&mut editor, foreign, 0), EditErrorKind::InvalidHandle);
    assert_eq!(parse(&mut editor, r, XINCLUDE), unsupported);
    assert_eq!(parse(&mut editor, r, DTDVALID), unsupported);
    assert_eq!(parse(&mut editor, r, 1 << 30), unsupported);
    // HTML has its own fragment parser.
    #[cfg(feature = "html")]
    {
        let mut page = yaxml::html::parse(b"<p>x", yaxml::html::Options::DEFAULT)
            .unwrap()
            .document;
        let body = page.root_element().first_child().unwrap().id();
        let mut editor = page.edit(EditLimits::default()).unwrap();
        assert_eq!(parse(&mut editor, body, 0), EditErrorKind::WrongNodeKind);
    }
}

#[test]
fn parse_budgets_and_edit_limits_apply() {
    let deep = b"<a><a><a/></a></a>";
    let mut document = OwnedDocument::parse("<r/>").unwrap();
    let r = document.root_element().id();
    let before = saved(&document);
    let mut editor = document.edit(EditLimits::default()).unwrap();
    for flags in [0, RECOVER] {
        let options = ParseOptions::new(flags).with_limits(Limits::DEFAULT.max_depth(2));
        let error = editor.parse_content(r, deep, options).unwrap_err();
        let depth = EditErrorKind::Parse(ErrorKind::LimitExceeded(Limit::Depth));
        assert_eq!(error.kind, depth);
        let options = ParseOptions::new(flags).with_limits(Limits::DEFAULT.max_input_bytes(4));
        let error = editor.parse_content(r, deep, options).unwrap_err();
        let input = EditErrorKind::Parse(ErrorKind::LimitExceeded(Limit::InputBytes));
        assert_eq!(error.kind, input);
    }
    let tight = |limits: EditLimits| {
        let mut document = OwnedDocument::parse("<r/>").unwrap();
        let r = document.root_element().id();
        let mut editor = document.edit(limits).unwrap();
        let error = editor.parse_content(r, deep, ParseOptions::default());
        (error.map(drop).unwrap_err().kind, document.nodes().count())
    };
    let limit = |limit| (EditErrorKind::LimitExceeded(limit), 2);
    let nodes = EditLimits::DEFAULT.max_created_nodes(2);
    assert_eq!(tight(nodes), limit(EditLimit::CreatedNodes));
    let depth = EditLimits::DEFAULT.max_depth(2);
    assert_eq!(tight(depth), limit(EditLimit::Depth));
    let bytes = EditLimits::DEFAULT.max_owned_bytes(1);
    assert_eq!(tight(bytes), limit(EditLimit::OwnedBytes));
    assert_eq!(saved(&document), before);
}

#[test]
fn external_entities_parse_like_xml_parse_ctxt_external_entity() {
    // Recorded with xmlParseCtxtExternalEntity (document context: prefixes
    // are unbound), appended to `r`.
    type Row<'a> = (&'a [u8], Result<(&'a str, usize), ErrorKind>);
    let rows: [Row; 11] = [
        (b"a<p:b>x</p:b>c", Ok(("<r>a<p:b>x</p:b>c</r>", 1))),
        (b"<?xml encoding=\"UTF-8\"?>t<e/>", Ok(("<r>t<e/></r>", 0))),
        (
            b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>caf\xe9",
            Ok(("<r>caf&#xE9;</r>", 0)),
        ),
        // No version check: the parser context has no document version.
        (
            b"<?xml version=\"1.1\" encoding=\"UTF-8\"?>v",
            Ok(("<r>v</r>", 0)),
        ),
        (b"\xff\xfe<\0e\0/\0>\0", Ok(("<r><e/></r>", 0))),
        (b"", Ok(("<r/>", 0))),
        (b"&amp;<q:x/>", Ok(("<r>&amp;<q:x/></r>", 1))),
        // A text declaration needs an encoding and has no standalone.
        (
            b"<?xml version=\"1.0\"?>v",
            Err(ErrorKind::InvalidDeclaration),
        ),
        (
            b"<?xml encoding=\"UTF-8\" standalone=\"yes\"?>x",
            Err(ErrorKind::InvalidDeclaration),
        ),
        (
            b"<?xml  encoding=\"bogus\"?>x",
            Err(ErrorKind::UnsupportedEncoding),
        ),
        (b"<a>", Err(ErrorKind::UnexpectedEof)),
    ];
    for (input, expected) in rows {
        let mut document = OwnedDocument::parse("<r/>").unwrap();
        let (root, r) = (document.root().id(), document.root_element().id());
        let mut editor = document.edit(EditLimits::default()).unwrap();
        let parsed = editor.parse_external_entity(root, input, ParseOptions::default());
        let found = parsed.map_err(|error| error.kind).map(|fragment| {
            for node in fragment.nodes {
                editor.append_child(r, node).unwrap();
            }
            fragment.errors.len()
        });
        let found = found.map(|errors| (saved(&document), errors));
        let expected = expected.map(|(xml, errors)| (xml.to_owned(), errors));
        assert_eq!(found, expected.map_err(EditErrorKind::Parse), "{input:?}");
    }
}
