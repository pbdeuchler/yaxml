# Fuzzing and replay

The current harness is safe Rust with no added Cargo dependencies. It provides deterministic mutation smoke tests and a process-per-testcase replay entry point. It does not yet provide a validated coverage-guided campaign or 100% execution coverage. The release policy and coverage definitions are in [the security plan](../docs/security.md).

Run the smoke tests and replay a testcase:

```sh
cargo test --test fuzz_smoke
cargo run --release --example fuzz_replay -- fuzz/corpus/minimal.xml
cargo run --release --example fuzz_replay < testcase.xml
cargo run --release --example fuzz_replay -- --libxml2 /path/to/pinned/xmllint fuzz/corpus/minimal.xml
```

The example accepts each filename as a separate testcase, or one testcase from stdin when there are no arguments. It caps testcase size at one MiB before parsing. Syntax rejection is an expected result and exits successfully; an invariant violation panics. File/read errors and oversized input fail the process. Keep that distinction when configuring an external runner.

## Current target inventory

| Implemented input surface | Exercise path | Oracle or invariant |
| --- | --- | --- |
| `Reader::new`, `with_limits`, `read_event`, `position`, `event_range` | `driver::exercise`, `exercise_with_limits` | Bounded forward progress; terminal EOF/error; valid error offsets and event spans |
| `Document::parse_bytes` | `driver::exercise` | Acceptance matches complete reader consumption; structural invariants |
| `Document::parse` | `driver::exercise`, for valid UTF-8 | Acceptance matches byte input |
| `Document::with_limits` | Derived profiles and each explicit resource ceiling | Policy acceptance matches the reader; each limit has a known rejection case |
| `OwnedDocument::parse`, `parse_bytes`, `with_limits`, `with_decode_limits` | `owned::exercise` and `encoding::exercise`, default and derived profiles | Temporary source overwritten/dropped before views; complete values/topology/spans match borrowing DOM or independently decoded reference; aggregate and decoded ceilings |
| Owned tree views and navigation | Successful owned trees | Every kind, declaration, name, attribute, PI target, payload and navigation method compared; exact iterator lengths; valid lookup and foreign-identity rejection |
| `OwnedDocument::edit`, `Editor::document`, `set_node_value`, `set_attribute_value`, owned handle lookup | `mutation::exercise`, independent operation and quota selectors | Model predicts exact outcomes before production; complete state after success/failure; foreign handles, moves, persistent quotas, recovery and strict model-based serialization |
| `PushParser::new`, `default`, `with_limits`, `feed`, `finish`, `position`, `event_range` | Input-derived chunk and lifecycle schedules, capped at 4096 chunks | Acceptance and exact typed event/span parity with slice reader; monotonic consumed position; repeated EOF; sticky errors after feeding final EOF |
| `PushParser::read_from` | Cursor plus bounded scheduled and invalid-count readers | Same events/spans; one Interrupted retry; original injected I/O error kind/message; cancellation stops reads; terminal parser states perform no I/O; oversized reported counts are rejected |
| DOM traversal and node access | Successful document checks | Unique IDs; preorder traversal; parent/child/sibling agreement; nested source ranges; subtree sizes |
| `Writer::new`, `with_limits`, `write_event`, `finish`, `position`, `into_inner` | Accepted reader events, generated caller-created event sequences and scheduled sinks | Exact typed round trips; complete output reparses; malformed operations emit no bytes; finite encoded bounds; confirmed partial positions; sticky failure; no I/O after failure or repeated successful finish |
| `Writer::write_document`, `write_owned_document` | `serialize::borrowed`, `serialize::owned`, UTF-8 and independently decoded UTF-16 references | Complete tree/declaration comparison; exact output and quota goldens; fresh-writer requirement; short/interrupted/invalid writes, prefix and flush faults; successful completion and terminal misuse |
| XML normalization and references | Reader/DOM corpora and generated documents | Exercised indirectly; semantic golden tests are also required in the ordinary test suite |
| `PushParser::with_decode_limits`, UTF-8/UTF-16 detection and decoding | `encoding::exercise`, `exercise_decode_limits`, encoded fixtures/mutations | Standard-library UTF-16 reference, declaration agreement, exact typed events and original source spans, split-independent acceptance, sticky failures and decoded ceilings |
| `BufferedReader::new`, `with_limits`, `with_decode_limits`, `read_event`, `position`, `get_ref`, `into_inner` | `pull::exercise`, explicit resource/decode profiles, deterministic pull selectors | Lending event/range parity with the independent slice/UTF-16 reference; complete EOF sequence; small valid input must succeed; exact consume ledger, unread suffix, bounded caller copies, original injected I/O error and terminal no-I/O behavior |

