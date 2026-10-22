# Owned document storage

`OwnedDocument` survives destruction of its UTF-8 or UTF-16 input and supports
bounded rooted construction, element/leaf insertion, value replacement, moves,
unlink and replacement, attribute, name and namespace edits, copies within and
across documents, compaction and permanent subtree removal.
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
`root_element()` returns the single element (`document_element()` is `None`
for an HTML or `RECOVER` document without one, where `root_element()` is the
synthetic root). `nodes()` visits every node in
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
`harness/benches/parse.rs` fixtures, node and attribute capacity is 93–99% of
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
`add_content` appends to that data (`xmlTextConcat`), or adds text to an
element (`xmlNodeAddContent`): into a Text last child, else as a new one.
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
`set_attribute_value`; [`set_attribute`, `remove_attribute` and
`declare_namespace`](#attributes-names-and-declarations) change membership later.

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

`remove_subtree` permanently removes a node and every descendant, attached or
[detached](#moving-unlinking-and-replacing). All their
handles become stale for lookup and editing. The synthetic root and document
element return `ProtectedNode`; foreign, invalid or stale handles return
`InvalidHandle` first. Surviving handles, values, namespace metadata and source
ranges remain unchanged. Removal allocates nothing and does not recurse.

The `append_*` creators and removal never merge adjacent Text nodes. Appending
Text `b` after Text `a` retains both handles but writes `ab`; empty Text retains
a handle while contributing no bytes. Removing an intervening element has the
same Text behavior. Reparse can coalesce Text runs and omit empty Text. CDATA,
comments and PIs retain their distinct nodes, including empty payloads. Only
`append_child`, libxml2's `xmlAddChild`, merges.

Every error preserves the logical tree and leaves the editor usable. Appends
reserve storage before publishing links. Capacity changes and cumulative charges
may remain after allocation failure. Removal clears payload, provenance and
links from each tombstone; slots, old pool strings and attribute storage are not
reclaimed or zeroized. The document stays well-formed, so complete serialization
remains available under the writer's own limits.

## Moving, unlinking and replacing

Every node except the synthetic root, the DOCTYPE and the document element can
move; each method takes the node's handle wherever it is:

| Method | libxml2 | Behavior |
| --- | --- | --- |
| `append_child(parent, node)` | `xmlAddChild` | Last child of `parent`. Text after a Text last child (not CDATA) is appended to that child, whose handle is returned; `node` is removed. Already last: no change. |
| `insert_before(reference, node)` | `xmlAddPrevSibling` | Before `reference`, never merging Text (2.13 and later). |
| `insert_after(reference, node)` | `xmlAddNextSibling` | After `reference`, never merging Text. |
| `unlink(node)` | `xmlUnlinkNode` | Detaches the subtree. Its handles stay live; it can be inserted again or removed. A detached root is left as it is. |
| `replace(old, new)` | `xmlReplaceNode` | `new` takes `old`'s place and `old` is detached. The document element can be replaced by an element. |

```rust
use yaxml::{EditLimits, OwnedDocument};
let mut document = OwnedDocument::parse("<r>a<b/>c</r>")?;
let root = document.root_element();
let first = root.first_child().unwrap().id();
let b = root.first_child().unwrap().next_sibling().unwrap().id();
let last = root.last_child().unwrap().id();
let root = root.id();
let mut editor = document.edit(EditLimits::default())?;
assert_eq!(editor.append_child(root, first)?, last); // "a" merges into "c"
assert!(editor.document().node(first).is_none());
editor.unlink(b)?;
editor.append_child(root, b)?;
assert_eq!(editor.document().root_element().content().as_deref(), Some("ca"));
# Ok::<(), Box<dyn std::error::Error>>(())
```

Checks run in order: handles (`InvalidHandle`); protected nodes and
entity-reference content, which `save` writes as the reference, so its
structure is read-only (`ProtectedNode`); the parent kind (`WrongNodeKind`
unless an element or the document); `Hierarchy` for a node that would contain
its own parent, a sibling anchor without a parent, Text, CDATA, entity
references or a second element under the document; `LimitExceeded(Depth)` when
the move makes the subtree deeper than `max_depth` and deeper than it was; then
namespace reconciliation, quotas and reservations. Publication only relinks.
A merge checks the joined length against `max_value_bytes` and grows the pool;
merging into a Text node at the pool's tail appends in place.

Detached subtrees are ordinary live nodes outside `nodes()`: `parent()` of
their root is `None`, `path()` starts at their root, and namespace and
`xml:*` lookups stop there. Anything can be inserted into a detached element.
libxml2's leftovers that a checked arena does not need are absent: freeing,
`xmlSetTreeDoc` and the `xmlAddChild` quirk that appends to a Text parent's
content (here `WrongNodeKind`).

## Attributes, names and declarations

| Method | libxml2 | Behavior |
| --- | --- | --- |
| `set_attribute(element, NameSpec, value)` | `xmlSetProp`, `xmlSetNsProp` | An existing attribute with the expanded name keeps its spelling and position and takes the value (`set_attribute_value`). A new one is appended. `Exact` keeps the prefix; `Automatic` takes an in-scope prefixed binding of the URI, else the first unbound `nsN`; the XML namespace is `xml`. |
| `remove_attribute(element, namespace, local)` | `xmlUnsetProp`, `xmlUnsetNsProp` | Removes by expanded name. A declaration (namespace XMLNS, local `xmlns` for the default) is removed only while no name in the element's subtree resolves through it. |
| `declare_namespace(element, prefix, uri)` | `xmlNewNs` | Adds a declaration (`None`: default, `""` undeclares it). The prefix must not already be declared there (`DuplicateAttribute`); `xml`, `xmlns` and the XML/XMLNS URIs are reserved. |
| `set_name(element, NameSpec)` | `xmlNodeSetName` + `xmlSetNs` | Renames; `Automatic` prefers the in-scope default, then a prefixed binding, then `nsN`. |
| `set_url(url)` | `doc->URL` | The URL `base_uri` resolves against. |

New attributes join the ID table when libxml2's `xmlIsID` would: `xml:id`,
HTML `id` (`name` on `a`), or a DTD-declared `ID`; removal drops the entry.
Single-item name errors locate attribute or namespace item 0; the rename locates
the element name. HTML documents take no namespaces. Like construction, these
methods copy strings before returning and check lengths, characters and names
before planning namespaces, then quotas, then reservations.

## Namespace consistency

An owned name stores its URI, so moving a node never changes what it means; what
can break is serialization, where a prefix must be declared in scope. yaxml keeps
a stronger invariant than libxml2: in the attached tree every element and
attribute prefix resolves, where it stands, to its stored URI. Each edit that
could break this reconciles before publishing. For each name of the affected
subtree whose prefix no longer resolves to its URI, the binding is declared on
the outermost element of that subtree that needs it, under the name's own
prefix; descendants then inherit it. Names never change spelling. When the
element itself declares the prefix differently, the edit fails with `Namespace`
and changes nothing. Entity-reference content is not entered, as in
`xmlReconciliateNs`.

```rust
use yaxml::{EditLimits, OwnedDocument, save};
let mut document = OwnedDocument::parse("<r><p:a xmlns:p='urn:p'><p:b/></p:a><c/></r>")?;
let a = document.root_element().first_child().unwrap();
let (b, c) = (a.first_child().unwrap().id(), a.next_sibling().unwrap().id());
document.edit(EditLimits::default())?.append_child(c, b)?;
let saved = save::to_vec(&document, &save::Options::default())?;
assert!(String::from_utf8(saved)?.contains("<c><p:b xmlns:p=\"urn:p\"/></c>"));
# Ok::<(), Box<dyn std::error::Error>>(())
```

libxml2's `xmlAddChild` leaves the moved `ns` pointers alone, so a later free of
the declaring element dangles and `xmlSaveDoc` can write an undeclared prefix;
callers are told to run `xmlReconciliateNs` or `xmlDOMWrapAdoptNode`. That
function declares everything on the subtree root and may rename a prefix to
reuse an in-scope binding of the URI (or invent `default1` for the default
namespace). Reconciling automatically, per element and without renaming, keeps
handles, spellings and XPath results stable; there is no separate
reconciliation call because the tree is never inconsistent. Moves stay within
one document ([copies](#copying-and-importing) cross documents); detached
subtrees resolve against their own root until inserted. `xmlSetNs` on an attribute is `remove_attribute` plus
`set_attribute`.

Each call charges `max_namespace_work` for every element step, examined
attribute and planned declaration probe; declarations count toward
`max_created_attributes` and their strings toward `max_owned_bytes`.

## Copying and importing

| Call | libxml2 | Result |
| --- | --- | --- |
| `editor.import(&source, node, deep)` | `xmlDocCopyNode(node, doc, deep ? 1 : 2)` | A copy of a node of another document (any allocator), detached in this one. |
| `editor.copy(node, deep)` | `xmlCopyNode` | The same within this document; the copy shares the source's strings. |
| `document.try_clone()` | `xmlCopyDoc(doc, 1)` | The whole document in the same allocator. |

The copy comes back detached, without a source range; insert it with
`append_child` or the other moves, which reconcile it against its new parent
like any detached subtree. A shallow copy (`deep` false) keeps an element's
attributes and declarations but no children (libxml2's `extended` 2; its 0,
which drops the element's namespace, is not offered). An entity reference's
content is part of the reference and is always copied. The document node and
the DOCTYPE are `WrongNodeKind`; nodes inside entity content and detached
subtrees can be copied.

```rust
use yaxml::{EditLimits, OwnedDocument, save};
let source = OwnedDocument::parse("<r xmlns:p='urn:p'><a><p:b/><p:c/></a></r>")?;
let a = source.root_element().first_child().unwrap().id();
let mut document = OwnedDocument::parse("<t/>")?;
let t = document.root_element().id();
let mut editor = document.edit(EditLimits::default())?;
let copy = editor.import(&source, a, true)?;
editor.append_child(t, copy)?;
let saved = String::from_utf8(save::to_vec(&document, &save::Options::default())?)?;
assert!(saved.ends_with("<t><a xmlns:p=\"urn:p\"><p:b/><p:c/></a></t>\n"));
# Ok::<(), Box<dyn std::error::Error>>(())
```

**Namespaces.** A copy keeps every name's spelling and URI. Each binding its
names took from outside it is declared once on the copy's root, in first-use
order after the root's own declarations, which is where `xmlDocCopyNode`
puts them; the copy is then namespace-consistent on its own. When that would
need more declarations than placing each on the outermost element needing it
(the root relies on the default namespace being unbound, which a consistent
source cannot produce), declarations go there instead. A declaration that the
new parent makes redundant stays, as with `xmlDocCopyNode` followed by
`xmlAddChild`. Differences:

- `xmlDOMWrapCloneNode` with a destination parent reuses any in-scope binding
  of the URI, renaming the prefix (`p:b` under `xmlns:q="urn:p"` becomes
  `q:b`), declares on each element that needs a binding rather than on the
  root, and renames a conflicting prefix to `p_1`; without a parent it stores
  bindings in `doc->oldNs`, which serialization never writes. yaxml never
  renames, so handles, spellings and XPath results stay stable.
- Inserting a copy whose unnamespaced element relied on the default namespace
  being unbound declares `xmlns=""` on it; `xmlAddChild` does not, and its
  output puts the element in the parent's default namespace.
- A copy taken out of entity content keeps the namespaces of the reference's
  scope, since yaxml expands each reference in its own scope where libxml2
  parses the entity once outside any ([compatibility](compatibility.md#entity-references-without-noent)).

**What the destination decides.**

- *Entity references.* A reference the destination's DTD declares stays a
  reference, with the source's content. One it does not declare is replaced
  by copies of its content, and an attribute's kept references by its
  substituted value; libxml2 keeps a reference that no longer reparses. A
  copied node that is itself such a reference is `Hierarchy`. libxml2 binds a
  kept reference to the destination's declaration, whose content is empty
  until the destination first references the entity.
- *IDs.* The destination types IDs, as `set_attribute` does: `xml:id`, HTML
  `id` (and `name` on `a`) with a value, or a DTD-declared `ID`; the copies
  join its ID table. `xmlDocCopyNode` asks the source's `xmlIsID`;
  `xmlDOMWrapCloneNode` the destination's.
- *DTD defaults* belong to the DTD, not the node, as in libxml2: `attribute()`
  on a copy falls back to the destination's defaults. Defaults that `DTDATTR`
  materialized are ordinary attributes and copy as such.
- *HTML and XML.* libxml2 copies names verbatim, so an HTML node in an XML
  document can write XML that does not parse. Into an XML document, what XML
  cannot represent fails: a non-NCName or `xmlns` attribute name
  (`InvalidName`), characters outside XML (`InvalidCharacter`), a comment
  with `--` (`InvalidValue`). Into an HTML document, any name in a namespace,
  `xml:` attributes included, is `Namespace`, as `set_attribute` refuses it.
  A valueless HTML attribute keeps that flag only in an HTML destination and
  is `a=""` in XML (boolean attributes such as `checked` already hold their
  name as value).
- *Source ranges.* Copies have none; libxml2 copies an element's line number.

**Limits and atomicity.** The copy is staged from the source without changing
anything: kind compatibility, `max_depth` (the copy's own height),
`max_created_nodes` and `max_created_attributes` are checked in preorder as
it goes. Then `max_owned_bytes` is checked for the copied strings (none for
`copy`), every store is reserved, the records are appended unreachable, and
the root's declarations are planned and added under `max_namespace_work`,
`max_created_attributes` and `max_owned_bytes`. A failure there truncates every
store back to its length before the call, so the document, its counters and
its ID table are unchanged. Values come from a document that validated them
and are not checked against `max_value_bytes`.

**Moving between documents.** There is no `xmlDOMWrapAdoptNode`: a handle
names one document, so a node moved to another arena is a new node. A move is
`import` followed by `remove_subtree` on the source's editor; the
[bindings guide](bindings.md#moving-nodes-between-documents) shows how an
adapter remaps its proxies.

**Whole documents.** `try_clone` copies everything a document holds,
detached subtrees, tombstones, the DTD, URL, ID table, diagnostics and
editing counters included, in clones of its allocator. The clone has a new
identity, so handles of the original do not resolve in it, but its `nodes()`
visit the same tree in the same order. It fails only on allocation
(`Allocation`) and identity exhaustion (`IdentifierExhausted`).

## Parsing content in context

| Call | libxml2 | Result |
| --- | --- | --- |
| `editor.parse_content(node, input, options)` | `xmlParseInNodeContext(node, data, len, options, &list)`, `xmlCtxtParseContent(ctxt, input, node, 0)` | The top-level nodes of `input`, parsed as content of `node`, detached in this document. |
| `editor.parse_content(document.root().id(), input, ParseOptions::default())` | `xmlParseBalancedChunkMemory(doc, NULL, NULL, 0, string, &list)` | The same, with the document's entities and no namespaces. |

`input` is any sequence of content: text, several elements, comments, PIs,
CDATA and references, but no XML declaration or DOCTYPE. It is parsed in the
nearest element or document at or above `node` (libxml2's parent chain, so
entity content parses in the document), with that element's in-scope
namespaces and the document's general entities. The nodes come back in
`Fragment::nodes`, detached like a [copy](#copying-and-importing); under the
element they were parsed in they need no new declarations, and anywhere else
insertion declares what the new place lacks.

```rust
use yaxml::{EditLimits, OwnedDocument, ParseOptions, save};
let mut document = OwnedDocument::parse("<r xmlns:p='urn:p' xmlns='urn:d'><a/></r>")?;
let r = document.root_element().id();
let a = document.root_element().first_child().unwrap().id();
let mut editor = document.edit(EditLimits::default())?;
let fragment = editor.parse_content(r, b"text <p:b/><c/>", ParseOptions::default())?;
for node in fragment.nodes {
    editor.insert_before(a, node)?;
}
let saved = String::from_utf8(save::to_vec(&document, &save::Options::default())?)?;
assert!(saved.ends_with("<r xmlns:p=\"urn:p\" xmlns=\"urn:d\">text <p:b/><c/><a/></r>\n"));
# Ok::<(), Box<dyn std::error::Error>>(())
```

**Why a detached list.** The parsed names use the context's bindings, so they
cannot live in a document of their own the way
[`html::parse_fragment`](html.md#fragments)'s nodes do (HTML needs no
context): they are created in the target document, as libxml2 returns an
unlinked list in `node->doc`. Returning them detached composes with every move
(`append_child`, `insert_before`, `insert_after`, `replace`) rather than adding
a parse-and-insert method per anchor, and the caller can still inspect or
discard them first. The borrowed `Document` has no in-context parsing: it
cannot be edited, so the nodes would have nowhere to go.

**How it parses.** The one parser engine runs in content mode: no prolog or
epilog, any number of top-level nodes, no XML declaration (`<?xml` is a
misnamed PI), and the context's bindings seeded below every element. A
document with a DTD reads the content through the DTD stack seeded with its
entity and notation declarations only: libxml2's fresh parser context has no
attribute defaults or attribute types from the document, so neither applies
while parsing. The nodes are then copied in as `import` copies them, so the
document's DTD types their IDs (`element_by_id` finds them once inserted).
Parse budgets (`options.limits`) apply as to a document; edit limits as to an
`import`, and the copy is atomic: a failure leaves the document unchanged.

**Errors.** Malformed content is `EditErrorKind::Parse(kind)` with
`value_offset` the error's offset in `input`; `Error { kind, offset }.describe(&LineIndex::content(input))`
gives libxml2's diagnostic (an end tag with nothing open is
`XML_ERR_NOT_WELL_BALANCED`, 85). That is libxml2's first error;
`xmlParseInNodeContext` returns its last one. A namespace error (an unbound
prefix, `xmlns:p=""`) never fails: the nodes come back with it in
`Fragment::errors`, an unbound name in no namespace, as libxml2 keeps them
([compatibility](compatibility.md#namespace-errors)); inserted, such a node
keeps its prefix undeclared, as libxml2 links it. Under `RECOVER` only limits
fail: the nodes come back with `Fragment::errors`, as libxml2 returns its
list (and `XML_ERR_OK`) once it built one. A DOCTYPE node or an HTML document
is `WrongNodeKind` (`XML_ERR_INTERNAL_ERROR` in libxml2; HTML has
`html::parse_fragment`), and `XINCLUDE` or `DTDVALID` is
`Parse(UnsupportedOption)`. Differences from libxml2:

- The input is UTF-8. `xmlParseInNodeContext` decodes it as the document's
  declared encoding, which its documentation calls "typically not what you
  want"; a text declaration (`xmlCtxtParseContent`'s `hasTextDecl`) is not
  offered.
- A reference is parsed where it stands, in the context's scope and under
  `options`. libxml2 reuses the content it parsed at the document's first
  reference to the entity, outside any namespace scope and under the
  document's options (so under `NOENT` a nested reference in it stays a
  reference); see [copies](#copying-and-importing).
- `xmlParseInNodeContext` does not register IDs (`XML_PARSE_SKIP_IDS`);
  yaxml types them as for any copy.
- `NOBLANKS` inside the content ignores the DTD's element-content
  declarations, which libxml2's `areBlanks` consults.
- External entities are loaded only through `options.resolver`; libxml2
  tries the file system and warns (`XML_IO_LOAD_ERROR`).

`harness/tests/fragment_oracle.rs` compares `parse_content` with
`xmlParseInNodeContext` on every node of seven context documents (prefixes,
default namespaces and `xmlns=""`, DTD entities with markup and nested
references, unparsed and external entities, an unloaded external subset) and
with `xmlParseBalancedChunkMemory`, for random chunks of well-formed and
malformed content (top-level text, several elements, references, misplaced
declarations, end tags with nothing open, cut-off tags) under eight option
sets, through `harness/tooling/libxml2_fragment.c`. It compares the
node list, the document after appending it, success and the first error
code:

```sh
YAXML_FRAGMENT_HELPER=$(sh -c '. ./lib.sh; helper fragment') YAXML_FRAGMENT_CASES=20000 \
  YAXML_FRAGMENT_SEED=1 cargo test --release --test fragment_oracle -- --nocapture
```

At the last run (seeds 1-4, 20,000 chunks each) 98.38-98.55% of chunks agree
exactly (92.02-92.36% while namespace errors were fatal) and none differ
unexplained. The rest are documented: entities libxml2 reuses from the
document (about 0.5%), and about 1% that show the same difference when the chunk is the
content of a whole document (`RECOVER` repairs and `NOBLANKS`, see
[compatibility](compatibility.md#recovery-recover)), which the test checks by
parsing that document on both sides. First error codes agree on 93.7-94.3%
of chunks (97.7-98.0% while namespace errors were fatal; the chunks they
stopped are now compared past them); the others are whole-document diagnostic differences (an invalid
character reported before earlier errors, the offset of a malformed reference
on the DTD path, libxml2's load warning). libxml2's return code (its last
error) equals yaxml's first on 61-63% of failures. The `fragment` fuzz target
parses random content in a random node under random options and edit limits
and checks that a failure leaves the document unchanged, that the nodes come
back detached, and that the document stays namespace-consistent and reparses
the same after they are inserted.

## Compaction and memory growth

Slots are never reused, so stale handles stay rejectable. Three things grow
in a long-lived edited document: the append-only pool (replaced values,
renamed names, merged Text, removed nodes' strings), the attribute arena (an
element whose range is not at the arena tail moves it there to grow, and removed
attributes leave records behind) and tombstoned slots (one record each, bounded
by `max_created_nodes` plus the parsed count). `Editor::compact` rebuilds the
first two in place: live attributes are packed in arena order and every live
pool range, including kept attribute forms and detached subtrees, is copied
into one exactly sized pool. Every handle and view stays the same. The new pool
is the owned-byte baseline, as a parsed pool is, so `max_owned_bytes` again
bounds growth since the last compaction; the node and attribute counters are
not reset. The arena and pool requests are reserved before anything changes;
packing attributes changes no view, so a failed pool reservation still leaves
a consistent document. Reclaiming tombstones would renumber handles and is
left to a rebuild (save and reparse).

## Accessors

Both DOMs share one implementation over the crate's `Navigate` trait, which
also serves `TextReader::base_uri`, XInclude, catalogs, XPath `lang()` and
`NOBLANKS` (whose `xml:space` now skips values other than `preserve` and
`default`, as libxml2's parser does):

| Method (`Node`, `OwnedNode`) | libxml2 |
| --- | --- |
| `previous_sibling()` | `prev`; the borrowed tree climbs from the preceding slot, the owned tree has a link |
| `attribute(name)` | `xmlGetProp`/`xmlHasProp`: first attribute with that name (local name when namespaced) in any namespace, else the DTD default |
| `attribute_ns(local, namespace)` | `xmlGetNsProp`; `None` is no namespace |
| `content()` | `xmlNodeGetContent`: concatenated Text/CDATA of a container or entity reference, a leaf's payload; `None` for the DOCTYPE |
| `list_string(substitute)` | `xmlNodeListGetString`: Text/CDATA of the node and its following siblings; entity references add their content, or without `substitute` are `&name;` with the text escaped as `xmlEscapeText` does (non-ASCII as `&#xH;` without a declared encoding; only `&<>` in HTML) |
| `path()` | `xmlGetNodePath`, same format (`/*` for the default namespace, `[n]` among same-named siblings); `None` for the DOCTYPE and entity content |
| `base_uri()` | `xmlNodeGetBase`, against `url()`; in HTML, the first `<base href>` |
| `lang()`, `space_preserve()` | `xmlNodeGetLang`, `xmlNodeGetSpacePreserve` (elements only) |
| `search_ns(prefix)`, `search_ns_by_href(uri)`, `namespaces()` | `xmlSearchNs`, `xmlSearchNsByHref`, `xmlGetNsList`; `NamespaceBinding { prefix, uri }` |
| `Document::url()`, `OwnedDocument::url()` | `doc->URL`, from `ParseOptions::base` |

libxml2's entity content has the entity declaration as its parent, so its
lookups never see the reference's ancestors; the same scope boundary applies
here. Divergences: the document node's `content()` omits the text of parsed
entity declarations, which libxml2's `xmlBufGetChildContent` reaches through
the DTD node; and a base inside an external parsed entity resolves as an
internal entity's does, since the entity's URI is not retained.

`harness/tests/dom_edit_oracle.rs` compares these accessors on every node, and the tree
after random `append_child`/`insert_before`/`insert_after`/`unlink`/`replace`
and create-then-`xmlAddChild` scripts, or after copy scripts (`import` from a
second document or `copy` within, then `append_child`, as `xmlDocCopyNode` then
`xmlAddChild`; `try_clone` as `xmlCopyDoc`), with `harness/tooling/libxml2_domedit.c`:

```sh
cc -std=c11 -D_GNU_SOURCE -Wall -Wextra -Werror -I../../yaxml-oracle/build \
  -I../../yaxml-oracle/libxml2-2.15.4/include tooling/libxml2_domedit.c \
  ../../yaxml-oracle/build/libxml2.a -lm -lz -o target/domedit-helper
YAXML_DOMEDIT_HELPER=$PWD/target/domedit-helper YAXML_DOMEDIT_CASES=100000 \
  cargo test --release --test dom_edit_oracle -- --nocapture
```

At the last run (seeds 1-3, 100,000 documents each, about 80,000 accepted
operations each, 38,500 of them copies) there were 0 mismatches. Namespaced
documents take copies but no moves, since libxml2 does not reconcile moves.
About 3,400 copies per seed are not sent because they show one of the
documented [copy divergences](#copying-and-importing) (`xmlns=""` on
insertion, an entity the destination never referenced, a copy out of entity
content with namespaces). The `domedit` fuzz target runs random edit scripts
over two parses of one input, with imports between them, copies within one,
clones and swaps, and checks link consistency, namespace resolution, atomic
failure of both documents, stale-handle rejection and save-then-reparse
equality after every operation, that a copy resolves its names on its own and
saves as its source, and that a clone saves as its original.

## Editing limits

| `EditLimits` field | Default | Accounting |
| --- | --- | --- |
| `max_value_bytes` | 1 MiB (1,048,576 bytes) | UTF-8 bytes per supplied value, PI target or URL, checked before scanning, and per Text merged by `append_child`; excludes selector keys. |
| `max_owned_bytes` | 64 MiB (67,108,864 bytes) | Full newly requested pool capacities across all editors for this document. |
| `max_created_nodes` | 1,000,000 | Successful fresh slots across all editors; excludes parsed nodes and never refunds removal. |
| `max_name_bytes` | 1 MiB | Per local name, prefix or namespace URI in construction, `set_attribute`, `set_name` and `declare_namespace`. |
| `max_supplied_bytes` | 8 MiB | All string occurrences in one construction call; repeated borrowed strings count repeatedly. |
| `max_attributes` | 1,024 | Explicit plus generated items on a newly constructed element. |
| `max_depth` | 256 | Prospective element depth; the real document element is depth one. A move may not make a subtree deeper than this unless it was already as deep. |
| `max_created_attributes` | 1,000,000 | Successful fresh ordinary attributes and declarations, including initial creation, `set_attribute`, `declare_namespace` and reconciliation; no removal refund. |
| `max_namespace_work` | 64 MiB | Per-call element-scope probes, binding/expanded-name comparisons and selected prefix work, in construction and reconciliation. |

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
adapters. Slot reuse (a renumbering
rebuild), DOCTYPE and declaration editing, entity-interpreting setters
(`xmlNodeSetContent`), foreign registries, an allocator-aware string pool,
formatting and UTF-16 output remain separate work items. Comparative
parsing/serialization performance and complete fuzz coverage remain open
evidence gates.
