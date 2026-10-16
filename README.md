# yaxml

A safe Rust XML processor pursuing libxml2 behavior, with `memchr` as its only
dependency. This is an early implementation, not a libxml2 drop-in replacement.

The current library parses an explicitly bounded subset: XML 1.0 Fifth Edition
in UTF-8 and UTF-16, namespaces, attributes, predefined/numeric references, comments,
processing instructions, CDATA, a borrowed DOM, and an owned DOM with bounded
value edits, leaf insertion and permanent subtree removal. DTDs and other
encodings return structured errors.
`Node::text_chunks` and `OwnedNode::text_chunks` visit string values without
allocating: containers yield descendant Text/CDATA, and leaves yield their
own payload. Callers can collect the chunks or stream them to a sink.
`Reader` and `Document` borrow complete UTF-8 input.
`OwnedDocument`, `PushParser`, and `BufferedReader` detect UTF-8 and UTF-16LE/BE.
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
            println!("{} {:?}", name.local, name.namespace);
        }
    }

    let mut parser = PushParser::new();
    for chunk in xml.as_bytes().chunks(7) {
        parser.feed(chunk, |event, _range| {
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

`Limits` controls input bytes, raw token bytes, element depth, attributes per
element, emitted events, and cumulative requested string capacity for normalized
values, namespace copies, and retained streaming names. Defaults are finite. The string budget counts work
across an entire operation, including released buffers; it is not a precise
process memory quota. `DecodeLimits` separately bounds decoded UTF-8 bytes and
decoded token bytes, including on the UTF-8 owning/streaming paths. Scratch token
storage is bounded by both token ceilings. Streaming copies spend more of the string budget than borrowing slices.
Parsing opens no files or network connections; `read_from` reads only its supplied
source and preserves underlying I/O errors in `ReadError`.

`BufferedReader::read_event` returns the typed event and its original-byte range.
The event can borrow the retained token, so it must be dropped before advancing
again. Successful EOF repeats without I/O. XML failures are sticky; an I/O error
is returned once and later calls return `InvalidState` without touching the
source. See the [streaming contract](docs/streaming.md) for exact consumption,
lookahead, and limit behavior.

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
assert_eq!(document.root_element().name().unwrap().local, "root");
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
`Editor` replaces existing leaf or ordinary-attribute values, appends fresh
Text/CDATA/comment/PI leaves, and permanently removes non-root subtrees.
Text and CDATA require an element parent; comments and PIs may also append after
the document's epilog. Insertion never merges Text nodes. Removal makes all
subtree handles stale and protects the synthetic root and document element.
Parsed survivors retain their source ranges; new nodes have none. `EditLimits`
bounds each supplied string, cumulative pool-capacity requests, and successful
created slots across editors; removal refunds neither counter. Element/attribute
construction, moves/adoption, detached ownership, namespace edits, slot reuse and
compaction remain pending.

The core uses `#![forbid(unsafe_code)]`; standard-library and `memchr` internals
are separate boundaries. Native C ABI compatibility and language adapters need
their own scope decision because libxml2 exposes raw pointers and public mutable
structs. No adapter may weaken the core's restrictions.

The [compatibility catalogue](docs/compatibility.md), [design](docs/design.md),
[roadmap](docs/roadmap.md), and [security plan](docs/security.md) describe the full
target and known gaps. Rust 1.100 allocator support is a planned storage milestone;
the current stable baseline uses ordinary safe vectors. No performance advantage
over libxml2 or complete fuzz execution coverage is claimed yet.

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
