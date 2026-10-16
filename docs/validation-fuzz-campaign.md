# Coverage-guided campaign validation

A finite LLVM feedback campaign completed 500 testcase attempts against the
release `fuzz_replay` binary at source revision
`e9edcb4eaf5700a1ae0f992d013d3cc93f23ed44`. It completed its case limit in 37
seconds with empty stdout and stderr. The runner retained 79 candidates, marked
421 duplicate outcomes, and recorded 78 feedback-increasing candidates. The run
used 19 baseline seeds and 481 deterministic mutations with PRNG seed 1.

This is a separate measurement from the 95.53% regions in the fuzz-smoke
profile. The smoke and campaign profiles use different executables and run
inputs, while the frozen production denominators are the same. Do not treat the
percentage difference as a coverage regression or improvement. This campaign
reports 13 executable source rows and includes all 14 production source paths
in `--sources`; `lib.rs` has no executable mapping.

An independent journal audit regenerated all 500 candidates from the recorded
seeds and PRNG state, then matched each candidate's lineage, novelty decision,
and retained bytes. It confirmed all 19 seed cases were retained and 60
mutation cases added novel coverage; the other 18 feedback-increasing outcomes
were baseline seed cases. The final corpus has 79 files totaling 9,057 bytes.
An independent LLVM report of the saved cumulative profile reproduced the totals below.
The replay function count for `fuzz_replay::main` was 500, confirming one
process execution per journaled candidate.

After the 19 baseline seeds, the cumulative profile reached 4,560 regions,
308 functions and 557 branch outcomes. Mutations added 185 regions, six
functions and 45 branch outcomes. Independently replaying the 79 retained files
into a separate fresh profile reproduced the full library report byte for byte,
showing that the retained corpus preserves the measured coverage. Those replay
artifacts are in
`/private/tmp/yaxml-feedback-retained-replay-e9edcb4-independent`.

## Per-file coverage

The campaign report counts LLVM regions, functions, executable lines, and
instrumented branch outcomes. Branch outcomes reflect this compiler's coverage
instrumentation; they do not represent every parser state or boolean expression.

| Source file | Regions | Functions | Lines | Branch outcomes |
| --- | ---: | ---: | ---: | ---: |
| `buffered.rs` | 130 / 133 (97.74%) | 8 / 8 (100.00%) | 94 / 95 (98.95%) | 18 / 20 (90.00%) |
| `dom.rs` | 268 / 292 (91.78%) | 33 / 37 (89.19%) | 203 / 209 (97.13%) | 9 / 10 (90.00%) |
| `encoding.rs` | 351 / 366 (95.90%) | 19 / 19 (100.00%) | 272 / 277 (98.19%) | 77 / 84 (91.67%) |
| `error.rs` | 56 / 56 (100.00%) | 8 / 8 (100.00%) | 36 / 36 (100.00%) | 2 / 2 (100.00%) |
| `escape.rs` | 152 / 162 (93.83%) | 10 / 12 (83.33%) | 82 / 84 (97.62%) | 27 / 30 (90.00%) |
| `owned.rs` | 1,145 / 1,211 (94.55%) | 84 / 93 (90.32%) | 812 / 831 (97.71%) | 79 / 84 (94.05%) |
| `parser.rs` | 945 / 1,053 (89.74%) | 42 / 49 (85.71%) | 618 / 648 (95.37%) | 156 / 200 (78.00%) |
| `push.rs` | 179 / 198 (90.40%) | 20 / 20 (100.00%) | 139 / 151 (92.05%) | 20 / 28 (71.43%) |
| `reader.rs` | 294 / 299 (98.33%) | 17 / 17 (100.00%) | 187 / 189 (98.94%) | 59 / 62 (95.16%) |
| `serialize.rs` | 198 / 218 (90.83%) | 11 / 12 (91.67%) | 132 / 143 (92.31%) | 16 / 20 (80.00%) |
| `stream.rs` | 191 / 199 (95.98%) | 22 / 23 (95.65%) | 127 / 134 (94.78%) | 7 / 10 (70.00%) |
| `tokenizer.rs` | 336 / 346 (97.11%) | 17 / 18 (94.44%) | 230 / 234 (98.29%) | 75 / 78 (96.15%) |
| `writer.rs` | 500 / 549 (91.07%) | 23 / 24 (95.83%) | 307 / 315 (97.46%) | 57 / 64 (89.06%) |
| **Total** | **4,745 / 5,082 (93.37%)** | **314 / 340 (92.35%)** | **3,239 / 3,346 (96.80%)** | **602 / 692 (86.99%)** |

