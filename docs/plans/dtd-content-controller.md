# Private general-entity content controller

The concrete Text/reference boundary follow-up is specified in
[dtd-text-reference-adapter.md](dtd-text-reference-adapter.md). The lexical
foundation alone does not yet implement that composition.

Plan-only base: `f43f90b`. No behavior code or public DTD acceptance is authorized
by this note. Consume the borrowed-DTD adapter's reviewed `DtdBodyState` handoff;
do not implement a parallel declaration engine or general parser framework.

## Existing constraints

`Reader::read_event` returns `Event<'input>`, not a loan tied to `&mut Reader`
(`src/reader.rs:175`). Both DOMs rely on that useful contract. `Stream::parse`
currently lends from one tokenizer and copies only persistent parser structure
(`src/stream.rs:114`). `ParserState::process_token` returns names/leaf strings
borrowing raw token text; its retention callback concerns stack/namespace data,
not event ownership (`src/parser.rs:207`). Its normalizer always applies physical
EOL processing (`src/parser.rs:273`). A generated token therefore cannot simply
pass through the Reader path or the physical normalizer.

Tokenizer Text currently ends only at `<`; it consumes that delimiter as saved
lookahead (`src/tokenizer.rs:90`). Recognizing GEs only after reading a whole Text
token would already consume physical `post` in `pre&e;post`. The new controller
must stop physical input at the reference before scheduling replacement input.
`EntityFrames::check_before_end_element` must run before `end_element` pops a
frame/unwinds namespaces (`src/dtd_frames.rs:327`, `src/parser.rs:585`). Checking
only final depth allows a replacement to close/reopen its caller incorrectly.

## Concrete ownership and entry points

Keep ordinary DTD-free Reader/Stream paths. Add one lazily allocated, fallibly
reserved slot for the optional DTD session: a `Vec<DtdSession<'input>>` whose
length is zero or one. This uses stable `try_reserve_exact(1)` rather than an
unverified fallible Box API. No catalog, frame stack, replacement tokenizer or
reference scratch is stored inline in every ordinary ParserState. The session
container itself is deferred until frontend activation; the first private
foundation tests concrete scanner/controller values directly. This means
zero/one *session-container* allocation, not a claim that DTD maps/scratch need
only one allocation. Record actual layouts before activation.

```text
DtdSession<'input> = Declarations(DtdController<'input>)
                   | Content(ContentController<'input>)

ContentController<'input> {
    body: DtdBodyState,             // SAME catalog, frames, nonce, counters/limits
    physical_source: sealed stable UTF8 window | streaming,
    physical: Tokenizer,
    replacement: Tokenizer,
    reference: ReferenceScratch,    // at most one bounded pending reference
    active: Vec<EntityId>,         // active GEs only, no occurrence history
    pending/ready/error state,
}
```

The adapter owns `take_current` and `into_body_state`. Only successful taken End
permits handoff. Drop DTD unit/scanner buffers; move the existing catalog,
EntityFrames, limits and counters without recreating them. Reader supplies its
same immutable UTF-8 input window to the sealed body constructor; Stream uses
streaming mode. An owned-mode `advance(bytes)` cannot be used to inject bytes
into the stable-window Reader mode. Reader's `advance_borrowed` reads its own
window. This follows the adapter proposal rather than adding a retention trait.

The handoff/resume also needs an explicit cursor pair `(decoded, physical)`.
DTD physical scanner position supplies decoded bytes consumed since DOCTYPE
start; add the saved decoded start with checked arithmetic before dropping it.
A physical byte offset is not a UTF-16 decoded offset. Existing Decoder remains
in Stream, preserving transport carry and BOM/declaration state.

Proposed narrow content methods (private, final spelling subject to API review):

- `borrowing_utf8(body, stable_window, cursor)` / `streaming(body, cursor, encoding)`;
- `advance_borrowed(&mut Budget)` / `advance(decoded_bytes, &mut Budget) -> Advance`;
- `finish_step(&mut Budget)`, `has_token`, `is_non_event`, `release`;
- `read_event(&mut ParserState<'input>) -> Option<(Event<'input>, SourceOrigin)>`
  for Reader, with physical stable borrowing and generated owned fields;
