# yaxml

A safe Rust XML processor pursuing libxml2 behavior, with `memchr` as its only
dependency. This is an early implementation, not a libxml2 drop-in replacement.

The current library parses an explicitly bounded subset: XML 1.0 Fifth Edition
in UTF-8, UTF-16 and libxml2's built-in single-byte encodings, namespaces, attributes, predefined/numeric references, comments,
processing instructions, CDATA, a borrowed DOM, and an owned DOM with bounded
value edits, leaf insertion and permanent subtree removal. Internal DTD subsets
are processed (entities, default attributes, attribute normalization) and the
declaration is reported as `Event::Doctype`. External subsets and entities
load only through a caller-supplied `Resolver` (see below). Other encodings
return structured errors.
`Node::text_chunks` and `OwnedNode::text_chunks` visit string values without
allocating: containers yield descendant Text/CDATA, and leaves yield their
own payload. Callers can collect the chunks or stream them to a sink.
`Reader` and `Document` borrow complete UTF-8 input.
`OwnedDocument`, `PushParser`, and `BufferedReader` detect UTF-8 and UTF-16LE/BE,
and decode a declared US-ASCII, ISO-8859-1..16 or windows-1252 document.
`PushParser` consumes arbitrary byte chunks; `read_from` drives it from a
caller-provided `std::io::Read`. `BufferedReader` lends one event at a time from
caller-provided `BufRead` input without consuming the unread suffix. Both retain
a bounded token and active document/namespace state. `Writer` validates UTF-8
events and serializes them to a caller-provided `std::io::Write`; fresh writers
can also serialize complete borrowed or owned DOMs.

```rust
use std::ops::ControlFlow;
use yaxml::{Document, Event, PushParser};

fn main() -> Result<(), yaxml::Error> {
    let xml = "<root xmlns='urn:example'><item key='v'>text&amp;more</item></root>";
    let document = Document::parse(xml)?;
    for node in document.root_element().descendants() {
        if let Some(name) = node.name() {
            println!("{} {:?}", name.local(), name.namespace());
        }
    }

    let mut parser = PushParser::new();
    for chunk in xml.as_bytes().chunks(7) {
        parser.feed(chunk, |event, _origin| {
            if let Event::Text(value) = event {
                assert_eq!(value, "text&more");
            }
            ControlFlow::Continue(())
        })?;
    }
    parser.finish(|_, _| ControlFlow::Continue(()))?;
    Ok(())
}
```

Names use `Name::new(qualified, namespace)` for validated construction and
`qualified()`, `prefix()`, `local()` and `namespace()` for access. Borrowed
QName inputs are not copied; owned strings move into the name. Namespace
metadata is preserved, including `Some("")`; Writer checks it against the
actual namespace scope. For example:

```rust
use std::borrow::Cow;
use yaxml::Name;

let name = Name::new(String::from("p:item"), Some(Cow::Borrowed("urn:example")))?;
assert_eq!(name.prefix(), Some("p"));
assert_eq!(name.local(), "item");
# Ok::<(), yaxml::Error>(())
```

This replaces the earlier public name fields. Getter references borrow the
name view, which may own its QName: keep that view alive while using them.
Processing-instruction event and borrowing-DOM targets now use `Cow<str>` too;
construct a borrowed target with `Cow::Borrowed("pi")` or move an owned string.

`Limits` controls input bytes, raw token bytes, element depth, attributes per
element, emitted events, and cumulative requested string capacity for normalized
values, namespace copies, and retained streaming names, plus DOCTYPE bytes and
entity expansion bytes, references and nesting depth. Defaults are finite and
stop billion-laughs and quadratic-blowup documents. The string budget counts work
across an entire operation, including released buffers; it is not a precise
process memory quota. `DecodeLimits` separately bounds decoded UTF-8 bytes and
decoded token bytes, including on the UTF-8 owning/streaming paths. Scratch token
storage is bounded by both token ceilings. Streaming copies spend more of the string budget than borrowing slices.
Parsing opens no files or network connections; `read_from` reads only its supplied
source and preserves underlying I/O errors in `ReadError`.

