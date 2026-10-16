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
surface: 12 helper and protocol unit tests, all 7 actual CLI tests including the
pinned libxml2 test, the controlled driver failure gate, strict C compilation,
strict all-target Clippy and formatting. The final C command used the pinned
libxml2 2.15.4 static archive and these flags:

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

## Performance evidence gate

The code review does not close the immutable performance-evidence gate. A
retained run must verify that tracked helper sources equal their committed blobs,
record exact release build commands, compiler and pinned-reference identities,
hash all sources, binaries and inputs before sampling, then verify those hashes
again after the full matrix. Every raw paired row and its alternating execution
order must be retained.

Until those artifacts pass independent review, the benchmark is ready to collect
evidence but supports no performance claim.
