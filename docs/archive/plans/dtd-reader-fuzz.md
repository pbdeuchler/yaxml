# Private DTD Reader fuzz and coverage integration

Reviewed base `fc364ed9706e3f8ceccba034bae375bbaded7e73`, tree
`d4adffd398b3aa15936c01b0a473959517ecd521`. This plan adds no production
behavior and does not activate DTD support publicly. It describes the smallest
safe fuzz boundary for the private stable-input `DtdReader` and the coverage
accounting required before the private implementation can count toward the
project's 100% target-coverage goal.

The implementation must keep `unsafe_code = "forbid"`, add no dependency, and
continue to use only the existing `memchr` dependency. All generated work,
retained evidence, subprocess output and coverage artifacts remain bounded.

## Measured boundary and current gaps

The current replay path is `examples/fuzz_replay.rs` -> `fuzz/driver.rs`. The
deterministic smoke target is `tests/fuzz_smoke.rs`. Both compile the library as
an ordinary dependency. The DTD implementation is compiled only under the
library crate's `cfg(test)`: `reader::dtd`, the DTD controller/catalog/frame
modules, the DTD stream adapters and the local/reference/text tokenizers.
Consequently, no current fuzz entry point can construct `DtdReader`, and those
modules are absent from the instrumented fuzz-smoke binary rather than merely
showing zero execution.

The exact source inventory at the reviewed base is 32 Rust files under `src`.
The feedback campaign still freezes a 15-file manifest. It omits the two
ordinary modules `src/name.rs` and `src/origin.rs`, and it omits these 15
test-only DTD implementation files:

- `src/dtd.rs`
- `src/dtd_catalog.rs`
- `src/dtd_controller.rs`
- `src/dtd_controller/content.rs`
- `src/dtd_frames.rs`
- `src/dtd_reference.rs`
- `src/dtd_scan.rs`
- `src/dtd_tag.rs`
- `src/dtd_writer.rs`
- `src/reader/dtd.rs`
- `src/stream/dtd.rs`
- `src/stream/dtd_buffered.rs`
- `src/tokenizer/local.rs`
- `src/tokenizer/reference.rs`
- `src/tokenizer/text.rs`

Those 15 files contain 11,246 physical source lines at this checkpoint. That is
an inventory fact, not an LLVM region count. The exact campaign master-list gap
is 17 files: the 15 private files plus `name.rs` and `origin.rs`. A search of the
current fuzz driver, replay example and smoke target finds no `DtdReader`
invocation. No fresh LLVM export for this private target exists, so this plan
does not invent a region, function, line or branch total. The implementation
must establish those denominators from the pinned LLVM toolchain and retain the
raw export before making a percentage claim.

The security criterion has two separate parts:

1. **Target coverage:** every input-bearing subsystem has an executable,
   deterministic fuzz target with bounded work and an independent oracle.
   `DtdReader` currently fails this criterion.
2. **Measured execution coverage:** the pinned raw counts and gaps for that
   target are visible and eventually reach the stated 100% goal. A target's
   existence, a unit-test result, or a merged percentage from incompatible
   binaries does not establish this part.

## Boundary decision

Add a test-only driver at `tests/internal/dtd_reader_fuzz.rs`, included as a
sibling module under `reader` with `#[cfg(test)]`. Its core entry point is:

```rust,ignore
pub(crate) fn exercise(input: &[u8]);
```

The same file owns bounded model helpers, focused unit tests and one ignored
stdin entry test. The stdin test is the process boundary used by the feedback
campaign. Keeping the file below `tests/` has three useful properties: it can
access the private `reader::dtd::DtdReader`, it adds no public API or Cargo
feature, and LLVM reporting can exclude harness code while retaining all
production source rows.

Add only two narrow `#[cfg(test)]`, `pub(crate)` observability seams. They do not
exist in ordinary builds:

- `DtdReader::fuzz_snapshot()` returns a copyable private snapshot containing
  the phase/failure discriminants, physical position, last origin, finished
  state, controller counters and, when present, frame and text usage counters.
  Declaration and content phases must populate the same logical fields so a
  before/after comparison cannot accidentally reset accounting at handoff.
