# Bounded owned-value mutation validation

This report covers existing-value edits and checked owned handles. It is a
working evidence record, not a structural-mutation or production-readiness
claim. Production, fuzz and measurement snapshot: `688c3cf`. The final restored
verification after documentation and independent review remains pending.

## Implemented contract

`OwnedNodeId` has private nonzero document identity and slot fields. Construction
assigns identity after XML processing succeeds; identities never recycle or
wrap. Exhaustion returns `IdentifierExhausted` at offset zero. Moving a document
preserves its handles; foreign lookups/edits fail. Parsed `range()` values are
`Some(original_range)`, persisting as source provenance after edits.

`OwnedDocument::edit(EditLimits)` returns an exclusive `Editor` with
`document()`, `set_node_value` and `set_attribute_value`. It replaces existing
Text/CDATA/Comment/PI data or an ordinary attribute selected by expanded name.
It preserves names, namespace declarations, attribute order, topology, identity
and declaration. Input strings are literal values. Validation precedes storage;
one range is published only after append succeeds. Errors preserve logical
state and the editor remains usable. The [owned contract](owned-dom.md) records
lexical restrictions, error order and value byte indices.

Defaults are 1 MiB per replacement and 64 MiB cumulative edit-capacity requests.
Parsed capacity is the baseline; new full-capacity requests remain charged
across editor sessions, including failed reservations. Equal values still take
the append path. This is not a resident-memory quota or reclamation mechanism.

## Tests and fuzz evidence

The production tranche passed its 59 library tests, eight doctests and strict
Clippy. Seven focused edit tests include full-capacity boundaries 2/3/4 and
reopened-editor boundaries 6/7/8, error precedence, UTF-8 indices, atomic
publication, recovery, source provenance and literal output. Identity tests use
local counters for deterministic exhaustion and concurrency; they never reset
the shared production counter.

Twelve [public mutation tests](../tests/mutation.rs) cover every value kind,
UTF-8/UTF-16LE/BE construction, namespace selection/shadowing/reset, empty values,
literal references, limits, baseline spare capacity, shared node/attribute
accounting, moves and source destruction. Byte goldens and complete snapshots
check state after success and rejection. Depth 8,192 succeeds on a 256 KiB
stack; a 10,000-sibling case preserves order. The integrated snapshot passed
225 all-target tests with six ignored subprocess helpers, eight doctests,
formatting, strict Clippy and the dependency/unsafe checks. The combined run
completed in the integration terminal without a retained standalone log.

Draft-document verification on the `688c3cf` production snapshot passed all 12
public mutation tests, the three mutation-filtered fuzz functions, eight crate
doctests and three Markdown-guide doctests. Local logs are
`/private/tmp/yaxml-mutation-docs-public-tests.log`,
`/private/tmp/yaxml-mutation-docs-fuzz-tests.log`,
`/private/tmp/yaxml-mutation-docs-doctests.log` and
`/private/tmp/yaxml-mutation-docs-guide-tests.log`. These focused runs do not
replace the pending final integrated suite.

The [operation target](../fuzz/mutation.rs), integrated at `688c3cf`, uses two
independent logical models and up to 31 eight-byte operations with bounded
payload. It predicts results before production, checks complete state after
each operation, and exercises saved/foreign handles, moves, reopened editors,
error recovery and serialization. Mandatory small valid operations prevent
blanket rejection. Literal quotas cover both setters, UTF-16-created documents,
baseline spare capacity, equal replacements and cumulative charging without
replicating production pool growth. Three new mutation smoke functions bring
the fuzz suite to 23 functions. This is deterministic replay, not a sustained
coverage-guided campaign or an external modeled mutation oracle.

## Coverage and independent review

Both runs use default features, debug optimization and
`-C instrument-coverage -Z coverage-options=branch`, with nightly `2026-06-06`:
rustc 1.98.0-nightly (`8954863c8`, dated 2026-06-05), LLVM 22.1.6,
`aarch64-apple-darwin`. The reporter is Apple LLVM 21.0.0. Reports exclude
dependency/toolchain/test/fuzz/example/benchmark sources and no production
source. Build-specific denominators are reported separately rather than
combined or treated as identical.

| Run | Regions | Functions | Lines | Instrumented branch outcomes |
| --- | --- | --- | --- | --- |
| Fuzz smoke only | 4,375 / 4,586 (95.40%) | 289 / 313 (92.33%) | 2,909 / 2,979 (97.65%) | 611 / 646 (94.58%) |
| Ordinary tests only | 4,464 / 4,659 (95.81%) | 298 / 319 (93.42%) | 2,983 / 3,039 (98.16%) | 625 / 652 (95.86%) |

`owned.rs`, including parsing, views and the editor, reaches 696 / 733 regions,
61 / 68 functions, 466 / 475 lines and 36 / 38 branch outcomes under fuzz smoke.
Ordinary tests reach 706 / 733 regions, 63 / 68 functions, 469 / 475 lines and
all 38 branch outcomes. These are below the project's 100% execution target.

Checked-in summaries: [fuzz coverage](validation/fuzz-coverage-688c3cf.txt) and
[ordinary coverage](validation/test-coverage-688c3cf.txt). Raw directories
`/private/tmp/yaxml-coverage-mutation-fuzz-688c3cf` and
`/private/tmp/yaxml-coverage-mutation-tests-688c3cf` retain compiler/reporter
versions, production revision and SHA-256 source manifests, profiles and exports.

**PENDING: independent holistic review verdict, caught mutation list and report
link.** No final review pass or 100% execution claim is made here.

## Benchmark evidence

The [mutation runner](../benches/mutation.rs) separates fresh parse/handle setup,
editing, final state/serialization/reparse/checksum validation, and destruction.
It covers repeated/distributed leaf edits, attribute lookup position,
growth/shrink, depth/width and three input encodings. Its debug smoke passed 72
rows; selected empty-payload and low-value-quota runs passed and unknown
selectors were rejected.

`cargo bench --bench mutation --offline --locked` completed all 72 default
release rows at `688c3cf`, with five fresh documents and 10,000 requested edits
per document. Observed rates across different workloads ranged from
1,680,248.68 to 25,146,149.42 completed operations/s. This was one runner
execution, not repeated independent trials, a statistical baseline or a
comparison with libxml2. Encoding conversion occurs before the edit clock.

The host was Darwin arm64 T6000, kernel 27.0.0, target
`aarch64-apple-darwin`; release rustc 1.95.0 (`59807616e`, 2026-04-14),
LLVM 22.1.2. Raw log: `/private/tmp/yaxml-mutation-benchmark-release.txt`;
SHA-256 `ee88369ff0baedfae941eaf5e63889ac94471425b3b1968e55c6b6722a4c736c`.
The checksum, row count, complete statuses and reported range were independently
reproduced. See the [benchmark guide](benchmarks.md) for phase boundaries and
remaining finite validation ceilings.

## Remaining gates

- Complete the independent mutation review and retain the final restored
  verification results after documentation/review integration.
- Add deterministic allocation-failure injection and investigate uncovered code.
- Run sustained feedback-guided fuzzing and a matched libxml2 mutation benchmark.
- Establish an external modeled mutation oracle if one is added; the existing
  source/serialization oracle is not evidence for edited-model operations.
- Design structural mutation, deletion/reuse, name/namespace/declaration edits,
  cross-document ownership, reclamation and future binding registries separately.
