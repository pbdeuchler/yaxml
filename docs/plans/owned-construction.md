# Bounded owned-tree construction API proposal

This is a design proposal against source revision `9e4b3a6`, not an implemented
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

## Public API proposal

Use borrowed construction specifications rather than reusing `Name`, `Element`
and `Attribute`. Reader/event `Name` carries both lexical QName and expanded
parts, so construction would have to check redundant fields on every call;
`Element` owns a `Vec`, and these types cannot express an automatic-prefix
request. Keep them as read-only parser/writer views.

```rust,ignore
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrefixPolicy<'a> {
    /// Preserve this exact choice. `None` means explicitly unprefixed.
    Exact(Option<&'a str>),
    /// Preserve a usable existing spelling or select a deterministic binding.
    Automatic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NameSpec<'a> {
    pub local: &'a str,
    /// `None` means no namespace; `Some("")` is invalid here.
    pub namespace: Option<&'a str>,
    pub prefix: PrefixPolicy<'a>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttributeSpec<'a> { pub name: NameSpec<'a>, pub value: &'a str }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NamespaceSpec<'a> {
    /// `None` is the default namespace; `Some("")` is invalid.
    pub prefix: Option<&'a str>,
    /// Empty URI is allowed only for a default namespace undeclaration.
    pub uri: &'a str,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElementItem<'a> {
    Namespace(NamespaceSpec<'a>),
    Attribute(AttributeSpec<'a>),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ElementSpec<'a> { pub name: NameSpec<'a>, pub items: &'a [ElementItem<'a>] }

impl OwnedDocument {
    pub fn create(root: ElementSpec<'_>, limits: EditLimits)
        -> Result<Self, EditError>;
}
impl Editor<'_> {
    pub fn append_element(&mut self, parent: OwnedNodeId, element: ElementSpec<'_>)
        -> Result<OwnedNodeId, EditError>;
    pub fn set_attribute(&mut self, element: OwnedNodeId,
                         name: NameSpec<'_>, value: &str) -> Result<(), EditError>;
    pub fn remove_attribute(&mut self, element: OwnedNodeId,
                            namespace: Option<&str>, local: &str)
        -> Result<(), EditError>;
    pub fn declare_namespace(&mut self, element: OwnedNodeId,
                             declaration: NamespaceSpec<'_>) -> Result<(), EditError>;
}
```

`PrefixPolicy::Exact(None)` preserves no prefix, `Exact(Some("p"))` preserves `p`,
and `Automatic` delegates spelling choice. Exact prefixes are never silently
substituted. Automatic attributes in an existing-element upsert first keep
their existing prefix if it still resolves to the requested URI, then use the
first matching nondefault binding in scope order (nearest element first,
declaration order within that element), then try `ns1`, `ns2`, and so on. An
automatic namespaced element uses a
matching default binding when available; otherwise it declares the requested
URI as the local default namespace on the fresh element unless an explicit
default declaration conflicts, then uses a matching nondefault binding or a
generated prefix. The XML namespace always uses the reserved `xml` prefix.
An automatic unnamespaced element stays unprefixed and adds `xmlns=""` when
needed; if an explicit local default declaration conflicts, report `Namespace`
instead of overriding it. Generated declarations follow caller items in order
of first use.
Generated prefixes obey `max_name_bytes` and `max_namespace_work`. This is a
deterministic naming policy, not a storage promise.

Before allocating `ns1`, `ns2`, and later names for an `ElementSpec`, reserve
every nonempty prefix requested with `Exact` by its element and attribute
names; generated names skip those reserved spellings so an earlier automatic
choice cannot invalidate a later exact request.

Gather all declarations in `ElementSpec` before resolving the element name or
ordinary attributes, matching the parser's start-tag behavior. Preserve the
order of supplied items in the attribute view; append generated declarations
after them. On a fresh element, an exact prefixed ordinary attribute may
declare or shadow an inherited binding when its requested URI is consistent
with the new element name and every other exact use of that prefix in the
specification. This is safe because the new element has no existing descendants
whose stored expanded names could be retargeted. Conflicting exact uses within
the same specification fail. Existing-element upserts retain the stricter
rule: they may add a declaration only for an unbound or already-matching
prefix, because shadowing could change existing descendants. Reject duplicate
declaration prefixes and duplicate ordinary attribute expanded names.
Namespace declarations are excluded from ordinary attribute upsert and removal.

