# Bounded owned leaf insertion and subtree removal plan

This P1 increment replaces the owned tree's contiguous-preorder assumption with
live linked traversal, then adds fresh leaf insertion and permanent subtree
removal. It is intentionally smaller than libxml2's general node-moving API.
The public operations are:

```rust
impl Editor<'_> {
    pub fn append_text(
        &mut self,
        parent: OwnedNodeId,
        value: &str,
    ) -> Result<OwnedNodeId, EditError>;

    pub fn append_cdata(
        &mut self,
        parent: OwnedNodeId,
        value: &str,
    ) -> Result<OwnedNodeId, EditError>;

    pub fn append_comment(
        &mut self,
        parent: OwnedNodeId,
        value: &str,
    ) -> Result<OwnedNodeId, EditError>;

    pub fn append_processing_instruction(
        &mut self,
        parent: OwnedNodeId,
        target: &str,
        data: &str,
    ) -> Result<OwnedNodeId, EditError>;

    pub fn remove_subtree(&mut self, node: OwnedNodeId) -> Result<(), EditError>;
}
```

Element and attribute construction, namespace declarations and repair, sibling
insertion, node moves, detached ownership, copying/import/adoption, replacement,
slot reuse, compaction and declaration editing remain separate contracts. The
milestone must be described as **leaf insertion and permanent subtree removal**,
not as general `append_child` compatibility.

## Why this is a useful boundary

Fresh leaves let callers add mixed content, comments and processing instructions
to an existing parsed structure. Permanent removal can prune any nested element
subtree or leaf and can remove prolog/epilog comments and processing instructions.
Together with existing value edits, this supports sanitization, redaction and
annotation without solving namespace-bearing node construction or detached-tree
ownership first.

The boundary cannot construct a document from scratch, add an element or
attribute, introduce a namespace binding, move existing content, or preserve a
removed subtree for later insertion. Those gaps are material and stay explicit
in the compatibility catalogue.

## Pinned libxml2 behavior and intentional differences

The behavioral reference is libxml2 2.15.4. Its
[`xmlAddChild`](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L2870-L2947)
is a move/adopt operation, not merely fresh construction:

- it unlinks the supplied node and appends it;
- an appended Text node can be merged into the preceding Text node, after which
  the supplied node is freed and the surviving sibling is returned;
- attributes enter a separate property list and can replace/destroy a duplicate;
- cross-document insertion rewrites document association, and allocation failure
  can leave the node unlinked with lost strings;
- it generally does not enforce a valid XML parent/child relationship; and
- namespace reconciliation is separate and recommended after moves because raw
  namespace pointers can otherwise outlive their declaration.

[`xmlUnlinkNode`](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L3271-L3333)
only detaches. The caller owns the still-live pointer and must later call
[`xmlFreeNode`](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L3209-L3270),
which does not unlink a linked node. libxml2 permits unlinking the document
element and leaving a document with no root. Neither unlinking nor freeing
coalesces Text nodes exposed on the two sides of a removed subtree.

This milestone differs deliberately:

| Case | libxml2 2.15.4 | This milestone |
| --- | --- | --- |
| Operand | Existing allocated node; insertion moves it | Borrowed strings; insertion creates one fresh leaf |
| Adjacent Text | `xmlAddChild` can merge and free the new node | Never merge; every successful append returns one new stable handle |
| Removal | Detach, then caller-managed free | Atomic permanent removal; every removed handle becomes stale |
| Document element | May be unlinked | Protected so `root_element()` and complete-document serialization remain total |
| Parent validity | Mostly caller responsibility | Checked before allocation; the document remains well-formed |
| Cross-document use | Rewrites document pointers, with partial OOM behavior | Foreign handles are rejected before payload validation |
| Namespace repair | Separate, pointer-sensitive operation | Unneeded here: leaves have no element/attribute namespace and no node moves occur |
| Failure | Some move/OOM paths can partially alter the operand | No logical change; capacity, staged pool bytes and cumulative charges may remain |

