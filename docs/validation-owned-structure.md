# Bounded owned leaf-structure validation

This report covers fresh leaf insertion and permanent subtree removal in
`OwnedDocument`. It is a working evidence record for the bounded structural
slice, not a general mutable-DOM or production-readiness claim. Implementation
and initial fuzz snapshot: `adefca6`; assembled implementation/tooling snapshot:
`08a1ff6`; strengthened iterator/corpus snapshot: `0ef8e77`.
Full assembled verification ran at `c9f9420`, with production source tree
`6578b39f230ae5d22569235d3dd2e965a27ed47e` unchanged across these snapshots.

## Implemented contract

`Editor` appends fresh Text, CDATA, comment and processing-instruction leaves.
Text and CDATA require an element parent. Comments and processing instructions
also accept the synthetic document parent and append after the existing epilog.
Each successful append creates a stable, document-scoped handle with no source
range. Appending never merges adjacent Text nodes.

`remove_subtree` unlinks a non-root node and permanently tombstones its complete
subtree. Removed handles never resolve again and their slots are never reused.
The synthetic document node and document element are protected. Surviving node
handles, order, namespace metadata, values and source provenance remain stable.
Removal is iterative and allocation-free.

Append validates the handle and placement before caller strings. It checks both
processing-instruction string lengths before scanning either string, then
validates the target and data in argument order. PI targets are XML Names without
a colon and cannot be any ASCII case spelling of `xml`. The created-node ceiling,
owned-pool ceiling and storage reservation follow lexical validation. Every
returned error preserves the logical tree and leaves the editor usable.

The default limits are 1 MiB per supplied string, 64 MiB of cumulative full
pool-capacity requests and 1,000,000 successful created slots. The two
cumulative counters persist across editors. Removal refunds neither counter,
and failed pool reservations may retain their charge. Parsed pool capacity is
the baseline. These limits bound requested edit work; they are not resident
memory or CPU quotas. The [owned DOM contract](owned-dom.md) specifies exact
error precedence and accounting.

## Deterministic test evidence

The following commands passed on `adefca6` with the locked dependency graph:

- `cargo test --lib owned::structure_tests:: --offline --locked`: 14 passed.
- `cargo test --test owned_structure --offline --locked`: 15 passed.
- `cargo test --test fuzz_smoke structural_ --offline --locked`: 4 passed,
  comprising three structural smoke matrices and one independent-model unit
  test.
- `cargo test --test fuzz_smoke --offline --locked`: 29 passed, including all
  three mutation-model unit tests and the three structural smoke matrices.

The private tests cover all four leaf kinds, placement and error precedence,
exact PI pool staging, quota persistence, first/middle/last/sole-child removal,
root protection, adjacent Text behavior, counter overflow, and iterative deep
and wide removal. A safe thread-local failpoint exercises actual node-vector and
pool-growth reservation boundaries. It checks node-before-pool ordering,
publication atomicity, retained pool charges, editor recovery, spare-capacity
paths, skip counts, panic cleanup and thread isolation. It simulates returned
reservation errors; it does not inject allocator or operating-system failures.

The public suite maintains an independent live-tree model through the public
API. It compares node kind, value, name, attributes, parent/child/sibling order,
descendant counts, source ranges, stable handles and serialized output after
successes and rejections. It covers UTF-8, UTF-16LE and UTF-16BE construction,
empty and adjacent Text projection, namespaced mixed subtrees, foreign and stale
handles, reopened editors, 64 append/remove cycles, depth 8,192 on a 256 KiB
thread stack, and 10,000 siblings.

The mutation fuzz driver uses two documents and a separate logical model. Its
four-bit operation selector retains the existing value-edit operations and adds
all four leaf appends, subtree removal, saved-handle acquisition,
created-ceiling reopen checks and forced foreign structural operations. The
model predicts exact errors before calling production code, then checks the
complete public state and stale-handle behavior. Mandatory successful appends
and removal prevent blanket rejection from satisfying the oracle. Serialization
is reparsed under the documented empty/adjacent Text projection. This is
deterministic corpus replay and mutation smoke, not a sustained feedback-guided
fuzzing campaign.

