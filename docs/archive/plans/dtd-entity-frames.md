# Iterative entity-source frame prototype

Base: `3e493e4d37e21b2c29c74bb2a9bdedbe2503d4f3`.

This private prototype schedules already-constructed internal entity replacement
strings. It does not bind declarations, build replacement text, enable DOCTYPE,
change public `SourceOrigin`, or emit parser events. The semantic catalog owns
each immutable processed replacement string. Frames retain only declaration and
invocation identities, byte cursors, lengths, entry boundaries, and the real
outer physical reference span. A caller obtains a replacement borrow for the
current declaration only while advancing that frame; no frame retains a `&str`
across later catalog mutation.

## Concrete interface and invariants

`EntityFrames` owns one bounded `Vec<EntityFrame>` and cumulative counters.
`EntityDeclarationId` identifies a catalog declaration. Every successful
reference receives a fresh `EntityInvocationId`; repeated uses of one
declaration therefore remain distinct sources. `EntityKind` distinguishes
general and parameter entities.

`ReferenceContext` has four explicit phases: document content, attribute value,
entity-value construction, and between-declaration DTD subset content. General
entities can be scheduled only from document content or an attribute value.
Parameter entities can be scheduled only while constructing an entity value or
between DTD declarations. Character references never create frames:

- entity-value construction decodes character references, includes parameter
  replacement text, and retains general references for later use;
- document and attribute processing decode character references directly, and
  only named general references schedule frames.

The catalog adapter passes one concrete lookup result: internal replacement,
external unavailable, unparsed, unread, or undeclared. This keeps every
reference attempt inside the accounting chokepoint without introducing a
generic resolver framework. Internal results carry the declaration ID and
replacement byte length. A narrow current-source method additionally receives
that same ID and the catalog's current `&str`, verifies the frozen length and
UTF-8 cursor boundary, then lends only the unconsumed suffix.

A physical entry records the original `&name;` or `%name;` byte span. A nested
entry records its reference range in the current replacement and inherits that
frame's outer physical reference span. Nested ranges must start at the current
cursor and end on a UTF-8 boundary. Scheduling commits the parent cursor only
after every child-frame check and reservation succeeds.

Each frame snapshots a `FrameBoundary`: content plus current element depth, an
attribute literal, entity-value construction, or between-declaration DTD
state. Before accepting an end tag from replacement text, the controller must
call the frame guard; it rejects a close when current depth is at or below the
active content frame's entry depth. This prevents `</outer><b>` from closing a
caller element and later restoring the same numeric depth. A frame can finish
only after consuming its complete replacement and returning to the same
boundary; content must also return to the entry element depth.

The core cannot prove lexical state from byte counts alone. The future
controller must parse each token wholly within its source, call the end-tag
guard before publication or source-origin closure, and report its actual state
at frame completion. Attribute/entity-literal rules and the DTD fragment scanner
provide the corresponding complete-source checks for their contexts. These
controller checks together enforce the parsed-entity nesting requirement; the
frame stack alone does not claim to validate arbitrary replacement bytes.

## Provenance without a history arena

`FragmentOrigin` is a private prototype of the eventual `SourceOrigin`
representation, not metadata stored beside it. A direct fragment has an input
span. A replacement fragment carries declaration ID, fresh invocation ID,
replacement-local span, and the outer physical reference span used for errors
and cancellation. It does not retain the full invocation chain.

Only contiguous direct spans, or contiguous ranges from the same exact
replacement invocation, can join exactly. Different declarations,
invocations, gaps, or already-mixed origins join to `Mixed`, retaining only a
bounded physical causal cover and exposing no exact input or replacement range.
Activation replaces the existing origin representation and teaches both DOM
builders the same join rule; it must not duplicate this metadata in a second
persistent arena. Layout sizes for frames, origins, the stack, and current
`SourceOrigin` are measured at the compiling checkpoint.

## Resource and failure contract

Independent ceilings bound active depth, reference attempts, cumulative work,
and consumed expanded bytes. Every lookup outcome, repeated reference, cycle,
and empty replacement charges one reference attempt and base work. Cycle work
also charges the bounded active-stack comparisons. Consuming replacement bytes
charges both work and expanded bytes before advancing the cursor. Nested
reference spelling bytes are charged before entry. Thus empty entities and
failed lookups cannot amplify without consuming a non-output budget.

Checked arithmetic distinguishes counter overflow from an ordinary exhausted
limit. Invocation identity exhaustion is explicit. Cycle, wrong-kind/context,
source mismatch, premature EOF, lexical-boundary mismatch, and element-depth
mismatch are typed failures with the outer physical diagnostic offset. Logical
limits are checked before fallible stack reservation; allocation failure does
not publish a frame or parent cursor. Frames are an ordinary flat `Vec`, so
release and drop are iterative.

## Verification and activation boundary

Author and independent table catalogs cover:

- physical and nested success, repeated declaration uses with fresh identities,
  exact local origins, contiguous joins, gaps and mixed joins;
- direct and indirect cycles, empty replacements, every lookup outcome, and
  the complete kind/context matrix;
- premature completion, depth changes, lexical-state crossings, source-ID and
  length mismatches, non-boundary UTF-8 cursors, and nested span validation;
- exact/below/above attempt, work, depth, and expanded-byte limits, checked
  counter/identity overflow, failed-entry atomicity, deep iterative release,
  and stable physical error context through nested frames;
- bounded deterministic sequences compared with a literal stack/counter model.

Activation requires the catalog, scanner, unit grammar, replacement
construction, and this frame core to share one controller across Reader, push,
buffered pull, and both DOM builders. The same change must add replacement and
Mixed `SourceOrigin`, provenance-aware element closure, source-homogeneous text
events, and parse-only DOM text coalescing. Until then, the module remains
test-only and supports no DTD compatibility or acceptance claim.
