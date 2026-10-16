# Initial implementation validation

This records the first UTF-8 reader/DOM slice, not completion of the replacement
goal. Core source is unchanged between `bd52745` and `e935e2b`; later commits in
that range strengthen tests and coverage reporting.

## Functional checks and review

On `aarch64-apple-darwin`, Rust 1.95.0, the implementation passes:

- `tests/reader.rs`: 12 test functions, including table-driven malformed input,
  Unicode boundaries, namespace aliases/scope, source ranges, and every limit.
- `tests/dom.rs`: 7 test functions, including 4,096-level and 4,096-sibling trees.
- `tests/fuzz_smoke.rs`: 13 test functions covering replay, truncation, mutation,
  generated documents, reference semantics, policies, and reader/DOM projection.
- `examples/fuzz_replay.rs`: 4 argument, I/O, and oracle-classification tests.
- README doctest: 1 passing example.

`cargo clippy --all-targets -- -D warnings` passes. `cargo tree --edges all`
contains only `memchr` 2.8.3. Core and package lint configuration forbid unsafe.
The CI workflow is configured; no hosted CI execution has been claimed.

Independent review deliberately changed three crucial behaviors: accepting a
mismatched end tag, assigning different identities to equivalent namespace URIs,
and truncating DOM subtree traversal. Tests caught each mutation, and each was
restored. Review also found a test loop that could hang on an incorrectly accepted
document; it now fails at unexpected EOF. A fourth mutation verified that fix.

## Coverage evidence

Coverage is **not 100%**. These are raw library totals, without production-code
exclusions. Dependencies, the toolchain, tests, and the external driver are
excluded from the library summary; complete JSON exports retain their data.

| Run | Regions | Functions | Lines | Instrumented branch outcomes |
| --- | --- | --- | --- | --- |
| Fuzz smoke only, `e935e2b` | 1,332 / 1,402 (95.01%) | 76 / 88 (86.36%) | 831 / 846 (98.23%) | 227 / 228 (99.56%) |
| Ordinary reader/DOM tests only, `f984b56` | 1,331 / 1,402 (94.94%) | 76 / 88 (86.36%) | 830 / 846 (98.11%) | 224 / 228 (98.25%) |

The [fuzz report](validation/fuzz-coverage-e935e2b.txt) and
[ordinary-test report](validation/test-coverage-f984b56.txt) are checked in.
Local raw profiles, merged profiles, and complete exports are in ignored
`coverage/fuzz-e935e2b/` and `coverage/tests-f984b56/`. Fuzz-run metadata includes
source/corpus hashes, compiler/tool versions, and compiler-artifact paths. The
13 fuzz smoke test functions ran successfully in roughly 0.19 seconds. Individual
testcase executions are not yet counted; this is a short deterministic campaign,
not sustained coverage-guided fuzzing.

Both reports use nightly `2026-06-06` (Rust 1.98.0-nightly, LLVM 22.1.6), with
`-C instrument-coverage -Z coverage-options=branch`, default features, and debug
optimization. Apple LLVM 21.0.0 tools successfully merged and read these profiles;
the version mismatch is recorded rather than concealed. CI installs the selected
Rust toolchain's matching `llvm-tools-preview` tools instead. The compiler's branch
instrumentation does not enumerate every match arm, state transition, or arbitrary
boolean expression. [Rust coverage documentation](https://doc.rust-lang.org/rustc/instrument-coverage.html)
and [branch flag documentation](https://doc.rust-lang.org/unstable-book/compiler-flags/coverage-options.html)
define the measurement mechanism.

The reader and normalization modules reach every branch outcome instrumented by
this configuration. The remaining DOM branch is the missing-root guard after
successful reader EOF, an invariant failure that ordinary input cannot cause.
Uncovered functions include allocation-error closures and an internal DOM stack
guard. Error propagation regions behind already-validated internal names and
allocation failures also remain uncovered. They stay in the denominator. Safe,
deterministic allocation-failure injection and a sustained coverage-guided engine
remain open work; passing this campaign does not establish security readiness.

## Differential and performance limits

The installed `/usr/bin/xmllint` uses libxml2 2.9.13, not the pinned compatibility
target 2.15.4. On the six seed files, the acceptance-only process comparison
reported four clean agreements, one inconclusive namespace diagnostic, and one
unsupported-DTD skip. Version 2.9.13 can print namespace errors while exiting zero;
the driver does not count that as an agreement. Full semantic comparison against
a separately built 2.15.4 oracle is still pending.

The release benchmark runs DOM parsing/destruction and slice-event consumption
for tiny, flat, text-heavy, normalized, and namespace-heavy inputs. It is a yaxml
measurement tool; a fair libxml2 comparison, repeated statistical samples, memory
measurements, and true buffered streaming are pending. No universal performance
claim follows from it.

Independent review found namespace hashing amplification and checked its repair
with 32 child tags containing 256 qualified attributes each. At URI lengths
1 KiB / 16 KiB / 256 KiB, the exploratory release probe changed from approximately
4.96 / 47.49 / 654.93 ms to 1.59 / 1.59 / 2.53 ms. Interned namespace identities
remove repeated hashing of the URI for every attribute. These are exploratory
same-machine observations, not a published benchmark suite or a libxml2 result.
The regression test checks aliases, scope, and long-URI behavior without timing
thresholds.

## Next gates

Finish the pinned oracle/conformance harness, allocation audit and injection
strategy, measured memory/work limits, sustained fuzzing, and comparable libxml2
DOM/event baselines before making production or performance claims. Then expand
to incremental input, owned documents, allocator-backed storage on the verified
target compiler, encoding support, writing, and mutation. The complete feature
burndown remains in [the roadmap](roadmap.md).
