## Holistic Review

**Plan:** `/private/tmp/yaxml-pull-review/docs/plans/buffered-pull.md` ([repository copy](plans/buffered-pull.md))
**Base:** `0fd669d` (explicit); reviewed production head `4639d91454005a7b21372c33a2594271b936c275` (equivalent integration commit `c1d1e12`); integration evidence inspected at `d02b2e2`.
**Files Changed:** 23
**Commits:** 8 production/plan commits, excluding review and validation-report commits.

### Summary

The bounded pull implementation follows the plan and shares decoding, tokenization, normalization, and namespace semantics with push parsing without an event queue or duplicate parser. All 23 changed files were read in full, including surrounding code and the final six-line UTF-8 fast path; correctness, security, elegance, simplicity, design boundaries, Rust idioms, plan adherence, and test sensitivity were evaluated. Exact source consumption, lending lifetimes, poison states, UTF-16 carry, cumulative quotas, and independent fuzz goldens have concrete tests, and six deliberate semantic mutations were caught. Final coverage and balanced performance evidence were inspected: execution coverage remains below 100%, several push workloads remain slower than before the milestone, and no matched libxml2 performance win is established.

### Mutation Results

**Baseline:** green

The working tree was clean before mutations. `cargo test --all-targets` passed 183 tests, with six existing tests ignored, and completed both benchmark smoke runners. `cargo test --doc` passed four ordinary examples and the lending compile-fail example; `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` also passed. `cargo tree --all-features` contained only `memchr 2.8.3` beneath yaxml, and the source audit found no unsafe operations.

Predictions were made before execution. Each mutation was restored immediately after its run, and a clean `git status --porcelain` was confirmed before proceeding.

| # | Mutation | Location | Predicted failures | Actual failures | Verdict |
| - | -------- | -------- | ------------------ | --------------- | ------- |
| 1 | Consume `consumed + 1` instead of the exact raw prefix. | `src/buffered.rs:180` | Exact-consumption and unread-suffix tests, especially `event_delivery_consumes_only_its_prefix_and_documented_text_lookahead`. | All 11 `buffered` tests failed, including that test; 44 private tests passed. Cargo stopped at this failing binary, so later I/O/fuzz binaries were not executed in this run. | caught |
| 2 | Discard the saved `<` lookahead after completing text. | `src/tokenizer.rs:224` | `suspension_retains_completed_tokens_and_replays_lookahead_once`, independent buffered goldens, and `pull_schedules_and_independent_policy_boundaries`. | Five private, four buffered, one buffered-I/O, and 12 fuzz tests failed, including all three predicted tests. | caught |
| 3 | Advance the UTF-16 pending cursor to its end, dropping the unconsumed UTF-8 scalar suffix. | `src/encoding.rs:155` | `utf16_scalar_is_charged_once_before_suspension_and_carry_survives`. | That private test failed; the other 43 private tests passed. | caught |
| 4 | Replace every configured pull input quota with zero, rejecting bounded valid documents. | `src/buffered.rs:69` | `every_ceiling_is_inclusive_and_cumulative_across_returned_events`, `pull_schedules_and_independent_policy_boundaries`, and fuzz `known_acceptance`. | All 11 buffered and 13 fuzz tests failed, including the predicted tests. | caught |
| 5 | Leave the reader Active before source operations instead of installing its poison state. | `src/buffered.rs:106` | `mid_token_io_failures_preserve_the_original_error_without_partial_events`, `recovered_source_panics_poison_without_later_fill_or_consume`, and pull fault schedules. | Three buffered-I/O and 15 fuzz tests failed, including the predicted tests and the exact-quota I/O probe test. | caught |
| 6 | Let settled UTF-16 enter the new UTF-8 fast path by replacing the encoding predicate with `encoding.is_some()`. | `src/encoding.rs:135` | Decoder boundary tests, `both_orders_every_split_preserve_logical_values_names_and_raw_spans`, and independent buffered goldens. | Three private decoder, ten buffered, and all ten UTF-16 tests failed, including the predicted cases. | caught |

Mutation 2 used `cargo test --no-fail-fast --lib --test buffered --test buffered_io --test fuzz_smoke`; mutation 3 used `cargo test --lib`. Mutations 4 and 5 used the relevant public and fuzz binaries with `--no-fail-fast`. Full outputs were recorded under `/private/tmp/yaxml-pull-review-mutation-{1,2,3,4,5}.log` during review.

Rows 1–5 record the original `22d9d86` source locations. Mutation 6 ran after cherry-picking `4639d91` as local commit `8faf2bd`; its suite was `cargo test --no-fail-fast --lib --test utf16 --test buffered`, with output recorded in `/private/tmp/yaxml-pull-review-mutation-6.log`.

