# Construction fuzz-gap validation

The final executable checkpoint is
`d6848862c3b77e9ea52adfcef25844d13f8af413`, based on local main
`51e0aea6ed5e56eb4454139072474147fa71178b`. Production source, `Cargo.toml`
and `Cargo.lock` are byte-identical to that base. Both revisions have source
tree object `34f9acb22ae65b99fcea2cc1cc52e9ee6e036b3f`. Subsequent commits
add review and validation documents only.

## Behavior under test

Actual replay previously entered construction only when the first byte's low
nibble was `0x0c`. That gate prevented the existing `0x2a` formatting and
`0x2b` long-prefix paths from running through the real driver. Construction now
runs once for every bounded testcase before grammar and encoding dispatch.
A test-only per-thread counter verifies empty, malformed, sparse-selector,
UTF-8 and UTF-16 inputs. It introduces no production seam.

Three new independent child models extend the earlier construction oracle:

- Validation: 42 literal invalid/control pairs through both creation and
  append, with exact kind, item, component and UTF-8 byte offset. Append
  rejections preserve full snapshots and accept a valid same-editor retry.
- Limits: hand-counted below/exact/above boundaries for namespace work,
  generated name/item counts, explicit items, depth, persistent creation
  quotas, component bytes and aggregate supplied bytes. Removal does not refund
  persistent quotas. Generated-item limits must precede later namespace errors.
- Namespaces: 28 literal cases, including implicit `xml`, default reset/reuse,
  shadowing, local/generated conflicts, automatic fallback, later exact-prefix
  reservation, duplicate expanded names and aliases. All 23 append cases run
  in UTF-8, UTF-16LE and UTF-16BE; five create cases are encoding-independent.

Expectations precede API calls and do not invoke the production planner.
Successful append checks anchor the expected topology to the returned handle:
exact preorder, parent/child/sibling links, ancestor descendant lists, unrelated
old nodes and source provenance. Full serialization/reparse adds a separate
check. Parsed fixture bytes are overwritten before editing; the shared
parsed-parent helper also drops its buffer.
Three new corpus inputs exercise selector combinations, formatting and the
4,336-byte inherited prefix through actual replay. The corpus has 22 files.

## Regression and fault checks

At the final checkpoint, all-target checks passed **456 tests**, with seven
ignored timing tests, and all five custom benchmark smoke executables completed.
All **11 doctests**, strict all-target Clippy, formatting and diff whitespace
checks passed. The fuzz-smoke suite contains **59 tests**.

The [holistic review](review-construction-fuzz-gaps.md) records independent
review of every changed line and the final supplement, with no remaining
findings. Separately injected and restored faults were detected for dispatch
skipping, reserved XMLNS acceptance, namespace-work undercharging, persistent
quota publication, generated-item first-error precedence and missing subtree-size
publication. Negative oracle tests reject changed expanded names and shifted
component byte offsets.
These checks establish sensitivity to those faults, not all possible faults.

Root logs are `/private/tmp/yaxml-construction-fuzz-{all-targets,doctests,clippy,fmt}-d684886.txt`.
The three new seeds also passed local replay with pinned libxml2 2.15.4
(`21504`): two rejection agreements, one explicit unsupported-feature/encoding/
policy skip, zero disagreements and zero inconclusive results. The skipped
seed supplies construction controls; it is not XML conformance evidence.
The log is `/private/tmp/yaxml-construction-fuzz-seeds-oracle-d684886.txt`.

## Fresh fuzz-only coverage

Only the 59-test fuzz-smoke binary ran under the profiler; it passed in
102.47 seconds. This uses nightly `2026-06-06`, rustc 1.98.0-nightly
(`8954863c8`), matching LLVM **22.1.6**, default features, debug optimization
and `-C instrument-coverage -Z coverage-options=branch` on
`aarch64-apple-darwin`. Its target and profile directories are fresh.

All **15 recursive production Rust sources** were hashed and passed explicitly
to LLVM. Fourteen executable module rows include `owned/construction.rs`;
`lib.rs` has no executable mapping and remains in the source identity manifest.
No production code is excluded. Dependencies, harnesses, examples and tests are
outside this library-only denominator. Before/after source and harness
manifests match, the worktree was clean, and merge/report/export diagnostics
are empty. Ordinary failpoint tests do not count as fuzz execution.
An independent audit matched all 51 source/harness hash records to committed
blobs, checked the complete recursive source and flat fuzz inventories, and
remerged only the one fresh raw profile. Indexed profile, report and JSON export
reproduce byte-for-byte. Its full commands, hashes and reachability notes are
retained in the evidence directory's `independent-audit/final-audit.txt`.

| Metric | Reached / total | Coverage | Gap |
| --- | ---: | ---: | ---: |
| Regions | 6,810 / 7,260 | 93.80% | 450 |
| Functions | 386 / 436 | 88.53% | 50 |
| Lines | 4,712 / 4,883 | 96.50% | 171 |
| Instrumented branch outcomes | 919 / 970 | 94.74% | 51 |

