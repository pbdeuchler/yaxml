//! RELAX NG: table-driven schema and instance cases, and (ignored, with
//! `YAXML_LIBXML2_SOURCE`) libxml2's `test/relaxng` suite, James Clark's
//! spec test suite and the XML Schema datatypes suite.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use yaxml::relaxng::{Code, Diagnostic, Limits, Schema, SchemaCode};
use yaxml::{
    BufferedReader, Document, Event, Node, NodeKind, OwnedDocument, ParseOptions, PushParser,
    Reader, Request,
};

const RNG: &str = "http://relaxng.org/ns/structure/1.0";
const XSD: &str = "http://www.w3.org/2001/XMLSchema-datatypes";

fn none() -> Box<dyn yaxml::Resolver> {
    Box::new(|_: Request<'_>| Ok(None))
}

fn schema(text: &str) -> Result<Schema, yaxml::relaxng::SchemaErrors> {
    Schema::parse(text.as_bytes(), "s.rng", none(), &Limits::default())
}

/// Wraps a pattern in an element with the RELAX NG and XSD declarations.
fn wrap(body: &str) -> String {
    format!(r#"<element name="r" xmlns="{RNG}" datatypeLibrary="{XSD}">{body}</element>"#)
}

/// Code, offset and message of each diagnostic: what DOM and streaming
/// validation must agree on.
type Summary = Vec<(Code, Option<usize>, String)>;

fn summary<I>(found: &[Diagnostic<I>]) -> Summary {
    (found.iter())
        .map(|d| (d.code, d.offset, d.message.clone()))
        .collect()
}

/// Streaming validation over `BufferedReader` events, with the DTD;
/// `None` if the document is not well-formed.
fn streamed(schema: &Schema, xml: &[u8], options: ParseOptions) -> Option<Summary> {
    let mut reader = BufferedReader::with_options(xml, options).unwrap();
    let mut stream = schema.stream();
    while let Some((event, origin)) = reader.read_event().ok()? {
        let doctype = matches!(event, Event::Doctype(_));
        stream.event(&event, origin);
        if let Some(dtd) = reader.dtd().filter(|_| doctype) {
            stream.set_dtd(dtd);
        }
    }
    Some(summary(stream.diagnostics()))
}

/// Streaming validation over the slice `Reader` and over `PushParser` fed
/// one byte at a time.
fn other_streams(schema: &Schema, xml: &str) -> [Summary; 2] {
    let (mut sliced, mut pushed) = (schema.stream(), schema.stream());
    let mut reader = Reader::new(xml.as_bytes()).unwrap();
    while let Some(event) = reader.read_event().unwrap() {
        sliced.event(&event, reader.event_origin().unwrap());
    }
    let mut parser = PushParser::new();
    let mut feed = |event: &Event<'_>, origin| {
        pushed.event(event, origin);
        ControlFlow::Continue(())
    };
    for byte in xml.as_bytes().chunks(1) {
        parser.feed(byte, &mut feed).unwrap();
    }
    parser.finish(&mut feed).unwrap();
    [summary(sliced.diagnostics()), summary(pushed.diagnostics())]
}

fn codes(schema: &Schema, xml: &str) -> Vec<Code> {
    let document = Document::parse(xml).unwrap();
    let owned = OwnedDocument::parse(xml).unwrap();
    let borrowed = summary(&schema.validate(&document));
    assert_eq!(borrowed, summary(&schema.validate(&owned)), "{xml}");
    let options = ParseOptions::default();
    assert_eq!(
        Some(&borrowed),
        streamed(schema, xml.as_bytes(), options).as_ref(),
        "{xml}"
    );
    for found in other_streams(schema, xml) {
        assert_eq!(borrowed, found, "{xml}");
    }
    borrowed.into_iter().map(|(code, ..)| code).collect()
}

/// Elements `a` with an `xsd:ID` and `b` with `xsd:IDREFS` attribute.
const IDS: &str = "<zeroOrMore><choice><element name='a'><attribute name='id'><data type='ID'/></attribute></element><element name='b'><attribute name='ref'><data type='IDREFS'/></attribute></element></choice></zeroOrMore>";

/// (pattern inside `<element name="r">`, instance, expected codes)
const PATTERNS: &[(&str, &str, &[Code])] = &[
    ("<empty/>", "<r/>", &[]),
    ("<empty/>", "<r>x</r>", &[Code::UnexpectedText]),
    ("<empty/>", "<r> </r>", &[]),
    ("<empty/>", "<r><a/></r>", &[Code::UnexpectedElement]),
    ("<text/>", "<r>any<!-- c -->thing</r>", &[]),
    ("<notAllowed/>", "<r/>", &[Code::UnexpectedElement]),
    ("<element name='a'><empty/></element>", "<r><a/></r>", &[]),
    (
        "<element name='a'><empty/></element>",
        "<r/>",
        &[Code::MissingElement],
    ),
    (
        "<element name='a'><empty/></element>",
        "<r><b/></r>",
        &[Code::ElementName, Code::MissingElement],
    ),
    (
        "<element name='a' ns='urn:x'><empty/></element>",
        "<r><a/></r>",
        &[Code::ElementNoNamespace, Code::MissingElement],
    ),
    (
        "<element name='a'><empty/></element>",
        "<r><a xmlns='urn:x'/></r>",
        &[Code::ElementExtraNamespace, Code::MissingElement],
    ),
    (
        "<element name='a' ns='urn:x'><empty/></element>",
        "<r><a xmlns='urn:y'/></r>",
        &[Code::ElementWrongNamespace, Code::MissingElement],
    ),
    ("<attribute name='a'/>", "<r a='v'/>", &[]),
    ("<attribute name='a'/>", "<r/>", &[Code::Attributes]),
    (
        "<attribute name='a'/>",
        "<r a='1' b='2'/>",
        &[Code::InvalidAttribute],
    ),
    (
        "<attribute name='a'><data type='int'/></attribute>",
        "<r a='x'/>",
        &[Code::Attributes],
    ),
    (
        "<attribute name='a'><data type='int'/></attribute>",
        "<r a=' 12 '/>",
        &[],
    ),
    ("<optional><attribute name='a'/></optional>", "<r/>", &[]),
    (
        "<zeroOrMore><attribute><anyName/></attribute></zeroOrMore>",
        "<r a='' xmlns:p='urn:p' p:b=''/>",
        &[],
    ),
    (
        "<group><element name='a'><empty/></element><element name='b'><empty/></element></group>",
        "<r><a/><b/></r>",
        &[],
    ),
    (
        "<group><element name='a'><empty/></element><element name='b'><empty/></element></group>",
        "<r><b/><a/></r>",
        &[Code::ElementName, Code::MissingElement],
    ),
    (
        "<interleave><element name='a'><empty/></element><element name='b'><empty/></element></interleave>",
        "<r><b/><a/></r>",
        &[],
    ),
    (
        "<interleave><element name='a'><empty/></element><element name='b'><empty/></element></interleave>",
        "<r><b/><b/></r>",
        &[Code::ElementName, Code::MissingElement],
    ),
    (
        "<choice><element name='a'><empty/></element><element name='b'><empty/></element></choice>",
        "<r><b/></r>",
        &[],
    ),
    (
        "<choice><element name='a'><empty/></element><element name='b'><empty/></element></choice>",
        "<r><c/></r>",
        &[Code::UnexpectedElement, Code::MissingElement],
    ),
    (
        "<optional><element name='a'><empty/></element></optional>",
        "<r/>",
        &[],
    ),
    (
        "<zeroOrMore><element name='a'><empty/></element></zeroOrMore>",
        "<r><a/><a/><a/></r>",
        &[],
    ),
    (
        "<oneOrMore><element name='a'><empty/></element></oneOrMore>",
        "<r/>",
        &[Code::MissingElement],
    ),
    (
        "<oneOrMore><element name='a'><empty/></element></oneOrMore>",
        "<r><a/><a/></r>",
        &[],
    ),
    (
        "<mixed><element name='a'><empty/></element></mixed>",
        "<r>x<a/>y</r>",
        &[],
    ),
    (
        "<element name='a'><empty/></element>",
        "<r>x<a/></r>",
        &[Code::UnexpectedText],
    ),
    (
        "<list><oneOrMore><data type='int'/></oneOrMore></list>",
        "<r> 1 2\n3 </r>",
        &[],
    ),
    (
        "<list><oneOrMore><data type='int'/></oneOrMore></list>",
        "<r>1 x</r>",
        &[Code::List],
    ),
    (
        "<list><data type='int'/><data type='int'/></list>",
        "<r>1</r>",
        &[Code::List],
    ),
    ("<data type='int'/>", "<r>7</r>", &[]),
    ("<data type='int'/>", "<r>7.5</r>", &[Code::Datatype]),
    ("<data type='int'/>", "<r/>", &[Code::Datatype]),
    (
        "<data type='string'><param name='maxLength'>3</param></data>",
        "<r>abcd</r>",
        &[Code::Datatype],
    ),
    (
        "<data type='string'><param name='pattern'>[a-c]+</param><param name='minLength'>2</param></data>",
        "<r>ab</r>",
        &[],
    ),
    (
        "<data type='string'><param name='pattern'>[a-c]+</param><param name='minLength'>2</param></data>",
        "<r>a</r>",
        &[Code::Datatype],
    ),
    (
        "<data type='integer'><param name='minInclusive'>3</param><param name='maxExclusive'>5</param></data>",
        "<r>4</r>",
        &[],
    ),
    (
        "<data type='integer'><param name='minInclusive'>3</param><param name='maxExclusive'>5</param></data>",
        "<r>5</r>",
        &[Code::Datatype],
    ),
    (
        "<data type='decimal'><param name='totalDigits'>3</param><param name='fractionDigits'>1</param></data>",
        "<r>12.5</r>",
        &[],
    ),
    (
        "<data type='decimal'><param name='totalDigits'>3</param></data>",
        "<r>1234</r>",
        &[Code::Datatype],
    ),
    (
        "<data type='token'><except><value>no</value></except></data>",
        "<r>yes</r>",
        &[],
    ),
    (
        "<data type='token'><except><value>no</value></except></data>",
        "<r> no </r>",
        &[Code::Datatype],
    ),
    ("<data type='QName'/>", "<r xmlns:p='urn:p'>p:x</r>", &[]),
    ("<data type='QName'/>", "<r>q:x</r>", &[Code::Datatype]),
    // An unprefixed QName in an `xmlns=""` scope has no namespace, as with no default at all.
    ("<value type='QName'>x</value>", "<r>x</r>", &[]),
    ("<value type='QName'>x</value>", "<r xmlns=''>x</r>", &[]),
    ("<data type='QName'/>", "<r xmlns=''>x</r>", &[]),
    (
        "<value type='QName' ns='urn:d'>x</value>",
        "<r xmlns=''>x</r>",
        &[Code::Value],
    ),
    ("<value>a  b</value>", "<r> a b </r>", &[]),
    (
        "<value type='string' datatypeLibrary=''>a</value>",
        "<r> a</r>",
        &[Code::Value],
    ),
    ("<value type='integer'>10</value>", "<r>+010</r>", &[]),
    (
        "<value type='QName' xmlns:p='urn:p'>p:x</value>",
        "<r xmlns:q='urn:p'>q:x</r>",
        &[],
    ),
    (
        "<value type='QName' xmlns:p='urn:p'>p:x</value>",
        "<r xmlns:q='urn:z'>q:x</r>",
        &[Code::Value],
    ),
    // Namespace bindings end with their element; inner ones shadow outer ones.
    (
        "<element name='a'><data type='QName'/></element><element name='b'><data type='QName'/></element>",
        "<r><a xmlns:p='urn:p'>p:x</a><b>p:x</b></r>",
        &[Code::Datatype],
    ),
    (
        "<element name='a'><value type='QName' xmlns:p='urn:q'>p:x</value></element>",
        "<r xmlns:p='urn:p'><a xmlns:p='urn:q'>p:x</a></r>",
        &[],
    ),
    // A disallowed element's subtree is skipped.
    (
        "<element name='a'><empty/></element>",
        "<r><x xmlns:p='urn:p'><y>t</y><a/></x><a/></r>",
        &[Code::ElementName],
    ),
    (IDS, "<r><a id='x'/><b ref='x x'/></r>", &[]),
    (IDS, "<r><b ref='x'/><a id='x'/></r>", &[]),
    (IDS, "<r><a id='x'/><a id='x'/></r>", &[Code::DuplicateId]),
    (IDS, "<r><b ref='x y'/><a id='x'/></r>", &[Code::UnknownId]),
    (
        "<choice><value>a</value><value>b</value></choice>",
        "<r>b</r>",
        &[],
    ),
    (
        "<element><anyName/><empty/></element>",
        "<r><x:y xmlns:x='urn:x'/></r>",
        &[],
    ),
    (
        "<element><anyName><except><name>a</name></except></anyName><empty/></element>",
        "<r><a/></r>",
        &[Code::UnexpectedElement, Code::MissingElement],
    ),
    (
        "<element><nsName ns='urn:x'/><empty/></element>",
        "<r><y xmlns='urn:x'/></r>",
        &[],
    ),
    (
        "<element><nsName ns='urn:x'><except><name>y</name></except></nsName><empty/></element>",
        "<r><y xmlns='urn:x'/></r>",
        &[Code::UnexpectedElement, Code::MissingElement],
    ),
    (
        "<element><choice><name>a</name><name>b</name></choice><empty/></element>",
        "<r><b/></r>",
        &[],
    ),
    (
        "<element xmlns:p='urn:p'><name>p:a</name><empty/></element>",
        "<r><a xmlns='urn:p'/></r>",
        &[],
    ),
    (
        "<element name='p:a' xmlns:p='urn:p'><empty/></element>",
        "<r><q:a xmlns:q='urn:p'/></r>",
        &[],
    ),
    (
        "<attribute name='a' ns='urn:p'/>",
        "<r xmlns:p='urn:p' p:a=''/>",
        &[],
    ),
    (
        "<attribute name='a'/>",
        "<r xmlns='urn:x' a=''/>",
        &[Code::ElementExtraNamespace],
    ),
];

#[test]
fn patterns_validate_both_doms() {
    let failures: Vec<_> = PATTERNS
        .iter()
        .filter_map(|&(body, xml, expected)| {
            let schema = schema(&wrap(body)).unwrap_or_else(|e| panic!("{body}: {e}"));
            let found = codes(&schema, xml);
            (found != expected).then(|| format!("{body} | {xml}: {found:?} != {expected:?}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// (schema, error code)
const INCORRECT: &[(&str, SchemaCode)] = &[
    ("<thisIsJunk/>", SchemaCode::Empty),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><bogus/></element>",
        SchemaCode::UnknownConstruct,
    ),
    (
        "<element name='r' foo='1' xmlns='http://relaxng.org/ns/structure/1.0'><empty/></element>",
        SchemaCode::UnknownAttribute,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'/>",
        SchemaCode::ElementNoContent,
    ),
    (
        "<element xmlns='http://relaxng.org/ns/structure/1.0'/>",
        SchemaCode::ElementEmpty,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'>text<empty/></element>",
        SchemaCode::TextExpected,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><group/></element>",
        SchemaCode::EmptyConstruct,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><empty><empty/></empty></element>",
        SchemaCode::EmptyNotEmpty,
    ),
    (
        "<element name='1r' xmlns='http://relaxng.org/ns/structure/1.0'><empty/></element>",
        SchemaCode::ElementName,
    ),
    (
        "<element name='p:r' xmlns='http://relaxng.org/ns/structure/1.0'><empty/></element>",
        SchemaCode::PrefixUndefined,
    ),
    (
        "<element xmlns='http://relaxng.org/ns/structure/1.0'><foo/><empty/></element>",
        SchemaCode::UnknownConstruct,
    ),
    (
        "<element xmlns='http://relaxng.org/ns/structure/1.0'><anyName><except><anyName/></except></anyName><empty/></element>",
        SchemaCode::PatAnyNameExceptAnyName,
    ),
    (
        "<element xmlns='http://relaxng.org/ns/structure/1.0'><nsName><except><nsName/></except></nsName><empty/></element>",
        SchemaCode::PatNsNameExceptNsName,
    ),
    (
        "<element xmlns='http://relaxng.org/ns/structure/1.0'><nsName><except><anyName/></except></nsName><empty/></element>",
        SchemaCode::PatNsNameExceptAnyName,
    ),
    (
        "<element xmlns='http://relaxng.org/ns/structure/1.0'><anyName><except><name>a</name></except><except><name>b</name></except></anyName><empty/></element>",
        SchemaCode::ExceptMultiple,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><attribute name='xmlns'/></element>",
        SchemaCode::XmlnsName,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><attribute name='a'><text/><text/></attribute></element>",
        SchemaCode::AttributeChildren,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><ref name='x'/></element>",
        SchemaCode::RefNoDef,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><ref/></element>",
        SchemaCode::RefNoName,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><parentRef name='x'/></start></grammar>",
        SchemaCode::ParentRefNoParent,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><define name='x'><element name='r'><empty/></element></define></grammar>",
        SchemaCode::GrammarNoStart,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><element name='r'><empty/></element></start><foo/></grammar>",
        SchemaCode::UnknownConstruct,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><element name='r'><empty/></element></start><element name='x'><empty/></element></grammar>",
        SchemaCode::GrammarContent,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><ref name='a'/></start><define name='a'><ref name='b'/></define><define name='b'><ref name='a'/></define></grammar>",
        SchemaCode::RefCycle,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><ref name='a'/></start><define name='a'><element name='a'><empty/></element></define><define name='a'><element name='b'><empty/></element></define></grammar>",
        SchemaCode::NeedCombine,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start combine='choice'><element name='a'><empty/></element></start><start combine='interleave'><element name='b'><empty/></element></start></grammar>",
        SchemaCode::StartChoiceAndInterleave,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><ref name='a'/></start><define name='a' combine='choice'><element name='a'><empty/></element></define><define name='a' combine='interleave'><element name='b'><empty/></element></define></grammar>",
        SchemaCode::DefChoiceAndInterleave,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start combine='both'><element name='a'><empty/></element></start></grammar>",
        SchemaCode::UnknownCombine,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><element name='a'><empty/></element></start><define name='1'><empty/></define></grammar>",
        SchemaCode::InvalidDefineName,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><element name='a'><empty/></element></start><define name='x'><ref name='y'/></define></grammar>",
        SchemaCode::RefNoDef,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><data/></element>",
        SchemaCode::TypeMissing,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><data type='int'/></element>",
        SchemaCode::TypeNotFound,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0' datatypeLibrary='urn:unknown'><data type='int'/></element>",
        SchemaCode::UnknownTypeLib,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0' datatypeLibrary='relative'><text/></element>",
        SchemaCode::UriNotAbsolute,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0' datatypeLibrary='http://x/#f'><text/></element>",
        SchemaCode::UriFragment,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><data type='string'><param name='length'>1</param></data></element>",
        SchemaCode::ParamForbidden,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0' datatypeLibrary='http://www.w3.org/2001/XMLSchema-datatypes'><data type='int'><param name='bogus'>1</param></data></element>",
        SchemaCode::TypeValue,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0' datatypeLibrary='http://www.w3.org/2001/XMLSchema-datatypes'><data type='int'><param name='enumeration'>1</param></data></element>",
        SchemaCode::TypeValue,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0' datatypeLibrary='http://www.w3.org/2001/XMLSchema-datatypes'><data type='int'><param>1</param></data></element>",
        SchemaCode::ParamNameMissing,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0' datatypeLibrary='http://www.w3.org/2001/XMLSchema-datatypes'><data type='int'><except><value>1</value></except><param name='x'>1</param></data></element>",
        SchemaCode::DataContent,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0' datatypeLibrary='http://www.w3.org/2001/XMLSchema-datatypes'><value type='int'>x</value></element>",
        SchemaCode::InvalidValue,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><value><empty/></value></element>",
        SchemaCode::TextExpected,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><externalRef/></element>",
        SchemaCode::MissingHref,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><attribute/></element>",
        SchemaCode::AttributeEmpty,
    ),
    (
        "<element xmlns='http://relaxng.org/ns/structure/1.0'><choice/><empty/></element>",
        SchemaCode::ChoiceEmpty,
    ),
    (
        "<element xmlns='http://relaxng.org/ns/structure/1.0'><choice><text/></choice><empty/></element>",
        SchemaCode::ChoiceContent,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><ref name='d'/></start><define name='d'></define></grammar>",
        SchemaCode::DefineEmpty,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><ref name='d'/></start><define><empty/></define></grammar>",
        SchemaCode::DefineNameMissing,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><ref name='d'/></start><define name='d'><text/></define><define name='1x'><empty/></define></grammar>",
        SchemaCode::InvalidDefineName,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><data type='string'><except/></data></element>",
        SchemaCode::ExceptNoContent,
    ),
    (
        "<element xmlns='http://relaxng.org/ns/structure/1.0'><anyName><except/></anyName><empty/></element>",
        SchemaCode::ExceptEmpty,
    ),
    (
        "<element xmlns='http://relaxng.org/ns/structure/1.0'><anyName><name>a</name></anyName><empty/></element>",
        SchemaCode::ExceptMissing,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'></grammar>",
        SchemaCode::GrammarEmpty,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><interleave/></element>",
        SchemaCode::InterleaveNoContent,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><mixed/></element>",
        SchemaCode::EmptyConstruct,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0' datatypeLibrary='h ttp://x'><empty/></element>",
        SchemaCode::InvalidUri,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><notAllowed><empty/></notAllowed></element>",
        SchemaCode::NotAllowedNotEmpty,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><text><empty/></text></element>",
        SchemaCode::TextHasChild,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><element name='r'><grammar><start><parentRef/></start></grammar></element></start></grammar>",
        SchemaCode::ParentRefNoName,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><element name='r'><grammar><start><parentRef name='1x'/></start></grammar></element></start><define name='1x'><empty/></define></grammar>",
        SchemaCode::ParentRefNameInvalid,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><element name='r'><grammar><start><parentRef name='d'><empty/></parentRef></start></grammar></element></start><define name='d'><empty/></define></grammar>",
        SchemaCode::ParentRefNotEmpty,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><ref name='1x'/></start><define name='1x'><empty/></define></grammar>",
        SchemaCode::RefNameInvalid,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><ref name='d'><empty/></ref></start><define name='d'><element name='r'><empty/></element></define></grammar>",
        SchemaCode::RefNotEmpty,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start/></grammar>",
        SchemaCode::StartEmpty,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><attribute name='a' ns='http://www.w3.org/2000/xmlns'><text/></attribute></element>",
        SchemaCode::XmlNs,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><attribute><nsName ns='http://www.w3.org/2000/xmlns'/><text/></attribute></element>",
        SchemaCode::XmlNs,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><empty name='x'/></element>",
        SchemaCode::ForbiddenAttribute,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><externalRef href='x.rng#f'/></element>",
        SchemaCode::UriFragment,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><externalRef href='missing.rng'/></element>",
        SchemaCode::ExternalRefFailure,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><include href='missing.rng'/></grammar>",
        SchemaCode::IncludeFailure,
    ),
    // Section 7.1 prohibited paths.
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><attribute name='a'><element name='b'><empty/></element></attribute></element>",
        SchemaCode::PatAttrElem,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><attribute name='a'><attribute name='b'/></attribute></element>",
        SchemaCode::PatAttrAttr,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><oneOrMore><group><attribute name='a'/><element name='b'><empty/></element></group></oneOrMore></element>",
        SchemaCode::PatOneMoreGroupAttr,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><oneOrMore><interleave><attribute name='a'/><element name='b'><empty/></element></interleave></oneOrMore></element>",
        SchemaCode::PatOneMoreInterleaveAttr,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><list><list><text/></list></list></element>",
        SchemaCode::PatListList,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><list><element name='a'><empty/></element></list></element>",
        SchemaCode::PatListElem,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><list><attribute name='a'/></list></element>",
        SchemaCode::PatListAttr,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><list><text/></list></element>",
        SchemaCode::PatListText,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><list><interleave><value>a</value><value>b</value></interleave></list></element>",
        SchemaCode::PatListInterleave,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><data type='string'><except><attribute name='a'/></except></data></element>",
        SchemaCode::PatDataExceptAttr,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><data type='string'><except><element name='a'><empty/></element></except></data></element>",
        SchemaCode::PatDataExceptElem,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><data type='string'><except><empty/></except></data></element>",
        SchemaCode::PatDataExceptEmpty,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><data type='string'><except><group><value>a</value><value>b</value></group></except></data></element>",
        SchemaCode::PatDataExceptGroup,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><data type='string'><except><text/></except></data></element>",
        SchemaCode::PatDataExceptText,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><data type='string'><except><list><value>a</value></list></except></data></element>",
        SchemaCode::PatDataExceptList,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><data type='string'><except><oneOrMore><value>a</value></oneOrMore></except></data></element>",
        SchemaCode::PatDataExceptOneMore,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><data type='string'><except><interleave><value>a</value><value>b</value></interleave></except></data></element>",
        SchemaCode::PatDataExceptInterleave,
    ),
    (
        "<attribute name='a' xmlns='http://relaxng.org/ns/structure/1.0'/>",
        SchemaCode::PatStartAttr,
    ),
    (
        "<text xmlns='http://relaxng.org/ns/structure/1.0'/>",
        SchemaCode::PatStartText,
    ),
    (
        "<empty xmlns='http://relaxng.org/ns/structure/1.0'/>",
        SchemaCode::PatStartEmpty,
    ),
    (
        "<data type='string' xmlns='http://relaxng.org/ns/structure/1.0'/>",
        SchemaCode::PatStartData,
    ),
    (
        "<value xmlns='http://relaxng.org/ns/structure/1.0'>a</value>",
        SchemaCode::PatStartValue,
    ),
    (
        "<list xmlns='http://relaxng.org/ns/structure/1.0'><value>a</value></list>",
        SchemaCode::PatStartList,
    ),
    (
        "<oneOrMore xmlns='http://relaxng.org/ns/structure/1.0'><element name='a'><empty/></element></oneOrMore>",
        SchemaCode::PatStartOneMore,
    ),
    (
        "<group xmlns='http://relaxng.org/ns/structure/1.0'><element name='a'><empty/></element><element name='b'><empty/></element></group>",
        SchemaCode::PatStartGroup,
    ),
    (
        "<interleave xmlns='http://relaxng.org/ns/structure/1.0'><element name='a'><empty/></element><element name='b'><empty/></element></interleave>",
        SchemaCode::PatStartInterleave,
    ),
    // Sections 7.2 to 7.4.
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><data type='string'/><element name='a'><empty/></element></element>",
        SchemaCode::ElemContentError,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><attribute name='a'/><attribute name='a'/></element>",
        SchemaCode::GroupAttrConflict,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><interleave><attribute name='a'/><oneOrMore><attribute><anyName/></attribute></oneOrMore></interleave></element>",
        SchemaCode::AttrConflict,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><attribute><anyName/></attribute></element>",
        SchemaCode::AnyNameAttrAncestor,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><attribute><nsName/></attribute></element>",
        SchemaCode::NsNameAttrAncestor,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><interleave><element name='a'><empty/></element><element name='a'><text/></element></interleave></element>",
        SchemaCode::ElemTextConflict,
    ),
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><interleave><text/><text/></interleave></element>",
        SchemaCode::ElemTextConflict,
    ),
];

#[test]
fn incorrect_schemas_are_rejected() {
    let failures: Vec<_> = INCORRECT
        .iter()
        .filter_map(|&(text, expected)| match schema(text) {
            Ok(_) => Some(format!("accepted: {text}")),
            Err(errors) if errors.0[0].code != expected => Some(format!(
                "{text}: {:?} != {expected:?} ({errors})",
                errors.0[0].code
            )),
            Err(_) => None,
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Simplification rules that must not reject correct schemas.
const CORRECT: &[(&str, &str, bool)] = &[
    // Unreachable definitions are not checked against section 7.
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><element name='r'><empty/></element></start><define name='u'><attribute name='a'><element name='b'><empty/></element></attribute></define></grammar>",
        "<r/>",
        true,
    ),
    // Annotations and divs.
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0' xmlns:a='urn:a'><a:doc>x</a:doc><div><start a:x='1'><ref name='r'/></start></div><div><define name='r'><element name='r'><empty/></element></define></div></grammar>",
        "<r/>",
        true,
    ),
    // combine="choice" and "interleave".
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><element name='r'><ref name='c'/><ref name='i'/></element></start><define name='c'><element name='a'><empty/></element></define><define name='c' combine='choice'><element name='b'><empty/></element></define><define name='i' combine='interleave'><element name='x'><empty/></element></define><define name='i' combine='interleave'><element name='y'><empty/></element></define></grammar>",
        "<r><b/><y/><x/></r>",
        true,
    ),
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><element name='r'><ref name='c'/><ref name='i'/></element></start><define name='c'><element name='a'><empty/></element></define><define name='c' combine='choice'><element name='b'><empty/></element></define><define name='i' combine='interleave'><element name='x'><empty/></element></define><define name='i' combine='interleave'><element name='y'><empty/></element></define></grammar>",
        "<r><a/><b/><y/><x/></r>",
        false,
    ),
    // Nested grammars and parentRef.
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><element name='r'><grammar><start><parentRef name='x'/></start><define name='x'><element name='inner'><empty/></element></define></grammar></element></start><define name='x'><element name='outer'><empty/></element></define></grammar>",
        "<r><outer/></r>",
        true,
    ),
    // Recursive elements.
    (
        "<grammar xmlns='http://relaxng.org/ns/structure/1.0'><start><ref name='n'/></start><define name='n'><element name='n'><zeroOrMore><ref name='n'/></zeroOrMore></element></define></grammar>",
        "<n><n><n/></n><n/></n>",
        true,
    ),
    // ns inheritance and datatypeLibrary inheritance.
    (
        "<element name='r' ns='urn:a' xmlns='http://relaxng.org/ns/structure/1.0' datatypeLibrary='http://www.w3.org/2001/XMLSchema-datatypes'><element name='c'><data type='int'/></element><attribute name='a'/></element>",
        "<r xmlns='urn:a' a='1'><c>3</c></r>",
        true,
    ),
    (
        "<element name='r' ns='urn:a' xmlns='http://relaxng.org/ns/structure/1.0'><element name='c' ns=''><empty/></element></element>",
        "<r xmlns='urn:a'><c xmlns=''/></r>",
        true,
    ),
    // notAllowed propagation leaves the other alternatives.
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><choice><notAllowed/><empty/></choice></element>",
        "<r/>",
        true,
    ),
    // Attributes in a oneOrMore with an anyName.
    (
        "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><zeroOrMore><attribute><anyName/></attribute></zeroOrMore></element>",
        "<r a='1' b='2'/>",
        true,
    ),
];

