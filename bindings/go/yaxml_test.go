package yaxml

import (
	"bytes"
	"errors"
	"io"
	"os"
	"runtime"
	"slices"
	"strings"
	"testing"
	"time"
)

func TestMain(m *testing.M) {
	code := m.Run()
	// Every test closes what it opens (except the cleanup test, which waits
	// for its document to be collected).
	if handles, allocations := LiveObjects(); code == 0 && (handles != 0 || allocations != 0) {
		println("leaked", handles, "handles and", allocations, "allocations")
		code = 1
	}
	os.Exit(code)
}

func mustParse(t *testing.T, xml string, opts ...Option) *Document {
	t.Helper()
	d, err := Parse([]byte(xml), opts...)
	if err != nil {
		t.Fatalf("Parse(%q): %v", xml, err)
	}
	t.Cleanup(func() { d.Close() })
	return d
}

func root(t *testing.T, d *Document) Node {
	t.Helper()
	r, ok := d.Root()
	if !ok {
		t.Fatal("no root element")
	}
	return r
}

func TestParseErrorsWrapLibxml2Diagnostics(t *testing.T) {
	cases := []struct {
		name   string
		xml    string
		opts   []Option
		status Status
		code   int
		line   int
	}{
		{"mismatch", "<a>\n  <b></c>\n</a>", nil, StatusXML, 76, 2},
		{"empty", "", nil, StatusXML, 4, 1},
		{"depth limit", "<a><b/></a>", []Option{WithLimits(Limits{MaxDepth: 1})}, StatusXML, 114, 0},
		{"unsupported flag", "<a/>", []Option{WithFlags(1 << 17)}, StatusXML, 115, 0},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			_, err := Parse([]byte(c.xml), c.opts...)
			var e *Error
			if !errors.As(err, &e) || e.Status != c.status {
				t.Fatalf("err = %v", err)
			}
			var d Diagnostic
			if !errors.As(err, &d) || d.Code != c.code || (c.line != 0 && d.Line != c.line) {
				t.Fatalf("diagnostic = %+v", d)
			}
		})
	}
	_, err := Parse([]byte("<a>\n  <b></c>\n</a>"))
	if want := "yaxml: XML error: line 2: Opening and ending tag mismatch: b line 2 and c"; err.Error() != want {
		t.Fatalf("%q", err.Error())
	}
}

func TestNavigationAndAttributes(t *testing.T) {
	d := mustParse(t, "<p:r xmlns:p='urn:p' a='1' p:b='2'>t<?pi x?><!--c--><k/></p:r>", WithBase("file:///d.xml"))
	r := root(t, d)
	if got := r.Name(); got != (Name{"p:r", "p", "r", "urn:p"}) {
		t.Fatalf("name %+v", got)
	}
	var kinds []NodeKind
	for c := range r.Children() {
		kinds = append(kinds, c.Kind())
	}
	if !slices.Equal(kinds, []NodeKind{TextNode, PINode, CommentNode, ElementNode}) {
		t.Fatalf("kinds %v", kinds)
	}
	first, _ := r.FirstChild()
	if v, ok := first.Value(); !ok || v != "t" {
		t.Fatalf("value %q %v", v, ok)
	}
	if _, ok := r.Value(); ok {
		t.Fatal("element has a value")
	}
	if p, ok := first.Parent(); !ok || p != r {
		t.Fatal("parent")
	}
	if _, ok := d.DocumentNode().Parent(); ok {
		t.Fatal("document node has a parent")
	}
	if _, ok := first.PreviousSibling(); ok {
		t.Fatal("previous of first")
	}
	last, _ := r.LastChild()
	pi, _ := first.NextSibling()
	if pi.Name().Local != "pi" || last.Path() != "/p:r/k" || r.Text() != "t" || last.Line() != 1 {
		t.Fatalf("pi %v path %q text %q line %d", pi.Name(), last.Path(), r.Text(), last.Line())
	}
	for _, c := range []struct {
		get  func() (string, bool)
		want string
		ok   bool
	}{
		{func() (string, bool) { return r.Attr("a") }, "1", true},
		{func() (string, bool) { return r.Attr("b") }, "2", true},
		{func() (string, bool) { return r.Attr("z") }, "", false},
		{func() (string, bool) { return r.AttrNS("urn:p", "b") }, "2", true},
		{func() (string, bool) { return r.AttrNS("", "b") }, "", false},
		{func() (string, bool) { return last.LookupNamespace("p") }, "urn:p", true},
		{func() (string, bool) { return last.LookupNamespace("") }, "", false},
	} {
		if v, ok := c.get(); v != c.want || ok != c.ok {
			t.Errorf("got %q %v, want %q %v", v, ok, c.want, c.ok)
		}
	}
	attrs := r.Attrs()
	if len(attrs) != 3 || attrs[2] != (Attr{Name{"p:b", "p", "b", "urn:p"}, "2"}) {
		t.Fatalf("attrs %+v", attrs)
	}
	if first.Attrs() != nil && len(first.Attrs()) != 0 {
		t.Fatal("text attributes")
	}
	var zero Node
	if zero.Kind() != 0 || zero.Text() != "" || zero.Attrs() != nil {
		t.Fatal("zero node")
	}
}