`BufferedReader::read_event` returns the typed event and a `SourceOrigin`.
The event can borrow the retained token, so it must be dropped before advancing
again. Successful EOF repeats without I/O. XML failures are sticky; an I/O error
is returned once and later calls return `InvalidState` without touching the
source. See the [streaming contract](docs/streaming.md) for exact consumption,
lookahead, and limit behavior.

`SourceOrigin::input_range()` returns an optional exact original-byte span,
including lexical delimiters. Normalization changes values without changing
these spans. Push callbacks receive origins too. `Reader::event_origin()` and
`PushParser::event_origin()` are absent before the first event and retain the
last delivered origin after EOF or failure. Their `event_range()` convenience
method now returns `Option<Range<usize>>`; `Node::range()` does too.
`OwnedNode::origin()` is absent for constructed nodes. `position()` still counts
physical input consumption, which can include lookahead beyond the event span.
Values produced by entity expansion have no input range; `physical_context()`
names the outermost entity reference and `replacement_range()` the span inside
its replacement text.

An empty feed means temporary exhaustion; `finish` means final EOF. A callback's
`Break(())` cancels permanently with `Aborted`. Errors are sticky, successful finish
is idempotent, and feeding after finish returns `InvalidState`. Text event spans
are independent of chunk boundaries. Callback values borrow only for the call.
Incremental validation can deliver valid prefix events before discovering a later
error; slice construction validates all input characters before emitting events.

Declarations with a `1.x` version use XML 1.0 processing rules, as required by
that specification; this does not enable XML 1.1 semantics. Encoding names are
syntax-checked before an unsupported encoding is reported. The
[encoding contract](docs/encoding.md) defines BOM detection, declaration agreement,
surrogate rejection, original byte offsets, and intentional libxml2 differences. The
[conformance regressions](docs/conformance.md) and
[independent oracle protocol](docs/oracle.md) describe the validation scope.

Copy reader events to a writer with explicit namespace declarations:

```rust
use yaxml::{Reader, Writer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = br#"<p:root xmlns:p="urn:example" key="a&amp;b"/>"#;
    let mut reader = Reader::new(input)?;
    let mut writer = Writer::new(Vec::new());
    while let Some(event) = reader.read_event()? {
        writer.write_event(&event)?;
    }
    writer.finish()?;
    assert_eq!(writer.into_inner().as_slice(), input);
    Ok(())
}
```

The [writer contract](docs/writer.md) defines its six resource ceilings,
escaping, namespace checks, and partial-output behavior. Each event is validated
before writing. A sink failure can leave a partial event; later calls return
`InvalidState`. `finish` requires a closed document and flushes the sink.
`write_document` and `write_owned_document` traverse complete trees iteratively,
rewrite a present declaration encoding to `UTF-8`, flush on success, and leave
the writer finished. Namespace repair, subtree output, formatting, and UTF-16
output remain pending.

Use `OwnedDocument` when a tree must outlive its source:

```rust
use yaxml::{EditLimits, OwnedDocument};

let mut document = {
    let input = String::from("<root a='value'>text</root>");
    OwnedDocument::parse(&input)?
};
assert_eq!(document.root_element().name().unwrap().local(), "root");
assert_eq!(document.root_element().first_child().unwrap().text(), Some("text"));
let text = document.root_element().first_child().unwrap().id();
document.edit(EditLimits::default())?.set_node_value(text, "updated")?;
assert_eq!(document.node(text).unwrap().text(), Some("updated"));
let root = document.root_element().id();
let mut editor = document.edit(EditLimits::default())?;
let note = editor.append_comment(root, "reviewed")?;
assert_eq!(editor.document().node(note).unwrap().range(), None);
editor.remove_subtree(text)?;
assert!(editor.document().node(text).is_none());
# Ok::<(), Box<dyn std::error::Error>>(())
```