- `event(&mut ParserState<'static>) -> (Event<'_>, SourceOrigin)` and `emit`
  for Stream's existing lending/callback policies;
- `physical_position`, with no source-progress inference from event origins.

`Advance` is a concrete result `{ consumed: usize, outcome: DriveOutcome }`, with
`DriveOutcome = EventReady | NeedPhysicalInput | Progress`. `consumed` is only
new physical decoded bytes accepted by this call, including bounded lookahead;
it is zero for generated-only steps. EventReady means one complete source token
can be parsed/delivered. NeedPhysicalInput means no queued reference, frame
return, or replacement byte can make further progress without new primary input.
Progress means one internal transition occurred without a token: for example,
entering an empty GE or popping a finished frame. A zero-byte Progress must
spend a reference/work attempt or decrease pending/frame state; unchanged state
must return NeedPhysicalInput, never Progress. The coordinator loops Progress,
handles EventReady then release, and requests bytes only on NeedPhysicalInput.
A GE at the end of a physical chunk is drained before returning from that feed.
The later facade may loop internally to collapse Progress; the private source
foundation keeps the distinction observable for independent termination tests.

Advancing drains pending references/frame returns without physical input until
one semantic token or NeedPhysicalInput. Reader/Push/Buffered must drain this
zero-input work before returning for more bytes or calling the source. In
particular, Push's current `while !remaining.is_empty()` loop cannot remain the
DTD-aware driver: a GE at the final byte of a feed must still expand during that
feed. Use an explicit progress/need-input result or one drain call after each
release; do not infer progress solely from consumed physical bytes. Empty feeds
remain no-ops once the previous call has drained all available work.

These methods reuse concrete leaf/tag preparation and frame operations; they do
not introduce a generic Processor/Source/Resolver trait hierarchy. Stream/Reader
keep their existing first-error/EOF/panic poison owners. An internal controller
failure records the same typed cause for subsequent operations.

## Named references and lexical units

Use the existing markup tokenizer. Add narrow internal operations to flush a
nonempty Text at a named-reference boundary, resume at a checked cursor, and
skip a validated reference spelling after releasing the prefix. Do not fake
bytes, normalize a reference during classification, or concatenate expansion.
A bounded `ReferenceScratch` is owned by the optional session, not every ordinary
tokenizer. It collects one `&...;` incrementally with checked source lengths.

At Text state, search for `<` or `&`; markup states keep the tokenizer's existing
quote/comment/CDATA/PI handling. Drain existing `<` lookahead before deciding the
state. A numeric or predefined spelling is appended back into the containing
Text token; its decoded *output* is never scanned again. A non-predefined named
spelling becomes a pending GE reference. If preceding Text exists, deliver and
release it BEFORE catalog lookup, frame entry, attempt charging or child-source
publication. Cancellation or callback panic there leaves no entity inclusion
side effects. Read-only bounded reference classification/lookahead may already
have consumed its physical spelling, but no body suffix is consumed.

Scratch is bounded separately from the Text prefix: each reference and each
Text/markup unit obey their source/decoded byte ceilings; combined pending
scratch is at most one token plus one reference, with checked capacity/work
limits. Never reject two individually permitted units merely because their
combined temporary prefix/reference length exceeds one token ceiling. This
extra bounded buffer is the deliberate cost of preserving numeric/predefined
Text boundaries without rescanning prefixes or reading the suffix.

Physical `position()` may include this reference lookahead, just as Stream
already includes `<` lookahead. Prefix Text has its exact pre-reference origin;
its cancellation endpoint remains that origin's end. Generated events cancel
at the real outer reference end. Transport progress, callback cancellation and
replacement-local diagnostics remain separate.

