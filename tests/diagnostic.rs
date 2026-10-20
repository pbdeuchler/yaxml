//! `Error::describe`, `LineIndex`, `Diagnostic::render`, node lines and the
//! validators' `describe`. Expected codes, positions and messages were
//! produced by libxml2 2.15.4 (`yaxml-harness/tooling/libxml2_errors.c` and `xmllint
//! --noout`) for these inputs; yaxml-harness/tests/diagnostic_oracle.rs compares whole corpora.

use yaxml::diagnostic::{Domain, Level};
use yaxml::options::RECOVER;
use yaxml::{
    Document, Error, ErrorKind, Limit, LineIndex, NodeKind, OwnedDocument, OwnedNodeKind,
    ParseOptions, TextReader, relaxng, schematron, xsd,
};

/// The first error: the one that stopped the parse, or a namespace error
/// it kept.
fn describe(input: &[u8]) -> yaxml::Diagnostic {
    let error = match OwnedDocument::parse_with(input, ParseOptions::default()) {
        Ok(document) => *document.errors().first().expect("malformed"),
        Err(error) => error,
    };
    error.describe(&LineIndex::new(input))
}

#[test]
fn describe_matches_libxml2() {
    // (input, code, line, column, message)
    let cases: &[(&[u8], u16, usize, usize, &str)] = &[
        // libxml2's nesting limit for content models, which the re-reader keeps too.
        (
            format!(
                "<!DOCTYPE r [<!ELEMENT r {}a{}>]><r/>",
                "(".repeat(300),
                ")".repeat(300)
            )
            .leak()
            .as_bytes(),
            114,
            1,
            283,
            "xmlParseElementChildrenContentDecl : depth 257 too deep, use XML_PARSE_HUGE",
        ),
        // Prefixes bound on ancestors, one of them prefixed itself.
        (
            b"<a xmlns:p=\"u\"><p:b xmlns:q=\"v\"><c q:x=\"1\" q:x=\"2\"/></p:b></a>",
            42,
            1,
            51,
            "Attribute q:x redefined",
        ),
        (
            b"<a xmlns:p=\"u\" xmlns:q=\"u\"><p:b><c p:x=\"1\" q:x=\"2\"/></p:b></a>",
            203,
            1,
            51,
            "Namespaced Attribute x in 'u' redefined",
        ),
        (
            b"<a>\n  <b></c>\n</a>",
            76,
            2,
            10,
            "Opening and ending tag mismatch: b line 2 and c",
        ),
        (
            b"<doc>",
            77,
            1,
            6,
            "Premature end of data in tag doc line 1",
        ),
        (b"<doc>\n<a b='1'", 65, 2, 9, "attributes construct error"),
        (b"<d a=\"1\" a=\"2\"/>", 42, 1, 15, "Attribute a redefined"),
        (
            b"<d xmlns:p=\"u\" xmlns:q=\"u\" p:a=\"1\" q:a=\"2\"/>",
            203,
            1,
            43,
            "Namespaced Attribute a in 'u' redefined",
        ),
        (
            b"<p:d/>",
            201,
            1,
            5,
            "Namespace prefix p on d is not defined",
        ),
        (
            b"<d p:a=\"1\"/>",
            201,
            1,
            11,
            "Namespace prefix p for a on d is not defined",
        ),
        (
            b"<d xmlns:x=\"\"/>",
            200,
            1,
            14,
            "xmlns:x: Empty XML namespace is not allowed",
        ),
        (
            b"<d xmlns:xml=\"urn:x\"/>",
            200,
            1,
            21,
            "xml namespace prefix mapped to wrong URI",
        ),
        (
            b"<d xmlns=\"http://www.w3.org/XML/1998/namespace\"/>",
            200,
            1,
            48,
            "xml namespace URI cannot be the default namespace",
        ),
        (b"<d>&foo;</d>", 26, 1, 9, "Entity 'foo' not defined"),
        (
            b"<d>&#xZ;</d>",
            6,
            1,
            7,
            "CharRef: invalid hexadecimal value",
        ),
        (b"<d>&#12a;</d>", 7, 1, 8, "CharRef: invalid decimal value"),
        (
            b"<d>&#1;</d>",
            9,
            1,
            8,
            "xmlParseCharRef: invalid xmlChar value 1",
        ),
        (
            b"<d>&#x110000;</d>",
            9,
            1,
            14,
            "xmlParseCharRef: character reference out of bounds",
        ),
        (b"<d>& x</d>", 68, 1, 5, "xmlParseEntityRef: no name"),
        (b"<d>&amp x</d>", 23, 1, 8, "EntityRef: expecting ';'"),
        (
            b"<d><!-- a -- b --></d>",
            80,
            1,
            11,
            "Double hyphen within comment: <!-- a ",
        ),
        (
            b"<d><!-- \xc3\xa9 -- b --></d>",
            80,
            1,
            13,
            "Comment must not contain '--' (double-hyphen)",
        ),
        (b"<d><!-- open", 45, 1, 13, "Comment not terminated"),
        (
            b"<d><!-- \x01 --></d>",
            9,
            1,
            9,
            "xmlParseComment: invalid xmlChar value 1",
        ),
        (b"<d><? pi?></d>", 46, 1, 6, "xmlParsePI : no target name"),
        (
            b"<d><?pi\x01?></d>",
            65,
            1,
            8,
            "ParsePI: PI pi space expected",
        ),
        (
            b"<d><?xml version=\"1.0\"?></d>",
            64,
            1,
            9,
            "XML declaration allowed only at the start of the document",
        ),
        (b"<d><?XmL x?></d>", 64, 1, 9, "Invalid PI name"),
        (
            b"<d><?a:b x?></d>",
            205,
            1,
            9,
            "colons are forbidden from PI names 'a:b'",
        ),
        (b"<d><?pi x", 47, 1, 10, "ParsePI: PI pi never end ..."),
        (b"<d><![CDATA[ab", 63, 1, 15, "CData section not finished\n"),
        (b"<d><![CDATA[\x01", 63, 1, 13, "Unregistered error message"),
        (
            b"<?xml version=\"2.0\"?><d/>",
            108,
            1,
            20,
            "Unsupported version '2.0'",
        ),
        (
            b"<?xml encoding=\"UTF-8\"?><d/>",
            96,
            1,
            7,
            "Malformed declaration expecting version",
        ),
        (
            b"<?xml version=\"1.0\" encoding=\"+\"?><d/>",
            79,
            1,
            31,
            "Invalid XML encoding name",
        ),
        (
            b"<?xml version=\"1.0\" standalone=\"maybe\"?><d/>",
            78,
            1,
            33,
            "standalone accepts only 'yes' or 'no'",
        ),
        (
            b"<?xml version=\"1.0\" x=\"1\"?><d/>",
            57,
            1,
            21,
            "parsing XML declaration: '?>' expected",
        ),
        (
            b"<?xml version=\"1.0\"standalone=\"yes\"?><d/>",
            65,
            1,
            20,
            "Blank needed here",
        ),
        (
            b"<?xml version=1.0?><d/>",
            33,
            1,
            15,
            "String not started expecting ' or \"",
        ),
        (
            b"<?xml version=\"1.0'?><d/>",
            34,
            1,
            19,
            "String not closed expecting \" or '",
        ),
        (
            b"<!DOCTYPEd><d/>",
            65,
            1,
            10,
            "Space required after 'DOCTYPE'",
        ),
        (
            b"<!DOCTYPE 1><d/>",
            68,
            1,
            11,
            "xmlParseDocTypeDecl : no DOCTYPE name !",
        ),
        (
            b"<!DOCTYPE d x><d/>",
            61,
            1,
            13,
            "DOCTYPE improperly terminated",
        ),
        (
            b"<!DOCTYPE d [<!ELEMENT d x>]><d/>",
            54,
            1,
            26,
            "xmlParseElementDecl: 'EMPTY', 'ANY' or '(' expected",
        ),
        (
            b"<!DOCTYPE d [<!ELEMENT d (#PCDATA|a)>]><d/>",
            52,
            1,
            36,
            "MixedContentDecl : '|' or ')*' expected",
        ),
        (
            b"<!DOCTYPE d [<!ELEMENT d (a,b|c)>]><d/>",
            66,
            1,
            30,
            "xmlParseElementChildrenContentDecl : ',' expected",
        ),
        (
            b"<!DOCTYPE d [<!ELEMENT d (a b)>]><d/>",
            55,
            1,
            29,
            "ContentDecl : ',' '|' or ')' expected",
        ),
        (
            b"<!DOCTYPE d [<!ELEMENT d ()>]><d/>",
            54,
            1,
            27,
            "ContentDecl : Name or '(' expected",
        ),
        (
            b"<!DOCTYPE d [<!ELEMENT d (a)) >]><d/>",
            73,
            1,
            29,
            "expected '>'",
        ),
        (
            b"<!DOCTYPE d [<!ATTLIST d a NAME #IMPLIED>]><d/>",
            50,
            1,
            28,
            "'(' required to start ATTLIST enumeration",
        ),
        (
            b"<!DOCTYPE d [<!ATTLIST d a (x,y) #IMPLIED>]><d/>",
            51,
            1,
            30,
            "')' required to finish ATTLIST enumeration",
        ),
        (
            b"<!DOCTYPE d [<!ATTLIST d a NOTATION x #IMPLIED>]><d/>",
            48,
            1,
            37,
            "NOTATION: Name expected here",
        ),
        (
            b"<!DOCTYPE d [<!ATTLIST d a CDATA #FIXED>]><d/>",
            65,
            1,
            40,
            "Space required after '#FIXED'",
        ),
        (
            b"<!DOCTYPE d [<!ATTLIST d a CDATA x>]><d/>",
            39,
            1,
            34,
            "AttValue: \" or ' expected",
        ),
        (
            b"<!DOCTYPE d [<!ENTITY e>]><d/>",
            65,
            1,
            24,
            "Space required after the entity name",
        ),
        (
            b"<!DOCTYPE d [<!ENTITY e SYSTEM>]><d/>",
            65,
            1,
            31,
            "Space required after 'SYSTEM'",
        ),
        (
            b"<!DOCTYPE d [<!ENTITY e PUBLIC \"a{\" \"b\">]><d/>",
            44,
            1,
            34,
            "Unfinished System or Public ID \" or ' expected",
        ),
        (
            b"<!DOCTYPE d [<!NOTATION n>]><d/>",
            65,
            1,
            26,
            "Space required after the NOTATION name'",
        ),
        (
            b"<!DOCTYPE d [ % x; ]><d/>",
            24,
            1,
            16,
            "PEReference: no name",
        ),
        (
            b"<!DOCTYPE d [ x ]><d/>",
            118,
            1,
            15,
            "Content error in the internal subset",
        ),
        (
            b"<!DOCTYPE d [<!Foo>]><d/>",
            118,
            1,
            14,
            "Content error in the internal subset",
        ),
        (
            b"<!DOCTYPE d [<!ENTITY e \"&f;\"><!ENTITY f \"&e;\">]><d>&e;</d>",
            89,
            1,
            4,
            "Detected an entity reference loop",
        ),
        (b"\n", 4, 2, 1, "Start tag expected, '<' not found"),
        (b"  x<d/>", 4, 1, 3, "Start tag expected, '<' not found"),
        (
            b"<d/>x",
            5,
            1,
            5,
            "Extra content at the end of the document",
        ),
        (
            b"<d/><e/>",
            5,
            1,
            5,
            "Extra content at the end of the document",
        ),
        (b"</d>", 68, 1, 2, "StartTag: invalid element name"),
        (
            b"<d>\xff</d>",
            81,
            1,
            4,
            "Invalid bytes in character encoding",
        ),
        (
            b"<d>a]]>b</d>",
            62,
            1,
            5,
            "Sequence ']]>' not allowed in content",
        ),
        (b"<d>\x01</d>", 9, 1, 4, "PCDATA invalid Char value 1"),
        (
            b"<d \x01/>",
            73,
            1,
            4,
            "Couldn't find end of Start Tag d line 1",
        ),
        (b"<d a=b/>", 39, 1, 6, "AttValue: \" or ' expected"),
        (
            b"<d a/>",
            41,
            1,
            5,
            "Specification mandates value for attribute a",
        ),
        (
            b"<d a=\"<\"/>",
            38,
            1,
            7,
            "Unescaped '<' not allowed in attributes values",
        ),
        (b"<d a=\"x", 40, 1, 8, "AttValue: ' expected"),
        (
            b"<d a=\"1\"b=\"2\"/>",
            65,
            1,
            9,
            "attributes construct error",
        ),
        (b"<d 1=\"1\"/>", 68, 1, 4, "error parsing attribute name"),
        (
            b"<d><a></b></d>",
            76,
            1,
            11,
            "Opening and ending tag mismatch: a line 1 and b",
        ),
        (
            b"<d></d >x",
            5,
            1,
            9,
            "Extra content at the end of the document",
        ),
    ];
    for &(input, code, line, column, message) in cases {
        let d = describe(input);
        let shown = String::from_utf8_lossy(input);
        assert_eq!(
            (d.code, d.line, d.column, d.message.as_str()),
            (code, line, column, message),
            "{shown}"
        );
        let expected = match code {
            200..=205 => (Domain::Namespace, Level::Error),
            81 => (Domain::Io, Level::Fatal),
            _ => (Domain::Parser, Level::Fatal),
        };
        assert_eq!((d.domain, d.level), expected, "{shown}");
    }
}

