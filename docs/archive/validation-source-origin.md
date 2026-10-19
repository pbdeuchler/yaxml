# Source-origin boundary validation

The frozen candidate is `b2f20205d1fb5970476342cb6d5f749567fd8f18`, based on
main `3726b3ddcbc0cc6c6845ff4a17801dc8ba460ff9`. Production source tree:
`28ac931425855790f29b6ab25fa6683e5fcc3aa7`.

This implements the [limited metadata boundary](plans/source-origin-boundary.md),
not replacement-text provenance or DTD acceptance. `SourceOrigin` is opaque and
copyable; `input_range()` returns an optional exact span in the supplying
document's original bytes. It does not authenticate a document identity. All
currently supported parsed origins have such a span. Normalization and owned
edits do not turn provenance into a description of serialized logical content.

Reader and Push last-delivery accessors are absent before the first event and
retain the last origin after EOF, failure and cancellation. Push callbacks and
BufferedReader tuples carry origins. Borrowed node ranges are now optional;
owned constructed nodes retain no parsed origin. Both DOM builders close through
a fallible ordered join, and missing owned provenance returns `InvalidState`
instead of panicking. Physical consumption, UTF-16 error mapping, parser grammar,
resource ceilings, dependencies and the unsafe-code prohibition are unchanged.

The intentional young-API break is documented in README/streaming/encoding
contracts. Real occurrence identity, replacement coordinates and reference
origins remain prerequisites for generated markup. This stage adds no unused
entity arena or unsupported public replacement variant.

## Tests and independent review

The complete final all-target run passed **535 test executions**, with nine
ignored tests across 44 result groups. All five benchmark smoke executables
completed. All **13 doctests**, strict all-target Clippy, formatting and diff
checks passed. These totals include the ordinary fuzz-smoke execution; its
instrumented replay below is not added again to the ordinary count.

The nine independent public tests and five author tests cover absent/sticky
origins, UTF-8 BOM and UTF-16LE/BE literal ranges, every byte split across a
surrogate/reference fixture, lookahead versus exact spans, cancellation endpoints,
both DOMs, source-drop lifetimes, construction/edits/removal, and private invalid
joins/missing provenance. Expectations include literal physical offsets rather
than only frontend agreement.

The [holistic review](review-source-origin.md) passes with zero findings. Six
independent faults were caught and restored: late push publication, swapped
diagnostic/cancellation endpoints, EOF origin clearing, decoded instead of
physical UTF-16 spans, disabled join ordering, and a missing-provenance panic.
The public-test author separately broke range ends, EOF retention and constructed
root provenance. Root read the production diff, migration contract, independent
tests and review; the final inventory-only correction adds no code.

## Complete XMLTS rerun

All **2,585 cases / 12,925 unique rows** across five interfaces are byte-identical
to the prior Name and normalization results, including the catalog and both
diagnostic streams. All **1,871 required passing pairs** remain passes. The
independent audit checks 270 tracked blobs, 139 build inputs including all 17
production files, 3,386 suite files, oracle/helper binaries, static library,
archive, recipe and before/after identities.

Unsupported DTD/entity/version cases remain classified gaps; unchanged results
are not a claim that every suite case passes or that full XML conformance has
been achieved.

## Fresh fuzz-only coverage

Only the fresh instrumented `fuzz_smoke` executable supplied profiles: **64 tests
passed** in 101.36 seconds, with 24 corpus files plus two inline seeds. The
compiler is nightly 2026-06-06, LLVM 22.1.6, using region/branch instrumentation.
All seventeen production source files are supplied, with `lib.rs` having no
mapped code. No ordinary-test profiles or production exclusions are mixed in.

| Metric | Covered / total | Percent |
| --- | ---: | ---: |
| Regions | 6,936 / 7,406 | 93.65% |
| Functions | 406 / 459 | 88.45% |
| Lines | 4,797 / 4,970 | 96.50% |
| Instrumented branch outcomes | 910 / 964 | 94.40% |

The [complete source report](validation/fuzz-coverage-source-origin-b2f2020.txt)
retains every file's denominator. `origin.rs` reaches all five mapped functions,
19/22 regions, 24/25 lines and one of two branch outcomes. Its rejected-order
join and new DOM invariant guards are covered by ordinary private tests, but
not by this fuzz run; those missed outcomes remain in the denominator. The
crate-wide 100% target and sustained campaigns remain open. This finite run
does not establish absence of security defects.

