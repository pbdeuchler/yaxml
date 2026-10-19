//! Stream validation (`Schema::validate_stream`, `StreamValidator`) against
//! tree validation of the same input: every streaming surface must report
//! what the owned DOM reports, at the same offsets with the same messages.

use std::ops::ControlFlow;

use yaxml::options::{DTDATTR, NOCDATA, NOENT};
use yaxml::xsd::{self, Code, Limits, Schema, StreamValidator};
use yaxml::{OwnedDocument, ParseOptions, PushParser, Reader};

const XS: &str = "http://www.w3.org/2001/XMLSchema";

fn schema(body: &str) -> Schema {
    schema_with(body, Limits::default())
}

fn schema_with(body: &str, limits: Limits) -> Schema {
    let text = format!(
        "<xs:schema xmlns:xs='{XS}' xmlns:t='urn:t' targetNamespace='urn:t' elementFormDefault='qualified'>{body}</xs:schema>"
    );
    Schema::with_resolver(text.as_bytes(), "s.xsd", None, limits).unwrap()
}

type Projection = Vec<(Code, Option<usize>, String)>;

fn project<Id>(diagnostics: &[xsd::Diagnostic<Id>]) -> Projection {
    let project = |d: &xsd::Diagnostic<Id>| (d.code, d.offset, d.message.clone());
    diagnostics.iter().map(project).collect()
}

/// Stream diagnostics, equal over a buffered reader, a push parser fed one
/// byte at a time and the slice reader.
fn streamed(schema: &Schema, instance: &str, flags: u32) -> Vec<xsd::Diagnostic<usize>> {
    let options = || ParseOptions::new(flags);
    let buffered = schema
        .validate_stream(instance.as_bytes(), options())
        .unwrap();

    let mut validator = StreamValidator::new(schema);
    let mut parser = PushParser::with_options(options()).unwrap();
    let mut feed = |event: &yaxml::Event<'_>, origin| {
        validator.event(event, origin);
        ControlFlow::Continue(())
    };
    for byte in instance.as_bytes().chunks(1) {
        parser.feed(byte, &mut feed).unwrap();
    }
    parser.finish(&mut feed).unwrap();
    assert_eq!(buffered, validator.finish(), "push");

    let mut validator = StreamValidator::new(schema);
    let mut reader = Reader::with_options(instance.as_bytes(), options()).unwrap();
    while let Some(event) = reader.read_event().unwrap() {
        validator.event(&event, reader.event_origin().unwrap());
    }
    assert_eq!(buffered, validator.finish(), "slice reader");
    buffered
}

fn codes<Id>(diagnostics: &[xsd::Diagnostic<Id>]) -> Vec<u16> {
    diagnostics.iter().map(|d| d.code as u16).collect()
}

/// `(name, schema body, instance, parse flags, expected codes)`.
type Case = (
    &'static str,
    &'static str,
    &'static str,
    u32,
    &'static [u16],
);

const ELEMENT_ONLY: &str = "<xs:element name='r'><xs:complexType><xs:sequence>
    <xs:element name='c' minOccurs='0' maxOccurs='9'/></xs:sequence></xs:complexType></xs:element>";
const EMPTY: &str = "<xs:element name='r'><xs:complexType/></xs:element>";
const NILLABLE: &str = "<xs:element name='r' type='xs:string' nillable='true'/>";
const ENTITY: &str = "<xs:element name='r'><xs:complexType>
    <xs:attribute name='a' type='xs:ENTITY'/></xs:complexType></xs:element>";
const IDS: &str = "<xs:element name='r'><xs:complexType><xs:sequence>
    <xs:element name='c' maxOccurs='9'><xs:complexType>
      <xs:attribute name='s' type='xs:ID'/><xs:anyAttribute processContents='skip'/>
    </xs:complexType></xs:element></xs:sequence></xs:complexType></xs:element>";
const KEYS: &str = "<xs:element name='r'><xs:complexType><xs:sequence>
    <xs:element name='k' type='xs:int' maxOccurs='9'/>
    <xs:element name='f' type='xs:int' maxOccurs='9'/>
  </xs:sequence></xs:complexType>
  <xs:key name='key'><xs:selector xpath='t:k'/><xs:field xpath='.'/></xs:key>
  <xs:keyref name='ref' refer='t:key'><xs:selector xpath='t:f'/><xs:field xpath='.'/></xs:keyref>
</xs:element>";
const QNAMES: &str = "<xs:element name='r'><xs:complexType><xs:sequence>
    <xs:element name='q' type='xs:QName' maxOccurs='9'/></xs:sequence></xs:complexType></xs:element>
  <xs:complexType name='base'/>";

