# Streaming milestone validation

This extends the [initial validation](validation.md). It covers bounded callback
push input and the explicit `Read` adapter, with shared slice/push XML semantics.
The measured revision is `4c150d116cb2426b9dc5510bfcc17b472fd99f74`.

## Functional checks and review

Rust 1.95.0 on `aarch64-apple-darwin` passes 69 all-target test functions and one
README doctest. This includes 13 internal parser/lexer tests, 9 streaming tests,
7 I/O tests, 2 callback-panic tests, and the existing slice/DOM regressions.
Formatting and `cargo clippy --all-targets -- -D warnings` pass. The dependency
graph still contains only `memchr` 2.8.3, and package/core lints forbid unsafe.
CI now also runs the public documentation example; hosted execution is pending.

Every two-way split and one-byte chunks of small valid fixtures preserve exact
typed events, normalized values, namespaces, attribute order and source spans.
Tests cover malformed splits, all prefixes, 64 deterministic random schedules,
each limit below/at/above its boundary, every-event cancellation, final EOF,
I/O interruption/failure, invalid reader counts, and recovered callback panics.
Private tests verify namespace entries do not accumulate across 1,000 completed
siblings and scratch capacity stays within the configured token ceiling.
These checks establish retention invariants, not measured peak process memory.

The [independent review](review-streaming.md) found no open production issues.
Four deliberately broken behaviors were caught: quoted delimiter scanning,
namespace ancestor restoration, preexisting-error handling before I/O, and feed
panic poisoning. All mutations were restored. Subsequent fuzz-harness additions
were reviewed by the orchestrator; they change no production code.

## Coverage evidence

These are raw LLVM library summaries, with no production-code exclusions.
Dependencies, toolchain code, tests and the driver are filtered from the summary;
complete exports retain them. Internal tests live under `tests/internal/` so the
same filter excludes test bodies from unit-test objects as well.

| Run | Regions | Functions | Lines | Instrumented branch outcomes |
| --- | --- | --- | --- | --- |
| Fuzz smoke only | 2,066 / 2,185 (94.55%) | 123 / 137 (89.78%) | 1,341 / 1,380 (97.17%) | 328 / 346 (94.80%) |
| Ordinary tests only | 2,044 / 2,185 (93.55%) | 123 / 137 (89.78%) | 1,335 / 1,380 (96.74%) | 317 / 346 (91.62%) |

The [fuzz summary](validation/fuzz-coverage-4c150d1.txt) and
[ordinary-test summary](validation/test-coverage-4c150d1.txt) are checked in.
Raw profiles, merged profiles, compiler artifacts, complete JSON exports,
source/corpus SHA-256 hashes and output are retained locally in ignored
`coverage/fuzz-4c150d1/` and `coverage/tests-4c150d1/`.

Both runs use nightly `2026-06-06` (Rust 1.98.0-nightly, LLVM 22.1.6), default
features, debug optimization and
`RUSTFLAGS='-C instrument-coverage -Z coverage-options=branch'`. Apple LLVM 21.0.0
tools successfully read the profiles; this version mismatch remains a limitation.
CI requests the toolchain's matching `llvm-tools-preview` instead.

The fuzz run executes 14 deterministic smoke test functions in 0.36 seconds,
with zero unexpected failures. Individual testcase executions are not yet
counted. It replays/mutates the seed corpus, generates bytes/documents and checks
input-derived streaming/lifecycle/I/O schedules. This is a short smoke campaign,
not sustained coverage-guided fuzzing. The ordinary run executes 51 unit and
reader/DOM/streaming/I/O/callback tests, excluding `fuzz_smoke`, examples and benches.

LLVM summary totals require care with generic functions. Its instantiation-group
merge takes the maximum covered count per metric rather than the union of
distinct executed regions across instantiations. Consequently an annotated
source line can execute somewhere while the raw summary still records a miss.
This behavior follows LLVM's [summary implementation](https://github.com/llvm/llvm-project/blob/llvmorg-21.1.0/llvm/tools/llvm-cov/CoverageSummaryInfo.cpp)
and [merge definitions](https://github.com/llvm/llvm-project/blob/llvmorg-21.1.0/llvm/tools/llvm-cov/CoverageSummaryInfo.h).
The reported totals are retained unchanged; no higher adjusted percentage is
claimed. Branch instrumentation also does not enumerate every match arm or
semantic state transition.

Known gaps include allocation-failure closures, unreachable parser/lexer
invariant guards, shared-engine checks already enforced by both frontends, and
the DOM missing-root guard after successful reader EOF. They stay in the raw
denominator. Deterministic allocation failure injection, toolchain-matched local
measurement, and a reviewed generic-instantiation aggregation policy remain open.
The target inventory in [fuzz/README.md](../fuzz/README.md) includes every new
public input API; this is separate from the unmet 100% execution-coverage goal.

## Differential and performance scope

The available Homebrew libxml2 2.15.3 executable, with `--strict-namespace`, gives
five acceptance agreements and one unsupported-DTD skip on the six seed files.
The target stays 2.15.4; this exploratory result does not close the pinned-oracle
gate or compare complete event/tree semantics.

The benchmark now consumes push events in 8 KiB chunks alongside slice events
and DOM parsing/destruction. Bulk `memchr` scans skip ordinary byte runs without
rescanning retained token prefixes. Exploratory local timings informed that
implementation choice; repeated matched libxml2 measurements, input size series,
latency and peak-memory measurements remain pending. No across-the-board
performance improvement has been established.

Buffered pull, owned DOM, UTF-16, allocation policy and the remaining libxml2
features continue in [the roadmap](roadmap.md). This milestone is not a release
security audit or completion of the replacement goal.