## Measured storage

Compiler layout probes use Rust 1.95.0 / LLVM 22.1.2 and eight-byte alignment:

| Type | Before bytes | Candidate bytes |
| --- | ---: | ---: |
| Reader | 352 | 360 |
| PushParser | 616 | 624 |
| BufferedReader | 576 | 576 |
| Borrowed node record | 168 | 168 |
| Owned node record | 144 | 144 |
| Event | 88 | 88 |
| SourceOrigin | — | 16 |
| Option<SourceOrigin> | — | 24 |

The [complete layout table](validation/source-origin-layout-b2f2020.tsv) includes
DOMs, views and names. Private record measurements use the compiler's type-size
diagnostic in separate inspection builds; these are not timed benchmark builds.
No permanent ABI or future replacement-origin size is promised.

## Paired performance

The [full table](validation/source-origin-full-b2f2020.tsv) retains all 65 frozen
DTD-free UTF-8 workloads, seven alternating pairs each: **455 pairs / 910 rows**.
Each invocation uses the frozen iteration count, input bytes, mode and chunk
size. Preflight and every measured row preserve six semantic counters and the
canonical hash. Child processes have 60-second/4 KiB bounds; the wrapper checks
all source blobs, scripts, binaries, inputs and matrix identities before/after.
An [independent audit](validation/source-origin-performance-audit-b2f2020.md)
recomputed all 65 summaries and verified all 455 pairs and retained identities.

Primary time is `(parse_ns + drop_ns) / iterations`. DOM observation is separately
timed and excluded; streaming observation executes within parse time. Cross-mode
clocks therefore describe different work.

No workload has a median slowdown of two percent or more in this run. The
largest is tiny DOM, 489.4 versus 498.2 ns/document (about 1.8% slower, with wide
overlapping ranges). Text push/seven-byte chunks changes from 2.472 to 2.312
ms/document: base/candidate ratio 1.0695, about 6.5% lower primary time. Every
other median change is within two percent. These are local descriptive results,
not statistical significance claims or a fresh libxml2 comparison.

The prior Name change's compiler/caller-context sensitivity remains relevant.
This result compares the current Name-based main with the new boundary; it does
not by itself close every older performance debt or establish a generally faster
algorithm. No library inlining hint or benchmark-body change was introduced.

Both helpers use Rust 1.95.0 / LLVM 22.1.2, release optimization level three,
thin LTO, one codegen unit, no debug assertions/overflow checks, unwind panics
and empty Rust flags on the same Apple M1 Max/Darwin arm64 host. Baseline helper
SHA-256 is `1c35473ab41905a231ced141497550c12f24b2426c7acc4402e9fc0fd3defdfe`;
candidate helper is `7d5fdff20eaec293ec9e50225fc347ea92e07cd236309b11d69d8a1b1e977182`.
The retained baseline was built at `e3dafa8`; its production/Cargo/benchmark
inputs match main `3726b3d`. All owned heavy jobs were stopped during timing;
OS-wide idleness and CPU pinning are not claimed.

## Retained artifacts

Root ordinary logs: `/private/tmp/yaxml-source-origin-final-{all-targets,doctests,clippy,fmt,diff}.log`.
Recipes/evidence: `/private/tmp/yaxml-source-origin-fuzz-coverage-b2f2020.sh` and
its matching evidence directory; `/private/tmp/yaxml-source-origin-xmlconf-final.sh`
and `/private/tmp/yaxml-source-origin-xmlconf-evidence-b2f2020`;
`/private/tmp/yaxml-source-origin-release-record-b2f2020`;
`/private/tmp/yaxml-source-origin-full`; and
`/private/tmp/yaxml-source-origin-layout-*`.

The author index is `/private/tmp/yaxml-source-origin-checks.txt`; independent
review logs and hashes are indexed by `/private/tmp/yaxml-source-origin-holistic.md`.
Compact reports/tables are retained here; raw profiles, per-process samples,
executables, recipes and detailed identity manifests remain in temporary storage.
The performance audit and its frozen output manifest are retained at
`/private/tmp/yaxml-source-origin-performance-audit-b2f2020`.
