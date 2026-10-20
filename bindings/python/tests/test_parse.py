import io
import pathlib

import pytest

import yaxml as et


@pytest.mark.parametrize(
    "source",
    [
        b"<r><a/></r>",
        bytearray(b"<r><a/></r>"),
        "<r><a/></r>",
        "  <r><a/></r>",
        io.BytesIO(b"<r><a/></r>"),
        io.StringIO("<r><a/></r>"),
    ],
)
def test_parse_sources(source):
    tree = et.parse(source)
    assert tree.getroot().tag == "r"
    assert [c.tag for c in tree.getroot()] == ["a"]


def test_parse_path_and_str_filename(tmp_path):
    path = tmp_path / "doc.xml"
    path.write_bytes(b"<r>x</r>")
    for source in (path, str(path)):
        tree = et.parse(source)
        assert tree.getroot().text == "x"
        assert tree.docinfo.URL == str(path)


def test_parse_missing_file():
    with pytest.raises(FileNotFoundError):
        et.parse("/nonexistent/yaxml/doc.xml")


def test_fromstring_and_xml():
    assert et.fromstring("<r/>").tag == "r"
    assert et.XML(b"<r/>").tag == "r"
    with pytest.raises(TypeError):
        et.fromstring(42)


def test_str_input_ignores_declared_encoding():
    root = et.fromstring("<?xml version='1.0' encoding='ISO-8859-1'?><r>café</r>")
    assert root.text == "café"


def test_bytes_input_honours_declared_encoding():
    root = et.fromstring(b"<?xml version='1.0' encoding='ISO-8859-1'?><r>caf\xe9</r>")
    assert root.text == "café"


@pytest.mark.parametrize(
    ("text", "code", "line", "column"),
    [
        ("<a>\n  <b></c>\n</a>", 76, 2, 10),
        ("<a", 73, 1, 3),
        ("", 4, 1, 1),
        ("<a/><b/>", 5, 1, 5),
        ("<a x='1' x='2'/>", 42, 1, 15),
    ],
)
def test_syntax_error_fields(text, code, line, column):
    with pytest.raises(et.XMLSyntaxError) as info:
        et.fromstring(text)
    error = info.value
    assert isinstance(error, SyntaxError)
    assert isinstance(error, et.ParseError)
    assert isinstance(error, et.Error)
    assert error.code == code
    assert (error.lineno, error.offset) == (line, column)
    assert error.position == (line, column)
    assert error.domain in (1, 3)
    assert len(error.error_log) == 1
    entry = error.error_log[0]
    assert entry.type == code
    assert entry.line == line
    assert entry.level_name == "FATAL"
    assert entry.domain_name in ("PARSER", "NAMESPACE")
    assert error.error_log.last_error is entry


def test_syntax_error_message_is_libxml2s():
    with pytest.raises(et.XMLSyntaxError) as info:
        et.fromstring("<a>\n  <b></c>\n</a>")
    assert "Opening and ending tag mismatch: b line 2 and c" in str(info.value)


def test_namespace_error_domain():
    with pytest.raises(et.XMLSyntaxError) as info:
        et.fromstring("<p:a/>")
    assert info.value.domain == 3
    assert info.value.code == 201


@pytest.mark.parametrize(
    ("text", "code", "saved"),
    [
        ("<r><p:c/></r>", 201, b"<r><p:c/></r>"),
        ("<r xmlns:p=''/>", 200, b"<r/>"),
        ("<a:b:c/>", 202, b"<a:b:c/>"),
        ("<?a:b?><r/>", 205, b"<r/>"),
    ],
)
def test_namespace_errors_raise_unless_recovering(text, code, saved):
    # As lxml: libxml2 only reports these, lxml raises them.
    with pytest.raises(et.XMLSyntaxError) as info:
        et.fromstring(text)
    assert info.value.code == code
    parser = et.XMLParser(recover=True)
    root = et.fromstring(text, parser)
    assert et.tostring(root) == saved
    assert parser.error_log[0].type == code


def test_recover_keeps_tree_and_logs_errors():
    parser = et.XMLParser(recover=True)
    root = et.fromstring("<r><a></r>", parser)
    assert root.tag == "r"
    assert root[0].tag == "a"
    assert len(parser.error_log) >= 1
    assert parser.error_log[0].type == 76


def test_recover_option_keyword():
    root = et.fromstring("<r><a>&undefined;</a></r>", recover=True)
    assert root[0].tag == "a"


def test_parser_error_log_after_failure():
    parser = et.XMLParser()
    with pytest.raises(et.XMLSyntaxError):
        et.fromstring("<r>", parser)
    assert parser.error_log[0].type == 77


def test_unknown_option_rejected():
    with pytest.raises(TypeError):
        et.fromstring("<r/>", bogus=True)


DTD_DOC = b"<!DOCTYPE r [<!ENTITY e 'ent'><!ATTLIST r d CDATA 'dflt'>]><r>&e;</r>"


def test_entities_kept_without_noent():
    root = et.fromstring(DTD_DOC)
    assert root.text is None
    assert root[0].tag is et.Entity
    assert root[0].text == "&e;"
    assert root[0].name == "e"


def test_noent_substitutes():
    root = et.fromstring(DTD_DOC, noent=True)
    assert root.text == "ent"


def test_dtd_attr_adds_defaults():
    assert et.fromstring(DTD_DOC).attrib == {}
    assert et.fromstring(DTD_DOC, dtd_attr=True).get("d") == "dflt"


