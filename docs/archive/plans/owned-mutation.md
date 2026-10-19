# Bounded owned-value mutation plan

This P1 increment adds atomic edits to logical values already present in an
`OwnedDocument`. It deliberately preserves tree topology, names, namespaces,
attribute membership and ordering. Structural mutation needs a separate owned
storage migration because today's records and exact-size iterators encode
preorder as contiguous slot ranges.

`Document` remains immutable. This increment adds no detached nodes, insertion,
removal, reparenting, copying, namespace repair, declaration editing, entity
setter, allocator parameter or foreign-language ABI.

## Identity and public boundary

Split owned handles from borrowing-DOM `NodeId` before either API is stable:

```rust
pub struct OwnedNodeId { /* private document identity and slot */ }

impl OwnedDocument {
    pub fn node(&self, id: OwnedNodeId) -> Option<OwnedNode<'_>>;
    pub fn edit(&mut self, limits: EditLimits) -> Result<Editor<'_>, EditError>;
}

impl OwnedNode<'_> {
    pub fn id(self) -> OwnedNodeId;
    pub fn range(self) -> Option<Range<usize>>;
}

impl Editor<'_> {
    pub fn document(&self) -> &OwnedDocument;
    pub fn set_node_value(
        &mut self,
        node: OwnedNodeId,
        value: &str,
    ) -> Result<(), EditError>;
    pub fn set_attribute_value(
        &mut self,
        element: OwnedNodeId,
        namespace: Option<&str>,
        local: &str,
        value: &str,
    ) -> Result<(), EditError>;
}
```

An owned handle contains a private, nonzero process-local document identity and
an arena slot. A safe atomic counter assigns identities without recycling them,
after XML construction has otherwise succeeded. Moving a document preserves its
identity. A separately parsed document has a different identity even when its
slot layout is identical. Foreign handles are rejected before validation or
allocation. Counter exhaustion returns `ErrorKind::IdentifierExhausted` at
offset zero from the `OwnedDocument` constructor. A private deterministic test
uses an injected local counter and never resets shared production state. The
borrowing DOM keeps its smaller document-local `NodeId` contract.

`Editor` holds an exclusive ordinary borrow; there is no interior mutability,
callback, implicit commit or work on drop. Node and string views therefore
cannot survive into an edit in safe Rust. The separate handle type is also the
future binding-registry input, but the safe core does not promise stable object
addresses or expose raw pointers.

Parsed nodes return `Some(original_range)`, including after value edits: this is
the node's location in its original source, not a claim that current logical
content can still be sliced from those bytes. Future created nodes return
`None`. `0..0` is not used as a generated-node sentinel.

## Supported edits and validation

`set_node_value` accepts existing Text, CDATA, Comment and processing-instruction
data nodes. It rejects document and element nodes. PI targets are unchanged.
`set_attribute_value` finds one existing attribute by expanded name
`(namespace, local)`, preserves its qualified name and list position, and
replaces only its logical value. Missing attributes and non-element handles are
typed errors. Attributes in the XMLNS namespace cannot be edited in this slice.
Lookup keys identify existing data and are neither parsed as new names nor
copied. `Some("")` is distinct from `None` and therefore does not match an
ordinary unnamespaced attribute. A malformed or unknown local selector simply
returns the missing-attribute error. A default namespace never applies to an
unprefixed attribute.

Every supplied value must contain only XML 1.0 characters. Text and attribute
values may contain CR, LF and tab because the writer uses character references
where necessary and reparsing preserves their logical values. Other leaf kinds
must be losslessly representable by the current writer:

- CDATA rejects `]]>` and raw CR.
- Comment rejects `--`, a final `-`, and raw CR.
- PI data rejects `?>`, raw CR, and leading XML whitespace.

The stricter profile prevents a successful edit from being silently normalized
or structurally changed by `write_owned_document` followed by parse. Empty Text
is allowed in memory; the existing serializer omits it, so round-trip comparison
uses concatenated adjacent Text runs and explicitly accounts for disappearance
of an empty Text node. Empty CDATA, Comment and PI data remain distinct.

`EditError` is separate from parse/write errors because caller-created values
have no original document offset. It reports a non-exhaustive kind plus an
optional byte index within the replacement value. Invalid XML characters point
at the scalar's first byte. Forbidden `]]>`, `--`, and `?>` point at the first
delimiter byte; a final comment `-` points at that byte; raw CR points at its
byte; leading PI whitespace points at zero. Handle, kind, selector, limit and
allocation errors have no value index. Length is checked first, then all XML
characters, then kind-specific constraints; competing violations follow that
precedence, with the earliest kind-specific byte reported. Its categories distinguish
foreign/invalid handles, wrong node kinds, missing attributes, forbidden
namespace declarations, invalid XML characters, invalid lexical values,
per-value and cumulative-owned limits, and allocation failure.

## Bounds, accounting and atomicity

`EditLimits` has inclusive `max_value_bytes` and `max_owned_bytes` ceilings and
a documented default matching the writer's 1 MiB token and 64 MiB owned-string
ceilings. The first bounds each replacement value before scanning it; selector
keys are borrowed lookup inputs and are not copied or parsed. The second bounds
cumulative full pool-capacity requests made by all editors for one document.
Opening a new editor does not reset spent work. A caller may raise a later
editor's ceiling, but lowering it below work already spent is rejected.