The [owned DOM contract](docs/owned-dom.md) describes pool-backed views,
source ranges and aggregate string accounting. Retained copies share the
normalization budget, so owned parsing can reach its ceiling before borrowing
parsing. `Document` continues to borrow unchanged input.
`OwnedDocument::create` builds a complete rooted document from borrowed name,
attribute and namespace specifications. `Editor::append_element` adds fresh
elements with initial attributes, preserving expanded names and checked handles.
`Editor` also replaces existing leaf or ordinary-attribute values, appends fresh
Text/CDATA/comment/PI leaves, and permanently removes non-root subtrees.
Text and CDATA require an element parent; comments and PIs may also append after
the document's epilog. Insertion never merges Text nodes. Removal makes all
subtree handles stale and protects the synthetic root and document element.
Parsed survivors retain their source ranges; new nodes have none. `EditLimits`
bounds each supplied string, cumulative pool-capacity requests, and successful
created slots/attributes across editors. Construction also bounds aggregate
supplied bytes, element depth/items and namespace lookup work. Removal refunds
no cumulative counters. Existing-element attribute insertion/removal,
moves/adoption, detached ownership, namespace edits, slot reuse and
compaction remain pending.

`yaxml::xpath` evaluates XPath 1.0 over either DOM: all axes, node tests,
predicates, unions, the four value types, the 27 core functions, and
caller-supplied prefixes, variables and extension functions. Number
conversion and formatting, and `substring`/`round` edge cases, follow libxml2.
Compilation and evaluation are bounded by finite `xpath::Limits`. The
[XPath contract](docs/xpath.md) lists the remaining differences from libxml2.

```rust
let document = yaxml::Document::parse("<r><i k='1'/><i k='2'/></r>")?;
let sum = yaxml::xpath::XPath::compile("sum(//i/@k)")?;
let value = sum.evaluate(&document, document.root().id(), &yaxml::xpath::Context::new())?;
assert_eq!(value, yaxml::xpath::Value::Number(3.0));
# Ok::<(), Box<dyn std::error::Error>>(())
```

Nothing outside the input is read unless the caller passes a `Resolver`
(any `FnMut(Request) -> io::Result<Option<Vec<u8>>> + Send` closure, or the
root-confined `FileResolver`) to `with_resolver` on any parser or DOM. It then
loads the external subset and external parsed entities, like libxml2's
`XML_PARSE_DTDLOAD | XML_PARSE_NOENT` with a custom resource loader; budgets
are `Limits::max_external_loads` and `max_external_bytes`. DTD `ID` attributes
and `xml:id` back `element_by_id` and XPath `id()`.

```rust
let dtd = "<!ATTLIST r id ID #IMPLIED><!ENTITY e 'loaded'>";
let resolver = move |request: yaxml::Request<'_>| {
    Ok((request.uri == "dtd/r.dtd").then(|| dtd.as_bytes().to_vec()))
};
let input = b"<!DOCTYPE r SYSTEM 'r.dtd'><r id='a'>&e;</r>";
let document = yaxml::Document::with_resolver(input, Default::default(), "dtd/doc.xml", Box::new(resolver))?;
assert_eq!(document.element_by_id("a").unwrap().text_chunks().collect::<String>(), "loaded");
# Ok::<(), Box<dyn std::error::Error>>(())
```

`TextReader` is libxml2's `xmlTextReader` cursor (`xmlreader.h`) over any
`BufRead`: node types and depths numbered as libxml2, attribute navigation,
namespace/`xml:lang`/`xml:base` scope, `next`, `read_string`, inner/outer XML
and `expand` to an `OwnedDocument`, in bounded memory. See the
[TextReader contract](docs/textreader.md) for libxml2 agreement and gaps.

```rust
use yaxml::{NodeType, TextReader};

let mut reader = TextReader::new(b"<r xmlns:p='urn:p'><p:i k='v'>x</p:i></r>".as_slice());
while reader.read()? {
    if reader.node_type() == NodeType::Element && reader.local_name() == Some("i") {
        assert_eq!((reader.depth(), reader.get_attribute("k")), (1, Some("v")));
        assert_eq!(reader.read_outer_xml()?.unwrap(), r#"<p:i xmlns:p="urn:p" k="v">x</p:i>"#);
    }
}
# Ok::<(), yaxml::ReadError>(())
```