Additional option combinations, allocation failures, and future constructors require new cases before this inventory can claim complete execution coverage. Structural mutation, entities, queries, schemas, and foreign binding registries require targets when implemented. A target that calls an API does not establish that all its branches execute.

Owned trees use the same normalization budget for pool growth and parsing. They
can reject a valid borrowed tree with OwnedBytes, or reach that ceiling before a
later syntax failure. All other errors and successful trees are compared
explicitly. For accepted inputs up to 1024 bytes under default limits, owned
success is required: with DTDs unsupported, a conservative parser/pool capacity
bound is `16*n*(n+64)`, at most 17 MiB here, below the 64 MiB default. This check
prevents the OwnedBytes exception from hiding blanket rejection; it must be
revisited when entity expansion or storage accounting changes. Default byte and string constructors run sequentially, dropping the
first owned tree before the second. Complete comparisons borrow pool views
rather than cloning repeated namespace URIs. Canonical owned trees check
foreign document identities; generated Unicode/normalization cases force
pool growth, and tiny aggregate limit profiles exercise parser/storage sharing.
Independent budget goldens require minima of 1, 3, 8 and 10 bytes for `<r/>`,
`<r>a</r>`, `<r>&amp;</r>` and `<r a='&#9;'/>` respectively once the reader accepts.
These check full replacement-capacity charges as well as parser/storage sharing,
so accepting extra documents after undercounting does not evade fuzz assertions.

Streaming comparisons preserve exact logical token boundaries and raw byte spans. `tests/stream.rs` checks every split of small valid/malformed fixtures, all prefixes, single-byte and deterministic random schedules, normalization goldens, resource boundaries, and cancellation at every event. The fuzz driver adds one bounded input-derived schedule per testcase, ordinary `Cursor` I/O, and a custom reader that interrupts once before either delivering bytes or injecting an error at a chosen read. It verifies error kind/message/source and the exact last allowed read after XML failure or cancellation. Arbitrary-input comparisons advance a borrowed slice reader inside each callback: they do not clone expanded namespace URIs into snapshots. Owned captures in `tests/support/capture.rs` are restricted to bounded ordinary fixtures. These checks do not measure process memory or replace retention tests with a discard callback.

## Writer operation format

Recognized UTF-16/UTF-32 signatures take a dedicated path in `fuzz/encoding.rs`.
The bounded harness independently decodes code units using `char::decode_utf16`,
requires BOM/declaration agreement, and rewrites only a matching declaration's
encoding value to UTF-8 for reference grammar validation. It preserves the
original declaration for comparison and independently maps reference spans back
to source bytes. Materializing decoded input and a global offset map is confined
to this test oracle; production decoding retains constant carry. A second BOM is
explicitly rejected rather than letting the UTF-8 reference strip it. Both
auto-decoding surfaces must reject malformed code units, XML and labels.
For independently decoded malformed grammar, error kinds and mapped source
offsets are compared across whole/chunked/I/O/owned paths. Upfront character
validation, allocation and resource ordering have scoped exclusions. Incomplete
second-root markup or top-level CDATA can report streaming EOF instead of the
slice boundary error; both must still identify the same source start.

Successful UTF-16 input compares complete stream/I/O events and owned-tree views
against the UTF-8 reference. Small valid inputs must succeed under default
budgets, preventing blanket rejection behind resource exceptions. Larger inputs
can reach owned/decoded-token ceilings. Derived source, owned and decoded profiles
check offsets, sticky errors and split-independent acceptance. Every UTF-8 input
also exercises derived decoded ceilings. Writer operation/sink generation still
runs for encoded inputs; their retained UTF-16 declaration is not blindly copied
to the UTF-8 writer. Generated fixtures cover both orders, optional/matching BOMs,
surrogates, normalization, malformed declarations, truncations and mutations.

Every testcase also enters `fuzz/writer.rs`. Accepted XML events are serialized with an encoded byte ceiling of `6 * input.len() + 1024`, then compared in lockstep with the original reader and reparsed by DOM and push APIs. The writer may exhaust its owned-string budget earlier than the borrowing slice reader; this outcome is explicitly checked for terminal failure. Escaping can expand output, so reader input/token ceilings are raised for the reparse. Raw markup values supplied by the reader have already undergone XML normalization.

Independent operation generation interprets seven bytes as policy/I/O controls, the next byte's low five bits as an operation count (capped by available bytes), then up to 31 operation bytes and at most 96 bytes of shared logical payload. Each operation selects an event kind, name-field inconsistencies, and declaration/PI variants. The low bit of the first byte selects default versus derived limits; defaults here cap output at 16 KiB and a token at 4096 bytes. Completed sequences must reparse; rejected events leave the Vec sink position unchanged and poison subsequent calls. Deterministic seeds cover accepted nested sequences as well as invalid operations and values.

