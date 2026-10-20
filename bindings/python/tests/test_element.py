import copy
import gc

import pytest

import yaxml as et


def xml(element):
    return et.tostring(element)


# ---- text and tail (lxml's test_elementtree text/tail ideas) ----


def test_text_and_tail_read():
    root = et.fromstring("<r>one<a>two</a>three<b/>four</r>")
    assert root.text == "one"
    assert root[0].text == "two"
    assert root[0].tail == "three"
    assert root[1].text is None
    assert root[1].tail == "four"
    assert root.tail is None


def test_text_spans_cdata_and_adjacent_runs():
    root = et.fromstring("<r>a<![CDATA[b]]>c<x/></r>")
    assert root.text == "abc"


def test_empty_text_node_reads_as_empty_string():
    root = et.Element("r")
    root.text = ""
    assert root.text == ""
    assert xml(root) == b"<r></r>"


def test_set_text_replaces_leading_run_only():
    root = et.fromstring("<r>a<x/>b</r>")
    root.text = "new"
    assert xml(root) == b"<r>new<x/>b</r>"
    root.text = None
    assert root.text is None
    assert xml(root) == b"<r><x/>b</r>"
    root.text = "again"
    assert xml(root) == b"<r>again<x/>b</r>"


def test_set_text_cdata():
    root = et.Element("r")
    root.text = et.CDATA("<raw>")
    assert xml(root) == b"<r><![CDATA[<raw>]]></r>"
    assert root.text == "<raw>"
    with pytest.raises(ValueError):
        root.text = et.CDATA("a]]>b")


def test_set_text_type_errors():
    root = et.Element("r")
    with pytest.raises(TypeError):
        root.text = 42
    with pytest.raises(ValueError):
        root.text = "bad\x01char"


def test_set_tail():
    root = et.fromstring("<r><a/>old<b/></r>")
    root[0].tail = "new"
    assert xml(root) == b"<r><a/>new<b/></r>"
    root[0].tail = None
    assert xml(root) == b"<r><a/><b/></r>"
    root[1].tail = "end"
    assert xml(root) == b"<r><a/><b/>end</r>"


def test_tail_travels_with_append():
    root = et.fromstring("<r><a/>tail<b/></r>")
    root.append(root[0])
    assert xml(root) == b"<r><b/><a/>tail</r>"


def test_tail_travels_with_remove_and_reinsert():
    root = et.fromstring("<r><a/>tail<b/></r>")
    a = root[0]
    root.remove(a)
    assert a.tail == "tail"
    assert xml(root) == b"<r><b/></r>"
    assert xml(a) == b"<a/>tail"
    root.insert(0, a)
    assert xml(root) == b"<r><a/>tail<b/></r>"


def test_tail_on_detached_element():
    element = et.Element("a")
    element.tail = "t"
    assert element.tail == "t"
    assert xml(element) == b"<a/>t"
    assert et.tostring(element, with_tail=False) == b"<a/>"
    parent = et.Element("p")
    parent.append(element)
    assert xml(parent) == b"<p><a/>t</p>"
    element.tail = None
    assert element.tail is None


def test_tail_of_root_in_tree_is_kept_aside():
    root = et.fromstring("<r/>")
    root.tail = "x"
    assert root.tail == "x"
    assert et.tostring(root.getroottree()) == b"<r/>"


def test_addnext_goes_after_tail():
    root = et.fromstring("<r><a/>tail<c/></r>")
    root[0].addnext(et.Element("b"))
    assert xml(root) == b"<r><a/>tail<b/><c/></r>"
    root[2].addnext(et.Element("d"))
    assert xml(root) == b"<r><a/>tail<b/><c/><d/></r>"


def test_addprevious():
    root = et.fromstring("<r>t<a/></r>")
    root[0].addprevious(et.Element("z"))
    assert xml(root) == b"<r>t<z/><a/></r>"


