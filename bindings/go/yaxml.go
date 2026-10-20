// Package yaxml parses, queries, edits, validates and writes XML through
// yaxml, a Rust rewrite of libxml2, over its C ABI (bindings/capi).
//
// The capi static library must be built first:
//
//	cargo build --release --manifest-path ../Cargo.toml -p yaxml-capi
//
// (or go generate). Documents hold memory outside the Go heap: Close them.
// A cleanup frees a forgotten document when it becomes unreachable, but only
// as a safety net. A Document may be read from several goroutines at once;
// an edit while another call is in progress fails with StatusBusy.
package yaxml

//go:generate cargo build --release --manifest-path ../Cargo.toml -p yaxml-capi

/*
#cgo CFLAGS: -I${SRCDIR}/../capi/include
#cgo LDFLAGS: ${SRCDIR}/../target/release/libyaxml_capi.a -lm
#cgo linux LDFLAGS: -lgcc_s -lutil -lrt -lpthread -ldl
#include <stdlib.h>
#include "yaxml.h"
*/
import "C"

import (
	"bytes"
	"fmt"
	"runtime"
	"unsafe"
)

// Status is the C ABI's status code.
type Status int32

// Status codes (yaxml.h).
const (
	StatusOK       Status = 0
	StatusNone     Status = 1
	StatusNull     Status = -1
	StatusArgument Status = -2
	StatusHandle   Status = -3
	StatusBusy     Status = -4
	StatusPoisoned Status = -5
	StatusPanic    Status = -6
	StatusXML      Status = -7
	StatusEdit     Status = -8
	StatusXPath    Status = -9
	StatusSchema   Status = -10
	StatusC14N     Status = -11
	StatusCallback Status = -12
)

var statusNames = map[Status]string{
	StatusNull: "null argument", StatusArgument: "invalid argument",
	StatusHandle: "invalid handle", StatusBusy: "busy", StatusPoisoned: "poisoned",
	StatusPanic: "internal panic", StatusXML: "XML error", StatusEdit: "edit error",
	StatusXPath: "XPath error", StatusSchema: "schema error", StatusC14N: "C14N error",
	StatusCallback: "callback error",
}

func (s Status) String() string {
	if name, ok := statusNames[s]; ok {
		return name
	}
	return fmt.Sprintf("status %d", int32(s))
}

// Level is libxml2's xmlErrorLevel.
type Level int32

// Levels.
const (
	LevelWarning Level = 1
	LevelError   Level = 2
	LevelFatal   Level = 3
)

// Diagnostic is libxml2's xmlError: Domain is xmlErrorDomain (1 parser,
// 12 XPath, 17 XSD, 19 RELAX NG, 23 DTD validity, 28 Schematron, ...; 1000
// for yaxml edit errors), Code its error number. Line and Column are 0
// when unknown.
type Diagnostic struct {
	Domain  int
	Code    int
	Level   Level
	Line    int
	Column  int
	Message string
}

func (d Diagnostic) Error() string {
	if d.Line > 0 {
		return fmt.Sprintf("line %d: %s", d.Line, d.Message)
	}
	return d.Message
}

// Error is a failed call: its status and the libxml2 diagnostic it wraps
// (errors.As(err, &yaxml.Diagnostic{}) retrieves it). Offset is the input
// byte offset of a parse error.
type Error struct {
	Status     Status
	Offset     int
	Diagnostic Diagnostic
}

func (e *Error) Error() string {
	return fmt.Sprintf("yaxml: %s: %s", e.Status, e.Diagnostic.Error())
}

// Unwrap returns the diagnostic.
func (e *Error) Unwrap() error { return e.Diagnostic }

// Valid reports whether diagnostics hold no error.
func Valid(diagnostics []Diagnostic) bool {
	for _, d := range diagnostics {
		if d.Level >= LevelError {
			return false
		}
	}
	return true
}

func goString(s C.yaxml_str) string {
	if s.ptr == nil {
		return ""
	}
	return string(unsafe.Slice((*byte)(unsafe.Pointer(s.ptr)), s.len))
}

func diagnostic(d *C.yaxml_diagnostic) Diagnostic {
	return Diagnostic{
		Domain: int(d.domain), Code: int(d.code), Level: Level(d.level),
		Line: int(d.line), Column: int(d.column), Message: goString(d.message),
	}
}

// check turns a status into an error, freeing the message.
func check(status C.yaxml_status, err *C.yaxml_error) error {
	if status >= 0 {
		return nil
	}
	e := &Error{Status: Status(status), Offset: int(err.offset), Diagnostic: diagnostic(&err.diagnostic)}
	C.yaxml_free(unsafe.Pointer(err.diagnostic.message.ptr))
	return e
}

