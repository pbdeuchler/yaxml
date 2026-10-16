# Matched parsing baseline review

## Scope and verdict

This review covers the matched DOM and streaming parsing benchmark from base
`a25b8a6` through code freeze
`dfbd5969db786548addd96e123052beaa3ac23b3` and documentation freeze
`233638ab308d4377762c8c5ef779f509c9a5327e`. The governing plan is
[`matched-parse-baseline.md`](plans/matched-parse-baseline.md).

The implementation passes holistic code review. The review found no remaining
critical, important or minor issues after the clock-boundary corrections listed
below. This is a scoped code-quality verdict. It is not performance evidence and
does not establish a performance advantage over libxml2.

Production crate sources, Cargo metadata, fuzz targets and existing benchmark
targets are byte-identical to `a25b8a6`. The reviewed changes add external
benchmark helpers, their driver and tests, CI smoke coverage, and documentation.

## Clock and observation boundaries

The final implementation measures the lifecycle described by the plan:

- DOM parse, public traversal and destruction use separate clocks. The primary
  DOM lifecycle is parse plus destruction; traversal is reported separately.
- Streaming clocks include parser and observer construction, complete event
  consumption, terminal checks and observation. Destruction remains separate.
- The C DOM helper installs and clears the process-global diagnostic hook outside
  the parse clock. Per-parser reader and SAX callback setup stays inside the
  streaming parse clock.
- C DOM traversal creates a fresh logical observer after validation and inside
  the traversal clock, matching the Rust observer lifecycle.
- Buffered-reader cursor completion and SAX result, depth, doctype and input
  completion checks occur before the streaming clock ends.
- Rust uses `black_box` on completed documents and observations before ending
  their clocks. C materializes every observation field through a volatile sink
  and an `atomic_signal_fence` before the ending timestamp. The fixed anchor cost
  is included in the reported phase.

Apple Clang 21 at `-O3` and rustc 1.95 release LLVM IR retained the observation
work before the clock even before the explicit anchors. The anchors make this
boundary deliberate across later compiler and code-shape changes; this review
does not claim that either compiler had eliminated the work.

The clock corrections landed in these integration commits:

- `054c6f8871f13d95b03b53c0e26b1af8c63a0d76` moves streaming setup inside its
  clock and C DOM global diagnostic setup outside its parse clock.
- `907eab438e0b757e13a57628e55da3bba0951e03` includes fresh C DOM observer
  initialization in traversal time.
- `dfbd5969db786548addd96e123052beaa3ac23b3` anchors observations and moves
  terminal streaming checks inside parse time.
- `233638ab308d4377762c8c5ef779f509c9a5327e` records those boundaries and their
  fixed costs in the plan.

## Mutation review

Four deliberate faults were applied one at a time in the isolated review
worktree. Each test failed as expected, and each mutation was restored before
the next one.

| Fault | Expected and observed detection | Retained log and SHA-256 |
| --- | --- | --- |
| Removed digest comparison while retaining counter comparison | The controlled semantic-corruption driver test failed. | `/private/tmp/yaxml-parse-benchmark-review-logs/mutant-1-hash-gate.log` — `b774672371ea5edfe536b9c064adfc1d0041d57d3787788f3dce547c7bf0f959` |
| Published the first row before validating the complete pair | The publication test detected the leaked half-pair. | `/private/tmp/yaxml-parse-benchmark-review-logs/mutant-2-publication-boundary.log` — `876b86fe3f84c2754a9714951f92fac53bef191167e23e7ee0c353f14f2e33a8` |
| Dropped streaming processing-instruction observation | The cross-mode semantic test failed for the reader path with zero rather than two PIs. | `/private/tmp/yaxml-parse-benchmark-review-logs/mutant-3-rust-pi-observer.log` — `02365ce7e301425d5de872cb0e42de09efd1f11121e23f57dc5f058bc1e8f05a` |
| Suppressed C DOM namespace-declaration observation | The pinned helper literal anchor failed with zero rather than two namespaces. | `/private/tmp/yaxml-parse-benchmark-review-logs/mutant-4-c-dom-namespace.log` — `b23edb2201a817375f63ffcaeec06fe4cae659062e6c22359e20538af214cbff` |

The final review worktree was clean after restoring all four faults.

## Validation evidence

At integration commit `8830f744336afb48dc2ef23aa074542ee11e2d7c`, before
the final clock-only corrections, the full repository run completed 488 tests
with 9 ignored across 40 test groups, plus 11 doctests. Strict all-target Clippy
and formatting checks passed.

After the clock corrections, focused validation covered the complete changed
surface: 12 distinct helper and driver unit tests, plus 7 tests in the CLI test
binary: 3 actual-command tests and 4 shared protocol units that Cargo ran a
second time. The actual-command tests include the pinned libxml2 matrix and the
controlled driver failure gate. Strict C compilation, strict all-target Clippy
and formatting also passed. The final C command used the pinned libxml2 2.15.4
static archive and these flags:

```text
-std=c11 -O3 -DNDEBUG -Wall -Wextra -Werror -pedantic -Wconversion -Wshadow
```

