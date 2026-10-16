# Buffered pull input plan

This plan freezes the next P1 milestone after UTF-16 input. It adds a bounded,
lending pull reader over caller-provided buffered I/O. It does not add a
libxml2-shaped cursor facade, parser options, async I/O, DTD processing, subtree
expansion, or source loading.

The implementation must reuse the existing decoder, tokenizer, and semantic
state. Pull is a normal suspension point in that pipeline, not cancellation,
EOF, or a recoverable parser error.

## Why this is next

Buffered pull closes the remaining ordinary streaming gap and makes
time-to-first-event and retained-memory comparisons meaningful. A caller cannot
construct it from `PushParser`: callback `Break` deliberately produces terminal
`Aborted`, and callback events cannot escape their borrowing lifetime.

Complete-document serialization follows this milestone. Mutation follows a
serialization contract because the immutable DOM currently relies on contiguous
preorder storage and stable subtree ranges. Foreign handle design follows
mutation identity and generation rules.

## Reference structure and deliberate differences

- [libxml2 2.15.4 `xmlTextReaderRead`][libxml2-reader] advances a current-node cursor and reports
  success, EOF, or failure. Its implementation incrementally feeds the push
  parser, constructs transient tree nodes, and frees unpreserved nodes. Its
  deprecated byte-consumed query is read-ahead-dependent. Yaxml will expose
  typed logical events and exact event ranges instead of building a transient
  DOM or copying the broad cursor/property surface.
