# Bounded feedback-guided fuzz campaign

## Goal and boundary

Add one finite, reproducible coverage-guided campaign command using the
repository's existing safe replay target and process overlay. This closes the
current gap between deterministic mutation smoke tests and feedback-guided
mutation. It does not claim 100% execution coverage or replace a sustained
external fuzzing service. Preserve the current dependency set and
`unsafe_code = "forbid"`; do not add libFuzzer bindings, an unsafe export, a
Cargo dependency, or a tooling crate.

The implementation is limited to `examples/fuzz_campaign.rs`,
`examples/support/fuzz_campaign.rs`, their focused tests, and the fuzz README
section needed to document invocation and evidence. Do not modify unrelated
validation/review/benchmark artifacts. The core crate currently has its
existing `memchr` dependency; this work adds no dependency. Reuse
`examples/support/process.rs` for bounded process execution and environment
overlay. The measured target is the
existing `fuzz_replay` example built with the pinned coverage toolchain and
`-C instrument-coverage -Z coverage-options=branch`, optimized with LTO disabled
and the repository's normal fuzz driver. Exact compiler and tool versions must
be recorded; the profiler and report tools must match the compiler LLVM major.

## Campaign contract

The command accepts a target binary, `llvm-profdata`, `llvm-cov`, a seed
directory, a fresh output path, a source revision label, and finite case/time
limits. Defaults and hard maxima are frozen here; the CLI may lower a limit
but must reject values above the maximum.

The CLI limit flags are `--max-cases`, `--max-seconds`,
`--target-timeout-seconds`, `--max-input-bytes`, `--max-output-bytes`,
`--max-profile-bytes`, `--max-seeds`, `--max-seed-bytes`,
`--max-corpus-files`, `--max-corpus-bytes`, and `--max-pending-bytes`; omitted
flags use the defaults in the table.

| Limit | Default | Hard maximum |
| --- | ---: | ---: |
| Cases, including baseline seeds | 500 | 10,000 |
| Total campaign wall time | 300 s | 3,600 s |
| Per-target process time | 5 s | 30 s |
| Input bytes per testcase | 1 MiB | 1 MiB |
| Combined target stdout/stderr | 64 KiB | 1 MiB |
| Any raw, pending-merge, or cumulative profile | 32 MiB | 128 MiB |
| Total transient profile storage (raw + current + pending merge) | 96 MiB | 384 MiB |
| Seed files | 128 | 128 |
| Total seed bytes | 8 MiB | 8 MiB |
| Retained corpus files (including seeds) | 256 | 1,024 |
| Retained corpus bytes | 16 MiB | 64 MiB |
| Pending candidate plus metadata | 2 MiB | 2 MiB |

Reject an existing output path. Create the output root atomically with mode
`0700` on Unix before writing campaign files. Resolve and sort regular seed
files deterministically; reject symlinks, unreadable files, empty seed sets,
seeds over the input limit, and seed/corpus count or byte totals over the
ceilings. Record canonical source paths and bounded seed names in the manifest.

Every candidate is persisted before execution with its bytes, parent identity,
seed identity, mutation operation, and deterministic PRNG state. Each run
invokes the replay target in a fresh process, with an isolated unique
`LLVM_PROFILE_FILE` path and a cleared/explicit environment overlay. The input,
process output, profile, case count, total wall time, and stored corpus all have
finite limits from the table. Total wall time is an orchestration bound checked
between operations; clamp each target timeout to remaining time where possible.
Filesystem operations and process spawn are not hard-deadlined. Use the
existing process helper's timeout and combined stdout/stderr cap. It returns
an error without captured output on timeout or output overflow, so preserve the
candidate and typed error and include captured bytes only when available.
Check raw and pending cumulative profile size after each producer exits and
before merge/rename; reject an over-limit or missing profile. Keep only the
current cumulative profile, one pending merged profile, and the current raw
profile. Remove a raw profile after a successful merge; do not archive every
raw profile. The aggregate transient profile cap is three times the per-profile
limit. These are observed storage ceilings, not an operating-system disk quota
against a misbehaving child.

Reject any target stderr, nonzero status, timeout, output-limit error, missing
or invalid raw profile, failed merge, malformed report, report diagnostic,
changed denominator, or source, target, or tool identity change. Stop on the first
failure. Preserve the bounded triggering candidate and bounded captured logs,
and classify target crash, timeout, or output-limit failure separately from
campaign infrastructure failure. Clean only pending temporary files owned by
the campaign using RAII; retain completed cases and failure evidence.

