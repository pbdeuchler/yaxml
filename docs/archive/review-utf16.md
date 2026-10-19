## Holistic Review

**Plan:** `docs/plans/utf16.md`
**Base:** `9a83078` (provided)
**Target:** `e7e2691e4a0d4bb0cae464f19ea936d7e9ab1638`
**Files Changed:** 44
**Commits:** 9

### Summary

The implementation follows the frozen UTF-16 plan: one bounded decoder owns transport detection, the existing grammar consumes decoded tokens, and the owned consumer shares the parser's aggregate budget without retaining a complete decoded input. Review included every changed file in full, public and internal tests, the independent fuzz reference, the external-oracle bridge, and seven deliberate production regressions. The review identified or independently verified one production error-classification defect, two meaningful fuzz assertion gaps, one oracle-cap classification defect, and two documentation inaccuracies; the production and oracle defects are repaired, and both fuzz gaps are caught after their repairs. No unresolved issue remains in the milestone scope; final restored checks pass.

### Mutation Results

**Baseline:** green. At final `e7e2691`, all-target tests passed 156 functions, with six intentionally ignored subprocess-helper entries launched by their parent test; three doctests also passed. The benchmark target's debug smoke workloads executed. Rust was `1.95.0 (59807616e 2026-04-14)`, host `aarch64-apple-darwin`, LLVM 22.1.2.

Predictions were written before each invocation in `/private/tmp/yaxml-utf16-audit-mutation-predictions.txt`. Each mutation changed one production behavior, was restored immediately, and left a clean tree before another mutation. Initial mutations used `cargo test --offline --lib --test utf16 --test fuzz_smoke --no-fail-fast`; final replays also included `--test encoding_errors`. All failures below were runtime assertions, not compilation failures.

| # | Mutation | Location | Predicted failures | Actual failures | Verdict |
| --- | --- | --- | --- | --- | --- |
| 1 | Count supplementary scalars as one UTF-16 unit when remapping semantic errors | `src/encoding.rs:69` | Internal and public raw-offset tests; malformed-input fuzz offsets | At `aaf4d54`, one private and one public test failed while all 18 fuzz tests survived. After the independent error-offset oracle repair, the same mutation at `9338656` also failed `encoded_equivalents_and_every_truncation` and `mutate_each_seed_byte`. | Caught; fuzz gap repaired |
| 2 | Accept every supported encoding label regardless of detected UTF-16 endian/BOM | `src/parser.rs:756` | Declaration agreement tests and encoded fuzz reference | One private declaration test, public `encoding_labels_and_declaration_grammar_are_independent_checks`, and fuzz `encoded_equivalents_and_every_truncation` failed, including final replay at `9338656`. | Caught |
| 3 | Replace a lone low surrogate with U+FFFD | `src/encoding.rs:216` | Malformed-unit tests and independently decoded fuzz inputs | One private and one public test plus three fuzz tests failed: encoded equivalents, corpus replay, and byte mutation. | Caught |
| 4 | Omit full newly requested owned-pool capacity charges while retaining growth clamping | `src/owned.rs:125` | Cumulative pool tests, public aggregate budget, independent fuzz quota goldens | Four private pool tests, public `owned_pool_normalization_and_streaming_retention_share_one_budget`, and fuzz `owned_storage_growth_and_aggregate_limits` failed. | Caught |
| 5 | Disable the UTF-16 decoded-total ceiling | `src/encoding.rs:221` | Exact decoded-byte boundaries and independently sized fuzz profiles | One private test, public `raw_and_decoded_byte_ceilings_are_distinct_and_inclusive`, and three fuzz tests failed. | Caught |
| 6 | Reintroduce UTF-8 validation of an incomplete non-ASCII InvalidMarkup prefix | `src/push.rs:241` | Both new public error-classification regressions and malformed-input fuzz reference | Both `encoding_errors` tests and two fuzz tests failed; private tests and the other ten public UTF-16 tests stayed green. | Caught |
| 7 | Give only the UTF-16 owned consumer a second budget, separate from parser normalization/retention | `src/owned.rs:196` | Public shared-budget golden; initially predicted fuzz-only survivor | At `9338656`, the public aggregate test failed but all 18 fuzz tests passed. After the conformance-authored aggregate oracle repair, the public test and fuzz `encoded_equivalents_and_every_truncation` both failed, including an independent replay on final `e7e2691`. | Caught; fuzz gap repaired |

Mutations 3–5 ran on `aaf4d54`; the corresponding production semantics are unchanged in the final candidate. Mutations 1, 2 and 6 were run on `9338656`. Mutation 7 was independently replayed on clean audit-only commit `2450ef1`, consisting of `fea6cf8` plus the author's staged `fuzz/encoding.rs` patch, SHA-256 `b0b1e4ade0ef31b135305e93b1e3b8e0a85cefe25d25b05e3928785ababd7f85`; this temporary commit is not an integration change. The final `e7e2691` replay also had an unmodified 67-test green baseline. Each replay was immediately restored; final all-target tests, doctests, formatting and strict Clippy passed on the clean final source.