// empty is what "" points to: present but empty, unlike NULL (absent).
var empty byte

// cstr lends a Go string for one call.
func cstr(s string) C.yaxml_str {
	if len(s) == 0 {
		return C.yaxml_str{ptr: (*C.char)(unsafe.Pointer(&empty)), len: 0}
	}
	return C.yaxml_str{ptr: (*C.char)(unsafe.Pointer(unsafe.StringData(s))), len: C.size_t(len(s))}
}

// pin is cstr for a string stored in Go memory that is passed to C: the
// bytes stay pinned until p is unpinned.
func pin(p *runtime.Pinner, s string) C.yaxml_str {
	c := cstr(s)
	p.Pin(unsafe.Pointer(c.ptr))
	return c
}

func bytesPtr(b []byte) *C.uint8_t {
	if len(b) == 0 {
		return nil
	}
	return (*C.uint8_t)(unsafe.Pointer(&b[0]))
}

// take copies an owned buffer and frees it.
func take(b C.yaxml_buffer) []byte {
	if b.ptr == nil {
		return []byte{}
	}
	out := bytes.Clone(unsafe.Slice((*byte)(b.ptr), b.len))
	C.yaxml_free(unsafe.Pointer(b.ptr))
	return out
}

// ParseFlag is a libxml2 xmlParserOption value.
type ParseFlag uint32

// Parser flags.
const (
	Recover  ParseFlag = 1 << 0
	NoEnt    ParseFlag = 1 << 1
	DTDLoad  ParseFlag = 1 << 2
	DTDAttr  ParseFlag = 1 << 3
	DTDValid ParseFlag = 1 << 4
	NoBlanks ParseFlag = 1 << 8
	NSClean  ParseFlag = 1 << 13
	NoCDATA  ParseFlag = 1 << 14
	Huge     ParseFlag = 1 << 19
	NoXXE    ParseFlag = 1 << 23
	SkipIDs  ParseFlag = 1 << 27
)

// Limits are yaxml's parser ceilings; 0 keeps the default.
type Limits struct {
	MaxInputBytes, MaxTokenBytes, MaxDepth, MaxAttributes, MaxEvents     uint64
	MaxOwnedBytes, MaxDTDBytes, MaxEntityExpansionBytes                  uint64
	MaxEntityAmplification, MaxEntityReferences, MaxEntityDepth          uint64
	MaxExternalLoads, MaxExternalBytes, MaxDecodedBytes, MaxDecodedToken uint64
}

type config struct {
	flags  ParseFlag
	base   *string
	limits Limits
}

// Option configures parsing.
type Option func(*config)

// WithFlags adds parser flags.
func WithFlags(flags ParseFlag) Option { return func(c *config) { c.flags |= flags } }

// WithBase sets the document's URI.
func WithBase(uri string) Option { return func(c *config) { c.base = &uri } }

// WithLimits sets parser ceilings.
func WithLimits(l Limits) Option { return func(c *config) { c.limits = l } }

// options builds the C struct in C memory (it may hold the base string);
// the caller frees it with free.
func options(opts []Option) (*C.yaxml_parse_options, func()) {
	var c config
	for _, o := range opts {
		o(&c)
	}
	o := (*C.yaxml_parse_options)(C.calloc(1, C.sizeof_yaxml_parse_options))
	o.struct_size = C.uint32_t(C.sizeof_yaxml_parse_options)
	o.flags = C.uint32_t(c.flags)
	l := c.limits
	for field, v := range map[*C.uint64_t]uint64{
		&o.max_input_bytes: l.MaxInputBytes, &o.max_token_bytes: l.MaxTokenBytes,
		&o.max_depth: l.MaxDepth, &o.max_attributes: l.MaxAttributes, &o.max_events: l.MaxEvents,
		&o.max_owned_bytes: l.MaxOwnedBytes, &o.max_dtd_bytes: l.MaxDTDBytes,
		&o.max_entity_expansion_bytes: l.MaxEntityExpansionBytes,
		&o.max_entity_amplification:   l.MaxEntityAmplification,
		&o.max_entity_references:      l.MaxEntityReferences, &o.max_entity_depth: l.MaxEntityDepth,
		&o.max_external_loads: l.MaxExternalLoads, &o.max_external_bytes: l.MaxExternalBytes,
		&o.max_decoded_bytes: l.MaxDecodedBytes, &o.max_decoded_token_bytes: l.MaxDecodedToken,
	} {
		*field = C.uint64_t(v)
	}
	var base *C.char
	if c.base != nil {
		base = C.CString(*c.base)
		o.base = C.yaxml_str{ptr: base, len: C.size_t(len(*c.base))}
	}
	return o, func() {
		C.free(unsafe.Pointer(base))
		C.free(unsafe.Pointer(o))
	}
}

