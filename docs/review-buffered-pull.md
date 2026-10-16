## Holistic Review

**Plan:** `/private/tmp/yaxml-pull-review/docs/plans/buffered-pull.md` ([repository copy](plans/buffered-pull.md))
**Base:** `0fd669d` (explicit); reviewed head `22d9d86f2f3f110240b97f96c43a09933ae424c4`
**Files Changed:** 23
**Commits:** 7

### Summary

The bounded pull implementation follows the plan and shares decoding, tokenization, normalization, and namespace semantics with push parsing without an event queue or duplicate parser. All 23 changed files were read in full, including surrounding code; correctness, security, elegance, simplicity, design boundaries, Rust idioms, plan adherence, and test sensitivity were evaluated. Exact source consumption, lending lifetimes, poison states, UTF-16 carry, cumulative quotas, and independent fuzz goldens have concrete tests, and five deliberate semantic mutations were caught. This is an implementation review: final raw coverage publication and release performance evidence remain parent-owned integration gates and are not represented here as completed measurements or a libxml2 performance win.

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

Mutation 2 used `cargo test --no-fail-fast --lib --test buffered --test buffered_io --test fuzz_smoke`; mutation 3 used `cargo test --lib`. Mutations 4 and 5 used the relevant public and fuzz binaries with `--no-fail-fast`. Full outputs were recorded under `/private/tmp/yaxml-pull-review-mutation-{1,2,3,4,5}.log` during review.

After the final restoration, `cargo test --lib --test buffered --test buffered_io --test fuzz_smoke` passed all 82 tests. No mutation survived and no production or test edits remain from the protocol.

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
| Inclusive raw/decoded quotas and cumulative semantic/owned budgets across suspension. | Met | Below/exact/above cases cover all ceilings, ignored whitespace, BOM exclusion, normalization, and lookahead. Exact raw quota probes EOF or the original I/O error without consuming excess bytes. |
| Bounded token and active-scope retention independent of sibling history. | Met | Token reservations remain bounded by raw/decoded ceilings. Private retained-entry/capacity tests inspect tokenizer, structural stack, namespace index, and namespace interner across sibling counts 64/256/1024/4096 and multiple chunk capacities. These are structural tests, not a process-RSS claim. |
| Sticky XML errors, one original I/O cause, repeated EOF, and panic poisoning without later I/O. | Met | Source faults cover mid-text, quoted markup, split surrogate/BOM, late failure after the root, and panics before/after consume progress. Mutation 5 confirms these checks are sensitive. |
| Pull fuzz schedules, independent values/ranges, non-vacuous success and quota checks, lifecycle faults, and corpus/inventory updates. | Met | Live comparisons reuse the independent UTF-16 reference and slice semantics. Small valid inputs must succeed; explicit minima and decoded-limit profiles prevent blanket rejection from passing as a resource exception. Large-input budget exceptions are stated. Source-operation bounds detect stalled parsing. |
| Existing parser, DOM, owned, writer, oracle, UTF-16, and push behavior remains tested. | Met | Full all-target baseline was green; ignored oracle cases remain ignored rather than being reported as executed. |
| Benchmark first-event latency, full throughput, early-stop consumption, token-size/capacity axes, and fair interpretation. | Met | The runner observes semantic fields, checks complete consumption, and labels rejected samples without throughput. Documentation separates caller prefetch, retained state, cumulative allocation work, and RSS; it makes no matched-libxml2 claim. |
| Final raw fuzz/ordinary coverage artifacts and recorded release push before/after evidence before main integration. | Partial | Parent-owned integration work was still in progress at review time. The parent reported preliminary matched push results and a reproduction follow-up, but this reviewer did not independently validate those timings or inspect final coverage artifacts. Publish them against the final production revision, retain raw denominators without new production exclusions, and complete that gate before main is fast-forwarded. This pending execution step is not a discovered implementation defect. |

### Verdict

**PASS** -- Zero issues found. Implementation is ready.
