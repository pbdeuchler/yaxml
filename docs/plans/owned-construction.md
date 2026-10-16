# Rough plan for bounded owned-tree construction

This is a design proposal based on `28ca0a4`, not a frozen API or an implemented
feature. The preceding leaf-structure milestone has passed its implementation
and evidence review. Its [matched benchmark
report](../validation-owned-structure-matched.md) now records the required
libxml2 comparison, including slower workloads and measurement limits.

The next coherent slice is complete rooted document creation, fresh child
elements, ordinary-attribute upsert/removal, and namespace bindings that preserve
every existing expanded name. It builds on
[leaf insertion and permanent removal](owned-leaf-structure.md). It does not
reproduce libxml2's detached pointers, adopt/move operations or permissive invalid
trees. The core remains safe Rust with `memchr` as its only dependency.

## API decisions before freezing the plan

The following signatures describe operations, not settled public type names:

```text
OwnedDocument::create(root: ElementSpec, limits: EditLimits)
    -> Result<OwnedDocument, ConstructionError>
Editor::append_element(parent: OwnedNodeId, element: ElementSpec)
    -> Result<OwnedNodeId, ConstructionError>
Editor::set_attribute(element: OwnedNodeId, name: NameSpec, value: &str)
    -> Result<(), ConstructionError>
Editor::remove_attribute(element: OwnedNodeId, namespace: Option<&str>, local: &str)
    -> Result<(), EditError>
Editor::declare_namespace(element: OwnedNodeId, declaration: NamespaceSpec)
    -> Result<(), ConstructionError>
```

Evaluate these two input shapes with real call-site examples before adding types:

| Choice | Advantage | Cost and required contract |
| --- | --- | --- |
| Reuse public `Name`, `Element`, `Attribute` | Familiar vocabulary; existing event producers can supply exact resolved data | `Name` redundantly supplies QName, prefix, local name and URI; all must agree. `Element` owns a `Vec`, and attributes contain `Cow`. Borrow these inputs rather than cloning owned values. Every supplied prefix and QName is a strict spelling request. |
| Borrowed construction-specific specifications | Names can contain only local name, URI and prefix choice; slices avoid a caller-side attribute vector; indexed diagnostics fit naturally | More public types. Their names, defaults, constructors and lifetime ergonomics need review. They must not become a parallel general tree framework. |

Prefer a borrowed specification if examples demonstrate a clear improvement.
Its conceptual name contains an expanded name and a prefix policy. Distinguish
`Exact(None)` (explicitly unprefixed), `Exact(Some("p"))`, and `Automatic`;
an ordinary `Option<&str>` cannot distinguish those three intentions. Reusing a
public `Name` is always exact, including its unprefixed spelling. Never silently
rename or substitute a caller's explicit prefix. An automatic choice may select
a deterministic usable binding or create a deterministic fresh prefix.

An element specification should accept ordered start-tag items: namespace
declarations and ordinary attributes. The complete local declaration set is in
scope for all names regardless of item order. Preserve explicit item order;
append generated declarations in a documented deterministic order. XML permits
a declaration to follow an attribute that uses it in the same start tag.

No content string is interpreted as XML or entity syntax. Callers append Text
through the existing literal-value API. Whole-subtree batch construction is not
required for this slice; each public operation is individually atomic.

## Complete documents and handles

`create` returns a document with exactly one element. Do not expose an empty
`OwnedDocument` whose `root_element()` could fail, and do not create a temporary
fake root by parsing `<r/>`. Stage the synthetic root, real root, attributes,
declarations and pool directly, then assign the non-recycled document identity
after all other validation and storage work succeeds. Identity exhaustion has
an explicit typed result. Whether to extend `EditError` or introduce a
construction error is part of the API review below.

Root element depth is one. The synthetic root does not consume the created-node
counter; the new real root does. Initial created attributes/declarations and
initial pool growth consume the construction/editing counters: there is no
parsed baseline to exempt. A returned document can immediately use the existing
editor, views and complete-document writer. XML declaration creation/editing is
deferred; newly constructed documents initially have no declaration.