func TestXPath(t *testing.T) {
	d := mustParse(t, "<r xmlns:p='urn:p'><p:i k='1'/><p:i k='2'/></r>")
	nodes, err := d.XPath("//q:i/@k", Namespace{"q", "urn:p"})
	if err != nil || len(nodes) != 2 || nodes[1].Kind() != AttributeNode {
		t.Fatalf("%v %v", nodes, err)
	}
	if v, _ := nodes[1].Value(); v != "2" || nodes[1].Name().Local != "k" || nodes[1].Text() != "2" {
		t.Fatalf("attribute node %q %v", v, nodes[1].Name())
	}
	if p, ok := nodes[0].Parent(); !ok || p.Name().Local != "i" {
		t.Fatal("attribute parent")
	}
	r := root(t, d)
	for expr, want := range map[string]any{"count(*)": 2.0, "count(*) = 2": true, "string(*/@k)": "1"} {
		if got, err := r.Eval(expr); err != nil || got != want {
			t.Errorf("%s = %v %v", expr, got, err)
		}
	}
	for expr, code := range map[string]int{"'open": 1202, "//z:i": 1219, "count(*)": 1211} {
		_, err := d.XPath(expr)
		var e *Error
		if !errors.As(err, &e) || e.Status != StatusXPath || e.Diagnostic.Code != code {
			t.Errorf("%s: %v", expr, err)
		}
	}
}

type failingWriter struct{}

func (failingWriter) Write([]byte) (int, error) { return 0, io.ErrShortWrite }

func TestSaveWriteToAndC14N(t *testing.T) {
	d := mustParse(t, "<r b='2' a='1'><x>café</x></r>")
	out, err := d.Save(0)
	if err != nil || string(out) != "<?xml version=\"1.0\"?>\n<r b=\"2\" a=\"1\"><x>caf&#xE9;</x></r>\n" {
		t.Fatalf("%q %v", out, err)
	}
	x, _ := root(t, d).FirstChild()
	if out, _ := x.Save(0, "ISO-8859-1"); !bytes.Equal(out, []byte("<x>caf\xe9</x>")) {
		t.Fatalf("%q", out)
	}
	if _, err := x.Save(0, "nope"); err == nil {
		t.Fatal("unknown encoding")
	}
	var buf bytes.Buffer
	if n, err := d.WriteTo(&buf); err != nil || n != int64(buf.Len()) || buf.String() != string(out) {
		t.Fatalf("%d %v %q", n, err, buf.String())
	}
	if _, err := d.WriteTo(failingWriter{}); !errors.Is(err, io.ErrShortWrite) {
		t.Fatalf("%v", err)
	}
	c, err := d.C14N(C14N10, false, nil)
	if err != nil || string(c) != "<r a=\"1\" b=\"2\"><x>café</x></r>" {
		t.Fatalf("%q %v", c, err)
	}
	set, _ := d.XPath("//x")
	if c, err := d.C14N(ExclusiveC14N10, false, set); err != nil || string(c) != "<x></x>" {
		t.Fatalf("%q %v", c, err)
	}
	if _, err := d.C14N(9, false, nil); err == nil {
		t.Fatal("bad mode")
	}
}

