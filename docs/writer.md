# UTF-8 event writer

`Writer<W: std::io::Write>` serializes the existing `Event` types to an explicit
sink. It validates each complete encoded event before writing any of that event's
bytes. Its shared parser state checks document structure, declarations, names,
namespace scope, duplicate attributes, and resource limits.

Events from `Reader` already include the namespace metadata the writer requires:

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

`new(sink)` uses `WriterLimits::default()`. `with_limits(sink, limits)` selects
other inclusive ceilings. Neither constructor performs I/O. `write_event(&event)`
borrows the supplied event for the call; retained names and namespace bindings
are copied, so later calls do not depend on its storage.

## Namespaces and spelling

When constructing events directly, every `Name` must have matching `qualified`,
`prefix`, `local`, and `namespace` fields. The qualified spelling determines the
prefix and local name; the namespace must match the current declarations. An
empty namespace is `None`, rather than `Some("")`.

Declare namespaces through ordinary entries in `Element::attributes`, including
the XMLNS namespace metadata supplied by `Reader`. The XMLNS URI is
`http://www.w3.org/2000/xmlns/`. A declaration on an element applies to that
element and its attributes regardless of attribute order. Default namespaces
apply to unprefixed elements; unprefixed ordinary attributes have no namespace.
The predefined `xml` prefix resolves to `http://www.w3.org/XML/1998/namespace`.
Reserved-prefix restrictions, undeclared prefixes, and duplicate expanded
attribute names are rejected. Bindings are restored when their scope ends.

Start and end events produce explicit tags; `Empty` produces `<name/>`.
Attributes keep event order and use double quotes. Declarations keep their field
values and emit version, optional encoding, then optional standalone, without a
trailing newline. Only UTF-8 encoding declarations are supported. A declaration
is optional and can appear only at the start.

Text escapes `&`, `<`, and `>` as `&amp;`, `&lt;`, and `&gt;`; CR becomes `&#13;`.
Escaping every text `>` also prevents adjacent events from producing a raw
`]]>` delimiter. Attribute values escape `&`, `<`, and double quotes; TAB, LF,
and CR become `&#9;`, `&#10;`, and `&#13;`. Attribute `>` stays literal.
These references preserve the caller's logical text and attribute values.

Comments, CDATA, and PI data are written literally within their delimiters.
Comments containing `--` or ending in `-`, CDATA containing `]]>`, and PI data
containing `?>` are rejected. All generated text must contain valid XML
characters. A PI gets one separator space when its data is nonempty. On reparse,
XML line-ending normalization changes raw CR/CRLF in these three value kinds;
the reader also trims leading XML whitespace from PI data.

Only XML whitespace is allowed as text outside the root. It is written literally,
including CR, because character references are forbidden there. Empty Text emits
no bytes. Empty Text and top-level whitespace do not count as semantic events.
Adjacent Text events can coalesce into one lexical text run when reparsed;
CDATA and empty-tag spelling are preserved only when represented by those events.

## Limits and retained state

| `WriterLimits` field | Default | Counts |
| --- | --- | --- |
| `max_output_bytes` | 64 MiB (67,108,864 bytes) | All encoded bytes confirmed by the sink, including markup and references. A complete next event must fit the remaining budget before I/O. |
| `max_token_bytes` | 1 MiB (1,048,576 bytes) | Encoded bytes of one supplied event. |
| `max_depth` | 256 | Element nesting, with the root at depth one. |
| `max_attributes` | 1,024 | Attributes per element, including namespace declarations. |
| `max_events` | 1,000,000 | Shared semantic events, including declarations, Empty, and empty CDATA. Empty Text and top-level whitespace are ignored. |
| `max_owned_bytes` | 64 MiB (67,108,864 bytes) | Cumulative requested string capacity for normalization, namespace copies, and retained structural names, including allocations already released. |

Encoded expansion counts against both byte ceilings. The attribute-count limit
is checked before serializing an attribute slice. Encoded scratch storage grows
fallibly up to the token ceiling, and its capacity can retain a high-water mark.
Other retained state depends on open elements and active/shadowed namespace
bindings. Inactive interned namespace strings are released when their last
binding leaves scope; metadata capacity can remain allocated.

The owned-byte budget is cumulative allocation work, not a resident-memory quota.
Scratch token storage is bounded separately. Caller-owned events and sink buffers
are outside this accounting. Writer and reader budget consumption can differ:
writer escaping adds bytes, validation may normalize references again, and
adjacent Text events can exceed the reader's single-token ceiling after merging.
Choose reparse limits for the encoded output rather than assuming equal quotas.

## Completion and failures

`finish()` requires exactly one completed root and then flushes the sink.
It reports `MissingRoot` or `UnexpectedEof` for an incomplete document and does
not generate missing end tags. Repeating successful finish performs no I/O.
Any later write, including empty Text, starts terminal `InvalidState`.

The first failure returns `WriteError::Xml(Error)` or the original
`WriteError::Io(std::io::Error)`. Display and `source()` preserve that cause.
Every later write or finish returns XML `InvalidState` without touching the
sink. A caught sink panic leaves the same terminal state. Interrupted write and
flush calls are retried; zero-byte writes produce `WriteZero`, and an oversized
reported byte count produces `InvalidData`.

Validation failure emits none of the rejected event, but earlier output remains.
I/O failure can leave part of the current event in the sink. `position()` counts
bytes confirmed by successful write returns, including those before a later
failure. Bytes accepted inside a panicking sink call cannot be measured. XML
error offsets refer to prospective encoded output and can exceed `position()`
even though the rejected event emitted no bytes. Existing output cannot be
rolled back; the caller decides how to discard or retain it.

`into_inner(self)` returns the sink without validating, flushing, or adding end
tags. The writer performs no I/O on drop. The sink's own destructor retains its
usual behavior, so dropping a `BufWriter` can still flush its buffer.

## Remaining scope

The first writer supports the reader's UTF-8 XML profile, with explicit namespace
declarations and end events. DOM serialization helpers, namespace repair,
formatting, UTF-16 output, DTD writing, raw XML injection, and libxml2's remaining
writer convenience APIs are pending. This is neither a canonicalizer nor a
byte-preserving rewrite of source spelling.

The pinned [libxml2 writer](https://raw.githubusercontent.com/GNOME/libxml2/v2.15.4/xmlwriter.c)
informs the state and escaping boundaries. Its EndDocument operation can close
open elements before flushing; this API requires callers to supply complete
events so omitted end tags remain visible as errors. The writer adds no unsafe
code or Cargo dependency. Complete compatibility, sustained coverage-guided
fuzzing, and comparative serialization performance remain open validation work.
