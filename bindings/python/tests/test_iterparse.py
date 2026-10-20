import io

import pytest

import yaxml as et

DOC = b"""<?xml version="1.0"?>
<!--prolog-->
<root xmlns="urn:d" xmlns:p="urn:p">
  <a x="1">text</a>
  <p:b/>
  <!--inner-->
  <?pi data?>
</root>
<!--epilog-->"""


def events(source, **kwargs):
    return [(event, item if event in ("start-ns", "end-ns") else getattr(item, "tag", item))
            for event, item in et.iterparse(source, **kwargs)]


def test_default_end_events():
    assert events(io.BytesIO(DOC)) == [("end", "{urn:d}a"), ("end", "{urn:p}b"), ("end", "{urn:d}root")]


def test_all_events():
    got = events(DOC, events=("start", "end", "start-ns", "end-ns", "comment", "pi"))
    assert got == [
        ("comment", et.Comment),
        ("start-ns", ("", "urn:d")),
        ("start-ns", ("p", "urn:p")),
        ("start", "{urn:d}root"),
        ("start", "{urn:d}a"),
        ("end", "{urn:d}a"),
        ("start", "{urn:p}b"),
        ("end", "{urn:p}b"),
        ("comment", et.Comment),
        ("pi", et.PI),
        ("end", "{urn:d}root"),
        ("end-ns", None),
        ("end-ns", None),
        ("comment", et.Comment),
    ]


def test_tag_filter():
    assert events(DOC, events=("start", "end"), tag="{urn:p}b") == [("start", "{urn:p}b"), ("end", "{urn:p}b")]
    assert events(DOC, tag="{*}a") == [("end", "{urn:d}a")]


def test_tree_is_built_while_streaming():
    context = et.iterparse(DOC, events=("start", "end"))
    assert context.root is None
    seen = {}
    for event, element in context:
        if event == "end" and element.tag == "{urn:d}a":
            seen["text"] = element.text
            seen["attrib"] = dict(element.attrib)
            seen["parent"] = element.getparent().tag
    assert seen == {"text": "text", "attrib": {"x": "1"}, "parent": "{urn:d}root"}
    root = context.root
    assert root.tag == "{urn:d}root"
    assert [c.tag for c in root] == ["{urn:d}a", "{urn:p}b", et.Comment, et.PI]
    assert et.tostring(root.getroottree()).endswith(b"</root><!--epilog-->")


def test_clear_while_iterating():
    big = b"<r>" + b"<item><v>x</v></item>" * 1000 + b"</r>"
    count = 0
    context = et.iterparse(big, tag="item")
    for _, element in context:
        count += 1
        element.clear()
        while element.getprevious() is not None:
            del element.getparent()[0]
    assert count == 1000
    assert len(context.root) == 1


def test_file_and_path_sources(tmp_path):
    path = tmp_path / "d.xml"
    path.write_bytes(DOC)
    assert len(events(path)) == 3
    assert len(events(str(path))) == 3
    with open(path, "rb") as handle:
        assert len(events(handle)) == 3
    assert len(events(io.StringIO(DOC.decode()))) == 3


def test_entities_substituted_by_default():
    doc = b"<!DOCTYPE r [<!ENTITY e 'ent'>]><r>&e;</r>"
    assert [el.text for _, el in et.iterparse(doc)] == ["ent"]


def test_syntax_error():
    with pytest.raises(et.XMLSyntaxError) as info:
        for _ in et.iterparse(b"<r><a></r>"):
            pass
    assert info.value.code == 76
    assert info.value.lineno == 1


def test_error_from_file_reader():
    class Broken:
        def read(self, n):
            raise RuntimeError("disk on fire")

    with pytest.raises(RuntimeError):
        list(et.iterparse(Broken()))


def test_bad_arguments(tmp_path):
    with pytest.raises(ValueError):
        et.iterparse(DOC, events=("bogus",))
    with pytest.raises(OSError):
        et.iterparse(tmp_path / "missing.xml")
    with pytest.raises(et.XMLSyntaxError):
        et.iterparse(DOC, recover=True)
    with pytest.raises(TypeError):
        et.iterparse(DOC, tag=42)
