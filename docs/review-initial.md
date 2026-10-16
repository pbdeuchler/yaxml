## Holistic Review

**Plan:** `/private/tmp/yaxml-review/docs/roadmap.md`, first bounded UTF-8 reader/immutable DOM slice and its test/replay infrastructure.
**Base:** `8810b88`; final reviewed snapshot `e6cf836`.
**Files Changed:** 30. Implementation, tests, CI, and validation documentation were reviewed here; the generated header inventory's extraction/provenance audit was performed by the parent reviewer and was not independently rederived here.
**Commits:** 8.

### Summary

The first implementation slice follows the planned shared-parser design: strict slice events feed a flat immutable DOM, with explicit limits, borrowed unchanged text, namespace identities, and structured errors. The code is concrete and iterative, retains only the allowed dependency, and states its compatibility and security limits accurately. Review found one test loop that could hang after a parser regression; commit `f984b56` fixes it, and a further mutation verified immediate failure at unexpected EOF.

This verdict covers the first slice, not completion of P0 measurement gates or the whole replacement goal. Full conformance, a built libxml2 2.15.4 semantic oracle, sustained coverage-guided fuzzing, allocation-failure injection, 100% execution coverage, comparative performance, allocator-backed storage, and true buffered streaming remain explicit open work.

### Mutation Results

**Baseline:** green. Initial snapshot: 7 DOM, 12 reader, 12 fuzz-smoke, 4 example tests, and 1 doctest. Follow-up snapshot: 13 fuzz-smoke tests, for 37 checks in total. Strict Clippy checks passed. Every mutation was restored immediately, and the review worktree is clean.

| # | Mutation | Location | Predicted failures | Actual failures | Verdict |
| - | --- | --- | --- | --- | --- |
| 1 | Permit a mismatched `</s>` end tag. | `src/reader.rs:587` | `malformed_documents_are_rejected` | `malformed_documents_are_rejected`; DOM and fuzz-smoke suites stayed green. | caught |
| 2 | Return a fresh namespace ID when an equivalent URI already exists. | `src/reader.rs:519` | Malformed expanded-attribute and namespace-alias rejection tests. | `malformed_documents_are_rejected`, `namespace_interning_preserves_aliases_and_scope`. | caught |
| 3 | Close a DOM element's preorder range immediately after itself, excluding descendants. | `src/dom.rs:91` | DOM descendant/structure tests; fuzz structural invariants. | `document_structure_and_navigation`, `generated_tree_preorder_and_parent_invariants`; seven fuzz tests: `each_resource_budget`, `generated_reference_semantics`, `generated_valid_documents`, `known_acceptance`, `mutate_each_seed_byte`, `projection_preserves_all_payload_kinds`, `replay_corpus_and_every_truncation`. Reader tests and doctest stayed green. | caught |
| 4 | After the test fix, permit mismatched `</x>` and run the sticky-error test directly. | `src/reader.rs:587`, `tests/reader.rs:421` | Immediate `fatal_errors_are_sticky` failure at unexpected EOF. | `fatal_errors_are_sticky` failed in 0.00 seconds with `invalid document reached EOF: "<r></x>"`. | caught |

The first three mutations ran against the integrated `bd52745` implementation. The fourth verified the test fix on `f984b56`. Later commits add tests and documentation; core source remains unchanged.

### Findings

#### Critical

None.

#### Important

None.

#### Minor

None remaining. The initial unexpected-EOF loop finding at `tests/reader.rs:417` was fixed and verified before this verdict.

### Plan Adherence

| Requirement | Status | Notes |
| --- | --- | --- |
| Pure Rust core, unsafe prohibition, only `memchr`. | Met | Crate/package lints forbid unsafe; the lockfile contains only the allowed library dependency. Standard-library and dependency internals remain separate boundaries. |
| Strict UTF-8 XML subset with namespaces and normalization. | Met | Accepted/rejected fixtures cover names, declarations, document phases, comments, PI, CDATA, references, namespace aliases, default namespaces, and rollback. DTDs and unsupported encodings fail explicitly. Full conformance-suite verification remains open. |
| Finite input, token, depth, attribute, event, and normalized-string budgets. | Met | Boundary tests exercise acceptance and rejection. Requested string capacity is documented separately from actual process memory. Checked arithmetic and fallible reservation protect growth paths; allocator-failure injection remains open. |
| Borrowed immutable DOM built from validated events. | Met | Flat records, constant-time sibling attachment, iterative traversal/destruction, full element spans, and document-bounded node views. Deep/wide and generated invariant tests exercise relationships. Document-local ID limitations are explicit. |
| Avoid namespace lookup/hash amplification. | Met | Scoped prefix lookup and interned namespace identities replace repeated URI hashing in attribute checks. Alias equality and normalization remain tested. The recorded exploratory probe supports this local change without claiming universal performance. |
| Regression tests and fuzz entry points for implemented input APIs. | Met | Reader/DOM acceptance, logical payload projection, traversal invariants, policies, terminal states, and checked lookup are exercised. Mutation results establish that crucial semantic regressions are detected. |
| Replay, preliminary oracle comparison, benchmark and coverage infrastructure. | Met | Safe process replay, bounded input, explicit oracle classification, isolated benchmark modes, and coverage artifacts exist. The pinned semantic oracle and full performance comparison are subsequent gates. |
| Honest scope, coverage, and release claims. | Met | Reviewed reports distinguish fuzz-only and ordinary-test coverage. Fuzz evidence is 1,332/1,402 regions, 76/88 functions, 831/846 lines, and 227/228 compiler-instrumented branch outcomes; unreached production code remains in the denominator. These figures do not establish complete security or behavior coverage. |

Correctness, simplicity, idiomatic Rust, dependency boundaries, API ownership, operational diagnostics, and test effectiveness produced no unresolved findings for this slice. The validation document closes the added documentation link and preserves the remaining measurement and compatibility gaps.

### Verdict

**PASS** -- Zero unresolved issues found in the reviewed first slice. The implementation is ready for local integration within its documented scope.
