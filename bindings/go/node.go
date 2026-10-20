package yaxml

/*
#include "yaxml.h"
*/
import "C"

import (
	"iter"
	"runtime"
)

// NodeKind is libxml2's xmlElementType.
type NodeKind int

// Node kinds.
const (
	ElementNode      NodeKind = 1
	AttributeNode    NodeKind = 2
	TextNode         NodeKind = 3
	CDATANode        NodeKind = 4
	EntityRefNode    NodeKind = 5
	PINode           NodeKind = 7
	CommentNode      NodeKind = 8
	DocumentNode     NodeKind = 9
	HTMLDocumentNode NodeKind = 13
	DTDNode          NodeKind = 14
	NamespaceNode    NodeKind = 18
)

// Node is a node handle: a value, checked on every use. A node that was
// removed, or whose document was closed, behaves as an empty node and its
// edits fail with StatusHandle. XPath can also return attribute and
// namespace nodes, which name an element's attribute or in-scope
// namespace by position.
type Node struct {
	doc   *Document
	h     C.yaxml_node
	item  C.int32_t
	index C.uint32_t
}

// Name is an expanded name with its lexical prefix.
type Name struct {
	Qualified, Prefix, Local, URI string
}

// Attr is an attribute; namespace declarations are attributes too.
type Attr struct {
	Name  Name
	Value string
}

func (n Node) call(f func(d *C.yaxml_doc) C.yaxml_status) bool {
	if n.doc == nil {
		return false
	}
	status := f(n.doc.h)
	runtime.KeepAlive(n.doc)
	return status == C.YAXML_OK
}

// Kind is the node's type; 0 for an invalid node.
func (n Node) Kind() NodeKind {
	if n.item != 0 {
		return NodeKind(n.item)
	}
	var kind C.int32_t
	n.call(func(d *C.yaxml_doc) C.yaxml_status { return C.yaxml_node_kind(d, n.h, &kind, nil) })
	return NodeKind(kind)
}

// Document is the node's document.
func (n Node) Document() *Document { return n.doc }

func (n Node) related(relation C.int32_t) (Node, bool) {
	var out C.yaxml_node
	ok := n.item == 0 && n.call(func(d *C.yaxml_doc) C.yaxml_status {
		return C.yaxml_node_relative(d, n.h, relation, &out, nil)
	})
	return Node{doc: n.doc, h: out}, ok
}

// Parent is the parent node (for attribute and namespace nodes, their element).
func (n Node) Parent() (Node, bool) {
	if n.item != 0 {
		return Node{doc: n.doc, h: n.h}, true
	}
	return n.related(C.YAXML_PARENT)
}

// FirstChild, LastChild, NextSibling and PreviousSibling navigate the tree.
func (n Node) FirstChild() (Node, bool)      { return n.related(C.YAXML_FIRST_CHILD) }
func (n Node) LastChild() (Node, bool)       { return n.related(C.YAXML_LAST_CHILD) }
func (n Node) NextSibling() (Node, bool)     { return n.related(C.YAXML_NEXT_SIBLING) }
func (n Node) PreviousSibling() (Node, bool) { return n.related(C.YAXML_PREVIOUS_SIBLING) }

// Children iterates over the node's children.
func (n Node) Children() iter.Seq[Node] {
	return func(yield func(Node) bool) {
		for c, ok := n.FirstChild(); ok && yield(c); c, ok = c.NextSibling() {
		}
	}
}

func goName(name *C.yaxml_name) Name {
	return Name{goString(name.qname), goString(name.prefix), goString(name.local), goString(name.uri)}
}

// attr is the attribute at an index of an element.
func (n Node) attr(index C.size_t) (Attr, bool) {
	var a C.yaxml_attribute
	if !n.call(func(d *C.yaxml_doc) C.yaxml_status { return C.yaxml_node_attribute_at(d, n.h, index, &a, nil) }) {
		return Attr{}, false
	}
	return Attr{goName(&a.name), goString(a.value)}, true
}

// Name is an element's or attribute's name, a PI's target (Local) or an
// entity reference's entity (Local).
func (n Node) Name() Name {
	if n.item == C.int32_t(AttributeNode) {
		a, _ := n.attr(C.size_t(n.index))
		return a.Name
	}
	var name C.yaxml_name
	if n.item != 0 || !n.call(func(d *C.yaxml_doc) C.yaxml_status { return C.yaxml_node_name(d, n.h, &name, nil) }) {
		return Name{}
	}
	return goName(&name)
}

// Value is the payload of a text, CDATA, comment or PI node, or an
// attribute node's value; ok is false for other nodes.
func (n Node) Value() (value string, ok bool) {
	if n.item == C.int32_t(AttributeNode) {
		a, ok := n.attr(C.size_t(n.index))
		return a.Value, ok
	}
	var s C.yaxml_str
	ok = n.item == 0 && n.call(func(d *C.yaxml_doc) C.yaxml_status { return C.yaxml_node_value(d, n.h, &s, nil) })
	return goString(s), ok
}

