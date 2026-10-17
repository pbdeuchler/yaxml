# Private Writer protocol for internal DTDs

Base: `864b7b259b8d1b4493b69db2c0a835a269710039`, including the reviewed
borrowed-unit and complete-End handoff API. This is a design for a private
Writer path. It does not add public Event variants, make a header-only DOCTYPE
successful, enable DTD parsing in a frontend, or perform external I/O.

## Protocol and lifecycle

Add one private adapter over the existing owning controller event:

```text
Writer::write_dtd_event(&TakenDtdEvent<'_>)

TakenDtdEvent::Begin(DtdUnit)
TakenDtdEvent::Item { unit: DtdUnit, literals: LiteralSource }
TakenDtdEvent::End
```

Borrowing the owning event lets a Reader/controller pipeline take an event,
write it, and then release it without copying raw text. Event source origin is
not serialized. The later public Begin/Item/End variants can call the same
private path only when Reader, streaming, both DOMs and Writer activate
together.

Writer keeps an explicit DTD phase: Absent, Open, or Complete. Open records
whether the header declared an internal subset and owns the one semantic state
built from the emitted units. The existing ParserState remains authoritative
for prolog/content/epilog placement and event/depth/namespace limits; add narrow
private DOCTYPE transition methods instead of a second document-phase machine.

The allowed sequence is:

- Begin once in the prolog, after an optional XML declaration and any allowed
  Misc, but before the root;
- Item only while a subset-bearing Begin is open;
- End exactly once for the open DTD;
- ordinary document events only before Begin or after End.

Begin, every Item and End each consume one entry from the existing shared
semantic-event ceiling as well as the private DTD-event accounting. Whitespace
and declarations keep their current ordinary-event rules. The DOCTYPE root
name need not match the later document element: that constraint is validity,
not well-formedness, and this remains a nonvalidating path.

A second or misplaced Begin, Item without a subset, duplicate End, ordinary
event while Open, or DTD event after the root reports Syntax at the current
output position and poisons the Writer. Writes after any terminal failure or a
successful finish report InvalidState; a repeated successful `finish()` remains
the existing no-I/O success. `finish()` while Open reports UnexpectedEof before
root completeness. `into_inner()` keeps its existing unchecked behavior and
never turns a partial DOCTYPE into success.

The no-subset header unit contains its real final `>`, but Begin writes the
validated prefix excluding that byte. Its End writes `>`. A subset-bearing
Begin writes its complete header through `[`, and End writes `]>`. Thus every
logical event has nonempty output while the protocol preserves the controller's
held-delimiter semantics. A sink failure between Begin and End can leave a
partial document, under the Writer's existing documented policy.

## One semantic state

Writer Open creates one `DtdCatalog`, applies the supplied header/items through
`DtdCatalog::apply`, and moves that exact catalog into Complete at End for
body-tag default processing. It does not reparse emitted bytes, clone the
catalog or rebuild bindings at End. This is the same concrete declaration and
default engine held by the parser's DtdBodyState; the Writer needs no parallel
entity table, default parser or namespace resolver. DtdController and its
reviewed handoff remain unchanged by this slice.

Apply the Header and every Item with `internal_subset: true` and the event's
explicit `LiteralSource`; logical items are the controller's expanded internal
subset sequence. Preserve first-binding, declaration order, unread barriers and
every reported unavailable dependency. The Writer does not construct entity
frames because its body events are already logical values and it never expands
their serialized references.

The Writer does not expand arbitrary body references: ordinary Text and
attribute Event values are logical values and the existing encoder escapes
ampersands. Consequently no resolver or filesystem/network callback exists.
External identifiers are serialized lexically and never opened. Unused
external declarations can be represented; an unavailable dependency
encountered while applying a declaration/default is a typed non-success. The
XML declaration's standalone value, or false when absent, initializes the
shared catalog exactly once at Begin.

The controller handoff retains its checked decoded/physical resume pair. Writer
catalog state has no fabricated source coordinates or frame occurrences;
logical body values are serialized without entity-reference activation.

## Per-event preparation and output

Retain the existing poison-before-mutation rule. Refactor only the current sink
write loop into a private `write_buffer()` used by ordinary and DTD preparation.
For each DTD event, perform all sequence, family, quota, encoding and semantic
checks before the first sink call. Once prepared, use the same short-write,
Interrupted retry, WriteZero, oversized-count, position and panic behavior as
ordinary events. Successful completion returns Writer to Active; every error
or unwind leaves it Failed.

The deterministic validation order is:

1. Writer lifecycle, ParserState placement and DTD sequence;
2. shared event ceiling, DTD event/aggregate ceilings, and required unit family;
3. unit metadata limits and complete encoding into the existing TokenBuffer;
4. semantic catalog application and unavailable-dependency checks;
5. infallible phase/counter commit;
6. the existing sink loop.

Begin accepts only a Header view. On successful XML declaration preparation,
Writer records its standalone value; absent or unspecified means false for the
catalog created at Begin. Item rejects Header and accepts only Element,
ATTLIST, Entity, Notation, Comment or PI. End carries no unit. The family check
does not trust a scanner tag or caller enum alone.

Use the existing `DtdUnit::physical_fragments` iterator; do not add another DTD
encoder or intermediate rewritten String. Physical Items append their exact raw
fragment. Replacement Items preserve reference spellings, encode literal CR in
EntityValue as `&#13;`, encode literal attribute CR as space, and reject CR in
system/public identifiers with a typed unit-local error. Keep that local offset
for diagnostics, but map the Writer-facing error to the prospective output
position after the already-encoded fragment prefix. This is
`Writer::position + TokenBuffer::len()` at the iterator error, rather than
blindly adding the raw-unit offset: an earlier replacement CR may have changed
the encoded length. No bytes have reached the sink. Comments and PIs retain
Writer's documented raw-CR normalization policy.

