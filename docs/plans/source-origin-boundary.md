# Source-origin API boundary

This stage removes unconditional original-input-range promises from parser
metadata and both DOM builders. It is not complete replacement-text provenance.
No DTD acceptance, entity expansion, resolver, decoding, physical progress,
resource ceilings, or lexical behavior changes are included.

## Contract

Introduce an opaque `SourceOrigin` with two private input offsets and an
`input_range() -> Option<Range<usize>>` accessor. Every currently accepted parsed
origin has an exact contiguous original-byte range. The optional return type
removes the public promise that all future events have such a range; no unused
replacement variant or source arena is introduced.

Reader and PushParser expose optional last-delivered origins, initially absent.
Their `event_range()` convenience method becomes optional. Successful EOF and
failures preserve the last delivered origin. Push stores it before invoking a
callback, including one that cancels. Push callbacks and BufferedReader tuples
carry SourceOrigin rather than Range. Physical position and buffered source
consumption continue to derive exclusively from the decoder/tokenizer.

Borrowed nodes carry an origin; owned parsed nodes carry Some(origin), while
constructed nodes carry None. Node ranges become/retain optional exact-input
ranges. Closing parsed elements uses a shared fallible origin join rather than
writing a range endpoint or expecting optional provenance to exist. Private
error and callback-cancellation endpoints are distinct operations: currently
respectively the input start and end.

This is a deliberate young-API break. Consumer migrations must preserve observed
content, physical offsets, event counts, observer hashes, and benchmark workloads.
Before-first-event empty sentinel ranges are intentionally replaced by None.

## Required follow-up before generated markup

A real replacement representation must exist and be tested before generated
events can be accepted. It must distinguish expansion occurrences (not just
entity declarations), processed replacement-text byte coordinates, immediate
reference locations, and an outermost physical document reference span. It must
keep exact lexical ranges, physical transport progress, diagnostic reference
starts, and cancellation reference ends separate. Replacement offsets must not
be passed through physical UTF-16 mapping or presented as EntityValue offsets.

Entity frames must enforce balanced element depth and lexical boundaries before
DOM delivery. Both DOM joins must reject incompatible occurrence/source origins.
Generated parsed nodes need real origins, not the constructed-node None marker.
A direct input parent may retain a contiguous span containing a reference while
its generated child has no direct input range. External identities and rich
error details require an explicit resource/ownership contract; this stage keeps
Error Copy and all currently accepted error offsets unchanged.

## Implementation and validation

1. Add the origin value, private physical construction and element joining.
2. Migrate Reader/Stream/Push/Buffered metadata and cancellation together.
3. Migrate borrowed and owned DOM storage, accessors and closure; preserve edits.
4. Migrate all existing examples, tests, fuzz projections and benchmark observers.
5. Add literal metadata/lifecycle tests and negative internal join tests, then
   focused tests, all targets, doctests, strict clippy and formatting.

Independent review/tests follow the initial compiling implementation. Record
actual before/after layouts. Full conformance, fuzz coverage, and timing require
separate coordinated frozen-source runs; do not claim them from ordinary tests.

Initial layout expectation: SourceOrigin remains two words, borrowed node storage
unchanged, optional owned provenance unchanged, and Reader/Push last-delivery
state grows one word. Actual layouts must be measured; no permanent ABI or
future replacement-layout promise is made.
