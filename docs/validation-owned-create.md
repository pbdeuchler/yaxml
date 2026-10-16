# Bounded rooted construction validation

`OwnedDocument::create` and `Editor::append_element` implement the
[rooted construction slice](plans/owned-create-slice.md). The reviewed source
checkpoint is `02cb2a510156202ba1fb1b20ca2054d033115bac`, based on local main
`3db2aad`. Its production `src` tree is
`34f9acb22ae65b99fcea2cc1cc52e9ee6e036b3f`. Later changes in this integration
are documentation and retained summaries. The core remains safe Rust with only
`memchr 2.8.3` in the locked dependency graph. C helpers are external test tools;
Cargo never compiles or links them into the library.

## Implemented behavior

Creation returns one real root under the synthetic document node. Append creates
a fresh last child under an element. Borrowed `ElementSpec`, `NameSpec`,
`AttributeSpec` and `NamespaceSpec` inputs are copied before return. Exact
prefixes keep their spelling; automatic names follow the documented bounded
scope/default/generated-prefix policy. Explicit items preserve caller order,
with generated declarations following in order of first use. All explicit
declarations are gathered before resolving names. Later exact prefix requests
cannot retarget an earlier automatic name.

Every element keeps one contiguous initial attribute range, using the existing
attribute iterator, value editor and serializer. Existing-element attribute
insertion/removal, declaration edits, rename, moves, detached ownership and
namespace repair remain unimplemented. New records have no source provenance;
parsed survivors retain theirs. Handles remain document-scoped and never reuse
removed slots. Construction appends switch to the existing linked traversal.

The nine `EditLimits` fields bound values, names, aggregate supplied bytes,
start-tag items, element depth, namespace/expanded-name work, persistent created
nodes/attributes and cumulative full pool-capacity requests. Repeated supplied
strings count per occurrence. Length checks precede validation scans, and
namespace work is charged before scope steps or comparisons, including ordinary
ancestor attributes and long inherited prefixes. Scratch reservations are
fallible and input/work-bounded. Persistent publication follows all fallible
steps; failures preserve the logical tree and permit retry. Allowed retained
capacity and previously charged pool requests are not rolled back. These are
logical bounds, not process-memory or operating-system OOM guarantees.

Construction reports typed `EditLocation` fields/items and UTF-8 component
offsets. Existing scalar methods retain `value_offset` behavior. Adding
`EditError.location` and six `EditLimits` fields breaks exhaustive public struct
literals under the crate's pre-1.0 policy; no C ABI is provided.

## Tests and review

The assembled `9e145bf` checkpoint passed **404 all-target tests**, with zero
failures and seven ignored subprocess helpers. All five custom benchmark
executables also completed their test-mode smoke runs. `02cb2a5` subsequently
changed only four source documentation lines; executable source, tests and
harness bytes are unchanged. Final checks at `02cb2a5` passed **11 doctests**,
including four compile-fail cases, strict all-target Clippy and formatting.
The independent reviewer also passed the restored 17-test public construction
suite and audited the dependency graph and unsafe prohibition.

Public tests use literal names, ordered attributes, serialization, exact errors
and whole-tree snapshots. They cover default undeclarations, inherited/shadowed
bindings, reserved names, duplicate declarations/expanded names, later exact
prefix reservations, UTF-8 byte boundaries, every public quota family,
foreign/stale/wrong-kind handles, no-refund accounting and successful recovery.
A 4,096-byte Unicode inherited prefix forces scratch growth with a small child
spec and checks exact success, one-byte-short `NameBytes` failure and recovery.

Private tests enumerate actual scratch reservation sites until success and
require that success was reached. Node, attribute and pool failpoints check
reservation order, retained allowed charges, unchanged topology/counters and
same-editor retry. These safe returned-error seams do not inject faults inside
the allocator. The separate fuzz model predicts success/error state before calls
and checks full public views; serialize/reparse is an additional oracle.

The [holistic review](review-owned-create.md) found no remaining code-quality
findings. Three separate restored production faults were caught: changing the
automatic prefix stem, inventing a component for an item semantic error and
bypassing the zero created-node quota. The orchestrator separately removed the
oracle's version exit-status gate and its result comparison; CLI tests failed
on `version-exit` and `body-changed`, respectively. All mutations were restored.

