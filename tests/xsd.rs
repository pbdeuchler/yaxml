//! Table-driven XML Schema cases. Each expectation is written as the
//! libxml2 oracle helper prints it (`tooling/libxml2_xsd.c`): `S codes` for
//! a schema that fails to build, `V codes` for validation (`V` alone is
//! valid). Every case runs against yaxml over both DOMs; with
//! `YAXML_XSD_HELPER` set, the ignored test checks the same expectations
//! against libxml2 2.15.4.

use std::io::Write;
use std::process::{Command, Stdio};

use yaxml::xsd::{self, Code, Schema};
use yaxml::{Document, OwnedDocument};

const XS: &str = "http://www.w3.org/2001/XMLSchema";

fn schema(body: &str) -> String {
    match body.starts_with("<xs:schema") || body.starts_with("<schema") {
        true => body.to_owned(),
        false => format!("<xs:schema xmlns:xs='{XS}'>{body}</xs:schema>"),
    }
}

fn project<Id>(diagnostics: &[xsd::Diagnostic<Id>]) -> Vec<(Code, Option<usize>, String)> {
    diagnostics
        .iter()
        .map(|d| (d.code, d.offset, d.message.clone()))
        .collect()
}

/// yaxml's outcome in the oracle's format.
fn outcome(schema_text: &str, instance: &str) -> String {
    let built = match Schema::parse(schema_text.as_bytes(), "schema.xsd") {
        Err(error) => {
            let codes: Vec<String> = error
                .diagnostics
                .iter()
                .filter(|d| !d.code.is_warning())
                .map(|d| (d.code as u16).to_string())
                .collect();
            return format!("S {}", codes.join(" ")).trim_end().to_owned();
        }
        Ok(schema) => schema,
    };
    let Ok(owned) = OwnedDocument::parse(instance) else {
        return "X".into();
    };
    let diagnostics = built.validate(&owned);
    let borrowed = Document::parse(instance).unwrap();
    let again = built.validate(&borrowed);
    assert_eq!(project(&diagnostics), project(&again), "both DOMs agree");
    let streamed = built
        .validate_stream(instance.as_bytes(), Default::default())
        .unwrap();
    assert_eq!(project(&diagnostics), project(&streamed), "stream agrees");
    for diagnostic in &diagnostics {
        assert!(!diagnostic.message.is_empty());
    }
    let codes: Vec<String> = diagnostics
        .iter()
        .filter(|d| !d.code.is_warning())
        .map(|d| (d.code as u16).to_string())
        .collect();
    assert_eq!(xsd::is_valid(&diagnostics), codes.is_empty());
    format!("V {}", codes.join(" ")).trim_end().to_owned()
}

/// `(name, schema body, instance, expected outcome)`.
type Case = (&'static str, &'static str, &'static str, &'static str);

const ROOT_INT: &str = "<xs:element name='r' type='xs:int'/>";