#[test]
fn describe_fills_libxml2_extras() {
    let mismatch = describe(b"<a>\n<b></c></a>");
    assert_eq!(
        (
            mismatch.str1.as_deref(),
            mismatch.str2.as_deref(),
            mismatch.int1
        ),
        (Some("b"), Some("c"), 2)
    );
    let duplicate = describe(b"<d p:a='1' p:a='2' xmlns:p='u'/>");
    assert_eq!(
        (
            duplicate.code,
            duplicate.str1.as_deref(),
            duplicate.str2.as_deref()
        ),
        (42, Some("p"), Some("a"))
    );
    let reference = describe(b"<d>&#1;</d>");
    assert_eq!((reference.int1, reference.str1), (1, None));
    assert!(!reference.entity && reference.element.is_none());
}

#[test]
fn describe_without_context_falls_back_to_the_kind() {
    // (kind, code, domain)
    let cases = [
        (ErrorKind::UnsupportedOption, 115, Domain::Parser),
        (ErrorKind::Allocation, 2, Domain::Parser),
        (ErrorKind::LimitExceeded(Limit::Depth), 114, Domain::Parser),
        (ErrorKind::ExternalResource, 1549, Domain::Io),
        (ErrorKind::XInclude, 1609, Domain::XInclude),
        (ErrorKind::Aborted, 111, Domain::Parser),
        (ErrorKind::InvalidState, 1, Domain::Parser),
        // Well-formed input around the offset: nothing to refine.
        (ErrorKind::Syntax, 68, Domain::Parser),
        (ErrorKind::MismatchedEndTag, 76, Domain::Parser),
    ];
    let input = b"<r>\n<a/>\n</r>";
    let lines = LineIndex::new(input);
    for (kind, code, domain) in cases {
        let d = Error { kind, offset: 5 }.describe(&lines);
        assert_eq!(
            (d.code, d.domain, d.line, d.column),
            (code, domain, 2, 2),
            "{kind:?}"
        );
        assert_eq!(kind.libxml2_code(), (domain, code), "{kind:?}");
        assert!(!d.message.is_empty());
    }
    // An offset past the input is clamped; an empty input has one line.
    let past = Error {
        kind: ErrorKind::UnexpectedEof,
        offset: 99,
    }
    .describe(&lines);
    assert_eq!((past.code, past.line, past.column), (5, 3, 5));
    let empty = Error {
        kind: ErrorKind::MissingRoot,
        offset: 0,
    }
    .describe(&LineIndex::new(b""));
    assert_eq!(
        (empty.code, empty.line, empty.message.as_str()),
        (4, 1, "Document is empty")
    );
}