// Text is the node's text content (xmlNodeGetContent).
func (n Node) Text() string {
	if n.item != 0 {
		v, _ := n.Value()
		return v
	}
	var b C.yaxml_buffer
	n.call(func(d *C.yaxml_doc) C.yaxml_status { return C.yaxml_node_content(d, n.h, &b, nil) })
	return string(take(b))
}

// Path is the node's XPath location (xmlGetNodePath).
func (n Node) Path() string {
	var b C.yaxml_buffer
	n.call(func(d *C.yaxml_doc) C.yaxml_status { return C.yaxml_node_path(d, n.h, &b, nil) })
	return string(take(b))
}

// Line is the node's line in the parsed input; 0 for created nodes.
func (n Node) Line() int {
	var line C.uint64_t
	n.call(func(d *C.yaxml_doc) C.yaxml_status { return C.yaxml_node_line(d, n.h, &line, nil) })
	return int(line)
}

// Attrs lists an element's attributes in source order.
func (n Node) Attrs() []Attr {
	var count C.size_t
	if n.item != 0 || !n.call(func(d *C.yaxml_doc) C.yaxml_status { return C.yaxml_node_attribute_count(d, n.h, &count, nil) }) {
		return nil
	}
	out := make([]Attr, 0, count)
	for i := C.size_t(0); i < count; i++ {
		if a, ok := n.attr(i); ok {
			out = append(out, a)
		}
	}
	return out
}

// Attr is an attribute value by name in any namespace, or the DTD default
// (xmlGetProp).
func (n Node) Attr(name string) (string, bool) {
	var s C.yaxml_str
	ok := n.call(func(d *C.yaxml_doc) C.yaxml_status { return C.yaxml_node_attribute(d, n.h, cstr(name), &s, nil) })
	runtime.KeepAlive(name)
	return goString(s), ok
}

// AttrNS is an attribute value by namespace URI ("" for none) and local name.
func (n Node) AttrNS(uri, local string) (string, bool) {
	var s C.yaxml_str
	u := C.yaxml_str{}
	if uri != "" {
		u = cstr(uri)
	}
	ok := n.call(func(d *C.yaxml_doc) C.yaxml_status { return C.yaxml_node_attribute_ns(d, n.h, cstr(local), u, &s, nil) })
	runtime.KeepAlive(uri)
	runtime.KeepAlive(local)
	return goString(s), ok
}

// LookupNamespace is the URI bound to a prefix ("" for the default) in scope.
func (n Node) LookupNamespace(prefix string) (string, bool) {
	var s C.yaxml_str
	p := cstr(prefix)
	if prefix == "" {
		p = C.yaxml_str{}
	}
	ok := n.call(func(d *C.yaxml_doc) C.yaxml_status { return C.yaxml_node_namespace_uri(d, n.h, p, &s, nil) })
	runtime.KeepAlive(prefix)
	return goString(s), ok
}

// ---- editing ----

func (n Node) edit(f func(d *C.yaxml_doc, e *C.yaxml_error) C.yaxml_status) error {
	if n.doc == nil {
		return &Error{Status: StatusHandle, Diagnostic: Diagnostic{Message: "zero Node"}}
	}
	var e C.yaxml_error
	status := f(n.doc.h, &e)
	runtime.KeepAlive(n.doc)
	return check(status, &e)
}

func (n Node) created(f func(d *C.yaxml_doc, out *C.yaxml_node, e *C.yaxml_error) C.yaxml_status) (Node, error) {
	var out C.yaxml_node
	err := n.edit(func(d *C.yaxml_doc, e *C.yaxml_error) C.yaxml_status { return f(d, &out, e) })
	return Node{doc: n.doc, h: out}, err
}

// spec is a name for creation; Prefix "" with a URI lets yaxml choose a
// prefix (preferring the in-scope default namespace). Its strings stay
// pinned until p is unpinned.
func spec(p *runtime.Pinner, name Name) *C.yaxml_name_spec {
	s := &C.yaxml_name_spec{local: pin(p, name.Local)}
	if name.URI != "" {
		s.uri = pin(p, name.URI)
		s.automatic = C.int32_t(boolInt(name.Prefix == ""))
	}
	if name.Prefix != "" {
		s.prefix = pin(p, name.Prefix)
	}
	return s
}

func boolInt(b bool) int {
	if b {
		return 1
	}
	return 0
}

// AppendElement appends a new element child.
func (n Node) AppendElement(name Name) (Node, error) {
	var p runtime.Pinner
	defer p.Unpin()
	s := spec(&p, name)
	return n.created(func(d *C.yaxml_doc, out *C.yaxml_node, e *C.yaxml_error) C.yaxml_status {
		return C.yaxml_append_element(d, n.h, s, out, e)
	})
}

func (n Node) appendLeaf(kind NodeKind, target, value string) (Node, error) {
	defer runtime.KeepAlive(target)
	defer runtime.KeepAlive(value)
	return n.created(func(d *C.yaxml_doc, out *C.yaxml_node, e *C.yaxml_error) C.yaxml_status {
		return C.yaxml_append_leaf(d, n.h, C.int32_t(kind), cstr(target), cstr(value), out, e)
	})
}