The safe Rust API should not reproduce raw detached ownership or a freed-pointer
return convention. A future compatibility layer can project safe operations onto
the C operation index without weakening the core handle model.

The other references reinforce this boundary. quick-xml exposes event buffers
and ownership conversion rather than a mutable DOM. roxmltree 0.20 keeps compact
document-local indices and immutable links, including a predecessor and a
preorder shortcut; its public `NodeId` alone does not carry foreign-document
identity. xmloxide uses an arena and direct document-scoped link mutation, but
its wider API and unsafe foreign boundary do not provide yaxml's stale-handle,
quota or atomicity contract. Adopt the compact arena/link shape while retaining
checked identity and explicit failure behavior.

## Parent and placement rules

The synthetic document node and elements are the only possible parents.

| Operation | Element parent | Synthetic document parent | Any leaf parent |
| --- | --- | --- | --- |
| `append_text` | append as last child | reject | reject |
| `append_cdata` | append as last child | reject | reject |
| `append_comment` | append as last child | append after the current epilog tail | reject |
| `append_processing_instruction` | append as last child | append after the current epilog tail | reject |

Text and CDATA outside the document element would violate XML document grammar.
Comments and non-XML processing instructions are legal after the document
element, so appending to the synthetic document root places them after every
existing top-level child. This API does not insert into the prolog; sibling
placement is deferred.

The target of a new processing instruction must be an XML Name without a colon
and must not be any ASCII case spelling of `xml`. Data follows the existing PI
value rules. The other payloads follow the existing value-editor rules:

- Text accepts XML 1.0 characters, including CR, LF and TAB.
- CDATA rejects `]]>` and raw CR.
- Comment rejects `--`, a final `-`, and raw CR.
- PI data rejects `?>`, raw CR and leading XML whitespace.

Empty payloads are accepted. Empty CDATA, Comment and PI nodes serialize with
their delimiters. Empty Text is a live in-memory node whose writer projection is
empty; it disappears on serialize/reparse.

## Text segmentation

Insertion never coalesces adjacent Text nodes. This keeps the operation total:
one success creates one handle, existing handles never change identity, and no
new node is silently destroyed. For `<r>a</r>`, appending Text `b` produces two
live Text nodes and serializes as `<r>ab</r>`; reparsing produces one Text node
with value `ab`. Appending empty Text changes topology but not serialized bytes.

Removal also does not coalesce. Removing `c` from `<r>a<c/>b</r>` leaves two
adjacent Text nodes, matching `xmlUnlinkNode` topology. Serialization again emits
`ab`, and reparse can merge the run. Exact in-memory conformance and serialized
infoset conformance must therefore be separate assertions. The only
serialize/reparse projection is: remove empty Text and replace each maximal run
of adjacent Text nodes with one Text node containing concatenated values. CDATA
never participates in this projection.

## Removal contract

`remove_subtree` accepts a live node in the editor's document. It permanently
removes that node and all live descendants from navigation, `nodes()`,
serialization and lookup. Every corresponding `OwnedNodeId` immediately returns
`None` from `OwnedDocument::node` and is rejected by later edits. Sibling and
ancestor handles remain valid and unchanged.

The synthetic document node and the document element are protected. Rejecting
their removal preserves the successful-parse invariant that the document has
exactly one element and keeps `root_element() -> OwnedNode` honest. Top-level
comments and processing instructions may be removed. A protected-node error is
distinct from an invalid/stale handle; a second removal of an ordinary node sees
the now-stale handle and returns `InvalidHandle`.

Removing a subtree containing namespace declarations needs no reconciliation:
XML namespace declarations affect only that element and its descendants, all of
which are removed. No remaining node can legally depend on a declaration from a
removed sibling or descendant.

## Storage migration and invariants

Only owned storage changes. The borrowing `Document` retains contiguous preorder
records. The owned tree needs permanent slots and live links, conceptually:

```text
Slot = Live {
    content,
    source_range: Option<Range<usize>>,
    parent,
    first_child,
    last_child,
    previous_sibling,
    next_sibling,
    subtree_size,
} | Removed
```