#[test]
fn correct_schemas_follow_simplification() {
    let failures: Vec<_> = CORRECT
        .iter()
        .filter_map(|&(text, xml, valid)| match schema(text) {
            Err(errors) => Some(format!("rejected {text}: {errors}")),
            Ok(schema) if codes(&schema, xml).is_empty() != valid => {
                Some(format!("{text} | {xml}: expected valid={valid}"))
            }
            Ok(_) => None,
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn map_resolver(files: HashMap<String, String>) -> Box<dyn yaxml::Resolver> {
    Box::new(move |request: Request<'_>| {
        Ok(files.get(request.uri).map(|text| text.clone().into_bytes()))
    })
}

#[test]
fn include_and_external_ref_load_through_the_resolver() {
    let files = HashMap::from([
        (
            "dir/lib.rng".to_owned(),
            format!(
                "<grammar xmlns='{RNG}'><start><ref name='a'/></start><define name='a'><element name='a'><externalRef href='leaf.rng'/></element></define><define name='b'><element name='b'><empty/></element></define></grammar>"
            ),
        ),
        (
            "dir/leaf.rng".to_owned(),
            format!("<element name='leaf' xmlns='{RNG}'><empty/></element>"),
        ),
    ]);
    let main = format!(
        "<grammar xmlns='{RNG}'><include href='dir/lib.rng'><define name='b'><element name='override'><empty/></element></define></include><start combine='choice'><ref name='b'/></start></grammar>"
    );
    let schema = Schema::parse(
        main.as_bytes(),
        "main.rng",
        map_resolver(files.clone()),
        &Limits::default(),
    )
    .unwrap();
    assert!(codes(&schema, "<a><leaf/></a>").is_empty());
    assert!(codes(&schema, "<override/>").is_empty());
    assert_eq!(codes(&schema, "<b/>"), [Code::UnexpectedElement]);

    let missing = format!(
        "<grammar xmlns='{RNG}'><include href='dir/lib.rng'><define name='zz'><empty/></define></include></grammar>"
    );
    let error = Schema::parse(
        missing.as_bytes(),
        "main.rng",
        map_resolver(files.clone()),
        &Limits::default(),
    )
    .unwrap_err();
    assert_eq!(error.0[0].code, SchemaCode::DefineMissing);

    let recursive = HashMap::from([(
        "self.rng".to_owned(),
        format!("<element name='r' xmlns='{RNG}'><externalRef href='self.rng'/></element>"),
    )]);
    let text = recursive["self.rng"].clone();
    let error = Schema::parse(
        text.as_bytes(),
        "self.rng",
        map_resolver(recursive),
        &Limits::default(),
    )
    .unwrap_err();
    assert_eq!(error.0[0].code, SchemaCode::ExternalRefRecurse);

    let limited = Limits::DEFAULT.xml(yaxml::Limits::DEFAULT.max_external_loads(1));
    let error =
        Schema::parse(main.as_bytes(), "main.rng", map_resolver(files), &limited).unwrap_err();
    assert_eq!(error.0[0].code, SchemaCode::ResourceLimit);
}

#[test]
fn diagnostics_locate_elements() {
    let schema = schema(&wrap("<element name='a'><data type='int'/></element>")).unwrap();
    let document = Document::parse("<r><a>x</a></r>").unwrap();
    let [diagnostic] = schema.validate(&document).try_into().unwrap();
    let a = document.root_element().first_child().unwrap();
    assert_eq!(
        (diagnostic.node, diagnostic.offset),
        (Some(a.id()), Some(3))
    );
    assert_eq!(diagnostic.message, "failed to validate type int");
}

#[test]
fn schemas_are_send_sync_and_reusable() {
    fn assert_send_sync<T: Send + Sync>(_: &T) {}
    let schema = schema(&wrap("<text/>")).unwrap();
    assert_send_sync(&schema);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| assert!(codes(&schema, "<r>x</r>").is_empty()));
        }
    });
}

