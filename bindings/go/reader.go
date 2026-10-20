package yaxml

/*
#include <stdlib.h>
#include "yaxml.h"
*/
import "C"

import (
	"io"
	"runtime"
	"unsafe"
)

// EventKind is a streaming event's kind.
type EventKind int32

// Event kinds.
const (
	DeclarationEvent EventKind = 1
	DoctypeEvent     EventKind = 2
	StartEvent       EventKind = 3
	EmptyEvent       EventKind = 4
	EndEvent         EventKind = 5
	TextEvent        EventKind = 6
	CDATAEvent       EventKind = 7
	CommentEvent     EventKind = 8
	PIEvent          EventKind = 9
	EntityStartEvent EventKind = 10
	EntityEndEvent   EventKind = 11
)

// Event is one streaming event, copied into Go memory. Value holds text,
// CDATA, comment and PI data, an entity's name, a declaration's version or
// a DOCTYPE's name; Target a PI's target, a declaration's encoding or a
// DOCTYPE's public identifier. Start and End are the input byte range, -1
// when the event has none (entity replacement text).
type Event struct {
	Kind       EventKind
	Name       Name
	Attrs      []Attr
	Value      string
	Target     string
	Start, End int
}

// Reader is a pull parser over a copy of its input. It fetches events in
// batches, one cgo call each.
type Reader struct {
	h       *C.yaxml_reader
	cleanup runtime.Cleanup
	out     *batch // reused, separate allocations: C sees no Go pointers
	err     *C.yaxml_error
	pending []C.yaxml_event // the rest of the batch, owned by the C reader
	names   map[string]string
}

// batch is yaxml_reader_next_batch's out-parameters.
type batch struct {
	events *C.yaxml_event
	count  C.size_t
}

// batchSize is the most events one call fetches.
const batchSize = 64

// maxNames bounds the name cache against documents with endless new names.
const maxNames = 4096

// name copies a name-like string, reusing an earlier copy: documents
// repeat their element and attribute names and namespace URIs.
func (r *Reader) name(s C.yaxml_str) string {
	if s.ptr == nil || s.len == 0 {
		return ""
	}
	b := unsafe.Slice((*byte)(unsafe.Pointer(s.ptr)), s.len)
	if v, ok := r.names[string(b)]; ok {
		return v
	}
	v := string(b)
	if len(r.names) < maxNames {
		r.names[v] = v
	}
	return v
}

func (r *Reader) goName(n *C.yaxml_name) Name {
	q := r.name(n.qname)
	p := q[:n.prefix.len]
	return Name{Qualified: q, Prefix: p, Local: q[len(q)-int(n.local.len):], URI: r.name(n.uri)}
}

// NewReader starts reading data. DTDValid and XInclude, which need a tree,
// are rejected; Recover recovers as the document parser does.
func NewReader(data []byte, opts ...Option) (*Reader, error) {
	o, free := options(opts)
	defer free()
	var h *C.yaxml_reader
	var e C.yaxml_error
	status := C.yaxml_reader_new(bytesPtr(data), C.size_t(len(data)), o, &h, &e)
	runtime.KeepAlive(data)
	if err := check(status, &e); err != nil {
		return nil, err
	}
	r := &Reader{h: h, out: new(batch), err: new(C.yaxml_error), names: make(map[string]string)}
	r.cleanup = runtime.AddCleanup(r, func(h *C.yaxml_reader) { C.yaxml_reader_free(h) }, h)
	return r, nil
}

func offset(v C.uint64_t) int {
	if v == C.UINT64_MAX {
		return -1
	}
	return int(v)
}

// Next returns the next event, or io.EOF after the last one.
func (r *Reader) Next() (Event, error) {
	if len(r.pending) == 0 {
		status := C.yaxml_reader_next_batch(r.h, batchSize, &r.out.events, &r.out.count, r.err)
		if err := check(status, r.err); err != nil {
			return Event{}, err
		}
		if status == C.YAXML_NONE {
			return Event{}, io.EOF
		}
		r.pending = unsafe.Slice(r.out.events, r.out.count)
	}
	ev := &r.pending[0]
	r.pending = r.pending[1:]
	out := Event{
		Kind: EventKind(ev.kind), Name: r.goName(&ev.name),
		Value: goString(ev.value), Target: goString(ev.target),
		Start: offset(ev.start), End: offset(ev.end),
	}
	if ev.kind == C.YAXML_EVENT_PI {
		out.Target = r.name(ev.target)
	}
	if ev.attribute_count > 0 {
		attrs := unsafe.Slice(ev.attributes, ev.attribute_count)
		out.Attrs = make([]Attr, len(attrs))
		for i := range attrs {
			out.Attrs[i] = Attr{r.goName(&attrs[i].name), goString(attrs[i].value)}
		}
	}
	return out, nil
}

// Close frees the reader.
func (r *Reader) Close() error {
	if r.h != nil {
		r.cleanup.Stop()
		C.yaxml_reader_free(r.h)
		r.h, r.pending = nil, nil
	}
	return nil
}
