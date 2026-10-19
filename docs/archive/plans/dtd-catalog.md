# Private DTD declaration and normalization state

Base: `3e493e4`. This private, test-only slice consumes validated `DtdUnit`s.
It does not accept DOCTYPE in any public parser, emit DTD events, run content
entity frames, retain a second raw declaration catalog, or perform I/O.
The activation gates in dtd-foundations.md and dtd-representation.md remain.

## Normative decisions

XML 1.0 [section 4.1](https://www.w3.org/TR/xml/#sec-references) distinguishes
an earlier declaration for a direct default reference (WFC Entity Declared)
from the additional direct/indirect ordering wording in VC Entity Declared.
[Section 3.3.3](https://www.w3.org/TR/xml/#AVNormalize) independently requires
attribute normalization against declarations that have actually been read.
Normalize a default when its declaration is processed, including traversal of
indirect references. Never revisit it after a later declaration. A direct unknown
GE without a potentially unread external/PE dependency is an EntityDeclared
failure. An unknown GE reached indirectly through a replacement is a private
MissingReadDeclaration dependency when no source may be unread; otherwise its
reason is UnreadEntity. An unknown PE in permitted external EntityValue context
is also an explicit unavailable dependency. These outcomes are non-success for
a future strict controller, which cannot silently omit a read default or claim
it has a normalized value. This is distinct from lexical/WF failure and is not
a blanket rejection of the indirect-order validity constraint. External GE
references in attributes are forbidden even if a future resolver could retrieve
them. Unused GE replacement text is not swept for unresolved references,
malformed content or cycles; those checks apply at actual inclusion.

GE references inside EntityValue are bypassed and retained verbatim; they may
forward-bind at use. Character references are decoded during replacement
construction; their output is not rescanned in the same phase. Included PE text
is processed iteratively in EntityValue context, without the spaces required
around PE inclusion between declarations. [Sections 4.4–4.5](https://www.w3.org/TR/xml/#entproc)

Physical literal CRLF/CR is normalized exactly once before constructing an
entity replacement. Numeric CR is appended after that phase and preserved.
Replacement-source literals are never normalized again as physical input.
Attribute normalization maps literal XML whitespace to spaces, expands refs
with explicit frames, then collapses only U+0020 for non-CDATA declarations.
Direct numeric TAB/LF/CR remains that character. [Sections 2.11 and 3.3.3](https://www.w3.org/TR/xml/#sec-line-ends)

The internal-subset profile forbids PE references anywhere inside markup
declarations, including EntityValue. External declaration context is explicit;
it permits EntityValue PE inclusion without claiming external-subset scanning
support. An unread-PE barrier, including a PE not read within a permitted external
EntityValue, suppresses later ENTITY/ATTLIST effects unless
standalone=yes. Units are still grammar-validated before application, and the
internal PE placement check still runs. [Sections 2.8 and 5.1](https://www.w3.org/TR/xml/#sec-prolog-dtd)

Do not enforce ID uniqueness, required/fixed instance constraints, default
membership/type validity, undeclared notations, or other validity-only rules.
Recognize the five predefined entities independently of declarations; check
any supplied predefined declarations against section 4.6's required form.
This strict private diagnostic is a documented compatibility difference from
pinned libxml2 2.15.4, which warns for invalid amp/lt declarations while keeping
built-in uses fixed; it is not labeled a well-formedness or validity constraint.

## Concrete state and API

`DtdCatalog::new(CatalogLimits, standalone: bool)` owns:

- separate first-binding GE/PE HashMap<String, EntityId> indexes;
- one Vec of effective processed entity records with stable opaque Copy IDs;
- a first-binding element index and per-element ordered attribute Vec plus
  attribute-name index; attribute names have two budgeted copies to keep both
  insertion order and constant-time lookup simple without self-reference;
- counters and a reusable bounded Vec of value-expansion frames;
- external-subset and unread-PE dependency flags, plus sticky fatal failure.

ExternalIdentifier is a separate semantic view of unquoted identifier contents,
with physical EOL handling applied once. Public-ID whitespace normalization
before matching, base URI resolution, escaping and retrieval are deferred;
these IDs are never resolver-ready lexical quoted literals.

No raw DTD units are retained here. The eventual ordered DOM catalog owns all
lexical declarations, duplicates and comments/PIs separately. Entity records
retain processed replacement Strings or external IDs/unavailability. Attribute
records retain declared type class, required/implied/fixed/default form and
normalized value or explicit unavailability, never unresolved raw defaults.

Proposed methods:

- `apply(&DtdUnit, DeclarationContext) -> Result<DeclarationEffect, CatalogError>`;
- `lookup_general(name)` / `lookup_parameter(name)` return Internal with ID/text,
  ExternalUnavailable with ID/identifiers, Unparsed, UnreadEntity or Undeclared;
- `replacement(EntityId) -> Option<&str>` supports narrow frame lookups;
- `attributes(element)` iterates first bindings in declaration order;
- `normalize_attribute_value(element, name, literal_content, LiteralSource)`
  returns a normalized owned value or explicit unavailable dependency;
- `note_unread_parameter()` applies the barrier when a controller declines a PE;
- `usage()` exposes deterministic logical counters for quota verification.

`DeclarationContext.internal_subset` selects internal-subset lexical restrictions
(including declarations from internal PEs), not standalone external-markup
provenance. The latter accounting remains an activation prerequisite. Context
also distinguishes
`LiteralSource::{Physical, Replacement}`. The physical case takes decoded text
whose literal EOL bytes have not yet been normalized; the replacement case is
already constructed text. A header with an external ID records an unread
external-subset dependency; it never fetches it. Standalone=yes disables the
unread-source exemption for direct GE lookups and leaves later declaration
effects active. Explicit indirect unavailability remains non-success.

`DeclarationEffect` reports new effective bindings and unavailable defaults.
Duplicate declarations still receive grammar/context/normalization checking,
but never override effective bindings or retain duplicate semantic payloads.
Unavailable entity construction (e.g. an unread external PE in an external
EntityValue) retains an unavailable first binding, not a promise to rebind it.

Errors distinguish WF/lexical failures, cycles, quotas and allocation failure.
Unavailable dependencies are separate outcomes. Fatal failure is sticky: counters
and already-published first bindings are not rolled back, but all subsequent
mutating processing returns the same failure before changing state. Read-only
queries may inspect that prefix; they do not establish parse success. Locations
identify current-operation input offsets, (declaration ordinal, unit offset), or
(EntityId, replacement-local offset); they do not
claim physical document offsets or occurrence provenance. Each binding retains
its declaration ordinal for a future origin adapter. Real occurrence provenance
and physical error anchoring remain prerequisites for activation.

Add exact O(1) quoted-literal offset getters to EntityDeclarationView and
AttributeDefinitionView, plus DtdUnit::reference_positions(). Existing view
values stay unchanged. These are
validated unit-relative token positions, not pointer-derived guesses or a new
source framework. The stored DtdUnit representation stays unchanged.

## Iterative processing and limits

Use one concrete value processor with EntityValue and Attribute modes, a root
literal and frames keyed by EntityId. Frames hold cursors only; replacement
borrows are short-lived and no borrow survives catalog mutation. All frames
are popped before apply/normalization returns. A stack scan detects active-ID
cycles; charge its comparisons to work, so configured depth cannot hide work.
No recursive Rust calls, expanded document, or expanded intermediate entity
copies. Numeric output is appended directly and never reparsed in either mode.
Predefined decoding is exclusive to attribute processing: EntityValue retains
all GE spellings, including predefined names.

Independent limits cover processed declaration attempts, effective bindings,
retained string bytes, one constructed/normalized value's bytes, cumulative
emitted expansion bytes, cumulative scanned work, reference attempts, and active
entity depth. Empty references consume attempts/work/depth even with no output.
Work charges each applied unit's raw byte length, each entered value/frame's
complete text length, and the active-frame count before each cycle scan. Work
quota failure is anchored at the entry/reference causing that charge; it does
not claim a physical first-exhausted-byte offset. Reference syntax failures
identify the reference start; literal character failures identify that scalar.
The unit parser/scanner retain their independent raw-unit/token limits.

Count retained payload bytes (including both indexed/ordered attribute-name
copies); effective binding count also bounds fixed map/Vec metadata. Frame
metadata is bounded by depth. Reservation capacities may include allocator
rounding; these are logical allocation limits rather than exact RSS claims.
Every String/Vec/HashMap growth uses fallible reservation. Output String growth
is geometric and capped at the value ceiling, avoiding per-character exact
reallocations. Normalize an entire default into one buffer, including tokenized
space collapse, rather than repeatedly concatenate expanded entity Strings.

## Verification and staging

First commit this plan, then send a compiling narrow API checkpoint before
broad author tests. Keep only cfg(test) module staging on this private branch;
main and public parsing behavior stay unchanged. Independent spec-derived tests
will be provided separately at tests/internal/dtd_catalog_spec.rs.

Author tests cover literal expected values and exact errors for all effects,
first bindings, GE/PE separation, forward EntityValue vs immediate defaults,
unknown vs unread dependencies, standalone/barriers, internal PE restrictions,
external/unparsed references, predefined declarations, numeric markup vs direct
numeric text, AT&amp;T;, no same-phase rescan, CR phases, numeric attribute
whitespace, Unicode, cycles, empty amplification, and each resource ceiling.
Exercise multiple ATTLIST definitions and duplicates without quadratic copying.
Run focused tests, all-target/debug smoke, doctests, strict Clippy and fmt.
No public acceptance, fuzz-coverage, XMLTS or performance claims follow from
this private core; those require the integrated source-frame engine and freeze.
