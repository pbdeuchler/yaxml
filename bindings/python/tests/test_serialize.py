import io

import pytest

import yaxml as et


def test_tostring_defaults_to_ascii_with_references():
    root = et.fromstring("<r a='é'>café</r>")
    assert et.tostring(root) == b'<r a="&#233;">caf&#233;</r>'


@pytest.mark.parametrize(
    ("encoding", "expected"),
    [
        ("UTF-8", b"<r>\xc3\xa9</r>"),
        ("utf-8", b"<r>\xc3\xa9</r>"),
        ("ISO-8859-1", b"<?xml version='1.0' encoding='ISO-8859-1'?>\n<r>\xe9</r>"),
        ("US-ASCII", b"<r>&#233;</r>"),
    ],
)
def test_tostring_encodings(encoding, expected):
    assert et.tostring(et.fromstring("<r>é</r>"), encoding=encoding) == expected


def test_tostring_unicode():
    root = et.fromstring("<r>é</r>")
    assert et.tostring(root, encoding="unicode") == "<r>é</r>"
    assert et.tostring(root, encoding=str) == "<r>é</r>"
    with pytest.raises(ValueError):
        et.tostring(root, encoding="unicode", xml_declaration=True)


def test_tostring_unknown_encoding():
    with pytest.raises(LookupError):
        et.tostring(et.Element("r"), encoding="no-such-encoding")


def test_xml_declaration_control():
    root = et.Element("r")
    assert et.tostring(root, xml_declaration=True) == b"<?xml version='1.0' encoding='ASCII'?>\n<r/>"
    assert et.tostring(root, encoding="ISO-8859-1", xml_declaration=False) == b"<r/>"
    assert et.tostring(root, xml_declaration=True, standalone=True).startswith(
        b"<?xml version='1.0' encoding='ASCII' standalone='yes'?>"
    )


def test_pretty_print():
    root = et.fromstring("<r><a><b/></a></r>")
    assert et.tostring(root, pretty_print=True) == b"<r>\n  <a>\n    <b/>\n  </a>\n</r>\n"
    mixed = et.fromstring("<r>t<a/></r>")
    assert et.tostring(mixed, pretty_print=True) == b"<r>t<a/></r>\n"
    tree = et.parse(io.BytesIO(b"<r><a/></r>"))
    assert et.tostring(tree, pretty_print=True) == b"<r>\n  <a/>\n</r>\n"


def test_tree_serialization_keeps_prolog():
    tree = et.parse(io.BytesIO(b"<?xml version='1.0'?><!DOCTYPE r [<!ENTITY e 'x'>]><!--c--><r>&e;</r><?p?>"))
    assert et.tostring(tree) == b'<!DOCTYPE r [\n<!ENTITY e "x">\n]>\n<!--c--><r>&e;</r><?p?>'
    assert et.tostring(tree.getroot()) == b"<r>&e;</r>"


def test_subtree_declares_inherited_namespaces():
    root = et.fromstring("<r xmlns:p='urn:p' xmlns='urn:d'><p:a><b/></p:a></r>")
    assert et.tostring(root[0]) == b'<p:a xmlns:p="urn:p" xmlns="urn:d"><b/></p:a>'


def test_method_text_and_html():
    root = et.fromstring("<r>a<b>c</b>d</r>")
    assert et.tostring(root, method="text") == b"acd"
    assert et.tostring(root, method="text", encoding="unicode") == "acd"
    assert et.tostring(et.fromstring("<r>é</r>"), method="text", encoding="UTF-8") == b"\xc3\xa9"
    with pytest.raises(UnicodeEncodeError):
        et.tostring(et.fromstring("<r>é</r>"), method="text")
    html = et.fromstring("<html><body><br/><p>x</p></body></html>")
    assert et.tostring(html, method="html") == b"<html><body><br><p>x</p></body></html>"
    with pytest.raises(ValueError):
        et.tostring(root, method="bogus")
    with pytest.raises(ValueError):
        et.tostring(root, method="c14n2")


def test_tail_escaping():
    root = et.fromstring("<r><a/></r>")
    root[0].tail = "<&>é"
    assert et.tostring(root[0]) == b"<a/>&lt;&amp;&gt;&#233;"
    assert et.tostring(root[0], encoding="unicode") == "<a/>&lt;&amp;&gt;é"


def test_c14n():
    root = et.fromstring("<r xmlns:p='urn:p' b='2' a='1'><!--c--><p:x/><y/></r>")
    assert et.c14n(root) == b'<r xmlns:p="urn:p" a="1" b="2"><p:x></p:x><y></y></r>'
    assert et.c14n(root, with_comments=True) == b'<r xmlns:p="urn:p" a="1" b="2"><!--c--><p:x></p:x><y></y></r>'
    assert et.c14n(root[2], exclusive=True) == b"<y></y>"
    assert et.c14n(root[2]) == b'<y xmlns:p="urn:p"></y>'
    assert et.c14n(root[2], exclusive=True, inclusive_ns_prefixes=["p"]) == b'<y xmlns:p="urn:p"></y>'
    assert et.c14n(root, version="1.1") == et.c14n(root)
    assert et.tostring(root, method="c14n") == b'<r xmlns:p="urn:p" a="1" b="2"><!--c--><p:x></p:x><y></y></r>'
    with pytest.raises(ValueError):
        et.c14n(root, version="2.0")


def test_c14n_rejects_entity_references():
    root = et.fromstring("<!DOCTYPE r [<!ENTITY e 'x'>]><r>&e;</r>")
    with pytest.raises(et.C14NError):
        et.c14n(root)


def test_write_and_write_c14n(tmp_path):
    tree = et.parse(io.BytesIO(b"<r b='2' a='1'/>"))
    target = tmp_path / "out.xml"
    tree.write(target, encoding="UTF-8", xml_declaration=True)
    assert target.read_bytes() == b"<?xml version='1.0' encoding='UTF-8'?>\n<r b=\"2\" a=\"1\"/>"
    buffer = io.BytesIO()
    tree.write(buffer)
    assert buffer.getvalue() == b'<r b="2" a="1"/>'
    tree.write_c14n(tmp_path / "c.xml")
    assert (tmp_path / "c.xml").read_bytes() == b'<r a="1" b="2"></r>'
    buffer = io.BytesIO()
    tree.write_c14n(buffer, exclusive=True)
    assert buffer.getvalue() == b'<r a="1" b="2"></r>'
    text = io.BytesIO()
    tree.write(text, encoding="unicode")
    assert text.getvalue() == b'<r b="2" a="1"/>'


def test_elementtree_factory():
    root = et.Element("r")
    child = et.SubElement(root, "c")
    tree = et.ElementTree(child)
    assert tree.getroot() is child
    assert et.tostring(et.ElementTree(root)) == b"<r><c/></r>"
    empty = et.ElementTree()
    assert empty.getroot() is None
    with pytest.raises(ValueError):
        empty.find("x")
    with pytest.raises(ValueError):
        empty.docinfo
    empty._setroot(root)
    assert empty.getroot() is root
    parsed = et.ElementTree(file=io.BytesIO(b"<x/>"))
    assert parsed.getroot().tag == "x"
    assert parsed.parser is None