- `DtdUnit::raw_source()` returns a private `Borrowed | Owned` discriminant for
  its existing `Cow`; it does not expose or clone the field. The driver combines
  this with exact raw value and input-range checks. It does not claim that a
  lifetime-parameterized unit can outlive its source merely because its current
  raw field is owned.

Use public/private semantic values for every other oracle. Do not add getters
solely for an internal branch or mirror the entire controller state. Unit tests
must deliberately alter each snapshot field or raw-source result and prove the
corresponding oracle fails before these seams are accepted.

Build the process target under the pinned coverage compiler with this frozen
command shape:

```text
cargo test --lib --no-run --message-format=json-render-diagnostics
```

Parse bounded Cargo JSON lines and require exactly one current-package
`compiler-artifact` whose target is the library, whose profile has `test=true`,
and whose `executable` is present. Reject zero or multiple matches, non-UTF-8
paths, symlinks and an executable outside the configured target directory.
Record the package ID, target metadata, canonical executable path, length and
identity hash; recheck them after the run. Never scrape a human-readable hashed
filename.

Invoke that executable with the fixed, recorded libtest argument list:

```text
--exact reader::dtd_fuzz_tests::replay_stdin --ignored --test-threads=1
```

The final module path must be frozen by an actual process test. Define a
versioned `libtest-exact-v1` output grammar for the pinned rustc/libtest rather
than comparing bytes. After normalizing only CRLF to LF, accept exactly one
`running 1 test` line, exactly one `test <frozen-name> ... ok` line and exactly
one successful summary with `1 passed`, `0 failed`, `0 ignored` and `0
measured`. Parse the `filtered out` count as a bounded decimal and the elapsed
seconds as a bounded decimal with at most one fractional component; record both
but do not freeze their values. Permit only the blank lines in the pinned
grammar. Reject duplicate or out-of-order summaries, any other nonempty text,
captured test output, invalid/overflowing numbers, truncation, and any stderr.
Pin and record the rustc/libtest identity with the grammar version. Protocol
fixtures cover changing duration and filtered counts as successful cases, and
extra lines, duplicate summaries and the wrong test name as failures.

An example-included driver is the wrong boundary. Examples are separate crates
and cannot name this `pub(crate)`, `cfg(test)` implementation. Making that work
would require a hidden public API or a non-default feature whose sole purpose is
fuzz access. Duplicating `lib.rs` or module declarations in a custom example
would create a second crate root with ambiguous source identity and compilation
semantics. A harness under `src` would also add driver-only regions to the
production coverage denominator. If the exact libtest process protocol proves
unworkable, reconsider a sealed internal feature in a separate reviewed plan;
do not add one implicitly.

## Deterministic driver

The ignored stdin entry enforces the existing 1 MiB ceiling before any
unbounded allocation or copy: lock stdin, wrap it in `Read::take(MAX + 1)`, read
into a buffer whose maximum possible length is `MAX + 1`, reject the extra byte,
and only then call `exercise`. A short read is not EOF until the bounded reader
returns zero. Unit subprocess tests send exactly `MAX`, `MAX + 1` and a stream
split across several writes. The driver also defensively slices direct in-process
calls to at most `MAX`; it never allocates from an untrusted declared length.

Every accepted invocation performs fixed, finite work in this order:

1. Replay the supplied bytes directly through `DtdReader` with default limits.
   Fully consume success, error or EOF and check lifecycle invariants.
2. Use at most the first 64 bytes as control entropy to build one independently
   specified valid internal-DTD document, capped at 4 KiB source, 16 KiB
   logical expansion, 128 events, four declarations and eight body operations.
3. Run the valid document as a positive control, then run one selected
   truncation and one selected quota profile. Exhaustive prefix traversal is
   reserved for the small fixed smoke corpus.
4. Re-run the terminal operation to check sticky error or repeated EOF state.

Use fixed control-byte positions so corpus cases remain interpretable:

| Byte | Selection |
| ---: | --- |
| 0 | document family |
| 1 | declaration count, capped at four |
| 2 | body-operation count, capped at eight |
| 3 | entity/default/namespace shape |
| 4 | truncation family |
| 5 | quota axis |
| 6 | zero/below/exact/above boundary |
| 7 | retention/lifecycle oracle |
| 8..63 | bounded names, values and operation choices |

Short input supplies zero for missing controls. Names use a small independent
ASCII alphabet plus encoded byte values, rather than the production QName
validator. Values use a literal catalog with independently known XML escaping.
The builder must not parse its own output or call production normalization to
derive expected results.

### Structured valid documents

The generator emits a root declaration and a finite combination of:

- zero to four internal general entities, with references only to earlier
  declarations, including empty, text and `<n/>text` replacements;
- a root `ATTLIST` with CDATA and tokenized defaults;
- a fixed namespace/default-attribute template;
- an optional parameter-entity-created declaration; and
- zero to eight root body operations using physical text, entity references,
  child elements, comments and processing instructions.

It constructs an expected event transcript at the same time: DTD Begin/Item/End
families, XML event kinds, expanded names, values, attribute order, physical or
replacement origin class, physical ranges and entity-reference occurrence
identity. A small literal catalog covers semantic cases that should not be
derived from arbitrary bytes: standalone discrimination, unread external
declarations, repeated and nested occurrences, parameter-generated units,
namespace defaults and mixed physical/replacement text.

For an arbitrary testcase, choose one truncation at a delimiter, UTF-8 scalar,
reference or declaration boundary. The fixed smoke seeds exercise every prefix
of their small documents. Truncation expectations come from a literal table of
complete versus incomplete points; do not infer success or an exact error by
asking another yaxml frontend.

### Quota selection

Select one axis and one boundary per generated testcase. Pair every constrained
run with the default-limit valid control so a blanket rejection cannot pass.
The finite table must cover the public parser and decode limits and the private
session/controller limits for physical bytes, events, declarations/items,
catalog values, entity attempts/depth/work/expanded bytes, local/reference/text
scratch and owned bytes. Test zero, one below, exact and one above only where the
expected debit is independently hand-counted. Freeze the exact typed error,
offset/origin, test-only snapshot and failure atomicity before the API call.
Compare spent work only through `fuzz_snapshot`; do not infer private counters
from a public position. Never search production at runtime for the minimum
accepted limit or copy its accounting algorithm into the oracle.

## Runtime-observable oracles

The driver checks semantics and lifecycle, not only success agreement among
frontends that share parser code.

- Compare generated-document events to the independently built transcript,
  including complete ordered attributes, namespace URIs, DTD unit families and
  exact physical versus replacement origins.
- Retain returned events by value across later reads and across reader drop
  while the input remains alive. For physical borrowed fields, require that the
  pointer and range fall in the exact original occurrence. For generated and
  defaulted fields, require `Cow::Owned`, move the owned strings out, then drop
  events, reader and source before checking their values.
- Physical DTD-unit raw text must report `raw_source() == Borrowed` and point at
  the exact input occurrence. A parameter-generated unit must report `Owned`
  and equal the independently expected text. This is a storage-class oracle,
  not a claim that its enclosing lifetime can outlive the source. Repeated
  entity occurrences may have equal public spans but must compare unequal as
  complete `SourceOrigin` values.
- On the first error, snapshot the error, position, last origin and observable
  test state. Three further reads must return the same terminal result and an
  identical `fuzz_snapshot`, without publishing an event or advancing work. At
  EOF, three further reads return `None` with the same snapshot.
- On rejected quota or truncation cases, retain the prior event and verify it is
  still valid. Check that a paired fresh reader with the positive limits reaches
  the expected next event.

Rust's borrow checker prevents mutation or destruction of input while a
borrowed event remains live. Tests must not claim that behavior. Move only
owned fields before dropping their source. Panic/failpoint poison behavior stays
in focused unit tests; the fuzz driver treats an unwind as a failing testcase
rather than catching it and continuing.

## Corpus and mutation smoke

Add small checked-in seeds for:

- a no-subset document;
- internal entity plus attribute and namespace defaults;
- a parameter-generated declaration;
- nested, repeated and empty general entities;
- standalone/unread-external behavior;
- comments, processing instructions and mixed text; and
- malformed and deliberately truncated declarations/references.

Add compact binary control seeds selecting every generator family, quota axis,
truncation family and retention mode. At least one seed must select the largest
bounded nested occurrence shape. The smoke test replays the sorted corpus,
every prefix of only the small semantic fixtures, and a fixed bounded set of
byte replacement/insertion/deletion mutations. Preserve minimized crash inputs
with their selector interpretation in the corpus inventory.

## Coverage and campaign accounting

Keep the existing public fuzz-smoke profile and all private libtest profiles
separate. They are different binaries with different `cfg` source sets. Do not
add covered/total columns or merge raw profiles across different executable
identities. Profiles from several exact tests may be merged only when the
binary hash and complete LLVM object mapping are byte-identical.

Replace the stale 15-file campaign master list with the exact 32-file reviewed
`src/**/*.rs` inventory. Continue bounded recursive enumeration and reject
symlinks, missing files, unexpected Rust files and duplicate normalized relative
identities. Duplicate basenames are expected: `dtd.rs`, `reader/dtd.rs` and
`stream/dtd.rs` are distinct required paths. Preserve normalized source-root
relative identities such as `dtd_controller/content.rs`; never match a row by
basename.

Require LLVM report rows to use one frozen path mode. For an absolute row,
canonicalize the existing file, require it to remain below the canonical
`src` root, strip that root and compare the normalized relative path to the
master set. For a relative row, resolve it only against the recorded compiler
working directory/source root selected by the mode, reject `.`/`..` components,
then perform the same canonical containment and identity checks. Reject a row
outside the root, two rows mapping to one relative identity, or one expected
identity mapping to multiple absolute files. Parser fixtures must contain the
three distinct `dtd.rs` paths and prove that an omitted, duplicated or
basename-only row fails. Record full source identities even when a target does
not compile a file.

Define explicit target profiles and reviewed whole-file reachability manifests
rather than silently filtering missing rows:

- **Public replay:** the ordinary 17-file compile set, which is the existing 15
  paths plus `name.rs` and `origin.rs`. Record the 15 private paths as
  excluded-by-build for this target.
- **Stable DTD Reader:** the new byte driver and the stable-reader/common DTD
  source manifest. It includes the complete `stream/dtd.rs` file because the
  stable reader executes its shared `SessionError`/`SessionLimits` definitions
  and error conversions. Streaming-only regions in that file remain visible as
  stable-entry gaps and are closed by the streaming entry; they are not removed
  from the stable denominator. `stream/dtd_buffered.rs` remains outside the
  stable manifest and is owned by the buffered entry.
- **Streaming DTD Session:** a separate deterministic byte/chunk driver covering
  UTF-8 and UTF-16 transport, lending/callback behavior and generated scheduling.
- **DTD Buffered Reader:** a separate deterministic `BufRead` segmentation/error
  driver covering fill/consume, held/generated draining and I/O stickiness.
- **DTD Writer:** a separate semantic event driver for the private DTD writer
  state and its catalog/default validation.
- **Focused private guards:** a frozen list of exact unit tests for allocation,
  unwind, failpoint and invariant-only branches that cannot be selected by
  untrusted input. Record these as test coverage, not fuzz behavior.

The first implementation milestone adds the stable-reader driver. The other
entries may initially be named existing focused tests, but every input-bearing
streaming, buffered or writer subsystem must gain its own bounded byte/event
driver before target coverage is called complete. A stable-reader result never
stands in for them.

For each entry, the target-specific denominator is every LLVM instrumented
region/function/line/branch in its reviewed complete-file manifest. Do not
exclude individual production regions to improve a percentage. If a shared file
contains behavior reachable only from another entry, report that gap for the
first entry and close it in the identical-binary combined profile; do not label
the first entry 100%.

