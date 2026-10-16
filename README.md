# yaxml

A safe Rust XML processor pursuing libxml2 behavior, with `memchr` as its only
dependency. This is an early implementation, not a libxml2 drop-in replacement.

The current library parses an explicitly bounded subset: XML 1.0 Fifth Edition
in UTF-8, namespaces, attributes, predefined/numeric references, comments,
processing instructions, CDATA, and an immutable DOM. DTDs and other encodings
return structured errors. `Reader` borrows complete input. `PushParser` consumes
arbitrary byte chunks, retaining a bounded token and active document/namespace
state; `read_from` drives it from a caller-provided `std::io::Read`. `Writer`
validates UTF-8 events and serializes them to a caller-provided `std::io::Write`.

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
process memory quota. Scratch token storage is bounded separately by the raw token
ceiling. Streaming copies spend more of the string budget than borrowing slices.
Parsing opens no files or network connections; `read_from` reads only its supplied
source and preserves underlying I/O errors in `ReadError`.

An empty feed means temporary exhaustion; `finish` means final EOF. A callback's
`Break(())` cancels permanently with `Aborted`. Errors are sticky, successful finish
is idempotent, and feeding after finish returns `InvalidState`. Text event spans
are independent of chunk boundaries. Callback values borrow only for the call.
Incremental validation can deliver valid prefix events before discovering a later
error; slice construction validates all input characters before emitting events.

Declarations with a `1.x` version use XML 1.0 processing rules, as required by
that specification; this does not enable XML 1.1 semantics. Encoding names are
syntax-checked before an unsupported encoding is reported. The
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
`InvalidState`. `finish` requires a closed document and flushes the sink. Namespace
repair, formatting, DOM serialization helpers, and UTF-16 output remain pending.

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
and push throughput with 8 KiB chunks; it does not compare equivalent libxml2 workloads.