#[test]
fn every_kind_has_a_stable_code() {
    let kinds = [
        (ErrorKind::InvalidUtf8, 81),
        (ErrorKind::InvalidUtf16, 81),
        (ErrorKind::UnmappedByte, 81),
        (ErrorKind::EncodingMismatch, 81),
        (ErrorKind::InvalidCharacter, 9),
        (ErrorKind::UnexpectedEof, 77),
        (ErrorKind::Syntax, 68),
        (ErrorKind::InvalidName, 68),
        (ErrorKind::MismatchedEndTag, 76),
        (ErrorKind::DuplicateAttribute, 42),
        (ErrorKind::Namespace, 201),
        (ErrorKind::InvalidReference, 8),
        (ErrorKind::UnknownEntity, 26),
        (ErrorKind::EntityLoop, 89),
        (ErrorKind::UnparsedEntity, 28),
        (ErrorKind::ExternalEntityInAttribute, 29),
        (ErrorKind::UnbalancedEntity, 85),
        (ErrorKind::InvalidDtd, 61),
        (ErrorKind::ExternalResource, 1549),
        (ErrorKind::XInclude, 1609),
        (ErrorKind::MissingRoot, 4),
        (ErrorKind::MultipleRoots, 5),
        (ErrorKind::TextOutsideRoot, 5),
        (ErrorKind::InvalidDeclaration, 57),
        (ErrorKind::UnsupportedEncoding, 32),
        (ErrorKind::UnsupportedOption, 115),
        (ErrorKind::LimitExceeded(Limit::InputBytes), 114),
        (ErrorKind::Allocation, 2),
        (ErrorKind::IdentifierExhausted, 114),
        (ErrorKind::Aborted, 111),
        (ErrorKind::InvalidState, 1),
    ];
    for (kind, code) in kinds {
        assert_eq!(kind.libxml2_code().1, code, "{kind:?}");
    }
}