const CASES: &[Case] = &[
    // ---- element declarations and simple content --------------------------
    ("int valid", ROOT_INT, "<r> 42 </r>", "V"),
    ("int invalid", ROOT_INT, "<r>4x</r>", "V 1824"),
    ("undeclared root", ROOT_INT, "<s/>", "V 1845"),
    (
        "root in other namespace",
        ROOT_INT,
        "<r xmlns='urn:x'>1</r>",
        "V 1845",
    ),
    (
        "simple type with child",
        ROOT_INT,
        "<r><a/></r>",
        "V 1828 1824",
    ),
    (
        "simple type with attribute",
        ROOT_INT,
        "<r a='1'>1</r>",
        "V 1827",
    ),
    (
        "simple type xsi attributes",
        ROOT_INT,
        "<r xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' xsi:schemaLocation='a b'>1</r>",
        "V",
    ),
    (
        "default used when empty",
        "<xs:element name='r' type='xs:int' default='5'/>",
        "<r/>",
        "V",
    ),
    (
        "default invalid against type",
        "<xs:element name='r' type='xs:int' default='x'/>",
        "<r/>",
        "S 1824",
    ),
    (
        "fixed matches",
        "<xs:element name='r' type='xs:int' fixed='5'/>",
        "<r>5</r>",
        "V",
    ),
    (
        "fixed differs",
        "<xs:element name='r' type='xs:int' fixed='5'/>",
        "<r>6</r>",
        "V 1858",
    ),
    (
        "fixed compares the raw text",
        "<xs:element name='r' type='xs:int' fixed='5'/>",
        "<r> 5</r>",
        "V 1858",
    ),
    (
        "fixed used when empty",
        "<xs:element name='r' type='xs:int' fixed='5'/>",
        "<r/>",
        "V",
    ),
    (
        "default and fixed together",
        "<xs:element name='r' default='1' fixed='1'/>",
        "<r/>",
        "S 3038",
    ),
    (
        "type and inline type",
        "<xs:element name='r' type='xs:int'><xs:simpleType><xs:restriction base='xs:int'/></xs:simpleType></xs:element>",
        "<r/>",
        "S 3041",
    ),
    (
        "abstract root",
        "<xs:element name='r' abstract='true'/>",
        "<r/>",
        "V 1846",
    ),
    (
        "untyped element is anyType",
        "<xs:element name='r'/>",
        "<r a='1'><x><y/>t</x></r>",
        "V",
    ),
    (
        "anyType child with declared global",
        "<xs:element name='r'/><xs:element name='g' type='xs:int'/>",
        "<r><g>x</g></r>",
        "V 1824",
    ),
    (
        "unknown type",
        "<xs:element name='r' type='xs:nosuch'/>",
        "<r/>",
        "S 3004",
    ),
    (
        "unknown prefix",
        "<xs:element name='r' type='p:t'/>",
        "<r/>",
        "S 3037",
    ),
    (
        "missing name",
        "<xs:element type='xs:int'/>",
        "<r/>",
        "S 3036",
    ),
    (
        "bad attribute on element",
        "<xs:element name='r' bogus='1'/>",
        "<r/>",
        "S 3035",
    ),
    ("bad child of schema", "<xs:bogus/>", "<r/>", "S 3033"),
    (
        "text in schema",
        "<xs:element name='r'/>text",
        "<r/>",
        "S 3033",
    ),
    (
        "duplicate global element",
        "<xs:element name='r'/><xs:element name='r'/>",
        "<r/>",
        "S 1761",
    ),
    (
        "invalid nillable",
        "<xs:element name='r' nillable='maybe'/>",
        "<r/>",
        "S 1714",
    ),
    (
        "invalid block",
        "<xs:element name='r' block='list'/>",
        "<r/>",
        "S 3037",
    ),
    (
        "ID element with value constraint",
        "<xs:element name='r' type='xs:ID' default='a'/>",
        "<r/>",
        "S 3048",
    ),
    // ---- simple types and facets --------------------------------------------
    (
        "enumeration",
        "<xs:element name='r'><xs:simpleType><xs:restriction base='xs:string'><xs:enumeration value='a'/><xs:enumeration value='b'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>c</r>",
        "V 1840",
    ),
    (
        "enumeration matches",
        "<xs:element name='r'><xs:simpleType><xs:restriction base='xs:string'><xs:enumeration value='a'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>a</r>",
        "V",
    ),
    (
        "pattern",
        "<xs:element name='r'><xs:simpleType><xs:restriction base='xs:string'><xs:pattern value='[a-z]+'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>A1</r>",
        "V 1839",
    ),
    (
        "patterns of one step are ored",
        "<xs:element name='r'><xs:simpleType><xs:restriction base='xs:string'><xs:pattern value='a'/><xs:pattern value='b'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>b</r>",
        "V",
    ),
    (
        "patterns across steps are anded",
        "<xs:simpleType name='t'><xs:restriction base='xs:string'><xs:pattern value='[a-z]+'/></xs:restriction></xs:simpleType><xs:element name='r'><xs:simpleType><xs:restriction base='t'><xs:pattern value='.{2}'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>A1</r>",
        "V 1839",
    ),
    (
        "length",
        "<xs:element name='r'><xs:simpleType><xs:restriction base='xs:string'><xs:length value='2'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>abc</r>",
        "V 1830",
    ),
    (
        "minLength",
        "<xs:element name='r'><xs:simpleType><xs:restriction base='xs:string'><xs:minLength value='2'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>a</r>",
        "V 1831",
    ),
    (
        "maxLength",
        "<xs:element name='r'><xs:simpleType><xs:restriction base='xs:string'><xs:maxLength value='2'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>abc</r>",
        "V 1832",
    ),
    (
        "minInclusive",
        "<xs:element name='r'><xs:simpleType><xs:restriction base='xs:int'><xs:minInclusive value='3'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>2</r>",
        "V 1833",
    ),
    (
        "maxInclusive",
        "<xs:element name='r'><xs:simpleType><xs:restriction base='xs:int'><xs:maxInclusive value='3'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>4</r>",
        "V 1834",
    ),
    (
        "minExclusive",
        "<xs:element name='r'><xs:simpleType><xs:restriction base='xs:int'><xs:minExclusive value='3'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>3</r>",
        "V 1835",
    ),
    (
        "maxExclusive",
        "<xs:element name='r'><xs:simpleType><xs:restriction base='xs:int'><xs:maxExclusive value='3'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>3</r>",
        "V 1836",
    ),
    (
        "totalDigits",
        "<xs:element name='r'><xs:simpleType><xs:restriction base='xs:decimal'><xs:totalDigits value='2'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>1.23</r>",
        "V 1837",
    ),
    (
        "fractionDigits",
        "<xs:element name='r'><xs:simpleType><xs:restriction base='xs:decimal'><xs:fractionDigits value='1'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>1.23</r>",
        "V 1838",
    ),
    (
        "whiteSpace collapse before length",
        "<xs:element name='r'><xs:simpleType><xs:restriction base='xs:string'><xs:whiteSpace value='collapse'/><xs:length value='3'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>  a b  </r>",
        "V",
    ),
    (
        "date bounds",
        "<xs:element name='r'><xs:simpleType><xs:restriction base='xs:date'><xs:maxInclusive value='2000-01-01'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>2001-01-01</r>",
        "V 1834",
    ),
    (
        "facet not allowed for type",
        "<xs:simpleType name='t'><xs:restriction base='xs:boolean'><xs:length value='1'/></xs:restriction></xs:simpleType>",
        "<r/>",
        "S 3013",
    ),
    (
        "facet value invalid for base",
        "<xs:simpleType name='t'><xs:restriction base='xs:int'><xs:maxInclusive value='x'/></xs:restriction></xs:simpleType>",
        "<r/>",
        "S 1824 1717",
    ),
    (
        "facet value outside base facet",
        "<xs:simpleType name='b'><xs:restriction base='xs:int'><xs:maxInclusive value='5'/></xs:restriction></xs:simpleType><xs:simpleType name='t'><xs:restriction base='b'><xs:maxInclusive value='9'/></xs:restriction></xs:simpleType>",
        "<r/>",
        "S 1834 1717",
    ),
    (
        "minLength above base minLength",
        "<xs:simpleType name='b'><xs:restriction base='xs:string'><xs:minLength value='5'/></xs:restriction></xs:simpleType><xs:simpleType name='t'><xs:restriction base='b'><xs:minLength value='2'/></xs:restriction></xs:simpleType>",
        "<r/>",
        "S 1717",
    ),
    (
        "length with minLength",
        "<xs:simpleType name='t'><xs:restriction base='xs:string'><xs:length value='2'/><xs:minLength value='1'/></xs:restriction></xs:simpleType>",
        "<r/>",
        "S 1717",
    ),
    (
        "max below min",
        "<xs:simpleType name='t'><xs:restriction base='xs:int'><xs:minInclusive value='5'/><xs:maxInclusive value='1'/></xs:restriction></xs:simpleType>",
        "<r/>",
        "S 1717",
    ),
    (
        "fixed facet changed",
        "<xs:simpleType name='b'><xs:restriction base='xs:int'><xs:maxInclusive value='5' fixed='true'/></xs:restriction></xs:simpleType><xs:simpleType name='t'><xs:restriction base='b'><xs:maxInclusive value='4'/></xs:restriction></xs:simpleType>",
        "<r/>",
        "S 1717",
    ),
    (
        "whitespace is not compared with built-in bases",
        "<xs:simpleType name='t'><xs:restriction base='xs:token'><xs:whiteSpace value='preserve'/></xs:restriction></xs:simpleType>",
        "<r/>",
        "V 1845",
    ),
    (
        "invalid pattern",
        "<xs:simpleType name='t'><xs:restriction base='xs:string'><xs:pattern value='(a'/></xs:restriction></xs:simpleType>",
        "<r/>",
        "S 1756",
    ),
    (
        "facet without value",
        "<xs:simpleType name='t'><xs:restriction base='xs:string'><xs:length/></xs:restriction></xs:simpleType>",
        "<r/>",
        "S 1708",
    ),
    (
        "restriction of anySimpleType",
        "<xs:simpleType name='t'><xs:restriction base='xs:anySimpleType'/></xs:simpleType>",
        "<r/>",
        "S 3008",
    ),
    (
        "base is a complex type",
        "<xs:complexType name='c'/><xs:simpleType name='t'><xs:restriction base='c'/></xs:simpleType>",
        "<r/>",
        "S 3008",
    ),
    (
        "final restriction",
        "<xs:simpleType name='b' final='restriction'><xs:restriction base='xs:int'/></xs:simpleType><xs:simpleType name='t'><xs:restriction base='b'/></xs:simpleType>",
        "<r/>",
        "S 3010",
    ),
    (
        "circular simple types",
        "<xs:simpleType name='a'><xs:restriction base='b'/></xs:simpleType><xs:simpleType name='b'><xs:restriction base='a'/></xs:simpleType>",
        "<r/>",
        "S 3009",
    ),
    (
        "restriction without base",
        "<xs:simpleType name='t'><xs:restriction/></xs:simpleType>",
        "<r/>",
        "S 3005",
    ),
    (
        "empty simpleType",
        "<xs:simpleType name='t'/>",
        "<r/>",
        "S 3034",
    ),
    (
        "list of int",
        "<xs:element name='r'><xs:simpleType><xs:list itemType='xs:int'/></xs:simpleType></xs:element>",
        "<r> 1 2  3 </r>",
        "V",
    ),
    (
        "list item invalid",
        "<xs:element name='r'><xs:simpleType><xs:list itemType='xs:int'/></xs:simpleType></xs:element>",
        "<r>1 x</r>",
        "V 1824 1825",
    ),
    (
        "list length",
        "<xs:simpleType name='l'><xs:list itemType='xs:int'/></xs:simpleType><xs:element name='r'><xs:simpleType><xs:restriction base='l'><xs:length value='2'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>1 2 3</r>",
        "V 1830 1825",
    ),
    (
        "list enumeration",
        "<xs:simpleType name='l'><xs:list itemType='xs:int'/></xs:simpleType><xs:element name='r'><xs:simpleType><xs:restriction base='l'><xs:enumeration value='1 2'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>01 2</r>",
        "V",
    ),
    (
        "list of list",
        "<xs:simpleType name='l'><xs:list itemType='xs:int'/></xs:simpleType><xs:simpleType name='t'><xs:list itemType='l'/></xs:simpleType>",
        "<r/>",
        "S 3015",
    ),
    (
        "list bound facet",
        "<xs:simpleType name='l'><xs:list itemType='xs:int'/></xs:simpleType><xs:simpleType name='t'><xs:restriction base='l'><xs:maxInclusive value='1'/></xs:restriction></xs:simpleType>",
        "<r/>",
        "S 3021",
    ),
    (
        "list with both item forms",
        "<xs:simpleType name='t'><xs:list itemType='xs:int'><xs:simpleType><xs:restriction base='xs:int'/></xs:simpleType></xs:list></xs:simpleType>",
        "<r/>",
        "S 3000",
    ),
    (
        "builtin list type",
        "<xs:element name='r' type='xs:NMTOKENS'/>",
        "<r>a b c</r>",
        "V",
    ),
    (
        "union",
        "<xs:element name='r'><xs:simpleType><xs:union memberTypes='xs:int xs:boolean'/></xs:simpleType></xs:element>",
        "<r>true</r>",
        "V",
    ),
    (
        "union no member",
        "<xs:element name='r'><xs:simpleType><xs:union memberTypes='xs:int xs:boolean'/></xs:simpleType></xs:element>",
        "<r>x</r>",
        "V 1826",
    ),
    (
        "union pattern",
        "<xs:simpleType name='u'><xs:union memberTypes='xs:int xs:boolean'/></xs:simpleType><xs:element name='r'><xs:simpleType><xs:restriction base='u'><xs:pattern value='\\d+'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>true</r>",
        "V 1839 1826",
    ),
    (
        "union of list",
        "<xs:simpleType name='l'><xs:list itemType='xs:int'/></xs:simpleType><xs:element name='r'><xs:simpleType><xs:union memberTypes='l xs:date'/></xs:simpleType></xs:element>",
        "<r>1 2</r>",
        "V",
    ),
    (
        "union without members",
        "<xs:simpleType name='t'><xs:union/></xs:simpleType>",
        "<r/>",
        "S 3007",
    ),
    (
        "circular union",
        "<xs:simpleType name='a'><xs:union memberTypes='b'/></xs:simpleType><xs:simpleType name='b'><xs:union memberTypes='a'/></xs:simpleType>",
        "<r/>",
        "S 3003 3003",
    ),
    (
        "union length facet",
        "<xs:simpleType name='u'><xs:union memberTypes='xs:int'/></xs:simpleType><xs:simpleType name='t'><xs:restriction base='u'><xs:length value='1'/></xs:restriction></xs:simpleType>",
        "<r/>",
        "S 3029",
    ),
    (
        "QName in scope",
        "<xs:element name='r' type='xs:QName'/>",
        "<r xmlns:p='urn:p'>p:a</r>",
        "V",
    ),
    (
        "QName unbound prefix",
        "<xs:element name='r' type='xs:QName'/>",
        "<r>p:a</r>",
        "V 1824 1824",
    ),
    (
        "QName enumeration uses namespaces",
        "<xs:element name='r'><xs:simpleType><xs:restriction base='xs:QName'><xs:enumeration xmlns:s='urn:p' value='s:a'/></xs:restriction></xs:simpleType></xs:element>",
        "<r xmlns:q='urn:p'>q:a</r>",
        "V",
    ),
    (
        "ENTITY in element content",
        "<xs:element name='r' type='xs:ENTITY'/>",
        "<r>e</r>",
        "V 1824",
    ),
    // ---- complex types ----------------------------------------------------------
    (
        "empty content",
        "<xs:element name='r'><xs:complexType/></xs:element>",
        "<r/>",
        "V",
    ),
    (
        "empty content with whitespace",
        "<xs:element name='r'><xs:complexType/></xs:element>",
        "<r> </r>",
        "V 1841",
    ),
    (
        "empty content with child",
        "<xs:element name='r'><xs:complexType/></xs:element>",
        "<r><a/></r>",
        "V 1841",
    ),
    (
        "element-only with text",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a'/></xs:sequence></xs:complexType></xs:element>",
        "<r>t<a/></r>",
        "V 1843",
    ),
    (
        "element-only with blank text",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a'/></xs:sequence></xs:complexType></xs:element>",
        "<r> <a/> </r>",
        "V",
    ),
    (
        "mixed content",
        "<xs:element name='r'><xs:complexType mixed='true'><xs:sequence><xs:element name='a'/></xs:sequence></xs:complexType></xs:element>",
        "<r>t<a/>u</r>",
        "V",
    ),
    (
        "mixed empty particle",
        "<xs:element name='r'><xs:complexType mixed='true'/></xs:element>",
        "<r>text</r>",
        "V",
    ),
    (
        "simple content extension",
        "<xs:element name='r'><xs:complexType><xs:simpleContent><xs:extension base='xs:int'><xs:attribute name='a'/></xs:extension></xs:simpleContent></xs:complexType></xs:element>",
        "<r a='x'>3</r>",
        "V",
    ),
    (
        "simple content invalid value",
        "<xs:element name='r'><xs:complexType><xs:simpleContent><xs:extension base='xs:int'/></xs:simpleContent></xs:complexType></xs:element>",
        "<r>x</r>",
        "V 1824",
    ),
    (
        "simple content with child",
        "<xs:element name='r'><xs:complexType><xs:simpleContent><xs:extension base='xs:int'/></xs:simpleContent></xs:complexType></xs:element>",
        "<r><a/></r>",
        "V 1842 1824",
    ),
    (
        "simple content restriction",
        "<xs:complexType name='b'><xs:simpleContent><xs:extension base='xs:int'/></xs:simpleContent></xs:complexType><xs:element name='r'><xs:complexType><xs:simpleContent><xs:restriction base='b'><xs:maxInclusive value='3'/></xs:restriction></xs:simpleContent></xs:complexType></xs:element>",
        "<r>4</r>",
        "V 1834",
    ),
    (
        "simple content restriction of simple type",
        "<xs:complexType name='t'><xs:simpleContent><xs:restriction base='xs:int'/></xs:simpleContent></xs:complexType>",
        "<r/>",
        "S 3076",
    ),
    (
        "complex content of simple type",
        "<xs:complexType name='t'><xs:complexContent><xs:extension base='xs:int'/></xs:complexContent></xs:complexType>",
        "<r/>",
        "S 3076",
    ),
    (
        "extension appends particles",
        "<xs:complexType name='b'><xs:sequence><xs:element name='a'/></xs:sequence></xs:complexType><xs:element name='r'><xs:complexType><xs:complexContent><xs:extension base='b'><xs:sequence><xs:element name='c'/></xs:sequence></xs:extension></xs:complexContent></xs:complexType></xs:element>",
        "<r><a/><c/></r>",
        "V",
    ),
    (
        "extension order",
        "<xs:complexType name='b'><xs:sequence><xs:element name='a'/></xs:sequence></xs:complexType><xs:element name='r'><xs:complexType><xs:complexContent><xs:extension base='b'><xs:sequence><xs:element name='c'/></xs:sequence></xs:extension></xs:complexContent></xs:complexType></xs:element>",
        "<r><c/><a/></r>",
        "V 1871",
    ),
    (
        "extension of final type",
        "<xs:complexType name='b' final='extension'/><xs:complexType name='t'><xs:complexContent><xs:extension base='b'/></xs:complexContent></xs:complexType>",
        "<r/>",
        "S 3063",
    ),
    (
        "extension mixing content kinds",
        "<xs:complexType name='b' mixed='true'><xs:sequence><xs:element name='a'/></xs:sequence></xs:complexType><xs:complexType name='t'><xs:complexContent><xs:extension base='b'><xs:sequence><xs:element name='c'/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>",
        "<r/>",
        "S 3063",
    ),
    (
        "restriction of final type",
        "<xs:complexType name='b' final='restriction'/><xs:complexType name='t'><xs:complexContent><xs:restriction base='b'/></xs:complexContent></xs:complexType>",
        "<r/>",
        "S 1787",
    ),
    (
        "restriction mixed of element-only",
        "<xs:complexType name='b'><xs:sequence><xs:element name='a'/></xs:sequence></xs:complexType><xs:complexType name='t' mixed='true'><xs:complexContent><xs:restriction base='b'><xs:sequence><xs:element name='a'/></xs:sequence></xs:restriction></xs:complexContent></xs:complexType>",
        "<r/>",
        "S 1787",
    ),
    (
        "restriction to empty of non-emptiable",
        "<xs:complexType name='b'><xs:sequence><xs:element name='a'/></xs:sequence></xs:complexType><xs:complexType name='t'><xs:complexContent><xs:restriction base='b'/></xs:complexContent></xs:complexType>",
        "<r/>",
        "S 1787",
    ),
    (
        "abstract type",
        "<xs:complexType name='t' abstract='true'/><xs:element name='r' type='t'/>",
        "<r/>",
        "V 1876",
    ),
    (
        "circular complex types",
        "<xs:complexType name='a'><xs:complexContent><xs:extension base='b'/></xs:complexContent></xs:complexType><xs:complexType name='b'><xs:complexContent><xs:extension base='a'/></xs:complexContent></xs:complexType>",
        "<r/>",
        "S 3009",
    ),
    (
        "simpleContent and mixed",
        "<xs:element name='r'><xs:complexType mixed='true'><xs:simpleContent><xs:extension base='xs:int'/></xs:simpleContent></xs:complexType></xs:element>",
        "<r>1</r>",
        "V",
    ),
    (
        "redefined content element",
        "<xs:complexType name='t'><xs:sequence/><xs:bogus/></xs:complexType>",
        "<r/>",
        "S 3033",
    ),
    // ---- model groups ----------------------------------------------------------
    (
        "sequence order",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a'/><xs:element name='b'/></xs:sequence></xs:complexType></xs:element>",
        "<r><b/><a/></r>",
        "V 1871",
    ),
    (
        "sequence missing",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a'/><xs:element name='b'/></xs:sequence></xs:complexType></xs:element>",
        "<r><a/></r>",
        "V 1871",
    ),
    (
        "choice",
        "<xs:element name='r'><xs:complexType><xs:choice><xs:element name='a'/><xs:element name='b'/></xs:choice></xs:complexType></xs:element>",
        "<r><b/></r>",
        "V",
    ),
    (
        "choice twice",
        "<xs:element name='r'><xs:complexType><xs:choice><xs:element name='a'/><xs:element name='b'/></xs:choice></xs:complexType></xs:element>",
        "<r><a/><b/></r>",
        "V 1871",
    ),
    (
        "counted element",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a' minOccurs='2' maxOccurs='3'/></xs:sequence></xs:complexType></xs:element>",
        "<r><a/><a/><a/></r>",
        "V",
    ),
    (
        "counted element too few",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a' minOccurs='2' maxOccurs='3'/></xs:sequence></xs:complexType></xs:element>",
        "<r><a/></r>",
        "V 1871",
    ),
    (
        "counted element too many",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a' minOccurs='2' maxOccurs='3'/></xs:sequence></xs:complexType></xs:element>",
        "<r><a/><a/><a/><a/></r>",
        "V 1871",
    ),
    (
        "counted sequence",
        "<xs:element name='r'><xs:complexType><xs:sequence maxOccurs='2'><xs:element name='a'/><xs:element name='b' minOccurs='0'/></xs:sequence></xs:complexType></xs:element>",
        "<r><a/><a/><b/></r>",
        "V",
    ),
    (
        "counted sequence too many",
        "<xs:element name='r'><xs:complexType><xs:sequence maxOccurs='2'><xs:element name='a'/><xs:element name='b' minOccurs='0'/></xs:sequence></xs:complexType></xs:element>",
        "<r><a/><a/><a/></r>",
        "V 1871",
    ),
    (
        "nested counters",
        "<xs:element name='r'><xs:complexType><xs:sequence minOccurs='2' maxOccurs='2'><xs:element name='a' minOccurs='2' maxOccurs='3'/></xs:sequence></xs:complexType></xs:element>",
        "<r><a/><a/><a/><a/></r>",
        "V",
    ),
    (
        "nested counters too few",
        "<xs:element name='r'><xs:complexType><xs:sequence minOccurs='2' maxOccurs='2'><xs:element name='a' minOccurs='2' maxOccurs='3'/></xs:sequence></xs:complexType></xs:element>",
        "<r><a/><a/><a/></r>",
        "V 1871",
    ),
    (
        "unbounded with minimum",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a' minOccurs='3' maxOccurs='unbounded'/></xs:sequence></xs:complexType></xs:element>",
        "<r><a/><a/></r>",
        "V 1871",
    ),
    (
        "huge maxOccurs",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a' maxOccurs='1000000'/><xs:element name='b'/></xs:sequence></xs:complexType></xs:element>",
        "<r><a/><a/><b/></r>",
        "V",
    ),
    (
        "all group any order",
        "<xs:element name='r'><xs:complexType><xs:all><xs:element name='a'/><xs:element name='b'/></xs:all></xs:complexType></xs:element>",
        "<r><b/><a/></r>",
        "V",
    ),
    (
        "all group repeated",
        "<xs:element name='r'><xs:complexType><xs:all><xs:element name='a'/><xs:element name='b'/></xs:all></xs:complexType></xs:element>",
        "<r><a/><a/></r>",
        "V 1871",
    ),
    (
        "all group missing",
        "<xs:element name='r'><xs:complexType><xs:all><xs:element name='a'/><xs:element name='b' minOccurs='0'/></xs:all></xs:complexType></xs:element>",
        "<r><b/></r>",
        "V 1871",
    ),
    (
        "all group optional",
        "<xs:element name='r'><xs:complexType><xs:all minOccurs='0'><xs:element name='a'/></xs:all></xs:complexType></xs:element>",
        "<r/>",
        "V",
    ),
    (
        "all maxOccurs 2",
        "<xs:element name='r'><xs:complexType><xs:all><xs:element name='a' maxOccurs='2'/></xs:all></xs:complexType></xs:element>",
        "<r/>",
        "S 3091",
    ),
    (
        "all inside sequence",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:all/></xs:sequence></xs:complexType></xs:element>",
        "<r/>",
        "S 3033",
    ),
    (
        "group reference",
        "<xs:group name='g'><xs:sequence><xs:element name='a'/></xs:sequence></xs:group><xs:element name='r'><xs:complexType><xs:sequence><xs:group ref='g' maxOccurs='2'/></xs:sequence></xs:complexType></xs:element>",
        "<r><a/><a/></r>",
        "V",
    ),
    (
        "group referencing all",
        "<xs:group name='g'><xs:all><xs:element name='a'/></xs:all></xs:group><xs:element name='r'><xs:complexType><xs:sequence><xs:group ref='g'/></xs:sequence></xs:complexType></xs:element>",
        "<r/>",
        "S 3091",
    ),
    (
        "circular groups",
        "<xs:group name='g'><xs:sequence><xs:group ref='h'/></xs:sequence></xs:group><xs:group name='h'><xs:sequence><xs:group ref='g'/></xs:sequence></xs:group>",
        "<r/>",
        "S 3075",
    ),
    (
        "unknown group",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:group ref='g'/></xs:sequence></xs:complexType></xs:element>",
        "<r/>",
        "S 3004",
    ),
    (
        "UPA violation",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a' minOccurs='0'/><xs:element name='a'/></xs:sequence></xs:complexType></xs:element>",
        "<r/>",
        "S 3070",
    ),
    (
        "UPA choice",
        "<xs:element name='r'><xs:complexType><xs:choice><xs:element name='a'/><xs:sequence><xs:element name='a'/><xs:element name='b'/></xs:sequence></xs:choice></xs:complexType></xs:element>",
        "<r/>",
        "S 3070",
    ),
    (
        "UPA wildcard overlap",
        "<xs:element name='r'><xs:complexType><xs:choice><xs:element name='a'/><xs:any namespace='##local'/></xs:choice></xs:complexType></xs:element>",
        "<r/>",
        "S 3070",
    ),
    (
        "min greater than max",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a' minOccurs='3' maxOccurs='2'/></xs:sequence></xs:complexType></xs:element>",
        "<r/>",
        "S 3043",
    ),
    (
        "maxOccurs zero",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a' minOccurs='0' maxOccurs='0'/></xs:sequence></xs:complexType></xs:element>",
        "<r/>",
        "V",
    ),
    (
        "invalid maxOccurs",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a' maxOccurs='-1'/></xs:sequence></xs:complexType></xs:element>",
        "<r/>",
        "S 3037",
    ),
    (
        "element ref",
        "<xs:element name='a' type='xs:int'/><xs:element name='r'><xs:complexType><xs:sequence><xs:element ref='a'/></xs:sequence></xs:complexType></xs:element>",
        "<r><a>x</a></r>",
        "V 1824",
    ),
    (
        "element ref with type",
        "<xs:element name='a'/><xs:element name='r'><xs:complexType><xs:sequence><xs:element ref='a' type='xs:int'/></xs:sequence></xs:complexType></xs:element>",
        "<r/>",
        "S 3040",
    ),
    (
        "unexpected element skips rest",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a' type='xs:int'/></xs:sequence></xs:complexType></xs:element>",
        "<r><b/><a>x</a></r>",
        "V 1871",
    ),
    (
        "qualified local elements",
        "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema' targetNamespace='urn:t' elementFormDefault='qualified'><xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a'/></xs:sequence></xs:complexType></xs:element></xs:schema>",
        "<r xmlns='urn:t'><a/></r>",
        "V",
    ),
    (
        "unqualified local elements",
        "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema' targetNamespace='urn:t'><xs:element name='r'><xs:complexType><xs:sequence><xs:element name='a'/></xs:sequence></xs:complexType></xs:element></xs:schema>",
        "<r xmlns='urn:t'><a/></r>",
        "V 1871",
    ),
    // ---- wildcards ----------------------------------------------------------
    (
        "any lax",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:any processContents='lax' maxOccurs='unbounded'/></xs:sequence></xs:complexType></xs:element>",
        "<r><x a='1'><y/></x></r>",
        "V",
    ),
    (
        "any strict undeclared",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:any/></xs:sequence></xs:complexType></xs:element>",
        "<r><x/></r>",
        "V 1845",
    ),
    (
        "any strict declared",
        "<xs:element name='x' type='xs:int'/><xs:element name='r'><xs:complexType><xs:sequence><xs:any/></xs:sequence></xs:complexType></xs:element>",
        "<r><x>q</x></r>",
        "V 1824",
    ),
    (
        "any skip",
        "<xs:element name='x' type='xs:int'/><xs:element name='r'><xs:complexType><xs:sequence><xs:any processContents='skip'/></xs:sequence></xs:complexType></xs:element>",
        "<r><x>q</x></r>",
        "V",
    ),
    (
        "any other namespace",
        "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema' targetNamespace='urn:t'><xs:element name='r'><xs:complexType><xs:sequence><xs:any namespace='##other' processContents='lax'/></xs:sequence></xs:complexType></xs:element></xs:schema>",
        "<r xmlns='urn:t'><x/></r>",
        "V 1871",
    ),
    (
        "any other excludes no namespace",
        "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema' targetNamespace='urn:t'><xs:element name='r'><xs:complexType><xs:sequence><xs:any namespace='##other' processContents='lax'/></xs:sequence></xs:complexType></xs:element></xs:schema>",
        "<t:r xmlns:t='urn:t'><x/></t:r>",
        "V 1871",
    ),
    (
        "any list",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:any namespace='urn:a ##local' processContents='skip' maxOccurs='2'/></xs:sequence></xs:complexType></xs:element>",
        "<r><x/><y xmlns='urn:a'/></r>",
        "V",
    ),
    (
        "invalid processContents",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:any processContents='maybe'/></xs:sequence></xs:complexType></xs:element>",
        "<r/>",
        "S 3037",
    ),
    (
        "any in namespace list",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:any namespace='##any ##local'/></xs:sequence></xs:complexType></xs:element>",
        "<r/>",
        "S 1792",
    ),
    // ---- attributes -----------------------------------------------------------
    (
        "attribute required",
        "<xs:element name='r'><xs:complexType><xs:attribute name='a' use='required'/></xs:complexType></xs:element>",
        "<r/>",
        "V 1868",
    ),
    (
        "attribute undeclared",
        "<xs:element name='r'><xs:complexType/></xs:element>",
        "<r a='1'/>",
        "V 1866",
    ),
    (
        "attribute typed",
        "<xs:element name='r'><xs:complexType><xs:attribute name='a' type='xs:int'/></xs:complexType></xs:element>",
        "<r a='x'/>",
        "V 1824",
    ),
    (
        "attribute fixed",
        "<xs:element name='r'><xs:complexType><xs:attribute name='a' type='xs:int' fixed='1'/></xs:complexType></xs:element>",
        "<r a='01'/>",
        "V",
    ),
    (
        "attribute fixed mismatch",
        "<xs:element name='r'><xs:complexType><xs:attribute name='a' type='xs:int' fixed='1'/></xs:complexType></xs:element>",
        "<r a='2'/>",
        "V 1874",
    ),
    (
        "attribute default",
        "<xs:element name='r'><xs:complexType><xs:attribute name='a' type='xs:int' default='1'/></xs:complexType></xs:element>",
        "<r/>",
        "V",
    ),
    (
        "attribute default invalid",
        "<xs:element name='r'><xs:complexType><xs:attribute name='a' type='xs:int' default='x'/></xs:complexType></xs:element>",
        "<r/>",
        "S 1824 3079",
    ),
    (
        "attribute default with required",
        "<xs:element name='r'><xs:complexType><xs:attribute name='a' default='x' use='required'/></xs:complexType></xs:element>",
        "<r/>",
        "S 3052",
    ),
    (
        "attribute prohibited",
        "<xs:complexType name='b'><xs:attribute name='a'/></xs:complexType><xs:element name='r'><xs:complexType><xs:complexContent><xs:restriction base='b'><xs:attribute name='a' use='prohibited'/></xs:restriction></xs:complexContent></xs:complexType></xs:element>",
        "<r a='1'/>",
        "V 1866",
    ),
    (
        "attribute ref",
        "<xs:attribute name='g' type='xs:int'/><xs:element name='r'><xs:complexType><xs:attribute ref='g'/></xs:complexType></xs:element>",
        "<r g='x'/>",
        "V 1824",
    ),
    (
        "unknown attribute ref",
        "<xs:element name='r'><xs:complexType><xs:attribute ref='g'/></xs:complexType></xs:element>",
        "<r/>",
        "S 3004",
    ),
    (
        "duplicate attribute uses",
        "<xs:element name='r'><xs:complexType><xs:attribute name='a'/><xs:attribute name='a'/></xs:complexType></xs:element>",
        "<r/>",
        "S 3087",
    ),
    (
        "two ID attributes",
        "<xs:element name='r'><xs:complexType><xs:attribute name='a' type='xs:ID'/><xs:attribute name='b' type='xs:ID'/></xs:complexType></xs:element>",
        "<r/>",
        "S 3087",
    ),
    (
        "attribute named xmlns",
        "<xs:element name='r'><xs:complexType><xs:attribute name='xmlns'/></xs:complexType></xs:element>",
        "<r/>",
        "S 3056",
    ),
    (
        "attribute type and inline type",
        "<xs:attribute name='a' type='xs:int'><xs:simpleType><xs:restriction base='xs:int'/></xs:simpleType></xs:attribute>",
        "<r/>",
        "S 3055",
    ),
    (
        "ID attribute with default",
        "<xs:attribute name='a' type='xs:ID' default='x'/>",
        "<r/>",
        "S 3090",
    ),
    (
        "attribute group",
        "<xs:attributeGroup name='g'><xs:attribute name='a' type='xs:int'/></xs:attributeGroup><xs:element name='r'><xs:complexType><xs:attributeGroup ref='g'/></xs:complexType></xs:element>",
        "<r a='x'/>",
        "V 1824",
    ),
    (
        "circular attribute groups",
        "<xs:attributeGroup name='g'><xs:attributeGroup ref='h'/></xs:attributeGroup><xs:attributeGroup name='h'><xs:attributeGroup ref='g'/></xs:attributeGroup>",
        "<r/>",
        "S 3073",
    ),
    (
        "anyAttribute lax",
        "<xs:element name='r'><xs:complexType><xs:anyAttribute processContents='lax'/></xs:complexType></xs:element>",
        "<r a='1' xmlns:p='urn:p' p:b='2'/>",
        "V",
    ),
    (
        "anyAttribute strict",
        "<xs:element name='r'><xs:complexType><xs:anyAttribute/></xs:complexType></xs:element>",
        "<r a='1'/>",
        "V 1878",
    ),
    (
        "anyAttribute strict declared",
        "<xs:attribute name='a' type='xs:int'/><xs:element name='r'><xs:complexType><xs:anyAttribute/></xs:complexType></xs:element>",
        "<r a='x'/>",
        "V 1824",
    ),
    (
        "anyAttribute namespace mismatch",
        "<xs:element name='r'><xs:complexType><xs:anyAttribute namespace='urn:a' processContents='skip'/></xs:complexType></xs:element>",
        "<r b='1'/>",
        "V 1867",
    ),
    (
        "attribute wildcard restriction subset",
        "<xs:complexType name='b'><xs:anyAttribute namespace='urn:a'/></xs:complexType><xs:complexType name='t'><xs:complexContent><xs:restriction base='b'><xs:anyAttribute namespace='urn:b'/></xs:restriction></xs:complexContent></xs:complexType>",
        "<r/>",
        "S 1798",
    ),
    (
        "attribute restriction needs base use",
        "<xs:complexType name='b'/><xs:complexType name='t'><xs:complexContent><xs:restriction base='b'><xs:attribute name='a'/></xs:restriction></xs:complexContent></xs:complexType>",
        "<r/>",
        "S 1790",
    ),
    (
        "required attribute dropped by restriction",
        "<xs:complexType name='b'><xs:attribute name='a' use='required'/></xs:complexType><xs:complexType name='t'><xs:complexContent><xs:restriction base='b'><xs:attribute name='a' use='prohibited'/></xs:restriction></xs:complexContent></xs:complexType>",
        "<r/>",
        "S 1791",
    ),
    (
        "optional restricts required",
        "<xs:complexType name='b'><xs:attribute name='a' use='required'/></xs:complexType><xs:complexType name='t'><xs:complexContent><xs:restriction base='b'><xs:attribute name='a'/></xs:restriction></xs:complexContent></xs:complexType>",
        "<r/>",
        "S 1788",
    ),
    (
        "duplicate ID values",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='e' maxOccurs='2'><xs:complexType><xs:attribute name='id' type='xs:ID'/></xs:complexType></xs:element></xs:sequence></xs:complexType></xs:element>",
        "<r><e id='a'/><e id='a'/></r>",
        "V 1824",
    ),
    (
        "ENTITY names an unparsed entity",
        "<xs:element name='r'><xs:complexType><xs:attribute name='a' type='xs:ENTITY'/></xs:complexType></xs:element>",
        "<!DOCTYPE r [<!NOTATION n SYSTEM 'n'><!ENTITY e SYSTEM 'e' NDATA n>]><r a='e'/>",
        "V",
    ),
    (
        "ENTITY names a parsed entity",
        "<xs:element name='r'><xs:complexType><xs:attribute name='a' type='xs:ENTITY'/></xs:complexType></xs:element>",
        "<!DOCTYPE r [<!ENTITY e 'x'>]><r a='e'/>",
        "V 1824",
    ),
    // ---- xsi:type and xsi:nil ------------------------------------------------
    (
        "xsi:type derived",
        "<xs:complexType name='b'/><xs:complexType name='d'><xs:complexContent><xs:extension base='b'><xs:attribute name='a'/></xs:extension></xs:complexContent></xs:complexType><xs:element name='r' type='b'/>",
        "<r xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' xsi:type='d' a='1'/>",
        "V",
    ),
    (
        "xsi:type unknown",
        "<xs:element name='r'/>",
        "<r xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' xsi:type='nosuch'/>",
        "V 1851",
    ),
    (
        "xsi:type not derived",
        "<xs:element name='r' type='xs:int'/>",
        "<r xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' xmlns:xs='http://www.w3.org/2001/XMLSchema' xsi:type='xs:string'>x</r>",
        "V 1852 1824",
    ),
    (
        "xsi:type builtin restriction",
        "<xs:element name='r' type='xs:decimal'/>",
        "<r xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' xmlns:xs='http://www.w3.org/2001/XMLSchema' xsi:type='xs:int'>1.5</r>",
        "V 1824",
    ),
    (
        "xsi:type blocked",
        "<xs:complexType name='b' block='extension'/><xs:complexType name='d'><xs:complexContent><xs:extension base='b'/></xs:complexContent></xs:complexType><xs:element name='r' type='b'/>",
        "<r xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' xsi:type='d'/>",
        "V 1852",
    ),
    (
        "xsi:type bad QName",
        "<xs:element name='r'/>",
        "<r xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' xsi:type='1x'/>",
        "V 1824",
    ),
    (
        "xsi:nil",
        "<xs:element name='r' type='xs:int' nillable='true'/>",
        "<r xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' xsi:nil='true'/>",
        "V",
    ),
    (
        "xsi:nil with content",
        "<xs:element name='r' type='xs:int' nillable='true'/>",
        "<r xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' xsi:nil='true'>1</r>",
        "V 1848",
    ),
    (
        "xsi:nil not nillable",
        "<xs:element name='r' type='xs:int'/>",
        "<r xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' xsi:nil='true'/>",
        "V 1847 1824",
    ),
    (
        "xsi:nil invalid",
        "<xs:element name='r' type='xs:int' nillable='true'/>",
        "<r xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' xsi:nil='perhaps'>1</r>",
        "V 1824",
    ),
    (
        "xsi:nil with fixed",
        "<xs:element name='r' type='xs:int' nillable='true' fixed='1'/>",
        "<r xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' xsi:nil='true'/>",
        "V 1849",
    ),
    // ---- substitution groups ----------------------------------------------------
    (
        "substitution member",
        "<xs:element name='h' type='xs:string'/><xs:element name='m' substitutionGroup='h'/><xs:element name='r'><xs:complexType><xs:sequence><xs:element ref='h'/></xs:sequence></xs:complexType></xs:element>",
        "<r><m>x</m></r>",
        "V",
    ),
    (
        "abstract head",
        "<xs:element name='h' abstract='true'/><xs:element name='m' substitutionGroup='h'/><xs:element name='r'><xs:complexType><xs:sequence><xs:element ref='h'/></xs:sequence></xs:complexType></xs:element>",
        "<r><h/></r>",
        "V 1846",
    ),
    (
        "blocked substitution",
        "<xs:element name='h' block='substitution'/><xs:element name='m' substitutionGroup='h'/><xs:element name='r'><xs:complexType><xs:sequence><xs:element ref='h'/></xs:sequence></xs:complexType></xs:element>",
        "<r><m/></r>",
        "V 1871",
    ),
    (
        "member type not derived",
        "<xs:element name='h' type='xs:int'/><xs:element name='m' type='xs:boolean' substitutionGroup='h'/>",
        "<r/>",
        "S 3047",
    ),
    (
        "circular substitution group",
        "<xs:element name='a' substitutionGroup='b'/><xs:element name='b' substitutionGroup='a'/>",
        "<r/>",
        "S 3049 3049",
    ),
    (
        "abstract element without members is empty",
        "<xs:element name='h' abstract='true'/><xs:element name='r'><xs:complexType><xs:sequence><xs:element ref='h'/></xs:sequence></xs:complexType></xs:element>",
        "<r/>",
        "V",
    ),
    // ---- identity constraints -------------------------------------------------
    (
        "unique",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='e' maxOccurs='unbounded'><xs:complexType><xs:attribute name='k'/></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:unique name='u'><xs:selector xpath='e'/><xs:field xpath='@k'/></xs:unique></xs:element>",
        "<r><e k='1'/><e k='2'/><e/></r>",
        "V",
    ),
    (
        "unique duplicate",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='e' maxOccurs='unbounded'><xs:complexType><xs:attribute name='k'/></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:unique name='u'><xs:selector xpath='e'/><xs:field xpath='@k'/></xs:unique></xs:element>",
        "<r><e k='1'/><e k='1'/></r>",
        "V 1877",
    ),
    (
        "key compares values",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='e' maxOccurs='unbounded'><xs:complexType><xs:attribute name='k' type='xs:int'/></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:key name='u'><xs:selector xpath='e'/><xs:field xpath='@k'/></xs:key></xs:element>",
        "<r><e k='1'/><e k='01'/></r>",
        "V 1877",
    ),
    (
        "key missing field",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='e' maxOccurs='unbounded'><xs:complexType><xs:attribute name='k'/></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:key name='u'><xs:selector xpath='e'/><xs:field xpath='@k'/></xs:key></xs:element>",
        "<r><e/></r>",
        "V 1877",
    ),
    (
        "keyref",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='e' maxOccurs='unbounded'><xs:complexType><xs:attribute name='k'/><xs:attribute name='f'/></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:key name='k'><xs:selector xpath='e'/><xs:field xpath='@k'/></xs:key><xs:keyref name='f' refer='k'><xs:selector xpath='e'/><xs:field xpath='@f'/></xs:keyref></xs:element>",
        "<r><e k='1'/><e k='2' f='1'/></r>",
        "V",
    ),
    (
        "keyref dangling",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='e' maxOccurs='unbounded'><xs:complexType><xs:attribute name='k'/><xs:attribute name='f'/></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:key name='k'><xs:selector xpath='e'/><xs:field xpath='@k'/></xs:key><xs:keyref name='f' refer='k'><xs:selector xpath='e'/><xs:field xpath='@f'/></xs:keyref></xs:element>",
        "<r><e k='1'/><e k='2' f='3'/></r>",
        "V 1877",
    ),
    (
        "keyref to key in descendant",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='s'><xs:complexType><xs:sequence><xs:element name='e' maxOccurs='unbounded'><xs:complexType><xs:attribute name='k'/></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:key name='k'><xs:selector xpath='e'/><xs:field xpath='@k'/></xs:key></xs:element><xs:element name='f' maxOccurs='unbounded'><xs:complexType><xs:attribute name='to'/></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:keyref name='f' refer='k'><xs:selector xpath='f'/><xs:field xpath='@to'/></xs:keyref></xs:element>",
        "<r><s><e k='1'/></s><f to='1'/><f to='2'/></r>",
        "V 1877",
    ),
    (
        "field element value",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='e' maxOccurs='unbounded' type='xs:int'/></xs:sequence></xs:complexType><xs:unique name='u'><xs:selector xpath='e'/><xs:field xpath='.'/></xs:unique></xs:element>",
        "<r><e>1</e><e> 1</e></r>",
        "V 1877",
    ),
    (
        "field of complex type",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='e' maxOccurs='unbounded'><xs:complexType><xs:sequence><xs:element name='x'/></xs:sequence></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:unique name='u'><xs:selector xpath='e'/><xs:field xpath='.'/></xs:unique></xs:element>",
        "<r><e><x/></e></r>",
        "V 1877",
    ),
    (
        "field matching twice",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='e' maxOccurs='unbounded'><xs:complexType><xs:sequence><xs:element name='x' maxOccurs='2' type='xs:int'/></xs:sequence></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:unique name='u'><xs:selector xpath='e'/><xs:field xpath='x'/></xs:unique></xs:element>",
        "<r><e><x>1</x><x>2</x></e></r>",
        "V 1877",
    ),
    (
        "two fields",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:element name='e' maxOccurs='unbounded'><xs:complexType><xs:attribute name='a'/><xs:attribute name='b'/></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:unique name='u'><xs:selector xpath='e'/><xs:field xpath='@a'/><xs:field xpath='@b'/></xs:unique></xs:element>",
        "<r><e a='1' b='1'/><e a='1' b='2'/></r>",
        "V",
    ),
    (
        "descendant selector",
        "<xs:element name='r'><xs:complexType><xs:sequence><xs:any processContents='skip' maxOccurs='unbounded'/></xs:sequence></xs:complexType><xs:unique name='u'><xs:selector xpath='.//e'/><xs:field xpath='@k'/></xs:unique></xs:element>",
        "<r><x><e k='1'/></x><e k='1'/></r>",
        "V",
    ),
    (
        "keyref refers to keyref",
        "<xs:element name='r'><xs:complexType/><xs:keyref name='a' refer='b'><xs:selector xpath='e'/><xs:field xpath='@k'/></xs:keyref><xs:keyref name='b' refer='a'><xs:selector xpath='e'/><xs:field xpath='@k'/></xs:keyref></xs:element>",
        "<r/>",
        "S 3080 3080",
    ),
    (
        "keyref cardinality",
        "<xs:element name='r'><xs:complexType/><xs:key name='a'><xs:selector xpath='e'/><xs:field xpath='@k'/></xs:key><xs:keyref name='b' refer='a'><xs:selector xpath='e'/><xs:field xpath='@k'/><xs:field xpath='@j'/></xs:keyref></xs:element>",
        "<r/>",
        "S 3080",
    ),
    (
        "selector with attribute",
        "<xs:element name='r'><xs:complexType/><xs:unique name='u'><xs:selector xpath='@a'/><xs:field xpath='@k'/></xs:unique></xs:element>",
        "<r/>",
        "S 3037",
    ),
    (
        "idc without field",
        "<xs:element name='r'><xs:complexType/><xs:unique name='u'><xs:selector xpath='e'/></xs:unique></xs:element>",
        "<r/>",
        "S 3033",
    ),
    // ---- notations ---------------------------------------------------------------
    (
        "notation",
        "<xs:notation name='gif' public='image/gif'/><xs:element name='r'><xs:simpleType><xs:restriction base='xs:NOTATION'><xs:enumeration value='gif'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>gif</r>",
        "V",
    ),
    (
        "undeclared notation",
        "<xs:notation name='gif' public='image/gif'/><xs:element name='r'><xs:simpleType><xs:restriction base='xs:NOTATION'><xs:enumeration value='gif'/></xs:restriction></xs:simpleType></xs:element>",
        "<r>png</r>",
        "V 1824",
    ),
    (
        "notation without name",
        "<xs:notation public='image/gif'/>",
        "<r/>",
        "S 1723",
    ),
    // ---- schema documents --------------------------------------------------------
    ("not a schema", "<schema/>", "<r/>", "S 1772"),
    (
        "invalid targetNamespace",
        "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema' targetNamespace='a b'/>",
        "<r/>",
        "V 1845",
    ),
    (
        "invalid elementFormDefault",
        "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema' elementFormDefault='yes'/>",
        "<r/>",
        "S 1705",
    ),
    (
        "blockDefault applies",
        "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema' blockDefault='#all'><xs:complexType name='b'/><xs:complexType name='d'><xs:complexContent><xs:extension base='b'/></xs:complexContent></xs:complexType><xs:element name='r' type='b'/></xs:schema>",
        "<r xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' xsi:type='d'/>",
        "V 1852",
    ),
    (
        "reference to unimported namespace",
        "<xs:element name='r' xmlns:p='urn:p' type='p:t'/>",
        "<r/>",
        "S 3004",
    ),
    (
        "import self namespace",
        "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema' targetNamespace='urn:t'><xs:import namespace='urn:t'/></xs:schema>",
        "<r/>",
        "S 3064",
    ),
    ("import without namespace", "<xs:import/>", "<r/>", "S 3065"),
    (
        "annotation content",
        "<xs:annotation><xs:bogus/></xs:annotation>",
        "<r/>",
        "S 3033",
    ),
    (
        "annotation attributes",
        "<xs:annotation bogus='1'><xs:documentation xml:lang='en'>x</xs:documentation></xs:annotation><xs:element name='r'/>",
        "<r/>",
        "S 3035",
    ),
    (
        "foreign attributes allowed",
        "<xs:element name='r' xmlns:f='urn:f' f:note='1'/>",
        "<r/>",
        "V",
    ),
    (
        "duplicate schema id",
        "<xs:element name='r' id='x'/><xs:element name='s' id='x'/>",
        "<r/>",
        "S 3037",
    ),
    (
        "empty document element",
        "<xs:element name='r'/>",
        "<!-- --><r/>",
        "V",
    ),
];

