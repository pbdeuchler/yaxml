# Private bounded internal-DTD controller

Base: `ff979893520582bba0c481fe6f92729da8c0f50b`. This combines the reviewed
scanner, lexical unit, semantic catalog, iterative frames and fallible unit
ownership handoff. It remains private and test-only. It does not enable DTDs in
Reader/push/buffered/DOM, parse GE content, apply defaults to instance tags,
change public provenance, or implement the eventual public writer protocol.
The full activation gates remain unchanged; an empty DOCTYPE is not success
for this implementation slice.

## Operation and API

The controller starts at a known physical DOCTYPE opening, after transport
encoding selection. It accepts decoded UTF-8 bytes incrementally, including
splits inside UTF-8 scalars, together with the already-selected SourceEncoding
and the real physical start offset. The existing Decoder remains the transport
owner; the controller never decodes another document or infers transport input
from replacement output.

Proposed interface:

- `DtdController::new(encoding, physical_start, limits, standalone)`;
- `advance(decoded_bytes, &mut Budget) -> Result<usize, ControllerError>` stops
  at one logical event or when physical input is needed; generated events may
  consume zero physical bytes;
- `current() -> Option<(DtdEvent<'_>, FragmentOrigin)>` lends the current event;
- `release()` drains that event; advancing while it remains ready consumes zero;
- `finish_step(&mut Budget)` seals physical input and drains generated/held events,
  then reports incomplete EOF rather than accepting a partial DOCTYPE;
- `is_complete()` becomes true after logical End is released;
- `physical_position()`, `doctype_range()` and `counters()` report physical
  progress/coverage and deterministic controller accounting;
- `catalog()` lends effective bindings for the later instance-tag controller.

DtdEvent is Begin(DoctypeHeaderView), Item(&DtdUnit), or End. The event borrows
one controller-owned validated unit, never a catalog replacement borrow.
Begin exposes a typed header view rather than pretending the full lexical
header's raw string has the Begin origin. For a no-subset header, raw grammar
includes the final `>`; Begin's origin excludes that real delimiter. Releasing
Begin queues End with precisely that delimiter's physical range. End requires
no additional input. For an internal subset, End covers the actual `] S? >`.
Only End completes the logical DTD. Body bytes are never consumed by this core.

Budget is borrowed only for one operation. The controller additionally owns a
cumulative owned-copy ceiling, so substituting a fresh external budget cannot
bypass its DTD copy quota. The same parser Budget can later account for DTD,
namespace and generated-event retention without a second ParserState or an
unused resolver/allocator abstraction.

## Source scheduling and bounded storage

Keep one physical Doctype scanner and one SubsetFragment scanner. The physical
scanner retains its subset state while replacement input is active; its input
cursor is paused. A single fragment scanner is restarted only between complete
units when entering or returning from an internal PE. Its reusable scratch is
not one buffer per active frame. Record the current frame's scanner cursor base;
EntityFrames retains actual source cursors and invocation identity. A small
active-only Vec of catalog EntityIds permits O(1) replacement lookup without
forging IDs, retaining borrows, or storing historical occurrences.

A PE replacement is already a complete immutable String in the catalog. Scan
it from the current frame cursor plus the pending unit length. Keep the frame
cursor at the unit start until its kind is known: ordinary completed units
commit through advance_current; a completed nested PEReference commits through
enter_nested, preserving that method's atomic parent-cursor checks. Reset the
fragment scanner at each complete unit/frame transition, never reparse a prefix
per chunk. Frame EOF must be between complete units. A declaration, comment or
PI cannot start in one PE and finish in its caller.

Discard subset whitespace and physical PEReference units from the logical
output. Included declarations/comments/PIs are emitted at their expansion
position; a stream never emits both `%p;` and its expanded declarations.
Between-declaration PE padding spaces have no observable item in this mode;
structural separation is enforced by complete-unit frame boundaries, without
inventing source offsets for padding. This is normalized DTD serialization
following PE expansion, not C14N or reference-preserving serialization.

Validate every framed family before effects: only a Header at the physical
start can become Begin, and a framed Declaration whose DtdUnit view is Header
is rejected. Whitespace/PEReference/End are scanner protocol units, not passed
to DtdUnit or the semantic catalog. All other units undergo full lexical
construction before catalog application or publication. Internal PE-generated
units retain internal-subset lexical permissions and LiteralSource::Replacement;
being generated never grants external-PE permission.