Parsed nodes receive `Some(original_range)`; created nodes receive `None`.
Slots are never reused during this milestone. Appending pushes one new slot.
Removal changes every slot in the subtree to `Removed`. Pool strings and element
attribute ranges remain append-only and are not reclaimed. This is predictable
stale-handle behavior, not a reclamation design.

`OwnedDocument::node` validates document identity, slot range and live state.
Every public view is constructed only from a live slot. No live link can target a
removed slot. The document root remains live, its `subtree_size` equals the live
node count, and each other live node is reachable exactly once from it.

Owned traversal must stop treating slot order as document order. `children()`,
`descendants()` and `nodes()` follow live links iteratively. `descendants()` keeps
its current `ExactSizeIterator` contract by maintaining `subtree_size` on all
ancestors during append/removal. Appending a new slot after an epilog comment but
under the document element is the decisive regression: slot order places it last,
while document preorder places it before the epilog comment.

`previous_sibling` makes detach and bottom-up iterative deletion constant time per
removed node. A removal can descend through `last_child`, tombstone leaves from
right to left, then tombstone their parents without recursion or a temporary
allocation. Ancestor sizes and the exterior sibling chain are updated only after
all validation has succeeded.

## Limits, error order and atomicity

Add an inclusive `max_created_nodes` field to `EditLimits` and a corresponding
`EditLimit::CreatedNodes`. It counts successful fresh slots across all editors
for one document, excluding the parsed baseline and never refunding removal.
This bounds append/remove churn because slots are not reused. A later editor may
raise the ceiling; opening an editor below already-spent creation work fails.
The default should match the parser's one-million-event ceiling unless measured
metadata costs justify a lower documented value.

`max_value_bytes` applies independently to the Text/CDATA/Comment payload and to
each PI string. `max_owned_bytes` retains its cumulative full pool-capacity
accounting. Node-vector growth uses `try_reserve`; successful created-node count
bounds retained slots, while deterministic allocator-failure injection remains a
separate evidence gap.

Add `EditErrorKind::ProtectedNode` and `EditErrorKind::InvalidName`. For PI target
errors, `value_offset` is the UTF-8 byte index in `target`; PI data errors point
into `data`. Other append methods retain the existing value-offset meaning.
Document this overloading explicitly until the error type gains named input
fields.

Append error precedence is:

1. foreign, out-of-range or removed parent handle;
2. parent-kind/document-placement rule;
3. per-string byte ceiling, in argument order;
4. XML character and lexical/name validation, in argument order;
5. cumulative created-node ceiling;
6. cumulative owned-pool ceiling;
7. allocator failure.

Removal checks handle validity before protected-node status. It has no fallible
work after those checks. A rejected operation leaves topology, values, live
state, handles, source provenance and counters unchanged. Append reserves the
node slot and stages all strings before publishing one live slot and its links;
capacity changes, unreachable staged bytes and already-defined cumulative pool
charges may remain after allocation failure. The created-node counter increments
only when the live slot is published. An editor remains usable after every error.

## Exact topology and byte golden

Use this state transition as the main public golden:

```text
input:
  <!--pre--><r><a>A</a><b>B</b></r><!--post-->

append_text(r, "x"):
  document order = Document, pre, r, a, A, b, B, x, post
  x.parent = r; x.range = None; x is r.last_child
  bytes = <!--pre--><r><a>A</a><b>B</b>x</r><!--post-->

append_comment(Document, "tail"):
  top-level order = pre, r, post, tail
  bytes end with <!--post--><!--tail-->

remove_subtree(a):
  a and A are stale; b, B, x and post keep their handles and provenance
  r children = b, x
  bytes = <!--pre--><r><b>B</b>x</r><!--post--><!--tail-->
```

The new `x` slot is physically allocated after `post` but must appear before it
in `nodes()`. This catches accidental fallback to arena order.

## Conformance matrix

Every success test compares the complete state before and after: declaration,
live handles, kinds, values, names, ordered attributes, namespace URIs,
provenance, parent/first/last/next navigation, child order, descendant order and
length, whole-document order and length, exact serialization, reparse projection,
and unchanged state outside the affected ancestor/subtree set.