// AppendText, AppendCDATA, AppendComment and AppendPI append a new leaf.
func (n Node) AppendText(text string) (Node, error)    { return n.appendLeaf(TextNode, "", text) }
func (n Node) AppendCDATA(text string) (Node, error)   { return n.appendLeaf(CDATANode, "", text) }
func (n Node) AppendComment(text string) (Node, error) { return n.appendLeaf(CommentNode, "", text) }
func (n Node) AppendPI(target, data string) (Node, error) {
	return n.appendLeaf(PINode, target, data)
}

// SetValue replaces a text, CDATA, comment or PI node's payload.
func (n Node) SetValue(value string) error {
	defer runtime.KeepAlive(value)
	return n.edit(func(d *C.yaxml_doc, e *C.yaxml_error) C.yaxml_status {
		return C.yaxml_set_value(d, n.h, cstr(value), e)
	})
}

// SetAttr sets an attribute, adding it if missing.
func (n Node) SetAttr(name Name, value string) error {
	var p runtime.Pinner
	defer p.Unpin()
	s := spec(&p, name)
	defer runtime.KeepAlive(value)
	return n.edit(func(d *C.yaxml_doc, e *C.yaxml_error) C.yaxml_status {
		return C.yaxml_set_attribute(d, n.h, s, cstr(value), e)
	})
}

// RemoveAttr removes an attribute by namespace URI ("" for none) and local
// name; removed is false when there was none.
func (n Node) RemoveAttr(uri, local string) (removed bool, err error) {
	var status C.yaxml_status
	u := cstr(uri)
	if uri == "" {
		u = C.yaxml_str{}
	}
	defer runtime.KeepAlive(uri)
	defer runtime.KeepAlive(local)
	err = n.edit(func(d *C.yaxml_doc, e *C.yaxml_error) C.yaxml_status {
		status = C.yaxml_remove_attribute(d, n.h, u, cstr(local), e)
		return status
	})
	return status == C.YAXML_OK, err
}

// Rename changes an element's name.
func (n Node) Rename(name Name) error {
	var p runtime.Pinner
	defer p.Unpin()
	s := spec(&p, name)
	return n.edit(func(d *C.yaxml_doc, e *C.yaxml_error) C.yaxml_status { return C.yaxml_set_name(d, n.h, s, e) })
}

func (n Node) move(op C.int32_t, node Node) (Node, error) {
	return n.created(func(d *C.yaxml_doc, out *C.yaxml_node, e *C.yaxml_error) C.yaxml_status {
		return C.yaxml_move(d, op, n.h, node.h, out, e)
	})
}

// AppendChild moves a node of this document to the end of n's children; a
// Text node merges into a Text last child, which is returned.
func (n Node) AppendChild(child Node) (Node, error) { return n.move(C.YAXML_APPEND_CHILD, child) }

// InsertBefore and InsertAfter move a node next to n.
func (n Node) InsertBefore(node Node) (Node, error) { return n.move(C.YAXML_INSERT_BEFORE, node) }
func (n Node) InsertAfter(node Node) (Node, error)  { return n.move(C.YAXML_INSERT_AFTER, node) }

// Replace puts node in n's place and detaches n.
func (n Node) Replace(node Node) error {
	_, err := n.move(C.YAXML_REPLACE, node)
	return err
}

// Unlink detaches the subtree; it stays usable and can be inserted again.
func (n Node) Unlink() error {
	return n.edit(func(d *C.yaxml_doc, e *C.yaxml_error) C.yaxml_status { return C.yaxml_unlink(d, n.h, e) })
}

// Remove deletes the subtree; its nodes become invalid.
func (n Node) Remove() error {
	return n.edit(func(d *C.yaxml_doc, e *C.yaxml_error) C.yaxml_status { return C.yaxml_remove(d, n.h, e) })
}

// Copy copies the node (deep: with its subtree) as a detached node.
func (n Node) Copy(deep bool) (Node, error) {
	return n.created(func(d *C.yaxml_doc, out *C.yaxml_node, e *C.yaxml_error) C.yaxml_status {
		return C.yaxml_copy(d, n.h, C.int32_t(boolInt(deep)), out, e)
	})
}

// Import copies a node of another document into d, detached.
func (d *Document) Import(node Node, deep bool) (Node, error) {
	if node.doc == nil {
		return Node{}, &Error{Status: StatusHandle, Diagnostic: Diagnostic{Message: "zero Node"}}
	}
	n, err := d.DocumentNode().created(func(h *C.yaxml_doc, out *C.yaxml_node, e *C.yaxml_error) C.yaxml_status {
		return C.yaxml_import(h, node.doc.h, node.h, C.int32_t(boolInt(deep)), out, e)
	})
	runtime.KeepAlive(node.doc)
	return n, err
}

// Create makes a document with one root element.
func Create(root Name) (*Document, error) {
	var h *C.yaxml_doc
	var e C.yaxml_error
	var p runtime.Pinner
	defer p.Unpin()
	status := C.yaxml_doc_create(spec(&p, root), &h, &e)
	if err := check(status, &e); err != nil {
		return nil, err
	}
	return newDocument(h), nil
}
