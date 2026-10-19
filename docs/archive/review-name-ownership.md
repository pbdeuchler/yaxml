## Holistic Review

**Plan:** [DTD foundations](plans/dtd-foundations.md), limited to `Ownership foundation`
**Base:** `2609aeff4c5287b5fbbdadd15eb55b1cbd296aa0` (provided)
**Files Changed:** 51
**Commits:** 4

### Summary

The assigned source scope passes review at candidate `e3dafa83ca2f7b2430a0613824010c337b8386ec`. I read every changed production file in full and reviewed `tests/name_ownership.rs`, `fuzz/names.rs`, `fuzz/driver.rs`, and `fuzz/writer.rs` in full; the remaining mechanical consumer migrations are assigned to a separate reviewer. Across correctness, security, elegance, simplicity, design, idiomatic Rust, plan adherence, and test coverage, the implementation keeps one validated `Cow<str>` QName, constant-time stored views, bounded input-derived fuzz work, and owned-capable PI targets without changing DTD acceptance or introducing unsafe code or dependencies.

### Mutation Results

**Baseline:** green (`95` library tests, `7` name-ownership tests, and `40` writer tests)

| # | Mutation | Location | Predicted failures | Actual failures | Verdict |
| - | -------- | -------- | ------------------ | --------------- | ------- |
| 1 | Bypassed `Name::new` QName validation and derived a split directly | `src/name.rs:38` | `constructor_rejects_invalid_qnames_at_utf8_byte_offsets` | `constructor_rejects_invalid_qnames_at_utf8_byte_offsets` failed immediately when an invalid name reached the private invariant assertion | caught |
| 2 | Discarded valid prefix metadata by forcing `prefix_length` to `None` | `src/name.rs:39` | `getters_recompose_bounded_borrowed_and_owned_qnames` | `getters_recompose_bounded_borrowed_and_owned_qnames` failed on `p:a`, reporting `None` instead of `Some("p")` | caught |
| 3 | Stored PI data in the owned target range | `src/owned.rs:1168` | `owned_names_and_pi_targets_survive_inputs_and_round_trip_through_all_surfaces` | `owned_names_and_pi_targets_survive_inputs_and_round_trip_through_all_surfaces` failed at the owned target assertion (`literal payload` versus `πtarget`) | caught |

Every mutation was restored immediately. The final restored `tests/name_ownership.rs` run passed all seven tests, `git status --porcelain` is empty, `git diff --exit-code` is clean, and HEAD is exactly `e3dafa83ca2f7b2430a0613824010c337b8386ec`.

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
| One owned-capable QName buffer with private split and namespace metadata | Met | `Name` stores one `Cow<str>`, `Option<NonZeroUsize>`, and optional namespace `Cow`; all fields are private. |
| Public validated construction with exact UTF-8 byte offsets and no borrowed-input copy | Met | `Name::new` validates through the shared QName grammar, moves owned buffers, and preserves namespace metadata without resolving it. Public tests cover accepted and rejected Unicode names, exact byte offsets, borrowing, and ownership. |
| Getter lifetimes and constant-time views | Met | Getters borrow `Name`; the compile-fail documentation records the shortened lifetime. Parser, serializer, and owned-pool views retain validated split metadata without rescanning. Private constructor assertions are constant-time. |
| Owned-capable PI targets across events and both DOMs | Met | `Event` and borrowing `NodeKind` use `Cow<str>`; direct parsing borrows, streaming remains token-lending, owned DOM storage copies through the existing budget, and both serializers preserve targets. |
| Preserve declaration borrowing and avoid a partial static-conversion API | Met | `Declaration` remains borrowed and no event-to-static conversion was introduced. |
| Preserve parsing behavior and keep DTD/provenance work future-scoped | Met | Name validation was moved without changing its grammar; DTD tokens remain unsupported and this slice adds no generated-source provenance semantics. |
| Security and resource constraints | Met | Public construction performs one linear scan and no hidden copy for borrowed inputs. The fuzz name model caps its source slice at 96 raw bytes; `String::from_utf8_lossy` can expand that to at most 288 UTF-8 bytes, inside the existing 1 MiB driver ceiling. Owned PI target copying uses the existing cumulative owned-byte budget. No unsafe code or dependency change appears in the diff. |
| Independent public tests and fuzz dispatch | Met | Tests cover constructor/getters, Unicode boundaries, namespace deferral, all five parsing surfaces, owned PI DOM/writer round trips, and declaration borrowing. Fuzz dispatch runs before grammar/encoding selection and checks finite literal and bounded arbitrary cases. All three crucial mutations were killed. |
| Size, performance, full fuzz coverage, and XMLTS evidence | Not assessed | These are external final-milestone evidence gates owned by the root and other reviewers. This report makes no claim about their pending results. |

### Verdict

**PASS** -- Zero issues found in the assigned production, public-test, and fuzz-model source scope. Source implementation is ready for the remaining independent evidence gates.

### Integration evidence review

Root reviewed the production diff, independent tests and fuzz target, both
scoped reports, retained validation logs, measured layouts, complete XMLTS
audit, fresh fuzz-only report and paired performance evidence. The
[migration review](review-name-migrations.md) covers all remaining changed
non-production files. Faults were restored and the reviewed source freeze is
`e3dafa83ca2f7b2430a0613824010c337b8386ec`.

The [validation report](validation-name-ownership.md) records 509 passing
ordinary test executions, nine ignored tests, thirteen doctests, Clippy and
formatting checks; unchanged 12,925-row XMLTS results; source-complete fresh
fuzz profiles with no exclusions; and sixteen-byte layout reductions. `name.rs`
reaches all instrumented outcomes, while crate-wide coverage remains below 100%.
The root closure corrects the fuzz-input description to distinguish 96 raw
bytes from the up-to-288-byte lossy UTF-8 result. No code changed after validation.

The performance gate required investigation, not a claim that this functionality
change improves every workload. Original full and repeat matrices retain a
roughly 12% text/push/seven-byte slowdown, a roughly 4% buffered/seven-byte
slowdown and a roughly 4.7% deep Reader slowdown. Independently outlined private
helpers remove the push difference without library changes; unmodified fresh
controls reproduce it. This supports compiler/caller-context sensitivity but
does not explain all regressions. Root accepts the needed ownership foundation
with this explicit performance debt and keeps the original benchmark unchanged.
Neither an unproven library hint nor the outlined helper is part of integration.

**Integration PASS with recorded performance debt.** Ownership invariants,
semantic behavior, resource policy and source-review gates pass. Broader
performance superiority, DTD implementation, 100% fuzz coverage, allocator
support on the target compiler, complete libxml2 behavior and language adapters
remain open. The next DTD work must establish real source provenance before
generated markup; owned names alone do not activate replacement input.
