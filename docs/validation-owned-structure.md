# Bounded owned leaf-structure validation

This report covers fresh leaf insertion and permanent subtree removal in
`OwnedDocument`. It is a working evidence record for the bounded structural
slice, not a general mutable-DOM or production-readiness claim. Implementation
and deterministic-test snapshot: `adefca6`. Sections marked **PENDING** must be
replaced with measured results from the final integrated snapshot.

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

## Coverage evidence

**PENDING — final measured coverage.** Run fuzz-smoke-only and ordinary-test
coverage on the final integrated source using the repository's documented LLVM
instrumentation. Record compiler and reporter versions, source revision and
manifest, raw region/function/line/branch-outcome totals, `owned.rs` totals and
all reviewed gaps. Do not carry forward the value-mutation percentages: this
milestone changes both source and test denominators.

## Pinned libxml2 oracle

**PENDING — final external-oracle result.** Record the pinned libxml2 2.15.4
build configuration, helper/source hashes, cases, pass/fail counts and triaged
differences. Comparisons must account for libxml2 Text coalescing,
`xmlUnlinkNode` detached ownership and its ability to unlink the document
element. No C code may enter the Rust dependency graph or persist pointers after
unlink/free.

## Benchmark evidence

**PENDING — final benchmark result.** Record release compiler, host, revision,
workload parameters, repetitions and raw artifact hashes. Separate append,
first/middle/last/deep removal, tombstone traversal, serialization and drop
timings. Report yaxml-only results as such. A libxml2 comparison requires matched
fresh-leaf plus unlink/free work; unlink alone is not equivalent to permanent
removal.

## Independent review

The independent [holistic review](review-owned-structure.md) inspected every
changed line from `c695e61` through `08a1ff6` and returned **PASS** with no
Critical, Important or Minor findings. All 15 frozen-plan fault classes and the
expanded allocation, handle, independent-model, benchmark and oracle failure-gate
mutations were caught and restored. Clean format, clippy, all-target tests,
dependency and unsafe-code checks passed; the review records exact commands and
evidence limits.

## Remaining scope

This milestone does not construct elements or attributes, insert siblings,
change names or namespaces, move existing nodes, detach nodes, copy/import/adopt
across documents, repair namespaces, reclaim pool/attribute storage, reuse slots
or compact tombstones. Those operations need separate ownership, cycle,
namespace and quota contracts. Sustained fuzzing, final coverage, matched
performance and the pinned structural oracle remain open until the **PENDING**
sections above are replaced with final evidence.
