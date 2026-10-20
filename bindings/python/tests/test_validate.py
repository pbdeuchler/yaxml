import io

import pytest

import yaxml as et

XSD = b"""<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="r">
    <xs:complexType>
      <xs:sequence><xs:element name="n" type="xs:integer" maxOccurs="unbounded"/></xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"""

RNG = b"""<element name="r" xmlns="http://relaxng.org/ns/structure/1.0">
  <oneOrMore><element name="n"><text/></element></oneOrMore>
</element>"""

DTD_TEXT = b"<!ELEMENT r (n+)><!ELEMENT n (#PCDATA)>"

SCH = b"""<schema xmlns="http://www.ascc.net/xml/schematron">
  <pattern name="p"><rule context="r"><assert test="count(n) &gt; 1">need two n</assert></rule></pattern>
</schema>"""

GOOD = b"<r><n>1</n><n>2</n></r>"
BAD = b"<r>\n<m/></r>"


def validators():
    return [
        et.XMLSchema(et.parse(io.BytesIO(XSD))),
        et.XMLSchema(file=io.BytesIO(XSD)),
        et.RelaxNG(et.fromstring(RNG)),
        et.RelaxNG(file=io.BytesIO(RNG)),
        et.DTD(io.BytesIO(DTD_TEXT)),
        et.DTD(DTD_TEXT),
        et.Schematron(et.fromstring(SCH)),
    ]


@pytest.mark.parametrize("validator", validators(), ids=lambda v: type(v).__name__)
def test_valid_document(validator):
    tree = et.parse(io.BytesIO(GOOD))
    assert validator.validate(tree) is True
    assert validator(tree.getroot()) is True
    assert len(validator.error_log) == 0
    validator.assertValid(tree)
    validator.assert_(tree)


@pytest.mark.parametrize("validator", validators(), ids=lambda v: type(v).__name__)
def test_invalid_document(validator):
    tree = et.parse(io.BytesIO(BAD))
    assert validator.validate(tree) is False
    assert len(validator.error_log) >= 1
    entry = validator.error_log[0]
    assert entry.message
    assert entry.level >= 2
    with pytest.raises(et.DocumentInvalid) as info:
        validator.assertValid(tree)
    assert len(info.value.error_log) >= 1


def test_validation_lines_and_domains():
    schema = et.XMLSchema(file=io.BytesIO(XSD))
    assert not schema.validate(et.parse(io.BytesIO(BAD)))
    entry = schema.error_log[0]
    assert entry.domain_name == "SCHEMASV"
    assert entry.line == 2
    relax = et.RelaxNG(file=io.BytesIO(RNG))
    assert not relax.validate(et.parse(io.BytesIO(BAD)))
    assert relax.error_log[0].domain_name == "RELAXNGV"
    dtd = et.DTD(DTD_TEXT)
    assert not dtd.validate(et.fromstring(b"<r><m/></r>"))
    assert dtd.error_log[0].domain_name == "VALID"
    assert dtd.name is None
    assert dtd.elements() == ["r", "n"]
    sch = et.Schematron(et.fromstring(SCH))
    assert not sch.validate(et.fromstring(b"<r><n/></r>"))
    assert sch.error_log[0].domain_name == "SCHEMATRONV"
    assert "need two n" in sch.error_log[0].message


def test_built_trees_validate():
    root = et.Element("r")
    et.SubElement(root, "n").text = "5"
    assert et.XMLSchema(file=io.BytesIO(XSD)).validate(root)


@pytest.mark.parametrize(
    ("factory", "error"),
    [
        (lambda: et.XMLSchema(et.fromstring(b"<notaschema/>")), et.XMLSchemaParseError),
        (lambda: et.RelaxNG(et.fromstring(b"<notaschema/>")), et.RelaxNGParseError),
        (lambda: et.DTD(b"<!ELEMENT r (a"), et.DTDParseError),
        (lambda: et.Schematron(et.fromstring(b"<notaschema/>")), et.SchematronParseError),
    ],
)
def test_bad_schemas(factory, error):
    with pytest.raises(error) as info:
        factory()
    assert isinstance(info.value, et.Error)
    assert len(info.value.error_log) >= 1


def test_schema_source_required():
    with pytest.raises(ValueError):
        et.XMLSchema()
    with pytest.raises(ValueError):
        et.DTD()
    with pytest.raises(ValueError):
        et.DTD(external_id="-//X//EN")


def test_schema_include_through_resolver():
    main = b"""<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:include schemaLocation="inc.xsd"/></xs:schema>"""
    seen = []

    def resolver(uri, public_id, kind):
        seen.append((uri, kind))
        return XSD[XSD.index(b"<xs:schema"):] if uri.endswith("inc.xsd") else None

    schema = et.XMLSchema(et.parse(io.BytesIO(main), base_url="file:///s/main.xsd"), resolver=resolver)
    assert schema.validate(et.fromstring(GOOD))
    assert seen == [("file:///s/inc.xsd", "schema")]
    with pytest.raises(et.XMLSchemaParseError):
        et.XMLSchema(et.parse(io.BytesIO(main), base_url="file:///s/main.xsd"))