Reparsing alone cannot detect a raw delimiter that changes the caller's value into other legal markup. The operation oracle separately requires rejection for forbidden comment/CDATA/PI delimiters and inconsistent qualified/prefix/local fields, XMLNS declaration metadata, and namespaces on unprefixed ordinary attributes. Full namespace-scope rejection still also relies on the parser and ordinary independent semantic goldens.

The first two bytes independently select a small fixed valid output operation's sink schedule: short writes, one write/flush interruption, zero/oversized counts, a failure after a chosen prefix, immediate failure, or flush failure. Checks preserve the original I/O kind/message/source, confirmed output position and no subsequent I/O. Recovered sink panics are covered by ordinary tests, not the fuzz target, since the replay harness treats unexpected panics as crashes. No output-performance claim follows from these checks.

Complete-document serialization has a separate oracle in `fuzz/serialize.rs`.
Both borrowed and owned helpers serialize accepted trees, reparse the entire
output and compare every node kind, name, namespace, ordered attribute, payload,
PI target, declaration and navigation link. Node counts and complete DOM parsing
prevent a matching prefix from concealing omitted or extra output. Comparisons
borrow views rather than retaining namespace snapshots. Encoding labels become
`UTF-8`; declaration absence, version and standalone remain exact. Source ranges,
top-level whitespace and the lexical distinction between `<r/>` and `<r></r>`
are not retained by the DOM. Empty CDATA, comments and PIs remain distinct nodes.

The output ceiling is `6 * reference.len() + 1024`; reparse input/token ceilings
are raised accordingly. A valid reference of at most 256 bytes must serialize
successfully with default string budgets. Larger inputs may exhaust the
cumulative owned-string budget, but only that specific error is allowed and it
must poison the writer. This restriction prevents blanket OwnedBytes rejection,
including on owned UTF-16 trees. A prior empty Text event must consume freshness;
reusing a completed document writer must fail without further I/O.

Seven small canonical shapes have independent literal output and below/exact/
above ceilings for output bytes, encoded token bytes, output event count,
attributes, depth and cumulative owned-string bytes. In particular,
`<r><a>x</a><a>x</a></r>` requires three owned bytes and `<r>&amp;</r>` requires
six; these catch per-token resets and duplicate normalization charges. Every
shape runs through borrowed, owned UTF-8 and both UTF-16 orders, with deterministic
sink selectors independent of grammar bytes. Scheduled sinks compare accepted
prefixes directly with the expected output and preserve injected error identity,
confirmed positions and the final flush. Arbitrary outputs up to 4096 bytes also
run an input-selected sink schedule. Recovered sink panics, deep iterative
traversal, retained capacity and allocation-failure evidence require ordinary
tests and measurement in addition to these fuzz assertions. `serialize.xml`
combines declaration rewriting, unused namespace declarations, inherited
prefixes, namespace resets, normalization, empty lexical nodes and epilog nodes.

The same input selects a default constructor and terminal operation sequence. Successful parsing is followed by either an empty/nonempty feed after final EOF or direct I/O consumption of the finished parser. Failed states are also exercised through `read_from` with a source that panics if touched. An input-selected invalid-count reader reports more bytes than its output buffer holds and must produce `InvalidData` after exactly one read. These operations reuse states reached by the testcase; no fixed canonical document is parsed on every invocation.

Acceptance comparisons allow streaming to exhaust its owned-byte budget earlier because it retains names that the slice parser borrows. Arbitrary malformed inputs compare acceptance rather than identical first-error precedence: slice construction validates the complete input first, while streaming validates arriving tokens. Successful inputs require exact values, namespaces, attribute order, event kinds, and spans, with no text coalescing.

`tests/fuzz_smoke.rs` replays every seed and every prefix, replaces every seed byte with delimiter/boundary bytes, deletes every seed byte, generates deterministic arbitrary bytes, and generates accepted namespace-aware nested documents. These tests check invariants and some known acceptance outcomes. Reader/DOM agreement alone is weak because they share a parser; the structural and semantic regression checks remain necessary.

## Owned-value mutation operations

`fuzz/mutation.rs` runs once before the driver's UTF-16 early return, independently
of whether the original testcase is valid XML. Two ten-node documents have
manually specified logical models, explicit children, ordered attributes and
fixed names/namespaces. They contain every leaf kind, Unicode, empty lexical
nodes, same-local-name attributes in different namespaces, inherited/shadowed
bindings and a default reset. Source bytes are overwritten and dropped before
editing. The documents use independently selected UTF-8/BOM/UTF-16 transport;
moving their containers must preserve saved handles and foreign rejection.