Root logs are `/private/tmp/yaxml-owned-create-{integrated-all-targets,doctests,clippy,fmt}.txt`.
The oracle fault logs are
`/private/tmp/yaxml-owned-create-oracle-mutant-{version-exit,result-check}.txt`.

## Source-complete fuzz evidence

The measurements below describe the original construction milestone.
[Subsequent construction fuzz-gap work](validation-construction-fuzz-gaps.md)
records a fresh harness, coverage report and campaign over identical production
source. Profiles from the two milestones are not combined.

Fresh fuzz-only coverage at `02cb2a5` used nightly `2026-06-06`, rustc
1.98.0-nightly (`8954863c8`), matching LLVM **22.1.6** tools, debug optimization,
default features and `-C instrument-coverage -Z coverage-options=branch` on
`aarch64-apple-darwin`. Only the 46-test fuzz-smoke binary ran. It passed in
88.83 seconds. Its unique target/profile directory contains no old profiles.

All **15** recursive production source paths were hashed and passed explicitly
to LLVM, including `src/owned/construction.rs`. Fourteen executable module rows
are present; `lib.rs` has no executable mapping but remains in the identity
manifest. No production source is excluded. Dependency, test, fuzz, benchmark
and example code is outside this library-only measurement. Source and harness
manifests match before/after; merge/report/export diagnostics are empty.
An independent remerge and regenerated report/export are byte-identical to the
saved profile/report/export. All 15 source blobs match committed `02cb2a5`.

| Metric | Reached / total | Coverage | Gap |
| --- | ---: | ---: | ---: |
| Regions | 6,484 / 7,260 | 89.31% | 776 |
| Functions | 381 / 436 | 87.39% | 55 |
| Lines | 4,495 / 4,883 | 92.05% | 388 |
| Instrumented branch outcomes | 839 / 970 | 86.49% | 131 |

The new construction module reaches 1,596/2,149 regions, 63/93 functions,
1,189/1,501 lines and 177/274 branch outcomes. Its uncovered public validation
and namespace cases need more independent fuzz generation; allocator/overflow
and defensive invariant paths also remain in the denominator. The overall
100% execution target remains unmet. These are LLVM instrumentation metrics,
not semantic or path-space completeness, and ordinary failpoint tests are not
counted as fuzz execution.

The [per-module summary](validation/fuzz-coverage-owned-create-02cb2a5.txt) has
SHA-256 `da43865f688b4e892281f133f52b8dd28cd7728805157b1bcf204d06e88d4831`.
Raw profiles, JSON export, compiler/tools and source/harness/artifact manifests
are in `/private/tmp/yaxml-owned-create-coverage-02cb2a5`; the build/run script
is `/private/tmp/yaxml-owned-create-coverage.sh`.

A separate optimized, no-LTO feedback campaign ran **500** cases: 19 seeds and
481 mutations. It retained 78 inputs and rejected 422 duplicates, stopping at
the case limit after 36 seconds without target/infrastructure failure. Its exact
15-source inventory accepted the real LLVM `owned/construction.rs` row.
Independent profiles are not combined with the fuzz-smoke measurement. Final
counts were 6,312/7,260 regions, 379/436 functions, 4,430/4,883 lines and 772/970
branch outcomes. This finite run validates campaign mechanics; it is not a
sustained security campaign. Artifacts are
`/private/tmp/yaxml-owned-create-campaign-02cb2a5-500` and
`/private/tmp/yaxml-owned-create-campaign-evidence-02cb2a5`; the exact command
and build flags are in `/private/tmp/yaxml-owned-create-campaign.sh`.
An independent offline audit reproduced every mutation's PRNG state, operation,
bytes, parent/origin lineage, identity and retention decision across all 500
journal rows. All 19 seeds and 59 mutations were retained, totaling 9,467 bytes.
It separately regenerated LLVM report/export from the cumulative profile; the
campaign deliberately does not retain every raw profile, so this was not a
500-profile remerge or a fresh target replay. The audit and gap triage are in
`/private/tmp/yaxml-owned-create-campaign-evidence-02cb2a5/independent-audit`.

## Pinned libxml2 comparison

