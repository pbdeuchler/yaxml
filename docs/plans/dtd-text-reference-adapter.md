# Text and reference boundary adapter

Plan-only follow-up to the private lexical foundation at `1181f3b`. That
checkpoint still reads Text through `<`; it cannot schedule a GE before its
following body bytes. This note specifies the missing adapter. It changes no
source, tests, public acceptance, or ParserState API.

## One tokenizer buffer and one pending reference

Add a DTD-aware tokenizer entry that stops **before** an ampersand in Text.
Keep ordinary `advance` behavior. The new entry shares existing markup states:
ampersands inside tags, comments, CDATA and PI never become content-reference
boundaries.

At that stop, the tokenizer retains its unfinished Text prefix. Its position
is the ampersand's position. ReferenceScratch collects from that ampersand
through the semicolon without consuming the following body byte. The adapter
retains a separate accepted-input cursor while this bounded lookahead is held.
It never feeds the reference spelling back through the lexer.

Numeric/predefined references join the same raw Text buffer after classification.
Their raw spellings remain present for normalization into final character data.
General references end the Text source segment. If its prefix is nonempty,
publish that prefix and wait for successful release before skipping the spelling
or performing catalog lookup/frame entry.

This uses the existing token buffer plus one separately bounded reference
buffer. It adds no expanded-input buffer, occurrence history, or source trait.
The coordinator owns the pending state; physical and replacement tokenizers do
not each need an inline ReferenceScratch.

## Narrow tokenizer operations

Proposed private operations, with names subject to API review:

```text
advance_content(bytes) -> Result<ContentStep, BoundaryError>
ContentStep { consumed, stop: Exhausted | TokenReady | ReferenceStart }

flush_text_before_reference() -> Result<bool, BoundaryError>
append_character_reference(validated_reference, reference_extent)
    -> Result<(), BoundaryError>
skip_general_reference(validated_reference, reference_extent)
    -> Result<(), BoundaryError>
```

`advance_content` replays saved `<` first, exactly as ordinary `advance` does.
It stops at `&` only while the actual scan state is Text. The ampersand remains
unconsumed and unappended. ReferenceStart may have zero consumed bytes; the
adapter must change to collection before calling again. Empty input with no
saved delimiter cannot manufacture ReferenceStart.

Use `memchr2('<', '&', bytes)` for this Text branch. Other states retain existing
delimiter handling. A private compile-time entry selection can preserve the
ordinary one-delimiter Text loop and avoid an unconditional runtime DTD flag.
This is two concrete entry paths, not a generic lexer framework. Measure actual
layout and DTD-free costs rather than assuming the extra path is free.

`flush_text_before_reference` requires no ready token, no saved `<`, and Text
state. A nonempty buffer becomes a ready Text with its current exact endpoint;
an empty buffer returns false. It does not seal EOF, clear the prefix, consume
a reference, or change accepted-input progress.

`append_character_reference` accepts only a Numeric/Predefined result from the
current validated scratch. It checks Text state, matching start/encoding,
checked endpoints and combined token ceilings before reserving. It appends the
**raw spelling**, then advances decoded/source cursors to the saved endpoint.
It does not call `consume`, `plain_run`, or `advance` on those bytes. The
resulting character is never scanned as source.

`skip_general_reference` accepts only a General result after prefix release.
It requires an empty Text buffer, no ready token/lookahead, matching source and
checked endpoints. It advances relevant cursors and the next token start without
appending, allocating, or changing parser/catalog/frame state. The reference
remains held until its caller acknowledges the lookup/entry outcome; ordinary
body scanning cannot overtake it.

Expose a small `ValidatedReference<'scratch>` view from ReferenceScratch with
private kind/raw/local-range fields. Only successful classification constructs
it. The adapter supplies a private extent built from actual collection; no
public operation sets cursor endpoints. Existing foundation getters can remain
during migration. This avoids an unchecked raw skip and avoids reparsing the
name to recover its classification.

The pending extent also retains the actual active source identity: the one
primary input, or the existing replacement invocation ID. Releasing/restarting
a source requires no pending reference. The coordinator checks that identity
before calling a tokenizer mutation; the tokenizer checks its own local/input
mode, cursor and encoding. This uses existing frame identity and needs no
additional occurrence arena. Foreign stable-input advances are structurally
unavailable; owned/local drivers reject the wrong active source or lifecycle.

Mutation operations still check extent geometry against the tokenizer's current
cursor and mode. This protects state consistency without repeating reference
grammar. Required copy/cursor effects execute outside debug assertions.

