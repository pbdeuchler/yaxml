# Private DTD lending publication

This extends the reviewed owned streaming session privately. It does not activate
Reader, Push, BufferedReader, DOM or Writer APIs. The same decoder, ParserState,
Budget, catalog, frames and transport cursor remain owned by the session.

1. `StreamingSession::prepare(input)` scans/drains to `SessionStep { consumed,
   outcome: Ready | NeedInput | Complete }`. Ready parks exactly one lexical
   token or logical DTD unit. Repeated prepare at Ready returns consumed zero
   without new work. `finish_step()` seals transport EOF and uses the same
   result. Non-events are parsed once and drained internally.
2. `lend()` returns `(LocatedEvent<'_>, SourceOrigin)` directly. It performs
   parser publication once; returned physical fields borrow paused tokenizer
   storage, while replacement fields may own their payloads. No Event is cached
   inside its own source, cloned, or reparsed. A private helper also lends the
   same Budget to callback driving. Semantic token errors can surface here.
3. `acknowledge()` releases the token/unit after its event borrow ends. Until
   then, preparation/final EOF and a second lend reject with sticky InvalidState;
   acknowledge before lend also rejects. Prefix release and subsequent General
   reference lookup/frame entry remain after acknowledgment. DTD End handoff is
   likewise deferred until acknowledgment. Explicit acknowledgment confirms
   successful delivery; merely dropping the event does not release its source.
4. Existing callback advance/finish loop over this same preparation, lending and
   acknowledgment. Callback failure preserves its typed offset and original
   provenance, without release; unwind poisoning remains centralized and is
   resolved against truthful transport progress. ContentController uses the same
   one-shot lending/publication and acknowledgment split, retaining its existing
   pre-End guards and monotone work/copy accounting.

Tests cover callback/lending semantic and origin parity (including actual UTF-16),
source pause/one-shot event budgets, prefix cancellation and entity scheduling,
DTD End ordering, EOF/zero-input draining, lifecycle misuse and sticky failures.
Deliberate production faults must fail runtime assertions, then be restored.

The safe frontend should acknowledge automatically at the beginning of the next
mutable read, after the prior loan has ended. It must not expose acknowledgment
as a public user obligation or mutate the owner from a drop guard. Buffered I/O
must commit the returned transport-byte count before lending the event, drain
generated and held DTD work before `fill_buf`, and never count prefetched bytes.
Stable `Reader` publication remains a separate immutable-input design. Language
bindings should copy event data during a callback or use generation-checked
opaque views; Rust references are not an FFI contract.