These calls show creation, append, upsert, removal and a safe declaration:

```rust,ignore
use yaxml::{EditLimits, ElementItem, ElementSpec, NameSpec, NamespaceSpec,
            OwnedDocument, PrefixPolicy};

let mut doc = OwnedDocument::create(ElementSpec {
    name: NameSpec { local: "catalog", namespace: None,
                     prefix: PrefixPolicy::Exact(None) },
    items: &[],
}, EditLimits::default())?;
let catalog = doc.root_element().id();
let mut edit = doc.edit(EditLimits::default())?;
let child = edit.append_element(catalog, ElementSpec {
    name: NameSpec { local: "item", namespace: Some("urn:shop"),
                     prefix: PrefixPolicy::Exact(Some("s")) },
    items: &[ElementItem::Namespace(NamespaceSpec {
        prefix: Some("s"), uri: "urn:shop",
    })],
})?;
edit.set_attribute(child,
    NameSpec { local: "id", namespace: None, prefix: PrefixPolicy::Exact(None) },
    "A-17")?;
edit.set_attribute(child,
    NameSpec { local: "id", namespace: None, prefix: PrefixPolicy::Exact(None) },
    "A-18")?; // Upsert replaces this expanded name in place.
edit.set_attribute(child,
    NameSpec { local: "lang", namespace: Some("http://www.w3.org/XML/1998/namespace"),
              prefix: PrefixPolicy::Exact(Some("xml")) }, "en")?;
edit.remove_attribute(child, None, "id")?;
edit.declare_namespace(child, NamespaceSpec {
    prefix: Some("meta"), uri: "urn:metadata",
})?;
# Ok::<(), yaxml::EditError>(())
```

Specifications borrow caller strings and slices; no owned event vector or
general mutable-subtree framework is needed. Construction never parses a value
as markup: callers add literal text through `append_text`.

## Open decisions before freezing

The proposal intentionally leaves attribute storage to the matched
geometric-block experiment. Remaining public-contract choices still need review:

- Adding five fields to public `EditLimits` and a typed location to `EditError`
  changes source compatibility for struct literals and error destructuring.
  The recommendation is to take that pre-1.0 change now rather than split the
  construction operations across parallel options and error types. If keeping
  `EditError`'s current size is required, the tradeoff is to keep only error
  kinds for batch construction and retain `value_offset` on existing scalar
  value operations.
- Automatic prefix behavior is concrete here (`ns1`, `ns2`, ...; nearest
  matching binding first; default namespace for a fresh element when safe).
  Freeze it before implementation because it determines serialized bytes.
- Namespace work is now specified as a per-operation allowance: charge before
  each URI validation scan and declaration/prefix probe, including parsed
  ancestor bindings. The 64 MiB default and exact units still need adversarial
  depth/width tests before implementation. This work limit is independent of
  the persistent metadata bound, which remains open pending the geometric-block
  descriptor-cost results.

## Fit with the current owned API

