## Holistic Review

**Plan:** none -- intent reconstructed from commits and the streaming contract in `docs/streaming.md`
**Base:** `41ef2dd` (provided); reviewed implementation snapshot `d7c6f57` plus documentation-only correction `489ed91`
**Files Changed:** 21
**Commits:** 6

### Summary

This review assessed bounded incremental UTF-8 XML parsing, shared slice/push semantics, chunk-independent events and spans, terminal cancellation and panic behavior, and the explicit `Read` boundary. The implementation keeps lexical scanning separate from semantic state without introducing dependencies or unsafe code. No production-code correctness, resource-retention, or maintainability findings were identified; a stale future-work sentence was corrected in the independently inspected documentation-only follow-up. There was no standalone implementation plan, so intent-adherence confidence is limited to the documented contract and commit scope.

### Mutation Results

**Baseline:** green -- `cargo test --all-targets --offline` passed 68 tests; the README doctest separately passed (69 total). Strict all-target clippy and formatting also passed.

| # | Mutation | Location | Predicted failures | Actual failures | Verdict |
| - | -------- | -------- | ------------------ | --------------- | ------- |
| 1 | Search for `>` rather than the active quote when bulk-scanning a quoted tag value | `src/tokenizer.rs:133` | Lexer chunk/span and long-run tests; streaming split parity | Both lexer tests; 6 streaming tests; I/O logical-event test; 8 fuzz smoke tests | caught |
| 2 | Restore a removed namespace binding slot instead of its ancestor index | `src/parser.rs:530` | Namespace shadow/unwind tests and streaming parity | Interner alias/shadow test; 2 reader tests; 2 streaming tests; 3 fuzz smoke tests | caught |
| 3 | Remove the preexisting terminal-error check before `Read` I/O | `src/push.rs:72` | `terminal_parsers_do_not_touch_new_readers` | Predicted test panicked on forbidden I/O | caught |
| 4 | Omit the in-flight poison guard before feed processing | `src/push.rs:131` | `recovered_callback_panics_poison_the_parser` | Predicted test failed after recovered feed callback panic; ordinary feed test still passed | caught |

An initial attempted mutation broadening the scalar tag-close match from `(None, b'>')` to `(_, b'>')` was behaviorally ineffective: the quoted bulk scanner only dispatches the active quote to that match. It was restored and replaced by mutation 1 above, which actually breaks the active delimiter contract. It is not counted as a semantic mutation or a surviving test gap.

Every mutation ran the complete all-target suite with `--no-fail-fast`, was immediately restored, and was followed by an empty `git status --porcelain`. The final restored all-target suite is green. Logs are `/private/tmp/yaxml-stream-review-m1b.log`, `-m2.log`, `-m3.log`, `-m4.log`, and `-restored.log`; predictions were written before execution to `/private/tmp/yaxml-stream-review-mutations.txt`. All mutations occurred only in detached `/private/tmp/yaxml-stream-review`.

### Findings

#### Critical

None.

#### Important

None.

#### Minor

None. The initial M1 at `docs/design.md:260` listed “true incremental input” as future work. Commit `489ed91` now states that callback push input and the explicit `Read` adapter are delivered and identifies buffered pull input as pending. Its complete diff was inspected; it changes no executable code.

### Intent Adherence

| Requirement | Status | Notes |
| ----------- | ------ | ----- |
| Core forbids unsafe and retains only memchr as dependency | Met | No dependency additions or unsafe source; strict clippy passes |
| Shared names, normalization, declarations, namespaces, phases and budgets | Met | Concrete `ParserState` with a narrow source-retention callback; borrowed slice behavior remains tested |
| Arbitrary split points with stable successful events and spans | Met | Every split and one-byte tests, generated schedules, all prefixes, typed field comparison, and independent normalization goldens |
| Bounded streaming retention without storing complete input | Met | One bounded lexical token, fixed preamble, open-element names and scoped namespace state; completed sibling interner entries released; capacities retain high-water marks |
| Need-more-input distinct from final EOF and terminal failures | Met | Explicit feed/finish, sticky errors/cancellation, idempotent successful finish, invalid feed-after-finish, recovered panic poisoning |
| Explicit I/O boundary preserves errors and stops after failure | Met | Interrupted retry, original I/O errors, invalid Read byte counts, cancellation and preexisting failure checks |
| Fuzz entry points and target inventory grow with public input APIs | Met | Push schedules and Read fault paths added; incomplete execution coverage and sustained campaigns remain expressly open |
| Full libxml2 compatibility, 100% fuzz execution coverage, allocator delivery, universal performance win | Partial | Larger goal items remain outside this streaming implementation; no unsupported completion claim is made |
| Design narrative consistently distinguishes delivered push from pending pull | Met | M1 resolved by independently inspected documentation-only commit `489ed91` |

### Verdict

**PASS** -- Zero open issues in the reviewed streaming implementation and its documentation correction. This verdict does not cover later fuzz-harness changes, establish complete XML/libxml2 compatibility, prove absence of security defects, or close the larger allocator, coverage, and performance goals.
