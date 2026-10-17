# Private stable-input DTD Reader

Inspected base `992c177e8fd93650087e713f3ff99d13404c22fa`, tree
`27095ff58d0d353450f35b5b251875c7164bd2f7`. This is a plan, with no source changes
or public activation. Keep Reader's complete UTF-8 input contract and explicit
UTF-16 rejection. StreamingSession and Buffered UTF-16 behavior stay unchanged.

## Ownership contract and smallest API

Add a test-only private `reader::dtd::DtdReader<'input>`:

```text
new(input: &'input [u8], options: SessionLimits) -> Result<Self, SessionError>
with_limits(input: &'input [u8], limits: Limits, options: SessionLimits) -> Result<Self, SessionError>
with_decode_limits(input: &'input [u8], limits: Limits, decode: DecodeLimits,
                   options: SessionLimits) -> Result<Self, SessionError>
read_event(&mut self) -> Result<Option<(ReaderEvent<'input>, SourceOrigin)>, SessionError>
position(&self) -> usize
event_origin(&self) -> Option<SourceOrigin>
event_range(&self) -> Option<Range<usize>>
into_inner(self) -> &'input [u8]

ReaderEvent<'input> = Xml(Event<'input>) | Dtd(TakenDtdEvent<'input>)
```

The result lifetime is the caller's immutable input lifetime. Callers can hold
several events, advance, and destroy the reader while keeping those events.
Unchanged physical QNames and leaf text borrow original input. Normalized values,
defaults and generated values own their necessary data, charged to the same
ParserState Budget. Namespace metadata also follows the existing parser's
ownership rules; this is not a promise that every field of a physical event is
borrowed. XML Declaration's borrowed version/encoding remain valid directly;
there is no event-to-static conversion and no Declaration representation change.

An ordinary `DtdEvent::Item` contains a reference to a DtdUnit. Returning it with
`'input` would require retaining metadata forever or a self-reference. Use the
existing by-value TakenDtdEvent instead (`src/dtd_controller.rs:101–117,306–326`).
Its physical DtdUnit owns token metadata while its raw Cow borrows exact input.
Generated units own raw text too. Typed unit views borrow the returned event,
not the reader; they need not outlive that event. No cached self-borrowing Event,
clone, reparse or permanent declaration-event arena is needed.

Successful read_event is the delivery boundary: parse once, move the result out,
and release only storage the result does not borrow. No explicit public-style
prepare/lend/acknowledge is needed for this stable API. Releasing a prefix does
not enter a GE; lookup/frame entry happens on the next read. If a DOM consumer
rejects that returned prefix and stops, no inclusion side effects occur. Taken
DTD End leaves a completed declaration phase until the next read performs the
fallible body handoff, so a constructor failure cannot hide a deliverable End.

## Concrete reuse and missing seams

Public Reader currently validates the complete UTF-8 input, XML characters, BOM,
whole-input quota and optional decode quota at construction
(`src/reader.rs:101–148`). It returns Event<'input> independently of &mut self
(`:167–179`), using ParserState<'input> and borrowing retention. Preserve that
constructor/error boundary rather than decoding or copying the document.

1. **Intercept the checked DOCTYPE boundary and move the existing parser.**
   Reuse Reader's lexical/semantic loop through a concretely specialized private
   entry that can return `Xml(Event<'input>) | Doctype(start) | Complete`.
   The public path remains the ordinary specialization; no runtime DTD-mode flag
   or second lexical implementation. Stop before process_token rejects Dtd
   (`src/reader.rs:211–237`), call the same check_doctype_start, and enforce the
   private nine-byte prefix token ceiling. Keep the ordinary consumed-error
   cursor rules unchanged. A narrow consuming handoff moves the SAME
   ParserState<'input>, input and relevant configuration; no replacement parser
   or budget is constructed. Capture standalone from the actual earlier XML
   Declaration. Reserve the optional zero/one session slot before the ownership
   move. The stable DTD controller starts at the original '<' and charges its
   own prefix scan once; no Decoder or transport replay exists in this path.

2. **Complete stable publication in the shared content core.**
   Make the existing ContentController a lifetime-parameterized concrete core,
   retaining its streaming alias at `'static`. Its ParserState, TextCoordinator,
   paused Input lease and released Local lease carry `'input`; active IDs,
   scheduling, frames and accounting remain the same. Do not create a second GE
   controller. Add a sealed stable-UTF8 constructor consuming DtdBodyState plus
   ParserState<'input>, resuming the checked equal cursor pair and encoding, then
   adopting TextSourceLease::stable_utf8 (`src/tokenizer/text.rs:247–282`).

   Add one stable-current accessor to the lease/coordinator: it returns the
   token kind, an exact `&'input str` and an owned TokenSpan value. Map the ready
   decoded range through the immutable stored window with checked subtraction,
   bounds, UTF-8 boundaries and UTF8/equal-coordinate checks. It cannot accept a
   caller-provided source. TokenSpan is copied geometry, not an offset map or
   input copy. Existing current/current_with_work lend scratch and cannot supply
   this lifetime (`src/tokenizer/text.rs:326–347,477–492`).

   Stable advance must also respect the controller's remaining physical quota.
   Add a bounded stable-window advance operation (maximum accepted UTF-8 bytes,
   chosen from the existing remaining physical limit), rather than passing a
   foreign slice to streaming advance or scanning the entire suffix first.
   It reads only its own stored window and preserves partial-window progress,
   global pre-caps, EOF seals and the existing reference boundary protocol.

   Add a stable take-event operation sharing prepare_publication and the same
   parser publication code with lending (`src/dtd_controller/content.rs:661–902`).
   Physical parsing uses the exact external raw slice and borrowed retention;
   generated parsing uses existing owned start/leaf/end methods. Event<'static>
   from those owned methods can shorten to Event<'input> without conversion.
   Run the same pre-End guard, catalog cap/synchronization, origin mapping,
   normalization and event accounting exactly once. After success, acknowledge
   the parked token before returning the independently owned/borrowed event.
   Do not call lend and then parse again to replace its fields.