/// Each case also runs over the owned DOM, which must report the same.
const CASES: &[Case] = &[
    (
        "valid",
        ELEMENT_ONLY,
        "<r xmlns='urn:t'><c/><c/></r>",
        0,
        &[],
    ),
    (
        "unexpected child skips the rest",
        ELEMENT_ONLY,
        "<r xmlns='urn:t'><x><y>1</y></x><c/><z/></r>",
        0,
        &[1871],
    ),
    (
        "missing child",
        "<xs:element name='r'><xs:complexType><xs:sequence>
           <xs:element name='c'/></xs:sequence></xs:complexType></xs:element>",
        "<r xmlns='urn:t'></r>",
        0,
        &[1871],
    ),
    (
        "text around an entity reference is three nodes",
        ELEMENT_ONLY,
        "<!DOCTYPE r [<!ENTITY e 'x'>]><r xmlns='urn:t'>a&e;b<c/></r>",
        0,
        &[1843, 1843, 1843],
    ),
    (
        "substituted entity text is one node",
        ELEMENT_ONLY,
        "<!DOCTYPE r [<!ENTITY e 'x'>]><r xmlns='urn:t'> &e; <c/></r>",
        NOENT,
        &[1843],
    ),
    (
        "blank text then non-blank entity text",
        ELEMENT_ONLY,
        "<!DOCTYPE r [<!ENTITY e '<c/>x'>]><r xmlns='urn:t'> &e;</r>",
        NOENT,
        &[1843],
    ),
    (
        "blank substituted text",
        ELEMENT_ONLY,
        "<!DOCTYPE r [<!ENTITY e ' '>]><r xmlns='urn:t'> &e; <c/></r>",
        NOENT,
        &[],
    ),
    (
        "CDATA is never blank",
        ELEMENT_ONLY,
        "<r xmlns='urn:t'> <![CDATA[ ]]><![CDATA[ ]]></r>",
        0,
        &[1843, 1843],
    ),
    (
        "CDATA as text joins its neighbors",
        EMPTY,
        "<r xmlns='urn:t'>a<![CDATA[b]]>c</r>",
        NOCDATA,
        &[1841],
    ),
    (
        "comments split text",
        EMPTY,
        "<r xmlns='urn:t'>a<!---->b<?p?>c</r>",
        0,
        &[1841, 1841, 1841],
    ),
    (
        "nilled element with text",
        NILLABLE,
        "<!DOCTYPE r [<!ENTITY e 'x'>]><r xmlns='urn:t' xmlns:i='http://www.w3.org/2001/XMLSchema-instance' i:nil='true'>a&e;</r>",
        NOENT,
        &[1848],
    ),
    (
        "nilled element valid",
        NILLABLE,
        "<r xmlns='urn:t' xmlns:i='http://www.w3.org/2001/XMLSchema-instance' i:nil='1'/>",
        0,
        &[],
    ),
    (
        "unparsed entity from the internal subset",
        ENTITY,
        "<!DOCTYPE r [<!NOTATION n SYSTEM 'n'><!ENTITY e SYSTEM 'e' NDATA n>]><r xmlns='urn:t' a='e'/>",
        0,
        &[],
    ),
    (
        "parsed entity is not an ENTITY value",
        ENTITY,
        "<!DOCTYPE r [<!ENTITY e 'x'>]><r xmlns='urn:t' a='e'/>",
        0,
        &[1824],
    ),
    (
        "ENTITY without a DTD",
        ENTITY,
        "<r xmlns='urn:t' a='e'/>",
        0,
        &[1824],
    ),
    (
        "duplicate schema IDs",
        IDS,
        "<r xmlns='urn:t'><c s='a'/><c s=' a '/></r>",
        0,
        &[1824],
    ),
    (
        "schema ID repeats a DTD ID",
        IDS,
        "<!DOCTYPE r [<!ATTLIST c d ID #IMPLIED>]><r xmlns='urn:t'><c d='a'/><c s='a'/></r>",
        0,
        &[1824],
    ),
    (
        "schema ID repeats an xml:id",
        IDS,
        "<r xmlns='urn:t'><c xml:id='a'/><c s='a'/></r>",
        0,
        &[1824],
    ),
    (
        "DTD-typed ID the schema also types",
        IDS,
        "<!DOCTYPE r [<!ATTLIST c s ID #IMPLIED>]><r xmlns='urn:t'><c s='a'/><c s='b'/></r>",
        0,
        &[],
    ),
    (
        "keyref resolves",
        KEYS,
        "<r xmlns='urn:t'><k>1</k><k>2</k><f>2</f></r>",
        0,
        &[],
    ),
    (
        "keyref unresolved and key duplicated",
        KEYS,
        "<r xmlns='urn:t'><k>1</k><k>1</k><f>3</f></r>",
        0,
        &[1877, 1877],
    ),
    (
        "QNames resolve in nested scopes",
        QNAMES,
        "<r xmlns='urn:t' xmlns:p='urn:a'><q xmlns:p='urn:b'>p:x</q><q>p:y</q><q>xml:z</q></r>",
        0,
        &[],
    ),
    (
        "QName prefix out of scope",
        QNAMES,
        "<r xmlns='urn:t'><q xmlns:p='urn:b'>p:x</q><q>p:y</q></r>",
        0,
        &[1824, 1824],
    ),
    (
        "xsi:type resolves an ancestor's prefix",
        "<xs:element name='r'><xs:complexType><xs:sequence>
           <xs:element name='e' type='xs:anyType'/></xs:sequence></xs:complexType></xs:element>
         <xs:simpleType name='n'><xs:restriction base='xs:int'/></xs:simpleType>",
        "<r xmlns='urn:t' xmlns:a='urn:t'><e xmlns:i='http://www.w3.org/2001/XMLSchema-instance' i:type='a:n'>x</e></r>",
        0,
        &[1824],
    ),
    (
        "DTD default attribute is validated",
        EMPTY,
        "<!DOCTYPE r [<!ATTLIST r a CDATA 'x'>]><r xmlns='urn:t'/>",
        DTDATTR,
        &[1866],
    ),
];

