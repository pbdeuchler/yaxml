## Holistic Review

**Plan:** [Complete DOM serialization plan](plans/dom-serialization.md), read before code review
**Base:** `9904278` (provided); implementation reviewed through `b1819604dbdb36bfc94b0a8e3ff406b84dca4fc1`
**Files Changed:** 29
**Commits:** 5

### Summary

The implementation follows the complete-document serialization plan for both immutable storage forms, with explicit fresh-writer guards, canonical UTF-8 declaration labels, iterative navigation and reusable borrowed attribute metadata. Review covered every changed file in full, including production, tests, fuzz helpers, replay tooling, benchmarks and documentation. Correctness, security/resource behavior, operational failures, simplicity, design boundaries and idiomatic Rust use produced no findings; the concrete tree adapter and shared Writer validation keep the added mechanism proportional to its scope. The scoped milestone is ready, while complete libxml2 compatibility, matched performance results, allocator support, allocation-failure injection and 100% fuzz execution coverage remain open project goals.

### Mutation Results

**Baseline:** green.

The clean `b181960` worktree passed `cargo test --all-targets --offline --locked`: 200 tests passed and six opt-in external-oracle tests remained ignored. All three benchmark targets also completed their debug functional smoke matrices. `cargo test --doc --offline --locked` passed five doctests, including the lending API compile-fail case. `cargo fmt --check`, `cargo clippy --all-targets --offline --locked -- -D warnings` and `git diff --check 9904278..b181960` passed. Review ran on aarch64-apple-darwin, Rust/Cargo 1.95.0; these debug benchmark executions establish functional behavior, not comparative performance.

Tests below were executed separately so an ordinary-test failure could not substitute for the requested fuzz assertion. The abbreviations identify exact test filters:

- **U:** `cargo test --offline --test serialize independent_byte_goldens_preserve_every_kind_and_normalized_value`
- **Q:** `cargo test --offline --test serialize each_writer_ceiling_counts_emitted_utf8_events_and_expansion`
- **F:** `cargo test --offline --test fuzz_smoke document_serialization_semantics_and_independent_quotas`
- **E:** `cargo test --offline --test fuzz_smoke encoded_equivalents_and_every_truncation`
- **I:** `cargo test --offline --test serialize_io first_sink_error_preserves_cause_and_confirmed_partial_position`
- **D:** `cargo test --offline --test serialize deep_and_wide_trees_serialize_without_call_stack_growth_or_lost_siblings`

| # | Mutation | Location at reviewed revision | Predicted failures | Actual failures | Verdict |
| - | -------- | ----------------------------- | ------------------ | --------------- | ------- |
| 1 | Return without emitting any document events | `src/serialize.rs:147` | U and F require successful nonempty output | U fails; F rejects `MissingRoot` at offset zero through its bounded-valid-success assertion | caught |
| 2 | Serialize every tree as the well-formed constant `<r/>` | `src/serialize.rs:147` | U byte golden; F strict declaration/tree comparison | U byte golden; F reports changed declaration presence at `fuzz/serialize.rs:134` | caught |
| 3 | Omit end tags while climbing ancestors | `src/serialize.rs:188` | U and F reject incomplete or mismatched closure | U fails; F rejects `UnexpectedEof` through bounded-valid-success assertion | caught |
| 4 | Skip only empty CDATA events | `src/serialize.rs:170` | U byte golden; F strict node count | U fails; F reports lost/added nodes at `fuzz/serialize.rs:136` | caught |
| 5 | Remove the unused `xmlns:q` declaration from metadata | `src/serialize.rs:109` | U fixtures remain valid; F must detect XMLNS loss despite well-formed output | U passes as predicted; F reports changed ordered attributes/XMLNS declarations at `fuzz/serialize.rs:146` | caught |
| 6 | Forward original declaration encoding instead of rewriting it | `src/serialize.rs:148` | U canonical-label golden; E owned UTF-16 output | U fails; E rejects `UnsupportedEncoding` at output byte 20 through bounded-valid-success assertion | caught |
| 7 | Reset only the Writer's cumulative owned counter before each output event | `src/writer.rs:217`, temporary private parser hook | Q below-minimum rejection; F and E independent cumulative minima | Q fails; both F and E report ignored cumulative quota minimum at `fuzz/serialize.rs:242` | caught |
| 8 | Duplicate only the Writer's actual per-event owned charge | `src/writer.rs:329`, temporary private parser hooks | Q exact-minimum success; F and E independent exact minima | Q fails; both F and E report rejected exact quota minimum at `fuzz/serialize.rs:248` | caught |
| 9 | Return blanket `OwnedBytes` failure from common serialization | `src/serialize.rs:147` | U and F must reject failure of bounded valid trees | U fails; F reports rejected bounded valid tree at `fuzz/serialize.rs:56` | caught |
| 10 | Return blanket `OwnedBytes` failure only from owned serialization | `src/serialize.rs:133` | U owned cases; E's owned UTF-16 bounded-valid path | U fails; E reports rejected bounded valid tree at `fuzz/serialize.rs:56` | caught |
| 11 | Swallow a non-Interrupted final flush failure | `src/writer.rs:206` | I preserves first error; F fault schedule rejects false success | I fails on unexpected success; F reports swallowed sink failure at `fuzz/serialize.rs:350` | caught |
| 12 | Add a non-tail recursive child walk before serialization, with a live 128-byte frame | `src/serialize.rs:147` | D must fail at depth 8,192 on its 256 KiB stack; shallow F remains green | D aborts with stack overflow/SIGABRT; F passes as predicted | caught |

