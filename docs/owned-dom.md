# Owned document storage

`OwnedDocument` survives destruction of its UTF-8 or UTF-16 input and supports
bounded rooted construction, element/leaf insertion, value replacement and
permanent subtree removal.
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
assert_eq!(root.name().unwrap().namespace(), Some("urn:example"));
assert_eq!(root.attributes().nth(1).unwrap().value, "x\ty");
assert_eq!(root.first_child().unwrap().text(), Some("text"));
# Ok::<(), yaxml::Error>(())
```

## Views and source ranges

`parse(&str)`, `parse_bytes(&[u8])`, `with_limits(&[u8], Limits)`,
`with_decode_limits(&[u8], Limits, DecodeLimits)` and
`parse_with(&[u8], ParseOptions)` consume complete input and return a tree
only on success. Without `NOENT` an entity reference is an
`OwnedNodeKind::EntityRef` node (`entity_name()`) whose children are the
replacement's nodes. The source is neither borrowed nor
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

Values are substituted. Without `NOENT` an attribute that kept a general
entity reference also has libxml2's unexpanded form (`&name;` kept, `&#38;`
for a literal `&`; see [compatibility](compatibility.md#entity-references-without-noent)),
which `unexpanded_attribute(position)` returns and `save` writes. Both DOMs
hold these forms in a sorted side table, `(element, position, form)` in
`Document` and `(attribute index, pool range)` in `OwnedDocument`, so an
attribute without one costs nothing: the table is empty and a lookup is one
binary search. Each kept form costs its bytes (at most the literal's) plus
24 bytes (owned) or 40 bytes (borrowed). `set_attribute_value` drops the
form of the attribute it replaces.

`text()` returns the direct text, CDATA, comment or PI payload; elements return
None and do not concatenate descendant text. `processing_instruction_target()`
returns a target only for PI nodes. Empty strings remain present values.

`text_chunks()` visits the string value without allocating. Documents and
elements yield descendant Text/CDATA in document order, excluding comment and
PI data; leaves yield their own payload once. Empty leaves and adjacent Text
remain separate chunks. Collecting the iterator into a `String` joins those
values; callers can instead write each chunk directly. Iteration uses constant
extra space and visits only the selected subtree. Its shared borrow prevents
edits while the iterator is still in use.

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
counter never recycles or wraps. Parsing reports exhaustion at source offset
zero; rooted creation reports `EditErrorKind::IdentifierExhausted` without an
input location. IDs are not serialized
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
This is not a process-memory quota or a complete allocation-failure guarantee.
The core continues to forbid unsafe code and has only `memchr` as a dependency.

## Supplied allocators

With the `allocator_api` cargo feature (Rust 1.100), node records and
attributes live in a caller-supplied allocator:

```rust,ignore
let document: OwnedDocument<&Arena> = OwnedDocument::parse_in(input, options, &arena)?;
let created = OwnedDocument::create_in(root_spec, EditLimits::default(), shared.clone())?;
assert!(Arc::ptr_eq(created.allocator(), &shared));
```

`OwnedDocument<A = Global>`, `OwnedNode<'d, A = Global>` and
`Editor<'d, A = Global>` default to the global heap, so code without the
feature, or code that never names `A`, is unchanged. `A: Allocator + Clone`.
The document keeps one clone per allocator-backed vector (nodes,
attributes and kept attribute references), which is three. Pass `&A`
to share one allocator, or `Arc<dyn Allocator + Send + Sync>` for an owned,
erased type. `parse_in` accepts the same options and reports the same errors as
`parse_with`. The exception is `XINCLUDE`, which is `UnsupportedOption` because
XInclude builds a new global-heap tree. `create_in` matches `create`.

Node, attribute and kept-reference growth reserves fallibly. While parsing, failure returns
`ErrorKind::Allocation` and drops the partial tree. While editing, it returns
`EditErrorKind::Allocation` before any link is published. Navigation, editing,
XPath, XPointer, C14N, patterns, DTD validation, RELAX NG, XSD, Schematron and
`Writer::write_owned_document` accept any `A`. `save::to_vec`,
`html::to_html`/`save` and `xinclude::process` take `OwnedDocument` (global)
only.

These allocations stay on the global heap for every `A`:

| Allocation | Owner |
| --- | --- |
| String pool: names, namespace URIs, values, text, declaration and DOCTYPE strings | `String`, which takes no allocator; a byte pool would re-validate UTF-8 on every view |
| ID-attribute list, `DTDVALID` diagnostics, `RECOVER` errors | document `Vec`s, built by collecting |
| DTD declarations (`dtd()`), HTML metadata | `Box`es and maps inside them |
| Parser working memory: open-element stack, `xml:id` scratch, tokenizer, normalization, namespace frames, entity expansion, decoding buffers | the reader or push parser, freed when parsing ends |
| Construction scratch: staged names, attributes and prefix planning | `append_element`/`create_in`, freed per call |
| Consumer output and working sets: XPath node sets, C14N and `Writer` buffers, validator state | each consumer |
| The caller's input and resolver-loaded resources | the caller and the `Resolver` |

Parsing reserves two node records (200 bytes each) per `<` in the first 16 KB,
scaled to the input and capped at 4,096; the vector then grows as usual. On
`benches/parse.rs` fixtures, node and attribute capacity is 93–99% of
retained document bytes (node + attribute + pool capacity) for markup-heavy
input; one 500 KB text node reserves 92 records (18 KB), where an estimate
from input length reserved 4,097 (800 KB, 62% of retained bytes).

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

`EditError` has public `kind`, `value_offset` and `location` fields. Adding
`location` and the six construction fields in `EditLimits` breaks exhaustive
struct literals under the crate's pre-1.0 API policy. Prefer defaults with
explicit field overrides for limits. Existing
scalar methods leave `location` empty. Target identity, kind,
attribute lookup and XMLNS restrictions precede value validation. Validation
checks byte length, then all XML characters, then the earliest lexical violation,
then storage. Character and lexical failures report a UTF-8 index in the new
value; other failures have no index. This index is never an original source
coordinate. An error preserves logical state and the editor remains usable.
Names, namespace metadata, declaration, topology, IDs and original source ranges
also persist after successful value edits.

## Creating documents and elements

`OwnedDocument::create(ElementSpec, EditLimits)` returns a complete document
with one root element. `Editor::append_element(parent, ElementSpec)` creates a
fresh last child under an element. Neither exposes a detached or rootless tree.
The synthetic document node and every new element have no source range. Names,
attributes and namespace declarations are copied before the call returns.

```rust
use yaxml::{AttributeSpec, EditLimits, ElementItem, ElementSpec, NameSpec,
            OwnedDocument, PrefixPolicy};
let mut document = OwnedDocument::create(ElementSpec {
    name: NameSpec { local: "catalog", namespace: Some("urn:shop"),
                     prefix: PrefixPolicy::Exact(None) },
    items: &[],
}, EditLimits::default())?;
let root = document.root_element().id();
let mut editor = document.edit(EditLimits::default())?;
let item = editor.append_element(root, ElementSpec {
    name: NameSpec { local: "item", namespace: Some("urn:shop"),
                     prefix: PrefixPolicy::Automatic },
    items: &[ElementItem::Attribute(AttributeSpec {
        name: NameSpec { local: "id", namespace: None,
                         prefix: PrefixPolicy::Exact(None) },
        value: "A-17",
    })],
})?;
editor.append_text(item, "literal & text")?;
assert_eq!(editor.document().node(item).unwrap().range(), None);
# Ok::<(), Box<dyn std::error::Error>>(())
```

`NameSpec` separates local name, namespace URI and prefix policy. `None` means
no namespace; `Some("")` is invalid. `Exact(None)` requests no prefix and
`Exact(Some("p"))` requests `p` without substitution. Ordinary unprefixed
attributes have no namespace. A namespaced attribute must request a prefix or
use `Automatic`; the XML namespace always uses `xml`. The XMLNS URI and
ordinary unprefixed attribute name `xmlns` are reserved for declarations.
Namespace URIs must contain XML characters; general URI syntax is not checked.

`ElementItem::Namespace(NamespaceSpec)` supplies a declaration. All explicit
declarations apply before resolving names, regardless of item order. The
attribute view preserves caller item order, followed by generated declarations
in order of first use. Duplicate prefixes and duplicate expanded attribute
names fail. A fresh element can safely shadow an inherited binding when every
exact use in that specification agrees. Existing descendants are unchanged.

`Automatic` elements prefer a matching default namespace, or declare the
requested URI as their local default when safe. Otherwise they reuse a matching
nondefault prefix or generate `ns1`, `ns2`, and so on. Automatic attributes reuse
the nearest usable nondefault binding in declaration order, then generate a
prefix. A shadowed binding is unusable. Generated prefixes skip later exact
prefix requests, and automatic choices cannot conflict with their requested
URIs. An unnamespaced element adds `xmlns=""` when needed. Conflicting explicit
declarations fail instead of being silently rewritten.

Initial attributes occupy one contiguous range. Their values can use
`set_attribute_value`; membership stays fixed. Attribute insertion/removal on
existing elements and namespace rebinding remain separate work.

Construction errors use `EditError.location`, identifying an element field or
zero-based `ElementSpec.items` index. Byte offsets refer to the supplied UTF-8
component, including the first excess byte for component/aggregate byte
limits. Semantic item errors identify the item without inventing a byte offset.
Handle, node-kind, persistent quota and allocation errors have no location.
Handle/kind and depth/item-count checks precede supplied lengths; all supplied
length checks precede character/name scans. Namespace planning and duplicates
precede persistent node/attribute/pool quota checks and publication. Failed
operations preserve the complete logical tree and remain retryable.

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
| `max_name_bytes` | 1 MiB | Per local name, prefix or namespace URI in construction. |
| `max_supplied_bytes` | 8 MiB | All string occurrences in one construction call; repeated borrowed strings count repeatedly. |
| `max_attributes` | 1,024 | Explicit plus generated items on a newly constructed element. |
| `max_depth` | 256 | Prospective element depth; the real document element is depth one. |
| `max_created_attributes` | 1,000,000 | Successful fresh ordinary attributes and declarations, including initial creation; no removal refund. |
| `max_namespace_work` | 64 MiB | Per-call element-scope probes, binding/expanded-name comparisons and selected prefix work. |

Editing starts with the parsed pool and spare capacity as its baseline. It does
not charge parsing again or reset persistent editing counters when an editor closes.
Rooted creation has no free parsed baseline: its real root, initial attributes
and pool allocation consume their respective counters; the synthetic root is
excluded from created-node accounting.
A growth charges its full replacement capacity before reservation; earlier
charges remain spent, including failed reservation requests. A lower ceiling
than expenditure already incurred prevents opening an editor; when multiple
cumulative ceilings are too low, checks are `CreatedNodes`, `OwnedBytes`, then
`CreatedAttributes`. A later
higher ceiling is allowed. An equal-value replacement follows the same edit path.
Old values remain in the append-only pool. Capacity already available can serve
an edit with zero remaining growth allowance. This bounds requested growth, not resident
memory or all CPU work. PI target and data share one combined reservation: after
parsing `<r/>` into one pool byte, appending target `p` and data `d` charges one
full-capacity request of 3 bytes. A 2-byte editing allowance fails and a 3-byte
allowance succeeds. Removal refunds none of these cumulative counters.
Construction-specific live depth/item limits do not prevent editing values on
a parsed baseline above those ceilings. Supplied-byte and namespace-work
allowances reset for each construction call. Namespace work charges ancestor
attributes even when they are ordinary attributes skipped during scope lookup;
reserved spelling validation belongs to the supplied-byte allowance.
Temporary planning has separate input/work-derived bounds and fallible
reservations. These logical limits exclude allocator overhead and do not form
a process-memory quota.
Private tests use a safe thread-local failpoint at actual editing node-vector,
attribute-vector, pool and construction-scratch growth boundaries. They verify returned `Allocation` errors,
publication atomicity, retained pool charges and editor recovery without global
test races. This seam does not inject faults inside the allocator or establish
operating-system out-of-memory behavior.

The [value-mutation validation report](archive/validation-owned-mutation.md) and
[leaf-structure validation report](archive/validation-owned-structure.md), and
[construction validation report](archive/validation-owned-create.md) distinguish
implemented behavior from pending coverage, review and performance evidence.

## Next steps

Owned storage supports temporary decoded input, complete UTF-8 serialization
through `Writer::write_owned_document`, and retaining trees in language
adapters. Existing-element attribute insertion/removal, sibling insertion, moves,
copy/import/adoption, detached ownership, slot reuse, compaction, name and namespace
edits, declaration editing, foreign registries, an allocator-aware string pool, formatting,
namespace repair, and UTF-16 output remain separate work items. Comparative
parsing/serialization performance and complete fuzz coverage remain open
evidence gates.
