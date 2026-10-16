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
| XML normalization and references | Reader/DOM corpora and generated documents | Exercised indirectly; semantic golden tests are also required in the ordinary test suite |

Additional option combinations, allocation failures, and future constructors require new cases before this inventory can claim complete execution coverage. Writer operations, entities, queries, schemas, and binding handles do not exist yet and require targets when implemented. A target that calls an API does not establish that all its branches execute.

Streaming snapshots preserve exact logical token boundaries and raw byte spans. `tests/stream.rs` checks every split of small valid/malformed fixtures, all prefixes, single-byte and deterministic random schedules, normalization goldens, resource boundaries, and cancellation at every event. The fuzz driver adds one bounded input-derived schedule per testcase, ordinary `Cursor` I/O, and a custom reader that interrupts once before either delivering bytes or injecting an error at a chosen read. It verifies error kind/message/source and the exact last allowed read after XML failure or cancellation. Snapshot storage intentionally grows with emitted output; parser retention must be evaluated separately with a discard callback.

The same input selects a default constructor and terminal operation sequence. Successful parsing is followed by either an empty/nonempty feed after final EOF or direct I/O consumption of the finished parser. Failed states are also exercised through `read_from` with a source that panics if touched. An input-selected invalid-count reader reports more bytes than its output buffer holds and must produce `InvalidData` after exactly one read. These operations reuse states reached by the testcase; no fixed canonical document is parsed on every invocation.

Acceptance comparisons allow streaming to exhaust its owned-byte budget earlier because it retains names that the slice parser borrows. Arbitrary malformed inputs compare acceptance rather than identical first-error precedence: slice construction validates the complete input first, while streaming validates arriving tokens. Successful inputs require exact values, namespaces, attribute order, event kinds, and spans, with no text coalescing.

`tests/fuzz_smoke.rs` replays every seed and every prefix, replaces every seed byte with delimiter/boundary bytes, deletes every seed byte, generates deterministic arbitrary bytes, and generates accepted namespace-aware nested documents. These tests check invariants and some known acceptance outcomes. Reader/DOM agreement alone is weak because they share a parser; the structural and semantic regression checks remain necessary.

## Source coverage

CI instruments `fuzz_smoke` and uploads raw profiles, merged profiles, a compiler version, a revision, a library-only summary, and the complete LLVM coverage export. The summary excludes dependency/toolchain code and the test/driver source so its denominator is library code. The complete export retains those files for inspection. No production library regions are excluded by this filter. The run exercises only the default feature set and only deterministic fuzz smoke tests; it is separate from ordinary unit-test coverage.

Coverage uses pinned nightly `2026-06-06` with `-C instrument-coverage -Z coverage-options=branch`. The unstable branch instrumentation is documented in the [Rust Unstable Book](https://doc.rust-lang.org/unstable-book/compiler-flags/coverage-options.html). Its branch denominator covers the conditions instrumented by that compiler; it does not enumerate every parser state or boolean condition. The library itself remains stable Rust. See [measured results and gaps](../docs/validation.md).

Use LLVM tools from the matching Rust toolchain's `llvm-tools-preview` component. A system LLVM version can reject profiles from a newer Rust compiler. If tooling is unavailable, record that coverage was not measured; do not infer it from passing tests. The workflow publishes gaps and currently imposes no percentage threshold. A release requires reviewed coverage data and the additional gates in the security plan.

## External fuzz engines and differential testing

An external process-based fuzzer can feed stdin to the release example without changing the crate's dependency graph. Validate its instrumentation and execution feedback before calling it coverage-guided fuzzing. This repository does not yet supply an AFL++/libFuzzer integration, custom mutation runtime, or sustained-run budget. Adding a runtime dependency or unsafe foreign export requires a policy decision.

The optional `--libxml2 PATH` comparison runs `PATH --version` and records its output, then uses `--nonet --noout -` on stdin with ambient catalog variables cleared. It skips DTDs, unsupported encodings, invalid UTF-8, and yaxml limit failures because these do not share the same policy. A clean exit 0 is acceptance; exit 4 is rejection; every other status or a successful exit with diagnostics is inconclusive. This matters because system libxml2 2.9.13 can report a namespace error yet exit 0. Disagreements fail the replay command for classification. Inconclusive and skipped cases are reported explicitly and are not agreements. External subprocess timeouts must be enforced by the invoking runner. [xmllint options and return codes](https://gnome.pages.gitlab.gnome.org/libxml2/xmllint.html).

A release comparison must use a separately built, pinned binary with recorded version/build flags. The CLI does not establish a shared resource policy beyond the exclusions above. Malformed-input recovery, diagnostic wording, and attribute/event ordering need their own comparison contracts. A system `xmllint` is useful for investigation but does not establish compatibility with a chosen release. Minimize disagreements and retain the original and reduced inputs with their classification. No external XML implementation is a Cargo dependency.