#[test]
fn recovered_errors_describe_in_order() {
    let input = b"<r><a></b><c x='1' x='2'/></r>";
    let document = OwnedDocument::parse_with(input, ParseOptions::new(RECOVER)).unwrap();
    let lines = LineIndex::new(input);
    let found: Vec<_> = lines
        .describe_all(document.errors())
        .into_iter()
        .map(|d| (d.code, d.column))
        .collect();
    assert_eq!(found, [(76, 11), (42, 25)]);
    assert!(lines.describe_all(&[]).is_empty());
    // libxml2 reports an unclosed element at the end only while well-formed.
    let input = b"<a>\n<b>\n</a>";
    let document = OwnedDocument::parse_with(input, ParseOptions::new(RECOVER)).unwrap();
    assert_eq!(document.errors().len(), 2);
    let codes: Vec<_> = LineIndex::new(input)
        .describe_all(document.errors())
        .iter()
        .map(|d| d.code)
        .collect();
    assert_eq!(codes, [76]);
}

#[test]
fn render_matches_xmllint() {
    // (input, xmllint's report through the context lines)
    let cases: &[(&[u8], &str)] = &[
        (
            b"<a>\n  <b></c>\n</a>",
            "f:2: parser error : Opening and ending tag mismatch: b line 2 and c\n  <b></c>\n         ^\n",
        ),
        (
            b"<p:d/>",
            "f:1: namespace error : Namespace prefix p on d is not defined\n<p:d/>\n    ^\n",
        ),
        (
            b"<d>\xff</d>",
            "f:1: I/O error : Invalid bytes in character encoding\nBytes: 0xFF 0x3C 0x2F 0x64\n<d>\n   ^\n",
        ),
        (
            b"<d>a]]>b</d>",
            "f:1: parser error : Sequence ']]>' not allowed in content\n<d>a]]>b</d>\n   ^\n",
        ),
        (
            b"<d>\r\nabc]]>x</d>",
            "f:2: parser error : Sequence ']]>' not allowed in content\n<d>\n   ^\n",
        ),
        (
            b"<d>\xc3\xa9]]>x</d>",
            "f:1: parser error : Sequence ']]>' not allowed in content\n<d>é]]>x</d>\n     ^\n",
        ),
        (
            b"<d>\t\xc3\xa9\t<a></b></d>",
            "f:1: parser error : Opening and ending tag mismatch: a line 1 and b\n<d>\té\t<a></b></d>\n   \t  \t       ^\n",
        ),
        (
            b"\xff\xfe<\x00d\x00>\x00\x01\x00<\x00/\x00d\x00>\x00",
            "f:1: parser error : PCDATA invalid Char value 1\n<d>\x01</d>\n   ^\n",
        ),
        (
            b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>\n<d>\xe9<a></b></d>",
            "f:2: parser error : Opening and ending tag mismatch: a line 2 and b\n<d>é<a></b></d>\n            ^\n",
        ),
        (
            b"<doc>",
            "f:1: parser error : Premature end of data in tag doc line 1\n<doc>\n     ^\n",
        ),
        (
            b"<!DOCTYPE d [<!ENTITY e \"&f;\"><!ENTITY f \"&e;\">]><d>&e;</d>",
            "Entity: line 1: parser error : Detected an entity reference loop\n",
        ),
    ];
    for &(input, expected) in cases {
        let lines = LineIndex::new(input);
        assert_eq!(
            describe(input).render("f", &lines).as_bytes(),
            expected.as_bytes(),
            "{}",
            String::from_utf8_lossy(input)
        );
    }
    let mut long = b"<d>".to_vec();
    long.extend([b'x'; 100]);
    long.extend(b"<a></b></d>");
    let window = format!("{}<a></b>\n{}^\n", "x".repeat(73), " ".repeat(79));
    assert!(
        describe(&long)
            .render("f", &LineIndex::new(&long))
            .ends_with(&window)
    );
}