`yaxml::c14n` writes Canonical XML 1.0 and 1.1 and Exclusive XML
Canonicalization 1.0 for either DOM, whole or as an XPath node-set, with
libxml2's bytes; see the [C14N contract](docs/c14n.md).

```rust
let document = yaxml::Document::parse("<r xmlns:p='urn:p'><p:e b='2' a='1'/></r>")?;
let options = yaxml::c14n::Options::new(yaxml::c14n::Mode::Exclusive10);
let canonical = yaxml::c14n::to_string(&document, &options)?;
assert_eq!(canonical, r#"<r><p:e xmlns:p="urn:p" a="1" b="2"></p:e></r>"#);
# Ok::<(), Box<dyn std::error::Error>>(())
```

`yaxml::html` parses HTML the way libxml2 2.15.4 does (HTML5-style
tokenization, libxml2's legacy tree construction with implied elements and
auto-close tables, recovery with diagnostics, meta/BOM/windows-1252 encoding
selection) into an `OwnedDocument` marked as HTML, incrementally through
`html::PushParser` with chunk-independent results, and serializes it like
`htmlDocDumpMemory` (`html::to_html`) or `xmllint --html` (`html::save`):

```rust
use yaxml::html;

let parsed = html::parse(b"<title>T</title><p>a<p>b &amp; c", html::Options::default())?;
let output = html::save(&parsed.document, false)?;
assert!(output.ends_with(b"<body><p>a</p><p>b &amp; c</p></body></html>\n"));
# Ok::<(), yaxml::Error>(())
```

The [HTML contract](docs/html.md) lists options, encodings, limits, the
xmllint differential and known differences.

The core uses `#![forbid(unsafe_code)]`; standard-library and `memchr` internals
are separate boundaries. Native C ABI compatibility and language adapters need
their own scope decision because libxml2 exposes raw pointers and public mutable
structs. No adapter may weaken the core's restrictions.

The [compatibility catalogue](docs/compatibility.md), [design](docs/design.md),
[roadmap](docs/roadmap.md), and [security plan](docs/security.md) describe the full
target and known gaps. Rust 1.100 allocator support is a planned storage milestone;
the current stable baseline uses ordinary safe vectors. No performance advantage
over libxml2 or complete fuzz execution coverage is claimed yet.
Coverage-guided cargo-fuzz targets and a coverage report script are described in
[fuzz/README.md](fuzz/README.md).

Run `cargo test`, `cargo clippy --all-targets -- -D warnings`,
`cargo bench --bench parse`, `cargo bench --bench pull`,
`cargo bench --bench serialize`, `cargo bench --bench mutation`, and
`cargo bench --bench structural_mutation`. These runners report yaxml-only DOM,
slice-event, push, buffered-pull, complete-tree serialization, value-edit and
structural-edit measurements. The [structural validation report](docs/validation-owned-structure.md)
records the pinned libxml2 conformance results, independent review and measured
fuzz coverage. A separate [matched structural benchmark](docs/validation-owned-structure-matched.md)
compares edit lifecycles with libxml2, including slower workloads and the
differences in Text topology, validation and buffering.

Entities and defaults declared in the internal subset follow libxml2's
`XML_PARSE_NOENT | XML_PARSE_DTDATTR` behavior; see the
[DOCTYPE section](docs/compatibility.md#document-type-declarations) for exact
semantics and known differences:

```rust
let xml = "<!DOCTYPE r [<!ENTITY e '<b>x</b>'><!ATTLIST r a CDATA 'd'>]><r>&e;</r>";
let document = yaxml::Document::parse(xml)?;
assert_eq!(document.doctype().unwrap().name, "r");
assert_eq!(document.root_element().attributes()[0].value, "d");
assert_eq!(document.root_element().text_chunks().collect::<String>(), "x");
# Ok::<(), yaxml::Error>(())
```