Compared with the [original milestone](validation-owned-create.md), the same
production denominator gains 326 regions, five functions, 217 lines and 80
branch outcomes. Construction reaches 1,922/2,149 regions, 68/93 functions,
1,406/1,501 lines and 257/274 branch outcomes. Its gaps remain 227 regions,
25 functions, 95 lines and 17 branch outcomes. These instrumentation metrics
do not measure semantic completeness or the full path space.

The [per-module report](validation/fuzz-coverage-construction-d684886.txt) has
SHA-256 `09256087b3c212217be2d6fd517206ada957103e3d14d47c5aefc4767d486bb7`.
Evidence is `/private/tmp/yaxml-construction-fuzz-coverage-d684886`, with
source/harness/compiler/tool identities, raw profile, indexed profile, JSON
export and artifact hashes. The exact script is
`/private/tmp/yaxml-construction-fuzz-coverage-complete.sh`.

## Separate feedback campaign

A fresh optimized no-LTO campaign ran **500 cases**, comprising 22 seeds and
478 mutations, stopping at the case limit after 37 seconds without target or
infrastructure failure. It retained 86 inputs, recorded 414 duplicate cases
and observed 85 novelty transitions. Its cumulative profile reaches
6,632/7,260 regions, 384/436 functions, 4,640/4,883 lines and 840/970 branch
outcomes. The campaign and deterministic fuzz-smoke suite select different
operation combinations and are reported separately.

An independent offline replay reproduced all 500 PRNG states, parent choices,
mutation operations, candidate bytes, identities, lineage and retention
decisions. The retained corpus contains 21 seeds and 65 mutations, totaling
10,478 bytes. Independent report/export from the saved cumulative indexed
profile reproduce the final counters with empty diagnostics. All 60 saved
input hashes match committed blobs; source and binary identities match before
and after. Audit scripts, hashes and results are in the campaign evidence
directory's `independent-audit/final-audit.txt`.

This profile is separate from fuzz-smoke coverage. A finite campaign checks
feedback and retention mechanics and does not establish sustained security.
The campaign retains a cumulative indexed profile and bounded recent raw
profiles; it does not retain a 500-profile set for independent remerging.
Artifacts are `/private/tmp/yaxml-construction-fuzz-campaign-d684886-500` and
`/private/tmp/yaxml-construction-fuzz-campaign-evidence-d684886`. The exact
script is `/private/tmp/yaxml-construction-fuzz-campaign-complete.sh`.

The earlier checkpoint `d5ea7d1` has a separate retained
[report](validation/fuzz-coverage-construction-d5ea7d1.txt) and raw evidence
directory. Adding the directed local-prefix conflict case closes one further
branch outcome in the final fuzz-smoke run. Profiles from these runs are never
combined. Different retained corpora produce different bounded mutation
sequences; the 500-case campaign counts are not a controlled coverage A/B.

## Remaining work

The 100% fuzz-execution target remains unmet. Allocation-return paths,
checked-arithmetic and identity failures, defensive consistency checks and
helper combinations that public callers do not currently select remain in
the denominator. Each needs a documented reachability assessment, a suitable
independent target, or a justified simplification; none is silently excluded.

The exact remaining construction branch locations below use the unchanged
production file's line and column. These are assessments of present guards
and call sequences, not general proofs that a branch is unreachable.

| Location | Current guard or call sequence |
| --- | --- |
| `986:16` | Empty exact prefixes fail XML-name validation before semantic resolution. The `xmlns` alternative is covered. |
| `1014:35`, `1082:31` | The fallback requires an explicit default declaration after lookup of that same local binding returned none. |
| `1195:79` | Earlier resolution rejects a lexical prefix on a namespace-less ordinary attribute. |
| `566:12` | Automatic generated-prefix availability is checked before binding publication; no public conflicting path was found. |
| `856:8` | Scratch growth exceeds the bound derived from supplied bytes and namespace work. |
| `449:15` | A malformed internal XMLNS name has neither permitted lexical declaration form. |
| `731:40`, `746:40`, `780:36` | Every current matching-prefix caller passes `include_default = false`. |
| `735:17`, `750:17` | The preceding default-prefix guard skips these candidates. |
| `751:20` | Generated-prefix allocation reserves later exact intents; no valid incompatible generated candidate was found. |
| `783:20` | Lookup of a known ancestor declaration should find the same prefix or a nearer shadowing binding. |
| `645:8` | The exact element's binding is resolved before automatic attributes; no incompatible effective candidate was found. |
| `1376:8` | Incremental explicit/generated item checks precede the final quota recount. |
| `476:12` | Current binding-publication paths check generated bindings separately before general scope lookup. |

The directed final case closes `736:20`, the local binding whose URI matches
an automatic element but conflicts with a later exact attribute intent. Its
literal final error and recovery check failure behavior; they do not establish
sensitivity to every alternate prefix choice inside a rejected operation.
Additional resource-precedence cases can make those internal choices observable.

Sustained campaigns, broader compatibility and matched libxml2 performance
comparisons remain open. No new production or performance claim follows from
this harness-only milestone.
