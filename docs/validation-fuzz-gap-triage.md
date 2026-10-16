# Fuzz-only coverage gap triage

This review examined the fuzz-only LLVM coverage produced from `51944f1` in
`/private/tmp/yaxml-text-chunks-coverage-51944f1` and the independent
reproduction in `/private/tmp/yaxml-text-chunks-coverage-review-fe2e8bd`.
Both reports have the same per-file totals and the same 4,855/5,082 regions,
315/340 functions, 3,269/3,346 lines and 657/692 instrumented branch outcomes.
Production source hashes are identical to the `9e4b3a6` base used here.

## Method and triage

The raw `llvm-cov export` was grouped by source file, start/end coordinate and
region kind. Counts from mangled generic instantiations at the same source span
were combined; a span was called missed only when every instantiation had count
zero. This turns the report's 227 missed regions into 207 distinct zero-count
source spans. It does not change the official denominator. The 25 missed
functions collapse exactly as follows:

- 21 are allocation, checked identifier-exhaustion or equivalent failure
  closures: `dom.rs` 57, 105 and 130; `escape.rs` 27 and 92; `owned.rs` 129,
  327, 374, 970, 1026 and 1054; `parser.rs` 47, 371, 380, 393, 408, 426 and
  484; `serialize.rs` 101; `tokenizer.rs` 215; and `writer.rs` 424.
- `dom.rs:121` and `owned.rs:1008` reject an end event without a corresponding
  open element. The public readers establish that event-order invariant before
  either builder sees the event.
- `stream.rs:76` constructs `InvalidState` if the private lending frontend asks
  for an event without a current token. Its callers establish the token first.
- `owned.rs:1354` is the private `LinkedDescendants::count`. Public
  `Descendants::count` at line 1307 overrides dispatch with `self.len()`, so no
  safe public call can enter the inner method. Commit `0ef8e77` independently
  checks the public count and post-`nth` length, but does not execute this
  private body.

The source-span triage is below. A line group names every zero-count location
from that file without repeating LLVM subregions on the same expression.

| File | Baseline missed regions | Source line groups | Reachability classification |
| --- | ---: | --- | --- |
| `buffered.rs` | 3 | 152-154, 179-181 | Candidate public decoder-carry/lookahead and zero-consume schedules. This pass did not establish an exact reaching sequence; a separate pull-schedule model should resolve them. |
| `dom.rs` | 24 | 57, 105, 121, 130-162 | Allocation failures and builder invariants already established by `Reader`; the final missing-root guard is redundant after successful reader EOF. |
| `encoding.rs` | 8 | 153, 160, 231, 252, 265, 362-363 | Selection/error propagation plus private decoder-consumer progress validation. Invalid progress requires a faulty internal consumer; allocator failure is not injected by this harness. |
| `escape.rs` | 8 | 27, 92 | `try_reserve` allocation failures only. |
| `owned.rs` | 55 | 129, 301-392, 451, 492, 670-702, 970-1107, 1143, 1335-1356 | Allocation/identifier exhaustion, bounded arithmetic overflow, parser-established builder invariants, unforgeable safe-handle slot failure, removed-node invariant and private iterator terminal bodies. |
| `parser.rs` | 59 | 47, 169, 219-266, 290, 330, 371-426, 479-520, 547, 755, 787 | Mostly allocation and tokenizer-protected delimiter/kind checks. Line 755, BOM-less UTF-16 declaration rejection, is publicly reachable and was selected. Lines 227-231 cannot receive a non-instruction as the first BOM-less token because encoding detection itself requires the `<?` signature. |
| `push.rs` | 2 | 176-177 | Publicly reachable cancellation while `finish` emits pending text; selected. |
| `reader.rs` | 2 | 193, 253-256 | The pre-root whitespace quota failure at line 193 is publicly reachable and was selected. The `None` event branch is excluded by the slice reader's own whitespace skip and tokenizer classification. |
| `serialize.rs` | 15 | 67, 70-77, 101, 168-192 | Allocation plus live-node/kind and traversal invariants. The `Option` failures require an invalid internal node record; terminal traversal exits through the earlier root-parent returns. |
| `stream.rs` | 6 | 76, 124-128 | Private no-current-token invariant in the lending and callback frontends. |
| `tokenizer.rs` | 10 | 94-98, 126, 163-165, 215, 304-305 | Allocation and private ready-token/tag-state invariants, with carry/lookahead propagation left for the separate pull-schedule work above. |
| `writer.rs` | 15 | 262, 283-318, 359-360, 389, 424 | Candidate public token/output-limit sequences at individual suffix writes, allocator failure, and defensive event-coherence arms. Existing fuzzing covers every event and both quota kinds, but this pass did not establish which missing suffix subregions are independently reachable. |