Replacement tokenization uses an explicit local-source mode, not a fake physical
TokenSpan. Recommend a private exhaustive location discriminator:
`TokenLocation::Input(TokenSpan)` or `Replacement(local_range)`. Existing
physical consumers map Input as today; the content controller combines the
local range with its current declaration/invocation/outer-reference metadata.
This is a concrete extension of Tokenizer for its two actual source types,
not a new generic lexer. Keep extra replacement/reference payload out of the
ordinary persistent parser state and measure any Tokenizer layout change.

Only Text recognizes GE references. Tags keep complete lexical attribute values
for the DTD tag preparer. Comments, CDATA and PI data never expand references.
DOCTYPE/XML declaration/other forbidden markup in a replacement is a typed
content failure; it never re-enters the DTD engine. A physical GE outside the
root's content is rejected before inclusion, including an empty GE.

## Iterative scheduling and semantic publication

Keep the primary tokenizer paused while active replacement frames run. One
replacement tokenizer is reset only at a complete token/source boundary.
Source cursors include consumed lexical lookahead separately from the frame's
committed cursor: a Text token ending before saved `<` commits only its own
range; the next markup token includes the replayed delimiter exactly once.
For references, commit prefix Text first, then `enter_nested` commits the parent
reference spelling atomically. Reserve the mirrored active-ID slot before frame
entry. At return, restart from the parent's committed local cursor without
resetting document work/attempt/expansion counters.

Every source must end between complete lexical units. A partial tag, reference,
comment, CDATA or PI cannot finish in the caller. Flush final source-local Text,
deliver/release it, then require `finish_current(Content { element_depth })` to
match the entry depth before resuming the parent. Empty replacements emit no
empty Text but still spend attempt/work budgets and preserve the input gap.

Before each End reaches ParserState, call `check_before_end_element(depth)`.
Prepare/validate event fields and reserve ownership before namespace/element
publication. Charge/commit replacement source bytes before semantic publication,
so a frame byte/work failure cannot pop a parser frame or emit an event. Fatal
preparation may leave spent work/source progress but publishes no rejected
semantic event; there is no whole-document transaction/rollback framework.

Start tags use the other agent's `RawTag -> PreparedTag -> publish_prepared_tag`
seam with explicit LiteralSource and token-local offsets. DTD defaults and
explicit values are normalized once before namespace resolution. Add only the
corresponding narrow prepared-End/leaf operations needed for owned generated
payloads. ParserState's element-depth getter is supplied by that refactor.

Reader-generated start/end Names, attribute Names/values, PI target/data and
Text/CDATA/comment data must be owned before their token/catalog borrow ends.
Move already owned values; copy borrowed values fallibly under the shared Budget.
PreparedTag already owns/promotes the relevant name fields. Prepare an owned
End name before its namespace unwind. Construct each supported content event
from these owned payloads; do NOT add a general Event-to-static converter or
pretend Declaration's borrowed fields are static. Generated XML declarations
are rejected before leaf preparation. Physical Reader events continue borrowing
stable source; Stream events may lend current scratch. No borrowed catalog value
survives a mutable normalization operation or event return.

LiteralSource::Replacement disables physical CR/CRLF normalization in content
Text/CDATA/comment/PI; replacement construction already handled literal EOL once.
Numeric/predefined outputs append as final data. Numeric `<` created during
EntityValue construction becomes replacement markup; direct content `&#60;`
and `&lt;` remain Text data. Attribute normalization follows the separate catalog
rules, including literal whitespace replacement. Keep the documented numeric-CR
libxml divergence explicit. Never check normalized/concatenated output for raw
CharData `]]>`; validate each actual source spelling in its lexical context.

## Limits, errors and staged integration

The one global work total begins with `DtdBodyState.counters.work` and is capped
by the same retained `limits.max_work`. Its accounting units are explicit:

- count each newly accepted decoded source byte once in scanner work, whether
  physical or replacement; include reference scratch collection in that scan;
- charge each raw reference append-copy before growing its scratch buffer;
- charge one reference-spelling length before its final UTF-8/name/classification
  pass; incomplete fragments are never reclassified from the beginning per chunk;