### Findings

#### Critical

None open.

Resolved correctness finding: non-ASCII invalid markup could be recognized after only a UTF-8 leading byte, then incorrectly fail dispatch's UTF-8 validation (`src/push.rs:238`). Commit `aa1a6df` routes `InvalidMarkup` through the shared parser's immediate rejection without inspecting incomplete payload bytes. Grammar/event-budget precedence and original source mapping remain shared. Two public regression tests cover both byte orders, every split and terminal behavior; mutation 6 reintroduces and catches the defect in both public tests and fuzz smoke.

#### Important

None open.

Resolved findings are retained here so the review does not imply an issue-free history:

| Finding and location | Severity | Resolution and verification |
| --- | --- | --- |
| Malformed semantic-error offsets were only bounded, permitting a scalar-count remapping defect to survive fuzz checks: `fuzz/encoding.rs:135` | Important | `9338656` compares independently computed original-source offsets and error kinds for grammar failures, with explicit exclusions for whole-input character validation/resource precedence and enumerated incomplete-token differences. Mutation 1 now fails two fuzz tests, as well as existing ordinary tests. |
| UTF-16 owned-budget sharing could regress while all fuzz quota assertions remained true: `fuzz/encoding.rs:409` | Important | `e7e2691` adds independent exact minima 1/4/9/10 for four declaration-free tiny documents, checks immediately below and at each boundary, and restricts assertions to default non-owned limits. Deterministic driver calls exercise all four fixtures in both byte orders with BOMs. Mutation 7 fails the encoded fuzz test on the final clean candidate. |
| UTF-16 oracle projection storage exhaustion was classified as invalid XML: `examples/support/infoset.rs:76` | Important | `c2aff22` maps owned projection storage exhaustion to an explicit semantic-projection cap. A long-URI fixture establishes Push acceptance and OwnedBytes exhaustion, then verifies the cap classification instead of inventing agreement or rejecting the comparison run. The architecture reviewer independently reviewed the oracle bridge, including this repair. |

#### Minor

None open.

Two documentation inaccuracies were repaired in `9338656`: `docs/streaming.md:43` now states the correct UTF-16 scratch reservation bound, `min(decoded ceiling, 3 * floor(raw ceiling / 2))`; `docs/security.md:21` lists decoded totals and tokens as current enforcement. Preserved raw xmllint output has a trailing space on its compiled-feature line; this is evidence fidelity rather than source formatting.

The independent production review did not author the decoder, tokenizer, parser or owned-builder changes. The reviewer authored the earlier two-file oracle bridge, which received a separate architecture review.

### Plan Adherence

| Requirement | Status | Notes |
| --- | --- | --- |
| Default UTF-8/UTF-16LE/UTF-16BE byte input without changing borrowing Reader/Document | Met | Owned and streaming constructors use detection; borrowing APIs remain UTF-8-only. UTF-8 bulk forwarding and the owned slice fast path remain. No cross-library performance claim follows from preserving them. |
| One preamble decision; bounded incomplete code-unit/surrogate carry | Met | Decoder is the sole transport-BOM owner. Unsupported UTF-32 signatures, second BOM content, truncated units and malformed pairs have explicit tests and independent fuzz decoding. |
| Declaration grammar before source-label agreement | Met | Complete grammar validation precedes agreement. BOMless endian labels, generic UTF-16/BOM requirements, unknown labels and mismatches are covered; mutation 2 verifies sensitivity. |
| Raw and decoded total/token ceilings checked before excess retention | Met | Decoder accounts decoded totals; tokenizer accounts both coordinates and bounds fallible scratch reservation. Text lookahead is excluded from the preceding token while public progress includes supplied carry/lookahead. Mutation 5 verifies decoded-total enforcement. |
| Original-byte coordinates and exactly-once semantic remapping | Met | TokenSpan carries both ranges; remapping counts UTF-16 code units in the unnormalized token prefix. Decoder and owned-consumer errors already use source coordinates. Mutation 1 verifies supplementary-scalar handling. |
| Fallible owned consumer using one parser/storage budget | Met | Concrete OwnedBuilder consumes events and raw ranges, with the parser Budget passed directly. Full replacement pool capacities remain charged, prior capacities remain spent, and no complete decoded input is retained. Mutations 4 and 7 verify distinct charging/sharing behaviors. |
| Sticky errors, cancellation, panic poisoning and terminal calls | Met | Existing lifecycle is preserved around decoder and consumer errors, with public split/schedule tests and fuzz callback/I/O checks. |
| Independent fuzz validity, values, topology, coordinates and policy oracles | Met | Reference decoding uses standard-library UTF-16 iteration and a harness-only offset map. Typed events and owned views are compared in lockstep. Error and aggregate-budget gaps found by mutation were repaired explicitly. |
| Original encoded input reaches the external oracle | Met | UTF-16 projection uses owned borrowed views; original bytes go to xmllint. Explicit projection caps remain inconclusive semantic comparisons, not agreements. |
| Safe Rust and memchr-only dependency | Met | No unsafe or dependency changes. Input-sized growth uses fallible reservation; global allocator-failure injection remains a documented future gate. |
| All targets, doctests, fmt, strict Clippy and restored clean mutations | Met on reviewed snapshot | 156 tests and three doctests passed on final `e7e2691` after the final mutation was restored. |
| Coverage evidence and honest milestone boundary | Met for published 9338656 evidence | Raw summaries independently checked; current coverage is below 100%, pinned libxml2 2.15.4 was not run, and sustained fuzzing, allocator work, broader encodings and fair comparative performance remain open. The new aggregate guard is covered by executed assertions and mutation replay here; a refreshed raw fuzz-coverage artifact remains parent-owned evidence. |