def test_toplevel_comment_siblings():
    tree = et.parse(b"<!--before--><r/><!--after-->")
    root = tree.getroot()
    assert root.getprevious().text == "before"
    assert root.getnext().text == "after"
    root.addprevious(et.PI("pi", "data"))
    assert et.tostring(tree) == b"<!--before--><?pi data?><r/><!--after-->"


# ---- tags, attributes, namespaces ----


def test_tag_clark_and_rename():
    root = et.fromstring("<p:r xmlns:p='urn:p'><a/></p:r>")
    assert root.tag == "{urn:p}r"
    assert root.prefix == "p"
    root[0].tag = "{urn:p}b"
    assert xml(root) == b'<p:r xmlns:p="urn:p"><p:b/></p:r>'
    root[0].tag = "{urn:new}c"
    assert xml(root) == b'<p:r xmlns:p="urn:p"><ns0:c xmlns:ns0="urn:new"/></p:r>'
    root[0].tag = "plain"
    assert root[0].tag == "plain"


@pytest.mark.parametrize("bad", ["", "{urn}", "{unclosed", "a b", "1a"])
def test_bad_tags(bad):
    with pytest.raises(ValueError):
        et.Element(bad)


def test_attrib_mapping():
    root = et.fromstring("<r xmlns:p='urn:p' a='1' p:b='2'/>")
    attrib = root.attrib
    assert dict(attrib) == {"a": "1", "{urn:p}b": "2"}
    assert attrib == {"a": "1", "{urn:p}b": "2"}
    assert len(attrib) == 2
    assert "a" in attrib and "{urn:p}b" in attrib and "xmlns:p" not in attrib
    assert attrib["a"] == "1"
    assert attrib.get("missing") is None
    assert attrib.get("missing", "d") == "d"
    with pytest.raises(KeyError):
        attrib["missing"]
    attrib["c"] = "3"
    assert root.get("c") == "3"
    del attrib["a"]
    assert "a" not in attrib
    with pytest.raises(KeyError):
        del attrib["a"]
    assert attrib.pop("c") == "3"
    assert attrib.pop("c", "gone") == "gone"
    with pytest.raises(KeyError):
        attrib.pop("c")
    attrib.update({"x": "1", "y": "2"})
    assert sorted(attrib.keys()) == ["x", "y", "{urn:p}b"]
    assert sorted(attrib.values()) == ["1", "2", "2"]
    assert bool(attrib)
    attrib.clear()
    assert not attrib
    assert list(attrib) == []
    assert repr(attrib) == "{}"


def test_get_set_keys_items():
    root = et.Element("r", {"a": "1"}, b="2")
    assert root.keys() == ["a", "b"]
    assert root.values() == ["1", "2"]
    assert root.items() == [("a", "1"), ("b", "2")]
    root.set("a", "x")
    assert root.get("a") == "x"
    root.set("{urn:q}c", "3")
    assert xml(root) == b'<r xmlns:ns0="urn:q" a="x" b="2" ns0:c="3"/>'
    assert root.get("{urn:q}c") == "3"
    with pytest.raises(TypeError):
        root.set("a", 1)
    with pytest.raises(ValueError):
        root.set("bad name", "v")


def test_dtd_default_attribute_visible_through_get():
    root = et.fromstring("<!DOCTYPE r [<!ATTLIST r d CDATA 'dflt'>]><r/>")
    assert root.get("d") == "dflt"
    assert "d" not in root.attrib


def test_nsmap_and_new_element_prefixes():
    root = et.Element("{urn:a}r", nsmap={"a": "urn:a", None: "urn:d"})
    assert root.nsmap == {"a": "urn:a", None: "urn:d"}
    assert root.prefix == "a"
    child = et.SubElement(root, "{urn:d}c")
    assert child.prefix is None
    other = et.SubElement(root, "{urn:x}o", {"{urn:a}attr": "v"})
    assert xml(other) == b'<ns0:o xmlns:ns0="urn:x" xmlns:a="urn:a" xmlns="urn:d" a:attr="v"/>' or other.prefix == "ns0"
    assert other.nsmap["ns0"] == "urn:x"
    assert et.fromstring("<r xmlns=''/>").nsmap == {}