TokenBuffer remains the only per-event output scratch. Begin's held delimiter
and End's fixed delimiter are appended through its checked `push`; fragment
fusion is unnecessary. Iterate and validate every generated fragment into the
buffer first; never prefix-write and discover a late identifier error after
sink I/O. The complete encoded event must fit both remaining output bytes and
the current-token ceiling before catalog mutation or I/O.

Applying a validated unit can mutate the catalog before a later semantic error,
as it does in parser operation. This is safe because Writer is already poisoned
and cannot resume. Encoding/allocation/limit errors occur before catalog apply,
so an unrepresentable event never publishes a declaration even privately.

## Body events, defaults and namespaces

Writer must validate actual serialized body syntax against the DTD state. It
must not feed logical Event values directly into AttValue normalization: for
example, logical `AT&T`, a numeric CR and literal whitespace have different
lexical phases. Continue the shipped Writer contract: encode the supplied
Start/Empty event into its one bounded TokenBuffer first, then run that exact
escaped tag spelling through the shared concrete pipeline:

```text
ParserState::collect_raw_tag(encoded, output_start)
prepare_dtd_tag(semantic.catalog_mut(), state.owned_budget(), raw,
                LiteralSource::Physical)
ParserState::publish_prepared_tag(prepared, retain)
```

This replaces the ordinary `process_token` start-tag path for a Writer with a
completed DTD; it does not run in addition to it. `prepare_dtd_tag` therefore
normalizes each encoded Physical attribute literal exactly once. This is the
Writer's existing single-token encode-and-validate policy, not a reparse of the
document, expanded input, DTD declarations or semantic catalog. Reader/Stream
continue to prepare their input token directly without a serialize/reparse
adapter.

One narrow orchestration helper shared with the future content controller owns
event-limit/phase checks and event-count commit around those three calls. Writer
must not duplicate this order. Publication remains the sole atomic namespace
overlay and element-frame path.

The supplied Writer Event describes attributes to serialize, not a promise that
every DTD default is materialized lexically. The encoded explicit attributes
remain the prefix of the prepared/published list; defaults append in effective
declaration order. Validate:

- supplied element QName and namespace against the published element;
- every supplied attribute, in order, against the corresponding explicit
  published attribute, including namespace and logical value;
- all remaining published attributes as successfully validated DTD defaults,
  without adding them to the output buffer.

This accepts both sparse events that omit defaults and expanded Reader events
that include them explicitly, regardless of whether the caller originated from
a Reader. Omitted defaulted namespace declarations still participate in
resolving the supplied element and explicit attributes; a Name with
inconsistent namespace metadata is rejected. An included default becomes an
explicit serialized attribute and suppresses that default by lexical QName.
Defaulted ordinary attributes need not be redundantly written because the same
DOCTYPE precedes the root and recreates them on reparse. Tests must pin all
three forms (sparse, explicitly expanded and inconsistent) rather than infer
parity from the implementation.

End-tag and non-tag body validation continues through ParserState. DTD body
state is retained until finish so later tags at every depth see the same first
bindings. Fixed-value mismatch, required attributes, enumeration membership and
ID validity remain validity constraints outside the nonvalidating policy.

## Bounds and errors

Keep existing Writer output/token/depth/attribute/event/owned ceilings. Add one
private `DtdWriteLimits` for the prototype with explicit unit limits, catalog
limits, cumulative encoded DTD bytes, logical DTD events/items and DTD work.
Before public activation, freeze a public nested limits shape rather than adding
many unrelated flat WriterLimits fields.

- `DtdUnit::check_limits` inspects already-validated metadata and raw length; it
  does not reparse raw text.
- encoded bytes are checked by TokenBuffer and cumulatively by the DTD byte
  ceiling with checked arithmetic;
- DtdCatalog retains its declaration, binding, storage, value, expansion,
  reference, depth and work ceilings;
- ParserState's owned budget charges prepared logical values, default copies
  and namespace/frame retention;
- catalog storage and TokenBuffer scratch retain their separate documented
  ceilings and are not double-charged as ParserState owned bytes.

DTD event counters commit only after successful preparation. Failed work and
catalog work need not be refunded because failure is terminal. No unchecked
addition, allocator panic path, ambient I/O, retry with a fresh budget, or
whole-subset output buffer is introduced.

## Narrow source and test slice

Expected implementation files:

- `src/writer.rs`: private DTD phase, preparation entry point and shared sink
  loop;
- `src/dtd_writer.rs`: small sequence/catalog/output adapter, compiled only for
  the private milestone;
- `src/parser.rs` and `src/dtd_tag.rs`: one concrete shared DTD-aware tag
  orchestration method and DOCTYPE placement/event accounting;
- `src/dtd.rs`: metadata-only limit check; reuse `physical_fragments` unchanged.

Tests must cover every valid Begin/Item/End sequence, no-subset delimiter split,
all misplaced/duplicate/unfinished permutations, family rejection, Physical and
Replacement output bytes, identifier errors, exact/one-short quotas, sparse and
expanded defaults, defaulted namespace resolution, conflicting Name metadata,
catalog first-binding effects, and default suppression. Existing Writer I/O
fixtures must repeat short/interrupted writes, zero/oversized counts, mid-event
errors, flush errors and recovered panics for each DTD event kind, asserting
position and terminal poison.

Fault gates should remove the held delimiter, bypass Item catalog application,
drop defaulted namespace publication, compare logical rather than encoded
attribute spelling, reset semantic state at End, and permit finish while Open.
Public activation still requires Reader/Push/Buffered, both DOMs, fuzz/XMLTS,
full conformance and performance gates; this private protocol alone is not DTD
acceptance evidence.