The run left 337 regions, 26 functions, 107 lines, and 90 instrumented branch
outcomes uncovered. They remain in the denominator. This campaign is evidence
that cumulative LLVM coverage guided mutation selection; it is not a 100%
coverage claim, a security sign-off, or a substitute for the independent
structural and oracle checks.

## Reproduction inputs and limits

The source checkout was clean at build time. The target used nightly
`2026-06-06`, Rust `1.98.0-nightly (8954863c8 2026-06-05)`, LLVM `22.1.6`,
release optimization, default features, and branch instrumentation. Release LTO
was disabled and codegen-units were set to 16. The resulting target is
`/private/tmp/yaxml-feedback-target-e9edcb4/release/examples/fuzz_replay`,
SHA-256 `7e9afd146ebbce41c7ce07fcbfaaa061a7cb1dba704005cf4a35519db260a4c3`.
The build's verbose rustc invocations show `-C embed-bitcode=no`,
`-C codegen-units=16`, `-C instrument-coverage`, and
`-Z coverage-options=branch`, with no LTO flag. Matching `llvm-profdata` and
`llvm-cov` are version `22.1.6-rust-1.98.0-nightly`.

The command set 500 cases, a 300-second total limit, and PRNG seed 1. Other
limits used their documented defaults: 5 seconds per target, 1 MiB per input,
64 KiB combined target output, 32 MiB per profile, 128 seed files / 8 MiB total
seed bytes, 256 retained files / 16 MiB corpus bytes, and 2 MiB per pending
candidate and metadata. The campaign stopped at `case_limit`, well before the
wall-time limit. The runner is currently Unix-only because it relies on
rename-over-existing-file semantics, creates output roots with mode `0700`, and
its process test uses `/bin/sh`.

The report counts coverage only for `fuzz_replay`'s production library
instrumentation. It excludes harness, dependency, and toolchain code through the
explicit source list. The campaign retains cumulative regions, function, and
branch counts as novelty feedback. It does not provide edge-level feedback,
path-frequency guidance, minimization, parallel runs, or campaign resume.

## Build and campaign failures

The first target-build attempt passed `-C lto=false` through `RUSTFLAGS`, but
`Cargo.toml` still selected thin LTO for the release profile. Apple ld64's LLVM
21 LTO reader rejected the LLVM 22 bitcode. The successful build disabled LTO
through Cargo's `CARGO_PROFILE_RELEASE_LTO=false` override and set
`CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16`; verbose compiler output confirms
`-C embed-bitcode=no`.

The earlier campaign against revision `538fd737acab7f94c835f5b2e19c131213dc57a6`
failed on its first seed because LLVM 22 rejects `--color=never` for the
boolean color option. The preserved stderr states that the value must be `0` or
`1`. The runner now uses `--color=0`, and the complete 500-case run passed with
that spelling. The failed attempt was classified as infrastructure failure and
contributed no accepted campaign cases.

## Retained artifacts

The campaign output is retained under
`/private/tmp/yaxml-feedback-campaign-e9edcb4-500/`. It contains
`campaign-report.txt`, the 500-line `cases.tsv` journal, the 19-seed `seeds.tsv`,
the retained corpus, and final cumulative and raw profiles. The full per-file
LLVM report is
`/private/tmp/yaxml-feedback-build-e9edcb4/campaign-llvm-cov-report.txt`; its
stderr file is empty. The build directory also records the exact command,
revision and clean status, SHA-256 for the binary, all 14 source modules and
harness files, matching LLVM tools, compiler flags, and report/journal/profile
hashes.

The earlier failed campaign output remains under
`/private/tmp/yaxml-feedback-campaign-538fd73-500/`, including its classification,
first seed, raw profile, and stderr. The failed thin-LTO build log is preserved
separately as
`/private/tmp/yaxml-feedback-build-e9edcb4/initial-thin-lto-failure.log`. The
independent journal and profile audits, along with their hash manifest, are in
`/private/tmp/yaxml-feedback-build-e9edcb4/`.

The combined integration suite passed 329 tests with seven ignored subprocess
helpers, plus ten doctests, strict all-target Clippy, formatting and diff checks
on stable Rust 1.95.0. Both the runner and text-chunk implementation passed
independent code review. Deliberate faults were caught for configured ceilings,
identity changes, profile/report validation, publication rollback, evidence
preservation, source selection and the real LLVM color-option spelling.
These tooling checks remain distinct from production-library fuzz coverage.