`append_element` accepts an element parent, creates one empty last child, and
returns its checked handle. The synthetic document already has its protected
root and cannot receive a second element. Fresh slots retain the existing
non-recycled document identity plus monotonically appended slot semantics.
Removed slots never become live again. New records have `None` source provenance;
parsed survivors retain their original ranges. No view crosses an exclusive edit
borrow. Bindings retain document ownership and use checked handles, not record
addresses. Attribute IDs are not added speculatively.

Existing leaf insertion/removal, Text segmentation and root protection remain
unchanged. Every topology update preserves reciprocal links, ancestor counts,
exact-size iteration and the permanent transition out of contiguous preorder.

## Namespace rules

Expanded names are semantic identity; prefixes describe lexical spelling.
Validate local names and nonempty prefixes with the parser's existing
XML Name-without-colon rules,
then enforce namespace constraints with shared semantic helpers. Avoid emitting
an XML token and reparsing it merely to validate construction.

| Input | Required result |
| --- | --- |
| Unnamespaced element under a nonempty default binding | Preserve no namespace by creating local `xmlns=""` when no explicit item conflicts |
| Namespaced, explicitly unprefixed new element | Use a matching inherited/default declaration, or add a local default declaration |
| Namespaced new element with exact prefix | Preserve that prefix; safely declare/shadow it on the fresh element if needed |
| Prefix with no namespace | Reject |
| Ordinary unprefixed attribute | No namespace; the default binding never applies |
| Namespaced attribute with exact prefix | Preserve the prefix; resolve or safely add its binding, otherwise reject a conflict |
| Namespaced attribute with automatic prefix | Reuse a deterministic matching nondefault binding, otherwise generate a fresh prefix |
| Namespaced attribute with explicitly unprefixed spelling | Reject; do not silently add a prefix |

Use `None` for an absent construction namespace; reject `Some("")` as a name's
namespace. An empty URI remains valid for an explicit default undeclaration.
Keep existing selector behavior: `Some("")` does not match `None` when selecting
an existing attribute. Namespace URIs are literal XML-character strings; preserve
the current lack of general URI-syntax validation rather than claiming it.

Enforce reserved `xml`/`xmlns` prefix and URI rules consistently with parsing.
The XML binding is implicit; an explicit matching `xml` declaration may be
retained. Reject duplicate local declaration prefixes, including duplicate
default declarations. Reject duplicate ordinary attributes by expanded name,
including aliases with different prefixes and the same URI/local pair. An
explicit declaration inconsistent with an explicit name is an error, not a
request for automatic repair.

### Declarations on existing elements

A local prefix rebind can change the serialized meaning of existing elements,
attributes and descendants even though their stored URI ranges are unchanged.
Do not introduce that inconsistency.

Support append-only `declare_namespace` for a named prefix that is currently
unbound, or whose effective binding already has the same URI. A duplicate
declaration on that element still fails. An inherited conflicting binding fails
even if it appears unused; inspecting and rewriting affected descendants is a
separate namespace-repair contract. For the default namespace, allow only an
unchanged effective binding: introducing a nonempty default where none existed
can retarget existing unprefixed nodes. A fresh prefixed binding absent from
scope cannot retarget an existing valid use:
descendants using that prefix already require their own local declaration.

Attribute upsert may add a matching safe declaration in the same transaction.
For an ordinary nonreserved URI, automatic choice can select an unused prefix
within its work allowance. The XML URI uses its reserved `xml` prefix. An exact
conflicting prefix must fail. Thus an arbitrary supported
namespace URI remains usable without changing the requested spelling silently.
Namespace replacement/removal is deferred. Removing an ordinary attribute leaves
its declarations in place; do not guess that a binding is unused.