#[test]
fn pathological_schemas_stay_bounded() {
    // 200 interleaved optional elements, in reverse order: 2^200 subsets, yet
    // derivatives only meet the ones the document visits.
    let names: Vec<String> = (0..200).map(|i| format!("e{i}")).collect();
    let body: String = names
        .iter()
        .map(|n| format!("<optional><element name='{n}'><empty/></element></optional>"))
        .collect();
    let interleave = wrap(&format!("<interleave>{body}</interleave>"));
    let xml = format!(
        "<r>{}</r>",
        names
            .iter()
            .rev()
            .map(|n| format!("<{n}/>"))
            .collect::<String>()
    );
    let schema = schema(&interleave).unwrap();
    assert!(codes(&schema, &xml).is_empty());
    assert_eq!(
        codes(&schema, &xml.replace("<e7/>", "<e7/><e7/>")),
        [Code::UnexpectedElement]
    );

    // Nested interleaves around a repeated choice, with a long document.
    let mut pattern = "<zeroOrMore><choice><element name='a'><empty/></element><element name='b'><empty/></element></choice></zeroOrMore>".to_owned();
    for i in 0..30 {
        pattern = format!(
            "<interleave>{pattern}<optional><element name='x{i}'><empty/></element></optional></interleave>"
        );
    }
    let schema = self::schema(&wrap(&pattern)).unwrap();
    let long = format!("<r>{}<x3/><b/><x9/><a/></r>", "<a/><b/>".repeat(2000));
    assert!(codes(&schema, &long).is_empty());

    // Exhausted budgets: building fails, validation stops with one diagnostic.
    let tight = |max_work| Limits::DEFAULT.max_work(max_work);
    let error = Schema::parse(interleave.as_bytes(), "s", none(), &tight(100)).unwrap_err();
    assert_eq!(error.0[0].code, SchemaCode::ResourceLimit);
    let schema = Schema::parse(interleave.as_bytes(), "s", none(), &tight(40_000)).unwrap();
    assert_eq!(codes(&schema, &xml), [Code::Internal]);

    // Deeply inlined references fail cleanly instead of exhausting the stack.
    let defines: String = (0..2000)
        .map(|i| format!("<define name='d{i}'><ref name='d{}'/></define>", i + 1))
        .collect();
    let deep = format!(
        "<grammar xmlns='{RNG}'><start><element name='r'><ref name='d0'/></element></start>{defines}<define name='d2000'><empty/></define></grammar>"
    );
    let error = Schema::parse(deep.as_bytes(), "s", none(), &Limits::default()).unwrap_err();
    assert_eq!(error.0[0].code, SchemaCode::ResourceLimit);
}