### Fresh leaf insertion

| Axis | Required cases and exact result |
| --- | --- |
| Kinds | Text, CDATA, Comment and PI each create one node of that exact kind with `range() == None` |
| Element position | Empty parent becomes nonempty; existing one/three-child parent appends after the old last child and updates only old-last/new/ancestor links |
| Document position | Comment and PI append after root element and every epilog node; Text and CDATA return `WrongNodeKind` with no state change |
| Parent kind | Every leaf kind as parent rejects; foreign and removed parents return `InvalidHandle` before payload validation |
| Values | Empty, ASCII, Unicode, escaping characters, LF and TAB; Text additionally preserves CR logically and writes `&#13;` |
| Lexical failures | NUL/non-XML scalar; CDATA `]]>`/CR; Comment `--`/final dash/CR; PI data `?>`/leading XML space/CR, with exact UTF-8 byte offsets |
| PI target | Empty, digit start, bad continuation, colon, each ASCII case of `xml`, invalid scalar, over/exact byte boundary, and valid Unicode Name |
| Identity | Returned handle resolves to the new node; all earlier handles resolve identically; a moved `OwnedDocument` preserves it; another document rejects it |
| Source provenance | New node is `None`; every parsed survivor retains its exact `Some(original_range)` |
| Arena/document order split | Append beneath an element followed by an epilog node; `nodes()` and serialization visit the new node inside the element, not at arena tail |
| Adjacent Text | Append nonempty and empty Text after Text; both handles remain distinct; exact memory has two nodes, bytes concatenate/omit, reparse uses the stated projection |
| CDATA boundary | Text next to CDATA and two adjacent CDATA nodes remain distinct in memory and after reparse |
| Empty-element spelling | `<e/>` plus a leaf writes a start/end pair; removing its only child writes `<e/>` under current writer rules |

### Permanent removal

| Axis | Required cases and exact result |
| --- | --- |
| Sibling position | Remove first, middle, last and sole child; verify parent first/last and both internal sibling directions |
| Node kind | Remove Text, CDATA, Comment, PI and a nested Element independently |
| Subtree | Remove an element with attributes, namespace declarations, mixed children and depth; every descendant handle becomes stale and no survivor changes metadata |
| Top level | Remove prolog/epilog Comment and PI; root element and remaining top-level order stay valid |
| Protected | Synthetic root and document element return `ProtectedNode`, including with malicious payload limits set to zero |
| Stale/foreign | Second removal and later value edit of every removed handle return `InvalidHandle`; same-slot foreign handles do likewise |
| Text exposure | Removing the separator from `a<c/>b` leaves two adjacent Text handles; bytes/reparse follow the Text projection without in-memory merge |
| Stable slots | Remove a just-created node, append another, and prove IDs differ; the first remains stale permanently |
| Counters | Removal does not refund created-node or pool work; reopening at the spent boundary behaves exactly |
| Editor recovery | After invalid, protected and stale removals, a valid append/removal succeeds in the same editor |

### Boundaries, atomicity and adversarial shape

| Axis | Required cases |
| --- | --- |
| Created-node quota | below/exact/above one append; exact sequences across reopened editors; removal followed by append proves no refund; later raised ceiling succeeds |
| Pool quota | existing spare capacity, first growth, repeated long/short inserts, PI target plus data, full-capacity rather than delta charging |
| Value quota | zero/empty success, exact UTF-8 byte boundary, one byte above, multi-byte scalar boundaries |
| Failure snapshots | wrong parent, invalid PI target/data, every lexical error, both quotas and injected allocation failure preserve complete logical state |
| Depth | On a 256 KiB thread stack, remove the top child of an 8,192-deep subtree, then serialize/drop; no recursive traversal or destruction |
| Width | At least 10,000 siblings: append, remove first/middle/last, traverse and serialize in exact order |
| Tombstones | Alternate append/remove to the creation ceiling; iteration cost is proportional to live topology, lookup rejects all dead slots, and memory stays within parsed baseline plus bounded slot/pool growth |
| Encoding provenance | Parse equivalent UTF-8, UTF-16LE and UTF-16BE inputs, perform the same operations, compare logical state and original encoded ranges of survivors |

