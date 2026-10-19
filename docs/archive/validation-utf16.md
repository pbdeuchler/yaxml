# UTF-16 input milestone validation

This milestone adds UTF-16LE/BE input to push parsing and owned documents. It
does not complete the libxml2 replacement goal. The production source measured
at clean `9338656765df486b36608b9f640b720f7f457bed` is byte-identical to final
test and fuzz snapshot `e7e2691e4a0d4bb0cae464f19ea936d7e9ab1638`.
The [encoding contract](../encoding.md) and [frozen plan](plans/utf16.md) define the
supported subset and deliberate compatibility differences.

## Implementation and functional evidence

One private decoder owns transport detection, rejects UTF-32 signatures, checks
surrogates and carries only bounded partial state. UTF-8 still uses borrowed
bulk input; UTF-16 emits stack-buffered scalars. Tokens carry separate decoded
and source coordinates. Semantic failures are mapped once before callbacks or
the owned builder, whose errors already use source offsets. The library retains
neither a complete decoded source nor a document-wide offset map.

`DecodeLimits` adds independent decoded-total and decoded-token ceilings without
changing the six existing `Limits` fields. Source/token limits still count raw
bytes. The UTF-8 owned fast path enforces the new limits too. A concrete shared
builder charges pool growth, normalization and retained streaming names against
one allocation-work budget. Borrowing APIs remain UTF-8-only, and output remains
UTF-8-only.

On `aarch64-apple-darwin`, stable Rust 1.95.0:

- `cargo test --offline --all-targets`: 156 test functions passed, plus the
  benchmark target's debug smoke workloads. Six ignored subprocess helper
  entries are deliberately invoked by their parent test.
- `cargo test --offline --doc`: three README examples passed.
- Formatting, strict all-target Clippy, diff checks and dependency inspection
  passed. `memchr` 2.8.3 remains the only dependency; unsafe code stays forbidden.

Ten public UTF-16 test functions cover both byte orders, every split, declaration
spelling/agreement, all eight UTF-32 signatures, repeated BOMs, surrogate and odd
EOF failures, Unicode before semantic failures, original ranges, normalization,
source/decoded/aggregate budget boundaries, cancellation, panic poisoning,
Interrupted I/O, source-independent owned views and preserved UTF-8/writer
contracts. Two additional tests exercise malformed non-ASCII markup prefixes
and event-limit precedence. Private tests cover decoder carry, scratch capacity,
mapping and fallible-consumer propagation.

Review found and corrected three implementation/integration problems: scratch
capacity could ignore a small UTF-8 source-token ceiling; UTF-16 projection pool
exhaustion needed an explicit acceptance-only ceiling; malformed markup could
terminate a token inside a decoded scalar and incorrectly report InvalidUtf8.
The last now uses the shared syntax/event-budget guard before scalar validation.
All have targeted regressions.

## Fuzz oracle and mutation sensitivity

`fuzz/encoding.rs` independently decodes with `char::decode_utf16`. Only the
bounded harness materializes a reference source and offset map. A separately
checked matching declaration label is rewritten to UTF-8 for grammar validation;
original declaration fields, semantic values and raw ranges are compared against
whole/chunked/I/O push parsing and owned trees. Malformed grammar checks source
offsets, with explicit exclusions for upfront character validation, resource
ordering and two incomplete-token precedence differences.

Review found three oracle gaps. Raw-byte-selected decoded quotas often failed
before useful parsing, so valid inputs now receive exact/below/above profiles
for each independent ceiling and must succeed when within budget. Malformed
semantic offsets originally had only bounds checks; they now have independent
source-coordinate comparisons. The harness also lacked an independent boundary
for the one aggregate budget shared by parsing and owned-tree construction. Four
small canonical trees now supply exact allocation-work goldens in both UTF-16
byte orders. Small valid inputs cannot hide blanket rejection behind the
owned-budget exception.

