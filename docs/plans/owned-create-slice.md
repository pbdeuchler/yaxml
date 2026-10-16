# Rooted document and element construction slice

This plan is a proposed next implementation slice against `9e4b3a6`. It adds
complete rooted document creation and appending fresh empty elements with their
initial attributes and namespace declarations. It does not add ordinary
attribute insertion/removal on existing elements, namespace declaration on an
existing element, detached nodes, moves, or namespace repair. Those operations
can wait for the attribute-membership representation decision.

This is implementable with the current contiguous attribute arena. Each
element stores a range into `OwnedDocument::attributes` in
[`owned.rs`](../../src/owned.rs#L411), and
[`OwnedBuilder::consume`](../../src/owned.rs#L1010) currently appends parsed
attributes contiguously. Every new element receives one final contiguous range;
creation appends the root's items, and `append_element` appends the new
element's items. Neither operation changes a surviving element's range. The
existing [`append_leaf` transaction](../../src/owned.rs#L636) provides the
model for checked reservations and publication. This slice therefore does not
need geometric blocks or a changed attribute read path.

## Public surface

Keep the borrowed `NameSpec`, `PrefixPolicy`, `NamespaceSpec`, `AttributeSpec`,
`ElementItem`, and `ElementSpec` shape from
[`owned-construction.md`](owned-construction.md). A complete call site remains:

```rust,ignore
let mut doc = OwnedDocument::create(ElementSpec {
    name: NameSpec {
        local: "catalog",
        namespace: None,
        prefix: PrefixPolicy::Exact(None),
    },
    items: &[],
}, EditLimits::default())?;

let parent = doc.root_element().id();
let mut edit = doc.edit(EditLimits::default())?;
let child = edit.append_element(parent, ElementSpec {
    name: NameSpec {
        local: "item",
        namespace: Some("urn:shop"),
        prefix: PrefixPolicy::Exact(Some("s")),
    },
    items: &[
        ElementItem::Namespace(NamespaceSpec {
            prefix: Some("s"),
            uri: "urn:shop",
        }),
        ElementItem::Attribute(AttributeSpec {
            name: NameSpec {
                local: "id",
                namespace: None,
                prefix: PrefixPolicy::Exact(None),
            },
            value: "A-17",
        }),
    ],
})?;
edit.append_text(child, "literal text")?;
```

`create` returns a document with exactly one real element under the synthetic
document node; it never exposes a rootless intermediate document. The root and
appended elements start empty. Callers add literal text with the existing
`append_text`. New records have no source provenance. The XML declaration
remains absent. The existing writer can validate and serialize the resulting
tree; tests can parse the serialized buffer separately for round-trip checks.

`ElementItem` is valid only while constructing a fresh element. Items preserve
caller order. Namespace declarations are stored as XMLNS attributes in the
same contiguous range, followed by generated declarations in order of first
use. Ordinary attribute identity is `(namespace, local)`; duplicate expanded
names within one specification fail. A declaration is not addressable through
ordinary attribute APIs. Attribute membership and each element's contiguous
range remain fixed after construction. The existing `set_attribute_value`
operation remains available because it replaces a value in place without
changing membership or range; insertion/removal is deferred.

## Namespace behavior

Reuse the prefix policies and reserved `xml`/`xmlns` rules in
[`owned-construction.md`](owned-construction.md#namespace-rules). Gather all
explicit declarations in the specification before resolving the element or
its attributes, as the parser does. Preserve every `Exact` spelling. For a
fresh element, an exact attribute prefix may shadow an inherited binding only
when that URI agrees with the element name and every other exact use of that
prefix in the specification. A conflict among exact intents is a namespace
error. This is safe here because the new element has no descendants whose
stored expanded names could be retargeted. Existing-element namespace edits
remain out of scope.

When allocating an automatic prefix, skip all nonempty prefixes requested by
`Exact` in the element and attribute names, including exact requests that occur
later in the item list. This keeps generated `ns1`, `ns2`, … names from making a
later exact name impossible. Explicit declarations are already known before
name resolution and participate in normal scope checks.

Do not materialize every ancestor binding into an unbounded temporary map.
Walk existing element ancestors iteratively, examining each element's
contiguous attribute range. Charge one namespace-work unit before examining
each ancestor node, including an empty element, and before examining every
attribute in its range, including ordinary attributes skipped while looking
for declarations. This bounds wide parsed start tags as well as namespace
probes. Local declarations from the specification are in the bounded staging
plan. Before string comparisons, charge the full length of every operand as a
worst-case bound: prefix and URI bytes for binding resolution, and local-name
plus URI bytes for expanded-attribute duplicate checks. Include prefixes and
URIs from parsed ancestors. The 64 MiB default allowance is per public call,
shared across traversal and comparison work; fail before the next node,
attribute, or comparison that would exceed it. Root creation uses the same
limit, starting with no inherited scope. Scratch state should be proportional
to the supplied item count, not ancestor depth times all inherited attributes.

## Resource limits and accounting

Add construction fields to `EditLimits`; do not introduce a second limits type.
Use these defaults for the first slice:

| Limit | Default | Accounting |
| --- | ---: | --- |
| `max_value_bytes` | 1 MiB | Existing per-value bound, applied to each initial attribute value. |
| `max_name_bytes` | 1 MiB | Per local name, prefix, namespace URI, declaration prefix, or declaration URI. |
| `max_supplied_bytes` | 8 MiB | Sum every supplied string occurrence in one public call, in argument order: names, URIs, prefixes, declaration fields, and values. Repeated identical strings count repeatedly. Check lengths and this aggregate before any XML character/name validation scan. |
| `max_attributes` | 1,024 | Prospective items for each constructed element, including declarations and generated declarations. |
| `max_depth` | 256 | Prospective element depth; root is one. |
| `max_created_nodes` | 1,000,000 | Existing persistent counter across editors; new root and appended elements count, synthetic document root does not. No removal refund. |
| `max_created_attributes` | 1,000,000 | Persistent count of initial and appended ordinary attributes and namespace declarations; no removal exists in this slice. |
| `max_owned_bytes` | 64 MiB | Existing cumulative full pool-capacity requests across editor calls. Creation's pool allocation is charged from zero; names and values are stored once per occurrence, without requiring content deduplication. |
| `max_namespace_work` | 64 MiB | Per-call namespace and expanded-name work: one unit per element ancestor and every attribute examined, plus declaration/prefix candidates, binding/expanded-key comparison operands, and bytes copied for selected inherited/generated prefixes. The synthetic document is not a namespace scope. |

The aggregate supplied-byte limit closes a gap in the wider proposal: a
per-component 1 MiB name/value bound and a 1,024-item start tag could otherwise
require scanning nearly 1 GiB of caller strings in one operation. Count bytes
with checked addition and return `LimitExceeded(SuppliedBytes)` before scanning
when the next length would exceed the ceiling. Per-component limit checks run
before the aggregate check for that component; fields are visited in API
argument order. An overlong component reports its component limit even if it
would also exceed the aggregate. Empty and repeated values are counted by
length/occurrence, not pointer identity.

This slice intentionally adjusts the broader construction proposal's work
split: the earlier draft charges prefix/URI validation scans to
`max_namespace_work` and has no aggregate supplied-byte field. Here,
`max_supplied_bytes` bounds all caller-string validation scans, while
`max_namespace_work` bounds namespace lookup and expanded-name comparisons.
Both are per operation. This split is simpler to audit across local names and
attribute values too, but it is a public quota-contract change that must be
reconciled in the parent proposal before implementation.

Reserved spelling classification (`xml`, `xmlns`, empty prefix/URI and the two
reserved URIs) is caller-string validation covered by `max_supplied_bytes`.
It does not spend namespace lookup work. That rule also covers checking whether
an unprefixed ordinary attribute is named `xmlns`. Scope lookup and duplicate
expanded-name comparisons charge their full operands before comparing, even
when those operands came from supplied strings. Copying an inherited prefix is
charged separately because its bytes were not supplied in this operation.
Consequently a plain unnamespaced root with no items needs zero namespace work.
An unnamespaced append beneath three empty element ancestors needs three scope
steps; two ordinary attributes encountered on that path add two. A matching
five-byte default URI needs one ancestor, one attribute, one declaration
candidate and ten URI-comparison bytes: thirteen units, with no extra charge
for returning a borrowed URI slice.

The aggregate bounds validation work and supplied payload, not all metadata or
allocation overhead. Here, each successful fresh attribute has fixed metadata
in the flat arena, and `max_created_attributes` bounds its entry count. Node
metadata is similarly bounded by `max_created_nodes`; fallible reservation is
still required. The selected string-pool policy separately charges full new
capacity requests. Document allocator overhead as outside these logical quotas.

`max_namespace_work` is separate because ancestor-scope probes can be large
even with a small specification. It also bounds duplicate-name comparisons
across the bounded local item list. Charge before comparisons; do not compare a
long inherited prefix, expanded-name local part or URI first and discover quota
exhaustion afterward. With `max_depth`, per-element `max_attributes`, per-call
work and no retained scope map, both lookup work and scratch are bounded.
Include inherited declarations even when unused. The parser's namespace scope remains the source
of reserved-binding and expanded-name semantics. Its start-tag checks in
[`parser.rs`](../../src/parser.rs#L338) and XML-name validator at
[`parser.rs`](../../src/parser.rs#L792) are the reference. Share helpers where
their contracts match rather than serializing and reparsing a fragment.

## Error and publication order

Extend the existing public errors without changing how existing methods report
their value offsets. Add `EditLimit::{NameBytes, SuppliedBytes, Attributes,
Depth, CreatedAttributes, NamespaceWork}` and
`EditErrorKind::{Namespace, DuplicateAttribute, IdentifierExhausted}`. Proposed
location types are:

```rust,ignore
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditComponent { LocalName, NamespaceUri, Prefix, Value }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditLocation {
    ElementName {
        component: EditComponent,
        byte_offset: Option<usize>,
    },
    NamespaceItem {
        index: usize,
        component: Option<EditComponent>,
        byte_offset: Option<usize>,
    },
    AttributeItem {
        index: usize,
        component: Option<EditComponent>,
        byte_offset: Option<usize>,
    },
}
```

Add `location: Option<EditLocation>` to `EditError`; keep `value_offset` with
its current meaning for existing methods such as `append_text`,
`set_node_value`, and `set_attribute_value`. The new construction methods set
`location` and leave `value_offset` as `None`. `item.index` is the zero-based
index in `ElementSpec.items`. Component errors identify the failing field and
UTF-8 byte offset. Namespace and duplicate-name errors identify the implicated
item when applicable with `component: None` and `byte_offset: None`; semantic
errors must not invent a byte offset. Component and aggregate supplied-byte
limit errors identify the current input component and the first excess byte.
Handle, node-kind, persistent quota, identity and allocation failures have no
location. Adding the field breaks public `EditError` struct literals; take that
source compatibility change under the crate's current pre-1.0 policy.

Freeze the error and publication order as follows:

1. For `append_element`, resolve handle ownership/liveness and require an
   element parent. Then check prospective depth first and known explicit item
   count second. Creation skips the handle check and has root depth one.
2. In API argument order, check each string against its component limit and
   the checked cumulative `max_supplied_bytes` total. This length-only pass
   precedes XML validation.
3. Validate XML characters and local names in field order: the element's local
   name, namespace URI and prefix, then each item in order (namespace prefix
   and URI; or attribute local name, namespace URI, prefix and value). Then
   check all explicit declarations' reserved rules in item order. Report
   component-local UTF-8 byte offsets with the typed `EditLocation`.
4. Fallibly reserve the bounded scratch needed for namespace planning. Scratch
   reservation errors are global `Allocation` errors and have no location.
5. Gather the local declaration plan and reject duplicate declaration prefixes
   in item order before resolving names. Then resolve the element name and
   resolve/check each ordinary attribute in item order. Charge
   namespace-work before each ancestor-node step, every ancestor attribute
   examined, candidate probe, and prefix/local-name/URI comparison; return the
   first exceeded limit before that work. Check duplicate expanded attribute
   names in item order. Check the per-element item count
   immediately after each generated namespace declaration is planned, as well
   as the final count.
6. Check cumulative created node/attribute counts and the full new
   pool-capacity request (`OwnedBytes`) before persistent reservation. Existing
   `edit()` below-spent persistent checks keep their order: created nodes, owned
   bytes, then created attributes.
7. Fallibly reserve only persistent node, attribute and pool capacity. Only
   after every fallible step succeeds, store ranges/records, link the new node,
   update ancestor subtree sizes, flip the preorder fast-path flag, and publish
   successful counters. A failure publishes no element, declaration,
   attribute, handle or partial link. Failed pool-capacity charges and any
   retained persistent allocation capacity follow existing editor accounting.

The result is an empty element with fixed attribute membership and range. Its
existing attribute values remain editable through `set_attribute_value`. The
normal attribute view, serializer, traversal, and source-provenance behavior do
not need a separate path.

## Implementation and evidence gates

1. Freeze the limits, aggregate byte order, namespace-work units, prefix
   allocation policy and indexed error contract in tests before implementation.
2. Implement shared construction-name validation and bounded prefix resolution.
   Exercise default undeclarations, implicit `xml`, explicit declaration
   ordering, exact-prefix inheritance/shadowing, later exact-prefix reservation,
   duplicate prefixes/expanded names, aliases, and malformed characters.
3. Implement rooted creation by staging one synthetic document record, one real
   root record, its final contiguous attribute range and pool strings. Assign
   document identity only after staging and reservations succeed.
4. Implement `append_element` through the existing editor transaction pattern.
   Test parent-kind/handle precedence, depth and quota boundaries, reciprocal
   sibling links, ancestor subtree sizes, preorder mode transition, successful
   recovery after every error and unchanged complete-document snapshots.
5. Add independent public/model tests and writer serialize/reparse parity. Test
   every supplied-byte, component, attribute-count, depth, persistent-node,
   created-attribute, namespace-work and pool boundary below/at/above; include
   aggregate overflow before malformed-string validation and exact indexed
   diagnostics.
6. Use the pinned libxml2 2.15.4 helper for matching rooted creation and child
   cases. Classify intentional differences such as complete-root invariant,
   explicit namespace resolution and literal text separately. Benchmark
   construction and append workloads without changing or filtering existing
   attribute iteration, value-edit or serialization paths. Require the normal
   independent coverage, oracle, resource and holistic-review gates.

## Deliberately deferred

Ordinary attribute upsert/removal on existing elements, `declare_namespace` on
an existing element, attribute-range mutation, namespace replacement/removal,
subtree namespace repair, rename, root replacement, detach/move/adopt, and
geometric-block metadata are not prerequisites for this slice. They remain
separate work after the storage experiment and must not be implied by the new
construction methods.
