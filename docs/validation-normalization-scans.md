# Normalization scan validation

The source freeze is `7efc22fbe147d0d997b668bd21ac0bc0850d7280`, based
on main `f1b824ecf81cd4e4c62788ac56dfde0f53b74038`. Its production source
tree is `ac8fa2dace02ca42bd4add877b78e2249e547789`. The change uses the
existing `memchr` dependency for normalization detection and removes a duplicate
check. It preserves the public API, XML results, borrowing, resource policy,
and diagnostic precedence. The [plan](plans/normalization-scans.md) limits the
work to this private path. Cargo inputs and fuzz harness/corpus are unchanged.

## Measurement and rejected candidate

The [matched libxml2 baseline](validation-parse-baseline.md) identified long
text as a large gap. A private probe of unchanged main measured these medians
for the 500,013-byte text fixture: 513 µs for XML-character scanning, 482 µs
for reader construction/drop, 465 µs for detection followed by normalization,
and 1,033 µs for document parse/drop. These are independently clocked phases;
they cannot be added as a causal partition of parsing. The source shows that
the detection predicate runs both in parser state and inside the normalizer.
The probe uses a private release harness with debug symbols enabled; it is
diagnostic evidence, separate from the matched release A/B builds below.

Candidate A (`a771f649`) replaces only the scalar predicate with safe
`memchr` searches. Its [25-workload screen](validation/normalization-a-screen-7efc22f.tsv)
improved long-text DOM medians about 1.75× but slowed attribute-heavy borrowing
DOM about 6%. It was rejected as a standalone change.

Candidate B (`ab425c0f`) moves the borrowed return into parser state, then
charges the raw payload length before calling `normalize_required`. The latter
returns `String` and retains its existing allocation and transformation loop.
The [combined screen](validation/normalization-ab-screen-7efc22f.tsv) recovered
the attribute regression. The final freeze adds independent tests in a separate
commit. No size threshold, custom SIMD, new dependency, or validation shortcut
was introduced.

## Paired Rust results

The [full table](validation/normalization-ab-full-7efc22f.tsv) contains all 65
baseline workloads, with seven samples each: **455 pairs / 910 rows**. Both
helpers use identical fixed iteration counts from the baseline matrix. Each
pair runs in fresh processes, alternating order. Both preflight and every
sample match the baseline's six semantic counters and canonical hash. Child
deadlines are 60 seconds with 4 KiB output limits; there is no overall deadline.

For the text fixture at chunk size 8,192, primary medians are:

| API | Main, µs/document | Candidate, µs/document | Main / candidate |
| --- | ---: | ---: | ---: |
| Borrowing DOM | 1,056.8 | 589.6 | 1.7925 |
| Owned DOM | 1,055.0 | 593.1 | 1.7786 |
| Reader | 1,679.5 | 1,213.1 | 1.3845 |
| Push | 1,700.6 | 1,234.9 | 1.3771 |
| Buffered pull | 1,698.6 | 1,237.9 | 1.3722 |

DOM primary time is parse plus drop and excludes observation. Streaming time
includes canonical observation, EOF, and drop. These clocks describe different
work and must not be compared as equivalent operations. Smaller text chunks
also improve: ratios range from 1.0537 to 1.2054 for chunks 1 and 7.

Nine full-matrix medians improve by more than the descriptive 5% band; 55 are
within it. Tiny reader is the sole apparent regression beyond the band, with
very wide ranges on both sides. Its earlier combined screen ratio was 1.0056.
A [repeat of all nine tiny workloads](validation/normalization-ab-tiny-7efc22f.tsv),
with **31 samples each / 279 pairs**, gives reader ratio 1.0012 and all nine
ratios between 0.9860 and 1.0088. The original full-run outlier remains in its
table; the repeat does not replace it. No material regression repeated in these
measurements. Small differences and these bands are descriptive, not statistical
significance claims. This Rust A/B experiment is not a fresh libxml2 comparison
or evidence of performance superiority across the board.

The host is the same Apple M1 Max/Darwin arm64 host as the baseline. Both helpers
use rustc 1.95.0, LLVM 22.1.2, opt-level 3, thin LTO, one codegen unit, disabled
release debug assertions/overflow checks, unwind panics, and empty Rust flags.
Cargo fingerprint profile/config IDs agree. The baseline helper SHA-256 is
`33ce3f27e7e319169569394f5bffe4b1729f65ee5899b4e41e0d218f99f00943`;
the final helper is
`8fdbb1f7b85eaae4305a07f7a54c3e8ad0a98ca1439c3b47409be5f0f1db5b3d`.
All agent-owned compilation/testing stopped during timing. System process
enumeration was sandbox-denied; OS-wide idleness, CPU pinning, and governor
control are not claimed.

