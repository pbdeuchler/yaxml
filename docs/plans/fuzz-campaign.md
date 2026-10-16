# Bounded feedback-guided fuzz campaign

## Goal and boundary

Add one finite, reproducible coverage-guided campaign command using the
repository's existing safe replay target and process overlay. This closes the
current gap between deterministic mutation smoke tests and feedback-guided
mutation. It does not claim 100% execution coverage or replace a sustained
external fuzzing service. Keep the runtime crate dependency-free and
`unsafe_code = "forbid"`; do not add libFuzzer bindings, an unsafe export, a
Cargo dependency, or a tooling crate.

The implementation is limited to `examples/fuzz_campaign.rs`,
`examples/support/fuzz_campaign.rs`, their focused tests, and the fuzz README
section needed to document invocation and evidence. Do not modify unrelated
validation/review/benchmark artifacts. Reuse `examples/support/process.rs` for
bounded process execution and environment overlay. The measured target is the
existing `fuzz_replay` example built with the pinned coverage toolchain and
`-C instrument-coverage -Z coverage-options=branch`, optimized with LTO disabled
and the repository's normal fuzz driver. Exact compiler and tool versions must
be recorded; the profiler and report tools must match the compiler LLVM major.

## Campaign contract

The command accepts a target binary, `llvm-profdata`, `llvm-cov`, a seed
directory, a fresh output path, and finite case/time limits. It rejects an
existing output path. Create the output root atomically with mode `0700` on
Unix before writing campaign files. Resolve and sort regular seed files
deterministically; reject symlinks, unreadable files, empty seed sets, seeds
over the input limit, and seed/corpus count or byte totals over configured
ceilings. Record canonical source paths and bounded seed names in the manifest.

Every candidate is persisted before execution with its bytes, parent identity,
seed identity, mutation operation, and deterministic PRNG state. Each run
invokes the replay target in a fresh process, with an isolated unique
`LLVM_PROFILE_FILE` path and a cleared/explicit environment overlay. The input,
process output, profile, case count, total wall time, and stored corpus all have
finite configured limits. Use the existing process helper's timeout and
combined stdout/stderr cap. Check profile size after process completion and
before merge; reject an over-limit or missing profile. These are observed
per-run storage ceilings, not an operating-system disk quota against a
misbehaving child.

Reject any target stderr, nonzero status, timeout, output-limit error, missing
or invalid raw profile, failed merge, malformed report, report diagnostic,
changed denominator, or source/build identity mismatch. Stop on the first
failure. Preserve the bounded triggering candidate and bounded captured logs,
and classify target crash, timeout, or output-limit failure separately from
campaign infrastructure failure. Clean only pending temporary files owned by
the campaign using RAII; retain completed cases and failure evidence.

Merge each successful raw profile exactly once into a newly written cumulative
profile. Generate a library-only LLVM coverage summary from the fixed target
binary and cumulative profile. Parse the exact LLVM 22 summary header, expected
production source rows, and `TOTAL` row. The production file list is the
explicit sorted set of tracked `src/*.rs` files; reject missing, extra, or
duplicate rows. Freeze all four `TOTAL` denominators (regions, functions,
lines, instrumented branches) from the first valid report and reject any later
change. Lines remain diagnostic only. Retain a candidate only when cumulative
covered-region, executed-function, or covered-branch count increases; otherwise
record it as a duplicate. This is set-like novelty over LLVM's cumulative
covered counts, not edge-level novelty, and therefore makes no claim about
path-frequency or full control-flow coverage.

The deterministic, safe mutation schedule selects parents from the sorted
retained corpus and applies a small fixed set of byte mutations (replace,
insert, delete, truncate, and bounded splice) using a documented PRNG. Bound
every result by the input ceiling. Persist mutation metadata before launching
the target so a failure is replayable. Retained corpus size is bounded by both
file count and total bytes; deterministic eviction of a least-recently-used
or largest entry is prohibited because it would add hidden scheduling state.
When the next novel candidate cannot be retained within the cap, stop with a
clear capacity result and preserve its evidence.

Write a final manifest and human-readable report containing source revision,
source file identities, target/tool paths and versions, binary identity,
profile format, command/options, four frozen denominators and final covered
counts, region/function/branch gaps, seed and mutation counts, retained and
rejected candidate counts, elapsed time, and failure classification if any.
Use a stable non-cryptographic hash plus length for accidental mismatch checks;
label it clearly as such. External artifact SHA-256 can be recorded by the
caller. Never call a result "100%" without the raw counts and gaps.

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
summary, target/tool metadata, and command log outside the repository. Review
this implementation before running a longer campaign.

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
