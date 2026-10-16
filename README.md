# yaxml

A safe Rust XML processor pursuing libxml2 behavior, with `memchr` as its only
dependency. This is an early implementation, not a libxml2 drop-in replacement.

The current library parses an explicitly bounded subset: XML 1.0 Fifth Edition
in UTF-8, namespaces, attributes, predefined/numeric references, comments,
processing instructions, CDATA, and an immutable DOM. DTDs and other encodings
return structured errors. The slice reader emits events without constructing a
DOM, but retains the complete input; buffered/push I/O streaming is still pending.

```rust
use yaxml::{Document, Event, Reader};

fn main() -> Result<(), yaxml::Error> {
    let xml = "<root xmlns='urn:example'><item key='v'>text&amp;more</item></root>";
    let document = Document::parse(xml)?;
    for node in document.root_element().descendants() {
        if let Some(name) = node.name() {
            println!("{} {:?}", name.local, name.namespace);
        }
    }

    let mut reader = Reader::new(xml.as_bytes())?;
    while let Some(event) = reader.read_event()? {
        if let Event::Text(value) = event {
            assert_eq!(value, "text&more");
        }
    }
    Ok(())
}
```

`Limits` controls input bytes, raw token bytes, element depth, attributes per
element, emitted events, and cumulative requested string capacity for normalized
values and namespace copies. Defaults are finite. The string budget counts work
across an entire operation, including released buffers; it is not a precise
process memory quota. Parsing performs no filesystem or network access.

The core uses `#![forbid(unsafe_code)]`; standard-library and `memchr` internals
are separate boundaries. Native C ABI compatibility and language adapters need
their own scope decision because libxml2 exposes raw pointers and public mutable
structs. No adapter may weaken the core's restrictions.

The [compatibility catalogue](docs/compatibility.md), [design](docs/design.md),
[roadmap](docs/roadmap.md), and [security plan](docs/security.md) describe the full
target and known gaps. Rust 1.100 allocator support is a planned storage milestone;
the current stable baseline uses ordinary safe vectors. No performance advantage
over libxml2 or complete fuzz execution coverage is claimed yet.

Run `cargo test`, `cargo clippy --all-targets -- -D warnings`, and
`cargo bench --bench parse`. The benchmark reports yaxml-only DOM and slice-event
throughput; it does not compare equivalent libxml2 workloads.