#[test]
fn cases_match_libxml2_expectations() {
    let mut failures = Vec::new();
    for &(name, body, instance, want) in CASES {
        let got = outcome(&schema(body), instance);
        if got != want {
            failures.push(format!("{name}: expected {want}, got {got}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn hex(text: &str) -> String {
    match text.is_empty() {
        true => "0".into(),
        false => text.bytes().map(|b| format!("{b:02x}")).collect(),
    }
}

/// The oracle helper's outcome for every case, in the expectations' format.
fn libxml2_outcomes(args: &[&str]) -> Vec<String> {
    let helper = std::env::var("YAXML_XSD_HELPER").unwrap();
    let mut child = Command::new(helper)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = String::new();
    for &(_, body, instance, _) in CASES {
        input.push_str(&format!("{}\t{}\n", hex(&schema(body)), hex(instance)));
    }
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
    let outcomes: Vec<String> = output
        .lines()
        .map(|line| match line.strip_prefix("V ") {
            // The oracle prints the validation return value after "V".
            Some(rest) => {
                let mut parts = rest.split(' ');
                let _ret = parts.next();
                format!("V {}", parts.collect::<Vec<_>>().join(" "))
                    .trim_end()
                    .to_owned()
            }
            None => line.to_owned(),
        })
        .collect();
    assert_eq!(outcomes.len(), CASES.len());
    outcomes
}

#[test]
#[ignore = "requires YAXML_XSD_HELPER pointing at the compiled tooling/libxml2_xsd.c"]
fn libxml2_agrees_with_every_expectation() {
    let mut failures = Vec::new();
    for (&(name, _, _, want), got) in CASES.iter().zip(libxml2_outcomes(&[])) {
        if got != want {
            failures.push(format!("{name}: expected {want}, libxml2 {got}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// libxml2's stream validation (`xmlSchemaValidateStream`) has no nodes, so
/// it never registers an ID and rejects every `ENTITY` value; yaxml's stream
/// validation keeps the tree's verdicts.
#[test]
#[ignore = "requires YAXML_XSD_HELPER pointing at the compiled tooling/libxml2_xsd.c"]
fn libxml2_stream_agrees_except_ids_and_entities() {
    let differences: Vec<&str> = CASES
        .iter()
        .zip(libxml2_outcomes(&["stream"]))
        .filter(|(case, got)| *got != case.3)
        .map(|(case, _)| case.0)
        .collect();
    let quirks = ["duplicate ID values", "ENTITY names an unparsed entity"];
    assert_eq!(differences, quirks);
}

#[test]
fn code_values_are_libxml2_numbers() {
    let table = [
        (Code::S4sElemNotAllowed, 3033),
        (Code::SrcResolve, 3004),
        (Code::NotDeterministic, 3070),
        (Code::ElementContent, 1871),
        (Code::CvcDatatypeValid1_2_1, 1824),
        (Code::CvcIdc, 1877),
    ];
    for (code, value) in table {
        assert_eq!(code as u16, value);
    }
    assert!(Code::WarnSkipSchema.is_warning() && !Code::SrcResolve.is_warning());
}

/// Derivation-OK-restriction 2.1.3: a restricted attribute use must keep the
/// base's fixed value. libxml2 2.15.4 compares the base with itself and never
/// reports it, so these cases stay out of the oracle-checked `CASES`.
#[test]
fn restriction_keeps_the_base_fixed_value() {
    let base = "<xs:complexType name='b'><xs:attribute name='a' type='xs:string' fixed='x'/></xs:complexType>";
    let cases = [
        ("same fixed value", "fixed='x'", false),
        ("fixed value omitted", "", true),
        ("other fixed value", "fixed='y'", true),
        ("default instead of fixed", "default='x'", true),
    ];
    for (name, constraint, rejected) in cases {
        let text = schema(&format!(
            "{base}<xs:complexType name='d'><xs:complexContent><xs:restriction base='b'>\
             <xs:attribute name='a' type='xs:string' {constraint}/></xs:restriction>\
             </xs:complexContent></xs:complexType>"
        ));
        let codes: Vec<Code> = match Schema::parse(text.as_bytes(), "schema.xsd") {
            Ok(_) => Vec::new(),
            Err(error) => error.diagnostics.iter().map(|d| d.code).collect(),
        };
        assert_eq!(
            codes.contains(&Code::DerivationOkRestriction2_1_3),
            rejected,
            "{name}: {codes:?}"
        );
    }
}
