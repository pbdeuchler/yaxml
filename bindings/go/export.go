package yaxml

/*
#include <stdint.h>
#include <stddef.h>
*/
import "C"

import (
	"runtime/cgo"
	"unsafe"
)

// yaxmlGoWrite is the yaxml_write_fn behind Document.WriteTo: user points
// to the cgo.Handle of a *sink.
//
//export yaxmlGoWrite
func yaxmlGoWrite(user unsafe.Pointer, bytes *C.uint8_t, length C.size_t) C.int32_t {
	s := (*(*cgo.Handle)(user)).Value().(*sink)
	n, err := s.w.Write(unsafe.Slice((*byte)(bytes), int(length)))
	s.n += int64(n)
	if err != nil {
		s.err = err
		return 1
	}
	return 0
}