// Document is a parsed or created XML document.
type Document struct {
	h       *C.yaxml_doc
	cleanup runtime.Cleanup
}

func newDocument(h *C.yaxml_doc) *Document {
	d := &Document{h: h}
	d.cleanup = runtime.AddCleanup(d, func(h *C.yaxml_doc) { C.yaxml_doc_free(h) }, h)
	return d
}

// Parse parses a complete document.
func Parse(data []byte, opts ...Option) (*Document, error) {
	o, free := options(opts)
	defer free()
	var h *C.yaxml_doc
	var e C.yaxml_error
	status := C.yaxml_parse(bytesPtr(data), C.size_t(len(data)), o, &h, &e)
	runtime.KeepAlive(data)
	if err := check(status, &e); err != nil {
		return nil, err
	}
	return newDocument(h), nil
}

// Close frees the document; later calls on it or its nodes fail with
// StatusHandle. Closing twice is harmless.
func (d *Document) Close() error {
	if d.h == nil {
		return nil
	}
	d.cleanup.Stop()
	C.yaxml_doc_free(d.h)
	d.h = nil
	return nil
}

// Clone copies the document (xmlCopyDoc); its nodes are new handles.
func (d *Document) Clone() (*Document, error) {
	var h *C.yaxml_doc
	var e C.yaxml_error
	status := C.yaxml_doc_clone(d.h, &h, &e)
	runtime.KeepAlive(d)
	if err := check(status, &e); err != nil {
		return nil, err
	}
	return newDocument(h), nil
}

// DocumentNode is the document node, parent of the root element.
func (d *Document) DocumentNode() Node {
	var n C.yaxml_node
	C.yaxml_doc_root(d.h, &n, nil)
	runtime.KeepAlive(d)
	return Node{doc: d, h: n}
}

// Root is the document element; ok is false for a document without one.
func (d *Document) Root() (root Node, ok bool) {
	var n C.yaxml_node
	status := C.yaxml_doc_element(d.h, &n, nil)
	runtime.KeepAlive(d)
	return Node{doc: d, h: n}, status == C.YAXML_OK
}

// ElementByID finds an element by ID attribute value.
func (d *Document) ElementByID(id string) (Node, bool) {
	var n C.yaxml_node
	status := C.yaxml_doc_element_by_id(d.h, cstr(id), &n, nil)
	runtime.KeepAlive(d)
	runtime.KeepAlive(id)
	return Node{doc: d, h: n}, status == C.YAXML_OK
}

func diagnostics(list C.yaxml_diagnostics) []Diagnostic {
	items := unsafe.Slice(list.items, list.len)
	out := make([]Diagnostic, len(items))
	for i := range items {
		out[i] = diagnostic(&items[i])
	}
	C.yaxml_free(unsafe.Pointer(list.items))
	return out
}

// Diagnostics lists the namespace errors a parse kept (Recover or not), the
// errors a Recover parse repaired and the validity errors of a DTDValid parse.
func (d *Document) Diagnostics() ([]Diagnostic, error) {
	var list C.yaxml_diagnostics
	var e C.yaxml_error
	status := C.yaxml_doc_diagnostics(d.h, &list, &e)
	runtime.KeepAlive(d)
	if err := check(status, &e); err != nil {
		return nil, err
	}
	return diagnostics(list), nil
}

// ValidateDTD checks the document against its DTD (xmlValidateDocument).
func (d *Document) ValidateDTD() ([]Diagnostic, error) {
	var list C.yaxml_diagnostics
	var e C.yaxml_error
	status := C.yaxml_validate_dtd(d.h, &list, &e)
	runtime.KeepAlive(d)
	if err := check(status, &e); err != nil {
		return nil, err
	}
	return diagnostics(list), nil
}

// LiveObjects reports the library's live handles and unfreed allocations,
// for leak tests.
func LiveObjects() (handles, allocations int) {
	var h, a C.size_t
	C.yaxml_live_objects(&h, &a)
	return int(h), int(a)
}

// Version is the yaxml version.
func Version() string { return C.GoString(C.yaxml_version()) }
