# Next private streaming frontend seam

Reviewed private implementation outline, inspected
`e7e0b835e9320640effb3a1cbe0eb038a4268ecd`.
Stream/Push/Buffered/Reader/DOM/Decoder/Parser sources match root `7d66aa5`.
No public frontend or Event change is proposed for this slice.

## 1. One concrete owned session, preserving the ordinary engine

A private driver keeps `ordinary: Option<Stream>` and a fallibly reserved
`Vec<PrivateDtdSession>` of length zero or one. Only interception of Dtd reserves
the slot and consumes the ordinary Stream; do not put the 2240-byte content
controller inline in an ordinary parser. The session owns the SAME Decoder and
`phase: Option<Declarations { DtdController, ParserState<'static> } |
Content(ContentController)>`. Option is empty only during a poisoned ownership
move. Exactly one ParserState exists; no placeholder parser, duplicate Budget,
replacement Decoder or generic source/resolver framework. Public ordinary
Stream and its direct tokenizer path remain untouched in this private slice.
Start the private driver at document start, so it sees actual XML Declaration
standalone metadata. ParserState currently retains declaration_seen, encoding,
initial offset, scope, event count and Budget, but not standalone
(`src/parser.rs:273–290`, `src/reader.rs:53–57`). Capture that small value from the
real Declaration event before its callback; never reconstruct it from defaults
or accept a caller substitute for the document's declaration.

At the ordinary tokenizer's ready Dtd prefix, consume the owned Stream into the
new phase through a narrow consuming helper in `stream.rs`. Its Decoder stays
intact. Call the SAME ParserState.check_doctype_start before DTD processing.
Tokenizer has accepted exactly `<!DOCTYPE`, with a real TokenSpan; DtdController
requires the complete header prefix (`src/tokenizer.rs:325–326`,
`src/dtd_controller.rs:187–205`). Feed that fixed nine-byte prefix once from the
retained token into `new_at(encoding, span.source.start, span.decoded.start, ...)`.
Lend the still-owned old token's bytes for this call: existing DTD scanner/unit
work charges the prefix rescan explicitly. Do not feed it through Decoder,
increment transport/input bytes again, replay saved lookahead separately, or
discard its real starting coordinates.
Drop the old ordinary tokenizer after this bounded transfer; a fresh body
Tokenizer is allocation-free until used. This avoids keeping an unaccounted old
large token allocation during DTD scanning or misusing configure's BOM argument.

Use a private located callback enum `Xml(Event<'event>) | Dtd(DtdEvent<'event>)`.
The DTD branch lends current validated unit views, preserving LiteralSource,
Begin/Item/End ordering and real delimiter origins. For each ready DTD event,
check_event_limit and commit_checked_dtd_event exactly once before callback
publication, like ordinary events; do not silently omit these events from
max_events (`src/parser.rs:352–375`). Release the DTD unit only after callback
acceptance. After accepted End, into_body_state moves the SAME catalog, frames,
nonce, limits, counters and encoding; move the SAME parser into ContentController
(`src/dtd_controller.rs:306–345,406–434`). No early body decoding or catalog clone.

## 2. Preserve callback acknowledgement and the ownership budget

Extend ContentController.emit's callback to receive the retained
`self.parser.owned_budget()`, matching Stream.emit's existing third argument
(`src/stream.rs:77–83`, `src/dtd_controller/content.rs:195–207`). Physical events
still borrow source scratch for the callback, generated event fields own; no
Event-to-static conversion. Catalog work is already synchronized before this
callback through current_with_work. Callback copies, default/scope ownership and
DOM pool retention must debit this same Budget, never a reset budget.

The callback extension also refines the first content prototype's error policy.
Preserve the sink's supplied physical Error offset, as ordinary Stream.emit
does. An aborting caller supplies `Aborted` at origin.cancellation_offset;
allocation/Budget errors can instead anchor at origin.error_offset. Retain the
full source origin in either case. A replacement-local cancellation endpoint
belongs only to Aborted; do not label another sink's physical offset as a local
coordinate. Pin this distinction with a real same-Budget copy failure and an
abort at a different endpoint. The older `e7e0b835` callback contract remains its
own reviewed test boundary until this private refinement is integrated.