- charge copied reference bytes once before replaying numeric/predefined spelling
  into the Text buffer; that replay is a copy, NOT newly scanned input;
- charge a complete token's raw length once before semantic grammar/leaf processing;
- add committed catalog and frame work deltas, including deltas on failure.

Saved `<` replay, a pending GE spelling's already-counted physical bytes, and
returning to a parent cursor are not new scanned bytes. Tokenizer positions may
advance during a replay/skip, but the coordinator's newly-consumed source count
must not. Resetting a fragment tokenizer does not reset global scan/work totals.
Field retention/default/namespace allocations remain bounded by the SAME shared
parser Budget; this work counter is named bounded processing passes, not an
instruction counter or total allocator-memory measurement.

Before each bounded scan/classification/copy/grammar operation, check/charge its
known length against remaining global work. Before a catalog/frame call, cap its
absolute component work ceiling to `usage.work + remaining_global` (checked or
saturating only as an explicit upper cap), then synchronize its committed delta
after success or failure. Existing component caps only decrease. No operation
may overrun global work and have its spent delta checked only afterwards.
A pop-only Progress can cost zero work because active state strictly decreases;
an empty entry still incurs the frame attempt charge. Tests must cover the
empty-entry/pop cycle at exact attempt/work ceilings and forbid zero-state loops.

Move every DTD/body counter without resetting it. Frame attempt/work/expanded
ceilings cover prior PE use and later GE use together. Charge incremental lexer,
reference classification/copy and semantic/catalog work before their bounded
allocations; scanner restarts cannot restore allowances. Global input/decoded,
unit, event, parser depth and shared owned-capacity limits remain in force.
The existing DTD `owned_bytes` specifically counts retained raw-unit copies;
retain that accounting honestly. Body event/scope/default ownership spends the
SAME cumulative parser Budget, never a fresh one. Do not silently relabel the
DTD-only raw-copy counter as total allocator memory. If activation requires a
separate total DTD+body copy ceiling, root must explicitly select its semantics
and apply it before allocations rather than checking a spent delta afterwards.

Physical errors map through real TokenSpan/SourceEncoding, flooring UTF-16 scalar
boundaries. Replacement errors retain local UTF-8 offset, declaration/invocation
and real outer-reference context; never remap local offsets as UTF-16 input.
Retain a typed private content failure (semantic kind, physical anchor, exact
origin/local offset) in the session. Existing public Error is Copy plus one offset;
a boundary adapter can return the truthful physical anchor while the private
context remains retained. This is not a completed public replacement-diagnostic
API: exposing that context is an explicit activation review, never a local
offset disguised as a physical Error offset.
Unavailable external/unparsed/unread/missing dependencies remain explicit
non-success with no resource I/O or silent omission.

Implementation after review: (1) finish adapter/tag seams; (2) add bounded reference
and local-tokenizer foundations privately; (3) GE scheduling/owned content leaf
preparation; (4) wire Reader/Stream behind private activation; (5) integrate all
public DTD events/defaults/DOM/serialization surfaces together. No partial public
DOCTYPE acceptance follows merely from completing the content controller.

Independent literal tests must cover prefix-callback abort before lookup/entry;
numeric/predefined same-run/no-rescan pairs; all GE entry/exit boundaries,
empty-reference Mixed DOM gaps, nested/repeated invocation identity, actual
UTF-8/UTF-16 split schedules and untouched suffix/cancellation endpoints;
partial lexical sources; close/reopen attacks and missing closure; EOL phase
pairs in all leaf kinds; generated namespaces/defaults; Reader events retained
across subsequent calls; prior-PE-to-GE counter/nonce continuity; exact/one-short
source/scratch/work/copy/depth/attempt/expanded/event limits and atomic rejected
End/start publication. Public activation still needs reviewed full API/DOM/Writer
representation, complete fuzz/XMLTS/layout/performance gates and measured evidence.

## First compiling lexical foundation (private staging)

