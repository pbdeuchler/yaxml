# Owned document storage

`OwnedDocument` survives destruction of its UTF-8 or UTF-16 input and supports
bounded value replacement, leaf insertion and permanent subtree removal.
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

`range()` returns `Some(range)` for parsed nodes, referring to their location in
the original encoded input, including delimiters and an element's end tag.
This is persistent provenance, not a promise that current logical content can
be sliced from the original bytes. Created nodes return `None`; parsed survivors
keep their original ranges after edits and removal elsewhere. Pool ranges are private and describe normalized logical
strings. A source range cannot index the pool, and there is no retained source
buffer to slice after input destruction.

`OwnedNodeId` is a Copy handle with private document identity and slot fields.
`node(id)` rejects foreign handles, even when the slot exists in both documents,
as well as out-of-range and permanently removed slots. Moving a document preserves its handles; separately
parsed documents have different identities. The nonzero process-local identity
counter never recycles or wraps; exhaustion returns `IdentifierExhausted` at
offset zero after XML construction otherwise succeeds. IDs are not serialized
identifiers or stable addresses. Slots are never reused, so removed handles
cannot resolve to later insertions. Binding registries remain future work.
The borrowing tree keeps its separate `NodeId` type and
document-local index contract.

This is an intentional API change before stabilization: owned `id()`/`node()`
use `OwnedNodeId`, and owned `range()` now returns an Option. Borrowing-tree IDs
cannot be passed to an owned lookup:

```compile_fail
use yaxml::{Document, OwnedDocument};
let borrowed = Document::parse("<r/>").unwrap();
let owned = OwnedDocument::parse("<r/>").unwrap();
owned.node(borrowed.root().id());
```

## Parsing string budget

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

## Editing existing values

`document.edit(EditLimits)` takes an exclusive borrow and returns an `Editor`.
Its `document()` accessor exposes read-only views between operations. Rust
prevents using a node or string view across an edit; retain `OwnedNodeId`
handles instead. Dropping the editor performs no implicit work.

```rust
use yaxml::{EditLimits, OwnedDocument};
let mut document = OwnedDocument::parse("<r a='old'>text</r>")?;
let root = document.root_element().id();
let text = document.root_element().first_child().unwrap().id();
let mut editor = document.edit(EditLimits::default())?;
editor.set_node_value(text, "new & text")?;
editor.set_attribute_value(root, None, "a", "new\tvalue")?;
assert_eq!(editor.document().node(text).unwrap().text(), Some("new & text"));
# Ok::<(), Box<dyn std::error::Error>>(())
```

`set_node_value` replaces Text, CDATA, Comment or PI data; PI targets stay fixed.
`set_attribute_value` replaces one existing attribute by expanded name
`(namespace, local)`, preserving its qualified spelling and position. Missing
attributes are errors. The default namespace does not apply to ordinary
unprefixed attributes; `Some("")` differs from `None`. Selector strings identify
existing data and are not parsed as names. XMLNS declaration values cannot be
edited.

Values are literal logical strings: `"&amp;"` remains those five characters and
serializes as `&amp;amp;` in Text or attributes. Every replacement must contain
valid XML 1.0 characters. Text and attributes allow CR, LF and TAB, preserved by
writer references where needed. CDATA rejects `]]>` and raw CR; comments reject
`--`, a trailing `-`, and raw CR; PI data rejects `?>`, raw CR, and leading XML
whitespace. These stricter lexical rules avoid changes on serialization/reparse.
Empty values remain present nodes; only empty Text disappears from output.
Adjacent Text runs can coalesce on reparse. Empty CDATA/comments/PIs stay distinct.

`EditError` has public `kind` and `value_offset` fields. Target identity, kind,
attribute lookup and XMLNS restrictions precede value validation. Validation
checks byte length, then all XML characters, then the earliest lexical violation,
then storage. Character and lexical failures report a UTF-8 index in the new
value; other failures have no index. This index is never an original source
coordinate. An error preserves logical state and the editor remains usable.
Names, namespace metadata, declaration, topology, IDs and original source ranges
also persist after successful value edits.

## Leaf insertion and permanent removal