The **private DTD combined** denominator is the reviewed union of the complete
source manifests owned by the stable, streaming, buffered, writer and focused
guard entries. Compute that union before invoking LLVM and freeze it in the
report. The libtest binary also instruments ordinary modules such as `dom.rs`,
`owned.rs`, `push.rs` and `serialize.rs`; rows outside the DTD-manifest union are
retained in the raw export and 32-file source inventory but are not included in
the private-DTD percentage or zero-gap claim. Their absence from this denominator
is target attribution, not a coverage exclusion: the report lists each such
path under `outside private-DTD target union` with its identity.

A future **full cfg(test) crate** denominator may merge the DTD profiles with a
same-byte-identical-libtest ordinary driver matrix that reaches those ordinary
modules. Until that matrix exists, do not publish a complete-32-file unit-test
percentage. The existing public replay remains a separate executable and cannot
fill this denominator. Establish all actual rows with the pinned LLVM 22 report.
Any no-executable mapping exception within a target manifest names one reviewed
path and fails if that path later gains executable coverage; do not assume the
old `lib.rs` exception is the only one.

For each entry and the combined profile, freeze all four LLVM denominators from
the first valid report, require the exact expected rows, reject diagnostics,
duplicate relative identities and extra rows, and check source, binary and tool
identities before and after the campaign. Publish every raw profile, the merged
same-binary profile, report and export, plus focused tables for each reachability
manifest and the 15 DTD files. Report regions, functions, lines and instrumented
branches with raw covered/total/gap counts. Call private DTD coverage 100% only
when the frozen DTD-manifest union has no gap and every input-bearing subsystem
in that union has its own driver. This is not a complete-32-file crate claim.
Named focused tests may close failpoint/invariant regions but do not count as
fuzz target coverage.

CI adds separate instrumented exact stable-reader, streaming, buffered, writer
and focused-guard libtest invocations with distinct raw-profile paths, plus one
same-binary merged private-DTD report artifact. It does not replace the public
fuzz-smoke job. The feedback campaign first gains the reviewed
`libtest-exact-v1` protocol for the stable target and later reuses it for the
other exact entries. Fake subprocess tests reject a wrong test name, a skipped
or duplicate test, a malformed summary, unexpected stdout/stderr, a nonzero
exit, Cargo artifact ambiguity and source-row drift.

## Implementation burndown

1. **Private driver and model.** Add the `tests/internal` driver, bounded exact
   ignored stdin entry, two test-only observability seams, structured generator,
   transcript oracle and finite semantic corpus. Add happy/sad unit tests and
   deliberate oracle-negative tests. No non-test visibility change.
2. **Boundaries and lifecycle.** Add selected truncation, quota, retention,
   ownership, repeated EOF and sticky-error checks. Deliberately break origin
   identity, owned/borrowed classification, quota precedence and terminal
   stickiness; require named tests to fail and restore each fault.
3. **Replay and campaign protocol.** Parse Cargo JSON to select one test
   executable, implement and fault-test `libtest-exact-v1`, replace the stale
   inventory with the 32-file master set and collision-safe relative mapping,
   and update the fake helper. Keep all existing process/artifact bounds.
4. **Reachable target matrix.** Add bounded streaming, buffered and writer
   drivers and freeze the exact focused-guard list. Give every entry a complete
   source manifest and separate raw profile, including `stream/dtd.rs` in both
   the stable and streaming manifests. Freeze their union independently from the
   32-file inventory. Prove profiles merge only when the libtest executable and
   object mapping identities match.
5. **Measured coverage and closure.** Add the separate CI entries, run the
   pinned LLVM toolchain, retain raw profiles/exports/manifests, and triage every
   remaining stable-entry and combined DTD gap by exact source location. Extend
   the appropriate driver only for safely reachable substantive behavior.
   Finish with independent holistic review and deliberate production mutations.
   Leave public DTD activation and broad libxml2 conformance claims outside this
   milestone.

The milestone is complete only when the private target is callable through its
recorded process protocol, all fixed corpus and oracle tests pass in debug and
release, every input-bearing private DTD frontend has its own bounded target,
the campaign rejects every malformed protocol/inventory case, raw LLVM evidence
names the exact source and binary identities, and remaining numerical coverage
gaps are reported without exclusions or inflated claims.