At `9e4b3a6`, `OwnedDocument` stores document identity, node records, one
attribute arena, the UTF-8 string pool and persistent `edit_owned_bytes` and
`edit_created_nodes` counters ([`owned.rs`](../../src/owned.rs#L411)).
`EditLimits` currently bounds supplied value bytes, pool-capacity work and
created nodes ([`owned.rs`](../../src/owned.rs#L557)); the proposal adds only
construction-specific ceilings and counters to this bounded editor model.
`append_leaf` validates handle/kind, lengths, content, created-node quota, pool
capacity and reservations before publishing the new link
([`owned.rs`](../../src/owned.rs#L636)). Use the same staged publication shape
for element and attribute edits. Existing attribute views are exact-size and
include namespace declarations ([`owned.rs`](../../src/owned.rs#L1212)).

## Complete documents and handles

`create` returns a document with exactly one element. Do not expose an empty
`OwnedDocument` whose `root_element()` could fail, and do not create a temporary
fake root by parsing `<r/>`. Stage the synthetic root, real root, attributes,
declarations and pool directly, then assign the non-recycled document identity
after all other validation and storage work succeeds. Identity exhaustion is
reported as `EditErrorKind::IdentifierExhausted`.

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
| Namespaced attribute with exact prefix | On a fresh element, preserve the prefix and allow a consistent local declaration/shadow of inherited bindings; on an existing element, accept the same in-scope binding or declare an unbound prefix, and reject a conflicting binding |
| Namespaced attribute with automatic prefix | Reuse a deterministic matching nondefault binding, otherwise generate a fresh prefix |
| Namespaced attribute with explicitly unprefixed spelling | Reject; do not silently add a prefix |

These rules reflect the existing parser/writer contract. The parser first reads
all declarations from a start tag, applies their scope, resolves each QName,
then rejects duplicate attributes by `(local, namespace identity)` in
[`parser.rs`](../../src/parser.rs#L346) and
[`parser.rs`](../../src/parser.rs#L412). Name and reserved-binding validation
are in [`parser.rs`](../../src/parser.rs#L792) and
[`parser.rs`](../../src/parser.rs#L368). The writer compares `qualified`,
`prefix`, `local` and `namespace` against reparsed output in
[`writer.rs`](../../src/writer.rs#L339), so constructed records must already
have mutually consistent lexical and expanded names. Construction should share
the parser's XML Name and reserved-binding rules; it must not serialize-and-
reparse as a substitute for resolving a namespace.

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
explicit declaration inconsistent with an exact name is an error, not a request
for automatic repair. Ordinary attribute construction rejects the XMLNS URI
and an unprefixed `xmlns` attribute; declarations use `NamespaceSpec`. The
unprefixed `xmlns` spelling remains permitted as an element local name.

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

Attribute upsert on an existing element may add a matching declaration only
when that prefix is currently unbound or already resolves to the same URI;
shadowing a conflicting binding on an existing element could change
descendants' lexical meaning. Fresh `ElementSpec` construction has no existing
descendants, so exact prefixed attribute bindings may safely shadow inherited
bindings if the element name and all exact uses of that prefix in the new
specification agree on one URI. Conflicting exact uses are a `Namespace` error;
automatic names may use or generate another prefix. For an ordinary
nonreserved URI, automatic choice can select an unused prefix
within its work allowance. The XML URI uses its reserved `xml` prefix. An exact
conflict with a local explicit declaration or another exact use fails; for an
existing-element upsert, a conflict with the current binding also fails.
Thus an arbitrary supported
namespace URI remains usable without changing the requested spelling silently.
Namespace replacement/removal is deferred. Removing an ordinary attribute leaves
its declarations in place; do not guess that a binding is unused.

Keep declarations in the ordered attribute view in the XMLNS namespace, as
today. Existing declaration order survives ordinary-attribute edits. New
declarations count toward both live start-tag items and cumulative created
attribute entries. General moves, namespace adoption and subtree repair remain
out of scope.

## Attribute membership and storage

The current [builder](../../src/owned.rs) and
[serializer](../../src/serialize.rs) expose an ordered, exact-size borrowed
attribute view over contiguous ranges. Keep that view and read path unchanged.
Storage for insertion/removal remains open; the geometric-block experiment
owns that decision. This API proposal requires ordered membership, replacement
at the existing position, exact iteration length and atomic updates. It does not
prescribe per-element vectors, links, block descriptors, record size or public
layout.

Ordinary upsert matches URI/local identity. New attributes append; replacements
keep their position and honor an explicit requested spelling through the safe
binding rules. Automatic spelling should preserve a usable existing spelling
before choosing another. Duplicate creation in one element specification fails;
upsert of an already-present expanded name replaces instead. Removal selects by
expanded name, returns a missing-attribute error when absent, and removes that
ordered item. XMLNS items cannot be edited through ordinary-attribute APIs.

## Resource policy and transaction boundary

Extend the existing `EditLimits`; do not add another limits type. Add five
inclusive construction fields. Existing `max_value_bytes`, `max_owned_bytes`,
and `max_created_nodes` keep their current meanings:

| Resource | Proposed accounting |
| --- | --- |
| `max_depth` (256) | Prospective depth of a new element; root is one. Check parent ancestry iteratively. No whole-document rescan on editor open. |
| `max_attributes` (1,024) | Prospective live start-tag items, including declarations. Membership growth must fit; removal and value-only edits remain available on a parsed baseline above a lowered ceiling. |
| Created nodes | Existing persistent counter, including new elements; parsed nodes and synthetic roots excluded; no removal refund. |
| `max_created_attributes` (1,000,000) | Persistent successful fresh attribute/declaration entries; parsed baseline excluded; no removal refund. In-place replacement does not create an entry. |
| `max_name_bytes` (1 MiB) | Per local name, prefix, namespace URI, declaration prefix or declaration URI, checked before scanning. |
| `max_value_bytes` (1 MiB) | Per attribute value; keep existing value/PI-target behavior. |
| Retained string work | Existing cumulative full requested pool capacities, including charged failures; initial construction has no free parsed baseline. Reuse known immutable URI pool ranges when safe, without content-dedup scans. |
| `max_namespace_work` (64 MiB) | Per-operation work units, reset for each public call. Charge one per declaration/prefix candidate inspected; charge prefix and URI validation scans by full length and each prefix/URI equality or hash operation by the full length of every operand before scanning it. Include parsed ancestor declaration prefixes; charging only new declarations is insufficient. |

`EditLimits::default()` keeps the current 1 MiB value, 64 MiB pool-work and
1,000,000 created-node ceilings, and adds 1 MiB name components, depth 256,
1,024 live start-tag items, 1,000,000 created attribute entries and 64 MiB
namespace work. Add `EditLimit::NameBytes`, `Depth`, `Attributes`,
`CreatedAttributes`, and `NamespaceWork` for these ceilings. Opening an editor
below a spent persistent counter fails in this order: created nodes, owned
bytes, created attributes. The first two preserve current `edit()` precedence.
Namespace work is per-call and is not an editor-open failure. Raising a ceiling
is allowed; persistent counters do not reset on reopen. Depth and live
attribute ceilings are checked for
prospective insertions, so a parsed baseline above a lowered ceiling remains
value-editable or reducible. Attribute/node metadata is outside the string
ceiling. Its precise bound depends on the geometric-block representation and
must include both membership entries and block/descriptor overhead;
`max_created_attributes` alone does not prove a finite metadata bound if one
successful insertion can allocate many descriptors. Keep that bound open until
the storage experiment provides concrete accounting. Reservations remain
fallible.
Temporary planning is bounded by `max_attributes` and uses fallible reservation.
Local-name XML validation is bounded only by the per-component
`max_name_bytes` ceiling, not by `max_namespace_work`. Since up to 1,024 items
could each carry a near-limit local name or value, the implementation still
needs an aggregate supplied-byte/work bound before this proposal can pass its
resource gate; the per-component limits alone do not bound aggregate scanning.

Build scope once per operation rather than scanning every ancestor for each
attribute. Assign temporary URI identities once per distinct resolved binding
and check duplicate attributes using those identities. Account for work before
validating or comparing bytes: do not validate a shared 1 MiB URI independently
for 1,024 items and only then discover the 64 MiB work ceiling. Prefix and URI
validation scans, plus their equality/hash comparisons, count against this
allowance; charge the full length of each operand before scanning it. This
includes prefix comparisons against declarations inherited from parsed
ancestors. The first operation that would exceed the allowance returns
`NamespaceWork` before beginning that scan. A bounded scratch table may
deduplicate identical borrowed URI identities, but its lookup and comparison
work must itself be charged and bounded; do not use content hashing that scans
an uncharged URI. Store pool ranges/indices in staged metadata; do not retain
pool references across a reservation that can move it.

The transaction is validate, plan, reserve, store, then publish. Freeze this
error precedence:

1. Resolve handle ownership/liveness, then node kind and placement. Creation has
   no handle checks.
2. Check prospective depth and any supplied item count already known from an
   `ElementSpec`. The final live-item count, including generated declarations
   and any upsert insertion, is checked after prefix planning/key lookup.
3. Check supplied string byte lengths in argument order: element local name,
   namespace URI, prefix; then each item in order (declaration prefix then URI;
   attribute local name, namespace URI, prefix, then value). Name components use
   `max_name_bytes`, values use `max_value_bytes`.
4. Validate XML characters and XML Names in the same order. Before scanning a
   namespace URI for XML characters, charge its full byte length to this
   operation's namespace-work allowance; if that would exceed the ceiling,
   return `NamespaceWork` before scanning the URI. Return the first
   component-local UTF-8 byte offset for validation failures that fit the
   allowance.
5. Charge namespace work before each declaration/prefix candidate probe and
   before each URI equality check (charge full operand lengths as the worst-case
   comparison cost). Then validate explicit declaration reserved rules in item
   order and resolve the element name using all declarations. Walk items in order; resolve each
   ordinary attribute and immediately check its expanded-name duplicate.
   Declaration items have their XMLNS expanded name checked at their position
   in that same pass, so duplicate declaration prefixes return
   `DuplicateAttribute`. If a prospective probe charge exceeds the
   per-operation ceiling, return `NamespaceWork` before beginning that probe.
   Check the final live-item count each time prefix planning adds a declaration
   or an upsert resolves to a new attribute; this returns `Attributes` before later
   item validation if the newly planned start tag is already too large.
6. Check applicable persistent created-node and created-attribute-entry limits,
   then the complete cumulative pool-capacity request (`OwnedBytes`). For
   `edit()`, below-spent ceilings are checked created nodes, owned bytes and
   created attribute entries, preserving the current first two checks.
7. Reserve all scratch, node, attribute-membership and pool capacity; store all
   strings/records; publish one coherent update and successful-entry counters.

For `set_attribute`, resolve the expanded key after name validation. Existing
matches replace in place and do not consume a fresh-entry count; new attributes
append. For removal, validate the selector after handle/kind and return
`MissingAttribute` if the exact `(namespace, local)` pair is absent. The
`Some("")` selector remains distinct from `None`, as in the current value
setter. Treat selector strings as exact fields, not a QName: don't parse them.
Bound local name and URI by `max_name_bytes` before scanning. For an XMLNS
selector, preserve `set_attribute_value` precedence: return `MissingAttribute`
if no declaration matches, otherwise `NamespaceDeclaration`.
`declare_namespace` checks an existing local declaration before checking
safe-rebind conflicts. Validation/quota failures leave logical values and
successful-entry counters unchanged. Namespace work counts against this call's
allowance and is not a persistent counter; attempted pool-capacity reservations
retain their existing cumulative charge. Allocation failure may retain capacity
but never publishes a partial node, attribute, declaration, link or handle.
Planning scratch is bounded and fallibly reserved. Persistent arena/pool growth
and publication wait until the plan succeeds.
Pool quota preflight precedes persistent metadata reservation, as in leaf
insertion. Combined operations reserve their full string payload once; tests
freeze requested capacity charges, not allocator-specific observed capacities.

### Indexed errors

Extend `EditErrorKind` with `Namespace`, `DuplicateAttribute`, and
`IdentifierExhausted`; keep common categories such as `InvalidName`,
`InvalidCharacter`, `InvalidValue`, `LimitExceeded`, and `Allocation`. Add an
optional typed location alongside `EditError.value_offset`:

```rust,ignore
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditComponent { LocalName, NamespaceUri, Prefix, Value }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditLocation {
    ElementName { component: EditComponent, byte_offset: usize },
    Name { component: EditComponent, byte_offset: usize },
    Declaration { index: usize, component: EditComponent, byte_offset: usize },
    Attribute { index: usize, component: EditComponent, byte_offset: usize },
}
```

`Name` identifies a direct `set_attribute` name argument; `Attribute` identifies
an `ElementItem::Attribute`. Direct value failures retain the existing
`value_offset` contract, while `Attribute { component: Value, .. }` locates a
value inside an element specification. Offsets are UTF-8 byte indices in the
named component, never source coordinates. Keep `value_offset` unchanged for
existing scalar edits. No per-node state or success-path work is added. Avoid
string-encoded paths. Handle, placement, missing-item, count, quota,
identity-exhaustion and allocation failures have no location.

## Pinned references and compatibility differences

The inspected reference is local libxml2 2.15.4 `tree.c`; links pin that tag.
Its behavior motivates the compatibility boundary below, not a requirement to
copy unsafe ownership or implicit namespace inheritance:

| Symbol/location | Observed behavior and design implication |
| --- | --- |
| [`xmlNewDoc`, 880](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L880) | Creates a rootless document and defaults version to 1.0. Yaxml retains a rooted complete-document invariant. |
| [`xmlNewNs`, 517](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L517) | Appends declarations in order; duplicate local prefixes fail; URI syntax is not checked. Preserve order without importing raw namespace ownership. |
| [`xmlNewDocNode`, 1899](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L1899) | Its content path interprets references and can silently ignore malformed content. Keep yaxml's literal values and explicit errors. |
| [`xmlNewDocRawNode`, 1976](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L1976), [`xmlNewTextChild`, 2082](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L2082) | Provide raw-text construction, closer to separate element/append_text operations. |
| [`xmlNewChild`, 2673](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L2673) | NULL namespace inherits the parent's namespace pointer. Yaxml resolves explicit expanded identity and lexical binding, without this implicit inheritance. |
| [`xmlNewProp`, 1545](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L1545), [`xmlNewNsProp`, 1571](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L1571) | Append literal attributes without duplicate checking. Yaxml preserves unique expanded attributes. |
| [`xmlSetProp`, 6516](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L6516), [`xmlSetNsProp`, 6556](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L6556) | QName lookup and namespace-aware upsert are distinct; replacement finds URI/local identity and assigns the supplied namespace association at line 6587. Use expanded identity and an explicit spelling policy. |
| [`xmlGetPropNodeInternal`, 6122](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L6122), [`xmlUnsetNsProp`, 6493](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L6493) | Namespace-aware lookup/removal uses local name plus URI; `xmlUnsetProp` is only the no-namespace case. Mirror this selector shape and return a typed missing-item error. |
| [`xmlSearchNsSafe`, 5654](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L5654), [`xmlNewReconciledNs`, 5914](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L5914) | Scope lookup walks ancestors; reconciliation may reuse a URI or generate a new prefix, truncating a long requested prefix and limiting attempts. Yaxml's explicit prefix policy avoids truncation and bounds its own deterministic generation. |
| [`xmlUnsetProp`, 6473](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L6473), [`xmlUnsetNsProp`, 6493](https://github.com/GNOME/libxml2/blob/v2.15.4/tree.c#L6493) | Select, unlink and free attributes. Yaxml exposes checked document state rather than raw pointer invalidation. |
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

1. The matched leaf-structure benchmark gate is complete. Review this proposal
   with representative call sites, settle any remaining API choices, then
   freeze an implementation plan.
2. Finish the active geometric-block evaluation of attribute membership while
   preserving existing values, source/order, views, serialization and failure
   behavior. Keep the construction API independent of that storage choice.
3. Extract shared QName/reserved-binding validation. Implement atomic rooted
   creation and fresh empty elements, with explicit/generated declarations and
   stable IDs. Keep borrowing DOM storage unchanged.
4. Add ordinary-attribute upsert/removal and safe append-only declarations on
   existing elements. Extend quotas, indexed diagnostics and reservation faults.
5. Add independent public/model/fuzz/oracle tests, matched workloads, coverage
   artifacts and a clean holistic review before integration.

The independent model must store expanded identity separately from prefix
spelling and declarations, plus ordered membership, provenance and persistent
counters. Predict results before invoking production. Test:

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
- deep/wide iterative build/traversal/removal/drop, attribute churn and
  namespace-resolution size series that expose quadratic work;
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
results. Preserve parsed/value-only performance gates and measure the selected
edited-tree representation; do not hide a material row regression in an
aggregate score. Report source revision, compiler/profile, sizes,
allocations/retention and raw artifacts. Fuzz
coverage uses its actual denominator and remaining gaps, without a 100% claim.

## Deferred full scope

Sibling insertion, rename, arbitrary namespace rebind/removal/repair, declaration
editing, root replacement, detach/reinsert, move/copy/import/adoption, document
fragments, entity-aware setters, DTD attribute typing/IDs/defaults, attribute
handles, node-ID reuse, compaction/reclamation, allocator parameters, formatting,
additional output encodings and language/ABI adapters remain required later
work. This slice must not advertise full `tree.h` or C ownership compatibility.
