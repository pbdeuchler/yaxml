# Name ownership performance evidence audit

## Verdict

**PASS.** The three retained A/B data sets are internally consistent, reproduce their published summaries exactly, preserve the frozen semantic observations, and retain unchanged source, executable, runner, input, and matrix identities from start to finish. The evidence supports a material regression in the candidate's `text`/`push`/chunk-7 row. It does not support a general performance claim or a libxml2 comparison.

## Scope and identities

- Candidate source: `e3dafa83ca2f7b2430a0613824010c337b8386ec`, 247 tracked files.
- Nominal baseline source: `2609aeff4c5287b5fbbdadd15eb55b1cbd296aa0`, 242 tracked files.
- Retained baseline executable source: `7efc22fbe147d0d997b668bd21ac0bc0850d7280`. The audit compared the 20 relevant paths (`Cargo.toml`, `Cargo.lock`, the parse helper/support files, and every `src/` file) with `2609aef`; all bytes match. The intervening changes are documentation only.
- Baseline helper SHA-256: `8fdbb1f7b85eaae4305a07f7a54c3e8ad0a98ca1439c3b47409be5f0f1db5b3d`.
- Candidate helper SHA-256: `1c35473ab41905a231ced141497550c12f24b2426c7acc4402e9fc0fd3defdfe`.
- Frozen 65-row workload matrix SHA-256: `c1c1611155494e2053aeacc2fc1c776c5da79bb904a7494f572e32df5b5d49c0`.
- Compiler: rustc 1.95.0 (`59807616e`, LLVM 22.1.2), target `aarch64-apple-darwin`.
- Profile on both helpers: release, opt-level 3, thin LTO, one codegen unit, no debug assertions or overflow checks, unwind panic, empty rustflags. The candidate build's full tracked-source manifest is identical before and after the build and matches the timing identity manifest.
- Host recorded during audit: macOS 27.0.1 build 26A434, Darwin 27.0.0, arm64. This is a local-machine comparison.

Every artifact listed in each run's identity file still has its recorded SHA-256. Each source manifest exactly matches a Git archive of its recorded commit. Every run's `identity-before.json` and `identity-after.json` is byte-identical. After the run, while the candidate was still at `e3dafa8`, all 247 working-tree files also matched the retained manifest; later documentation changes do not affect this frozen evidence audit.

## Method audit

| Data set | Workloads | Samples per workload | Retained pairs | Raw rows |
| --- | ---: | ---: | ---: | ---: |
| screen | 25 | 5 | 125 | 250 |
| full | 65 | 7 | 455 | 910 |
| repeat | 14 | 15 | 210 | 420 |
| total | 104 | — | 790 | 1,580 |

The runners use the frozen input bytes, iteration counts, modes, and chunk sizes. The 65-row matrix spans 23 to 500,013 input bytes and 6 to 10,000 iterations. Before retained measurements, each helper runs once per workload and must match the frozen observation. Each retained invocation is bounded to 60 seconds and 4,096 output bytes.

Within every workload, odd samples execute baseline then candidate and even samples execute candidate then baseline. The audit checked the physical raw-row order and the declared order field for all 790 pairs. This avoids a case-order/parity cancellation across the matrix.

For all 1,580 raw rows, the audit checked the exact protocol schema, numeric fields, fixed byte/iteration/chunk values, and all seven semantic observations: element, attribute, and namespace counts; text bytes; comment and PI counts; and the observation hash. Every observation equals the frozen reference and its paired implementation.

The reported metric is `(parse_ns + drop_ns) / iterations`, in nanoseconds per document. `observe_ns` is deliberately excluded. Observation work still executes and its result is checked, so the rows establish semantic comparability while measuring parse plus destruction. The audit independently sorted every raw distribution, recomputed min/median/max and the baseline-over-candidate ratio, and matched all 104 published summary rows exactly to four decimal places.

Root clarification: DOM observation has its own `observe_ns` clock; streaming
observation runs inside `parse_ns` and sets `observe_ns` to zero. Thus streaming
primary time includes the observer. Cross-mode clocks do not describe identical
work.

All three runners exited successfully, emitted the exact completion inventory, had empty stderr, and reported the expected helper protocol/version. Inputs were checked before and after every retained invocation. The wrappers also proved source and artifact identities unchanged across each complete run.

## Results relevant to the reported regressions

Candidate deltas below are `candidate / baseline - 1`, using medians of per-document parse-plus-drop time.

| Data set | Fixture / mode / chunk | Iterations | Baseline min / median / max ns | Candidate min / median / max ns | Candidate delta |
| --- | --- | ---: | ---: | ---: | ---: |
| full | text / push / 7 | 25 | 2,207,561 / 2,209,423 / 2,216,947 | 2,463,825 / 2,468,515 / 2,475,273 | +11.73% |
| repeat | text / push / 7 | 25 | 2,209,645 / 2,213,683 / 2,260,895 | 2,469,557 / 2,475,832 / 2,513,847 | +11.84% |
| repeat | text / buffered / 7 | 16 | 2,744,037 / 2,748,745 / 2,764,898 | 2,857,675 / 2,869,573 / 2,939,560 | +4.40% |
| repeat | deep / reader / 8192 | 2,159 | 15,470 / 15,504 / 15,683 | 16,168 / 16,230 / 16,368 | +4.68% |

The full run independently shows +4.53% for `text`/`buffered`/7 and +4.64% for `deep`/`reader`/8192. The 15-sample repeat confirms the directions and magnitudes. In the full 65-row matrix, `text`/`push`/7 is the only row at or above a 5% slowdown. The screen used chunk 8192 and did not expose these small-chunk regressions; its largest slowdown was +0.38%.

These distributions establish a retained performance debt for the ownership change, most clearly the approximately 12% `text`/`push`/7 regression. They do not identify its cause. The sample sizes, fixed local host, and absence of an environmental-noise model limit broader inference. The evidence compares two yaxml builds only.

## Reproduction and audit artifacts

The original orchestration is retained at:

- `/private/tmp/yaxml-name-ownership-matrix.rs`
- `/private/tmp/yaxml-name-ownership-run.rb`
- `/private/tmp/yaxml-name-ownership-focused.rs`
- `/private/tmp/yaxml-name-ownership-focused-run.rb`
- `/private/tmp/yaxml-name-ownership-{screen,full,repeat}`

This audit directory contains `audit.rb`, its clean stdout/stderr, a complete recomputed summary, identity and build-provenance checks, the exact claim rows, and the baseline-to-main source equivalence result. `SHA256SUMS` freezes those audit artifacts.