#[test]
fn line_index_counts_as_libxml2() {
    // (input, offset, line, column)
    let cases: &[(&[u8], usize, usize, usize)] = &[
        (b"ab\ncd", 0, 1, 1),
        (b"ab\ncd", 4, 2, 2),
        (b"ab\r\ncd", 5, 2, 2),
        (b"\xc3\xa9x\n", 2, 1, 2),
        (b"\xef\xbb\xbf<r/>", 3, 1, 1),
        (b"ab", 99, 1, 3),
        (b"", 0, 1, 1),
        // UTF-16LE with a BOM, BE without: offsets stay input bytes.
        (b"\xff\xfe<\x00\n\x00x\x00", 6, 2, 1),
        (b"\x00<\x00?\x00\n\x00x", 8, 2, 2),
        // A declared single-byte charset.
        (
            b"<?xml version='1.0' encoding='ISO-8859-1'?><r>\xa9\xa9</r>",
            48,
            1,
            49,
        ),
    ];
    for &(input, offset, line, column) in cases {
        let lines = LineIndex::new(input);
        assert_eq!(
            (lines.line(offset), lines.column(offset)),
            (line, column),
            "{input:?} at {offset}"
        );
    }
}

const TREE: &str = "<r\n a='1'>text\n<!--c\n--><?p\n?><![CDATA[x]]>&e;<e/></r>";