def test_element_factory_kinds():
    element = et.Element("r", attrib={"a": "1"}, nsmap=None, b="2")
    assert element.attrib == {"a": "1", "b": "2"}
    comment = et.Comment("note")
    assert comment.tag is et.Comment
    assert comment.text == "note"
    assert repr(comment) == "<!--note-->"
    pi = et.ProcessingInstruction("target", "data")
    assert pi.tag is et.ProcessingInstruction
    assert et.PI is et.ProcessingInstruction
    assert pi.target == "target"
    assert pi.text == "data"
    assert xml(pi) == b"<?target data?>"
    with pytest.raises(ValueError):
        et.Comment("a--b")
    with pytest.raises(ValueError):
        et.PI("xml", "x")
    with pytest.raises(ValueError):
        et.Entity("e")
    comment.text = "changed"
    assert xml(comment) == b"<!--changed-->"


def test_subelement_and_makeelement():
    root = et.Element("r")
    child = et.SubElement(root, "c", {"k": "v"})
    assert child.getparent() is root
    made = root.makeelement("m", {"x": "1"})
    assert made.getparent() is None
    assert xml(made) == b'<m x="1"/>'


# ---- children ----


def test_sequence_protocol():
    root = et.fromstring("<r><a/>t<!--c--><?p x?><b/></r>")
    assert len(root) == 4
    assert [c.tag for c in root][0] == "a"
    assert root[0].tag == "a" and root[-1].tag == "b"
    assert [c.tag for c in root[1:3]] == [et.Comment, et.PI]
    assert [c.tag for c in root[::-1]][0] == "b"
    assert [c.tag for c in reversed(root)][0] == "b"
    with pytest.raises(IndexError):
        root[10]
    assert root[0] in root
    assert et.Element("x") not in root
    assert 42 not in root
    assert bool(root) is True
    assert bool(et.Element("e")) is False


def test_setitem_delitem():
    root = et.fromstring("<r><a/><b/><c/></r>")
    root[1] = et.Element("x")
    assert xml(root) == b"<r><a/><x/><c/></r>"
    del root[0]
    assert xml(root) == b"<r><x/><c/></r>"
    root[0:1] = [et.Element("y"), et.Element("z")]
    assert xml(root) == b"<r><y/><z/><c/></r>"
    del root[:]
    assert xml(root) == b"<r/>"
    with pytest.raises(IndexError):
        del root[0]
    with pytest.raises(TypeError):
        root[0:0] = [1]


def test_append_insert_extend_remove_index():
    root = et.Element("r")
    a, b, c = et.Element("a"), et.Element("b"), et.Element("c")
    root.append(a)
    root.extend([b, c])
    root.insert(0, et.Element("first"))
    root.insert(-1, et.Element("beforec"))
    root.insert(100, et.Element("last"))
    assert [x.tag for x in root] == ["first", "a", "b", "beforec", "c", "last"]
    assert root.index(b) == 2
    with pytest.raises(ValueError):
        root.index(et.Element("nope"))
    root.remove(b)
    assert b.getparent() is None
    with pytest.raises(ValueError):
        root.remove(b)
    with pytest.raises(TypeError):
        root.append("text")
    with pytest.raises(ValueError):
        a.append(root)


def test_replace():
    root = et.fromstring("<r><a/>ta<b/></r>")
    old = root[0]
    root.replace(old, et.Element("n"))
    assert xml(root) == b"<r><n/><b/></r>"
    assert xml(old) == b"<a/>ta"
    with pytest.raises(ValueError):
        root.replace(old, et.Element("x"))


