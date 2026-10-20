package yaxml

import (
	"bytes"
	"encoding/xml"
	"fmt"
	"io"
	"strings"
	"testing"
)

// document is a namespaced catalog of n items (about 100 bytes each).
func document(n int) []byte {
	var b strings.Builder
	b.WriteString(`<?xml version="1.0"?><catalog xmlns="urn:shop" xmlns:p="urn:price">`)
	for i := range n {
		fmt.Fprintf(&b, `<item id="i%d" p:currency="EUR"><name>Item &amp; %d</name><p:price>%d.50</p:price><!--c--></item>`, i, i, i)
	}
	b.WriteString(`</catalog>`)
	return []byte(b.String())
}

// The FFI overhead story: one cgo call per document for the DOM, one per
// event (plus copying its strings into Go) for the Reader, against
// encoding/xml's pure-Go tokenizer, which allocates per token.
func BenchmarkParse(b *testing.B) {
	for _, size := range []struct {
		name string
		data []byte
	}{{"tiny", []byte(`<r a="1"/>`)}, {"100KB", document(1000)}} {
		b.Run(size.name+"/yaxml-dom", func(b *testing.B) {
			b.SetBytes(int64(len(size.data)))
			for b.Loop() {
				d, err := Parse(size.data)
				if err != nil {
					b.Fatal(err)
				}
				d.Close()
			}
		})
		b.Run(size.name+"/yaxml-reader", func(b *testing.B) {
			b.SetBytes(int64(len(size.data)))
			for b.Loop() {
				r, err := NewReader(size.data)
				if err != nil {
					b.Fatal(err)
				}
				for {
					if _, err := r.Next(); err == io.EOF {
						break
					} else if err != nil {
						b.Fatal(err)
					}
				}
				r.Close()
			}
		})
		b.Run(size.name+"/encoding-xml", func(b *testing.B) {
			b.SetBytes(int64(len(size.data)))
			for b.Loop() {
				d := xml.NewDecoder(bytes.NewReader(size.data))
				for {
					if _, err := d.Token(); err == io.EOF {
						break
					} else if err != nil {
						b.Fatal(err)
					}
				}
			}
		})
	}
}