3. **Compose the Reader driver with existing DTD units and handoff.**
   Keep `ordinary: Option<Reader<'input>>` plus a fallibly reserved zero/one
   `Vec<ReaderDtdSession<'input>>`; the large declaration/content phase stays
   out of the ordinary inline state. Declarations use the existing
   BorrowedDtdController::stable_utf8 and advance_borrowed exclusively.
   Every Begin/Item/End checks and commits the SAME parser event counter before
   take_current. Physical raw unit promotion is already avoided by
   stable_physical_raw (`src/dtd_controller.rs:590–610,935–960`).
   After returned End, into_body_state moves the SAME catalog, EntityFrames,
   nonce, native limits and spent counters (`:406–434`). Drop the declaration
   scanners/scratch; move the same parser into the shared stable content core.
   Preserve the inherited global work ceiling, including any lower Text limit.
   Drain finite generated/return transitions internally; no fabricated progress,
   empty Text for empty GEs, suffix consumption ahead of a GE or occurrence arena.
   Keep typed sticky SessionError and last successful origin through errors/EOF.
   A preoperation poison marker resolves against current position after unwind.
   Repeated EOF returns None. Drop and into_inner do not drain or validate; the
   returned source is the full original slice, with position available separately.

4. **Freeze and verify before considering activation.**
   First checkpoint: compiling private types, narrow stable raw accessor,
   physical/generated take-event and one real DTD-to-body fixture. Then independent
   lifecycle/ownership/quota tests and existing streaming suites in debug/release.
   Measure actual layouts and allocation counters, run ordinary Reader behavior
   checks, and leave comparative performance/conformance/fuzz gates to the root's
   separately coordinated frozen integration. No public Reader/DOM/FFI mapping is
   enabled by passing these private tests.

## Costs and later mapping

This avoids whole decoded/expanded input and full-subset storage. The optional
session Vec adds three machine words and allocates one out-of-line phase slot
only when DTD entry succeeds; catalog, metadata and bounded scanner buffers have
separate fallible allocations. Generic lifetimes alone add no stored pointers,
but the wrapper retains configuration and source references: measure Reader,
DtdReader, phase, content core and returned envelope rather than assuming their
layouts. Moving DtdUnit's metadata Vec transfers capacity without cloning it;
a caller retaining all events chooses to retain that output storage.

Stable publication is zero-copy for unchanged returned physical strings, not
zero-copy lexing. The existing DTD scanner still copies a bounded unit, and the
existing Text tokenizer copies physical tokens into bounded scratch even in
stable mode. Keep both tokenizer slots plus reference scratch covered by the
reviewed aggregate reservation, including paused Input. Borrowing event payloads
and persistent physical parser strings reduces ownership charges relative to
streaming, but does not remove scan/reference work or metadata allocations.
Record these differences in counter tests; do not claim a preserved Reader
performance result without measurement.

A future public event change can map this by-value DTD unit envelope into the
reviewed DTD API without exposing scanner state. Borrowed DOM can retain
Event<'input> and moved units directly; its text coalescing and ordered DTD storage
remain separate activation work. Language bindings must retain their input owner
for borrowed payloads or copy under their own explicit policy. No stable C layout,
public raw occurrence ID, unsafe FFI trampoline or library-global owner is added.

## Targeted independent tests

- Compile-and-run retention: hold XML and DTD events across later reads, move and
  drop Reader, then inspect them. Verify original input pointer/range identity for
  physical element/attribute QNames, unchanged leaves, Declaration and unit raw;
  verify owned payloads for generated names/PI, normalized strings and defaults.
- Literal UTF8/BOM/Unicode prolog plus all DTD unit families, internal PE-created
  declarations, nested/repeated/empty GE, defaulted/tokenized/namespace attributes.
  Compare semantic rows and origins with the reviewed streaming driver; pin the
  intentional borrowed-versus-owned copy/work deltas rather than equal totals.
- Prefix before missing/cyclic GE: first read returns the prefix, counters show no
  frame entry until the next read, and dropping there performs none. Test pre-End
  frame failure and raw-spelling numeric/predefined references without rescan.
- Exact/one-short input, decode/token, event, owned, DTD storage/physical/work,
  reference attempt/depth and aggregate scratch limits. Include pre-DTD XML
  Declaration/events and parser budget use; no counter reset at handoff. Bound
  stable advance at the first excluded scalar, with exact sticky position/origin.
- Missing/duplicate/misplaced DTD, malformed units, unavailable defaults/entities,
  input/window/boundary mismatch and EOF mid-unit/token. Last origin remains the
  delivered one on errors/EOF; repeated error and EOF calls do no further work.
- Constructor parity with current Reader, including UTF16 LE/BE BOM and detected
  no-BOM rejection, malformed UTF8/XML characters and decode ceilings. Existing
  UTF16 StreamingSession/Buffered tests must remain unchanged and green.
- Deliberate faults: use scratch instead of input, own all physical values, release
  prefix and enter GE early, omit DTD event charges, recreate frames at handoff,
  ignore bounded stable quota, reparse publication, or return a unit view backed
  by controller storage. Require runtime or compile-time lifetime failures as
  appropriate, retain evidence, restore source, and rerun green checks.