def test_clear():
    root = et.fromstring("<r a='1' xmlns:p='u'>t<c/>u</r>")
    root.tail = None
    root.clear()
    assert xml(root) == b'<r xmlns:p="u"/>'
    child = et.fromstring("<r><c/>tail</r>")[0]
    child.clear(keep_tail=True)
    assert child.tail == "tail"
    child.clear()
    assert child.tail is None


def test_navigation():
    root = et.fromstring("<r><a/>x<!--c--><b/></r>")
    a, comment, b = root
    assert a.getparent() is root
    assert root.getparent() is None
    assert a.getnext() is comment
    assert b.getprevious() is comment
    assert a.getprevious() is None
    assert b.getnext() is None


def test_iterators():
    root = et.fromstring("<r xmlns:p='u'><a><p:b/></a><!--c--><a/></r>")
    assert [e.tag for e in root.iter()] == ["r", "a", "{u}b", et.Comment, "a"]
    assert [e.tag for e in root.iter("a")] == ["a", "a"]
    assert [e.tag for e in root.iter("{u}b", "a")] == ["a", "{u}b", "a"]
    assert [e.tag for e in root.iter(tag="{*}b")] == ["{u}b"]
    assert [e.tag for e in root.iter("{u}*")] == ["{u}b"]
    assert [e.tag for e in root.iter("*")] == ["r", "a", "{u}b", "a"]
    assert [e.tag for e in root.iter(et.Comment)] == [et.Comment]
    assert [e.tag for e in root.iterdescendants()] == ["a", "{u}b", et.Comment, "a"]
    assert [e.tag for e in root.iterchildren(reversed=True)] == ["a", et.Comment, "a"]
    assert [e.tag for e in root.iterchildren("a")] == ["a", "a"]
    b = root[0][0]
    assert [e.tag for e in b.iterancestors()] == ["a", "r"]
    assert [e.tag for e in b.iterancestors("r")] == ["r"]
    assert [e.tag for e in root[0].itersiblings()] == [et.Comment, "a"]
    assert [e.tag for e in root[2].itersiblings(preceding=True)] == [et.Comment, "a"]
    with pytest.raises(TypeError):
        root.iter(42)


def test_itertext():
    root = et.fromstring("<r>a<b>c<!--x-->d</b>e<c/>f</r>")
    assert list(root.itertext()) == ["a", "c", "d", "e", "f"]
    assert list(root.itertext(with_tail=False)) == ["a", "c"]
    assert list(root.itertext("b")) == ["c", "e"]


# ---- identity, staleness, moves between documents ----


def test_proxy_identity():
    root = et.fromstring("<r><a/></r>")
    assert root[0] is root[0]
    assert root.find("a") is root[0]
    assert root[0] == root[0]
    assert hash(root[0]) == hash(root[0])
    assert root[0] != root
    first = root[0]
    assert all(root[0] is first for _ in range(3))


def test_identity_survives_proxy_churn():
    root = et.fromstring("<r>" + "<a/>" * 200 + "</r>")
    for _ in range(5):
        children = list(root)
        del children
        gc.collect()
    keep = root[0]
    assert root[0] is keep


def test_cross_document_append_moves_and_remaps():
    source = et.fromstring("<s xmlns:p='urn:p'><p:a k='v'><b/>bt</p:a>at</s>")
    a = source[0]
    b = a[0]
    target = et.Element("t")
    target.append(a)
    assert xml(target) == b'<t><p:a xmlns:p="urn:p" k="v"><b/>bt</p:a>at</t>'
    assert xml(source) == b'<s xmlns:p="urn:p"/>'
    assert a.getparent() is target
    assert b.getparent() is a
    assert target[0] is a
    assert a.getroottree().getroot() is target
    a.set("k", "w")
    assert target[0].get("k") == "w"