func TestValidation(t *testing.T) {
	d := mustParse(t, "<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]>\n<r><b/></r>")
	diags, err := d.ValidateDTD()
	if err != nil || Valid(diags) || diags[0].Domain != 23 || diags[0].Line != 2 {
		t.Fatalf("%+v %v", diags, err)
	}
	if diags, _ := mustParse(t, "<r><b/></r>", WithFlags(Recover)).Diagnostics(); len(diags) != 0 {
		t.Fatalf("%v", diags)
	}
	cases := []struct {
		kind      SchemaKind
		schema    string
		good, bad string
	}{
		{XSD, "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema'><xs:element name='r' type='xs:int'/></xs:schema>", "<r>1</r>", "<r>x</r>"},
		{RelaxNG, "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><text/></element>", "<r>t</r>", "<r><x/></r>"},
		{Schematron, "<schema xmlns='http://purl.oclc.org/dsdl/schematron'><pattern id='p'><rule context='i'><assert test='@ok'>ok</assert></rule></pattern></schema>", "<r><i ok='1'/></r>", "<r><i/></r>"},
	}
	for _, c := range cases {
		s, err := CompileSchema(c.kind, []byte(c.schema), "s.xml")
		if err != nil {
			t.Fatal(err)
		}
		if diags, err := s.Validate(mustParse(t, c.good)); err != nil || !Valid(diags) {
			t.Errorf("%d good: %v %v", c.kind, diags, err)
		}
		if diags, err := s.Validate(mustParse(t, c.bad)); err != nil || Valid(diags) {
			t.Errorf("%d bad: %v %v", c.kind, diags, err)
		}
		s.Close()
		s.Close()
		if _, err := s.Validate(d); err == nil {
			t.Error("closed schema")
		}
		var e *Error
		if _, err := CompileSchema(c.kind, []byte("<x/>"), ""); !errors.As(err, &e) || e.Status != StatusSchema {
			t.Errorf("%d bad schema: %v", c.kind, err)
		}
	}
}

func TestEditing(t *testing.T) {
	d, err := Create(Name{Local: "r", URI: "urn:r"})
	if err != nil {
		t.Fatal(err)
	}
	defer d.Close()
	r := root(t, d)
	item, err := r.AppendElement(Name{Local: "item", URI: "urn:i", Prefix: "i"})
	if err != nil {
		t.Fatal(err)
	}
	steps := []error{
		item.SetAttr(Name{Local: "id"}, "1"),
		func() error { _, err := item.AppendText("a & b"); return err }(),
		func() error { _, err := r.AppendComment("c"); return err }(),
		func() error { _, err := r.AppendCDATA("d"); return err }(),
		func() error { _, err := r.AppendPI("pi", "x"); return err }(),
		item.Rename(Name{Local: "thing", URI: "urn:i", Prefix: "i"}),
	}
	for i, err := range steps {
		if err != nil {
			t.Fatalf("step %d: %v", i, err)
		}
	}
	out, _ := d.Save(NoDecl)
	want := "<r xmlns=\"urn:r\"><i:thing xmlns:i=\"urn:i\" id=\"1\">a &amp; b</i:thing><!--c--><![CDATA[d]]><?pi x?></r>\n"
	if string(out) != want {
		t.Fatalf("%q", out)
	}
	if removed, err := item.RemoveAttr("", "id"); !removed || err != nil {
		t.Fatal(removed, err)
	}
	if removed, err := item.RemoveAttr("", "id"); removed || err != nil {
		t.Fatal(removed, err)
	}
	copied, err := item.Copy(true)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := item.InsertAfter(copied); err != nil {
		t.Fatal(err)
	}
	if err := copied.Unlink(); err != nil {
		t.Fatal(err)
	}
	if _, err := r.AppendChild(copied); err != nil {
		t.Fatal(err)
	}
	if err := copied.Remove(); err != nil {
		t.Fatal(err)
	}
	var e *Error
	if err := copied.Remove(); !errors.As(err, &e) || e.Status != StatusHandle {
		t.Fatalf("stale: %v", err)
	}
	if err := r.Remove(); !errors.As(err, &e) || e.Status != StatusEdit || e.Diagnostic.Code != 3 {
		t.Fatalf("protected: %v", err)
	}
	if _, err := r.AppendComment("a--b"); !errors.As(err, &e) || e.Diagnostic.Code != 7 {
		t.Fatalf("invalid comment: %v", err)
	}
	if err := (Node{}).Remove(); err == nil {
		t.Fatal("zero node")
	}

	other := mustParse(t, "<s><t/></s>")
	tnode, _ := root(t, other).FirstChild()
	imported, err := d.Import(tnode, true)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := imported.InsertBefore(item); err == nil {
		// imported is detached: no parent to insert next to.
		t.Fatal("insert next to detached node")
	}
	text, _ := item.FirstChild()
	if err := text.SetValue("new"); err != nil || item.Text() != "new" {
		t.Fatal(err)
	}
	if err := item.Replace(imported); err != nil {
		t.Fatal(err)
	}
	clone, err := d.Clone()
	if err != nil {
		t.Fatal(err)
	}
	defer clone.Close()
	a, _ := d.Save(0)
	b, _ := clone.Save(0)
	if !bytes.Equal(a, b) {
		t.Fatalf("%q != %q", a, b)
	}
	if _, err := Create(Name{Local: "1x"}); err == nil {
		t.Fatal("invalid name")
	}
}