The first source checkpoint adds only the following components. It does not yet
compose references with Text, look up entities, schedule frames, prepare tags,
or wire a frontend/session container.

- `tokenizer::local::LocalTokenizer` is a sealed wrapper over the existing
  Tokenizer. Its ready span is `ReadySpan::Local(range)` rather than an Input
  TokenSpan. Ordinary physical consumer signatures remain unchanged. The local
  wrapper/variant/mode flag are test-only staging until activation; no blanket
  dead-code suppression is used.
- `LocalLimits` bounds token bytes, scratch bytes and aggregate work. `LocalUsage`
  counts newly examined source bytes once, plus one complete-token validation
  pass. Saved `<` replay advances no source counter. Release keeps transport-free
  local position; EOF seals appends, partial source EOF fails, and restart requires
  a released completed source while preserving aggregate usage. DOCTYPE and
  invalid-markup sentinel prefixes are failures, never content events.
- `dtd_reference::ReferenceScratch` retains one raw `&...;` and returns its exact
  local range plus Numeric/Predefined/General classification. Numeric scalar
  inspection does not replace its raw spelling or rescan output. It performs no
  catalog lookup. Separate byte/scratch/work ceilings are checked before growth.
  Usage records raw scan bytes, raw append-copy bytes and one final classification
  length, so a successful reference of n bytes costs 3n work units.
- `advance`, `current`, `release`, `finish_step`, `restart`, `position`, and
  `usage` are the concrete scanner operations. Completed reference units leave
  suffix bytes untouched; explicit EOF rejects later nonempty input. Failed
  reference/local operations retain their first typed local error. At EOF,
  known malformed Name/digit suffixes precede missing-semicolon UnexpectedEof;
  partial UTF-8 is diagnosed once at final classification. Literal non-XML
  characters return InvalidCharacter at their local byte; XML-character Name
  grammar faults remain Syntax, and invalid numeric scalars use the ampersand
  anchor. These checks share the one charged classification pass.

The coordinator must still aggregate these deltas with the moved BodyState.
Replaying numeric/predefined raw into a Text buffer is a distinct future copy
operation, not newly consumed source. The first foundation has no replay helper
or synthetic Progress variant before that real caller exists. Native Text/GE
partition, complete source scheduling and callback ordering remain unimplemented.

Measured with the same Rust1.95 aarch64-apple-darwin private cfg(test) layout
probe before and after the typed local span: Tokenizer152/align8, Stream520/8,
PushParser656/8, Reader392/8 in both builds. These are prototype unit-build
measurements, not a claim about future activated production layout or speed.
Retained logs: `/private/tmp/yaxml-dtd-content-layout-{base,candidate}.log`.
No required state change or counter update occurs inside a debug assertion.

Both foundation types expose monotone `cap_work(absolute_ceiling)`. The coordinator
sets each to component usage plus the remaining global allowance BEFORE advance
or finish; final classification inside advance and validation inside finish obey
that cap. Raising a later cap cannot restore an allowance, and restart never
resets usage. The coordinator retains original component limits to distinguish
a native component ceiling from a global-induced exhaustion. Raw reference
scratch append-copy is charged in ReferenceUsage; copying that raw spelling into
Text later is a separate actual copy and must be charged separately.

## Next private integration slice (reviewed root `29c0631`)

The Text/reference coordinator now exists. The following four steps replace the
older prospective method sketches as the next implementation boundary. They do
not activate Reader, Stream, Push, either DOM, or public DTD acceptance.

1. **Consume the existing body state.** Implement `ContentController` beside
   `DtdBodyState` in `dtd_controller.rs`, consuming that value directly. Keep its
   same catalog, frames, invocation nonce, counters, limits, DOCTYPE range and
   checked `(decoded, physical)` resume pair. No catalog clone, new frame stack,
   independent mutable getters, or session container is needed. Construct
   `TextShared` once with the inherited global work total and the fixed Input,
   Local and reference scratch reservations; move it through every coordinator
   handoff. Keep the caller's same `ParserState` and ownership Budget.

