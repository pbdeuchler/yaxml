# Owned element construction holistic review

**Plan:** `docs/plans/owned-create-slice.md`  
**Base:** `3db2aad`  
**Reviewed tip:** `02cb2a510156202ba1fb1b20ca2054d033115bac`  
**Changed files:** 26  
**Commits:** 23

## Verdict

**PASS (code quality).** No Critical, Important, or Minor finding remains in
the assembled implementation, public tests, independent fuzz model, bounded
oracle tooling, source-inventory changes, or current contract documentation.
At the reviewed `02cb2a5` boundary, the milestone is not yet complete under its
frozen plan: fresh source-complete fuzz coverage, actual pinned libxml2 2.15.4
oracle execution, and construction/append benchmark evidence remain external
pending gates.

## Findings

### Critical

None.

### Important

None.

### Minor

None. An earlier review found that `OwnedDocument::edit` documented only the
created-node and owned-byte persistent expenditures. `02cb2a5` corrects the
contract to include created attributes and removes stale wording that described
created-node provenance as future work.

## Inspected scope

Every changed line in `3db2aad..02cb2a5` was reviewed. The diff contains 6,980
insertions and 135 deletions across 26 files. Production review covered the new
construction API types and errors, `OwnedDocument::create`,
`Editor::append_element`, the construction planner, namespace resolution,
quota accounting, fallible reservations, arena publication, topology updates,
and public exports. Test review covered private failpoints, the 17-test public
construction suite, the independent fuzz model, fuzz-smoke hooks, the fixed
external-oracle catalog and protocol gates, and the recursive source inventory
used by the feedback campaign. README, owned-DOM contract, and frozen-plan
updates were checked against the implemented behavior.

The production design remains iterative and safe Rust. It validates caller
strings and explicit item counts before semantic planning, charges namespace
work before each bounded probe or comparison, checks cumulative persistent
ceilings before reservation, and publishes records, links, counters, and
handles only after all fallible work succeeds. Created document and element
records have no parsed-source provenance. Exact-prefix requests remain
lexically and semantically consistent with inherited and generated bindings.
Created attributes preserve caller item order; generated declarations appear
at first use. Stable handles, ancestor sizes, sibling links, and the permanent
linked-traversal transition follow the existing owned-tree contract.

The reservation seam covers node, attribute, pool, and every scratch-growth
site. Each scratch skip loop proves that it eventually reaches the named site.
Append failures compare the complete serialized state, counters, topology, and
then retry the same operation with the same editor. The seam models returned
reservation failures; it is not evidence about arbitrary allocator or
operating-system OOM behavior.

The independent construction model uses a bounded finite namespace catalog and
input-derived names, values, limits, and operations. It predicts exact success
state or typed error details before calling production. It checks attribute and
namespace order, expanded names, generated prefixes, UTF-8 byte offsets,
failure atomicity, stable pre-existing topology, persistent counters, UTF-8 and
UTF-16 parents, newly created documents, and interactions with value edits,
leaf insertion, and subtree removal. A sparse long inherited-prefix path forces
scratch growth with 512 to 4,336 prefix bytes and checks the exact successful
name plus a one-byte-short atomic `NameBytes` rejection. Dedicated negative
model tests prove that changed expanded names, reordered attributes, and changed
quota results are detected. Formatting checks cover the new public errors,
locations, enum outcomes, and failing `fmt::Write` sinks without imposing the
quadratic sink sweep on every fuzz input.

The campaign inventory now requires the exact 15-file production source set,
including `src/owned/construction.rs`, and expects LLVM's relative
`owned/construction.rs` report label. Enumeration rejects symlinks and missing,
extra, duplicate, over-depth, or over-cap source layouts while preserving full
source identities. The external construction oracle has a bounded 12-case
catalog and fails closed on helper status, stderr, version/feature identity,
changed output, extra output, or truncation. Its one documented attribute
escaping spelling difference is exact and case-specific. The C helper remains
test tooling and is not linked into the Rust library.

## Deliberate mutation results

Each source fault was introduced separately in the isolated review worktree.
The named failure was predicted before execution. The production source was
restored and its Git blob verified against `02cb2a5` before applying the next
fault.

| # | Deliberate fault | Predicted test | Observed result |
| - | ---------------- | -------------- | --------------- |
| 1 | Generate automatic prefixes with `zz` instead of the frozen `ns` stem | `explicit_items_keep_order_and_automatic_declarations_follow_first_use` | Caught. The exact serialization reported `zz1:first`/`xmlns:zz1` where `ns2:first`/`xmlns:ns2` was required. |
| 2 | Report an ordinary unprefixed `xmlns` attribute's semantic namespace error as its `Prefix` component | `namespace_and_duplicate_errors_identify_the_implicated_input` | Caught. The test required `AttributeItem { index: 0, component: None, byte_offset: None }` and rejected the invented component. |
| 3 | Bypass the zero `max_created_nodes` rejection during creation | `item_depth_and_persistent_limits_cover_exact_boundaries_and_reopen` | Caught. The test required `LimitExceeded(CreatedNodes)` and rejected the successful construction. |