The parsed pool and its existing spare capacity form the editing baseline;
parsing was already bounded by parser `Limits`. When an edit needs pool growth,
charge the complete requested replacement capacity before fallible reservation,
matching the existing owned-pool policy. Earlier charges remain spent after
success, logical rejection or allocator failure. Obsolete strings stay in the
append-only pool and therefore remain bounded by existing capacity plus charged
growth. Reclamation and compaction are separate measured work.

Each operation follows this order:

1. Resolve document identity, slot, node kind and attribute target.
2. Check the per-value ceiling and validate XML/lexical constraints.
3. Check arithmetic and cumulative capacity before requesting storage.
4. Append the replacement to the pool.
5. Publish one range with no fallible work.

An equal-value call follows the same edit path: it appends, consumes any needed
growth budget and succeeds. It is not an undocumented quota-free no-op.

Any failure preserves every observable value, topology link, handle and source
range. Capacity changes, unreachable staged bytes and cumulative allocation
charges may remain. Rejected editors remain usable, and a later valid edit must
succeed when its independent limits allow it. No operation is terminal.

## Model, fuzzing and conformance evidence

Extend the dependency-free replay driver with a bounded operation stream that
is independent from payload and limit selectors. Seed two documents containing
all leaf kinds, Unicode, ordinary attributes, inherited namespaces, shadowing
and a default-namespace reset. Operations acquire handles, read, replace leaf
values, replace existing attribute values, attempt foreign-handle edits, move a
document between containers, serialize and reopen editors under varied limits.

The oracle is a small independent vector model with owned strings, ordered
attributes and explicit topology. It must not reuse production pool ranges,
validation helpers or sibling traversal. It predicts acceptance and the exact
error category before invoking production, rather than updating itself only
after production succeeds. After every success and failure, compare all values,
node kinds, names, namespaces, attributes, declaration, navigation, traversal
order, identity and source-provenance state. Both setters have mandatory small
valid operations that must succeed under defaults.

For the representable subdomain, serialize and reparse to complete exhaustion
and compare with the model. The only projection is the documented adjacent/empty
Text behavior; a generic normalization exception is forbidden. Literal byte
goldens cover escaping, CR in Text/attributes, Unicode, empty lexical nodes and
preserved namespace declarations. External libxml2 checks consume model-derived
expectations and remain secondary because its infoset projection merges text
and CDATA and omits declaration/XMLNS distinctions.

Ordinary and fuzz tests require below/exact/above boundaries for values and pool
growth, repeated long/short replacement, recovery after rejection, foreign
handles before and after document moves, wrong kinds, every invalid delimiter,
Unicode byte indices, stable attribute order, and unchanged topology. Expanded
name cases distinguish no namespace, a default namespace and two prefixes with
the same local spelling but different URIs; only the selected URI/local pair
changes. Deep and wide trees verify that edits, serialization and destruction
do not recurse. Literal quota goldens and fuzz assertions independently exercise
reopened editors and both UTF-8- and UTF-16-created owned documents, and must
kill blanket rejection, budget reset, delta charging and double charging even
when ordinary tests already catch those mutations.

The review must deliberately inject and catch at least: blanket valid-edit
rejection; per-editor budget reset; delta-only or double capacity charging;
publication before failure; foreign-handle acceptance; wrong slot/attribute
updates; character-count byte indexing; namespace metadata/order loss; empty
CDATA loss; invalid delimiter acceptance; and recursive traversal.

## Implementation sequence

1. Introduce `OwnedNodeId`, document identity and optional owned provenance;
   migrate all owned callers while proving immutable behavior is unchanged.
2. Add public edit types, persistent accounting and transactional pool append.
3. Implement strict leaf and existing ordinary-attribute updates with focused
   success, failure, rollback and serialization tests.
4. Add the independent model/fuzz protocol, exact quota cases, adversarial size
   series, benchmarks, coverage artifacts and documentation.
5. Perform a clean-tree holistic review with deliberate mutations before local
   main integration.

The following mutation milestone will replace owned contiguous-preorder
iteration with live linked traversal and never-reused slots, then add bounded
node construction and permanent subtree removal. Existing-node moves, detached
ownership, slot reuse/generations, cross-document adoption, namespace editing
and repair remain later contracts.

## Compatibility basis

This slice matches libxml2's literal property-value behavior and stable position
when replacing an existing attribute by expanded name. QName-based attribute
creation/resolution, including libxml2's permissive unbound-prefix fallback, is
not exposed here. Libxml2 structural calls have
operation-specific text merging, detached pointer ownership and separate
namespace reconciliation; reproducing those through an immutable preorder
arena would commit the wrong Rust contract.

Primary references are the pinned
[tree API](https://github.com/GNOME/libxml2/blob/v2.15.4/include/libxml/tree.h),
[attribute setter](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L6505),
[node insertion](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L2789),
and [namespace reconciliation](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L5645).
The comparison also reflects quick-xml's event-buffer mutation, roxmltree's
immutable foreign-index caveat, and xmloxide's direct link manipulation; none
provides the checked, atomic, cumulative-budget contract required here.