## Concrete state and delivery order

The coordinator stores one enum rather than independent pending/ready flags:

```text
Scanning
CollectingReference { start, accepted_end }
PrefixReady { complete_reference_extent }
GeneralReady { complete_reference_extent }
Complete
Failed(first_error)
```

Its drive result is `{ consumed, outcome }`, where consumed counts new decoded
bytes accepted from the supplied physical slice. Outcomes are `TokenReady`,
`GeneralReferenceReady`, `NeedInput`, and `Complete`. Generated driving has an
equivalent local consumption count, never reported to Decoder as physical input.
A ready result stays ready until explicit release; returning it again does not
claim another transition occurred.

The sequence for `pre&e;post` is:

1. Scan `pre`, stop before `&`, and collect exactly `&e;` into scratch.
2. Classify once. Do not inspect `post` or look up `e`.
3. Flush `pre`; return TokenReady with its exact prefix origin.
4. After successful publication/callback and explicit `release_token`, clear
   the prefix, check/skip the General extent, and return GeneralReady.
5. The content controller looks up `e`, checks content context, and enters a
   frame. Only successful entry releases reference scratch. Pause the parent
   adapter while driving the replacement.

An empty prefix omits step 3. Catalog lookup, frame attempts/nonce publication,
and child-source publication still occur only after GeneralReady. Missing,
external, unparsed and unavailable outcomes remain controller non-successes.
Lexical skip is not an EntityFrames parent-cursor commit; nested entry commits
that cursor under the frame API.

`release_token` is an acceptance operation and returns InvalidState for a wrong
lifecycle call. Callback cancellation/panic never calls it. A Reader event is
independent of mutable scratch before release. Lexical classification or work
failure can precede prefix publication: ordering protects inclusion effects,
not publication of valid bytes before every later lexical error.

Numeric/predefined classification checks/appends raw spelling, releases scratch,
and resumes Scanning. More bytes can then be consumed because no inclusion is
pending. `&amp;T;` remains raw spelling and normalizes to `&T;`; it cannot cause
a second reference lookup.

The later ContentController maps TokenReady to EventReady and uses its existing
finite Progress result for frame transitions. The adapter needs no synthetic
Progress: an incomplete reference needs input; a token or General reference
waits for acknowledgement; Complete stays Complete. An empty GE costs one frame
entry and one decreasing-state pop. Zero-input draining cannot reclassify a
held reference or enter it twice.

One replacement tokenizer can restart at entry/return because every General
entry released preceding Text and skipped the full reference. Parent cursors
live in active frames, not copied token buffers. Saved `<` cannot cross a GE
entry: it first becomes markup, where ampersands are not content boundaries.

## Exact locations and physical transport

Reuse concrete Input-versus-Local spans. Input is a real TokenSpan with decoded
and physical ranges plus SourceEncoding. Local is a replacement UTF-8 range,
combined later with its actual occurrence. Neither becomes a fake input range.

At ReferenceStart, save decoded/source endpoints. While scratch accepts decoded
bytes, checked-add byte lengths to decoded progress and SourceEncoding source
lengths to physical progress. UTF-16 counts source width on leading UTF-8 bytes,
as the existing tokenizer does. Partial decoded slices remain supported without
decoding again. The completed extent excludes every body-suffix byte.

Keep the tokenizer's committed endpoint, adapter's accepted reference endpoint,
and Decoder's transport position/carry separate. Decoder may already have
accepted a UTF-16 scalar while lending only part of its UTF-8 encoding. Do not
replace Decoder, infer transport progress from origins, or consume its suffix.

Physical reference errors use the reference span, not the prefix span. Map
decoded diagnostics through the valid UTF-8 prefix and floor scalar boundaries
as TokenSpan does. UTF-8 input retains identity offsets. Replacement errors
retain local offset plus occurrence/reference context and never use UTF-16
input mapping.

Prefix cancellation uses its own end even if bounded reference lookahead has
advanced transport. Generated events cancel at their real outer reference end.
The eventual whole physical source range includes skipped General spellings.
No cancellation endpoint is inferred from generated byte counts.

Reader's stable decoded UTF-8 window has checked resume bounds and identity;
advance reads that same window, never foreign supplied bytes. Stream receives
decoded slices from its existing Decoder, with distinct physical/decoded
positions. Replacement mode accepts current catalog replacement bytes and local
coordinates. Physical Text containing numeric/predefined spellings stays
contiguous in stable input, so Reader can borrow that original slice.

## Byte, copy and work ceilings