The original restoration run passed all 82 focused tests. After the fast-path mutation was restored, `cargo test --lib --test buffered --test buffered_io --test stream --test utf16 --test encoding_errors --test callback --test io --test fuzz_smoke` passed all 112 tests; formatting and strict all-target Clippy also passed again. No mutation survived and no production or test edits remain from the protocol.

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
| Small lending `BufferedReader<BufRead>` API; no constructor I/O or mutable source accessor. | Met | Returned event borrows the retained token; the compile-fail example proves advancement cannot overlap that borrow. Caller copies survive advancement. |
| One shared resumable engine without an owned event mirror or transient DOM. | Met | Concrete `Stream` owns the existing semantic state and bounded tokenizer; push drives the same token-ready/release mechanism. Public event/name ownership was preserved. |
| Exact original-byte consumption, saved text lookahead, unread source suffix, and construction-relative positions. | Met | Independent ledgers cover one-byte through large windows, prepositioned sources, early extraction, malformed input, and UTF-8/UTF-16. |
| Constant decoder carry and unchanged scalar quota/error ordering. | Met | UTF-16 units, surrogate pairs, scalar suffix suspension, incomplete EOF, and decoded-total precedence have explicit tests. The otherwise hidden carry loss was detected by mutation 3. |
| Final steady-state UTF-8 optimization preserves the shared engine contract. | Met | `4639d91` enters the existing `forward_utf8` helper only after UTF-8 is selected and its preamble is fully drained. UTF-8 cannot have UTF-16 scalar carry; the helper retains the same consumption, suspension, quota, and failure checks. No allocation, API, validation, or unsafe boundary changes. Mutation 6 proves sensitivity to the encoding guard. |
| Inclusive raw/decoded quotas and cumulative semantic/owned budgets across suspension. | Met | Below/exact/above cases cover all ceilings, ignored whitespace, BOM exclusion, normalization, and lookahead. Exact raw quota probes EOF or the original I/O error without consuming excess bytes. |
| Bounded token and active-scope retention independent of sibling history. | Met | Token reservations remain bounded by raw/decoded ceilings. Private retained-entry/capacity tests inspect tokenizer, structural stack, namespace index, and namespace interner across sibling counts 64/256/1024/4096 and multiple chunk capacities. These are structural tests, not a process-RSS claim. |
| Sticky XML errors, one original I/O cause, repeated EOF, and panic poisoning without later I/O. | Met | Source faults cover mid-text, quoted markup, split surrogate/BOM, late failure after the root, and panics before/after consume progress. Mutation 5 confirms these checks are sensitive. |
| Pull fuzz schedules, independent values/ranges, non-vacuous success and quota checks, lifecycle faults, and corpus/inventory updates. | Met | Live comparisons reuse the independent UTF-16 reference and slice semantics. Small valid inputs must succeed; explicit minima and decoded-limit profiles prevent blanket rejection from passing as a resource exception. Large-input budget exceptions are stated. Source-operation bounds detect stalled parsing. |
| Existing parser, DOM, owned, writer, oracle, UTF-16, and push behavior remains tested. | Met | Full all-target baseline was green; ignored oracle cases remain ignored rather than being reported as executed. |
| Benchmark first-event latency, full throughput, early-stop consumption, token-size/capacity axes, and fair interpretation. | Met | The runner observes semantic fields, checks complete consumption, and labels rejected samples without throughput. Documentation separates caller prefetch, retained state, cumulative allocation work, and RSS; it makes no matched-libxml2 claim. |
| Final raw fuzz/ordinary coverage artifacts and recorded release push before/after evidence before main integration. | Met | Inspected validation commit `d02b2e2`, its checked-in summaries, local raw coverage metadata, and the balanced performance log. All thirteen production source hashes in both coverage runs match the reviewed final source, including the fast path. Remaining coverage and speed gaps are published rather than treated as completed overall project goals. |

The integration [validation report](validation-buffered-pull.md), [fuzz summary](validation/fuzz-coverage-4634a47.txt), and [ordinary-test summary](validation/test-coverage-4634a47.txt) preserve raw denominators with no production exclusions. Fuzz smoke reaches 3,871/4,059 regions, 256/277 functions, 2,571/2,632 lines, and 569/600 instrumented branch outcomes; ordinary tests reach 3,904/4,132 regions, 263/283 functions, 2,631/2,692 lines, and 571/606 branch outcomes. The local artifact directories and revision labels contain `4634a47`, but their per-file SHA-256 manifests match the final production files exactly. Their compiler metadata records nightly `2026-06-06`, Rust 1.98.0-nightly / LLVM 22.1.6, with Apple LLVM 21.0.0 reporting tools. These are execution measurements, not proof of security or completion of the 100% target.

The checked-in [performance summary](validation/push-performance-c1d1e12.txt) matches `/private/tmp/yaxml-pull-perf-summary.txt`. This reviewer independently parsed all 360 rows of `/private/tmp/yaxml-pull-perf-final.log`, checked identical encoded lengths and 50 iterations for each workload, verified all six pre/post/fix order permutations, and reproduced each push median from encoded bytes and elapsed nanoseconds. Each build has six samples; the summary also retains median absolute deviations and ranges. The parent records Rust 1.95.0 on aarch64-apple-darwin / LLVM 22.1.2 and reports no concurrent compilation/tests during sampling; this reviewer checked the recorded results rather than rerunning the timing experiment.

| Push workload | Pre-milestone median MiB/s | Shared-engine median MiB/s | Final fast-path median MiB/s | Final versus pre-milestone |
| ------------- | ------------------------- | -------------------------- | --------------------------- | ------------------------ |
| Tiny | 29.6189 | 28.6029 | 28.6266 | -3.350% |
| Flat | 63.5997 | 58.7555 | 59.7314 | -6.082% |
| Large text | 449.6193 | 448.6006 | 449.6741 | +0.012% |
| Normalized | 60.1427 | 56.5528 | 57.9152 | -3.704% |
| Namespaces | 46.0224 | 45.2421 | 45.2110 | -1.763% |

The fast path recovers 1.661% on flat input and 2.409% on normalized input relative to the first shared-engine implementation. It does not restore every pre-milestone workload. These measurements satisfy the plan's requirement to record before/after push behavior; the plan imposes no numerical non-regression gate, and the remaining regressions are explicit performance work. This evidence supplies no matched libxml2 comparison or claim of an improvement across every workload.

### Verdict

**PASS** -- Zero issues found. Implementation is ready.
