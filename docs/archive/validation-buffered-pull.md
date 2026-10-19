# Buffered pull validation

This report covers the bounded lending pull milestone through production commit
`c1d1e12`. It does not claim complete libxml2 reader compatibility, a matched
libxml2 performance win, sustained coverage-guided fuzzing, or 100% execution
coverage.

## Implemented contract

`BufferedReader<R: BufRead>` shares the decoder, tokenizer, and semantic state
with `PushParser`. It lends one typed event and its original-byte range, consumes
only the accepted source prefix, and leaves a visible suffix unread. UTF-8,
UTF-16LE, and UTF-16BE use the same declaration, limit, namespace, normalization,
and error semantics as push parsing.

Successful EOF repeats without I/O. XML failures repeat the same copyable error.
The first I/O failure preserves its original `std::io::Error`; later calls return
`InvalidState` without touching the source. A recovered source panic also poisons
the reader. The exact input ceiling distinguishes EOF, excess bytes, and an I/O
error without consuming an excess byte.

## Behavioral evidence

The final all-target run passed 183 test functions; six subprocess helpers were
intentionally ignored and exercised by their parent test. Five doctests passed,
including a compile-fail example proving that a borrowed event prevents another
mutable advance. Formatting and strict all-target Clippy passed on Rust 1.95.0.

Nineteen new public tests cover every byte split and buffer capacities
1/2/3/7/64/8192/65536 across UTF-8, BOM-prefixed UTF-8, UTF-16LE, and UTF-16BE.
They compare typed values and original-byte ranges, exercise every event kind and
resource ceiling, and inject interruptions, mid-token I/O failures, source
panics, truncated code units, unread suffixes, and repeated terminal calls.

Private tests suspend within decoded scalars, verify full-scalar decoded-budget
charging before tokenizer suspension, retain scalar suffixes exactly once, and
exercise final ambiguous preambles. Retained-state series parse 64, 256, 1,024,
and 4,096 sibling elements with chunk sizes 1, 7, and 8,192. Token capacity stays
64; peak stack/binding/prefix/URI length-capacity pairs stay fixed at
`(2,4)/(1,4)/(1,3)/(1,3)`, and all active entries return to zero at EOF.

The independent review in [review-buffered-pull.md](review-buffered-pull.md)
mutated exact consumption, text lookahead, UTF-16 scalar carry, quota rejection,
and I/O poisoning. Each mutation caused predicted test failures and was restored.

## Fuzz target and source coverage

The dependency-free fuzz driver now maps every `BufferedReader` constructor and
method. It varies visible capacity, interruption, injected I/O failure, early
stop, caller payload copies, EOF/error repetition, source extraction, and policy
limits. Successful runs require complete event exhaustion and exact typed/range
agreement with the independent UTF-16 or slice reference. Source-operation and
event bounds detect stalled parsing instead of allowing the harness to hang.

Both coverage runs use default features, debug optimization, nightly
`2026-06-06` (Rust 1.98.0-nightly / LLVM 22.1.6), and
`-C instrument-coverage -Z coverage-options=branch`. Apple LLVM 21.0.0 reads the
profiles. Reports exclude only dependency, toolchain, test, fuzz, and example
files; no production region is excluded.

| Run | Regions | Functions | Lines | Instrumented branch outcomes |
| --- | --- | --- | --- | --- |
| Fuzz smoke only | 3,871 / 4,059 (95.37%) | 256 / 277 (92.42%) | 2,571 / 2,632 (97.68%) | 569 / 600 (94.83%) |
| Ordinary tests only | 3,904 / 4,132 (94.48%) | 263 / 283 (92.93%) | 2,631 / 2,692 (97.73%) | 571 / 606 (94.22%) |

The new pull frontend reaches 130 / 133 regions, all 8 functions, 94 / 95 lines,
and 18 / 20 instrumented branch outcomes under fuzz smoke. The shared stream
module reaches 192 / 199 regions, 22 / 23 functions, 129 / 134 lines, and 9 / 10
branch outcomes. These figures remain below the 100% execution target.

Checked-in summaries are [fuzz coverage](validation/fuzz-coverage-4634a47.txt)
and [ordinary coverage](validation/test-coverage-4634a47.txt). Ignored local
`coverage/fuzz-4634a47/` and `coverage/tests-4634a47/` retain raw profiles,
merged profiles, full JSON exports, instrumented binaries, compiler/tool
versions, hashes, and reproduction logs. Fuzz smoke passed 19 functions in
17.63 seconds during the uninstrumented final all-target run. This deterministic
campaign is not sustained coverage-guided fuzzing.

## Performance evidence

`benches/pull.rs` measures complete consumption, first-event latency, and bytes
consumed at the first event for six workloads, three encodings, and seven buffer
capacities. It observes names, namespaces, attributes, payloads, and ranges. It
does not provide a matched libxml2 comparison.

The shared resumable engine initially reduced existing token-heavy push
throughput. A UTF-8 steady-state decoder fast path recovered 1.66% on flat input
and 2.41% on normalized input without changing validation. Balanced release runs
used six samples per build, 50 iterations per sample, and all six pre/post/fix
orders. Text remained unchanged within noise. Final push medians versus the
pre-milestone implementation are 59.73 versus 63.60 MiB/s for flat input,
449.67 versus 449.62 MiB/s for large text, 57.92 versus 60.14 MiB/s for
normalized input, and 45.21 versus 46.02 MiB/s for namespaces. The remaining
6.08%, 3.70%, and 1.76% token-heavy regressions are open performance work.

The checked-in [performance summary](validation/push-performance-c1d1e12.txt)
records medians, median absolute deviations, and ranges. The raw local log is
`/private/tmp/yaxml-pull-perf-final.log`. Compiler, workload, lifecycle, and
comparability rules are documented in [benchmarks.md](../benchmarks.md).

## Remaining gates

- Build the pinned libxml2 2.15.4 callback-I/O harness and compare the common
  logical projection. Libxml2 has no equivalent exact event-range API.
- Recover the measured token-heavy push regression without splitting semantic
  engines or weakening validation.
- Raise raw execution coverage from 95.37% fuzz regions and 92.42% fuzz
  functions to the 100% target, then run a sustained coverage-guided campaign.
- Add allocation-failure injection and adversarial time/memory series beyond
  the retained-state capacity checks.
