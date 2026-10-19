## Holistic Review

**Plan:** [Normalization scan optimization](plans/normalization-scans.md)
**Base:** `f1b824ecf81cd4e4c62788ac56dfde0f53b74038` (provided)
**Files Changed:** 4
**Commits:** 3

### Summary

The code and tests at `7efc22fbe147d0d997b668bd21ac0bc0850d7280`
follow the implementation portion of the plan with no critical, important, or
minor findings. The change uses the existing safe `memchr` dependency, removes
the duplicate normalization scan, and preserves borrowing, resource accounting,
error precedence, XML semantics, and source offsets. This PASS is scoped to the
code and focused validation; the plan's fresh fuzz-coverage and retained
performance gates remain independently owned by root, and this review makes no
performance claim.

#### Seven-dimension assessment

**Correctness.** The four predicate branches retain these exact trigger sets:

| Attributes | References | Trigger bytes |
| --- | --- | --- |
| false | false | carriage return |
| false | true | carriage return or `&` |
| true | false | carriage return, line feed, or tab |
| true | true | carriage return, line feed, tab, or `&` |

These ASCII bytes cannot occur as continuation bytes inside a valid multibyte
UTF-8 scalar. Frontends still validate UTF-8 and XML characters before parser
state processing, and UTF-16 errors continue to map through the existing
decoded-to-source `TokenSpan`. `ParserState::normalized` returns the original
borrow when no trigger exists. Otherwise it charges the complete raw lexical
length before allocation or reference parsing and returns the transformed
`String` as `Cow::Owned`. The output cannot exceed the reserved raw length.

**Elegance.** The predicate names all four behavioral contexts directly.
`normalize_required` communicates its precondition and returns the only possible
owned representation, while the caller retains borrowing and budget policy.

**Simplicity.** The implementation adds no state, public API, dependency, unsafe
code, custom SIMD, or speculative abstraction. `normalize_required` has one
private caller, and the existing transformation loop remains intact.

**Design principles.** Detection is a pure function, parser state owns resource
accounting and the `Cow` decision, and transformation operates only after its
precondition is established. Security policy remains at the parser-state
boundary.

**Idiomatic usage.** The implementation uses safe `memchr` APIs, preserves
fallible `String::try_reserve`, and uses `Cow` only where both representations
remain observable. Borrowed and owned lifetimes remain explicit. Strict
all-target Clippy and formatting checks pass.

**Plan adherence.** The source change is limited to the measured private hot
path. XML-character validation, normalization results, error kinds and offsets,
public APIs, and resource limits are unchanged. Candidate A and the combined
candidate remain separate commits for evidence review.

**Test coverage.** A bounded property covers all flag combinations, every
sentinel, Unicode controls, and first/middle/last positions around scan
boundaries. Public tests pin literal values, `Cow` ownership, zero-budget
borrowing, raw-length quota boundaries, quota-before-reference errors, and
UTF-8 plus UTF-16LE/BE offsets across selected input splits. The tests describe
behavior rather than a particular `memchr` arrangement.

### Mutation Results

**Baseline:** green

The clean baseline passed 95 library tests, 4 normalization integration tests,
and 7 conformance tests. Each mutation was applied alone to a clean private
review worktree and immediately restored before the next mutation.

| # | Mutation | Location | Predicted failures | Actual failures | Verdict |
| - | --- | --- | --- | --- | --- |
| 1 | Removed `&` from text/reference detection | `src/escape.rs:13` | `normalization_predicate_matches_all_flag_contexts_and_scan_boundaries` fails for `(false, true, "&")` | Failed exactly at `(false, true, "&")` | caught |
| 2 | Returned `Cow::Owned` for unchanged values | `src/parser.rs:280` | `unaffected_contexts_borrow_and_changed_contexts_own_literal_values` rejects the first unchanged value | Failed on the unchanged `Attribute` | caught |
| 3 | Parsed and allocated before charging normalized raw bytes | `src/parser.rs:282` | `normalization_budget_precedes_reference_semantics` observes `UnknownEntity` where `OwnedBytes` must win | Observed `UnknownEntity` at byte 4 instead of `OwnedBytes` at byte 3 | caught |
| 4 | Removed tab from attribute/reference detection | `src/escape.rs:16` | Predicate property fails for `(true, true, "\\t")` | Failed exactly at `(true, true, "\\t")` | caught |

Mutation logs:

| # | Retained log | SHA-256 |
| - | --- | --- |
| 1 | `/private/tmp/yaxml-normalization-review-mutant-1-sentinel.log` | `e73531fb010e502c1e1a78ee571ccd79dcf1914ae3b5623c677f30f5d39d9c57` |
| 2 | `/private/tmp/yaxml-normalization-review-mutant-2-borrowed.log` | `ee9c73831806d5f3f38f52a45490e74ca2dcf085093cfee5ffdd6a15b3963840` |
| 3 | `/private/tmp/yaxml-normalization-review-mutant-3-order.log` | `7d3515db2c8e4209caf8291ff7b2e5a8a5e6e870e4fa7e7dd90f9af0b172a0db` |
| 4 | `/private/tmp/yaxml-normalization-review-mutant-4-attribute-tab.log` | `5460652f1b19cd7140c58cb99e81816ab8a998373d115d504ac4c901a192e9e1` |

Final restored-candidate validation:

| Check | Result | Retained log | SHA-256 |
| --- | --- | --- | --- |
| `cargo test --offline --locked --lib` | 95 passed | `/private/tmp/yaxml-normalization-review-final-lib.log` | `310293025f42e35f055a18a94804572cf76ce778c33382ef2f2c359fca3c24ed` |
| `cargo test --offline --locked --test normalization_scan` | 4 passed | `/private/tmp/yaxml-normalization-review-final-scan.log` | `cd0b47e379bafc8a514ebf50b7369972e5c0eb19f60b6078892ae8305c44cb78` |
| `cargo test --offline --locked --test conformance` | 7 passed | `/private/tmp/yaxml-normalization-review-final-conformance.log` | `d4c65e77bb315755b5739e5a8ba1508dc38661896e4583ab259879422440f37f` |
| `cargo clippy --offline --locked --all-targets -- -D warnings` | passed | `/private/tmp/yaxml-normalization-review-final-clippy.log` | `8c58e1addd8037e69271c5e1a4d42f519e81646740b44f88f6ccbb85b97b041e` |
| `cargo fmt --all -- --check` | passed | `/private/tmp/yaxml-normalization-review-final-fmt.log` | `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` |

The review worktree `/private/tmp/yaxml-text-normalization-review` is clean at
`7efc22fbe147d0d997b668bd21ac0bc0850d7280` after all restorations and final
validation.

### Findings

#### Critical

None.

#### Important

None.

#### Minor

None.

### Plan Adherence

| Requirement | Status | Notes |
| --- | --- | --- |
| Preserve all four detection contexts | Met | Literal trigger table and bounded property cover every branch. |
| Borrow unchanged values and own changed values | Met | Public `Cow` goldens and zero-budget borrowing pass. |
| Preserve XML normalization and reference semantics | Met | Literal attribute, text, CDATA, comment, PI, named-reference, and numeric-reference cases pass. |
| Charge raw length before allocation/reference parsing | Met | Exact inclusive boundaries and first-error assertions pass; ordering mutation was caught. |
| Preserve XML-character validation and UTF-8/UTF-16 source offsets | Met | Invalid-character precedence and hard-coded UTF-8/UTF-16LE/BE offsets pass across selected splits. |
| No unsafe code, new dependency, public API, or capability | Met | Changes are confined to the private normalization path; the existing `memchr` dependency is used. |
| Baseline regressions precede optimization validation | Met | Tests first passed on unchanged `f1b824e`; final candidate passes the same suite. |
| Ordinary focused tests, strict Clippy, and formatting | Met | All retained commands above are green. |
| Full pinned XMLTS catalog | Met (external) | Root reports all 12,925 surface rows byte-identical to baseline; artifact audit is outside this focused review. |
| Fresh library-only fuzz-smoke coverage | Partial (external) | Root owns this still-pending evidence gate; no production exclusion was reviewed here. |
| Paired retained performance evidence | Partial (external) | Root owns the A/B evidence and regression investigation; no timing result is asserted here. |

### Verdict

**PASS** -- Zero code or test issues found. The implementation is ready for the
remaining root-owned fuzz-coverage and retained performance evidence gates.

### Integration evidence review

After the focused review, root inspected the production/test diff, the complete
review report, retained logs, and independently audited benchmark/conformance
artifacts. The [validation report](validation-normalization-scans.md) closes
the remaining milestone gates: 493 ordinary tests, 11 doctests, strict Clippy
and formatting; complete byte-identical XMLTS results; fresh source-complete
fuzz-only coverage with no production exclusions; and 65 paired benchmark
workloads plus the 31-sample tiny repeat. Root corrected two mutation table line
references and the function-count wording when recording this report; no
production or test changes followed validation.

All milestone gates are met. The Rust A/B result establishes a long-text gain
with no repeated material regression in the measured matrix. Full libxml2
compatibility, superiority across all workloads, 100% fuzz execution coverage,
and production security assurance remain open goals.
