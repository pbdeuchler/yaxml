# SourceOrigin parsing performance evidence audit

## Verdict

**PASS.** The retained full A/B run contains the exact 65-workload inventory, seven counterbalanced samples per workload, 455 complete pairs, and 910 valid raw rows. Every published summary row recomputes exactly from the raw parse-plus-drop measurements. All semantic observations match the frozen `233638a` references in every sample. Source, executable, runner, matrix, input, and reference artifacts remained identical from the wrapper's before snapshot through its successful completion marker.

The evidence contains no median regression above 2%. The largest candidate slowdown is the tiny DOM row at 1.7933%; 64 of 65 rows are within 2%, and the remaining row is an improvement. The text/push/chunk-7 row improved from 2,472,361.6 to 2,311,600.0 ns per document, a `base / candidate` ratio of 1.0695. These are descriptive medians from seven local samples. They do not establish statistical significance, general performance superiority, or a libxml2 comparison.

## Frozen identities

- Nominal baseline source: `3726b3ddcbc0cc6c6845ff4a17801dc8ba460ff9`, 265 tracked files.
- Retained baseline helper source: `e3dafa83ca2f7b2430a0613824010c337b8386ec`, production tree `408df38ccc35a3a3f2cab1cdd5b86734818d3490`.
- Candidate source: `b2f20205d1fb5970476342cb6d5f749567fd8f18`, production tree `28ac931425855790f29b6ab25fa6683e5fcc3aa7`, 270 tracked files.
- Baseline helper SHA-256: `1c35473ab41905a231ced141497550c12f24b2426c7acc4402e9fc0fd3defdfe`.
- Candidate helper SHA-256: `7d5fdff20eaec293ec9e50225fc347ea92e07cd236309b11d69d8a1b1e977182`.
- Matrix-runner SHA-256: `d1b685ee90b19021215e33341813bec12c652ecfb0272fea0857f6dae2332f31`.
- Frozen matrix SHA-256: `c1c1611155494e2053aeacc2fc1c776c5da79bb904a7494f572e32df5b5d49c0`.

The baseline helper's 21 relevant paths—`Cargo.toml`, `Cargo.lock`, three parse-helper files, and 16 production source files—are byte-identical between `e3dafa8` and nominal main `3726b3d`. The earlier estimate of 20 was inherited from the pre-Name baseline, which had one fewer production source file. The candidate union contains 22 paths because it adds `src/origin.rs`; the only eight relevant candidate changes are `src/{buffered,dom,lib,origin,owned,push,reader,stream}.rs`. The parse helper, observer, dependency manifests, and all other production files are unchanged.

The candidate build's complete tracked-source manifest is identical before and after compilation and exactly equals the timing wrapper's candidate source manifest. Both source manifests in the timing identity equal Git archives of their recorded commits. At the requested pre-documentation checkpoint, the candidate integration worktree was clean at `b2f2020`, `HEAD:src` was `28ac931`, and `src`, Cargo manifests, benches, and examples matched that commit.

Every artifact recorded by `identity-before.json` still exists with the recorded SHA-256, including both helpers, runner source and executable, orchestration script, matrix, nine input files, frozen calibration/reference rows, and frozen summaries. `identity-before.json` and `identity-after.json` are byte-identical. The two helper version files are byte-identical and report `YAXML-PARSE-BENCH-1`, engine `yaxml`, version `0.1.0`.

## Build profile

Both helpers were built with rustc 1.95.0 (`59807616e`, LLVM 22.1.2) for `aarch64-apple-darwin`. Their retained compiler records are identical. Their build scripts and verbose compiler invocations establish release opt-level 3, thin LTO, one codegen unit, stripped debug information, disabled debug assertions and overflow checks, unwind panic behavior, and empty Rust flags. Each build record binds its helper binary and build script by SHA-256 and records clean source state before and after compilation.

The retained Cargo host record identifies macOS 27.0.1 on arm64. This is a same-machine, yaxml-to-yaxml comparison.

## Raw and summary audit

The matrix has 65 unique `(fixture, mode, chunk)` workloads. The retained runner performs one one-iteration semantic preflight per helper for each workload before seven measured samples. Within every workload, odd samples run baseline then candidate; even samples reverse that order. The audit checked both physical row order and the `order` field for all 455 pairs.

For all 910 raw rows, the audit checked:

- exact metadata and protocol field schemas, with no missing or duplicate fields;
- known fixture/mode/chunk identity and fixed bytes, iterations, and chunk size from the frozen matrix;
- numeric, nonnegative phase times and counters and a 16-hex-digit observation hash;
- exact element, attribute, namespace, text-byte, comment, PI, and hash values against the frozen reference row;
- equal semantic observations within every baseline/candidate pair;
- exactly one baseline and one candidate row for samples 1 through 7.

The measured summary metric is `(parse_ns + drop_ns) / iterations`, in nanoseconds per document. `observe_ns` is excluded from the performance metric, but observation work executes and its result is verified. The audit independently sorted each distribution, recomputed min/median/max and `base / candidate`, and byte-matched the 65 published rows at four-decimal precision. It also matched all 65 ordered stdout completion lines to the recomputed ratios.

Root clarification: DOM observation has its own excluded `observe_ns` clock.
Streaming observation executes inside `parse_ns`, with `observe_ns` zero, so it
remains included in streaming primary time. Cross-mode clocks measure different
work.

The runner writes `results/complete` only after every workload and flush succeeds. It contains `workloads=65 pairs=455`; the outer wrapper wrote `identity-verified` only after the runner exited successfully, stderr was empty, and the second identity snapshot equaled the first.

## Distribution summary

Candidate deltas below are `candidate / baseline - 1`, using medians of parse-plus-drop time per document.

| Workload | Baseline median ns | Candidate median ns | Candidate delta |
| --- | ---: | ---: | ---: |
| tiny / DOM / 8192 | 489.4378 | 498.2148 | +1.7933% |
| unicode / owned / 8192 | 2,135,243.7037 | 2,168,023.3333 | +1.5352% |
| namespaces / owned / 8192 | 3,445,797.8500 | 3,497,831.2000 | +1.5101% |
| namespaces / push / 7 | 5,203,789.5556 | 5,281,525.5556 | +1.4938% |
| text / push / 7 | 2,472,361.6000 | 2,311,600.0000 | -6.5023% |

Of 65 workload medians, 31 are slower and 34 are faster in the candidate; all are non-equal at retained precision. The small differences near zero should be treated as noise without additional repeats. The text/push/chunk-7 improvement is the only row outside the ±2% band and is consistent with the changed push metadata path, but this run alone does not establish causality.

## Scope and artifacts

No timings, builds, tests, coverage, or XMLTS jobs were rerun for this audit. The audit read the frozen evidence and current immutable artifacts only. It does not assess libxml2, memory use, tail latency, statistical confidence, or future generated-provenance paths.

Audit artifacts are under `/private/tmp/yaxml-source-origin-performance-audit-b2f2020`:

- `audit.rb` and clean `audit.stdout`/`audit.stderr`;
- `recomputed-summary.tsv` for all 65 workloads;
- `identity-audit.tsv`, `relevant-source-delta.txt`, and `build-provenance.txt`;
- `claim-checks.tsv` and `extremes.tsv`;
- `SHA256SUMS` freezing the audit outputs.