- [quick-xml's buffered reader][quick-xml-reader] returns events borrowing caller-reused storage and
  therefore does not implement `Iterator`. Yaxml will lift that lending shape,
  but retain namespace resolution, complete XML validation, stable text
  segmentation, and explicit limits in the shared engine.
- [roxmltree][roxmltree-parse] and the inspected [xmloxide reader][xmloxide]
  operate over complete retained
  input. Their tree/cursor APIs inform navigation terminology, not bounded I/O
  storage.

The first API is deliberately smaller than `xmlTextReader`. Attribute cursor
navigation, namespace lookup, whitespace node classification, declaration
metadata getters, subtree skip/expand/preserve, validation, reset/reuse, URL
loading, and mutable parser properties remain catalogued follow-up operations.

[libxml2-reader]: https://github.com/GNOME/libxml2/blob/v2.15.4/xmlreader.c#L1114
[quick-xml-reader]: https://docs.rs/quick-xml/0.42.0/quick_xml/reader/struct.Reader.html
[roxmltree-parse]: https://github.com/RazrFalcon/roxmltree/blob/v0.20.0/src/parse.rs
[xmloxide]: https://github.com/jonwiggins/xmloxide

## Public API

```rust
pub struct BufferedReader<R: std::io::BufRead> { /* private */ }

impl<R: std::io::BufRead> BufferedReader<R> {
    pub fn new(input: R) -> Self;
    pub fn with_limits(input: R, limits: Limits) -> Self;
    pub fn with_decode_limits(
        input: R,
        limits: Limits,
        decode_limits: DecodeLimits,
    ) -> Self;

    pub fn read_event(
        &mut self,
    ) -> Result<Option<(Event<'_>, std::ops::Range<usize>)>, ReadError>;

    pub fn position(&self) -> usize;
    pub fn get_ref(&self) -> &R;
    pub fn into_inner(self) -> R;
}
```

`read_event` is a lending operation. The returned event can borrow the reader's
completed token, so another mutable operation cannot occur while that event is
live. It is not an `Iterator`. Returning the raw source range with the event
avoids requiring a second reader borrow merely to locate the current event.

The constructor performs no I/O. Callers with only `Read` wrap it in
`std::io::BufReader`; yaxml does not add a second internal input buffer. There is
no mutable source accessor because out-of-band reads would invalidate positions.
`into_inner` returns the buffered source at the parser-consumed position. That
position can include the `<` lookahead which ended a text event, so it need not
equal the latest event range end.

## Shared resumable pipeline

### Completed-token lifetime

The tokenizer retains one complete decoded token until the next call releases
it. `ParserState` processes that token exactly once. A returned `Event<'_>` can
borrow the retained bytes while owning only the normalization and namespace
values already required by streaming semantics.

The next `read_event` call releases the previous token before accepting more
input. Push parsing drives the same token-ready state immediately through its
callback and then releases it. No event queue, owned event mirror, transient DOM,
or full decoded source is introduced.

Top-level whitespace is a complete validated token which produces no public
event. A private non-event classification lets the driver release it and
continue before constructing the final lending return. This is shared grammar,
not a second whitespace parser.

### Text lookahead

When `<` terminates text, its decoded scalar and original source bytes are
consumed, but it belongs to the next markup token. Retain it as constant pending
lookahead outside the completed text buffer. The text event range ends before
it; `position()` includes it. On the next call, the tokenizer starts the markup
token from that saved delimiter without reading it twice.

### Decoder suspension

Decoder stepping reports the exact original-byte prefix consumed and one of:

- more input required;
- a complete token is ready;
- a structured failure occurred.

UTF-8 retains bulk delimiter scanning. UTF-16 retains its existing preamble,
partial code unit, and high-surrogate carry. If token recognition stops within
the UTF-8 bytes of one decoded scalar, at most three remaining scalar bytes are
kept as constant carry. This case matters for malformed non-ASCII markup; the
current `Syntax` classification and original offset must not regress.

The decoded-total limit is checked and charged for a complete UTF-16 scalar
before the tokenizer sees its first UTF-8 byte. Scanner suspension cannot change
`DecodedBytes` versus malformed-markup error precedence.

Internal suspension is not `ErrorKind::Aborted`. Decoder totals, source
positions, token ranges, and semantic event accounting advance exactly once
before suspension.

### Buffered I/O

Each step borrows `fill_buf()`, processes only a prefix, releases that borrow,
then calls `consume(exact_raw_count)`. It never consumes beyond the returned
slice and never processes the entire available buffer merely because it is
present. An unread suffix stays in the caller's buffer.

`Interrupted` is retried. Empty `fill_buf()` is final EOF and triggers decoder
finish, pending-token processing, and document-closure validation. Returning the
root end event does not establish successful EOF; a later call must still detect
trailing input or I/O failure.

## Positions, limits, and failures

- Event ranges use absolute bytes in the original encoded source.
- `position()` counts raw bytes accepted by the decoder, including BOM, partial
  carry, and text lookahead. It excludes bytes merely prefetched by `R`.
- `max_input_bytes` counts parser-consumed raw bytes. Pull does not reject an
  unread `fill_buf()` suffix. This intentionally differs from
  `PushParser::feed`, which preflights the caller's complete supplied chunk.
- At the exact `max_input_bytes` boundary, the driver calls `fill_buf()` once to
  distinguish EOF from excess input or an I/O error. Empty input finalizes the
  document. A nonempty slice returns `InputBytes` at offset 0 without consuming
  the first excess byte. An I/O error is returned unchanged. These rules apply
  whether the source already buffered a suffix or must refill.
- Raw/decoded token ceilings, decoded-total bytes, depth, attributes, events,
  and the shared cumulative owned-string budget retain their existing inclusive
  meanings. Suspension neither refreshes nor double-charges a budget.
- Scratch capacity stays bounded by the existing raw/decoded token relationship.
  Caller-owned buffering is outside yaxml's allocation accounting.
- Successful EOF repeats `Ok(None)` without more I/O.
- XML failures remain sticky and repeat the same copyable `Error`.
- The first I/O failure returns its original `std::io::Error`. Later calls return
  XML `InvalidState` without touching the source; `ReadError::Io` is not cloneable.
- The reader is poisoned before source operations and semantic mutation. If a
  caller catches a panic from `fill_buf`, `consume`, allocation, or callback-free
  internal processing, later calls return `InvalidState` without I/O.
- Dropping the reader or extracting its input before EOF does not drain or
  validate the remaining document.
- Positions start at zero relative to the source state supplied to the
  constructor. A generic `BufRead` cannot recover an underlying file's initial
  absolute offset.

## Implementation boundaries

- `src/tokenizer.rs`: explicit need-input/token-ready progress, retained complete
  token, release operation, exact decoded consumption, pending text delimiter.
- `src/encoding.rs`: exact raw consumption, normal suspension, pending decoded
  scalar suffix, unchanged whole-stream limit/error ordering.
- New private `src/stream.rs`: shared decoder/tokenizer/`ParserState` driver,
  token validation, source mapping, and non-event classification.
- `src/push.rs`: use the shared driver while preserving whole-chunk preflight,
  callback cancellation, callback panic poisoning, `read_from`, and terminal
  behavior.
- New `src/buffered.rs`: public `BufRead` owner, lifecycle, I/O errors, and lending
  `read_event`.
- `src/parser.rs`: only a narrow shared non-event helper if the driver needs it.
- `src/lib.rs`: export `BufferedReader`.

Do not make public events own all strings, change `Name` fields to `Cow`, add a
generic parser-engine trait, or duplicate tokenizer/parser logic. A concrete
shared engine is the smallest abstraction with two real consumers.

## Test-first acceptance matrix

Public tests must cover both successful and failing permutations, table-driven
where cases share a shape.

1. UTF-8, UTF-16LE, and UTF-16BE produce exactly the push reader's typed values
   and raw ranges for buffer capacities 1, 2, 3, 7, 64, 8,192, and 65,536.
2. Every byte boundary through BOMs, UTF-8 scalars, UTF-16 units/surrogates,
   delimiters, references, declarations, comments, CDATA, PI, tags, and quoted
   attributes preserves events and positions.
3. Text followed by `<`, consecutive tags, empty constructs, prolog/epilog
   whitespace, and EOF after partial `<` neither duplicate nor lose a byte or
   event.
4. Malformed Unicode, declarations, names, references, end tags, non-ASCII
   invalid markup, odd UTF-16 EOF, and truncated tokens retain exact error kinds,
   offsets, terminal state, and range/progress contracts.
5. Every limit is exercised below, exactly at, and above its boundary, including
   BOM exclusion from decoded counts, ignored whitespace, lookahead, and shared
   normalization/retention accounting.
6. Custom buffered sources exercise `Interrupted`, original I/O errors, late
   failures after a valid root event, `fill_buf`/`consume` panics, exact consume
   ledgers, unread suffixes, repeated EOF/error, and no source access after a
   terminal result. Mid-text, quoted-markup, and split-surrogate failures emit no
   incomplete event, preserve the consumed position, return the original I/O
   error once, and then return `InvalidState` without another source operation.
   Prepositioned sources confirm that ranges remain relative to construction.
7. Compile-fail documentation demonstrates that a borrowed event prevents
   advancing the reader. Runtime tests demonstrate that an explicit caller copy
   survives advancement.
8. Existing slice, push, owned, writer, oracle, and UTF-16 suites remain green.

Before implementation is accepted, predict and execute mutations which consume
one extra raw byte at suspension, lose text lookahead, lose UTF-16 scalar carry,
refresh a limit on resume, treat suspension as EOF, reset namespace scope, or
perform I/O after failure. Both ordinary and fuzz tests must catch each crucial
behavior assigned to them. The scalar-carry mutation needs a private
decoder/engine suspension test: the public parser makes malformed non-ASCII
markup terminal before later suffix loss is observable.

## Fuzz and performance evidence

Extend the dependency-free driver with pull operation schedules: advance,
inspect typed values and range, copy selected payloads, stop early, repeat EOF or
failure, and extract the source. Use bounded custom `BufRead` implementations to
vary visible capacity and inject lifecycle faults. Compare successful events and
original-byte spans with live push parsing and the independent UTF-16 reference;
compare malformed categories/offsets under the documented precedence rules.
Every bounded valid case must succeed, both implementations must reach EOF after
the same complete event sequence, and exact-limit cases need independent golden
results. Add sensitivity mutations for blanket quota rejection and double
charging after suspension.

The target inventory, corpus, and coverage artifact change in the same milestone.
Coverage remains raw production coverage with no exclusions added for new code.

Add benchmark cases for first-event latency, total event throughput, bytes
consumed on early stop, and retained scratch high-water behavior. Cover tiny
elements, attributes, large text/comments/CDATA, namespaces, normalization,
UTF-8 and both UTF-16 orders. Use total-size series and buffer capacities
1/2/3/7/64/8192/65536. At fixed token size, depth, and live namespace scope,
retained parser state must plateau as total sibling count grows. Measure retained
entry and capacity counts for tokenizer, namespace, and structural state, or use
fresh-process memory series when an internal count is not meaningful. Every
sample must consume the complete input. A deliberate mutation which retains
completed namespace scopes must make this check fail.

Record push throughput before and after the shared-engine refactor. Do not claim
a libxml2 performance win until a matched 2.15.4 callback-I/O harness consumes
the same logical names, namespaces, attributes, and values with equivalent
options. Report yaxml's exact-range bookkeeping as extra work, and validate those
ranges separately against the independent reference: `xmlTextReader` exposes no
equivalent exact event-range API. `xmlReaderForMemory` and xmllint timing are not
equivalent baselines.

## Review gate

Run all targets, doctests, formatting, strict all-target Clippy, dependency and
unsafe audits, focused fuzz replay, raw fuzz/ordinary coverage, and the benchmark
size-series smoke checks. A reviewer independent of production implementation
must inspect every changed line, write mutation predictions before executing
them, restore after each mutation, and issue a severity-classified PASS/FAIL
report before local main is fast-forwarded.