func TestReader(t *testing.T) {
	r, err := NewReader([]byte("<?xml version='1.0'?><r a='1'>t<!--c--><e/></r>"))
	if err != nil {
		t.Fatal(err)
	}
	defer r.Close()
	var kinds []EventKind
	for {
		ev, err := r.Next()
		if err == io.EOF {
			break
		}
		if err != nil {
			t.Fatal(err)
		}
		kinds = append(kinds, ev.Kind)
		if ev.Kind == StartEvent && (len(ev.Attrs) != 1 || ev.Attrs[0].Value != "1" || ev.Start != 21) {
			t.Fatalf("%+v", ev)
		}
	}
	if !slices.Equal(kinds, []EventKind{DeclarationEvent, StartEvent, TextEvent, CommentEvent, EmptyEvent, EndEvent}) {
		t.Fatalf("%v", kinds)
	}
	if _, err := r.Next(); err != io.EOF {
		t.Fatal(err)
	}
	bad, _ := NewReader([]byte("<r></x>"))
	defer bad.Close()
	bad.Next()
	if _, err := bad.Next(); err == nil || !strings.Contains(err.Error(), "mismatch") {
		t.Fatalf("%v", err)
	}
	if _, err := NewReader([]byte("<r/>"), WithFlags(DTDValid)); err == nil {
		t.Fatal("tree-only flag")
	}
}

func TestCloseAndCleanup(t *testing.T) {
	d := mustParse(t, "<r/>")
	r := root(t, d)
	d.Close()
	d.Close()
	if r.Kind() != 0 {
		t.Fatal("node of closed document")
	}
	if _, err := r.AppendText("x"); err == nil {
		t.Fatal("edit of closed document")
	}
	before, _ := LiveObjects()
	func() {
		if _, err := Parse([]byte("<forgotten/>")); err != nil {
			t.Fatal(err)
		}
	}()
	for deadline := time.Now().Add(5 * time.Second); time.Now().Before(deadline); {
		runtime.GC()
		if after, _ := LiveObjects(); after < before+1 {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("the cleanup did not free a forgotten document")
}

func TestConcurrentReads(t *testing.T) {
	d := mustParse(t, "<r><a/><b/><c/></r>")
	done := make(chan error)
	for range 8 {
		go func() {
			for range 200 {
				if _, err := d.XPath("//*"); err != nil {
					done <- err
					return
				}
			}
			done <- nil
		}()
	}
	for range 8 {
		if err := <-done; err != nil {
			t.Fatal(err)
		}
	}
	if Version() == "" {
		t.Fatal("version")
	}
}