Push's feed_with/finish_with already provide the right sink shape
(`src/push.rs:130–172,190–215`). OwnedBuilder.consume accepts it directly and
already handles replacement/Mixed Text joins (`src/owned.rs:1127–1232`);
UTF-8 OwnedDocument currently chooses Reader, while decoded input chooses Push
(`src/owned.rs:493–518`). A private session harness can exercise the owned sink
after dual-cursor resume, without changing that production choice. DTD rows need an
ordered located unit collector alongside XML node consumption; OwnedBuilder's
current Event-only match cannot silently discard them or claim a completed
DOCTYPE DOM. Borrowed Document continues through Reader until its stable-source
seam and the planned ordered DOCTYPE storage are implemented
(`src/dom.rs:101–166`, `docs/plans/dtd-representation.md`).

## 3. Drive source progress separately from event delivery and transport

The decoder callback must return decoded bytes accepted by the active phase;
the outer advance returns original transport bytes accepted by Decoder. They
are distinct in UTF-16. Suspend Decoder for a ready logical event OR a generated
transition, and drain ready/Progress/local-frame work with zero input before
requesting more physical bytes. `position()` remains max(decoder transport
position, active primary physical position), even on failure. Replacement bytes
never advance it. Event origins, callback cancellation endpoints and local
replacement diagnostics remain separate (`src/encoding.rs:128–247`,
`src/stream.rs:25–28`, `src/dtd_controller/content.rs:177–207`).

Replace the DTD-aware private feed loop's `while !remaining.is_empty()` criterion
with ready/drain/NeedPhysicalInput/Complete outcomes; the ordinary production
loop stays unchanged. An empty GE and a withheld no-subset End can progress with
zero transport input. Only finite state-changing Progress may repeat. A zero
consumption NeedPhysicalInput stops the drain; no decoder callback may return
zero, unsuspended, while bytes remain. Commit accepted source prefixes before
exposing lexical errors. Session poison is installed before parsing/callbacks;
retain the first typed DTD/content/decoder failure and only map its truthful
physical anchor at the current public Error boundary. Push still checks the
entire offered chunk against received/max_input_bytes first and stores the
latest delivered origin before callback. Cancellation is terminal, releases no
pending prefix and schedules no GE afterward.

At final EOF, first drain generated work and Decoder.finish_step, including its
pending scalar/preamble. Only once Decoder reports no remaining carry may the
active phase receive physical EOF. DTD partial EOF is an error; accepted End
hands off before body EOF. ContentController already validates the original
parser on Complete, so do not install a fresh parser or duplicate publication.
Preserve successful repeated finish/EOF and sticky post-finish invalid feed.
Inherited max_physical_bytes/counters cover DOCTYPE plus body primary bytes;
pre-DTD prolog bytes remain in the frontend's independent whole-input transport
accounting. Do not relabel that DTD-relative total as the whole input limit.

## 4. Buffered lending and encoding resume are explicit follow-ups

BufferedReader currently releases the previous token, drains advance([]), then
fill_buf/consume only when needed; it commits position delta even on XML error
(`src/buffered.rs:87–181`). Keep Interrupted retry, one-shot original I/O failure,
poison on source panic and no source calls after terminal state. Generated or
held DTD work must drain before fill_buf; consume only new transport bytes,
never replacement output or bytes merely prefetched by BufRead.

The current callback-only ContentController cannot return a borrowed Event from
inside a closure to BufferedReader. Add a concrete one-publication lending seam:
prepare/lend one event with `Event<'_>` tied to the controller, remember that it
was published, and acknowledge/release on the next mutable call after the loan
ends. Callback emit wraps the same preparation and acknowledges only on success.
Do not cache a physical Event borrowing the controller's own scratch, parse twice,
or clone an event to escape the callback. Generated events can return their owned
payloads; the source/token stays paused until acknowledgement. Buffer delivery
counts as acceptance, so any later General scheduling follows that delivery.

