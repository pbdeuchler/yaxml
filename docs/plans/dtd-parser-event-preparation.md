# DTD body event preparation

Plan-only slice for the private content controller. The baseline is
`9bf82baa59bd152854fc7ab5579b8f376c387d39`; the staged tag seam comes from
Writer checkpoint `084ee580` and the ordinary-tag fast path from `2855d09`.
This slice does not activate DTD parsing in Reader, Stream, DOM, or Writer.

## Existing boundary and invariants

`ParserState::process_token` currently returns events tied to its raw token.
That is ideal for Reader's stable physical UTF-8 input and Stream's lending
token buffer. It is wrong for a replacement token whose tokenizer/catalog
storage is released before Reader returns `Event<'input>`. The generated path
must therefore construct owned event fields directly. It must not forge a
lifetime or introduce a generic Event-to-owned conversion.

Keep these invariants:

- Physical tokens retain the existing borrowing path and DTD-free fast path.
- Replacement Text processes numeric/predefined spellings once, but skips the
  physical CR/CRLF pass. The decoded result is final data and is not rescanned.
- Replacement CDATA, comment, and PI data preserve literal CR. A generated PI
  whose target is any ASCII case of `xml` fails before leaf publication.
- A generated Start runs `collect_raw_tag -> prepare_dtd_tag -> publication`
  exactly once. Default normalization and namespace resolution are not replayed.
- A generated End owns its qualified name and namespace while the namespace
  scope is still active. EntityFrames checks the pre-End depth before any
  ParserState mutation; only then may ParserState unwind the element scope.
- ParserState shares its existing namespace tables and owned-byte Budget with
  the controller. Controller/global work remains outside ParserState.
- Replacement errors retain token-local byte offsets. The controller combines
  them with the real invocation/origin; it never maps them as physical UTF-16.

## Exact private API sketch

Add one closed leaf kind containing only Text, CDATA, Comment, and Processing
Instruction. It cannot express Start, End, DTD, or XML declaration.

Keep existing `process_token` and borrowed `process_dtd_start` signatures.
Add these private operations:

```text
process_dtd_leaf_owned(kind, raw, local_range) -> Result<Event<'static>, ReplacementError>

process_dtd_start_owned(raw, local_start, catalog)
    -> Result<Event<'static>, DtdTagError>

process_dtd_end_owned(raw, local_start)
    -> Result<Event<'static>, ReplacementError>

element_depth() -> usize
```

`process_dtd_leaf_owned` accepts only replacement input. It performs the same
grammar, phase, token, and event checks as the corresponding physical leaf,
then copies only its final fields through the shared Budget. Generated Text,
including empty or whitespace-only Text, rejects every non-content phase before
spending that budget or an event. As with `process_token`, the caller must first
pass bytes through the UTF-8/XML-character validation owned and work-charged by
LocalTokenizer; ParserState does not add an uncharged duplicate character scan.
The event counter changes after all checks and copies succeed.

`ReplacementError { kind, local_offset }` is a small private diagnostic, not a
new public error model. It prevents a generated UTF-8-local offset from entering
the physical TokenSpan mapper. `DtdTagError` already carries the equivalent
Input-versus-declaration/value distinction for generated Start tags.

`process_dtd_start_owned` uses `LiteralSource::Replacement`. After the shared
collector/preparer, a small checked ownership rebind moves already-owned names
and values into the generated publication path; a borrowed field is an internal
InvalidState, not an implicit uncharged copy. Publication resolves namespaces
once through the same planning/commit function. That function receives one
monomorphized namespace-output callback: the existing physical mode may borrow a
stable URI, while generated mode copies every non-static selected URI through
Budget. This removes the existing `'source: 'event` requirement only from the
shared core; the borrowed wrapper keeps it. Persistent stack/binding retention
uses a concrete Budget copy because generated raw storage is never stable and is
charged separately from the returned event copy. The result is explicitly
`Event<'static>` so callers may shorten it safely. There is no second
`PreparedOwnedTag` representation, lifetime assertion, or serialize/reparse.

`process_dtd_end_owned` is atomic. After the caller's EntityFrames guard, one
call validates syntax and the matching qualified name, copies the qualified name
and active namespace into a complete `Name<'static>`, then pops the exact current
frame, unwinds bindings, updates document phase, and commits the event count.
Any syntax, namespace, or copy failure leaves topology untouched. There is no
prepared token to replay against another parser or an ABA-equivalent stack.

Use a narrow phase-aware normalization entry beneath ParserState:

```text
normalize_replacement_text(raw, offset)
```

Physical delegates to today's normalization. Replacement omits only literal
EOL conversion. Reference parsing advances past the original spelling before
appending its scalar, so `&amp;T;` yields final `&T;` and cannot invoke `T`.
Unknown/general references are rejected defensively here because the content
adapter must schedule them before leaf preparation.

## Implementation steps

1. **Phase-aware leaf normalization.** Add the explicit Physical/Replacement
   branch in `escape.rs`/`parser.rs` and focused goldens for CR, CRLF,
   numeric/predefined spelling, `&amp;T;`, invalid references, XML-target PI, and
   exact owned-byte failures. Physical behavior must remain byte-for-byte equal.

2. **Owned generated leaves.** Add the closed leaf entry point and prove every
   returned Cow is Owned (except static constants), temporary raw buffers may be
   dropped immediately, failures do not increment events, and local Unicode
   offsets are preserved.

3. **Owned Start publication.** Preserve the fast borrowed method and share its
   collection, DTD preparation, namespace planning, and atomic commit. Test
   explicit/default attributes, inherited/generated namespaces, allocation
   failures at each copy boundary, and source-buffer destruction after return.

4. **Atomic End and controller handoff.** Add depth plus the owned End call.
   Exercise the order `frame guard -> atomic End`, namespace shadowing,
   mismatch, close/reopen guards, allocation failure before pop, and Reader-owned
   versus Stream-lent results. No public frontend wiring lands in this slice.

## Definition of done

All generated leaf/Start/End events survive release of their token, catalog
scratch, and controller-ready state without borrowing them. Physical Reader
events still borrow stable input and the ordinary no-DTD tag path is unchanged.
Every failure is typed at its truthful local coordinate, all allocations are
fallible and charged once per retained destination, and Start/End publication is
atomic. Focused debug/release tests, strict Clippy, fmt, and deliberate faults
for EOL phase, ownership, Start default preparation, and pre-End ordering pass
before any Reader/Stream activation review.