Private representation tests should assert after every operation:

- root `subtree_size == live_nodes`;
- each live non-root slot has one live parent;
- first/last and previous/next links are symmetric and acyclic;
- an independent linked walk visits each live slot once and no removed slot;
- each stored subtree size equals an independent walk count;
- `node(id).is_some()` exactly matches live model membership; and
- created count and slot length are monotonic and no removed slot is reused.

## Model, fuzz and differential protocol

Extend the independent mutation model with stable monotonically increasing model
IDs, an ordered child vector, `live` state and optional provenance. Generate saved
and foreign handles plus append/remove/value-edit/serialize/reopen operations.
The model predicts the exact result before production. Mandatory valid appends
and removals prevent blanket rejection from passing. Compare complete state and
representation invariants after every success and failure.

Property families are:

- append increases the selected parent and each ancestor's subtree size by one;
- removal's stale-ID set equals the pre-operation descendant set;
- operations outside the affected ancestor/subtree sets preserve exact state;
- no successful operation makes a previously stale ID live;
- serialize/reparse equality holds under only the explicit Text-run projection;
- operation sequences never create a cycle, dangling live link or second root;
- chunk/input encoding used to construct the document does not affect mutation
  results except original source ranges; and
- any failing operation followed by a known-valid operation matches a fresh model
  that skipped the failure.

Build a pinned libxml2 2.15.4 C differential harness for the compatible
projection. Create fresh leaves with `xmlNewDocText`, `xmlNewCDataBlock`,
`xmlNewDocComment` or `xmlNewDocPI`, then call `xmlAddChild`. For removal, call
`xmlUnlinkNode`, serialize/check survivors, then `xmlFreeNode`; never inspect the
freed pointer. Compare exact node kind/order/value and bytes when no documented
difference applies. Classify, rather than hide, these expected differences:
libxml2 Text coalescing/returned sibling, permissive invalid parent or lexical
content, root-element unlink, pointer ownership and OOM partial updates. Namespace
adoption and repair are outside this harness because this slice creates no
namespace-bearing node and moves nothing.

## Benchmarks and review faults

Benchmark append, first/middle/last subtree removal, repeated append/remove churn,
live traversal with many tombstones, serialization and destruction across depth,
width, payload and input encoding. Report setup, edit, validation/serialization
and drop separately. Compare matched libxml2 fresh-leaf plus unlink/free workloads;
do not compare yaxml permanent removal with libxml2 unlink alone.

The independent holistic review must deliberately inject and catch at least:

1. reuse a removed slot;
2. allow lookup/edit through a tombstone;
3. keep a removed child in parent first/last/next links;
4. traverse arena order instead of document order;
5. fail to update an ancestor subtree size;
6. refund created-node quota on removal or reset it on editor reopen;
7. publish a live slot before a fallible string allocation;
8. accept Text under the synthetic document node;
9. remove the document element;
10. merge adjacent Text and invalidate the new handle;
11. give a created node parsed provenance;
12. retain one descendant handle after subtree removal;
13. accept an invalid or reserved PI target;
14. lose an empty CDATA/Comment/PI during serialization; and
15. recurse during deep removal, traversal or drop.

## Implementation order

1. Split owned records from the borrowing DOM record. Add live/removed slots,
   previous links, optional provenance and subtree sizes while preserving all
   current immutable and value-edit behavior.
2. Replace owned slot-range iteration with iterative live linked iterators and
   prove exact-size/document-order behavior before exposing structural edits.
3. Add persistent created-node accounting, new errors and transactional fresh
   leaf publication.
4. Add allocation-free permanent subtree removal and stale-handle rejection.
5. Add public conformance tests, independent modeled fuzzing, exact quota cases,
   libxml2 projection tests, deep/wide stress, coverage and matched benchmarks.
6. Perform a clean-tree holistic review with deliberate faults before integration.