`append_text`, `append_cdata`, `append_comment` and
`append_processing_instruction` create one fresh last child and return its
stable handle. Text and CDATA require an element parent. Comments and PIs also
accept the synthetic document parent, placing the new node after all epilog
content. A leaf cannot be a parent. New nodes have no source range.

```rust
use yaxml::{EditLimits, OwnedDocument};
let mut document = OwnedDocument::parse("<r><private>secret</private></r><!--end-->")?;
let root = document.root_element().id();
let private = document.root_element().first_child().unwrap().id();
let mut editor = document.edit(EditLimits::default())?;
let note = editor.append_comment(root, "redacted")?;
editor.remove_subtree(private)?;
assert!(editor.document().node(private).is_none());
assert_eq!(editor.document().node(note).unwrap().range(), None);
assert_eq!(editor.document().root_element().first_child().unwrap().id(), note);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Payloads follow the value rules above. A PI target must be an XML Name without a
colon and cannot be any ASCII case spelling of `xml`. Invalid XML characters
precede name or lexical errors within each string. Append checks the parent
handle, placement, each string's byte ceiling, target/data validation in argument
order, created-node quota, owned-byte quota, then allocation. Both PI byte
ceilings precede scanning either string. `InvalidName` indexes the target;
character/lexical errors index whichever target or data argument failed.

`remove_subtree` permanently removes a node and every descendant. All their
handles become stale for lookup and editing. The synthetic root and document
element return `ProtectedNode`; foreign, invalid or stale handles return
`InvalidHandle` first. Surviving handles, values, namespace metadata and source
ranges remain unchanged. Removal allocates nothing and does not recurse.

Neither insertion nor removal merges adjacent Text nodes. Appending Text `b`
after Text `a` retains both handles but writes `ab`; empty Text retains a handle
while contributing no bytes. Removing an intervening element has the same Text
behavior. Reparse can coalesce Text runs and omit empty Text. CDATA, comments and
PIs retain their distinct nodes, including empty payloads.

Every error preserves the logical tree and leaves the editor usable. Appends
reserve storage before publishing links. Capacity changes and cumulative charges
may remain after allocation failure. Removal clears payload, provenance and
links from each tombstone; slots, old pool strings and attribute storage are not
reclaimed or zeroized. The document stays well-formed, so complete serialization
remains available under the writer's own limits.

## Editing limits

| `EditLimits` field | Default | Accounting |
| --- | --- | --- |
| `max_value_bytes` | 1 MiB (1,048,576 bytes) | UTF-8 bytes per supplied value or PI target, checked before scanning; excludes selector keys. |
| `max_owned_bytes` | 64 MiB (67,108,864 bytes) | Full newly requested pool capacities across all editors for this document. |
| `max_created_nodes` | 1,000,000 | Successful fresh slots across all editors; excludes parsed nodes and never refunds removal. |

Editing starts with the parsed pool and spare capacity as its baseline. It does
not charge parsing again or reset either editing counter when an editor closes.
A growth charges its full replacement capacity before reservation; earlier
charges remain spent, including failed reservation requests. A lower ceiling
than expenditure already incurred prevents opening an editor; when both
cumulative ceilings are too low, `CreatedNodes` precedes `OwnedBytes`. A later
higher ceiling is allowed. An equal-value replacement follows the same edit path.
Old values remain in the append-only pool. Capacity already available can serve
an edit with zero remaining growth allowance. This bounds requested growth, not resident
memory or all CPU work. PI target and data share one combined reservation: after
parsing `<r/>` into one pool byte, appending target `p` and data `d` charges one
full-capacity request of 3 bytes. A 2-byte editing allowance fails and a 3-byte
allowance succeeds. Removal refunds neither cumulative counter.
Allocation-failure injection remains unimplemented.

The [mutation validation report](validation-owned-mutation.md) distinguishes
implemented behavior from pending coverage, review and performance evidence.

## Next steps

Owned storage supports temporary decoded input, complete UTF-8 serialization
through `Writer::write_owned_document`, and retaining trees in language
adapters. General element/attribute construction, sibling insertion, moves,
copy/import/adoption, detached ownership, slot reuse, compaction, name and namespace
edits, declaration editing, foreign registries, allocator parameters, formatting,
namespace repair, and UTF-16 output remain separate work items. Comparative
parsing/serialization performance and complete fuzz coverage remain open
evidence gates.
