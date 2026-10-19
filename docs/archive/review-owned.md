## Holistic Review

**Plan:** `docs/plans/owned-dom.md`
**Base:** `4f7fb04`
**Target:** `ee7b6455e01883a633828723aefaa7e838092c0b`
**Files Changed:** 18
**Commits:** 7

### Summary

Read every changed file in full and reviewed the complete diff, beginning with the plan and tests. The immutable owned representation keeps ranges and flat indices rather than self-references, preserves the borrowing API, and shares parser normalization accounting with full replacement pool-capacity requests. No correctness, elegance, simplicity, design, idiomatic Rust, plan-adherence, or test-quality issue was found. The baseline and restored final checks pass; all four deliberate semantic regressions were caught by both ordinary tests and fuzz smoke checks.

### Mutation Results

**Baseline:** green. Final-target all-target tests: 128 passed, 6 intentionally ignored subprocess helpers, explicitly launched by their parent test; doctests: 3 passed. Baseline also executed the benchmark target's debug smoke workloads. Predictions were recorded before each test invocation in `/private/tmp/yaxml-owned-audit-mutation-predictions.txt`.

All mutations ran `cargo test --offline --lib --test owned --test fuzz_smoke --no-fail-fast`. Every mutation was individually restored with `git restore src/owned.rs`, followed by a clean `git status --porcelain`, before the next mutation. No implementation changes remain.

| # | Mutation | Location | Predicted failures | Actual failures | Verdict |
| --- | --- | --- | --- | --- | --- |
| 1 | Charge only replacement-capacity delta | `src/owned.rs:124` | Internal cumulative pool accounting, public aggregate budget, fuzz quota fixture | 3 library tests; public `parser_normalization_and_pool_reservations_share_one_cumulative_budget`; fuzz `owned_storage_growth_and_aggregate_limits` | KILLED |
| 2 | Split Unicode prefix using character count instead of byte offset | `src/owned.rs:141` | Private Unicode view test, public namespace/growth tests, Unicode fuzz fixture | 1 library test; 2 public tests; fuzz `owned_storage_growth_and_aggregate_limits` | KILLED |
| 3 | End source span at closing-tag start | `src/owned.rs:218` | Public goldens, corpus parity and nested range tests; fuzz ranges | 3 public tests; 11 fuzz tests, including generated documents and owned storage fixture; library pool tests stayed green as predicted | KILLED |
| 4 | Default byte constructor always rejects with OwnedBytes | `src/owned.rs:171` | Public valid/error goldens and fuzz small-valid-input guard | 6 public tests; 11 fuzz tests, including `known_acceptance`, corpus replay and owned storage fixture; explicit custom-limit library tests stayed green | KILLED |

Mutation 1 was first run at 47f78df and repeated at final ee7b645 with identical failures, so all four table results are pinned to the final target. Each mutated test command exited 101 for assertion failures rather than compilation failures.

### Findings

#### Critical

None.

#### Important

None.

#### Minor

None.

### Plan Adherence

| Requirement | Status | Notes |
| --- | --- | --- |
| Independent immutable owned storage; no self-reference or source retention | Met | One private UTF-8 pool, stored byte ranges and flat topology. Overwrite/drop source, document moves and thread transfer tested. |
| Planned constructors and complete borrowed views | Met | All planned node/content/navigation APIs exist; names/declarations returned by value, attributes exact-size and borrowed. Empty values remain distinct from absent values. |
| Shared aggregate allocation-work budget | Met | Parser and pool use one counter. Growth charges the full newly requested target before fallible reservation; prior charges remain spent. Exact minima and clamping verified, including mutation 1. |
| Namespace amplification and fallible growth | Met | URI copy is budgeted before copying; no speculative interning shortcut. Every new input-sized structural vector growth is fallible. Metadata and allocator overhead are explicitly outside the string budget. |
| Range validity and Unicode correctness | Met | Qualified names split at UTF-8 byte boundaries; pool ranges survive growth/moves; source ranges include closing tags. Mutations 2 and 3 verify test sensitivity. |
| Iterative navigation/destruction and document-local IDs | Met | Flat arrays, iterative stacks/ranges/successors, deep/wide tests on a small thread stack; foreign in-range ID limitations documented. |
| Logical goldens, corpus/writer parity and all ceilings | Met | Independent values plus complete borrowed/owned comparison; exact cumulative budget boundaries and malformed/unsupported inputs. |
| Fuzz mapping and owned-budget exceptions | Met | Every new public input/view API mapped. Small accepted default inputs must succeed; independent tiny-budget minima prevent undercharging from passing by accepting more. Mutation 4 verifies the exception cannot hide blanket rejection. |
| No unsafe code or extra dependency | Met | Existing crate prohibition retained; no dependency changes. |
| Formatting, strict Clippy, all targets, doctests and independent mutation audit | Met | Final restored checks all exit 0; clean audit worktree. |
| Fuzz-only execution coverage artifact | Parent-owned evidence gate | Not regenerated by this independent review; parent is collecting the ee7b645 fuzz artifact and same-production ordinary coverage. This report makes no execution-coverage percentage claim. |
| Full compatibility, allocator support, performance superiority and 100% fuzz coverage | Explicitly deferred | Documentation continues to state these gaps; this review approves only the owned DOM milestone implementation. |

### Verdict

**PASS** -- Zero issues found. Implementation is ready within the owned DOM milestone scope; the separate coverage artifact remains part of the parent's integration evidence.

Final restored commands (all exit 0):

- `cargo test --offline --all-targets`
- `cargo test --offline --doc`
- `cargo fmt --all -- --check`
- `cargo clippy --offline --all-targets -- -D warnings`
- `git diff 4f7fb04...HEAD --check`
- `git status --porcelain` (empty)

Raw evidence (also copied to ignored local `coverage/owned-audit-ee7b645/`):

- Baselines: `/private/tmp/yaxml-owned-audit-baseline-tests.txt`, `/private/tmp/yaxml-owned-audit-baseline-docs.txt` (47f78df); `/private/tmp/yaxml-owned-audit-ee7b645-baseline-tests.txt` (final target).
- Predictions and restoration record: `/private/tmp/yaxml-owned-audit-mutation-predictions.txt`.
- Final-target mutation logs: `/private/tmp/yaxml-owned-audit-mutation1-final.txt`, `/private/tmp/yaxml-owned-audit-mutation2.txt`, `/private/tmp/yaxml-owned-audit-mutation3.txt`, `/private/tmp/yaxml-owned-audit-mutation4.txt`.
- Final restored checks: `/private/tmp/yaxml-owned-audit-final-tests.txt`, `/private/tmp/yaxml-owned-audit-final-docs.txt`, `/private/tmp/yaxml-owned-audit-final-fmt.txt`, `/private/tmp/yaxml-owned-audit-final-clippy.txt`.
- Clean detached audit worktree: `/private/tmp/yaxml-owned-audit`.