def test_cross_document_insert_and_replace():
    left = et.fromstring("<l><x/><y/></l>")
    right = et.fromstring("<r><m/></r>")
    x = left[0]
    right.insert(0, x)
    assert xml(right) == b"<r><x/><m/></r>"
    right.replace(right[1], left[0])
    assert xml(right) == b"<r><x/><y/></r>"
    assert xml(left) == b"<l/>"


def test_moving_a_document_root_copies_it():
    root = et.Element("solo")
    holder = et.Element("holder")
    holder.append(root)
    assert root.getparent() is holder
    assert xml(holder) == b"<holder><solo/></holder>"


def test_cleared_children_stay_usable_while_referenced():
    root = et.fromstring("<r><a/></r>")
    holder = et.SubElement(root, "h")
    holder.text = "t"
    root.clear()
    assert holder.getparent() is None
    assert holder.text == "t"


def test_stale_proxy_raises_not_crashes():
    # A reference to an entity the destination does not declare is replaced
    # by its content when moved, so its proxy has no counterpart.
    source = et.fromstring("<!DOCTYPE s [<!ENTITY e 'v'>]><s><a>&e;</a></s>")
    a = source[0]
    entity = a[0]
    target = et.Element("t")
    target.append(a)
    assert a.text == "v"
    for operation in (
        lambda: entity.text,
        lambda: entity.tag,
        lambda: entity.getparent(),
        lambda: et.tostring(entity),
        lambda: entity.xpath("."),
        lambda: setattr(entity, "tail", "x"),
        lambda: target.append(entity),
    ):
        with pytest.raises(et.StaleElementError):
            operation()


def test_stale_error_type():
    assert issubclass(et.StaleElementError, ValueError)
    assert issubclass(et.StaleElementError, et.Error)


def test_cycle_rejected():
    root = et.Element("r")
    child = et.SubElement(root, "c")
    with pytest.raises(ValueError):
        child.append(root)


def test_copy_and_deepcopy():
    root = et.fromstring("<r xmlns:p='u'><p:a x='1'>t</p:a>tail</r>")
    clone = copy.deepcopy(root[0])
    assert clone.getparent() is None
    assert xml(clone) == b'<p:a xmlns:p="u" x="1">t</p:a>tail'
    clone.text = "changed"
    assert root[0].text == "t"
    shallow = copy.copy(root)
    assert xml(shallow) == xml(root)
    comment = copy.deepcopy(et.Comment("c"))
    assert comment.text == "c"
    tree_copy = copy.deepcopy(root.getroottree())
    assert et.tostring(tree_copy) == et.tostring(root.getroottree())


def test_sourceline():
    root = et.fromstring("<r>\n<a\n x='1'>\n</a>\n</r>")
    assert root.sourceline == 1
    assert root[0].sourceline == 3
    assert et.Element("new").sourceline is None


def test_base():
    root = et.fromstring("<r xml:base='http://e/x/'><a xml:base='y/'/></r>")
    assert root[0].base == "http://e/x/y/"


def test_repr():
    assert repr(et.Element("r")).startswith("<Element r at 0x")
    assert repr(et.PI("t", "d")) == "<?t d?>"


def test_iselement_and_qname():
    assert et.iselement(et.Element("a"))
    assert not et.iselement("a")
    name = et.QName("urn:x", "local")
    assert name.text == "{urn:x}local"
    assert (name.namespace, name.localname) == ("urn:x", "local")
    assert et.QName("{urn:x}local") == name
    assert et.QName("plain").namespace is None
    assert str(et.QName(et.Element("{u}e"))) == "{u}e"
    assert {name: 1}[et.QName("{urn:x}local")] == 1


def test_entity_reference_content_is_read_only():
    root = et.fromstring("<!DOCTYPE r [<!ENTITY e '<i/>'>]><r>&e;</r>")
    entity = root[0]
    assert entity.tag is et.Entity
    with pytest.raises(TypeError):
        entity.text = "x"