#[test]
fn stream_validation_matches_the_tree() {
    for &(name, body, instance, flags, want) in CASES {
        let schema = schema(body);
        let stream = streamed(&schema, instance, flags);
        assert_eq!(codes(&stream), want, "{name}");
        let tree =
            OwnedDocument::parse_with(instance.as_bytes(), ParseOptions::new(flags)).unwrap();
        assert_eq!(project(&schema.validate(&tree)), project(&stream), "{name}");
    }
}

/// A tree knows every ID before validation; a stream reports a schema ID
/// that a later DTD ID or `xml:id` repeats at the later attribute.
#[test]
fn later_dtd_ids_are_reported_where_they_repeat() {
    let schema = schema(IDS);
    for (instance, attribute) in [
        (
            "<!DOCTYPE r [<!ATTLIST c d ID #IMPLIED>]><r xmlns='urn:t'><c s='a'/><c d='a'/></r>",
            "d",
        ),
        (
            "<r xmlns='urn:t'><c s='a'/><c xml:id='a'/></r>",
            "{http://www.w3.org/XML/1998/namespace}id",
        ),
    ] {
        let stream = streamed(&schema, instance, 0);
        let tree = OwnedDocument::parse(instance).unwrap();
        let dom = schema.validate(&tree);
        assert_eq!(codes(&stream), codes(&dom));
        assert_eq!(stream[0].node, Some(2));
        let message = format!(
            "Element '{{urn:t}}c', attribute '{attribute}': 'a' is not a valid value of the atomic type 'xs:ID'."
        );
        assert_eq!(stream[0].message, message);
    }
}

#[test]
fn diagnostics_name_elements_by_document_order() {
    let schema = schema(KEYS);
    let instance = "<r xmlns='urn:t'><k>1</k><k>x</k><f>5</f></r>";
    let stream = streamed(&schema, instance, 0);
    let nodes: Vec<Option<usize>> = stream.iter().map(|d| d.node).collect();
    assert_eq!(codes(&stream), [1824, 1877, 1877, 1877]);
    assert_eq!(nodes, [Some(2), Some(2), Some(2), Some(3)]);
    let tree = OwnedDocument::parse(instance).unwrap();
    assert_eq!(project(&schema.validate(&tree)), project(&stream));
}

#[test]
fn identity_constraint_tables_are_bounded() {
    let limits = Limits {
        max_idc_keys: 2,
        ..Limits::default()
    };
    let schema = schema_with(KEYS, limits);
    for (instance, want) in [
        ("<r xmlns='urn:t'><k>1</k><f>1</f></r>", &[][..]),
        // The third key is over budget; keyrefs are then unchecked.
        ("<r xmlns='urn:t'><k>1</k><k>2</k><f>9</f></r>", &[1818]),
        (
            "<r xmlns='urn:t'><k>1</k><k>2</k><k>3</k><f>9</f></r>",
            &[1818],
        ),
    ] {
        let stream = streamed(&schema, instance, 0);
        assert_eq!(codes(&stream), want, "{instance}");
        let tree = OwnedDocument::parse(instance).unwrap();
        assert_eq!(project(&schema.validate(&tree)), project(&stream));
    }
}

#[test]
fn utf16_input_matches_the_tree() {
    let schema = schema(EMPTY);
    let text = "<r xmlns='urn:t'>x</r>";
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    let stream = schema
        .validate_stream(bytes.as_slice(), ParseOptions::default())
        .unwrap();
    let tree = OwnedDocument::parse_bytes(&bytes).unwrap();
    assert_eq!(codes(&stream), [1841]);
    assert_eq!(project(&schema.validate(&tree)), project(&stream));
}

#[test]
fn failures_without_a_document() {
    let schema = schema(EMPTY);
    let empty = StreamValidator::new(&schema).finish();
    assert_eq!(codes(&empty), [1872]);
    assert_eq!(empty[0].node, None);
    for malformed in ["", "<r xmlns='urn:t'>", "<r></s>"] {
        let result = schema.validate_stream(malformed.as_bytes(), ParseOptions::default());
        assert!(result.is_err(), "{malformed:?}");
    }
    let unsupported = ParseOptions::new(yaxml::options::DTDVALID);
    assert!(schema.validate_stream(&b"<r/>"[..], unsupported).is_err());
}