Merge each successful raw profile exactly once into a newly written cumulative
profile. Generate a library-only LLVM coverage summary from the fixed target
binary and cumulative profile. Parse the exact LLVM 22 summary header, expected
production source rows, and `TOTAL` row. The production file list is the
explicit sorted set of every regular `src/*.rs` module in the source tree,
including currently unexecuted modules; reject missing, extra, or
duplicate rows. Freeze all four `TOTAL` denominators (regions, functions,
lines, instrumented branches) from the first valid report and reject any later
change. Lines remain diagnostic only. Retain a candidate only when cumulative
covered-region, executed-function, or covered-branch count increases; otherwise
record it as a duplicate. This is set-like novelty over LLVM's cumulative
covered counts, not edge-level novelty, and therefore makes no claim about
path-frequency or full control-flow coverage.

Run every sorted seed as a baseline first and merge each successful profile
into the cumulative profile whether or not it contributes novelty. Retain the
first seed unconditionally so the mutation corpus is nonempty; report if it
adds no coverage. Subsequent seeds may be rejected as duplicates. The
deterministic, safe mutation schedule selects parents from the sorted retained
corpus and applies a small fixed set of byte mutations (replace,
insert, delete, truncate, and bounded splice) using a documented PRNG. Bound
every result by the input ceiling. Persist mutation metadata before launching
the target so a failure is replayable. Retained corpus size is bounded by both
file count and total bytes; deterministic eviction of a least-recently-used
or largest entry is prohibited because it would add hidden scheduling state.
When the next novel candidate cannot be retained within the cap, stop with a
clear capacity result and preserve its evidence.

Write a final manifest and human-readable report containing the caller-provided
source revision label, source file identities, target/tool paths and versions,
binary identity, profile format, command/options, four frozen denominators and final covered
counts, region/function/branch gaps, seed and mutation counts, retained and
rejected candidate counts, elapsed time, and failure classification if any.
Use a stable non-cryptographic hash plus length for accidental mismatch checks;
label it clearly as such. Verify that source-file, target-binary and tool
identities stay unchanged during the run. Hash the target and tools at start
and end, with inexpensive metadata checks between cases; hash source files at
start and end. Do not hash large binaries for every testcase or claim this
proves the binary was built from those sources or flags. Record caller-provided
revision and build-flags labels honestly. External artifact SHA-256 can be recorded by
the caller. Never call a result "100%" without the raw counts and gaps.

## Validation

Unit tests use pure functions and small fixtures for sorted seed ingestion,
bounded deterministic PRNG replay, mutation metadata, report schema parsing,
all four denominator freezes, covered-count novelty, duplicate rejection,
input/corpus/profile/case/time bound decisions, and failure classification.
Subprocess tests use controlled safe Rust helper executables to exercise
success, target nonzero exit, stderr rejection, timeout, output overflow,
missing/corrupt profile, malformed LLVM output, denominator drift, and
first-failure preservation. They must verify fresh output rejection, private
directory permissions where supported, and RAII cleanup of pending files.
Tests must not depend on an installed coverage toolchain; one manual proof run
with the pinned LLVM tools must then execute a short real mutation campaign,
showing at least one retained candidate and one duplicate rejected by
cumulative feedback. Save its full manifest, raw profiles, merged profiles,
summary, final raw profile, cumulative profile, target/tool metadata, and
command log outside the repository. Do not retain every intermediate raw
profile. Review this implementation before running a longer campaign.

The report and README must state that LLVM source regions/functions/branches
are proxies for execution, compiler instrumentation defines the branch
denominator, and code excluded from the explicit production source list is
outside this measurement. Existing debug coverage and structural coverage
reports remain separate evidence. Optimized LTO profiles are not accepted
because experiments show LLVM's mismatched-function diagnostics; no-LTO is a
measurement requirement until that toolchain behavior is resolved.

## Explicit non-goals

No resume mode, parallel workers, minimizer, corpus synchronizer, general
fuzzing framework, crash deduplication, hard filesystem quota, remote service,
or security claim based on elapsed runtime. A future continuous campaign or
external engine requires a separate design and policy decision.