An isolated zero-offset mutation initially survived all 18 fuzz functions while
the public Unicode-error test failed. After strengthening the oracle, the same
mutation fails three fuzz functions. Three other mutations verify ignored total
and token ceilings and blanket rejection by explicit decode constructors. A
mutation giving the UTF-16 owned builder a fresh budget originally survived fuzz
smoke while the public aggregate-budget test caught it; the new exact boundary
makes `encoded_equivalents_and_every_truncation` fail too. Each mutation was
restored, and the combined 30 focused tests and strict Clippy passed.
The independent [holistic review](review-utf16.md) records additional production
mutations and the final restored verdict.

## Source coverage

Both runs use default features, debug optimization, nightly `2026-06-06`
(Rust 1.98.0-nightly / LLVM 22.1.6) and
`-C instrument-coverage -Z coverage-options=branch`. Apple LLVM 21.0.0 successfully
reads the profiles; matching local LLVM tools remain unavailable. The
generic-instantiation summary behavior and instrumented-branch denominator limits
documented in [the streaming report](validation-streaming.md) still apply.

| Run | Regions | Functions | Lines | Instrumented branch outcomes |
| --- | --- | --- | --- | --- |
| Fuzz smoke only | 3,541 / 3,716 (95.29%) | 227 / 247 (91.90%) | 2,324 / 2,378 (97.73%) | 505 / 530 (95.28%) |
| Ordinary parser/DOM/writer tests only | 3,501 / 3,716 (94.21%) | 227 / 247 (91.90%) | 2,321 / 2,378 (97.60%) | 497 / 530 (93.77%) |

The new encoding module alone reaches 279 / 281 regions (99.29%), 17 / 17
functions, 205 / 205 lines and 64 / 64 instrumented branch outcomes under fuzz
smoke. This is not 100% region coverage, complete behavior coverage, or proof
of security. Whole-library gaps include allocation-error closures, internal
invariant/overflow guards and unexercised I/O outcomes; they remain in the
denominator.

The checked-in [fuzz summary](validation/fuzz-coverage-e7e2691.txt) and
[ordinary summary](validation/test-coverage-9338656.txt) exclude only dependency,
toolchain and test/fuzz/example files. No production regions are excluded.
Ignored local `coverage/fuzz-e7e2691/` and `coverage/tests-9338656/` retain raw and
merged profiles, full JSON exports, instrumented binaries, compiler/tool versions,
source/corpus/harness and binary hashes, build/test logs and reproduction scripts.
The fuzz reporting command needed an empty-array fix for macOS Bash after tests
and profile merging; reporting was completed from those same profiles without
rerunning or discarding executions.

Fuzz smoke ran 18 functions in 12.54 seconds with zero unexpected failures.
Individual testcase execution counts remain unrecorded. The separate ordinary
coverage run executes 118 library/parser/DOM/streaming/I/O/writer functions and
excludes fuzz smoke, oracle tools, examples and benchmarks. Neither run is a
sustained coverage-guided campaign.

## External comparison and remaining gates

The [exploratory corpus log](validation/oracle-21503-utf16.txt) uses installed
Homebrew libxml2 2.15.3: ten semantic agreements, three rejection agreements,
one unsupported-DTD skip, no inconclusive results or disagreements. Four semantic
fixtures exercise UTF-16 in both orders with BOMs and BOM-less endian declarations;
a malformed low surrogate participates in rejection comparison. Original encoded
bytes go to libxml2; its values are never reparsed by yaxml.

An earlier encoding fixture used a non-ASCII namespace URI. Strict namespace
checking in local 2.15.3 rejected it while yaxml accepted it. This separate
[URI-validation difference](../compatibility.md) is recorded, and encoding fixtures
now use ASCII URIs. A minimized fixture and raw diagnostic/exit status remain in
ignored `coverage/utf16-oracle-probes/`. Declared-encoding mismatches are explicit
policy skips because yaxml's strict agreement differs from libxml2's warning
path; they never count as compatibility agreements.

No pinned 2.15.4 binary pass is claimed. CI is configured but its hosted execution
has not been observed. UTF-16 output, additional encodings, DTDs, complete
conformance suites, allocator support on the requested toolchain, allocation-fault
injection, sustained fuzzing, 100% execution coverage and matched libxml2 DOM/
streaming performance remain open. This milestone adds no performance claim.