Mutation 12 is an equivalent guard against input-depth call-stack growth: it intentionally adds recursive traversal before the otherwise unchanged iterative emitter. Its failure demonstrates that the existing small-stack test detects recursion, while passing shallow cases alone cannot establish the bound. Core dumps were disabled for that subprocess. The original iterative implementation passes D in the baseline and restored suites.

Initial global accounting probes changed `Budget::charge_owned` for every parser. Those probes were caught first by preexisting UTF-16 parser quota assertions, so they were not credited as serializer mutation evidence. They were restored and replaced by rows 7 and 8, which restrict the wrong accounting to the Writer; both reach the new serializer quota assertions, including the independently decoded owned UTF-16 path. Temporary private hooks existed only during those mutations and were restored with their call sites.

Predictions were written before execution. Each mutation was applied independently, run against its listed ordinary and fuzz checks, immediately restored, and followed by an empty `git status --porcelain`. Logs were retained locally as `/private/tmp/yaxml-ser-m*-unit.log`, `-fuzz.log` and `-utf16.log`, with predictions in `/private/tmp/yaxml-serialize-mutations.log`. Full unmodified all-target validation ran before mutations and again after restoration; no production change is included in this review commit.

Fresh coverage artifacts for the exact reviewed revision are in `/private/tmp/yaxml-coverage-serialize-fuzz` and `/private/tmp/yaxml-coverage-serialize-tests`. The revision, source hashes, compiler/reporter metadata and raw reports were independently inspected; `src/serialize.rs` and `src/writer.rs` hashes match the restored worktree. Nightly 2026-06-06 is Rust 1.98.0-nightly/LLVM 22.1.6; local reporting used Apple LLVM 21.0.0, so the compiler/reporter version difference remains part of the evidence. Default features were measured; only dependency/toolchain and test/fuzz/example/bench sources were excluded, with no production exclusions.

| Coverage run | Regions | Functions | Lines | Instrumented branch outcomes |
| ------------ | ------- | --------- | ----- | ---------------------------- |
| Fuzz smoke, whole library | 4,130 / 4,329 (95.40%) | 272 / 294 (92.52%) | 2,736 / 2,804 (97.57%) | 588 / 622 (94.53%) |
| Ordinary tests, whole library | 4,214 / 4,402 (95.73%) | 280 / 300 (93.33%) | 2,809 / 2,864 (98.08%) | 601 / 628 (95.70%) |
| Fuzz smoke, `serialize.rs` | 202 / 218 (92.66%) | 11 / 12 (91.67%) | 136 / 143 (95.10%) | 17 / 20 (85.00%) |
| Ordinary tests, `serialize.rs` | 204 / 218 (93.58%) | 11 / 12 (91.67%) | 138 / 143 (96.50%) | 17 / 20 (85.00%) |

These are distinct raw denominators, not a claim of 100% execution or state-space coverage. Mutation results establish the listed behavioral checks; they do not replace remaining allocation-failure, defensive/invariant-path and sustained fuzz-campaign work. Repository publication of coverage summaries is a separate evidence-only follow-up to this review.

### Findings

#### Critical

None.

#### Important

None.

#### Minor

None.

### Plan Adherence

| Requirement | Status | Notes |
| ----------- | ------ | ----- |
| Complete borrowed and owned DOM methods, fresh-writer requirement | Met | Both public methods tested; any prior event, including ignored empty text, consumes freshness before further I/O |
| Compact UTF-8 output and declaration policy | Met | Presence/version/standalone preserved; encoding rewritten; no reconstructed whitespace, BOM, indentation or trailing newline |
| Complete logical tree, ordered XMLNS attributes and empty lexical nodes | Met | Independent byte goldens, full tree/navigation comparisons, constant-output/CDATA/unused-namespace mutations |
| Childless-element folding and emitted-event ceilings | Met | Independent one-event `<r></r>` and three-event empty-CDATA quotas, plus declaration event counts |
| Iterative constant-space navigation and reusable borrowed metadata | Met | Parent/child/sibling traversal; one metadata vector; fallible bounded growth; borrowed normalized strings; 8,192-depth small-stack and 10,000-sibling checks |
| Existing output limits, cumulative owned accounting and output coordinates | Met | All six ceilings below/exact/above; Writer-only reset/double-charge mutations reach serializer fuzz assertions; UTF-16 source offsets do not leak into output offsets |
| Completion, final flush, original I/O causes and panic poisoning | Met | Short/interrupted/zero/oversized/prefix/erroring/panicking sinks; final flush failures; no sink calls after terminal results |
| Fuzz targets, small-valid success, exact quotas and UTF-16 owned path | Met | Strict comparisons exceed the external projection's scope; both blanket rejection paths and accounting faults killed by fuzz |
| External oracle checks input and serialized output | Met | Original expected projection reused; acceptance/semantic/inconclusive dispositions tested at actual command boundary; no pinned binary pass inferred from configured tooling |
| Serialization benchmark and fair-comparison boundary | Met | Borrowed/owned trees built outside timing; counting/buffered sinks and flush observed; matched libxml2 harness/results explicitly pending |
| Documentation, constraints, raw coverage and independent review | Met | Contracts/inventories updated; only memchr 2.8.3 in normal/build/dev graph; unsafe forbidden; five doctests and strict lint checks pass; fresh raw coverage inspected |
| Deferred subtree/repair/formatting/UTF-16 output/convenience framework | Met | No accidental API or dependency expansion into deferred work |

### Verdict

**PASS** -- Zero issues found in `9904278..b181960`. The complete-DOM serialization implementation is ready for local integration with its coverage evidence. This verdict is scoped to the plan and reviewed revision; it does not close the larger replacement, performance or security goals.