The first eight bytes are controls: seed family, encoding, default/derived value
ceiling, derived ceiling, quota case, quota boundary, lifecycle flags and operation
count. The low five count bits select up to 31 complete eight-byte records, capped
by available bytes. At most 256 bytes after the records form shared payload.
Each record contains opcode, target container, saved-handle index, expanded-name
selector, payload start/length, value variant and a repeat/save selector. The low
three opcode bits select node replacement, attribute replacement, read/acquire,
serialize, reopen, move, foreign node replacement or foreign attribute replacement.
Payload slices become logical UTF-8 through bounded lossy decoding; fifteen fixed
variants also force Unicode, delimiters, XML character boundaries and growth.
Invalid UTF-8 is not an input to an `&str` mutation API.

General sequences retain default cumulative capacity limits, with independently
derived per-value ceilings. Their bounded work must succeed whenever the model
permits it: there is no OwnedBytes or Allocation exception. Both setters also
have mandatory small-valid success before operation interpretation. The model
predicts handle, kind, lookup, namespace, length, XML-character and lexical errors
in documented order, including exact UTF-8 indices. It updates expected state
before invoking production. Every result checks complete values, names,
namespace metadata, ordered attributes, declaration, topology, traversal,
lookup, handles and persistent original provenance. An optional rescue operation
uses the same editor after rejection; ordinary operations reopen editors.
The model does not reuse production validation, pool ranges or sibling links.

Quota selectors use separate literal fixtures, not a replica of pool growth.
For `<r>x</r>`, a one-byte replacement requires a full request of at least three
bytes. A fixed ceiling of ten allows four one-byte replacements and rejects the
fifth, with or without reopening; equal-value replacements still spend work.
For `<r a='x'/>`, a two-byte replacement needs five bytes, while one byte fits
existing spare capacity at a zero growth ceiling. Further cases check Unicode
value boundaries, rejection without spending/growing storage, lower/higher
reopened ceilings and empty Text at zero ceilings. Deterministic tests run every
case in UTF-8 and both UTF-16 orders. These assertions target blanket rejection,
per-editor reset, delta-only/full duplicate charging and equal-value shortcuts.

Final and selected intermediate states serialize through `write_owned_document`,
reparse the complete output and compare against the independent model. The only
projection removes empty Text. Templates have no adjacent Text siblings and
value-only editing cannot create them. Empty CDATA, comments and PI data remain
distinct. Output is capped at 64 KiB and encoded tokens at 16 KiB. Literal byte
goldens cover quotas, escaping, CR/whitespace references, Unicode and namespace
declarations; the `*` quota selector also runs the full namespace/lexical golden.
`mutation-operations.bin` and `mutation-quotas.bin` are operation seeds, also
replayed through every prefix and byte mutation by the existing corpus tests.

Public callers cannot forge an owned slot; private tests cover invalid slots and
identity exhaustion with a local counter. Allocation-failure injection remains
separate evidence. Deletion, stale generations, cycles, namespace editing and
structural mutation are not implemented and gain targets with those APIs. The
existing external oracle checks original parsing and pristine serialization;
it does not yet execute these modeled edit sequences against libxml2.

## Source coverage

Pull schedules derive a capacity from 1/2/3/7/64/8192/65536, an early-stop event
count from the low three control bits, and an I/O-failure byte boundary from the
full control byte. Ordinary replay derives that byte from the testcase midpoint;
the deterministic selector test varies it independently of XML grammar bytes.
Large cases raise the smallest visible slice to `ceil(input length / 4096)`;
per-event fill/consume calls can still exceed 4096. A second complete run exposes
the entire remaining input, so buffer
capacity cannot change the final XML result. One source retries an interruption;
another fails at an exact input boundary, including EOF. Source operations are
counted and rejected after terminal states; `consume` cannot exceed the exposed
slice. Source extraction must agree exactly with the parser's consumed position.
Recovered source panics remain ordinary-test cases: unexpected fuzz panics are
crashes, not swallowed outcomes.

The pull oracle compares live events rather than cloning namespaces. It copies
at most 32 payload scalars for the caller-retention operation. Default cases of
at most 1024 input bytes cannot conceal rejection behind an owned-budget
exception. Four canonical tiny documents have independent minima for raw input,
raw token, cumulative retained/normalized strings, events, attributes and depth.
Their below/exact/above profiles run through ordinary suspension. The existing
independent decoded-total/token profiles also exercise pull, including cases
which must succeed at the exact ceilings. UTF-16 values, raw spans and semantic
error offsets reuse the standard-library reference, not the shared push engine.

