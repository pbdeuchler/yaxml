## Holistic Review

**Plan:** `docs/plans/owned-leaf-structure.md`
**Base:** `c695e61` (the supplied local-main boundary)
**Files Changed:** 34
**Commits:** 18 (`c695e61..08a1ff6`)

### Summary

The implementation follows the bounded owned leaf-structure plan: it preserves stable document-scoped handles, publishes append state only after fallible reservations, removes subtrees iteratively, maintains reciprocal links and exact subtree sizes, and keeps cumulative quotas persistent. The public, private, independent-model, benchmark, and external-oracle tests express the important state transitions and rejected-operation guarantees clearly. No Critical, Important, or Minor finding remains after the full changed-line review and deliberate-fault audit.

### Mutation Results

**Baseline:** green

Every mutation was introduced separately in the isolated review worktree, run against the named test, and restored before the next mutation. “Caught” means the predicted semantic assertion failed; compile-only failures were corrected before recording the behavioral result.

| # | Mutation | Location | Predicted failures | Actual failures | Verdict |
| - | -------- | -------- | ------------------ | --------------- | ------- |
| 1 | Reject every Text append | `src/owned.rs:594` | `every_leaf_including_empty_payload_changes_topology_and_preserves_lexical_kind` | Same test rejected the first required successful append | caught |
| 2 | Select a tombstoned slot as the next created ID | `src/owned.rs:691` | `moved_documents_keep_created_handles_and_foreign_documents_reject_them` | Same test reported “a removed slot was reused” | caught |
| 3 | Resolve a tombstone as a live edit operand | `src/owned.rs:821` | `placement_handle_and_protection_checks_precede_payloads_and_quotas` | Same test observed `WrongNodeKind` instead of `InvalidHandle` | caught |
| 4 | Leave the predecessor's `next` link pointing at the removed node | `src/owned.rs:770` | sibling-removal conformance | `first_middle_last_sole_and_every_kind_removal_leave_exact_sibling_links` failed | caught |
| 5 | Leave the parent's `first_child` link pointing at the removed node | `src/owned.rs:772` | sibling-removal conformance | `first_middle_last_sole_and_every_kind_removal_leave_exact_sibling_links` failed | caught |
| 6 | Leave the successor's `previous` link pointing at the removed node | `src/owned.rs:775` | private reciprocal-link reconstruction | `removal_stales_exact_subtree_and_updates_every_sibling_position` failed | caught |
| 7 | Leave the parent's `last_child` link pointing at the removed node | `src/owned.rs:777` | sibling-removal conformance | `first_middle_last_sole_and_every_kind_removal_leave_exact_sibling_links` failed | caught |
| 8 | Keep contiguous arena traversal enabled after append | `src/owned.rs:722` | document-order golden | `golden_document_order_differs_from_new_slot_order_and_survivors_keep_source_ranges` failed on the epilog/new-slot order | caught |
| 9 | Update only the direct parent's subtree size | `src/owned.rs:740` | document-order and exact-size golden | `golden_document_order_differs_from_new_slot_order_and_survivors_keep_source_ranges` reported 8 descendants instead of 9 | caught |
| 10 | Publish the created-node counter before node reservation | `src/owned.rs:665` | node failpoint rollback | `node_reservation_failures_precede_pool_work_and_publish_nothing` observed counter 3 instead of 2 | caught |
| 11 | Reset created-node expenditure whenever an editor opens | `src/owned.rs:535` | reopened-editor quota golden | `exact_pool_requests_and_pi_staging_are_shared_and_never_refunded` accepted a ceiling below spent work | caught |
| 12 | Refund created-node expenditure on removal | `src/owned.rs:753` | append/remove quota persistence | `parsed_spare_capacity_and_removed_slots_do_not_reset_creation_work` accepted an extra append | caught |
| 13 | Publish the parent's new last-child link before pool reservation | `src/owned.rs:699` | pool failpoint rollback | `pool_reservation_failures_keep_charge_and_node_capacity_but_publish_nothing` observed the dangling unpublished link | caught |
| 14 | Reserve the pool before the node vector | `src/owned.rs:695` | node-before-pool failure order | `node_reservation_failures_precede_pool_work_and_publish_nothing` observed changed pool accounting/capacity | caught |
| 15 | Accept Text beneath the synthetic document node | `src/owned.rs:643` | placement precedence | `placement_handle_and_protection_checks_precede_payloads_and_quotas` reached the byte ceiling instead of returning `WrongNodeKind` | caught |
| 16 | Allow removal of the document element | `src/owned.rs:756` | protected-root contract | `placement_handle_and_protection_checks_precede_payloads_and_quotas` returned success instead of `ProtectedNode` | caught |
| 17 | Return the preceding Text handle instead of creating an adjacent Text node | `src/owned.rs:594` | Text segmentation golden | `adjacent_and_empty_text_have_distinct_live_handles_and_only_text_is_projected` detected the reused live handle | caught |
| 18 | Give created leaves a parsed source range | `src/owned.rs:723` | leaf provenance golden | `every_leaf_including_empty_payload_changes_topology_and_preserves_lexical_kind` observed `Some(0..0)` instead of `None` | caught |
| 19 | Tombstone only the removed root and retain descendants | `src/owned.rs:794` | mixed-subtree stale-handle assertions | `removing_namespaced_mixed_subtrees_invalidates_every_descendant_only` found a resolving removed handle | caught |
| 20 | Skip the PI target byte ceiling | `src/owned.rs:649` | PI length precedence | `handles_and_parent_rules_precede_payloads_and_quota` returned `CreatedNodes` instead of `ValueBytes` | caught |
| 21 | Validate PI data before its target | `src/owned.rs:655` | target/data argument order | `pi_target_names_have_independent_validation_and_precise_argument_precedence` returned the data's `InvalidCharacter` before target `InvalidName` | caught |
| 22 | Accept invalid and reserved PI targets after character validation | `src/owned.rs:655` | PI target-name matrix | `pi_target_names_have_independent_validation_and_precise_argument_precedence` advanced to invalid data instead of rejecting the target | caught |
| 23 | Convert empty CDATA to empty Text | `src/owned.rs:603` | empty lexical-kind/byte golden | `every_leaf_including_empty_payload_changes_topology_and_preserves_lexical_kind` observed `Text` instead of `Cdata` | caught |
| 24 | Accept CDATA closing delimiters and raw CR | `src/owned.rs:924` | lexical error matrix | `invalid_leaf_payloads_report_exact_byte_offsets_and_leave_complete_state_unchanged` accepted the invalid leaf | caught |
| 25 | Recursively descend before subtree removal | `src/owned.rs:761` | 8,192-deep small-stack test | isolated `deep_removal_and_wide_tombstone_navigation_are_iterative` subprocess terminated with stack-overflow `SIGABRT` | caught |
| 26 | Ignore document identity when resolving an edit operand | `src/owned.rs:821` | foreign-handle precedence | `placement_handle_and_protection_checks_precede_payloads_and_quotas` returned `WrongNodeKind` instead of `InvalidHandle` | caught |
| 27 | Report Unicode scalar indices instead of UTF-8 byte offsets | `src/owned.rs:914` | exact invalid-value offsets | `invalid_leaf_payloads_report_exact_byte_offsets_and_leave_complete_state_unchanged` reported offset 2 instead of 6 | caught |
| 28 | Invert the independent fuzz model's node-kind equality assertion | `fuzz/mutation.rs:725` | structural fuzz smoke | `structural_operations_cover_live_stale_foreign_and_protected_handles` failed immediately on `Document` | caught |
| 29 | Stop counting removed nodes in benchmark accounting | `benches/support/structural_mutation.rs:103` | benchmark accounting unit test | `accounting_tracks_append_remove_and_churn_without_refunds` observed 0 removals instead of 1 | caught |
| 30 | Accept trailing structural-oracle response bytes | `examples/support/structure.rs:309` | protocol malformed/excess-output test | `mismatches_and_malformed_or_excess_output_never_agree` accepted the trailing response | caught |
| 31 | Treat helper stderr as an agreement | `examples/structure_oracle.rs:91` | actual CLI failure gate | `actual_command_enforces_comparison_failure_and_classification_gates` incorrectly succeeded in diagnostics mode | caught |
| 32 | Omit ordinary attributes from the Rust structural projection | `examples/support/structure.rs:44` | namespace/attribute-order projection test | `ordered_attributes_namespaces_and_normalized_values_are_compared` rejected the supposedly exact response | caught |