General scratch has independent decoded-byte/scratch ceilings and an Input
physical-spelling ceiling. A retained prefix does not contribute to these:
prefix Text and General spelling are different units. The shared scratch policy
reserves fixed Input-token and replacement-token slots plus the effective
reference cap. Checked addition rejects overflow or a sum above the total before
constructing shared state. Both token slots stay reserved when Input is paused
and Local is active; switching sources cannot erase a retained primary buffer.

These are configured maximum capacity-request reservations, not an RSS or
whole-controller memory measurement. Source binding also checks its currently
retained Vec capacity against its slot, since configure does not shrink buffers.
The token requirement is the greater of retained capacity and its actual growth
cap: UTF-8 min(raw, decoded), UTF-16 min(saturating(raw/2*3), decoded). Reference
requests use min(reference bytes, reference scratch bytes). Allocation rounding
can exceed a requested target; catalogs, frame metadata and semantic outputs
have their separate ceilings. Every reserve remains fallible.

`TextCoordinator::new` is fallible and checks source fit through a read-only
shared-ledger operation before changing lease permission. A mismatch or prior
sticky failure cannot spend work, create reference scratch, or reset a lease's
boundary state. Retained semantic outputs spend the shared parser Budget.

For Numeric/Predefined, prefix and spelling become one Text token. Check combined
physical TokenBytes, decoded-token bytes and token scratch capacity **before**
appending. UTF-16 uses physical extent width, not UTF-8 length. Two individually
allowed pieces can fail the combined ceiling; flushing cannot evade this limit.

The first collector may precheck physical source width per newly accepted
decoded byte before calling ReferenceScratch. This is linear, handles split
scalars, and needs no decoder change. A later run optimization must preserve
acceptance/error positions. Do not first collect an oversized physical spelling
and only afterwards check its budget.

Work uses explicit bounded operations:

| Operation | Charge before operation |
| --- | --- |
| Normal bytes entering tokenizer | Newly scanned decoded bytes |
| Reference collection | New bytes scanned plus scratch-copy bytes |
| Final reference classification | Spelling length once |
| Numeric/predefined append | Raw spelling copy length once |
| General cursor skip / saved `<` replay | No new scan/copy charge |
| Complete ready-token validation | Raw length once for UTF-8/XMLChar checks |
| Parser grammar/leaf normalization | Its bounded raw-token processing pass |
| Catalog/frame call | Committed component delta, including failure |

The appended spelling participates in ready validation/semantic processing
because those passes read it again. It is not newly accepted source a second
time. Prefixes are not revalidated per chunk. Incomplete references classify
only at final EOF, never repeatedly per chunk.

Use the SAME DtdBodyState spent/remaining global total. Before each component
call, cap it at component usage plus global remaining, without increasing any
previous cap. Precharge known append-copy/validation lengths before allocation
or publication. Synchronize spent deltas on success and failure and distinguish
native limits from a global cap. Restart, release, frame entry/return and replay
restore no allowance.

## EOF and implementation gates

The adapter owns EOF sealing and sticky first error. Final input seals new
physical appends immediately, then drains a complete pending reference/prefix
and later frames before underlying tokenizer EOF. Do not finalize that tokenizer
while a pending spelling still requires cursor synchronization. Partial
reference EOF uses one final ReferenceScratch classification; partial markup
uses the tokenizer failure. Each replacement independently ends between complete
lexical units.

After root API review, implement the tokenizer operations and validated reference
view, then compose this private adapter without catalog/frame calls. Keep
test-only staging until the controller consumes it. ParserState/public-front-end
edits do not belong to this step. Record actual layout and DTD-free behavior.

Independent tests pin every byte split for prefix+named, numeric/predefined,
markup and supplementary UTF-16 scalars; raw bytes/spans/work; untouched suffix;
prefix cancellation before lookup; one `<` replay; adjacent/empty references;
malformed EOF; sticky errors/seals; stale/foreign extent rejection; exact and
one-short separate/combined physical/decoded/scratch/copy/work ceilings; finite
zero-input drains; stable-window identity. Later integration pins same catalog/
frame nonce/counters, generated ownership and pre-publication end guards. Full
DTD acceptance still requires all parser/DOM/Writer surfaces and independent
fuzz, conformance and performance evidence.

## Narrow operation checkpoint

The first implementation exposes the validated scratch view and the four
tokenizer operations above, with a checked ReferenceCursor/ReferenceExtent.
`checked_advance` previews physical/decoded limits and overflow before the same
byte is offered to scratch; only successful scratch acceptance commits that
cursor value. `finish` requires the validated view's exact decoded range.
Input extents retain a real TokenSpan; Local extents offer only a local range.
Cursor overflow remains a distinct private PositionOverflow outcome.

