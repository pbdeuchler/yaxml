# Owned-capable names and PI targets

Candidate: `e3dafa83ca2f7b2430a0613824010c337b8386ec`, compared with local
main `2609aeff4c5287b5fbbdadd15eb55b1cbd296aa0`. This is an ownership
foundation for future DTD replacement input; DTD parsing remains unsupported.
The frozen production source tree is `408df38ccc35a3a3f2cab1cdd5b86734818d3490`.

`Name` now validates one borrowed or owned QName buffer and retains a private
prefix split. Its getters borrow the name, and namespace metadata is preserved
until parser/writer scope validation. PI targets can also own their strings.
Direct input parsing still borrows these values. No dependency, unsafe code,
declaration-ownership conversion, or external I/O was added.

## Correctness and review

The complete all-target run passed 509 test executions, with nine ignored tests
across 42 groups; thirteen documentation tests passed separately. Formatting and
Clippy passed. Independent tests cover accepted/rejected Unicode QNames, exact
byte offsets, getter recomposition, buffer preservation, owned PI output, all
five parser interfaces, namespace-context rejection, and declaration borrowing.

Two reviewers read the complete [production](review-name-ownership.md) and
[non-production](review-name-migrations.md) scopes. Deliberate
faults in QName validation, prefix metadata, and owned PI storage were caught and
restored. The scoped reports and root closure accompany this integration.

Fresh complete XMLTS execution produced 12,925 unique rows for 2,585 cases,
byte-identical to the previous catalog, result, and diagnostic artifacts. The
1,871 required passing pairs remain unchanged, with no scored regression,
mismatch, or inconclusive result. Substantial unsupported DTD/entity behavior
remains; unchanged results do not establish full conformance.

## Fuzz-only coverage

Only the fresh instrumented `fuzz_smoke` executable supplied profiles: 61 tests,
23 corpus files plus two inline seeds, pinned nightly 2026-06-06 and LLVM
22.1.6, branch instrumentation, no ordinary-test profiles or source exclusions.
All sixteen production source files were included; `lib.rs` has no mapped code.
The [source report](validation/fuzz-coverage-name-ownership-e3dafa8.txt) retains
every mapped file's denominator.

| Metric | Covered / total | Percent |
| --- | ---: | ---: |
| Regions | 6,878 / 7,331 | 93.82% |
| Functions | 397 / 447 | 88.81% |
| Lines | 4,749 / 4,923 | 96.47% |
| Branch outcomes | 909 / 962 | 94.49% |

The new `name.rs` has 140/140 regions, 14/14 functions, 88/88 lines and 16/16
instrumented branch outcomes covered. This is compiler coverage for this finite
run, not proof of all possible conditions or security. The crate-wide 100%
target remains open.

The new target exercises 28 independently specified QName literals under four
namespace forms, borrowed and owned construction, pointer preservation, exact
errors, getter algebra, and owned PI Writer/Reader round trips. Arbitrary names
use at most 96 raw input bytes before lossy UTF-8 conversion (up to 288 UTF-8
bytes), independently of the driver's 1 MiB input ceiling. Dispatch occurs
before grammar/encoding selection and has its own execution assertion.

Writer's qualified-spelling and reparsed-attribute-count defensive failure arms
lost coverage after public name fields became private and validated. Former
structural-byte injection through forged fields is no longer available through
safe construction. Keep these release checks and their uncovered outcomes in
the denominator; namespace-context rejection remains reachable and covered.

## Layout

Probes linked against the actual release libraries, using the same compiler and
profile, report eight-byte alignment for every listed type. The raw
[before](validation/name-ownership-layout-base-e3dafa8.tsv) and
[candidate](validation/name-ownership-layout-candidate-e3dafa8.tsv) tables agree:

| Type | Before bytes | Candidate bytes |
| --- | ---: | ---: |
| Name | 72 | 56 |
| Attribute | 96 | 80 |
| Element | 96 | 80 |
| Event | 104 | 88 |
| Borrowing NodeKind | 96 | 80 |

These are measured compiler layouts, not public ABI guarantees.

## Performance investigation and accepted tradeoff

The original [frozen matrix](validation/name-ownership-full-e3dafa8.tsv) retains
65 workloads with seven alternating pairs each; a
[focused repeat](validation/name-ownership-repeat-e3dafa8.tsv) retains 14
workloads with fifteen pairs each. The earlier
[screen](validation/name-ownership-screen-e3dafa8.tsv) retains 25 workloads with
five pairs each. Every observed counter/hash agrees. Timing includes parse plus destruction; DOM
observation is separately timed, while streaming includes its callback observer.
No libxml2 comparison was rerun for this change.
An [independent audit](validation/name-ownership-performance-audit-e3dafa8.md)
recomputed all 104 summaries from 790 pairs / 1,580 raw rows and verified exact
observations, fixed work, ordering, source manifests and artifact identities.