`0ef8e77` adds a registered 13-operation structural corpus seed and independent
checks for full and partial iterator counts, midpoint identity, last identity,
exact remaining size hints and repeated exhaustion. Its full 29-test smoke run
passed in 63.53 seconds; the instrumented run passed in 77.41 seconds. The raw
timing logs are `/private/tmp/yaxml-structure-iterator-fuzz-smoke.txt` and
`/private/tmp/yaxml-coverage-structure-fuzz-0ef8e77/run.txt`.

The assembled snapshot passed 293 all-target tests with six ignored subprocess
helpers, eight doctests, strict all-target Clippy, formatting and the locked
dependency audit. All 54 structural benchmark smoke rows completed. Logs are
`/private/tmp/yaxml-structure-final-{all-targets,doc-tests,clippy,fmt}.txt`;
the dependency tree and production source manifest are
`/private/tmp/yaxml-structure-final-dependencies.txt` and
`/private/tmp/yaxml-structure-final-source-hashes.txt`. The graph remains
`yaxml -> memchr 2.8.3`; Rust source contains no unsafe syntax or foreign ABI.

## Coverage evidence

Both runs use default features, debug optimization and
`-C instrument-coverage -Z coverage-options=branch` on nightly `2026-06-06`:
rustc 1.98.0-nightly (`8954863c8`, dated 2026-06-05), LLVM 22.1.6,
`aarch64-apple-darwin`. Initial reporting used Apple LLVM 21.0.0. After installing
the pinned toolchain's matching LLVM 22.1.6 tools, both sets of raw profiles were
remerged and reported with zero diagnostics and byte-identical summary tables.
The raw directories retain `merged-llvm22.profdata`, `library-report-llvm22.txt`,
`reporter-llvm22.txt` and empty `diagnostics-llvm22.txt` files. Summaries exclude
dependency/toolchain, test, fuzz, example and benchmark source; no production
source is excluded. Denominators differ because ordinary tests compile the
private test seams and additional monomorphizations.

| Run | Regions | Functions | Lines | Instrumented branch outcomes |
| --- | --- | --- | --- | --- |
| Fuzz smoke only (`0ef8e77`) | 4,818 / 5,047 (95.46%) | 311 / 336 (92.56%) | 3,247 / 3,324 (97.68%) | 657 / 692 (94.94%) |
| Ordinary tests only (`08a1ff6`) | 4,946 / 5,197 (95.17%) | 329 / 352 (93.47%) | 3,376 / 3,447 (97.94%) | 671 / 708 (94.77%) |

Fuzz smoke ran all 29 tests with no failures. The ordinary
run executed library, integration and example tests, excluding the fuzz-smoke
binary and benchmark executables. Linked coverage objects include all built
targets so unexecuted code is still visible. `owned.rs` reaches 1,139 / 1,194
regions, 83 / 91 functions, 804 / 820 lines and 82 / 84 branch outcomes under
fuzz smoke. Ordinary tests reach 1,222 / 1,271 regions, 94 / 101 functions,
869 / 883 lines and 93 / 94 branch outcomes.

Checked-in summaries are [fuzz coverage](validation/fuzz-coverage-0ef8e77.txt)
and [ordinary coverage](validation/test-coverage-08a1ff6.txt). Raw profiles,
exports, executable inventories, compiler/reporter versions, revision and
production SHA-256 manifests remain in
`/private/tmp/yaxml-coverage-structure-fuzz-0ef8e77` and
`/private/tmp/yaxml-coverage-structure-tests-08a1ff6`. Their production manifests
match the final verification manifest exactly.

The 100% execution target remains unmet. Gaps include returned real allocator
errors, identity exhaustion, defensive unreachable states, iterator special
methods and existing parser/stream/writer error paths. Safe reservation
failpoints exercise editing's returned-error paths only in ordinary tests.
These gaps stay in the raw denominator; successful smoke replay does not prove
coverage-guided fuzzing or security against every hostile input.