Preview has an explicit same-byte precondition: the coordinator offers exactly
the previewed byte to ReferenceScratch, then commits the preview only after that
byte succeeds. A preview of a whole slice cannot be committed if scratch stops
at an earlier semicolon. Geometry alone does not prove that previewed bytes
equal the validated spelling or belong to the active occurrence.

ReferenceScratch keeps either pending Vec bytes or a ready String plus its
classification. The charged final pass moves the Vec through String::from_utf8
once and classifies that str. `validated`/`current` then borrow the checked str
in constant time. Release returns the same allocation to the cleared Vec for
restart; syntax/UTF-8 failures retain their buffer under the sticky error owner.

LocalTokenizer exposes the same operations under its existing seal/sticky error
owner. LocalUsage adds `reference_copied`: successful raw append bytes, separate
from newly scanned bytes. Work is charged before the append/reservation attempt;
a failed attempt can spend work without adding copied bytes. Ordinary physical
Tokenizer keeps its existing external lifecycle owner; it does not acquire an
EOF/DTD flag or source-invocation field in every parser instance.

This checkpoint has no pending-reference coordinator. Its tokenizer checks
mode, encoding, cursor and token geometry. The later coordinator must bind a
pending extent to the active input/replacement invocation, own physical EOF and
sticky failures, enforce prefix acknowledgement before skip, and pre-cap all
component work. Matching numeric coordinates alone do not establish invocation
identity. No direct helper claims to enforce that absent controller state.

## First concrete coordinator checkpoint

The coordinator owns an active `TextSourceLease` by value. A lease contains the
actual physical Tokenizer or LocalTokenizer; it is never re-supplied to later
drive/release calls. Stable input also retains its immutable UTF-8 window borrow,
while streaming input and local replacement have separate feed methods. A
replacement feed must present the same existing invocation ID. Replacement bytes
still come from that ID's catalog lookup under the later controller's short
borrow; no long catalog borrow is retained across mutable normalization.

This replaces the earlier tentative borrowing-source sketch. A persistent Reader
cannot store a coordinator borrowing its own tokenizer fields. Moving the lease
avoids that self-reference and prevents accepting a different tokenizer merely
because its coordinates match.

A new stable lease rejects a tokenizer with buffered bytes; it cannot attach
foreign scratch to the immutable window. A new local lease requires a fresh
LocalTokenizer. Reusing a worked replacement tokenizer requires the explicit
handoff boundary switch, preserving its counters/caps. Stable EOF also rejects
an unread window suffix instead of silently completing a truncated source.

`TextShared` owns one reference scratch allocation, native reference policy, the
global work total and its first failure. The same value moves with a consuming
`acknowledge_general` or `take_complete` handoff. The former is available only
after prefix acknowledgement and GeneralReady; it does not perform inclusion.
Its returned lease is at a clean boundary and can remain paused while a child
uses the shared state. The future controller keeps only its primary lease
paused. Nested replacement parents remain frame IDs/cursors, not a stack of
tokenizer buffers.

`switch_local_at_boundary` consumes that handoff permission to change the active
invocation/cursor on the SAME LocalTokenizer. Its narrow internal reset requires
empty Text, no ready token or saved delimiter; it preserves all usage and caps.
This allows a parent paused after a General spelling without marking it EOF.
The frame owner must still enforce entry depth, current replacement membership,
parent cursor commit and return guards before switching occurrences.

The first implementation returns raw token bytes with Input TokenSpan or Local
range/invocation, not a new borrowed-str proof. Its current/getter paths do not
repeat validation. Source readiness and pending reference collection charge
their bounded validation/classification work before publication. Semantic
grammar/owned-event preparation remains the later controller's separate pass.
No lookup, entity-frame mutation, ParserState call, or public acceptance occurs
inside this coordinator.

Stable mode still drives ordinary Tokenizer and copies every physical token
into its scratch buffer. The immutable window seals identity and permits later
borrowed publication; that publication does not exist here. `current` lends
scratch bytes, and a future Reader publication must select the sealed same
window and verified range. This is not zero-copy lexing. The prototype does
not preserve Reader's direct-string scanning cost on a future DTD-aware path.
Normal scan/append work and reference scratch/append copies remain real costs.
The ordinary DTD-free Reader implementation is untouched, but activation must
measure this DTD-path copy/performance debt. No second scanner is introduced to
hide it before those measurements.