The restored `tests/owned_construction.rs` run passed all 17 tests. Raw mutation
logs are retained at `/private/tmp/yaxml-owned-create-review-mutant-{1,2,3}.txt`;
the restored run is
`/private/tmp/yaxml-owned-create-review-restored.txt`.

## Plan adherence

| Frozen-plan gate | Status | Evidence and boundary |
| ---------------- | ------ | --------------------- |
| Public creation and append surface | Met | Public spec types, prefix policies, limits, errors, locations, exports, examples, and compile-fail ownership checks are present. |
| Namespace and ordered-attribute semantics | Met | Explicit/default/automatic policies, reserved bindings, inherited shadowing, duplicate expanded names, declaration order, generated-prefix precedence, and exact lexical/expanded results have public and independent-model tests. |
| Validation and error precedence | Met | Handle/kind/depth/item, length, XML validity, declaration, resolution, duplicate, persistent quota, and reservation stages have exact typed assertions and UTF-8 byte offsets. |
| Resource accounting and bounded temporary work | Met | Supplied/name/item/depth/namespace-work limits, persistent node/attribute/pool quotas, checked arithmetic, bounded iterative scope walks, scratch preflight, and every reservation site are exercised. |
| Failure atomicity and stable topology | Met | Failpoints and public tests verify no partial record/link/counter publication, retained documented charges, unchanged prior trees, and same-editor recovery. |
| Independent public and fuzz models | Met | Public goldens and the separate bounded construction model check exact trees and errors across UTF-8/UTF-16, created/parsed documents, long inherited prefixes, and existing edit operations. |
| Pure safe Rust and dependency constraint | Met | The crate forbids unsafe code; a source search found no unsafe block/declaration or Rust foreign ABI. The locked runtime graph is `yaxml -> memchr 2.8.3`. |
| Source-complete feedback-campaign inventory | Met as tooling | The exact nested 15-file inventory and relative report labels are tested. This establishes the input manifest for a future run, not coverage results. |
| Fresh source-complete fuzz coverage | **Pending** | No coverage percentage is claimed here. Root is producing the source-complete profile separately; its raw profiles, tool identities, source hashes, exclusions, and uncovered regions require their own validation. Ordinary tests and failpoints are recorded separately as deterministic verification. |
| Pinned libxml2 2.15.4 oracle | **Pending execution** | The fixed helper, expected table, protocol parser, version gate, and fake-helper failure tests are reviewed and green. An actual pinned 2.15.4 helper run and retained result are still required. |
| Construction/append benchmark evidence | **Pending** | This review found no retained bounded construction/append benchmark evidence at `02cb2a5`. The slice plan does not require a matched C timing matrix; no claim of parity or superiority over libxml2 follows from the implementation or correctness tests. |
| Independent holistic review | Met for code quality | This report covers the assembled `02cb2a5` boundary and the three restored production mutations above. |

## Verification evidence

The assembled checkpoint's retained `cargo test --all-targets --offline
--locked` log contains 404 passed tests, zero failures, and seven ignored tests.
The reviewer independently ran the restored 17-test public construction suite,
strict all-target Clippy, formatting, and doctests. Results were:

- `cargo +stable test --test owned_construction --offline --locked`: 17 passed.
- `cargo +stable clippy --all-targets --offline --locked -- -D warnings`: passed.
- `cargo +stable fmt --all -- --check`: passed.
- `cargo +stable test --doc --offline --locked`: 11 passed, including four
  compile-fail doctests.
- `cargo +stable tree --offline --locked`: only `memchr v2.8.3` beneath
  `yaxml`.
- `git diff --check`: passed.
- Unsafe/foreign-ABI syntax search over Rust production, fuzz, tests, examples,
  and benchmarks: no matches.

After mutation restoration, `src/owned/construction.rs` had Git blob
`c527a9fc78ea24be05c63ae36d8a2651ac22247b`, exactly matching `02cb2a5`; the
complete `src` tree was
`34f9acb22ae65b99fcea2cc1cc52e9ee6e036b3f`. Before this report was added, the
review worktree was clean at the reviewed tip.

## Evidence limitations

Passing deterministic tests and killing the three selected faults do not prove
all possible namespace, allocation, or state-machine faults are absent. The
failpoint is a safe returned-error seam, not arbitrary OOM injection. The fuzz
model is deliberately bounded, and this review did not run a sustained
feedback-guided campaign. The oracle implementation and fake-helper gates do
not substitute for executing the pinned external library. No construction
timing was measured during this review. Fresh source-complete fuzz coverage,
actual pinned-oracle results, and bounded construction/append benchmark
evidence must be reported against their exact source/tool manifests before the
frozen milestone can be declared complete.
