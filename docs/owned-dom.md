# Owned document storage

`OwnedDocument` is an immutable tree that survives destruction of its UTF-8 or UTF-16 input.
Its parser uses the same XML grammar, namespaces, normalization and resource
policy as `Document`; strings are copied into a document-owned UTF-8 pool.
Records keep pool ranges and flat node links rather than references into their
own document. Moving the document or growing the pool preserves those ranges.

```rust
use yaxml::{Error, OwnedDocument, OwnedNodeKind};

fn load() -> Result<OwnedDocument, Error> {
    let input = String::from("<p:r xmlns:p='urn:example' a='x&#9;y'>text</p:r>");
    OwnedDocument::parse(&input)
}

let document = load()?;
let root = document.root_element();
assert_eq!(root.kind(), OwnedNodeKind::Element);
assert_eq!(root.name().unwrap().namespace.as_deref(), Some("urn:example"));
assert_eq!(root.attributes().nth(1).unwrap().value, "x\ty");
assert_eq!(root.first_child().unwrap().text(), Some("text"));
# Ok::<(), yaxml::Error>(())
```

## Views and source ranges

`parse(&str)`, `parse_bytes(&[u8])`, `with_limits(&[u8], Limits)` and
`with_decode_limits(&[u8], Limits, DecodeLimits)` consume complete
input and return a tree only on success. The source is neither borrowed nor
retained. UTF-8 and UTF-16LE/BE are detected under the [encoding contract](encoding.md).
DTDs and other encodings remain explicitly unsupported. UTF-8 retains the
borrowing-reader construction path; UTF-16 feeds the same concrete tree builder
from the incremental decoder. Normalization, streaming names and the owned pool
share one aggregate budget.

`root()` includes the synthetic document node and surrounding comments/PIs.
`root_element()` returns the single element. `nodes()` visits every node in
preorder, including the synthetic root. `OwnedNode` is a Copy view bounded by
its document, with parent, child, sibling and subtree navigation matching `Node`.
`descendants()` includes the selected node itself. Parsing, traversal and
destruction do not recurse with XML nesting.

`kind()` returns an `OwnedNodeKind` tag. `name()` returns an existing `Name` view
by value for elements; `declaration()` returns a declaration view by value when
present. `attributes()` is an exact-size iterator of `Attribute` views in source
order, including XMLNS declarations. Values and namespaces use borrowed Cow
views into the pool. These operations do not allocate strings or collect a new
attribute vector.

`text()` returns the direct text, CDATA, comment or PI payload; elements return
None and do not concatenate descendant text. `processing_instruction_target()`
returns a target only for PI nodes. Empty strings remain present values.

Node source ranges refer to the original encoded input, including delimiters and
an element's end tag. Pool ranges are private and describe normalized logical
strings. A source range cannot index the pool, and there is no retained source
buffer to slice after input destruction.

`NodeId` is the same document-local index used by the borrowing tree. `node(id)`
rejects out-of-range indices, but an in-range ID from another document can select
an unrelated node. Retain the originating document with each ID. Foreign handle
registries and mutation generation checks remain future adapter work.

## Aggregate string budget

The six existing `Limits` fields apply. Owned parsing additionally charges its
pool to the same `max_owned_bytes` counter used by parser normalization and
namespace copies. Each pool growth charges the full newly requested capacity
before reservation, even if the allocator can resize in place. Previous charges
remain spent after reallocation; this bounds cumulative requested allocation
work rather than only the final string length.

The pool grows geometrically, capped by the remaining budget. A growth must fit
the pool's required total length as well as the remaining aggregate allowance.
Unchanged borrowed input is free for `Document`, but its owned copies are not
free for `OwnedDocument`. Owned parsing can therefore return
`LimitExceeded(OwnedBytes)` before a borrowing parser would fail or finish.
Escaped/normalized values can spend both normalization and storage capacity.

Namespace URIs can appear through many node and attribute names. The first
representation copies these logical values and applies its budget before
copying. Input size alone does not bound their expanded storage. Interning and
alternative storage layouts need measurement before replacing this baseline.

Structural vectors use fallible growth, but their capacity, allocator overhead,
caller input and later consumer allocations are outside this string budget.
This is not a process-memory quota, a complete allocation-failure guarantee or
custom allocator support. The core continues to forbid unsafe code and has only
`memchr` as a dependency.

## Next steps

Owned storage supports temporary decoded input, complete UTF-8 serialization
through `Writer::write_owned_document`, and retaining trees in language
adapters. Mutable trees, foreign registries, allocator parameters, formatting,
namespace repair, and UTF-16 output remain separate work items. Comparative
parsing/serialization performance and complete fuzz coverage remain open
evidence gates.