The final independent pinned-C CLI log is
`/private/tmp/yaxml-parse-benchmark-review-logs/final2-c-cli.log`, SHA-256
`f3944b2c81c9805a2610015f8a8344653fec058c521efd8b786bedba22695ef9`.
The final controlled-driver log is
`/private/tmp/yaxml-parse-benchmark-review-logs/final2-driver-fault-gates.log`,
SHA-256
`1931f2092577a3a31ea7f4d0b434ac4e38ffb6072960543f1ceaf2c83d5a4c54`.

## Completed performance evidence audit

The retained run at
`/private/tmp/yaxml-parse-baseline-evidence-233638a` passed the immutable
evidence gate. It used documentation freeze
`233638ab308d4377762c8c5ef779f509c9a5327e`, whose tree is
`47ac283ea5397b88527768833fa4006bf7247057` and whose `src` tree is
`34f9acb22ae65b99fcea2cc1cc52e9ee6e036b3f`. Both recorded working-tree status
files are empty. An independent audit matched all 229 tracked source entries to
the bytes of their Git blobs and confirmed that the source manifests were
identical before and after the run.

The reference was freshly extracted from the retained libxml2 2.15.4 archive,
SHA-256
`98087fd181d9070724f3fbc65c7377db03038eb92bd882374daff44940138821`.
An independent second extraction matched all 4,426 paths, sizes and hashes in
the retained reference-source manifest. The retained CMake build is Release and
uses `-O3 -DNDEBUG`; the C helper adds the strict warning flags listed above.
Rust used rustc 1.95.0 with `opt-level=3`, thin LTO and one codegen unit. The
pre-run and post-run binary manifests are identical. Their four identities are:

- outer Rust driver:
  `4894d0c6b25fdccbd66f9fd0e99999d813767a52789285b733319e4ed919812e`
- Rust benchmark helper:
  `33ce3f27e7e319169569394f5bffe4b1729f65ee5899b4e41e0d218f99f00943`
- C benchmark helper:
  `ce9c9ec97f0c2443322df7f4cbaaa4ced83399a602d5f04d3a627bc1cb3657ff`
- fresh static `libxml2.a`:
  `a7dec2ded28b38e5025cc273427a3950707742b4b9dfc181a6b60d41b3e7cf32`

The independent audit reconstructed the required 65-workload inventory and all
130 commands. It validated 130 calibration rows, recomputed every calibrated
iteration count and work bound, and validated 455 retained pairs comprising 910
rows. Every retained file has seven pairs; odd pairs run yaxml first and even
pairs run libxml2 first. Requested bytes, iterations, chunks, literal semantic
counts and semantic hashes match for every pair and surface. It regenerated all
65 summary rows, 195 phase rows and the JSON analysis byte for byte from the raw
rows. All 143 entries in the retained result manifest still match their file
hashes. The identity marker precedes analysis, and `baseline-complete` was
created only after analysis succeeded.

The principal retained-manifest hashes are:

- tracked source manifest:
  `148b7ac8a92e78d7ba3d10a04deb47df2236d04bd6fbe3306a68cf25ce57ad59`
- reference-source manifest:
  `59ae81487f477e5c0e874c9b82f8633d138ac5754906bcbbee8e48be36a5e7df`
- binary manifest:
  `c6c26b42281253d79a4a1214660a998071867bb55e496d5ad601f2264714b4bb`
- recipe manifest:
  `817604d86b33a89e1dad72caa035d85859721f1870b7118cb27b7ed6ff6e8fa0`
- input manifest:
  `748c08357dc1821043d5fb69aabc91e04b8cbded75a2e629e2048b27039615bd`
- result manifest:
  `2c5b0fcfea71375fb8d04fbb2161b838363e16cd29736dfb43dddc5b28b85734`
- matrix, summary, phases and analysis:
  `c1c1611155494e2053aeacc2fc1c776c5da79bb904a7494f572e32df5b5d49c0`,
  `351a49dc06fbe8ca3f6f6211a9059b290571c1ce45af737a39340f92734e2eaa`,
  `95bec1a31c1fc1cb343f23934e6384133b27eba600d46729fe86077f5e6e8a55`
  and
  `f61ef2332d43536aae4fb6c750eb94667fed9ab3c4ca13c4095f0fda638a0e29`

The independent audit script is
`/private/tmp/yaxml-parse-baseline-audit-233638a.rb`, SHA-256
`7f50c16b5a9ac1690c9c66b3a36b35a84d6f38c5bfdc2618d8d903fd8a810653`.
Its PASS log is `/private/tmp/yaxml-parse-baseline-audit-233638a.log`, SHA-256
`6b8316c7a77a08c41137b5de07b564d28fade6f805df4b307ebcaa0f9647a73b`.

The outer run stderr contains one `system_profiler` Activation Lock timeout
warning emitted before the builds. The benchmark matrix requires every driver
invocation to succeed with empty stderr, so helper execution itself was clean.
The comparison still has seven samples per workload and includes phase-specific
clock and observer costs; small phase ratios can be noisy. Descriptively, yaxml
has the lower median lifecycle time in 19 workloads, libxml2 in 42, and 4 fall
within a symmetric five-percent band. These counts characterize this retained
machine and matrix. They do not establish across-the-board superiority.
