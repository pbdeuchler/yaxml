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
| `PushParser::new`, `default`, `with_limits`, `feed`, `finish`, `position`, `event_range` | Input-derived chunk and lifecycle schedules, capped at 4096 chunks | Acceptance and exact typed event/span parity with slice reader; monotonic consumed position; repeated EOF; sticky errors after feeding final EOF |
| `PushParser::read_from` | Cursor plus bounded scheduled and invalid-count readers | Same events/spans; one Interrupted retry; original injected I/O error kind/message; cancellation stops reads; terminal parser states perform no I/O; oversized reported counts are rejected |
| DOM traversal and node access | Successful document checks | Unique IDs; preorder traversal; parent/child/sibling agreement; nested source ranges; subtree sizes |
| `Writer::new`, `with_limits`, `write_event`, `finish`, `position`, `into_inner` | Accepted reader events, generated caller-created event sequences and scheduled sinks | Exact typed round trips; complete output reparses; malformed operations emit no bytes; finite encoded bounds; confirmed partial positions; sticky failure; no I/O after failure or repeated successful finish |
| XML normalization and references | Reader/DOM corpora and generated documents | Exercised indirectly; semantic golden tests are also required in the ordinary test suite |

Additional option combinations, allocation failures, and future constructors require new cases before this inventory can claim complete execution coverage. Tree mutation, entities, queries, schemas, and binding handles require targets when implemented. A target that calls an API does not establish that all its branches execute.

Streaming comparisons preserve exact logical token boundaries and raw byte spans. `tests/stream.rs` checks every split of small valid/malformed fixtures, all prefixes, single-byte and deterministic random schedules, normalization goldens, resource boundaries, and cancellation at every event. The fuzz driver adds one bounded input-derived schedule per testcase, ordinary `Cursor` I/O, and a custom reader that interrupts once before either delivering bytes or injecting an error at a chosen read. It verifies error kind/message/source and the exact last allowed read after XML failure or cancellation. Arbitrary-input comparisons advance a borrowed slice reader inside each callback: they do not clone expanded namespace URIs into snapshots. Owned captures in `tests/support/capture.rs` are restricted to bounded ordinary fixtures. These checks do not measure process memory or replace retention tests with a discard callback.

## Writer operation format

Every testcase also enters `fuzz/writer.rs`. Accepted XML events are serialized with an encoded byte ceiling of `6 * input.len() + 1024`, then compared in lockstep with the original reader and reparsed by DOM and push APIs. The writer may exhaust its owned-string budget earlier than the borrowing slice reader; this outcome is explicitly checked for terminal failure. Escaping can expand output, so reader input/token ceilings are raised for the reparse. Raw markup values supplied by the reader have already undergone XML normalization.

Independent operation generation interprets seven bytes as policy/I/O controls, the next byte's low five bits as an operation count (capped by available bytes), then up to 31 operation bytes and at most 96 bytes of shared logical payload. Each operation selects an event kind, name-field inconsistencies, and declaration/PI variants. The low bit of the first byte selects default versus derived limits; defaults here cap output at 16 KiB and a token at 4096 bytes. Completed sequences must reparse; rejected events leave the Vec sink position unchanged and poison subsequent calls. Deterministic seeds cover accepted nested sequences as well as invalid operations and values.

The first two bytes independently select a small fixed valid output operation's sink schedule: short writes, one write/flush interruption, zero/oversized counts, a failure after a chosen prefix, immediate failure, or flush failure. Checks preserve the original I/O kind/message/source, confirmed output position and no subsequent I/O. Recovered sink panics are covered by ordinary tests, not the fuzz target, since the replay harness treats unexpected panics as crashes. No output-performance claim follows from these checks.

The same input selects a default constructor and terminal operation sequence. Successful parsing is followed by either an empty/nonempty feed after final EOF or direct I/O consumption of the finished parser. Failed states are also exercised through `read_from` with a source that panics if touched. An input-selected invalid-count reader reports more bytes than its output buffer holds and must produce `InvalidData` after exactly one read. These operations reuse states reached by the testcase; no fixed canonical document is parsed on every invocation.

Acceptance comparisons allow streaming to exhaust its owned-byte budget earlier because it retains names that the slice parser borrows. Arbitrary malformed inputs compare acceptance rather than identical first-error precedence: slice construction validates the complete input first, while streaming validates arriving tokens. Successful inputs require exact values, namespaces, attribute order, event kinds, and spans, with no text coalescing.

`tests/fuzz_smoke.rs` replays every seed and every prefix, replaces every seed byte with delimiter/boundary bytes, deletes every seed byte, generates deterministic arbitrary bytes, and generates accepted namespace-aware nested documents. These tests check invariants and some known acceptance outcomes. Reader/DOM agreement alone is weak because they share a parser; the structural and semantic regression checks remain necessary.

## Source coverage

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
yaxml's parser. Rejected fixtures compare acceptance. All invocations use
`--nonet --nocatalogs --strict-namespace --nowarning`, with both catalog variables
explicitly empty. Warnings such as forward-version notices are suppressed;
namespace errors remain fatal through `--strict-namespace`.

Each oracle invocation has a five-second execution deadline and an 8 MiB combined
stdout/stderr ceiling (16 KiB for its version report). Regular-file capture avoids
pipe deadlocks. Output-size polling can overshoot temporary disk usage; this is
not a disk quota or a sandbox for an untrusted executable. Timeout/output failures
kill and reap the direct child. File operations and arbitrary descendants are
outside that termination guarantee. Capture directories use best-effort cleanup.

DTD-containing bytes, unsupported encodings, invalid UTF-8 and yaxml limit
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