#[test]
fn node_lines_follow_xml_get_line_no() {
    let input = format!("<!DOCTYPE r [<!ENTITY e 'v'>]>\n{TREE}");
    let lines = LineIndex::new(input.as_bytes());
    let document = Document::parse(&input).unwrap();
    let borrowed: Vec<_> = document
        .root_element()
        .descendants()
        .map(|n| n.line(&lines))
        .collect();
    let owned = OwnedDocument::parse(&input).unwrap();
    let kept: Vec<_> = owned
        .root_element()
        .descendants()
        .map(|n| n.line(&lines))
        .collect();
    // r (tag ends on line 3), text (ends on 4), comment (5), PI (6), CDATA and
    // the entity reference (the PI's line), its text, e.
    let expected = [3, 4, 5, 6, 6, 6, 6, 6].map(Some);
    assert_eq!(
        borrowed.iter().take(5).collect::<Vec<_>>(),
        expected.iter().take(5).collect::<Vec<_>>()
    );
    assert_eq!(borrowed, kept);
    let kinds: Vec<_> = document
        .root_element()
        .children()
        .map(|n| matches!(n.kind(), NodeKind::Cdata(_)))
        .collect();
    assert!(kinds.contains(&true));
    assert!(
        owned
            .root_element()
            .children()
            .any(|n| n.kind() == OwnedNodeKind::EntityRef)
    );
    // The document node has no line, nor nodes without source text.
    assert_eq!(document.root().line(&lines), None);
    assert_eq!(document.root_element().line(&LineIndex::new(b"")), None);
}