## Correctness and security checks

The new tests first passed on unchanged main: one private predicate property
and four public integration tests. They cover all four trigger sets, bounded
exhaustive short strings, Unicode, scan boundaries/tails, borrowed versus owned
values, literal outputs, exact raw-length quotas, quota-before-reference error
precedence, and UTF-8/UTF-16LE/BE byte offsets across selected splits.

At the freeze, **493 tests passed**, nine external/timing tests were ignored,
and all five custom benchmark smoke executables completed. All **11 doctests**,
strict all-target Clippy, formatting and diff checks passed. The count includes
the ordinary fuzz smoke run; it does not add the instrumented replay twice.
The [independent holistic review](review-normalization-scans.md) caught all four
deliberate faults: missing text reference sentinel, owning unchanged values,
reference parsing before quota charging, and missing attribute tab detection.
Every mutation was restored in a separate review worktree.

The fresh full XMLTS run retains all **2,585 cases / 12,925 rows** and all
**1,871 required passing pairs**. Catalog, complete result table, and summary
are byte-identical to the [prior conformance run](validation-xml-conformance.md).
There are zero mismatches, inconclusive outcomes or required-pass regressions.
The substantial DTD/entity/version gaps are unchanged; this does not mean the
whole suite passes. The audit verified all 3,386 suite files, 234 tracked Git
blobs, 129 build inputs, and runner/helper identities before and after.

Fresh fuzz-only coverage ran the **59 fuzz smoke tests** with the same 22 seeds
and unchanged harness, pinned nightly `2026-06-06`/LLVM 22.1.6 and
`-C instrument-coverage -Z coverage-options=branch`. All 15 recursive library
source files are in the manifest; 14 have executable coverage mappings.
There are no production exclusions or ordinary-test profiles in this report.

| Metric | Reached / total | Coverage |
| --- | ---: | ---: |
| Regions | 6,808 / 7,260 | 93.77% |
| Functions | 385 / 435 | 88.51% |
| Lines | 4,711 / 4,883 | 96.48% |
| Branch outcomes | 913 / 964 | 94.71% |

The [complete report](validation/fuzz-coverage-normalization-7efc22f.txt)
keeps every missed region visible. The previous source had 6,810/7,260 regions
and 919/970 branch outcomes: the refactor changes instrumentation, so the
percentages do not establish a change in fuzzing quality. The new explicit
`(attribute=true, references=false)` arm adds two missed regions; no public
parser currently uses that context. Its property test is ordinary-test
evidence, not fuzz evidence. The two uncovered allocation-error closures in
`escape.rs` remain; broader allocation, overflow and defensive gaps described
in the [previous triage](validation-construction-fuzz-gaps.md) remain open.
LLVM reports 100% branch coverage for `escape.rs` despite the unvisited match
arm, demonstrating why branch coverage alone is insufficient. The 100% target
and sustained security assurance remain open.

## Retained evidence

Local build manifests are `/private/tmp/yaxml-text-normalization{,-ab}-target/BUILD.txt`.
The microphase source, private harness diff, hashes, compiler records and raw
samples are in `/private/tmp/yaxml-text-profile-results`. A bounded native
`sample` attempt was denied by macOS PID inspection permissions; that failure
is retained, and no native stack profile is claimed.

Exact A/B commands and before/after identity manifests are in
`/private/tmp/yaxml-text-normalization-{a-screen,ab-screen,ab-full,ab-tiny}`.
The private recipes are `/private/tmp/yaxml-normalization-{screen.rs,run.rb}`
and `/private/tmp/yaxml-normalization-focused{.rs,-run.rb}`. Manifests check
clean pinned commits, every tracked blob, binaries, scripts, matrix, all
baseline TSVs and input hashes before/after; runtime input bytes are checked
around each measured child. An independent audit recomputes every summary
field from all **1,968 raw rows** and rechecks recorded artifact/source hashes
in `/private/tmp/yaxml-text-normalization-ab-audit-7efc22f`.

Full conformance evidence is
`/private/tmp/yaxml-normalization-xmlconf-evidence-7efc22f`, produced by
`/private/tmp/yaxml-normalization-xmlconf-final.sh`. Fresh fuzz artifacts
are `/private/tmp/yaxml-normalization-fuzz-coverage-7efc22f`, produced by
`/private/tmp/yaxml-normalization-fuzz-coverage.sh`; they retain source/harness
manifests, raw/merged profiles, full JSON export and binary/report hashes.
The report SHA-256 is
`f545e2462761bc5d8a3b78d00c84bbcee43a425ac88c5059be7a7d95fdd427dd`.
Ordinary check logs are `/private/tmp/yaxml-normalization-{all-targets,doctests,clippy,fmt}.log`.
Later documentation changes preserve the measured production source tree.