This classification is about safe public reachability under current invariants.
It does not claim allocator failure is impossible, that uncovered defensive
checks are unnecessary, or that source coverage proves semantic completeness.

## Focused additions

Commit `1598673` adds three bounded checks without changing production code:

- an input-derived, valid ASCII text token is left pending until `finish`; the
  callback cancels it and the model checks its value, range, exact `Aborted`
  offset, parser position and sticky terminal behavior;
- BOM-less little- and big-endian UTF-16 declarations without an `encoding`
  pseudo-attribute are checked against an independent exact
  `EncodingMismatch` at source byte zero through push, buffered-pull and owned
  parsing;
- pre-root whitespace exceeding `max_token_bytes` is checked for both the
  exact limit kind and its original byte-zero offset.

Happy-path and negative oracle tests cover the exact cancellation and encoding
expectations. Deliberate faults were run separately and restored:

| Fault | Test that failed | Retained log |
| --- | --- | --- |
| Report finish cancellation at the event start instead of end | `known_acceptance` | `/private/tmp/yaxml-fuzz-gap-mutant-finish-abort.txt` |
| Remove the slice reader's pre-root whitespace quota check | `each_resource_budget` (observed offset 4 instead of 0) | `/private/tmp/yaxml-fuzz-gap-mutant-whitespace-limit.txt` |
| Accept a BOM-less UTF-16 declaration with no encoding | `bomless_utf16_requires_an_encoding_declaration` | `/private/tmp/yaxml-fuzz-gap-mutant-required-declaration.txt` |
| Ignore the cancellation oracle's expected offset | negative cancellation oracle test | `/private/tmp/yaxml-fuzz-gap-mutant-cancellation-oracle.txt` |
| Ignore the declaration oracle's exact result | negative declaration oracle test | `/private/tmp/yaxml-fuzz-gap-mutant-declaration-oracle.txt` |

## Fresh measurement

A fresh fuzz-only build at `1598673` used nightly `2026-06-06`,
`-C instrument-coverage -Z coverage-options=branch`, and matching LLVM 22.1.6
tools. It ran only the 37-test fuzz-smoke binary. Its target, raw profile and
indexed profile are separate from every earlier measurement; no profiles were
combined. All 14 production sources were passed to `llvm-cov`; `lib.rs` has no
executable mapping. Merge, report and export diagnostics were empty.

| Metric | Baseline | Focused harness | Change |
| --- | ---: | ---: | ---: |
| Regions | 4,855 / 5,082 (95.53%) | 4,859 / 5,082 (95.61%) | +4 reached |
| Functions | 315 / 340 (92.65%) | 315 / 340 (92.65%) | unchanged |
| Lines | 3,269 / 3,346 (97.70%) | 3,270 / 3,346 (97.73%) | +1 reached |
| Instrumented branch outcomes | 657 / 692 (94.94%) | 658 / 692 (95.09%) | +1 reached |

At source-span granularity, zero-count spans fell from 207 to 202 and unique
branch sites with a missed outcome fell from 27 to 25. The gains are the two
regions for required-declaration rejection, the finish-cancellation region,
the pre-root quota propagation region and their associated branch outcomes;
LLVM's official region aggregation accounts for four additional reached
regions rather than the five distinct coordinates.

Raw profiles, exact source/harness SHA-256 manifests, compiler and tool
versions, coverage JSON, the per-file report, the collapsed triage and empty
diagnostics are in `/private/tmp/yaxml-fuzz-gap-coverage-1598673`. The report
SHA-256 is `f3b8918338e2c5e3b2cac93b21f836f1682230822901bee827357a4ee159f042`.

The committed harness passed all 350 listed all-target tests, with 343 run and
7 ignored helpers, plus 10 documentation tests. Strict Clippy and formatting
checks also passed. `git diff --exit-code 9e4b3a6 -- src` confirms that the
production source is byte-identical to the reviewed base. The measurement
manifests refer to the exact code commit `1598673`; this documentation is a
later artifact-only commit.