Keep declarations in the ordered attribute view in the XMLNS namespace, as
today. Existing declaration order survives ordinary-attribute edits. New
declarations count toward both live start-tag items and cumulative attribute
slots. General moves, namespace adoption and subtree repair remain out of scope.

## Attribute membership and compact storage

Current owned elements store a contiguous attribute range in one global vector;
the [builder](../../src/owned.rs) and [serializer](../../src/serialize.rs) depend
on its exact-size view. Insertion/removal requires a deliberate migration before
exposing the API.

Prototype a global attribute-slot arena with a private next link and an element
descriptor containing first slot plus live count. An existing upsert/removal
search can retain the predecessor and tail, avoiding a separate tail scan.
Retired slots clear payload/link metadata and are not reused. Keep views borrowed,
allocation-free and exact-size, with O(1) `len`/`count`; update ordinary value edits
to resolve through the new membership.

Measure this against a per-element vector and immutable contiguous-block copying.
Per-element vectors introduce many allocations and enlarge element content.
Block copying preserves the current descriptor but can retain O(attributes)
obsolete metadata per membership change, requiring an explicit work/storage
budget. Linked slots can retain a two-word descriptor but add link bytes and a
dependent load per attribute. Do not freeze a 144-byte record requirement at the
expense of a clearer or faster representation; report actual sizes and matched
parse/traversal/serialization results before choosing. Avoid pointer arithmetic,
self-references, public layout promises and a generic collection framework.

Ordinary upsert matches URI/local identity. New attributes append; replacements
keep their position and honor an explicit requested spelling through the safe
binding rules. Automatic spelling should preserve a usable existing spelling
before choosing another. Duplicate creation in one element specification fails;
upsert of an already-present expanded name replaces instead. Removal selects by
expanded name, returns a missing-attribute error when absent, and unlinks exactly
that slot. XMLNS items cannot be edited through ordinary-attribute APIs.

## Resource policy and transaction boundary

Exact field names/defaults need the API review. Preserve finite defaults and
distinguish live ceilings, persistent work counters and temporary work:

| Resource | Proposed accounting |
| --- | --- |
| Element depth | Prospective depth of a new element; root is one. Check parent ancestry iteratively. No whole-document rescan on editor open. |
| Live attributes | Prospective start-tag items, including declarations. Membership growth must fit; deletion and existing value-only edits remain available on a parsed baseline above a later ceiling. |
| Created nodes | Existing persistent counter, including new elements; parsed nodes and synthetic roots excluded; no removal refund. |
| Created attribute slots | Persistent successful fresh slots, ordinary and declaration; parsed baseline excluded; no refund. In-place replacement does not create a slot. |
| Supplied bytes | Per QName/local/prefix/URI/value string ceiling before scanning; no unbounded name or URI exception. |
| Retained string work | Existing cumulative full requested pool capacities, including charged failures; initial construction has no free parsed baseline. Reuse known immutable URI pool ranges when safe, without content-dedup scans. |
| Namespace work | Per-operation bounded inspected declarations, prefix probes, generated-prefix attempts and bytes hashed/compared. Include an oversized parsed ancestor baseline; charging only new declarations is insufficient. |

Opening below spent persistent counters fails in a frozen documented order;
raising a ceiling is allowed. Do not reset counters on reopen. Attribute/node
metadata is outside the string ceiling but bounded by baseline plus persistent
slot quotas and documented vector growth. Scratch namespace/index metadata must
also have a finite bound and use fallible reservation.

Build scope once per operation rather than scanning every ancestor for each
attribute. Assign temporary URI identities once per distinct resolved binding
and check duplicate attributes using those identities. Include bytes in the work
allowance: rehashing a long shared URI for every attribute must not bypass bounds
merely because its pool range is reused. Store pool ranges/indices in staged
metadata; do not retain pool references across a reservation that can move it.

The transaction is validate, plan, reserve, store, then publish. Freeze exact
error precedence before implementation. The proposed order is:

1. Check handle identity, liveness, kind and document placement.
2. Check item count and every supplied string's byte ceiling in argument order.
3. Validate XML characters and names in indexed input order; enforce reserved
   bindings, namespace agreement and duplicate expanded names.
4. Check prospective depth, persistent slot ceilings and namespace work.
5. Preflight the complete string request and every metadata capacity request.
6. Reserve scratch, node/attribute arenas and pool in a documented order.
7. Store all strings and records, then publish one coherent membership/topology
   update and successful-slot counters with no remaining fallible work.

Namespace-work exhaustion necessarily occurs while resolving step 3; document
that first-exhaustion result rather than pretending resolution can finish for
free before reporting step 4. The frozen plan should make this exception explicit.
Scope and duplicate-name planning can also need fallible scratch allocation.
Check its own size/work ceiling before each reservation, and freeze whether
that allocation can precede later namespace/duplicate errors. Do not promise
that all semantic checks precede every allocation while also requiring an
allocated scope index to perform those checks. Persistent arena/pool growth
and publication still wait until planning succeeds.
Pool quota preflight must precede metadata reservations, as in leaf insertion.
Combined operations reserve their complete string payload once; freeze literal
capacity-charge goldens, not allocator-specific observed-capacity equalities.

Rejected validation/quota operations preserve logical state and counters.
Allocation failure may retain capacity and documented cumulative charges, but
cannot publish a partial element, attribute, declaration, link or handle. A
failed insertion consumes no successful-slot counter. Editor recovery remains
supported. Extend safe reservation failpoints to each new boundary.

### Indexed errors

`EditError.value_offset` alone cannot identify which member of an element's
attribute/declaration slice failed. Review extending that error with a typed
argument location versus a construction-specific error sharing `EditErrorKind`.
Provisional locations include element name component, declaration index and
prefix/URI component, and attribute index and name/value component. Byte offsets
remain UTF-8 indices within that component, never source coordinates. Handle,
placement, count, quota and allocation failures need no invented byte index.
New document creation also needs typed identity exhaustion. Avoid string-only
field paths so future bindings can translate diagnostics directly.

## Pinned references and compatibility differences

The inspected source is local libxml2 2.15.4 `tree.c`; links pin that tag:

| Symbol/location | Observed behavior and design implication |
| --- | --- |
| [`xmlNewDoc`, 880](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L880) | Creates a rootless document and defaults version to 1.0. Yaxml retains a rooted complete-document invariant. |
| [`xmlNewNs`, 517](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L517) | Appends declarations in order; duplicate local prefixes fail; URI syntax is not checked. Preserve order without importing raw namespace ownership. |
| [`xmlNewDocNode`, 1899](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L1899) | Its content path interprets references and can silently ignore malformed content. Keep yaxml's literal values and explicit errors. |
| [`xmlNewDocRawNode`, 1976](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L1976), [`xmlNewTextChild`, 2082](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L2082) | Provide raw-text construction, closer to separate element/append_text operations. |
| [`xmlNewChild`, 2673](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L2673) | NULL namespace inherits the parent's namespace pointer. Yaxml resolves explicit expanded identity and lexical binding, without this implicit inheritance. |
| [`xmlNewProp`, 1545](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L1545), [`xmlNewNsProp`, 1571](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L1571) | Append literal attributes without duplicate checking. Yaxml preserves unique expanded attributes. |
| [`xmlSetProp`, 6516](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L6516), [`xmlSetNsProp`, 6556](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L6556) | QName lookup and namespace-aware upsert are distinct; replacement finds URI/local identity and assigns the supplied namespace association at line 6587. Use expanded identity and an explicit spelling policy. |
| [`xmlUnsetProp`, 6473](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L6473), [`xmlUnsetNsProp`, 6493](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L6493) | Select, unlink and free attributes. Safe slot retirement replaces raw pointer invalidation. |
| [`xmlDocSetRootElement`, 4702](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L4702) | Unlinks/adopts the supplied root and returns the detached former root. Root replacement/adoption remains deferred. |