// ---------------------------------------------------------------------------
// External suites.

fn source() -> PathBuf {
    PathBuf::from(std::env::var_os("YAXML_LIBXML2_SOURCE").expect("YAXML_LIBXML2_SOURCE"))
}

/// libxml2's `test/relaxng`: each schema compiles or fails as in
/// `result/relaxng`, and each instance gets the same verdict.
#[test]
#[ignore = "requires YAXML_LIBXML2_SOURCE pointing at the pinned libxml2 2.15.4 source tree"]
fn libxml2_relaxng_suite_verdicts_agree() {
    let source = source();
    let mut names: Vec<String> = std::fs::read_dir(source.join("test/relaxng"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".rng"))
        .collect();
    names.sort();
    let (mut agree, mut total, mut error_counts) = (0, 0, (0, 0, 0));
    let (mut streams, mut stream_differences) = (0, Vec::new());
    let mut differences = Vec::new();
    for name in names {
        let prefix = &name[..name.len() - 4];
        let mut instances: Vec<String> = std::fs::read_dir(source.join("test/relaxng"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|file| {
                file.len() == prefix.len() + 6
                    && file.starts_with(prefix)
                    && file[prefix.len()..].starts_with('_')
                    && file.ends_with(".xml")
            })
            .collect();
        instances.sort();
        let base = format!("test/relaxng/{name}");
        let resolver = Box::new(yaxml::FileResolver::new(&source).unwrap());
        let input = std::fs::read(source.join(&base)).unwrap();
        let parsed = Schema::parse(&input, &base, resolver, &Limits::default());
        for instance in instances {
            let suffix = &instance[prefix.len() + 1..prefix.len() + 2];
            let expected = std::fs::read_to_string(
                source.join(format!("result/relaxng/{prefix}_{suffix}.err")),
            )
            .unwrap_or_default();
            total += 1;
            let (oracle, ours, oracle_errors, our_errors) = match &parsed {
                Err(errors) => (
                    if expected.contains("failed to compile") {
                        "compile-error"
                    } else {
                        "?"
                    },
                    "compile-error".to_owned(),
                    0,
                    errors.0.len(),
                ),
                Ok(schema) => {
                    let text = std::fs::read(source.join("test/relaxng").join(&instance)).unwrap();
                    let document =
                        OwnedDocument::with_limits(&text, yaxml::Limits::default()).unwrap();
                    let found = schema.validate(&document);
                    let stream = streamed(schema, &text, ParseOptions::default());
                    streams += 1;
                    if stream.as_ref() != Some(&summary(&found)) {
                        stream_differences.push(format!("{instance}: {stream:?} != {found:?}"));
                    }
                    let oracle = if expected.contains("fails to validate") {
                        "invalid"
                    } else if expected.contains(" validates") {
                        "valid"
                    } else {
                        "compile-error"
                    };
                    let ours = if found.is_empty() { "valid" } else { "invalid" };
                    let detail = found
                        .iter()
                        .map(|d| d.message.clone())
                        .collect::<Vec<_>>()
                        .join("; ");
                    (
                        oracle,
                        format!("{ours} [{detail}]"),
                        expected.matches("validity error").count(),
                        found.len(),
                    )
                }
            };
            if ours.starts_with(oracle) {
                agree += 1;
                if oracle == "invalid" {
                    if std::env::var_os("YAXML_RELAXNG_VERBOSE").is_some() {
                        let theirs: Vec<_> = expected
                            .lines()
                            .filter_map(|l| l.split("validity error : ").nth(1))
                            .collect();
                        println!("{instance}: libxml2 {theirs:?}\n  yaxml {ours}");
                    }
                    error_counts.0 += 1;
                    error_counts.1 += oracle_errors;
                    error_counts.2 += our_errors;
                }
            } else {
                let why = parsed
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                differences.push(format!("{instance}: libxml2 {oracle}, yaxml {ours} {why}"));
            }
        }
    }
    println!("libxml2 relaxng suite: {agree}/{total} verdicts agree");
    println!(
        "streaming equals DOM: {}/{streams}",
        streams - stream_differences.len()
    );
    assert!(
        stream_differences.is_empty(),
        "{}",
        stream_differences.join("\n")
    );
    println!(
        "invalid instances: {} (libxml2 {} validity errors, yaxml {})",
        error_counts.0, error_counts.1, error_counts.2
    );
    println!("{}", differences.join("\n"));
}

/// Serializes an element subtree with the namespaces in scope declared on it.
fn subtree(node: Node<'_, '_>) -> String {
    fn escape(text: &str, out: &mut String) {
        for c in text.chars() {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\r' => out.push_str("&#13;"),
                _ => out.push(c),
            }
        }
    }
    fn element(node: Node<'_, '_>, scope: &[(String, String)], out: &mut String) {
        let NodeKind::Element(e) = node.kind() else {
            return;
        };
        out.push('<');
        out.push_str(e.name.qualified());
        for (prefix, uri) in scope {
            let name = if prefix.is_empty() {
                "xmlns".to_owned()
            } else {
                format!("xmlns:{prefix}")
            };
            out.push_str(&format!(" {name}=\""));
            escape(uri, out);
            out.push('"');
        }
        for attribute in node.attributes() {
            out.push_str(&format!(" {}=\"", attribute.name.qualified()));
            escape(&attribute.value, out);
            out.push('"');
        }
        out.push('>');
        for child in node.children() {
            match child.kind() {
                NodeKind::Element(_) => element(child, &[], out),
                NodeKind::Text(text) | NodeKind::Cdata(text) => escape(text, out),
                _ => {}
            }
        }
        out.push_str(&format!("</{}>", e.name.qualified()));
    }
    // Declarations of the ancestors, innermost first, not redeclared below.
    let mut scope: Vec<(String, String)> = Vec::new();
    for ancestor in std::iter::successors(node.parent(), |n| n.parent()) {
        for attribute in ancestor.attributes() {
            let name = attribute.name.qualified();
            let prefix = match name {
                "xmlns" => "",
                _ => match name.strip_prefix("xmlns:") {
                    Some(prefix) => prefix,
                    None => continue,
                },
            };
            let declared_here = node.attributes().iter().any(|a| a.name.qualified() == name);
            if !declared_here && scope.iter().all(|(p, _)| p != prefix) {
                scope.push((prefix.to_owned(), attribute.value.to_string()));
            }
        }
    }
    let mut out = String::new();
    element(node, &scope, &mut out);
    out
}