Borrow the scanner's bounded scratch for DtdUnit construction. Retain the unit
with try_into_owned(shared Budget, physical error anchor), moving token metadata
and copying only borrowed raw text once. Then release scanner scratch and apply
the owned unit to the mutable catalog. No catalog replacement borrow survives
that mutation. Current event retention is one bounded unit, not the subset.

## Provenance and failure

Use private FragmentOrigin directly. Physical units have exact physical spans.
PE units carry stable declaration ID, fresh invocation ID, exact replacement
local range, and the real outer physical reference range. Nested PE invocations
inherit that physical reference context; the controller does not allocate a
history arena or invent contiguous original-input ranges.

Map physical unit errors with SourceEncoding and TokenSpan, including UTF-16
supplementary scalars and BOM/prolog offsets supplied by the caller. A narrow
scanner pending-raw/range view permits mapping incomplete and malformed unit
errors without rescanning accumulated prefixes during normal input. Errors in
replacement text retain local offset/declaration/invocation and a separate
physical reference anchor. Catalog declaration/replacement diagnostics retain
their own ValueLocation, rather than relabeling them as physical offsets.
Physical cursor and DOCTYPE source span count physical bytes only, including
physical PE-reference spellings, never generated declarations.

Unknown/external/unavailable PEs are explicit controller non-success. Mark the
catalog's unread-PE barrier before returning that outcome; no resolver or
ambient I/O is attempted. Catalog unavailable defaults are also controller
non-success at declaration processing, never a blank attribute or deferred
retroactive repair. Fatal failures are sticky. Read-only queries may inspect a
valid prefix; further processing cannot resume, and no transaction framework
or whole-catalog rollback is introduced.

## Resource accounting

Controller limits include the component scanner/unit/catalog/frame ceilings,
a physical-DTD byte ceiling, a cumulative owned-copy ceiling, and a global work
ceiling. The single catalog's retained-string budget is global for this DTD;
binding counts bound its map/Vec metadata. Two bounded scratch buffers, one
bounded retained unit/token vector, the value-normalization buffer and the
active frame/ID stacks have separate finite ceilings. These logical limits do
not claim exact allocator RSS accounting.

Global work counts scanner-consumed bytes, one raw-unit grammar pass, owned raw
copy bytes, catalog work, and frame work. Narrow component cap_work(absolute
ceiling) methods only decrease their existing work ceilings before each
operation, so component work cannot overrun the global allowance before an
allocation. Scanner work is capped similarly before feeding an allowed slice.
Synchronize committed component work deltas even when an operation fails.
Counter arithmetic is checked. Frame reference attempts include failed/empty
lookups; depth is checked before frame growth. Partial replacement bytes remain
bounded by scanner/raw/global-work limits before a unit can be committed to
frame byte accounting; no expanded whole input is retained.

Owned-copy quota is checked before unit promotion. Grammar token/model/attribute
ceilings remain checked by DtdUnit before metadata growth. Scanner byte and
scratch ceilings are checked before raw append/reservation. Reference/frame
limits are checked by EntityFrames before publishing a child or parent cursor.
No prefix is rescanned per byte/chunk. Actual allocation exhaustion remains
unforced unless a concrete safe test mechanism becomes available.

## Source changes and verification

Add src/dtd_controller.rs under cfg(test). Small reviewed component additions:
scanner pending raw/range access, checked restart_fragment and work cap;
catalog/frame work caps. No public API or transport decoder change is needed.
Send the first compiling narrow checkpoint before broad author test additions.
Coordinate the independent test path with the spec author.

Literal tests must cover complete header/all declaration families and ordered
Begin/Item/End; no-subset real delimiter retention; generated internal declarations
that mutate their own source catalog; nested/repeated/empty PEs with distinct
invocations; physical cursor pause; same name GE/PE distinction; PE context,
cycle/depth/work/reference quotas; unknown/external/unavailable non-success;
missing/partial lexical EOF, nested DOCTYPE, conditional-section rejection;
UTF-8 every split and one-byte feeds, UTF-16 mapping via existing Decoder,
Supplementary scalars, BOM/nonzero offsets and untouched body suffixes.
Verify exact and just-below resource ceilings and sticky errors, with literal
origins and no synthetic public unchecked Name construction.

After the author/independent suites: ordinary all-target/debug smoke, doctests,
strict Clippy/fmt, source-blob verification and deliberate faults. No heavy fuzz,
XMLTS or comparative timing before root's integrated-source freeze/CPU lease.
Public activation still requires all five frontends, GE content parsing,
instance defaults/normalization, public provenance/DTD events, both DOMs and
writer/serialization, full fuzz/conformance/layout/performance gates and review.