Distinguish upstream comments from executed evidence. In `xmlSetProp`, an
unresolved prefix can fall through to `xmlSetNsProp(node, NULL, name, value)`
with the original QName, despite the comment describing a missing binding as
an error. Add an oracle fixture before classifying its observable result;
yaxml's namespace-valid construction must reject such an unbound exact prefix.

The [documented reference lessons](../design.md#lessons-from-the-references)
remain applicable: borrow event/view data, use compact document-scoped arenas,
and measure mutation's ownership costs. quick-xml's event API and roxmltree's
immutable tree do not establish mutable namespace behavior. Do not infer an
xmloxide stale-handle or quota guarantee from its broader arena API.

## Implementation and evidence stages

1. Close the current matched leaf-structure benchmark gate. Review this rough
   plan with representative call sites, settle input/error types and exact
   namespace/limit policy, then freeze an implementation plan.
2. Migrate attribute membership privately, preserving all existing values,
   source/order, views, serialization and failure behavior. Measure sizes,
   allocations, parsing and attribute traversal before exposing new methods.
3. Extract shared QName/reserved-binding validation. Implement atomic rooted
   creation and fresh empty elements, with explicit/generated declarations and
   stable IDs. Keep borrowing DOM storage unchanged.
4. Add ordinary-attribute upsert/removal and safe append-only declarations on
   existing elements. Extend quotas, indexed diagnostics and reservation faults.
5. Add independent public/model/fuzz/oracle tests, matched workloads, coverage
   artifacts and a clean holistic review before integration.

The independent model must store expanded identity separately from prefix
spelling and declarations, plus ordered membership, permanent slots, provenance
and persistent counters. Predict results before invoking production. Test:

- new roots, children and attributes in no/default/prefixed/XML namespaces;
  default undeclaration; alias duplicate names; Unicode names and long URIs;
- exact-prefix conflict rejection, deterministic automatic choices, declarations
  on existing ancestors with descendants, and unchanged survivor resolution;
- insertion/replacement/removal order, generated declaration order, empty and
  escaped values, stale/foreign handles, source-independent documents and moves;
- every per-string/live/depth/persistent/work quota below/exact/above its boundary,
  reopen/no-refund behavior and complete failure snapshots;
- failure at every scratch/node/attribute/pool reservation, editor recovery and
  no publication before the last fallible step;
- deep/wide iterative build/traversal/removal/drop, attribute churn, many retired
  slots, and namespace-resolution size series that expose quadratic work;
- complete serialization/reparse parity with explicit Text-run projection and
  declaration/namespace/attribute order checks, using independent writer quotas.

Extend the pinned C differential helper with fresh constructor/property calls.
Compare matched supported cases and classify rootless documents, invalid names,
duplicate attributes, implicit namespace inheritance, reference-interpreting
content, prefix generation and unsafe ownership differences explicitly. An
unsupported or inconclusive oracle result is not agreement.

Benchmark setup, scope resolution, creation/upsert/removal, live traversal,
serialization and destruction separately. Match libxml2 semantics and validation
work, count namespace setup, alternate repeated runs and retain per-workload
results. Preserve parsed/value-only performance gates and measure linked edited
trees; do not hide a material row regression in an aggregate score. Report source
revision, compiler/profile, sizes, allocations/retention and raw artifacts. Fuzz
coverage uses its actual denominator and remaining gaps, without a 100% claim.

## Deferred full scope

Sibling insertion, rename, arbitrary namespace rebind/removal/repair, declaration
editing, root replacement, detach/reinsert, move/copy/import/adoption, document
fragments, entity-aware setters, DTD attribute typing/IDs/defaults, attribute
handles, slot reuse, compaction/reclamation, allocator parameters, formatting,
additional output encodings and language/ABI adapters remain required later
work. This slice must not advertise full `tree.h` or C ownership compatibility.
