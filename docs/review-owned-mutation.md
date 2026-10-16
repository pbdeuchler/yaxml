# Bounded owned-value mutation holistic review

## Holistic Review

**Plan:** [`docs/plans/owned-mutation.md`](plans/owned-mutation.md)
**Base:** `339bdad`; reviewed tip `8a89d29`
**Files Changed:** 28
**Commits:** 8

### Summary

The implementation follows the frozen value-only mutation plan and keeps the
public boundary small: checked owned handles, persistent provenance, an exclusive
editor, two existing-value setters, typed failures, and cumulative edit work.
The storage path validates before allocation and publishes one range after the
last fallible operation. The independent model, literal quota cases, public
snapshots, deep/wide cases, and serializer round trips test the contract at
different layers. No review finding survived inspection or deliberate mutation.

The clean baseline at `8a89d29` passed 225 all-target tests with six ignored
subprocess helpers, eight doctests, formatting, strict Clippy, the normal
dependency tree (`memchr` only), the unsafe-source audit, and `git diff --check`. After every
fault the changed source was restored before the next fault. The final source
tree equals `8a89d29:src`, tree object
`610b44ca7ae9c3c52aaffd2e4fb3311c0614b97b`.

### Mutation Results

**Baseline:** green

| # | Mutation | Location | Predicted failures | Actual failures | Verdict |
| - | -------- | -------- | ------------------ | --------------- | ------- |
| 1 | Reject every node-value edit before resolution | `src/owned.rs:393` | public all-kinds golden; modeled operation smoke | `every_value_kind_has_an_independent_complete_document_byte_golden`; `mutation_operations_predict_results_and_preserve_complete_trees` | caught |
| 2 | Reset cumulative owned work whenever an editor opens | `src/owned.rs:336` | reopen quota golden in public and fuzz tests | `exact_full_capacity_and_persistent_reopen_quotas_match_literal_goldens`; `mutation_literal_quota_sequences_cover_reopened_and_encoded_trees` | caught |
| 3 | Charge only the pool-capacity delta | `src/owned.rs:154` | exact full-capacity public and fuzz goldens | `exact_full_capacity_and_persistent_reopen_quotas_match_literal_goldens`; `mutation_literal_quota_sequences_cover_reopened_and_encoded_trees` | caught |
| 4 | Charge twice the requested full capacity | `src/owned.rs:154` | literal fuzz quota sequences | `mutation_literal_quota_sequences_cover_reopened_and_encoded_trees` | caught |
| 5 | Publish an empty range before validation can fail | `src/owned.rs:404` | rejected-edit rollback snapshot | `errors_have_exact_utf8_offsets_and_rejected_editors_recover` | caught |
| 6 | Accept same-slot handles from another document | `src/owned.rs:383` | move/foreign-handle public case | `source_overwrite_and_document_moves_preserve_checked_identity_and_provenance` | caught |
| 7 | Treat an element name range as editable node data | `src/owned.rs:395` | wrong-kind precedence case | `target_resolution_and_value_length_precede_character_and_lexical_checks` | caught |
| 8 | Select an attribute by local name while ignoring namespace | `src/owned.rs:429` | expanded-name state snapshot | `expanded_attribute_names_preserve_prefixes_order_and_namespace_scopes` | caught |
| 9 | Report Unicode scalar counts instead of UTF-8 byte indices | `src/owned.rs:467` | exact error-offset table | `errors_have_exact_utf8_offsets_and_rejected_editors_recover` | caught |
| 10 | Swap the edited attribute with the element's first attribute | `src/owned.rs:447` | namespace metadata/order snapshot | `expanded_attribute_names_preserve_prefixes_order_and_namespace_scopes` | caught |
| 11 | Omit empty owned CDATA during serialization | `src/serialize.rs:71` | empty lexical-node byte golden | `empty_values_preserve_nodes_but_only_empty_text_disappears_on_output` | caught |
| 12 | Accept a double dash in comment data | `src/owned.rs:476` | lexical error table | `errors_have_exact_utf8_offsets_and_rejected_editors_recover` | caught |
| 13 | Replace contiguous preorder traversal with recursive child walking | `src/owned.rs:733` | 8,192-deep, 256 KiB-stack case | `deep_and_wide_edits_serialize_and_drop_without_recursive_traversal` failed in its isolated test subprocess with stack overflow/SIGABRT | caught |
| 14 | Turn equal-value replacement into a quota-free return | `src/owned.rs:404` | equal-value/reopen capacity golden | `exact_full_capacity_and_persistent_reopen_quotas_match_literal_goldens` | caught |
| 15 | Permit edits to existing XMLNS attributes | `src/owned.rs:438` | namespace-declaration precedence case | `target_resolution_and_value_length_precede_character_and_lexical_checks` | caught |

The recursive fault ran only in the targeted Cargo test subprocess. Its expected
stack abort terminated that subprocess, and the source was restored immediately
afterward.

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
| Separate checked owned handles | Met | Private document identity and slot reject foreign and out-of-range handles; moves preserve identity. |
| Non-recycled identity with explicit exhaustion | Met | Safe atomic assignment occurs after successful XML construction; local-counter tests cover the terminal value. |
| Preserve original source provenance | Met | Parsed nodes retain `Some(original_range)` through successful and rejected edits. |
| Exclusive, nonterminal editor | Met | Ordinary mutable borrowing prevents live views across edits; errors leave the editor reusable and drop performs no work. |
| Existing leaf and ordinary-attribute values only | Met | Text, CDATA, comment, PI data, and expanded-name-selected existing attributes are supported; structural and namespace edits stay deferred. |
| Required validation and error precedence | Met | Target resolution, value bytes, XML characters, lexical rules, owned budget, and allocation are ordered as planned with byte offsets. |
| Atomic logical publication | Met | Storage completes before the single range assignment; rollback mutations were caught by complete snapshots. |
| Persistent full-capacity accounting | Met | Parsed capacity is baseline; full new capacity requests persist across editors; exact, delta, double, reset, and equal-value faults were caught. |
| Namespace, attribute order, names, topology, declaration and PI target preservation | Met | Public snapshots, independent model checks, byte goldens and round trips cover these fields. |
| UTF-8, UTF-16LE and UTF-16BE construction | Met | Public and literal quota cases run all three encodings; the operation model also covers a UTF-8 BOM. |
| Iterative deep and wide behavior | Met | The 8,192-depth small-stack case caught an intentionally recursive traversal; 10,000 siblings preserve order. |
| Independent modeled fuzz protocol | Met | The vector model predicts results before production and does not share pool ranges, validation helpers or topology traversal. |
| Measurement and documentation | Met | A phased benchmark runner and scoped release result are documented without a libxml2 comparison claim. |
| Holistic review before integration | Met | This report records a green baseline, 15 caught fault shapes, restoration, limitations and a zero-finding PASS. |

### Evidence limitations

Measured ordinary and fuzz-smoke execution remain below the project's 100%
coverage target. The deterministic replay suite is not a sustained
feedback-guided fuzzing campaign. Allocation failure is not deterministically
injectable, and the external libxml2 oracle does not execute modeled edit
sequences. The benchmark is one local runner execution without repeated trials
or a matched libxml2 comparison. These gaps are already recorded as follow-up
work and do not contradict this bounded milestone's acceptance contract.

### Verdict

**PASS** -- Zero issues found. Implementation is ready for this bounded milestone's integration.