#[test]
fn text_reader_reports_its_parsing_point() {
    let input = b"<r>\n  <a/>\n</r>";
    let lines = LineIndex::new(input);
    let mut reader = TextReader::new(&input[..]);
    assert_eq!(
        (reader.line_number(&lines), reader.column_number(&lines)),
        (1, 1)
    );
    let mut seen = vec![];
    while reader.read().unwrap() {
        seen.push((reader.line_number(&lines), reader.column_number(&lines)));
    }
    // Each read consumes at least the node; the point never moves back.
    assert!(seen.windows(2).all(|pair| pair[0] <= pair[1]), "{seen:?}");
    assert!(seen[0] >= (1, 4), "{seen:?}");
    assert_eq!(
        (reader.line_number(&lines), reader.column_number(&lines)),
        (3, 5)
    );
}

#[test]
fn validators_describe_as_libxml2() {
    let input = "<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]>\n<r\n>\n<b/></r>";
    let lines = LineIndex::new(input.as_bytes());
    let document = OwnedDocument::parse(input).unwrap();
    let found: Vec<_> = document
        .validate()
        .iter()
        .map(|d| d.describe(&lines))
        .collect();
    assert!(!found.is_empty());
    for d in &found {
        assert_eq!(
            (d.domain, d.level, d.column),
            (Domain::Valid, Level::Error, 0)
        );
        assert!(d.render("f", &lines).starts_with(&format!(
            "f:{}: element {}: validity error : ",
            d.line,
            d.element.as_deref().unwrap()
        )));
    }
    assert!(
        found
            .iter()
            .any(|d| d.element.as_deref() == Some("r") && d.line == 3)
    );

    let none = || -> Box<dyn yaxml::Resolver> { Box::new(|_: yaxml::Request<'_>| Ok(None)) };
    let rng = b"<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><empty/></element>";
    let schema = relaxng::Schema::parse(rng, "s.rng", none(), &Default::default()).unwrap();
    let instance = "<r>\n<x/></r>";
    let lines = LineIndex::new(instance.as_bytes());
    let found = schema.validate(&Document::parse(instance).unwrap());
    let d = found[0].describe(&lines);
    assert_eq!(
        (d.domain, d.code, d.element.as_deref()),
        (Domain::RelaxNgValidity, found[0].code as u16, Some("x"))
    );
    assert_eq!(d.line, 2);

    let xsd = b"<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema'><xs:element name='r'><xs:complexType/></xs:element></xs:schema>";
    let schema = xsd::Schema::parse(xsd, "s.xsd").unwrap();
    let found = schema.validate(&Document::parse(instance).unwrap());
    let d = found[0].describe(&lines);
    assert_eq!(
        (d.domain, d.element.as_deref(), d.line),
        (Domain::SchemasValidity, None, 1)
    );
    assert!(
        d.render("f", &lines)
            .starts_with("f:1: Schemas validity error : ")
    );

    let sct = b"<s:schema xmlns:s='http://purl.oclc.org/dsdl/schematron'><s:pattern id='p'><s:rule context='x'><s:assert test='0'>no</s:assert></s:rule></s:pattern></s:schema>";
    let schema = schematron::Schema::parse(sct, "s.sct", none(), &Default::default()).unwrap();
    let validation = schema.validate(
        &Document::parse(instance).unwrap(),
        schematron::OUT_ERROR,
        instance.as_bytes(),
    );
    let error = validation.output.iter().find_map(|output| match output {
        schematron::Output::Error(error) => Some(error),
        schematron::Output::Text(_) => None,
    });
    let error = error.expect("an assert error");
    let d = error.describe();
    assert_eq!(
        (d.domain, d.code, d.line, d.element.as_deref()),
        (Domain::SchematronValidity, 4000, 2, Some("x"))
    );
    // Without an offset there is no line, element or context.
    let code = yaxml::valid::Code::UndeclaredEntity;
    let unplaced = yaxml::valid::Diagnostic::<()> {
        code,
        node: None,
        offset: None,
        message: "m\n".into(),
    };
    let d = unplaced.describe(&lines);
    assert_eq!(
        (d.code, d.line, d.element.as_deref(), d.message.as_str()),
        (26, 0, None, "m")
    );
    assert_eq!(d.render("f", &lines), "f: validity error : m\n");
}

/// Errors in children of an element whose attribute references a large
/// internal entity: each description re-reads the ancestors' attributes for
/// namespace bindings and expands the entity again, which the parser did
/// once. The shared budget bounds the total; once it is spent, the later
/// errors get the context-free mapping.
#[test]
fn describe_all_is_bounded_across_errors() {
    for (entity, errors) in [(400, 20), (4000, 100), (40_000, 100)] {
        let input = format!(
            "<!DOCTYPE r [<!ENTITY e '{}'>]><r a='&e;'>{}</r>",
            "x".repeat(entity),
            "<c x='' x=''/>".repeat(errors)
        );
        let parsed =
            OwnedDocument::parse_with(input.as_bytes(), ParseOptions::new(RECOVER)).unwrap();
        assert_eq!(parsed.errors().len(), errors);
        let lines = LineIndex::new(input.as_bytes());
        let (found, used, ceiling) = lines.describe_counted(parsed.errors());
        assert_eq!(found.len(), errors);
        assert!(
            used <= ceiling && ceiling == 8 * input.len() + 64 * 1024,
            "{used} {ceiling}"
        );
        let full = found
            .iter()
            .take_while(|d| d.message == "Attribute x redefined")
            .count();
        // Each description costs about the entity: the budget decides how many.
        assert_eq!(
            full == errors,
            entity * errors < ceiling / 2,
            "{entity} {errors} {full}"
        );
        assert!(full >= 1);
        for d in &found[full..] {
            assert_eq!(
                (d.code, d.column, d.message.as_str()),
                (42, 0, "DuplicateAttribute")
            );
        }
    }
}

#[test]
fn owned_line_index_matches_borrowed() {
    let utf16: Vec<u8> = "<a>\n<b/>\u{e9}\n</a>"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let inputs: [&[u8]; 4] = [
        b"",
        b"<a>\n  <b></c>\n</a>",
        b"\xef\xbb\xbf<a>\r\n<b/></a>",
        &utf16,
    ];
    for input in inputs {
        let (borrowed, owned) = (LineIndex::new(input), LineIndex::owned(input.to_vec()));
        assert_eq!(owned.input(), input);
        for offset in 0..=input.len() + 1 {
            assert_eq!(
                owned.line(offset),
                borrowed.line(offset),
                "{input:?} @ {offset}"
            );
            assert_eq!(
                owned.column(offset),
                borrowed.column(offset),
                "{input:?} @ {offset}"
            );
        }
    }
    let input = "<a>\n  <b></c>\n</a>";
    let error = Document::parse(input).unwrap_err();
    let owned = LineIndex::owned(input.as_bytes().to_vec());
    assert_eq!(
        error.describe(&owned),
        error.describe(&LineIndex::new(input.as_bytes()))
    );
}
