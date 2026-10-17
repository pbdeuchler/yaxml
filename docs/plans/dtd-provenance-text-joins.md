# Replacement provenance and parse-only Text joins

Private implementation checkpoint based on `dbb5a3879a857a47ae844fe65dd5e669bfa514a1`.
This stage does not accept DOCTYPE, expand entities in a public parser, add DTD
events, or change construction/editing topology.

## Provenance representation

Replace the direct-input-only `SourceOrigin` with private Input, Replacement and
Mixed variants. Input stores one exact physical range. Replacement stores the
declaration identity, a fresh invocation identity, an exact replacement-local
range and the outer physical reference range. Mixed stores only an explicitly
inexact physical causal cover. No parser history arena or duplicate persistent
metadata is introduced.

Keep the existing public surface minimal: `input_range()` continues to return an
exact original-input range only for Input. Declaration/invocation numbers and
replacement-local ranges remain crate-private until public DTD activation has a
reviewed use for them. Error and cancellation offsets remain physical: the start
and end of the input range, outer reference, or Mixed causal cover.

Move the frame prototype's private byte-span and entity identity types behind
the origin module and use `SourceOrigin` directly from `EntityFrames`. This keeps
one representation and one identity domain. The frame module may re-export the
private types for its existing tests and catalog adapter; no public numeric ID is
added.

Element closure and Text joining are separate operations:

- element closure permits an ordered gap between start and end spans, but only
  within direct input or the same exact replacement declaration/invocation and
  physical reference;
- Text joining extends an exact range only when the spans are adjacent in direct
  input or the same exact replacement occurrence; every gap, source change,
  invocation change or prior Mixed value produces Mixed with a physical cover.

Both operations reject malformed or cross-occurrence element closure before DOM
metadata mutation. Text joining is total for valid origins and never treats
coarse equality as proof of source identity.

## Parse-only DOM Text coalescing

Reader, PushParser and BufferedReader continue to emit their existing bounded
Text events. Both parse builders coalesce an incoming Text event with the last
direct child when that child is Text. CDATA, comments, processing instructions
and elements remain boundaries. Empty Text fragments do not create an extra
node; their origin still participates in the join if supplied by a future
controller.

The borrowing DOM keeps its first borrowed Text without allocation. On the first
merge it creates one fallibly reserved String, charging the shared parser Budget
before allocation. Later growth is geometric and every full new capacity request
is charged before reservation. A failed charge or reservation leaves logical
text, origin, links and node count unchanged; consumed budget remains cumulative
under the existing policy.

The owned DOM stores the first Text in its pool. An adjacent parsed Text is always
the pool tail because no intervening event has stored data. Extend that range in
place with the pool's checked geometric reservation and shared Budget. Treat a
non-tail parsed Text merge as an internal-state error rather than introducing a
quadratic copy fallback for an unreachable state. Publish the longer range and
joined origin only after the append succeeds.

Construction and Editor append methods retain their existing behavior and may
create deliberately adjacent Text nodes. Only the parse builders coalesce.

## Resource and failure rules

- Checked addition precedes every length/capacity calculation.
- `Limit::OwnedBytes` is reported at the incoming fragment's physical diagnostic
  context.
- Capacity is charged before `try_reserve_exact`; allocation failure is typed and
  leaves public tree state unchanged.
- A merge adds no node, attribute, link or subtree count.
- Parsed Mixed provenance remains `Some(SourceOrigin)` in OwnedDocument;
  constructed nodes continue to use `None`.
- No replacement-local offset is passed through physical UTF-16 mapping.

## Verification boundary

Author tests cover exact Input/Replacement/Mixed joins, repeated invocation IDs,
physical error/cancellation endpoints, gap and overlap behavior, exact/below
owned-byte ceilings, borrowed-to-owned promotion, geometric growth, owned pool
tail extension, node/link/topology preservation, and CDATA/comment/PI/element
boundaries. Existing direct-input parsing must remain byte-for-byte equivalent.

Record `size_of`/`align_of` for SourceOrigin, Event, borrowing Record, OwnedRecord,
Reader and PushParser at the compiling checkpoint. These are evidence, not ABI
promises. Run focused library/DOM tests, strict Clippy and formatting; full fuzz,
coverage, conformance and performance remain later frozen-source gates.