The `pull-lookahead.xml` corpus seed combines text/markup suspension, empty
CDATA, PI, references, inherited prefixes and a default-namespace reset. Every
prefix and byte mutation is replayed by the existing corpus tests. Retained-state
plateau and source-panic checks require ordinary private/public tests in addition
to the fuzz assertions; a source-region percentage alone cannot establish them.

CI instruments `fuzz_smoke` and uploads raw profiles, merged profiles, a compiler version, a revision, a library-only summary, and the complete LLVM coverage export. The summary excludes dependency/toolchain code and the test/driver source so its denominator is library code. The complete export retains those files for inspection. No production library regions are excluded by this filter. The run exercises only the default feature set and only deterministic fuzz smoke tests; it is separate from ordinary unit-test coverage.

Coverage uses pinned nightly `2026-06-06` with `-C instrument-coverage -Z coverage-options=branch`. The unstable branch instrumentation is documented in the [Rust Unstable Book](https://doc.rust-lang.org/unstable-book/compiler-flags/coverage-options.html). Its branch denominator covers the conditions instrumented by that compiler; it does not enumerate every parser state or boolean condition. The library itself remains stable Rust. See [measured results and gaps](../docs/validation.md).

Use LLVM tools from the matching Rust toolchain's `llvm-tools-preview` component. A system LLVM version can reject profiles from a newer Rust compiler. If tooling is unavailable, record that coverage was not measured; do not infer it from passing tests. The workflow publishes gaps and currently imposes no percentage threshold. A release requires reviewed coverage data and the additional gates in the security plan.

## External fuzz engines and differential testing

An external process-based fuzzer can feed stdin to the release example without changing the crate's dependency graph. Validate its instrumentation and execution feedback before calling it coverage-guided fuzzing. This repository does not yet supply an AFL++/libFuzzer integration, custom mutation runtime, or sustained-run budget. Adding a runtime dependency or unsafe foreign export requires a policy decision.

`--libxml2 PATH` requires libxml2 **2.15.4** and records its version/build report.
`--allow-other-libxml2` explicitly selects an exploratory comparison with another
version; it does not change the target. Tree and XPath support are required.
Accepted fixtures compare complete, independently evaluated XPath values through
the [bounded infoset protocol](../docs/oracle.md); oracle values never pass through
yaxml's parser. Once the original comparison succeeds, the complete-document
helper serializes the tree and a second invocation evaluates those bytes with
the original query and expected values. UTF-8 uses the borrowing document helper;
UTF-16 uses the owning helper. Semantic agreement requires both comparisons.
Projection or serialization storage ceilings remain explicitly counted
acceptance-only outcomes. This external projection merges text/CDATA and omits
declaration/XMLNS metadata; the stricter in-process tree comparison above covers
those fields. Rejected fixtures compare acceptance. All invocations use
`--nonet --nocatalogs --strict-namespace --nowarning`, with both catalog variables
explicitly empty. Warnings such as forward-version notices are suppressed;
namespace errors remain fatal through `--strict-namespace`.

Each oracle invocation has a five-second execution deadline and an 8 MiB combined
stdout/stderr ceiling (16 KiB for its version report). Regular-file capture avoids
pipe deadlocks. Output-size polling can overshoot temporary disk usage; this is
not a disk quota or a sandbox for an untrusted executable. Timeout/output failures
kill and reap the direct child. File operations and arbitrary descendants are
outside that termination guarantee. Capture directories use best-effort cleanup.

DTD-containing bytes, unsupported encodings, declared-encoding mismatches, invalid UTF-8 and yaxml limit
failures are policy skips. The DTD byte prefilter is conservative and can also
skip that spelling inside a comment. Projection ceilings produce separately
counted acceptance-only results. A clean exit 0 is acceptance; exit 4 is rejection;
other statuses and successful exits with diagnostics are inconclusive. Both
disagreements and inconclusive results fail the command. Summaries distinguish
semantic agreement, rejection agreement, acceptance-only, policy skip,
inconclusive and disagreement. [xmllint options and return codes](https://gnome.pages.gitlab.gnome.org/libxml2/xmllint.html).

CI is configured to download the checksum-pinned release, build a static external
oracle, compare the corpus, and retain metadata. Local availability and actual
hosted execution must accompany any result; configuration alone is not evidence
of a pinned pass. The comparison does not establish identical resource policy,
error categories, recovery or diagnostic wording. Minimize disagreements and
retain their classification. No external XML implementation is a Cargo dependency.