The host is the same Apple M1 Max/Darwin arm64 machine as the frozen baseline.
Both helpers use rustc 1.95.0 / LLVM 22.1.2, optimization level three, thin LTO,
one codegen unit, no release debug assertions or overflow checks, unwind panics,
and empty Rust flags. Cargo fingerprint profile/config IDs agree. The retained
baseline helper was built at `7efc22f`, whose production and relevant benchmark
inputs are identical to main `2609aef`; it was not rebuilt at that documentation
commit. Baseline helper SHA-256 is
`8fdbb1f7b85eaae4305a07f7a54c3e8ad0a98ca1439c3b47409be5f0f1db5b3d`;
candidate helper SHA-256 is
`1c35473ab41905a231ced141497550c12f24b2426c7acc4402e9fc0fd3defdfe`.
All agent-owned heavy jobs stopped during timing. Process enumeration was
sandbox-denied, so OS-wide idleness or CPU pinning is not claimed.

Original full-matrix text/push/seven-byte chunks take 2.209 ms before and
2.469 ms after; the repeat takes 2.214 ms before and 2.476 ms after. This is
about 12% slower, with nonoverlapping sample ranges. Buffered seven-byte text
chunks are about 4% slower; deep Reader is about 4.7% slower. Attribute-heavy
DOM and owned parsing improve by approximately 9% and 10%, respectively.

Static comparison found identical ordinary instruction sequences in
`Stream::advance`, `Tokenizer::advance` and its append path. Fresh unmodified
builds reproduce the seven-byte push regression: 2.215 ms before and 2.480 ms
after (ratio 0.8931). An identical private `#[inline(never)]` on `measure_push`
in both helpers changes those medians to 2.267 ms and 2.267 ms (ratio 0.9998).
Deep push changes from ratio 0.9752 to 0.9933. Chunks one and 8,192 remain near
parity. Both sets retain fifteen alternating pairs per workload and exact
observations. The outlined experiment changes only helper code, not the library.
Its [report](validation/name-ownership-push-diagnostic-e3dafa8.txt) and separate
[unmodified](validation/name-ownership-push-unmodified-e3dafa8.tsv) and
[outlined](validation/name-ownership-push-outlined-e3dafa8.tsv) summary tables
retain the controls and limitations. The two sets ran sequentially; cross-set
changes are not independent paired estimates of outlining's speedup. Buffered
and Reader regressions were not retested by this push-only diagnostic.

These results support sensitivity to compiler/caller context, rather than added
ordinary decoder/tokenizer instructions. They do not establish the precise
hardware cause or guarantee other consumers' performance. Fresh unmodified
binary hashes differ from the retained helpers despite identical source blobs,
fingerprints and normalized benchmark/hot-path assembly; these controls are
reported separately rather than treated as byte-identical originals.

Root accepts this functionality foundation with explicit performance debt:
owned names and PI targets are needed for parser-owned replacement input, their
representation is smaller, and attribute/Unicode workloads improve. The original
repeated regressions remain in the reported matrix. No benchmark outlining or
production inlining hint was merged to hide them. Closing the observed slow
contexts remains part of the broader performance goal; this milestone makes no
claim of uniformly faster parsing or libxml2 superiority.

## Retained artifacts

Temporary evidence is rooted at `/private/tmp/yaxml-name-ownership-{screen,full,repeat}`,
`/private/tmp/yaxml-name-fuzz-coverage-e3dafa8`,
`/private/tmp/yaxml-name-xmlconf-evidence-e3dafa8`,
`/private/tmp/yaxml-name-release-record-e3dafa8`, and
`/private/tmp/yaxml-name-codegen-e3dafa8`. The complete diagnostic artifacts are
in `/private/tmp/yaxml-push-outline-diagnostic`. Each run records source, compiler,
binary, fixture and harness identities. Compact reports and summary tables are
retained in this repository; raw profiles, per-process rows, scripts, executable
artifacts and detailed identity manifests remain in these temporary directories.
The original-matrix audit is retained at
`/private/tmp/yaxml-name-performance-audit-e3dafa8`. Ordinary validation logs are
`/private/tmp/yaxml-name-final-{all-targets,doctests,clippy,fmt}.log`.