### Findings

#### Critical

None.

#### Important

None.

#### Minor

None.

### Plan Adherence

| Requirement | Status | Notes |
| ----------- | ------ | ----- |
| Linked owned representation with stable non-reused slots | Met | `Content::Removed`, compact links, permanent preorder-mode transition, and stable record IDs implement the frozen representation. Public and private tests reconstruct live topology. |
| Fresh Text, CDATA, Comment and PI append | Met | Each success creates one leaf with a fresh handle and `None` provenance; placement and all empty/nonempty lexical kinds have public goldens. |
| Permanent allocation-free subtree removal | Met | Removal detaches exterior links, decrements every ancestor, and tombstones a subtree with a rightmost iterative walk. Deep and wide tests cover stack-bounded behavior. |
| Stale, foreign and protected handle rules | Met | Resolution checks identity and tombstones before placement/protection. The synthetic document and document element are protected. |
| Exact linked preorder and `ExactSizeIterator` behavior | Met | Parsed trees retain the contiguous fast path; the first structural success permanently selects linked traversal. Subtree sizes support exact lengths. |
| PI and payload validation/error precedence | Met | Both PI lengths precede scanning; target precedes data; UTF-8 byte indices, XML Name rules, reserved `xml`, and lexical delimiters are tested. |
| Persistent created-node and owned-byte budgets | Met | Counters persist across editors and removal; creation publishes only with the live slot. Pool reservations retain their documented charge on failure. |
| Atomic reservation failure behavior | Met | Safe thread-local node/pool failpoints verify node-before-pool order, no logical publication, retained allowed charges, recovery, skip counts, panic cleanup, and thread isolation. |
| No adjacent-Text merging and explicit serialized projection | Met | In-memory Text handles stay distinct; only serialize/reparse removes empty Text and coalesces maximal Text runs. CDATA remains separate. |
| Independent public and fuzz models | Met | Public tests and `fuzz/mutation.rs` independently predict state/errors and compare complete trees, stale handles, quotas, serialization, UTF-8 and UTF-16 inputs. |
| Structural benchmark with honest accounting | Partial | Nine yaxml cases separate setup, edit, traversal, serialization, validation and drop, and report `comparison_status=unmeasured`; support tests cover counters and order. The plan-required matched libxml2 fresh-leaf and unlink/free comparison remains pending. |
| Version-pinned external structural oracle | Met as tooling; pinned execution pending | CI pins and checksums 2.15.4 and rejects compile/runtime mismatch. The helper bounds protocol I/O, observes live pointers before free, frees detached nodes before the document, and treats nonzero status or stderr as inconclusive. Local 2.15.3 evidence remains explicitly exploratory. |
| Pure safe Rust library and dependency constraint | Met | Cargo's runtime tree is `yaxml -> memchr`; Rust sources contain no unsafe block or declaration, and crate linting forbids unsafe code. The C file is an external test helper that Cargo never compiles or links. |
| Documented bounded scope | Met | General element/attribute construction, moves, namespace repair, reclamation, reuse and compaction remain explicitly deferred. |