First private callback integration may be UTF-8-only: reject UTF-16 DTD entry
explicitly; unchanged ordinary Stream still supports DTD-free UTF-16. Full private
streaming parity requires extending the checked input resume operation beyond
resume_utf8: carry the actual SourceEncoding and distinct inherited decoded and
physical cursors into a clean tokenizer, retain the SAME Decoder carry, and map
physical TokenSpans through that encoding. BOM is not a resume offset. Prove this
with real Decoder-fed UTF-16 LE/BE, BOM/no-BOM, non-BMP and all-split tests; never
label UTF-8 slices as transported UTF-16 in a frontend parity claim. Preserve the
UTF-16 scratch request bound min((raw_limit/2)*3, decoded_limit).

## Reader difference and activation boundary

Reader returns Event<'input>, independent of its mutable borrow
(`src/reader.rs:167–176`); BufferedReader returns Event<'_>. The borrowed DTD
adapter already supplies sealed stable_utf8/advance_borrowed and take_current,
which moves unit metadata while retaining a borrowed raw Cow. Stable Reader
body publication additionally needs exact same-window raw borrowing from the
sealed input span, ParserState<'input> retention, and owned generated fields.
Numeric/predefined raw spelling remains source-identical; the existing normalizer
chooses owned logical values. Current content current() only lends copied
scratch, and ContentController owns ParserState<'static>, so this is not already
a Reader fast path or an Event lifetime coercion. Keep it separate from the first
private owned-streaming session.

No ordinary parser fields, inner-loop reference flag, or public method changes
are needed to test this first private driver. Later activation still requires
reviewed public DTD events, both DOM ordered catalogs, Writer round-trip behavior,
private failure-context mapping, measured layouts/performance and complete
fuzz/conformance gates. Private end-to-end controls should first pin existing
preamble/event/Budget continuity, Begin/End cancellation, no suffix reads during
GE work, EOF ordering, buffered I/O poison, exact source consumption and quotas.

## First private callback checkpoint

The first implementation checkpoint is frozen at private integration commit
`7a650f846666692ac293dbab3d58761b3f8f3d7c`, tree
`968dd68daebf5fdb5e27f8b7ab61a9e4dd3b6dd3`. It implements the UTF-8 callback
slice described above without changing a public frontend or accepting public DTD
input. The driver keeps ordinary `Stream` intact until its actual DTD token,
fallibly reserves a length-zero/one session, and then moves the same Decoder,
ParserState, event counter, Budget, standalone value, catalog, frames and nonce.
Empty and nested generated work drains without consuming a physical suffix.
Cancellation, typed phase failures, allocation failure, callback unwind and
post-finish input are sticky.

The allocation boundary uses a private generic zero-sized policy. Normal
construction supplies `try_reserve_exact`; an injected rejecting policy is
driven through the same `advance -> start_dtd` route. Both specializations are
1056 bytes with alignment 8. This shape is a narrow failure-injection seam for
the current concrete `Vec` slot, not the allocator API design promised for the
public library.

Eight author groups and ten blind specification groups pass in debug and
release. They cover every UTF-8 byte split, DTD-free real UTF-16 parity, explicit
UTF-16 DTD rejection, event order and origins, shared quotas/Budget, standalone,
generated progress, EOF, callback errors and panic poisoning. The first holistic
pass caught a test-wiring defect because a helper was tested directly; after the
normal-route correction, reservation-bypass, standalone and all three poison
mutations fail. The final verdict is PASS with zero findings.

The combined private branch passes 990 tests with nine ignored, 552 release
library tests and 13 doctests. Strict all-target Clippy, formatting and byte-for-
byte source verification pass. Evidence is recorded in
`/private/tmp/yaxml-dtd-streaming-freeze.md`; the 345-file source manifest is
`/private/tmp/yaxml-dtd-streaming-freeze-source.tsv`.

This checkpoint deliberately leaves the lending publication seam, full UTF-16
DTD resume, BufferedReader I/O integration, stable Reader borrowing, ordered DOM
DTD storage, public error mapping and public activation for later slices. It adds
no fresh fuzz-coverage, XMLTS or performance claim.