def test_xmlparser_defaults_follow_lxml():
    root = et.fromstring(b"<r><![CDATA[x]]>&#65;</r>", et.XMLParser())
    assert et.tostring(root) == b"<r>xA</r>"
    root = et.fromstring(DTD_DOC, et.XMLParser())
    assert root.text == "ent"


def test_no_cdata_and_ns_clean_and_no_blanks():
    assert et.tostring(et.fromstring("<r><![CDATA[<]]></r>", no_cdata=True)) == b"<r>&lt;</r>"
    cleaned = et.fromstring("<r xmlns:p='u'><a xmlns:p='u'/></r>", ns_clean=True)
    assert et.tostring(cleaned) == b'<r xmlns:p="u"><a/></r>'
    blank = et.fromstring("<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]><r>\n  <a/>\n</r>", no_blanks=True)
    assert blank.text is None


def test_huge_option_accepted():
    assert et.fromstring("<r/>", huge=True).tag == "r"


EXTERNAL = b'<!DOCTYPE r SYSTEM "ext.dtd"><r>&ext;</r>'


def test_dtd_load_through_callable_resolver():
    requests = []

    def resolver(uri, public_id, kind):
        requests.append((uri, public_id, kind))
        return b"<!ENTITY ext 'from dtd'>"

    root = et.fromstring(EXTERNAL, dtd_load=True, noent=True, resolver=resolver, base_url="file:///d/doc.xml")
    assert root.text == "from dtd"
    assert requests == [("file:///d/ext.dtd", None, "dtd")]


def test_nothing_loaded_without_resolver():
    root = et.fromstring(EXTERNAL, dtd_load=True, noent=True)
    assert not root.text


def test_resolver_returning_str_and_none():
    root = et.fromstring(
        b'<!DOCTYPE r SYSTEM "x.dtd"><r/>', dtd_load=True, resolver=lambda *a: None
    )
    assert root.tag == "r"
    root = et.fromstring(
        EXTERNAL, dtd_load=True, noent=True, resolver=lambda *a: "<!ENTITY ext 's'>"
    )
    assert root.text == "s"


def test_resolver_exception_propagates():
    class Boom(Exception):
        pass

    def resolver(uri, public_id, kind):
        raise Boom(uri)

    with pytest.raises(Boom):
        et.fromstring(EXTERNAL, dtd_load=True, noent=True, resolver=resolver)


def test_resolver_bad_return_type():
    with pytest.raises(TypeError):
        et.fromstring(EXTERNAL, dtd_load=True, noent=True, resolver=lambda *a: 42)


def test_resolver_must_be_callable():
    with pytest.raises(TypeError):
        et.fromstring("<r/>", resolver=42)


def test_file_resolver(tmp_path):
    (tmp_path / "ext.dtd").write_bytes(b"<!ENTITY ext 'disk'>")
    doc = tmp_path / "doc.xml"
    doc.write_bytes(EXTERNAL)
    resolver = et.FileResolver(tmp_path)
    assert resolver.root == tmp_path
    tree = et.parse(doc, dtd_load=True, noent=True, resolver=resolver, base_url=doc.as_uri())
    assert tree.getroot().text == "disk"


def test_file_resolver_confined(tmp_path):
    inner = tmp_path / "inner"
    inner.mkdir()
    (tmp_path / "secret.dtd").write_bytes(b"<!ENTITY ext 'leak'>")
    text = b'<!DOCTYPE r SYSTEM "../secret.dtd"><r>&ext;</r>'
    with pytest.raises(et.XMLSyntaxError):
        et.fromstring(
            text,
            dtd_load=True,
            noent=True,
            resolver=et.FileResolver(inner),
            base_url=(inner / "doc.xml").as_uri(),
        )


def test_file_resolver_bad_root(tmp_path):
    with pytest.raises(OSError):
        et.FileResolver(tmp_path / "missing")


def test_dtd_validation_errors_raise():
    valid = b"<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]><r><a/></r>"
    assert et.fromstring(valid, dtd_validation=True).tag == "r"
    invalid = b"<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]><r><b/></r>"
    with pytest.raises(et.XMLSyntaxError) as info:
        et.fromstring(invalid, dtd_validation=True)
    assert info.value.error_log[0].domain_name == "VALID"


def test_xinclude(tmp_path):
    (tmp_path / "part.xml").write_bytes(b"<part/>")
    text = b"<r xmlns:xi='http://www.w3.org/2001/XInclude'><xi:include href='part.xml'/></r>"
    root = et.fromstring(
        text, xinclude=True, resolver=et.FileResolver(tmp_path), base_url=(tmp_path / "d.xml").as_uri()
    )
    assert [c.tag for c in root] == ["part"]


def test_utf16_input():
    root = et.fromstring("<r>é</r>".encode("utf-16"))
    assert root.text == "é"


def test_docinfo():
    tree = et.parse(io.BytesIO(b'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><!DOCTYPE r PUBLIC "-//P//EN" "r.dtd"><r/>'))
    info = tree.docinfo
    assert info.xml_version == "1.0"
    assert info.encoding == "UTF-8"
    assert info.standalone is True
    assert info.root_name == "r"
    assert info.public_id == "-//P//EN"
    assert info.system_url == "r.dtd"
    assert info.doctype == '<!DOCTYPE r PUBLIC "-//P//EN" "r.dtd">'
    plain = et.parse(io.BytesIO(b"<r/>")).docinfo
    assert plain.doctype == ""
    assert plain.standalone is None
    assert plain.root_name == "r"


def test_pathlike_is_accepted(tmp_path):
    path = pathlib.Path(tmp_path, "p.xml")
    path.write_text("<r/>")
    assert et.parse(path).getroot().tag == "r"