### Verification Evidence

The restored `08a1ff6` source passed:

- `cargo fmt --check`
- `cargo clippy --all-targets --offline --locked -- -D warnings`
- `cargo test --all-targets --offline --locked`
- `cargo tree --offline --locked` (only `memchr v2.8.3` beneath `yaxml`)
- `git diff --check c695e61..08a1ff6`
- `git diff --exit-code 08a1ff6 -- .` with only the two review Markdown artifacts excluded; every production, test, fuzz, benchmark, example, workflow and C-helper byte matched `08a1ff6`
- an unsafe-syntax search over every Rust source (no matches)

The external C helper compiled locally with Apple Clang under `-std=c11 -Wall -Wextra -Werror` against Homebrew libxml2 2.15.3. The exploratory catalog completed with 23 exact agreements, 3 documented projections, 6 verified intentional differences, 2 explicit non-comparisons, 0 disagreements and 0 inconclusive cases.

### Evidence Limitations

At the review boundary, checksummed libxml2 2.15.4 execution and final coverage were still pending; the local 2.15.3 result could not substitute for the pinned run. The subsequent [validation report](validation-owned-structure.md) records the completed local 2.15.4 run and measured coverage. The safe reservation seam proves the library's returned-error boundaries and logical atomicity, not arbitrary allocator or operating-system OOM behavior. The benchmark is yaxml-only and supports no libxml2 performance claim. This review did not conduct a sustained feedback-guided fuzzing campaign.

### Verdict

**PASS (code quality)** -- Zero code-quality issues found. The implementation satisfies the reviewed code-quality bar; this bounded milestone remains gated on the frozen plan's matched libxml2 structural benchmark.