2. **Bridge work into that ledger before composing engines.** Add these narrow
   private methods to `TextShared`, with forwarding methods on `TextCoordinator`
   for use while it owns the shared state:

   ```rust
   fn remaining_work(&self) -> Result<usize, TextError>;
   fn charge_external_work(
       &mut self,
       delta: usize,
       source: TextSourceId,
       offset: usize,
   ) -> Result<(), TextError>;
   ```

   Charges are checked, cumulative and sticky; offsets retain TextError's
   physical-Input/local-Replacement meaning. Before catalog/frame operations,
   request the remaining allowance; a failed ledger returns its prior error
   without authorizing a component call. Otherwise lower the component's
   absolute work cap to its current usage plus this remaining
   allowance, then synchronize committed deltas on both success and failure.
   Preserve native-versus-global failure attribution using the retained original
   limits, and preserve the first failure. Charge complete-token raw length
   before parser semantic preparation. Never reconstruct TextShared to add work:
   that would lose its scratch allocation, reservation, and failure state. Frame
   attempts remain authoritative; copy them into the retained counters after
   attempts. Keep `owned_bytes` as raw-DTD-copy accounting and use the unchanged
   parser Budget for event/default/scope ownership.

3. **Schedule exact sources with short catalog borrows.** After the preceding
   Text event is accepted and released, `general()` exposes the classified
   reference. Copy the lookup ID/outcome and length, end the catalog borrow,
   reserve controller metadata, pre-cap and enter the frame, then consume
   `acknowledge_general()` to move the same shared ledger and lease. Keep the
   primary Input lease paused while reusing one Local lease at acknowledged
   boundaries. Reacquire catalog replacement bytes only for the current drive or
   frame operation; match its frame invocation to the Local lease before use.
   Ordinary local token ranges must start at `CurrentSource.local_start`, end at
   a catalog UTF-8 boundary, and advance the frame before parser publication.
   Commit only the delivered prefix: `enter_nested` itself consumes and charges
   the retained reference range at the parent cursor. Never advance the frame
   by tokenizer lookahead or the total drive consumption. At exact replacement
   EOF, finish and drain its coordinator, check invocation and entry element
   depth with `finish_current`, then restore the parent or physical source.
   Primary chunk exhaustion becomes NeedPhysicalInput only after generated work
   drains; it is never local EOF. Empty entry/pop uses
   the existing finite Progress contract and produces no empty Text.

4. **Publish through the existing typed parser/origin paths.** Before every
   replacement End, run `check_before_end_element(parser.element_depth())`, then
   call the atomic `process_dtd_end_owned`; use `process_dtd_start_owned` and
   `process_dtd_leaf_owned` for the other generated tokens. Keep their explicit
   replacement phase, shared Budget, and exact `FragmentOrigin`; wrap it with
   `SourceOrigin::from_repr` for delivery. One sticky private ContentError must
   retain Text source/offset, frame outer-reference context, tag ValueLocation,
   and ReplacementError local offset. Source/work failure precedes semantic
   publication. The first cohesive private slice owns generated events;
   physical token actions remain scratch-lent and are consumed immediately by
   the same ParserState through `process_token` or `process_dtd_start` with
   Physical phase. It is not an all-events-static conversion, and generated
   declarations remain forbidden.

Stable Reader publication remains unimplemented: `current()` lends tokenizer
scratch, and stable mode still copies physical tokens for lexing. A later
stable-only operation must map the exact TokenSpan decoded range into the same
sealed immutable window before using the existing parser normalizer. Numeric
and predefined references currently append their **raw spelling**, so they do
not require a transformed-token flag; normalization determines logical Text
ownership. Streaming frontend resume also needs a checked tokenizer/lease entry
at both inherited decoded and physical cursors, especially for UTF-16, without
replaying the prefix. The owned generated-event slice may defer these two
frontend seams explicitly; it cannot claim Reader/Stream parity or a preserved
DTD-aware Reader fast path. Public activation and its full validation gates stay
open.
