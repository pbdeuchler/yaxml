## Holistic Review

**Plan:** [Construction binding fuzzing](plans/construction-binding-fuzz.md), recording the root-assigned corrected binding/`NameBytes` precedence catalog contract
**Base:** `e3dafa83ca2f7b2430a0613824010c337b8386ec`
**Files Changed:** 4
**Commits:** 2 (`07734bf9b87a9744d51ff27981eaf03b1f439c4e`, `092775f30fa21c5a7b389299b9f50ca0298fbcb8`)

### Summary

The final two-commit series follows the scoped contract and changes no production source: the `src` tree remains `408df38ccc35a3a3f2cab1cdd5b86734818d3490`. Its 12 literal cases independently distinguish the generated-prefix `NameBytes` failure, the later incompatible exact-attribute namespace failure, and compatible exact-prefix reuse across create, UTF-8 append, UTF-16LE append, and UTF-16BE append. Two test-strength gaps found in the first commit were corrected in the follow-up; the final dispatch and encoding assertions catch both deliberate regressions.

The model is bounded to three items, one attempted construction, and at most one recovery construction per selected case. Errors include exact kinds and locations, failed appends preserve a complete public snapshot and serialization, recovery uses the same editor, and successful cases check literal names, ordered declarations/attributes, topology, provenance, and serialization. The implementation is concise, test-only instrumentation is compiled only under `cfg(test)`, and the new nine-byte seed is registered in ordinary replay.

### Mutation Results

**Baseline:** green — five focused validation tests and the corpus replay/truncation test passed. Strict Clippy and formatting checks also passed.

| # | Mutation | Location | Predicted failures | Actual failures | Verdict |
| - | -------- | -------- | ------------------ | --------------- | ------- |
| 1 | Delete the binding catalog call from the real validation leaf dispatch | `fuzz/construction_validation.rs:971` | `top_level_driver_reaches_every_binding_limit_selection` observes no selection | Failed with `None` instead of `Some((0, 0))` | caught |
| 2 | Encode all append fixtures as UTF-8, ignoring the selected surface | `fuzz/construction_validation.rs:858` | Catalog fails at the first UTF-16LE case | Failed at surface 2, variant 0 on the literal encoded prefix | caught |
| 3 | Change the expected generated-prefix overflow offset from byte 2 to byte 1 | `fuzz/construction_validation.rs:727` | Catalog reports exact error-location mismatch | Failed at surface 0, variant 0 with actual offset 2 versus expected offset 1 | caught |

Retained independent logs:

- Baseline: `/private/tmp/yaxml-name-binding-review-v2-baseline.log` (`042227a44210188bed5435a369f3704320318af35cc8bc8ad2f7c53275fb0c27`)
- Seed replay: `/private/tmp/yaxml-name-binding-review-v2-seed.log` (`0689267eac42af4299b41df3a92e707930415e85611a3c0ddbd0a98ab0ed4f5c`)
- Dispatch mutation: `/private/tmp/yaxml-name-binding-review-v2-mutant-dispatch.log` (`a50c0dea0b97ef64edca3c34a81370e87ee8af517965fbca30df0762234c6f08`)
- Encoding mutation: `/private/tmp/yaxml-name-binding-review-v2-mutant-surface.log` (`d07f85e220d110032f0d419cec34e7d4ad114ed4ce67282f78c05f1d61930666`)
- Error-offset mutation: `/private/tmp/yaxml-name-binding-review-v2-mutant-offset.log` (`059aaa4a349bc76256a23348b7381baa74fa5d781ed886fcf2a256dcbf9966dc`)
- Clippy: `/private/tmp/yaxml-name-binding-review-v2-clippy.log` (`bfee8a24d013611e65af683e85f46902682d4157c95325575ef2574612918b79`)
- Formatting: `/private/tmp/yaxml-name-binding-review-v2-fmt.log` (`e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`)

The isolated review worktree `/private/tmp/yaxml-name-binding-review` is clean after every mutation. Its follow-up has a different cherry-pick SHA, but the reviewed authoritative tip is `092775f30fa21c5a7b389299b9f50ca0298fbcb8` with model blob `f389da6dce7e3f1f47079e75ceeaf9b134a65564`.

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
| Corrected explicit default `v`, local `r=u`, and exact `r` attribute fixture | Met | The default mismatch makes the matching-prefix and generated-prefix paths reachable. |
| Exact `max_name_bytes=2` precedence | Met | Predicts `LimitExceeded(NameBytes)` at element prefix byte offset 2 before the later attribute error. |
| Exact `max_name_bytes=3` precedence | Met | Predicts semantic `Namespace` at `AttributeItem` index 2 with no component or byte offset. |
| Compatible `r`/`u` control | Met | Requires `r:e`, `r:a`, ordered explicit declarations, and no generated declaration. |
| Four surfaces by three variants | Met | All 12 cases are enumerated; literal byte prefixes and parsed ranges distinguish UTF-8, UTF-16LE, and UTF-16BE. |
| Real fuzz-driver dispatch | Met | Test-only observation proves every selector and representative empty/malformed/encoded inputs reach the leaf through `driver::exercise`. |
| Exact public state and serialization | Met | Failures preserve full snapshots and bytes; successes check literal serialization and full relevant topology. |
| Atomic recovery | Met | Every failed append retries successfully through the same editor; failed root creation uses a separate literal valid-constructor control because no editor exists. |
| Bounded work | Met | Each input selects one fixed binding case; no loop scales with arbitrary input length. |
| Corpus and documentation | Met | The 9-byte BE-compatible seed is replayed and documented without overstating coverage. |
| No production changes | Met | Base and tip `src` tree hashes are identical. |

The seven review dimensions are satisfied: literal outcomes and ranges are correct; helpers remain readable and narrowly scoped; no new abstraction or dependency was introduced; test state is thread-local and test-only; Rust usage and bounded arithmetic are idiomatic; every assigned behavior is represented; and the three independent final-tip mutations prove dispatch, encoding selection, and exact diagnostic strength.

### Verdict

**PASS** -- Zero issues found. Implementation is ready.

### Integration evidence review

Root reviewed the complete two-commit model and inventory diff, the final scoped
report and its fault outcomes. The author series was cherry-picked onto main
`4348e49` as `5106eb5` and `f9c251a`; its production source tree remains
`408df38ccc35a3a3f2cab1cdd5b86734818d3490`. Root independently confirmed the
two-byte versus three-byte error precedence using the actual released library.

The [fresh validation](validation-construction-binding.md) records all 64
fuzz-smoke tests passing, complete production source coverage denominators and
unchanged source/harness identities. Seven additional regions/lines are covered;
instrumented branch coverage remains 909/962. No parser behavior, dependency,
public API, production hook or coverage exclusion changes. The final commits
after the freeze only record this evidence and update the inventory/roadmap.

**Integration PASS.** Crate-wide 100% fuzz coverage and sustained fuzz campaigns
remain open.