The final example and rebuilt helper ran against libxml2 **2.15.4**, with
compiled/runtime identity `21504` and tree/output support. The pinned upstream
archive SHA-256 is
`98087fd181d9070724f3fbc65c7377db03038eb92bd882374daff44940138821`;
the static archive hash is
`b48612c66e7d4d7fae1ceb7db9a0d3e288085bc91b560f037862607eccac1ab0`.
Strict C compilation and direct Rust/helper executions succeeded; direct
execution stderr was empty. Fresh Rust builds use the exact reviewed source.
Source, pinned input, compiler/tool and binary hashes are retained separately.

The 12 fixed cases cover plain/attributed roots, prefixed/default roots,
inherited prefixes, default undeclaration, prefix shadowing, implicit `xml`,
Unicode, literal text and automatic element/attribute choices. **Eleven are
byte-identical.** The attribute case separately verifies exactly this existing
legal escaping difference: Rust emits `quote="&quot;&lt;>"`, while libxml2 emits
`quote="&quot;&lt;&gt;"`. No general normalization hides other differences.
The helper uses explicit C namespace operations; these cases do not establish
libxml2's complete prefix-allocation or mutable-tree semantics.

An independent temporary harness printed the actual Rust bytes rather than
the comparison table, confirming the same eleven exact rows and one exception.
Evidence is `/private/tmp/yaxml-owned-create-oracle-02cb-evidence`, including
`verification.txt`, direct outputs, the literal diff and before/after manifests.
The helper's status/stderr/version/feature/body/extra/truncation failure gates
are also exercised by the real Rust CLI with controlled fake helpers.

## Performance

Release measurements used rustc **1.95.0**, LLVM 22.1.2, thin LTO and one
codegen unit on an Apple M1 Max, 64 GB, macOS 27.0.1. Benchmark runners are
external diagnostics with frozen source, lockfiles and binary/tool hashes.
An exclusive timing window excluded concurrent builds/tests/coverage jobs.
Warmup sample zero is excluded from all reported medians; each case retains
31 samples. No core implementation was changed in response to these results.

The legacy A/B compares clean `3db2aad` with clean `02cb2a5` using the same
ten fixtures, runner bytes, work counts, observer/checksums and serialization
hashes. Build order alternates by sample within each case, independently of
reversing case order. All 640 rows and the 49 phase medians/MADs/ranges were
independently recomputed. No median slowdown reached 5%; value-edit cases
include roughly 1–2% slowdowns and deep serialization is about 1% slower.
Microsecond-scale destruction has higher relative spread. These results do not
prove unchanged performance across arbitrary workloads.

The nine-case construction baseline separates root creation or child append
from parent setup, traversal, serialization and destruction. It checks literal
output, live counts, provenance and public layouts before timing. Created
counts are scenario-derived expectations checked against public live counts;
private persistent counters are tested by the quota suites, not read by the
benchmark. All 288 rows, exact sample membership, count arithmetic, literal
output hashes and 36 phase medians/MADs/ranges were independently recomputed.
Operation times include public editor acquisition for append and small loop/
dispatch overhead. Other normalized phases are per document, not per appended
element. Default limits are enabled.

Full medians, spreads and accounting are retained in the
[legacy A/B table](validation/owned-create-legacy-02cb2a5.md) and
[construction table](validation/owned-create-benchmark-02cb2a5.md). Raw evidence
is `/private/tmp/yaxml-owned-create-legacy-ab-20261016` and
`/private/tmp/yaxml-owned-create-construction-evidence-20261016`; the independent
root audit is `/private/tmp/yaxml-owned-create-root-benchmark-audit.{rb,txt}`.
Both final SHA-256 manifests verify. Their hashes are
`a7352783d06f9280a0ee34074b4d4a03f4a7b11a25bbb744b79c26714853a460`
for legacy A/B and
`0f85a1a89bd469222f52c6824fb8ea6bb38111e527b367b7104f6de30a372763`
for construction.
These benchmarks compare yaxml revisions or establish a Rust construction
baseline. **They provide no libxml2 performance comparison.** Matched broad DOM,
streaming and construction comparisons, allocator support, sustained fuzzing
and full compatibility remain open under the overall goal.