The seven review dimensions were evaluated explicitly:

| Dimension | Assessment |
| --- | --- |
| Correctness | Transport detection, malformed units, declaration agreement, dual ranges, limits and lifecycle have direct tests; the discovered dispatch defect is repaired. |
| Elegance | The decoder feeds the existing grammar rather than duplicating XML semantics. The InvalidMarkup fix is narrow and retains shared error precedence. |
| Simplicity | Constant decoder carry, token-local mapping and one concrete builder avoid a full decoded source, global map or speculative generic transport layer. |
| Design principles | Encoding, lexical state, grammar and owned storage have separate responsibilities; mutable aggregate accounting is passed explicitly through the private consumer. |
| Idiomatic Rust | Byte ranges and owned flat storage avoid self-reference. Checked source acceptance bounds coordinate arithmetic, fallible reservations handle input-sized growth, and no unsafe escape hatch was added. |
| Plan adherence | The frozen milestone is implemented without quietly broadening supported encodings or narrowing the eventual replacement goal. |
| Test coverage | Ordinary and fuzz assertions killed seven crucial semantic regressions. The two fuzz-only survivors were treated as Important gaps and repaired rather than explained away. |

The `9338656` raw coverage sums were independently checked against the ten production source files: fuzz regions 3,541/3,716, functions 227/247, lines 2,324/2,378, branch outcomes 505/530; ordinary regions 3,501/3,716, functions 227/247, lines 2,321/2,378, branch outcomes 497/530. The encoding module's fuzz region count is 279/281. These are execution measurements with compiler-instrumentation limits, not a security proof or 100% coverage claim. The separately recorded exploratory Homebrew libxml2 2.15.3 run reports ten semantic agreements, three rejection agreements, one unsupported-DTD skip and no disagreements; the pinned 2.15.4 target was not exercised.

### Verdict

**PASS** — Zero unresolved Critical, Important or Minor issues. The implementation is ready within the UTF-16 input milestone scope; the resolved findings above remain part of the review record.

Final restored commands all exited zero: `cargo test --offline --all-targets`, `cargo test --offline --doc`, `cargo fmt --all -- --check`, and `cargo clippy --offline --all-targets -- -D warnings`. The production/harness diff passed whitespace checks. `git status --porcelain` was empty before writing this report; only the report is added by the review commit.

Raw evidence:

- Toolchain: `/private/tmp/yaxml-utf16-audit-toolchain.txt`.
- Initial baselines: `/private/tmp/yaxml-utf16-audit-baseline-tests.txt`, `/private/tmp/yaxml-utf16-audit-baseline-docs.txt`.
- Production baseline: `/private/tmp/yaxml-utf16-audit-9338656-baseline.txt`; final target baseline: `/private/tmp/yaxml-utf16-audit-e7e2691-baseline.txt`.
- Predictions: `/private/tmp/yaxml-utf16-audit-mutation-predictions.txt`.
- Mutation logs: `/private/tmp/yaxml-utf16-audit-mutation1-final.txt`, `-mutation2-final.txt`, `-mutation3.txt`, `-mutation4.txt`, `-mutation5.txt`, `-mutation6-final.txt`, `-mutation7.txt`, `-mutation7-patch.txt`, and `-mutation7-final.txt` under the same prefix.
- Final-target restored checks: `/private/tmp/yaxml-utf16-audit-e7e2691-final-tests.txt`, `-e7e2691-final-docs.txt`, `-e7e2691-final-fmt.txt`, `-e7e2691-final-clippy.txt` under the same prefix.
- Staged-guard clean baseline and restored checks: `/private/tmp/yaxml-utf16-audit-budget-baseline.txt`, `-budget-restored-tests.txt`, `-budget-restored-docs.txt`, `-budget-restored-fmt.txt`, `-budget-restored-clippy.txt` under the same prefix.
- Clean final audit worktree: `/private/tmp/yaxml-utf16-audit`; clean staged-guard audit worktree: `/private/tmp/yaxml-utf16-budget-audit`.
