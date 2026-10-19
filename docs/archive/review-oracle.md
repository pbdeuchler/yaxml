## Holistic Review

**Plan:** `docs/roadmap.md`, P0 compatibility-harness evidence gates; scoped oracle contract in `docs/oracle.md` and the delegated CLI/CI review task.
**Base:** `18d74ce`; initial snapshot `6713cbf`, reviewed fixes through `8b9104e`.
**Files Changed:** 15 total; independent scope is eight CLI/CI/documentation/test-wiring files. Helper implementations and their dedicated tests were reviewed separately by the parent; the declaration change is covered by `docs/review-declarations.md`.
**Commits:** 6

### Summary

The CLI correctly enforces its reference version, compares independently evaluated semantic bytes, distinguishes every comparison disposition, and fails on disagreements and inconclusive outcomes. Review found a missing command-level exit-gate regression and failure-artifact metadata ordering; both were corrected and independently verified. The scoped implementation is simple, keeps process effects at the example boundary, adds no library dependency or unsafe code, and states the oracle's resource and compatibility limits accurately. The configured libxml2 2.15.4 build has not been executed locally or observed on hosted CI, so this review does not establish a pinned oracle pass.

The eight independently reviewed files are `.github/workflows/ci.yml`, `examples/fuzz_replay.rs`, `tests/oracle_cli.rs`, `tests/fixtures/fake_xmllint.rs`, `docs/oracle.md`, `fuzz/README.md`, `README.md`, and `docs/roadmap.md`; each was read in full. The supporting projection/process files, corpus wiring, and prior review report were read as context, without presenting review of the reviewer's own process helper as independent verification.

### Mutation Results

**Baseline:** green — initial snapshot passed 88 tests including one doctest; final snapshot passed 89 including one doctest. Six ignored process-helper entries are deliberately invoked as real subprocesses by their parent test. Formatting and strict all-target Clippy passed.

All predictions were recorded before execution. Mutations 1–3 ran `cargo test --all-targets` at `6713cbf`; mutations 4–5 ran `cargo test --test oracle_cli` at clean `8b9104e`. Every mutation was restored immediately, with empty `git status --porcelain` before the next mutation and after final verification.

| # | Mutation | Location | Predicted failures | Actual failures | Verdict |
| - | --- | --- | --- | --- | --- |
| 1 | Invert the pinned-version equality test | `examples/fuzz_replay.rs:217` | `tests::pinned_version_and_capabilities` | Exactly that test; the valid 21504 report was rejected | caught |
| 2 | Accept any available projection without checking stdout bytes | `examples/fuzz_replay.rs:262` | `tests::comparisons_and_summary_preserve_every_disposition` | Exactly that test; changed semantic bytes were classified as agreement | caught |
| 3 | Remove inconclusive results from main's failure gate | `examples/fuzz_replay.rs:51` | Existing helper tests would miss main's exit status | Entire all-target suite passed | survived initially; addressed by command regression below |
| 4 | Repeat mutation 3 after `6ccbb27` added the command regression | `examples/fuzz_replay.rs:51` | `replay_command_preserves_oracle_dispositions_and_exit_status` | Exactly that test; the inconclusive-mode process returned success with `inconclusive=1` | caught |
| 5 | Remove disagreements from main's failure gate | `examples/fuzz_replay.rs:51` | `replay_command_preserves_oracle_dispositions_and_exit_status` | Exactly that test; the semantic-disagreement process returned success with `disagreements=1` | caught |

The new regression builds and executes the actual example, with a separate safe Rust oracle fixture. It verifies agreement, rejection, semantic and acceptance disagreements, unexpected exit status, successful exit with diagnostics, exploratory override, strict version rejection, summary counts, and process exit status. It therefore exercises the production wiring that the initial helper-only tests missed.

Raw surviving/caught mutation logs are `/private/tmp/yaxml-oracle-mutation2.log` through `/private/tmp/yaxml-oracle-mutation5.log`. The final unmodified suite log is `/private/tmp/yaxml-oracle-restored.log`; mutation 1 and the initial baseline are recorded in the review tool transcript. Final `cargo test --all-targets`, `cargo test --doc`, `cargo fmt --all -- --check`, and `cargo clippy --all-targets -- -D warnings` passed. No mutation remains applied.

### Findings

#### Critical

None.

#### Important

None outstanding. Initial **I1**, at `examples/fuzz_replay.rs:51`, was a missing regression for main's inconclusive-result failure gate: mutation 3 survived. Commit `6ccbb27` added `tests/oracle_cli.rs` and its external fixture; mutations 4 and 5 independently verify the fix.

#### Minor

None outstanding. Initial **M1**, at `.github/workflows/ci.yml:90`, was failure-artifact metadata ordering: `set -e`/`pipefail` could exit before compiler, revision, and corpus hashes were written. Commit `8b9104e` moves those commands before the comparison pipeline; the artifact upload remains guarded by `if: always()`.

### Plan Adherence

| Requirement | Status | Notes |
| --- | --- | --- |
| Enforce the selected reference version and record its capabilities | Met | Strict 21504 matching plus Tree/XPath checks; exploratory override is explicit and labeled. Command-level regressions cover strict rejection and override. |
| Compare original-input semantics independently | Met | CLI supplies the generated XPath to the external executable and compares stdout directly with expected bytes; oracle values are not reparsed with yaxml. Semantic bypass mutation was caught. |
| Distinguish policy skips, acceptance-only results, agreement, disagreement, and inconclusive outcomes | Met | Classification and summary tests cover each disposition. Actual-command regressions protect the failure gates. |
| Bound subprocess work and avoid implicit document-controlled resources | Met within stated scope | Caller-selected trusted executable, finite process deadlines/output ceilings, catalog variables explicitly empty, defensive command flags, and conservative DTD policy skips. Disk polling overshoot, filesystem-operation deadlines, direct-child termination, and best-effort cleanup are documented limits. |
| Configure a reproducible pinned external-oracle CI build | Met as configuration | Official checksum matches the [GNOME checksum file](https://download.gnome.org/sources/libxml2/2.15/libxml2-2.15.4.sha256sum); options and the xmllint target match the [pinned CMake source](https://raw.githubusercontent.com/GNOME/libxml2/v2.15.4/CMakeLists.txt). Metadata precedes comparison and artifacts upload on failure. No actual 2.15.4 build/run was observed. |
| Preserve honest compatibility and execution claims | Met | The roadmap leaves pinned differential execution open; documentation does not turn configured CI, local exploratory results, deterministic smoke tests, or this review into full compatibility/security/performance evidence. |

Local release-CLI checks used `/opt/homebrew/Cellar/libxml2/2.15.3/bin/xmllint`. Without override, the CLI failed with the expected version error. With `--allow-other-libxml2`, corpus replay completed with six semantic agreements, two rejection agreements, one policy skip, zero acceptance-only results, zero inconclusive outcomes, and zero disagreements, matching `/private/tmp/yaxml-oracle-21503-comparison.txt`. These are exploratory 2.15.3 results, not evidence for the pinned 2.15.4 build.

Correctness, elegance, simplicity, design boundaries, idiomatic Rust usage, plan adherence, and test coverage have no remaining findings in this scope. The external executable and its C dependencies remain outside the safe Rust library boundary. Sustained coverage-guided fuzzing, complete execution coverage, complete libxml2 compatibility, and comparative performance remain separate open goals.

### Verdict

**PASS** — Zero outstanding issues in the scoped CLI/CI integration at `8b9104e`. Both review findings were fixed and verified; pinned 2.15.4 execution remains an explicit validation gate.
