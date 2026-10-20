import pytest

import yaxml as et

DOC = """<root xmlns:p="urn:p" xmlns="urn:d">
  <item id="1" p:k="a">one</item>
  <item id="2">two<sub>s</sub>tail</item>
  <p:other>three</p:other>
  <!--note-->
  <?pi data?>
</root>"""


@pytest.fixture
def root():
    return et.fromstring(DOC)


NS = {"d": "urn:d", "p": "urn:p"}


def test_xpath_result_types(root):
    items = root.xpath("//d:item", namespaces=NS)
    assert [e.get("id") for e in items] == ["1", "2"]
    assert items[0] is root[0]
    assert root.xpath("count(//d:item)", namespaces=NS) == 2.0
    assert root.xpath("boolean(//p:other)", namespaces=NS) is True
    assert root.xpath("string(//p:other)", namespaces=NS) == "three"
    assert root.xpath("//d:item/@id", namespaces=NS) == ["1", "2"]
    assert root.xpath("//d:item/text()", namespaces=NS) == ["one", "two", "tail"]
    assert [c.text for c in root.xpath("//comment()")] == ["note"]
    assert root.xpath("//processing-instruction()")[0].target == "pi"
    assert ("p", "urn:p") in root.xpath("namespace::*")
    assert root.xpath("/")[0].getroot() is root


def test_xpath_relative_context(root):
    item = root[1]
    assert item.xpath("string(d:sub)", namespaces=NS) == "s"
    assert item.xpath("..")[0] is root


def test_xpath_variables(root):
    assert root.xpath("//d:item[@id=$n]", namespaces=NS, n="2")[0].text == "two"
    assert root.xpath("$x * 2", x=21) == 42.0
    assert root.xpath("$x * 2", x=1.5) == 3.0
    assert root.xpath("$flag", flag=True) is True
    assert root.xpath("count($nodes)", nodes=list(root)) == 5.0
    assert root.xpath("count($one)", one=root[0]) == 1.0
    with pytest.raises(ValueError):
        root.xpath("$x", x=[et.Element("foreign")])


def test_xpath_errors(root):
    with pytest.raises(et.XPathSyntaxError) as info:
        root.xpath("//[")
    assert info.value.code > 1200
    with pytest.raises(et.XPathEvalError):
        root.xpath("//q:item")
    with pytest.raises(et.XPathEvalError):
        root.xpath("$undefined")
    with pytest.raises(et.XPathEvalError):
        root.xpath("unknown-function()")
    with pytest.raises(TypeError):
        root.xpath("//x", namespaces={None: "urn:d"})
    assert issubclass(et.XPathEvalError, et.XPathError)


def test_xpath_class(root):
    find = et.XPath("//d:item[@id=$n]/text()", namespaces=NS)
    assert find.path == "//d:item[@id=$n]/text()"
    assert find(root, n="1") == ["one"]
    assert find(root.getroottree(), n="2") == ["two", "tail"]
    assert repr(find) == find.path
    with pytest.raises(et.XPathSyntaxError):
        et.XPath("1 +")
    with pytest.raises(TypeError):
        find("not an element")


def test_tree_xpath_uses_document_context(root):
    tree = root.getroottree()
    assert tree.xpath("count(d:root)", namespaces=NS) == 1.0
    assert tree.xpath("/d:root", namespaces=NS)[0] is root


# ---- ElementPath (ideas from ElementTree/lxml selftests) ----

SIMPLE = et.fromstring(
    "<body><tag class='a'>text</tag><tag class='b'/><section><tag class='b' id='inner'>"
    "subtext</tag><nested><tag class='c'/></nested></section></body>"
)


@pytest.mark.parametrize(
    ("path", "expected"),
    [
        ("tag", ["a", "b"]),
        ("*", ["a", "b", None]),
        ("./tag", ["a", "b"]),
        (".//tag", ["a", "b", "b", "c"]),
        ("section/tag", ["b"]),
        ("section//tag", ["b", "c"]),
        ("*/tag", ["b"]),
        ("tag[@class]", ["a", "b"]),
        ("tag[@class='b']", ["b"]),
        ('tag[@class="a"]', ["a"]),
        ("tag[@class!='b']", ["a"]),
        (".//tag[@id]", ["b"]),
        ("tag[1]", ["a"]),
        ("tag[2]", ["b"]),
        ("tag[last()]", ["b"]),
        ("tag[last()-1]", ["a"]),
        ("section[tag]", [None]),
        ("section[tag='subtext']", [None]),
        (".//tag[.='text']", ["a"]),
        ("section/..", [None]),
        ("missing", []),
    ],
)
def test_elementpath(path, expected):
    assert [e.get("class") for e in SIMPLE.findall(path)] == expected


def test_find_findtext_iterfind():
    assert SIMPLE.find("tag").get("class") == "a"
    assert SIMPLE.find("missing") is None
    assert SIMPLE.findtext("tag") == "text"
    assert SIMPLE.findtext("tag[2]") == ""
    assert SIMPLE.findtext("missing") is None
    assert SIMPLE.findtext("missing", "default") == "default"
    assert [e.get("class") for e in SIMPLE.iterfind(".//tag")] == ["a", "b", "b", "c"]


def test_elementpath_namespaces():
    root = et.fromstring("<r xmlns='urn:d' xmlns:q='urn:q'><a/><q:b x='1'/></r>")
    assert len(root.findall("{urn:d}a")) == 1
    assert len(root.findall("a")) == 0
    assert len(root.findall("a", namespaces={None: "urn:d"})) == 1
    assert len(root.findall("d:a", namespaces={"d": "urn:d"})) == 1
    assert len(root.findall("{*}b")) == 1
    assert len(root.findall("{urn:q}*")) == 1
    assert len(root.findall("{*}*")) == 2
    assert len(root.findall("q:b[@x='1']", {"q": "urn:q"})) == 1
    with pytest.raises(SyntaxError):
        root.findall("z:a")


def test_elementpath_errors():
    with pytest.raises(SyntaxError):
        SIMPLE.find("/tag")
    with pytest.raises(SyntaxError):
        SIMPLE.find("tag/")
    with pytest.raises(SyntaxError):
        SIMPLE.find("")


def test_tree_find_accepts_absolute_paths():
    tree = SIMPLE.getroottree()
    assert tree.find("/tag").get("class") == "a"
    assert [e.get("class") for e in tree.iterfind("//tag")] == ["a", "b", "b", "c"]
    assert tree.findtext("tag") == "text"
    assert len(tree.findall("tag")) == 2
    assert [e.tag for e in tree.iter("nested")] == ["nested"]
    assert tree.getpath(SIMPLE[2][0]) == "/body/section/tag"
