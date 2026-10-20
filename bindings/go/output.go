package yaxml

/*
#include "yaxml.h"

extern int32_t yaxmlGoWrite(void *user, uint8_t *bytes, size_t len);

static int32_t yaxml_go_write(void *user, const uint8_t *bytes, size_t len) {
	return yaxmlGoWrite(user, (uint8_t *)bytes, len);
}

static yaxml_write_fn yaxml_go_writer(void) { return yaxml_go_write; }
*/
import "C"

import (
	"fmt"
	"io"
	"runtime"
	"runtime/cgo"
	"unsafe"
)

// Namespace binds a prefix for XPath name tests.
type Namespace struct{ Prefix, URI string }

// Eval evaluates an XPath 1.0 expression with n as the context node. The
// result is a []Node, bool, float64 or string.
func (n Node) Eval(expression string, namespaces ...Namespace) (any, error) {
	var p runtime.Pinner
	defer p.Unpin()
	ns := make([]C.yaxml_ns, len(namespaces))
	for i, b := range namespaces {
		ns[i] = C.yaxml_ns{prefix: pin(&p, b.Prefix), uri: pin(&p, b.URI)}
	}
	var nsp *C.yaxml_ns
	if len(ns) > 0 {
		nsp = &ns[0]
	}
	var v C.yaxml_xpath_value
	err := n.edit(func(d *C.yaxml_doc, e *C.yaxml_error) C.yaxml_status {
		return C.yaxml_xpath_eval(d, n.h, cstr(expression), nsp, C.size_t(len(ns)), &v, e)
	})
	runtime.KeepAlive(expression)
	if err != nil {
		return nil, err
	}
	switch v._type {
	case C.YAXML_XPATH_BOOLEAN:
		return v.boolean != 0, nil
	case C.YAXML_XPATH_NUMBER:
		return float64(v.number), nil
	case C.YAXML_XPATH_STRING:
		return string(take(v.string)), nil
	}
	items := unsafe.Slice(v.items, v.len)
	nodes := make([]Node, len(items))
	for i, item := range items {
		nodes[i] = Node{doc: n.doc, h: item.node, item: item._type, index: item.index}
	}
	C.yaxml_free(unsafe.Pointer(v.items))
	return nodes, nil
}

// XPath selects nodes with the document node as the context.
func (d *Document) XPath(expression string, namespaces ...Namespace) ([]Node, error) {
	value, err := d.DocumentNode().Eval(expression, namespaces...)
	if err != nil {
		return nil, err
	}
	nodes, ok := value.([]Node)
	if !ok {
		return nil, &Error{Status: StatusXPath, Diagnostic: Diagnostic{
			Domain: 12, Code: 1211, Level: LevelError,
			Message: fmt.Sprintf("Invalid type: %T is not a node-set", value),
		}}
	}
	return nodes, nil
}

// SaveFlag is a libxml2 xmlSaveOption value.
type SaveFlag uint32

// Save flags.
const (
	Format  SaveFlag = 1 << 0
	NoDecl  SaveFlag = 1 << 1
	NoEmpty SaveFlag = 1 << 2
	AsXML   SaveFlag = 1 << 5
	AsHTML  SaveFlag = 1 << 6
)

// Save serializes the node (xmlNodeDump), or the whole document for the
// document node (xmlSaveDoc). encoding "" uses the document's.
func (n Node) Save(flags SaveFlag, encoding string) ([]byte, error) {
	var b C.yaxml_buffer
	enc := C.yaxml_str{}
	if encoding != "" {
		enc = cstr(encoding)
	}
	err := n.edit(func(d *C.yaxml_doc, e *C.yaxml_error) C.yaxml_status {
		return C.yaxml_save(d, n.h, C.uint32_t(flags), enc, &b, e)
	})
	runtime.KeepAlive(encoding)
	if err != nil {
		return nil, err
	}
	return take(b), nil
}

// Save serializes the whole document.
func (d *Document) Save(flags SaveFlag) ([]byte, error) {
	return d.DocumentNode().Save(flags, "")
}

type sink struct {
	w   io.Writer
	n   int64
	err error
}

// WriteTo streams the document to w in chunks (io.WriterTo).
func (d *Document) WriteTo(w io.Writer) (int64, error) {
	s := &sink{w: w}
	h := cgo.NewHandle(s)
	defer h.Delete()
	n := d.DocumentNode()
	err := n.edit(func(doc *C.yaxml_doc, e *C.yaxml_error) C.yaxml_status {
		return C.yaxml_save_to(doc, n.h, 0, C.yaxml_str{}, C.yaxml_go_writer(), unsafe.Pointer(&h), e)
	})
	if s.err != nil {
		return s.n, s.err
	}
	return s.n, err
}

// C14NMode is libxml2's xmlC14NMode.
type C14NMode int32

// Canonicalization modes.
const (
	C14N10          C14NMode = 0
	ExclusiveC14N10 C14NMode = 1
	C14N11          C14NMode = 2
)

// C14N canonicalizes the document, or only the given node-set.
func (d *Document) C14N(mode C14NMode, withComments bool, nodes []Node) ([]byte, error) {
	var items *C.yaxml_xpath_item
	if nodes != nil {
		list := make([]C.yaxml_xpath_item, len(nodes)+1)
		for i, n := range nodes {
			list[i] = C.yaxml_xpath_item{node: n.h, _type: n.item, index: n.index}
		}
		items = &list[0]
	}
	var b C.yaxml_buffer
	err := d.DocumentNode().edit(func(h *C.yaxml_doc, e *C.yaxml_error) C.yaxml_status {
		return C.yaxml_c14n(h, C.int32_t(mode), C.int32_t(boolInt(withComments)), items, C.size_t(len(nodes)), nil, 0, &b, e)
	})
	if err != nil {
		return nil, err
	}
	return take(b), nil
}

// SchemaKind selects a schema language.
type SchemaKind int32

// Schema languages.
const (
	XSD        SchemaKind = 1
	RelaxNG    SchemaKind = 2
	Schematron SchemaKind = 3
)

// Schema is a compiled schema, reusable across documents and goroutines.
type Schema struct {
	h       *C.yaxml_schema
	cleanup runtime.Cleanup
}

// CompileSchema compiles a schema document; nothing it references is loaded.
func CompileSchema(kind SchemaKind, data []byte, base string) (*Schema, error) {
	var h *C.yaxml_schema
	var e C.yaxml_error
	status := C.yaxml_schema_new(C.int32_t(kind), bytesPtr(data), C.size_t(len(data)), cstr(base), &h, &e)
	runtime.KeepAlive(data)
	runtime.KeepAlive(base)
	if err := check(status, &e); err != nil {
		return nil, err
	}
	s := &Schema{h: h}
	s.cleanup = runtime.AddCleanup(s, func(h *C.yaxml_schema) { C.yaxml_schema_free(h) }, h)
	return s, nil
}

// Close frees the schema.
func (s *Schema) Close() error {
	if s.h != nil {
		s.cleanup.Stop()
		C.yaxml_schema_free(s.h)
		s.h = nil
	}
	return nil
}

// Validate checks a document; see Valid.
func (s *Schema) Validate(d *Document) ([]Diagnostic, error) {
	var list C.yaxml_diagnostics
	var e C.yaxml_error
	status := C.yaxml_validate(d.h, s.h, &list, &e)
	runtime.KeepAlive(d)
	runtime.KeepAlive(s)
	if err := check(status, &e); err != nil {
		return nil, err
	}
	return diagnostics(list), nil
}
