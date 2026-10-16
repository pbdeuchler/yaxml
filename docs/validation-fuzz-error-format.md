# Error formatting fuzz-oracle validation

Code snapshot: `dd7fca3`. Production source remains
`6578b39f230ae5d22569235d3dd2e965a27ed47e`; no library behavior changed.

The replay driver now formats actual parser, owned-editor, read-adapter and
writer errors through bounded `fmt::Write` sinks. Each sink rejects either the
first write or the write crossing the final-byte ceiling, then a third accepts
the complete message. Rejections must propagate `fmt::Error`; accepted chunks
must form a prefix of the complete display, and successful output must match
it exactly. This checks deterministic formatting and failure propagation without
freezing the human-readable wording. It does not inject allocator failure.

The checks run on actual input-driven failures, including structural errors
with optional value offsets. Focused oracle tests cover all four public error
types, Unicode output, maximum offsets, an empty display and a deliberately
incorrect display that swallows sink errors.

## Verification

On stable Rust/Cargo 1.95.0, the full all-target suite passed 312 tests with
seven ignored subprocess helpers. The 32-test fuzz-smoke suite took 64.85 s;
the full command including compilation took 88.54 s. Eight doctests, strict
all-target Clippy, formatting and diff checks passed. Independent review of
the five-file code/documentation diff returned PASS with no findings.

Two separate temporary faults in `EditError::fmt` ignored errors from its first
write and its optional offset write. Each fault failed both the focused oracle
test and `structural_pi_name_boundaries_and_each_input_length_are_independent`.
The faults were restored; production files match the starting revision exactly.
Logs use `/private/tmp/yaxml-fuzz-format-` with `mutant-first.txt`,
`mutant-offset.txt`, `model-mutant-first.txt`, `model-mutant-offset.txt`,
`restored.txt`, `model-restored.txt`, `all-targets.txt`, `doc-tests.txt` and
`clippy.txt` suffixes.

## Corpus-only coverage

The existing 19 corpus files ran through the instrumented release replay
executable, without ordinary tests or the new oracle unit tests. The build used
nightly `2026-06-06`, rustc 1.98.0-nightly (`8954863c8`), matching LLVM 22.1.6
tools, default features, `-C instrument-coverage -Z coverage-options=branch`,
LTO disabled and 16 codegen units. All 14 production source paths were supplied;
`lib.rs` has no executable coverage mapping. No production source is excluded.
Target execution, profile merging and reporting emitted no diagnostics.

| Run | Regions | Functions | Lines | Instrumented branch outcomes |
| --- | --- | --- | --- | --- |
| Same corpus before this change (`e487169`) | 4,523 / 5,047 (89.62%) | 304 / 336 (90.48%) | 3,143 / 3,324 (94.55%) | 557 / 692 (80.49%) |
| Same corpus with formatting checks (`dd7fca3`) | 4,525 / 5,047 (89.66%) | 304 / 336 (90.48%) | 3,143 / 3,324 (94.55%) | 557 / 692 (80.49%) |

`error.rs` now reaches all 56 regions, eight functions, 36 lines and two branch
outcomes in this corpus-only run. The two newly reached regions are formatter
error propagation. The [full library table](validation/fuzz-format-corpus-dd7fca3.txt)
retains the remaining gaps. This small corpus run does not supersede the earlier
[full fuzz-smoke coverage](validation/fuzz-coverage-0ef8e77.txt), which used a
larger generated/mutated input set and reached 95.46% of library regions. It is
not a coverage-guided campaign or a 100% crate-coverage claim.

Raw profiles, reports, empty diagnostics, source/driver SHA-256 manifests and
artifact hashes are in `/private/tmp/yaxml-fuzz-format-coverage-dd7fca3`.
The executable is
`/private/tmp/yaxml-campaign-target-format-dd7fca3/release/examples/fuzz_replay`;
its build output is `/private/tmp/yaxml-fuzz-format-coverage-build.txt`.
The pre-change comparison, compiler and matching tool versions are in
`/private/tmp/yaxml-campaign-build-e487169`. The crate still forbids unsafe and
has only the locked `memchr 2.8.3` dependency.