## Pinned libxml2 oracle

The [structural protocol](oracle-structure.md) ran against a locally built
libxml2 **2.15.4**, with compiled and runtime identities both `21504`. The source
archive SHA-256 is
`98087fd181d9070724f3fbc65c7377db03038eb92bd882374daff44940138821`.
CMake used Release static libraries, enabled programs/XPath/output, and disabled
iconv, ISO8859X, ICU, zlib, Python, docs and tests. Apple Clang built the helper
with `-std=c11 -Wall -Wextra -Werror`; Cargo never links it.

All 34 catalog cases completed: **23 exact tree/byte agreements, three
documented projections, six verified intentional differences, two explicit
non-comparisons, zero disagreements and zero inconclusive cases**. Projections
cover Text coalescing and empty-PI whitespace spelling. Differences cover
protected-root removal, parent/placement restrictions and lexical validation.
Ownership/OOM and namespace moves remain excluded. The helper observes only
live pointers, frees detached subtrees before their document, and never inspects
an appended pointer that libxml2 may have merged and freed.

The pristine-source/serialization oracle also ran the existing 18 corpus files:
12 semantic agreements, five rejection agreements, one explicit policy skip,
zero acceptance-only, inconclusive or disagreement results. This does not make
the one-operation structural catalog a multi-operation libxml2 model.

Build logs/options, compiler/version records, helper and source hashes and both
comparison logs are in `/private/tmp/yaxml-oracle-21504`. The structural helper
source SHA-256 is
`53091db541dfcd43c953c33cd6d2bccb9e3ad952db2f79d6398a69ce2d8d77b9`;
the catalog/protocol source hash is
`171adbb7b53c03f0abbe1f679502bb77f45bfa97c8ab7e96e1ffcebd1dd1fa62`.
The structural comparison log hash is
`8aabae859d27a8135dfed38db0fb8578e9cf5e90dedccbac3552adf8d8ead17a`.
The [checked-in run log](validation/oracle-21504-owned-structure.txt) preserves
the version, dispositions and summary.

## Benchmark evidence

The [structural benchmark report](validation-owned-structure-benchmark.md)
records the 216-row release matrix and seven independent focused samples per
case, including all four appends, first/middle/last/deep removal and churn.
Every row completed. Setup, editing, live traversal, serialization, validation
and document destruction have separate clocks. The report retains compiler,
host, parameters, phase medians/ranges and raw artifact hashes.

These are yaxml-only measurements. A libxml2 comparison still requires matched
fresh-leaf construction, insertion and unlink/free work; unlink alone does not
match permanent removal. The conformance helper supplies no timings and does
not close this performance gate.

## Independent review

The independent [holistic review](review-owned-structure.md) inspected every
changed line from `c695e61` through `08a1ff6` and returned **PASS** with no
Critical, Important or Minor findings. All 15 frozen-plan fault classes and the
expanded allocation, handle, independent-model, benchmark and oracle failure-gate
mutations were caught and restored. Clean format, clippy, all-target tests,
dependency and unsafe-code checks passed; the review records exact commands and
evidence limits.

The follow-up review through `0ef8e77` also passed. Three deliberate count,
partial-progress and exhausted-iterator faults were caught by the new model
assertions and restored. Production source stayed unchanged.

## Remaining scope

This milestone does not construct elements or attributes, insert siblings,
change names or namespaces, move existing nodes, detach nodes, copy/import/adopt
across documents, repair namespaces, reclaim pool/attribute storage, reuse slots
or compact tombstones. Those operations need separate ownership, cycle,
namespace and quota contracts. The implementation, deterministic tests, measured
coverage, pinned structural oracle, yaxml benchmark runner and independent review
are complete. The frozen milestone remains open until the matched libxml2
structural benchmark is recorded. Reaching 100% execution coverage, sustained
feedback-guided fuzzing, allocator-level failure testing and retained memory
measurements remain open.
