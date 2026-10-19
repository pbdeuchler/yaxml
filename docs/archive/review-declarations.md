## Holistic Review

**Plan:** none -- declaration intent reconstructed from commit `60cd974`, the parent review task, and the regression contract in `docs/conformance.md`.
**Base:** `252bc6a`; reviewed through integration commit `18d74ce`.
**Files Changed:** 8
**Commits:** 2

### Summary

The declaration change correctly accepts XML 1.0 forward-version spellings and validates encoding-name syntax before classifying unsupported encodings. The implementation uses simple safe byte predicates, adds no allocations or dependencies, preserves field order and source spelling, and leaves XML character restrictions intact. This review independently assesses the parent's production fix; the reviewer authored the new conformance tests, and the parent separately reviewed those tests. No standalone implementation plan existed, so intent-adherence confidence is limited to the explicit declaration contract.

Scope is the production and regression changes in `60cd974` and `18d74ce`; pending oracle CLI and CI edits are excluded. All eight changed files were read in full with surrounding parser context. The mutation worktree commits `78c6b66` and `51ab39b` have the same tree as integration commit `18d74ce` (`git diff 18d74ce 51ab39b --stat` is empty).

The grammar checks match [XML 1.0 Fifth Edition §2.8](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-prolog-dtd) and [§4.3.3](https://www.w3.org/TR/2008/REC-xml-20081126/#charencoding). Forward-version acceptance uses the existing XML 1.0 processing rules. Encoding names require an initial ASCII letter and the specified continuation characters; their syntax is checked before UTF-8 support is tested. Version digits are not converted to an integer, avoiding artificial numeric overflow restrictions. Traversal remains bounded by the existing declaration-token limit.

### Mutation Results

**Baseline:** green -- `cargo test --offline`; clean tree before mutation.

Every mutation ran `cargo test --offline --no-fail-fast`. Predictions were recorded before applying each mutation. Each mutation was restored immediately, and the tree was verified clean before the next mutation.

| # | Mutation | Location | Predicted failures | Actual failures | Verdict |
| - | --- | --- | --- | --- | --- |
| 1 | Replace the forward-version predicate with exact `1.0` equality | `src/parser.rs:656` | `declaration_grammar_and_forward_versions`, `declaration_values_preserve_source_spelling`, `grammar_regression_corpus` | Exactly those three tests; valid forward versions rejected | caught |
| 2 | Permit an ASCII digit as the initial encoding-name character | `src/parser.rs:668` | `declaration_grammar_and_forward_versions`, `grammar_regression_corpus` | Exactly those two tests; `8UTF` misclassified as `UnsupportedEncoding` | caught |
| 3 | Permit a space among encoding-name continuation characters | `src/parser.rs:671` | `declaration_grammar_and_forward_versions`, `grammar_regression_corpus` | Exactly those two tests; `UTF 8` misclassified as `UnsupportedEncoding` | caught |

Raw logs: `/private/tmp/yaxml-conformance-baseline.log`, `/private/tmp/yaxml-conformance-m1.log`, `/private/tmp/yaxml-conformance-m2.log`, `/private/tmp/yaxml-conformance-m3.log`, and `/private/tmp/yaxml-conformance-restored.log`.

After restoration, the full default test suite passed again (72 unit/integration tests and one doctest). Strict all-target Clippy and formatting checks passed. `git status --porcelain` remained empty. No mutation survives or remains applied.

### Findings

#### Critical

None.

#### Important

None.

#### Minor

None.

### Intent Adherence

| Requirement | Status | Notes |
| --- | --- | --- |
| Accept `1.` followed by one or more ASCII digits | Met | Forward, leading-zero minor, long minor, malformed, and non-ASCII-digit fixtures cover the predicate. |
| Continue applying XML 1.0 rules to forward versions | Met | A `1.1` declaration accepts ordinary XML and still rejects an XML 1.1-only control reference. |
| Separate malformed encoding names from unsupported encodings | Met | Empty, digit-leading, whitespace, punctuation, reference, and Unicode failures contrast with valid private unsupported names and mixed-case UTF-8. |
| Preserve declaration ordering and original values | Met | Existing ordering regressions remain; explicit Reader/DOM/one-byte Push goldens verify source version, encoding spelling, and standalone value. |
| Exercise supported public parser entry points | Met | Spec-derived acceptance/value fixtures cover Reader, DOM, and Push byte splits; fuzz seeds and regression corpus include the declaration changes. |
| Remain safe, pure Rust, and within dependency constraints | Met | Only safe string/byte predicates added; no dependency or allocator changes. |

Correctness, simplicity, idiomatic usage, and security checks found no issues in the scoped predicates. The change reuses the shared semantic parser, adds no state or I/O, and keeps error propagation explicit. It does not establish complete XML conformance, complete fuzz coverage, libxml2 equivalence, or a crate-wide security guarantee.

### Verdict

**PASS** -- Zero issues found in the scoped declaration change. Ready for integration review alongside the separately audited oracle work.