fn first_element<'a, 'd>(node: Node<'a, 'd>) -> Option<Node<'a, 'd>> {
    node.children()
        .find(|child| matches!(child.kind(), NodeKind::Element(_)))
}

fn named<'a, 'd>(node: Node<'a, 'd>, name: &'static str) -> impl Iterator<Item = Node<'a, 'd>> {
    node.children()
        .filter(move |child| matches!(child.kind(), NodeKind::Element(e) if e.name.local() == name))
}

fn attribute(node: Node<'_, '_>, name: &str) -> Option<String> {
    node.attributes()
        .iter()
        .find(|a| a.name.qualified() == name)
        .map(|a| a.value.to_string())
}

fn resources(node: Node<'_, '_>, dir: &str, files: &mut HashMap<String, String>) {
    for resource in named(node, "resource") {
        let name = attribute(resource, "name").unwrap();
        files.insert(
            format!("{dir}{name}"),
            first_element(resource).map(subtree).unwrap_or_default(),
        );
    }
    for sub in named(node, "dir") {
        let name = attribute(sub, "name").unwrap();
        resources(sub, &format!("{dir}{name}/"), files);
    }
}

/// libxml2's verdicts from `xmllint --relaxng`: `None` if the schema does
/// not compile, else one verdict per instance.
fn xmllint(
    files: &HashMap<String, String>,
    schema: &str,
    instances: &[String],
) -> Option<Vec<bool>> {
    let xmllint = std::env::var_os("YAXML_XMLLINT").unwrap();
    let directory = std::env::temp_dir().join(format!("yaxml-relaxng-{}", std::process::id()));
    std::fs::remove_dir_all(&directory).ok();
    for (name, text) in files
        .iter()
        .chain([(&"test.rng".to_owned(), &schema.to_owned())])
    {
        let path = directory.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let mut names = vec!["dummy.xml".to_owned()];
    std::fs::write(directory.join("dummy.xml"), "<dummy/>").unwrap();
    for (index, instance) in instances.iter().enumerate() {
        names.push(format!("i{index}.xml"));
        std::fs::write(directory.join(&names[index + 1]), instance).unwrap();
    }
    let output = std::process::Command::new(xmllint)
        .current_dir(&directory)
        .args(["--noout", "--relaxng", "test.rng"])
        .args(&names)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("failed to compile") {
        return None;
    }
    Some(
        names[1..]
            .iter()
            .map(|name| stderr.contains(&format!("{name} validates")))
            .collect(),
    )
}

/// Agreement counts of one suite.
#[derive(Debug, Default)]
struct Tally {
    schemas: usize,
    schemas_spec: usize,
    schemas_libxml2: usize,
    instances: usize,
    instances_spec: usize,
    instances_libxml2: usize,
    /// Instances whose streaming diagnostics equal the DOM's.
    instances_streamed: usize,
    /// Cases where yaxml or libxml2 departs from the expected verdict.
    differences: Vec<String>,
}

/// Runs every `testCase` of a James Clark style suite, comparing yaxml with
/// the expected verdicts and, with `YAXML_XMLLINT`, with libxml2.
fn run_suite(path: &Path) -> Tally {
    let text = std::fs::read(path).unwrap();
    let document = Document::parse(std::str::from_utf8(&text).unwrap()).unwrap();
    let mut cases = Vec::new();
    let mut stack = vec![document.root_element()];
    while let Some(node) = stack.pop() {
        cases.extend(named(node, "testCase"));
        stack.extend(named(node, "testSuite"));
    }
    cases.sort_by_key(|case| case.range().map(|r| r.start));
    let has_oracle = std::env::var_os("YAXML_XMLLINT").is_some();
    let mut tally = Tally::default();
    for case in cases {
        let mut files = HashMap::new();
        resources(case, "", &mut files);
        let at = case.range().map(|r| r.start).unwrap_or(0);
        let (schema_node, correct) = match (
            named(case, "correct").next(),
            named(case, "incorrect").next(),
        ) {
            (Some(node), _) => (node, true),
            (None, Some(node)) => (node, false),
            _ => continue,
        };
        let schema_text = first_element(schema_node).map(subtree).unwrap_or_default();
        let mut instances = Vec::new();
        for (kind, valid) in [("valid", true), ("invalid", false)] {
            for instance in named(case, kind) {
                if let Some(root) = first_element(instance) {
                    instances.push((
                        attribute(instance, "dtd").unwrap_or_default() + &subtree(root),
                        valid,
                    ));
                }
            }
        }
        let texts: Vec<String> = instances.iter().map(|(text, _)| text.clone()).collect();
        let oracle = has_oracle.then(|| xmllint(&files, &schema_text, &texts));
        let ours = Schema::parse(
            schema_text.as_bytes(),
            "test.rng",
            map_resolver(files),
            &Limits::default(),
        );
        tally.schemas += 1;
        tally.schemas_spec += usize::from(ours.is_ok() == correct);
        tally.schemas_libxml2 +=
            usize::from(oracle.as_ref().is_some_and(|o| o.is_some() == ours.is_ok()));
        if ours.is_ok() != correct || oracle.as_ref().is_some_and(|o| o.is_some() != correct) {
            let why = ours
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default();
            tally.differences.push(format!(
                "@{at} schema correct={correct} yaxml={} libxml2={:?} {why}\n{schema_text}",
                ours.is_ok(),
                oracle.as_ref().map(Option::is_some),
            ));
        }
        let Ok(schema) = ours else { continue };
        for (index, (xml, valid)) in instances.iter().enumerate() {
            tally.instances += 1;
            let found = OwnedDocument::parse(xml).map(|document| schema.validate(&document));
            let stream = streamed(&schema, xml.as_bytes(), ParseOptions::default());
            let dom = found.as_ref().ok().map(|found| summary(found));
            tally.instances_streamed += usize::from(stream == dom);
            if stream != dom {
                tally
                    .differences
                    .push(format!("streaming {stream:?} != DOM {dom:?}\n{xml}"));
            }
            let ours = found.as_ref().map_or(*valid, |found| found.is_empty());
            let libxml2 = oracle
                .as_ref()
                .and_then(|o| o.as_ref().map(|verdicts| verdicts[index]));
            tally.instances_spec += usize::from(ours == *valid);
            tally.instances_libxml2 += usize::from(libxml2 == Some(ours));
            if ours != *valid || libxml2.is_some_and(|l| l != *valid) {
                let messages: Vec<_> = found.iter().flatten().map(|d| d.message.clone()).collect();
                tally.differences.push(format!("@{at} instance valid={valid} yaxml={ours} libxml2={libxml2:?} {messages:?}\n{xml}\n{schema_text}"));
            }
        }
    }
    tally
}

#[test]
#[ignore = "requires YAXML_LIBXML2_SOURCE pointing at the pinned libxml2 2.15.4 source tree"]
fn james_clark_spec_suite() {
    for suite in [
        "test/relaxng/OASIS/spectest.xml",
        "test/relaxng/testsuite.xml",
        "test/xsdtest/xsdtestsuite.xml",
    ] {
        let tally = run_suite(&source().join(suite));
        println!(
            "{suite}: schemas {} (spec {}, libxml2 {}), instances {} (spec {}, libxml2 {}, streaming = DOM {})",
            tally.schemas,
            tally.schemas_spec,
            tally.schemas_libxml2,
            tally.instances,
            tally.instances_spec,
            tally.instances_libxml2,
            tally.instances_streamed,
        );
        println!("{}", tally.differences.join("\n"));
        assert_eq!(tally.instances_streamed, tally.instances, "{suite}");
    }
}

/// `TextReader` validation of libxml2's `test/relaxng` instances against
/// `xmllint --stream --relaxng` (`YAXML_XMLLINT`): verdicts must agree;
/// messages are counted.
#[test]
#[ignore = "requires YAXML_LIBXML2_SOURCE and YAXML_XMLLINT"]
fn libxml2_relaxng_suite_streams_like_xmllint() {
    let (source, xmllint) = (source(), std::env::var_os("YAXML_XMLLINT").unwrap());
    let directory = source.join("test/relaxng");
    let mut names: Vec<String> = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    let (mut total, mut same_messages, mut differences) = (0, 0, Vec::new());
    for name in names.iter().filter(|name| name.ends_with(".rng")) {
        let prefix = &name[..name.len() - 4];
        let base = format!("test/relaxng/{name}");
        let resolver = Box::new(yaxml::FileResolver::new(&source).unwrap());
        let input = std::fs::read(source.join(&base)).unwrap();
        let Ok(schema) = Schema::parse(&input, &base, resolver, &Limits::default()) else {
            continue;
        };
        let instances = names.iter().filter(|file| {
            file.len() == prefix.len() + 6
                && file.starts_with(&format!("{prefix}_"))
                && file.ends_with(".xml")
        });
        for instance in instances {
            let path = format!("test/relaxng/{instance}");
            let output = std::process::Command::new(&xmllint)
                .current_dir(&source)
                .args(["--stream", "--noout", "--relaxng", &base, &path])
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            let theirs: Vec<&str> = (stderr.lines())
                .filter_map(|line| line.split("validity error : ").nth(1))
                .collect();
            let text = std::fs::read(source.join(&path)).unwrap();
            let flags = yaxml::options::NOENT | yaxml::options::DTDLOAD;
            let resolver = Box::new(yaxml::FileResolver::new(&source).unwrap());
            let options = ParseOptions::new(flags).with_resolver(&path, resolver);
            let mut reader = yaxml::TextReader::with_options(text.as_slice(), options).unwrap();
            reader.relaxng_set_schema(Some(&schema)).unwrap();
            while reader.read().unwrap() {}
            let ours: Vec<&str> = (reader.relaxng_diagnostics().iter())
                .map(|d| d.message.as_str())
                .collect();
            total += 1;
            same_messages += usize::from(ours == theirs);
            if ours != theirs && std::env::var_os("YAXML_RELAXNG_VERBOSE").is_some() {
                println!("{instance}: libxml2 {theirs:?}\n  yaxml {ours:?}");
            }
            if reader.is_valid() != stderr.contains(" validates") {
                differences.push(format!("{instance}: libxml2 {theirs:?}, yaxml {ours:?}"));
            }
        }
    }
    println!(
        "--stream --relaxng: {}/{total} verdicts, {same_messages}/{total} message lists agree",
        total - differences.len()
    );
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

/// A stream reports each fault when its event arrives (with the element's
/// index in document order) and resolves references when the document
/// element ends. (document prefix, diagnostics so far)
#[test]
fn streams_report_as_events_arrive() {
    let schema = schema(&wrap(&format!("{IDS}<element name='c'><empty/></element>"))).unwrap();
    let cases: &[(&str, &[(Code, usize)])] = &[
        ("<r><b ref='x'/>", &[]),
        ("<r><b ref='x'/><c/></r>", &[(Code::UnknownId, 1)]),
        ("<r><a id='x'/><a id='x'/>", &[(Code::DuplicateId, 2)]),
        ("<r><a id='x'/>", &[]),
        ("<r><a id='x'/></r>", &[(Code::MissingElement, 0)]),
        ("<r><c>text", &[]),
        ("<r><c>text</c>", &[(Code::UnexpectedText, 1)]),
        ("<r><c/></r><!--after--><?pi?>", &[]),
        ("<x><c/></x>", &[(Code::ElementName, 0)]),
    ];
    for (xml, expected) in cases {
        let mut stream = schema.stream();
        let mut parser = PushParser::new();
        (parser.feed(xml.as_bytes(), |event, origin| {
            stream.event(event, origin);
            ControlFlow::Continue(())
        }))
        .unwrap();
        let found: Vec<_> = (stream.diagnostics().iter())
            .map(|d| (d.code, d.node.unwrap()))
            .collect();
        assert_eq!(&found, expected, "{xml}");
    }
}
